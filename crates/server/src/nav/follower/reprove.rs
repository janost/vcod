//! Whether the edge under way still holds from where the bot stands.

use super::*;

impl Follower {
    /// Whether the leap from the current waypoint, a leap's foot, arrives run
    /// from `at`, where the bot came to rest: the graph proved it from the
    /// node, and a body a few units nearer the lip leaves the ground before
    /// it has the pace to clear the gap. Without a world, taken as proved.
    pub(super) fn leap_holds(
        &self,
        world: Option<&CollisionWorld>,
        at: Vec3,
        g: &NavGraph,
    ) -> bool {
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
    /// or from up a slope under it, the walk has to arrive. Off a jump edge
    /// the walk has to arrive without one: the bot jumps only on the
    /// graph's jump edges (bot-navigation.md section 3).
    pub(super) fn edge_holds(
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
        let jump = g.has(a, b, |k| k.jump);
        let edge = (a, b);
        self.walked_jump = None;
        // No CoD1 walk climbs a mantle; the graph's probe proved it.
        if g.has(edge.0, edge.1, |k| k.mantle) {
            return true;
        }
        // A crouched jump's walk is the gait that proved it: no other gets
        // through the window.
        let gait = if g.has(edge.0, edge.1, |k| k.crouched) {
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
    pub(super) fn flings(
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
            && !g.has(a, b, |k| k.jump);
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
    pub(super) fn backs_short(
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
            && g.has(a, b, |k| k.mantle)
            && to.z < from.z - vcod_common::pmove::JUMP_HEIGHT
        {
            return body.on_ground && self.held != Some((a, b));
        }
        to.z < from.z - vcod_common::pmove::JUMP_HEIGHT
            && body.on_ground
            && !g.has(a, b, |k| k.jump)
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
    pub(super) fn deadly_fall(&self) -> f32 {
        self.fall_kills.unwrap_or(DEADLY_LADDER)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::nav::build::tests::map_world;

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
        g.edge_mut(0, 1).unwrap().jump = true;
        assert!(
            f.edge_holds(Some(w), aside, Vec3::ZERO, false, &g),
            "on a jump edge"
        );
    }
}
