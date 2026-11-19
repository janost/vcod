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
pub(super) const AWAY_DEG: f32 = 30.0;
pub(super) const AWAY_TICKS: u32 = 16;
/// Ticks past the hold a frag waits for its pin ([`BotView::grenade_ready`])
/// before it is put back: the raise and the pullback take 17 at most.
pub(super) const PIN_WAIT_TICKS: u32 = 30;
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
pub(super) const COVER_THROW_TICKS: u32 = 20;
const LOST_THROW_MIN: u32 = 4;
pub(super) const LOST_THROW_TICKS: u32 = 60;
/// ...if he was slower than this when lost, units/s: a frag at the spot a
/// runner left lands where he no longer is.
const LOST_STILL: f32 = 100.0;
