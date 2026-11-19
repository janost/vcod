//! The walk: the wander brain's movement, steering, stalls and unsticking, headings and ladders.

use super::*;

/// A live grenade this near is run from: a frag's blast is 350 across,
/// and a bot that followed its enemy after a throw, or stood where the
/// frag rolled back to, took its own blast (bot-objectives.md 6).
pub(crate) const FLEE_REACH: f32 = 400.0;
pub(crate) const FLEE_HEIGHT: f32 = 160.0;
/// The move keys' share a bot spacing out behind a teammate keeps
/// (`Bot::spaced`): pmove scales the pace by the key, so about half.
const EASE_KEY: f32 = 0.5;
/// A guard on its post looks this far either side of its objective, degrees.
pub(super) const WATCH_DEG: i32 = 60;
/// Ticks a bot stands for a held room's frag before it goes in without.
const ROOM_WAIT_TICKS: u32 = 20;

impl Bot {
    /// Held in the air, off the rungs and unlinked, without moving for
    /// [`WEDGE_TICKS`]: a crease that holds a body at rest under any key
    /// (bot-navigation.md section 3, "mp_hurtgen's crease"). A player
    /// there has `kill` left, and so does the bot, once per wait.
    /// Or [`BotView::stranded`] for [`STRAND_TICKS`]: the wander keeps off
    /// the fall, and the graph has no way on.
    pub(super) fn wedged(&mut self, view: &BotView) -> bool {
        self.strand_ticks = if view.stranded {
            self.strand_ticks + 1
        } else {
            0
        };
        if self.strand_ticks >= STRAND_TICKS {
            self.strand_ticks = 0;
            return true;
        }
        if view.on_ground
            || view.on_ladder
            || view.linked
            || dist_sq(view.origin, self.wedge_at) > WEDGE_SLACK * WEDGE_SLACK
        {
            self.wedge_at = view.origin;
            self.wedge_ticks = 0;
            return false;
        }
        self.wedge_ticks += 1;
        if self.wedge_ticks < WEDGE_TICKS {
            return false;
        }
        self.wedge_ticks = 0;
        true
    }

    /// The bot eases to a walk behind a teammate just ahead
    /// ([`BotView::mate_ahead`]): out of a fight and a retreat, not
    /// carrying, not with a bomb down (a defuse or its guard is a race).
    fn spaced(&self, view: &BotView) -> bool {
        view.mate_ahead
            && self.team_play
            && self.squad
            && view.enemy.is_none()
            && self.retreat.is_none()
            && !self.carrying(view)
            && view.sd.as_ref().is_none_or(|sd| sd.bomb.is_none())
    }

    /// A live grenade near: the keys run from it whatever the fight wants,
    /// the view stays where `yaw` (the cmd's own) has it.
    pub(super) fn flee(&self, view: &BotView, cmd: &mut UserCmd, yaw: f32) {
        if let Some(f) = view.frag_near
            && !view.deployed
            && !self.objective_busy(view)
        {
            let away = (view.origin[1] - f[1])
                .atan2(view.origin[0] - f[0])
                .to_degrees();
            let d = yaw_diff(away, yaw).to_radians();
            cmd.forward = (d.cos() * 127.0).round() as i8;
            cmd.right = (-d.sin() * 127.0).round() as i8;
            cmd.up = 0;
            cmd.wbuttons &= !(msg::WBUTTON_CROUCH
                | msg::WBUTTON_PRONE
                | msg::WBUTTON_LEAN_LEFT
                | msg::WBUTTON_LEAN_RIGHT);
        }
    }

    pub(super) fn think_wander(&mut self, view: &BotView, mut cmd: UserCmd) -> UserCmd {
        if let Some(cmd) = self.think_objective(view, cmd) {
            return cmd;
        }
        // A frag goes at an enemy the gun cannot reach ([`Bot::throw_spot`])
        // once the cooldown is spent and the server found a clear lob with
        // no teammate by the landing, from a body at rest: one that started
        // on the run slid on through the switch to where a wall over the
        // eye stopped every lob, and the hold let go into it. The switch
        // itself is the stage machine below.
        self.grenade_cooldown = self.grenade_cooldown.saturating_sub(1);
        if self.shoot
            && self.grenade_cooldown == 0
            && self.switch_to.is_none()
            && !view.deployed
            && view.speed < THROW_SPEED
            && !view.on_ladder
            && view.eye_settled
            && view.lob.is_some()
            && let (Some(g), Some(at)) = (view.grenade, self.throw_spot(view))
        {
            self.grenade_cooldown = GRENADE_COOLDOWN_TICKS;
            self.rifle = view.weapon;
            self.stage = Stage::ToGrenade;
            if self.room_frag(view) == Some(at) {
                self.room_thrown = Some(at);
            }
            // The start tick aims as the stages do: a cmd left at null
            // angles turned the view onto `delta_angles`, off the enemy,
            // and a target never recorded left the release no lob
            // (bot-objectives.md 6).
            self.throw_at = Some(at);
            self.throw_for = view
                .enemy
                .map_or(ThrowFor::Spot, |e| ThrowFor::Enemy(e.slot));
            cmd.angles = self.lob_angles(view);
            cmd.weapon = g;
            return cmd;
        }
        // A held room's frag waits on a body at rest: with a lob found the
        // bot stops in range for at most [`ROOM_WAIT_TICKS`], then goes in
        // without it.
        if self.shoot
            && self.grenade_cooldown == 0
            && self.switch_to.is_none()
            && view.grenade.is_some()
            && view.lob.is_some()
            && !view.on_ladder
            && let Some(at) = self.room_frag(view)
            && dist_sq(view.origin, at) < GRENADE_RANGE * GRENADE_RANGE
            && dist_sq(view.origin, at) > GRENADE_MIN * GRENADE_MIN
        {
            self.room_wait += 1;
            if self.room_wait > ROOM_WAIT_TICKS {
                self.room_thrown = Some(at);
            }
            cmd.angles = self.lob_angles(view);
            return cmd;
        }

        // A body that has not left a 15-unit circle in ten ticks (2 units
        // creeping) is stuck, path or not, and takes a random heading for a spell. One standing
        // at its objective, or held by a link, means to stay put.
        self.lmg_rest = self.lmg_rest.saturating_sub(1);
        self.lmg_linger = self.lmg_linger.saturating_sub(1);
        // A bipod down holds the body; it means to stay put.
        let standing = self.objective_standing(view)
            || view.deployed
            || self.holding_cover(view)
            || self.start_held(view);
        // On its post: it watches the objective and crouches where that
        // hides it from the ways in.
        let posted = self
            .objective_target(view)
            .and_then(|t| self.post(view, &t))
            .filter(|p| standing && !view.linked && Self::on_post(view, p));
        self.stall_ticks += 1;
        self.stall_reach = self
            .stall_reach
            .max(dist_sq(view.origin, self.stall_origin));
        self.rungs_ago = if view.on_ladder {
            0
        } else {
            self.rungs_ago.saturating_add(1)
        };
        // Coming to rest at a leap's foot is not being stuck either.
        if standing || view.stop {
            self.restart_stall(view.origin);
            self.unstick_ticks = 0;
        } else if (view.unpin.is_some() || view.detour) && self.unstick_ticks == 0 {
            // A pin with a heading out of it gets its run aside first, and
            // a body in the way the plan round it.
            self.restart_stall(view.origin);
        } else if self.stall_ticks >= STALL_TICKS {
            // A creep covers about 8 units in ten ticks and is stuck only
            // when it stops: a back down over a thin ladder's top creeps 20
            // and more (mp_depot's at (-2750, -1217), section 3).
            let radius = if self.creep.creeping() {
                2.0
            } else {
                STALL_RADIUS
            };
            // Left the circle and came back is no stall: a body that flew
            // past a node down mp_ship's crow's nest spar walked back up to
            // it and on, and the spell's heading ran it off the spar's side.
            if self.stall_reach < radius * radius
                && !self.closes_in(view, self.stall_origin)
                && !self.topping(view)
            {
                // Pinned mid-spell: another heading, but the spell still
                // ends on time. Chained spells never handed the body back
                // to its waypoint (mp_ship's hold ladder head).
                if self.unstick_ticks == 0 {
                    // Stuck again where the last spell started: a pocket a
                    // drop is the only way out of (mp_ship's rim at (4223,
                    // 345, 276)).
                    let again = dist_sq(view.origin, self.unstick_at) < CORNER * CORNER;
                    self.unstick_tries = if again { self.unstick_tries + 1 } else { 0 };
                    self.unstick_at = view.origin;
                    self.unstick_ticks = UNSTICK_TICKS;
                    self.rung_spell = self.rungs_ago <= RUNG_GRACE;
                    self.stuck = true;
                    // A creep's held heading belongs to where it began.
                    self.creep = Default::default();
                }
                self.pick_heading(view);
            } else {
                self.restart_stall(view.origin);
            }
        }
        let spell = self.unstick_ticks > 0;
        let steering = !standing && view.waypoint.is_some() && self.unstick_ticks == 0;
        // A leap's foot is slowed onto only on its floor: one at a ladder's
        // head is climbed to at the full rate first.
        let foot = view.waypoint.filter(|w| {
            steering && view.stop && !view.on_ladder && (w[2] - view.origin[2]).abs() < CLIMB_HEIGHT
        });
        let (mut pitch, mut yaw, mut forward) = match view.waypoint {
            // On guard: look about, feet still.
            _ if standing => {
                if self.heading_ticks == 0 {
                    match posted {
                        Some(p) => self.watch_heading(view, p.look),
                        None => self.pick_heading(view),
                    }
                } else {
                    self.heading_ticks -= 1;
                }
                (0.0, self.heading, 0)
            }
            Some(w) if self.unstick_ticks == 0 => {
                let (pitch, yaw, forward, right) = self.steer(view, w);
                if view.rungs_hold && view.on_ladder {
                    // No key: the rungs hold the body where it is.
                    (pitch, yaw, 0)
                } else {
                    cmd.right = right;
                    (pitch, yaw, forward)
                }
            }
            // On a ladder with no way to steer, or pinned on it: down.
            // The heading is nothing to a body on a ladder, and the level
            // view of a wander climbs at under two thirds of the rate, which
            // held bots under a spar on mp_ship's ladders for good.
            _ if view.on_ladder => {
                self.unstick_ticks = self.unstick_ticks.saturating_sub(1);
                (crate::nav::LADDER_PITCH, self.heading, 127)
            }
            // Just off the rungs, a spell or a wait for a plan holds still:
            // a climb that bobs under a ladder's top lets go and grabs again,
            // and a heading taken there, on the head's plate or in the air
            // over it, ran bots off mp_ship's crow's nest plate down its
            // hatch (bot-navigation.md section 3).
            _ if (self.rung_spell && self.unstick_ticks > 0)
                || (self.unstick_ticks == 0 && !view.on_ground && self.rungs_ago <= RUNG_GRACE) =>
            {
                self.unstick_ticks = self.unstick_ticks.saturating_sub(1);
                (0.0, self.heading, 0)
            }
            _ => {
                self.unstick_ticks = self.unstick_ticks.saturating_sub(1);
                if self.heading_ticks == 0
                    || self.shuns(view, self.heading)
                    || self.sliding_off(view)
                {
                    self.pick_heading(view);
                } else {
                    self.heading_ticks -= 1;
                }
                (0.0, self.heading, 127)
            }
        };
        // A spell's end starts the stall window afresh: one straddling the
        // turn back to the waypoint reads the way out and the way back as
        // standing still (mp_ship's ladder at x 3792, backed down a spell
        // at a time).
        if spell && self.unstick_ticks == 0 {
            self.restart_stall(view.origin);
        }
        // On level ground with a remembered spot in range, the view turns
        // toward it while the keys keep the path, so a returning enemy is
        // already near the crosshair. The reaction delay is `track`'s
        // business either way.
        // A UO bot with the breath to sprint a straight that ends short of
        // the spot runs it looking along the path: turned to the spot, the
        // keys leave the line and the sprint never starts, and in a game of
        // running fights that left 64 sprinted leg-ticks in 90 s (seed 19).
        let sprints_on = |r: &Recall| {
            view.fatigue
                .is_some_and(|f| view.sprinting || f >= SPRINT_FRESH)
                && view.sprint_room >= SPRINT_ROOM
                && dist_sq(view.origin, r.at) > view.sprint_room * view.sprint_room
        };
        // Not on a mantle edge (bot-combat.md 17.1). Not from under
        // `RECALL_PACE` either: a turned key backs and strafes at retail's
        // 0.7 and 0.8 scales, and a body against a wall slides along it
        // only at the run the graph proved, as at mp_ship's deck corner
        // at (3564, 8, 352) (bot-navigation.md, "The ship's leftovers").
        let mut recall_look = false;
        if self.shoot
            && view.enemy.is_none()
            && !view.mantle
            && view.speed >= RECALL_PACE
            && pitch == 0.0
            && forward == 127
            && let Some(r) = self.recall
            && dist_sq(view.origin, r.at) < SHOOT_RANGE * SHOOT_RANGE
            && !sprints_on(&r)
        {
            let (tp, ty) = aim_angles(view, r.at);
            let look = self.turn_toward(view, [tp, ty]);
            // The path heading relative to the view, counter-clockwise.
            let d = yaw_diff(yaw, look[1]).to_radians();
            forward = (d.cos() * 127.0).round() as i8;
            cmd.right = (-d.sin() * 127.0).round() as i8;
            [pitch, yaw] = look;
            recall_look = true;
        }
        // Pulled back to its objective, a guard walks the last stretch in
        // looking at it while the keys keep the path, as a recall does
        // (bot-objectives.md 14).
        let careful = self.careful(view).filter(|_| steering);
        if !recall_look
            && view.enemy.is_none()
            && !view.mantle
            && view.on_ground
            && !view.on_ladder
            && pitch == 0.0
            && forward == 127
            && let Some(at) = careful
        {
            let (_, ty) = aim_angles(view, at);
            let look = self.turn_toward(view, [0.0, ty]);
            let d = yaw_diff(yaw, look[1]).to_radians();
            forward = (d.cos() * 127.0).round() as i8;
            cmd.right = (-d.sin() * 127.0).round() as i8;
            [pitch, yaw] = look;
            recall_look = true;
        }
        // Slowing onto a leap's foot, then standing on it. Under a recall
        // look the keys are turned off the view, so both scale: a forward
        // key set along the view walked mp_carentan's bots away from their
        // stop into a corner (bot-combat.md 17.1).
        if let Some(w) = foot {
            let flat = (w[0] - view.origin[0]).hypot(w[1] - view.origin[1]);
            let slow = if flat < crate::nav::LEAP_FOOT * 0.5 {
                0.0
            } else {
                (flat / LEAP_SLOW).min(1.0)
            };
            if recall_look {
                forward = (f32::from(forward) * slow) as i8;
                cmd.right = (f32::from(cmd.right) * slow) as i8;
            } else {
                forward = if slow == 0.0 {
                    0
                } else {
                    (flat / LEAP_SLOW * 127.0).min(127.0) as i8
                };
            }
        }
        // Engaging overrides all of this: `footwork` owns the move keys and
        // the aim owns the view.
        cmd.forward = forward;
        if steering
            && view.jump
            && let Some(w) = view.waypoint
        {
            let flat = (w[0] - view.origin[0]).hypot(w[1] - view.origin[1]);
            let rise = w[2] - view.origin[2];
            let cue = if view.leap {
                crate::nav::leap_cue(view.lip, flat, rise)
            } else {
                // The walk that proved the edge held its key at the top
                // every tick: a tick counts as pinned only with the key held
                // at this waypoint, closing on it under the pinned pace (the
                // walk, straight at the top, closes as far as it moves), and
                // not in the air off a jump. A body set off from rest on a
                // foot has not yet held its key (`nav::Follower::jump_wait`).
                // No room test: the edge was proved again by this rule
                // (`nav::NavGraph::reprove_jumps`). Read off the distance
                // moved instead, a bot sliding along the rail over mp_ship's
                // stair drop at (5402, 321, 344) jumped late, past it, into
                // the crease at (5445, 355, 292) (bot-navigation.md, "The
                // jump cue and the re-proof"). On the ground the body has to
                // be slow too, as the walk measures it: one that struck the
                // slanted face of mp_ship's crate at (6207, -74, 80) and slid
                // along it at 2.6 a tick jumped where the walk slid on to the
                // corner, and came down on the crate beside it (seed 661);
                // read so off the ground as well, bots sliding on the hull's
                // curve at (2200, -300, -45) never jumped and stuck in its
                // crease (bot-navigation.md section 3.7, "The recheck's
                // leftovers").
                let held = self.pressed == Some(w);
                if !held {
                    self.pin = crate::nav::PinJump::default();
                }
                self.pin_air &= !(view.on_ground || view.on_ladder);
                let armed = !crate::nav::on_jump_top(flat, rise);
                let climbed = self.pin_from.map_or(0.0, |p| view.origin[2] - p[2]);
                let under = crate::nav::jump_under(flat, rise, climbed);
                let to = glam::Vec2::new(w[0] - view.origin[0], w[1] - view.origin[1]);
                let v = glam::Vec2::new(view.velocity[0], view.velocity[1]);
                let closing = v.dot(to.normalize_or_zero()) * (FRAME_MS as f32 / 1000.0);
                let moved = self.pin_from.map_or(f32::INFINITY, |p| {
                    glam::Vec3::from(p).distance(glam::Vec3::from(view.origin))
                });
                let slow = closing < crate::nav::PINNED
                    && !(view.on_ground && moved >= crate::nav::PINNED);
                let stalled = held && slow && !self.pin_air;
                self.pin.before(armed, view.on_ground, under, || true)
                    || self
                        .pin
                        .after(armed, stalled, view.on_ground, view.sliding, true, || true)
                        == crate::nav::PinTick::Jump
            };
            cmd.up = crate::nav::jump_key(view.on_ground, cue, self.jump_held);
            self.pin_air |= cmd.up > 0;
        } else {
            self.pin = crate::nav::PinJump::default();
            self.pin_air = false;
        }
        self.pin_from = Some(view.origin);
        // A crouched jump edge: `PM_CheckJump` refuses a crouched body, so
        // the crouch starts on the cmd after the jump's and is held, as a
        // retail client holds it, until the body is on the ground again.
        self.crouch_air &= !view.on_ground;
        if self.crouch_air {
            cmd.wbuttons |= msg::WBUTTON_CROUCH;
            cmd.up = -127;
        }
        self.crouch_air |= view.crouch_jump && cmd.up > 0;
        if posted.is_some_and(|p| p.crouch) && view.enemy.is_none() {
            cmd.wbuttons |= msg::WBUTTON_CROUCH;
            cmd.up = -127;
        }
        // The last of a careful walk in, crouched.
        if careful.is_some_and(|at| dist_sq(view.origin, at) < CAREFUL_CROUCH * CAREFUL_CROUCH)
            && view.enemy.is_none()
            && view.on_ground
            && !view.on_ladder
            && !view.jump
            && !view.mantle
        {
            cmd.wbuttons |= msg::WBUTTON_CROUCH;
            cmd.up = -127;
        }
        // Off the rungs backwards onto a floor behind: jump facing the
        // rungs, which reflects the jump out, then brake facing the same
        // way until the body lands, as `nav::push_off` proved it.
        if view.on_ground || (view.on_ladder && view.push_off.is_none()) {
            self.pushed_off = None;
        }
        if let Some(y) = view.push_off.filter(|_| steering) {
            (pitch, yaw, forward) = (0.0, y, 0);
            cmd.right = 0;
            cmd.up = if self.jump_held { 0 } else { 127 };
            self.pushed_off = Some(y);
        } else if let Some(y) = self.pushed_off {
            (pitch, yaw, forward) = (0.0, y, 127);
            cmd.right = 0;
        }
        // Pinned in a crease: run aside for a few ticks, then at the
        // waypoint again with the pace that slides through.
        if let Some(y) = view.unpin.filter(|_| steering) {
            self.unpin = Some((y, crate::nav::UNPIN_TICKS));
        }
        match self.unpin.as_mut() {
            Some((y, n)) if *n > 0 && steering && view.on_ground => {
                (pitch, yaw, forward) = (0.0, *y, 127);
                cmd.right = 0;
                *n -= 1;
            }
            _ => self.unpin = None,
        }
        // A plain run on the ground: the view comes round to the path's
        // heading at `walk_turn_deg` a tick and the keys keep the body on
        // the heading meanwhile. A climb, a back down, a jump, a run off a
        // drop and a leap's or a creep's foot keep the view the walk that
        // proved them had (bot-combat.md section 11). Handed back from a
        // view something else owned, the view takes the heading at once:
        // brought round from the threat, a bot leaving its cover saw the
        // same enemy again and dug back in (11.4).
        if steering
            && view.on_ground
            && !view.on_ladder
            && pitch == 0.0
            && forward == 127
            && cmd.right == 0
            && cmd.up == 0
            && foot.is_none()
            && !view.stop
            && !view.jump
            && !view.run_off
            && self.pushed_off.is_none()
            && self.unpin.is_none()
            && !self.creep.creeping()
            && !self.view_owned
        {
            (forward, cmd.right, yaw) = self.walk_turn(view, yaw);
        }
        let walked = [pitch, yaw];
        // On the ground the keys are cut to the graph's pace: its walks ran
        // at 190 units/s, and a light weapon's 215 to 228 flies a run off
        // mp_ship's mast platform at (3564, -127, 896) over the narrow ledge
        // at 760 it lands on (bot-navigation.md section 3). The air and
        // ladder moves take no weapon scale, and a jump's tick moves in
        // the air.
        // On UO a long straight leg is sprinted while there is breath for
        // it; the sprint leaves the weapon's scale out (coduo-movement.md
        // 2-4, bot-navigation.md "Other rules").
        // A plain run behind a teammate just ahead eases to a walk
        // (`Bot::spaced`): a team in single file opens out to a blast's
        // width apart instead of falling to one burst or frag.
        let ease = (self.spaced(view)
            || careful
                .is_some_and(|at| dist_sq(view.origin, at) >= CAREFUL_CROUCH * CAREFUL_CROUCH))
            && steering
            && view.on_ground
            && !view.on_ladder
            && cmd.up == 0
            && foot.is_none()
            && !view.stop
            && !view.jump
            && !view.run_off
            && !view.mantle
            && self.pushed_off.is_none()
            && self.unpin.is_none()
            && !self.creep.creeping();
        let flees = self.flees(view);
        self.sprint_leg = !ease
            && view.fatigue.is_some()
            && steering
            && forward == 127
            && cmd.right == 0
            && cmd.up == 0
            && foot.is_none()
            && !view.stop
            && !view.jump
            && !view.run_off
            && self.pushed_off.is_none()
            && self.unpin.is_none()
            && (view.enemy.is_none() || flees)
            && view.on_ground
            && !view.on_ladder
            && view.sprint_room
                >= if view.sprinting {
                    SPRINT_TAIL
                } else {
                    SPRINT_ROOM
                };
        let sprint = self.sprint_leg
            && view
                .fatigue
                .is_some_and(|f| view.sprinting || f >= SPRINT_FRESH);
        if sprint {
            cmd.buttons |= msg::BUTTON_SPRINT_V22;
        } else if view.on_ground && !view.on_ladder && cmd.up == 0 {
            forward = crate::nav::paced_key(forward, view.move_scale);
            cmd.right = crate::nav::paced_key(cmd.right, view.move_scale);
        }
        if ease {
            forward = (f32::from(forward) * EASE_KEY) as i8;
            cmd.right = (f32::from(cmd.right) * EASE_KEY) as i8;
        }
        cmd.forward = forward;
        self.jump_held = cmd.up > 0;
        self.pressed = view.waypoint.filter(|_| forward != 0);
        // A `bas` charge about to go: the keys keep running the graph's way
        // out whatever the fight wants, and no bipod goes down. A planter
        // and a defender fighting in the charge room both stood in the
        // blast to the end (seed 5). A retrieval carrier runs on the same
        // way (`team`'s `carrying`), and so does a bot by a burning hull.
        let clearing = Self::blast_near(view).is_some()
            || wreck_near(&view.wrecks, view.origin).is_some()
            || self.carrying(view);
        let path = ((cmd.forward, cmd.right, cmd.up, cmd.wbuttons), yaw);
        if self.shoot {
            self.track(view);
            let keys = (cmd.forward, cmd.right, cmd.up, cmd.wbuttons);
            if !clearing && let Some(aim) = self.think_armor(view, &mut cmd, yaw) {
                [pitch, yaw] = aim;
            } else if let Some(e) = view.enemy
                && !(clearing && view.lmg)
                && !flees
                && let Some(aim) = self.engage(view, e, &mut cmd)
            {
                if clearing {
                    (cmd.forward, cmd.right) = rekey(keys.0, keys.1, yaw, aim[1]);
                    (cmd.up, cmd.wbuttons) = (keys.2, keys.3);
                    cmd.buttons &= !msg::BUTTON_ADS;
                }
                [pitch, yaw] = aim;
            } else if view.lmg && view.deployed && self.lmg_linger > 0 && !clearing {
                // The bipod stays down a while for the target to show again.
                cmd.buttons |= msg::BUTTON_ADS;
                cmd.forward = 0;
                cmd.right = 0;
                [pitch, yaw] = [yaw_diff(view.view[0], 0.0), view.view[1]];
            } else if view.enemy.is_none()
                && !clearing
                && let Some(look) = self.face_hurt(view, &mut cmd, yaw)
            {
                [pitch, yaw] = look;
            } else if view.enemy.is_none()
                && !clearing
                && let Some(look) = self.cover_unseen(view, &mut cmd)
            {
                [pitch, yaw] = look;
            }
        }
        [pitch, yaw] = self.retreat_keys(view, &mut cmd, path.0, path.1, [pitch, yaw]);
        self.flee(view, &mut cmd, yaw);
        // A switch ends when it lands, or when the weapon is gone (dropped,
        // or swapped at a pickup) and the byte would read as a holster for
        // good.
        if let Some(w) = self.switch_to
            && (view.weapon == w
                || view
                    .weapons_held
                    .checked_shr(u32::from(w))
                    .is_none_or(|m| m & 1 == 0))
        {
            self.switch_to = None;
        }
        if self.shoot {
            self.off_launcher(view);
            self.off_dry(view);
            self.sidearm(view);
        }
        if let Some(w) = self.switch_to {
            cmd.weapon = w;
        }
        // A dry clip reloads, and so does one under half once no enemy has
        // been in sight for [`QUIET_RELOAD_TICKS`] (not the pistol, which
        // goes back by then), unless a switch is putting it away or there
        // is nothing to load; the tap is suppressed while the machine is
        // busy so a held bit cannot restart the reload it started.
        let tactical = self.quiet_ticks >= QUIET_RELOAD_TICKS
            && view.clip > 0
            && 2 * view.clip < view.survive.clip_size
            && view.pistol != Some(view.weapon)
            && view.grenade != Some(view.weapon);
        if (view.clip == 0 || tactical)
            && view.busy_ms == 0
            && self.switch_to.is_none()
            && view.loaded >> view.weapon.min(63) & 1 == 1
        {
            cmd.wbuttons |= msg::WBUTTON_RELOAD;
        }
        self.retreat_update(view);
        self.walking = steering && view.on_ground && !view.on_ladder && !self.engaged();
        self.view_owned = recall_look || [pitch, yaw] != walked;
        cmd.angles = cmd_angles(view, [pitch, yaw]);
        cmd
    }

    /// The body has closed on its waypoint by [`CLOSING`] since `from`,
    /// across plus up or down: the measure the follower's own give-up runs
    /// on (`nav::Follower::waypoint`). A slow way on is no stall. On
    /// mp_ship's crow's nest a bot backing for the deck slid along the
    /// hatch's rim at under a unit a tick, one climbing bobbed under the
    /// head's lip, one turned onto the head off its exit; each read as
    /// stuck, and the spell ran it down the hatch past the rungs
    /// (bot-navigation.md section 3).
    fn closes_in(&self, view: &BotView, from: [f32; 3]) -> bool {
        let Some(w) = view.waypoint else {
            return false;
        };
        let left = |p: [f32; 3]| (w[0] - p[0]).hypot(w[1] - p[1]) + (w[2] - p[2]).abs();
        left(from) - left(view.origin) >= CLOSING
    }

    /// On or just off the rungs right under a waypoint over them: a climb
    /// that bobs under the ladder's top, let go 8 under it, falling back
    /// and grabbing again until a step-up clears the lip. It steps on in
    /// about ten ticks; the stall circle read it as stuck and the spell
    /// climbed down, again and again under mp_ship's crow's nest plate
    /// (bot-navigation.md section 3).
    fn topping(&self, view: &BotView) -> bool {
        let Some(w) = view.waypoint else {
            return false;
        };
        let rise = w[2] - view.origin[2];
        self.rungs_ago <= RUNG_GRACE
            && rise > 0.0
            && rise < vcod_common::pmove::JUMP_HEIGHT
            && (w[0] - view.origin[0]).hypot(w[1] - view.origin[1]) < TOPPING_REACH
    }

    /// (pitch, yaw, forward) that take the body toward waypoint `to`. A
    /// waypoint well above, or the body on a ladder with the waypoint above,
    /// is climbed looking up; one well below, or below while on a ladder, is
    /// backed toward facing away, creeping at the lip. That is how the
    /// graph's walks proved the edge (`crate::nav`).
    pub(super) fn steer(&mut self, view: &BotView, to: [f32; 3]) -> (f32, f32, i8, i8) {
        let from = view.origin;
        let yaw = (to[1] - from[1]).atan2(to[0] - from[0]).to_degrees();
        let dz = to[2] - from[2];
        // A head a step below on a lip just across the face, within a body's
        // half-width of the rungs, is climbed over: backing off the rungs
        // there pushed the body off the face and short of mp_ship's hold
        // ladder lip at (4332, -87, 615). Farther across, the back off
        // stays: two bots side by side over the top of the ladder at
        // x 3792 pinned each other there. Rungs that let go only past their
        // top (`BotView::tops_out`) are climbed on for any head across:
        // backed down once level with the floor 77 across mp_matmata's
        // ladder at (5025, 6749), a CoD2 bot went up and down to the end
        // (bot-objectives.md 11.4).
        let n = view.ladder_normal;
        let across = (to[0] - from[0]) * n[0] + (to[1] - from[1]) * n[1];
        let over_top = dz > -vcod_common::pmove::STEPSIZE
            && (across < -1.0 && view.tops_out
                || (-vcod_common::pmove::HALF_WIDTH..-1.0).contains(&across));
        // A jump edge was proved by a forward run whatever its drop: backed
        // off mp_ship's stair at (5402, 321, 344) for the floor 140 under
        // it, a bot jumped backwards onto the stair and slid down it
        // (seed 1275, bot-navigation.md section 3). A drop deeper than a
        // jump that the graph's run misses was backed down, and is backed
        // here too (`BotView::back_drop`).
        let backs = (dz < -BACK_DOWN_HEIGHT || view.back_drop) && !view.run_off && !view.jump;
        if backs || (view.on_ladder && dz < 0.0 && !over_top) {
            let flat = (to[0] - from[0]).hypot(to[1] - from[1]);
            let forward = crate::nav::back_move(flat, view.on_ladder);
            // Off the head of a ladder whose fall kills, the creep faces
            // the rungs and strafes to their middle: backed toward the
            // foot from a few units off the line, bots slid past mp_ship's
            // crow's nest rungs down the hatch (bot-navigation.md section 3).
            if !view.on_ladder
                && forward == -crate::nav::CREEP
                && let Some((face, middle)) = view.back_face
            {
                let yaw = self.creep.hold(forward, face);
                let right = along_face(yaw, from, middle).clamp(forward, -forward);
                return (-crate::nav::LADDER_PITCH, yaw, forward, right);
            }
            let yaw = self.creep.hold(forward, yaw_diff(yaw + 180.0, 0.0));
            // On the rungs only the strafe moves a body along the face, and
            // only facing it: square into it, sliding toward the waypoint,
            // or a floor beside the ladder is passed under its lip
            // (mp_ship's hold ladder at z 304).
            if view.on_ladder
                && let Some(yaw) = crate::nav::climb_yaw(view.ladder_normal.into())
            {
                let right = along_face(yaw, from, [to[0], to[1]]);
                return (-crate::nav::LADDER_PITCH, yaw, forward, right);
            }
            return (-crate::nav::LADDER_PITCH, yaw, forward, 0);
        }
        self.creep.hold(127, yaw);
        if view.on_ladder && dz > vcod_common::pmove::STEPSIZE {
            // Square into the face, strafing toward the climb's line, until
            // level with the waypoint: a ledge beside the rungs is stepped
            // onto facing it.
            let yaw = crate::nav::climb_yaw(view.ladder_normal.into()).unwrap_or(yaw);
            let right = view
                .ladder_middle
                .filter(|_| dz > STRAFE_STOP)
                .map_or(0, |m| along_face(yaw, from, m));
            // Released while the body still slides the other way: under a
            // full-rate climb the strafe's accelerate adds nothing to a pace
            // the grab carried in, and only a released key bleeds it
            // (`pmove::ladder_move`). Held, a climber that grabbed mp_depot's
            // ladder at (-417, 1112) running west slid off its edge.
            let (s, c) = yaw.to_radians().sin_cos();
            let side = view.velocity[0] * s - view.velocity[1] * c;
            let right = if side * f32::from(right) < 0.0 && side.abs() > 1.0 {
                0
            } else {
                right
            };
            (-crate::nav::LADDER_PITCH, yaw, 127, right)
        } else if dz > CLIMB_HEIGHT || view.on_ladder {
            (-crate::nav::LADDER_PITCH, yaw, 127, 0)
        } else {
            (0.0, yaw, 127, 0)
        }
    }

    /// The walking view one tick on toward the path's heading `want`, at
    /// most [`Skill::walk_turn_deg`], and the (forward, right) keys that run
    /// the body along `want` from it ([`rekey`]'s turn), scaled so the
    /// larger is 127: `PM_CmdScale` then gives the run's full speed.
    fn walk_turn(&self, view: &BotView, want: f32) -> (i8, i8, f32) {
        let cur = view.view[1];
        let d = yaw_diff(want, cur);
        let most = self.skill.walk_turn_deg;
        if d.abs() <= most {
            return (127, 0, want);
        }
        let look = cur + d.clamp(-most, most);
        let off = yaw_diff(want, look).to_radians();
        let (f, r) = (off.cos(), -off.sin());
        let m = f.abs().max(r.abs());
        let key = |k: f32| (k / m * 127.0).round().clamp(-127.0, 127.0) as i8;
        (key(f), key(r), look)
    }

    /// A new stall window from `at`.
    pub(super) fn restart_stall(&mut self, at: [f32; 3]) {
        self.stall_origin = at;
        self.stall_ticks = 0;
        self.stall_reach = 0.0;
    }

    /// Whether this tick's [`Self::think`] can read [`BotView::drop_ahead`]:
    /// only a heading picked or kept off the waypoint does ([`Self::shuns`]),
    /// with no waypoint, on a spell, on guard, or on a stall window's last
    /// tick with the body still in its circle. The probe is the view's
    /// dearest part, so the server runs it only then (bot-navigation.md
    /// section 3).
    pub(crate) fn reads_drops(&self, view: &BotView) -> bool {
        let reach = self
            .stall_reach
            .max(dist_sq(view.origin, self.stall_origin));
        view.waypoint.is_none()
            || self.unstick_ticks > 0
            || (self.stall_ticks + 1 >= STALL_TICKS && reach < STALL_RADIUS * STALL_RADIUS)
            || self.objective_standing(view)
    }

    /// Whether this tick's [`Self::think`] can pick a wander heading, the
    /// one reader of [`BotView::wall_ahead`]: a tick [`Self::reads_drops`]
    /// names, or one on guard behind a bipod or at a cover spot (taken
    /// without asking whether a retreat holds it, which the think may
    /// start). Eight box traces a bot, so the server feels for walls only
    /// then.
    pub(crate) fn reads_walls(&self, view: &BotView) -> bool {
        self.reads_drops(view)
            || view.deployed
            || view
                .survive
                .cover
                .is_some_and(|c| retreat::at_cover(view.origin, c))
    }

    /// A guard's look from its post: toward `look`, the objective, up to
    /// [`WATCH_DEG`] either side, held as a wander heading is.
    fn watch_heading(&mut self, view: &BotView, look: [f32; 3]) {
        let toward = (look[1] - view.origin[1])
            .atan2(look[0] - view.origin[0])
            .to_degrees();
        let off = (self.rand() % (2 * WATCH_DEG + 1)) as f32 - WATCH_DEG as f32;
        self.heading = (toward + off).rem_euclid(360.0);
        self.heading_ticks = 40 + (self.rand() % 40) as u32;
        self.restart_stall(view.origin);
    }

    /// A fresh wander heading, and a new stall baseline to measure it by.
    pub(super) fn pick_heading(&mut self, view: &BotView) {
        self.heading = (self.rand() % 360) as f32;
        // Turned a step at a time off a hazard or a drop, beside the
        // heading too where it can (a heading into a wall slides along it:
        // on mp_ship's deck one 30 degrees off the drop slid a bot off it),
        // then off a hazard alone; boxed in by hazards, it goes anyway. A
        // heading into a wall goes nowhere, and reads as no drop, so it
        // comes after every open one (mp_ship's hold ladder head).
        let start = self.heading;
        let turn = |ok: &dyn Fn(f32) -> bool| {
            (0..8)
                .map(|i| (start + 45.0 * i as f32) % 360.0)
                .find(|h| ok(*h))
        };
        let wide = |h: f32| [-45.0, 0.0, 45.0].iter().all(|d| !self.shuns(view, h + d));
        let open = |h: f32| !view.wall_ahead[octant(h)];
        self.heading = turn(&|h| open(h) && wide(h))
            .or_else(|| turn(&|h| open(h) && !self.shuns(view, h)))
            .or_else(|| turn(&wide))
            .or_else(|| turn(&|h| !self.shuns(view, h)))
            .or_else(|| turn(&|h| !view.hazard_ahead[octant(h)]))
            .unwrap_or(start);
        self.heading_ticks = 40 + (self.rand() % 40) as u32;
        self.restart_stall(view.origin);
    }

    /// A hazard, or a drop, lies along `yaw`'s octant. A drop is fine to
    /// a cornered bot, and to one whose path drops: its waypoint lies more
    /// than a jump below. Neither takes one that hurts, unless the path
    /// drops as far: a back down onto a ladder is no licence to leave the
    /// roof (mp_depot's at (-2750, -1217), section 3).
    pub(super) fn shuns(&self, view: &BotView, yaw: f32) -> bool {
        let o = octant(yaw);
        let jump = vcod_common::pmove::JUMP_HEIGHT;
        let path_drop = view.waypoint.map_or(0.0, |w| view.origin[2] - w[2]);
        let path_drops = path_drop > jump;
        let shun = |depth: f32| {
            if path_drops {
                depth > FALL_HURTS && depth > path_drop + jump
            } else {
                self.unstick_tries < CORNERED || depth > FALL_HURTS
            }
        };
        view.hazard_ahead[o] || view.drop_ahead[o].is_some_and(shun)
    }

    /// The body moves toward a drop it shuns, whatever its heading: one
    /// pushed into a wall slides along it.
    fn sliding_off(&self, view: &BotView) -> bool {
        let [x, y, _] = view.velocity;
        x.hypot(y) > 1.0 && self.shuns(view, y.atan2(x).to_degrees())
    }
}

/// How far ahead of a wandering bot [`BotView::hazard_ahead`] looks.
pub const HAZARD_LOOK: f32 = 64.0;

/// The compass octant of a yaw in degrees, as [`BotView::hazard_ahead`]
/// indexes them.
pub fn octant(yaw: f32) -> usize {
    ((yaw.rem_euclid(360.0) + 22.5) / 45.0) as usize % 8
}

/// A cornered bot still keeps off a drop this deep: past
/// `bg_fallDamageMinHeight` the fall hurts. mp_ship's 280 off the 216
/// floor at x 2010 took a bot's health with it (section 3).
const FALL_HURTS: f32 = 256.0;
/// A climb this near the middle of the rungs along the face stops strafing.
const LADDER_CENTRED: f32 = 2.0;
/// A climb this near under its waypoint stops strafing: a body still
/// moving along the face when the step-up over the top comes has the step
/// reverted (`PM_StepSlideMove` keeps the plain slide when it got as far
/// along the velocity), and hangs under the lip once the ladder lets go 8
/// under its top. Two steps leave the bleed a fifth of a second.
const STRAFE_STOP: f32 = 2.0 * vcod_common::pmove::STEPSIZE;
/// Ticks a stuck bot spends on a random heading before its waypoint again.
pub(super) const UNSTICK_TICKS: u32 = 15;
/// The ticks of a stall window, and the circle a body that stays in it is
/// stuck in ([`Bot::think`]).
const STALL_TICKS: u32 = 10;
const STALL_RADIUS: f32 = 15.0;
/// A spell that starts this many ticks or fewer after the body was on the
/// rungs climbs down on them and holds still off them.
const RUNG_GRACE: u32 = 10;
/// How near a ladder's head, flat, a climb bobbing under it is not stuck
/// (`Bot::topping`).
const TOPPING_REACH: f32 = 24.0;
/// Five seconds held off the ground within a unit of one spot.
pub(super) const WEDGE_TICKS: u32 = 100;
/// Ten seconds nearest a node with no way on.
pub(super) const STRAND_TICKS: u32 = 200;
const WEDGE_SLACK: f32 = 1.0;
/// Unstick spells that start within [`CORNER`] units of the one before;
/// from this many on, the bot is cornered and a drop is a way out.
pub(super) const CORNERED: u32 = 3;
const CORNER: f32 = 96.0;
/// Inside this of a leap's foot, flat, the run slows in proportion.
const LEAP_SLOW: f32 = 48.0;
/// Under this pace, units/s, the keys keep the path's own run: no
/// pre-aim turns them.
pub(super) const RECALL_PACE: f32 = 66.0;
/// A waypoint this far above the feet is up a ladder: look up, where
/// `ladder_move` climbs at full rate.
const CLIMB_HEIGHT: f32 = 48.0;
/// A waypoint this far below is down a ladder or off a ledge: back onto it
/// facing the way it came, which is the side a ladder's grab traces toward
/// (the graph proved the edge the same way, `crate::nav`), unless a run off
/// arrives (`BotView::run_off`).
pub const BACK_DOWN_HEIGHT: f32 = 64.0;
/// The least a body closes on its waypoint in the stall window to count
/// as moving ([`Bot::closes_in`]).
const CLOSING: f32 = 2.0;
/// Ticks between one throw's start and the next's.
const GRENADE_COOLDOWN_TICKS: u32 = 200;
/// A throw starts from a body slower than this, units/s.
const THROW_SPEED: f32 = 60.0;
/// Ticks with no enemy in sight before a clip under half reloads.
pub(super) const QUIET_RELOAD_TICKS: u32 = 20;
/// A sprint starts with this much straight path ahead and this much of the
/// fatigue bar left, and stops this far from the straight's end, a tick or
/// two of friction back to the graph's pace.
const SPRINT_ROOM: f32 = 400.0;
const SPRINT_FRESH: f32 = 0.5;
const SPRINT_TAIL: f32 = 160.0;

/// The strafe key that slides a body on a ladder, facing it at `yaw`,
/// toward `m` along the face; none once within [`LADDER_CENTRED`].
pub(crate) fn along_face(yaw: f32, from: [f32; 3], m: [f32; 2]) -> i8 {
    let (s, c) = yaw.to_radians().sin_cos();
    let off = (m[0] - from[0]) * s - (m[1] - from[1]) * c;
    if off.abs() < LADDER_CENTRED {
        0
    } else {
        (off * 16.0).clamp(-64.0, 64.0) as i8
    }
}

/// The forward and right keys that move the body the way `forward` and
/// `right` did under a view at `from` (degrees), with the view at `to`.
pub(crate) fn rekey(forward: i8, right: i8, from: f32, to: f32) -> (i8, i8) {
    let d = yaw_diff(from, to).to_radians();
    let (f, r) = (forward as f32, right as f32);
    let key = |k: f32| k.round().clamp(-127.0, 127.0) as i8;
    (
        key(f * d.cos() + r * d.sin()),
        key(r * d.cos() - f * d.sin()),
    )
}
