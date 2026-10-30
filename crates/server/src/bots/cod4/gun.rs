//! CoD4 bots on the stock maps' mounted SAWs and on a live frag at their
//! feet (docs/bots.md, "CoD4"). Retail has no bots, so both rules are
//! vcod's own, over 1.7's use key and throw back (cod4-gsc.md 9.2, 9.3).
//!
//! Where no objective deals out roles, a bot takes a free gun whose arc
//! holds a lane enemies use: the enemy it fights stands in the arc past
//! [`MIN_RANGE`] from the gun, or one its team saw there within
//! [`LANE_MS`]. It walks to the spot behind the gun, straight on its own
//! floor or over the graph to another ([`BotView::post`]), and taps use
//! once the cursor hint names it. On the gun it turns the barrel onto the nearest
//! enemy in the arc it can see and holds the trigger once on target. It
//! lets go when flanked (the enemy it sees, or the one that just hit it,
//! stands outside the arc within [`FLANK_RANGE`]) or after
//! [`IDLE_RELEASE_MS`] with nobody in the arc, and takes no gun for
//! [`REST_MS`] after.
//!
//! A live frag under the cursor hint with at least [`THROW_BACK_MS`] left
//! on its fuse is taken with the frag key and thrown at once, back toward
//! its thrower (or the enemy in sight), as 1.7's throw back takes a key
//! press and a release.

use std::collections::HashMap;

use glam::Vec3;
use vcod_cod4::net::clmsg::UserCmd as UserCmd4;
use vcod_cod4::net::delta::ENTITYNUM_NONE;
use vcod_cod4::pmove::{PlayerState, button, ef};
use vcod_gsc::EntId;

use super::{Cod4Bots, deg_short, eye};
use crate::bots::BotView;
use crate::game::cod2_turret::GunInfo;
use crate::game::cod4::level::Cod4Level;
use crate::game::script::{TEAM_ALLIES, TEAM_AXIS};

/// How far from the spot behind a gun a bot walks straight to it.
const GUN_REACH: f32 = 400.0;
/// The most a bot's feet and the spot's may differ in height for the
/// straight walk.
const GUN_STEP: f32 = 40.0;
/// How far from the spot a bot with no enemy in sight sets off over the
/// graph for a gun on another floor (its post, [`BotView::post`]):
/// mp_backlot's two stand upstairs, where no straight walk reaches them.
const GUN_PATH_REACH: f32 = 800.0;
/// The graph's node nearest the spot must lie this near it, about a step
/// up or down, for the walk over the graph to end there.
const GUN_NODE_REACH: f32 = 48.0;
/// An enemy nearer the gun than this is fought by hand.
const MIN_RANGE: f32 = 250.0;
/// How long a team remembers an enemy seen in a gun's arc, ms.
const LANE_MS: i32 = 8_000;
/// A gun a team's bot set off for over the graph, or let go with nobody
/// in its arc, takes no other walk over the graph from that team for this
/// long, ms.
const FAR_REST_MS: i32 = 180_000;
/// A bot walks over the graph to a gun only this long past the last
/// enemy it saw, ms.
const CALM_MS: i32 = 20_000;
/// A bot that let a gun go takes none for this long, ms.
const REST_MS: i32 = 20_000;
/// The walk to a gun gives up after this, ms; one over the graph after
/// [`PATH_APPROACH_MS`].
const APPROACH_MS: i32 = 5000;
const PATH_APPROACH_MS: i32 = 15_000;
/// A gunner with nobody in the arc this long lets go, ms.
const IDLE_RELEASE_MS: i32 = 3000;
/// An enemy outside the arc this near flanks the gunner.
const FLANK_RANGE: f32 = 1200.0;
/// A hit this recent from outside the arc flanks the gunner, ms.
const FLANK_HIT_MS: i32 = 300;
/// The barrel's turn per tick, degrees, and how near the aim it fires.
const TURN_PER_TICK: f32 = 12.0;
const FIRE_CONE: f32 = 2.0;
/// The spot a bot mounts from, behind the gun along its yaw (the retail
/// capture's probe stood 50 behind mp_backlot's crouch SAW).
const BACK: f32 = 48.0;
/// The least fuse a bot takes a live frag back with, ms: the pickup, the
/// throw's wind-up and the flight fit in it.
pub(super) const THROW_BACK_MS: i32 = 1500;
/// The throw back's loft over the line to its target, degrees up.
const THROW_LOFT: f32 = 12.0;
/// Ticks the view stays on the throw's line after the release.
const THROW_HOLD_TICKS: u8 = 6;
/// The use key in a 1.7 cmd (`to_cmd4`'s map of `BUTTON_USE`).
const USE: u32 = 0x8;
/// The walk's box: the player's, the step height up.
const WALK_MINS: Vec3 = Vec3::new(-15.0, -15.0, 18.0);
const WALK_MAXS: Vec3 = Vec3::new(15.0, 15.0, 70.0);

/// One bot's business with a gun.
#[derive(Clone, Copy, Debug, Default)]
pub(super) struct GunTask {
    gun: Option<EntId>,
    /// When the walk began, and when the gunner last had a target.
    since: i32,
    seen: i32,
    /// The last cmd held use: a press is its rising edge.
    pressed: bool,
    /// On the gun, not walking to it.
    manning: bool,
    /// Walking to it over the graph, not straight, to the floor under
    /// the spot behind it.
    far: bool,
    spot: Vec3,
}

/// What the gun step makes of the brain's cmd.
#[derive(Clone, Copy, Debug)]
pub(super) enum GunCmd {
    /// Walking to the gun: use on this cmd or not.
    Approach { press: bool },
    /// On the gun: the view, the trigger, a use press to let go.
    Man {
        view: [f32; 2],
        fire: bool,
        release: bool,
    },
}

/// A throw back under way: the view to throw along, the frag key held
/// (taking the grenade) or let go (throwing it).
#[derive(Clone, Copy, Debug)]
pub(super) struct ThrowBack {
    view: [f32; 2],
    hold: bool,
}

/// What the bots keep for the guns and throw backs across ticks.
#[derive(Default)]
pub(crate) struct GunState {
    tasks: HashMap<usize, GunTask>,
    /// When each bot last let a gun go.
    released: HashMap<usize, i32>,
    /// When a team last set off for each gun over the graph or let it go
    /// idle: no walk over the graph goes to it for [`FAR_REST_MS`].
    far_rest: HashMap<(EntId, i32), i32>,
    /// When each bot last saw an enemy ([`CALM_MS`]).
    last_enemy: HashMap<usize, i32>,
    /// The last time each team saw an enemy in each gun's arc.
    lanes: HashMap<(EntId, i32), i32>,
    /// Each bot's throw back: its line and the ticks left to hold it.
    throws: HashMap<usize, ([f32; 2], u8)>,
    /// Guns manned and frags thrown back, for `bot_match`.
    pub mounts: u32,
    pub throw_backs: u32,
}

impl GunState {
    /// A new level's state keeps only the counts.
    pub(crate) fn carry_counts(&mut self, old: &GunState) {
        self.mounts = old.mounts;
        self.throw_backs = old.throw_backs;
    }

    /// A bot gone.
    pub(crate) fn forget(&mut self, slot: usize) {
        self.tasks.remove(&slot);
        self.throws.remove(&slot);
        self.released.remove(&slot);
    }
}

/// `at`'s yaw off the gun's own, folded into (-180, 180].
fn arc_yaw(gun: &GunInfo, at: Vec3) -> f32 {
    let d = at - gun.origin;
    angle_180(d.y.atan2(d.x).to_degrees() - gun.yaw)
}

fn angle_180(a: f32) -> f32 {
    let a = a.rem_euclid(360.0);
    if a > 180.0 { a - 360.0 } else { a }
}

/// Whether `at` is inside the gun's yaw arc.
fn in_arc(gun: &GunInfo, at: Vec3) -> bool {
    let y = arc_yaw(gun, at);
    y >= gun.yaw_range[0] && y <= gun.yaw_range[1]
}

/// Whether `at` lies down a lane the gun covers: in its arc and past
/// [`MIN_RANGE`] from it.
fn covers(gun: &GunInfo, at: Vec3) -> bool {
    in_arc(gun, at) && gun.origin.distance(at) > MIN_RANGE
}

/// The spot behind the gun a bot mounts from, at the gun's height.
fn mount_spot(gun: &GunInfo) -> Vec3 {
    let (s, c) = gun.yaw.to_radians().sin_cos();
    gun.origin - Vec3::new(c, s, 0.0) * BACK
}

/// The view from `eye` onto `at`: pitch (down positive), yaw.
fn look_at(eye: Vec3, at: Vec3) -> [f32; 2] {
    let d = at - eye;
    let flat = d.truncate().length();
    [-d.z.atan2(flat).to_degrees(), d.y.atan2(d.x).to_degrees()]
}

/// `from` turned toward `to` by at most `step` degrees an axis.
fn turn(from: [f32; 2], to: [f32; 2], step: f32) -> [f32; 2] {
    std::array::from_fn(|i| from[i] + angle_180(to[i] - from[i]).clamp(-step, step))
}

/// The cmd's angles for `view` over 1.7's float `delta_angles`.
fn cmd_angles(c: &mut UserCmd4, view: [f32; 2], delta: [f32; 3]) {
    for i in 0..2 {
        c.angles[i] = ((deg_short(view[i]) - deg_short(delta[i])) & 0xffff) as u16;
    }
}

fn foe(dm: bool, mine: i32, theirs: i32) -> bool {
    dm || !(mine == TEAM_AXIS || mine == TEAM_ALLIES)
        || (theirs != mine && (theirs == TEAM_AXIS || theirs == TEAM_ALLIES))
}

impl Cod4Bots {
    /// Guns manned and live frags thrown back so far, over every level.
    pub fn gun_counts(&self) -> (u32, u32) {
        (self.gun.mounts, self.gun.throw_backs)
    }

    /// Test-facing: `team` saw an enemy down `gun`'s lane at `now_ms`
    /// (the level's time).
    pub fn mark_lane_for_test(&mut self, gun: EntId, team: i32, now_ms: i32) {
        self.gun.lanes.insert((gun, team), now_ms);
    }

    /// The guns the bots may man this tick (`BotWorld::frame`): none where
    /// an objective deals out roles or [`Cod4Bots::man_guns`] is off.
    pub(super) fn read_guns(&mut self, level: &mut Cod4Level, free_play: bool) {
        self.pass.guns = if free_play && self.man_guns {
            level.gun_list()
        } else {
            Vec::new()
        };
    }

    /// Pass 3 after the route (`BotWorld::pre_think`): the gun step and
    /// the throw back, kept for [`Self::apply_steps`].
    pub(crate) fn pre_think(
        &mut self,
        level: &Cod4Level,
        slot: usize,
        view: &mut BotView,
        teams: &[i32],
    ) {
        let guns = std::mem::take(&mut self.pass.guns);
        let gun = self.gun_step(level, slot, view, &guns, teams);
        self.pass.guns = guns;
        let throw = if gun.is_none() {
            self.throw_back_step(level, slot, view)
        } else {
            None
        };
        self.pass.steps = Some((slot, gun, throw));
    }

    /// The steps [`Self::pre_think`] kept, over the cmd the brain asked
    /// for; a throw back ends the brain's own throw.
    pub(super) fn apply_steps(&mut self, slot: usize, c: &mut UserCmd4, ps: &PlayerState) {
        let Some((s, gun, throw)) = self.pass.steps.take() else {
            return;
        };
        if s != slot {
            return;
        }
        if let Some(step) = &gun {
            apply_gun(c, step, ps);
        }
        if let Some(t) = &throw {
            self.throwing.remove(&slot);
            apply_throw(c, t, ps);
        }
    }

    /// The gun step for the bot in `slot` this tick, after its route: a
    /// walk to a gun sets `view`'s waypoint. `guns` is empty where the
    /// gametype has objectives or the bots man none.
    pub(super) fn gun_step(
        &mut self,
        level: &Cod4Level,
        slot: usize,
        view: &mut BotView,
        guns: &[GunInfo],
        teams: &[i32],
    ) -> Option<GunCmd> {
        let now = self.pass.now;
        if guns.is_empty() || !self.playing(level, slot) {
            self.gun.tasks.remove(&slot);
            return None;
        }
        let ps = self.ps(slot)?.clone();
        let mine = teams.get(slot).copied().unwrap_or(0);
        // The lanes: an enemy this bot sees in a gun's arc marks it for
        // its team.
        if view.enemy.is_some() {
            self.gun.last_enemy.insert(slot, now);
        }
        if let Some(e) = view.enemy {
            for g in guns.iter().filter(|g| covers(g, Vec3::from(e.origin))) {
                self.gun.lanes.insert((g.id, mine), now);
            }
        }
        let mut task = self.gun.tasks.get(&slot).copied().unwrap_or_default();
        let out = if ps.e_flags & ef::TURRET != 0 {
            let gun = guns.iter().find(|g| g.owner == Some(slot))?;
            let my_eye = eye(&ps);
            let target = self.arc_enemy(level, slot, gun, my_eye, teams);
            if !task.manning || task.gun != Some(gun.id) {
                self.gun.mounts += 1;
                task.seen = now;
            }
            if target.is_some() {
                task.seen = now;
            }
            task.gun = Some(gun.id);
            task.manning = true;
            let cur = [ps.view_angles[0], ps.view_angles[1]];
            let (aim, fire) = match target {
                Some(at) => {
                    let want = look_at(my_eye, at);
                    let aim = turn(cur, want, TURN_PER_TICK);
                    let off = (0..2).map(|i| angle_180(want[i] - aim[i]).abs());
                    (aim, off.fold(0.0, f32::max) <= FIRE_CONE)
                }
                None => (cur, false),
            };
            let flanked = self.flanked(gun, view);
            let idle = now - task.seen >= IDLE_RELEASE_MS;
            let release = (flanked || idle) && !task.pressed;
            task.pressed = release;
            if release {
                self.gun.released.insert(slot, now);
                if idle && !flanked {
                    self.gun.far_rest.insert((gun.id, mine), now);
                }
            }
            Some(GunCmd::Man {
                view: aim,
                fire: fire && !release,
                release,
            })
        } else {
            if let Some(id) = task.gun {
                let gun = guns.iter().find(|g| g.id == id);
                let lost = gun.is_none_or(|g| g.owner.is_some());
                let limit = if task.far {
                    PATH_APPROACH_MS
                } else {
                    APPROACH_MS
                };
                if task.manning || lost || now - task.since > limit {
                    task = GunTask::default();
                }
            }
            let rested = (self.gun.released.get(&slot)).is_none_or(|&t| now - t >= REST_MS);
            if task.gun.is_none()
                && rested
                && let Some((gun, spot, far)) = self.pick_gun(slot, view, guns, teams)
            {
                if far {
                    self.gun.far_rest.insert((gun, mine), now);
                }
                task = GunTask {
                    gun: Some(gun),
                    since: now,
                    far,
                    spot,
                    ..GunTask::default()
                };
            }
            match task.gun.and_then(|id| guns.iter().find(|g| g.id == id)) {
                Some(gun) => {
                    // Over the graph the route walks to the post; the last
                    // of it is straight.
                    if !task.far || self.straight_walk(Vec3::from(view.origin), task.spot) {
                        view.waypoint = Some(mount_spot(gun).to_array());
                        view.stop = false;
                    }
                    let hint = ps.cursor_hint_ent_index == gun.id.0 as i32;
                    let press = hint && !task.pressed;
                    task.pressed = press;
                    Some(GunCmd::Approach { press })
                }
                None => None,
            }
        };
        if task.gun.is_some() {
            self.gun.tasks.insert(slot, task);
        } else {
            self.gun.tasks.remove(&slot);
        }
        out
    }

    /// Whether the gunner in `slot` is flanked: the enemy the brain sees,
    /// or the one that just hit it, outside the gun's arc and near.
    fn flanked(&self, gun: &GunInfo, view: &BotView) -> bool {
        let near_out = |at: Vec3| !in_arc(gun, at) && gun.origin.distance(at) < FLANK_RANGE;
        let seen = view.enemy.is_some_and(|e| near_out(Vec3::from(e.origin)));
        let hit = view
            .hurt
            .filter(|h| h.ago_ms <= FLANK_HIT_MS)
            .and_then(|h| self.ps(h.slot))
            .is_some_and(|p| near_out(p.origin));
        seen || hit
    }

    /// The post a bot walking to a gun over the graph makes for
    /// (`BotWorld::pre_route`): the floor behind the gun.
    pub(crate) fn gun_post(&self, slot: usize) -> Option<[f32; 3]> {
        let t = self.gun.tasks.get(&slot)?;
        (t.gun.is_some() && t.far && !t.manning).then_some(t.spot.to_array())
    }

    /// The floor under the spot behind `gun`. `None` with no world or no
    /// floor there.
    fn floor_spot(&self, gun: &GunInfo) -> Option<Vec3> {
        let (world, _) = self.world.as_ref()?;
        let spot = mount_spot(gun);
        let floor = world.collision.move_trace(
            spot + Vec3::Z * 32.0,
            spot - Vec3::Z * 96.0,
            Vec3::ZERO,
            Vec3::ZERO,
            vcod_cod4::pmove::MASK_PLAYERSOLID,
        );
        (floor.fraction < 1.0).then_some(floor.endpos)
    }

    /// Whether feet at `feet` walk straight to `spot`: within
    /// [`GUN_REACH`], level with it and unblocked.
    fn straight_walk(&self, feet: Vec3, spot: Vec3) -> bool {
        let Some((world, _)) = self.world.as_ref() else {
            return false;
        };
        if (spot - feet).truncate().length() > GUN_REACH || (spot.z - feet.z).abs() > GUN_STEP {
            return false;
        }
        let walk = world.collision.move_trace(
            feet,
            spot,
            WALK_MINS,
            WALK_MAXS,
            vcod_cod4::pmove::MASK_PLAYERSOLID,
        );
        walk.fraction >= 1.0
    }

    /// Whether the bot in `slot` stands nearer `spot` than every other
    /// playing bot on its team.
    fn nearest_mate_to(&self, slot: usize, teams: &[i32], spot: Vec3) -> bool {
        let Some(me) = self.ps(slot).map(|p| p.origin.distance(spot)) else {
            return false;
        };
        (0..teams.len())
            .filter(|&s| s != slot && teams[s] == teams[slot])
            .filter_map(|s| self.ps(s))
            .all(|p| p.origin.distance(spot) >= me)
    }

    /// Whether the graph's node nearest `spot` stands within
    /// [`GUN_NODE_REACH`] of it and about a step up or down: a walk over
    /// the graph ends beside the gun.
    fn graph_reaches(&self, spot: Vec3) -> bool {
        let Some(g) = self.driver.nav.as_ref() else {
            return false;
        };
        g.nearest(spot.to_array()).is_some_and(|n| {
            let d = g.nodes[n as usize] - spot;
            d.truncate().length() <= GUN_NODE_REACH && d.z.abs() <= vcod_common::pmove::STEPSIZE
        })
    }

    /// The nearest free gun the bot in `slot` should go for, the floor
    /// behind it and whether the walk there goes over the graph: one
    /// covering the enemy it fights with the spot a straight walk away
    /// ([`Self::straight_walk`]), or one down a lane its team saw an enemy
    /// in lately, a straight walk away, or on another floor within
    /// [`GUN_PATH_REACH`] and on the graph when [`CALM_MS`] have passed
    /// since it last saw an enemy, no teammate stands nearer the spot and
    /// the team's [`FAR_REST_MS`] on the gun have run. A bot fighting an
    /// enemy outside every arc goes for none.
    fn pick_gun(
        &self,
        slot: usize,
        view: &BotView,
        guns: &[GunInfo],
        teams: &[i32],
    ) -> Option<(EntId, Vec3, bool)> {
        let team = teams.get(slot).copied().unwrap_or(0);
        let now = self.pass.now;
        let enemy = view.enemy.map(|e| Vec3::from(e.origin));
        let feet = Vec3::from(view.origin);
        let calm = (self.gun.last_enemy.get(&slot)).is_none_or(|&t| now - t >= CALM_MS);
        let taken: Vec<EntId> = (self.gun.tasks.iter())
            .filter(|(s, _)| **s != slot)
            .filter_map(|(_, t)| t.gun)
            .collect();
        guns.iter()
            .filter(|g| g.owner.is_none() && !taken.contains(&g.id))
            .filter(|g| match enemy {
                Some(at) => covers(g, at),
                None => self
                    .gun
                    .lanes
                    .get(&(g.id, team))
                    .is_some_and(|&t| now - t <= LANE_MS),
            })
            .filter_map(|g| {
                let spot = self.floor_spot(g)?;
                let d = (spot - feet).truncate().length();
                if self.straight_walk(feet, spot) {
                    return Some((d, g.id, spot, false));
                }
                let far = enemy.is_none()
                    && calm
                    && (spot.z - feet.z).abs() > GUN_STEP
                    && d <= GUN_PATH_REACH
                    && (self.gun.far_rest.get(&(g.id, team)))
                        .is_none_or(|&t| now - t >= FAR_REST_MS)
                    && self.nearest_mate_to(slot, teams, spot)
                    && self.graph_reaches(spot);
                far.then_some((d, g.id, spot, true))
            })
            .min_by(|a, b| a.0.total_cmp(&b.0))
            .map(|(_, id, spot, far)| (id, spot, far))
    }

    /// The nearest foe whose chest is in `gun`'s arc, in range and in
    /// clear sight of `my_eye`.
    fn arc_enemy(
        &self,
        level: &Cod4Level,
        slot: usize,
        gun: &GunInfo,
        my_eye: Vec3,
        teams: &[i32],
    ) -> Option<Vec3> {
        if !self.shoot {
            return None;
        }
        let mine = teams.get(slot).copied().unwrap_or(0);
        let (world, _) = self.world.as_ref()?;
        let w = &world.collision;
        let mut seen: Vec<(f32, usize, Vec3)> = (self.pass.states.iter())
            .filter(|(i, _)| **i != slot && self.playing(level, **i))
            .filter(|(i, _)| foe(self.pass.dm, mine, teams.get(**i).copied().unwrap_or(0)))
            .filter_map(|(&i, p)| {
                let chest = Vec3::from(crate::bots::chest(p.origin.into(), p.view_height_current));
                let d = chest.distance(my_eye);
                (d <= crate::bots::SHOOT_RANGE && in_arc(gun, chest)).then_some((d, i, chest))
            })
            .collect();
        // The states map has no order: nearest first, then by slot.
        seen.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
        seen.into_iter()
            .find(|(_, _, chest)| self.shot_clear(w, my_eye, *chest))
            .map(|(_, _, chest)| chest)
    }

    /// The throw back for the bot in `slot` this tick: a live frag under
    /// its hint with [`THROW_BACK_MS`] or more left is taken with the frag
    /// key, the view on the line back to its thrower (else the enemy in
    /// sight, else straight ahead) lofted [`THROW_LOFT`]; once in hand the
    /// key is let go, which throws it, the view held on that line a few
    /// ticks. `None` while nothing is under way.
    pub(super) fn throw_back_step(
        &mut self,
        level: &Cod4Level,
        slot: usize,
        view: &BotView,
    ) -> Option<ThrowBack> {
        let ps = self.ps(slot)?.clone();
        let none = ENTITYNUM_NONE as i32;
        if !self.throw_back || !self.playing(level, slot) || ps.e_flags & ef::TURRET != 0 {
            self.gun.throws.remove(&slot);
            return None;
        }
        if ps.throw_back_grenade_owner != none && ps.grenade_time_left > 0 {
            // In hand: let the key go along the line taken.
            let line = self
                .gun
                .throws
                .get(&slot)
                .map_or([ps.view_angles[0], ps.view_angles[1]], |t| t.0);
            self.gun.throws.insert(slot, (line, THROW_HOLD_TICKS));
            return Some(ThrowBack {
                view: line,
                hold: false,
            });
        }
        if let Some((line, left)) = self.gun.throws.get(&slot).copied() {
            if left > 0 {
                self.gun.throws.insert(slot, (line, left - 1));
                return Some(ThrowBack {
                    view: line,
                    hold: false,
                });
            }
            self.gun.throws.remove(&slot);
        }
        let hint = ps.cursor_hint_ent_index;
        if hint == none || ps.throw_back_grenade_time_left < THROW_BACK_MS {
            return None;
        }
        let missile = (level.rt.host.cod4().missiles.iter()).find(|m| m.id.0 as i32 == hint)?;
        let thrower = missile
            .owner
            .filter(|&o| o != slot)
            .and_then(|o| self.ps(o))
            .map(|p| p.origin + Vec3::Z * 40.0);
        let target = thrower.or(view.enemy.map(|e| Vec3::from(e.origin)));
        let mut line = match target {
            Some(at) => look_at(eye(&ps), at),
            None => [ps.view_angles[0], ps.view_angles[1]],
        };
        line[0] -= THROW_LOFT;
        self.gun.throws.insert(slot, (line, THROW_HOLD_TICKS));
        self.gun.throw_backs += 1;
        Some(ThrowBack {
            view: line,
            hold: true,
        })
    }
}

/// The gun step's cmd over the brain's: on the gun nothing but the view,
/// the trigger and the release's use; on the way a use press added.
pub(super) fn apply_gun(c: &mut UserCmd4, step: &GunCmd, ps: &PlayerState) {
    match *step {
        GunCmd::Approach { press } => {
            c.buttons &= !USE;
            if press {
                c.buttons |= USE;
            }
        }
        GunCmd::Man {
            view,
            fire,
            release,
        } => {
            c.buttons = 0;
            if fire {
                c.buttons |= button::ATTACK;
            }
            if release {
                c.buttons |= USE;
            }
            c.forwardmove = 0;
            c.rightmove = 0;
            cmd_angles(c, view, ps.delta_angles);
        }
    }
}

/// The throw back's cmd over the brain's: the frag key alone of the
/// offhand keys, the view on the throw's line.
pub(super) fn apply_throw(c: &mut UserCmd4, t: &ThrowBack, ps: &PlayerState) {
    c.buttons &= !(button::FRAG | button::SMOKE | button::ATTACK);
    if t.hold {
        c.buttons |= button::FRAG;
    }
    cmd_angles(c, t.view, ps.delta_angles);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gun() -> GunInfo {
        GunInfo {
            id: EntId(393, 0),
            origin: Vec3::new(-720.0, 68.0, 290.0),
            yaw: 0.0,
            yaw_range: [-45.0, 45.0],
            pitch_range: [-15.0, 30.0],
            owner: None,
        }
    }

    #[test]
    fn a_lane_is_down_the_arc_past_the_near_range() {
        let g = gun();
        assert!(covers(&g, Vec3::new(-200.0, 100.0, 290.0)));
        assert!(!covers(&g, Vec3::new(-600.0, 68.0, 290.0)));
        assert!(!covers(&g, Vec3::new(-200.0, 700.0, 290.0)));
        assert!(!covers(&g, Vec3::new(-1200.0, 68.0, 290.0)));
        let spot = mount_spot(&g);
        assert!((spot - Vec3::new(-768.0, 68.0, 290.0)).length() < 1e-3);
    }

    #[test]
    fn a_gunner_cmd_holds_still_and_fires_or_lets_go() {
        let ps = PlayerState {
            delta_angles: [0.0, 90.0, 0.0],
            ..Default::default()
        };
        let mut c = UserCmd4 {
            buttons: button::CROUCH | USE,
            forwardmove: 127,
            ..Default::default()
        };
        let step = GunCmd::Man {
            view: [0.0, 90.0],
            fire: true,
            release: false,
        };
        apply_gun(&mut c, &step, &ps);
        assert_eq!(c.buttons, button::ATTACK);
        assert_eq!((c.forwardmove, c.angles[0], c.angles[1]), (0, 0, 0));
    }

    #[test]
    fn a_throw_back_holds_the_frag_key_then_lets_it_go() {
        let ps = PlayerState::default();
        let mut c = UserCmd4 {
            buttons: button::ATTACK | button::SMOKE,
            ..Default::default()
        };
        let take = ThrowBack {
            view: [-12.0, 45.0],
            hold: true,
        };
        apply_throw(&mut c, &take, &ps);
        assert_eq!(c.buttons, button::FRAG);
        assert_eq!(c.angles[1], deg_short(45.0) as u16);
        let throw = ThrowBack {
            hold: false,
            ..take
        };
        apply_throw(&mut c, &throw, &ps);
        assert_eq!(c.buttons, 0);
    }
}
