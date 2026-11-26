//! The fight: tracking the target, the aim's turn, the trigger, footwork, the bipod and the weapon
//! swaps.

use super::*;

/// The view's pitch either way of level: past pmove's 16000-short clamp
/// (87.9) no further, and high enough for a lob that comes down 250 off
/// (`Server::lob_probe` tries 85).
pub(super) const PITCH_MOST: f32 = 85.0;
/// Half the width of what a shot has to land in (chest or head), units; the
/// fire cone is the angle it subtends at the target's range.
const HIT_RADIUS: f32 = 10.0;
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
const HURT_LOOK_MS: i32 = 1500;
/// A target taken again after [`Bot::prior`] keeps its error times this;
/// one who just hit the bot starts at this share of `error_start`. Both
/// take a third and a half of the reaction delay.
const REACQUIRE_ERROR: f32 = 1.25;
const HIT_BY_ERROR: f32 = 0.75;

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

    /// Ticks the trigger stays up past a burst or a tap's spacing
    /// ([`Skill::fire_pause`]); no draw where the level has none.
    fn fire_pause(&mut self) -> u32 {
        match self.skill.fire_pause {
            (least, 0) => least,
            (least, spread) => least + self.rand_below(spread),
        }
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
                self.fire_cooldown = 3 + self.rand_below(4) + self.fire_pause();
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
            self.fire_cooldown = ticks + self.rand_below(3) + self.fire_pause();
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
const HOLSTER_TICKS: u32 = 40;
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
const LMG_TRY_TICKS: u32 = 10;
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bots::tests::*;
    use crate::bots::walk::QUIET_RELOAD_TICKS;
    use vcod_common::net::msg::{BUTTON_ADS, BUTTON_ATTACK, WBUTTON_RELOAD};

    /// The sight button stays down through a rechamber: 1.1 keeps the
    /// scope up through it, and 1.5 and UO lower it for the bolt and raise
    /// it again on the held button (cod15-movement.md 6), as a player's.
    /// No round leaves while the sight comes back up.
    #[test]
    fn a_scoped_bot_holds_the_sight_through_a_rechamber() {
        let (mut bot, mut v) = engaged(3);
        v.sniper = true;
        v.enemy.as_mut().unwrap().origin = [1000.0, 0.0, 64.0];
        for (busy, frac) in [(0, 1.0), (900, 1.0), (500, 0.0), (0, 0.4)] {
            v.busy_ms = busy;
            v.ads_frac = frac;
            let cmd = bot.think(&v);
            assert_ne!(cmd.buttons & BUTTON_ADS, 0, "sight let go at busy {busy}");
            if frac < 1.0 {
                assert_eq!(
                    cmd.buttons & BUTTON_ATTACK,
                    0,
                    "fired with the sight at {frac}"
                );
            }
        }
    }

    /// A scoped bot puts the swayed gun on the target, not the view, and
    /// fires on the view its last cmd left, which the round leaves along
    /// (combat doc 15.1). The old aim took the whole offset back at once;
    /// the hand takes `recoil_share` of it, `recoil_ticks` late, and fires
    /// once that brings the gun inside the cone.
    #[test]
    fn a_scoped_bot_aims_the_swayed_gun() {
        for human in [false, true] {
            let (mut bot, mut v) = engaged(3);
            bot.human_aim = human;
            bot.skill.error_start = 0.0;
            bot.skill.error_floor = 0.0;
            v.sniper = true;
            v.fire_time_ms = 1300;
            v.ads_frac = 1.0;
            v.enemy.as_mut().unwrap().origin = [1000.0, 0.0, 64.0];
            let (tp, ty) = aim_angles(&v, [1000.0, 0.0, 64.0]);
            v.gun_offset = [1.0, -1.2];
            // The view on the chest: the gun is off it past a bolt rifle's
            // reach, so no shot.
            v.view = [tp, ty, 0.0];
            let mut fired = None;
            for tick in 0..20 {
                let cmd = bot.think(&v);
                if cmd.buttons & BUTTON_ATTACK != 0 {
                    fired = Some((tick, v.view));
                    break;
                }
                let a = cmd.angles.map(|a| a as f32 / ANGLE2SHORT);
                v.view = [a[0], a[1], 0.0];
            }
            let (tick, at) = fired.expect("never fired");
            let miss = yaw_diff(at[0] + 1.0, tp).hypot(yaw_diff(at[1] - 1.2, ty));
            if human {
                assert!(
                    tick > bot.skill.recoil_ticks as usize,
                    "fired on tick {tick}"
                );
                let cone = (HIT_RADIUS / 1000.0).atan().to_degrees();
                assert!(miss < cone, "the gun missed the chest by {miss} degrees");
                assert!(miss > 0.1, "took the whole offset back: {miss}");
            } else {
                assert!(tick > 0, "fired with the gun off the chest");
                assert!(miss < 0.01, "the gun missed the chest by {miss} degrees");
            }
        }
    }

    /// An LMG bot holds the sight button to deploy, keeps its feet still,
    /// and fires only once the bipod is down and the sight up; a deploy
    /// the engine refuses lets the button go for a rest.
    #[test]
    fn an_lmg_bot_deploys_its_bipod_before_it_fires() {
        let (mut bot, mut v) = engaged(3);
        v.lmg = true;
        v.automatic = true;
        v.enemy.as_mut().unwrap().origin = [800.0, 0.0, 60.0];
        let cmd = bot.think(&v);
        assert_ne!(cmd.buttons & BUTTON_ADS, 0, "the press deploys");
        assert_eq!(cmd.buttons & BUTTON_ATTACK, 0, "no shot off the bipod");
        assert_eq!((cmd.forward, cmd.right), (0, 0));
        v.deployed = true;
        v.ads_frac = 1.0;
        let fired = (0..20).any(|_| {
            let cmd = bot.think(&v);
            // The sim takes the view where the cmd points it.
            let a = cmd.angles.map(|a| a as f32 / ANGLE2SHORT);
            v.view = [a[0], a[1], 0.0];
            cmd.buttons & BUTTON_ATTACK != 0
        });
        assert!(fired, "a deployed LMG fires");
        // Refused: the button rests, then tries again.
        v.deployed = false;
        v.ads_frac = 0.0;
        let held: Vec<bool> = (0..LMG_TRY_TICKS + 3)
            .map(|_| bot.think(&v).buttons & BUTTON_ADS != 0)
            .collect();
        assert!(held[0] && !*held.last().unwrap(), "{held:?}");
    }

    /// A target that left the yaw cap round the deploy's facing breaks the
    /// bipod down, for a fresh deploy facing it.
    #[test]
    fn an_lmg_target_past_the_yaw_cap_breaks_the_bipod_down() {
        let (mut bot, mut v) = engaged(3);
        v.lmg = true;
        v.deployed = true;
        v.ads_frac = 1.0;
        v.deploy_yaw = -90.0;
        let cmd = bot.think(&v);
        assert_eq!(cmd.buttons & (BUTTON_ADS | BUTTON_ATTACK), 0);
    }

    /// A target past the deployed view's pitch clamp breaks the bipod down
    /// rather than firing from the pinned view under the line: on mp_foy
    /// (bas, seed 21) a planter in the allied charge room emptied an MG34
    /// at a defender 53 degrees up through the hole over it, every round
    /// into the lip (bot-objectives.md 7.2).
    #[test]
    fn an_lmg_target_past_the_pitch_clamp_breaks_the_bipod_down() {
        let (mut bot, mut v) = engaged(3);
        v.lmg = true;
        v.automatic = true;
        v.deployed = true;
        v.ads_frac = 1.0;
        v.enemy.as_mut().unwrap().origin = [150.0, 0.0, 324.0];
        for _ in 0..20 {
            let cmd = bot.think(&v);
            assert_eq!(cmd.buttons & (BUTTON_ADS | BUTTON_ATTACK), 0);
            // The clamp pins the view 45 up.
            let a = cmd.angles.map(|a| a as f32 / ANGLE2SHORT);
            v.view = [a[0].max(-45.0), a[1], 0.0];
        }
    }

    /// The clamp measures from `proneTorsoPitch`, not level: prone on a
    /// slope rising toward him, a target 53 degrees up lies 33 off the
    /// torso and is fired at; laid down a slope, one 35 up lies 55 off and
    /// breaks the bipod down (coduo-movement.md 7).
    #[test]
    fn an_lmg_pitch_clamp_measures_from_the_prone_torso() {
        let fire = |torso: f32, z: f32| {
            let (mut bot, mut v) = engaged(3);
            v.lmg = true;
            v.automatic = true;
            v.deployed = true;
            v.ads_frac = 1.0;
            v.lmg_pitch_from = torso;
            v.enemy.as_mut().unwrap().origin = [150.0, 0.0, z];
            (0..20).any(|_| {
                let cmd = bot.think(&v);
                let a = cmd.angles.map(|a| a as f32 / ANGLE2SHORT);
                v.view = [a[0].clamp(torso - 45.0, torso + 45.0), a[1], 0.0];
                cmd.buttons & BUTTON_ATTACK != 0
            })
        };
        assert!(fire(-20.0, 324.0));
        assert!(!fire(0.0, 324.0));
        assert!(!fire(20.0, 229.0));
        assert!(fire(0.0, 229.0));
    }

    #[test]
    fn a_new_target_holds_fire_through_the_reaction_delay() {
        let (_, v) = engaged(7);
        let mut bot = Bot::new("allies", true, 7);
        // Settled aim from the start: without the delay the first tick fires.
        bot.skill.error_start = bot.skill.error_floor;
        let react = bot.skill.reaction_min as usize;
        for tick in 0..react {
            let cmd = bot.think(&v);
            assert_eq!(cmd.buttons & BUTTON_ATTACK, 0, "fired on tick {tick}");
        }
        let fired = (0..40).any(|_| bot.think(&v).buttons & BUTTON_ATTACK != 0);
        assert!(fired, "never fired once the delay ran out");
        // A switch to another enemy restarts the delay.
        let e = v.enemy.unwrap();
        let mut other = v;
        other.enemy = Some(EnemyView { slot: 4, ..e });
        bot.fire_cooldown = 0;
        bot.burst_ticks = 0;
        for tick in 0..react {
            let cmd = bot.think(&other);
            assert_eq!(cmd.buttons & BUTTON_ATTACK, 0, "fired on tick {tick}");
        }
    }

    /// Back to a target it let go of, the bot keeps most of its settled
    /// aim and a third of the delay; onto one who just hit it, half the
    /// delay and three quarters of the first error.
    #[test]
    fn a_known_target_is_taken_again_quicker() {
        let (mut bot, v) = engaged(7);
        bot.skill.reaction_min = 9;
        let e = v.enemy.unwrap();
        bot.track(&v);
        bot.target.as_mut().unwrap().error = bot.skill.error_floor;
        let mut other = engaged(7).1;
        other.enemy = Some(EnemyView { slot: 4, ..e });
        bot.track(&other);
        let t = bot.target.as_ref().unwrap();
        assert_eq!((t.slot, t.react, t.error), (4, 9, bot.skill.error_start));
        bot.track(&v);
        let t = bot.target.as_ref().unwrap();
        assert_eq!((t.slot, t.react), (3, 3));
        assert_eq!(t.error, bot.skill.error_floor * REACQUIRE_ERROR);

        let (mut bot, mut v) = engaged(7);
        bot.skill.reaction_min = 9;
        v.hurt = Some(Hurt {
            slot: 3,
            ago_ms: 50,
            dir: [0.0, 1.0, 0.0],
        });
        bot.track(&v);
        let t = bot.target.as_ref().unwrap();
        assert_eq!(t.react, 4);
        assert_eq!(t.error, bot.skill.error_start * HIT_BY_ERROR);
    }

    /// Shot from behind by nobody in sight, the bot comes round toward
    /// where the round came from at its turn rate.
    #[test]
    fn a_hit_from_out_of_sight_turns_the_view_toward_it() {
        let (mut bot, mut v) = engaged(3);
        v.enemy = None;
        v.view = [0.0, 0.0, 0.0];
        // From an attacker at -x toward the body.
        v.hurt = Some(Hurt {
            slot: 5,
            ago_ms: 50,
            dir: [1.0, 0.0, 0.0],
        });
        let mut yaw = 0.0f32;
        for _ in 0..20 {
            v.view = [0.0, yaw, 0.0];
            let next = bot.think(&v).angles[1] as f32 / ANGLE2SHORT;
            assert!(yaw_diff(next, yaw).abs() <= bot.skill.turn_deg + 0.01);
            yaw = next;
        }
        assert!(yaw_diff(180.0, yaw).abs() < 1.0, "never came round: {yaw}");
        // Past the look window the hit no longer steers.
        v.hurt.as_mut().unwrap().ago_ms = HURT_LOOK_MS;
        let mut cmd = NULL_USERCMD;
        assert!(bot.face_hurt(&v, &mut cmd, 0.0).is_none());
    }

    /// A flick turns `turn_deg` a tick on the mean and 1.875 times that
    /// at its peak (the drift moves the want a little besides).
    #[test]
    fn the_view_turns_a_bounded_step_per_tick() {
        let (mut bot, mut v) = engaged(3);
        // The enemy is behind: yaw 180 from a view at 0.
        v.enemy = Some(EnemyView {
            slot: 3,
            origin: [-500.0, 0.0, 124.0],
            velocity: [0.0; 3],
        });
        let step = 1.875 * bot.skill.turn_deg + 1.0;
        let mut yaw = 0.0f32;
        for _ in 0..30 {
            v.view = [0.0, yaw, 0.0];
            let cmd = bot.think(&v);
            let next = cmd.angles[1] as f32 / ANGLE2SHORT;
            assert!(
                yaw_diff(next, yaw).abs() <= step + 0.01,
                "turned {} in one tick",
                yaw_diff(next, yaw)
            );
            yaw = next;
        }
        assert!(yaw_diff(180.0, yaw).abs() < 5.0, "never came round: {yaw}");
    }

    /// A crouched bot's round leaves 40 over its feet, not 60: aimed off
    /// the standing eye, a crouched sniper put every round 20 units low at
    /// 900 and hit legs and low walls (bot-objectives.md 2).
    #[test]
    fn a_crouched_bot_aims_from_its_own_eye() {
        let mut v = view();
        v.eye_height = vcod_common::pmove::VIEW_CROUCH;
        let level = [0.0, 900.0, 64.0 + vcod_common::pmove::VIEW_CROUCH];
        let (pitch, yaw) = aim_angles(&v, level);
        assert!(pitch.abs() < 0.01, "level with the crouched eye: {pitch}");
        assert!((yaw - 90.0).abs() < 0.01);
        // The chest follows the enemy's stance too.
        assert_eq!(chest([0.0; 3], vcod_common::pmove::VIEW_STAND)[2], 40.0);
        assert!(chest([0.0; 3], vcod_common::pmove::VIEW_CROUCH)[2] < 30.0);
    }

    #[test]
    fn the_aim_error_shrinks_on_a_held_target() {
        let (mut bot, v) = engaged(5);
        bot.think(&v);
        let first = bot.target.as_ref().unwrap().error;
        for _ in 0..40 {
            bot.think(&v);
        }
        let later = bot.target.as_ref().unwrap().error;
        assert!(later < first / 4.0, "{first} -> {later}");
        assert_eq!(later, bot.skill.error_floor);
    }

    #[test]
    fn an_automatic_holds_the_trigger_and_a_semi_auto_taps() {
        let (mut bot, mut v) = engaged(9);
        v.automatic = true;
        let held: Vec<bool> = (0..40)
            .map(|_| bot.think(&v).buttons & BUTTON_ATTACK != 0)
            .collect();
        assert!(
            held.windows(2).any(|w| w[0] && w[1]),
            "an automatic never held across two ticks"
        );

        let (mut bot, v) = engaged(9);
        let mut last = None;
        let mut count = 0;
        let gap = (v.fire_time_ms / FRAME_MS).max(2);
        for tick in 0..60 {
            if bot.think(&v).buttons & BUTTON_ATTACK != 0 {
                if let Some(l) = last {
                    assert!(tick - l >= gap, "taps {l} and {tick} too close");
                }
                last = Some(tick);
                count += 1;
            }
        }
        assert!(count >= 3, "a semi-auto tapped {count} times in 3 s");
    }

    /// Easy lets the trigger up past a semi-auto's tap spacing; normal
    /// taps again as soon as it allows.
    #[test]
    fn an_easy_bot_lets_the_trigger_up_between_taps() {
        let least_gap = |level: u8| {
            let (mut bot, v) = engaged(9);
            bot.skill.fire_pause = Skill::level(level).fire_pause;
            let taps: Vec<u32> = (0..200u32)
                .filter(|_| bot.think(&v).buttons & BUTTON_ATTACK != 0)
                .collect();
            assert!(taps.len() >= 3, "level {level} tapped {}", taps.len());
            taps.windows(2).map(|w| w[1] - w[0]).min().unwrap()
        };
        let spacing = (engaged(9).1.fire_time_ms / FRAME_MS).max(2) as u32;
        assert!(least_gap(0) >= spacing + 4, "{}", least_gap(0));
        assert!(least_gap(1) < spacing + 4, "{}", least_gap(1));
    }

    #[test]
    fn a_teammate_across_the_line_holds_fire() {
        use glam::Vec3;
        let body = (Vec3::new(40.0, -15.0, 0.0), Vec3::new(70.0, 15.0, 70.0));
        let (eye, chest) = (Vec3::new(0.0, 0.0, 60.0), Vec3::new(100.0, 0.0, 40.0));
        assert!(friend_in_line(eye, chest, &[body]));
        // Beside the line by more than the margin, or behind the target.
        let beside = (body.0 + Vec3::Y * 40.0, body.1 + Vec3::Y * 40.0);
        let behind = (body.0 + Vec3::X * 100.0, body.1 + Vec3::X * 100.0);
        assert!(!friend_in_line(eye, chest, &[beside, behind]));

        for automatic in [false, true] {
            let (mut bot, mut v) = engaged(9);
            v.automatic = automatic;
            v.friend_in_line = true;
            for tick in 0..40 {
                let cmd = bot.think(&v);
                assert_eq!(cmd.buttons & BUTTON_ATTACK, 0, "fired on tick {tick}");
            }
            v.friend_in_line = false;
            assert!((0..40).any(|_| bot.think(&v).buttons & BUTTON_ATTACK != 0));
        }
    }

    /// Blocked by a teammate with a clear spot to the left (+y, the enemy
    /// down +x): the bot holds fire and walks there off its sights, every
    /// tick, instead of strafing at random.
    #[test]
    fn a_blocked_bot_steps_aside_to_clear_the_line() {
        for seed in [3, 9, 21] {
            let (mut bot, mut v) = engaged(seed);
            v.enemy = Some(EnemyView {
                slot: 3,
                origin: [900.0, 0.0, 124.0],
                velocity: [0.0; 3],
            });
            v.friend_in_line = true;
            v.sidestep = Some([0.0, 64.0, 64.0]);
            for tick in 0..30 {
                let cmd = bot.think(&v);
                assert_eq!(cmd.buttons & BUTTON_ATTACK, 0, "fired on tick {tick}");
                assert_eq!(cmd.buttons & BUTTON_ADS, 0, "sighted on tick {tick}");
                assert!(
                    cmd.right < -100,
                    "seed {seed} tick {tick}: right {}",
                    cmd.right
                );
                assert!(cmd.forward.abs() < 20, "forward {}", cmd.forward);
            }
        }
    }

    /// The old aim: a target running across the line widens the miss by
    /// its speed times `track_lag`; one running along the line adds
    /// nothing. The hand trails a crossing target instead: its yaw sits
    /// behind his run by about his speed times `track_lag`.
    #[test]
    fn a_target_crossing_the_line_is_missed_by_more() {
        // Mean (unsigned miss, signed yaw lead) over ticks 10..40, degrees.
        let miss = |velocity: [f32; 3], human: bool| {
            let (mut bot, mut v) = engaged(5);
            bot.human_aim = human;
            bot.skill.error_start = bot.skill.error_floor;
            if human {
                // The drift off, to read the trail alone.
                (bot.skill.error_start, bot.skill.error_floor) = (0.0, 0.0);
            }
            let mut at = [900.0, 0.0, 64.0];
            let (mut sum, mut lead) = (0.0, 0.0);
            for tick in 0..40 {
                // The old aim read only the velocity; the hand sees him move.
                if human {
                    (0..3).for_each(|i| at[i] += velocity[i] * 0.05);
                }
                v.enemy = Some(EnemyView {
                    slot: 3,
                    origin: at,
                    velocity,
                });
                let (tp, ty) = aim_angles(&v, at);
                let a = bot.think(&v).angles.map(|a| a as f32 / ANGLE2SHORT);
                v.view = [a[0], a[1], 0.0];
                if tick >= 10 {
                    sum += yaw_diff(a[0], tp).hypot(yaw_diff(a[1], ty));
                    lead += yaw_diff(a[1], ty);
                }
            }
            (sum / 30.0, lead / 30.0)
        };
        let still = miss([0.0; 3], false).0;
        let along = miss([190.0, 0.0, 0.0], false).0;
        let across = miss([0.0, 190.0, 0.0], false).0;
        assert!(along < still * 1.3, "along {along} still {still}");
        assert!(across > still * 1.8, "across {across} still {still}");

        let lag = Skill::default().track_lag;
        let (_, lead) = miss([0.0, 190.0, 0.0], true);
        // At 900 the target runs to +y (yaw up): the aim trails at less.
        let want = -(190.0 * lag / 900.0f32).atan().to_degrees();
        assert!(
            lead < want * 0.5 && lead > want * 2.0,
            "lead {lead} want {want}"
        );
        let (_, lead) = miss([0.0, -190.0, 0.0], true);
        assert!(
            lead > -want * 0.5 && lead < -want * 2.0,
            "lead {lead} want {}",
            -want
        );
    }

    /// Off the cone the trigger may still go down: an automatic sprays at a
    /// line a bolt rifle waits on.
    #[test]
    fn an_automatic_fires_off_the_cone_where_a_bolt_rifle_waits() {
        let fired = |automatic: bool, fire_time_ms: i32| {
            let (mut bot, mut v) = engaged(7);
            bot.skill.error_start = 0.0;
            bot.skill.error_floor = 0.0;
            v.automatic = automatic;
            v.fire_time_ms = fire_time_ms;
            v.ads_frac = 1.0;
            let at = [700.0, 0.0, 64.0];
            v.enemy = Some(EnemyView {
                slot: 3,
                origin: at,
                velocity: [0.0; 3],
            });
            let (tp, ty) = aim_angles(&v, at);
            // The gun 2.5 cones off a view held on the chest.
            let cone = (HIT_RADIUS / 700.0).atan().to_degrees();
            v.gun_offset = [0.0, 2.5 * cone];
            v.view = [tp, ty, 0.0];
            (0..40)
                .filter(|_| bot.think(&v).buttons & BUTTON_ATTACK != 0)
                .count()
        };
        assert_eq!(fired(false, 1300), 0, "a bolt rifle fired 2.5 cones off");
        assert!(fired(true, 100) > 0, "an automatic never fired");
    }

    /// Footwork on the sights: a bolt rifle backs off a close target and
    /// lies down at range; off, the body never does either.
    #[test]
    fn footwork_backs_a_bolt_rifle_off_and_lies_down_at_range() {
        let keys = |on: bool, dist: f32| {
            let (mut bot, mut v) = engaged(9);
            bot.footwork = on;
            bot.skill.footwork = 1.0;
            v.fire_time_ms = 1300;
            v.enemy = Some(EnemyView {
                slot: 3,
                origin: [dist, 0.0, 64.0],
                velocity: [0.0; 3],
            });
            (0..200).map(|_| bot.think(&v)).collect::<Vec<_>>()
        };
        let prone = |c: &UserCmd| c.wbuttons & msg::WBUTTON_PRONE != 0;
        assert!(
            keys(true, 200.0).iter().any(|c| c.forward < 0),
            "never backed off"
        );
        assert!(keys(false, 200.0).iter().all(|c| c.forward >= 0));
        assert!(keys(true, 1200.0).iter().any(prone), "never lay down");
        assert!(!keys(false, 1200.0).iter().any(prone));
    }

    #[test]
    fn sights_go_up_at_range_and_hold_fire_until_they_are() {
        let (mut bot, mut v) = engaged(11);
        v.enemy = Some(EnemyView {
            slot: 3,
            origin: [900.0, 0.0, 124.0],
            velocity: [0.0; 3],
        });
        for _ in 0..40 {
            let cmd = bot.think(&v);
            assert!(cmd.buttons & BUTTON_ADS != 0, "no sights at 900 units");
            assert_eq!(cmd.buttons & BUTTON_ATTACK, 0, "fired from the hip");
            assert!(cmd.right.abs() <= 64, "ran while sighted: {}", cmd.right);
        }
        v.ads_frac = 1.0;
        assert!((0..40).any(|_| bot.think(&v).buttons & BUTTON_ATTACK != 0));
    }

    /// The eye easing up out of a crouch: a round fired now leaves above
    /// the eye the aim was taken from, so the trigger waits for it.
    #[test]
    fn the_trigger_waits_for_the_eye_to_settle() {
        let (mut bot, mut v) = engaged(11);
        v.eye_settled = false;
        for tick in 0..40 {
            assert_eq!(
                bot.think(&v).buttons & BUTTON_ATTACK,
                0,
                "fired on tick {tick}"
            );
        }
        v.eye_settled = true;
        assert!((0..40).any(|_| bot.think(&v).buttons & BUTTON_ATTACK != 0));
    }

    #[test]
    fn an_engaging_bot_strafes() {
        let (mut bot, v) = engaged(13);
        let rights: Vec<i8> = (0..80).map(|_| bot.think(&v).right).collect();
        assert!(
            rights.contains(&127) && rights.contains(&-127),
            "{rights:?}"
        );
    }

    #[test]
    fn a_dry_primary_draws_the_pistol_against_a_close_enemy() {
        let (mut bot, mut v) = engaged(21);
        v.pistol = Some(2);
        v.weapons_held |= 1 << 2;
        v.loaded |= 1 << 2;
        v.clip = 0;
        let cmd = bot.think(&v);
        assert_eq!(cmd.weapon, 2, "kept the dry rifle up");
        assert_eq!(cmd.wbuttons & WBUTTON_RELOAD, 0, "reloaded instead");
        // The byte rides every cmd until `ps.weapon` reads it.
        assert_eq!(bot.think(&v).weapon, 2);
        v.weapon = 2;
        v.clip = 7;
        assert_eq!(bot.think(&v).weapon, 2);
        // Nobody in sight for a while: the rifle goes back up.
        v.enemy = None;
        let back = (0..HOLSTER_TICKS + 1).any(|_| bot.think(&v).weapon == 10);
        assert!(back, "the pistol stayed out");
    }

    /// A reload quicker than the draw (a Garand's 1.6 s against a slow
    /// putaway) keeps the primary up even point blank.
    #[test]
    fn a_quick_reload_beats_a_slow_draw() {
        let (mut bot, mut v) = engaged(21);
        v.pistol = Some(2);
        v.weapons_held |= 1 << 2;
        v.loaded |= 1 << 2;
        v.clip = 0;
        (v.reload_ms, v.draw_ms) = (700, 750);
        let cmd = bot.think(&v);
        assert_eq!(cmd.weapon, 10);
        assert!(cmd.wbuttons & WBUTTON_RELOAD != 0);
    }

    /// A primary with nothing left goes away for the pistol with nobody in
    /// sight, presses no reload, and is not drawn again while it stays
    /// empty; a spent pistol hands back to a primary that has rounds.
    #[test]
    fn an_empty_primary_is_swapped_and_not_redrawn() {
        let mut bot = Bot::new("allies", true, 1);
        let mut v = view();
        v.pistol = Some(2);
        v.weapons_held |= 1 << 2;
        (v.clip, v.loaded) = (0, 1 << 2);
        let cmd = bot.think(&v);
        assert_eq!(cmd.weapon, 2);
        assert_eq!(cmd.wbuttons & WBUTTON_RELOAD, 0, "reloaded an empty gun");
        (v.weapon, v.clip) = (2, 7);
        for _ in 0..HOLSTER_TICKS * 2 {
            assert_eq!(bot.think(&v).weapon, 2, "the empty primary came back");
        }
        // A pickup refills the primary: the calm spell hands it back.
        v.loaded |= 1 << 10;
        assert_eq!(bot.think(&v).weapon, 10);
        // The pistol runs out mid-fight with the primary loaded: back at
        // once.
        let (mut bot, mut v) = engaged(21);
        v.pistol = Some(2);
        v.weapons_held |= 1 << 2;
        v.loaded |= 1 << 2;
        v.clip = 0;
        assert_eq!(bot.think(&v).weapon, 2);
        (v.weapon, v.clip, v.loaded) = (2, 0, 1 << 10);
        assert_eq!(bot.think(&v).weapon, 10, "kept the spent pistol up");
    }

    #[test]
    fn a_dry_primary_at_range_reloads_instead() {
        let (mut bot, mut v) = engaged(21);
        v.pistol = Some(2);
        v.clip = 0;
        v.enemy = Some(EnemyView {
            slot: 3,
            origin: [900.0, 0.0, 124.0],
            velocity: [0.0; 3],
        });
        let cmd = bot.think(&v);
        assert_eq!(cmd.weapon, 10);
        assert!(cmd.wbuttons & WBUTTON_RELOAD != 0);
    }

    #[test]
    fn a_scope_stays_down_at_point_blank() {
        let (mut bot, mut v) = engaged(23);
        v.sniper = true;
        for _ in 0..20 {
            assert_eq!(bot.think(&v).buttons & BUTTON_ADS, 0, "scoped at 100");
        }
        v.enemy = Some(EnemyView {
            slot: 3,
            origin: [900.0, 0.0, 124.0],
            velocity: [0.0; 3],
        });
        assert!(bot.think(&v).buttons & BUTTON_ADS != 0, "no scope at 900");
    }

    #[test]
    fn an_empty_clip_is_reloaded() {
        let mut bot = Bot::new("allies", true, 1);
        let mut v = view();
        v.clip = 0;
        let cmd = bot.think(&v);
        assert!(cmd.wbuttons & WBUTTON_RELOAD != 0, "a dry clip reloads");
    }

    /// A clip under half reloads once no enemy has been in sight for
    /// [`QUIET_RELOAD_TICKS`]; half or more, or in a fight, it does not.
    #[test]
    fn a_bot_reloads_a_short_clip_between_fights() {
        let mut v = view();
        v.grenade = None;
        v.survive.clip_size = 8;
        v.clip = 3;
        let mut bot = Bot::new("allies", true, 1);
        let reloads = |bot: &mut Bot, v: &BotView| {
            (0..QUIET_RELOAD_TICKS + 1).any(|_| bot.think(v).wbuttons & WBUTTON_RELOAD != 0)
        };
        assert!(reloads(&mut bot, &v), "a short clip reloads");
        v.clip = 4;
        assert!(
            !reloads(&mut Bot::new("allies", true, 1), &v),
            "half a clip reloaded"
        );
        v.clip = 3;
        v.enemy = Some(EnemyView {
            slot: 3,
            origin: [2000.0, 0.0, 40.0],
            velocity: [0.0; 3],
        });
        assert!(
            !reloads(&mut Bot::new("allies", true, 1), &v),
            "reloaded mid-fight"
        );
    }

    /// An enemy no round goes at (an LMG that will not deploy, here) is
    /// given up on after [`FUTILE_TICKS`] for [`IGNORE_TICKS`]; one being
    /// shot at is not.
    #[test]
    fn an_enemy_no_round_goes_at_is_given_up_on() {
        let (mut bot, mut v) = engaged(21);
        let fired = (0..FUTILE_TICKS).any(|_| bot.think(&v).buttons & BUTTON_ATTACK != 0);
        assert!(fired && !bot.ignores(3), "a rifle fires");

        let (mut bot, mut v2) = engaged(21);
        v2.lmg = true;
        v2.automatic = true;
        for _ in 0..FUTILE_TICKS - 1 {
            assert_eq!(bot.think(&v2).buttons & BUTTON_ATTACK, 0);
        }
        assert!(!bot.ignores(3));
        bot.think(&v2);
        assert!(bot.ignores(3), "nothing fired in {FUTILE_TICKS} ticks");
        v.enemy = None;
        for _ in 0..IGNORE_TICKS - 1 {
            bot.think(&v);
        }
        assert!(bot.ignores(3));
        bot.think(&v);
        assert!(!bot.ignores(3), "back in view after {IGNORE_TICKS} ticks");
    }

    /// A rocket launcher in hand fires at no enemy, near or far; the
    /// pistol comes out against one, the gun first when there is one.
    #[test]
    fn no_rocket_goes_at_an_enemy() {
        let (mut bot, mut v) = engaged(21);
        v.rocket = Some(10);
        v.pistol = Some(2);
        v.weapons_held |= 1 << 2;
        v.loaded |= 1 << 2;
        v.clip = 1;
        let cmd = bot.think(&v);
        assert_eq!(cmd.buttons & BUTTON_ATTACK, 0, "a rocket at 100 units");
        assert_eq!(cmd.weapon, 2, "the pistol comes out");

        let (mut bot, mut v) = engaged(21);
        v.rocket = Some(10);
        v.clip = 1;
        v.has_ads = false;
        v.enemy.as_mut().unwrap().origin = [900.0, 0.0, 124.0];
        let fired = (0..20).any(|_| bot.think(&v).buttons & BUTTON_ATTACK != 0);
        assert!(!fired, "a rocket at 900 units");

        v.gun = Some(4);
        v.weapons_held |= 1 << 4;
        v.loaded |= 1 << 4;
        assert_eq!(bot.think(&v).weapon, 4);
    }

    #[test]
    fn a_lead_runs_ahead_by_the_rounds_flight() {
        // A rocket at 2800 on a hull crossing at 200: about a second's
        // flight, a little over for the longer way to the led point.
        let p = vehicle::lead([0.0; 3], [2800.0, 0.0, 0.0], [0.0, 200.0, 50.0], 2800.0);
        assert!((p[1] - 200.5).abs() < 0.5, "{p:?}");
        assert_eq!(p[2], 0.0, "a rise or fall of the target is not led");
        // A bullet goes at the body.
        let at = [500.0, 0.0, 0.0];
        assert_eq!(vehicle::lead([0.0; 3], at, [0.0, 100.0, 0.0], 0.0), at);
    }
}
