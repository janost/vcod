//! The bots' per-tick driver: what the brains ([`super::Bot`]) are fed and
//! what their cmds become, over a game's [`BotWorld`] (docs/bots.md).

use std::collections::BTreeMap;
use std::sync::Arc;

use glam::Vec3;
use vcod_common::net::msg::UserCmd;

use crate::nav::{Creases, Follower, NavGraph, NavJob};
use crate::world::World;

use super::{Bot, BotView, EnemyView, Goal, Noise, SeatView};

/// A* nodes a tick may expand across all bots; a plan left unfinished
/// resumes next tick (bot-navigation.md, section 3).
const PLAN_BUDGET: u32 = 4000;
/// How often a bot re-runs its enemy search (range gate + LOS traces).
/// The cached verdict is at most this stale.
const ENEMY_REFRESH_MS: i32 = 100;
/// How often a fighting bot re-checks its cover spot, or looks for one.
const COVER_REFRESH_MS: i32 = 250;

/// What the driver keeps per bot and per level, inside the game that owns
/// the bots.
#[derive(Default)]
pub(crate) struct BotDriver {
    /// The brains, by slot. An entry whose client is gone goes with it.
    pub(crate) bots: BTreeMap<usize, Bot>,
    /// Per bot, `(sv_time of the last look, the enemy that look found)`.
    /// A fresh LOS trace per bot per tick was the other half of the 24-bot
    /// CPU load; a bot does not need a new verdict every 50 ms.
    pub(crate) enemies: BTreeMap<usize, (i32, Option<EnemyView>)>,
    /// Per bot, the sidestep it is walking to and the sv_time it was picked
    /// ([`super::BotView::sidestep`]), held until reached, no longer clear
    /// or too old (`bots::held_sidestep`): picked afresh every tick, the
    /// nearest spot moved with the body and a moving teammate's line, and
    /// the bot jittered between spots.
    pub(crate) sidesteps: BTreeMap<usize, ([f32; 3], i32)>,
    /// Per fighting bot, `(sv_time of the last look, the cover spot it
    /// found)` ([`super::BotView::cover`]).
    pub(crate) covers: BTreeMap<usize, (i32, Option<super::cover::Spot>)>,
    /// Per hunting bot, `(the lost enemy's spot, its vantage)`
    /// ([`super::BotView::hunt`]).
    pub(crate) hunts: BTreeMap<usize, ([f32; 3], Option<[f32; 3]>)>,
    /// The level's navigation graph, built on the first tick with bots.
    pub(crate) nav: Option<Arc<NavGraph>>,
    /// The build of `nav` under way, and whether the tick waits for it or
    /// ticks on with the bots off the graph until it lands.
    pub(crate) nav_job: Option<NavJob>,
    pub(crate) nav_background: bool,
    /// The drop probe's crease verdicts on the loaded world.
    pub(crate) creases: std::cell::RefCell<Creases>,
    /// The drop and wall probes by the feet they were felt from, on a
    /// game whose world holds still ([`BotWorld::STILL_WORLD`]); kept as
    /// long as [`Self::creases`].
    pub(crate) probes: std::cell::RefCell<Probes>,
    /// Per bot, its walk along `nav`.
    pub(crate) paths: BTreeMap<usize, Follower>,
    /// Each zone's `(mins, maxs)` and the stand picked in it, for the life
    /// of the nav graph.
    pub(crate) sites: Vec<([f32; 3], [f32; 3], [f32; 3])>,
    /// Each headquarters radio's point and the stand picked for it, as
    /// [`Self::sites`].
    pub(crate) radios: Vec<([f32; 3], [f32; 3])>,
    /// The shots and blasts since the bots last listened, in sim order;
    /// the tick takes them at its top.
    pub(crate) noises: Vec<Noise>,
    /// Who may yell now, and who was alive last tick (`super::yell`).
    pub(crate) yells: super::yell::Gate,
    /// Edges the followers gave up on ([`Follower::gave_up`]), and those
    /// of them with another body within [`STALL_NEAR`]: the measure of
    /// stalls on a body (`bot_match`, `ship_stalls`).
    pub(crate) edge_stalls: [u32; 2],
    /// The guards' posts round their objectives ([`super::posts`]).
    pub(crate) posts: super::posts::Posts,
}

impl BotDriver {
    /// [`NavGraph::zone_stand`], cached per zone in [`Self::sites`]; the
    /// middle of the zone's floor until the graph lands.
    pub(crate) fn zone_stand(&mut self, mins: [f32; 3], maxs: [f32; 3]) -> [f32; 3] {
        if let Some(&(_, _, p)) = self
            .sites
            .iter()
            .find(|(lo, hi, _)| *lo == mins && *hi == maxs)
        {
            return p;
        }
        let mid = [
            (mins[0] + maxs[0]) * 0.5,
            (mins[1] + maxs[1]) * 0.5,
            mins[2],
        ];
        let Some(g) = self.nav.as_ref() else {
            return mid;
        };
        let p = g.zone_stand(mins, maxs).unwrap_or(mid);
        self.sites.push((mins, maxs, p));
        p
    }

    /// [`NavGraph::radio_stand`], cached per radio in [`Self::radios`];
    /// the radio itself until the graph lands.
    pub(crate) fn radio_stand(&mut self, at: [f32; 3], reach: f32, height: f32) -> [f32; 3] {
        if let Some(&(_, p)) = self.radios.iter().find(|(r, _)| *r == at) {
            return p;
        }
        let Some(g) = self.nav.as_ref() else {
            return at;
        };
        let p = g.radio_stand(at, reach, height).unwrap_or(at);
        self.radios.push((at, p));
        p
    }
}

/// A playing or idle body as the driver's team passes read it.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Body {
    pub(crate) origin: Vec3,
    pub(crate) velocity: Vec3,
    /// In the world and alive (`PM_NORMAL`, not dead).
    pub(crate) playing: bool,
    pub(crate) on_ground: bool,
    pub(crate) on_ladder: bool,
}

/// What [`cover::find`] needs of the bot and its threat that only the game
/// knows: the bot's feet, the threat's eye, the standing lean's fraction
/// (1.1's full lean, 1.5's and CoD2's half) and the lean sphere's clip mask.
#[derive(Clone, Copy, Debug)]
pub(crate) struct CoverBody {
    pub(crate) origin: Vec3,
    pub(crate) threat_eye: Vec3,
    pub(crate) lean: f32,
    pub(crate) mask: u32,
}

/// Where a bot stands in the join after its server commands were read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Joining {
    /// Nothing to do this tick past its brain.
    No,
    /// Connected: its gamestate goes out after the replies.
    Joined,
    /// Primed with no body yet: it sends the entering cmd.
    Entering,
}

/// What a game lends the driver for one tick. Each method is a read or a
/// write the driver makes at one point of the tick, and the driver keeps
/// that order (AGENTS.md, "Bots"): a call moved across another changes
/// what the bots draw and see.
pub(crate) trait BotWorld {
    /// The game's objective state, read once a tick before the views and
    /// dealt over them after ([`Self::frame`], [`Self::deal`]).
    type Frame;
    /// A bot's cmds for the tick as the game sends them ([`Self::cmd`]).
    type Cmd;

    /// What a game's route leaves out. Each is a behaviour change to
    /// turn on (the `bot_match` digests pin it).
    ///
    /// The route yields to teammates on and off the ladders, pays to share
    /// a teammate's path (`team::crowd`) and keeps off
    /// [`Self::blocked_nodes`].
    const YIELDS: bool = true;
    /// With [`Self::YIELDS`] off, the route still pays to share a
    /// teammate's path and plans with the bot's own noise and flank on an
    /// objective level, for a bot with `Bot::tactics` on
    /// (bot-objectives.md 13). Neither yields to anyone.
    const SPREADS: bool = false;
    /// The route plans round any other body standing in a bot's way
    /// once it is held up by it ([`crate::nav::Follower::bodies_in_way`]).
    const SHUNS_BODIES: bool = true;
    /// The route plans round a teammate standing still in the way of a
    /// bot held up beside him ([`mates_in_way`]).
    const PARTS_MATES: bool = false;
    /// With [`Self::PARTS_MATES`], a bot held off the ground beside a
    /// still teammate (hung on his capsule at a step) is held up by him
    /// as one standing is.
    const PARTS_MATES_ALOFT: bool = false;
    /// With [`Self::PARTS_MATES`], a still teammate counts by his distance
    /// across, within [`MATE_RISE`] up or down, not in three dimensions: a
    /// guard on a stair's top holds up the bot on the treads under him.
    const PARTS_MATES_FLAT: bool = false;
    /// With [`Self::PARTS_MATES`], the way round a still teammate keeps
    /// off every node where a body's box would meet his, not only the
    /// node nearest him: two guards a node apart left a gap on the grid
    /// no box fits through.
    const PARTS_MATES_BOX: bool = false;
    /// The route's edge cues that step CoD1's pmove: the lip, the push off
    /// the rungs, the unpin ([`Self::weapon_defs`]).
    const PMOVE_PROBES: bool = true;
    /// A mantle edge is followed to its landing however far across it
    /// lies ([`crate::nav::Follower::long_mantles`]).
    const LONG_MANTLES: bool = false;
    /// The follower reads a waypoint as off the path only a reach past
    /// the build's ([`crate::nav::Follower::foot_slack`]).
    const FOOT_SLACK: bool = false;
    /// On a mantle edge the bot runs at the landing its probe proved, not
    /// at the node the edge ends at ([`crate::nav::Follower::mantle_land`]).
    const MANTLE_AIMS: bool = false;
    /// A mantle edge down is taken from rest at its foot, facing the wall
    /// ([`crate::nav::Follower::mantle_rests`]), even where CoD1's walks
    /// proved the graph.
    const MANTLE_RESTS: bool = false;
    /// A bot that walked out its plan to an objective's point and comes no
    /// nearer it gives the point up for a while ([`Bot::arrived_goal`]).
    const GIVES_UP_GOALS: bool = false;
    /// What a jump edge adds to a plan's length
    /// ([`crate::nav::Follower::jump_cost`]).
    const JUMP_COST: f32 = 0.0;
    /// The bots' world holds still for the level ([`Self::world`] never
    /// moves a brush), so a drop or wall probe from a spot reads the same
    /// on every tick and [`BotDriver::probes`] keeps it.
    const STILL_WORLD: bool = false;

    fn driver(&self) -> &BotDriver;
    fn driver_mut(&mut self) -> &mut BotDriver;
    /// `level.time` of the frame the bots move in.
    fn now(&self) -> i32;
    /// What an `mr` names, and the rounds' rotation seed (`team::round_rank`).
    fn server_id(&self) -> i32;
    fn world(&self) -> Option<&World>;
    /// Whether a client still holds `slot`.
    fn connected(&self, slot: usize) -> bool;

    /// The shots and blasts since the last tick, in sim order.
    fn take_noises(&mut self) -> Vec<Noise>;
    /// The bot admin's pass (`crate::server::bot_admin::manage`).
    fn manage(&mut self);
    /// The tick found no bot after [`Self::manage`].
    fn idle(&mut self) {}
    /// The graph's build, poll and rebuild, and whatever the game blocks
    /// on it, before the bots read it.
    fn nav_tick(&mut self);
    /// Every slot's script team.
    fn teams(&mut self) -> Vec<i32>;

    /// Pass 1 for one bot: the reliable commands it was sent since the
    /// last tick, answered as a menu-opening client would, into `replies`.
    fn read_commands(
        &mut self,
        slot: usize,
        sid: i32,
        replies: &mut Vec<(usize, String)>,
    ) -> Joining;

    fn frame(&mut self) -> Self::Frame;
    /// The hit the bot in `slot` felt, read every tick ahead of its enemy
    /// lookup, cached or not; [`Self::enemy`] and [`Self::view`] read it.
    fn feel(&mut self, _slot: usize) {}
    /// The health packs lying about, for the retreat's heal.
    fn health_packs(&mut self) -> Vec<[f32; 3]>;
    fn body(&self, slot: usize) -> Option<Body>;
    /// The enemy the bot in `slot` fights, looked for afresh.
    fn enemy(&self, slot: usize, teams: &[i32]) -> Option<EnemyView>;
    /// The body in `slot` as the brain reads it; `None` with no body.
    fn view(&self, slot: usize, enemy: Option<EnemyView>) -> Option<BotView>;
    /// [`BotView::friend_in_line`] and [`BotView::sidestep`] against
    /// `target`, `held` the sidestep under way.
    fn friend_in_line(
        &self,
        slot: usize,
        target: [f32; 3],
        teams: &[i32],
        held: Option<([f32; 3], i32)>,
    ) -> (bool, Option<[f32; 3]>);
    fn cover_body(&self, slot: usize, threat: usize) -> Option<CoverBody>;
    /// Pass 2's objective roles over every view.
    fn deal(
        &mut self,
        frame: Self::Frame,
        teams: &[i32],
        solo: &[usize],
        round: u32,
        views: &mut Vec<(usize, BotView)>,
    );

    /// Pass 3 for one bot ahead of its goal and route: the game's own
    /// throws (CoD2's smoke).
    fn pre_route(&mut self, _slot: usize, _view: &mut BotView, _teams: &[i32]) {}
    /// The route just walked `follower` along `g`.
    fn routed(&mut self, _slot: usize, _follower: &Follower, _g: &NavGraph) {}
    /// Pass 3 for one bot after its route, ahead of the drop probe: the
    /// game's own walks over the waypoint (CoD2's guns and dropped guns).
    fn pre_think(&mut self, _slot: usize, _view: &mut BotView, _teams: &[i32]) {}
    /// Whether the brain's respawn (`Bot::take_kill`) is held off.
    fn keeps_alive(&mut self, _slot: usize) -> bool {
        false
    }
    /// The brain's `cmd` for the bot in `slot` as the game sends it, right
    /// after `think`; `None` sends nothing.
    fn cmd(&mut self, slot: usize, cmd: UserCmd, view: &BotView) -> Option<Self::Cmd>;

    /// The bot's body as the follower's edge tests read it (CoD1's
    /// playerstate).
    fn move_ps(&self, slot: usize) -> Option<vcod_common::pmove::PlayerState>;
    /// The fall that kills the body in `slot` now
    /// ([`crate::nav::fall_that_kills`]).
    fn fall_kills(&self, slot: usize) -> Option<f32>;
    /// The graph nodes the followers keep off.
    fn blocked_nodes(&self) -> Arc<[u32]>;
    /// The weapon table the edge probes step pmove with.
    fn weapon_defs(&self) -> &[Option<vcod_common::weapon::WeaponDef>];
    /// The driver's seat's waypoint toward `goal`.
    fn drive_waypoint(&mut self, slot: usize, goal: Goal, seat: &SeatView) -> Option<[f32; 3]>;
    /// The bot in `slot` is out of a driver's seat.
    fn forget_drive(&mut self, slot: usize);
    /// [`BotView::lob`]: the throw of grenade `weapon` that bursts nearest
    /// `target`.
    fn lob_probe(
        &self,
        slot: usize,
        weapon: u8,
        target: [f32; 3],
        teams: &[i32],
    ) -> Option<[f32; 2]>;

    /// Client commands queued for the bots outside the tick.
    fn take_orders(&mut self) -> Vec<(usize, String)>;
    /// One client command from the bot in `slot`.
    fn bot_command(&mut self, slot: usize, text: String);
    /// The index of the script menu `name` among the menu configstrings,
    /// what an `mr` names it by; `None` sends the bots no yells.
    fn menu_index(&self, _name: &str) -> Option<usize> {
        None
    }
    /// The client whose hit on the body in `slot` was the last, within the
    /// last second: who a death just now is credited to.
    fn killer(&self, _slot: usize) -> Option<usize> {
        None
    }
    /// The joined bot's gamestate.
    fn begin(&mut self, slot: usize);
    /// The bots' cmds are one packet: stamped once before them.
    fn bot_packet(&mut self);
    /// The bot's cmd for the tick, `climbing` on the rungs.
    fn send_cmd(&mut self, slot: usize, cmd: Self::Cmd, climbing: bool);
    /// The entering cmd of a primed bot.
    fn enter(&mut self, slot: usize);
    /// The bot read every reliable command sent.
    fn ack(&mut self, slot: usize);
    /// The tick's last step, after the acks.
    fn tick_done(&mut self) {}
}

/// A level whose bots share out a team objective: `sd`, `re` and UO's
/// `ctf`, `dom`, `hq` and `bas`.
pub(crate) fn team_objective(v: &BotView) -> bool {
    v.sd.is_some() || v.re.is_some() || v.uo.is_some()
}

/// [`BotView::squad`] over each side's S&D attackers with team play and
/// `Bot::squad` on, dead ones counted, so a death keeps the rest in their
/// parts: [`super::team::squad`] by their [`super::team::round_rank`].
fn deal_squads<W: BotWorld>(w: &W, views: &mut [(usize, BotView)], teams: &[i32], round: u32) {
    use crate::game::script::{TEAM_ALLIES, TEAM_AXIS};
    let attacks = |slot: usize, v: &BotView| {
        w.driver()
            .bots
            .get(&slot)
            .is_some_and(|b| b.team_play && b.squad)
            && v.sd
                .as_ref()
                .is_some_and(|sd| sd.role == super::ObjRole::Attack)
    };
    for team in [TEAM_AXIS, TEAM_ALLIES] {
        let side: Vec<usize> = views
            .iter()
            .filter(|(s, v)| teams.get(*s) == Some(&team) && attacks(*s, v))
            .map(|(s, _)| *s)
            .collect();
        for (s, v) in views.iter_mut().filter(|(s, _)| side.contains(s)) {
            let rank = super::team::round_rank(*s, &side, team, round);
            v.squad = super::team::squad(rank, side.len());
        }
    }
}

/// An S&D site with an enemy seen within this of its stand is under
/// attack, and stays so this long past the last such sighting, ms
/// ([`super::SdView::hot`]).
const HOT_NEAR: f32 = 1000.0;
pub(crate) const HOT_MS: i32 = 10_000;

/// [`super::SdView::hot`] over each side's S&D defenders before a plant,
/// from the enemies its playing bots see this tick: the site nearest a
/// sighting within [`HOT_NEAR`], held [`HOT_MS`] past the last one.
fn deal_hot<W: BotWorld>(w: &mut W, views: &mut [(usize, BotView)], teams: &[i32]) {
    use crate::game::script::{TEAM_ALLIES, TEAM_AXIS};
    let (now, round) = (w.now(), w.server_id() as u32);
    let posts = &mut w.driver_mut().posts;
    if posts.round != round {
        posts.round = round;
        posts.hot = [None; 2];
    }
    for (i, team) in [TEAM_AXIS, TEAM_ALLIES].into_iter().enumerate() {
        let mine = |s: usize| teams.get(s) == Some(&team);
        let sites: Vec<[f32; 3]> = views
            .iter()
            .filter(|(s, _)| mine(*s))
            .find_map(|(_, v)| {
                v.sd.as_ref()
                    .filter(|sd| sd.role == super::ObjRole::Defend && sd.bomb.is_none())
                    .map(|sd| sd.sites.iter().map(|x| x.stand).collect())
            })
            .unwrap_or_default();
        if sites.len() < 2 {
            w.driver_mut().posts.hot[i] = None;
            continue;
        }
        let near = views
            .iter()
            .filter(|(s, v)| mine(*s) && v.playing && !v.dead)
            .filter_map(|(_, v)| v.enemy.map(|e| e.origin))
            .flat_map(|e| {
                sites
                    .iter()
                    .enumerate()
                    .map(move |(k, st)| (k, super::dist_sq(e, *st)))
            })
            .filter(|(_, d)| *d < HOT_NEAR * HOT_NEAR)
            .min_by(|a, b| a.1.total_cmp(&b.1).then(a.0.cmp(&b.0)));
        let hot = &mut w.driver_mut().posts.hot[i];
        match near {
            Some((k, _)) => *hot = Some((k, now.wrapping_add(HOT_MS))),
            None if hot.is_some_and(|(_, until)| now.wrapping_sub(until) >= 0) => *hot = None,
            None => {}
        }
        let h = hot.map(|(k, _)| k);
        for (s, v) in views.iter_mut().filter(|(s, _)| mine(*s)) {
            let on = w
                .driver()
                .bots
                .get(s)
                .is_some_and(|b| b.converge && b.team_play);
            if let Some(sd) =
                v.sd.as_mut()
                    .filter(|sd| on && sd.role == super::ObjRole::Defend)
            {
                sd.hot = h;
            }
        }
    }
}

/// Every [`SHELTER_EVERY`]th tick a bot near its objective samples how
/// many of the ways in see it ([`super::BotView::exposure`]).
const SHELTER_EVERY: i32 = 5;
/// Near its objective: within this of the point it centres on.
pub(crate) const OBJ_NEAR: f32 = 800.0;

/// [`super::BotView::guard_post`] over each side's guards of one
/// objective ([`super::posts::assign`]), with team play and
/// `Bot::tactics` on, and [`super::BotView::exposure`] for every bot
/// near its objective. The surveys are made here, once per objective
/// and graph, before any bot thinks.
fn deal_posts<W: BotWorld>(w: &mut W, views: &mut [(usize, BotView)], teams: &[i32]) {
    use super::posts;
    let Some(g) = w.driver().nav.clone() else {
        return;
    };
    let (now, round) = (w.now(), w.server_id() as u32);
    // Each living bot on foot with an objective that holds still: the
    // anchor its posts are dealt round (a guard), else one its exposure is
    // counted against, and whether it plays by posts.
    let wants: Vec<(usize, i32, posts::Anchor, bool, bool)> = views
        .iter()
        .filter(|(_, v)| v.playing && !v.dead && v.seat.is_none())
        .filter_map(|(s, v)| {
            let b = w.driver().bots.get(s)?;
            let (a, guard) = match b.guard_anchor(v) {
                Some(a) => (a, true),
                None => (b.counted_anchor(v)?, false),
            };
            let posted = guard && b.tactics && b.team_play;
            Some((*s, *teams.get(*s)?, a, posted, b.lane_target(v)))
        })
        .collect();
    let mut kept = std::mem::take(&mut w.driver_mut().posts);
    let Some(world) = w.world() else {
        w.driver_mut().posts = kept;
        return;
    };
    let clear = |a: Vec3, b: Vec3| world.collision.shot_trace(a, b).fraction >= 1.0;
    type Groups = Vec<((i32, posts::Key), Vec<(usize, Vec3)>)>;
    let join = |groups: &mut Groups, k: (i32, posts::Key), m: (usize, Vec3)| match groups
        .iter_mut()
        .find(|(g, _)| *g == k)
    {
        Some((_, g)) => g.push(m),
        None => groups.push((k, vec![m])),
    };
    let (mut groups, mut laned): (Groups, Groups) = Default::default();
    let mut surveys: Vec<(posts::Key, std::sync::Arc<posts::Survey>, posts::Anchor)> = Vec::new();
    for &(slot, team, a, posted, lane) in &wants {
        let key = a.key();
        let survey = kept.survey(&g, &clear, &a);
        let Some(i) = views.iter().position(|(s, _)| *s == slot) else {
            continue;
        };
        let v = &mut views[i].1;
        if super::dist_sq(v.origin, a.centre()) < OBJ_NEAR * OBJ_NEAR {
            if (now / crate::server::FRAME_MS + slot as i32) % SHELTER_EVERY == 0 {
                match survey.exposure(&clear, Vec3::from(v.origin), v.eye_height) {
                    Some(e) => kept.exposure.insert(slot, e),
                    None => kept.exposure.remove(&slot),
                };
            }
            v.exposure = kept.exposure.get(&slot).copied();
        }
        if posted {
            join(&mut groups, (team, key), (slot, v.origin.into()));
        }
        if lane {
            join(&mut laned, (team, key), (slot, v.origin.into()));
        }
        if (posted || lane) && !surveys.iter().any(|(k, _, _)| *k == key) {
            surveys.push((key, survey, a));
        }
    }
    let member =
        |groups: &Groups, s: &usize| groups.iter().any(|(_, g)| g.iter().any(|(m, _)| m == s));
    kept.held.retain(|s, _| member(&groups, s));
    kept.lanes.retain(|s, _| member(&laned, s));
    for ((_, key), bots) in &laned {
        let Some((_, survey, a)) = surveys.iter().find(|(k, _, _)| k == key) else {
            continue;
        };
        let centre = Vec3::from(a.centre());
        for (slot, via) in posts::lanes(survey, *key, centre, bots, &mut kept.lanes, now) {
            if let Some((_, v)) = views.iter_mut().find(|(s, _)| *s == slot) {
                v.lane = Some(via.into());
            }
        }
    }
    for ((_, key), guards) in &groups {
        let Some((_, survey, a)) = surveys.iter().find(|(k, _, _)| k == key) else {
            continue;
        };
        for (slot, p) in posts::assign(survey, *key, guards, &mut kept.held, now, round) {
            if let Some((_, v)) = views.iter_mut().find(|(s, _)| *s == slot) {
                v.guard_post = Some(super::GuardPost {
                    at: p.at.into(),
                    crouch: p.crouch,
                    look: a.centre(),
                });
            }
        }
    }
    w.driver_mut().posts = kept;
}

/// How near its probe's landing a body on a mantle edge has climbed
/// ([`BotWorld::MANTLE_AIMS`]): a body in front of the wall stands at least
/// its radius and the wall's depth short of it.
const MANTLE_LANDED: f32 = 24.0;

/// The bots' pass of the tick: join like a client, then move like one.
/// Runs before the frame's client moves, so a bot's cmd for this frame is
/// queued when the sim reads it. Four passes, because the brains need the
/// whole world to think and their writes land after.
pub(crate) fn step<W: BotWorld>(w: &mut W) {
    let noises = w.take_noises();
    let p = crate::tick_profile::phase("bots/manage");
    w.manage();
    drop(p);
    if w.driver().bots.is_empty() {
        w.idle();
        return;
    }
    {
        let _p = crate::tick_profile::phase("bots/nav tick");
        w.nav_tick();
    }
    let head = crate::tick_profile::phase("bots/commands and inputs");
    let sid = w.server_id();
    // The team table once per frame; the game's team lookup may need its
    // mutable face, so the enemy pass reads this instead.
    let teams = w.teams();

    // Pass 1: the reliable commands each bot just received, answered the
    // way a menu-opening client would execute them client-side.
    let mut replies: Vec<(usize, String)> = Vec::new();
    let mut joined: Vec<usize> = Vec::new();
    let mut entering: Vec<usize> = Vec::new();
    let slots: Vec<usize> = w.driver().bots.keys().copied().collect();
    for &slot in &slots {
        match w.read_commands(slot, sid, &mut replies) {
            Joining::Joined => joined.push(slot),
            Joining::Entering => entering.push(slot),
            Joining::No => {}
        }
    }

    // Pass 2: what each bot's body sees, for the brains. The enemy
    // lookup refreshes at ~10 Hz per bot and is cached in between.
    let frame = w.frame();
    let packs = w.health_packs();
    // Every playing client on a side, by slot, team and feet: who stands
    // nearer a noise than a bot (`team::answers`).
    let on_side: Vec<(usize, i32, [f32; 3])> = teams
        .iter()
        .enumerate()
        .filter_map(|(slot, &team)| {
            let body = w.body(slot)?;
            (body.playing
                && (team == crate::game::script::TEAM_AXIS
                    || team == crate::game::script::TEAM_ALLIES))
                .then(|| (slot, team, body.origin.into()))
        })
        .collect();
    drop(head);
    let mut views: Vec<(usize, BotView)> = Vec::new();
    let sensing = crate::tick_profile::phase("bots/sense");
    for &slot in &slots {
        if let Some(view) = sense(w, slot, &teams, &noises, &on_side, &packs) {
            views.push((slot, view));
        }
    }
    drop(sensing);
    let round = sid as u32;
    // Bots with team play off (`Bot::team_play`) rank by slot, keep by
    // a carrier, cover nobody and plan alone, as before `bots::team`.
    let solo: Vec<usize> = slots
        .iter()
        .copied()
        .filter(|s| w.driver().bots.get(s).is_some_and(|b| !b.team_play))
        .collect();
    let p = crate::tick_profile::phase("bots/deal");
    w.deal(frame, &teams, &solo, round, &mut views);
    deal_squads(w, &mut views, &teams, round);
    deal_hot(w, &mut views, &teams);
    deal_posts(w, &mut views, &teams);
    drop(p);

    // Pass 3: the tick's waypoint toward each brain's goal, then its
    // cmd. A* is capped per tick across all bots; one that misses out
    // wanders for a tick and asks again.
    let mut moves: Vec<(usize, W::Cmd, bool)> = Vec::new();
    let mut yells: Vec<(usize, super::yell::Yell)> = Vec::new();
    let mut budget = PLAN_BUDGET;
    let p = crate::tick_profile::phase("bots/bodies and paths");
    // Every player on a ladder, and off one, for the bots that wait for
    // one.
    let bodies = |on_ladder: bool| -> Vec<(usize, Vec3, Vec3)> {
        (0..teams.len())
            .filter_map(|slot| {
                let b = w.body(slot)?;
                (b.playing && b.on_ladder == on_ladder).then_some((slot, b.origin, b.velocity))
            })
            .collect()
    };
    let (climbers, standing) = (bodies(true), bodies(false));
    // On an objective level each playing bot's path ahead, which its
    // teammates' plans pay more to share (`team::crowd`).
    let paths: Vec<super::team::TeamPath> = views
        .iter()
        .filter(|(_, v)| v.playing && !v.dead && team_objective(v))
        .filter_map(|(slot, _)| {
            let f = w.driver().paths.get(slot)?;
            Some((*slot, teams.get(*slot).copied()?, f.ahead().collect()))
        })
        .collect();
    // Whose paths an S&D flanker plans round.
    let main: Vec<usize> = views
        .iter()
        .filter(|(_, v)| v.squad == super::team::Squad::Main)
        .map(|(s, _)| *s)
        .collect();
    drop(p);
    for (slot, mut view) in views {
        let looking = crate::tick_profile::phase("bots/cover and vantage");
        if let Some((threat, held)) = w.driver().bots.get(&slot).and_then(|b| b.cover_query())
            && let (Some(g), Some(world)) = (w.driver().nav.as_ref(), w.world())
        {
            view.survive.cover =
                super::retreat::cover_spot(g, &world.collision, view.origin, threat, held);
        }
        view.hunt = vantage(w, slot, view.origin);
        drop(looking);
        let p = crate::tick_profile::phase("bots/goal");
        if W::PARTS_MATES {
            let _p = crate::tick_profile::phase("goal/give way");
            view.give_way = give_way(w, slot, &teams);
        }
        let pre = crate::tick_profile::phase("goal/pre route");
        w.pre_route(slot, &mut view, &teams);
        drop(pre);
        let brain = crate::tick_profile::phase("goal/brain");
        let Some(goal) = w.driver().bots.get(&slot).map(|b| b.goal(&view)) else {
            continue;
        };
        drop(brain);
        // A seat leaves the walk's follower alone; a driver steers along
        // a route its hull fits (`drive_waypoint`).
        if let Some(s) = view.seat.filter(|s| s.seat == 1) {
            let _p = crate::tick_profile::phase("goal/drive");
            view.waypoint = w.drive_waypoint(slot, goal, &s);
        } else {
            w.forget_drive(slot);
        }
        drop(p);
        if let Some(g) = w.driver().nav.clone().filter(|_| view.seat.is_none()) {
            let _p = crate::tick_profile::phase("bots/route");
            route(
                w,
                slot,
                &mut view,
                goal,
                &g,
                &teams,
                &solo,
                &paths,
                &main,
                &climbers,
                &standing,
                &mut budget,
            );
        }
        let probing = crate::tick_profile::phase("bots/probes");
        let p = crate::tick_profile::phase("probes/pre-think");
        w.pre_think(slot, &mut view, &teams);
        drop(p);
        let p = crate::tick_profile::phase("probes/drops and walls");
        if w.driver()
            .bots
            .get(&slot)
            .is_some_and(|b| b.reads_drops(&view))
        {
            view.drop_ahead = drop_probe(w, slot);
        }
        if w.driver()
            .bots
            .get(&slot)
            .is_some_and(|b| b.reads_walls(&view))
        {
            view.wall_ahead = wall_probe(w, slot);
        }
        drop(p);
        let p = crate::tick_profile::phase("probes/lob");
        lob(w, slot, &mut view, &teams);
        drop(p);
        drop(probing);
        let _p = crate::tick_profile::phase("bots/think");
        let Some((cmd, kill, yell)) = w
            .driver_mut()
            .bots
            .get_mut(&slot)
            .map(|bot| (bot.think(&view), bot.take_kill(), bot.take_yell()))
        else {
            continue;
        };
        if kill && !w.keeps_alive(slot) {
            replies.push((slot, "kill".into()));
        }
        yells.extend(yell.map(|y| (slot, y)));
        if let Some(cmd) = w.cmd(slot, cmd, &view) {
            moves.push((slot, cmd, view.on_ladder && !view.on_ground));
        }
    }

    // Pass 4: everything lands.
    let p = crate::tick_profile::phase("bots/land");
    yell(w, sid, &teams, yells, &mut replies);
    let gone: Vec<usize> = w
        .driver()
        .bots
        .keys()
        .chain(w.driver().enemies.keys())
        .chain(w.driver().sidesteps.keys())
        .chain(w.driver().paths.keys())
        .copied()
        .filter(|s| !w.connected(*s))
        .collect();
    let d = w.driver_mut();
    for slot in gone {
        d.bots.remove(&slot);
        d.enemies.remove(&slot);
        d.sidesteps.remove(&slot);
        d.paths.remove(&slot);
    }
    replies.append(&mut w.take_orders());
    for (slot, r) in replies {
        if w.driver().bots.contains_key(&slot) {
            w.bot_command(slot, r);
        }
    }
    for slot in joined {
        if let Some(bot) = w.driver_mut().bots.get_mut(&slot) {
            bot.rearm();
        }
        w.begin(slot);
    }
    drop(p);
    // Every bot's cmds are one packet, after every real one, in slot
    // order: `SV_Frame` bumps the DObj stamp once ahead of its bot pass
    // (movers doc 15), so the moves pose them under one build.
    let _p = crate::tick_profile::phase("bots/moves");
    w.bot_packet();
    for (slot, cmd, on_ladder) in moves {
        w.send_cmd(slot, cmd, on_ladder);
    }
    for slot in entering {
        w.enter(slot);
    }
    // The bot consumed everything; a real client's ack would say the same.
    let slots: Vec<usize> = w.driver().bots.keys().copied().collect();
    for slot in slots {
        w.ack(slot);
    }
    w.tick_done();
}

/// [`BotView::lob`] for the bot in `slot`, when its brain wants one.
fn lob<W: BotWorld>(w: &W, slot: usize, view: &mut BotView, teams: &[i32]) {
    if let Some(b) = w.driver().bots.get(&slot).filter(|b| b.wants_lob(view))
        && let (Some(g), Some(at)) = (view.grenade, b.throw_target(view))
    {
        view.lob = w.lob_probe(slot, g, at, teams);
    }
}

/// The tick's yells as quick-message `mr`s into `replies`: the brains'
/// own, and "Enemy down" for each bot whose hit was the last on an enemy
/// who died since the last tick. Only a playing bot yells: the stock
/// gametypes run the quick message inline in their `menuresponse` loop,
/// whose `wait 2` drops any menu answer behind it, and the balancer's
/// team move (`bot_admin::move_bot`) answers for a dead bot.
fn yell<W: BotWorld>(
    w: &mut W,
    sid: i32,
    teams: &[i32],
    mut yells: Vec<(usize, super::yell::Yell)>,
    replies: &mut Vec<(usize, String)>,
) {
    let playing: Vec<bool> = (0..teams.len())
        .map(|s| w.body(s).is_some_and(|b| b.playing))
        .collect();
    for victim in w.driver_mut().yells.deaths(playing.clone()) {
        if let Some(k) = w.killer(victim)
            && w.driver().bots.contains_key(&k)
            && teams.get(k) != teams.get(victim)
        {
            yells.push((k, super::yell::Yell::Down));
        }
    }
    let now = w.now();
    for (slot, y) in yells {
        if !playing.get(slot).copied().unwrap_or(false) {
            continue;
        }
        let Some(menu) = w.menu_index(y.menu()) else {
            continue;
        };
        let team = teams.get(slot).copied().unwrap_or_default();
        if let Some(row) = w.driver_mut().yells.pass(slot, team, y, now) {
            replies.push((slot, format!("mr {sid} {menu} {row}")));
        }
    }
}

/// Pass 2 for one bot: its view, with the enemy (cached), the noise it
/// answers, the heal, the sidestep round a friend in the line and the
/// cover spot.
fn sense<W: BotWorld>(
    w: &mut W,
    slot: usize,
    teams: &[i32],
    noises: &[Noise],
    on_side: &[(usize, i32, [f32; 3])],
    packs: &[[f32; 3]],
) -> Option<BotView> {
    let now = w.now();
    w.feel(slot);
    let fresh = w
        .driver()
        .enemies
        .get(&slot)
        .is_none_or(|(t, _)| now.wrapping_sub(*t) >= ENEMY_REFRESH_MS);
    let enemy = if fresh {
        let e = w.enemy(slot, teams);
        w.driver_mut().enemies.insert(slot, (now, e));
        e
    } else {
        w.driver().enemies.get(&slot)?.1
    };
    // One the brain gave up on (`Bot::ignores`) is left out.
    let enemy = enemy.filter(|e| {
        !w.driver()
            .bots
            .get(&slot)
            .is_some_and(|b| b.ignores(e.slot))
    });
    let mut view = w.view(slot, enemy)?;
    let hearing = w.driver().bots.get(&slot).map_or(1.0, |b| b.skill.hearing);
    // On a side with team play on, only the teammates nearest a
    // noise go to it.
    let mine = on_side.iter().find(|c| c.0 == slot).map(|c| c.1);
    let heard: Vec<Noise> = match mine {
        Some(mine) if w.driver().bots.get(&slot).is_some_and(|b| b.team_play) => {
            let mates: Vec<(usize, [f32; 3])> = on_side
                .iter()
                .filter(|(s, t, _)| *s != slot && *t == mine)
                .map(|(s, _, p)| (*s, *p))
                .collect();
            noises
                .iter()
                .filter(|n| super::team::answers(view.origin, n, &mates))
                .copied()
                .collect()
        }
        _ => noises.to_vec(),
    };
    view.noise = super::loudest(&heard, slot, view.origin, hearing);
    let heal_at = w.driver().bots.get(&slot).and_then(|b| b.heal_at);
    view.survive.heal = super::retreat::nearest_pack(&view, packs, heal_at);
    let held = w.driver_mut().sidesteps.remove(&slot);
    if let Some(e) = enemy {
        (view.friend_in_line, view.sidestep) = w.friend_in_line(slot, e.origin, teams, held);
        if let Some(p) = view.sidestep {
            let since = held.filter(|(h, _)| *h == p).map_or(now, |(_, t)| t);
            w.driver_mut().sidesteps.insert(slot, (p, since));
        }
    }
    view.cover = cover(w, slot, enemy);
    Some(view)
}

/// [`BotView::cover`] for the bot in `slot` fighting `enemy`: the spot it
/// holds while that still hides and peeks, else the nearest that does
/// ([`super::cover::find`]), looked at every [`COVER_REFRESH_MS`].
fn cover<W: BotWorld>(
    w: &mut W,
    slot: usize,
    enemy: Option<EnemyView>,
) -> Option<super::cover::Spot> {
    use super::cover;
    let Some(e) = enemy.filter(|_| w.driver().bots.get(&slot).is_some_and(|b| b.cover.enabled))
    else {
        w.driver_mut().covers.remove(&slot);
        return None;
    };
    let now = w.now();
    let held = w.driver().covers.get(&slot).copied();
    // A wary bot that shifted off the spot it was given has its own
    // checked at once.
    let mine = w
        .driver()
        .bots
        .get(&slot)
        .filter(|b| b.wary)
        .and_then(|b| b.held_spot());
    if let Some((t, spot)) = held
        && now.wrapping_sub(t) < COVER_REFRESH_MS
        && mine.is_none_or(|m| spot.is_some_and(|s| s.at == m.at))
    {
        return spot;
    }
    let body = w.cover_body(slot, e.slot)?;
    let (g, world) = (w.driver().nav.as_ref()?, w.world()?);
    let spot = if super::dist_sq(body.origin.into(), e.origin) < cover::MIN_RANGE * cover::MIN_RANGE
    {
        None
    } else {
        let clear = |a: Vec3, b: Vec3| world.collision.shot_trace(a, b).fraction >= 1.0;
        // `PM_UpdateLean`'s sphere from the eye toward the full lean.
        let lean_room = |a: Vec3, b: Vec3| {
            let r = Vec3::splat(8.0);
            world.collision.move_trace(a, b, -r, r, body.mask).fraction
        };
        let sight = cover::Sight {
            threat_eye: body.threat_eye,
            target: e.origin.into(),
            lean: body.lean,
            clear: &clear,
            lean_room: &lean_room,
        };
        mine.or(held.and_then(|(_, s)| s))
            .and_then(|s| sight.recheck(s))
            .or_else(|| cover::find(g, body.origin, &sight))
    };
    w.driver_mut().covers.insert(slot, (now, spot));
    spot
}

/// [`BotView::hunt`] for the bot in `slot` standing at `from`: the vantage
/// over the spot it hunts ([`super::hunt::vantage`]), looked for once per
/// spot.
fn vantage<W: BotWorld>(w: &mut W, slot: usize, from: [f32; 3]) -> Option<[f32; 3]> {
    let Some(spot) = w.driver().bots.get(&slot).and_then(|b| b.hunt_query()) else {
        w.driver_mut().hunts.remove(&slot);
        return None;
    };
    if let Some((s, v)) = w.driver().hunts.get(&slot)
        && *s == spot
    {
        return *v;
    }
    let (g, world) = (w.driver().nav.as_ref()?, w.world()?);
    let sees = |feet: Vec3| {
        world
            .collision
            .shot_trace(super::hunt::eye(feet), spot.into())
            .fraction
            >= 1.0
    };
    let v = super::hunt::vantage(g, from.into(), spot.into(), &sees).map(Into::into);
    w.driver_mut().hunts.insert(slot, (spot, v));
    v
}

/// [`BotView::drop_ahead`] for the body in `slot`: per octant, the drop
/// [`crate::nav::drop_ahead`] feels standing on the ground off a ladder.
fn drop_probe<W: BotWorld>(w: &W, slot: usize) -> [Option<f32>; 8] {
    let standing = w.body(slot).filter(|b| b.on_ground && !b.on_ladder);
    let (Some(body), Some(world)) = (standing, w.world()) else {
        return [None; 8];
    };
    let key = W::STILL_WORLD.then(|| body.origin.to_array().map(f32::to_bits));
    if let Some(d) = key.and_then(|k| w.driver().probes.borrow().drops.get(&k).copied()) {
        return d;
    }
    let creases = &mut w.driver().creases.borrow_mut();
    let d = std::array::from_fn(|i| {
        let (s, c) = (i as f32 * 45.0).to_radians().sin_cos();
        let dir = Vec3::new(c, s, 0.0);
        let look = super::HAZARD_LOOK;
        crate::nav::drop_ahead(&world.collision, body.origin, dir, look, creases)
    });
    if let Some(k) = key {
        Probes::keep(&mut w.driver().probes.borrow_mut().drops, k, d);
    }
    d
}

/// [`BotView::wall_ahead`] for the body in `slot`: per octant, whether
/// [`crate::nav::wall_ahead`] pins it standing on the ground off a ladder.
fn wall_probe<W: BotWorld>(w: &W, slot: usize) -> [bool; 8] {
    let standing = w.body(slot).filter(|b| b.on_ground && !b.on_ladder);
    let (Some(body), Some(world)) = (standing, w.world()) else {
        return [false; 8];
    };
    let key = W::STILL_WORLD.then(|| body.origin.to_array().map(f32::to_bits));
    if let Some(d) = key.and_then(|k| w.driver().probes.borrow().walls.get(&k).copied()) {
        return d;
    }
    let d = std::array::from_fn(|i| {
        let (s, c) = (i as f32 * 45.0).to_radians().sin_cos();
        crate::nav::wall_ahead(&world.collision, body.origin, Vec3::new(c, s, 0.0))
    });
    if let Some(k) = key {
        Probes::keep(&mut w.driver().probes.borrow_mut().walls, k, d);
    }
    d
}

/// The drop and wall probes' verdicts by the bit pattern of the feet they
/// were felt from ([`BotDriver::probes`]). A verdict is a function of the
/// feet and the still world alone: a crease cell it read was filled by
/// its first reading, so a probe run again reads the same.
#[derive(Default)]
pub(crate) struct Probes {
    drops: std::collections::HashMap<[u32; 3], [Option<f32>; 8]>,
    walls: std::collections::HashMap<[u32; 3], [bool; 8]>,
}

impl Probes {
    /// Spots kept before a map starts over.
    const KEPT: usize = 1 << 14;

    fn keep<T>(m: &mut std::collections::HashMap<[u32; 3], T>, k: [u32; 3], v: T) {
        if m.len() >= Self::KEPT {
            m.clear();
        }
        m.insert(k, v);
    }
}

/// A teammate this slow stands still.
const MATE_STILL: f32 = 10.0;
/// A body this slow, with a still teammate within [`MATE_REACH`], is
/// held up by him.
const HELD_UP_SPEED: f32 = 60.0;
const MATE_REACH: f32 = 48.0;
/// How far up or down a still teammate stands and still holds a bot up,
/// with `BotWorld::PARTS_MATES_FLAT`.
const MATE_RISE: f32 = 64.0;
/// Ticks held up before the plan goes round the teammate, and the ticks
/// it keeps going round where he stood: let go at once, the bot walked
/// back into him from the node beside him and paced between the two.
const MATE_TICKS: u32 = 20;
const MATE_HOLD_TICKS: u32 = 200;

/// The graph nodes still teammates (`allies` or `axis`) stand on, once the bot in `slot` has
/// been held up beside one for [`MATE_TICKS`]: its follower plans round
/// them, or to a clear node beside a destination one stands on, as round
/// a parked vehicle ([`crate::nav::Follower::blocked`]), for the next
/// [`MATE_HOLD_TICKS`]. A guard who
/// held a door left a queue of teammates behind him, each walking into
/// his back (bot-objectives.md 12.7). Empty otherwise.
#[allow(clippy::too_many_arguments)]
fn mates_in_way(
    follower: &mut crate::nav::Follower,
    g: &NavGraph,
    slot: usize,
    body: Option<&Body>,
    fighting: bool,
    aloft: bool,
    flat: bool,
    boxed: bool,
    teams: &[i32],
    standing: &[(usize, Vec3, Vec3)],
) -> Arc<[u32]> {
    use crate::game::script::{TEAM_ALLIES, TEAM_AXIS};
    let team = teams
        .get(slot)
        .copied()
        .filter(|&t| t == TEAM_ALLIES || t == TEAM_AXIS);
    let still: Vec<Vec3> = standing
        .iter()
        .filter(|(s, _, v)| {
            *s != slot
                && team.is_some()
                && teams.get(*s).copied() == team
                && v.length() < MATE_STILL
        })
        .map(|&(_, o, _)| o)
        .collect();
    // The node the path heads for next.
    let next = follower.current().0.map(|n| g.nodes[n as usize]);
    let held = !fighting
        && body.is_some_and(|b| {
            let ahead = |o: &Vec3| {
                next.is_some_and(|n| {
                    let (to, way) = ((*o - b.origin).truncate(), (n - b.origin).truncate());
                    to.dot(way) > 0.5 * to.length() * way.length()
                })
            };
            (b.on_ground || (aloft && !b.on_ladder))
                && b.velocity.truncate().length() < HELD_UP_SPEED
                && still.iter().any(|o| {
                    let near = if flat {
                        (*o - b.origin).truncate().length() < MATE_REACH
                            && (o.z - b.origin.z).abs() < MATE_RISE
                    } else {
                        o.distance(b.origin) < MATE_REACH
                    };
                    near && ahead(o)
                })
        });
    follower.mate_ticks = if held { follower.mate_ticks + 1 } else { 0 };
    if follower.mate_ticks >= MATE_TICKS {
        let mut nodes: Vec<u32> = still
            .iter()
            .filter_map(|o| g.nearest(o.to_array()))
            .collect();
        if boxed {
            // On his own floor only: a node a ledge under him is no way
            // past him (mp_crash's mantle foot under a guard 57 up).
            use vcod_common::pmove::{HALF_WIDTH, STEPSIZE};
            nodes.extend(
                still
                    .iter()
                    .flat_map(|o| g.nodes_in_box(*o, 2.0 * HALF_WIDTH, STEPSIZE)),
            );
        }
        nodes.sort_unstable();
        nodes.dedup();
        follower.mate_ticks = 0;
        follower.mate_block = (MATE_HOLD_TICKS, nodes.into());
    }
    let (left, nodes) = &mut follower.mate_block;
    if *left == 0 {
        return Arc::default();
    }
    *left -= 1;
    nodes.clone()
}

/// How near another body stands, flat and in height, for an edge given
/// up on to count as a stall on a body ([`BotDriver::edge_stalls`]).
const STALL_NEAR: f32 = 64.0;

/// How far a teammate held up behind a bot may stand for it to give way,
/// and how far it steps.
const GIVE_WAY_REACH: f32 = 96.0;
const GIVE_WAY_STEP: f32 = 64.0;

/// Where the bot in `slot`, standing still, steps to make room for a
/// teammate within [`GIVE_WAY_REACH`] planning round it
/// ([`mates_in_way`]): [`GIVE_WAY_STEP`] on from him, away. Two guards in
/// mp_crash's radio room door held a third outside it until the round
/// ended: there was no way round them (bot-objectives.md 12.7).
fn give_way<W: BotWorld>(w: &W, slot: usize, teams: &[i32]) -> Option<[f32; 3]> {
    let me = w
        .body(slot)
        .filter(|b| b.playing && b.velocity.length() < MATE_STILL)?;
    let team = teams.get(slot).copied()?;
    let g = w.driver().nav.as_ref()?;
    let mine = g.nearest(me.origin.to_array())?;
    let queued = (w.driver().paths.iter())
        .filter(|(s, f)| **s != slot && teams.get(**s) == Some(&team) && f.mate_block.0 > 0)
        .filter(|(_, f)| f.mate_block.1.contains(&mine))
        .filter_map(|(s, _)| w.body(*s))
        .map(|b| b.origin)
        .filter(|o| o.distance(me.origin) < GIVE_WAY_REACH)
        .min_by(|a, b| a.distance(me.origin).total_cmp(&b.distance(me.origin)))?;
    let away = (me.origin - queued)
        .truncate()
        .normalize_or_zero()
        .extend(0.0);
    Some((me.origin + away * GIVE_WAY_STEP).to_array())
}

/// The bodies off the rungs the bot in `slot` gives way to when they meet
/// it head-on ([`crate::nav::Follower::stand_aside`]), each with the way it
/// heads: every player not a bot, by its velocity, and every bot in a
/// lower slot, toward its waypoint. Of two bots one always gives way.
fn oncoming<W: BotWorld>(
    w: &W,
    slot: usize,
    standing: &[(usize, Vec3, Vec3)],
) -> Vec<(Vec3, Vec3)> {
    let d = w.driver();
    standing
        .iter()
        .filter(|(s, _, _)| *s != slot)
        .filter_map(|&(s, o, v)| match d.bots.contains_key(&s) {
            false => Some((o, v)),
            true if s < slot => Some((o, d.paths.get(&s)?.heading()? - o)),
            true => None,
        })
        .collect()
}

/// Pass 3's walk for one bot on the graph `g`: the follower's waypoint
/// toward `goal` and the edge cues it reads off the graph, after the
/// yields to teammates on and off the ladders (`climbers`, `standing`).
#[allow(clippy::too_many_arguments)]
fn route<W: BotWorld>(
    w: &mut W,
    slot: usize,
    view: &mut BotView,
    goal: Goal,
    g: &Arc<NavGraph>,
    teams: &[i32],
    solo: &[usize],
    paths: &[super::team::TeamPath],
    main: &[usize],
    climbers: &[(usize, Vec3, Vec3)],
    standing: &[(usize, Vec3, Vec3)],
    budget: &mut u32,
) {
    use super::team::{self, Squad};
    let g = g.clone();
    let mut follower = w.driver_mut().paths.remove(&slot).unwrap_or_default();
    let gave_up = follower.gave_up;
    let shared = team_objective(view) && !solo.contains(&slot);
    let squad = shared && w.driver().bots.get(&slot).is_some_and(|b| b.squad);
    let noise = |t: i32| team::route_seed(w.server_id() as u32, t, slot);
    let spreads =
        W::YIELDS || (W::SPREADS && w.driver().bots.get(&slot).is_some_and(|b| b.tactics));
    (follower.crowd, follower.crowd_cost, follower.noise) = match teams.get(slot) {
        // A flanker pays far more on the main group's ways than on its
        // other teammates'.
        Some(&t) if spreads && squad && view.squad == Squad::Flank => (
            team::crowd(paths.iter().filter(|p| main.contains(&p.0)), slot, t),
            crate::nav::FLANK_COST,
            noise(t),
        ),
        Some(&t) if spreads && shared => (
            team::crowd(paths, slot, t),
            crate::nav::CROWD_COST,
            if squad { noise(t) } else { 0 },
        ),
        _ => (Vec::new(), crate::nav::CROWD_COST, 0),
    };
    if W::YIELDS {
        follower.blocked = w.blocked_nodes();
    }
    if W::PARTS_MATES {
        let body = w.body(slot);
        let fighting = view.enemy.is_some();
        follower.blocked = mates_in_way(
            &mut follower,
            &g,
            slot,
            body.as_ref(),
            fighting,
            W::PARTS_MATES_ALOFT,
            W::PARTS_MATES_FLAT,
            W::PARTS_MATES_BOX,
            teams,
            standing,
        );
    }
    if w.driver_mut()
        .bots
        .get_mut(&slot)
        .is_some_and(|b| b.take_stuck())
    {
        follower.stuck();
    }
    let ps = w.move_ps(slot);
    // The pace the bot runs at: a light weapon's is cut to the graph's on
    // the ground (`Bot::think`).
    follower.move_scale = view.move_scale.min(1.0);
    // What a fall off a ladder takes from this body now: a hurt bot backed
    // off mp_ship's 296-unit hold ladder onto a climber's head and died of
    // it (bot-navigation.md section 3).
    follower.fall_kills = w.fall_kills(slot);
    follower.long_mantles = W::LONG_MANTLES;
    follower.foot_slack = W::FOOT_SLACK;
    // Only where a game's own step proved the graph (`NavGraph::step`).
    follower.mantle_rests = g.step.is_some() || W::MANTLE_RESTS;
    follower.jump_cost = W::JUMP_COST;
    let body = crate::nav::Footing {
        still: ps.as_ref().is_some_and(crate::nav::ready_to_leap),
        on_ground: ps.as_ref().is_some_and(|ps| ps.on_ground),
        on_ladder: ps.as_ref().is_some_and(|ps| ps.on_ladder),
        velocity: ps.as_ref().map_or(Vec3::ZERO, |ps| ps.velocity),
    };
    // The bot's own generator, copied out so the world stays readable and
    // written back below.
    let mut rng = w.driver().bots.get(&slot).map_or(1, |b| b.rng);
    let mut rand = || (vcod_common::rng::xorshift(&mut rng) >> 33) as i32 & 0x7fff_ffff;
    // A seat plans from the vehicle on the ground under it.
    let at = view.seat.map_or(view.origin, |s| s.origin);
    let world = w.world();
    let others = |all: &[(usize, Vec3, Vec3)]| -> Vec<(Vec3, Vec3)> {
        all.iter()
            .filter(|c| c.0 != slot)
            .map(|c| (c.1, c.2))
            .collect()
    };
    let (others, aground) = (others(climbers), others(standing));
    let collision = world.map(|w| &w.collision);
    if W::SHUNS_BODIES && view.enemy.is_none() {
        view.detour = follower.bodies_in_way(&g, at.into(), body, &aground);
    }
    let aside = (W::SHUNS_BODIES && view.enemy.is_none())
        .then(|| {
            let oncoming = oncoming(w, slot, standing);
            follower.stand_aside(&g, at.into(), body, &oncoming)
        })
        .flatten();
    if let Some(spot) = aside {
        // Come to rest there, as on a leap's foot.
        view.waypoint = Some(spot.into());
        view.stop = true;
    } else if W::YIELDS
        && let Some(spot) = follower.ladder_yield(&g, at.into(), body, &others, &aground)
    {
        // Come to rest there, as on a leap's foot.
        view.waypoint = Some(spot.into());
        view.stop = true;
    } else if W::YIELDS && follower.abreast_yield(&g, at.into(), body, &others) {
        // No waypoint on the rungs climbs down (`Bot::think`).
        view.waypoint = None;
    } else if W::YIELDS
        && let Some(head) = follower.head_on_yield(&g, at.into(), body, &others)
    {
        view.waypoint = Some(head.into());
    } else {
        view.waypoint = follower.waypoint(&g, collision, goal, at, body, budget, &mut rand);
        // No waypoint while the next plan is searched for: come to rest on
        // the last one, as on a leap's foot.
        let resting = view.waypoint.is_none() && goal != Goal::Hold;
        let rest = follower.rest_point(at, body).filter(|_| resting);
        if let Some(p) = follower.climb_line(&g) {
            view.ladder_middle = Some(p.into());
        }
        view.back_face = follower.back_face(&g).map(|(y, m)| (y, m.into()));
        if view.fatigue.is_some() {
            view.sprint_room = follower.straight_run(&g, at.into());
        }
        view.jump = follower.jump_cued(&g);
        view.leap = follower.leaping(&g);
        // A jump edge proved aimed beside its top: steer there.
        if let (true, Some(w), Some(p)) = (view.jump, view.waypoint, follower.jump_aim(&g)) {
            view.waypoint = Some([p.x, p.y, w[2]]);
        }
        view.stop = follower.holding(&g);
        if let Some(foot) = ps.as_ref().and_then(|ps| follower.jump_wait(&g, at, ps)) {
            // A jump that fails from rest there is planned round; the body
            // holds still meanwhile.
            let round = collision.is_some_and(|w| follower.rest_jump_fails(&g, w, foot));
            view.waypoint = Some(if round { at } else { foot });
            view.stop = true;
            view.jump = false;
        }
        if let Some(p) = rest {
            view.waypoint = Some(p);
            view.stop = true;
        }
        if W::YIELDS
            && let Some(w) = view.waypoint
            && let Some(p) = follower.rising_hold(at.into(), w.into(), body, &others)
        {
            view.waypoint = Some(p.into());
            view.stop = true;
        }
        if W::YIELDS
            && let Some(w) = view.waypoint
            && let Some(p) = follower.pressed_yield(at.into(), w.into(), body, &aground)
        {
            view.waypoint = Some(p.into());
        }
        view.mantle = follower.mantling(&g);
        // Until the body stands at the landing: from there on to the node.
        if W::MANTLE_AIMS
            && let (Some(w), Some(q)) = (view.waypoint, follower.mantle_land(&g))
            && !view.stop
            && q.truncate().distance(glam::Vec2::new(at[0], at[1])) > MANTLE_LANDED
        {
            view.waypoint = Some([q.x, q.y, w[2]]);
        }
        // A mantle down is climbed facing the wall, never backed down
        // (only where a game's step proved the graph: bot-objectives.md
        // 12.11).
        view.run_off = !body.on_ladder
            && ((follower.mantle_rests && view.mantle)
                || follower.running_off(&g, collision, at.into(), super::BACK_DOWN_HEIGHT));
        view.crouch_jump = view.jump && follower.crouch_jumping(&g);
        if W::PMOVE_PROBES {
            let defs = w.weapon_defs();
            view.lip = view.jump
                && matches!((world, ps, view.waypoint), (Some(w), Some(ps), Some(to))
                if crate::nav::lip_ahead(&w.collision, &ps, glam::Vec2::new(to[0], to[1]), defs));
            view.push_off = match (world, ps, view.waypoint) {
                (Some(w), Some(ps), Some(to)) if ps.on_ladder => {
                    crate::nav::push_off(&w.collision, &ps, to.into(), defs)
                }
                _ => None,
            };
            view.unpin = match (world, ps, view.waypoint) {
                (Some(w), Some(ps), Some(to)) if !view.stop => {
                    crate::nav::unpin_yaw(&w.collision, &w.hazards, &ps, to.into(), defs)
                }
                _ => None,
            };
        }
    }
    if squad
        && view.enemy.is_none()
        && let (Some(t), Some(to)) = (teams.get(slot), view.waypoint)
    {
        let mates: Vec<([f32; 3], [f32; 3])> = standing
            .iter()
            .filter(|(s, _, _)| *s != slot && teams.get(*s) == Some(t))
            .map(|(_, o, v)| (o.to_array(), v.to_array()))
            .collect();
        view.mate_ahead = team::mate_ahead(at, to, &mates);
    }
    if follower.gave_up > gave_up {
        let near = climbers.iter().chain(standing).any(|&(s, o, _)| {
            s != slot
                && (o - Vec3::from(at)).truncate().length() < STALL_NEAR
                && (o.z - at[2]).abs() < STALL_NEAR
        });
        let tally = &mut w.driver_mut().edge_stalls;
        tally[0] += 1;
        tally[1] += u32::from(near);
    }
    w.routed(slot, &follower, &g);
    if let Some(b) = w.driver_mut().bots.get_mut(&slot) {
        b.rng = rng;
        if W::GIVES_UP_GOALS {
            b.arrived_goal = match goal {
                Goal::To(p) if follower.walked_out() => Some(p),
                _ => None,
            };
        }
    }
    w.driver_mut().paths.insert(slot, follower);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game::script::{TEAM_ALLIES, TEAM_AXIS};
    use crate::nav::Footing;

    /// With the game's `PARTS_MATES_BOX` the way round a still teammate
    /// standing between two nodes 32 apart keeps off both, the nodes a
    /// body's box there would meet his; without it, off the nearest only.
    #[test]
    fn a_still_teammate_blocks_the_nodes_his_box_meets() {
        let nodes: Vec<Vec3> = (0..5)
            .map(|i| Vec3::new(32.0 * i as f32, 0.0, 0.0))
            .collect();
        let edges = (0..5u32)
            .map(|i| {
                [i.wrapping_sub(1), i + 1]
                    .into_iter()
                    .filter(|&n| n < 5)
                    .collect()
            })
            .collect();
        let g = NavGraph::from_parts(nodes, edges);
        let foot = Footing {
            still: false,
            on_ground: true,
            on_ladder: false,
            velocity: Vec3::ZERO,
        };
        let me = Body {
            origin: Vec3::new(4.0, 0.0, 0.0),
            velocity: Vec3::ZERO,
            playing: true,
            on_ground: true,
            on_ladder: false,
        };
        let teams = [TEAM_ALLIES, TEAM_ALLIES];
        let mate = [(1, Vec3::new(48.0, 0.0, 0.0), Vec3::ZERO)];
        let block = |boxed: bool| {
            let mut f = Follower::default();
            let mut rand = || 0;
            for _ in 0..4 {
                let mut budget = PLAN_BUDGET;
                f.waypoint(
                    &g,
                    None,
                    Goal::To([128.0, 0.0, 0.0]),
                    [4.0, 0.0, 0.0],
                    foot,
                    &mut budget,
                    &mut rand,
                );
            }
            (0..MATE_TICKS)
                .map(|_| {
                    mates_in_way(
                        &mut f,
                        &g,
                        0,
                        Some(&me),
                        false,
                        false,
                        true,
                        boxed,
                        &teams,
                        &mate,
                    )
                    .to_vec()
                })
                .last()
                .unwrap()
        };
        assert_eq!(block(false), [1]);
        assert_eq!(block(true), [1, 2]);
    }

    /// A bot held up by a still teammate between it and its next node
    /// plans round him after [`MATE_TICKS`]; one fighting, one with the
    /// teammate behind it, or one beside an enemy does not.
    #[test]
    fn a_still_teammate_in_the_way_is_planned_round() {
        let nodes: Vec<Vec3> = (0..4)
            .map(|i| Vec3::new(64.0 * i as f32, 0.0, 0.0))
            .collect();
        let edges = vec![vec![1], vec![0, 2], vec![1, 3], vec![2]];
        let g = NavGraph::from_parts(nodes, edges);
        let mut f = Follower::default();
        let foot = Footing {
            still: false,
            on_ground: true,
            on_ladder: false,
            velocity: Vec3::ZERO,
        };
        let mut rand = || 0;
        for _ in 0..4 {
            let mut budget = PLAN_BUDGET;
            f.waypoint(
                &g,
                None,
                Goal::To([192.0, 0.0, 0.0]),
                [10.0, 0.0, 0.0],
                foot,
                &mut budget,
                &mut rand,
            );
        }
        assert!(f.current().0.is_some(), "the bot has a path");
        let me = Body {
            origin: Vec3::new(10.0, 0.0, 0.0),
            velocity: Vec3::ZERO,
            playing: true,
            on_ground: true,
            on_ladder: false,
        };
        let teams = [TEAM_ALLIES, TEAM_ALLIES, TEAM_AXIS];
        let ahead = [(1, Vec3::new(40.0, 0.0, 0.0), Vec3::ZERO)];
        let behind = [(1, Vec3::new(-20.0, 0.0, 0.0), Vec3::ZERO)];
        let enemy = [(2, Vec3::new(40.0, 0.0, 0.0), Vec3::ZERO)];
        let run = |f: &mut Follower, fighting: bool, standing: &[(usize, Vec3, Vec3)]| {
            f.mate_ticks = 0;
            f.mate_block = (0, Arc::default());
            (0..MATE_TICKS)
                .map(|_| {
                    mates_in_way(
                        f,
                        &g,
                        0,
                        Some(&me),
                        fighting,
                        false,
                        false,
                        false,
                        &teams,
                        standing,
                    )
                })
                .last()
                .unwrap()
        };
        assert_eq!(&*run(&mut f, false, &ahead), &[1]);
        assert!(run(&mut f, true, &ahead).is_empty());
        assert!(run(&mut f, false, &behind).is_empty());
        assert!(run(&mut f, false, &enemy).is_empty());
        // Off the ground (hung on a step's lip by his capsule) only with
        // the game's `PARTS_MATES_ALOFT`.
        let aloft = Body {
            on_ground: false,
            ..me
        };
        let lifted = |f: &mut Follower, on: bool| {
            f.mate_ticks = 0;
            f.mate_block = (0, Arc::default());
            (0..MATE_TICKS)
                .map(|_| {
                    mates_in_way(
                        f,
                        &g,
                        0,
                        Some(&aloft),
                        false,
                        on,
                        false,
                        false,
                        &teams,
                        &ahead,
                    )
                })
                .last()
                .unwrap()
        };
        assert!(lifted(&mut f, false).is_empty());
        assert_eq!(&*lifted(&mut f, true), &[1]);
        // On a stair's top 40 over the bot and 30 across: out of reach in
        // three dimensions, in it across with the game's
        // `PARTS_MATES_FLAT`.
        let up = [(1, Vec3::new(40.0, 0.0, 40.0), Vec3::ZERO)];
        let stair = |f: &mut Follower, flat: bool| {
            f.mate_ticks = 0;
            f.mate_block = (0, Arc::default());
            (0..MATE_TICKS)
                .map(|_| mates_in_way(f, &g, 0, Some(&me), false, false, flat, false, &teams, &up))
                .last()
                .unwrap()
        };
        assert!(stair(&mut f, false).is_empty());
        assert_eq!(&*stair(&mut f, true), &[1]);
        // One tick short of the wait plans as before; once round, the
        // way round holds for its ticks after the teammate has gone.
        f.mate_ticks = 0;
        f.mate_block = (0, Arc::default());
        for _ in 1..MATE_TICKS {
            assert!(
                mates_in_way(
                    &mut f,
                    &g,
                    0,
                    Some(&me),
                    false,
                    false,
                    false,
                    false,
                    &teams,
                    &ahead
                )
                .is_empty()
            );
        }
        assert_eq!(
            &*mates_in_way(
                &mut f,
                &g,
                0,
                Some(&me),
                false,
                false,
                false,
                false,
                &teams,
                &ahead
            ),
            &[1]
        );
        for _ in 1..MATE_HOLD_TICKS {
            assert_eq!(
                &*mates_in_way(
                    &mut f,
                    &g,
                    0,
                    Some(&me),
                    false,
                    false,
                    false,
                    false,
                    &teams,
                    &[]
                ),
                &[1]
            );
        }
        assert!(
            mates_in_way(
                &mut f,
                &g,
                0,
                Some(&me),
                false,
                false,
                false,
                false,
                &teams,
                &[]
            )
            .is_empty()
        );
    }
}
