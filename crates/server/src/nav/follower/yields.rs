//! Giving way: to a climber, a bot abreast or head on, a body, a press.

use super::*;

/// How near a mantle's landing another body stands, flat, for the climb
/// to end in him ([`Follower::mantle_held`]): two half-widths and a margin.
const MANTLE_HELD_REACH: f32 = 2.0 * vcod_common::pmove::HALF_WIDTH + 2.0;
/// How near the landing, flat, a body under it gives the climb up.
const MANTLE_HELD_NEAR: f32 = 96.0;
/// A body this slow on the landing stands still on it.
const MANTLE_HELD_STILL: f32 = 10.0;

impl Follower {
    /// Whether the body, on the ground under the landing of the mantle
    /// edge it follows, would climb into another standing still there
    /// (`others`, off the rungs: origin and velocity): the climb ends in
    /// him, the two boxes push apart and the climber falls back to climb
    /// again (mp_crash's ledge at (1403, 196, 188), bot-combat.md 20).
    /// 1.7's pmove offers that climb too (bot-objectives.md 12.20). The
    /// caller plans round the edge ([`Self::stuck`]).
    pub fn mantle_held(
        &self,
        g: &NavGraph,
        at: Vec3,
        on_ground: bool,
        others: &[(Vec3, Vec3)],
    ) -> bool {
        use vcod_common::pmove::STEPSIZE;
        let Some(edge) = self.mantle_edge(g) else {
            return false;
        };
        let land = self.mantle_land(g).unwrap_or(g.nodes[edge.1 as usize]);
        on_ground
            && land.z - at.z > STEPSIZE
            && (land - at).truncate().length() < MANTLE_HELD_NEAR
            && others.iter().any(|&(o, v)| {
                v.length() < MANTLE_HELD_STILL
                    && (o - land).truncate().length() < MANTLE_HELD_REACH
                    && (o.z - land.z).abs() < STEPSIZE
            })
    }

    /// Where to wait instead of following the path: the bot is at one end
    /// of the ladder its path takes next, off the rungs, and another player
    /// on those rungs is about to step off there or hangs there. Two players
    /// meeting head-on at a ladder pin each other (bot-navigation.md
    /// section 3, "the head-on ladder"). The bot steps to the neighbour of
    /// its end farthest from the ladder until the rungs clear, at most
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
        // A climber holding still under the body counts too: it holds for
        // this body to climb back ([`Self::under_hold`]).
        climbers
            .iter()
            .any(|&(c, v)| {
                (v.z > 1.0 || v.length() < 1.0)
                    && c.z < at.z
                    && at.z - c.z < HEAD_ON
                    && (c - at).truncate().length() < 2.0 * HALF_WIDTH + 2.0
            })
            .then_some(head)
    }

    /// Whether the body, climbing toward a ladder's head on the rungs, holds
    /// still under another body over it on them: within [`HEAD_ON`] above,
    /// two half-widths flat, and not rising. A bot climbing back to the head
    /// off a climber ([`Self::head_on_yield`]) can bob under the lip and fall
    /// back; the climber rising under it stood it on his head and the two
    /// boxes pushed apart (`game::stuck::stuck_in_client`): up mp_ship's
    /// crow's nest the climber was pushed off the rungs at (4964, 86, 1141)
    /// and fell to 568 (bot-navigation.md section 3). At most
    /// [`RISING_TICKS`]: a body that stands still over the rungs holds no
    /// one for good. `others` are every other player, on the rungs or off.
    pub fn under_hold(
        &mut self,
        g: &NavGraph,
        at: Vec3,
        body: Footing,
        others: &[(Vec3, Vec3)],
    ) -> bool {
        use vcod_common::pmove::HALF_WIDTH;
        let over = body.on_ladder
            && self.climbing(g)
            && others.iter().any(|&(c, v)| {
                let dz = c.z - at.z;
                v.z < 1.0
                    && dz > 1.0
                    && dz < HEAD_ON
                    && (c - at).truncate().length() < 2.0 * HALF_WIDTH + 2.0
            });
        if !over || self.under_ticks >= RISING_TICKS {
            if !over {
                self.under_ticks = 0;
            }
            return false;
        }
        self.under_ticks += 1;
        true
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
    /// beside it past the same body (bot-navigation.md section 3.8, "Bodies
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
    /// (bot-navigation.md section 3.8, "Bodies in the way").
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
                self.drop_path();
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
    pub(super) fn meets_shunned(&self, g: &NavGraph, here: Vec3, path: &[u32]) -> bool {
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
        let near = || g.next(e).filter(floor);
        let on =
            |ms: &[u32]| -> Vec<u32> { ms.iter().flat_map(|&m| g.next(m).filter(floor)).collect() };
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::nav::build::tests::map_world;

    /// A mantle edge up 60 onto a ledge: a body standing still on its
    /// landing holds the climb, one walking past or off to the side does
    /// not, and neither does one on the next edge.
    #[test]
    fn a_body_still_on_a_mantle_landing_holds_the_climb() {
        let (foot, top, on) = (
            Vec3::new(1440.0, 180.0, 127.0),
            Vec3::new(1400.0, 190.0, 188.0),
            Vec3::new(1300.0, 190.0, 188.0),
        );
        let mut g = NavGraph::from_parts(vec![foot, top, on], vec![vec![1], vec![2], vec![]]);
        g.edge_mut(0, 1).unwrap().mantle = true;
        let mut f = Follower {
            path: vec![0, 1, 2],
            next: 1,
            ..Follower::default()
        };
        let at = foot + Vec3::new(-10.0, 0.0, 0.0);
        let still = |o: Vec3| [(o, Vec3::ZERO)];
        let mate = Vec3::new(1403.0, 196.0, 188.0);
        assert!(f.mantle_held(&g, at, true, &still(mate)));
        assert!(!f.mantle_held(&g, at, false, &still(mate)));
        assert!(!f.mantle_held(&g, at, true, &[(mate, Vec3::new(150.0, 0.0, 0.0))]));
        assert!(!f.mantle_held(&g, at, true, &still(mate + Vec3::new(0.0, 40.0, 0.0))));
        // The landing the probe proved, off the node, is the one read.
        g.edge_mut(0, 1).unwrap().land = Some(Vec3::new(1380.0, 150.0, 188.0));
        assert!(!f.mantle_held(&g, at, true, &still(mate)));
        g.edge_mut(0, 1).unwrap().land = None;
        f.next = 2;
        assert!(!f.mantle_held(&g, at, true, &still(mate)));
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
        let held = [(Vec3::new(-4.0, 2.0, 1054.0), Vec3::ZERO)];
        assert_eq!(
            f.head_on_yield(&g, at, rungs, &held),
            Some(head),
            "a climber holding still for it"
        );
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

    /// Climbing toward a head with a body over it on the rungs that is not
    /// rising, a climber holds still: one that kept climbing under a bot
    /// that bobbed under mp_ship's crow's nest plate was stood on and
    /// pushed off the rungs. Not for one rising, one far above or aside,
    /// one below, and not past [`RISING_TICKS`].
    #[test]
    fn a_climber_holds_under_a_body_over_it_that_is_not_rising() {
        let foot = Vec3::new(-4.0, 0.0, 692.0);
        let head = Vec3::new(0.0, 0.0, 1219.0);
        let g = NavGraph::from_parts(vec![foot, head], vec![vec![1], vec![]]);
        let mut f = Follower {
            path: vec![0, 1],
            next: 1,
            ..Default::default()
        };
        let rungs = Footing {
            still: false,
            on_ground: false,
            on_ladder: true,
            velocity: Vec3::new(0.0, 0.0, 95.0),
        };
        let at = Vec3::new(-4.0, 0.0, 1105.0);
        let over = |p: Vec3, vz: f32| [(p, Vec3::new(0.0, 0.0, vz))];
        let above = Vec3::new(-4.0, -3.0, 1197.0);
        assert!(
            f.under_hold(&g, at, rungs, &over(above, -150.0)),
            "falling back"
        );
        assert!(f.under_hold(&g, at, rungs, &over(above, 0.0)), "still");
        assert!(!f.under_hold(&g, at, rungs, &over(above, 95.0)), "rising");
        let far = Vec3::new(-4.0, 0.0, 1105.0 + HEAD_ON + 1.0);
        assert!(!f.under_hold(&g, at, rungs, &over(far, 0.0)), "far above");
        let aside = Vec3::new(-4.0, 40.0, 1197.0);
        assert!(!f.under_hold(&g, at, rungs, &over(aside, 0.0)), "aside");
        let below = Vec3::new(-4.0, 0.0, 1040.0);
        assert!(!f.under_hold(&g, at, rungs, &over(below, 0.0)), "below");
        let ground = Footing {
            on_ladder: false,
            on_ground: true,
            ..rungs
        };
        assert!(
            !f.under_hold(&g, at, ground, &over(above, 0.0)),
            "off the rungs"
        );
        let held = (0..2 * RISING_TICKS)
            .filter(|_| f.under_hold(&g, at, rungs, &over(above, 0.0)))
            .count();
        assert_eq!(held as u32, RISING_TICKS, "at most RISING_TICKS on end");
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
        assert!(g.linked(head, foot));
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
        assert!(g.linked(head, foot));
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
}
