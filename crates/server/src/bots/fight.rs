//! The fight: tracking the target, the aim's turn, the trigger, footwork, the bipod and the weapon
//! swaps.

use super::*;

/// The view's pitch either way of level: past pmove's 16000-short clamp
/// (87.9) no further, and high enough for a lob that comes down 250 off
/// (`Server::lob_probe` tries 85).
pub(super) const PITCH_MOST: f32 = 85.0;
/// Half the width of what a shot has to land in (chest or head), units; the
/// fire cone is the angle it subtends at the target's range.
pub(super) const HIT_RADIUS: f32 = 10.0;
/// A semi-automatic with a `fireTime` at least this long works a bolt.
const BOLT_MS: i32 = 500;
/// How far off its line the trigger may go down, in fire cones: past one
/// the chance falls to none at this ([`Bot::trigger`]). An automatic
/// sprays early, a bolt rifle waits for its one round.
const REACH_AUTO: f32 = 4.5;
const REACH_SEMI: f32 = 3.0;
const REACH_BOLT: f32 = 2.0;
/// Inside this an automatic fires from the hip while its sights come up.
const AUTO_HIP_RANGE: f32 = 700.0;
/// Inside this a bolt rifle's footwork backs away from the target.
const BACK_OFF_RANGE: f32 = 300.0;
/// Past this a footwork spell may lie down; inside three quarters of it
/// the body gets up again.
const PRONE_RANGE: f32 = 900.0;

/// A hit [`BotView::hurt`] reports for this long, ms: the attacker is
/// preferred as a target and, out of sight, looked for along the hit.
pub(crate) const HURT_MS: i32 = 2000;
/// How long a hit from an unseen enemy turns the view toward it, ms.
pub(super) const HURT_LOOK_MS: i32 = 1500;
/// A target taken again after [`Bot::prior`] keeps its error times this;
/// one who just hit the bot starts at this share of `error_start`. Both
/// take a third and a half of the reaction delay.
pub(super) const REACQUIRE_ERROR: f32 = 1.25;
pub(super) const HIT_BY_ERROR: f32 = 0.75;

/// The enemy the bot is engaging and how settled its aim on it is.
pub(super) struct Target {
    pub(super) slot: usize,
    /// Ticks before the bot turns toward it at all.
    pub(super) react: u32,
    /// Aim error, units at the target.
    pub(super) error: f32,
    /// The old aim's error direction, (pitch, yaw), unit length; resampled
    /// every `wobble` ticks ([`Bot::human_aim`] off).
    dir: [f32; 2],
    wobble: u32,
    /// The human aim's state.
    hand: aim::Hand,
    /// Ticks out of sight.
    lost: u32,
}

impl Bot {
    /// An enemy in sight whom no round has gone at for [`FUTILE_TICKS`] is
    /// given up on for [`IGNORE_TICKS`]: a UO bot with only an LMG it
    /// could not deploy on a stair, and one with a dry pistol and rockets
    /// (none go at players), stood facing each other for twenty minutes.
    pub(super) fn count_futile(&mut self, view: &BotView, cmd: &UserCmd) {
        if let Some((slot, left)) = self.ignored {
            self.ignored = (left > 1).then_some((slot, left - 1));
        }
        match view.enemy {
            Some(e) if cmd.buttons & msg::BUTTON_ATTACK == 0 => {
                self.futile += 1;
                if self.futile >= FUTILE_TICKS {
                    self.futile = 0;
                    self.ignored = Some((e.slot, IGNORE_TICKS));
                }
            }
            _ => self.futile = 0,
        }
    }

    /// Whether the server leaves the enemy in `slot` out of this bot's
    /// view ([`Bot::count_futile`]).
    pub fn ignores(&self, slot: usize) -> bool {
        self.ignored.is_some_and(|(s, _)| s == slot)
    }

    /// A held rocket launcher fires at no player (`trigger`): rockets are
    /// for the bases and for vehicles (`Bot::think_armor`). With a 320-unit floor on the range, a rocket at a
    /// moving body still burst on cover by the shooter often enough that a
    /// bot killed itself in most four-bot rounds (bot-objectives.md 7.2).
    fn launcher_in_hand(view: &BotView) -> bool {
        view.rocket.is_some_and(|r| r == view.weapon)
    }

    /// The gun, else the pistol, comes out over a held rocket launcher
    /// with an enemy in sight: rockets are for the bases and vehicles,
    /// and one at an enemy is a splash on the shooter as often as not. The
    /// breach and a vehicle draw the launcher again.
    pub(super) fn off_launcher(&mut self, view: &BotView) {
        if view.enemy.is_none()
            || self.rider.at_armor
            || view.rocket.is_none_or(|r| r != view.weapon)
        {
            return;
        }
        let loaded = |w: u8| w < 64 && view.loaded >> w & 1 == 1;
        let to = view.gun.or(view.pistol.filter(|p| loaded(*p)));
        if let Some(to) = to
            && self.switch_to.is_none()
        {
            self.primary = view.weapon;
            self.calm_ticks = 0;
            self.switch_to = Some(to);
        }
    }

    /// The held weapon and the pistol both spent (a launcher fired dry):
    /// the gun comes out, or the bot stands with nothing to fire.
    pub(super) fn off_dry(&mut self, view: &BotView) {
        let loaded = |w: u8| w != 0 && w < 64 && view.loaded >> w & 1 == 1;
        if self.switch_to.is_some()
            || view.weapon == 0
            || loaded(view.weapon)
            || view.pistol.is_some_and(loaded)
            || view.grenade == Some(view.weapon)
        {
            return;
        }
        if let Some(g) = view.gun {
            self.switch_to = Some(g);
        }
    }

    /// The pistol comes out when the held weapon runs dry with an enemy
    /// close and the draw beats the reload to the next shot, or when the
    /// held weapon has nothing left to reload. It goes back once no enemy
    /// has been in sight for [`HOLSTER_TICKS`], or at once when the pistol
    /// is spent, as long as the primary has a round left.
    pub(super) fn sidearm(&mut self, view: &BotView) {
        let Some(pistol) = view.pistol else {
            return;
        };
        if self.switch_to.is_some() {
            return;
        }
        let loaded = |w: u8| w != 0 && w < 64 && view.loaded >> w & 1 == 1;
        if view.weapon != pistol {
            let held = view.weapon != 0 && view.weapons_held >> view.weapon & 1 == 1;
            if !held || view.grenade == Some(view.weapon) || !loaded(pistol) {
                return;
            }
            let close = view
                .enemy
                .is_some_and(|e| dist_sq(view.origin, e.origin) < PISTOL_RANGE * PISTOL_RANGE);
            let quicker = view.draw_ms < view.reload_ms;
            // An LMG can be put away only with the bipod up, and its deploy
            // takes as long as a fight at pistol range.
            let bipod_off = view.lmg && !view.deployed && (close || self.lmg_rest > 0);
            if (view.clip == 0 && close && quicker)
                || !loaded(view.weapon)
                || (bipod_off && view.enemy.is_some())
            {
                self.primary = view.weapon;
                self.calm_ticks = 0;
                self.switch_to = Some(pistol);
            }
            return;
        }
        self.calm_ticks = if view.enemy.is_some() {
            0
        } else {
            self.calm_ticks + 1
        };
        let back = Some(self.primary).filter(|w| loaded(*w)).or(view.gun);
        if let Some(back) = back
            && (self.calm_ticks >= HOLSTER_TICKS || !loaded(pistol))
        {
            self.switch_to = Some(back);
        }
    }

    /// Keeps [`Bot::target`] in step with the server's enemy: a new slot is
    /// a new target, one out of sight for `forget_ticks` is dropped. A new
    /// target starts with the full reaction delay and aim error, less for
    /// one the bot let go of within `forget_ticks` (it keeps most of the
    /// error it had) or one who just hit it.
    pub(super) fn track(&mut self, view: &BotView) {
        if let Some(p) = self.prior.as_mut() {
            p.2 += 1;
            if p.2 > self.skill.forget_ticks {
                self.prior = None;
            }
        }
        let Some(e) = view.enemy else {
            // Out of sight behind its own cover the bot still knows where
            // the target is.
            let covering = self.covering();
            if let Some(t) = self.target.as_mut().filter(|_| !covering) {
                t.lost += 1;
                if t.lost > self.skill.forget_ticks {
                    self.disengage();
                }
            }
            return;
        };
        if let Some(t) = self.target.as_mut().filter(|t| t.slot == e.slot) {
            t.lost = 0;
            return;
        }
        let full = self.skill.reaction_min + self.rand_below(self.skill.reaction_spread);
        let again = self.prior.filter(|p| p.0 == e.slot && self.aware);
        let (react, error) = if let Some((_, error, _)) = again {
            (
                full / 3,
                (error * REACQUIRE_ERROR).clamp(self.skill.error_floor, self.skill.error_start),
            )
        } else if view.hurt.is_some_and(|h| h.slot == e.slot) && self.aware {
            (full / 2, self.skill.error_start * HIT_BY_ERROR)
        } else {
            (full, self.skill.error_start)
        };
        if again.is_none() {
            self.want_yell(yell::Yell::Spotted);
        }
        if let Some(t) = self.target.take() {
            self.prior = Some((t.slot, t.error, 0));
        }
        self.burst_ticks = 0;
        let floor = self.skill.error_floor;
        let hand = aim::Hand::new(&e, self.human_aim.then_some((&mut self.rng, floor)));
        self.target = Some(Target {
            slot: e.slot,
            react,
            error,
            dir: [0.0, 1.0],
            wobble: 0,
            hand,
            lost: 0,
        });
    }

    /// Drops the target, remembered for a while as [`Bot::prior`].
    pub(super) fn disengage(&mut self) {
        if let Some(t) = self.target.take() {
            self.prior = Some((t.slot, t.error, 0));
        }
        self.burst_ticks = 0;
        self.crouch_ticks = 0;
        self.prone_ticks = 0;
    }

    /// Hit by an enemy not in sight: the view turns toward where the hit
    /// came from, at the bot's turn rate, for [`HURT_LOOK_MS`], so the
    /// sight cone sweeps over him. The path's keys follow the turn. Not on
    /// a mantle edge, whose climb wants the view on the wall.
    pub(super) fn face_hurt(
        &self,
        view: &BotView,
        cmd: &mut UserCmd,
        yaw: f32,
    ) -> Option<[f32; 2]> {
        let h = view
            .hurt
            .filter(|h| h.ago_ms < HURT_LOOK_MS && self.aware && !view.mantle)?;
        let [x, y, z] = h.dir;
        if x == 0.0 && y == 0.0 {
            return None;
        }
        // Back along the direction the damage travelled.
        let want = [
            (z.atan2(x.hypot(y)).to_degrees()).clamp(-45.0, 45.0),
            (-y).atan2(-x).to_degrees(),
        ];
        let look = self.turn_toward(view, want);
        (cmd.forward, cmd.right) = rekey(cmd.forward, cmd.right, yaw, look[1]);
        Some(look)
    }

    /// One tick against the tracked target: turn toward it at a bounded
    /// rate, sights, trigger and footwork into `cmd`. Returns the commanded
    /// (pitch, yaw), or `None` while the reaction delay still runs.
    pub(super) fn engage(
        &mut self,
        view: &BotView,
        e: EnemyView,
        cmd: &mut UserCmd,
    ) -> Option<[f32; 2]> {
        let mut t = self.target.take()?;
        if self.human_aim {
            t.hand.see(&e);
            self.gun_kept.rotate_right(1);
            self.gun_kept[0] = view.gun_offset;
        }
        if t.react > 0 {
            t.react -= 1;
            self.target = Some(t);
            return None;
        }
        t.error = (t.error * self.skill.error_decay).max(self.skill.error_floor);
        if !self.human_aim {
            if t.wobble == 0 {
                let a = (self.rand() % 360) as f32;
                t.dir = [a.to_radians().sin(), a.to_radians().cos()];
                t.wobble = 4 + self.rand_below(5);
            }
            t.wobble -= 1;
        }
        let dist = dist_sq(view.origin, e.origin).sqrt().max(1.0);
        let moving =
            1.0 + (self.skill.error_moving - 1.0) * (view.speed / RUN_SPEED).min(1.0).powi(2);
        let (tp, ty) = aim_angles(view, e.origin);
        let g = view.gun_offset;
        let aim = if self.human_aim {
            let eye = [0, 1, 2].map(|i| {
                view.origin[i] + view.lean_offset[i] + if i == 2 { view.eye_height } else { 0.0 }
            });
            let (pp, py) = aim_angles(view, t.hand.perceived(eye, self.skill.track_lag));
            let miss = t.error * (1.0 + dist / self.skill.error_range) * moving;
            let err_deg = (miss / dist).atan().to_degrees();
            let d = t.hand.drift(&mut self.rng);
            let back = aim::pulled(&self.gun_kept, &self.skill);
            let want = [pp + d[0] * err_deg - back[0], py + d[1] * err_deg - back[1]];
            let now = [yaw_diff(view.view[0], 0.0), view.view[1]];
            let aim = t
                .hand
                .steer(now, want, self.clock, &self.skill, &mut self.rng);
            self.target = Some(t);
            aim
        } else {
            // The target's speed across the line: the aim trails it.
            let to = [0, 1, 2].map(|i| (e.origin[i] - view.origin[i]) / dist);
            let along: f32 = (0..3).map(|i| e.velocity[i] * to[i]).sum();
            let across = (0..3)
                .map(|i| (e.velocity[i] - along * to[i]).powi(2))
                .sum::<f32>()
                .sqrt();
            let miss = (t.error + across * self.skill.track_lag)
                * (1.0 + dist / self.skill.error_range)
                * moving;
            let err_deg = (miss / dist).atan().to_degrees();
            let dir = t.dir;
            self.target = Some(t);
            // The view goes where it puts the gun on the line.
            self.turn_toward(
                view,
                [tp - g[0] + dir[0] * err_deg, ty - g[1] + dir[1] * err_deg],
            )
        };
        // This cmd's round leaves along the view the last one left, not
        // this cmd's turn: the gun's miss of the true chest point from it.
        let off = yaw_diff(view.view[0] + g[0], tp).hypot(yaw_diff(view.view[1] + g[1], ty));
        let cone = (HIT_RADIUS / dist).atan().to_degrees();

        log::trace!(
        target: "vcod_server::bots::engage",
        "BOT org={:?} e={} dist={:.0} off={:.3} cone={:.3} err={:.1} cd={} busy={} ads_frac={} fil={} clip={} view={:?} aim={:?}",
            view.origin,
            e.slot,
            dist,
            off,
            cone,
            self.target.as_ref().map_or(0.0, |t| t.error),
            self.fire_cooldown,
            view.busy_ms,
            view.ads_frac,
            view.friend_in_line,
            view.clip,
            view.view,
            aim
        );
        if view.lmg {
            let ready = self.bipod(view, [tp - g[0], ty], cmd);
            self.trigger(view, dist, off, cone, ready, cmd);
            return Some(aim);
        }
        let ads = view.has_ads
            && if view.sniper {
                dist > SCOPE_MIN_RANGE
            } else {
                dist > self.skill.ads_range
            };
        if ads {
            cmd.buttons |= msg::BUTTON_ADS;
        }
        // A scope or sights still coming up would waste the shot on hip
        // spread; an automatic inside `AUTO_HIP_RANGE` sprays it anyway.
        let rising = view.automatic && dist < AUTO_HIP_RANGE;
        let ready = (!ads || view.ads_frac >= 1.0 || rising) && view.eye_settled;
        self.trigger(view, dist, off, cone, ready, cmd);
        self.footwork(view, dist, ads, cmd);
        // A teammate across the line: step aside, standing and off the
        // sights, until the line clears.
        if view.friend_in_line
            && let Some(p) = view.sidestep
        {
            let toward = (p[1] - view.origin[1])
                .atan2(p[0] - view.origin[0])
                .to_degrees();
            let d = yaw_diff(toward, aim[1]).to_radians();
            cmd.forward = (d.cos() * 127.0).round() as i8;
            cmd.right = (-d.sin() * 127.0).round() as i8;
            cmd.buttons &= !msg::BUTTON_ADS;
            cmd.wbuttons &= !(msg::WBUTTON_CROUCH | msg::WBUTTON_PRONE);
            cmd.up = 0;
        } else {
            self.cover_engage(view, e, dist, aim[1], cmd);
        }
        Some(aim)
    }

    /// A United Offensive LMG fires only with its bipod down and the sight
    /// fully up (coduo-movement.md 6, 7): the sight button deploys it where
    /// the bot stands, on a mount or prone, and the body holds still. A
    /// deploy the engine refuses, or a target past the yaw cap round the
    /// deploy's facing, lets the button go for a while: the breakdown, then
    /// a fresh deploy facing the target. A target past the deploy's pitch
    /// clamp rests the button too, so the pistol comes out: the pinned view
    /// fires under the line. Whether the trigger may go down.
    fn bipod(&mut self, view: &BotView, want: [f32; 2], cmd: &mut UserCmd) -> bool {
        self.lmg_linger = LMG_LINGER_TICKS;
        cmd.forward = 0;
        cmd.right = 0;
        // A pistol draw under way: the sight would hold the LMG out.
        if self.lmg_rest > 0 || self.switch_to.is_some() {
            return false;
        }
        if yaw_diff(want[0], view.lmg_pitch_from).abs() > LMG_PITCH_DEG
            || (view.deployed && yaw_diff(want[1], view.deploy_yaw).abs() > LMG_ARC_DEG)
        {
            self.lmg_rest = LMG_REST_TICKS;
            return false;
        }
        self.lmg_tries = if view.deployed { 0 } else { self.lmg_tries + 1 };
        if self.lmg_tries > LMG_TRY_TICKS {
            self.lmg_tries = 0;
            self.lmg_rest = LMG_REST_TICKS;
            return false;
        }
        cmd.buttons |= msg::BUTTON_ADS;
        view.deployed && view.ads_frac >= LMG_READY_FRAC
    }

    /// The (pitch, yaw) one tick's turn takes the view to on its way to
    /// `want`, at most `turn_deg` along the straight line between them.
    pub(super) fn turn_toward(&self, view: &BotView, want: [f32; 2]) -> [f32; 2] {
        let cur = [yaw_diff(view.view[0], 0.0), view.view[1]];
        let d = [yaw_diff(want[0], cur[0]), yaw_diff(want[1], cur[1])];
        let len = d[0].hypot(d[1]);
        let k = if len > self.skill.turn_deg {
            self.skill.turn_deg / len
        } else {
            1.0
        };
        [
            (cur[0] + d[0] * k).clamp(-PITCH_MOST, PITCH_MOST),
            cur[1] + d[1] * k,
        ]
    }

    /// Semi-autos tap, at least `fireTime` apart (combat doc 1.4); automatics
    /// hold the bit for a burst, shorter at range.
    /// Inside the cone the trigger goes down; past it, with a chance that
    /// falls to none at the weapon's reach ([`REACH_AUTO`]), so a settling
    /// aim fires before it has settled.
    fn trigger(
        &mut self,
        view: &BotView,
        dist: f32,
        off: f32,
        cone: f32,
        ready: bool,
        cmd: &mut UserCmd,
    ) {
        if view.clip == 0 || view.friend_in_line || Self::launcher_in_hand(view) {
            self.burst_ticks = 0;
            return;
        }
        let reach = if view.automatic {
            REACH_AUTO
        } else if view.fire_time_ms >= BOLT_MS {
            REACH_BOLT
        } else {
            REACH_SEMI
        };
        if view.automatic && self.burst_ticks > 0 {
            // A burst rides out recoil and wobble, but not a lost line.
            if off <= cone * reach {
                cmd.buttons |= msg::BUTTON_ATTACK;
                self.burst_ticks -= 1;
            } else {
                self.burst_ticks = 0;
            }
            if self.burst_ticks == 0 {
                self.fire_cooldown = 3 + self.rand_below(4);
            }
            return;
        }
        if self.fire_cooldown > 0 || view.busy_ms != 0 || !ready || off >= cone * reach {
            return;
        }
        if off > cone {
            let chance = (reach - off / cone) / (reach - 1.0);
            if (self.rand_below(1000) as f32) >= chance * 1000.0 {
                return;
            }
        }
        cmd.buttons |= msg::BUTTON_ATTACK;
        if view.automatic {
            let long = dist > 800.0;
            self.burst_ticks = if long { 1 } else { 2 } + self.rand_below(if long { 3 } else { 6 });
        } else {
            let fire = if view.fire_time_ms > 0 {
                view.fire_time_ms
            } else {
                200
            };
            // Rounded up, and never under two ticks: the bit has to come up
            // between taps for the next to read as a press.
            let ticks = ((fire + FRAME_MS - 1) / FRAME_MS).max(2) as u32;
            self.fire_cooldown = ticks + self.rand_below(3);
        }
    }

    /// Strafe in spells and crouch now and then while engaging; close in
    /// only past the weapon's working range. On the sights a spell moves
    /// only with [`Skill::footwork`]'s chance: a slow strafe, or with a bolt
    /// rifle inside [`BACK_OFF_RANGE`] a step back; at range it may lie
    /// down instead. A scope holds still. Off ([`Bot::footwork`]), sighted
    /// shots always stand still, since moving widens the error.
    pub(super) fn footwork(&mut self, view: &BotView, dist: f32, ads: bool, cmd: &mut UserCmd) {
        if self.strafe_ticks == 0 {
            self.strafe = if self.rand() % 2 == 0 { 127 } else { -127 };
            self.strafe_ticks = 8 + self.rand_below(16);
            let roll = self.rand_below(1000) as f32 / 1000.0;
            self.shuffle = self.footwork && roll < self.skill.footwork;
            if self.shuffle
                && self.prone_ticks == 0
                && dist > PRONE_RANGE
                && roll < self.skill.footwork / 4.0
            {
                self.prone_ticks = 40 + self.rand_below(40);
            } else if self.crouch_ticks == 0
                && self.prone_ticks == 0
                && self.rand_below(if view.sniper { 2 } else { 4 }) == 0
            {
                self.crouch_ticks = 20 + self.rand_below(30);
            }
        } else {
            self.strafe_ticks -= 1;
        }
        let range = if view.sniper {
            f32::INFINITY
        } else if view.automatic {
            350.0
        } else {
            700.0
        };
        let bolt = !view.automatic && view.fire_time_ms >= BOLT_MS;
        cmd.forward = if dist > range {
            127
        } else if self.shuffle && bolt && dist < BACK_OFF_RANGE {
            -127
        } else {
            0
        };
        cmd.right = if !ads {
            self.strafe
        } else if self.shuffle && !view.sniper {
            self.strafe / 2
        } else {
            0
        };
        if self.prone_ticks > 0 && dist < PRONE_RANGE * 0.75 {
            self.prone_ticks = 0;
        }
        if self.prone_ticks > 0 {
            self.prone_ticks -= 1;
            // Held like the crouch, with `upmove` -127 (protocol doc,
            // "Usercmd input bits"); a prone body fires from where it lies.
            cmd.wbuttons |= msg::WBUTTON_PRONE;
            cmd.up = -127;
            cmd.forward = 0;
            cmd.right = 0;
        } else if self.crouch_ticks > 0 {
            self.crouch_ticks -= 1;
            // The stance is a held level; retail sends `upmove` -127 with it
            // (protocol doc, "Usercmd input bits").
            cmd.wbuttons |= msg::WBUTTON_CROUCH;
            cmd.up = -127;
        }
    }
}

/// The pistol comes out against an enemy this close with the primary dry.
const PISTOL_RANGE: f32 = 500.0;
/// Ticks with no enemy in sight before the pistol goes back.
pub(super) const HOLSTER_TICKS: u32 = 40;
/// A scope inside this is a liability: the zoomed view cannot track a
/// target this close, so the shot goes from the hip.
const SCOPE_MIN_RANGE: f32 = 200.0;

/// In sight this long with no round fired, an enemy is left out of the
/// view for [`IGNORE_TICKS`] ([`Bot::count_futile`]).
pub(super) const FUTILE_TICKS: u32 = 200;
/// How long an enemy given up on stays out of the view.
pub(super) const IGNORE_TICKS: u32 = 600;
/// An LMG's deploy that has not taken this many ticks after the press was
/// refused (a wall ahead, no room to go prone).
pub(super) const LMG_TRY_TICKS: u32 = 10;
/// Ticks the button rests after a refused deploy before the next try.
const LMG_REST_TICKS: u32 = 40;
/// Ticks the bipod stays down once the target is gone.
const LMG_LINGER_TICKS: u32 = 60;
/// The shot waits on the sight this far up (coduo-movement.md 6).
const LMG_READY_FRAC: f32 = 0.99;
/// A target more than this far round the deploy's facing is past
/// `bg_lmg_yawcap`'s 55 degrees, near enough.
const LMG_ARC_DEG: f32 = 50.0;
/// The pitch clamp of a deployed LMG's view, either way of
/// `proneTorsoPitch` (`PM_UpdateViewAngles`, coduo-movement.md 7;
/// [`BotView::lmg_pitch_from`]).
const LMG_PITCH_DEG: f32 = 45.0;
