//! The grenade stage machine: when a frag goes, where it lobs, the cook and the switch back.

use super::*;

/// A frag goes at a target this close and no further, and no nearer than
/// `GRENADE_MIN`, inside which the 350-unit blast takes a third or more of
/// the thrower's health too; before a lob is probed the view turns
/// `LOB_DEG` over the line to it at the longest range.
pub(super) const GRENADE_RANGE: f32 = 800.0;
pub(super) const GRENADE_MIN: f32 = 250.0;
const LOB_DEG: f32 = 25.0;
/// A probed lob's frag goes off this near its target or is not thrown;
/// a throw is flown in full only when its arc, unhindered, comes down
/// this near the range (`Server::lob_probe`).
pub(crate) const LOB_MISS: f32 = 150.0;
pub(crate) const LOB_REACH_SLACK: f32 = 250.0;
/// A teammate is kept out of the blast for this many seconds of his pace,
/// and a body out of the throw's early arc by this much all round, as it
/// may step in before the release (bot-combat.md 12).
pub(crate) const MATE_LEAD_S: u32 = 2;
pub(crate) const LOB_BODY_GROW: f32 = 24.0;
/// A throw turned away from a close enemy with no clear lob found goes
/// this high (wire pitch, up). A release waits for a clear lob and the view
/// within `LOB_SLACK` of it, else within `AWAY_DEG` of the throw's yaw,
/// `AWAY_TICKS` at most.
const AWAY_PITCH: f32 = -30.0;
const AWAY_DEG: f32 = 30.0;
const AWAY_TICKS: u32 = 16;
/// Ticks past the hold a frag waits for its pin ([`BotView::grenade_ready`])
/// before it is put back: the raise and the pullback take 17 at most.
const PIN_WAIT_TICKS: u32 = 30;
/// A UO cook's hold: `COOK_TICKS` plus a draw under `COOK_SPREAD`, the
/// sight button down from `COOK_SPOON_AFTER` ticks in (`Bot::think_grenade`).
const COOK_TICKS: u32 = 24;
const COOK_SPREAD: u32 = 10;
const COOK_SPOON_AFTER: u32 = 4;
/// The most of a UO fuse a bot's cook burns before the release: the
/// longest spooned hold and the longest wait for the view after it.
pub(crate) const COOK_MOST_MS: i32 =
    ((COOK_TICKS + COOK_SPREAD - 1 - COOK_SPOON_AFTER + AWAY_TICKS) * FRAME_MS as u32) as i32;
/// What the spooned hold burns on the mean, the view already on the lob:
/// where the probe looks for the burst.
pub(crate) const COOK_MEAN_MS: i32 =
    ((COOK_TICKS + COOK_SPREAD / 2 - COOK_SPOON_AFTER) * FRAME_MS as u32) as i32;
/// A turned-away throw is probed at a spot this far off along its yaw.
const AWAY_REACH: f32 = 400.0;
/// A release on a probed lob waits until the view is this near it in
/// pitch and yaw, under a lob step of the probe.
pub(crate) const LOB_SLACK: f32 = 2.0;
/// A death this near a charge was in its room or the bunker over it: the
/// room is held ([`Bot::room_frag`]).
const BAS_ROOM: f32 = 300.0;

/// Whom a throw under way goes at ([`Bot::throw_enemy`]).
#[derive(Clone, Copy)]
pub(super) enum ThrowFor {
    /// Any enemy in sight (a stage set by hand in a test).
    Any,
    /// The enemy in this slot while in sight, else where he was last seen.
    Enemy(usize),
    /// A lost enemy's spot, whoever comes into sight.
    Spot,
}

pub(super) enum Stage {
    Wander,
    ToGrenade,
    /// The pullback held `left` more ticks; from `spoon` ticks left the
    /// sight button is held too, which starts a UO fuse.
    Cook {
        left: u32,
        spoon: u32,
    },
    /// The throw is out; hold the rifle byte until `ps.weapon` follows.
    BackToRifle {
        weapon: u8,
    },
}

impl Bot {
    /// Whether the server should probe [`BotView::lob`] this tick: a throw
    /// may start, or one is in hand.
    pub(crate) fn wants_lob(&self, view: &BotView) -> bool {
        self.shoot
            && view.grenade.is_some()
            && self.throw_target(view).is_some()
            && (self.grenade_cooldown <= 1 || !matches!(self.stage, Stage::Wander))
    }

    /// What a throw goes at: a turned-away throw's spot `AWAY_REACH` off
    /// along its yaw; with a throw under way the enemy in sight, else
    /// where it was last seen; before one, [`Bot::throw_spot`]. A frag let
    /// go along the view once the enemy had stepped behind cover struck
    /// the cover and rolled back.
    pub(crate) fn throw_target(&self, view: &BotView) -> Option<[f32; 3]> {
        if let Some(yaw) = self.throw_away {
            let (s, c) = yaw.to_radians().sin_cos();
            let [x, y, z] = view.origin;
            return Some([x + c * AWAY_REACH, y + s * AWAY_REACH, z]);
        }
        if matches!(self.stage, Stage::Wander) {
            return self.throw_spot(view);
        }
        self.throw_enemy(view).map(|e| e.origin).or(self.throw_at)
    }

    /// The enemy in sight a throw under way follows: the one it started
    /// at, none for a lost enemy's spot. A frag held for one enemy that
    /// went at another who came into sight mid-hold left on the old view,
    /// off the new lob, and killed its thrower.
    pub(super) fn throw_enemy(&self, view: &BotView) -> Option<EnemyView> {
        view.enemy.filter(|e| match self.throw_for {
            ThrowFor::Any => true,
            ThrowFor::Enemy(slot) => e.slot == slot,
            ThrowFor::Spot => false,
        })
    }

    /// Where a throw would start for, between [`GRENADE_MIN`] and
    /// [`GRENADE_RANGE`] off: an enemy in sight no round has gone at for
    /// [`COVER_THROW_TICKS`] (behind cover, or never lined up), else the
    /// spot a seen enemy was lost at within [`LOST_THROW_TICKS`], standing
    /// or slow ([`LOST_STILL`]). An enemy the gun can hit takes the gun: a
    /// frag at one in the open made 5 of 2908 kills (bot-combat.md 6.5).
    pub(super) fn throw_spot(&self, view: &BotView) -> Option<[f32; 3]> {
        let at = match view.enemy {
            Some(e) => (self.futile >= COVER_THROW_TICKS).then_some(e.origin)?,
            None => self
                .recall
                .filter(|r| r.seen && self.seen_speed < LOST_STILL)
                .filter(|r| {
                    let lost = self.skill.memory_ticks.saturating_sub(r.ticks);
                    (LOST_THROW_MIN..=LOST_THROW_TICKS).contains(&lost)
                })
                .map(|r| r.at)
                .or_else(|| self.room_frag(view))?,
        };
        let d = dist_sq(view.origin, at);
        (d > GRENADE_MIN * GRENADE_MIN && d < GRENADE_RANGE * GRENADE_RANGE).then_some(at)
    }

    /// The charge a `bas` bot makes for to plant or defuse, when a
    /// teammate died in its room of late ([`BotView::fallen`]) and this
    /// life has thrown no frag into it yet: a defender set in the room
    /// killed attackers arriving one at a time (mp_kursk, bot-objectives.md
    /// 7.2), so the frag goes in before the bot does.
    pub(super) fn room_frag(&self, view: &BotView) -> Option<[f32; 3]> {
        if !matches!(view.uo, Some(UoView::Bas(_))) {
            return None;
        }
        let Some(ObjTarget::Defuse { bomb, .. }) = self.objective_target(view) else {
            return None;
        };
        let at = bomb.origin;
        (self.room_thrown != Some(at)
            && view
                .fallen
                .iter()
                .any(|f| dist_sq(*f, at) < BAS_ROOM * BAS_ROOM))
        .then_some(at)
    }

    /// The view through a throw: up over the enemy in sight, or where it
    /// was last seen, by a lob that grows with the range, else held where
    /// it is. Zero angles here would throw the frag wherever `delta_angles`
    /// points, often into a wall at the thrower's feet.
    pub(super) fn lob_angles(&self, view: &BotView) -> [i32; 3] {
        let aim = match self.throw_target(view) {
            // The throw leaves along the aim, the view and the gun's
            // offset off it, as a round does ([`BotView::gun_offset`]).
            Some(_) if let Some(aim) = view.lob => {
                let g = view.gun_offset;
                self.turn_toward(view, [aim[0] - g[0], aim[1] - g[1]])
            }
            // Turning away to throw it clear, no lob found yet: up and behind.
            _ if let Some(yaw) = self.throw_away => self.turn_toward(view, [AWAY_PITCH, yaw]),
            Some(at) => {
                let (pitch, yaw) = aim_angles(view, at);
                let range = dist_sq(view.origin, at).sqrt();
                let lob = (range / GRENADE_RANGE * LOB_DEG).clamp(5.0, LOB_DEG);
                self.turn_toward(view, [pitch - lob, yaw])
            }
            None => [yaw_diff(view.view[0], 0.0), view.view[1]],
        };
        cmd_angles(view, aim)
    }

    pub(super) fn think_grenade(&mut self, view: &BotView, mut cmd: UserCmd) -> UserCmd {
        if let Some(g) = view.grenade {
            cmd.weapon = g;
            if view.weapon == g {
                self.stage = if view.grenade_cooks {
                    // UO's fuse starts at the sight button's edge once the
                    // pullback runs (coduo-movement.md 12): pressed 200 ms
                    // in and held 1 to 1.5 s, which leaves 2.5 s of a 4 s
                    // fuse for the flight.
                    let left = COOK_TICKS + (self.rand() % COOK_SPREAD as i32) as u32;
                    Stage::Cook {
                        left,
                        spoon: left - COOK_SPOON_AFTER,
                    }
                } else {
                    // No cook exists in 1.1 (combat doc 1.11): the release
                    // throws, so the hold is only long enough to look like
                    // a throw, not a cook.
                    let left = 6 + (self.rand() % 8) as u32;
                    Stage::Cook { left, spoon: 0 }
                };
            }
            return cmd;
        }
        self.stage = Stage::Wander;
        cmd
    }

    pub(super) fn think_cook(
        &mut self,
        view: &BotView,
        mut cmd: UserCmd,
        left: u32,
        spoon: u32,
    ) -> UserCmd {
        if let Some(g) = view.grenade {
            cmd.weapon = g;
        }
        if view.weapon != cmd.weapon || !view.playing {
            return cmd;
        }
        // An enemy closed inside the blast during the hold: the frag goes
        // the other way, high, once the view has come round. A UO frag
        // cooks on in the hand meanwhile, with 2.5 s and more left.
        let close = view
            .enemy
            .is_some_and(|e| dist_sq(view.origin, e.origin) < GRENADE_MIN * GRENADE_MIN);
        if left <= 1
            && self.throw_away.is_none()
            && let Some(e) = view.enemy.filter(|_| close)
        {
            self.throw_away = Some(aim_angles(view, e.origin).1 + 180.0);
        }
        // The release waits for the view to come round onto the throw: a
        // new target late in the hold turns it at the usual rate, and a
        // frag let go on the old heading hit the wall beside the thrower.
        // A probed lob is waited for in pitch too: its flight is the only
        // one known clear, and one let go 16 degrees over it struck the
        // wall above the enemy and rolled back past the thrower.
        let target = self.throw_target(view);
        let lob = view.lob.filter(|_| target.is_some());
        let yaw = lob
            .map(|l| l[1])
            .or(self.throw_away)
            .or_else(|| target.map(|at| aim_angles(view, at).1));
        // No clear lob at the target yet: hold on for one.
        let blocked = target.is_some() && view.lob.is_none();
        let off = match lob {
            Some(l) => {
                let g = view.gun_offset;
                yaw_diff(l[0], yaw_diff(view.view[0] + g[0], 0.0)).abs() > LOB_SLACK
                    || yaw_diff(l[1], view.view[1] + g[1]).abs() > LOB_SLACK
            }
            None => yaw.is_some_and(|y| yaw_diff(y, view.view[1]).abs() > AWAY_DEG),
        };
        // The pin is not out yet, or the pullback's `holdFireTime` not run:
        // the trigger stays down. A release before it waits for the pin
        // in pmove, and the next tick's switch to the rifle cancelled the
        // throw: 1.1's hold of 6 to 13 ticks from the switch is shorter
        // than the 0.25 s raise and 0.6 s hold, and 30 of 31 frags never
        // left the hand (bot-combat.md 12).
        let pinned = left <= 1 && !view.grenade_ready;
        if pinned {
            self.pin_wait += 1;
        }
        let turning = left <= 1 && !pinned && self.held_over < AWAY_TICKS && (blocked || off);
        if turning {
            self.held_over += 1;
        }
        // Still no clear lob, or the view off it, once the wait is out: a
        // frag with no fuse running is put back, the switch cancelling the
        // pullback with the clip unspent (cod11-combat.md 1.14). One let go
        // anyway met the wall the probe saw and came back at the thrower.
        let put_back = if pinned {
            self.pin_wait > PIN_WAIT_TICKS
        } else {
            left <= 1 && !turning && (blocked || off) && !view.grenade_cooks
        };
        if put_back {
            self.end_throw();
            cmd.weapon = self.rifle;
            self.stage = Stage::BackToRifle { weapon: self.rifle };
            return cmd;
        }
        if left > 1 || turning || pinned {
            cmd.buttons = msg::BUTTON_ATTACK;
            if left <= spoon {
                cmd.buttons |= msg::BUTTON_ADS;
            }
            self.stage = Stage::Cook {
                left: left.saturating_sub(1).max(1),
                spoon,
            };
        } else {
            self.end_throw();
            // The release; the next stage walks the byte back to the rifle.
            self.throws += 1;
            self.want_yell(yell::Yell::Grenade);
            log::debug!(
                "{} throws from {:?}, enemy {:?} at {:.0}, view {:?}",
                self.name,
                view.origin.map(|x| x.round()),
                view.enemy.map(|e| e.origin.map(|x| x.round())),
                view.enemy
                    .map_or(-1.0, |e| dist_sq(view.origin, e.origin).sqrt()),
                view.view.map(|x| x.round()),
            );
            self.stage = Stage::BackToRifle { weapon: self.rifle };
        }
        cmd
    }

    /// A throw is out, put back or cut short by a death: nothing of it
    /// carries over to the next.
    pub(super) fn end_throw(&mut self) {
        self.throw_away = None;
        self.held_over = 0;
        self.pin_wait = 0;
        self.throw_at = None;
        self.throw_for = ThrowFor::Any;
    }

    pub(super) fn think_back(&mut self, view: &BotView, mut cmd: UserCmd, weapon: u8) -> UserCmd {
        cmd.weapon = weapon;
        if view.weapon == weapon {
            self.stage = Stage::Wander;
        }
        cmd
    }
}
/// An enemy in sight this many ticks with no round at him is thrown at;
/// one lost from sight between these ticks ago is thrown at where he went
/// ([`Bot::throw_spot`]).
const COVER_THROW_TICKS: u32 = 20;
const LOST_THROW_MIN: u32 = 4;
const LOST_THROW_TICKS: u32 = 60;
/// ...if he was slower than this when lost, units/s: a frag at the spot a
/// runner left lands where he no longer is.
const LOST_STILL: f32 = 100.0;

/// A frag throw as CoD2's and CoD4's lob flies it ([`lob`]): from the eye
/// `from` at `projectileSpeed` along the view plus `projectileSpeedUp` up,
/// its `fuseTime` in ms, its `explosionRadius`, and the thrower's origin it
/// must go off that far from.
pub(crate) struct Lob {
    pub(crate) from: glam::Vec3,
    pub(crate) thrower: glam::Vec3,
    pub(crate) speed: f32,
    pub(crate) speed_up: f32,
    pub(crate) fuse_ms: i32,
    pub(crate) radius: f32,
}

/// [`BotView::lob`] for `throw` at `target` over `w`: of the throws from
/// 10 degrees down to 85 up, every 5, the one whose frag comes down
/// nearest `target`, within [`LOB_MISS`], and not within its radius of the
/// thrower. The flight is `weapon_grenadelauncher_fire`'s under
/// `TR_GRAVITY`'s 800, up to where it first meets the world: a bounce is
/// not flown. As (pitch, yaw), wire degrees.
pub(crate) fn lob(
    w: &vcod_common::collision::CollisionWorld,
    throw: &Lob,
    target: glam::Vec3,
) -> Option<[f32; 2]> {
    const STEP_S: f32 = 0.05;
    let to = target - throw.from;
    let yaw = to.y.atan2(to.x).to_degrees();
    // The whole fuse, however short: a frag that goes off in the air near
    // the thrower is not thrown.
    let fuse_s = throw.fuse_ms as f32 / 1000.0;
    let mut best: Option<([f32; 2], f32)> = None;
    for up in (-2..=17).map(|i| i as f32 * 5.0) {
        let (sp, cp) = up.to_radians().sin_cos();
        let (sy, cy) = yaw.to_radians().sin_cos();
        let mut v = glam::Vec3::new(cp * cy, cp * sy, sp) * throw.speed;
        v.z += throw.speed_up;
        let mut at = throw.from;
        let mut t = 0.0;
        let mut landed = None;
        while t < fuse_s {
            let next = at + v * STEP_S - glam::Vec3::Z * 400.0 * STEP_S * STEP_S;
            v.z -= 800.0 * STEP_S;
            let tr = w.shot_trace(at, next);
            if tr.fraction < 1.0 {
                landed = Some(at + (next - at) * tr.fraction);
                break;
            }
            at = next;
            t += STEP_S;
        }
        let burst = landed.unwrap_or(at);
        if burst.distance(throw.thrower) <= throw.radius {
            continue;
        }
        let miss = burst.distance(target);
        if miss < LOB_MISS && best.is_none_or(|(_, m)| miss < m) {
            best = Some(([-up, yaw], miss));
        }
    }
    best.map(|(aim, _)| aim)
}

/// Whether one of `mates` (team, origin) on `mine`'s side stands within
/// `radius` of `target`: with friendly fire on no frag goes there.
pub(crate) fn mate_in_blast(
    mine: Option<i32>,
    mates: impl IntoIterator<Item = (i32, glam::Vec3)>,
    target: glam::Vec3,
    radius: f32,
) -> bool {
    mates
        .into_iter()
        .any(|(t, at)| same_team(mine, Some(t)) && at.distance(target) < radius)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bots::tests::*;
    use vcod_common::net::msg::{BUTTON_ATTACK, BUTTON_USE};

    /// An intermission or a spectator spell drops a throw under way, as a
    /// death does: the next level does not start on last level's frag
    /// (BUG-135).
    #[test]
    fn a_throw_under_way_ends_when_the_bot_stops_playing() {
        let mut bot = Bot::new("allies", true, 1);
        bot.stage = Stage::Cook { left: 5, spoon: 2 };
        bot.throw_at = Some([100.0, 0.0, 0.0]);
        let mut v = view();
        v.playing = false;
        bot.think(&v);
        assert!(matches!(bot.stage, Stage::Wander));
        assert_eq!(bot.throw_at, None);
    }

    /// UO's frag cooks: the sight button goes down a few ticks into the
    /// pullback, and the hold stays well inside the 4 s fuse.
    #[test]
    fn a_uo_frag_is_spooned_after_the_pullback() {
        let mut bot = Bot::new("allies", true, 7);
        let mut v = view();
        v.grenade_cooks = true;
        v.weapon = 6;
        bot.stage = Stage::ToGrenade;
        let mut ticks = Vec::new();
        for _ in 0..60 {
            let cmd = bot.think(&v);
            ticks.push(cmd.buttons);
            if matches!(bot.stage, Stage::BackToRifle { .. }) {
                break;
            }
        }
        let held = ticks
            .iter()
            .filter(|b| *b & msg::BUTTON_ATTACK != 0)
            .count();
        let spoon = ticks.iter().position(|b| b & msg::BUTTON_ADS != 0);
        assert!(
            spoon.is_some_and(|i| i >= 3),
            "spooned after the pullback: {ticks:?}"
        );
        assert!(held * 50 < 2000, "held {held} ticks");
    }

    #[test]
    fn a_bot_throw_switches_to_the_frag_cooks_and_throws() {
        let mut bot = Bot::new("allies", true, 0x1234_5678);
        let mut v = view();
        v.enemy = Some(EnemyView {
            slot: 3,
            origin: [300.0, 0.0, 40.0],
            velocity: [0.0; 3],
        });
        // The server's probe found the flight clear.
        v.lob = Some([-12.0, 0.0]);
        // Grind until the grenade machine starts; the cooldown is short with
        // a live enemy this close.
        let mut switched = None;
        let mut held = false;
        let mut released = false;
        for _ in 0..600 {
            let cmd = bot.think(&v);
            // The real sim follows the byte; the fake view does the same.
            v.weapon = cmd.weapon;
            if cmd.weapon == 6 {
                switched = Some(cmd.weapon);
                held |= cmd.buttons & BUTTON_ATTACK != 0;
            }
            if held && cmd.weapon == 6 && cmd.buttons & BUTTON_ATTACK == 0 {
                released = true;
            }
        }
        assert!(switched.is_some(), "the bot never switched to the frag");
        assert!(held, "the pullback never held the trigger");
        assert!(released, "the cooked throw never released");
    }

    /// The hold runs out before the raise and the pullback's
    /// `holdFireTime` have: the trigger stays down until the pin is out,
    /// and only then comes up. Let go early, the throw waited for the pin
    /// and the next tick's switch to the rifle cancelled it.
    #[test]
    fn a_frag_is_held_until_its_pin_is_out() {
        let mut bot = Bot::new("allies", true, 1);
        let mut v = view();
        v.weapon = 6;
        v.view = [-12.0, 0.0, 0.0];
        v.enemy = Some(EnemyView {
            slot: 3,
            origin: [300.0, 0.0, 40.0],
            velocity: [0.0; 3],
        });
        v.lob = Some([-12.0, 0.0]);
        v.grenade_ready = false;
        bot.stage = Stage::Cook { left: 1, spoon: 0 };
        for _ in 0..10 {
            let cmd = bot.think(&v);
            assert_ne!(cmd.buttons & BUTTON_ATTACK, 0, "let go on the pin");
            assert_eq!(cmd.weapon, 6);
        }
        v.grenade_ready = true;
        bot.take_yell();
        let cmd = bot.think(&v);
        assert_eq!(cmd.buttons & BUTTON_ATTACK, 0, "held past the pin");
        assert_eq!(bot.throws(), 1);
        assert_eq!(bot.take_yell(), Some(yell::Yell::Grenade));
    }

    /// The view on the lob with the gun kicked 17 degrees up off it (a
    /// blast's knock): the release waits until the view plus the offset is
    /// on the lob. UO mp_cassino seed 13 let go there, the frag went up
    /// at 87 degrees and came down on its thrower.
    #[test]
    fn a_throw_waits_for_the_aim_not_the_view() {
        let mut bot = Bot::new("allies", true, 1);
        let mut v = view();
        v.weapon = 6;
        v.view = [-70.0, 173.0, 0.0];
        v.gun_offset = [-17.0, 0.0];
        v.enemy = Some(EnemyView {
            slot: 3,
            origin: [-300.0, 40.0, 40.0],
            velocity: [0.0; 3],
        });
        v.lob = Some([-70.0, 173.0]);
        bot.stage = Stage::Cook { left: 1, spoon: 0 };
        let cmd = bot.think(&v);
        assert_ne!(cmd.buttons & BUTTON_ATTACK, 0, "let go off the aim");
        let pitch = cmd.angles[0] as f32 / ANGLE2SHORT;
        assert!(pitch > -70.0, "the view goes down under the kick: {pitch}");
        v.view = [-53.0, 173.0, 0.0];
        let cmd = bot.think(&v);
        assert_eq!(cmd.buttons & BUTTON_ATTACK, 0, "held on the aim");
    }

    /// A pin that never comes out (the raise refused) puts the frag back.
    #[test]
    fn a_frag_whose_pin_never_comes_out_is_put_back() {
        let mut bot = Bot::new("allies", true, 1);
        bot.rifle = 10;
        let mut v = view();
        v.weapon = 6;
        v.view = [-12.0, 0.0, 0.0];
        v.enemy = Some(EnemyView {
            slot: 3,
            origin: [300.0, 0.0, 40.0],
            velocity: [0.0; 3],
        });
        v.lob = Some([-12.0, 0.0]);
        v.grenade_ready = false;
        bot.stage = Stage::Cook { left: 1, spoon: 0 };
        for _ in 0..=PIN_WAIT_TICKS + 1 {
            bot.think(&v);
        }
        assert!(matches!(bot.stage, Stage::BackToRifle { weapon: 10 }));
        assert_eq!(bot.throws(), 0);
    }

    /// Through the throw the view turns toward the enemy and up over it,
    /// never to the zero angles that threw the frag wherever
    /// `delta_angles` pointed.
    #[test]
    fn a_throw_lobs_over_the_enemy() {
        let mut bot = Bot::new("allies", true, 1);
        let mut v = view();
        v.weapon = 6;
        // The enemy 400 ahead at eye height, the view level on it: the
        // pitch rises over the line, a bounded step a tick.
        v.enemy = Some(EnemyView {
            slot: 3,
            origin: [0.0, 400.0, 64.0 + vcod_common::pmove::VIEW_STAND],
            velocity: [0.0; 3],
        });
        bot.stage = Stage::Cook { left: 20, spoon: 0 };
        let cmd = bot.think(&v);
        let pitch = cmd.angles[0] as f32 / ANGLE2SHORT;
        let yaw = cmd.angles[1] as f32 / ANGLE2SHORT;
        assert!(pitch < -10.0, "pitched up: {pitch}");
        assert!((yaw - 90.0).abs() < 1.0, "on the enemy: {yaw}");
    }

    /// An enemy that closed inside the blast during the hold: the release
    /// waits while the view comes round, then the frag goes the other way,
    /// high (the self-kills of bot-objectives.md 6).
    #[test]
    fn a_throw_turns_away_from_an_enemy_inside_the_blast() {
        let mut bot = Bot::new("allies", true, 1);
        let mut v = view();
        v.weapon = 6;
        v.view = [0.0, 90.0, 0.0];
        v.enemy = Some(EnemyView {
            slot: 3,
            origin: [0.0, 100.0, 64.0 + vcod_common::pmove::VIEW_STAND],
            velocity: [0.0; 3],
        });
        bot.stage = Stage::Cook { left: 1, spoon: 0 };
        let mut released = None;
        for tick in 0..30 {
            let cmd = bot.think(&v);
            v.view = [
                cmd.angles[0] as f32 / ANGLE2SHORT,
                cmd.angles[1] as f32 / ANGLE2SHORT,
                0.0,
            ];
            if cmd.buttons & BUTTON_ATTACK == 0 {
                released = Some((tick, v.view));
                break;
            }
        }
        let (tick, at) = released.expect("never thrown");
        assert!(tick > 5, "released at once, at the enemy: {at:?}");
        assert!(yaw_diff(at[1], -90.0).abs() <= AWAY_DEG + 1.0, "{at:?}");
        assert!(at[0] < -20.0, "high: {at:?}");
    }

    /// The view still high off an earlier lob when the hold runs out: the
    /// release waits for the probed lob's pitch, not only its yaw. mp_cassino
    /// on UO, a bot let go at -22 with the probe's lob at -5.75; the frag
    /// struck the wall above the enemy and its blast killed the thrower.
    #[test]
    fn a_throw_waits_for_the_lobs_pitch() {
        let mut bot = Bot::new("allies", true, 1);
        let mut v = view();
        v.weapon = 6;
        v.view = [-22.0, 6.0, 0.0];
        v.enemy = Some(EnemyView {
            slot: 3,
            origin: [270.0, -6.0, 40.0],
            velocity: [0.0; 3],
        });
        v.lob = Some([-5.75, -1.35]);
        bot.stage = Stage::Cook { left: 1, spoon: 0 };
        let mut released = None;
        for tick in 0..30 {
            let cmd = bot.think(&v);
            if cmd.buttons & BUTTON_ATTACK == 0 {
                released = Some((tick, v.view));
                break;
            }
            v.view = [
                cmd.angles[0] as f32 / ANGLE2SHORT,
                cmd.angles[1] as f32 / ANGLE2SHORT,
                0.0,
            ];
        }
        let (tick, at) = released.expect("never thrown");
        assert!(tick > 0, "let go on the old pitch: {at:?}");
        assert!(yaw_diff(at[0], -5.75).abs() <= LOB_SLACK, "{at:?}");
        assert!(yaw_diff(at[1], -1.35).abs() <= LOB_SLACK, "{at:?}");
    }

    /// The enemy steps out of sight during the hold: the frag still goes
    /// at where it was, and only by a probed lob, held until one is found.
    /// mp_cassino on UO, a bot let go along its view the tick its enemy
    /// left sight; the frag struck the cover and killed the thrower.
    #[test]
    fn a_throw_at_an_enemy_gone_from_sight_waits_for_a_lob() {
        let mut bot = Bot::new("allies", true, 1);
        let mut v = view();
        v.weapon = 6;
        v.enemy = Some(EnemyView {
            slot: 3,
            origin: [300.0, 0.0, 40.0],
            velocity: [0.0; 3],
        });
        v.lob = Some([-10.0, 0.0]);
        bot.stage = Stage::Cook { left: 3, spoon: 0 };
        bot.think(&v);
        v.enemy = None;
        v.lob = None;
        assert_eq!(bot.throw_target(&v), Some([300.0, 0.0, 40.0]));
        assert!(bot.wants_lob(&v), "the server stops probing");
        for tick in 0..4 {
            let cmd = bot.think(&v);
            assert!(cmd.buttons & BUTTON_ATTACK != 0, "let go blind at {tick}");
        }
        v.lob = Some([-10.0, 0.0]);
        v.view = [-10.0, 0.0, 0.0];
        let cmd = bot.think(&v);
        assert_eq!(cmd.buttons & BUTTON_ATTACK, 0, "never thrown on the lob");
        assert_eq!(bot.throw_target(&v), None);
    }

    /// A 1.1 frag with no clear lob once the wait is out is put back: the
    /// cmd that lets the trigger go asks for the rifle, which cancels the
    /// pullback. A UO frag, its fuse running, is let go.
    #[test]
    fn a_throw_with_no_lob_is_put_back() {
        for cooks in [false, true] {
            let mut bot = Bot::new("allies", true, 1);
            bot.rifle = 10;
            let mut v = view();
            v.weapon = 6;
            v.grenade_cooks = cooks;
            v.enemy = Some(EnemyView {
                slot: 3,
                origin: [300.0, 0.0, 40.0],
                velocity: [0.0; 3],
            });
            bot.stage = Stage::Cook { left: 1, spoon: 0 };
            let cmd = (0..=AWAY_TICKS)
                .map(|_| bot.think(&v))
                .find(|c| c.buttons & BUTTON_ATTACK == 0)
                .expect("held for good");
            assert_eq!(cmd.weapon, if cooks { 6 } else { 10 }, "cooks {cooks}");
            assert_eq!(bot.throws(), u32::from(cooks), "cooks {cooks}");
        }
    }

    /// mp_cassino on UO, seed 1: the tick a throw started sent null angles,
    /// which turned the view onto `delta_angles` (45 degrees off the
    /// enemy), and the enemy left sight before any target was recorded.
    /// The cook ran out with no target and the frag went flat along that
    /// heading into the wall beside the thrower.
    #[test]
    fn the_tick_a_throw_starts_aims_and_records_the_enemy() {
        let mut v = view();
        v.view = [0.0, 10.0, 0.0];
        v.delta_angles = [0, 8192, 0];
        v.grenade = Some(6);
        v.lob = Some([-10.0, 0.0]);
        v.enemy = Some(EnemyView {
            slot: 3,
            origin: [400.0, 0.0, 40.0],
            velocity: [0.0; 3],
        });
        let mut bot = Bot::new("allies", true, 1);
        let cmd = (0..400)
            .find_map(|_| {
                let cmd = bot.think(&v);
                matches!(bot.stage, Stage::ToGrenade).then_some(cmd)
            })
            .expect("a throw starts");
        assert_eq!(cmd.weapon, 6);
        let yaw = (cmd.angles[1] + v.delta_angles[1]) as f32 / ANGLE2SHORT;
        assert!(yaw.abs() < 10.0, "the view turns onto the enemy: {yaw}");
        v.enemy = None;
        assert_eq!(bot.throw_target(&v), Some([400.0, 0.0, 40.0]));
    }

    /// On the rungs the hands stay empty: no throw starts there, and one
    /// whose switch never lands (the body grabbed a ladder mid-switch) is
    /// given up, the bot back on its keys. mp_ship's hold ladder held
    /// such a bot 3000 ticks (bot-combat.md 17.2).
    #[test]
    fn a_throw_whose_switch_never_lands_is_given_up() {
        let mut v = view();
        v.grenade = Some(6);
        v.lob = Some([-10.0, 0.0]);
        v.enemy = Some(EnemyView {
            slot: 3,
            origin: [400.0, 0.0, 40.0],
            velocity: [0.0; 3],
        });
        v.on_ladder = true;
        let mut bot = Bot::new("allies", true, 1);
        for _ in 0..400 {
            bot.think(&v);
            assert!(
                matches!(bot.stage, Stage::Wander),
                "a throw starts on the rungs"
            );
        }
        bot.stage = Stage::ToGrenade;
        v.weapon = 0;
        v.busy_ms = 0;
        for _ in 0..SWITCH_IDLE_TICKS {
            bot.think(&v);
            assert!(matches!(bot.stage, Stage::ToGrenade), "given up early");
        }
        bot.think(&v);
        assert!(
            matches!(bot.stage, Stage::Wander),
            "the switch waited for good"
        );
    }

    /// An enemy lost from sight a second ago is thrown at where he went;
    /// one gone longer than [`LOST_THROW_TICKS`] is not.
    #[test]
    fn a_bot_throws_where_an_enemy_was_lost() {
        let mut v = view();
        v.grenade = Some(6);
        v.lob = Some([-10.0, 0.0]);
        let mut bot = Bot::new("allies", true, 1);
        bot.grenade_cooldown = 0;
        let lost = |ticks| Recall {
            at: [400.0, 0.0, 40.0],
            ticks,
            seen: true,
        };
        bot.recall = Some(lost(bot.skill.memory_ticks - LOST_THROW_TICKS - 2));
        bot.think(&v);
        assert!(
            matches!(bot.stage, Stage::Wander),
            "a stale spot drew a throw"
        );
        bot.recall = Some(lost(bot.skill.memory_ticks - 20));
        let cmd = bot.think(&v);
        assert!(matches!(bot.stage, Stage::ToGrenade));
        assert_eq!(cmd.weapon, 6);
        assert_eq!(bot.throw_target(&v), Some([400.0, 0.0, 40.0]));
    }

    /// An enemy in sight takes the gun until no round has gone at him for
    /// [`COVER_THROW_TICKS`].
    #[test]
    fn a_bot_throws_at_an_enemy_only_once_the_gun_has_failed() {
        let mut v = view();
        v.lob = Some([-10.0, 0.0]);
        v.enemy = Some(EnemyView {
            slot: 3,
            origin: [400.0, 0.0, 40.0],
            velocity: [0.0; 3],
        });
        let mut bot = Bot::new("allies", true, 1);
        bot.grenade_cooldown = 0;
        bot.futile = COVER_THROW_TICKS - 2;
        assert_eq!(bot.throw_target(&v), None);
        bot.futile = COVER_THROW_TICKS;
        assert_eq!(bot.throw_target(&v), Some([400.0, 0.0, 40.0]));
        bot.think(&v);
        assert!(matches!(bot.stage, Stage::ToGrenade));
    }

    /// No clear lob at the enemy (a wall, a ceiling or a body in the way):
    /// no throw starts, whatever the dice.
    #[test]
    fn no_frag_goes_without_a_clear_lob() {
        let mut v = view();
        v.lob = None;
        v.enemy = Some(EnemyView {
            slot: 3,
            origin: [300.0, 0.0, 40.0],
            velocity: [0.0; 3],
        });
        let mut bot = Bot::new("allies", true, 0x1234_5678);
        for _ in 0..600 {
            let cmd = bot.think(&v);
            assert_ne!(cmd.weapon, 6, "switched to the frag");
        }
    }

    /// A death mid-grenade-phase must not strand the corpse: the stage
    /// machine must not swallow the respawn press.
    #[test]
    fn a_bot_killed_mid_grenade_phase_still_presses_use() {
        for start_stage in 0..3 {
            let mut bot = Bot::new("allies", true, 1);
            // Walk the machine into each non-wander stage by force.
            match start_stage {
                0 => {
                    bot.rifle = 10;
                    bot.stage = Stage::ToGrenade;
                }
                1 => {
                    bot.rifle = 10;
                    bot.stage = Stage::Cook { left: 5, spoon: 0 };
                }
                _ => {
                    bot.stage = Stage::BackToRifle { weapon: 10 };
                }
            }
            let mut v = view();
            v.dead = true;
            v.playing = false;
            let mut pressed = false;
            for _ in 0..RESPAWN_DELAY + RESPAWN_RETRY * 3 {
                if bot.think(&v).buttons & BUTTON_USE != 0 {
                    pressed = true;
                    break;
                }
            }
            assert!(
                pressed,
                "stage {start_stage}: a bot killed mid-grenade-phase never pressed use"
            );
        }
    }

    /// A frag aimed into a teammate's blast is refused, one into an enemy's
    /// or a `free` player's is not (BUG-058).
    #[test]
    fn a_lob_into_a_teammate_s_blast_is_refused() {
        use crate::game::script::{TEAM_ALLIES, TEAM_AXIS};
        use glam::Vec3;
        let target = Vec3::new(100.0, 0.0, 0.0);
        let near = Vec3::new(150.0, 0.0, 0.0);
        let far = Vec3::new(600.0, 0.0, 0.0);
        let mine = Some(TEAM_AXIS);
        assert!(mate_in_blast(mine, [(TEAM_AXIS, near)], target, 256.0));
        assert!(!mate_in_blast(mine, [(TEAM_AXIS, far)], target, 256.0));
        assert!(!mate_in_blast(mine, [(TEAM_ALLIES, near)], target, 256.0));
        assert!(!mate_in_blast(Some(3), [(3, near)], target, 256.0));
    }

    /// A lob flies its whole fuse and no more: one that goes off in the air
    /// within its radius of the thrower is not thrown.
    #[test]
    fn a_lob_flies_the_fuse_it_has() {
        let w = vcod_common::collision::test_world(&[]);
        let throw = |fuse_ms| Lob {
            from: glam::Vec3::new(0.0, 0.0, 60.0),
            thrower: glam::Vec3::ZERO,
            speed: 600.0,
            speed_up: 0.0,
            fuse_ms,
            radius: 256.0,
        };
        let target = glam::Vec3::new(500.0, 0.0, 0.0);
        assert!(lob(&w, &throw(3500), target).is_some());
        assert_eq!(lob(&w, &throw(300), target), None);
    }
}
