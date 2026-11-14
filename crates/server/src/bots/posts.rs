//! Guard posts: where a team's bots stand round an objective they guard,
//! hold or wait at (docs/research/bot-objectives.md 13). The driver
//! surveys an objective once per graph ([`survey`]): the graph nodes round
//! it with a line to it, each scored by how many of the ways in
//! ([`Survey::approaches`]) a crouched body there hides from. Once a tick,
//! before any bot thinks, it deals each side's guards of one objective
//! posts apart from each other ([`assign`]) and moves each on to another
//! after a spell there. The brain walks to its post and holds it
//! ([`super::BotView::guard_post`]).

use std::cmp::Reverse;
use std::collections::{BTreeMap, BinaryHeap};
use std::sync::Arc;

use glam::Vec3;

use crate::nav::NavGraph;

/// What a bot guards, as the brain names it ([`super::Bot::guard_anchor`]).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Anchor {
    /// A ring round `at`, `inner` to `outer` across, with a line to `at`.
    Ring {
        at: [f32; 3],
        inner: f32,
        outer: f32,
    },
    /// Inside a zone's bounds, [`super::SITE_MARGIN`] in: a domination
    /// flag, a capture goal. `stand` is the zone's own stand.
    Zone {
        mins: [f32; 3],
        maxs: [f32; 3],
        stand: [f32; 3],
    },
    /// Within `radius` across and `height` up or down of a radio.
    Radio {
        at: [f32; 3],
        radius: f32,
        height: f32,
        stand: [f32; 3],
    },
}

/// An anchor's bits: the survey cache and the deal key on them.
pub type Key = [u32; 10];

impl Anchor {
    pub fn key(&self) -> Key {
        let mut k = [0u32; 10];
        let mut put = |i: usize, v: f32| k[i] = v.to_bits();
        match *self {
            Anchor::Ring { at, inner, outer } => {
                put(0, 1.0);
                (0..3).for_each(|i| put(1 + i, at[i]));
                put(4, inner);
                put(5, outer);
            }
            Anchor::Zone { mins, maxs, .. } => {
                put(0, 2.0);
                (0..3).for_each(|i| put(1 + i, mins[i]));
                (0..3).for_each(|i| put(4 + i, maxs[i]));
            }
            Anchor::Radio {
                at, radius, height, ..
            } => {
                put(0, 3.0);
                (0..3).for_each(|i| put(1 + i, at[i]));
                put(4, radius);
                put(5, height);
            }
        }
        k
    }

    /// The point a post looks at and the survey floods from.
    pub fn centre(&self) -> [f32; 3] {
        match *self {
            Anchor::Ring { at, .. } | Anchor::Radio { at, .. } => at,
            Anchor::Zone { stand, .. } => stand,
        }
    }

    /// Where the flood starts: a ring's point may stand in the air (a
    /// bomb's trigger, a radio), a zone's stand is a node.
    fn start(&self) -> [f32; 3] {
        match *self {
            Anchor::Ring { at, .. } => at,
            Anchor::Zone { stand, .. } | Anchor::Radio { stand, .. } => stand,
        }
    }

    /// The anchor's reach across, which the ways in lie beyond.
    fn reach(&self) -> f32 {
        match *self {
            Anchor::Ring { outer, .. } => outer,
            Anchor::Zone { mins, maxs, .. } => 0.5 * (maxs[0] - mins[0]).hypot(maxs[1] - mins[1]),
            Anchor::Radio { radius, .. } => radius,
        }
    }

    /// Whether feet at `n` stand where the anchor asks.
    fn holds(&self, n: Vec3) -> bool {
        match *self {
            Anchor::Ring { at, inner, outer } => {
                let d = (n - Vec3::from(at)).truncate().length();
                d >= inner && d <= outer && (n.z - at[2]).abs() < RING_RISE
            }
            Anchor::Zone { mins, maxs, .. } => {
                (0..2).all(|i| {
                    n[i] >= mins[i] + super::SITE_MARGIN && n[i] <= maxs[i] - super::SITE_MARGIN
                }) && n.z <= maxs[2]
                    && n.z + super::PLAYER_HEIGHT >= mins[2]
            }
            Anchor::Radio {
                at, radius, height, ..
            } => (n - Vec3::from(at)).truncate().length() < radius && (n.z - at[2]).abs() < height,
        }
    }
}

/// A ring's post stands no more than this above or below its point.
const RING_RISE: f32 = 160.0;
/// A post is reached along the graph within this many times its distance
/// across from the anchor (plus a grid cell): one behind a wall with the
/// way round it far off is no post of this objective.
const WALK_SLACK: f32 = 1.6;
/// Kept candidates stand at least this far apart.
const THIN: f32 = 40.0;
/// At most this many candidates per anchor, nearest along the graph first.
const MAX_POSTS: usize = 48;
/// The ways in: nodes this far past the anchor's reach along the graph,
/// the one nearest [`APPROACH_AT`] past it in each of [`SECTORS`] bearings.
const APPROACH_NEAR: f32 = 200.0;
const APPROACH_FAR: f32 = 700.0;
const APPROACH_AT: f32 = 400.0;
const SECTORS: usize = 8;
/// A standing eye, a crouched eye and a crouched chest over the feet.
const EYE_STAND: f32 = vcod_common::pmove::VIEW_STAND;
const EYE_CROUCH: f32 = 40.0;
const CHEST_CROUCH: f32 = EYE_CROUCH * (2.0 / 3.0);
/// A ring's post sees this high over its point: a planted bomb's or a
/// site's chest-high middle.
const SIGHT_RISE: f32 = 40.0;

/// A post: the feet (a graph node), the share of the ways in a crouched
/// body there hides from, and whether it hides from most of them (the
/// bot crouches there).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Post {
    pub at: Vec3,
    pub score: f32,
    pub crouch: bool,
}

/// One objective's posts and the eyes on its ways in.
#[derive(Debug, Default)]
pub struct Survey {
    /// Best first.
    pub posts: Vec<Post>,
    /// The ways in: their nodes' feet, and a standing eye over each.
    pub ways: Vec<Vec3>,
    pub approaches: Vec<Vec3>,
}

impl Survey {
    /// The share of the ways in with a line to a body with its feet at
    /// `feet` and its eye `eye` over them (to the eye or the chest);
    /// `None` with no way in. `bot_match`'s exposure.
    pub fn exposure(
        &self,
        clear: &dyn Fn(Vec3, Vec3) -> bool,
        feet: Vec3,
        eye: f32,
    ) -> Option<f32> {
        let n = self.approaches.len();
        (n > 0).then(|| 1.0 - hidden_from(&self.approaches, clear, feet, eye) as f32 / n as f32)
    }
}

/// How many of `from` see neither the eye `eye` over `feet` nor the chest
/// two thirds of the way up.
fn hidden_from(from: &[Vec3], clear: &dyn Fn(Vec3, Vec3) -> bool, feet: Vec3, eye: f32) -> usize {
    let (head, chest) = (feet + Vec3::Z * eye, feet + Vec3::Z * eye * (2.0 / 3.0));
    from.iter()
        .filter(|a| !clear(**a, head) && !clear(**a, chest))
        .count()
}

/// Path lengths from `start` along the graph's edges (no risky edge), out
/// to `far`; unreached nodes left out.
fn flood(g: &NavGraph, start: u32, far: f32) -> BTreeMap<u32, f32> {
    let mut cost: BTreeMap<u32, f32> = BTreeMap::new();
    let mut open = BinaryHeap::new();
    cost.insert(start, 0.0);
    open.push((Reverse(0u32), start));
    while let Some((Reverse(c), n)) = open.pop() {
        let here = cost[&n];
        if c as f32 > here + 1.0 {
            continue;
        }
        let p = g.nodes[n as usize];
        for &m in &g.edges[n as usize] {
            if g.risky.contains(&(n, m)) {
                continue;
            }
            let next = here + p.distance(g.nodes[m as usize]);
            if next <= far && cost.get(&m).is_none_or(|c| next < *c) {
                cost.insert(m, next);
                open.push((Reverse(next as u32), m));
            }
        }
    }
    cost
}

/// The posts round `a` and its ways in, `clear` the shot trace's verdict
/// on a segment. Floods the graph once from the anchor and traces each
/// kept candidate against each way in: a few hundred traces, once per
/// anchor and graph.
pub fn survey(g: &NavGraph, clear: &dyn Fn(Vec3, Vec3) -> bool, a: &Anchor) -> Survey {
    let Some(start) = g.nearest(a.start()) else {
        return Survey::default();
    };
    let reach = a.reach();
    let cost = flood(g, start, reach + APPROACH_FAR);
    let centre = Vec3::from(a.centre());
    // The ways in: per bearing from the centre, the node nearest
    // `APPROACH_AT` past the reach along the graph.
    let mut ways: [Option<(f32, u32)>; SECTORS] = [None; SECTORS];
    for (&n, &c) in &cost {
        if c < reach + APPROACH_NEAR {
            continue;
        }
        let d = g.nodes[n as usize] - centre;
        let bearing = d.y.atan2(d.x).rem_euclid(std::f32::consts::TAU);
        let s = ((bearing / std::f32::consts::TAU) * SECTORS as f32) as usize % SECTORS;
        let off = (c - reach - APPROACH_AT).abs();
        if ways[s].is_none_or(|(o, _)| off < o) {
            ways[s] = Some((off, n));
        }
    }
    let ways: Vec<Vec3> = ways
        .iter()
        .flatten()
        .map(|(_, n)| g.nodes[*n as usize])
        .collect();
    let approaches: Vec<Vec3> = ways.iter().map(|w| *w + Vec3::Z * EYE_STAND).collect();
    // Candidates nearest along the graph first, thinned.
    let mut near: Vec<(f32, u32)> = cost
        .iter()
        .filter(|(n, c)| {
            let p = g.nodes[**n as usize];
            a.holds(p) && **c <= (p - centre).truncate().length() * WALK_SLACK + g.spacing * 2.0
        })
        .map(|(n, c)| (*c, *n))
        .collect();
    near.sort_by(|x, y| x.0.total_cmp(&y.0).then(x.1.cmp(&y.1)));
    let mut kept: Vec<Vec3> = Vec::new();
    for (_, n) in near {
        let p = g.nodes[n as usize];
        if kept.iter().all(|k| k.distance(p) >= THIN) {
            kept.push(p);
            if kept.len() >= MAX_POSTS {
                break;
            }
        }
    }
    let ring = matches!(a, Anchor::Ring { .. });
    let sight = centre + Vec3::Z * SIGHT_RISE;
    let mut posts: Vec<Post> = kept
        .into_iter()
        .filter(|p| !ring || clear(*p + Vec3::Z * EYE_CROUCH, sight))
        .map(|p| {
            let hid = approaches
                .iter()
                .filter(|w| {
                    !clear(**w, p + Vec3::Z * EYE_CROUCH) && !clear(**w, p + Vec3::Z * CHEST_CROUCH)
                })
                .count();
            let n = approaches.len().max(1);
            Post {
                at: p,
                score: hid as f32 / n as f32,
                crouch: !approaches.is_empty() && 2 * hid >= approaches.len(),
            }
        })
        .collect();
    // Stable: equal scores keep the graph's order, nearest first.
    posts.sort_by(|x, y| y.score.total_cmp(&x.score));
    Survey {
        posts,
        ways,
        approaches,
    }
}

/// A lane is dealt to a bot this far or more from its objective, and only
/// one whose way in it reaches in no more than [`LANE_DETOUR`] times the
/// straight line (by straight lines).
pub const LANE_FAR: f32 = 1200.0;
pub const LANE_DETOUR: f32 = 1.3;
/// A lane's way in is passed within this, or once the bot stands nearer
/// the objective than it; given up past [`LANE_MS`].
pub const LANE_PASS: f32 = 160.0;
pub const LANE_MS: i32 = 30_000;

/// A bot's way in to its objective as the driver keeps it.
#[derive(Clone, Copy, Debug)]
pub struct Lane {
    pub key: Key,
    /// `None` once passed or none dealt: no lane till the objective moves.
    pub via: Option<Vec3>,
    pub since: i32,
}

/// Deals one side's bots making for one objective (`bots`: slot and feet,
/// in slot order) a way in each, those [`LANE_FAR`] or more from `centre`
/// that hold none: the most direct ways first and in turn, so a team goes
/// in by as many ways as it has bots, up to the ways the detour cap
/// leaves. A bot keeps its lane until passed.
pub fn lanes(
    survey: &Survey,
    key: Key,
    centre: Vec3,
    bots: &[(usize, Vec3)],
    kept: &mut BTreeMap<usize, Lane>,
    now: i32,
) -> Vec<(usize, Vec3)> {
    let mut out = Vec::new();
    for (i, &(slot, feet)) in bots.iter().enumerate() {
        let lane = match kept.get(&slot).copied().filter(|l| l.key == key) {
            Some(l) => l,
            None if feet.distance(centre) < LANE_FAR => continue,
            None => {
                let direct = feet.distance(centre).max(1.0);
                let mut ok: Vec<(f32, Vec3)> = survey
                    .ways
                    .iter()
                    .map(|w| ((feet.distance(*w) + w.distance(centre)) / direct, *w))
                    .filter(|(d, _)| *d <= LANE_DETOUR)
                    .collect();
                ok.sort_by(|a, b| a.0.total_cmp(&b.0));
                Lane {
                    key,
                    via: (!ok.is_empty()).then(|| ok[i % ok.len()].1),
                    since: now,
                }
            }
        };
        let passed = |via: Vec3| {
            feet.distance(via) < LANE_PASS
                || feet.distance(centre) < via.distance(centre)
                || now.wrapping_sub(lane.since) >= LANE_MS
        };
        let via = lane.via.filter(|v| !passed(*v));
        kept.insert(slot, Lane { via, ..lane });
        if let Some(v) = via {
            out.push((slot, v));
        }
    }
    out
}

/// Guards of one objective keep at least this far apart.
pub const SPREAD: f32 = 128.0;
/// A post this near across (and [`super::ARRIVE_HEIGHT`] up or down) is
/// reached.
pub const ARRIVE: f32 = 32.0;
/// A guard holds a post this long once on it, then takes another, ms: the
/// least and the spread a bot's own draw adds.
pub const HOLD_MS: i32 = 8000;
pub const HOLD_SPREAD_MS: i32 = 8000;
/// A post not reached in this long is given up for another, ms.
pub const WALK_MS: i32 = 20_000;
/// What a post's distance from the bot takes off its score, per unit: of
/// two posts as good, the nearer.
const NEAR_WEIGHT: f32 = 1.0 / 1500.0;
/// The most a bot's own draw adds to a post's score: guards of one
/// objective spread over the good posts, not queue for the best.
const JITTER: f32 = 0.35;

/// A guard's post as the driver keeps it between ticks.
#[derive(Clone, Copy, Debug)]
pub struct Held {
    pub key: Key,
    pub post: Post,
    /// `level.time` it was dealt, and reached.
    pub since: i32,
    pub reached: Option<i32>,
    /// How long it holds the post once reached.
    pub hold: i32,
    /// Posts dealt this life, the rotation's draw.
    pub turn: u32,
}

/// splitmix64 over a slot, a round and a turn.
fn draw(slot: usize, round: u32, turn: u32) -> u64 {
    let mut z = ((slot as u64) << 40) ^ (u64::from(round) << 8) ^ u64::from(turn);
    z = z.wrapping_add(0x9e37_79b9_7f4a_7c15);
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^ (z >> 31)
}

/// Deals one side's guards of one objective (`guards`: slot and feet, in
/// slot order) their posts this tick, keeping each on the post it holds
/// until its spell there ends or a teammate earlier in slot order stands
/// on it, then the best free post at least [`SPREAD`] from the others'
/// and off its last. With more guards than posts that far apart, the rest
/// share.
pub fn assign(
    survey: &Survey,
    key: Key,
    guards: &[(usize, Vec3)],
    held: &mut BTreeMap<usize, Held>,
    now: i32,
    round: u32,
) -> Vec<(usize, Post)> {
    let mut taken: Vec<Vec3> = Vec::new();
    let mut out = Vec::new();
    if survey.posts.is_empty() {
        return out;
    }
    let reached = |feet: Vec3, p: Vec3| {
        (feet - p).truncate().length() < ARRIVE && (feet.z - p.z).abs() < super::ARRIVE_HEIGHT
    };
    for &(slot, feet) in guards {
        let prior = held.get(&slot).copied().filter(|h| h.key == key);
        let keep = prior.filter(|h| {
            let done = h.reached.is_some_and(|t| now.wrapping_sub(t) >= h.hold);
            let late = h.reached.is_none() && now.wrapping_sub(h.since) >= WALK_MS;
            !done && !late && taken.iter().all(|t| t.distance(h.post.at) >= SPREAD)
        });
        let h = match keep {
            Some(mut h) => {
                if h.reached.is_none() && reached(feet, h.post.at) {
                    h.reached = Some(now);
                }
                h
            }
            None => {
                let turn = prior.map_or(0, |h| h.turn + 1);
                // The post before, which the pick passes over.
                let last = prior.map(|h| h.post.at);
                let r = draw(slot, round, turn);
                let value = |i: usize, p: &Post| {
                    let jitter = ((r.rotate_left(i as u32 * 7) & 0xffff) as f32 / 65535.0) * JITTER;
                    p.score + jitter - p.at.distance(feet) * NEAR_WEIGHT
                };
                let fresh = |p: &Post| last.is_none_or(|l| l.distance(p.at) >= SPREAD * 0.5);
                let pick = |spread: bool| {
                    survey
                        .posts
                        .iter()
                        .enumerate()
                        .filter(|(_, p)| fresh(p))
                        .filter(|(_, p)| {
                            !spread || taken.iter().all(|t| t.distance(p.at) >= SPREAD)
                        })
                        .max_by(|(i, p), (j, q)| value(*i, p).total_cmp(&value(*j, q)))
                        .map(|(_, p)| *p)
                };
                let Some(post) = pick(true).or_else(|| pick(false)).or(prior.map(|h| h.post))
                else {
                    continue;
                };
                Held {
                    key,
                    post,
                    since: now,
                    reached: reached(feet, post.at).then_some(now),
                    hold: HOLD_MS + (r % HOLD_SPREAD_MS as u64) as i32,
                    turn,
                }
            }
        };
        taken.push(h.post.at);
        held.insert(slot, h);
        out.push((slot, h.post));
    }
    out
}

/// What the driver keeps for the posts: the surveys of the graph they were
/// made on, and each guard's post.
#[derive(Default)]
pub struct Posts {
    /// The graph the surveys were made on (`Arc::as_ptr`).
    graph: usize,
    pub surveys: Vec<(Key, Arc<Survey>)>,
    pub held: BTreeMap<usize, Held>,
    /// Per bot, the last exposure sample ([`Survey::exposure`]).
    pub exposure: BTreeMap<usize, f32>,
    /// Per team (axis 0, allies 1), the S&D site its defenders converge on
    /// and the `level.time` its last sighting there runs out.
    pub hot: [Option<(usize, i32)>; 2],
    /// The round [`Self::hot`] was read in.
    pub round: u32,
    /// Per bot, its way in to the objective it makes for ([`lanes`]).
    pub lanes: BTreeMap<usize, Lane>,
}

impl Posts {
    /// The survey of `a` on `g`, made now if it is new; `clear` traces.
    pub fn survey(
        &mut self,
        g: &Arc<NavGraph>,
        clear: &dyn Fn(Vec3, Vec3) -> bool,
        a: &Anchor,
    ) -> Arc<Survey> {
        let ptr = Arc::as_ptr(g) as usize;
        if ptr != self.graph {
            self.graph = ptr;
            self.surveys.clear();
            self.held.clear();
        }
        let key = a.key();
        if let Some((_, s)) = self.surveys.iter().find(|(k, _)| *k == key) {
            return s.clone();
        }
        let s = Arc::new(survey(g, clear, a));
        // A planted bomb is a new anchor every round: keep the cache small.
        if self.surveys.len() >= 64 {
            self.surveys.remove(0);
        }
        self.surveys.push((key, s.clone()));
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Posts in a row 40 apart, every fourth one hidden better.
    fn row() -> Survey {
        let mut posts: Vec<Post> = (0..12)
            .map(|i| Post {
                at: Vec3::new(40.0 * i as f32, 0.0, 0.0),
                score: if i % 4 == 0 { 1.0 } else { 0.5 },
                crouch: i % 4 == 0,
            })
            .collect();
        posts.sort_by(|x, y| y.score.total_cmp(&x.score));
        Survey {
            posts,
            ways: vec![Vec3::new(600.0, 500.0, 0.0), Vec3::new(600.0, -500.0, 0.0)],
            approaches: Vec::new(),
        }
    }

    /// Three guards standing together are dealt posts at least `SPREAD`
    /// apart, and keep them tick to tick.
    #[test]
    fn guards_of_one_objective_get_posts_apart() {
        let s = row();
        let guards = [(1, Vec3::ZERO), (2, Vec3::ZERO), (3, Vec3::ZERO)];
        let mut held = BTreeMap::new();
        let a = assign(&s, [0; 10], &guards, &mut held, 0, 1);
        assert_eq!(a.len(), 3);
        for (i, x) in a.iter().enumerate() {
            for y in &a[i + 1..] {
                assert!(x.1.at.distance(y.1.at) >= SPREAD, "{x:?} {y:?}");
            }
        }
        let b = assign(&s, [0; 10], &guards, &mut held, 50, 1);
        assert_eq!(a, b, "kept");
    }

    /// A guard on its post moves to another once its hold there runs out,
    /// and not before.
    #[test]
    fn a_guard_moves_on_after_its_hold() {
        let s = row();
        let mut held = BTreeMap::new();
        let far = [(1, Vec3::new(0.0, 3000.0, 0.0))];
        let first = assign(&s, [0; 10], &far, &mut held, 0, 1)[0].1;
        let on = [(1, first.at)];
        assign(&s, [0; 10], &on, &mut held, 50, 1);
        assert_eq!(held[&1].reached, Some(50));
        let hold = held[&1].hold;
        assert!((HOLD_MS..HOLD_MS + HOLD_SPREAD_MS).contains(&hold));
        let before = assign(&s, [0; 10], &on, &mut held, 50 + hold - 50, 1)[0].1;
        assert_eq!(before, first);
        let after = assign(&s, [0; 10], &on, &mut held, 50 + hold, 1)[0].1;
        assert!(after.at.distance(first.at) >= SPREAD * 0.5, "moved on");
    }

    /// Two bots far off get different ways in; a third near the objective
    /// gets none.
    #[test]
    fn bots_far_off_take_different_lanes() {
        let s = row();
        let centre = Vec3::ZERO;
        let bots = [
            (1, Vec3::new(3000.0, 0.0, 0.0)),
            (2, Vec3::new(3000.0, 50.0, 0.0)),
            (3, Vec3::new(300.0, 0.0, 0.0)),
        ];
        let mut kept = BTreeMap::new();
        let l = lanes(&s, [0; 10], centre, &bots, &mut kept, 0);
        assert_eq!(l.len(), 2);
        assert_ne!(l[0].1, l[1].1);
        // Once nearer the objective than its way in, the lane is passed.
        let on = [(1, Vec3::new(100.0, 0.0, 0.0))];
        assert!(lanes(&s, [0; 10], centre, &on, &mut kept, 50).is_empty());
    }
}
