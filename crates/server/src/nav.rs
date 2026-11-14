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
use vcod_common::net::msg::{self, NULL_USERCMD, UserCmd};
use vcod_common::pmove::cmd::{ANGLE2SHORT, EventRing, chop, player_step};
use vcod_common::pmove::{MoveRules, PlayerState};

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
/// A walk whose fall, from the highest the body rose to where it lands,
/// is deeper than this is refused: past `bg_fallDamageMinHeight` (256 in
/// 1.1d's and UO's `gameCvarTable`) the fall hurts. mp_cassino's allied
/// upper floor leaves by a window sill 240 over the street
/// (bot-navigation.md, "Other rules").
const MAX_DROP: f32 = 250.0;
/// [`MAX_DROP`] for a walk set off at a body's own pace, the follower's
/// proof again from where a bot stands, read in the air from where the
/// body last stood: a bot moving onto a drop deeper than this is held to
/// take it from rest. mp_ship's ledge 211 under the
/// deck at (3647, -352.9, 611.1) lands a back down at the 190 a bot came
/// along the deck with, and a bot that did fell past it to the hold
/// (bot-navigation.md section 3).
const PACED_DROP: f32 = 200.0;
/// How long [`coasts_off`] lets a body coast: 190 units/s coasts to rest
/// in about 35 units and half a second.
const COAST_TICKS: usize = 16;
/// How high over a drop's landing [`a_miss_hurts`] looks out from.
const MISS_EYE: f32 = 48.0;
/// How far either side of the body a run off a drop, or a run on that
/// leaves the ground, whose miss can hurt has to land from too
/// ([`Follower::edge_holds`]).
const ASIDE_DROP: f32 = 2.0;
/// An edge whose ends differ by less than this in height is level, and a
/// walk one way stands in for the walk back.
const LEVEL: f32 = 1.0;
/// The flood stops here; a stock map fills well under it. United
/// Offensive's largest, mp_rhinevalley, fills 146 000 (bot-navigation.md,
/// "Other rules").
const MAX_NODES: usize = 200_000;
/// The longest a walk runs with a ladder in it: 15 s, ~1400 units of climb
/// at the 95 units/s a full-rate climb makes. mp_ship's hold ladder is 560.
const LADDER_TICKS: usize = 300;
/// Degrees up a climbing body looks; past ~9 deg `ladder_move`'s upscale
/// saturates. Shared with the bots, which climb the same way.
pub const LADDER_PITCH: f32 = 45.0;
/// A* gives up past this many expansions, or the graph's node count when
/// that is more, and reports no path.
const MAX_EXPANSIONS: usize = 40_000;

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

pub struct NavGraph {
    /// Feet origins where pmove came to rest.
    pub nodes: Vec<Vec3>,
    /// Directed: `edges[a]` holds every `b` a run from `a` reaches. A drop
    /// off a ledge has no edge back.
    pub edges: Vec<Vec<u32>>,
    /// The edges a walk proved only with a jump; a bot following one jumps
    /// where the walk did ([`PinJump`], [`leap_cue`]).
    pub jumps: HashSet<(u32, u32)>,
    /// The jump edges proved from a standstill at their foot, jumping at the
    /// lip ([`NavGraph::link_leaps`]): a bot comes to rest at the foot first.
    pub leaps: HashSet<(u32, u32)>,
    /// The jump edges proved only with crouch held from the tick after the
    /// jump until the body lands ([`Gait::CrouchJump`]), through a window
    /// too low for a standing body ([`NavGraph::link_pockets`]).
    pub crouch_jumps: HashSet<(u32, u32)>,
    /// The jump edges whose walk lands only aimed beside their top
    /// ([`NavGraph::reprove_jumps`]), and the point it aimed at: a bot on
    /// one steers there ([`NavGraph::aim`]).
    pub aims: HashMap<(u32, u32), glam::Vec2>,
    /// Drops deeper than a jump whose walk from 4 units aside of their node
    /// falls, and walks down beside a fall that hurts whose forward walk
    /// does: a plan takes one only where there
    /// is no other way ([`RISK_COST`], [`NavGraph::mark_risky`]).
    pub risky: HashSet<(u32, u32)>,
    /// Nodes whose middle stands over a fall that kills, on a rail or a
    /// spar's edge: a roam never ends on one ([`NavGraph::mark_perches`]).
    pub perches: HashSet<u32>,
    /// The edges only a CoD2 mantle climbs ([`NavGraph::link_mantles`]): a
    /// bot on one runs at the wall and jumps once pmove offers the climb.
    /// Empty on a CoD1 graph.
    pub mantles: HashSet<(u32, u32)>,
    /// Each mantle edge's landing as its probe proved it, which may stand
    /// a cell off the node the edge ends at: a body run at the node need
    /// not be offered the climb ([`Follower::mantle_land`]).
    pub mantle_lands: HashMap<(u32, u32), Vec3>,
    /// The nodes the flood started from at the spawn points. A ladder's
    /// ends seed a flood of their own, which may never meet the spawns'
    /// floors: mp_shipment's grounds round the yard, out of bounds, are
    /// the larger part of its graph.
    seeds: Vec<u32>,
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
    /// The largest strongly connected component with the [`Self::risky`]
    /// edges left out that holds a spawn's seed ([`Self::seeds`]): a roam
    /// from inside it never ends where only a risky edge leads
    /// ([`Self::roam_from`]).
    safe: Vec<bool>,
    /// The world's ladders ([`ladders`]), for the build.
    ladder_boxes: Vec<(Vec3, Vec3)>,
    /// The movement rules the walks proved the edges under.
    pub rules: MoveRules,
    /// The game's own step they moved by instead, if any ([`with_step`]).
    pub step: Option<Arc<dyn WalkStep>>,
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
    #[allow(clippy::too_many_arguments)]
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
            jumps: HashSet::new(),
            leaps: HashSet::new(),
            crouch_jumps: HashSet::new(),
            aims: HashMap::new(),
            risky: HashSet::new(),
            perches: HashSet::new(),
            mantles: HashSet::new(),
            mantle_lands: HashMap::new(),
            seeds: Vec::new(),
            spacing: spacing.clamp(SPACING_MIN, SPACING_MAX),
            columns: HashMap::new(),
            comp: Vec::new(),
            members: Vec::new(),
            reach: Vec::new(),
            safe: Vec::new(),
            ladder_boxes: ladders(world),
            rules: RULES.get(),
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
            g.leaps.clear();
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

    /// Marks [`Self::risky`]: every walk down off a jump edge beside a fall
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
            .edges
            .iter()
            .enumerate()
            .flat_map(|(a, out)| out.iter().map(move |&b| (a as u32, b)))
            .filter(|&(a, b)| {
                let (p, q) = (self.nodes[a as usize], self.nodes[b as usize]);
                q.z < p.z - LEVEL
                    && !self.jumps.contains(&(a, b))
                    && !self.mantles.contains(&(a, b))
            })
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
            if q.z < p.z - DEADLY_LADDER
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
        self.risky = drops
            .into_iter()
            .zip(risky)
            .filter_map(|(e, r)| r.then_some(e))
            .collect();
    }

    /// Links every node from `from` on to where a mantle off it lands, by
    /// `probe` facing each of the eight compass ways, as a mantle edge
    /// ([`Self::mantles`]) unless a walk already proved the edge; a landing
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
                if a != b && !self.edges[a as usize].contains(&b) {
                    self.edges[a as usize].push(b);
                    self.mantles.insert((a, b));
                    self.mantle_lands.insert((a, b), q);
                }
            }
        }
        new
    }

    /// Marks [`Self::perches`]: nodes with no floor under their middle
    /// within a fall that kills ([`DEADLY_LADDER`]). mp_ship's crow's nest
    /// rails stand a body 2 units from the edge: bots whose roam ended on
    /// one at a run, the next plan a few ticks off, ran on off the rail to
    /// the deck (bot-navigation.md section 3).
    fn mark_perches(&mut self, world: &CollisionWorld) {
        let over = par_map(&self.nodes, |&p| {
            let down = p - Vec3::Z * DEADLY_LADDER;
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

    /// Runs `f` with the walks' bodies moving as this graph's did.
    pub fn moving<R>(&self, f: impl FnOnce() -> R) -> R {
        with_rules(self.rules, || with_step(self.step.clone(), f))
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

    fn index_components(&mut self) {
        self.comp = self.components();
        let safe = self.components_by(|a, b| !self.risky.contains(&(a, b)));
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

    /// The nodes that reach no node of the largest strongly connected
    /// component, in index order.
    fn cut_off(&self) -> Vec<u32> {
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
            for &b in out {
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
                        if !pocket.contains(&m) && !self.edges[n as usize].contains(&m) {
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
    fn link_leaps(&mut self, world: &CollisionWorld) {
        use vcod_common::pmove::{JUMP_HEIGHT, STEPSIZE};
        let mut heads: HashSet<u32> = HashSet::new();
        for (a, out) in self.edges.iter().enumerate() {
            let z = self.nodes[a].z + JUMP_HEIGHT;
            heads.extend(out.iter().filter(|&&b| self.nodes[b as usize].z > z));
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
            if ok && !self.edges[a as usize].contains(&b) {
                self.link_as(a, b, true);
                self.leaps.insert((a, b));
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
    /// aimed, keeps that aim ([`Self::aims`]); one that lands at none is
    /// dropped.
    fn reprove_jumps(&mut self, world: &CollisionWorld) {
        let mut jobs: Vec<(u32, u32)> = self
            .jumps
            .iter()
            .copied()
            .filter(|e| !self.leaps.contains(e))
            .collect();
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
                    self.aims.insert((a, b), p);
                }
                Some(None) => {}
                None => {
                    self.edges[a as usize].retain(|&m| m != b);
                    self.jumps.remove(&(a, b));
                }
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
            out.retain(|&b| {
                let e = (a as u32, b);
                let keep = self.mantles.contains(&e)
                    || (self.nodes[b as usize] - p).truncate().length() <= reach;
                if !keep {
                    self.jumps.remove(&e);
                    self.crouch_jumps.remove(&e);
                    self.aims.remove(&e);
                }
                keep
            });
            self.edges[a] = out;
        }
    }

    /// Where a bot on edge `a -> b` steers: `b`'s spot, or the point
    /// beside it the edge's jump was proved at ([`Self::aims`]).
    pub fn aim(&self, a: u32, b: u32) -> glam::Vec2 {
        self.aims
            .get(&(a, b))
            .copied()
            .unwrap_or_else(|| self.nodes[b as usize].truncate())
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
            self.crouch_jumps.remove(&(a, b));
        }
    }

    /// [`Self::link_as`], a crouched jump edge when `crouched`.
    fn link_crouched(&mut self, a: u32, b: u32, jumped: bool, crouched: bool) {
        self.link_as(a, b, jumped);
        if crouched && self.jumps.contains(&(a, b)) {
            self.crouch_jumps.insert((a, b));
        }
    }

    fn add(&mut self, c: (i32, i32), p: Vec3) -> u32 {
        let n = self.nodes.len() as u32;
        self.nodes.push(p);
        self.edges.push(Vec::new());
        self.columns.entry(c).or_default().push(n);
        n
    }

    /// Column `c`'s node nearest height `z`, within [`Z_MERGE`].
    fn find(&self, c: (i32, i32), z: f32) -> Option<u32> {
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
    /// (sorted) and passes `ok`.
    pub fn clear_near(&self, n: u32, blocked: &[u32], ok: impl Fn(u32) -> bool) -> Option<u32> {
        let at = self.nodes[n as usize];
        let (cx, cy) = self.column(at);
        let mut best: Option<(f32, u32)> = None;
        for dy in -CLEAR_STEPS..=CLEAR_STEPS {
            for dx in -CLEAR_STEPS..=CLEAR_STEPS {
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
                && !self.jumps.contains(&(a, b))
                && !self.risky.contains(&(a, b))
        };
        let mut seen = vec![start];
        let mut ring = vec![start];
        for _ in 0..2 {
            let mut next = Vec::new();
            for &a in &ring {
                for &b in &self.edges[a as usize] {
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
            for &b in &self.edges[n as usize] {
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
        self.walkable_start(world, p).map(|(n, _)| n)
    }

    /// [`Self::nearest_walkable`], and whether the walk there takes a jump.
    /// Where no walk arrives without one, the first of the nearest few a
    /// bot's walk ([`as_a_bot`]) arrives at with its jump: a dent off the
    /// graph walled higher than a step all round has no other way out, and
    /// the bot runs that first leg as a jump edge ([`Follower::jumping`]).
    /// mp_cassino's roof dent at (4948, -1413, 280), 23 under the roof
    /// round it, held a bot pressing at the node at 280 beyond its wall for
    /// 12 s (bot-objectives.md 10.6).
    pub fn walkable_start(&self, world: &CollisionWorld, p: Vec3) -> Option<(u32, bool)> {
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
        // Up out of it, from rest: a body that found no walk out stands.
        let jumps = |n: u32| {
            let to = self.nodes[n as usize];
            to.z - p.z > vcod_common::pmove::STEPSIZE
                && matches!(
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

    /// A* from `from` to `to` over the directed edges, Euclidean cost and
    /// heuristic. The node list runs from `from` to `to` inclusive; `None`
    /// when `to` is unreachable or the search runs past its budget.
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
        self.components_by(|_, _| true)
    }

    /// [`Self::components`] as indexed when the graph was finished, which
    /// nothing changes after: a lookup, where a fresh walk of a stock UO
    /// graph took 9 to 26 ms (map-cycle doc 4.9).
    pub fn component_ids(&self) -> &[u32] {
        &self.comp
    }

    /// [`Self::components`] over only the edges `keep` passes.
    fn components_by(&self, keep: impl Fn(u32, u32) -> bool) -> Vec<u32> {
        let n = self.nodes.len();
        let edges: Vec<Vec<u32>> = self
            .edges
            .iter()
            .enumerate()
            .map(|(a, out)| out.iter().copied().filter(|&b| keep(a as u32, b)).collect())
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
            edges,
            jumps: HashSet::new(),
            leaps: HashSet::new(),
            crouch_jumps: HashSet::new(),
            aims: HashMap::new(),
            risky: HashSet::new(),
            perches: HashSet::new(),
            mantles: HashSet::new(),
            mantle_lands: HashMap::new(),
            seeds: Vec::new(),
            spacing: SPACING_MIN,
            columns: HashMap::new(),
            comp: Vec::new(),
            members: Vec::new(),
            reach: Vec::new(),
            safe: Vec::new(),
            ladder_boxes: Vec::new(),
            rules: MoveRules::V1_1,
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

/// The graphs built so far this process, by map. A map cycle comes back
/// to a map without a second build, and the test gates that start a server
/// per test share one.
type CacheKey = (
    String,
    MoveRules,
    Option<String>,
    usize,
    usize,
    usize,
    usize,
    Vec<[u32; 3]>,
    Vec<[u32; 3]>,
    Vec<bool>,
    Vec<u32>,
);
static CACHE: std::sync::Mutex<Vec<(CacheKey, Arc<NavGraph>)>> = std::sync::Mutex::new(Vec::new());

/// The key carries the collision's counts and the spawn points too, so a
/// test world built without the static props is a different graph from the
/// real one, and which submodels clip: each gametype deletes the others'
/// brushes (`_gameobjects.gsc`), so one map's graph differs by gametype.
/// Where the movers, world copies and vehicles stand is in it as well: a
/// rebuild walks round them, and two servers on one map hold them apart,
/// so a graph keyed without them made a gate's result ride which test
/// built first.
fn cache_key(
    map: &str,
    c: &CollisionWorld,
    world: &crate::world::World,
    rules: MoveRules,
) -> CacheKey {
    (
        map.to_ascii_lowercase(),
        rules,
        own_step().map(|s| s.key()),
        c.brushes.len(),
        c.tris.len(),
        c.model_tris.len(),
        world.hazards.len(),
        world
            .spawn_points
            .iter()
            .map(|p| p.map(f32::to_bits))
            .collect(),
        world
            .nav_cover
            .iter()
            .map(|p| p.map(f32::to_bits))
            .collect(),
        (0..c.model_count()).map(|m| c.model_linked(m)).collect(),
        c.placements(),
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
    // A rebuild keys on where the vehicles stood, so it is seldom asked
    // for again; the oldest go once the cache holds CACHE_GRAPHS.
    if cache.len() >= CACHE_GRAPHS {
        cache.remove(0);
    }
    cache.push((key, g.clone()));
    g
}

/// The most graphs [`cached`] keeps.
const CACHE_GRAPHS: usize = 24;

/// Test hook: forgets every cached graph, so a gate can run a match as a
/// fresh process would after another match filled the cache.
#[doc(hidden)]
pub fn clear_cache() {
    CACHE.lock().unwrap_or_else(|e| e.into_inner()).clear();
}

/// `map`'s graph under the rules in force ([`with_rules`]), built on
/// first use on the calling thread.
pub fn graph_for(map: &str, world: &crate::world::World) -> Arc<NavGraph> {
    let rules = RULES.get();
    cached(cache_key(map, &world.collision, world, rules), || {
        NavGraph::build(
            &world.collision,
            &world.spawn_points,
            &world.nav_cover,
            &world.hazards,
            rules,
        )
    })
}

/// Where a mantle off a node lands: the body's feet at the node, facing the
/// yaw (degrees), and the feet where the climb leaves it standing, if the
/// wall ahead takes one. The CoD2 bots hand one in
/// (`crate::bots::cod2::mantle_probe`); CoD1 has no mantle.
pub type MantleProbe = Arc<dyn Fn(&CollisionWorld, Vec3, f32) -> Option<Vec3> + Send + Sync>;

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
    /// change later never reach the build. The walks move by `rules`.
    pub fn start(map: &str, world: &crate::world::World, rules: MoveRules) -> NavJob {
        Self::start_with(map, world, rules, None, None)
    }

    /// [`Self::start`] for a graph with mantle edges off `mantle`, its
    /// bodies moving by `step` ([`NavGraph::build_with`]).
    pub fn start_with(
        map: &str,
        world: &crate::world::World,
        rules: MoveRules,
        mantle: Option<MantleProbe>,
        step: Option<Arc<dyn WalkStep>>,
    ) -> NavJob {
        Self::start_opts(map, world, rules, mantle, step, true)
    }

    /// [`Self::start_with`] for bots that read a leap's lip or not
    /// ([`NavGraph::build_opts`]).
    pub fn start_opts(
        map: &str,
        world: &crate::world::World,
        rules: MoveRules,
        mantle: Option<MantleProbe>,
        step: Option<Arc<dyn WalkStep>>,
        lip: bool,
    ) -> NavJob {
        let collision = world.collision.clone();
        for &m in &world.rubble {
            collision.set_model_linked(m, false);
        }
        let mut key = with_step(step.clone(), || cache_key(map, &collision, world, rules));
        if mantle.is_some() {
            key.0.push_str(" (mantles)");
        }
        if !lip {
            key.0.push_str(" (no lip)");
        }
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
        let seeds = world.spawn_points.clone();
        let cover = world.nav_cover.clone();
        let hazards = world.hazards.clone();
        let build = move || {
            // One core stays the tick's, so a build doesn't jitter the
            // schedule it runs beside.
            SPARE_CORE.set(true);
            cached(key, || {
                NavGraph::build_opts(
                    &collision,
                    &seeds,
                    &cover,
                    &hazards,
                    rules,
                    mantle.as_ref(),
                    step,
                    lip,
                )
            })
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
/// A start that reaches fewer nodes than this round the parked vehicles'
/// is hemmed in by them ([`Follower::penned`]): mp_cassino's terrace at
/// (383, -916, 76) reaches 7 with a tank over its stair, of the 7 676 it
/// reaches without.
const PEN_NODES: usize = 64;
/// How many grid steps across [`NavGraph::clear_near`] looks: past a
/// parked tank's reach (`server::bot_vehicles::PARK_REACH`).
const CLEAR_STEPS: i32 = 6;
/// A waypoint further than this many grid steps away means the bot has left
/// its path (knocked back, fell off a ledge): plan again.
const OFF_PATH_STEPS: f32 = 4.0;
/// Ticks without closing on the waypoint before the follower gives up on
/// it, and the ticks it then leaves the bot to its own unstick.
const STUCK_TICKS: u32 = 40;
const REST_TICKS: u32 = 20;
/// How far, flat, the last waypoint may be for a body to wait on it while a
/// plan is searched for ([`Follower::rest_point`]), and for how many ticks.
const WAIT_REACH: f32 = 48.0;
const WAIT_TICKS: u32 = 40;
/// The most of the tick's plan budget one follower's search takes, so a
/// long search leaves some for the bots after it.
pub(crate) const PLAN_SLICE: u32 = 2000;
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
/// What a [`NavGraph::risky`] drop adds to a plan's cost: a long way round
/// is taken first.
const RISK_COST: f32 = 4000.0;
/// What a teammate's path costs another plan over its length, on top of
/// the length: a way round up to this much longer is taken instead
/// (bot-objectives.md section 9).
pub const CROWD_COST: f32 = 1.0;
/// [`CROWD_COST`] for an S&D flanker on the main group's paths: a way
/// round up to this much longer again is taken (bot-objectives.md 9.1).
pub const FLANK_COST: f32 = 4.0;
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
/// How far aside of a drop's node [`NavGraph::mark_risky`] walks it again.
const RISK_ASIDE: f32 = 4.0;
/// How many of the nearest nodes [`NavGraph::nearest_walkable`] walks to.
const WALKABLE_TRIES: usize = 8;
/// A body on the ground this far under its waypoint, off a ladder, has
/// fallen off the path.
const FELL_BELOW: f32 = 48.0;
/// Within this of a waypoint flat, a walk on that fails from where the bot
/// stands fails from the node ([`Follower::edge_holds`]).
const ON_NODE: f32 = 12.0;
/// A roam goal is picked at least this far away when it can be.
const ROAM_MIN: f32 = 1000.0;
/// A bot this near a ladder, flat, waits aside for a climber on it
/// ([`Follower::ladder_yield`]); a player this near is on its rungs, and
/// this near the bot's end in height about to step off there; the spot
/// the bot waits at is at least this far from the ladder.
const YIELD_NEAR: f32 = 64.0;
const CLIMBER_NEAR: f32 = 32.0;
const CLIMBER_CLOSE: f32 = 64.0;
const YIELD_CLEAR: f32 = 32.0;
/// Two climbers this near in height share the rungs' top abreast
/// ([`Follower::abreast_yield`]): a standing body's height.
const ABREAST_HEIGHT: f32 = 70.0;
/// A climber coming up this far under a body backing down the same rungs
/// sends it back up ([`Follower::head_on_yield`]): two bodies' height.
const HEAD_ON: f32 = 2.0 * ABREAST_HEIGHT;
/// Only on a ladder taller than a fall that kills (`bg_fallDamageMaxHeight`;
/// a hurt bot's is shorter, [`fall_that_kills`]): off a shorter one a whole
/// body slides off a climber's head and lives, and
/// turning back held two bots at mp_ship's hold ladder (3369, -379) for
/// 12 s (bot-navigation.md section 3).
const DEADLY_LADDER: f32 = 480.0;

/// The lowest fall a body with `health` of `max_health` does not live
/// through: `PM_CrashLand`'s percent, linear from `h.min` to 100 at
/// `h.max`, of `max_health`, 110 percent past 99 (`pmove::fall_damage`,
/// `Server`'s `fall_damage`, player-clip doc 8.3 and 8.8). A drop no
/// higher lands whatever the health.
pub fn fall_that_kills(health: i32, max_health: i32, h: vcod_common::pmove::FallHeights) -> f32 {
    if h.max <= h.min || h.min < 0.0 || max_health <= 0 {
        return f32::INFINITY;
    }
    // The smallest percent whose share of max health reaches the health.
    let percent = (0..=99)
        .find(|&p| (f64::from(max_health) * f64::from(p) * f64::from(0.01f32)) as i32 >= health)
        .unwrap_or(100);
    // The percent truncates up from the height, so the height that first
    // gives it; past the max the whole 110 lands.
    h.min + (h.max - h.min) * percent as f32 / 100.0
}
/// A body on the ground holds still rather than step within this of a
/// climber rising under it, flat ([`Follower::rising_hold`]): two
/// half-widths are where `StuckInClient` pushes the two apart.
const RISING_REACH: f32 = 2.0 * vcod_common::pmove::HALF_WIDTH + 6.0;
/// How far under the feet the climber's top may be: a climb at the bots'
/// 95 units/s rises this in half a second.
const RISING_BELOW: f32 = 48.0;
/// The longest such hold.
const RISING_TICKS: u32 = 60;
/// Two bodies on the ground pressed together this long, both slow and
/// near their waypoints, and one of them gives way for `GIVE_TICKS`,
/// stepping `GIVE_STEP` back from the other ([`Follower::pressed_yield`]).
const PRESSED_TICKS: u32 = 10;
const GIVE_TICKS: u32 = 20;
const GIVE_STEP: f32 = 24.0;
/// A body on the ground slower than this across, units/s, with another
/// within [`BODY_REACH`] across and [`BODY_RISE`] up or down toward its
/// next node, is held up by it; after [`BODY_TICKS`] its plans keep off
/// the nodes the other stands over for [`SHUN_TICKS`]
/// ([`Follower::bodies_in_way`]).
const BODY_HELD_SPEED: f32 = 60.0;
const BODY_REACH: f32 = 48.0;
const BODY_RISE: f32 = 64.0;
const BODY_TICKS: u32 = 4;
const SHUN_TICKS: u32 = 60;
/// The longest a bot stands aside for an oncoming body, the ticks before
/// it does again, and how far it backs off when no node lies aside
/// ([`Follower::stand_aside`]).
const ASIDE_TICKS: u32 = 60;
const ASIDE_REST: u32 = 60;
const ASIDE_BACK: f32 = 64.0;
/// The longest wait: a climb of mp_ship's 296-unit ladders takes about
/// 110 ticks. With the climber still, the most of it: a climber that hangs
/// on with the way clear is stuck for another reason (mp_ship's ladder at
/// x 3808). A wait that runs out is not taken again for `YIELD_REST`.
const YIELD_TICKS: u32 = 200;
const YIELD_HUNG: u32 = 40;
const YIELD_REST: u32 = 200;
/// How far off the line along a [`Follower::straight_run`] a node may
/// stand, and how many of the path's nodes it looks along.
const SPRINT_SLACK: f32 = 16.0;
const SPRINT_LOOK: usize = 40;
/// A seen enemy has to move this far off the planned destination before the
/// path is planned again.
const RETARGET: f32 = 128.0;
/// [`Follower::pull`]: how far, in grid steps, a pulled waypoint may lie
/// flat (under [`OFF_PATH_STEPS`], past which the follower replans), how
/// far a skipped node may stand off the line to it, the floor probe's
/// spacing along that line ([`straight_floor`]), and the ticks between
/// tries after one fails.
const PULL_STEPS: f32 = 3.0;
const PULL_SLACK: f32 = 0.75;
const PULL_PROBE: f32 = 16.0;
const PULL_RETRY: u32 = 4;

/// What the body a [`Follower`] steers stands on this tick.
#[derive(Clone, Copy)]
pub struct Footing {
    /// At rest with a jump ready ([`ready_to_leap`]).
    pub still: bool,
    pub on_ground: bool,
    pub on_ladder: bool,
    /// `ps.velocity`: a drop is re-proved at the pace the body comes onto it.
    pub velocity: Vec3,
}

impl Footing {
    #[cfg(test)]
    const STANDING: Footing = Footing {
        still: true,
        on_ground: true,
        on_ladder: false,
        velocity: Vec3::ZERO,
    };
}

/// How far `p` stands across from the segment `a` to `b`, all flat.
fn seg_dist_flat(a: Vec3, b: Vec3, p: Vec3) -> f32 {
    let (a, b, p) = (a.truncate(), b.truncate(), p.truncate());
    let ab = b - a;
    let t = ((p - a).dot(ab) / ab.length_squared().max(1e-6)).clamp(0.0, 1.0);
    p.distance(a + ab * t)
}

/// One bot's walk along the graph: the destination, the planned path and
/// the bookkeeping that notices when it has gone wrong. The server keeps one
/// per bot and asks it for the waypoint every tick.
#[derive(Clone, Default)]
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
    /// The last waypoint handed out, and the ticks since one was: where
    /// the body waits while a plan is searched for ([`Self::rest_point`]).
    last: Option<Vec3>,
    waited: u32,
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
    /// Edges it got stuck on again while still avoided, planned through
    /// for want of a way round: left out of that plan too, until the tick
    /// given ([`Self::avoid_edge`]).
    dead: Vec<(u32, u32, u32)>,
    /// The plan under way, and whether it leaves out `avoid` (a plain one
    /// follows if it fails).
    search: Option<Search>,
    /// Edges teammates' paths run along, each `(low, high)` and sorted: a
    /// plan pays [`CROWD_COST`] more on them (`bots::team::crowd`). Set by
    /// the server each tick; read when a plan starts.
    pub crowd: Vec<(u32, u32)>,
    /// What [`Self::crowd`] costs: [`CROWD_COST`], or [`FLANK_COST`] for
    /// an S&D flanker. Set with it.
    pub crowd_cost: f32,
    /// The bot's route noise seed ([`Search::noisy`]), 0 for none. Set
    /// with [`Self::crowd`].
    pub noise: u64,
    /// Nodes a parked vehicle stands over, sorted
    /// ([`NavGraph::blocked_near`]): a plan runs through none but its own
    /// ends, and a path with one ahead is planned again. Set by the
    /// server each tick.
    pub blocked: Arc<[u32]>,
    /// The path runs through [`Self::blocked`]: no plan went round.
    through: bool,
    /// The last plan found the start hemmed in by [`Self::blocked`]
    /// ([`PEN_NODES`]).
    penned: bool,
    round: bool,
    /// The threat an [`Goal::Away`](crate::bots::Goal::Away) destination
    /// was picked against.
    threat: Option<Vec3>,
    /// The body fell off its path: the next plan starts at a node it walks
    /// to ([`NavGraph::nearest_walkable`]).
    fell: bool,
    /// The body was on a ladder when the plan began: it starts at one of
    /// the ladder's ends ([`NavGraph::nearest_ladder_end`]).
    on_rungs: bool,
    /// The plan under way starts at a node a walk from the body reached
    /// or at a ladder's end, not merely the nearest.
    proved_start: bool,
    /// That walk from the body arrived only with a jump
    /// ([`NavGraph::walkable_start`]): the way to the first node is run as
    /// a jump edge.
    jump_start: bool,
    /// The last jump edge waited on, and whether its jump lands from rest
    /// ([`Self::rest_jump_fails`]).
    rest_jump: Option<((u32, u32), bool)>,
    /// The last drop asked about, and whether the graph's walk ran off it
    /// ([`Self::runs_off`]).
    run_off: Option<((u32, u32), bool)>,
    /// The last drop asked about, and whether a miss of it hurts
    /// ([`Self::miss_hurts`]).
    miss_hurt: Option<((u32, u32), bool)>,
    /// A jump edge whose walk on from where the body passed its first node
    /// arrived without the jump ([`Self::edge_holds`]): the bot runs it
    /// as that walk did ([`Self::jump_cued`]).
    walked_jump: Option<(u32, u32)>,
    /// The last edge asked about, and whether only a creep proved it
    /// ([`Self::creeps_off`]).
    creep_off: Option<((u32, u32), bool)>,
    /// The creep whose top the body comes to rest on before it heads on
    /// ([`Self::creep_top`]).
    stop_at: Option<(u32, u32)>,
    /// The edge whose first node the body comes to rest on and heads on
    /// from only at rest: its pace flung it off the way on
    /// ([`Self::flings`]).
    held: Option<(u32, u32)>,
    /// The scale the body runs at, which [`Self::flings`]' walk runs at:
    /// the held weapon's `moveSpeedScale`, cut to the graph's 1 for a
    /// light weapon on the ground (`paced_key`), so only a heavy one's
    /// shows. A body can still pass 190 units/s off a slope or a fall.
    pub move_scale: f32,
    /// Ticks spent waiting aside for a climber ([`Self::ladder_yield`]),
    /// those of them with the climber still, and the ticks before it waits
    /// again once a wait ran out.
    yield_ticks: u32,
    yield_hung: u32,
    yield_rest: u32,
    /// Ticks held still for a climber rising under the body
    /// ([`Self::rising_hold`]).
    rising_ticks: u32,
    /// Ticks the body has been held up by a teammate standing still at
    /// its side, and the nodes it plans round for the ticks left after
    /// (`bots::driver`'s `mates_in_way`).
    pub mate_ticks: u32,
    pub mate_block: (u32, Arc<[u32]>),
    /// Ticks pressed on another body, and the spot and ticks of a give way
    /// ([`Self::pressed_yield`]).
    pressed_ticks: u32,
    give: Option<(Vec3, u32)>,
    /// The lowest fall that kills this body now ([`fall_that_kills`]):
    /// a hurt bot dies off a shorter ladder. `None` reads [`DEADLY_LADDER`].
    pub fall_kills: Option<f32>,
    /// A mantle edge whose landing lies past [`OFF_PATH_STEPS`] is still
    /// followed: the probe's climb ends where the body comes down, which
    /// on CoD4's mp_crash rubble at (291, -1439, 84) is 162 across
    /// (bot-objectives.md 12.4). Set by the driver per game.
    pub long_mantles: bool,
    /// A waypoint up to [`REACH`] past [`OFF_PATH_STEPS`] is still on the
    /// path: a body passes an edge's foot from up to [`REACH`] short of
    /// it, so an edge nearly [`OFF_PATH_STEPS`] long read as off the path
    /// there. Set by the driver per game.
    pub foot_slack: bool,
    /// A mantle edge down more than a jump is taken from rest at its foot
    /// without the creep's re-proof ([`Self::backs_short`]).
    pub mantle_rests: bool,
    /// What a jump edge adds to a plan's length ([`Search::jumps_costing`]).
    /// Set by the driver per game.
    pub jump_cost: f32,
    /// Off the ground and off the rungs last tick: a landing is read
    /// against the edge under way.
    airborne: bool,
    /// The point goal's plan the last goal move left, for a goal that
    /// comes back.
    parked: Option<Parked>,
    /// The waypoint before [`Self::pull`] took it further along the plan,
    /// while the pulled one is still ahead: a re-check that fails falls
    /// back toward it.
    pull_base: Option<usize>,
    /// Ticks before the next pull is tried, after one failed.
    pull_wait: u32,
    /// Edges given up on, by the stall count or the bot's own unstick
    /// ([`Self::stuck`]): what `bots::driver` tallies as edge stalls.
    pub gave_up: u32,
    /// Ticks held up by another body in the way ([`Self::bodies_in_way`]),
    /// the nodes the plans keep off for it, sorted, until the tick given,
    /// and whether the plan found no way round them.
    body_ticks: u32,
    shun: Arc<[u32]>,
    shun_until: u32,
    shun_through: bool,
    /// Where the bodies [`Self::shun`] keeps off stood.
    shunned: Vec<Vec3>,
    /// A give way to an oncoming body ([`Self::stand_aside`]): the spot, the
    /// way the other came from, the ticks left; and the ticks before the
    /// next.
    aside: Option<(Vec3, Vec3, u32)>,
    aside_rest: u32,
}

/// A [`Follower`] plan set aside when its point goal moved off, and the
/// tick it was.
#[derive(Clone)]
struct Parked {
    dest: u32,
    path: Vec<u32>,
    next: usize,
    best: f32,
    idle: u32,
    at: u32,
}

/// How long a [`Parked`] plan waits for its goal to come back: a second.
const PARK_TICKS: u32 = 20;

impl Follower {
    /// The waypoint toward `goal` for a bot standing at `at` on `body`'s
    /// footing. `world`
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
        body: Footing,
        budget: &mut u32,
        rand: &mut dyn FnMut() -> i32,
    ) -> Option<[f32; 3]> {
        let w = self.follow(g, world, goal, at, body, budget, rand);
        if let Some(w) = w {
            self.last = Some(w.into());
            self.waited = 0;
        } else {
            self.waited += 1;
        }
        w
    }

    /// Where a body on the ground with no waypoint this tick comes to rest
    /// while the next plan is searched for: the last waypoint, when it
    /// stands within [`WAIT_REACH`] on the body's floor, for up to
    /// [`WAIT_TICKS`]. Wandering on at a run, bots left the crow's nest's
    /// top for its hatch and mp_ship's shelf at (5403, -160, 396) for a
    /// slope they never came off (bot-navigation.md section 3). Not while
    /// the follower rests on purpose: a body stuck before its first node
    /// is left to its own unstick. Round a body in the way, where it
    /// stands.
    pub fn rest_point(&self, at: [f32; 3], body: Footing) -> Option<[f32; 3]> {
        // The plan round a body in the way ([`Self::bodies_in_way`]) is
        // waited for where the body stands: on mp_ship's spar at (4676,
        // 64, 1172), 69 from its last waypoint, a bot met head-on ran a
        // random heading for the 7 ticks the search took and fell off it
        // to its death (seed 1381, bot-navigation.md, "The ship's
        // leftovers").
        if self.rest == 0
            && !self.shun.is_empty()
            && self.path.is_empty()
            && self.waited <= WAIT_TICKS
            && body.on_ground
            && !body.on_ladder
        {
            return Some(at);
        }
        let p = self.last?;
        let at = Vec3::from(at);
        (self.rest == 0
            && self.waited <= WAIT_TICKS
            && body.on_ground
            && !body.on_ladder
            && (p - at).truncate().length() < WAIT_REACH
            && (p.z - at.z).abs() < vcod_common::pmove::STEPSIZE)
            .then_some(p.into())
    }

    /// [`Self::waypoint`] before the bookkeeping.
    #[allow(clippy::too_many_arguments)]
    fn follow(
        &mut self,
        g: &NavGraph,
        world: Option<&CollisionWorld>,
        goal: crate::bots::Goal,
        at: [f32; 3],
        body: Footing,
        budget: &mut u32,
        rand: &mut dyn FnMut() -> i32,
    ) -> Option<[f32; 3]> {
        use crate::bots::Goal;
        self.clock += 1;
        self.stop_at = None;
        let landed = self.airborne && body.on_ground;
        self.airborne = !body.on_ground && !body.on_ladder;
        let clock = self.clock;
        self.avoid.retain(|e| e.2 > clock);
        self.dead.retain(|e| e.2 > clock);
        if clock >= self.shun_until && !self.shun.is_empty() {
            self.shun = Arc::default();
            self.shun_through = false;
            self.shunned.clear();
        }
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
                // A destination within reach of the point but a floor off the
                // node a plan to it ends at is no longer the point's: a hurt
                // bot whose fight had planned to mp_ship's hold floor kept
                // that node for the health item on the ledge 116 over it
                // and paced under it while the item lay (bot-navigation.md,
                // "The ship's leftovers").
                let target = g.goal_node(p).map(|t| g.nodes[t as usize].z);
                let off = |d: u32| {
                    let n = g.nodes[d as usize];
                    n.distance(Vec3::from(p)) > RETARGET
                        || target.is_some_and(|z| (n.z - z).abs() > vcod_common::pmove::JUMP_HEIGHT)
                };
                let moved = self.dest.is_none_or(off);
                if moved {
                    // A goal back within `PARK_TICKS` takes up the plan it
                    // left where it was: an enemy in sight for a tick or
                    // two from mp_cassino's stair at (1440, -700, 24)
                    // flipped the goal and back, each replan started at
                    // the node behind the body, and the bot paced the
                    // step for 12 s (seed 17, bot-navigation.md "Jumps").
                    let back = self
                        .parked
                        .take()
                        .filter(|k| clock - k.at <= PARK_TICKS && !off(k.dest));
                    if let Some(d) = self.dest.filter(|_| !self.path.is_empty() && !self.arrived) {
                        self.parked = Some(Parked {
                            dest: d,
                            path: std::mem::take(&mut self.path),
                            next: self.next,
                            best: self.best,
                            idle: self.idle,
                            at: clock,
                        });
                    }
                    self.reset();
                    match back {
                        Some(k) => {
                            self.dest = Some(k.dest);
                            self.path = k.path;
                            self.next = k.next;
                            self.best = k.best;
                            self.idle = k.idle;
                        }
                        None => self.dest = g.goal_node(p),
                    }
                } else if self.arrived {
                    let fell = here.z < self.arrived_at.z - ARRIVED_DROP;
                    if !fell {
                        return Some(self.arrival(g, p));
                    }
                    self.reset();
                    self.dest = g.goal_node(p);
                }
            }
        }
        // A vehicle parked over a node ahead, short of the path's end: plan
        // again round it, from a node the body walks to. Its first node
        // may be one: the start a plan falls back to when no walk arrives.
        let ahead = self
            .path
            .get(self.next.max(1)..self.path.len().saturating_sub(1));
        if !self.through && ahead.is_some_and(|a| a.iter().any(|&n| self.blocks(n))) {
            self.path.clear();
            self.held = None;
            self.pull_base = None;
            self.fell = true;
        }
        if self.path.is_empty() {
            self.on_rungs = body.on_ladder;
            self.path = self.plan(g, world, goal, here, budget, rand)?;
            self.next = 0;
            self.best = f32::INFINITY;
            self.idle = 0;
        }
        let flat = |p: Vec3| (p - here).truncate().length();
        // A plan made or taken up on the rungs starts at the ladder's end the
        // body is nearer, and when the edge on from there is that ladder the
        // body is already on it: head for the other end. Heading for the
        // start first, a bot halfway up mp_ship's hold ladder at (3370,
        // -380) whose goal flipped between two plans went down for one and
        // up for the other, over and over (bot-combat.md 17.2).
        if body.on_ladder
            && self.next == 0
            && let (Some(&a), Some(&b)) = (self.path.first(), self.path.get(1))
        {
            let (a, b) = (g.nodes[a as usize], g.nodes[b as usize]);
            let (low, high) = if a.z < b.z { (a, b) } else { (b, a) };
            if high.z - low.z > vcod_common::pmove::JUMP_HEIGHT
                && (low.z..high.z).contains(&here.z)
                && flat(low) < g.spacing
            {
                self.next = 1;
                self.best = f32::INFINITY;
                self.idle = 0;
            }
        }
        // Past a waypoint once inside its reach, or once nearer the next one
        // than the waypoint itself is: that keeps a bot from doubling back
        // to touch a node it cut the corner on. Either only on the
        // waypoint's floor: a ladder's head is right above its foot.
        while let Some(&w) = self.path.get(self.next) {
            // The top of a jump is passed only standing on it: from its
            // foot the node is in reach flat and the jump not yet made.
            // So is a node a drop leaves from: from under it the drop is
            // not where the graph proved it.
            let jump = self.jumping(g);
            let rise = if jump {
                JUMP_PASS
            } else if self.dropping(g) {
                DROP_PASS
            } else {
                48.0
            };
            // A leap's foot is passed only at rest on it: the leap was
            // proved from a standstill there. A creep's top is held while
            // the body comes onto it faster than [`CREEP_TOP_PACE`].
            let leap = self
                .path
                .get(self.next + 1)
                .is_some_and(|&b| g.leaps.contains(&(w, b)));
            let wp = g.nodes[w as usize];
            // A node held for a body too fast for the walk on is passed only
            // from at rest on it, like a leap's foot.
            let held = self
                .path
                .get(self.next + 1)
                .is_some_and(|&b| self.held == Some((w, b)));
            let foot = leap
                || held
                || (body.on_ground
                    && !body.on_ladder
                    && body.velocity.truncate().length() > CREEP_TOP_PACE
                    && world.is_some_and(|world| self.creep_top(world, g)));
            let w = wp;
            // From under it, only from its floor: a ladder's head is passed
            // standing at its lip, not from the rungs a storey down.
            let below = rise.min(vcod_common::pmove::STEPSIZE);
            // A ladder's head only standing on it: the climb proved it facing
            // the rungs to the top, and a body that turns for the next node
            // on the rungs or in the air over the lip lets go of them
            // (mp_ship's mast, section 3). A jump's top too: in the air
            // beside the ledge, under its floor, the jump is not yet made
            // (mp_ship's ledge at (3347, 250, 616)).
            // On the rungs, a node below only from its floor: the back
            // down that proved it ran onto that floor, and a body that
            // turns for the next node from above it hangs on the rungs
            // under the floor's lip (mp_ship's hold ladder at z 304).
            let above = if body.on_ladder {
                vcod_common::pmove::STEPSIZE
            } else {
                rise
            };
            // And no node off the rungs in the air: a body that turns for
            // the next node in the air over a narrow shelf flies off its
            // side (mp_ship's shelf at (5384, -161, 396)), and one sliding
            // down terrain too steep to stand on turns off a patch it never
            // landed on and slides on into mp_hurtgen's crease (section 3).
            // On the rungs, not a node more than a step below.
            let high = here.z > w.z + vcod_common::pmove::STEPSIZE;
            // A plan begun on the rungs starts at a ladder's end
            // ([`NavGraph::nearest_ladder_end`]); one over the feet is the
            // head, passed only on the ground like any climb's: from the
            // rungs under mp_ship's crow's nest plate a bot turned for the
            // rim's exit and fell down the hatch.
            let head_first = self.next == 0 && w.z > here.z;
            let level = here.z > w.z - below
                && here.z < w.z + above
                && (body.on_ground
                    || (body.on_ladder && !(self.climbing(g) || jump || high || head_first)));
            // A ladder's foot is passed only on it, past cutting nothing:
            // the climb was proved from there, and a body beside the face
            // heading for the head runs past the rungs (mp_ship, section 3).
            let rung = self.rising(g);
            // Nor is a jump's top cut past: a body still in the air beside
            // it is nearer the next node too, and falls back to the foot.
            // Nor a drop's top: the drop was proved from there, and a body
            // cutting across for the bottom pins on whatever stands between
            // (mp_ship's ladder head at (3298, -380, 352), its foot east
            // behind the wall at y -333).
            // Nor a jump's foot: the jump was proved from there, and a body
            // that cut to its top from a ledge beside it jumped off that
            // ledge (mp_ship's at (3468, -210, 798), section 3).
            let past = !foot
                && !jump
                && !rung
                && self.path.get(self.next + 1).is_some_and(|&n| {
                    if g.jumps.contains(&(self.path[self.next], n)) {
                        return false;
                    }
                    let n = g.nodes[n as usize];
                    n.z >= w.z - vcod_common::pmove::JUMP_HEIGHT
                        && flat(n) < (n - w).truncate().length()
                        // Only with floor the whole way: off mp_ship's mast
                        // ladder at x 3696 a body 27 short of the head's exit
                        // at (3705, 85, 992) read as past it and ran straight
                        // for (3648, 71, 994), over the hatch, and fell back
                        // down it (seed 1031, bot-navigation.md section 3).
                        && world.is_none_or(|world| straight_floor(world, here, n))
                });
            let reached = if foot {
                flat(w) < LEAP_FOOT && body.still
            } else if rung {
                // Square in front of the rungs, the run turns into the face
                // from anywhere near (`face_ahead`); off to the side, only
                // the foot itself proved the climb.
                let square = g.face_ahead(w).is_some();
                flat(w) < if square { REACH } else { ARRIVE }
            } else {
                flat(w) < REACH
            };
            if level && reached && leap && !self.leap_holds(world, here, g) {
                // The leap fails from where this body came to rest: round it.
                let edge = (self.path[self.next], self.path[self.next + 1]);
                self.avoid_edge(edge.0, edge.1);
                self.reset();
                return None;
            }
            // An edge planned while it was avoided had no way round it, and
            // is taken as it is: checked again, it would be avoided again
            // every tick and the bot stand without a waypoint (mp_ship's
            // corner at (5375, -193, 352), where a body from the east jams
            // 2 units past the node on the shelf's slope and its seam).
            let planned_round = self.path.get(self.next + 1).is_some_and(|&b| {
                let a = self.path[self.next];
                self.avoid.iter().any(|e| (e.0, e.1) == (a, b))
            });
            // A body that a light weapon's pace flings off the way on comes
            // to rest on the node first and heads on only from at rest
            // ([`Self::flings`]). Passed on the way to rest at the first
            // pace whose walk arrived, bots came onto mp_hurtgen's slope at
            // (338, 2830, -25) and slid into its crease.
            if level
                && (reached || past)
                && (!foot || held)
                && !planned_round
                && self.flings(world, here, body, g)
            {
                let edge = (self.path[self.next], self.path[self.next + 1]);
                if held {
                    // Flung from at rest too: round it.
                    self.avoid_edge(edge.0, edge.1);
                    self.reset();
                    return None;
                }
                self.held = Some(edge);
                break;
            }
            // So does one whose back down off a drop, at its pace, falls
            // short of the floor it was proved onto ([`Self::backs_short`]).
            if level
                && (reached || past)
                && (!foot || held)
                && !planned_round
                && self.backs_short(world, here, body, g)
            {
                let edge = (self.path[self.next], self.path[self.next + 1]);
                if held {
                    self.avoid_edge(edge.0, edge.1);
                    self.reset();
                    return None;
                }
                self.held = Some(edge);
                break;
            }
            if level
                && (reached || past)
                && (!foot || held)
                && !planned_round
                && !self.edge_holds(world, here, body.velocity, body.on_ladder, g)
            {
                // The walk on was proved from the node, and fails from here:
                // close in on the node first, and from on it, round the edge.
                // A drop more than a step down whose walk on from rest on
                // the node arrives is taken from rest there, as a fling is
                // (not a run off a rim that needs its running start: bots
                // stopped at each of mp_depot's pit drops at 212 in turn and
                // paced the rim, seed 149). Avoided at the pace a bot came
                // onto mp_ship's crow's nest rim at (4929, 61, 1254), the
                // spar at 1212 that it walks onto from rest was left, the
                // roam picked anew down the ladder and met the next bot up
                // (seed 307). Not a level or rising edge: brought to rest
                // on the hull rib at (2214, -325, -48), a bot took the next
                // plan's jump edge from rest, where the graph proved it with
                // a running start, and slid into the crease beside it (seed
                // 603, bot-navigation.md section 3).
                let edge = (self.path[self.next], self.path[self.next + 1]);
                if (flat(w) < ON_NODE || held) && here.z < w.z + ARRIVE {
                    let drop = g.nodes[edge.1 as usize].z < w.z - vcod_common::pmove::STEPSIZE;
                    if !held && drop && self.edge_holds(world, w, Vec3::ZERO, false, g) {
                        self.held = Some(edge);
                        break;
                    }
                    self.avoid_edge(edge.0, edge.1);
                    self.reset();
                    return None;
                }
                break;
            }
            // Taken as it is, but from at rest on the node: passed at the
            // pace that had it avoided, bots ran off the crow's nest's top
            // at (4928, 61, 1254) on a turn, over the spar at 1212 and down
            // to the deck at 400 (bot-navigation.md section 3).
            if level
                && (reached || past)
                && planned_round
                && !held
                && !self.edge_holds(world, here, body.velocity, body.on_ladder, g)
            {
                self.held = Some((self.path[self.next], self.path[self.next + 1]));
                break;
            }
            if level && (reached || past) {
                self.held = None;
                self.pull_base = None;
                self.next += 1;
                self.best = f32::INFINITY;
                self.idle = 0;
            } else {
                break;
            }
        }
        // Not on a landing: the landing is read against the edge under way.
        if !landed {
            self.pull(g, world, here, body);
        }
        let Some(&w) = self.path.get(self.next) else {
            // Arrived. A roam picks somewhere new; a point is walked at.
            self.path.clear();
            return match goal {
                Goal::To(p) => {
                    self.arrived = true;
                    self.arrived_at = here;
                    Some(self.arrival(g, p))
                }
                _ => {
                    self.dest = None;
                    None
                }
            };
        };
        let w = g.nodes[w as usize];
        let slack = if self.foot_slack { REACH } else { 0.0 };
        if flat(w) > OFF_PATH_STEPS * g.spacing + slack && !(self.long_mantles && self.mantling(g))
        {
            // Fallen this far off the edge under way, still in the air: round
            // it, as a landing under it below plans. Planned again with it,
            // a bot ran off mp_ship's ledge at (7256, 237, 166) down the
            // roof to the hull floor four times in 20 s (seed 125,
            // bot-navigation.md section 3).
            if self.next > 0 && !self.climbing(g) && w.z - here.z > FELL_BELOW {
                let edge = (self.path[self.next - 1], self.path[self.next]);
                self.avoid_edge(edge.0, edge.1);
                self.fell = true;
            }
            self.path.clear();
            return None;
        }
        // Fallen off the path: a waypoint more than a climb over the feet,
        // and no ladder leads to it. Plan again from where the body landed,
        // round the edge it fell off (mp_ship's lip at (5424, -187, 344),
        // under the shelf at 396).
        if body.on_ground && self.next > 0 && !self.climbing(g) && w.z - here.z > FELL_BELOW {
            let edge = (self.path[self.next - 1], self.path[self.next]);
            self.avoid_edge(edge.0, edge.1);
            self.reset();
            self.fell = true;
            return None;
        }
        // Landed more than a step under both ends of a level edge that is
        // no jump: off its side into a pocket the walk never went
        // through. mp_cassino's roof dent at (4948, -1413, 280), 23 under
        // the edge from (4938, -1393, 303) to (4991, -1440, 303), took a
        // bot whose next plan started at the nearest node, level with
        // the dent's floor behind its wall, and held it 4 s (seed 25,
        // bot-objectives.md 10.6). Plan again round the edge from a node
        // it walks or jumps to.
        if landed && self.next > 0 {
            let (a, b) = (self.path[self.next - 1], self.path[self.next]);
            let (za, zb) = (g.nodes[a as usize].z, g.nodes[b as usize].z);
            let step = vcod_common::pmove::STEPSIZE;
            if !g.jumps.contains(&(a, b)) && (za - zb).abs() < step && here.z < za.min(zb) - step {
                self.avoid_edge(a, b);
                self.reset();
                self.fell = true;
                return None;
            }
        }
        // Down a drop onto a floor partway, the waypoint more than a jump
        // under the feet and within reach flat, the edge's top more than a
        // jump over them: a run off mp_ship's slot at (5403, -160, 396)
        // for the floor at 204 landed on the ledge at 336 over its
        // waypoint, turned on the spot until an unstick spell took it off
        // (bot-navigation.md section 3). Plan again from where it landed,
        // the edge not avoided: round it, the plan went back up to the
        // shelf and on by a jump edge the bot bounced under. Within a jump
        // of the top the body may still stand on the drop's own slope: one
        // backing off mp_ship's dome at (3648, -317, 618) stood on its 45
        // degree face 23 under the node, planned again from there and
        // walked off the dome's south side into the hold (seed 2245).
        if body.on_ground
            && !body.on_ladder
            && self.next > 0
            && here.z - w.z > vcod_common::pmove::JUMP_HEIGHT
            && flat(w) < REACH
            && g.nodes[self.path[self.next - 1] as usize].z - here.z
                > vcod_common::pmove::JUMP_HEIGHT
        {
            self.reset();
            self.fell = true;
            return None;
        }
        // Run off a drop and brought to a stop short of the lip on the
        // edge's top, where the run on from rest no longer lands: plan
        // round the edge. A bot that ran along the wall at y -400 on
        // mp_ship's hold floor at 56 for the drop to (3264, -378, -64)
        // bumped a bot dropping ahead of it, then pressed at its waypoint
        // behind the wall until an unstick spell ran it off the floor's
        // south side (seed 2195, bot-navigation.md section 3). Once: a
        // plan with no way round takes the edge again from rest, and on
        // mp_depot's roof at 212 a run off from rest misses every drop, so
        // the bot planned round and back for good.
        if let Some(world) = world
            && body.on_ground
            && !body.on_ladder
            && self.next > 0
            && here.z - w.z > vcod_common::pmove::JUMP_HEIGHT
            && flat(w) > REACH
            && body.velocity.truncate().length() < UNPIN_SPEED
        {
            let edge = (self.path[self.next - 1], self.path[self.next]);
            let top = g.nodes[edge.0 as usize];
            if (here.z - top.z).abs() < vcod_common::pmove::STEPSIZE
                && !g.jumps.contains(&edge)
                && !self.avoid.iter().any(|a| (a.0, a.1) == edge)
                && self.runs_off(world, edge, top, w)
                && !matches!(
                    walk_with(
                        world,
                        here,
                        Some(body.velocity),
                        w.truncate(),
                        Some(w.z),
                        Gait::Forward
                    ),
                    Walked::Arrived(_, false)
                )
            {
                self.avoid_edge(edge.0, edge.1);
                self.reset();
                return None;
            }
        }
        // A plan's first node more than a step over the feet is a floor
        // the body left while the search ran: a bot that wandered off
        // mp_ship's crow's nest top at 1254 onto the hatch's rim at 1219
        // headed back for the top, pinned on its face and slid along it
        // down the hatch. Plan again from a node it walks to.
        if body.on_ground
            && !body.on_ladder
            && self.next == 0
            && !self.proved_start
            && w.z - here.z > vcod_common::pmove::STEPSIZE
        {
            self.reset();
            self.fell = true;
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
                // Either way the next plan starts at a node a walk from
                // the body reaches: the nearest may stand across a wall
                // (mp_cassino's slot at (945, -775, 64), seed 1 with no
                // vehicles; its roof dent, seed 25, bot-objectives.md 10.6).
                self.fell = true;
                self.gave_up += 1;
                if let Some(from) = self.next.checked_sub(1).map(|i| self.path[i]) {
                    let to = self.path[self.next];
                    self.avoid_edge(from, to);
                } else {
                    self.rest = REST_TICKS;
                }
                self.reset();
                return None;
            }
        }
        // Up a ladder from its foot: into the face first, the way the climb
        // that found the head went, then at the head once level with it.
        if self.climbing(g)
            && here.z < w.z - vcod_common::pmove::STEPSIZE
            && let Some(p) = g.face_ahead(g.nodes[self.path[self.next - 1] as usize])
        {
            return Some([p.x, p.y, w.z]);
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
        world: Option<&CollisionWorld>,
        goal: crate::bots::Goal,
        here: Vec3,
        budget: &mut u32,
        rand: &mut dyn FnMut() -> i32,
    ) -> Option<Vec<u32>> {
        if *budget == 0 {
            return None;
        }
        if self.search.is_none() {
            self.proved_start = true;
            self.jump_start = false;
            let start = match world {
                _ if self.on_rungs => g.nearest_ladder_end(here),
                // Nor from a node a parked vehicle stands over.
                Some(w)
                    if std::mem::take(&mut self.fell)
                        || g.off_floor(here)
                        || g.nearest(here.into()).is_some_and(|n| self.blocks(n)) =>
                {
                    g.walkable_start(w, here).map(|(n, jump)| {
                        self.jump_start = jump;
                        n
                    })
                }
                _ => {
                    self.proved_start = false;
                    g.nearest(here.into())
                }
            }?;
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
            // A destination a parked vehicle stands over, as a boarding's
            // is: the clear node nearest it, and from there a point goal
            // walks at its point.
            let to = if self.blocks(to) {
                g.clear_near(to, &self.blocked, |n| g.reaches(start, n))
                    .unwrap_or(to)
            } else {
                to
            };
            // Round the edges it got stuck on when there is a way round,
            // else through them again: whatever blocked one may have moved.
            // Not through one that failed again so ([`Self::avoid_edge`]).
            let avoid: Vec<(u32, u32)> = self.avoid.iter().map(|e| (e.0, e.1)).collect();
            self.round = !avoid.is_empty();
            self.search = Some(
                Search::new(g, start, to, avoid)
                    .crowded(self.crowd.clone(), self.crowd_cost)
                    .noisy(self.noise)
                    .blocking(self.blocked.clone())
                    .shunning(self.shun.clone())
                    .jumps_costing(self.jump_cost),
            );
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
                let s = self.search.take()?;
                self.through = s.blocked.is_empty() && !self.blocked.is_empty();
                self.shun_through = !self.shun.is_empty()
                    && (s.shun.is_empty() || self.meets_shunned(g, here, &path));
                self.penned = false;
                Some(path)
            }
            Plan::Failed if self.search.as_ref().is_some_and(|s| !s.shun.is_empty()) => {
                // No way round the bodies in the way: the same search
                // through them, the bot waiting on them meanwhile.
                let s = self.search.take()?;
                self.search = Some(
                    Search::new(g, s.from, s.to, s.avoid)
                        .crowded(s.crowd, s.crowd_cost)
                        .noisy(s.noise)
                        .blocking(s.blocked)
                        .jumps_costing(s.jump_cost),
                );
                None
            }
            Plan::Failed => {
                let s = self.search.take()?;
                // Round the edges it got stuck on, then through all but the
                // dead ones.
                let blocked = if self.round {
                    self.round = false;
                    s.blocked.clone()
                } else if s.blocked.is_empty() {
                    self.reset();
                    self.rest = REST_TICKS;
                    return None;
                } else if s.reached() < PEN_NODES {
                    // Hemmed in by parked vehicles: no plan until one
                    // moves off, and the bot reads as stranded meanwhile.
                    // Planned through them, bots that got out of a tank
                    // onto mp_cassino's terrace at (383, -916, 76), the
                    // tank parked over the terrace's one stair, wedged on
                    // its hull (seeds 23 and 1).
                    self.penned = true;
                    self.reset();
                    self.rest = REST_TICKS;
                    return None;
                } else {
                    // A destination no way round them reaches: through
                    // them, as round an edge it got stuck on.
                    Arc::default()
                };
                let dead = self.dead.iter().map(|e| (e.0, e.1)).collect();
                self.search = Some(
                    Search::new(g, s.from, s.to, dead)
                        .crowded(s.crowd, s.crowd_cost)
                        .noisy(s.noise)
                        .blocking(blocked)
                        .jumps_costing(s.jump_cost)
                        .risking(),
                );
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

    /// Whether the walk from the current waypoint to the next arrives run
    /// from `at`: the graph proved it from the node, and over a hull beam's
    /// slope a body a few units aside slides back. A climb or a drop
    /// steeper than a jump is taken as proved (a ladder, a back down), and
    /// so is a walk from the rungs under the node's floor (`on_rungs`, a
    /// body on a ladder under its head) or without a world. From over it,
    /// or from up a slope under it, the walk has to arrive:
    /// mp_ship's lip at (5424, -187, 344) stands 24 over a stair node,
    /// within reach of it flat, and the walk on from there pins under a
    /// shelf. Off a jump edge the walk has to arrive
    /// without one: the bot jumps only on the graph's jump edges, and a
    /// body a few units aside of a hull beam's end pins on it where the
    /// walk from the node slides past (bot-navigation.md section 3).
    fn edge_holds(
        &mut self,
        world: Option<&CollisionWorld>,
        at: Vec3,
        pace: Vec3,
        on_rungs: bool,
        g: &NavGraph,
    ) -> bool {
        let (Some(world), Some(&a), Some(&b)) = (
            world,
            self.path.get(self.next),
            self.path.get(self.next + 1),
        ) else {
            return true;
        };
        let jump = g.jumps.contains(&(a, b));
        let edge = (a, b);
        self.walked_jump = None;
        // No CoD1 walk climbs a mantle; the graph's probe proved it.
        if g.mantles.contains(&edge) {
            return true;
        }
        // A crouched jump's walk is the gait that proved it: no other gets
        // through the window.
        let gait = if g.crouch_jumps.contains(&edge) {
            Gait::CrouchJump
        } else {
            Gait::Forward
        };
        let (a, b) = (g.nodes[a as usize], g.nodes[b as usize]);
        // Only from the rungs: up mp_dawnville's slope to (-1193, -16944,
        // 102) a body passes the node 12 under it, and from 4 units off
        // the line the walk on pins where the bot did, stalled until an
        // unstick spell ran it back down the slope (seed 239,
        // bot-navigation.md section 3).
        if on_rungs && at.z < a.z - ARRIVE {
            return true;
        }
        // A run off a drop has to arrive at the pace the body comes onto
        // it: mp_ship's slot between the stair's top and the shelf at
        // (5403, -160, 396) lands a run straight at it from anywhere near
        // the node, and misses from a turn off the shelf. A drop the run
        // misses from the node itself was backed down, and is taken as
        // proved.
        if b.z < a.z - vcod_common::pmove::JUMP_HEIGHT {
            if !self.runs_off(world, edge, a, b) {
                return true;
            }
            let lands = |at: Vec3| {
                matches!(
                    walk_with(
                        world,
                        at,
                        Some(pace),
                        b.truncate(),
                        Some(b.z),
                        Gait::Forward
                    ),
                    Walked::Arrived(_, false)
                )
            };
            // Where a miss can hurt, the run has to land from a body a
            // couple of units either side of this one too: off mp_ship's (3564, 252, 896) a run
            // onto the ledge at 760 lands on a slope's corner the walk
            // clips and the bot's own run, two units on, misses, and three
            // bots fell to the deck at 352 (bot-navigation.md section 3).
            let across = pace.truncate().perp().normalize_or_zero().extend(0.0) * ASIDE_DROP;
            // Not where a body on the node, turned round the edge with
            // nothing to steer by, coasts off a lip anyway: off mp_ship's
            // mast top at (3301, 128, 1030), 3 units from its lip, one did
            // and fell to the deck at 480 (seed 449).
            let on_node = (at - a).truncate().length() < ON_NODE;
            return lands(at)
                && (across == Vec3::ZERO
                    || (on_node && coasts_off(world, at, pace))
                    || !self.miss_hurts(world, edge, b)
                    || (lands(at + across) && lands(at - across)));
        }
        if (b.z - a.z).abs() > vcod_common::pmove::JUMP_HEIGHT {
            return true;
        }
        // Just up a ladder, a fall at the weapon's own scale, as in
        // [`Self::flings`]: off mp_ship's mast hatch at (3705, 58, 992) a
        // run at 1 arrives and one at 0.9 falls back down the hatch, so the
        // bot climbed it again and again. Only there and only a fall: read
        // on every walk, or off every ladder's head floor, it held and
        // rerouted bots on the hold and the shelf at (5384, -161, 396).
        let up_a_ladder = (self.next.saturating_sub(2)..self.next).any(|i| {
            let (p, q) = (self.path[i] as usize, self.path[i + 1] as usize);
            g.nodes[q].z - g.nodes[p].z > vcod_common::pmove::JUMP_HEIGHT
        });
        if self.move_scale < 1.0
            && up_a_ladder
            && matches!(
                self.walk_paced(world, at, pace, b, Gait::Forward),
                Walked::Fell
            )
        {
            return false;
        }
        let target = if jump {
            g.aim(edge.0, edge.1)
        } else {
            b.truncate()
        };
        let walk = |at: Vec3, trail: &mut Trail| {
            let w = walk_sliding(world, at, Some(pace), 1.0, target, Some(b.z), gait, trail);
            match w {
                Walked::Arrived(_, jumped) if jump || !jumped => Some(jumped),
                _ => None,
            }
        };
        let mut trail = Trail::default();
        let Some(jumped) = walk(at, &mut trail) else {
            return false;
        };
        // A jump edge the walk on runs without its jump: the bot does not
        // jump either. Off mp_ship's stair stringer at (5823, 567, 92) the
        // walk on to (5792, 576, 76) slides along the stringer's end where
        // the bot, pinned on it, jumped on its cue and fell back, again and
        // again (seed 2553 on 1080e30d, bot-navigation.md section 3).
        if jump && !jumped {
            self.walked_jump = Some(edge);
        }
        // A run on, neither a jump nor down a step, that comes down off
        // the ground under where it set off, where a miss can hurt: a run
        // a couple of units either side may not fall deeper than a jump.
        // Off mp_ship's dome crest at (3615, -345, 635) the walk on from
        // where a bot passed the node dips 7 onto the south face, the same
        // walk two units across flies off it, and the bot flew off it to
        // the hold at 56 (seed 4231, bot-navigation.md section 3).
        let across = pace.truncate().perp().normalize_or_zero().extend(0.0) * ASIDE_DROP;
        let flies = |at: Vec3| {
            let mut trail = Trail::default();
            walk(at, &mut trail).is_none() && trail.dip > vcod_common::pmove::JUMP_HEIGHT
        };
        jump || trail.dip <= 0.0
            || b.z < a.z - vcod_common::pmove::STEPSIZE
            || across == Vec3::ZERO
            || !self.miss_hurts(world, edge, b)
            || !(flies(at + across) || flies(at - across))
    }

    /// Whether a body faster than the walk's run, at `pace` and its
    /// weapon's [`Self::move_scale`], falls off the way on from the current
    /// waypoint where [`Self::edge_holds`]' walk at the run's own scale
    /// holds: a run over mp_depot's roof peak at (-2660, -1216, 552) at 220
    /// units/s flies off the crest and over the roof's far edge, where one
    /// at 190 lands inside it (bot-navigation.md section 3). Only a fall:
    /// read for every miss at the weapon's scale, the verdict held and
    /// rerouted bots on mp_ship far more often and more of them died. At
    /// rest on a held node the same walk decides whether the edge is taken
    /// at all: a weapon at 1.16 is back at 220 by the crest from a
    /// standstill 17 units short of it.
    fn flings(
        &self,
        world: Option<&CollisionWorld>,
        at: Vec3,
        body: Footing,
        g: &NavGraph,
    ) -> bool {
        let (Some(world), Some(&a), Some(&b)) = (
            world,
            self.path.get(self.next),
            self.path.get(self.next + 1),
        ) else {
            return false;
        };
        // Only a walk down off a crest, more than a step and no more than
        // a jump: a body flies off one. A climb, a jump or a backed drop
        // is taken another way; a drop run off needs the running start a
        // stop takes away (held on mp_ship's (3648, -317, 618), bots ran
        // off its drop to the ledge at 400 from rest and died); and a miss
        // on the level or uphill held bots off mp_hurtgen's (292, 2785,
        // -52) onto a way round that slides into its crease.
        let (from, to) = (g.nodes[a as usize], g.nodes[b as usize]);
        let drop = from.z - to.z;
        let run = drop > vcod_common::pmove::STEPSIZE
            && drop <= vcod_common::pmove::JUMP_HEIGHT
            && !g.jumps.contains(&(a, b));
        let fast = body.velocity.truncate().length() > vcod_common::pmove::SPEED_RUN;
        run && body.on_ground
            && (fast || (body.still && self.move_scale > 1.0))
            && matches!(
                self.walk_paced(world, at, body.velocity, g.nodes[b as usize], Gait::Forward),
                Walked::Fell
            )
    }

    /// Whether the way on from the current waypoint is a drop deeper than a
    /// jump that a run off misses, so the bot backs down it, and that the
    /// bot's own back down ([`Gait::Creep`]: a run, creeping at the lip)
    /// from `at` at the body's pace misses: a body that comes onto it off
    /// a turn carries its pace over the lip (mp_ship's ledge at
    /// (3686, -354, 400) over the hold, and the hold ladder's top at
    /// (4319, -133, 600), bot-navigation.md section 3). At rest on a held
    /// node the walk decides whether the edge is taken at all.
    fn backs_short(
        &mut self,
        world: Option<&CollisionWorld>,
        at: Vec3,
        body: Footing,
        g: &NavGraph,
    ) -> bool {
        let (Some(world), Some(&a), Some(&b)) = (
            world,
            self.path.get(self.next),
            self.path.get(self.next + 1),
        ) else {
            return false;
        };
        let (from, to) = (g.nodes[a as usize], g.nodes[b as usize]);
        // A mantle down over a rail: no walk climbs it, the graph's probe
        // proved it from rest at its foot. Under CoD1's walks the creep
        // back down missed it at pace and arrived from rest, so the bot
        // came to rest first and took it; a game step that mantles
        // (`bots::cod4::Cod4Walk`) misses it either way, so rest and go
        // (bot-objectives.md 12.11).
        if self.mantle_rests
            && g.mantles.contains(&(a, b))
            && to.z < from.z - vcod_common::pmove::JUMP_HEIGHT
        {
            return body.on_ground && self.held != Some((a, b));
        }
        to.z < from.z - vcod_common::pmove::JUMP_HEIGHT
            && body.on_ground
            && !g.jumps.contains(&(a, b))
            && !self.runs_off(world, (a, b), from, to)
            && !matches!(
                self.walk_paced(world, at, body.velocity, to, Gait::Creep),
                Walked::Arrived(..)
            )
    }

    /// The walk from `at` onto `to`'s floor with the body moving at `pace`
    /// and at its weapon's [`Self::move_scale`]: how this bot comes onto
    /// the edge, where the graph's walks ran at 1 from the node.
    fn walk_paced(
        &self,
        world: &CollisionWorld,
        at: Vec3,
        pace: Vec3,
        to: Vec3,
        gait: Gait,
    ) -> Walked {
        walk_sliding(
            world,
            at,
            Some(pace),
            self.move_scale,
            to.truncate(),
            Some(to.z),
            gait,
            &mut Trail::default(),
        )
    }

    /// The height of a fall off a ladder that kills this body.
    fn deadly_fall(&self) -> f32 {
        self.fall_kills.unwrap_or(DEADLY_LADDER)
    }

    /// Where to wait instead of following the path: the bot is at one end
    /// of the ladder its path takes next, off the rungs, and another player
    /// on those rungs is about to step off there or hangs there. Two players
    /// meeting head-on at a ladder pin each other: the climber cannot step
    /// off where the other stands (mp_ship's ladder at (3354, -380),
    /// bot-navigation.md section 3). The bot steps to the neighbour of its
    /// end farthest from the ladder until the rungs clear, at most
    /// [`YIELD_TICKS`] ([`YIELD_HUNG`] of them with the climber still).
    /// `climbers` are the other players on a ladder, as (origin, velocity);
    /// `off_rungs` the others off one, as the same.
    pub fn ladder_yield(
        &mut self,
        g: &NavGraph,
        at: Vec3,
        body: Footing,
        climbers: &[(Vec3, Vec3)],
        off_rungs: &[(Vec3, Vec3)],
    ) -> Option<Vec3> {
        self.yield_rest = self.yield_rest.saturating_sub(1);
        let spot = self.yield_spot(g, at, body, climbers, off_rungs);
        if let Some((p, moving, under_way)) = spot
            && self.yield_ticks < YIELD_TICKS
            && self.yield_hung < YIELD_HUNG
        {
            // Passed already: the end is the waypoint again once the wait
            // is over. Headed straight for the foot from the spot aside,
            // bots backed down off mp_ship's crow's nest rim beside the
            // plate and fell past the rungs.
            if under_way {
                self.next -= 1;
            }
            self.yield_ticks += 1;
            self.yield_hung += u32::from(!moving);
            return Some(p);
        }
        if self.yield_ticks > 0 {
            if spot.is_some() {
                self.yield_rest = YIELD_REST;
            }
            self.yield_ticks = 0;
            self.yield_hung = 0;
            // The way back to the waypoint is progress afresh.
            self.best = f32::INFINITY;
            self.idle = 0;
        }
        None
    }

    /// Whether the body, climbing toward a ladder's head on the rungs, lets
    /// another climber beside it go first: one within two half-widths flat
    /// and a body's height in z that is higher, or level and first in
    /// (y, x) order, so of two abreast exactly one gives way; only to one
    /// still climbing, or a climber that hangs would hold it off for good.
    /// mp_ship's ladder at x 3792 is 25 wide and the hatch over its top
    /// lets a body through only between y 58 and 72; two bots that climbed
    /// it abreast at y 46 and 76, 30 apart, each pressed at the lane the
    /// other's capsule stood in and neither moved again (bot-navigation.md
    /// section 3). The one that gives way climbs down until the other is
    /// off the rungs. `climbers` as in [`Self::ladder_yield`].
    pub fn abreast_yield(
        &self,
        g: &NavGraph,
        at: Vec3,
        body: Footing,
        climbers: &[(Vec3, Vec3)],
    ) -> bool {
        use vcod_common::pmove::HALF_WIDTH;
        if !body.on_ladder || !self.climbing(g) {
            return false;
        }
        climbers.iter().any(|&(c, v)| {
            let dz = c.z - at.z;
            v.z > 1.0
                && (c - at).truncate().length() < 2.0 * HALF_WIDTH + 2.0
                && dz.abs() < ABREAST_HEIGHT
                && (dz > 1.0 || (dz.abs() <= 1.0 && (c.y, c.x) < (at.y, at.x)))
        })
    }

    /// On the rungs backing down a ladder with another climber coming up
    /// under the body: the head to climb back to. A body backing down
    /// onto a climber's head stands on it and slides off; down mp_ship's
    /// crow's nest ladder that is a fall to the deck at 400 (bot-navigation.md
    /// section 3). At the head the bot waits aside ([`Self::ladder_yield`]).
    /// `climbers` as in [`Self::ladder_yield`].
    pub fn head_on_yield(
        &self,
        g: &NavGraph,
        at: Vec3,
        body: Footing,
        climbers: &[(Vec3, Vec3)],
    ) -> Option<Vec3> {
        use vcod_common::pmove::{HALF_WIDTH, JUMP_HEIGHT};
        if !body.on_ladder {
            return None;
        }
        let i = self.next.checked_sub(1)?;
        let (head, foot) = (
            g.nodes[*self.path.get(i)? as usize],
            g.nodes[*self.path.get(self.next)? as usize],
        );
        if foot.z > head.z - self.deadly_fall().max(JUMP_HEIGHT) {
            return None;
        }
        climbers
            .iter()
            .any(|&(c, v)| {
                v.z > 1.0
                    && c.z < at.z
                    && at.z - c.z < HEAD_ON
                    && (c - at).truncate().length() < 2.0 * HALF_WIDTH + 2.0
            })
            .then_some(head)
    }

    /// Where to stand instead of heading for `waypoint`: the body is on the
    /// ground off the rungs, a climber rises under it with its top less
    /// than [`RISING_BELOW`] under the feet, and the step toward the
    /// waypoint closes on it inside [`RISING_REACH`] flat. Two players'
    /// boxes that overlap are pushed apart at their speed
    /// (`game::stuck::stuck_in_client`, cod11-player-clip.md 6): a bot
    /// that stepped toward mp_ship's crow's nest rim over a climber coming
    /// up the hatch was thrown 190 units/s west off the top's far side
    /// (seed 1307, bot-navigation.md section 3). At most [`RISING_TICKS`],
    /// and not toward a waypoint more than a jump down: a body bound down
    /// the climber's own ladder is [`Self::ladder_yield`]'s, and held at
    /// mp_ship's x 3793 head it stood in the climber's way off (seed 3223).
    /// `climbers` as in [`Self::ladder_yield`].
    pub fn rising_hold(
        &mut self,
        at: Vec3,
        waypoint: Vec3,
        body: Footing,
        climbers: &[(Vec3, Vec3)],
    ) -> Option<Vec3> {
        use vcod_common::pmove::HEIGHT_STAND;
        let toward = (waypoint - at).truncate().normalize_or_zero();
        let step = at.truncate() + toward * ARRIVE;
        let rising = body.on_ground
            && !body.on_ladder
            && waypoint.z > at.z - vcod_common::pmove::JUMP_HEIGHT
            && climbers.iter().any(|&(c, v)| {
                let (now, next) = (
                    c.truncate().distance(at.truncate()),
                    c.truncate().distance(step),
                );
                v.z > 1.0
                    && c.z < at.z
                    && c.z + HEIGHT_STAND > at.z - RISING_BELOW
                    && next < RISING_REACH
                    && next < now
            });
        if !rising || self.rising_ticks >= RISING_TICKS {
            if !rising {
                self.rising_ticks = 0;
            }
            return None;
        }
        self.rising_ticks += 1;
        Some(at)
    }

    /// Another body (`others`, off the rungs: origin and velocity) in
    /// the way: the body on the ground, off the rungs, slower than
    /// [`BODY_HELD_SPEED`] for [`BODY_TICKS`] with another within
    /// [`BODY_REACH`] across and [`BODY_RISE`] up or down, toward its next
    /// node. The plan is made again keeping off the nodes the others' boxes
    /// stand over ([`NavGraph::nodes_in_box`]) for [`SHUN_TICKS`], or through
    /// them when no way goes round, and not again until those run out.
    /// Before, the follower took one edge out of its plans after
    /// [`STUCK_TICKS`] without closing, and the next plan took the edge
    /// beside it past the same body (bot-navigation.md section 3, "Bodies
    /// in the way"). Returns whether the plan was dropped.
    pub fn bodies_in_way(
        &mut self,
        g: &NavGraph,
        at: Vec3,
        body: Footing,
        others: &[(Vec3, Vec3)],
    ) -> bool {
        use vcod_common::pmove::{HALF_WIDTH, STEPSIZE};
        let Some(&n) = self.path.get(self.next) else {
            self.body_ticks = 0;
            return false;
        };
        // Toward the next node, or the one after it when on the next.
        let to = [Some(n), self.path.get(self.next + 1).copied()]
            .into_iter()
            .flatten()
            .map(|n| (g.nodes[n as usize] - at).truncate())
            .find(|d| d.length() > 8.0);
        let ahead = |o: Vec3| {
            let d = (o - at).truncate();
            d.length() < BODY_REACH
                && (o.z - at.z).abs() < BODY_RISE
                && to.is_some_and(|w| d.dot(w) > 0.5 * d.length() * w.length())
        };
        let held = self.shun.is_empty()
            && body.on_ground
            && !body.on_ladder
            && body.velocity.truncate().length() < BODY_HELD_SPEED;
        let in_way: Vec<Vec3> = if held {
            others.iter().map(|o| o.0).filter(|&o| ahead(o)).collect()
        } else {
            Vec::new()
        };
        if in_way.is_empty() {
            self.body_ticks = 0;
            return false;
        }
        self.body_ticks += 1;
        if self.body_ticks < BODY_TICKS {
            return false;
        }
        self.body_ticks = 0;
        let mut nodes: Vec<u32> = in_way
            .iter()
            .flat_map(|&o| {
                g.nodes_in_box(o, 2.0 * HALF_WIDTH, STEPSIZE)
                    .into_iter()
                    .chain(g.nearest(o.into()))
            })
            .collect();
        // The node under way, when the others stand on the edge to it.
        if let Some(&from) = self.next.checked_sub(1).and_then(|i| self.path.get(i)) {
            let (a, b) = (g.nodes[from as usize], g.nodes[n as usize]);
            if in_way
                .iter()
                .any(|&o| seg_dist_flat(a, b, o) < 2.0 * HALF_WIDTH)
            {
                nodes.push(n);
            }
        }
        nodes.sort_unstable();
        nodes.dedup();
        self.shun = nodes.into();
        self.shunned = in_way;
        self.shun_until = self.clock + SHUN_TICKS;
        self.path.clear();
        self.search = None;
        self.held = None;
        self.pull_base = None;
        self.fell = true;
        true
    }

    /// Where to stand aside for an oncoming body in a way with no room to
    /// pass: the plan found no way round the bodies in the way
    /// ([`Self::bodies_in_way`]), and one of `oncoming`'s (origin, and the
    /// way it heads: the bodies this one gives way to; read only past the
    /// tests on this body, at most once) stands in the way,
    /// heading at the body. The spot is a node one or two level edges off
    /// across the line between them ([`NavGraph::sidestep`]), clear of
    /// that line by two half-widths, else [`ASIDE_BACK`] straight back
    /// from the other. The bot rests there until no oncoming body is
    /// ahead of it within two reaches across the way it came, or for
    /// [`ASIDE_TICKS`], then plans again; not again for
    /// [`ASIDE_REST`]. Pressed in a corridor, two bots each planned
    /// through the other and paced until an unstick spell parted them
    /// (bot-navigation.md section 3, "Bodies in the way").
    pub fn stand_aside(
        &mut self,
        g: &NavGraph,
        at: Vec3,
        body: Footing,
        oncoming: impl FnOnce() -> Vec<(Vec3, Vec3)>,
    ) -> Option<Vec3> {
        use vcod_common::pmove::HALF_WIDTH;
        self.aside_rest = self.aside_rest.saturating_sub(1);
        if let Some((spot, dir, left)) = self.aside {
            let ahead = oncoming().iter().any(|&(o, _)| {
                let d = (o - at).truncate();
                d.length() < 2.0 * BODY_REACH && d.dot(dir.truncate()) > 0.0
            });
            if left == 0 || !ahead {
                self.aside = None;
                self.aside_rest = ASIDE_REST;
                self.shun = Arc::default();
                self.shun_through = false;
                self.path.clear();
                self.search = None;
                self.fell = true;
                return None;
            }
            self.aside = Some((spot, dir, left - 1));
            return Some(spot);
        }
        if !self.shun_through
            || self.aside_rest > 0
            || !body.on_ground
            || body.on_ladder
            || body.velocity.truncate().length() >= BODY_HELD_SPEED
        {
            return None;
        }
        let to = self
            .path
            .get(self.next)
            .map(|&n| (g.nodes[n as usize] - at).truncate())?;
        let &(o, _) = oncoming().iter().find(|&&(o, way)| {
            let (d, way) = ((o - at).truncate(), way.truncate());
            d.length() < BODY_REACH
                && (o.z - at.z).abs() < BODY_RISE
                && d.dot(to) > 0.5 * d.length() * to.length()
                && (-d).dot(way) > 0.5 * d.length() * way.length()
        })?;
        let dir = (o - at).truncate().normalize_or_zero();
        let clear = |p: Vec3| {
            let off = (p - at).truncate();
            off.perp_dot(dir).abs() > 2.0 * HALF_WIDTH
                && (p - o).truncate().length() > 2.0 * HALF_WIDTH
        };
        let spot = g
            .sidestep(at, dir.extend(0.0), clear)
            .unwrap_or(at - dir.extend(0.0) * ASIDE_BACK);
        self.aside = Some((spot, dir.extend(0.0), ASIDE_TICKS - 1));
        Some(spot)
    }

    /// Whether `path`, from a body at `here`, runs into one of the bodies
    /// [`Self::shun`] keeps off on its first two legs: the plan's start
    /// can be a node beside or under one of them, which the search never
    /// keeps off (mp_uo_stanjel's slope at (1, 2885, -154), two bots each
    /// planning from the node between them).
    fn meets_shunned(&self, g: &NavGraph, here: Vec3, path: &[u32]) -> bool {
        let pts: Vec<Vec3> = std::iter::once(here)
            .chain(path.iter().take(2).map(|&n| g.nodes[n as usize]))
            .collect();
        pts.windows(2).any(|w| {
            self.shunned
                .iter()
                .any(|&o| seg_dist_flat(w[0], w[1], o) < 2.0 * vcod_common::pmove::HALF_WIDTH)
        })
    }

    /// The waypoint the follower handed out this tick, for the bodies
    /// that give way to it ([`Self::stand_aside`]).
    pub fn heading(&self) -> Option<Vec3> {
        self.last.filter(|_| self.waited == 0)
    }

    /// Where to step instead of heading for `waypoint`: the body and
    /// another off the rungs stand pressed together on the ground (within
    /// two half-widths and 2 flat, a step in z), both slower than
    /// [`UNPIN_SPEED`] for [`PRESSED_TICKS`], the body within two reaches
    /// of its waypoint and farther from it than the other is from the
    /// body's waypoint (of two level, the later in (y, x)). It steps
    /// [`GIVE_STEP`] back from the other for [`GIVE_TICKS`]. Neither moves
    /// otherwise: `unpin_yaw` traces the world only. Two bots off mp_ship's
    /// ladder at x 3792 onto the plate at x 3800, at y 45 and y 79, each
    /// held the other out of the hatch's lane at y 58-72 for 180 ticks
    /// (seed 3223, bot-navigation.md section 3). `off_rungs` as in
    /// [`Self::ladder_yield`].
    pub fn pressed_yield(
        &mut self,
        at: Vec3,
        waypoint: Vec3,
        body: Footing,
        off_rungs: &[(Vec3, Vec3)],
    ) -> Option<Vec3> {
        use vcod_common::pmove::{HALF_WIDTH, STEPSIZE};
        if let Some((spot, left)) = self.give {
            self.give = (left > 1).then_some((spot, left - 1));
            return Some(spot);
        }
        let slow = |v: Vec3| v.truncate().length() < UNPIN_SPEED;
        let flat = |p: Vec3| (p - waypoint).truncate().length();
        let me = flat(at);
        let other = (body.on_ground && !body.on_ladder && slow(body.velocity) && me < 2.0 * REACH)
            .then(|| {
                off_rungs.iter().find(|&&(o, v)| {
                    slow(v)
                        && (o - at).truncate().length() < 2.0 * HALF_WIDTH + 2.0
                        && (o.z - at.z).abs() < STEPSIZE
                })
            })
            .flatten();
        let Some(&(o, _)) = other else {
            self.pressed_ticks = 0;
            return None;
        };
        self.pressed_ticks += 1;
        let them = flat(o);
        let gives = me > them + 1.0 || ((me - them).abs() <= 1.0 && (at.y, at.x) > (o.y, o.x));
        if self.pressed_ticks < PRESSED_TICKS || !gives {
            return None;
        }
        self.pressed_ticks = 0;
        let back = (at - o).truncate().normalize_or_zero().extend(0.0);
        let spot = at + back * GIVE_STEP;
        self.give = Some((spot, GIVE_TICKS - 1));
        Some(spot)
    }

    fn yield_spot(
        &self,
        g: &NavGraph,
        at: Vec3,
        body: Footing,
        climbers: &[(Vec3, Vec3)],
        off_rungs: &[(Vec3, Vec3)],
    ) -> Option<(Vec3, bool, bool)> {
        use vcod_common::pmove::{HALF_WIDTH, JUMP_HEIGHT, STEPSIZE};
        let waiting = self.yield_ticks > 0;
        if body.on_ladder || self.yield_rest > 0 || (climbers.is_empty() && !waiting) {
            return None;
        }
        // The edge under way, else the one on from the waypoint.
        let i = self.next;
        let (e, f, under_way) = [i.checked_sub(1).map(|h| (h, i)), Some((i, i + 1))]
            .into_iter()
            .flatten()
            .filter_map(|(a, b)| Some((*self.path.get(a)?, *self.path.get(b)?, b == i)))
            .find(|&(a, b, _)| {
                (g.nodes[b as usize].z - g.nodes[a as usize].z).abs() > JUMP_HEIGHT
            })?;
        let (end, far) = (g.nodes[e as usize], g.nodes[f as usize]);
        // The nearest: the crow's nest's ladder on mp_ship has another
        // under the deck at its foot within reach of both ends.
        let near_both = |b: &&(Vec3, Vec3)| box_flat(b, end) + box_flat(b, far);
        let rungs = *g
            .ladder_boxes
            .iter()
            .filter(|b| box_flat(b, end) < LADDER_NEAR && box_flat(b, far) < LADDER_NEAR)
            .min_by(|a, b| near_both(a).total_cmp(&near_both(b)))?;
        // A wait under way holds at its spot, which may stand farther off.
        if (at.z - end.z).abs() > STEPSIZE || (!waiting && box_flat(&rungs, at) > YIELD_NEAR) {
            return None;
        }
        // At the head of a ladder a fall off kills, the bot backs down
        // onto a climber coming up from [`HEAD_ON`] under it: from 70 to
        // 100 under mp_ship's crow's nest plate, the back down met the
        // climber's head and slid off the rungs down the hatch
        // (bot-navigation.md section 3).
        let deadly_head = end.z - far.z > self.deadly_fall();
        let close = if deadly_head { HEAD_ON } else { CLIMBER_CLOSE };
        let floor = |m: &u32| (g.nodes[*m as usize].z - end.z).abs() < STEPSIZE;
        let farthest = |ms: &mut dyn Iterator<Item = u32>| {
            ms.map(|m| g.nodes[m as usize])
                .max_by(|a, b| box_flat(&rungs, *a).total_cmp(&box_flat(&rungs, *b)))
        };
        let near = || g.edges[e as usize].iter().copied().filter(floor);
        let on = |ms: &[u32]| -> Vec<u32> {
            ms.iter()
                .flat_map(|&m| g.edges[m as usize].iter().copied().filter(floor))
                .collect()
        };
        let two = on(&near().collect::<Vec<_>>());
        let three = if deadly_head { on(&two) } else { Vec::new() };
        // Where a fall off kills, a body's width clear of every exit too: a
        // bot waiting 21 off the crow's nest's exit held the climber on the
        // plate over the hatch.
        let exits: Vec<Vec3> = near().map(|m| g.nodes[m as usize]).collect();
        let clear = |m: &u32| {
            let q = g.nodes[*m as usize];
            !deadly_head || exits.iter().all(|x| x.distance(q) >= 2.0 * HALF_WIDTH)
        };
        // A neighbour clear of the rungs or, where none is, the node two
        // steps off on the same floor farthest from them, or at such a head
        // three: the crow's nest's head on mp_ship has its exits 26 from the
        // ladder and the nodes past them 29, and with no spot the bot backed
        // down onto the climber.
        let spot = farthest(&mut near())
            .filter(|q| box_flat(&rungs, *q) >= YIELD_CLEAR)
            .or_else(|| farthest(&mut two.iter().copied().filter(clear)))
            .or_else(|| farthest(&mut three.iter().copied().filter(clear)))
            .filter(|q| box_flat(&rungs, *q) > box_flat(&rungs, end) + HALF_WIDTH)?;
        let mut coming = climbers
            .iter()
            .filter(|&&(c, v)| {
                box_flat(&rungs, c) < CLIMBER_NEAR
                    && c.z > rungs.0.z - STEPSIZE
                    && c.z < rungs.1.z + STEPSIZE
                    && (end.z - c.z).abs() < close
                    // Or bobbing under such a head: the rungs let go under
                    // the crow's nest plate and the body falls back onto them.
                    && ((end.z - c.z) * v.z >= 0.0 || (deadly_head && end.z - c.z < JUMP_HEIGHT))
            })
            .map(|&(_, v)| v.z.abs() > 1.0)
            .peekable();
        // A wait under way at such a head lasts while a body off the rungs
        // stands, or bobs, between the spot and the rungs: let go when the
        // climber stepped off, mp_ship's waiting bots walked back onto the
        // crow's nest's exit and shoved it off the plate down the hatch
        // (bot-navigation.md section 3).
        // Not one pressed against the waiter where a fall off is no death:
        // it stepped off and is on its way out through the spot. At mp_ship's
        // x 3793 head the climber pressed on the bot waiting at
        // (3751, -4, 616) on its way south until the wait ran out (seed 761).
        let mut on_head = off_rungs
            .iter()
            .filter(|&&(c, _)| {
                waiting
                    && (c.z - end.z).abs() < STEPSIZE
                    && box_flat(&rungs, c) < box_flat(&rungs, spot)
                    && (deadly_head || (c - at).truncate().length() > 2.0 * HALF_WIDTH + 2.0)
            })
            .map(|&(_, v)| v.truncate().length() > 1.0)
            .peekable();
        if coming.peek().is_none() && on_head.peek().is_none() {
            return None;
        }
        let moving = coming.chain(on_head).any(|m| m);
        // A body well past the head toward the rungs (backing down over
        // the lip) steps back onto the head first: headed straight for the
        // spot, a bot on mp_ship's hatch at (3806, 60, 623) slid along the
        // hatch's side and fell through it to the deck at 304
        // (bot-navigation.md section 3).
        // Not from beside the head: a bot that came to rest 5 units past
        // mp_ship's (3337, -440, 56) toward its rungs went on asking for
        // the head and stood in the climber's way off it (seed 3769).
        let p = if box_flat(&rungs, at) + HALF_WIDTH < box_flat(&rungs, end) {
            end
        } else {
            spot
        };
        Some((p, moving, under_way))
    }

    /// The bot's own stall circle caught the body on the way to the
    /// current waypoint and sent it on an unstick spell: plan round the
    /// edge under way. The spell runs the body off the path, and the plan
    /// from where it ends took the same edge back into the same trap
    /// (mp_ship's hold crates at (2234, 494, -42), bot-navigation.md
    /// section 3).
    pub fn stuck(&mut self) {
        if let Some(from) = self.next.checked_sub(1).and_then(|i| self.path.get(i))
            && let Some(&to) = self.path.get(self.next)
        {
            self.avoid_edge(*from, to);
            self.reset();
            self.fell = true;
            self.gave_up += 1;
        }
    }

    /// Test-facing: the ticks of the wait aside for a climber under way.
    pub fn yielding(&self) -> u32 {
        self.yield_ticks
    }

    /// Test-facing: the current waypoint's node and the one after it.
    /// The edges of the path still ahead, the one under way first.
    pub fn ahead(&self) -> impl Iterator<Item = (u32, u32)> + '_ {
        self.path
            .get(self.next.saturating_sub(1)..)
            .unwrap_or(&[])
            .windows(2)
            .map(|w| (w[0], w[1]))
    }

    pub fn current(&self) -> (Option<u32>, Option<u32>) {
        (
            self.path.get(self.next).copied(),
            self.path.get(self.next + 1).copied(),
        )
    }

    /// Whether the current waypoint is a leap's foot, or the node of an edge
    /// the body comes onto too fast ([`Self::stop_at`], [`Self::held`]): the bot comes to
    /// rest on it before it heads on.
    pub fn holding(&self, g: &NavGraph) -> bool {
        match (self.path.get(self.next), self.path.get(self.next + 1)) {
            (Some(&a), Some(&b)) => {
                g.leaps.contains(&(a, b))
                    || self.stop_at == Some((a, b))
                    || self.held == Some((a, b))
            }
            _ => false,
        }
    }

    /// Whether the current waypoint is the top of a creep that misses off a
    /// turn ([`Self::creeps_off`]), which the bot slows onto before it backs
    /// off: the graph's walk crept from the node with a run straight at the
    /// drop behind it, and a bot that came onto mp_depot's roof drop off a
    /// turn left the lip at 81 units/s to the walk's 14 and fell past the
    /// ledge. Re-proved at the body's pace each tick instead, the creep
    /// read as arriving by a floor 3 under the ledge that the bot's own
    /// fall missed; held at every creep's top, bots slowed onto mp_ship's
    /// lip node (5402, 321, 344) from 8 over it and bounced off the stair
    /// below (bot-navigation.md section 3).
    fn creep_top(&mut self, world: &CollisionWorld, g: &NavGraph) -> bool {
        if !self.creeps_off(world, g) {
            return false;
        }
        self.stop_at = Some((self.path[self.next], self.path[self.next + 1]));
        true
    }

    /// Whether the way on from the current waypoint is a drop more than a
    /// jump deep that neither a run nor a back down from the node arrives
    /// at, so the graph's walk crept it ([`Gait::Creep`]), that misses
    /// coming onto it off a turn ([`creep_takes_a_turn`]) and arrives from
    /// rest: where a stop helps. Held at every creep that misses off a
    /// turn, mp_ship's bots slowed onto its lip node (5402, 321, 344) and
    /// bounced off the stair below, and stalled about twice as long over
    /// the sweep (bot-navigation.md section 3). A body
    /// that comes onto it off a turn carries its pace over the lip and past
    /// the grab: mp_depot's thin ledges at (-2749, -1281, 326) and
    /// (-2755, -1150, 326), 182 under the roof (bot-navigation.md
    /// section 3). So is a creep that arrives only from rest: the back down
    /// off mp_ship's mast head (3675, -95.1, 1147.1) on the spar beside the
    /// rungs' top. Walked once per edge.
    fn creeps_off(&mut self, world: &CollisionWorld, g: &NavGraph) -> bool {
        let (Some(&a), Some(&b)) = (self.path.get(self.next), self.path.get(self.next + 1)) else {
            return false;
        };
        if let Some((e, creep)) = self.creep_off
            && e == (a, b)
        {
            return creep;
        }
        let (from, to) = (g.nodes[a as usize], g.nodes[b as usize]);
        let arrives = |pace, gait| {
            matches!(
                walk_with(world, from, pace, to.truncate(), Some(to.z), gait),
                Walked::Arrived(..)
            )
        };
        // Or only from rest: a ladder's back down `rung` proved that way.
        let creep = to.z < from.z - vcod_common::pmove::JUMP_HEIGHT
            && !self.runs_off(world, (a, b), from, to)
            && arrives(Some(Vec3::ZERO), Gait::Creep)
            && ((!arrives(None, Gait::Backward)
                && !creep_takes_a_turn(world, from, to.truncate(), Some(to.z)))
                || !arrives(None, Gait::Creep));
        self.creep_off = Some(((a, b), creep));
        creep
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

    /// Up a ladder from a foot square in front of the rungs, the foot: the
    /// climb that proved the head went up level with it along the face,
    /// and a body that grabbed the rungs off that line strafes back to it
    /// (`bots::Bot::steer`).
    pub fn climb_line(&self, g: &NavGraph) -> Option<glam::Vec2> {
        if !self.climbing(g) {
            return None;
        }
        let foot = g.nodes[self.path[self.next - 1] as usize];
        g.face_ahead(foot).map(|_| foot.truncate())
    }

    /// Down a ladder whose fall kills, from its head: the yaw square into
    /// the rungs' face and the middle of the rungs along it. The creep off
    /// the head backs along that yaw strafing to the middle
    /// (`bots::Bot::steer`). Backed toward the foot, as the graph's walk
    /// did, it grabs mp_ship's crow's nest rungs only from a diamond about
    /// 4 units across round the head; a bot that came onto the head off
    /// the line, or along the rim, fell down the hatch past them
    /// (bot-navigation.md section 3).
    pub fn back_face(&self, g: &NavGraph) -> Option<(f32, glam::Vec2)> {
        let i = self.next.checked_sub(1)?;
        let (head, foot) = (
            g.nodes[*self.path.get(i)? as usize],
            g.nodes[*self.path.get(self.next)? as usize],
        );
        if foot.z > head.z - self.deadly_fall() {
            return None;
        }
        let (lo, hi) = *g
            .ladder_boxes
            .iter()
            .filter(|b| box_flat(b, head) < LADDER_NEAR && box_flat(b, foot) < LADDER_NEAR)
            .min_by(|a, b| {
                (box_flat(a, head) + box_flat(a, foot))
                    .total_cmp(&(box_flat(b, head) + box_flat(b, foot)))
            })?;
        let ext = hi - lo;
        // The thin axis is across the face; the foot stands on the
        // climber's side of it.
        let (a, b) = if ext.x < ext.y { (0, 1) } else { (1, 0) };
        let into = ((lo[a] + hi[a]) * 0.5 - foot[a]).signum();
        let yaw = match (a, into > 0.0) {
            (0, true) => 0.0,
            (0, false) => 180.0,
            (_, true) => 90.0,
            (_, false) => -90.0,
        };
        let mut middle = foot.truncate();
        middle[b] = (lo[b] + hi[b]) * 0.5;
        Some((yaw, middle))
    }

    /// Whether the edge on from the current waypoint climbs more than a
    /// jump: the waypoint is a ladder's foot.
    fn rising(&self, g: &NavGraph) -> bool {
        match (self.path.get(self.next), self.path.get(self.next + 1)) {
            (Some(&a), Some(&b)) => {
                g.nodes[b as usize].z > g.nodes[a as usize].z + vcod_common::pmove::JUMP_HEIGHT
            }
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

    /// Whether the way to the current waypoint climbs more than a jump: a
    /// ladder's foot to its head.
    fn climbing(&self, g: &NavGraph) -> bool {
        let Some(i) = self.next.checked_sub(1) else {
            return false;
        };
        match (self.path.get(i), self.path.get(self.next)) {
            (Some(&a), Some(&b)) => {
                g.nodes[b as usize].z > g.nodes[a as usize].z + vcod_common::pmove::JUMP_HEIGHT
            }
            _ => false,
        }
    }

    /// How far the plan runs on from `at` along a straight, level line of
    /// plain edges: every node within [`SPRINT_SLACK`] of the line from
    /// `at` to the last, each step under a stair's rise, no jump, leap or
    /// risky edge. Where a United Offensive bot sprints (bot-navigation.md,
    /// "Other rules"): the graph's walks ran at 190 units/s, and a sprint's
    /// 304 off a turn or a drop flies a body past what they proved.
    pub fn straight_run(&self, g: &NavGraph, at: Vec3) -> f32 {
        if self.arrived {
            return 0.0;
        }
        let ahead = &self.path[self.next.min(self.path.len())..];
        let mut pts = vec![at];
        let mut prev = self
            .next
            .checked_sub(1)
            .and_then(|i| self.path.get(i).copied());
        for &n in ahead.iter().take(SPRINT_LOOK) {
            let p = g.nodes[n as usize];
            let plain = prev.is_none_or(|m| {
                !g.jumps.contains(&(m, n))
                    && !g.leaps.contains(&(m, n))
                    && !g.risky.contains(&(m, n))
            });
            if !plain || (p.z - pts[pts.len() - 1].z).abs() > vcod_common::pmove::STEPSIZE {
                break;
            }
            let line = (p - at).truncate();
            let len = line.length();
            let across = |q: &Vec3| {
                let d = (*q - at).truncate();
                if len < 1.0 {
                    d.length()
                } else {
                    (d.x * line.y - d.y * line.x).abs() / len
                }
            };
            if pts.iter().any(|q| across(q) > SPRINT_SLACK) {
                break;
            }
            pts.push(p);
            prev = Some(n);
        }
        pts.windows(2)
            .map(|w| (w[1] - w[0]).truncate().length())
            .sum()
    }

    /// Whether the way to the current waypoint, a drop of more than
    /// `deep`, is taken at a run rather than backed onto: whether the
    /// graph's walk proved it running off ([`Self::runs_off`]). The pass
    /// onto it re-proved the run at the body's pace (`edge_holds`).
    pub fn running_off(
        &mut self,
        g: &NavGraph,
        world: Option<&CollisionWorld>,
        at: Vec3,
        deep: f32,
    ) -> bool {
        let (Some(world), Some(i)) = (world, self.next.checked_sub(1)) else {
            return false;
        };
        let (Some(&a), Some(&b)) = (self.path.get(i), self.path.get(self.next)) else {
            return false;
        };
        let (from, to) = (g.nodes[a as usize], g.nodes[b as usize]);
        to.z < at.z - deep && self.runs_off(world, (a, b), from, to)
    }

    /// Whether a run that misses the drop `edge` onto `to` can fall far
    /// enough to hurt ([`a_miss_hurts`]). Probed once per edge.
    fn miss_hurts(&mut self, world: &CollisionWorld, edge: (u32, u32), to: Vec3) -> bool {
        if let Some((e, hurts)) = self.miss_hurt
            && e == edge
        {
            return hurts;
        }
        let hurts = a_miss_hurts(world, to);
        self.miss_hurt = Some((edge, hurts));
        hurts
    }

    /// Whether a forward walk from `from` arrives on `to`'s floor without a
    /// jump: the first gait the graph's walk tries (`walk_to`), so a drop
    /// it misses was backed down. Walked once per edge.
    fn runs_off(&mut self, world: &CollisionWorld, edge: (u32, u32), from: Vec3, to: Vec3) -> bool {
        if let Some((e, run)) = self.run_off
            && e == edge
        {
            return run;
        }
        let run = matches!(
            walk_as(world, from, to.truncate(), Some(to.z), Gait::Forward),
            Walked::Arrived(_, false)
        );
        self.run_off = Some((edge, run));
        run
    }

    /// Whether the way to the current waypoint is a jump edge, or the way
    /// from the body to a plan's first node that only a jump walked
    /// ([`Self::jump_start`]).
    pub fn jumping(&self, g: &NavGraph) -> bool {
        let Some(i) = self.next.checked_sub(1) else {
            return self.jump_start && !self.path.is_empty();
        };
        match (self.path.get(i), self.path.get(self.next)) {
            (Some(&a), Some(&b)) => g.jumps.contains(&(a, b)),
            _ => false,
        }
    }

    /// Whether the way to the current waypoint is a mantle edge
    /// ([`NavGraph::mantles`]).
    pub fn mantling(&self, g: &NavGraph) -> bool {
        self.mantle_edge(g).is_some()
    }

    /// The mantle edge the current waypoint ends, if it is one.
    pub fn mantle_edge(&self, g: &NavGraph) -> Option<(u32, u32)> {
        let i = self.next.checked_sub(1)?;
        let e = (*self.path.get(i)?, *self.path.get(self.next)?);
        g.mantles.contains(&e).then_some(e)
    }

    /// On a mantle edge, where its probe landed ([`NavGraph::mantle_lands`]).
    pub fn mantle_land(&self, g: &NavGraph) -> Option<Vec3> {
        g.mantle_lands.get(&self.mantle_edge(g)?).copied()
    }

    /// Whether the bot jumps on the way to the current waypoint: a jump
    /// edge, unless its walk on from where the body passed the edge's
    /// first node ran it without the jump ([`Self::walked_jump`]).
    pub fn jump_cued(&self, g: &NavGraph) -> bool {
        let into = self
            .next
            .checked_sub(1)
            .and_then(|i| Some((*self.path.get(i)?, *self.path.get(self.next)?)));
        self.jumping(g) && (self.walked_jump.is_none() || self.walked_jump != into)
    }

    /// On a jump edge proved aimed beside its top ([`NavGraph::aims`]),
    /// the point to steer at in place of the top.
    pub fn jump_aim(&self, g: &NavGraph) -> Option<glam::Vec2> {
        let i = self.next.checked_sub(1)?;
        let (a, b) = (*self.path.get(i)?, *self.path.get(self.next)?);
        g.aims.get(&(a, b)).copied()
    }

    /// Whether the way to the current waypoint is a crouched jump edge: the
    /// bot holds crouch from the cmd after its jump until it lands, as
    /// [`Gait::CrouchJump`] proved it.
    pub fn crouch_jumping(&self, g: &NavGraph) -> bool {
        let Some(i) = self.next.checked_sub(1) else {
            return false;
        };
        match (self.path.get(i), self.path.get(self.next)) {
            (Some(&a), Some(&b)) => g.crouch_jumps.contains(&(a, b)),
            _ => false,
        }
    }

    /// On a jump edge with the body's jump still cooling down
    /// (`PM_CheckJump` refuses one inside `JUMP_COOLDOWN_MS` of the last)
    /// or under 1.5's penalty ([`jump_ready`]), the edge's foot to come to
    /// rest on until it is ready: the graph's walk proved the edge with a
    /// full jump to hand. A bot that jumped onto
    /// mp_depot's crate at (-2275, 131, 46) ran on for the rim at 66 with
    /// the jump refused and off the crate's edge into the open crate below
    /// (bot-navigation.md section 3).
    pub fn jump_wait(&self, g: &NavGraph, at: [f32; 3], ps: &PlayerState) -> Option<[f32; 3]> {
        if !self.jumping(g) || !ps.on_ground || ps.on_ladder || jump_ready(ps) {
            return None;
        }
        // A jump start's foot is where the body stands.
        let Some(i) = self.next.checked_sub(1) else {
            return Some(at);
        };
        let foot = g.nodes[self.path[i] as usize];
        let rise = at[2] - foot.z;
        if rise.abs() >= vcod_common::pmove::STEPSIZE {
            return None;
        }
        // Stepped up off the foot's floor onto a ledge the top still needs
        // a jump from, it waits where it stands: the walk jumped from up
        // there too, and one sent back down off mp_cassino's ledge at
        // (3667, -1240, 128) jumped again from the foot 53 under the top,
        // into the penalty, and back for good (bot-navigation.md "Jumps").
        let top = g.nodes[self.path[self.next] as usize];
        let ledge = rise > ARRIVE && top.z - at[2] > vcod_common::pmove::STEPSIZE;
        Some(if ledge { at } else { foot.into() })
    }

    /// Whether the jump edge [`Self::jump_wait`] holds the body at `spot`
    /// for fails from there at rest, where the wait leaves it: then the
    /// plan is dropped and the edge avoided. The graph proved the edge
    /// with a running start, and mp_cassino's from the roof at (4992,
    /// -1338, 295) to (4938, -1393, 303) clears the trench at 256 between
    /// them only so: a UO bot that waited out the penalty there walked
    /// into the trench and jumped at the roof's face 47 over it until it
    /// gave up (bot-objectives.md 10.6). An edge already avoided had no
    /// way round and is waited on. A jump start has no edge and is kept.
    pub fn rest_jump_fails(
        &mut self,
        g: &NavGraph,
        world: &CollisionWorld,
        spot: [f32; 3],
    ) -> bool {
        let (Some(i), Some(&b)) = (self.next.checked_sub(1), self.path.get(self.next)) else {
            return false;
        };
        let edge = (self.path[i], b);
        if self.avoid.iter().any(|e| (e.0, e.1) == edge) {
            return false;
        }
        let lands = match self.rest_jump {
            Some((e, lands)) if e == edge => lands,
            _ => {
                let top = g.nodes[b as usize];
                let gait = if g.crouch_jumps.contains(&edge) {
                    Gait::CrouchJump
                } else {
                    Gait::Forward
                };
                let walked = as_a_bot(|| {
                    walk_with(
                        world,
                        spot.into(),
                        Some(Vec3::ZERO),
                        g.aim(edge.0, edge.1),
                        Some(top.z),
                        gait,
                    )
                });
                let lands = matches!(walked, Walked::Arrived(..));
                self.rest_jump = Some((edge, lands));
                lands
            }
        };
        if lands {
            return false;
        }
        self.avoid_edge(edge.0, edge.1);
        self.reset();
        true
    }

    /// String-pulling: takes the waypoint on to the next node of the plan
    /// when a body walks there straight from where it stands
    /// ([`straight_floor`], then a walk from the body at its pace that
    /// arrives without a jump, a dip or a slide), so a bot cuts the grid's
    /// 45 degree kinks. Only along a run of plain edges
    /// ([`NavGraph::plain_edge`]) whose last node leaves by one too: a
    /// jump, leap, ladder, drop or risky edge is still come onto along the
    /// edge the graph proved. One node a tick, no further than
    /// [`PULL_STEPS`] grid steps and a step's height away, every node
    /// skipped within [`PULL_SLACK`] steps of the line, on the ground
    /// past the plan's first node. While a pulled waypoint is ahead the
    /// straight line to it is checked again every tick; a failed check
    /// falls back to the furthest skipped node it still reaches
    /// (bot-navigation.md, "String-pulling").
    fn pull(&mut self, g: &NavGraph, world: Option<&CollisionWorld>, here: Vec3, body: Footing) {
        let Some(world) = world else {
            return;
        };
        if !body.on_ground || body.on_ladder || self.arrived || self.next >= self.path.len() {
            return;
        }
        let node = |i: usize| g.nodes[self.path[i] as usize];
        if let Some(base) = self.pull_base
            && !straight_floor(world, here, node(self.next))
        {
            let back = (base + 1..self.next)
                .rev()
                .find(|&j| straight_floor(world, here, node(j)));
            self.next = back.unwrap_or(base);
            if back.is_none() {
                self.pull_base = None;
            }
            self.best = f32::INFINITY;
            self.idle = 0;
            self.pull_wait = PULL_RETRY;
            return;
        }
        if self.pull_wait > 0 {
            self.pull_wait -= 1;
            return;
        }
        let k = self.next + 1;
        if self.next == 0 || k >= self.path.len() || self.held.is_some() {
            return;
        }
        let plain = |i: usize| {
            i + 1 >= self.path.len()
                || (g.plain_edge(self.path[i], self.path[i + 1])
                    && !self
                        .avoid
                        .iter()
                        .any(|e| (e.0, e.1) == (self.path[i], self.path[i + 1])))
        };
        if !(plain(self.next - 1) && plain(self.next) && plain(k)) {
            return;
        }
        let to = node(k);
        let line = (to - here).truncate();
        if line.length() > PULL_STEPS * g.spacing
            || (to.z - here.z).abs() > vcod_common::pmove::STEPSIZE
        {
            return;
        }
        // The node skipped stays near the line: off it the grid went round
        // something no trace sees (a hazard's edge, a fall beside a run).
        let skipped = (node(self.next) - here).truncate();
        let t = (skipped.dot(line) / line.length_squared().max(1.0)).clamp(0.0, 1.0);
        if (skipped - line * t).length() > PULL_SLACK * g.spacing {
            return;
        }
        let mut trail = Trail::default();
        let walked = straight_floor(world, here, to)
            && matches!(
                walk_sliding(
                    world,
                    here,
                    Some(body.velocity),
                    self.move_scale,
                    to.truncate(),
                    Some(to.z),
                    Gait::Forward,
                    &mut trail,
                ),
                Walked::Arrived(_, false)
            )
            && trail.dip <= vcod_common::pmove::STEPSIZE
            && trail.slid.is_empty();
        if !walked {
            self.pull_wait = PULL_RETRY;
            return;
        }
        self.pull_base.get_or_insert(self.next);
        self.next = k;
        self.best = f32::INFINITY;
        self.idle = 0;
    }

    /// Where a walked-out point goal is headed: the point, or the
    /// destination node when [`NavGraph::goal_node`] kept it off a perch
    /// the point stands on.
    fn arrival(&self, g: &NavGraph, p: [f32; 3]) -> [f32; 3] {
        match (self.dest, g.nearest(p)) {
            (Some(d), Some(n)) if d != n && g.perches.contains(&n) => g.nodes[d as usize].into(),
            _ => p,
        }
    }

    /// Whether the plan to the point goal is walked out: the body heads
    /// straight at the point ([`Self::arrival`]).
    pub fn walked_out(&self) -> bool {
        self.arrived
    }

    /// Whether the last plan found the body hemmed in by parked vehicles
    /// ([`Self::blocked`]) with no way out round them: what the bot reads
    /// as stranded (`bots::BotView::stranded`).
    pub fn penned(&self) -> bool {
        self.penned
    }

    /// Whether a parked vehicle stands over node `n` ([`Self::blocked`]).
    fn blocks(&self, n: u32) -> bool {
        self.blocked.binary_search(&n).is_ok()
    }

    /// Plans round the edge `a` to `b` for [`AVOID_TICKS`]. One stuck on
    /// again while still avoided was planned through for want of a way
    /// round and failed again: no plan takes it until it expires. Taken
    /// again and again, a bot paced the slalom on mp_ship's deck at (3565,
    /// 95, 352) for most of a UO match: its way out north is proved only
    /// with a running start, and no turn into it gives one
    /// (bot-combat.md 17.1).
    fn avoid_edge(&mut self, a: u32, b: u32) {
        let until = self.clock + AVOID_TICKS;
        if self.avoid.iter().any(|e| (e.0, e.1) == (a, b)) {
            self.dead.push((a, b, until));
        }
        self.avoid.push((a, b, until));
    }

    fn reset(&mut self) {
        self.held = None;
        self.pull_base = None;
        self.dest = None;
        self.search = None;
        self.path.clear();
        self.arrived = false;
    }
}

impl NavGraph {
    /// Whether the edge `a` to `b` is a plain walk on one floor: no jump,
    /// leap, crouched jump or risky drop, and no more than a step between
    /// its ends. What [`Follower::pull`] cuts across.
    pub fn plain_edge(&self, a: u32, b: u32) -> bool {
        let e = (a, b);
        !self.jumps.contains(&e)
            && !self.leaps.contains(&e)
            && !self.crouch_jumps.contains(&e)
            && !self.risky.contains(&e)
            && !self.mantles.contains(&e)
            && (self.nodes[a as usize].z - self.nodes[b as usize].z).abs()
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
    fn face_ahead(&self, foot: Vec3) -> Option<glam::Vec2> {
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
    fn roam_pool(&self, start: u32) -> Option<&[u32]> {
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
    fn roam_from(&self, start: u32, from: Vec3, rand: &mut dyn FnMut() -> i32) -> Option<u32> {
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
    from: u32,
    to: u32,
    avoid: Vec<(u32, u32)>,
    crowd: Vec<(u32, u32)>,
    crowd_cost: f32,
    noise: u64,
    blocked: Arc<[u32]>,
    /// Nodes another body stands over ([`Follower::bodies_in_way`]):
    /// passed through only by the plan after this one fails.
    shun: Arc<[u32]>,
    jump_cost: f32,
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
    fn reached(&self) -> usize {
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
    fn flee_from(
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

thread_local! {
    /// Set on a [`NavJob`]'s thread: its [`par_map`]s leave one core free.
    static SPARE_CORE: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    /// The movement rules every walk's body moves by ([`with_rules`]).
    static RULES: std::cell::Cell<MoveRules> = const { std::cell::Cell::new(MoveRules::V1_1) };
    /// A game's own step every walk's body moves by in place of CoD1's
    /// pmove ([`with_step`]).
    static STEP: std::cell::RefCell<Option<Arc<dyn WalkStep>>> = const { std::cell::RefCell::new(None) };
    /// Set inside [`walk_to`]: a forward run that stalls bobbing on a steep
    /// face jumps on its next tick on the ground ([`walk_sliding`]).
    static BOB: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    /// Set inside [`as_a_bot`]: a pinned walk jumps without
    /// [`jump_clear`]'s room test, as a bot on a jump edge does.
    static ROOMLESS: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Runs the walk `f` with a bot's jump on a jump edge: out of a bob too
/// ([`BOB`]), and pinned with no room test ([`ROOMLESS`]).
fn as_a_bot<R>(f: impl FnOnce() -> R) -> R {
    BOB.set(true);
    ROOMLESS.set(true);
    let r = f();
    BOB.set(false);
    ROOMLESS.set(false);
    r
}

/// Runs `f` with every walk's body moving by `rules`: a graph built for
/// a 1.5 or UO server is proved under that server's pmove, and the
/// follower's walks again from where a bot stands under the same
/// (bot-navigation.md, "Other rules").
pub fn with_rules<R>(rules: MoveRules, f: impl FnOnce() -> R) -> R {
    let was = RULES.replace(rules);
    let r = f();
    RULES.set(was);
    r
}

/// What a [`WalkStep`] keeps of a walk's body between its cmds, beside the
/// [`PlayerState`] the walk reads: the game's own playerstate.
pub type WalkCarry = Option<Arc<dyn std::any::Any + Send + Sync>>;

/// A game's own move for the walks' bodies, in place of CoD1's pmove
/// under [`MoveRules`]: CoD2's (`crate::bots::cod2::Cod2Walk`), so its
/// graph is proved by a body that moves as its bots do
/// (bot-navigation.md, "Other rules").
pub trait WalkStep: Send + Sync {
    /// Moves `ps` by one cmd over `world`, its clock from `from_ms` to
    /// `cmd.server_time`. The view is `cmd.angles` plus `delta`, which the
    /// step may push as `delta_angles`; `weapons` holds the def of
    /// `ps.weapon`. `carry` is what the step kept after the walk's last
    /// cmd, `None` on its first.
    #[allow(clippy::too_many_arguments)]
    fn step(
        &self,
        ps: &mut PlayerState,
        delta: &mut [i32; 3],
        carry: &mut WalkCarry,
        from_ms: i32,
        cmd: &UserCmd,
        world: &CollisionWorld,
        weapons: &[Option<vcod_common::weapon::WeaponDef>],
    );

    /// Tells graphs proved by two steps apart in the cache.
    fn key(&self) -> String;
}

/// Runs `f` with every walk's body moving by `step`, `None` for CoD1's
/// pmove under the rules in force ([`with_rules`]).
pub fn with_step<R>(step: Option<Arc<dyn WalkStep>>, f: impl FnOnce() -> R) -> R {
    let was = STEP.replace(step);
    let r = f();
    STEP.set(was);
    r
}

/// The walks' rules and step in force on a thread ([`with_rules`],
/// [`with_step`]), carried to a worker thread that walks for it: the bots'
/// pass plans on workers under the ticking thread's (`bots::driver::par`).
#[derive(Clone)]
pub(crate) struct WalkRules(MoveRules, Option<Arc<dyn WalkStep>>);

impl WalkRules {
    /// This thread's.
    pub(crate) fn here() -> WalkRules {
        WalkRules(RULES.get(), own_step())
    }

    /// Runs `f` under them.
    pub(crate) fn enter<R>(&self, f: impl FnOnce() -> R) -> R {
        with_rules(self.0, || with_step(self.1.clone(), f))
    }
}

/// The step in force ([`with_step`]).
fn own_step() -> Option<Arc<dyn WalkStep>> {
    STEP.with_borrow(Clone::clone)
}

/// A body standing at `p` facing `yaw`, moving by the rules in force.
fn body(p: Vec3, yaw: f32) -> PlayerState {
    let mut ps = PlayerState::spawn(p, yaw);
    ps.rules = RULES.get();
    ps
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
    let rules = RULES.get();
    let step = own_step();
    std::thread::scope(|s| {
        let handles: Vec<_> = (0..threads.min(items.len()))
            .map(|_| {
                let (step, next, f) = (step.clone(), &next, &f);
                s.spawn(move || {
                    RULES.set(rules);
                    STEP.set(step);
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
/// world model and of every submodel linked where it spawned (mp_italy's
/// allied `bas` bunker is left by the two in its base's `*4`,
/// bot-navigation.md section 2), and a world clip's (CoD2's, which keeps
/// model 0's brushes out of [`CollisionWorld::brushes`]), with touching
/// ones merged, since a tall ladder is often several brushes stacked.
fn ladders(world: &CollisionWorld) -> Vec<(Vec3, Vec3)> {
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
fn stands(world: &CollisionWorld, p: Vec3) -> bool {
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

/// Whether a standing body with its feet at `p` touches one of `hazards`.
pub(crate) fn hazard(hazards: &[BrushHull], p: Vec3) -> bool {
    use vcod_common::pmove::{HALF_WIDTH, Stance};
    let half = Vec3::new(HALF_WIDTH, HALF_WIDTH, Stance::Stand.height() * 0.5);
    !hazards.is_empty() && box_contacts_hulls(p + Vec3::Z * half.z, half, Vec3::ZERO, hazards)
}

/// A wall whose normal meets a heading under this cosine (against it, more
/// than 45 degrees off square) is slid along, not stopped at.
const GRAZE: f32 = std::f32::consts::FRAC_1_SQRT_2;
/// How far apart along a heading [`drop_ahead`] feels for the floor.
const DROP_SAMPLE: f32 = 16.0;

/// How deep the first drop deeper than a jump (one a body cannot come
/// back up) is that a body standing at `p`, walking `look` units along the
/// flat unit vector `dir`, meets before a wall stops it; infinite past
/// the default `bg_fallDamageMinHeight`, where a fall starts to hurt.
/// Floors are felt every [`DROP_SAMPLE`], each against the one before, so
/// a stair down is no drop unless it runs more than a jump down within
/// the look. Each is felt with the standing capsule: one
/// standing on a lip it overhangs is held by it.
/// A floor too steep to stand on whose spot drains into a crease
/// ([`Creases`]) reads as a drop that hurts: a body sent down it never
/// comes out (mp_hurtgen's crease, bot-navigation.md section 3).
pub(crate) fn drop_ahead(
    world: &CollisionWorld,
    p: Vec3,
    dir: Vec3,
    look: f32,
    creases: &mut impl CreaseCache,
) -> Option<f32> {
    use vcod_common::pmove::{HALF_WIDTH, JUMP_HEIGHT, MIN_WALK_NORMAL, STEPSIZE, Stance};
    let mins = Vec3::new(-HALF_WIDTH, -HALF_WIDTH, 0.0);
    let maxs = Vec3::new(HALF_WIDTH, HALF_WIDTH, Stance::Stand.height());
    let up = p + Vec3::Z * STEPSIZE;
    let sweep = |dir: Vec3| world.box_trace(up, up + dir * look, mins, maxs - Vec3::Z * STEPSIZE);
    let mut t = sweep(dir);
    let mut dir = dir;
    // Against a wall the heading only grazes, the body slides along it:
    // feel along the slide instead (mp_ship's wall at (6111, -304, 97),
    // whose foot drops 41 to the deck).
    let n = t.normal.truncate().extend(0.0);
    let slide = dir - n * dir.dot(n);
    if t.fraction * look < DROP_SAMPLE && dir.dot(n) > -GRAZE && slide.length() > 0.1 {
        dir = slide.normalize();
        t = sweep(dir);
    }
    if t.startsolid || t.allsolid {
        return None;
    }
    let reach = t.fraction * look;
    let hurts = vcod_common::pmove::FallHeights::default().min;
    let mut floor = p.z;
    // A sample every DROP_SAMPLE, and half way between for a hole a body
    // falls through where the samples either side stand on its rims (a
    // point trace first, the capsule only where it finds no floor), and
    // one where the sweep stops: a body run at a wall ends up there, over
    // whatever lies before it (mp_ship's crow's nest plate, 11 units from
    // the top's face across the hatch, bot-navigation.md section 3).
    let half = DROP_SAMPLE / 2.0;
    let last = (reach > WALL_NEAR && reach % half > 1.0).then_some(reach);
    let mut d = half;
    while let Some(d_at) = (d <= reach).then_some(d).or(last.filter(|&r| d - half < r)) {
        let at = (p + dir * d_at).truncate().extend(floor + STEPSIZE);
        let between = d_at == d && d % DROP_SAMPLE != 0.0;
        d += half;
        if between
            && world
                .point_trace(
                    at,
                    at - Vec3::Z * (STEPSIZE + JUMP_HEIGHT),
                    MASK_PLAYERSOLID,
                    false,
                )
                .fraction
                < 1.0
        {
            continue;
        }
        let t = world.box_trace(at, at - Vec3::Z * (STEPSIZE + JUMP_HEIGHT), mins, maxs);
        if t.startsolid {
            return None;
        }
        if t.fraction >= 1.0 {
            let deep = world.box_trace(at, at - Vec3::Z * (STEPSIZE + hurts), mins, maxs);
            let depth = deep.fraction * (STEPSIZE + hurts) - STEPSIZE;
            return Some(if deep.fraction >= 1.0 {
                f32::INFINITY
            } else {
                depth
            });
        }
        if t.normal.z > 0.0 && t.normal.z < MIN_WALK_NORMAL && creases.drains(world, t.endpos) {
            return Some(f32::INFINITY);
        }
        floor = t.endpos.z;
        // A slope or a stair as steep runs as far down within the look:
        // mp_ship's hull slope at (3752, 340-414) runs 49 down in 75 to a
        // wedge off the graph.
        if floor < p.z - JUMP_HEIGHT {
            return Some(p.z - floor);
        }
    }
    None
}

/// Which spots on ground too steep to stand on drain into a crease
/// ([`held_off_ground`]), by 8-unit cell. A level's world is fixed, and the
/// test runs a body for up to 50 ticks, too dear for every bot every tick.
#[derive(Default)]
pub struct Creases(std::collections::HashMap<[i32; 3], bool>);

/// The crease verdicts [`drop_ahead`] reads and fills: a level's
/// [`Creases`], or a worker's [`CreaseRead`] over them.
pub(crate) trait CreaseCache {
    /// Whether the spot `p` drains into a crease, felt from `p` the first
    /// time its cell is read.
    fn drains(&mut self, world: &CollisionWorld, p: Vec3) -> bool;
}

fn crease_cell(p: Vec3) -> [i32; 3] {
    (p / 8.0).floor().as_ivec3().to_array()
}

impl CreaseCache for Creases {
    fn drains(&mut self, world: &CollisionWorld, p: Vec3) -> bool {
        *self
            .0
            .entry(crease_cell(p))
            .or_insert_with(|| drains(world, p))
    }
}

impl Creases {
    /// Keeps the cells a [`CreaseRead`] felt, when each reads as here
    /// wherever both hold it: then a probe that read them on a worker read
    /// what it would have read here. `false` keeps none.
    pub(crate) fn merge(&mut self, fresh: &[([i32; 3], bool)]) -> bool {
        if fresh
            .iter()
            .any(|(c, v)| self.0.get(c).is_some_and(|k| k != v))
        {
            return false;
        }
        for &(c, v) in fresh {
            self.0.entry(c).or_insert(v);
        }
        true
    }
}

/// [`Creases`] read from a worker thread: a cell they lack is felt into
/// `fresh`, in the order felt, for [`Creases::merge`].
pub(crate) struct CreaseRead<'a> {
    pub(crate) base: &'a Creases,
    pub(crate) fresh: Vec<([i32; 3], bool)>,
}

impl CreaseCache for CreaseRead<'_> {
    fn drains(&mut self, world: &CollisionWorld, p: Vec3) -> bool {
        let cell = crease_cell(p);
        if let Some(&v) = self.base.0.get(&cell) {
            return v;
        }
        if let Some(&(_, v)) = self.fresh.iter().find(|c| c.0 == cell) {
            return v;
        }
        let v = drains(world, p);
        self.fresh.push((cell, v));
        v
    }
}

/// How near a wall [`wall_ahead`] calls a heading blocked.
const WALL_NEAR: f32 = 4.0;

/// Whether a body standing at `p` meets a wall within [`WALL_NEAR`] along
/// the flat unit vector `dir`, square enough that it pins rather than
/// slides ([`GRAZE`]). [`drop_ahead`] reads such a heading as no drop.
pub(crate) fn wall_ahead(world: &CollisionWorld, p: Vec3, dir: Vec3) -> bool {
    use vcod_common::pmove::{HALF_WIDTH, STEPSIZE, Stance};
    let mins = Vec3::new(-HALF_WIDTH, -HALF_WIDTH, 0.0);
    let maxs = Vec3::new(HALF_WIDTH, HALF_WIDTH, Stance::Stand.height() - STEPSIZE);
    let up = p + Vec3::Z * STEPSIZE;
    let t = world.box_trace(up, up + dir * WALL_NEAR, mins, maxs);
    t.startsolid || (t.fraction < 1.0 && dir.dot(t.normal.truncate().extend(0.0)) <= -GRAZE)
}

/// Idle cmds until a body dropped at `p` stands on something; `None` when it
/// falls out of the world or never lands.
fn settle(world: &CollisionWorld, p: Vec3, yaw: f32) -> Option<Vec3> {
    let mut ps = body(p + Vec3::Z, yaw);
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
/// A body on the ground faster than this, units/s, slows onto a creep's
/// top before it backs off ([`Follower::creep_top`]).
const CREEP_TOP_PACE: f32 = 100.0;
/// The pace across a creep's way a bot comes onto its top at, off a turn
/// ([`creep_takes_a_turn`]): seed 183's 148 on mp_depot's roof.
const TURN_PACE: f32 = 150.0;
/// How far across from a ladder box a fall is retried backing down.
const LADDER_NEAR: f32 = 96.0;
/// The ticks a walk's budget stretches by while the body is in the air:
/// a fall of [`MAX_DROP`] takes 16.
const FALL_TICKS: usize = 18;

/// Inside this of its target, horizontally, a body backing onto a ladder
/// below a ledge creeps: an airborne grab reaches 17 units past the box
/// (`check_ladder_move`, the box shrunk 6 and the trace 8), and at a run the
/// body is past that within the tick it leaves the lip.
const CREEP_RADIUS: f32 = 24.0;
/// The creep's move key, about 16 units/s. The body leaves the lip at 15
/// units past the face and is first low enough to grab (under the ladder's
/// top) a tick later, so two ticks of travel have to stay under 2 units.
pub const CREEP: i8 = 15;
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

/// The yaw a climb holds: square into the ladder's face, whatever the
/// head's bearing. Aimed at a head off to one side, the body slides along
/// the rungs to the ladder's edge and pins under what overhangs it there
/// (mp_ship's ladder at x 3354, bot-navigation.md "Ladders").
pub fn climb_yaw(ladder_normal: Vec3) -> Option<f32> {
    let n = ladder_normal.truncate();
    (n.length_squared() > 0.01).then(|| (-n.y).atan2(-n.x).to_degrees())
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

    /// Whether the last key sent was the creep's.
    pub fn creeping(&self) -> bool {
        self.0.is_some()
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
    /// [`Gait::Forward`] that holds crouch from the tick after its jump
    /// until the body lands again: through a window too low for a standing
    /// body (`NavGraph::crouch_jumps`).
    CrouchJump,
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
    walk_to(world, ladders, from, target, None)
}

/// [`walk`], arriving only on `floor` when given. Only these runs jump
/// out of a bob ([`BOB`]): the flood proves such a way as a jump edge,
/// where a bot's walk on from its body would read one as a way it walks.
fn walk_to(
    world: &CollisionWorld,
    ladders: &[(Vec3, Vec3)],
    from: Vec3,
    target: glam::Vec2,
    floor: Option<f32>,
) -> Option<(Vec3, bool)> {
    for gait in [Gait::Forward, Gait::Backward, Gait::Creep] {
        if gait == Gait::Creep && !ladder_near(ladders, from) {
            return None;
        }
        let walked = if gait == Gait::Forward {
            BOB.set(true);
            let w = walk_as(world, from, target, floor, gait);
            BOB.set(false);
            w
        } else {
            walk_as(world, from, target, floor, gait)
        };
        match walked {
            Walked::Arrived(p, jumped) => return Some((p, jumped)),
            Walked::Blocked => return None,
            Walked::Fell => {}
        }
    }
    None
}

/// Whether a creep from `from` also arrives with the body coming onto it
/// off a turn, at [`TURN_PACE`] across the way to `target` either side,
/// where the graph's walk ran straight at the drop. mp_depot's thin ledges
/// at (-2749, -1281, 326) and (-2755, -1150, 326), 182 under the roof, land
/// only the straight run (bot-navigation.md section 3).
fn creep_takes_a_turn(
    world: &CollisionWorld,
    from: Vec3,
    target: glam::Vec2,
    floor: Option<f32>,
) -> bool {
    let dir = (target - from.truncate()).normalize_or_zero();
    let across = glam::Vec2::new(-dir.y, dir.x) * TURN_PACE;
    [across, -across].into_iter().all(|v| {
        matches!(
            walk_with(world, from, Some(v.extend(0.0)), target, floor, Gait::Creep),
            Walked::Arrived(..)
        )
    })
}

/// Whether a floor point 32 or 48 units from `q` in any of the eight
/// octants has nothing under it within `bg_fallDamageMinHeight` (256).
fn beside_a_fall_that_hurts(world: &CollisionWorld, q: Vec3) -> bool {
    (0..8).any(|i| {
        let a = (i as f32 * 45.0).to_radians();
        let dir = Vec3::new(a.cos(), a.sin(), 0.0);
        [32.0, 48.0].iter().any(|&r| {
            let top = q + dir * r + Vec3::Z * 16.0;
            let down = top - Vec3::Z * (16.0 + 256.0);
            world
                .point_trace(top, down, MASK_PLAYERSOLID, false)
                .fraction
                >= 1.0
        })
    })
}

/// Whether a point 32 to 96 units from a drop's landing `q` in any of the
/// eight octants, in the open from [`MISS_EYE`] over `q`, has no floor
/// within `bg_fallDamageMinHeight` (256) under `q`: a run off the drop
/// that misses it carries that far (mp_ship's run onto the ledge at
/// (3521, 289, 760) flew 93 units before it was level with it). From
/// lower, a slope over that ledge hides the way it flew.
fn a_miss_hurts(world: &CollisionWorld, q: Vec3) -> bool {
    (0..8).any(|i| {
        let a = (i as f32 * 45.0).to_radians();
        let dir = Vec3::new(a.cos(), a.sin(), 0.0);
        let eye = q + Vec3::Z * MISS_EYE;
        [32.0, 48.0, 64.0, 80.0, 96.0].iter().any(|&r| {
            let top = eye + dir * r;
            let down = top - Vec3::Z * (MISS_EYE + 256.0);
            // Not through a wall: past the hull lies nothing at all.
            world
                .point_trace(eye, top, MASK_PLAYERSOLID, false)
                .fraction
                >= 1.0
                && world
                    .point_trace(top, down, MASK_PLAYERSOLID, false)
                    .fraction
                    >= 1.0
        })
    })
}

/// Whether a body at `at` moving at `pace` with no key held leaves the
/// ground and falls more than a jump within [`COAST_TICKS`].
fn coasts_off(world: &CollisionWorld, at: Vec3, pace: Vec3) -> bool {
    let mut ps = body(at, 0.0);
    ps.velocity = pace.truncate().extend(0.0);
    let mut sim = Sim::default();
    (0..COAST_TICKS).any(|_| {
        sim.tick(world, &mut ps, &NULL_USERCMD);
        ps.origin.z < at.z - vcod_common::pmove::JUMP_HEIGHT
    })
}

/// How far `p` lies from a ladder box, flat.
fn box_flat(b: &(Vec3, Vec3), p: Vec3) -> f32 {
    let q = p.truncate().clamp(b.0.truncate(), b.1.truncate());
    q.distance(p.truncate())
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
    walk_with(world, from, None, target, floor, gait)
}

/// [`walk_as`] with the body moving at `pace` to begin with, in place of
/// the gait's own start.
///
/// A run from the gait's own start that slides on ground too steep to stand
/// on arrives only if no spot it slid over drains into a crease: a body
/// left there at rest comes to rest held off the ground. A bot comes onto
/// an edge off a turn and slower than the run, and on such a slope the pace
/// it carries, not its keys, says where it goes (mp_hurtgen's crease,
/// bot-navigation.md section 3).
fn walk_with(
    world: &CollisionWorld,
    from: Vec3,
    pace: Option<Vec3>,
    target: glam::Vec2,
    floor: Option<f32>,
    gait: Gait,
) -> Walked {
    let mut trail = Trail::default();
    let walked = walk_sliding(world, from, pace, 1.0, target, floor, gait, &mut trail);
    let proving = pace.is_none() && matches!(gait, Gait::Forward | Gait::Backward);
    if proving
        && matches!(walked, Walked::Arrived(..))
        && trail.slid.iter().any(|&p| drains(world, p))
    {
        return Walked::Blocked;
    }
    walked
}

/// Where a body off the ground left to itself comes to rest is off the
/// ground too: it never lands in [`SETTLE_TICKS`] and then stays within a
/// unit of one spot for [`HELD_TICKS`]. A crease, not a bounce on a slope
/// or a fall. A player left on mp_hurtgen's slope slides into its crease
/// and stays (bot-navigation.md section 3).
fn held_off_ground(world: &CollisionWorld, mut ps: PlayerState, mut sim: Sim) -> bool {
    let mut idle = || {
        sim.tick(world, &mut ps, &NULL_USERCMD);
        !ps.on_ground && !ps.on_ladder
    };
    if !(0..SETTLE_TICKS).all(|_| idle()) {
        return false;
    }
    let at = ps.origin;
    (0..HELD_TICKS).all(|_| {
        sim.tick(world, &mut ps, &NULL_USERCMD);
        !ps.on_ground && !ps.on_ladder && ps.origin.distance(at) < 1.0
    })
}

/// Whether a body left at rest at `p` comes to rest held off the ground
/// ([`held_off_ground`]) under a bot's 50 ms cmds or any of a client's
/// three 16/17 ms splits. On mp_hurtgen's slope the answer turns on the
/// cmd split and the last bits of the move: retail holds a body left at
/// (385.42, 2813.30, -29.90) and one a tenth of a unit off it, where one
/// split of ours lets each go (bot-navigation.md section 3, "The drain
/// test's cmd splits").
fn drains(world: &CollisionWorld, p: Vec3) -> bool {
    std::iter::once(None)
        .chain(CLIENT_SPLITS.iter().map(Some))
        .any(|split| {
            let sim = Sim {
                split: split.copied(),
                ..Sim::default()
            };
            held_off_ground(world, body(p, 0.0), sim)
        })
}

/// A 60 Hz client's cmds over a 50 ms frame, each phase.
const CLIENT_SPLITS: [[i32; 3]; 3] = [[16, 17, 17], [17, 16, 17], [17, 17, 16]];

const SETTLE_TICKS: usize = 40;
const HELD_TICKS: usize = 10;

/// What a walk passed over: the spots where the body was off the ground
/// against a plane too steep to stand on, and the deepest it went under its
/// start off the ground and off the rungs.
#[derive(Default)]
struct Trail {
    slid: Vec<Vec3>,
    dip: f32,
}

/// [`walk_with`] without the crease test, the body's weapon moving it at
/// `move_scale`; `trail` gathers what it passed over.
#[allow(clippy::too_many_arguments)]
fn walk_sliding(
    world: &CollisionWorld,
    from: Vec3,
    pace: Option<Vec3>,
    move_scale: f32,
    target: glam::Vec2,
    floor: Option<f32>,
    gait: Gait,
    trail: &mut Trail,
) -> Walked {
    let dist = target.distance(from.truncate());
    let on_floor = |z: f32| floor.is_none_or(|f| (z - f).abs() < ARRIVE);
    if dist < ARRIVE && on_floor(from.z) {
        return Walked::Arrived(from, false);
    }
    // A running start: a following bot never stops at a node, and the
    // spin-up from a standstill was a third of every walk.
    let mut ps = body(from, 0.0);
    let dir = (target - from.truncate()) / dist;
    // A leap starts standing: a bot comes to rest at its foot first.
    if let Some(v) = pace {
        ps.velocity = v.truncate().extend(0.0);
    } else if gait != Gait::Leap {
        ps.velocity = (dir * vcod_common::pmove::SPEED_RUN).extend(0.0);
    }
    let mut sim = Sim::scaled(move_scale);
    let mut budget = run_ticks(dist);
    // A floor-bound walk is a ladder's (`link_ladders`): room to reach it.
    if floor.is_some() {
        budget = budget.max(60);
    }
    if gait == Gait::Creep {
        budget += CREEP_TICKS;
    }
    let mut creep = Creep::default();
    let mut climbed = false;
    // The highest the body held on the rungs: a walk to a floor no higher
    // than its start that goes up a ladder and over the top arrives by a
    // way a bot heading for the target never takes (mp_depot's ladder at
    // (-417, 1112), bot-navigation.md section 3).
    let mut rung_top = f32::MIN;
    let mut fell = false;
    // The highest the body rose since it last stood or held on: a fall is
    // measured from there, as `PM_CrashLand` measures it. A paced walk
    // measures from where the body last stood, in the air only.
    let mut fall_top = from.z;
    let mut support_z = from.z;
    let mut tick = 0;
    let mut airborne = false;
    let cap = budget + FALL_TICKS + run_ticks(dist);
    // A forward run pinned on the ground jumps once (`jump_key`); one that
    // still doesn't arrive reports what it came to before the jump.
    let mut jumped = false;
    let mut jump = false;
    let mut fell_before_jump = false;
    let mut pin = PinJump::default();
    let mut crouched = false;
    // A body in the air when the budget runs out is let land: a drop down
    // a steep companionway is slower than the run it replaced.
    while tick < budget || (airborne && tick < budget + FALL_TICKS) {
        tick += 1;
        let before = ps.origin;
        let to = target - ps.origin.truncate();
        let (yaw, forward) = match gait {
            Gait::Forward | Gait::Leap | Gait::CrouchJump => (to.y.atan2(to.x).to_degrees(), 127),
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
        let mut cmd = UserCmd {
            forward,
            up: if std::mem::take(&mut jump) { 127 } else { 0 },
            angles: [(pitch * ANGLE2SHORT) as i32, (yaw * ANGLE2SHORT) as i32, 0],
            ..NULL_USERCMD
        };
        // `PM_CheckJump` refuses a crouched body, so the crouch starts on
        // the cmd after the jump's, held as a retail client holds it.
        crouched &= !ps.on_ground;
        if crouched {
            cmd.wbuttons |= msg::WBUTTON_CROUCH;
            cmd.up = -127;
        }
        crouched |= gait == Gait::CrouchJump && cmd.up > 0;
        if gait == Gait::Leap && !jumped && ps.on_ground {
            // The lip ([`lip_ahead`]): the tick run without the jump, kept
            // when it stays on the ground, run again with it when it does not.
            let (mut trial, mut run) = (ps, sim.clone());
            run.tick(world, &mut trial, &cmd);
            if trial.on_ground || trial.on_ladder {
                (ps, sim) = (trial, run);
            } else {
                jumped = true;
                fell_before_jump = fell;
                budget = budget.max(tick + JUMP_TICKS + run_ticks(to.length()));
                cmd.up = 127;
                sim.tick(world, &mut ps, &cmd);
            }
        } else {
            sim.tick(world, &mut ps, &cmd);
        }
        // Landed past the budget: the run back to the target from there.
        if airborne && ps.on_ground && tick >= budget {
            let rest = run_ticks(target.distance(ps.origin.truncate()));
            budget = budget.max((tick + rest).min(cap));
        }
        airborne = !ps.on_ground && !ps.on_ladder;
        if ps.on_ladder {
            rung_top = rung_top.max(ps.origin.z);
        }
        if ps.sliding() && !ps.on_ladder {
            trail.slid.push(ps.origin);
        }
        let held = ps.on_ground || ps.on_ladder;
        if !held {
            fell = true;
            trail.dip = trail.dip.max(from.z - ps.origin.z);
            fall_top = fall_top.max(ps.origin.z);
        }
        let deep = match pace {
            Some(_) => !held && ps.origin.z < support_z - PACED_DROP,
            None => ps.origin.z < fall_top - MAX_DROP,
        };
        if deep {
            return Walked::Fell;
        }
        if held {
            fall_top = ps.origin.z;
            support_z = ps.origin.z;
        }
        if ps.on_ground && target.distance(ps.origin.truncate()) < ARRIVE && on_floor(ps.origin.z) {
            if under_ground(world, ps.origin)
                || floor.is_some_and(|f| rung_top > f.max(from.z) + vcod_common::pmove::JUMP_HEIGHT)
            {
                return Walked::Blocked;
            }
            return Walked::Arrived(ps.origin, jumped);
        }
        // Right under a floor more than a step up: re-aimed at a point
        // overhead, the run only jitters and never stalls. Or pinned under
        // a face too steep to stand on, where the step lifts the body onto
        // it and it slides back down, so it is on the ground one tick in a
        // few: it jumps on the next one (mp_berlin's roof gutter,
        // bot-navigation.md "Jumps").
        let armed = matches!(gait, Gait::Forward | Gait::CrouchJump) && !jumped;
        let roomless = ROOMLESS.get();
        // Only a bot's jump waits out a ramp's climb ([`jump_under`]); the
        // flood's walks keep the jump under the top. Taken out of them too,
        // it turned 26 of mp_ship's jump edges plain and dropped a node,
        // and bots stalled and fell at its stairwell and crow's nest
        // (bot-navigation.md, "The jump cue and the re-proof").
        let rose = if roomless {
            ps.origin.z - before.z
        } else {
            0.0
        };
        let under = floor.is_some_and(|f| {
            jump_under(target.distance(ps.origin.truncate()), f - ps.origin.z, rose)
        });
        let clear = || roomless || jump_clear(world, &ps, target, gait == Gait::CrouchJump);
        if pin.before(armed, ps.on_ground, under, clear) {
            jumped = true;
            jump = true;
            fell_before_jump = fell;
            let rest = run_ticks(target.distance(ps.origin.truncate()));
            budget = budget.max(tick + JUMP_TICKS + rest);
            continue;
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
        let least = if forward.abs() < 127 { 0.5 } else { PINNED };
        let hanging = (climbed || jumped) && !ps.on_ground && !ps.on_ladder;
        let bob = gait == Gait::Forward && BOB.get();
        let clear = || roomless || jump_clear(world, &ps, target, gait == Gait::CrouchJump);
        match pin.after(
            armed,
            moved < least && !hanging,
            ps.on_ground,
            ps.sliding(),
            bob,
            clear,
        ) {
            PinTick::Go => {}
            PinTick::Stuck => break,
            PinTick::Jump => {
                jumped = true;
                jump = true;
                fell_before_jump = fell;
                let rest = run_ticks(target.distance(ps.origin.truncate()));
                budget = budget.max(tick + JUMP_TICKS + rest);
            }
        }
    }
    if jumped {
        fell = fell_before_jump;
    }
    if fell { Walked::Fell } else { Walked::Blocked }
}

/// Under this many units a tick a body is pinned: a run against a ledge's
/// face, which a jump may clear.
pub const PINNED: f32 = 2.0;
/// A body this close under a jump edge's top, flat, jumps whatever its pace:
/// past it, a run carries it off the ledge's foot (mp_depot's stair to the
/// documents' step).
const JUMP_UNDER: f32 = 16.0;
/// The ticks a jump stays in the air: 2 * 249.8 / 800 s.
const JUMP_TICKS: usize = 13;

/// Whether a body standing `flat` from a jump edge's top and `rise` under
/// it, having `climbed` the last tick, jumps whatever its pace
/// ([`JUMP_UNDER`]). Not while it still climbs a ramp under the top: off
/// mp_uo_dawnville's ramp at (-195, -16833, 138) that jump flies past the
/// floor at 176 and falls back, where the run on stalls at its foot and
/// jumps onto it (bot-navigation.md, "The jump cue and the re-proof").
pub fn jump_under(flat: f32, rise: f32, climbed: f32) -> bool {
    flat < JUMP_UNDER && rise > vcod_common::pmove::STEPSIZE && climbed < CLIMBING
}

/// A tick's rise at which a body still climbs a ramp ([`jump_under`]).
const CLIMBING: f32 = 1.0;

/// Whether a leap jumps now, its top `flat` away and `rise` above: at the
/// lip ([`lip_ahead`]), as its walk did, and never once on the top.
pub fn leap_cue(lip: bool, flat: f32, rise: f32) -> bool {
    lip && !on_jump_top(flat, rise)
}

/// Whether a body `flat` from a jump edge's top and `rise` under it
/// already stands on it: no jump from there.
pub fn on_jump_top(flat: f32, rise: f32) -> bool {
    flat < REACH && rise.abs() < JUMP_PASS
}

/// The forward walk's jump ([`walk_sliding`]), one tick at a time: on the
/// ground right under a floor above ([`jump_under`]), on the ground out
/// of a bob, or after two ticks pinned (moved under [`PINNED`] units, half
/// one creeping), each only where [`jump_clear`] passes. A bot on a jump
/// edge runs the same rule without the room test (`Bot::think`), and
/// every jump edge is walked again that way ([`NavGraph::reprove_jumps`],
/// [`as_a_bot`]), so it jumps where the walk that proved the edge did.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct PinJump {
    stalled: u32,
    bobbing: bool,
}

/// What a [`PinJump`] makes of a tick.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PinTick {
    /// Jump on the next cmd.
    Jump,
    /// Run on.
    Go,
    /// Pinned with no jump: the walk gives up.
    Stuck,
}

impl PinJump {
    /// The jumps that read before the tick's progress: right `under` the
    /// target floor, or out of a bob. `armed` is false once the walk has
    /// jumped or in a gait that never jumps.
    pub fn before(
        &mut self,
        armed: bool,
        on_ground: bool,
        under: bool,
        clear: impl Fn() -> bool,
    ) -> bool {
        let jump = armed && on_ground && (under || self.bobbing) && clear();
        if jump {
            *self = Self::default();
        }
        jump
    }

    /// The tick's progress: `stalled` when it moved under the pinned
    /// pace, not hanging in the air off a jump or a climb. A stall that
    /// cannot jump turns into a bob once where `bob` allows it and the
    /// body slides on a steep face.
    pub fn after(
        &mut self,
        armed: bool,
        stalled: bool,
        on_ground: bool,
        sliding: bool,
        bob: bool,
        clear: impl Fn() -> bool,
    ) -> PinTick {
        if !stalled {
            self.stalled = 0;
            return PinTick::Go;
        }
        self.stalled += 1;
        if self.stalled < 2 {
            return PinTick::Go;
        }
        if armed && on_ground && clear() {
            *self = Self::default();
            PinTick::Jump
        } else if armed && bob && !self.bobbing && sliding {
            self.bobbing = true;
            self.stalled = 0;
            PinTick::Go
        } else {
            PinTick::Stuck
        }
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
        && jump_ready(ps)
}

/// Whether the body's next jump rises the full 39 units the graph's walks
/// jumped: off `PM_CheckJump`'s 500 ms cooldown and, under 1.5's and UO's
/// rules, clear of the jump penalty, which cuts a jump inside 1.8 s of the
/// last landing to as little as 15.6 (cod15-movement.md 2).
pub fn jump_ready(ps: &PlayerState) -> bool {
    ps.since_jump_ms >= vcod_common::pmove::JUMP_COOLDOWN_MS && !ps.jump_penalty
}

/// Key `k` cut to the graph's pace for a weapon moving the body at
/// `move_scale`: a following bot holds this on the ground (`Bot::think`),
/// so a light weapon runs at the 190 units/s the graph's walks ran at.
pub fn paced_key(k: i8, move_scale: f32) -> i8 {
    if move_scale > 1.0 {
        (f32::from(k) / move_scale).round() as i8
    } else {
        k
    }
}

/// [`paced_key`] for the weapon `ps` holds.
fn paced_key_for(
    k: i8,
    ps: &PlayerState,
    weapons: &[Option<vcod_common::weapon::WeaponDef>],
) -> i8 {
    let scale = weapons
        .get(ps.weapon as usize)
        .and_then(Option::as_ref)
        .map_or(1.0, |d| d.move_speed_scale);
    paced_key(k, scale)
}

/// Whether the body `ps` loses the ground in the coming tick running at
/// `target`: the tick's move, run on a copy. A leap jumps there
/// (`Gait::Leap`, and the bots on its edges). The move, not the pace: a
/// body speeding up from rest covers half again what its velocity says,
/// and a bot that runs faster than the walk's body (`weapons`' move
/// scale) walked off mp_depot's step to the documents' crate a tick
/// before the lip read (bot-navigation.md, "Jumps").
pub fn lip_ahead(
    world: &CollisionWorld,
    ps: &PlayerState,
    target: glam::Vec2,
    weapons: &[Option<vcod_common::weapon::WeaponDef>],
) -> bool {
    let mut ps = *ps;
    let to = target - ps.origin.truncate();
    let cmd = UserCmd {
        server_time: FRAME_MS,
        forward: paced_key_for(127, &ps, weapons),
        angles: [0, (to.y.atan2(to.x).to_degrees() * ANGLE2SHORT) as i32, 0],
        weapon: ps.weapon,
        ..NULL_USERCMD
    };
    let dt = FRAME_MS as f32 / 1000.0;
    if let Some(own) = own_step() {
        own.step(&mut ps, &mut [0; 3], &mut None, 0, &cmd, world, weapons);
        return !ps.on_ground && !ps.on_ladder;
    }
    let mw = MoveWorld::bare(world);
    player_step(
        &mut ps,
        &mut [0; 3],
        &mut EventRing::default(),
        &cmd,
        dt,
        &mw,
        weapons,
    );
    !ps.on_ground && !ps.on_ladder
}

/// Ticks an [`unpin_yaw`] heading is run for, and held by the bot.
pub const UNPIN_TICKS: usize = 4;
/// A body slower than this, units/s, on the ground short of its waypoint
/// is checked for a pin ([`unpin_yaw`]).
pub const UNPIN_SPEED: f32 = 20.0;
/// The rise per unit across a waypoint off the body's floor may take and
/// still be checked for a pin: a stair's 32 in 48 and a little more.
const UNPIN_GRADE: f32 = 0.75;

/// A heading that frees the body `ps`, standing still on the ground and
/// pinned short of `to` on its floor: `None` when the run straight at `to`
/// closes 2 units in [`UNPIN_TICKS`]; else, of the headings 10 to 90
/// degrees either side of it, every 10, and the compass axes within 90 of
/// it, the one whose run, held for those ticks,
/// closes on `to` most and by 8 or more, staying on the ground within a
/// step of the floors between the body and `to` and off `hazards`. A body
/// at rest in a crease where two faces meet does not move at all toward a
/// point a few degrees past one of them, and the same run with a running
/// start slides through (mp_ship's corner at x 5375.1, bot-navigation.md
/// section 3). A waypoint down or up a stair counts as the body's floor
/// when the rise is a walkable one: mp_uo_stanjel's stair turn at
/// (711, 235, 246) pins a body on the newel's corner, slower every tick.
pub fn unpin_yaw(
    world: &CollisionWorld,
    hazards: &[BrushHull],
    ps: &PlayerState,
    to: Vec3,
    weapons: &[Option<vcod_common::weapon::WeaponDef>],
) -> Option<f32> {
    let flat = (to - ps.origin).truncate();
    let step = vcod_common::pmove::STEPSIZE;
    if !ps.on_ground
        || ps.on_ladder
        || ps.velocity.truncate().length() >= UNPIN_SPEED
        || flat.length() < ARRIVE
        || (to.z - ps.origin.z).abs() > step + flat.length() * UNPIN_GRADE
    {
        return None;
    }
    let (lo, hi) = (to.z.min(ps.origin.z) - step, to.z.max(ps.origin.z) + step);
    let mw = MoveWorld::bare(world);
    let dt = FRAME_MS as f32 / 1000.0;
    // How much nearer `to` a run held at `yaw` ends, when it keeps its
    // footing on the floor all the way.
    let key = paced_key_for(127, ps, weapons);
    let own = own_step();
    let closes = |yaw: f32| {
        let mut ps = *ps;
        let mut delta = [0; 3];
        let mut ring = EventRing::default();
        let mut carry = None;
        for i in 0..UNPIN_TICKS {
            let cmd = UserCmd {
                server_time: FRAME_MS * (i as i32 + 1),
                forward: key,
                angles: [0, (yaw * ANGLE2SHORT) as i32, 0],
                weapon: ps.weapon,
                ..NULL_USERCMD
            };
            match &own {
                Some(own) => {
                    let from = FRAME_MS * i as i32;
                    own.step(&mut ps, &mut delta, &mut carry, from, &cmd, world, weapons);
                }
                None => {
                    player_step(&mut ps, &mut delta, &mut ring, &cmd, dt, &mw, weapons);
                }
            }
            if !ps.on_ground || !(lo..=hi).contains(&ps.origin.z) || hazard(hazards, ps.origin) {
                return None;
            }
        }
        Some(flat.length() - (to - ps.origin).truncate().length())
    };
    let straight = flat.y.atan2(flat.x).to_degrees();
    if closes(straight).is_none_or(|d| d >= 2.0) {
        return None;
    }
    // Most faces are axial, and the run exactly along one slides where
    // one a hair into it pins: the axes go in as they are.
    let axes = [0.0f32, 90.0, 180.0, 270.0]
        .into_iter()
        .filter(|a| ((a - straight + 540.0).rem_euclid(360.0) - 180.0).abs() < 90.0);
    (1..=9)
        .flat_map(|i| [straight + i as f32 * 10.0, straight - i as f32 * 10.0])
        .chain(axes)
        .filter_map(|yaw| Some((closes(yaw)?, yaw)))
        .filter(|&(d, _)| d >= 8.0)
        .max_by(|a, b| a.0.total_cmp(&b.0))
        .map(|(_, yaw)| yaw)
}

/// Ticks a [`push_off`] flight is followed before it counts as failed.
const PUSH_OFF_TICKS: usize = 40;

/// The yaw of a push-off from the rungs `ps` hangs on toward `to`, a
/// floor behind the climber's back: `Some` when `to` lies 8 or more
/// units out from the face, between a jump under and a step over the
/// body, and the push-off run on a copy lands on that floor within 64
/// units of `to`. The flight is the jump facing the face, which
/// `PM_CheckJump` reflects straight out, then the forward key, held facing
/// the same way, until the body lands: facing out, `bg_ladder_yawcap`
/// would turn the view 80 degrees on the jump's frame and the jump with
/// it (cod11-mantle.md, "The ladder yaw cap"). A ladder
/// move climbs and strafes along the face but never steps off it, so a
/// floor behind the climber is left only by the jump, and the jump alone
/// carries a body about 60 units out (bot-navigation.md section 3, the
/// ladder at (2150, -214)).
pub fn push_off(
    world: &CollisionWorld,
    ps: &PlayerState,
    to: Vec3,
    weapons: &[Option<vcod_common::weapon::WeaponDef>],
) -> Option<f32> {
    let n = ps.ladder_normal.truncate();
    if !ps.on_ladder || n.length_squared() < 0.01 {
        return None;
    }
    let n = n.normalize();
    let dz = to.z - ps.origin.z;
    let out = (to.truncate() - ps.origin.truncate()).dot(n);
    if out < 8.0 || !(-vcod_common::pmove::JUMP_HEIGHT..=vcod_common::pmove::STEPSIZE).contains(&dz)
    {
        return None;
    }
    let yaw = (-n.y).atan2(-n.x).to_degrees();
    let mut ps = *ps;
    let mut delta = [0; 3];
    let mut ring = EventRing::default();
    let mw = MoveWorld::bare(world);
    let dt = FRAME_MS as f32 / 1000.0;
    let own = own_step();
    let mut carry = None;
    for i in 0..PUSH_OFF_TICKS {
        let cmd = UserCmd {
            server_time: FRAME_MS * (i as i32 + 1),
            forward: if i == 0 { 0 } else { 127 },
            up: if i == 0 { 127 } else { 0 },
            angles: [0, (yaw * ANGLE2SHORT) as i32, 0],
            weapon: ps.weapon,
            ..NULL_USERCMD
        };
        match &own {
            Some(own) => {
                let from = FRAME_MS * i as i32;
                own.step(&mut ps, &mut delta, &mut carry, from, &cmd, world, weapons);
            }
            None => {
                player_step(&mut ps, &mut delta, &mut ring, &cmd, dt, &mw, weapons);
            }
        }
        if i == 0 && ps.on_ladder {
            return None;
        }
        if ps.on_ground || ps.on_ladder {
            break;
        }
    }
    let landed = ps.on_ground
        && !ps.on_ladder
        && (ps.origin.z - to.z).abs() <= vcod_common::pmove::STEPSIZE
        && ps.origin.truncate().distance(to.truncate()) < 64.0;
    landed.then_some(yaw)
}

/// Whether a jump from where `ps` stands could clear what pins it: room
/// ahead toward `target` at the top of the jump, or under a ceiling lower
/// than the jump's height, as high as the body rises. A wall to the
/// ceiling fails it, which keeps a jump off the walks that end at one.
/// A `crouched` jump looks for room at the crouched body's height.
fn jump_clear(
    world: &CollisionWorld,
    ps: &PlayerState,
    target: glam::Vec2,
    crouched: bool,
) -> bool {
    let (mins, mut maxs) = (ps.mins(), ps.maxs());
    if crouched {
        maxs.z = vcod_common::pmove::Stance::Crouch.height();
    }
    let up = world.box_trace(
        ps.origin,
        ps.origin + Vec3::Z * (vcod_common::pmove::JUMP_HEIGHT + JUMP_SLACK),
        mins,
        maxs,
    );
    if up.startsolid {
        return false;
    }
    let top = up.endpos;
    let dir = (target - ps.origin.truncate())
        .normalize_or_zero()
        .extend(0.0);
    let ahead = world.box_trace(top, top + dir * 16.0, mins, maxs);
    !ahead.startsolid && ahead.fraction == 1.0
}

/// The room over [`JUMP_HEIGHT`](vcod_common::pmove::JUMP_HEIGHT) that
/// [`jump_clear`] allows: a jump out of mp_berlin's roof gutter from z
/// 612.26 lands on its ledge at 652.42, 40.2 up (UO rules, measured).
const JUMP_SLACK: f32 = 2.0;

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
/// Whether a standing body at `from` on the ground walks straight to `to`
/// on one floor: its capsule swept a step up clears, and every
/// [`PULL_PROBE`] units along the line there is ground a body stands on
/// within a step of the line's height ([`Follower::pull`]).
fn straight_floor(world: &CollisionWorld, from: Vec3, to: Vec3) -> bool {
    let step = vcod_common::pmove::STEPSIZE;
    let ps = body(from, 0.0);
    let up = Vec3::Z * step;
    let sweep = world.move_trace(from + up, to + up, ps.mins(), ps.maxs(), MASK_PLAYERSOLID);
    if sweep.startsolid || sweep.fraction < 1.0 {
        return false;
    }
    let n = ((to - from).truncate().length() / PULL_PROBE).ceil() as usize;
    (1..n).all(|i| {
        let p = from.lerp(to, i as f32 / n as f32);
        let t = world.point_trace(p + up, p - up, MASK_PLAYERSOLID, false);
        !t.startsolid && t.fraction < 1.0 && t.normal.z >= vcod_common::pmove::MIN_WALK_NORMAL
    })
}

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

/// The cmd times of one 50 ms tick for a body that starts it at `start`:
/// one, or on the rungs off the ground three to six, the count cycling
/// with the tick, as a client's frames come. Off the top of a ladder the
/// step-up has to clear the lip before `PM_CheckLadderMove`'s box lets go
/// 8 units under the ladder's top, and the step's slide carries the wall
/// glue's 500 units/s: whether it clears depends on the frame's length and
/// phase. In 50 ms frames, or any one length, a body can bob under a lip
/// for good where retail's client, whose frames vary, gets over on the
/// next try (docs/research/bot-navigation.md, "Ladders"). Shared by the
/// bots (`Server::step_bots`) and the graph's walks, so a bot climbs the
/// way the graph proved it.
pub fn tick_times(start: i32, climbing: bool) -> impl Iterator<Item = i32> {
    let n = if climbing {
        LADDER_SUBSTEPS[(start / FRAME_MS).rem_euclid(LADDER_SUBSTEPS.len() as i32) as usize]
    } else {
        1
    };
    (1..=n).map(move |i| start + FRAME_MS * i / n)
}

/// Cmds a climbing body sends a tick, by tick ([`tick_times`]).
const LADDER_SUBSTEPS: [i32; 4] = [5, 4, 3, 6];

/// The bits of a client a walk carries between cmds.
#[derive(Clone, Default)]
struct Sim {
    time: i32,
    delta: [i32; 3],
    ring: EventRing,
    /// Weapon 0's def, when the body moves at a `moveSpeedScale` other
    /// than 1 ([`Sim::scaled`]).
    weapons: Option<Arc<[Option<vcod_common::weapon::WeaponDef>]>>,
    /// Off the rungs, the tick's cmds as a client's lengths instead of one
    /// 50 ms cmd ([`drains`]).
    split: Option<[i32; 3]>,
    /// The [`WalkStep`]'s own body, when one is in force.
    carry: WalkCarry,
}

impl Sim {
    /// A walk whose body holds a weapon moving it at `move_scale`, as weapon
    /// 0: the walk's body holds no other.
    fn scaled(move_scale: f32) -> Sim {
        if move_scale <= 0.0 || move_scale == 1.0 {
            return Sim::default();
        }
        let map = HashMap::from([("moveSpeedScale".to_string(), move_scale.to_string())]);
        Sim {
            weapons: Some(Arc::new([Some(vcod_common::weapon::WeaponDef::from_map(
                &map,
            ))])),
            ..Sim::default()
        }
    }

    fn tick(&mut self, world: &CollisionWorld, ps: &mut PlayerState, cmd: &UserCmd) {
        let mw = MoveWorld::bare(world);
        let weapons = self.weapons.as_deref().unwrap_or(&[]);
        let climbing = ps.on_ladder && !ps.on_ground;
        // At most a climb's six cmds; no allocation on the walks' hot path.
        let mut times = [0; 6];
        let mut n = 0;
        match self.split {
            Some(split) if !climbing => {
                let mut t = self.time;
                for ms in split {
                    t += ms;
                    times[n] = t;
                    n += 1;
                }
            }
            _ => {
                for t in tick_times(self.time, climbing) {
                    times[n] = t;
                    n += 1;
                }
            }
        }
        // The walk's angles are absolute: net of `delta_angles` as the tick
        // starts, as a bot's aim is (`bots::cmd_angles`), so a ladder yaw
        // cap's push holds the view on the cap instead of turning the rest
        // of the walk.
        let d = self.delta;
        let angles = [0, 1, 2].map(|i| cmd.angles[i] - d[i]);
        let own = own_step();
        for &server_time in &times[..n] {
            let cmd = UserCmd {
                server_time,
                angles,
                ..*cmd
            };
            if let Some(own) = &own {
                own.step(
                    ps,
                    &mut self.delta,
                    &mut self.carry,
                    self.time,
                    &cmd,
                    world,
                    weapons,
                );
                self.time = server_time;
                continue;
            }
            chop(self.time, &cmd, |step, dt| {
                player_step(ps, &mut self.delta, &mut self.ring, step, dt, &mw, weapons);
                ps.jump_latched
            });
            self.time = server_time;
        }
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

    /// A vehicle parked over node 1 of [`tiny`]: a path under way through
    /// it is planned again round by node 3, and a goal on it is still
    /// walked to. Parked over node 3 as well, node 0 is hemmed in until
    /// the vehicle moves off.
    #[test]
    fn a_plan_runs_round_the_nodes_a_parked_vehicle_covers() {
        use crate::bots::Goal;
        let g = tiny();
        let mut f = Follower::default();
        let mut budget = 100;
        let mut step = |f: &mut Follower, goal| {
            let at = [-16.0, 16.0, 0.0];
            f.waypoint(
                &g,
                None,
                goal,
                at,
                Footing::STANDING,
                &mut budget,
                &mut || 0,
            )
        };
        let goal = Goal::To([64.0, 0.0, 0.0]);
        assert_eq!(step(&mut f, goal), Some([32.0, 0.0, 0.0]));
        f.blocked = Arc::new([1]);
        assert_eq!(step(&mut f, goal), Some([32.0, 64.0, 0.0]));
        assert_eq!(f.path, [0, 3, 2]);
        let mut f = Follower {
            blocked: Arc::new([1]),
            ..Follower::default()
        };
        assert_eq!(
            step(&mut f, Goal::To([32.0, 0.0, 0.0])),
            Some([32.0, 0.0, 0.0])
        );
        // Both ways covered: hemmed in, no plan.
        let mut f = Follower {
            blocked: Arc::new([1, 3]),
            ..Follower::default()
        };
        assert_eq!((0..3).find_map(|_| step(&mut f, goal)), None);
        assert!(f.penned());
        f.blocked = Arc::default();
        let w = (0..REST_TICKS + 2).find_map(|_| step(&mut f, goal));
        assert_eq!(w, Some([32.0, 0.0, 0.0]));
        assert!(!f.penned());
    }

    /// A body standing on node 1 of [`tiny`], in the way of a bot bound
    /// along 0, 1, 2: after [`BODY_TICKS`] held up the plan goes round by
    /// node 3. On [`row`] there is no way round: the plan goes through him,
    /// and is not dropped again until the nodes he stood over are let go.
    #[test]
    fn a_body_in_the_way_is_planned_round() {
        use crate::bots::Goal;
        let held = |f: &mut Follower, g: &NavGraph, at: [f32; 3], others: &[(Vec3, Vec3)]| {
            f.bodies_in_way(g, at.into(), Footing::STANDING, others)
        };
        let g = tiny();
        let mut f = Follower::default();
        let at = [-16.0, 16.0, 0.0];
        let goal = Goal::To([64.0, 0.0, 0.0]);
        let step = |f: &mut Follower| {
            f.waypoint(&g, None, goal, at, Footing::STANDING, &mut 100, &mut || 0)
        };
        assert_eq!(step(&mut f), Some([32.0, 0.0, 0.0]));
        let other = [(Vec3::new(28.0, 4.0, 0.0), Vec3::ZERO)];
        for _ in 1..BODY_TICKS {
            assert!(!held(&mut f, &g, at, &other));
        }
        // One behind the bot holds nothing up.
        let behind = [(Vec3::new(-40.0, 16.0, 0.0), Vec3::ZERO)];
        assert!(!held(&mut f, &g, at, &behind));
        for _ in 1..BODY_TICKS {
            assert!(!held(&mut f, &g, at, &other));
        }
        assert!(held(&mut f, &g, at, &other));
        assert_eq!(step(&mut f), Some([32.0, 64.0, 0.0]));
        assert_eq!(f.path, [0, 3, 2]);
        // A corridor: through him, once.
        let g = row();
        let mut f = Follower::default();
        let at = [8.0, 0.0, 0.0];
        let goal = Goal::To([160.0, 0.0, 0.0]);
        let step = |f: &mut Follower| {
            f.waypoint(&g, None, goal, at, Footing::STANDING, &mut 100, &mut || 0)
        };
        let other = [(Vec3::new(36.0, 0.0, 0.0), Vec3::ZERO)];
        assert_eq!(step(&mut f), Some([32.0, 0.0, 0.0]));
        for _ in 1..BODY_TICKS {
            assert!(!held(&mut f, &g, at, &other));
        }
        assert!(held(&mut f, &g, at, &other));
        let w = (0..3).find_map(|_| step(&mut f));
        assert_eq!(w, Some([32.0, 0.0, 0.0]));
        assert_eq!(f.path, [0, 1, 2, 3, 4, 5]);
        for _ in 0..2 * BODY_TICKS {
            assert!(!held(&mut f, &g, at, &other));
            step(&mut f);
        }
    }

    /// Two bodies meet head-on in a corridor along +x with an alcove off
    /// node 1: the one whose plan goes through the other stands aside in
    /// the alcove until the other has passed it, then plans again.
    #[test]
    fn a_body_met_head_on_in_a_corridor_is_stood_aside_for() {
        use crate::bots::Goal;
        let mut nodes: Vec<Vec3> = (0..6)
            .map(|i| Vec3::new(i as f32 * 32.0, 0.0, 0.0))
            .collect();
        nodes.push(Vec3::new(32.0, 48.0, 0.0));
        let mut edges: Vec<Vec<u32>> = (0..6u32)
            .map(|i| {
                [i.wrapping_sub(1), i + 1]
                    .into_iter()
                    .filter(|&n| n < 6)
                    .collect()
            })
            .collect();
        edges[1].push(6);
        edges.push(vec![1]);
        let g = NavGraph::from_parts(nodes, edges);
        let at = [8.0, 0.0, 0.0];
        let mut f = Follower::default();
        let step = |f: &mut Follower| {
            f.waypoint(
                &g,
                None,
                Goal::To([160.0, 0.0, 0.0]),
                at,
                Footing::STANDING,
                &mut 100,
                &mut || 0,
            )
        };
        let other = Vec3::new(40.0, 0.0, 0.0);
        let coming = [(other, Vec3::new(-100.0, 0.0, 0.0))];
        assert_eq!(step(&mut f), Some([32.0, 0.0, 0.0]));
        // Room to pass: no give way.
        assert_eq!(
            f.stand_aside(&g, at.into(), Footing::STANDING, || coming.to_vec()),
            None
        );
        let held = |f: &mut Follower| {
            f.bodies_in_way(&g, at.into(), Footing::STANDING, &[(other, Vec3::ZERO)])
        };
        assert!((0..BODY_TICKS).any(|_| held(&mut f)));
        assert!((0..3).find_map(|_| step(&mut f)).is_some());
        // One standing still is waited on, not given way to.
        let still = [(other, Vec3::ZERO)];
        assert_eq!(
            f.stand_aside(&g, at.into(), Footing::STANDING, || still.to_vec()),
            None
        );
        let spot = Some(Vec3::new(32.0, 48.0, 0.0));
        assert_eq!(
            f.stand_aside(&g, at.into(), Footing::STANDING, || coming.to_vec()),
            spot
        );
        assert_eq!(
            f.stand_aside(&g, at.into(), Footing::STANDING, || coming.to_vec()),
            spot
        );
        // Past the bot: plan again.
        let past = [(Vec3::new(-24.0, 0.0, 0.0), Vec3::new(-100.0, 0.0, 0.0))];
        assert_eq!(
            f.stand_aside(&g, at.into(), Footing::STANDING, || past.to_vec()),
            None
        );
        assert_eq!(step(&mut f), Some([32.0, 0.0, 0.0]));
        assert_eq!(f.path, [0, 1, 2, 3, 4, 5]);
    }

    #[test]
    fn a_drop_splits_the_components() {
        let c = tiny().components();
        assert_eq!(tiny().component_ids(), &c[..]);
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

    /// Off the side of a level edge into a pocket a step and more under it:
    /// the landing drops the plan and avoids the edge, and the next plan
    /// starts at a node a walk reaches.
    #[test]
    fn a_landing_under_a_level_edge_plans_again_round_it() {
        use crate::bots::Goal;
        let g = row();
        let mut f = Follower::default();
        let goal = Goal::To([160.0, 0.0, 0.0]);
        let mut budget = 100;
        let mut step = |f: &mut Follower, at: [f32; 3], body: Footing| {
            f.waypoint(&g, None, goal, at, body, &mut budget, &mut || 0)
        };
        assert!(step(&mut f, [40.0, 0.0, 0.0], Footing::STANDING).is_some());
        let air = Footing {
            on_ground: false,
            ..Footing::STANDING
        };
        assert!(step(&mut f, [70.0, 0.0, -10.0], air).is_some());
        assert_eq!(step(&mut f, [72.0, 0.0, -23.0], Footing::STANDING), None);
        assert!(f.fell && f.avoid.len() == 1);
    }

    /// Off the side of an edge and still falling past the waypoint's reach:
    /// the plan is dropped round the edge, as a landing under it drops it.
    /// Knocked as far off on its floor, the plan is only dropped.
    #[test]
    fn a_fall_off_the_path_past_its_reach_plans_round_the_edge() {
        use crate::bots::Goal;
        let g = row();
        let goal = Goal::To([160.0, 0.0, 0.0]);
        let air = Footing {
            on_ground: false,
            ..Footing::STANDING
        };
        for (z, avoided) in [(-100.0, true), (0.0, false)] {
            let mut f = Follower::default();
            let mut budget = 100;
            let mut step = |f: &mut Follower, at: [f32; 3], body: Footing| {
                f.waypoint(&g, None, goal, at, body, &mut budget, &mut || 0)
            };
            assert!(step(&mut f, [40.0, 0.0, 0.0], Footing::STANDING).is_some());
            assert_eq!(step(&mut f, [70.0, 200.0, z], air), None);
            assert_eq!((f.fell, f.avoid.len() == 1), (avoided, avoided), "z {z}");
        }
    }

    /// A plan round a stuck edge takes no risky drop: with the only plain
    /// way avoided, the plan goes through the stuck edge again, as
    /// mp_ship's crow's nest's ladder after a meeting at its head, not
    /// down the hatch beside it. With nothing avoided, the risky drop is
    /// still the way when it is the only one.
    #[test]
    fn a_plan_round_a_stuck_edge_takes_no_risky_drop() {
        use crate::bots::Goal;
        // 0 -> 1 -> 2 plain, 0 -> 2 a risky drop.
        let nodes = vec![
            Vec3::ZERO,
            Vec3::new(64.0, 64.0, 0.0),
            Vec3::new(128.0, 0.0, 0.0),
        ];
        let mut g = NavGraph::from_parts(nodes, vec![vec![1, 2], vec![2], vec![]]);
        g.risky.insert((0, 2));
        g.index_components();
        let goal = Goal::To([128.0, 0.0, 0.0]);
        let mut f = Follower::default();
        f.avoid.push((0, 1, u32::MAX));
        let mut budget = 100;
        let at = [0.0, 0.0, 0.0];
        let plan = (0..4).find_map(|_| {
            f.waypoint(
                &g,
                None,
                goal,
                at,
                Footing::STANDING,
                &mut budget,
                &mut || 0,
            )
        });
        assert_eq!(plan, Some([64.0, 64.0, 0.0]), "through the stuck edge");
        let mut s = Search::new(&g, 0, 2, vec![(0, 1)]);
        assert!(matches!(s.run(&g, &mut 100), Plan::Failed));
        let mut s = Search::new(&g, 0, 2, Vec::new());
        assert!(matches!(s.run(&g, &mut 100), Plan::Found(p) if p == [0, 1, 2]));
        g.edges[0] = vec![2];
        let mut s = Search::new(&g, 0, 2, Vec::new());
        assert!(matches!(s.run(&g, &mut 100), Plan::Found(p) if p == [0, 2]));
    }

    /// Round only the edges that failed twice, the plan may take a risky
    /// drop: with the one plain way out of mp_ship's deck pocket at (3560,
    /// -60, 352) dead, plans with no drop failed and the bots stood there
    /// 10 s a time (bot-navigation.md, "The ship's leftovers").
    #[test]
    fn a_plan_round_a_dead_edge_takes_a_risky_drop() {
        use crate::bots::Goal;
        let nodes = vec![
            Vec3::ZERO,
            Vec3::new(64.0, 64.0, 0.0),
            Vec3::new(128.0, 0.0, 0.0),
        ];
        let mut g = NavGraph::from_parts(nodes, vec![vec![1, 2], vec![2], vec![]]);
        g.risky.insert((0, 2));
        g.index_components();
        let mut f = Follower::default();
        f.avoid.push((0, 1, u32::MAX));
        f.dead.push((0, 1, u32::MAX));
        let mut budget = 100;
        let plan = (0..4).find_map(|_| {
            f.waypoint(
                &g,
                None,
                Goal::To([128.0, 0.0, 0.0]),
                [0.0, 0.0, 0.0],
                Footing::STANDING,
                &mut budget,
                &mut || 0,
            )
        });
        assert_eq!(plan, Some([128.0, 0.0, 0.0]), "down the drop");
    }

    /// A point goal within [`RETARGET`] of the destination, a floor over
    /// it, is planned for again: a bot kept its hold floor node for a
    /// health item on mp_ship's ledge 116 over it and paced under it.
    #[test]
    fn a_goal_a_floor_off_its_destination_is_planned_for_again() {
        use crate::bots::Goal;
        let nodes = vec![
            Vec3::ZERO,
            Vec3::new(64.0, 0.0, 0.0),
            Vec3::new(64.0, 32.0, 100.0),
        ];
        let mut g = NavGraph::from_parts(nodes, vec![vec![1], vec![0, 2], vec![1]]);
        g.index_components();
        let mut f = Follower::default();
        let wp = |f: &mut Follower, p| {
            f.waypoint(
                &g,
                None,
                Goal::To(p),
                [0.0, 0.0, 0.0],
                Footing::STANDING,
                &mut 100,
                &mut || 0,
            )
        };
        wp(&mut f, [64.0, 0.0, 0.0]);
        assert_eq!(f.dest, Some(1));
        wp(&mut f, [64.0, 32.0, 100.0]);
        assert_eq!(f.dest, Some(2));
    }

    /// Node 0 to node 1 4000 units on, by a way `north` units off the line
    /// (nodes 2 to 9) or one `south` off it (10 to 17).
    fn two_ways(north: f32, south: f32) -> NavGraph {
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

    /// An edge stuck on again while it is avoided, planned through for want
    /// of a way round, is planned through no more until it expires: the
    /// bot gets no waypoint rather than the same edge again.
    #[test]
    fn an_edge_stuck_on_again_while_avoided_is_not_planned_through() {
        use crate::bots::Goal;
        let nodes = vec![Vec3::ZERO, Vec3::new(64.0, 0.0, 0.0)];
        let mut g = NavGraph::from_parts(nodes, vec![vec![1], vec![]]);
        g.index_components();
        let goal = Goal::To([64.0, 0.0, 0.0]);
        let plan = |f: &mut Follower| {
            let mut budget = 100;
            (0..4).find_map(|_| {
                f.waypoint(
                    &g,
                    None,
                    goal,
                    [0.0; 3],
                    Footing::STANDING,
                    &mut budget,
                    &mut || 0,
                )
            })
        };
        let mut f = Follower::default();
        f.avoid_edge(0, 1);
        assert_eq!(plan(&mut f), Some([64.0, 0.0, 0.0]), "through it once");
        f.reset();
        f.avoid_edge(0, 1);
        assert_eq!(plan(&mut f), None, "not again");
        f.clock += AVOID_TICKS;
        f.rest = 0;
        assert_eq!(plan(&mut f), Some([64.0, 0.0, 0.0]), "once it expires");
    }

    /// A point goal that moves off and comes back within `PARK_TICKS`
    /// takes up its old plan where it was, with no new search; later, it
    /// plans afresh.
    #[test]
    fn a_goal_back_within_a_second_takes_up_its_old_plan() {
        use crate::bots::Goal;
        let g = row();
        let mut f = Follower::default();
        let wp = |f: &mut Follower, goal, mut budget| {
            f.waypoint(
                &g,
                None,
                goal,
                [40.0, 0.0, 0.0],
                Footing::STANDING,
                &mut budget,
                &mut || 0,
            )
        };
        let far = Goal::To([160.0, 0.0, 0.0]);
        let first = wp(&mut f, far, 100);
        assert!(first.is_some());
        assert!(wp(&mut f, Goal::To([0.0, 0.0, 0.0]), 100).is_some());
        assert_eq!(wp(&mut f, far, 0), first, "no budget, the old plan");
        // Flipped away again and left there past the wait: no plan without
        // a search.
        wp(&mut f, Goal::To([0.0, 0.0, 0.0]), 100);
        for _ in 0..=PARK_TICKS {
            wp(&mut f, Goal::To([0.0, 0.0, 0.0]), 100);
        }
        assert_eq!(wp(&mut f, far, 0), None);
    }

    /// With no waypoint while the next plan is searched for, the body waits
    /// on the last one handed out, near it on its floor, for a while.
    #[test]
    fn a_body_waits_on_its_last_waypoint_while_a_plan_is_searched_for() {
        use crate::bots::Goal;
        let g = row();
        let mut f = Follower::default();
        let wp = |f: &mut Follower, goal, mut budget| {
            f.waypoint(
                &g,
                None,
                goal,
                [0.0; 3],
                Footing::STANDING,
                &mut budget,
                &mut || 0,
            )
        };
        assert_eq!(
            wp(&mut f, Goal::To([160.0, 0.0, 0.0]), 100),
            Some([32.0, 0.0, 0.0])
        );
        // A new goal, and no budget left this tick to plan for it.
        assert_eq!(wp(&mut f, Goal::To([0.0, 0.0, 0.0]), 0), None);
        let at = [10.0, 0.0, 0.0];
        assert_eq!(f.rest_point(at, Footing::STANDING), Some([32.0, 0.0, 0.0]));
        let rungs = Footing {
            on_ground: false,
            on_ladder: true,
            ..Footing::STANDING
        };
        assert_eq!(f.rest_point(at, rungs), None, "not on a ladder");
        assert_eq!(
            f.rest_point([10.0, 0.0, 40.0], Footing::STANDING),
            None,
            "a floor up"
        );
        for _ in 0..WAIT_TICKS {
            wp(&mut f, Goal::To([0.0, 0.0, 0.0]), 0);
        }
        assert_eq!(f.rest_point(at, Footing::STANDING), None, "waited out");
    }

    /// The plan round a body in the way is waited for where the body
    /// stands, however far its last waypoint: on mp_ship's spar a bot ran
    /// a random heading meanwhile and fell off it (seed 1381).
    #[test]
    fn a_body_waits_where_it_stands_for_the_plan_round_a_body() {
        let mut f = Follower::default();
        let at = [500.0, 0.0, 0.0];
        assert_eq!(f.rest_point(at, Footing::STANDING), None);
        f.shun = Arc::from([3u32]);
        assert_eq!(f.rest_point(at, Footing::STANDING), Some(at));
        let rungs = Footing {
            on_ground: false,
            on_ladder: true,
            ..Footing::STANDING
        };
        assert_eq!(f.rest_point(at, rungs), None, "not on a ladder");
    }

    /// The run off mp_ship's mast platform at (3564, -127, 896) onto the
    /// narrow ledge at 760 lands at the graph's 190 units/s and flies over
    /// it at a light weapon's 228, to the ledge at 480 that killed 15
    /// bots in 384 seeds: why a following bot runs at the graph's pace
    /// (`paced_key`).
    #[test]
    fn the_run_off_the_mast_platform_lands_only_at_the_graphs_pace() {
        let Some(world) = map_world("mp_ship") else {
            return;
        };
        let top = Vec3::new(3564.0, -127.0, 896.125);
        let ledge = Vec3::new(3520.0, -162.0, 760.0);
        let run = |scale: f32| {
            let pace = (ledge - top).truncate().normalize() * 190.0 * scale;
            walk_sliding(
                &world.collision,
                top,
                Some(pace.extend(0.0)),
                scale,
                ledge.truncate(),
                Some(ledge.z),
                Gait::Forward,
                &mut Trail::default(),
            )
        };
        assert!(matches!(run(1.0), Walked::Arrived(..)));
        assert!(matches!(run(1.2), Walked::Fell));
        assert_eq!(paced_key(127, 1.2), 106);
    }

    /// mp_ship's deck at (3515, -323, 631) has an edge to the narrow ledge
    /// at 580 that no gait lands node to node: bots ran it and fell past
    /// the ledge to the hold. It is risky.
    #[test]
    fn a_drop_no_gait_lands_node_to_node_is_risky() {
        let Some(world) = map_world("mp_ship") else {
            return;
        };
        let g = graph_for("mp_ship", &world);
        let deck = g.nearest([3515.0, -323.0, 631.0]).unwrap();
        let ledge = g.nearest([3488.0, -358.0, 580.0]).unwrap();
        assert!(g.edges[deck as usize].contains(&ledge));
        let (p, q) = (g.nodes[deck as usize], g.nodes[ledge as usize]);
        for gait in [Gait::Forward, Gait::Backward, Gait::Creep] {
            assert!(!matches!(
                walk_as(&world.collision, p, q.truncate(), Some(q.z), gait),
                Walked::Arrived(..)
            ));
        }
        assert!(g.risky.contains(&(deck, ledge)));
    }

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
        assert!(g.edges[roof as usize].contains(&ridge));
        assert!(g.risky.contains(&(roof, ridge)));
        let hull = g.nearest([3839.6, -611.7, 54.125]).unwrap();
        let below = g.nearest([3875.0, -578.4, 27.13]).unwrap();
        assert!(g.edges[hull as usize].contains(&below));
        assert!(!g.risky.contains(&(hull, below)));
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
            assert!(g.edges[exit as usize].contains(&below));
            assert!(g.risky.contains(&(exit, below)));
        }
        let (head, foot) = (
            node([4963.9, 64.0, 1219.125]),
            node([4959.0, 64.0, 692.125]),
        );
        assert!(g.edges[head as usize].contains(&foot));
        assert!(!g.risky.contains(&(head, foot)));
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
            let Some(w) = f.waypoint(
                &g,
                None,
                goal,
                at,
                Footing::STANDING,
                &mut budget,
                &mut rand,
            ) else {
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

    /// A two-way chain of nodes 0.125 over `test_world`'s floor: the grid's
    /// stair-step across a diagonal, east then north-east by turns.
    fn zigzag() -> NavGraph {
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

    /// Along plain edges the waypoint is pulled on to the furthest node a
    /// straight walk reaches, one a tick: a pillar on the line holds it at
    /// the grid's next node, and so does a jump edge.
    #[test]
    fn a_waypoint_is_pulled_along_a_clear_straight_line() {
        use crate::bots::Goal;
        let goal = Goal::To([160.0, 64.0, 0.125]);
        let run = |g: &NavGraph, world: &CollisionWorld, at: [[f32; 2]; 2]| {
            let mut f = Follower::default();
            let mut budget = 100;
            at.map(|[x, y]| {
                f.waypoint(
                    g,
                    Some(world),
                    goal,
                    [x, y, 0.125],
                    Footing::STANDING,
                    &mut budget,
                    &mut || 0,
                )
                .map(|w| [w[0], w[1]])
            })
        };
        let open = vcod_common::collision::test_world(&[]);
        let at = [[0.0, 0.0], [40.0, 10.0]];
        assert_eq!(
            run(&zigzag(), &open, at),
            [Some([64.0, 32.0]), Some([96.0, 32.0])]
        );
        // Clear of node 1's capsule, across the line from node 0 to node 2.
        let pillar = vcod_common::collision::test_world(&[(
            Vec3::new(24.0, 20.0, 0.0),
            Vec3::new(40.0, 28.0, 100.0),
        )]);
        assert_eq!(run(&zigzag(), &pillar, at)[0], Some([32.0, 0.0]));
        let mut g = zigzag();
        g.jumps.insert((1, 2));
        assert_eq!(run(&g, &open, at)[0], Some([32.0, 0.0]));
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
            Footing::STANDING,
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
            Footing::STANDING,
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
            let w = f.waypoint(
                &g,
                None,
                goal,
                [0.0; 3],
                Footing::STANDING,
                &mut budget,
                &mut || 0,
            );
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
            Footing::STANDING,
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
            Footing::STANDING,
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
        let w = f.waypoint(
            &g,
            None,
            goal,
            [64.0, 0.0, 0.0],
            Footing::STANDING,
            &mut 100,
            &mut rand,
        );
        assert_eq!(f.dest, Some(5));
        assert_eq!(w, Some([96.0, 0.0, 0.0]));
        // The threat moves past the far end: the flight turns round.
        let goal = crate::bots::Goal::Away([300.0, 0.0, 0.0]);
        f.waypoint(
            &g,
            None,
            goal,
            [64.0, 0.0, 0.0],
            Footing::STANDING,
            &mut 100,
            &mut rand,
        );
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
            last = f.waypoint(
                &g,
                None,
                goal,
                at,
                Footing::STANDING,
                &mut budget,
                &mut || 0,
            );
        }
        assert_eq!(last, None, "still steering at a waypoint it never nears");
        let w = f.waypoint(
            &g,
            None,
            goal,
            at,
            Footing::STANDING,
            &mut budget,
            &mut || 0,
        );
        assert_eq!(
            w,
            Some([60.0, 30.0, 0.0]),
            "planned through the blocked edge"
        );
    }

    /// The bot's own stall circle sends it on a spell: the edge under way
    /// is planned round, and the plan from wherever the spell leaves it
    /// does not take it again.
    #[test]
    fn a_spell_plans_round_the_edge_it_began_on() {
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
        let mut at = |f: &mut Follower, p: [f32; 3]| {
            f.waypoint(&g, None, goal, p, Footing::STANDING, &mut budget, &mut || 0)
        };
        assert_eq!(at(&mut f, [32.0, 0.0, 0.0]), Some([64.0, 0.0, 0.0]));
        f.stuck();
        assert_eq!(f.current(), (None, None), "the path is dropped");
        assert_eq!(
            at(&mut f, [32.0, 0.0, 0.0]),
            Some([60.0, 30.0, 0.0]),
            "round the edge it was stuck on"
        );
    }

    /// mp_ship's crow's nest: a bot planned from the top at 1254, walked off
    /// it onto the hatch's rim at 1219 while the search ran, and headed
    /// back for the top's node 35 over its feet with no way up from there;
    /// it pinned on the top's face and slid down the hatch. A plan whose
    /// first node is a floor over the feet is planned again from a node a
    /// walk reaches.
    #[test]
    fn a_plan_that_starts_a_floor_up_is_planned_again() {
        let Some(world) = map_world("mp_ship") else {
            return;
        };
        let g = graph_for("mp_ship", &world);
        let w = Some(&world.collision);
        // Down on the deck: a point goal on the rail itself is walked to
        // the rim now (`NavGraph::goal_node`).
        let goal = crate::bots::Goal::To([4929.0, 68.0, 692.125]);
        let mut f = Follower::default();
        let top = [4928.5, 60.75, 1254.125];
        let rim = [4961.2, 38.7, 1219.125];
        let mut budget = 1;
        assert_eq!(
            f.waypoint(&g, w, goal, top, Footing::STANDING, &mut budget, &mut || 0),
            None,
            "the search runs on"
        );
        let at = |f: &mut Follower| {
            let mut budget = 100_000;
            f.waypoint(&g, w, goal, rim, Footing::STANDING, &mut budget, &mut || 0)
        };
        assert_eq!(at(&mut f), None, "the plan from the top is dropped");
        let wp = at(&mut f).expect("a plan from the rim");
        assert!(wp[2] < rim[2] + 1.0, "{wp:?}");
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
            f.waypoint(
                &g,
                None,
                goal,
                at,
                Footing::STANDING,
                &mut budget,
                &mut || 0
            ),
            Some([40.0, 0.0, 120.0])
        );
        assert_eq!(
            f.waypoint(
                &g,
                None,
                goal,
                at,
                Footing::STANDING,
                &mut budget,
                &mut || 0
            ),
            Some([40.0, 0.0, 120.0])
        );
        // Fell to the floor below: a new plan back up, its waypoints nodes.
        let below = [34.0, 0.0, 0.0];
        let before = budget;
        let w = f.waypoint(
            &g,
            None,
            goal,
            below,
            Footing::STANDING,
            &mut budget,
            &mut || 0,
        );
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
        let mut at = |p: [f32; 3]| {
            f.waypoint(
                &g,
                None,
                goal,
                p,
                Footing {
                    still: false,
                    on_ground: true,
                    on_ladder: false,
                    velocity: Vec3::ZERO,
                },
                &mut budget,
                &mut || 0,
            )
        };
        assert_eq!(at([64.0, 0.0, -64.0]), Some([32.0, 0.0, -48.0]));
        assert_eq!(at([12.0, 0.0, -48.0]), Some([0.0, 0.0, -32.0]));
        assert_eq!(at([2.0, 0.0, -32.0]), Some([-48.0, 0.0, -64.0]));
    }

    /// A body in the air beside a jump's top is nearer the next node than
    /// the top is, and still not on it.
    #[test]
    fn a_jumps_top_is_not_cut_past_in_the_air() {
        let mut g = NavGraph::from_parts(
            vec![
                Vec3::new(0.0, 0.0, 0.0),
                Vec3::new(32.0, 0.0, 36.0),
                Vec3::new(32.0, 64.0, 36.0),
            ],
            vec![vec![1], vec![0, 2], vec![1]],
        );
        g.jumps.insert((0, 1));
        let mut f = Follower::default();
        let goal = crate::bots::Goal::To([32.0, 64.0, 36.0]);
        let mut budget = 100;
        let mut at = |p: [f32; 3]| {
            f.waypoint(
                &g,
                None,
                goal,
                p,
                Footing {
                    still: false,
                    on_ground: true,
                    on_ladder: false,
                    velocity: Vec3::ZERO,
                },
                &mut budget,
                &mut || 0,
            )
        };
        assert_eq!(at([0.0, 0.0, 0.0]), Some([32.0, 0.0, 36.0]));
        // 25 flat from the top, 15 under it, mid-jump.
        assert_eq!(at([20.0, 22.0, 30.0]), Some([32.0, 0.0, 36.0]));
        assert_eq!(at([32.0, 2.0, 36.0]), Some([32.0, 64.0, 36.0]));
    }

    /// mp_ship's ledge at (3347, 250, 616): a bot passed the jump's top at
    /// 652 from 15 under it in the air, within reach flat, headed on for
    /// the floor at 664 and fell back to the deck. The top is passed only
    /// on the ground.
    #[test]
    fn a_jumps_top_is_passed_only_on_the_ground() {
        let mut g = NavGraph::from_parts(
            vec![
                Vec3::new(3361.3, 256.0, 616.125),
                Vec3::new(3366.2, 224.0, 652.125),
                Vec3::new(3328.2, 252.4, 664.125),
            ],
            vec![vec![1], vec![0, 2], vec![1]],
        );
        g.jumps.insert((0, 1));
        let mut f = Follower::default();
        let goal = crate::bots::Goal::To([3328.2, 252.4, 664.125]);
        let mut budget = 100;
        let top = Some([3366.2, 224.0, 652.125]);
        let mut at = |p: [f32; 3], on_ground: bool| {
            let body = Footing {
                still: false,
                on_ground,
                on_ladder: false,
                velocity: Vec3::ZERO,
            };
            f.waypoint(&g, None, goal, p, body, &mut budget, &mut || 0)
        };
        assert_eq!(at([3367.5, 239.1, 616.125], true), top);
        assert_eq!(at([3349.2, 239.1, 637.1], false), top, "in the air");
        assert_eq!(
            at([3366.0, 230.0, 652.125], true),
            Some([3328.2, 252.4, 664.125])
        );
    }

    /// mp_hurtgen's slope west of the crease at (437, 2824, -97): a body
    /// left at rest there, or on the slope above it where retail's
    /// `probe_wedge` slid into it, comes to rest in the air; on the patch
    /// of the node (428.2, 2783.2, -60.3) it stands. The run from the node
    /// (385.2, 2831.8, -22.2) down to (385.6, 2785.7, -52.6) arrives, but
    /// over spots that drain into the crease, so it proves no edge.
    #[test]
    fn a_slide_over_ground_that_drains_into_a_crease_does_not_hold() {
        let Some(world) = map_world("mp_hurtgen") else {
            return;
        };
        let w = &world.collision;
        let held = |x: f32, y: f32, z: f32| {
            held_off_ground(
                w,
                PlayerState::spawn(Vec3::new(x, y, z), 0.0),
                Sim::default(),
            )
        };
        assert!(held(436.8, 2824.3, -97.2));
        assert!(held(416.7, 2799.0, -60.0));
        assert!(held(392.1, 2797.3, -55.1));
        assert!(!held(428.2, 2783.2, -59.0));
        // Retail's `probe_wedge` holds a body left at the walk's first slid
        // spot and lands one at its second (bot-navigation.md section 3).
        assert!(drains(w, Vec3::new(385.4227, 2813.2952, -29.899721)));
        assert!(!drains(w, Vec3::new(383.21103, 2802.2563, -42.117294)));
        let node = |x: f32, y: f32, z: f32| settle(w, Vec3::new(x, y, z + 2.0), 0.0).unwrap();
        let (top, patch) = (node(385.2, 2831.8, -22.2), node(385.6, 2785.7, -52.6));
        let mut trail = Trail::default();
        let straight = walk_sliding(
            w,
            top,
            None,
            1.0,
            patch.truncate(),
            None,
            Gait::Forward,
            &mut trail,
        );
        assert!(matches!(straight, Walked::Arrived(..)) && !trail.slid.is_empty());
        let walked = walk_with(w, top, None, patch.truncate(), None, Gait::Forward);
        assert!(!matches!(walked, Walked::Arrived(..)));
    }

    /// mp_depot's ladder at (-417, 1112) up to the roof at 133: a walk
    /// from its foot to the floor at (-484, 1051), 7 units below a bracket
    /// and a wall, climbed the rungs, crossed the roof and dropped back to
    /// the floor beside the target. A bot heading there pins in the corner
    /// at (-433, 1129, -26) instead.
    #[test]
    fn a_level_walk_over_a_ladders_top_proves_no_edge() {
        let Some(world) = map_world("mp_depot") else {
            return;
        };
        let w = &world.collision;
        let (foot, floor) = (
            Vec3::new(-416.5, 1130.0, -31.875),
            Vec3::new(-484.1, 1051.2, -31.875),
        );
        let walked = walk_with(
            w,
            foot,
            None,
            floor.truncate(),
            Some(floor.z),
            Gait::Forward,
        );
        assert!(!matches!(walked, Walked::Arrived(..)));
        // The same walk without a floor arrives on the roof, over it.
        let roof = walk_with(w, foot, None, floor.truncate(), None, Gait::Forward);
        assert!(matches!(roof, Walked::Arrived(p, _) if p.z > 100.0));
    }

    /// mp_hurtgen, seed 43: a bot slid down the terrain west of the crease
    /// at (437, 2824, -97), in the air from (379, 2807, -34) on, turned for
    /// the node after (381.7, 2790.8) 12 units past it and for the next
    /// 22 past that, never landing on either patch, and slid into the
    /// crease. A node level with the body is passed only on the ground.
    #[test]
    fn a_node_is_not_passed_sliding_past_it_in_the_air() {
        let g = NavGraph::from_parts(
            vec![
                Vec3::new(381.7, 2790.8, -53.6),
                Vec3::new(428.2, 2783.2, -60.3),
                Vec3::new(480.1, 2833.2, -123.2),
            ],
            vec![vec![1], vec![2], vec![]],
        );
        let mut f = Follower::default();
        let goal = crate::bots::Goal::To([480.1, 2833.2, -123.2]);
        let mut budget = 100;
        let mut at = |p: [f32; 3], on_ground: bool| {
            let body = Footing {
                still: false,
                on_ground,
                on_ladder: false,
                velocity: Vec3::ZERO,
            };
            f.waypoint(&g, None, goal, p, body, &mut budget, &mut || 0)
        };
        let slid = [392.1, 2797.3, -55.1];
        assert_eq!(at(slid, false), Some([381.7, 2790.8, -53.6]));
        assert_eq!(at(slid, true), Some([428.2, 2783.2, -60.3]));
    }

    /// mp_ship, seed 693: from (3365, -323, 352) the bot was nearer the
    /// ladder's foot (3370, -380, 56) than the head (3298, -380, 352) it
    /// backs down from, cut past the head and pinned on the wall at
    /// y -333 above the foot. A drop's top is passed only on reaching it.
    #[test]
    fn a_drops_top_is_not_cut_past() {
        let g = NavGraph::from_parts(
            vec![
                Vec3::new(3353.0, -321.0, 352.0),
                Vec3::new(3298.0, -380.0, 352.0),
                Vec3::new(3370.0, -380.0, 56.0),
            ],
            vec![vec![1], vec![2], vec![]],
        );
        let mut f = Follower::default();
        let goal = crate::bots::Goal::To([3370.0, -380.0, 56.0]);
        let mut budget = 100;
        let mut at =
            |p: [f32; 3]| f.waypoint(&g, None, goal, p, Footing::STANDING, &mut budget, &mut || 0);
        assert_eq!(at([3353.0, -321.0, 352.0]), Some([3298.0, -380.0, 352.0]));
        assert_eq!(at([3365.4, -324.2, 352.1]), Some([3298.0, -380.0, 352.0]));
    }

    /// mp_ship's hold ladder: a bot backing down the rungs passed the node
    /// of the floor at 304 beside it from 42 above, headed level for the
    /// next node and hung on the rungs under the floor's lip. On a ladder a
    /// node below is passed only from a step over its floor.
    #[test]
    fn a_node_below_the_rungs_is_passed_only_from_its_floor() {
        let g = NavGraph::from_parts(
            vec![
                Vec3::new(4349.5, -93.1, 600.125),
                Vec3::new(4318.5, -96.0, 304.125),
                Vec3::new(4280.5, -96.1, 304.125),
            ],
            vec![vec![1], vec![2], vec![1]],
        );
        let mut f = Follower::default();
        let goal = crate::bots::Goal::To([4280.5, -96.1, 304.125]);
        let mut budget = 100;
        let floor = Some([4318.5, -96.0, 304.125]);
        let mut at = |p: [f32; 3]| {
            let body = Footing {
                still: false,
                on_ground: false,
                on_ladder: true,
                velocity: Vec3::ZERO,
            };
            f.waypoint(&g, None, goal, p, body, &mut budget, &mut || 0)
        };
        assert_eq!(at([4349.5, -93.1, 600.125]), floor);
        assert_eq!(at([4328.5, -93.1, 346.3]), floor, "42 above");
        assert_eq!(at([4322.0, -93.1, 315.0]), Some([4280.5, -96.1, 304.125]));
    }

    /// mp_ship's hull beam: from its edge node a walk over the beam to the
    /// floor beyond arrives, from 16 units north along the edge it slides
    /// back off the slope. A bot that passed the node there stalled 120
    /// ticks.
    #[test]
    fn a_walk_on_proved_from_the_node_is_proved_again_from_the_body() {
        let Some(world) = map_world("mp_ship") else {
            return;
        };
        let w = &world.collision;
        let edge = Vec3::new(2247.1, 511.2, -47.875);
        let beyond = Vec3::new(2203.5, 511.4, -63.875);
        let g = NavGraph::from_parts(vec![edge, beyond], vec![vec![1], vec![0]]);
        let mut f = Follower::default();
        let goal = crate::bots::Goal::To(beyond.into());
        let mut budget = 100;
        // Running at the floor beyond, as a follower on the path does.
        let mut at = |p: Vec3| {
            let run = (beyond - p).truncate().normalize() * vcod_common::pmove::SPEED_RUN;
            f.waypoint(
                &g,
                Some(w),
                goal,
                p.into(),
                Footing {
                    still: false,
                    on_ground: true,
                    on_ladder: false,
                    velocity: run.extend(0.0),
                },
                &mut budget,
                &mut || 0,
            )
        };
        // In reach of the edge node along the ledge, but the walk on slides
        // back from here: close in on the node.
        let along = Vec3::new(2256.2498, 523.75616, -47.87189);
        assert!(!matches!(
            walk_as(w, along, beyond.truncate(), Some(beyond.z), Gait::Forward),
            Walked::Arrived(..)
        ));
        assert_eq!(at(along), Some(edge.into()));
        assert_eq!(at(edge), Some(beyond.into()));
    }

    /// mp_depot by (-64, 352): a body run off a ledge at z 12 comes to rest
    /// on a step's corner at z 3, right under a floor at z 27 and under a
    /// ceiling 37 over the feet. A flood walk resting there was merged into
    /// the floor's node until 2026-10-09; the walk to the node itself jumps.
    #[test]
    fn a_walk_jumps_onto_a_floor_right_overhead_under_a_ceiling() {
        let Some(world) = map_world("mp_depot") else {
            return;
        };
        let w = &world.collision;
        let from = Vec3::new(-28.519163, 352.00345, 12.125);
        let node = Vec3::new(-64.23049, 352.02036, 27.125);
        let free = walk_as(w, from, node.truncate(), None, Gait::Forward);
        assert!(matches!(free, Walked::Arrived(p, false) if (p.z - 3.125).abs() < 0.5));
        let on = walk_as(w, from, node.truncate(), Some(node.z), Gait::Forward);
        assert!(matches!(on, Walked::Arrived(p, true) if (p.z - node.z).abs() < 0.5));
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
        // floor is a step off: from further down the rungs the next node
        // is no way on.
        while at[2] <= 182.0 {
            let w = f.waypoint(
                &g,
                None,
                goal,
                at,
                Footing::STANDING,
                &mut budget,
                &mut || 0,
            );
            assert_eq!(w, Some([10.0, 0.0, 200.0]), "at z {}", at[2]);
            at[2] += 2.5;
        }
        at[0] = 10.0;
        at[2] = 200.0;
        // Level with it but still on the rungs: turning for the next node
        // there lets go of them (mp_ship's mast).
        let climbing = Footing {
            still: false,
            on_ground: false,
            on_ladder: true,
            velocity: Vec3::ZERO,
        };
        let w = f.waypoint(&g, None, goal, at, climbing, &mut budget, &mut || 0);
        assert_eq!(w, Some([10.0, 0.0, 200.0]), "on the rungs");
        let w = f.waypoint(
            &g,
            None,
            goal,
            at,
            Footing::STANDING,
            &mut budget,
            &mut || 0,
        );
        assert_eq!(w, Some([40.0, 0.0, 200.0]), "on the head's floor");
    }

    /// Two climbers abreast on the rungs, 30 apart: exactly one gives way,
    /// the lower, or of two level the later in (y, x) order. One a body's
    /// height above, or off to the side, or a bot not climbing, changes
    /// nothing (mp_ship's hatch over the ladder at x 3792, seed 417).
    #[test]
    fn of_two_climbers_abreast_one_gives_way() {
        let g = NavGraph::from_parts(
            vec![
                Vec3::new(3808.0, 65.0, 304.0),
                Vec3::new(3783.0, 64.0, 623.0),
            ],
            vec![vec![1], vec![]],
        );
        let f = Follower {
            path: vec![0, 1],
            next: 1,
            ..Follower::default()
        };
        let rungs = Footing {
            still: false,
            on_ground: false,
            on_ladder: true,
            velocity: Vec3::new(0.0, 0.0, 53.0),
        };
        let (a, b) = (
            Vec3::new(3808.1, 46.2, 600.0),
            Vec3::new(3808.1, 76.3, 600.0),
        );
        let up = Vec3::Z * 53.0;
        assert!(!f.abreast_yield(&g, a, rungs, &[(b, up)]), "first in y");
        assert!(f.abreast_yield(&g, b, rungs, &[(a, up)]), "gives way");
        let higher = b + Vec3::Z * 10.0;
        assert!(
            f.abreast_yield(&g, a, rungs, &[(higher, up)]),
            "lower gives way"
        );
        assert!(!f.abreast_yield(&g, higher, rungs, &[(a, up)]));
        let above = b + Vec3::Z * 80.0;
        assert!(
            !f.abreast_yield(&g, a, rungs, &[(above, up)]),
            "in a column"
        );
        let aside = b + Vec3::Y * 10.0;
        assert!(!f.abreast_yield(&g, a, rungs, &[(aside, up)]), "40 apart");
        assert!(
            !f.abreast_yield(&g, b, Footing::STANDING, &[(a, up)]),
            "off the rungs"
        );
        let down = Follower {
            path: vec![1, 0],
            next: 1,
            ..Follower::default()
        };
        assert!(
            !down.abreast_yield(&g, b, rungs, &[(a, up)]),
            "climbing down"
        );
    }

    /// A climb from a foot square in front of a ladder runs into the face
    /// until it is level with the head; from a foot off to the side it runs
    /// at the head, the way the walk that linked it went.
    #[test]
    fn a_climb_runs_into_the_face_from_a_foot_in_front_of_it() {
        let mut g = NavGraph::from_parts(
            vec![
                Vec3::new(0.0, 0.0, 0.0),
                Vec3::new(-40.0, 10.0, 200.0),
                Vec3::new(0.0, 40.0, 0.0),
            ],
            vec![vec![1], vec![0], vec![1]],
        );
        // A ladder plate 1 thick at x -17, 24 wide along y.
        g.ladder_boxes = vec![(Vec3::new(-18.0, -12.0, -8.0), Vec3::new(-17.0, 12.0, 200.0))];
        let goal = crate::bots::Goal::To([-40.0, 10.0, 200.0]);
        let mut f = Follower::default();
        let mut budget = 100;
        let w = |f: &mut Follower, budget: &mut u32, at: [f32; 3]| {
            f.waypoint(&g, None, goal, at, Footing::STANDING, budget, &mut || 0)
        };
        assert_eq!(w(&mut f, &mut budget, [0.0; 3]), Some([-17.0, 0.0, 200.0]));
        assert_eq!(
            w(&mut f, &mut budget, [-2.0, 0.0, 190.0]),
            Some([-40.0, 10.0, 200.0]),
            "level with the head"
        );
        let mut f = Follower::default();
        let at = [0.0, 40.0, 0.0];
        assert_eq!(w(&mut f, &mut budget, at), Some([-40.0, 10.0, 200.0]));
        assert_eq!(
            g.ladder_middle(Vec3::new(-2.0, 9.0, 100.0)),
            Some(glam::Vec2::new(-2.0, 0.0))
        );
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
    /// that got no rung before 2026-10-07. The climb under the mast's spar
    /// ends on the spar's edge at 1147 beside the plate, and the creep back
    /// off it at a run is 40 units out before it has dropped the 8 under the
    /// plate `PM_CheckLadderMove`'s box needs (2026-10-12): up only.
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

    /// The walk's jump: two pinned ticks where a jump has room, a bob
    /// first where it has none and the body slides, then the next tick on
    /// the ground; right under the top at once.
    #[test]
    fn the_walks_jump_needs_two_pinned_ticks_and_room() {
        let (yes, no) = (|| true, || false);
        let mut p = PinJump::default();
        assert_eq!(p.after(true, true, true, false, false, yes), PinTick::Go);
        assert_eq!(
            p.after(true, false, true, false, false, yes),
            PinTick::Go,
            "moved"
        );
        assert_eq!(p.after(true, true, true, false, false, yes), PinTick::Go);
        assert_eq!(p.after(true, true, true, false, false, yes), PinTick::Jump);
        let mut p = PinJump::default();
        p.after(true, true, true, false, false, no);
        assert_eq!(
            p.after(true, true, true, false, false, no),
            PinTick::Stuck,
            "no room"
        );
        let mut p = PinJump::default();
        p.after(true, true, false, true, true, no);
        assert_eq!(
            p.after(true, true, false, true, true, no),
            PinTick::Go,
            "a bob"
        );
        assert!(!p.before(true, false, false, yes), "off the ground");
        assert!(p.before(true, true, false, yes), "out of the bob");
        let mut p = PinJump::default();
        assert!(p.before(true, true, true, yes), "right under the top");
        assert!(!p.before(false, true, true, yes), "disarmed");
    }

    #[test]
    fn a_leap_jumps_only_at_the_lip() {
        // flat, rise: the top 40 off and 32 up.
        assert!(!leap_cue(false, 40.0, 32.0), "pinned is no cue");
        assert!(leap_cue(true, 40.0, 32.0));
        // Right under a ledge's top, not under a step.
        assert!(jump_under(10.0, 32.0, 0.0));
        assert!(!jump_under(30.0, 32.0, 0.0), "still short of it");
        assert!(!jump_under(10.0, 10.0, 0.0), "under a step");
        assert!(!jump_under(10.0, 32.0, 2.0), "still climbing a ramp");
        // On the top: no more jumping.
        assert!(!leap_cue(true, 10.0, 2.0));
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
            f.waypoint(
                &g,
                None,
                goal,
                at,
                Footing {
                    still: false,
                    on_ground: true,
                    on_ladder: false,
                    velocity: Vec3::ZERO,
                },
                &mut budget,
                &mut || 0
            ),
            foot
        );
        assert!(f.holding(&g) && !f.jumping(&g));
        // Running past the foot, nearer the top than the foot is: held.
        let at = [40.0, 0.0, 0.0];
        assert_eq!(
            f.waypoint(
                &g,
                None,
                goal,
                at,
                Footing {
                    still: false,
                    on_ground: true,
                    on_ladder: false,
                    velocity: Vec3::ZERO,
                },
                &mut budget,
                &mut || 0
            ),
            foot
        );
        let at = [33.0, 0.0, 0.0];
        assert_eq!(
            f.waypoint(
                &g,
                None,
                goal,
                at,
                Footing::STANDING,
                &mut budget,
                &mut || 0
            ),
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

    /// A point goal on a perch is walked to the nearest node off the
    /// perches within [`RETARGET`], and the walked-out path heads at that
    /// node, not at the point: a chase onto mp_ship's crow's nest rails
    /// ended at a run on the rail as a roam once did.
    #[test]
    fn a_point_goal_on_a_perch_ends_beside_it() {
        let nodes = (0..3)
            .map(|i| Vec3::new(i as f32 * 32.0, 0.0, 0.0))
            .collect();
        let mut g = NavGraph::from_parts(nodes, vec![vec![1], vec![0, 2], vec![1]]);
        g.index_components();
        assert_eq!(g.goal_node([64.0, 0.0, 0.0]), Some(2));
        g.perches.insert(2);
        assert_eq!(g.goal_node([64.0, 0.0, 0.0]), Some(1));
        let f = Follower {
            dest: g.goal_node([64.0, 0.0, 0.0]),
            ..Follower::default()
        };
        assert_eq!(f.arrival(&g, [64.0, 0.0, 0.0]), [32.0, 0.0, 0.0]);
        assert_eq!(f.arrival(&g, [0.0, 0.0, 0.0]), [0.0, 0.0, 0.0]);
        let Some(world) = map_world("mp_ship") else {
            return;
        };
        let g = graph_for("mp_ship", &world);
        for rail in [[5023.8, 123.7, 1254.125], [4928.5, 60.8, 1254.125]] {
            let n = g.goal_node(rail).unwrap();
            assert!(!g.perches.contains(&n), "{rail:?}");
            assert!(g.nodes[n as usize].distance(Vec3::from(rail)) <= RETARGET);
        }
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
        g.risky.insert((1, 2));
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

    fn map_world(map: &str) -> Option<crate::world::World> {
        let fs = vcod_common::testing::game_fs()?;
        let entry = fs.resolve_map(map)?;
        let bsp = vcod_common::bsp::parse(&fs.read(&entry).unwrap()).unwrap();
        Some(crate::world::World::from_bsp(&bsp, Some(&fs)))
    }

    /// mp_ship, seed 5: a bot against the slanted wall at (6111, -304, 97)
    /// headed south slid along it and off the ledge 41 units to the deck.
    /// The sweep due south touches the wall at once; the slide finds it.
    /// mp_ship's hull slope north of (3762, 339, 352): no sample a jump
    /// under the one before, but 49 down within 75 units, to a wedge off
    /// the graph at (3752, 414, 303) that a seed 5 unstick heading ran a
    /// bot into.
    #[test]
    fn a_drop_ahead_is_felt_down_a_steep_slope() {
        let Some(world) = map_world("mp_ship") else {
            return;
        };
        let at = Vec3::new(3762.4, 338.9, 352.125);
        let depth = drop_ahead(&world.collision, at, Vec3::Y, 64.0, &mut Creases::default());
        assert!(depth.is_some_and(|d| d > 39.0), "{depth:?}");
    }

    #[test]
    fn a_drop_ahead_is_felt_along_a_wall_it_slides_on() {
        let Some(world) = map_world("mp_ship") else {
            return;
        };
        let at = Vec3::new(6095.754, -303.89618, 97.125);
        let depth = drop_ahead(
            &world.collision,
            at,
            -Vec3::Y,
            64.0,
            &mut Creases::default(),
        );
        assert!(depth.is_some_and(|d| (d - 41.0).abs() < 1.0), "{depth:?}");
        assert_eq!(
            drop_ahead(&world.collision, at, Vec3::Y, 64.0, &mut Creases::default()),
            None
        );
    }

    /// mp_hurtgen, seed 67: an unstick heading north-east from
    /// (411.7, 2786, -57.7) ran a bot down the slope into the crease at
    /// (437, 2824, -97). The slope falls under a jump within the look, so
    /// it reads as a drop only through the crease it drains into.
    #[test]
    fn a_slope_draining_into_a_crease_reads_as_a_drop_that_hurts() {
        let Some(world) = map_world("mp_hurtgen") else {
            return;
        };
        let at = Vec3::new(411.7, 2786.0, -57.7);
        let ne = Vec3::new(1.0, 1.0, 0.0).normalize();
        let mut creases = Creases::default();
        let depth = drop_ahead(&world.collision, at, ne, 64.0, &mut creases);
        assert_eq!(depth, Some(f32::INFINITY));
        // Down the slope west of its foot there is no crease.
        let w = drop_ahead(&world.collision, at, -Vec3::X, 64.0, &mut creases);
        assert_eq!(w, None);
    }

    /// mp_ship's deck lip at (3285.5, -448.5, 56.125), where seed 5's
    /// unstick heading walked a bot off to the hull floor at -64: the deck
    /// runs on east and north, the drop lies west and south.
    #[test]
    fn a_drop_ahead_is_felt_off_a_decks_lip() {
        let Some(world) = map_world("mp_ship") else {
            return;
        };
        let lip = Vec3::new(3285.5413, -448.488, 56.125);
        let drops: Vec<bool> = (0..8)
            .map(|i| {
                let (s, c) = (i as f32 * 45.0f32).to_radians().sin_cos();
                drop_ahead(
                    &world.collision,
                    lip,
                    Vec3::new(c, s, 0.0),
                    64.0,
                    &mut Creases::default(),
                )
                .is_some()
            })
            .collect();
        assert_eq!((drops[0], drops[2]), (false, false), "{drops:?}");
        assert_eq!((drops[4], drops[6]), (true, true), "{drops:?}");
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

    /// mp_depot, seed 7: a bot with a carbine (move scale over 1) at rest
    /// by the leap's foot on the step, running for the crate, covered 8.9
    /// units in the tick after the lip read clear off its pace of 5.1, and
    /// walked off the step's edge without the jump. The lip is the tick's
    /// move: it reads on the last tick on the ground, whatever the speed.
    #[test]
    fn the_lip_reads_on_the_last_tick_on_the_ground() {
        let Some(world) = map_world("mp_depot") else {
            return;
        };
        let fs = vcod_common::testing::game_fs().unwrap();
        let weapons = crate::weapons::WeaponTable::load(&fs, &crate::defs::V1);
        let w = &world.collision;
        let target = glam::Vec2::new(-1856.1, -538.4);
        let mut ps = PlayerState::spawn(Vec3::new(-1779.3, -607.2, 116.125), 0.0);
        ps.weapon = crate::configstrings::weapon_index("m1carbine_mp").unwrap() as u8;
        let mw = MoveWorld::bare(w);
        let mut last = None;
        for tick in 1..20 {
            let lip = lip_ahead(w, &ps, target, weapons.defs());
            let was = ps.on_ground;
            let to = target - ps.origin.truncate();
            let cmd = UserCmd {
                server_time: tick * FRAME_MS,
                forward: 127,
                angles: [0, (to.y.atan2(to.x).to_degrees() * ANGLE2SHORT) as i32, 0],
                ..NULL_USERCMD
            };
            let dt = FRAME_MS as f32 / 1000.0;
            player_step(
                &mut ps,
                &mut [0; 3],
                &mut EventRing::default(),
                &cmd,
                dt,
                &mw,
                weapons.defs(),
            );
            if tick > 1 && was && !ps.on_ground {
                last = Some(lip);
                break;
            }
        }
        assert_eq!(
            last,
            Some(true),
            "the run leaves the step's edge, and the lip reads first"
        );
    }

    /// mp_ship, seed 245: the drop from the crow's nest's top at
    /// (4928.5, 60.8, 1254) to the spar at 1212 had been avoided, and with
    /// no way round it the plan took it again. Passed as it was at the
    /// pace of a turn onto the node, the run flew past the spar to the
    /// deck at 400. It is held: the body comes to rest on the node first.
    #[test]
    fn an_edge_with_no_way_round_is_taken_from_rest() {
        let Some(world) = map_world("mp_ship") else {
            return;
        };
        let top = Vec3::new(4928.5, 60.8, 1254.125);
        let spar = Vec3::new(4888.3, 64.6, 1211.65);
        let g = NavGraph::from_parts(vec![top, spar], vec![vec![1], vec![]]);
        let mut f = Follower {
            path: vec![0, 1],
            avoid: vec![(0, 1, u32::MAX)],
            ..Follower::default()
        };
        let mut body = Footing {
            still: false,
            on_ground: true,
            on_ladder: false,
            velocity: Vec3::new(-118.0, 120.0, 0.0),
        };
        let at = [4940.6, 48.3, 1254.125];
        let mut budget = 100;
        let mut rand = || 0;
        let mut wp = |f: &mut Follower, at: [f32; 3], body: Footing| {
            f.waypoint(
                &g,
                Some(&world.collision),
                crate::bots::Goal::Roam,
                at,
                body,
                &mut budget,
                &mut rand,
            )
        };
        assert_eq!(
            wp(&mut f, at, body),
            Some(top.into()),
            "held at the turn's pace"
        );
        assert!(f.holding(&g));
        body.velocity = Vec3::ZERO;
        body.still = true;
        assert_eq!(
            wp(&mut f, (top + Vec3::new(3.0, -2.0, 0.0)).into(), body),
            Some(spar.into()),
            "from rest"
        );
    }

    /// mp_depot, seed 9: a bot jumped onto the crate at (-2275, 131, 46)
    /// and ran on for the open crate's rim at 66, 20 over it, with its
    /// jump still cooling down. The jump under the rim was refused and the
    /// run went off the crate's edge into the open crate at 23, where the
    /// rim is out of a jump's reach. The bot waits on the foot until the
    /// jump is ready, as the graph's walk had it.
    #[test]
    fn a_jump_edge_waits_on_its_foot_for_the_jump() {
        let Some(world) = map_world("mp_depot") else {
            return;
        };
        let w = &world.collision;
        let mw = MoveWorld::bare(w);
        let foot = Vec3::new(-2275.2, 131.3, 46.125);
        let top = Vec3::new(-2277.2, 95.5, 66.125);
        // The run from the foot with the jump key on the cue under the top.
        let run = |since_jump_ms: f32| {
            let mut ps = PlayerState::spawn(foot, 0.0);
            ps.since_jump_ms = since_jump_ms;
            ps.velocity = ((top - foot).truncate().normalize() * 190.0).extend(0.0);
            for tick in 1..30 {
                let to = top.truncate() - ps.origin.truncate();
                let cue = jump_under(to.length(), top.z - ps.origin.z, 0.0);
                let cmd = UserCmd {
                    server_time: tick * FRAME_MS,
                    forward: 127,
                    up: jump_key(ps.on_ground, cue, false),
                    angles: [0, (to.y.atan2(to.x).to_degrees() * ANGLE2SHORT) as i32, 0],
                    ..NULL_USERCMD
                };
                let dt = FRAME_MS as f32 / 1000.0;
                player_step(
                    &mut ps,
                    &mut [0; 3],
                    &mut EventRing::default(),
                    &cmd,
                    dt,
                    &mw,
                    &[],
                );
                if ps.on_ground && (ps.origin.z - top.z).abs() < 1.0 {
                    return true;
                }
            }
            false
        };
        assert!(run(f32::INFINITY), "a jump to hand lands on the rim");
        assert!(!run(150.0), "a jump refused runs off the crate");
        let mut g = NavGraph::from_parts(vec![foot, top], vec![vec![1], vec![]]);
        g.jumps.insert((0, 1));
        let f = Follower {
            path: vec![0, 1],
            next: 1,
            ..Follower::default()
        };
        let mut ps = PlayerState::spawn(foot + Vec3::new(0.0, -6.0, 0.0), 0.0);
        ps.since_jump_ms = 150.0;
        ps.on_ground = true;
        let at = ps.origin.into();
        assert_eq!(f.jump_wait(&g, at, &ps), Some(foot.into()));
        ps.since_jump_ms = vcod_common::pmove::JUMP_COOLDOWN_MS;
        assert_eq!(f.jump_wait(&g, at, &ps), None, "ready");
    }

    /// Under 1.5's and UO's rules a jump inside the penalty after a landing
    /// rises as little as 15.6 units: the bot waits on the foot for it to
    /// run out, as for the cooldown.
    #[test]
    fn a_jump_edge_waits_out_the_jump_penalty() {
        let (foot, top) = (Vec3::ZERO, Vec3::new(32.0, 0.0, 30.0));
        let mut g = NavGraph::from_parts(vec![foot, top], vec![vec![1], vec![]]);
        g.jumps.insert((0, 1));
        let f = Follower {
            path: vec![0, 1],
            next: 1,
            ..Follower::default()
        };
        let mut ps = PlayerState::spawn(foot + Vec3::new(-4.0, 0.0, 0.0), 0.0);
        ps.rules = MoveRules::Uo;
        ps.on_ground = true;
        ps.jump_penalty = true;
        let at = ps.origin.into();
        assert_eq!(f.jump_wait(&g, at, &ps), Some(foot.into()));
        assert!(!ready_to_leap(&ps));
        ps.jump_penalty = false;
        assert_eq!(f.jump_wait(&g, at, &ps), None, "ready");
        assert!(ready_to_leap(&ps));
    }

    /// A body that stepped up off a jump edge's foot onto a ledge under the
    /// top waits out the penalty there, where the walk jumped from: sent
    /// back down off mp_cassino's ledge at (3667, -1240, 128) it jumped
    /// from the foot 53 under the top and came back for good (seed 17).
    #[test]
    fn a_jump_edge_is_waited_on_from_a_ledge_above_its_foot() {
        let (foot, top) = (Vec3::ZERO, Vec3::new(48.0, 0.0, 53.0));
        let mut g = NavGraph::from_parts(vec![foot, top], vec![vec![1], vec![]]);
        g.jumps.insert((0, 1));
        let f = Follower {
            path: vec![0, 1],
            next: 1,
            ..Follower::default()
        };
        let mut ps = PlayerState::spawn(Vec3::new(18.0, 0.0, 16.7), 0.0);
        ps.rules = MoveRules::Uo;
        ps.on_ground = true;
        ps.jump_penalty = true;
        let at: [f32; 3] = ps.origin.into();
        assert_eq!(f.jump_wait(&g, at, &ps), Some(at));
        let at = [-4.0, 0.0, 2.0];
        assert_eq!(f.jump_wait(&g, at, &ps), Some(foot.into()));
        // On the top's own floor, beside it: back to the foot, as before
        // (mp_cassino's ctf at (5065, -1274, 256), a top 16 over its foot).
        let mut low = NavGraph::from_parts(
            vec![foot, Vec3::new(48.0, 0.0, 16.0)],
            vec![vec![1], vec![]],
        );
        low.jumps.insert((0, 1));
        assert_eq!(
            f.jump_wait(&low, [-20.0, 0.0, 16.0], &ps),
            Some(foot.into())
        );
    }

    /// The sprint's room runs along a straight, level line of plain edges
    /// and stops at a turn, a rise past a stair's and a jump edge.
    #[test]
    fn the_straight_run_stops_at_a_turn_a_rise_and_a_jump() {
        let row = |pts: &[(f32, f32, f32)]| {
            let nodes: Vec<Vec3> = pts.iter().map(|&(x, y, z)| Vec3::new(x, y, z)).collect();
            let n = nodes.len() as u32;
            let edges = (0..n)
                .map(|i| if i + 1 < n { vec![i + 1] } else { vec![] })
                .collect();
            NavGraph::from_parts(nodes, edges)
        };
        let follow = |n: u32| Follower {
            path: (0..n).collect(),
            next: 1,
            ..Follower::default()
        };
        // 0..4 straight along x, then a right angle.
        let g = row(&[
            (0.0, 0.0, 0.0),
            (48.0, 0.0, 0.0),
            (96.0, 0.0, 8.0),
            (144.0, 0.0, 8.0),
            (192.0, 0.0, 8.0),
            (192.0, 48.0, 8.0),
            (192.0, 96.0, 8.0),
        ]);
        let run = follow(7).straight_run(&g, Vec3::ZERO);
        assert!((run - 192.0).abs() < 1.0, "to the corner: {run}");
        // A zigzag within the slack counts as straight.
        let g = row(&[
            (0.0, 0.0, 0.0),
            (48.0, 8.0, 0.0),
            (96.0, 0.0, 0.0),
            (144.0, 8.0, 0.0),
        ]);
        assert!(follow(4).straight_run(&g, Vec3::ZERO) > 140.0);
        // A rise past a stair's: the run ends under it.
        let g = row(&[
            (0.0, 0.0, 0.0),
            (48.0, 0.0, 0.0),
            (96.0, 0.0, 30.0),
            (144.0, 0.0, 30.0),
        ]);
        let run = follow(4).straight_run(&g, Vec3::ZERO);
        assert!((run - 48.0).abs() < 1.0, "to the rise: {run}");
        // A jump edge, level or not, ends it too.
        let mut g = row(&[
            (0.0, 0.0, 0.0),
            (48.0, 0.0, 0.0),
            (96.0, 0.0, 0.0),
            (144.0, 0.0, 0.0),
        ]);
        g.jumps.insert((2, 3));
        let run = follow(4).straight_run(&g, Vec3::ZERO);
        assert!((run - 96.0).abs() < 1.0, "to the jump: {run}");
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

    /// mp_ship's slot between the stair's top and the shelf: the graph's
    /// walk from the shelf node, a running start straight at the floor
    /// 192 under it, lands. A bot that comes along the shelf at a run and
    /// turns onto the drop falls past the stair: the node is held while the
    /// walk at the body's pace fails, and passed from at rest on it.
    #[test]
    fn a_drop_is_proved_again_at_the_pace_the_bot_comes_onto_it() {
        let Some(world) = map_world("mp_ship") else {
            return;
        };
        let w = &world.collision;
        let shelf = Vec3::new(5402.5635, -160.24321, 396.125);
        let slot = Vec3::new(5404.874, -192.03677, 204.125);
        assert!(matches!(
            walk_as(w, shelf, slot.truncate(), Some(slot.z), Gait::Forward),
            Walked::Arrived(_, false)
        ));
        let g = NavGraph::from_parts(vec![shelf, slot], vec![vec![1], vec![]]);
        let mut f = Follower::default();
        let goal = crate::bots::Goal::To(slot.into());
        let mut budget = 100;
        let mut at = |p: Vec3, velocity: Vec3| {
            let body = Footing {
                still: velocity == Vec3::ZERO,
                on_ground: true,
                on_ladder: false,
                velocity,
            };
            f.waypoint(&g, Some(w), goal, p.into(), body, &mut budget, &mut || 0)
        };
        let run = Vec3::new(-218.0, -2.0, 0.0);
        assert_eq!(
            at(Vec3::new(5417.173, -160.85359, 396.125), run),
            Some(shelf.into())
        );
        assert_eq!(
            at(shelf, Vec3::ZERO),
            Some(slot.into()),
            "from at rest on the node"
        );
    }

    /// mp_depot's thin ledge (-2749, -1281, 326), 182 under the roof's node
    /// (-2750, -1244, 508) by the ladder at x -2750: only a creep from the
    /// node lands on it. Seed 183's bot came onto it off a turn, running
    /// west at 148 units/s, and fell past the ledge to the yard. The node is
    /// held until the body is under [`CREEP_TOP_PACE`].
    #[test]
    fn a_creeps_top_is_passed_only_slow() {
        let Some(world) = map_world("mp_depot") else {
            return;
        };
        let w = &world.collision;
        let roof = Vec3::new(-2750.1372, -1244.103, 508.125);
        let ledge = Vec3::new(-2748.646, -1280.8044, 326.125);
        let to = ledge.truncate();
        for gait in [Gait::Forward, Gait::Backward] {
            assert!(matches!(
                walk_as(w, roof, to, Some(ledge.z), gait),
                Walked::Fell
            ));
        }
        for pace in [None, Some(Vec3::ZERO)] {
            assert!(matches!(
                walk_with(w, roof, pace, to, Some(ledge.z), Gait::Creep),
                Walked::Arrived(..)
            ));
        }
        let g = NavGraph::from_parts(vec![roof, ledge], vec![vec![1], vec![]]);
        let mut f = Follower::default();
        let goal = crate::bots::Goal::To(ledge.into());
        let mut budget = 100;
        let mut at = |f: &mut Follower, p: Vec3, velocity: Vec3| {
            let body = Footing {
                still: velocity == Vec3::ZERO,
                on_ground: true,
                on_ladder: false,
                velocity,
            };
            let wp = f.waypoint(&g, Some(w), goal, p.into(), body, &mut budget, &mut || 0);
            (wp, f.holding(&g))
        };
        let seed_183 = Vec3::new(-2742.7, -1246.2, 508.125);
        assert_eq!(
            at(&mut f, seed_183, Vec3::new(-148.0, -16.0, 0.0)),
            (Some(roof.into()), true),
            "comes to rest on the node"
        );
        assert_eq!(
            at(&mut f, roof, Vec3::ZERO),
            (Some(ledge.into()), false),
            "from at rest on the node"
        );
        assert_eq!(
            at(
                &mut Follower::default(),
                seed_183,
                Vec3::new(-60.0, -10.0, 0.0)
            ),
            (Some(ledge.into()), false),
            "slow enough"
        );
    }

    /// mp_ship's ledge at (3685.5, -354.3, 400.1), 211 under the deck
    /// node (3647, -352.9, 611.1) west of it and narrow over the hold at
    /// 92: a run off the node misses it, a back down lands. A bot that came
    /// along the deck from the north and turned onto it carried its pace
    /// south over the ledge and died in the hold; the node is held and
    /// passed from at rest on it.
    #[test]
    fn a_backed_drop_is_proved_again_at_the_pace_the_bot_comes_onto_it() {
        let Some(world) = map_world("mp_ship") else {
            return;
        };
        let w = &world.collision;
        let deck = Vec3::new(3647.0, -352.9, 611.125);
        let ledge = Vec3::new(3685.5, -354.3, 400.125);
        let to = ledge.truncate();
        assert!(matches!(
            walk_as(w, deck, to, Some(ledge.z), Gait::Forward),
            Walked::Fell
        ));
        let g = NavGraph::from_parts(vec![deck, ledge], vec![vec![1], vec![]]);
        let goal = crate::bots::Goal::To(ledge.into());
        let mut budget = 100;
        let mut at = |f: &mut Follower, p: Vec3, velocity: Vec3| {
            let body = Footing {
                still: velocity == Vec3::ZERO,
                on_ground: true,
                on_ladder: false,
                velocity,
            };
            let wp = f.waypoint(&g, Some(w), goal, p.into(), body, &mut budget, &mut || 0);
            (wp, f.holding(&g))
        };
        let mut f = Follower::default();
        assert_eq!(
            at(&mut f, deck, Vec3::new(0.0, -190.0, 0.0)),
            (Some(deck.into()), true),
            "coming along the deck from the north"
        );
        assert_eq!(
            at(&mut f, deck, Vec3::ZERO),
            (Some(ledge.into()), false),
            "from at rest on the node"
        );
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
        assert!(!g.edges[north as usize].contains(&ledge), "from the north");
        assert!(g.edges[west as usize].contains(&ledge), "from the west");
        assert!(
            g.risky.contains(&(west, ledge)),
            "missed from 4 units south"
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
        g.risky.insert((0, 1));
        assert_eq!(g.path(0, 1), Some(vec![0, 2, 3, 1]), "round it");
        g.edges[2].clear();
        assert_eq!(g.path(0, 1), Some(vec![0, 1]), "the only way");
    }

    /// mp_ship's mast: the head (3675, -95.1, 1147.1) stands on the spar
    /// beside the rungs' top, and only a creep from rest gets back onto
    /// them. A bot that comes onto the head at a run is held there until it
    /// is slow.
    #[test]
    fn a_creep_down_proved_only_from_rest_is_passed_only_slow() {
        let Some(world) = map_world("mp_ship") else {
            return;
        };
        let w = &world.collision;
        let head = Vec3::new(3675.0, -95.13204, 1147.125);
        let foot = Vec3::new(3675.0, -103.0, 896.125);
        let g = NavGraph::from_parts(vec![head, foot], vec![vec![1], vec![]]);
        let goal = crate::bots::Goal::To(foot.into());
        let mut budget = 100;
        let mut at = |f: &mut Follower, p: Vec3, velocity: Vec3| {
            let body = Footing {
                still: velocity == Vec3::ZERO,
                on_ground: true,
                on_ladder: false,
                velocity,
            };
            let wp = f.waypoint(&g, Some(w), goal, p.into(), body, &mut budget, &mut || 0);
            (wp, f.holding(&g))
        };
        let mut f = Follower::default();
        let along = head + Vec3::new(-10.0, 0.0, 0.0);
        assert_eq!(
            at(&mut f, along, Vec3::new(190.0, 0.0, 0.0)),
            (Some(head.into()), true),
            "along the spar at a run"
        );
        assert_eq!(
            at(&mut f, head, Vec3::ZERO),
            (Some(foot.into()), false),
            "from at rest on the head"
        );
    }

    /// mp_depot's roof peak (-2660, -1216, 552): a run east over it flies
    /// off the crest and lands 67 units on at 190 units/s, on the narrow
    /// flat at 508 inside the roof's edge at x -2570. Seed 45's bot held a
    /// weapon at 1.16, came over at 220 and flew past the edge to the yard.
    /// The walk at its weapon's scale falls: the peak is held, and from at
    /// rest on it, where the same walk falls again, the edge is rounded.
    #[test]
    fn a_run_over_a_crest_too_fast_for_the_weapon_is_held_at_the_top() {
        let Some(world) = map_world("mp_depot") else {
            return;
        };
        let w = &world.collision;
        let peak = Vec3::new(-2660.245, -1215.9744, 552.13025);
        let east = Vec3::new(-2626.9204, -1215.9722, 531.5182);
        let g = NavGraph::from_parts(vec![peak, east], vec![vec![1], vec![]]);
        let goal = crate::bots::Goal::To(east.into());
        let mut budget = 100;
        let mut at = |f: &mut Follower, p: Vec3, velocity: Vec3| {
            let body = Footing {
                still: velocity == Vec3::ZERO,
                on_ground: true,
                on_ladder: false,
                velocity,
            };
            let wp = f.waypoint(&g, Some(w), goal, p.into(), body, &mut budget, &mut || 0);
            (wp, f.holding(&g))
        };
        let scaled = |move_scale: f32| Follower {
            move_scale,
            ..Follower::default()
        };
        let seed_45 = Vec3::new(-2663.9, -1216.1, 552.125);
        // Where seed 45's bot, held on the peak, came to rest: 6 short.
        let rest = Vec3::new(-2666.2, -1215.7, 552.125);
        assert_eq!(
            at(&mut scaled(1.0), seed_45, Vec3::new(190.0, 0.0, 0.0)),
            (Some(east.into()), false),
            "at the walk's own pace"
        );
        let mut f = scaled(1.16);
        assert_eq!(
            at(&mut f, seed_45, Vec3::new(220.0, 0.0, 0.0)),
            (Some(peak.into()), true),
            "with a light weapon"
        );
        assert_eq!(
            at(&mut f, rest, Vec3::ZERO),
            (None, false),
            "flung from at rest on the peak too: round it"
        );
        assert!(f.avoid.iter().any(|e| (e.0, e.1) == (0, 1)));
        assert_eq!(
            at(&mut scaled(1.0), rest, Vec3::ZERO),
            (Some(east.into()), false),
            "at the walk's own scale from rest"
        );
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

    /// mp_ship's run off (3564, 252, 896) onto the ledge at 760: from where
    /// three bots passed the node (seeds 427, 2477, 4523), at their pace,
    /// the walk lands on a slope's corner it clips, and two units across
    /// misses it, as the bots' own runs did, and falls to the deck at 352.
    /// A miss there falls to the deck, so the run has to land from either
    /// side too, and the drop does not hold from there; from the node,
    /// straight at the ledge, it does. A drop into the hold at (4246, 232,
    /// 56), where a miss lands on the hold's floor, is not asked: held
    /// for it, a bot jammed on a node another stood on (seed 439). Nor is
    /// a body on mp_ship's mast top at (3301, 128, 1030), which turned
    /// round coasts off its lip anyway (seed 449).
    #[test]
    fn a_drop_that_lands_only_on_a_corner_does_not_hold() {
        let Some(world) = map_world("mp_ship") else {
            return;
        };
        let w = &world.collision;
        let a = Vec3::new(3563.6223, 252.0, 896.125);
        let b = Vec3::new(3520.91, 289.35446, 760.125);
        let g = NavGraph::from_parts(vec![a, b], vec![vec![1], vec![]]);
        let mut f = Follower {
            path: vec![0, 1],
            ..Follower::default()
        };
        assert!(a_miss_hurts(w, b));
        assert!(!a_miss_hurts(w, Vec3::new(4246.0, 232.0, 56.125)));
        let at = Vec3::new(3573.1277, 245.13213, 896.125);
        let pace = Vec3::new(-173.0, 80.0, 0.0);
        assert!(matches!(
            walk_with(w, at, Some(pace), b.truncate(), Some(b.z), Gait::Forward),
            Walked::Arrived(..)
        ));
        assert!(
            !f.edge_holds(Some(w), at, pace, false, &g),
            "from where the bots passed"
        );
        assert!(!coasts_off(w, at, pace));
        let mast = Vec3::new(3309.6, 128.8, 1030.125);
        assert!(coasts_off(w, mast, Vec3::new(-176.0, -72.0, 0.0)));
        let straight = (b - a).truncate().normalize().extend(0.0) * vcod_common::pmove::SPEED_RUN;
        assert!(
            f.edge_holds(Some(w), a, straight, false, &g),
            "from the node"
        );
    }

    /// mp_ship's dome crest: a bot climbing the dome passed the node at
    /// (3615, -345, 635) at `REACH`, turned for (3584, -349, 640), left
    /// the ground over the crest and flew off the south face to the hold
    /// at 56 (seed 4231 on 1080e30d; body and pace as the follower read
    /// them). The walk on from the body lands; two units across it flies
    /// off, so the edge does not hold from there. From the node it does,
    /// and so does a walk whose runs aside miss without falling.
    #[test]
    fn a_run_on_that_leaves_the_ground_over_a_fall_lands_from_either_side() {
        let Some(world) = map_world("mp_ship") else {
            return;
        };
        let w = &world.collision;
        let a = Vec3::new(3615.4631, -344.67303, 635.2741);
        let b = Vec3::new(3583.9604, -348.952, 640.3489);
        let g = NavGraph::from_parts(vec![a, b], vec![vec![1], vec![]]);
        let mut f = Follower {
            path: vec![0, 1],
            ..Follower::default()
        };
        let at = Vec3::new(3636.6252, -336.76816, 628.8017);
        let pace = Vec3::new(-122.0, -124.0, 31.0);
        let mut trail = Trail::default();
        let on = walk_sliding(
            w,
            at,
            Some(pace),
            1.0,
            b.truncate(),
            Some(b.z),
            Gait::Forward,
            &mut trail,
        );
        assert!(matches!(on, Walked::Arrived(_, false)) && trail.dip > 0.0);
        assert!(a_miss_hurts(w, b));
        assert!(
            !f.edge_holds(Some(w), at, pace, false, &g),
            "from where the bot passed"
        );
        let straight = (b - a).truncate().normalize().extend(0.0) * vcod_common::pmove::SPEED_RUN;
        assert!(
            f.edge_holds(Some(w), a, straight, false, &g),
            "from the node"
        );
        assert!(
            f.edge_holds(Some(w), a, Vec3::ZERO, false, &g),
            "from rest on it"
        );
        // The crow's nest ladder's foot at 692: the walk hops a lip and its
        // runs aside never arrive, but neither falls.
        let (a, b) = (
            Vec3::new(4992.056, 97.13228, 692.125),
            Vec3::new(4959.0, 64.0, 692.125),
        );
        let g = NavGraph::from_parts(vec![a, b], vec![vec![1], vec![]]);
        let mut f = Follower {
            path: vec![0, 1],
            ..Follower::default()
        };
        let at = Vec3::new(5006.1475, 115.67403, 692.125);
        let pace = Vec3::new(-42.0, -172.0, 0.0);
        assert!(
            f.edge_holds(Some(w), at, pace, false, &g),
            "onto the ladder's foot"
        );
    }

    /// mp_depot's pit, seed 1145: the flood proved the jump from
    /// (-1569, 2087, 212) to the step at (-1533.2, 2112, 239.1) running at
    /// the column's centre (-1536, 2112). Aimed at the node, where a bot
    /// steers, the walk bobs under the step and never lands, nor aimed 8
    /// units either side: the re-proof drops the edge, and the bot goes
    /// round.
    #[test]
    fn a_jump_edge_that_lands_only_off_where_a_bot_steers_is_dropped() {
        let Some(fs) = vcod_common::testing::game_fs_11() else {
            return;
        };
        let bsp = vcod_common::bsp::parse(&fs.read(&fs.resolve_map("mp_depot").unwrap()).unwrap())
            .unwrap();
        let world = crate::world::World::from_bsp(&bsp, Some(&fs));
        let w = &world.collision;
        let pit = settle(w, Vec3::new(-1569.0, 2087.0, 230.0), 0.0).unwrap();
        let step = settle(w, Vec3::new(-1533.2, 2112.0, 250.0), 0.0).unwrap();
        let at =
            |target: glam::Vec2| as_a_bot(|| walk_as(w, pit, target, Some(step.z), Gait::Forward));
        assert!(
            !matches!(at(step.truncate()), Walked::Arrived(..)),
            "lands aimed at the node"
        );
        let flood = walk(w, &[], pit, glam::Vec2::new(-1536.0, 2112.0));
        assert!(
            flood.is_some_and(|(p, jumped)| jumped && (p.z - step.z).abs() < 1.0),
            "the flood's jump"
        );
        let mut g = NavGraph::from_parts(vec![pit, step], vec![vec![1], vec![]]);
        g.jumps.insert((0, 1));
        g.reprove_jumps(w);
        assert!(g.edges[0].is_empty() && g.jumps.is_empty() && g.aims.is_empty());
    }

    /// mp_depot's step at (-1731, 32, 11): the jump to it from (-1760, 71,
    /// -24) lands only aimed 8 units aside of the node, and the re-proof
    /// keeps that aim, which a bot on the edge steers at.
    #[test]
    fn a_jump_edge_that_lands_only_aimed_aside_keeps_its_aim() {
        let Some(fs) = vcod_common::testing::game_fs_11() else {
            return;
        };
        let bsp = vcod_common::bsp::parse(&fs.read(&fs.resolve_map("mp_depot").unwrap()).unwrap())
            .unwrap();
        let world = crate::world::World::from_bsp(&bsp, Some(&fs));
        let w = &world.collision;
        let foot = settle(w, Vec3::new(-1760.0, 71.0, 0.0), 0.0).unwrap();
        let top = settle(w, Vec3::new(-1731.0, 32.0, 30.0), 0.0).unwrap();
        let mut g = NavGraph::from_parts(vec![foot, top], vec![vec![1], vec![]]);
        g.jumps.insert((0, 1));
        g.reprove_jumps(w);
        let aim = g.aims.get(&(0, 1)).copied().expect("aimed aside");
        assert!((aim.distance(top.truncate()) - SIDESTEP).abs() < 0.01);
        assert!(g.edges[0] == [1] && g.jumps.contains(&(0, 1)));
        let f = Follower {
            path: vec![0, 1],
            next: 1,
            ..Follower::default()
        };
        assert_eq!(f.jump_aim(&g), Some(aim));
    }

    /// United Offensive's mp_uo_dawnville: the jump edge from (-241,
    /// -16802, 103) up the ramp to the floor at (-190, -16848, 176), the
    /// way to spawns 220 and 248. Right under the floor while still
    /// climbing the ramp, a jump flies past it and falls back; the walk
    /// runs on to the ramp's head, stalls and jumps on, and the re-proof
    /// keeps the edge.
    #[test]
    fn a_jump_from_a_ramp_waits_for_its_head() {
        let Some(fs) = vcod_common::testing::uo_fs() else {
            return;
        };
        let bsp = vcod_common::bsp::parse(
            &fs.read(&fs.resolve_map("mp_uo_dawnville").unwrap())
                .unwrap(),
        )
        .unwrap();
        let world = crate::world::World::from_bsp(&bsp, Some(&fs));
        world.collision.unlink_script_brushes(&bsp.entities, "tdm");
        let w = &world.collision;
        with_rules(MoveRules::Uo, || {
            let foot = settle(w, Vec3::new(-241.0, -16802.0, 120.0), 0.0).unwrap();
            let top = settle(w, Vec3::new(-190.0, -16848.0, 190.0), 0.0).unwrap();
            let mut g = NavGraph::from_parts(vec![foot, top], vec![vec![1], vec![]]);
            g.jumps.insert((0, 1));
            g.reprove_jumps(w);
            assert!(g.edges[0] == [1] && g.jumps.contains(&(0, 1)));
        });
    }

    /// mp_ship's stair from (5792, -423, 124) up to (5792, -382, 204): the
    /// flood's walk jumps under the top while still climbing and proves a
    /// jump edge, as it did before the bot's jump waited out a ramp's
    /// climb. Taken into the flood's walks, that wait turned 26 of
    /// mp_ship's jump edges plain, and bots stalled and fell at its
    /// stairwell and crow's nest.
    #[test]
    fn the_floods_walk_jumps_under_a_top_while_climbing() {
        let Some(world) = map_world("mp_ship") else {
            return;
        };
        let w = &world.collision;
        let foot = settle(w, Vec3::new(5792.0, -423.0, 140.0), 0.0).unwrap();
        let top = settle(w, Vec3::new(5792.0, -382.0, 220.0), 0.0).unwrap();
        let walked = walk_as(w, foot, top.truncate(), Some(top.z), Gait::Forward);
        assert!(matches!(walked, Walked::Arrived(_, true)), "a jump edge");
    }

    /// mp_dawnville's spawns 88 and 154 at (145, -15311, 7) are reached
    /// only by the jump from (-96, -15409, -13) to (-89, -15360, 19), which
    /// `link_back` proved 8 units aside. Aimed at the node the run pins on
    /// the wall at y -15375, where `jump_clear` finds no room; a bot jumps
    /// there anyway and lands, and so does the re-proof, which jumps as a
    /// bot does: the edge stays, aimed at its node.
    #[test]
    fn a_jump_edge_is_proved_again_by_the_bots_jump() {
        let Some(fs) = vcod_common::testing::game_fs_11() else {
            return;
        };
        let bsp =
            vcod_common::bsp::parse(&fs.read(&fs.resolve_map("mp_dawnville").unwrap()).unwrap())
                .unwrap();
        let world = crate::world::World::from_bsp(&bsp, Some(&fs));
        let w = &world.collision;
        let foot = settle(w, Vec3::new(-96.0, -15409.0, 0.0), 0.0).unwrap();
        let top = settle(w, Vec3::new(-89.0, -15360.0, 30.0), 0.0).unwrap();
        let roomed = walk_as(w, foot, top.truncate(), Some(top.z), Gait::Forward);
        assert!(
            !matches!(roomed, Walked::Arrived(..)),
            "the walk with the room test"
        );
        let mut g = NavGraph::from_parts(vec![foot, top], vec![vec![1], vec![]]);
        g.jumps.insert((0, 1));
        g.reprove_jumps(w);
        assert!(g.edges[0] == [1] && g.jumps.contains(&(0, 1)) && g.aims.is_empty());
        assert_eq!(g.aim(0, 1), top.truncate());
    }

    /// mp_ship's stair stringer by the hull crates: the jump edge from
    /// (5829, 544, 92) to (5792, 576, 76) was proved with a jump from its
    /// node, and a bot passed the node on the stringer's top at (5823,
    /// 567, 92), where the walk on slides off without one. Pinned on the
    /// stringer, the bot jumped on its cue and fell back for 140 ticks
    /// (seed 2553 on 1080e30d). From there it is not cued to jump; run
    /// from the node, as the graph's walk proved it, it still is.
    #[test]
    fn a_jump_edge_walked_on_without_its_jump_cues_no_jump() {
        let Some(world) = map_world("mp_ship") else {
            return;
        };
        let w = &world.collision;
        let a = Vec3::new(5829.127, 544.00085, 92.125);
        let b = Vec3::new(5792.3965, 576.17664, 76.125);
        let mut g = NavGraph::from_parts(vec![a, b], vec![vec![1], vec![]]);
        g.jumps.insert((0, 1));
        let mut f = Follower {
            path: vec![0, 1],
            ..Follower::default()
        };
        let at = Vec3::new(5823.3916, 567.12866, 92.125);
        assert!(f.edge_holds(Some(w), at, Vec3::new(22.0, 0.0, 0.0), false, &g));
        f.next = 1;
        assert!(f.jumping(&g) && !f.jump_cued(&g), "from the stringer");
        f.next = 0;
        let run = (b - a).truncate().normalize().extend(0.0) * vcod_common::pmove::SPEED_RUN;
        assert!(f.edge_holds(Some(w), a, run, false, &g));
        f.next = 1;
        assert!(f.jump_cued(&g), "from the node at a run");
    }

    /// mp_ship's hull corridor at (3825, -800, -64): a beam's end stands
    /// across the line from 3 units north of the node at (3837, -805). A
    /// run from there pins on it and arrives only by jumping, which a bot
    /// does only on a jump edge, so the edge does not hold from there.
    #[test]
    fn an_edge_that_needs_a_jump_from_aside_does_not_hold() {
        let Some(world) = map_world("mp_ship") else {
            return;
        };
        let w = &world.collision;
        let a = Vec3::new(3837.1023, -804.9357, -63.875);
        let b = Vec3::new(3879.2532, -799.9733, -63.875);
        let mut g = NavGraph::from_parts(vec![a, b], vec![vec![1], vec![]]);
        let mut f = Follower {
            path: vec![0, 1],
            ..Follower::default()
        };
        let aside = Vec3::new(3822.0, -802.0, -63.875);
        assert!(
            f.edge_holds(Some(w), a, Vec3::ZERO, false, &g),
            "from the node"
        );
        assert!(
            !f.edge_holds(Some(w), aside, Vec3::ZERO, false, &g),
            "3 units north"
        );
        g.jumps.insert((0, 1));
        assert!(
            f.edge_holds(Some(w), aside, Vec3::ZERO, false, &g),
            "on a jump edge"
        );
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

    /// mp_ship's hold ladder: its head stands on a lip over the shaft. A
    /// leap from the head to the ledge at 641 arrives, but a climb tips the
    /// body onto the lip 4 units south of it, and from there the leap
    /// falls. A ladder's head is no leap's foot.
    #[test]
    fn a_ladder_head_is_no_leaps_foot() {
        let Some(world) = map_world("mp_ship") else {
            return;
        };
        let w = &world.collision;
        let foot = Vec3::new(4332.0, -94.0, 56.125);
        let head = Vec3::new(4332.0, -87.12833, 615.125);
        let top = Vec3::new(4322.311, -158.08282, 641.125);
        let leap = |from: Vec3| walk_as(w, from, top.truncate(), Some(top.z), Gait::Leap);
        assert!(matches!(leap(head), Walked::Arrived(_, true)));
        let tipped = Vec3::new(4333.2227, -91.545074, 615.125);
        assert!(matches!(leap(tipped), Walked::Fell));
        let mut g = NavGraph::from_parts(vec![foot, head, top], vec![vec![1], vec![0], vec![]]);
        g.link_leaps(w);
        assert!(g.leaps.is_empty(), "{:?}", g.leaps);
        // Not reached by the climb, the same spot is a foot.
        let mut g = NavGraph::from_parts(vec![foot, head, top], vec![vec![], vec![0], vec![]]);
        g.link_leaps(w);
        assert!(g.leaps.contains(&(1, 2)), "{:?}", g.leaps);
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

    /// mp_cassino's allied upper floor at z 208 leaves by a window sill at
    /// 240 over the street at 0: the walk jumps onto the sill and falls 240,
    /// under `bg_fallDamageMinHeight`'s 256 (bot-navigation.md, "Other
    /// rules").
    #[test]
    fn a_walk_off_cassinos_allied_sill_falls_240_and_lands() {
        let Some(fs) = vcod_common::testing::uo_fs() else {
            return;
        };
        let entry = fs.resolve_map("mp_cassino").unwrap();
        let bsp = vcod_common::bsp::parse(&fs.read(&entry).unwrap()).unwrap();
        let world = crate::world::World::from_bsp(&bsp, Some(&fs));
        let floor = Vec3::new(193.0, 1103.0, 208.125);
        with_rules(MoveRules::Uo, || {
            let out = walk_to(
                &world.collision,
                &[],
                floor,
                glam::Vec2::new(16.0, 1104.0),
                None,
            );
            assert!(
                out.is_some_and(|(p, jumped)| jumped && p.z < 40.0),
                "{out:?}"
            );
        });
    }

    /// mp_cassino's terrace at (383, -916, 76), whose one way down to the
    /// street at 11 is a stair: seed 23 left a Panzer IV parked at
    /// (450, -1099, 10.5) facing -79.6 with its collmap's rear over the
    /// stair, and a bot on the stair wedged between the stair's side and
    /// the hull at (425.7, -957.5, 60.1) (bot-navigation.md, "Parked
    /// vehicles"). The stair's nodes read as blocked, and the terrace is
    /// hemmed in.
    #[test]
    fn a_tank_parked_over_cassinos_stair_blocks_its_nodes() {
        use crate::bots::Goal;
        let Some(fs) = vcod_common::testing::uo_fs() else {
            return;
        };
        let entry = fs.resolve_map("mp_cassino").unwrap();
        let bsp = vcod_common::bsp::parse(&fs.read(&entry).unwrap()).unwrap();
        let world = crate::world::World::from_bsp(&bsp, Some(&fs));
        let c = &world.collision;
        let g = with_rules(MoveRules::Uo, || graph_for("mp_cassino", &world));
        let tank = Vec3::new(450.0, -1099.0, 10.5);
        assert!(g.blocked_near(c, tank, 192.0).is_empty());
        let contents = crate::game::vehicle::CONTENTS_COLLMAP as u32;
        c.set_model_instance(181, 28, Some((tank, Vec3::new(0.0, -79.6, 0.0), contents)));
        let mut blocked = g.blocked_near(c, tank, 192.0);
        c.set_model_instance(181, 28, None);
        blocked.sort_unstable();
        let blocked: Arc<[u32]> = blocked.into();
        let node = |p: [f32; 3]| g.nearest(p).unwrap();
        let (top, street) = ([383.0, -916.0, 76.0], [576.0, -1052.0, 17.0]);
        assert!(
            blocked.contains(&node([431.0, -964.0, 57.0])),
            "{blocked:?}"
        );
        assert!(!blocked.contains(&node(top)));
        let mut search =
            Search::new(&g, node(top), node(street), Vec::new()).blocking(blocked.clone());
        assert!(matches!(search.run(&g, &mut 100_000), Plan::Failed));
        let mut f = Follower {
            blocked,
            ..Follower::default()
        };
        let mut budget = 100_000;
        let w = (0..3).find_map(|_| {
            f.waypoint(
                &g,
                None,
                Goal::To(street),
                top,
                Footing::STANDING,
                &mut budget,
                &mut || 0,
            )
        });
        assert!(w.is_none() && f.penned());
    }

    /// Node 4 of [`tiny`] is a drop's foot with no way on: cut off from the
    /// component the other four make.
    /// mp_cassino's axis room: from the wall under its south-west window a
    /// forward walk does not get onto the sill, a crouched jump does, as a
    /// retail UO client's does at a bot's 50 ms cmds
    /// (`moves_ab.rs` `CROUCHJUMP_UO`).
    #[test]
    fn a_crouched_jump_gets_onto_cassinos_axis_sill_where_a_run_does_not() {
        let Some(fs) = vcod_common::testing::uo_fs() else {
            return;
        };
        let entry = fs.resolve_map("mp_cassino").unwrap();
        let bsp = vcod_common::bsp::parse(&fs.read(&entry).unwrap()).unwrap();
        let world = crate::world::World::from_bsp(&bsp, Some(&fs));
        let from = Vec3::new(5347.0, -1496.0, 128.125);
        let sill = glam::Vec2::new(5305.0, -1548.0);
        with_rules(MoveRules::Uo, || {
            let walk = |gait| walk_as(&world.collision, from, sill, Some(160.125), gait);
            assert!(matches!(
                walk(Gait::Forward),
                Walked::Blocked | Walked::Fell
            ));
            assert!(matches!(walk(Gait::CrouchJump), Walked::Arrived(_, true)));
        });
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
        let (mut nodes, mut edges) = (g.nodes.clone(), g.edges.clone());
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

    #[test]
    fn a_run_bobbing_under_a_steep_face_jumps_out_of_the_gutter() {
        let Some(fs) = vcod_common::testing::uo_fs() else {
            return;
        };
        let entry = fs.resolve_map("mp_berlin").unwrap();
        let bsp = vcod_common::bsp::parse(&fs.read(&entry).unwrap()).unwrap();
        let world = crate::world::World::from_bsp(&bsp, Some(&fs));
        let w = &world.collision;
        let slope = Vec3::new(-673.7223, 4131.117, 616.0533);
        let ledge = glam::Vec2::new(-720.0, 4080.0);
        with_rules(MoveRules::Uo, || {
            assert!(!matches!(
                walk_as(w, slope, ledge, None, Gait::Forward),
                Walked::Arrived(..)
            ));
            let out = walk_to(w, &[], slope, ledge, None);
            assert!(
                out.is_some_and(|(p, jumped)| jumped && p.z > 645.0),
                "{out:?}"
            );
        });
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

    /// mp_ship's hold ladder: a floor at z 304 opens west of the shaft,
    /// its edge 15 units from where a body grabbed at x 4328.5 hangs. On
    /// the rungs only the strafe moves a body along the face: backing
    /// down facing the face and sliding toward the floor's node lands on
    /// it; without the slide the body passes the lip and hangs under it.
    /// mp_ship's ladder at (2150, -214): the floor at 104 lies behind a
    /// climber hanging level with it, 16 units out from the rungs. The
    /// push-off lands on it only braked; run on toward the floor, the
    /// jump carries the body past its edge into the hull at -64.
    #[test]
    fn a_push_off_lands_on_the_floor_behind_the_rungs() {
        let Some(world) = map_world("mp_ship") else {
            return;
        };
        let w = &world.collision;
        let floor = Vec3::new(2144.741, -229.90549, 104.125);
        let mut ps = PlayerState::spawn(Vec3::new(2146.5, -214.1, 140.0), 90.0);
        let mut sim = Sim::default();
        // Let go of the key 6 above: a 95 units/s descent stops in 5.6.
        for _ in 0..80 {
            let down = if ps.origin.z > 109.0 { -127 } else { 0 };
            let cmd = UserCmd {
                forward: down,
                angles: [
                    (-LADDER_PITCH * ANGLE2SHORT) as i32,
                    (90.0 * ANGLE2SHORT) as i32,
                    0,
                ],
                ..NULL_USERCMD
            };
            sim.tick(w, &mut ps, &cmd);
        }
        assert!(
            ps.on_ladder && (ps.origin.z - floor.z).abs() < 4.0,
            "{}",
            ps.origin
        );
        let yaw = push_off(w, &ps, floor, &[]).expect("a push-off");
        assert!((yaw - 90.0).abs() < 1.0, "{yaw}");
        // Run on at the floor, the same jump overshoots.
        let mut free = ps;
        for i in 0..40 {
            let cmd = UserCmd {
                forward: if i == 0 { 0 } else { -127 },
                up: if i == 0 { 127 } else { 0 },
                angles: [0, (yaw * ANGLE2SHORT) as i32, 0],
                ..NULL_USERCMD
            };
            sim.tick(w, &mut free, &cmd);
            if free.on_ground {
                break;
            }
        }
        assert!(free.on_ground && free.origin.z < 0.0, "{}", free.origin);
        // Climbing past the floor, or at its foot, there is none.
        let mut low = ps;
        low.origin.z = 60.0;
        assert_eq!(push_off(w, &low, floor, &[]), None);
        assert_eq!(
            push_off(w, &ps, Vec3::new(2160.0, -156.1, 216.1), &[]),
            None
        );
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

    /// mp_ship's shelf corners at (5375.1, 321.1, 352.1) and (5375.1,
    /// -193.1, 352.1), a body respawned at rest there and run 2 s at a
    /// yaw. Retail 1.1d (`client-probes/probe_shipcrease`, 2026-10-18,
    /// bot-navigation.md section 3, "The corners at x 5375.1"): the north
    /// one runs free at 200 and 225 to (5343.12, 227.13) and at 140 to
    /// (5343.12, 332.87); the south one stays at x 5375.1 at 140 and runs
    /// at 200 and 225 to (5343.12, -204.87). So the run at the north
    /// node needs no unpin, and the south side's way out is the west axis,
    /// 50 degrees off the way to its node.
    #[test]
    fn a_body_pinned_in_a_crease_runs_aside() {
        let Some(world) = map_world("mp_ship") else {
            return;
        };
        let w = &world.collision;
        let settled = |p: Vec3| {
            let mut ps = PlayerState::spawn(p, 0.0);
            Sim::default().tick(w, &mut ps, &NULL_USERCMD);
            ps
        };
        let run = |ps: PlayerState, yaw: f32, ticks: usize| {
            let mut run = ps;
            let mut sim = Sim::default();
            for _ in 0..ticks {
                let cmd = UserCmd {
                    forward: 127,
                    angles: [0, (yaw * ANGLE2SHORT) as i32, 0],
                    ..NULL_USERCMD
                };
                sim.tick(w, &mut run, &cmd);
            }
            run.origin
        };
        let north = settled(Vec3::new(5375.1, 321.1, 352.125));
        let south = settled(Vec3::new(5375.1, -193.1, 352.125));
        for (ps, yaw, x, y) in [
            (north, 200.0, 5343.12, 227.13),
            (north, 225.0, 5343.12, 227.13),
            (north, 140.0, 5343.12, 332.87),
            (south, 140.0, 5375.1, -193.1),
            (south, 200.0, 5343.12, -204.87),
            (south, 225.0, 5343.12, -204.87),
        ] {
            let at = run(ps, yaw, 40);
            assert!(
                (at.x - x).abs() < 0.05 && (at.y - y).abs() < 0.05,
                "{} at {yaw}: {at}",
                ps.origin
            );
        }
        // The north run at its node is free; nothing to unpin.
        let to = Vec3::new(5344.0938, 312.65878, 352.125);
        assert_eq!(unpin_yaw(w, &[], &north, to, &[]), None);
        // The south side's crease, 50 degrees off the way north-west.
        let to = Vec3::new(5344.4, -156.1, 352.125);
        let yaw = unpin_yaw(w, &[], &south, to, &[]).expect("a way out");
        assert!(run(south, yaw, UNPIN_TICKS).x < 5360.0, "{yaw}");
    }

    #[test]
    fn a_back_down_slides_along_the_face_onto_a_floor_beside_the_ladder() {
        let Some(world) = map_world("mp_ship") else {
            return;
        };
        let w = &world.collision;
        let node = glam::Vec2::new(4318.5, -96.0);
        let down = |slide: bool| {
            let mut ps = PlayerState::spawn(Vec3::new(4328.5, -93.1, 420.0), 90.0);
            let mut sim = Sim::default();
            for _ in 0..200 {
                let yaw = climb_yaw(ps.ladder_normal).unwrap_or(90.0);
                let right = if slide {
                    crate::bots::along_face(yaw, ps.origin.into(), node.into())
                } else {
                    0
                };
                let cmd = UserCmd {
                    forward: -127,
                    right,
                    angles: [
                        (-LADDER_PITCH * ANGLE2SHORT) as i32,
                        (yaw * ANGLE2SHORT) as i32,
                        0,
                    ],
                    ..NULL_USERCMD
                };
                sim.tick(w, &mut ps, &cmd);
                if ps.on_ground {
                    break;
                }
            }
            ps
        };
        let slid = down(true);
        assert!(
            slid.on_ground && (slid.origin.z - 304.125).abs() < 0.5,
            "{}",
            slid.origin
        );
        let plain = down(false);
        assert!(
            !(plain.on_ground && (plain.origin.z - 304.125).abs() < 0.5),
            "{}",
            plain.origin
        );
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

    /// A bot that ran off the side of mp_ship's shelf at 396 landed on a
    /// lip at 344 and pushed at the shelf node 52 over it until the stuck
    /// count ran out. On the ground more than a climb under its waypoint,
    /// off a ladder, it plans again, round the edge it fell off.
    #[test]
    fn a_follower_that_fell_off_its_path_plans_again() {
        let g = NavGraph::from_parts(
            vec![
                Vec3::new(5343.4, -155.3, 448.125),
                Vec3::new(5383.6, -161.1, 396.125),
                Vec3::new(5402.6, -160.2, 396.125),
            ],
            vec![vec![1], vec![2], vec![1]],
        );
        let mut f = Follower::default();
        let goal = crate::bots::Goal::To([5402.6, -160.2, 396.125]);
        let mut budget = 100;
        let mut at =
            |p: [f32; 3]| f.waypoint(&g, None, goal, p, Footing::STANDING, &mut budget, &mut || 0);
        assert_eq!(
            at([5343.4, -155.3, 448.125]),
            Some([5383.6, -161.1, 396.125])
        );
        assert_eq!(at([5453.9, -181.9, 344.125]), None);
        assert!(
            f.avoid.iter().any(|e| (e.0, e.1) == (0, 1)),
            "the drop it fell off"
        );
    }

    /// mp_ship's slot at (5403, -160, 396): a run off its drop for the
    /// floor at 204 landed on the ledge at 336, 8 units from straight over
    /// the waypoint, and turned there until an unstick spell threw it off
    /// (seeds 73, 149, 183, 317, 1217, 1913, 3151, 4177, 4333). It plans
    /// again from the ledge.
    #[test]
    fn a_drop_that_lands_partway_down_is_planned_again() {
        let g = NavGraph::from_parts(
            vec![
                Vec3::new(5384.0, -161.0, 396.125),
                Vec3::new(5403.0, -160.0, 396.125),
                Vec3::new(5405.0, -192.0, 204.125),
                Vec3::new(5376.0, -196.0, 204.125),
            ],
            vec![vec![1], vec![2], vec![3], vec![2]],
        );
        let mut f = Follower::default();
        let goal = crate::bots::Goal::To([5376.0, -196.0, 204.125]);
        let mut budget = 100;
        let mut at =
            |p: [f32; 3]| f.waypoint(&g, None, goal, p, Footing::STANDING, &mut budget, &mut || 0);
        assert_eq!(
            at([5384.0, -161.0, 396.125]),
            Some([5405.0, -192.0, 204.125])
        );
        // On the top still, the drop is under way.
        assert_eq!(
            at([5398.0, -184.0, 396.125]),
            Some([5405.0, -192.0, 204.125])
        );
        assert_eq!(at([5404.4, -200.4, 336.125]), None);
        assert!(f.fell, "planned from where it landed");
        // Not round the drop: avoided, the plan from mp_ship's ledge went
        // back up to the shelf and on by a jump edge it bounced under
        // (seed 1409).
        assert!(f.avoid.is_empty());
    }

    /// mp_ship's hold floor at 56: a bot run along the wall at y -400 for
    /// the drop to (3264, -378, -64) was stopped short of the lip by a bot
    /// dropping ahead, pressed at its waypoint behind the wall and was run
    /// off the floor's south side by an unstick spell (seed 2195). Stopped
    /// there, it plans round the drop; on the move it runs on.
    #[test]
    fn a_run_off_a_drop_stopped_short_of_the_lip_plans_round_it() {
        let Some(world) = map_world("mp_ship") else {
            return;
        };
        let w = &world.collision;
        let g = NavGraph::from_parts(
            vec![
                Vec3::new(3329.0, -419.0, 56.125),
                Vec3::new(3296.0635, -419.05017, 56.125),
                Vec3::new(3264.001, -378.19662, -63.875),
                Vec3::new(3232.0, -351.0, -63.875),
            ],
            vec![vec![1], vec![2], vec![3], vec![2]],
        );
        let goal = crate::bots::Goal::To([3232.0, -351.0, -63.875]);
        let mut budget = 100;
        let run = |v: Vec3| Footing {
            still: false,
            velocity: v,
            ..Footing::STANDING
        };
        let bottom = Some([3264.001, -378.19662, -63.875]);
        let mut f = Follower::default();
        let mut at =
            |p: [f32; 3], v: Vec3| f.waypoint(&g, Some(w), goal, p, run(v), &mut budget, &mut || 0);
        let west = Vec3::new(-190.0, 0.0, 0.0);
        assert_eq!(
            at([3329.0, -419.0, 56.125], west),
            Some([3296.0635, -419.05017, 56.125])
        );
        assert_eq!(at([3299.8, -415.13, 56.125], west), bottom);
        assert_eq!(at([3280.3, -415.13, 56.125], west), bottom, "on the move");
        assert_eq!(
            at([3266.6, -415.13, 56.125], Vec3::new(-6.0, 0.0, 0.0)),
            None,
            "stopped"
        );
        assert_eq!(f.avoid.len(), 1);
    }

    /// mp_ship's mast hatch, seed 2171: off the rungs at (3705, 58, 992)
    /// the walk on west to (3648, 71, 994) arrives at the graph's scale and
    /// falls back down the hatch at a weapon's 0.9. The bot heading on at
    /// 0.9 climbed the ladder four times. Now the node is not passed.
    #[test]
    fn a_walk_on_that_falls_at_the_weapons_scale_is_not_taken() {
        let Some(world) = map_world("mp_ship") else {
            return;
        };
        let w = &world.collision;
        let (side, west) = (
            Vec3::new(3704.8745, 84.5, 992.125),
            Vec3::new(3648.0496, 71.05647, 994.125),
        );
        // Up the ladder from the deck at 760 to its head, then on.
        let (foot, head) = (
            Vec3::new(3695.0, 56.0, 760.125),
            Vec3::new(3704.8745, 56.0, 992.125),
        );
        let g = NavGraph::from_parts(
            vec![foot, head, side, west],
            vec![vec![1], vec![2], vec![3], vec![]],
        );
        let goal = crate::bots::Goal::To(west.into());
        let body = Footing {
            still: false,
            on_ground: true,
            on_ladder: false,
            velocity: Vec3::new(0.0, 85.0, 0.0),
        };
        // From where the body steps off, the straight line west crosses
        // the hatch: `side` is not passed early whatever the scale (seed
        // 1031), and the walk on from there falls at 0.9 only.
        let off_rungs = [3704.875, 57.676594, 992.125];
        for (move_scale, holds) in [(1.0, true), (0.9, false)] {
            let mut f = Follower {
                move_scale,
                path: vec![0, 1, 2, 3],
                next: 2,
                dest: Some(3),
                ..Follower::default()
            };
            let at = Vec3::from(off_rungs);
            assert_eq!(
                f.edge_holds(Some(w), at, body.velocity, false, &g),
                holds,
                "walk on at {move_scale}"
            );
            let mut budget = 100;
            let wp = f.waypoint(&g, Some(w), goal, off_rungs, body, &mut budget, &mut || 0);
            assert_eq!(wp, Some(side.into()), "at {move_scale}");
        }
    }

    /// The fall a body lives through shrinks with its health: whole, only
    /// past `bg_fallDamageMaxHeight` (the 110 percent); at 20 of 100, past
    /// 21 percent of the way (20 percent's share truncates to 19).
    #[test]
    fn the_fall_that_kills_follows_the_health() {
        let h = vcod_common::pmove::FallHeights::default();
        assert_eq!(fall_that_kills(100, 100, h), 480.0);
        let at_20 = fall_that_kills(20, 100, h);
        assert!((at_20 - (256.0 + 224.0 * 0.21)).abs() < 0.01, "{at_20}");
        assert!(fall_that_kills(1, 100, h) < 262.0);
        let bad = vcod_common::pmove::FallHeights {
            min: 480.0,
            max: 256.0,
        };
        assert_eq!(fall_that_kills(1, 100, bad), f32::INFINITY);
    }

    /// mp_ship's hold ladder, seed 1327: a bot at the head of a 296-unit
    /// ladder backed off onto a climber 70-90 under it and fell to its
    /// death, hurt. A hurt bot treats the ladder as one whose fall kills and
    /// waits for a climber from two bodies' height under the head; a whole
    /// one only from 64, as before.
    #[test]
    fn a_hurt_bot_waits_for_a_climber_further_down() {
        let nodes = vec![
            Vec3::new(0.0, 0.0, 352.0),
            Vec3::new(40.0, 0.0, 56.0),
            Vec3::new(-48.0, 0.0, 352.0),
        ];
        let mut g = NavGraph::from_parts(nodes, vec![vec![1, 2], vec![], vec![0]]);
        g.ladder_boxes = vec![(Vec3::new(4.0, -12.0, 56.0), Vec3::new(5.0, 12.0, 352.0))];
        let body = Footing {
            still: false,
            ..Footing::STANDING
        };
        let at = Vec3::new(-2.0, 0.0, 352.0);
        let up = [(Vec3::new(20.0, 0.0, 262.0), Vec3::new(0.0, 0.0, 60.0))];
        let h = vcod_common::pmove::FallHeights::default();
        for (health, want) in [(100, None), (20, None), (10, Some(g.nodes[2]))] {
            let mut f = Follower {
                path: vec![0, 1],
                next: 1,
                fall_kills: Some(fall_that_kills(health, 100, h)),
                ..Default::default()
            };
            assert_eq!(
                f.ladder_yield(&g, at, body, &up, &[]),
                want,
                "health {health}"
            );
        }
    }

    /// mp_ship's hold ladder (3369, -379), seed 1337: its head node stands
    /// west of a doorway the climber walks through off the rungs. The wait
    /// holds at a spot farther than [`YIELD_NEAR`] from the rungs, and
    /// lasts while the climber, off them, walks between the spot and the
    /// rungs: let go there, two bots went for the doorway as the climber
    /// came out of it and the three pinned each other for 12 s.
    #[test]
    fn a_wait_aside_lasts_until_the_climber_is_past_the_spot() {
        let nodes = vec![
            Vec3::new(0.0, 0.0, 352.0),
            Vec3::new(40.0, 0.0, 56.0),
            Vec3::new(-70.0, 20.0, 352.0),
        ];
        let mut g = NavGraph::from_parts(nodes, vec![vec![1, 2], vec![], vec![0]]);
        g.ladder_boxes = vec![(Vec3::new(4.0, -12.0, 56.0), Vec3::new(5.0, 12.0, 352.0))];
        let mut f = Follower {
            path: vec![0, 1],
            next: 1,
            ..Default::default()
        };
        let body = Footing {
            still: false,
            ..Footing::STANDING
        };
        let (head, spot) = (Vec3::new(-2.0, 0.0, 352.0), g.nodes[2]);
        let up = [(Vec3::new(20.0, 0.0, 300.0), Vec3::new(0.0, 0.0, 60.0))];
        assert_eq!(f.ladder_yield(&g, head, body, &up, &[]), Some(spot));
        assert_eq!(
            f.ladder_yield(&g, spot, body, &up, &[]),
            Some(spot),
            "at the spot"
        );
        let out = [(Vec3::new(-20.0, 0.0, 352.0), Vec3::new(-150.0, 0.0, 0.0))];
        assert_eq!(
            f.ladder_yield(&g, spot, body, &[], &out),
            Some(spot),
            "coming out"
        );
        let past = [(Vec3::new(-90.0, 0.0, 352.0), Vec3::new(-150.0, 0.0, 0.0))];
        assert_eq!(
            f.ladder_yield(&g, spot, body, &[], &past),
            None,
            "past the spot"
        );
    }

    /// A ladder's head at 352 with its foot at 56 east of the rungs, and
    /// two deck nodes west of the head: a bot at the head about to back
    /// down waits at the far one while a climber comes up near the top,
    /// not while it goes down or is far below, and never on the rungs
    /// itself.
    #[test]
    fn a_bot_at_a_ladders_end_waits_aside_for_a_climber() {
        let nodes = vec![
            Vec3::new(0.0, 0.0, 352.0),
            Vec3::new(40.0, 0.0, 56.0),
            Vec3::new(-48.0, 0.0, 352.0),
            Vec3::new(-24.0, 20.0, 352.0),
        ];
        let mut g = NavGraph::from_parts(nodes, vec![vec![1, 2, 3], vec![], vec![0], vec![0]]);
        g.ladder_boxes = vec![(Vec3::new(4.0, -12.0, 56.0), Vec3::new(5.0, 12.0, 352.0))];
        let mut f = Follower {
            path: vec![0, 1],
            next: 1,
            ..Default::default()
        };
        let body = Footing {
            still: false,
            ..Footing::STANDING
        };
        let at = Vec3::new(-2.0, 0.0, 352.0);
        let up = [(Vec3::new(20.0, 0.0, 300.0), Vec3::new(0.0, 0.0, 60.0))];
        let hung = [(Vec3::new(20.0, 10.0, 352.5), Vec3::ZERO)];
        let down = [(Vec3::new(20.0, 0.0, 300.0), Vec3::new(0.0, 0.0, -60.0))];
        // Far down the rungs, the climb is the time it takes; the two pass
        // on the ladder.
        let low = [(Vec3::new(20.0, 0.0, 200.0), Vec3::new(0.0, 0.0, 60.0))];
        assert_eq!(f.ladder_yield(&g, at, body, &low, &[]), None);
        assert_eq!(f.ladder_yield(&g, at, body, &up, &[]), Some(g.nodes[2]));
        assert_eq!(f.ladder_yield(&g, at, body, &hung, &[]), Some(g.nodes[2]));
        assert_eq!(f.ladder_yield(&g, at, body, &down, &[]), None);
        assert_eq!(f.ladder_yield(&g, at, body, &[], &[]), None);
        let rungs = Footing {
            on_ladder: true,
            ..body
        };
        assert_eq!(f.ladder_yield(&g, at, rungs, &up, &[]), None);
        // Walking to the head with the ladder next waits too.
        f.next = 0;
        assert_eq!(f.ladder_yield(&g, at, body, &up, &[]), Some(g.nodes[2]));
        // A wait that runs out is not taken again for a while.
        for _ in 1..YIELD_TICKS {
            assert!(f.ladder_yield(&g, at, body, &up, &[]).is_some());
        }
        assert_eq!(f.ladder_yield(&g, at, body, &up, &[]), None);
        assert_eq!(f.ladder_yield(&g, at, body, &up, &[]), None);
        // A climber hanging still is waited for a shorter while.
        let mut f = Follower {
            path: vec![0, 1],
            next: 1,
            ..Default::default()
        };
        let waited =
            (0..YIELD_TICKS).take_while(|_| f.ladder_yield(&g, at, body, &hung, &[]).is_some());
        assert_eq!(waited.count() as u32, YIELD_HUNG);
    }

    /// A plan begun on the rungs under mp_ship's crow's nest plate starts
    /// at the head; passed from the rungs 8 under it, the bot turned for
    /// the rim's exit, let go and fell down the hatch. It is passed only
    /// on the ground.
    #[test]
    fn a_plan_from_the_rungs_passes_its_head_only_on_the_ground() {
        let head = Vec3::new(4963.9, 64.0, 1219.125);
        let exit = Vec3::new(4963.9, 35.5, 1219.125);
        let g = NavGraph::from_parts(vec![head, exit], vec![vec![1], vec![0]]);
        let mut f = Follower {
            path: vec![0, 1],
            ..Follower::default()
        };
        let rungs = Footing {
            still: false,
            on_ground: false,
            on_ladder: true,
            velocity: Vec3::new(0.0, 0.0, 120.0),
        };
        let mut budget = 100;
        let mut rand = || 0;
        let goal = crate::bots::Goal::Roam;
        let at = [4959.9, 62.4, 1210.9];
        assert_eq!(
            f.waypoint(&g, None, goal, at, rungs, &mut budget, &mut rand),
            Some(head.into())
        );
        let ground = Footing {
            on_ground: true,
            on_ladder: false,
            ..rungs
        };
        let at = [4962.0, 63.0, 1219.125];
        assert_eq!(
            f.waypoint(&g, None, goal, at, ground, &mut budget, &mut rand),
            Some(exit.into())
        );
    }

    /// mp_ship's crow's nest head stands on the ladder's plate in a hatch
    /// about 40 by 46 units, 11 units from the top's face to the west. The
    /// sweep west stops at the face before the first sample, and a sample
    /// north-west stands on the hatch's rim: the probe read no drop, and a
    /// spell's heading ran bots down the hatch.
    #[test]
    fn the_drop_probe_feels_the_crows_nest_hatch() {
        let Some(world) = map_world("mp_ship") else {
            return;
        };
        let plate = Vec3::new(4963.9, 64.0, 1219.125);
        let mut creases = Creases::default();
        let mut drop = |yaw: f32| {
            let (s, c) = yaw.to_radians().sin_cos();
            drop_ahead(
                &world.collision,
                plate,
                Vec3::new(c, s, 0.0),
                64.0,
                &mut creases,
            )
        };
        assert_eq!(drop(180.0), Some(f32::INFINITY));
        assert_eq!(drop(135.0), Some(f32::INFINITY));
        // The exit along the rim stays open.
        assert_eq!(drop(270.0), None);
    }

    /// Backing down a ladder with a climber coming up under it, a body
    /// climbs back to the head: one that met the climber stood on its head
    /// and slid off mp_ship's crow's nest ladder to the deck.
    #[test]
    fn a_bot_backing_down_onto_a_climber_climbs_back_up() {
        let head = Vec3::new(0.0, 0.0, 1219.0);
        let foot = Vec3::new(-4.0, 0.0, 692.0);
        let g = NavGraph::from_parts(vec![head, foot], vec![vec![1], vec![]]);
        let mut f = Follower {
            path: vec![0, 1],
            next: 1,
            ..Default::default()
        };
        let rungs = Footing {
            still: false,
            on_ground: false,
            on_ladder: true,
            velocity: Vec3::new(0.0, 0.0, -120.0),
        };
        let at = Vec3::new(-4.0, 1.0, 1131.0);
        let up = |z: f32| [(Vec3::new(-4.0, 2.0, z), Vec3::new(0.0, 0.0, 100.0))];
        assert_eq!(f.head_on_yield(&g, at, rungs, &up(1054.0)), Some(head));
        assert_eq!(f.head_on_yield(&g, at, rungs, &up(900.0)), None, "far down");
        let down = [(Vec3::new(-4.0, 2.0, 1054.0), Vec3::new(0.0, 0.0, -100.0))];
        assert_eq!(
            f.head_on_yield(&g, at, rungs, &down),
            None,
            "going down too"
        );
        let ground = Footing {
            on_ground: true,
            on_ladder: false,
            ..rungs
        };
        assert_eq!(
            f.head_on_yield(&g, head, ground, &up(1180.0)),
            None,
            "off the rungs"
        );
        // Climbing up, the climber below is behind.
        f.path = vec![1, 0];
        assert_eq!(f.head_on_yield(&g, at, rungs, &up(1054.0)), None);
        // Off a ladder no fall off kills, a body slides off the climber's
        // head and lives.
        f.path = vec![0, 1];
        let short = NavGraph::from_parts(
            vec![head, foot.with_z(1219.0 - 296.0)],
            vec![vec![1], vec![]],
        );
        assert_eq!(f.head_on_yield(&short, at, rungs, &up(1054.0)), None);
    }

    /// At the head of a ladder a fall off kills, a bot about to back down
    /// waits aside for a climber coming up from two bodies' height under
    /// it: from 70 to 100 under mp_ship's crow's nest plate, the back down
    /// met the climber's head and slid off the rungs down the hatch. At the
    /// foot, and on a shorter ladder, the window stays 64.
    #[test]
    fn a_bot_at_a_ladders_head_waits_for_a_climber_two_bodies_down() {
        let nodes = vec![
            Vec3::new(0.0, 0.0, 1219.0),
            Vec3::new(40.0, 0.0, 692.0),
            Vec3::new(-48.0, 0.0, 1219.0),
            Vec3::new(88.0, 0.0, 692.0),
        ];
        let mut g = NavGraph::from_parts(nodes, vec![vec![1, 2], vec![0, 3], vec![0], vec![1]]);
        g.ladder_boxes = vec![(Vec3::new(4.0, -12.0, 692.0), Vec3::new(5.0, 12.0, 1219.0))];
        let body = Footing {
            still: false,
            ..Footing::STANDING
        };
        let mut f = Follower {
            path: vec![0, 1],
            next: 1,
            ..Default::default()
        };
        let at = Vec3::new(-2.0, 0.0, 1219.0);
        let up = [(Vec3::new(20.0, 0.0, 1127.0), Vec3::new(0.0, 0.0, 60.0))];
        assert_eq!(f.ladder_yield(&g, at, body, &up, &[]), Some(g.nodes[2]));
        let mut f = Follower {
            path: vec![1, 0],
            next: 1,
            ..Default::default()
        };
        let at = Vec3::new(42.0, 0.0, 692.0);
        let down = [(Vec3::new(20.0, 0.0, 784.0), Vec3::new(0.0, 0.0, -60.0))];
        assert_eq!(
            f.ladder_yield(&g, at, body, &down, &[]),
            None,
            "at the foot"
        );
        // A ladder no fall off kills keeps the 64.
        let short = 1219.0 - 296.0;
        g.nodes[1].z = short;
        g.nodes[3].z = short;
        g.ladder_boxes[0].0.z = short;
        let mut f = Follower {
            path: vec![0, 1],
            next: 1,
            ..Default::default()
        };
        let at = Vec3::new(-2.0, 0.0, 1219.0);
        assert_eq!(
            f.ladder_yield(&g, at, body, &up, &[]),
            None,
            "a short ladder"
        );
    }

    /// mp_ship's crow's nest head has its two exits on the hatch's rim 26
    /// units from the ladder, none clear of it: with no spot to wait at, a
    /// bot backed down onto a climber and slid off the rungs. It waits two
    /// steps off on the rim, and goes back onto the head after.
    #[test]
    fn the_crows_nest_head_has_a_spot_to_wait_at() {
        let Some(world) = map_world("mp_ship") else {
            return;
        };
        let g = graph_for("mp_ship", &world);
        let head = g.nearest([4963.9, 64.0, 1219.125]).unwrap();
        let foot = g.nearest([4959.0, 64.0, 692.125]).unwrap();
        assert!(g.edges[head as usize].contains(&foot));
        let mut f = Follower {
            path: vec![head, foot],
            next: 1,
            ..Default::default()
        };
        let body = Footing {
            still: false,
            ..Footing::STANDING
        };
        let at = g.nodes[head as usize] + Vec3::new(-2.0, 0.0, 0.0);
        let up = [(Vec3::new(4960.0, 64.0, 1110.0), Vec3::new(0.0, 0.0, 95.0))];
        let spot = f.ladder_yield(&g, at, body, &up, &[]).expect("a spot");
        assert!((spot.z - 1219.125).abs() < 1.0, "on the rim: {spot}");
        assert!(spot.distance(g.nodes[head as usize]) > 2.0 * vcod_common::pmove::HALF_WIDTH);
        // A body's width clear of both exits: one 21 off the south exit
        // held the climber on the plate over the hatch.
        for exit in [[4963.9, 35.5, 1219.125], [4963.9, 92.5, 1219.125]] {
            let exit = g.nodes[g.nearest(exit).unwrap() as usize];
            assert!(
                spot.distance(exit) >= 2.0 * vcod_common::pmove::HALF_WIDTH,
                "{spot} by {exit}"
            );
        }
        // The head was passed: it is the waypoint again after the wait, or
        // the bot backs down off the rim from the spot aside.
        assert_eq!(f.current().0, Some(head));
    }

    /// mp_ship's crow's nest: a bot waiting aside at the head walked back
    /// for it the tick the climber stepped off the rungs onto the plate,
    /// and shoved it off the plate down the hatch (seeds 81, 383, 1273). The
    /// wait lasts while the climber bobs under the plate, stands on it or
    /// on an exit, and ends once it is up on the rail or off the rim's
    /// end; a body on the plate starts no wait.
    #[test]
    fn a_wait_at_the_crows_nest_lasts_until_the_climber_is_off_the_head() {
        let Some(world) = map_world("mp_ship") else {
            return;
        };
        let g = graph_for("mp_ship", &world);
        let head = g.nearest([4963.9, 64.0, 1219.125]).unwrap();
        let foot = g.nearest([4959.0, 64.0, 692.125]).unwrap();
        let body = Footing {
            still: false,
            ..Footing::STANDING
        };
        let at = g.nodes[head as usize] + Vec3::new(-2.0, 0.0, 0.0);
        let still = Vec3::ZERO;
        let plate = (Vec3::new(4963.9, 64.0, 1219.125), still);
        let mut f = Follower {
            path: vec![head, foot],
            next: 1,
            ..Default::default()
        };
        assert_eq!(
            f.ladder_yield(&g, at, body, &[], &[plate]),
            None,
            "no wait under way"
        );
        let up = (Vec3::new(4960.0, 64.0, 1110.0), Vec3::new(0.0, 0.0, 95.0));
        let spot = f.ladder_yield(&g, at, body, &[up], &[]).expect("a spot");
        let bob = (Vec3::new(4959.9, 62.0, 1208.0), Vec3::new(0.0, 0.0, -80.0));
        assert_eq!(
            f.ladder_yield(&g, spot, body, &[bob], &[]),
            Some(spot),
            "back onto the rungs"
        );
        let air = (Vec3::new(4959.9, 62.0, 1216.0), Vec3::new(0.0, 0.0, -40.0));
        assert_eq!(
            f.ladder_yield(&g, spot, body, &[], &[air]),
            Some(spot),
            "off them in the bob"
        );
        assert_eq!(
            f.ladder_yield(&g, spot, body, &[], &[plate]),
            Some(spot),
            "on the plate"
        );
        let exit = (
            Vec3::new(4963.9, 35.5, 1219.125),
            Vec3::new(0.0, -150.0, 0.0),
        );
        assert_eq!(
            f.ladder_yield(&g, spot, body, &[], &[exit]),
            Some(spot),
            "on the exit"
        );
        let rail = (Vec3::new(4932.9, 32.6, 1254.125), still);
        assert_eq!(
            f.ladder_yield(&g, spot, body, &[], &[rail]),
            None,
            "up on the rail"
        );
    }

    /// mp_ship's ladder at x 3793: a bot backing down over its hatch's
    /// lip at (3806, 60, 623) met a climber, headed straight for the spot
    /// aside, slid along the hatch's side and fell through to the deck at
    /// 304 (seed 1795, the bot hurt). Well past the head toward the rungs it
    /// steps back onto the head first; from the head it goes on to the spot.
    #[test]
    fn a_yield_from_over_the_lip_steps_back_onto_the_head_first() {
        let Some(world) = map_world("mp_ship") else {
            return;
        };
        let g = graph_for("mp_ship", &world);
        let head = g.nearest([3760.0, 65.0, 616.0]).unwrap();
        let foot = g.nearest([3809.0, 65.0, 304.0]).unwrap();
        assert!(g.edges[head as usize].contains(&foot));
        let body = Footing {
            still: false,
            ..Footing::STANDING
        };
        // Hurt: a fall of 300 kills, so the head's climber is looked for
        // [`HEAD_ON`] under it.
        let up = [(Vec3::new(3808.1, 63.1, 485.6), Vec3::new(0.0, 0.0, 95.0))];
        let mut f = Follower {
            path: vec![head, foot],
            next: 1,
            fall_kills: Some(300.0),
            ..Default::default()
        };
        let lip = Vec3::new(3806.5, 60.0, 623.125);
        assert_eq!(
            f.ladder_yield(&g, lip, body, &up, &[]),
            Some(g.nodes[head as usize])
        );
        let spot = f
            .ladder_yield(&g, g.nodes[head as usize], body, &up, &[])
            .expect("a spot");
        assert_ne!(spot, g.nodes[head as usize]);
        // Come to rest just past the head, it goes on to the spot: asking
        // for the head from there, a bot on mp_ship's (3337, -440, 56)
        // stood in its climber's way off the rungs (seed 3769).
        let beside = g.nodes[head as usize] + Vec3::X * 5.0;
        assert_eq!(f.ladder_yield(&g, beside, body, &up, &[]), Some(spot));
    }

    /// mp_ship's crow's nest: the creep off the head faces the rungs (they
    /// face -x at x 4975) and strafes to their middle at y 64; at the head
    /// of the ladder at x 3793, whose fall does not kill, it backs toward
    /// the foot as the graph's walk did.
    #[test]
    fn the_crows_nest_back_down_faces_the_rungs() {
        let Some(world) = map_world("mp_ship") else {
            return;
        };
        let g = graph_for("mp_ship", &world);
        let head = g.nearest([4963.9, 64.0, 1219.125]).unwrap();
        let foot = g.nearest([4959.0, 64.0, 692.125]).unwrap();
        let f = Follower {
            path: vec![head, foot],
            next: 1,
            ..Default::default()
        };
        let (yaw, middle) = f.back_face(&g).expect("a face");
        assert_eq!(yaw, 0.0);
        assert!((middle.y - 64.0).abs() < 0.5, "{middle}");
        let f = Follower {
            path: vec![
                g.nearest([3760.0, 65.0, 616.0]).unwrap(),
                g.nearest([3809.0, 65.0, 304.0]).unwrap(),
            ],
            next: 1,
            ..Default::default()
        };
        assert_eq!(f.back_face(&g), None);
    }

    /// mp_ship's ladder at x 3793: the climber stepped off the head and
    /// pressed on the bot waiting aside at (3751, 2, 616), the way south
    /// it was headed, and the wait held for the body between the spot and
    /// the rungs until it ran out (seed 761, 26 stalled bot-ticks). A body
    /// on the head holds the wait; one pressed on the waiter ends it.
    #[test]
    fn a_climber_pressed_on_the_waiter_ends_the_wait() {
        let Some(world) = map_world("mp_ship") else {
            return;
        };
        let g = graph_for("mp_ship", &world);
        let head = g.nearest([3760.0, 65.0, 616.0]).unwrap();
        let foot = g.nearest([3809.0, 65.0, 304.0]).unwrap();
        let body = Footing {
            still: false,
            ..Footing::STANDING
        };
        let mut f = Follower {
            path: vec![head, foot],
            next: 1,
            ..Default::default()
        };
        let up = (Vec3::new(3808.1, 65.7, 554.5), Vec3::new(0.0, 0.0, 95.0));
        let spot = f
            .ladder_yield(&g, g.nodes[head as usize], body, &[up], &[])
            .expect("a spot");
        let at = Vec3::new(3751.1, 1.9, 616.125);
        assert!(at.truncate().distance(spot.truncate()) < 16.0, "{spot}");
        let pushing = Vec3::new(0.0, -20.0, 0.0);
        let off = (Vec3::new(3785.0, 58.0, 623.125), pushing);
        assert_eq!(
            f.ladder_yield(&g, at, body, &[], &[off]),
            Some(spot),
            "on the head"
        );
        let pressed = (Vec3::new(3751.1, 32.1, 616.125), pushing);
        assert_eq!(f.ladder_yield(&g, at, body, &[], &[pressed]), None);
    }

    /// mp_ship's dome at (3648, -317, 618): backing off it for the floor
    /// at 352, a body stands on the dome's 45 degree face 23 under the
    /// node, still on the drop. Planned again from there, it walked off
    /// the dome's south side into the hold (seed 2245). It backs on.
    #[test]
    fn a_body_on_a_drops_own_slope_keeps_the_drop() {
        let top = Vec3::new(3648.0232, -316.91226, 618.36273);
        let bottom = Vec3::new(3679.998, -318.20682, 352.125);
        let g = NavGraph::from_parts(vec![top, bottom], vec![vec![1], vec![]]);
        let mut f = Follower::default();
        let goal = crate::bots::Goal::To(bottom.into());
        let mut budget = 100;
        let mut at =
            |p: [f32; 3]| f.waypoint(&g, None, goal, p, Footing::STANDING, &mut budget, &mut || 0);
        assert_eq!(at(top.into()), Some(bottom.into()));
        assert_eq!(at([3666.7, -317.2, 595.6]), Some(bottom.into()));
        assert!(!f.fell);
    }

    /// mp_ship's crow's nest top at 1254: a bot stepping toward the rim
    /// over a climber coming up the hatch met its box, and `StuckInClient`
    /// threw it 190 units/s west off the top's far side (seed 1307). It
    /// holds still while the step closes on the climber, and only then.
    #[test]
    fn a_body_holds_still_over_a_climber_rising_under_it() {
        let at = Vec3::new(4936.2, 56.0, 1254.125);
        let rim = Vec3::new(4963.9, 35.5, 1219.125);
        let climber = (Vec3::new(4960.0, 63.0, 1187.0), Vec3::new(0.0, 0.0, 95.0));
        let mut f = Follower::default();
        let body = Footing::STANDING;
        assert_eq!(f.rising_hold(at, rim, body, &[climber]), Some(at));
        // Stepping away, standing on the rungs, or the climber still.
        let away = Vec3::new(4900.0, 56.0, 1254.125);
        assert_eq!(f.rising_hold(at, away, body, &[climber]), None);
        let rungs = Footing {
            on_ladder: true,
            ..body
        };
        assert_eq!(f.rising_hold(at, rim, rungs, &[climber]), None);
        let hung = (climber.0, Vec3::ZERO);
        assert_eq!(f.rising_hold(at, rim, body, &[hung]), None);
        // Nor a body bound down the ladder, which the ladder waits own.
        let foot = Vec3::new(4959.0, 64.0, 692.125);
        assert_eq!(f.rising_hold(at, foot, body, &[climber]), None);
        // A climber far under the feet holds nothing yet.
        let deep = (Vec3::new(4960.0, 63.0, 1100.0), climber.1);
        assert_eq!(f.rising_hold(at, rim, body, &[deep]), None);
        // The hold ends after RISING_TICKS (a tick without one starts it afresh).
        for _ in 0..RISING_TICKS {
            assert_eq!(f.rising_hold(at, rim, body, &[climber]), Some(at));
        }
        assert_eq!(f.rising_hold(at, rim, body, &[climber]), None);
    }

    /// mp_ship's plate over the ladder at x 3792: two bots off the rungs at
    /// y 49 and y 79, each pressed on the other out of the hatch's lane
    /// (y 58-72) toward the same waypoint, stood 180 ticks (seed 3223).
    /// After `PRESSED_TICKS` one of the two steps back from the other.
    #[test]
    fn of_two_bodies_pressed_together_one_gives_way() {
        let wp = Vec3::new(3779.0, 64.0, 623.125);
        let (south, north) = (
            Vec3::new(3800.1, 48.9, 623.125),
            Vec3::new(3800.1, 79.0, 623.125),
        );
        let body = Footing::STANDING;
        let (mut s, mut n) = (Follower::default(), Follower::default());
        for _ in 1..PRESSED_TICKS {
            assert_eq!(
                s.pressed_yield(south, wp, body, &[(north, Vec3::ZERO)]),
                None
            );
            assert_eq!(
                n.pressed_yield(north, wp, body, &[(south, Vec3::ZERO)]),
                None
            );
        }
        assert_eq!(
            s.pressed_yield(south, wp, body, &[(north, Vec3::ZERO)]),
            None
        );
        let spot = n
            .pressed_yield(north, wp, body, &[(south, Vec3::ZERO)])
            .expect("the later in (y, x) gives way");
        assert!(
            (spot - Vec3::new(3800.1, 103.0, 623.125)).length() < 0.5,
            "{spot}"
        );
        // It holds the spot for the give way, the other body gone or not.
        for _ in 1..GIVE_TICKS {
            assert_eq!(n.pressed_yield(north, wp, body, &[]), Some(spot));
        }
        assert_eq!(n.pressed_yield(north, wp, body, &[]), None);
        // A body on the move is no press.
        let mut m = Follower::default();
        let moving = (south, Vec3::new(0.0, 60.0, 0.0));
        for _ in 0..2 * PRESSED_TICKS {
            assert_eq!(m.pressed_yield(north, wp, body, &[moving]), None);
        }
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

    /// A rebuild walks round the vehicles, movers and world copies where
    /// they stand, so the cache keys on each: keyed without them, a bas
    /// gate got whichever graph a test beside it built first.
    #[test]
    fn the_cache_key_covers_every_placement() {
        let Some(world) = map_world("mp_carentan") else {
            return;
        };
        let c = &world.collision;
        let key = |c: &CollisionWorld| cache_key("mp_carentan", c, &world, MoveRules::V1_1);
        let bare = key(c);
        let at = |x: f32| Some((Vec3::new(x, 0.0, 0.0), Vec3::ZERO, 1));
        c.set_model_instance(300, 1, at(0.0));
        let parked = key(c);
        assert_ne!(parked, bare);
        c.set_model_instance(300, 1, at(64.0));
        assert_ne!(key(c), parked, "a vehicle that moved");
        c.set_model_instance(300, 1, None);
        assert_eq!(key(c), bare);
        c.set_model_pose(1, Vec3::new(0.0, 0.0, 512.0), Vec3::ZERO);
        assert_ne!(key(c), bare, "a mover off its spawn");
        c.reset_model_pose(1);
        assert_eq!(key(c), bare);
        c.set_world_copy(301, Some((Vec3::ZERO, Vec3::ZERO, 1)));
        assert_ne!(key(c), bare, "a world copy");
        c.set_world_copy(301, None);
        c.set_arith(vcod_common::patch::Arith::Stored);
        assert_ne!(key(c), bare, "another engine's arithmetic");
    }
}
