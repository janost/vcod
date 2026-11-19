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
    fn pick_heading(&mut self, view: &BotView) {
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
    fn shuns(&self, view: &BotView, yaw: f32) -> bool {
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
const UNSTICK_TICKS: u32 = 15;
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
const WEDGE_TICKS: u32 = 100;
/// Ten seconds nearest a node with no way on.
const STRAND_TICKS: u32 = 200;
const WEDGE_SLACK: f32 = 1.0;
/// Unstick spells that start within [`CORNER`] units of the one before;
/// from this many on, the bot is cornered and a drop is a way out.
const CORNERED: u32 = 3;
const CORNER: f32 = 96.0;
/// Inside this of a leap's foot, flat, the run slows in proportion.
const LEAP_SLOW: f32 = 48.0;
/// Under this pace, units/s, the keys keep the path's own run: no
/// pre-aim turns them.
const RECALL_PACE: f32 = 66.0;
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bots::tests::*;

    #[test]
    fn a_wandering_bot_runs_forward_carrying_the_held_weapon() {
        let mut bot = Bot::new("allies", false, 1);
        let cmd = bot.think(&view());
        assert_eq!(cmd.forward, 127, "the wander holds forward");
        assert_eq!(cmd.weapon, 10, "the cmd carries the held weapon byte");
        assert_eq!(cmd.buttons, 0, "nothing is pressed without an enemy");
        assert_eq!(cmd.right, 0);
    }

    #[test]
    fn a_stalled_heading_is_swapped_for_a_new_one() {
        let mut bot = Bot::new("allies", false, 1);
        let v = view();
        let first = bot.think(&v).angles[1];
        // Grinding into a wall: ten ticks without moving off the stall spot.
        for _ in 0..20 {
            bot.think(&v);
        }
        let after = bot.think(&v).angles[1];
        assert_ne!(first, after, "a bot that cannot move keeps one heading");
        // And the new heading eventually turns again too.
        for _ in 0..40 {
            bot.think(&v);
        }
        assert_ne!(after, bot.think(&v).angles[1], "the stall swap is not once");
    }

    #[test]
    fn the_view_turns_toward_a_remembered_spot_while_the_keys_keep_the_path() {
        let (mut bot, mut v) = lost([0.0, 500.0, 124.0], [0.0; 3]);
        v.view = [0.0, 0.0, 0.0];
        v.waypoint = Some([500.0, 0.0, 64.0]);
        v.speed = vcod_common::pmove::SPEED_RUN;
        let cmd = bot.think(&v);
        let yaw = cmd.angles[1] as f32 / ANGLE2SHORT;
        assert!((yaw - bot.skill.turn_deg).abs() < 0.1, "yaw {yaw}");
        // The path runs along +x, right of a view turned 15 degrees left.
        assert!(cmd.forward > 120 && cmd.right > 30, "{cmd:?}");
    }

    /// With no path a bot wanders on a random heading; it never takes one
    /// into a minefield, and turns off one it walks up to.
    #[test]
    fn a_wandering_bot_keeps_out_of_hazards() {
        for seed in 1..20 {
            let mut bot = Bot::new("allies", false, seed);
            let mut v = view();
            // Everywhere but -y (octant 6) is a minefield.
            v.hazard_ahead = [true; 8];
            v.hazard_ahead[6] = false;
            let cmd = bot.think(&v);
            assert_eq!(
                octant(bot.heading),
                6,
                "seed {seed}: heading {}",
                bot.heading
            );
            assert_eq!(cmd.forward, 127);
            // A hazard turns up in front: the next tick picks again.
            v.hazard_ahead = [false; 8];
            v.hazard_ahead[6] = true;
            bot.think(&v);
            assert_ne!(octant(bot.heading), 6, "seed {seed}: kept walking in");
        }
    }

    #[test]
    fn octants_run_counter_clockwise_from_plus_x() {
        assert_eq!(octant(0.0), 0);
        assert_eq!(octant(22.0), 0);
        assert_eq!(octant(23.0), 1);
        assert_eq!(octant(90.0), 2);
        assert_eq!(octant(-90.0), 6);
        assert_eq!(octant(350.0), 0);
    }

    #[test]
    fn a_bot_runs_at_its_waypoint() {
        let mut bot = Bot::new("allies", false, 1);
        let mut v = view();
        v.waypoint = Some([0.0, 100.0, 64.0]);
        let cmd = bot.think(&v);
        assert_eq!(cmd.forward, 127);
        assert_eq!(cmd.angles, [0, deg_short(90.0), 0], "level, facing +y");
    }

    /// A waypoint off the walking view's heading: the view comes round at
    /// `walk_turn_deg` a tick while the keys, full, run the body at the
    /// waypoint all along.
    #[test]
    fn the_walking_view_comes_round_while_the_keys_keep_the_path() {
        let mut bot = Bot::new("allies", false, 1);
        let mut v = view();
        v.waypoint = Some([100.0, 0.0, 64.0]);
        let most = bot.skill.walk_turn_deg;
        let mut ticks = 0;
        loop {
            let cmd = bot.think(&v);
            let yaw = cmd.angles[1] as f32 / ANGLE2SHORT;
            let turn = yaw_diff(yaw, v.view[1]);
            assert!(turn.abs() <= most + 0.01, "turned {turn}");
            let way = yaw + (-(cmd.right as f32)).atan2(cmd.forward as f32).to_degrees();
            assert!(yaw_diff(way, 0.0).abs() < 1.0, "keys run at {way}");
            assert_eq!(
                cmd.forward.unsigned_abs().max(cmd.right.unsigned_abs()),
                127
            );
            v.view[1] = yaw;
            ticks += 1;
            if yaw_diff(yaw, 0.0).abs() < 0.01 {
                assert_eq!((cmd.forward, cmd.right), (127, 0));
                break;
            }
            assert!(ticks < 10);
        }
        assert_eq!(ticks, (90.0 / most).ceil() as i32);
    }

    /// A view a fight owned is handed back to the walk at once: the first
    /// walking tick after it faces the path, and the limit runs again
    /// from the tick after.
    #[test]
    fn the_walking_view_takes_the_path_at_once_after_a_fight() {
        let (mut bot, mut v) = engaged(1);
        // Enemy east, waypoint north, the view on the enemy.
        v.waypoint = Some([0.0, 100.0, 64.0]);
        bot.think(&v);
        bot.think(&v);
        // Out of sight, and out of mind, so nothing else owns the view.
        v.enemy = None;
        bot.skill.forget_ticks = 0;
        bot.disengage();
        bot.prior = None;
        bot.recall = None;
        let yaw = |c: &UserCmd| c.angles[1] as f32 / ANGLE2SHORT;
        let cmd = bot.think(&v);
        assert!(yaw_diff(yaw(&cmd), 90.0).abs() < 0.01, "yaw {}", yaw(&cmd));
        v.view[1] = yaw(&cmd);
        // The path turns south-east: the limit is back.
        v.waypoint = Some([100.0, -100.0, 64.0]);
        let cmd = bot.think(&v);
        let turn = yaw_diff(yaw(&cmd), v.view[1]).abs();
        assert!(
            (turn - bot.skill.walk_turn_deg).abs() < 0.01,
            "turned {turn}"
        );
    }

    /// On UO a bot sprints a long straight leg with breath to spare, at
    /// the full key (the sprint takes no weapon scale), and stops short of
    /// the leg's end, with an enemy in sight and once the bar runs low.
    #[test]
    fn a_uo_bot_sprints_a_long_straight_leg() {
        let mut bot = Bot::new("allies", false, 1);
        let mut v = view();
        v.waypoint = Some([0.0, 100.0, 64.0]);
        v.move_scale = 1.2;
        v.sprint_room = 600.0;
        let sprints = |c: &UserCmd| c.buttons & msg::BUTTON_SPRINT_V22 != 0;
        assert!(!sprints(&bot.think(&v)), "no sprint off UO");
        v.fatigue = Some(1.0);
        let cmd = bot.think(&v);
        assert!(sprints(&cmd));
        assert_eq!(cmd.forward, 127, "the full key");
        v.sprinting = true;
        v.sprint_room = 200.0;
        assert!(sprints(&bot.think(&v)), "carries on to the tail");
        v.sprint_room = 100.0;
        assert!(!sprints(&bot.think(&v)), "stops short of the end");
        v.sprinting = false;
        v.sprint_room = 600.0;
        v.fatigue = Some(0.3);
        assert!(!sprints(&bot.think(&v)), "no fresh sprint on a low bar");
        v.fatigue = Some(1.0);
        v.jump = true;
        assert!(!sprints(&bot.think(&v)), "not onto a jump edge");
    }

    /// A remembered spot past the end of a straight the bot has the breath
    /// to sprint is run at along the path; one short of it turns the view
    /// (seed 19 of `uo_bots_fight_and_sprint_in_tdm`).
    #[test]
    fn a_uo_bot_sprints_toward_a_spot_past_the_straight() {
        let sprints = |c: &UserCmd| c.buttons & msg::BUTTON_SPRINT_V22 != 0;
        let mut v = view();
        v.waypoint = Some([0.0, 100.0, 64.0]);
        v.sprint_room = 600.0;
        v.fatigue = Some(1.0);
        v.speed = vcod_common::pmove::SPEED_RUN;
        let think = |at: [f32; 3]| {
            let mut bot = Bot::new("allies", true, 1);
            bot.recall = Some(Recall {
                at,
                ticks: 100,
                seen: true,
            });
            bot.think(&v)
        };
        let cmd = think([400.0, 1200.0, 64.0]);
        assert!(sprints(&cmd), "far spot");
        assert_eq!((cmd.forward, cmd.right), (127, 0));
        assert!(
            !sprints(&think([300.0, 300.0, 64.0])),
            "spot short of the end"
        );
    }

    /// A remembered spot to the side turns the view off the path, but not
    /// on a CoD2 mantle edge: pmove offers the climb only to a view on the
    /// wall (bot-combat.md 17.1).
    #[test]
    fn a_recall_look_keeps_off_a_mantle_edge() {
        let mut v = view();
        v.waypoint = Some([0.0, 100.0, 64.0]);
        v.speed = vcod_common::pmove::SPEED_RUN;
        let think = |v: &BotView| {
            let mut bot = Bot::new("allies", true, 1);
            bot.recall = Some(Recall {
                at: [300.0, -300.0, 64.0],
                ticks: 100,
                seen: true,
            });
            bot.think(v)
        };
        assert_ne!(think(&v).right, 0, "the plain run turns to the spot");
        v.mantle = true;
        assert_eq!(think(&v).right, 0, "a mantle edge keeps the view");
    }

    /// Under [`RECALL_PACE`] the keys keep the path's run: turned off it,
    /// a back key and a strafe at retail's 0.7 and 0.8 scales left a body
    /// against mp_ship's deck corner at (3564, 8, 352) sliding along it at
    /// a few units a tick (bot-navigation.md, "The ship's leftovers").
    #[test]
    fn a_slow_body_keeps_the_path_under_a_recall() {
        let mut v = view();
        v.waypoint = Some([0.0, 100.0, 64.0]);
        let think = |v: &BotView| {
            let mut bot = Bot::new("allies", true, 1);
            bot.recall = Some(Recall {
                at: [300.0, -300.0, 64.0],
                ticks: 100,
                seen: true,
            });
            bot.think(v)
        };
        v.speed = RECALL_PACE - 1.0;
        assert_eq!(think(&v).right, 0, "slow: the path's own keys");
        v.speed = vcod_common::pmove::SPEED_RUN;
        assert_ne!(think(&v).right, 0, "at a run: turned to the spot");
    }

    /// Slowing onto a stop under a recall look scales the turned keys, so
    /// the body still heads for the stop (bot-combat.md 17.1).
    #[test]
    fn a_recall_look_slows_onto_a_stop_along_the_path() {
        let mut v = view();
        v.waypoint = Some([0.0, 24.0, 64.0]);
        v.stop = true;
        v.speed = vcod_common::pmove::SPEED_RUN;
        // Already turned to the spot.
        v.view = [0.0, -45.0, 0.0];
        let mut bot = Bot::new("allies", true, 1);
        bot.recall = Some(Recall {
            at: [300.0, -300.0, 64.0],
            ticks: 100,
            seen: true,
        });
        let cmd = bot.think(&v);
        // The keys in the world: the view's yaw turned by the keys' angle.
        let yaw = f32::from(cmd.angles[1] as i16) * 360.0 / 65536.0;
        let keys = f32::from(-cmd.right)
            .atan2(f32::from(cmd.forward))
            .to_degrees();
        let heading = yaw + keys;
        assert!(
            yaw_diff(heading, 90.0).abs() < 10.0,
            "heads for the stop: view {yaw}, keys ({}, {})",
            cmd.forward,
            cmd.right
        );
    }

    /// A light weapon's keys are cut to the graph's 190 units/s on the
    /// ground, and left whole on the rungs, where the climb takes no
    /// weapon scale.
    #[test]
    fn a_light_weapon_follows_at_the_graphs_pace() {
        let mut bot = Bot::new("allies", false, 1);
        let mut v = view();
        v.waypoint = Some([0.0, 100.0, 64.0]);
        v.move_scale = 1.2;
        assert_eq!(bot.think(&v).forward, 106, "127 / 1.2");
        v.move_scale = 0.9;
        assert_eq!(bot.think(&v).forward, 127, "a heavy one keeps the key");
        v.move_scale = 1.2;
        v.on_ground = false;
        v.on_ladder = true;
        v.waypoint = Some([0.0, 0.0, 300.0]);
        assert_eq!(bot.think(&v).forward, 127, "on the rungs");
    }

    /// Pinned at a ledge's face on a jump edge, as the graph's walk was
    /// (two ticks moved under 2 units with the key held at the top): jump,
    /// release the key the next tick (a held key never jumps again), then
    /// jump again after two more pinned ticks.
    #[test]
    fn a_bot_jumps_a_jump_edge_and_releases_the_key() {
        let mut bot = Bot::new("allies", false, 1);
        let mut v = view();
        v.waypoint = Some([30.0, 0.0, 96.0]);
        assert_eq!(bot.think(&v).up, 0, "a plain edge");
        v.jump = true;
        let ups: Vec<i8> = (0..4).map(|_| bot.think(&v).up).collect();
        assert_eq!(ups, [0, 127, 0, 127]);
    }

    /// Not closing on the top for one tick is no jump; the walk's second
    /// pinned tick is. Jumping on the first, a bot in mp_depot's pit at
    /// (-1569, 2087, 212) carried back off each landing jumped at the step
    /// at (-1533, 2112, 239) again and again (seed 1145, bot-navigation.md
    /// "Jumps").
    #[test]
    fn a_bot_jumps_on_its_second_tick_not_closing_on_the_top() {
        let mut bot = Bot::new("allies", false, 1);
        let mut v = view();
        v.jump = true;
        v.waypoint = Some([60.0, 0.0, 96.0]);
        v.velocity = [190.0, 0.0, 0.0];
        assert_eq!(bot.think(&v).up, 0, "running at it");
        // Carried away from the top by a landing, 4 units a tick.
        v.velocity = [-80.0, 0.0, 0.0];
        let ups: Vec<i8> = (0..2).map(|_| bot.think(&v).up).collect();
        assert_eq!(ups, [0, 127]);
    }

    /// Sliding along a ledge's face on the ground at a run, across the
    /// way to the top, is no pin, as the graph's walk measures it: the bot
    /// slides on to where it comes to rest and jumps there, where the walk
    /// jumped.
    #[test]
    fn a_bot_sliding_along_a_ledges_face_jumps_where_it_comes_to_rest() {
        let mut bot = Bot::new("allies", false, 1);
        let mut v = view();
        v.jump = true;
        v.waypoint = Some([20.0, 10.0, 96.0]);
        v.velocity = [0.0, 190.0, 0.0];
        let ups: Vec<i8> = [0.0, 9.5, 19.0, 28.5, 29.0, 29.0]
            .into_iter()
            .map(|y| {
                v.origin = [0.0, y, 64.0];
                bot.think(&v).up
            })
            .collect();
        assert_eq!(ups, [0, 0, 0, 0, 0, 127]);
    }

    /// Sliding along a face too steep to stand on, across the way to the
    /// top, the bot is not closing on it: two such ticks start the walk's
    /// bob, and it jumps on its next tick on the ground.
    #[test]
    fn a_bot_sliding_along_a_steep_face_jumps_out_of_the_bob() {
        let mut bot = Bot::new("allies", false, 1);
        let mut v = view();
        v.jump = true;
        v.waypoint = Some([40.0, 0.0, 96.0]);
        bot.think(&v);
        v.on_ground = false;
        v.sliding = true;
        v.velocity = [0.0, 100.0, 0.0];
        for i in 1..4 {
            v.origin = [0.0, 5.0 * i as f32, 64.0];
            assert_eq!(bot.think(&v).up, 0, "in the air");
        }
        v.on_ground = true;
        v.sliding = false;
        v.origin = [0.0, 20.0, 64.0];
        assert_eq!(bot.think(&v).up, 127, "out of the bob");
    }

    /// Set off from rest on a jump edge's foot (`nav::Follower::jump_wait`),
    /// the body moves little on its first tick: no jump until the key has
    /// been held against the waypoint for two ticks.
    #[test]
    fn a_bot_set_off_from_rest_is_not_pinned() {
        let mut bot = Bot::new("allies", false, 1);
        let mut v = view();
        v.stop = true;
        v.waypoint = Some([2.0, 0.0, 64.0]);
        assert_eq!(bot.think(&v).forward, 0, "at rest on the foot");
        v.stop = false;
        v.jump = true;
        v.waypoint = Some([30.0, 0.0, 84.0]);
        let ups: Vec<i8> = (0..3).map(|_| bot.think(&v).up).collect();
        assert_eq!(ups, [0, 0, 127]);
    }

    /// On a crouched jump edge the cmd after the jump holds crouch, with
    /// `upmove` -127, through the air, and lets it go on the ground.
    #[test]
    fn a_bot_on_a_crouched_jump_edge_crouches_in_the_air() {
        use vcod_common::net::msg::WBUTTON_CROUCH;
        let mut bot = Bot::new("allies", false, 1);
        let mut v = view();
        v.stop = true;
        v.waypoint = Some([2.0, 0.0, 64.0]);
        bot.think(&v);
        v.stop = false;
        v.jump = true;
        v.crouch_jump = true;
        v.waypoint = Some([30.0, 0.0, 84.0]);
        bot.think(&v);
        bot.think(&v);
        let jump = bot.think(&v);
        assert_eq!((jump.up, jump.wbuttons & WBUTTON_CROUCH), (127, 0));
        v.on_ground = false;
        for _ in 0..3 {
            let air = bot.think(&v);
            assert_eq!(
                (air.up, air.wbuttons & WBUTTON_CROUCH),
                (-127, WBUTTON_CROUCH)
            );
        }
        v.on_ground = true;
        v.jump = false;
        v.crouch_jump = false;
        assert_eq!(bot.think(&v).wbuttons & WBUTTON_CROUCH, 0, "landed");
    }

    /// A jump edge whose top is more than a jump below, the body still
    /// slowing from the last waypoint's pace: pinned only after two cmds
    /// with the key held at the new waypoint, as the graph's walk stalled.
    /// Read as pinned at once, a bot jumped off mp_ship's shelf at (5403,
    /// -160, 396) where the walk ran off its lip (bot-navigation.md
    /// section 3).
    #[test]
    fn a_bot_turning_for_a_top_below_is_not_pinned() {
        let mut bot = Bot::new("allies", false, 1);
        let mut v = view();
        v.waypoint = Some([-60.0, 0.0, 64.0]);
        assert_eq!(bot.think(&v).up, 0, "a plain edge west");
        // Slowing west, the jump edge's top east and 140 below.
        v.velocity = [-30.0, 0.0, 0.0];
        v.speed = 30.0;
        v.jump = true;
        v.waypoint = Some([60.0, -40.0, -76.0]);
        let ups: Vec<i8> = (0..3).map(|_| bot.think(&v).up).collect();
        assert_eq!(ups, [0, 0, 127], "the third cmd against the key jumps");
    }

    /// At a leap's foot the run slows and stops on it; the leap itself
    /// jumps only at the lip.
    #[test]
    fn a_bot_stops_at_a_leaps_foot_and_leaps_at_the_lip() {
        let mut bot = Bot::new("allies", false, 1);
        let mut v = view();
        v.stop = true;
        v.waypoint = Some([100.0, 0.0, 64.0]);
        assert_eq!(bot.think(&v).forward, 127);
        v.waypoint = Some([24.0, 0.0, 64.0]);
        assert_eq!(bot.think(&v).forward, 63);
        v.waypoint = Some([4.0, 0.0, 64.0]);
        assert_eq!(bot.think(&v).forward, 0);
        v.stop = false;
        v.jump = true;
        v.leap = true;
        v.waypoint = Some([60.0, 0.0, 96.0]);
        assert_eq!(bot.think(&v).up, 0, "at rest, no lip");
        v.lip = true;
        assert_eq!(bot.think(&v).up, 127);
    }

    #[test]
    fn a_waypoint_up_a_ladder_looks_up_and_one_below_is_backed_onto() {
        let mut bot = Bot::new("allies", false, 1);
        let mut v = view();
        v.waypoint = Some([32.0, 0.0, 200.0]);
        let up = bot.think(&v);
        assert_eq!(
            (up.forward, up.angles[0]),
            (127, deg_short(-crate::nav::LADDER_PITCH))
        );
        v.waypoint = Some([32.0, 0.0, -100.0]);
        let down = bot.think(&v);
        assert_eq!(down.forward, -127, "backs off the ledge");
        assert_eq!(
            down.angles[1],
            deg_short(-180.0),
            "facing away from the waypoint"
        );
    }

    #[test]
    fn a_climb_looks_up_to_the_top_and_a_descent_backs_all_the_way_down() {
        let mut bot = Bot::new("allies", false, 1);
        let mut v = view();
        v.on_ladder = true;
        // Near the top: the head is only 20 above, still looking up.
        v.waypoint = Some([16.0, 0.0, 84.0]);
        let up = bot.think(&v);
        assert_eq!(
            (up.forward, up.angles[0]),
            (127, deg_short(-crate::nav::LADDER_PITCH))
        );
        // Near the bottom of a descent: the foot is 20 below, still backing.
        v.waypoint = Some([16.0, 0.0, 44.0]);
        assert_eq!(bot.think(&v).forward, -127);
    }

    /// A ladder whose rungs let go only past its top (CoD2's): a head a
    /// few units under the body and far across the face is climbed on to,
    /// where CoD1's rungs, which step off onto it, back down.
    #[test]
    fn a_far_head_over_a_ladder_that_tops_out_is_climbed_on_to() {
        let mut v = view();
        v.on_ladder = true;
        v.ladder_normal = [1.0, 0.0, 0.0];
        v.waypoint = Some([v.origin[0] - 77.0, v.origin[1] - 11.0, v.origin[2] - 7.0]);
        let backs = Bot::new("allies", false, 1).think(&v);
        assert!(backs.forward < 0, "CoD1's backs down: {}", backs.forward);
        v.tops_out = true;
        let climbs = Bot::new("allies", false, 1).think(&v);
        assert_eq!(
            (climbs.forward, climbs.angles[0]),
            (127, deg_short(-crate::nav::LADDER_PITCH))
        );
    }

    #[test]
    fn a_leaps_foot_at_a_ladders_head_is_climbed_to_at_full_rate() {
        let mut bot = Bot::new("allies", false, 1);
        let mut v = view();
        v.stop = true;
        v.on_ladder = true;
        v.waypoint = Some([4.0, 0.0, 600.0]);
        let climb = bot.think(&v);
        assert_eq!(
            (climb.forward, climb.angles[0]),
            (127, deg_short(-crate::nav::LADDER_PITCH))
        );
    }

    /// Up a ladder the climb faces square into it, whatever the waypoint's
    /// bearing, and strafes toward the middle of the rungs.
    #[test]
    fn a_climb_faces_the_ladder_and_strafes_to_its_middle() {
        let mut bot = Bot::new("allies", false, 1);
        let mut v = view();
        v.on_ladder = true;
        v.ladder_normal = [-1.0, 0.0, 0.0];
        v.waypoint = Some([40.0, -60.0, 300.0]);
        v.ladder_middle = Some([v.origin[0], v.origin[1] - 8.0]);
        let cmd = bot.think(&v);
        assert_eq!(cmd.angles[1], deg_short(0.0), "square into the face");
        assert_eq!(cmd.forward, 127);
        assert!(
            cmd.right > 0,
            "facing +x, the middle at -y is to the right: {}",
            cmd.right
        );
        v.ladder_middle = Some([v.origin[0], v.origin[1]]);
        assert_eq!(bot.think(&v).right, 0, "centred");
    }

    /// A strafe held under a full-rate climb never turns the pace the grab
    /// carried in, and keeps the bleed off it: the key comes up until the
    /// body no longer slides away from the middle.
    #[test]
    fn a_climber_sliding_away_from_the_middle_lets_the_strafe_go() {
        let mut bot = Bot::new("allies", false, 1);
        let mut v = view();
        v.on_ladder = true;
        v.ladder_normal = [-1.0, 0.0, 0.0];
        v.waypoint = Some([40.0, -60.0, 300.0]);
        v.ladder_middle = Some([v.origin[0], v.origin[1] - 8.0]);
        v.velocity = [0.0, 30.0, 95.0];
        assert_eq!(bot.think(&v).right, 0, "sliding +y, the middle at -y");
        v.velocity = [0.0, -30.0, 95.0];
        assert!(bot.think(&v).right > 0, "sliding toward the middle");
        v.velocity = [0.0, 0.5, 95.0];
        assert!(bot.think(&v).right > 0, "all but still");
    }

    #[test]
    fn a_bot_on_a_ladder_with_no_waypoint_or_pinned_climbs_down() {
        let mut bot = Bot::new("allies", false, 1);
        let mut v = view();
        v.on_ladder = true;
        let down = (127, deg_short(crate::nav::LADDER_PITCH));
        let cmd = bot.think(&v);
        assert_eq!((cmd.forward, cmd.angles[0]), down, "no waypoint");
        // Pinned under something on the way up: the unstick goes down too.
        v.waypoint = Some([16.0, 0.0, 200.0]);
        let unstuck = (0..25)
            .map(|_| bot.think(&v))
            .any(|c| (c.forward, c.angles[0]) == down);
        assert!(unstuck, "a pinned climber kept pushing up");
    }

    #[test]
    fn a_bot_creeps_backward_over_a_lip_and_keeps_facing_the_ladder() {
        let mut bot = Bot::new("allies", false, 1);
        let mut v = view();
        // The foot is 300 below, 20 ahead in +x: back toward it facing -x.
        v.waypoint = Some([20.0, 0.0, -236.0]);
        let first = bot.think(&v);
        assert_eq!(first.forward, -crate::nav::CREEP);
        assert_eq!(first.angles[1], deg_short(-180.0));
        // Over the lip and past the foot's column: same heading.
        v.origin[0] = 21.0;
        let past = bot.think(&v);
        assert_eq!(
            (past.forward, past.angles[1]),
            (first.forward, first.angles[1])
        );
    }

    /// A waypoint 50 under the feet is run at, unless the graph's run off
    /// that drop missed: then it is backed toward, as the graph proved it
    /// (`BotView::back_drop`). A body on the rungs told to hold presses no
    /// key (`BotView::rungs_hold`).
    #[test]
    fn a_drop_the_graph_backed_down_is_backed_and_a_held_climber_presses_nothing() {
        let mut bot = Bot::new("allies", false, 1);
        let mut v = view();
        // 50 under the feet at 64.
        v.waypoint = Some([20.0, 0.0, 14.0]);
        assert!(bot.think(&v).forward > 0, "run at");
        v.back_drop = true;
        assert!(bot.think(&v).forward < 0, "backed toward");
        let mut bot = Bot::new("allies", false, 1);
        let mut v = view();
        v.on_ladder = true;
        v.waypoint = Some([0.0, 0.0, 200.0]);
        assert_ne!(bot.think(&v).forward, 0, "climbs");
        v.rungs_hold = true;
        v.stop = true;
        let cmd = bot.think(&v);
        assert_eq!((cmd.forward, cmd.right), (0, 0), "holds");
    }

    /// mp_ship's crow's nest: backed toward the foot from a few units off
    /// the rungs' line, bots slid past the rungs down the hatch (seeds
    /// 1919, 3941, 4161, 4545). Off the head of a ladder whose fall kills
    /// the creep faces the rungs and strafes to their middle.
    #[test]
    fn a_creep_off_a_deadly_head_faces_the_rungs_and_strafes_to_them() {
        let mut bot = Bot::new("allies", false, 1);
        let mut v = view();
        // The rungs face -x at x 4975, y 59 to 69; the body is 8 south.
        v.origin = [4963.9, 56.0, 1219.125];
        v.waypoint = Some([4959.0, 64.0, 692.125]);
        v.back_face = Some((0.0, [4959.0, 64.0]));
        let cmd = bot.think(&v);
        assert_eq!(cmd.forward, -crate::nav::CREEP);
        assert_eq!(cmd.angles[1], deg_short(0.0));
        // Facing +x, the middle at +y is to the left.
        assert!(
            cmd.right < 0 && cmd.right >= -crate::nav::CREEP,
            "{}",
            cmd.right
        );
        v.origin[1] = 64.0;
        assert_eq!(bot.think(&v).right, 0, "on the line");
    }

    #[test]
    fn a_stuck_bot_leaves_its_waypoint_for_a_spell() {
        let mut bot = Bot::new("allies", false, 1);
        let mut v = view();
        v.waypoint = Some([0.0, 100.0, 64.0]);
        let toward = bot.think(&v).angles[1];
        // Pinned: the origin never moves, and the stall reads within two
        // ten-tick windows.
        let turned = (0..25).any(|_| bot.think(&v).angles[1] != toward);
        assert!(turned, "a pinned bot kept pushing at its waypoint");
        // The random heading gets it moving again.
        for _ in 0..UNSTICK_TICKS {
            v.origin[0] += 5.0;
            v.waypoint = Some([v.origin[0], 100.0, 64.0]);
            bot.think(&v);
        }
        assert_eq!(bot.think(&v).angles[1], toward, "and goes back to it after");
    }

    /// A creep onto a ladder below a lip covers under a unit a tick, short
    /// of the 15-unit stall circle in ten: mp_depot's back down over the
    /// 1-unit ladder at (-2750, -1217, 504) read as stuck, and the spell
    /// walked off the roof (bot-navigation.md section 3). A creep that stops
    /// is stuck all the same.
    #[test]
    fn a_creep_is_not_stuck_until_it_stops() {
        let mut bot = Bot::new("allies", false, 1);
        let mut v = view();
        v.waypoint = Some([20.0, 0.0, -236.0]);
        for _ in 0..30 {
            let cmd = bot.think(&v);
            assert_eq!(cmd.forward, -crate::nav::CREEP);
            assert!(!bot.unsticking(), "creeping at {:?}", v.origin);
            v.origin[0] += 0.7;
        }
        let stuck = (0..12).any(|_| {
            bot.think(&v);
            bot.unsticking()
        });
        assert!(stuck, "a creep held still never unsticks");
    }

    /// mp_ship's crow's nest: a bot backing from the platform for the deck
    /// 527 below pins on the hatch's rim and slides along it at under a
    /// unit a tick before it drops through onto the rungs. That gains on
    /// the waypoint and is no stall; backing in place is.
    #[test]
    fn a_back_down_sliding_along_a_rim_is_not_stuck() {
        let mut bot = Bot::new("allies", false, 1);
        let mut v = view();
        v.origin = [4954.1, 92.0, 1219.125];
        v.waypoint = Some([4929.0, 68.0, 692.125]);
        for _ in 0..30 {
            let cmd = bot.think(&v);
            assert_eq!(cmd.forward, -127);
            assert!(!bot.unsticking(), "sliding at {:?}", v.origin);
            v.origin[1] -= 0.6;
        }
        let stuck = (0..12).any(|i| {
            v.origin[0] += if i % 2 == 0 { 0.5 } else { -0.5 };
            bot.think(&v);
            bot.unsticking()
        });
        assert!(stuck, "a back down held on the rim never unsticks");
    }

    /// On the rungs a head across the face just below is climbed over, a
    /// floor on the climber's side below is backed down to
    /// (`tests/bots.rs`, `a_bot_climbs_over_mp_ships_hold_ladder_top`).
    #[test]
    fn a_head_across_the_face_is_climbed_over() {
        let mut bot = Bot::new("allies", false, 1);
        let mut v = view();
        v.on_ladder = true;
        v.on_ground = false;
        // The face at y 15, its normal toward the climber.
        v.ladder_normal = [0.0, -1.0, 0.0];
        v.waypoint = Some([v.origin[0], v.origin[1] + 6.0, v.origin[2] - 2.0]);
        assert_eq!(bot.think(&v).forward, 127);
        v.waypoint = Some([v.origin[0], v.origin[1] - 1.0, v.origin[2] - 2.0]);
        assert_eq!(bot.think(&v).forward, -127);
    }

    /// A path that drops licenses a drop as deep as its own, not a fall
    /// that hurts below it: mp_depot's bots backing down the ladder from
    /// the roof at 508 to its foot at 326 took unstick headings off the
    /// roof's edge to the yard at -24 and died (section 3).
    #[test]
    fn a_path_down_licenses_no_deeper_fall() {
        let bot = Bot::new("allies", false, 1);
        let mut v = view();
        v.waypoint = Some([20.0, 0.0, v.origin[2] - 180.0]);
        v.drop_ahead[0] = Some(150.0);
        v.drop_ahead[2] = Some(f32::INFINITY);
        v.drop_ahead[4] = Some(500.0);
        assert!(!bot.shuns(&v, 0.0), "a drop the path takes anyway");
        assert!(bot.shuns(&v, 90.0), "a fall that hurts");
        assert!(bot.shuns(&v, 180.0), "a fall that hurts");
        v.waypoint = Some([20.0, 0.0, v.origin[2] - 600.0]);
        assert!(!bot.shuns(&v, 180.0), "a path that drops as far");
    }

    /// The server feels for drops and walls only on the ticks
    /// [`Bot::reads_drops`] and [`Bot::reads_walls`] name: a bot fed them
    /// every tick and one fed them only then press the same keys, stuck
    /// on the way to a waypoint, on its spells and wandering with none.
    #[test]
    fn drops_are_felt_only_when_the_brain_reads_them() {
        let mut drops = [Some(100.0); 8];
        drops[2] = None;
        let walls = [true, false, true, true, false, true, false, true];
        for seed in 1..20 {
            let (mut every, mut lazy) = (
                Bot::new("allies", false, seed),
                Bot::new("allies", false, seed),
            );
            let mut v = view();
            let mut skipped = 0;
            for tick in 0..120 {
                v.waypoint = (tick < 60).then_some([0.0, 100.0, 64.0]);
                v.drop_ahead = drops;
                v.wall_ahead = walls;
                let want = every.think(&v);
                if !lazy.reads_drops(&v) {
                    v.drop_ahead = [None; 8];
                    skipped += 1;
                }
                if !lazy.reads_walls(&v) {
                    v.wall_ahead = [false; 8];
                }
                assert_eq!(lazy.think(&v), want, "seed {seed} tick {tick}");
            }
            assert!(skipped > 10, "seed {seed}: felt every tick");
        }
    }

    /// A bot pinned by a deck's edge takes its unstick heading along the
    /// deck, never off it, and turns off a drop it walks up to. Seed 5 on
    /// mp_ship walked one off the deck at (3288, -460, 56) to the hull.
    #[test]
    fn a_stuck_bot_unsticks_away_from_a_drop() {
        for seed in 1..20 {
            let mut bot = Bot::new("allies", false, seed);
            let mut v = view();
            v.waypoint = Some([0.0, 100.0, 64.0]);
            // A drop everywhere but +y (octant 2), a minefield at -y.
            v.drop_ahead = [Some(100.0); 8];
            v.drop_ahead[2] = None;
            v.hazard_ahead[6] = true;
            let unstuck = (0..25).find(|_| {
                bot.think(&v);
                bot.unsticking()
            });
            assert!(unstuck.is_some(), "seed {seed}: never unstuck");
            assert_eq!(octant(bot.heading), 2, "seed {seed}: {}", bot.heading);
            // Boxed in by drops: off the hazard still.
            v.drop_ahead = [Some(100.0); 8];
            bot.pick_heading(&v);
            assert_ne!(octant(bot.heading), 6, "seed {seed}: into the minefield");
            // A drop turns up ahead mid-spell: the next tick picks again.
            v.drop_ahead = [None; 8];
            v.hazard_ahead = [false; 8];
            bot.heading = 90.0;
            v.drop_ahead[2] = Some(100.0);
            bot.think(&v);
            assert_ne!(octant(bot.heading), 2, "seed {seed}: kept walking off");
        }
    }

    /// A climb that bobs under a ladder's top lets go of the rungs and
    /// stands on the head's plate in the stall window: the spell holds still
    /// there, where a heading ran bots down mp_ship's crow's nest hatch. So
    /// does a body in the air just off the rungs with no waypoint.
    #[test]
    fn a_spell_just_off_the_rungs_holds_still() {
        let mut bot = Bot::new("allies", false, 1);
        let mut v = view();
        v.waypoint = Some([0.0, 100.0, 64.0]);
        v.on_ground = false;
        v.on_ladder = true;
        bot.think(&v);
        v.on_ground = true;
        v.on_ladder = false;
        let mut keys = Vec::new();
        for _ in 0..12 {
            bot.think(&v);
        }
        v.on_ground = false;
        v.on_ladder = true;
        bot.think(&v);
        v.on_ground = true;
        v.on_ladder = false;
        for _ in 0..12 {
            let cmd = bot.think(&v);
            if bot.unstick_ticks > 0 {
                keys.push(cmd.forward);
            }
        }
        assert!(!keys.is_empty() && keys.iter().all(|&k| k == 0), "{keys:?}");
        // Off the ground with no waypoint, the same.
        let mut bot = Bot::new("allies", false, 1);
        let mut v = view();
        v.on_ground = false;
        v.on_ladder = true;
        bot.think(&v);
        v.on_ladder = false;
        assert_eq!(bot.think(&v).forward, 0);
        // Away from the rungs a spell walks.
        let mut bot = Bot::new("allies", false, 1);
        let mut v = view();
        v.waypoint = Some([0.0, 100.0, 64.0]);
        let walked = (0..30).any(|_| {
            let cmd = bot.think(&v);
            bot.unstick_ticks > 0 && cmd.forward != 0
        });
        assert!(walked);
    }

    /// A body that runs 20 units out and back within the ten ticks left
    /// the stall circle: no spell. One that never left it gets one.
    #[test]
    fn out_and_back_is_no_stall() {
        let mut bot = Bot::new("allies", false, 1);
        let mut v = view();
        v.waypoint = Some([0.0, 300.0, 64.0]);
        for tick in 0..60 {
            let x = [0.0, 10.0, 20.0, 10.0, 0.0][tick % 5];
            v.origin = [x, 0.0, 64.0];
            bot.think(&v);
            assert_eq!(bot.unstick_ticks, 0, "tick {tick}");
        }
        v.origin = [0.0, 0.0, 64.0];
        assert!((0..30).any(|_| {
            bot.think(&v);
            bot.unstick_ticks > 0
        }));
    }

    /// A climb bobbing under a ladder's top, on and off the rungs right
    /// under its waypoint, is not stuck: no spell.
    #[test]
    fn a_climb_bobbing_under_the_top_is_not_stuck() {
        let mut bot = Bot::new("allies", false, 1);
        let mut v = view();
        v.waypoint = Some([4.0, 0.0, 84.0]);
        v.on_ground = false;
        for tick in 0..40 {
            v.on_ladder = tick % 6 < 3;
            bot.think(&v);
            assert_eq!(bot.unstick_ticks, 0, "tick {tick}");
        }
        // Far under it, pinned under a spar, it is.
        v.waypoint = Some([4.0, 0.0, 164.0]);
        assert!((0..40).any(|tick| {
            v.on_ladder = tick % 6 < 3;
            bot.think(&v);
            bot.unstick_ticks > 0
        }));
    }

    /// Stuck spell after spell in one spot with only drops open (mp_ship's
    /// rim at (4223, 345, 276), under a ramp), a bot is cornered and takes
    /// a drop out.
    #[test]
    fn a_cornered_bot_takes_a_drop_out() {
        let mut dropped = 0;
        for seed in 1..20 {
            let mut bot = Bot::new("allies", false, seed);
            let mut v = view();
            v.waypoint = Some([0.0, 100.0, 64.0]);
            // +y (octant 2) is the wall it is pinned against.
            v.drop_ahead = [Some(100.0); 8];
            v.drop_ahead[2] = None;
            let mut spells = Vec::new();
            for _ in 0..200 {
                bot.think(&v);
                // A spell starts at UNSTICK_TICKS and its first tick spends one.
                if bot.unstick_ticks == UNSTICK_TICKS - 1 {
                    spells.push(octant(bot.heading));
                }
            }
            let first = CORNERED as usize;
            assert!(spells.len() > first, "seed {seed}: {spells:?}");
            assert!(
                spells[..first].iter().all(|o| *o == 2),
                "seed {seed}: {spells:?}"
            );
            dropped += usize::from(spells[first..].iter().any(|o| *o != 2));
        }
        assert!(dropped > 10, "only {dropped} of 19 seeds took a drop out");
        // A drop that hurts stays off even cornered.
        for seed in 1..20 {
            let mut bot = Bot::new("allies", false, seed);
            let mut v = view();
            v.waypoint = Some([0.0, 100.0, 64.0]);
            v.drop_ahead = [Some(f32::INFINITY); 8];
            v.drop_ahead[2] = None;
            for _ in 0..200 {
                bot.think(&v);
                if bot.unstick_ticks == UNSTICK_TICKS - 1 {
                    assert_eq!(octant(bot.heading), 2, "seed {seed}: off a deep drop");
                }
            }
        }
    }

    /// mp_ship's hold ladder head, seed 149: walls north and east, the
    /// shaft south. A heading into a wall reads as no drop, and the bot
    /// stood pinned on one spell after spell; the open one west comes
    /// first.
    #[test]
    fn a_stuck_bot_takes_a_heading_into_a_wall_last() {
        for seed in 1..20 {
            let mut bot = Bot::new("allies", false, seed);
            let mut v = view();
            v.wall_ahead = [true, true, true, true, false, false, false, true];
            v.drop_ahead = [
                None,
                None,
                None,
                None,
                None,
                Some(500.0),
                Some(500.0),
                Some(500.0),
            ];
            bot.pick_heading(&v);
            assert_eq!(octant(bot.heading), 4, "seed {seed}: {}", bot.heading);
            // Walled in but for drops: a wall it is.
            v.wall_ahead[4] = true;
            bot.pick_heading(&v);
            assert!(octant(bot.heading) < 5, "seed {seed}: {}", bot.heading);
        }
    }

    /// A push-off jumps facing the rungs, then brakes facing the same way
    /// until the body lands (`nav::push_off`).
    #[test]
    fn a_push_off_jumps_then_brakes_until_it_lands() {
        let mut bot = Bot::new("allies", false, 1);
        let mut v = view();
        v.on_ladder = true;
        v.on_ground = false;
        v.ladder_normal = [0.0, -1.0, 0.0];
        v.origin = [2146.5, -214.1, 103.0];
        v.waypoint = Some([2144.7, -229.9, 104.1]);
        v.push_off = Some(90.0);
        let out = cmd_angles(&v, [0.0, 90.0]);
        let cmd = bot.think(&v);
        assert_eq!((cmd.up, cmd.forward, cmd.angles), (127, 0, out));
        v.on_ladder = false;
        v.push_off = None;
        v.origin = [2146.5, -230.0, 120.0];
        for _ in 0..5 {
            let cmd = bot.think(&v);
            assert_eq!((cmd.up, cmd.forward, cmd.angles), (0, 127, out));
        }
        // Landed facing the waypoint, it runs on.
        v.on_ground = true;
        v.view[1] = (-229.9f32 + 230.0).atan2(2144.7 - 2146.5).to_degrees();
        assert_eq!(bot.think(&v).forward, 127);
    }

    /// An unpinning heading is held for its ticks, then the bot heads at
    /// the waypoint again.
    #[test]
    fn an_unpinning_heading_is_held_for_its_ticks() {
        let mut bot = Bot::new("allies", false, 1);
        let mut v = view();
        v.waypoint = Some([0.0, 100.0, 64.0]);
        let toward = bot.think(&v).angles;
        v.unpin = Some(180.0);
        let aside = cmd_angles(&v, [0.0, 180.0]);
        assert_eq!(bot.think(&v).angles, aside);
        v.unpin = None;
        for _ in 1..crate::nav::UNPIN_TICKS {
            assert_eq!(bot.think(&v).angles, aside);
        }
        assert_eq!(bot.think(&v).angles, toward);
    }

    /// Held still off the ground (mp_hurtgen's crease at (437, 2824, -97))
    /// for [`WEDGE_TICKS`], the bot asks for a `kill`; on the ground, on
    /// the rungs or moving, never.
    #[test]
    fn a_body_held_in_the_air_asks_for_a_kill() {
        let mut bot = Bot::new("allies", false, 1);
        let mut v = view();
        v.waypoint = Some([0.0, 100.0, 64.0]);
        let ticks = |bot: &mut Bot, v: &BotView| {
            (1..=2 * WEDGE_TICKS).find(|_| {
                bot.think(v);
                bot.take_kill()
            })
        };
        assert_eq!(ticks(&mut bot, &v), None, "on the ground");
        v.on_ground = false;
        v.on_ladder = true;
        assert_eq!(ticks(&mut bot, &v), None, "on the rungs");
        v.on_ladder = false;
        assert_eq!(ticks(&mut bot, &v), Some(WEDGE_TICKS));
        assert!(!bot.take_kill(), "asked once");
        for _ in 0..2 * WEDGE_TICKS {
            v.origin[2] -= 2.0;
            bot.think(&v);
            assert!(!bot.take_kill(), "falling");
        }
        // A round's one life is worth more than the crease costs.
        v.one_life = true;
        assert_eq!(ticks(&mut bot, &v), None, "one life a round");
    }

    /// mp_ship, seed 577: a bot slid off the bow roof into the gutter at
    /// (1955, 281, 312), whose nodes lead nowhere: north is a fall of 376,
    /// south the roof's slope it slides back down. The wander keeps off
    /// the fall for good, so the bot asks for a kill, as in a crease.
    #[test]
    fn a_body_stranded_off_the_graph_asks_for_a_kill() {
        let mut bot = Bot::new("allies", false, 1);
        let mut v = view();
        v.stranded = true;
        let ticks = |bot: &mut Bot, v: &BotView| {
            (1..=2 * STRAND_TICKS).find(|_| {
                bot.think(v);
                bot.take_kill()
            })
        };
        assert_eq!(ticks(&mut bot, &v), Some(STRAND_TICKS));
        v.one_life = true;
        assert_eq!(ticks(&mut bot, &v), None, "one life a round");
        v.one_life = false;
        v.stranded = false;
        assert_eq!(ticks(&mut bot, &v), None, "on the graph");
    }

    /// Pinned through a spell, the bot picks a new heading but the spell
    /// still ends on time and the waypoint gets its turn. Chained spells
    /// held one on mp_ship's hold ladder head for the rest of the run
    /// while its waypoint lay a walk away.
    #[test]
    fn a_pinned_spell_hands_back_to_the_waypoint() {
        let mut bot = Bot::new("allies", false, 1);
        let mut v = view();
        v.waypoint = Some([0.0, 100.0, 64.0]);
        let toward = bot.think(&v).angles[1];
        let start = (0..25).find(|_| {
            bot.think(&v);
            bot.unsticking()
        });
        assert!(start.is_some(), "never unstuck");
        let back = (0..UNSTICK_TICKS + 1).any(|_| bot.think(&v).angles[1] == toward);
        assert!(back, "spell after spell, never the waypoint");
    }

    /// mp_ship's ladder at x 3792: a climb pinned under another bot takes
    /// a spell down the rungs, then climbs back. A stall window that
    /// straddles the turn reads the way down and the way back up as
    /// standing still, and spell followed spell down the ladder.
    #[test]
    fn a_spell_end_starts_the_stall_window_afresh() {
        let mut bot = Bot::new("allies", false, 1);
        let mut v = view();
        v.on_ladder = true;
        v.on_ground = false;
        v.ladder_normal = [-1.0, 0.0, 0.0];
        v.waypoint = Some([3783.0, 64.0, 623.0]);
        v.origin = [3808.0, 67.0, 511.0];
        let start = (0..25).find(|_| {
            bot.think(&v);
            bot.unsticking()
        });
        assert!(start.is_some(), "never unstuck");
        // Down the rungs for the spell at the climb's rate, then back up.
        while bot.unsticking() {
            v.origin[2] -= 2.67;
            bot.think(&v);
        }
        for tick in 0..UNSTICK_TICKS {
            v.origin[2] += 2.67;
            bot.think(&v);
            assert!(!bot.unsticking(), "a new spell {tick} ticks after the last");
        }
    }

    /// Behind a teammate just ahead (`BotView::mate_ahead`) a bot on its
    /// way eases its keys to half; not with an enemy in sight, a bomb down
    /// or `Bot::squad` off.
    #[test]
    fn a_bot_walks_behind_a_teammate_just_ahead() {
        let forward = |ahead: bool, enemy: bool, bomb: bool, on: bool| {
            let mut bot = Bot::new("allies", false, 1);
            bot.squad = on;
            let mut v = sd_view(ObjRole::Attack);
            v.origin = [1000.0, 0.0, 64.0];
            v.view = [0.0, 90.0, 0.0];
            v.waypoint = Some([1000.0, 400.0, 64.0]);
            v.on_ground = true;
            v.move_scale = 1.0;
            v.mate_ahead = ahead;
            if bomb && let Some(sd) = v.sd.as_mut() {
                sd.bomb = Some(BombView {
                    origin: [0.0; 3],
                    aim: [0.0; 3],
                });
            }
            if enemy {
                v.enemy = Some(EnemyView {
                    slot: 3,
                    origin: [3000.0, 3000.0, 64.0],
                    velocity: [0.0; 3],
                });
            }
            bot.think(&v).forward
        };
        assert_eq!(forward(false, false, false, true), 127);
        assert_eq!(forward(true, false, false, true), 63);
        assert_eq!(forward(true, false, false, false), 127);
        assert_eq!(forward(true, false, true, true), 127);
        assert_eq!(
            forward(false, true, false, true),
            forward(true, true, false, true)
        );
    }

    /// A live frag near, anyone's: the goal is away from it and the keys
    /// run that way, the view still on the fight.
    #[test]
    fn a_bot_runs_from_a_live_frag() {
        let (mut bot, mut v) = engaged(3);
        v.frag_near = Some([60.0, 0.0, 0.0]);
        assert!(matches!(bot.goal(&v), Goal::Away(_)));
        let cmd = bot.think(&v);
        // The enemy is down +x, the frag too: the run is backwards.
        assert!(cmd.forward < -100, "{}", cmd.forward);
        let yaw = cmd.angles[1] as f32 / ANGLE2SHORT;
        assert!(yaw.abs() < 20.0, "the view stays on the enemy: {yaw}");
    }

    /// mp_cassino, seed 1: a frag thrown away from a close enemy came back
    /// off the wall and rolled to the thrower, who stood through the switch
    /// back to its rifle and took the blast. The keys run from it there too.
    #[test]
    fn a_bot_switching_back_from_a_throw_runs_from_a_live_frag() {
        let (mut bot, mut v) = engaged(3);
        v.frag_near = Some([60.0, 0.0, 0.0]);
        bot.stage = Stage::BackToRifle { weapon: 10 };
        let cmd = bot.think(&v);
        assert!(cmd.forward < -100, "{}", cmd.forward);
    }
}
