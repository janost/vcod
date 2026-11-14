//! Self-preservation: a bot hurt in a fight, or whose clip ran dry with no
//! quicker sidearm, breaks contact. It walks to the nearest graph node the
//! threat's eye cannot see ([`cover_spot`]), backing off with its aim still
//! on the threat, reloads there and holds a while facing the way the threat
//! was, then fights or plays on. A bot with a duty no retreat should break
//! (a carrier, a defuser, a body holding a zone) stays.
//! docs/research/bot-combat.md 5.

use super::*;
use crate::nav::NavGraph;
use glam::Vec3;
use std::cmp::Reverse;
use std::collections::BinaryHeap;
use vcod_common::collision::CollisionWorld;

/// Under this much health, percent of the most, a bot in a fight breaks
/// contact: one more round from most guns kills it.
pub(crate) const HURT_PCT: i32 = 50;
/// Cover is looked for along the graph this far from the body, units.
const COVER_REACH: f32 = 1200.0;
/// At most this many nodes are tested for cover, nearest first.
const COVER_TESTS: usize = 300;
/// A cover node nearer the threat than this is no cover: the threat walks
/// round it in a second.
const COVER_MIN: f32 = 200.0;
/// The cover is reached this near across and up or down.
const COVER_REACHED: f32 = 24.0;
const COVER_HEIGHT: f32 = 48.0;
/// A hurt bot holds its cover this long, ticks, before it plays on.
const HOLD_TICKS: u32 = 60;
/// No retreat lasts longer than this, ticks.
const RETREAT_TICKS: u32 = 200;

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Why {
    /// Health under [`HURT_PCT`].
    Hurt,
    /// The clip ran dry, the reserve holds more, and no pistol came out.
    Dry,
}

/// A retreat under way.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Retreat {
    pub(crate) why: Why,
    /// The threat's eye as last seen: what the cover hides the bot from.
    pub(crate) threat: [f32; 3],
    /// The cover the server found last tick.
    pub(crate) cover: Option<[f32; 3]>,
    ticks: u32,
    /// Ticks stood at the cover.
    held: u32,
}

/// What the retreat reads off the body; the server fills it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Vitals {
    pub health: i32,
    pub max_health: i32,
    /// The held weapon's `clipSize`; 0 when unknown.
    pub clip_size: i16,
    /// [`BotView::enemy`]'s eye.
    pub enemy_eye: Option<[f32; 3]>,
    /// The nearest health item lying within [`HEAL_REACH`] of a hurt body
    /// ([`nearest_pack`]).
    pub heal: Option<[f32; 3]>,
    /// While the bot retreats ([`Bot::cover_query`]), the spot it hides at
    /// ([`cover_spot`]); `None` when there is none.
    pub cover: Option<[f32; 3]>,
}

impl Default for Vitals {
    fn default() -> Self {
        Vitals {
            health: 100,
            max_health: 100,
            clip_size: 0,
            enemy_eye: None,
            heal: None,
            cover: None,
        }
    }
}

/// Under this much health, percent, a bot with nothing in sight walks to a
/// health item lying within [`HEAL_REACH`]: a pickup adds what the item
/// gives up to the most (cod11-items.md 5).
pub(crate) const HEAL_PCT: i32 = 70;
const HEAL_REACH: f32 = 1200.0;
const HEAL_HEIGHT: f32 = 200.0;

/// The nearest of `packs` within [`HEAL_REACH`] across and [`HEAL_HEIGHT`]
/// up or down of a body under [`HEAL_PCT`]; `held`, the item walked to
/// last tick, while it still lies there within [`HEAL_REACH`] across at
/// any height. Picked afresh each tick, a bot between two items on
/// mp_ship's hold at (4450, 700, -64), one on the floor and one on the
/// deck over it, turned for whichever stood nearer across and paced
/// between their ways for 10 s; and one whose way to an item climbed a
/// stair more than [`HEAL_HEIGHT`] over it dropped the item at the top,
/// took the drop back down for its other goal and climbed again
/// (bot-navigation.md, "The ship's leftovers").
pub(crate) fn nearest_pack(
    view: &BotView,
    packs: &[[f32; 3]],
    held: Option<[f32; 3]>,
) -> Option<[f32; 3]> {
    let v = &view.survive;
    if v.health * 100 >= v.max_health * HEAL_PCT {
        return None;
    }
    let o = view.origin;
    let across = |p: &[f32; 3]| (p[0] - o[0]).hypot(p[1] - o[1]);
    if let Some(h) = held.filter(|h| packs.contains(h) && across(h) < HEAL_REACH) {
        return Some(h);
    }
    packs
        .iter()
        .map(|p| (across(p), *p))
        .filter(|(d, p)| *d < HEAL_REACH && (p[2] - o[2]).abs() < HEAL_HEIGHT)
        .min_by(|a, b| a.0.total_cmp(&b.0))
        .map(|(_, p)| p)
}

/// Whether `at` stands at the cover `c`.
pub(super) fn at_cover(at: [f32; 3], c: [f32; 3]) -> bool {
    (c[0] - at[0]).hypot(c[1] - at[1]) < COVER_REACHED && (c[2] - at[2]).abs() < COVER_HEIGHT
}

impl Bot {
    /// Turns the retreat on or off for this bot (on by default).
    pub fn set_retreat(&mut self, on: bool) {
        self.retreat_on = on;
        if !on {
            self.retreat = None;
        }
    }

    /// Breaking contact ([`BotBody::retreating`]).
    pub fn retreating(&self) -> u8 {
        match self.retreat {
            None => 0,
            Some(r) if r.held > 0 => 3,
            Some(r) if r.why == Why::Hurt => 1,
            Some(_) => 2,
        }
    }

    /// While a retreat is under way, the threat's eye and the cover held:
    /// what the server's [`cover_spot`] fills [`Vitals::cover`] from before
    /// [`Bot::goal`] reads it.
    pub(crate) fn cover_query(&self) -> Option<([f32; 3], Option<[f32; 3]>)> {
        self.retreat.map(|r| (r.threat, r.cover))
    }

    /// The goal of a retreat with cover found: the cover, or holding it.
    pub(super) fn retreat_goal(&self, view: &BotView) -> Option<Goal> {
        self.retreat?;
        let c = view.survive.cover?;
        Some(if at_cover(view.origin, c) {
            Goal::Hold
        } else {
            Goal::To(c)
        })
    }

    /// A hurt fighting bot with nothing in sight and no duty walks to a
    /// health item lying near ([`Vitals::heal`]).
    pub(super) fn heal_goal(&self, view: &BotView) -> Option<Goal> {
        if !self.retreat_on
            || !self.shoot
            || self.retreat.is_some()
            || view.enemy.is_some()
            || view.seat.is_some()
        {
            return None;
        }
        let p = view.survive.heal?;
        (!self.duty_bound(view)).then_some(Goal::To(p))
    }

    /// Stood at the cover: the body holds still on purpose.
    pub(super) fn holding_cover(&self, view: &BotView) -> bool {
        self.retreat.is_some() && view.survive.cover.is_some_and(|c| at_cover(view.origin, c))
    }

    /// A duty a retreat would break: a plant or defuse under way, a carried
    /// objective, a defuse against the clock, a body counted in a zone.
    fn duty_bound(&self, view: &BotView) -> bool {
        if self.objective_busy(view)
            || self.carrying(view)
            || matches!(&view.uo, Some(UoView::Ctf(c)) if c.carrying)
        {
            return true;
        }
        match self.objective_target(view) {
            Some(ObjTarget::Defuse { .. }) => true,
            Some(t @ (ObjTarget::Occupy(_) | ObjTarget::Radio { .. })) => {
                Self::at_objective(view, &t)
            }
            _ => false,
        }
    }

    /// The tick's keys under a retreat, after the fight set its own:
    /// toward the cover along the path whatever the footwork wanted, off
    /// the sights; at the cover still, crouched and facing where the threat
    /// was while nothing is in sight, topping the clip up. `path` is the
    /// path's (forward, right, up, wbuttons) under the view at `path_yaw`,
    /// `look` the (pitch, yaw) the cmd aims at; returns the look to use.
    pub(super) fn retreat_keys(
        &mut self,
        view: &BotView,
        cmd: &mut UserCmd,
        path: (i8, i8, i8, u8),
        path_yaw: f32,
        look: [f32; 2],
    ) -> [f32; 2] {
        let Some(r) = self.retreat else {
            return look;
        };
        let Some(c) = view.survive.cover else {
            return look;
        };
        // Backing off, the sights stay up on an enemy in sight; dropped, the
        // retreating side scored fewer kills (bot-combat.md 5).
        if view.enemy.is_none() {
            cmd.buttons &= !msg::BUTTON_ADS;
        }
        if !at_cover(view.origin, c) {
            (cmd.forward, cmd.right) = rekey(path.0, path.1, path_yaw, look[1]);
            (cmd.up, cmd.wbuttons) = (path.2, path.3);
            return look;
        }
        cmd.forward = 0;
        cmd.right = 0;
        if view.enemy.is_some() {
            return look;
        }
        cmd.up = -127;
        cmd.wbuttons |= msg::WBUTTON_CROUCH;
        let loaded = view.loaded >> view.weapon.min(63) & 1 == 1;
        if view.clip >= 0
            && view.clip < view.survive.clip_size
            && view.busy_ms == 0
            && loaded
            && self.switch_to.is_none()
        {
            cmd.wbuttons |= msg::WBUTTON_RELOAD;
        }
        let (p, y) = aim_angles(view, r.threat);
        self.turn_toward(view, [p, y])
    }

    /// Starts, carries on or ends the retreat, at the end of the tick's
    /// think: the sidearm has had its say on a dry clip by then.
    pub(super) fn retreat_update(&mut self, view: &BotView) {
        if !self.retreat_on || !self.shoot || view.on_ladder || view.deployed {
            self.retreat = None;
            return;
        }
        // Healed past the last wound: the next one counts afresh.
        if view.survive.health > self.hurt_at {
            self.hurt_at = i32::MAX;
        }
        let loaded = view.loaded >> view.weapon.min(63) & 1 == 1;
        let dry = view.clip == 0 && loaded;
        // Loaded again, or nothing left to load: the next dry clip counts
        // afresh.
        if !dry {
            self.dry_had = false;
        }
        let duty = self.duty_bound(view);
        let seen = view.enemy.and(view.survive.enemy_eye);
        if let Some(r) = self.retreat.as_mut() {
            r.ticks += 1;
            r.cover = view.survive.cover;
            if let Some(eye) = seen {
                r.threat = eye;
            }
            if r.cover.is_some_and(|c| at_cover(view.origin, c)) {
                r.held += 1;
            }
            let done = match r.why {
                // A health item near ends the wait: the heal goal takes over.
                Why::Hurt => {
                    r.held >= HOLD_TICKS
                        || (r.held > 0 && view.enemy.is_none() && view.survive.heal.is_some())
                }
                Why::Dry => (view.clip > 0 && view.busy_ms == 0) || !loaded,
            };
            if duty || done || r.cover.is_none() || r.ticks >= RETREAT_TICKS {
                self.retreat = None;
            }
            return;
        }
        let Some(threat) = seen else {
            return;
        };
        if duty || self.switch_to.is_some() {
            return;
        }
        let v = &view.survive;
        let hurt = v.health * 100 < v.max_health * HURT_PCT && v.health < self.hurt_at;
        let why = if hurt {
            // Once per wound: a bot that held its cover fights on until it
            // is hit again.
            self.hurt_at = v.health;
            Why::Hurt
        } else if dry && !self.dry_had {
            // Once per dry clip: one that found no cover reloads where it
            // stands rather than asking again every other tick.
            self.dry_had = true;
            Why::Dry
        } else {
            return;
        };
        self.retreat = Some(Retreat {
            why,
            threat,
            cover: None,
            ticks: 0,
            held: 0,
        });
    }
}

/// The nearest node along the graph from `from`, within [`COVER_REACH`],
/// whose standing eye and chest the threat's eye at `threat` cannot see
/// (the shot trace, props included), nearer the bot than the threat and no
/// nearer the threat than [`COVER_MIN`]. `held`, the cover found before,
/// stands while the threat still cannot see it.
pub(crate) fn cover_spot(
    g: &NavGraph,
    world: &CollisionWorld,
    from: [f32; 3],
    threat: [f32; 3],
    held: Option<[f32; 3]>,
) -> Option<[f32; 3]> {
    let (from, threat) = (Vec3::from(from), Vec3::from(threat));
    let hidden = |n: Vec3| {
        [
            vcod_common::pmove::VIEW_STAND,
            vcod_common::pmove::VIEW_STAND * (2.0 / 3.0),
        ]
        .iter()
        .all(|h| world.shot_trace(threat, n + Vec3::Z * *h).fraction < 1.0)
    };
    let far = |n: Vec3| n.distance(threat) > COVER_MIN;
    if let Some(h) = held.map(Vec3::from)
        && far(h)
        && hidden(h)
    {
        return Some(h.into());
    }
    let start = g.nearest(from.into())?;
    let mut cost = vec![f32::INFINITY; g.nodes.len()];
    let mut open = BinaryHeap::new();
    cost[start as usize] = 0.0;
    open.push((Reverse(0u32), start));
    let mut tests = 0;
    while let Some((Reverse(c), n)) = open.pop() {
        let c = c as f32;
        if c > cost[n as usize] + 1.0 {
            continue;
        }
        let p = g.nodes[n as usize];
        if far(p) && p.distance(from) < p.distance(threat) {
            if hidden(p) {
                return Some(p.into());
            }
            tests += 1;
            if tests >= COVER_TESTS {
                return None;
            }
        }
        for &m in &g.edges[n as usize] {
            if g.risky.contains(&(n, m)) {
                continue;
            }
            let next = cost[n as usize] + p.distance(g.nodes[m as usize]);
            if next < cost[m as usize] && next <= COVER_REACH {
                cost[m as usize] = next;
                open.push((Reverse(next as u32), m));
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::super::tests::{engaged, re_view};
    use super::*;
    use vcod_common::net::msg::{WBUTTON_CROUCH, WBUTTON_RELOAD};

    /// An engaged bot at 30 health, the enemy's eye 20 over his chest.
    fn hurt(seed: u64) -> (Bot, BotView) {
        let (bot, mut v) = engaged(seed);
        v.survive.health = 30;
        v.survive.clip_size = 8;
        v.survive.enemy_eye = Some([100.0, 0.0, 144.0]);
        (bot, v)
    }

    #[test]
    fn a_hurt_bot_in_a_fight_goes_for_cover_then_holds_it() {
        let (mut bot, mut v) = hurt(1);
        assert!(matches!(bot.goal(&v), Goal::To(_)));
        bot.think(&v);
        assert_eq!(bot.retreating(), 1);
        assert_eq!(bot.cover_query(), Some(([100.0, 0.0, 144.0], None)));
        let cover = [-300.0, 0.0, 64.0];
        v.survive.cover = Some(cover);
        assert_eq!(bot.goal(&v), Goal::To(cover));
        // Backing off along the path with the aim on the enemy: the path
        // runs -x, the view +x, so the keys pull back.
        v.waypoint = Some(cover);
        let cmd = bot.think(&v);
        assert!(cmd.forward < 0, "{cmd:?}");
        // At the cover with nothing in sight: still, crouched, the clip
        // topped up.
        v.origin = cover;
        v.enemy = None;
        v.clip = 3;
        assert_eq!(bot.goal(&v), Goal::Hold);
        let cmd = bot.think(&v);
        assert_eq!((cmd.forward, cmd.right), (0, 0));
        assert_ne!(cmd.wbuttons & WBUTTON_CROUCH, 0);
        assert_ne!(cmd.wbuttons & WBUTTON_RELOAD, 0);
        // Held out, it plays on, and the same wound starts no second one.
        for _ in 0..HOLD_TICKS {
            bot.think(&v);
        }
        assert_eq!(bot.retreating(), 0);
        v.enemy = engaged(1).1.enemy;
        bot.think(&v);
        assert_eq!(bot.retreating(), 0, "retreated twice on one wound");
        v.survive.health = 20;
        bot.think(&v);
        assert_eq!(bot.retreating(), 1, "a fresh wound retreats again");
    }

    #[test]
    fn no_cover_means_fighting_on() {
        let (mut bot, v) = hurt(1);
        bot.think(&v);
        assert_eq!(bot.retreating(), 1);
        bot.think(&v);
        assert_eq!(bot.retreating(), 0);
        assert!(matches!(bot.goal(&v), Goal::To(e) if e == v.enemy.unwrap().origin));
    }

    #[test]
    fn a_dry_clip_retreats_until_the_reload_lands() {
        let (mut bot, mut v) = engaged(1);
        v.survive.enemy_eye = Some([100.0, 0.0, 144.0]);
        v.clip = 0;
        bot.think(&v);
        assert_eq!(bot.retreating(), 2);
        v.survive.cover = Some([-300.0, 0.0, 64.0]);
        v.busy_ms = 1500;
        bot.think(&v);
        assert_eq!(bot.retreating(), 2);
        v.clip = 8;
        v.busy_ms = 0;
        bot.think(&v);
        assert_eq!(bot.retreating(), 0);
    }

    #[test]
    fn a_dry_clip_with_no_cover_reloads_in_place_once() {
        let (mut bot, mut v) = engaged(1);
        v.survive.enemy_eye = Some([100.0, 0.0, 144.0]);
        v.clip = 0;
        v.busy_ms = 2000;
        bot.think(&v);
        assert_eq!(bot.retreating(), 2);
        // No cover found: the retreat ends and the same clip starts none.
        for _ in 0..20 {
            bot.think(&v);
            assert_eq!(bot.retreating(), 0, "restarted on one dry clip");
        }
        // The reload lands; the next dry clip retreats again.
        (v.clip, v.busy_ms) = (8, 0);
        bot.think(&v);
        v.clip = 0;
        bot.think(&v);
        assert_eq!(bot.retreating(), 2);
    }

    #[test]
    fn a_carrier_does_not_retreat() {
        let mut v = re_view(ObjRole::Attack);
        let (mut bot, e) = hurt(1);
        v.enemy = e.enemy;
        v.survive = e.survive;
        v.re.as_mut().unwrap().objectives[0].mine = true;
        bot.think(&v);
        assert_eq!(bot.retreating(), 0);
        v.re.as_mut().unwrap().objectives[0].mine = false;
        v.re.as_mut().unwrap().objectives[0].pickup = None;
        bot.think(&v);
        assert_eq!(bot.retreating(), 1);
    }

    /// The item walked to is kept while it lies in reach, nearer items or
    /// not.
    #[test]
    fn a_hurt_bot_keeps_to_the_health_item_it_walks_to() {
        let (_, v) = hurt(1);
        let (far, near) = ([600.0, 0.0, 64.0], [300.0, 0.0, 64.0]);
        assert_eq!(nearest_pack(&v, &[far, near], None), Some(near));
        assert_eq!(nearest_pack(&v, &[far, near], Some(far)), Some(far));
        let over = [600.0, 0.0, 64.0 + HEAL_HEIGHT + 10.0];
        assert_eq!(nearest_pack(&v, &[over, near], None), Some(near));
        assert_eq!(
            nearest_pack(&v, &[over, near], Some(over)),
            Some(over),
            "any height"
        );
        assert_eq!(
            nearest_pack(&v, &[near], Some(far)),
            Some(near),
            "picked up"
        );
    }

    #[test]
    fn a_hurt_bot_out_of_a_fight_walks_to_a_health_item_near() {
        let (mut bot, mut v) = hurt(1);
        let pack = [400.0, 300.0, 64.0];
        assert_eq!(
            nearest_pack(&v, &[pack, [5000.0, 0.0, 64.0]], None),
            Some(pack)
        );
        v.survive.heal = Some(pack);
        // In a fight the fight comes first.
        assert_ne!(bot.goal(&v), Goal::To(pack));
        v.enemy = None;
        assert_eq!(bot.goal(&v), Goal::To(pack));
        // Not hurt enough: no detour.
        v.survive.health = 90;
        assert_eq!(nearest_pack(&v, &[pack], None), None);
        bot.set_retreat(false);
        v.survive.health = 30;
        assert_ne!(bot.goal(&v), Goal::To(pack));
    }

    #[test]
    fn a_bot_with_the_retreat_off_fights_on() {
        let (mut bot, v) = hurt(1);
        bot.set_retreat(false);
        bot.think(&v);
        assert_eq!(bot.retreating(), 0);
    }
}
