//! One usercmd through the mover: the input words, the view, `Pmove`'s chop
//! and a live player's step with its event ring. The server's
//! `ClientSim::step` and the client's predictor (`super::predict`) both run
//! a playing client's cmds through here, so a prediction replays the
//! server's own code.

use super::{MAX_FRAME_MS, MoveRules, PlayerState, PmEvent, PmInput, weapon};
use crate::movetrace::MoveWorld;
use crate::net::msg::{self, UserCmd};
use crate::weapon::WeaponDef;

/// ANGLE2SHORT units per degree (codextended shared.h).
pub const ANGLE2SHORT: f32 = 65536.0 / 360.0;
/// `Pmove`'s catch-up (`game.mp.i386.so` 0x34492): a client further in arrears
/// than this has the excess dropped rather than simulated, which also bounds
/// the chop to 16 steps per cmd. `docs/protocol-1.1.md`, "How long a cmd is
/// simulated for", has the rest of retail's rule, including the two clamps
/// vcod does not apply.
pub const MAX_PMOVE_ARREARS_MS: i32 = 1000;

fn short_deg(v: i32) -> f32 {
    let deg = v as f32 / ANGLE2SHORT;
    (deg + 180.0).rem_euclid(360.0) - 180.0
}

/// A usercmd's input words as pmove's per-frame input. The stance bits are
/// level, and a crouched or prone client holds `up` at -127 for as long as it
/// is down, so only a positive `up` is a jump. `walk_slow` has no wire source:
/// CoD 1 has one move speed and no walk key, and pmove's walk scale is
/// reachable only from the client's own fly mode. Bit table and evidence:
/// docs/protocol-1.1.md, "Usercmd input bits".
pub fn pm_input(cmd: &UserCmd) -> PmInput {
    PmInput {
        forward: f32::from(cmd.forward) / 127.0,
        right: f32::from(cmd.right) / 127.0,
        up: f32::from(cmd.up) / 127.0,
        jump: cmd.up > 0,
        crouch: cmd.wbuttons & msg::WBUTTON_CROUCH != 0,
        prone: cmd.wbuttons & msg::WBUTTON_PRONE != 0,
        stance_held: cmd.wbuttons & msg::WBUTTON_STANCE_HELD != 0,
        shellshock: cmd.wbuttons & msg::WBUTTON_SHELLSHOCK != 0,
        walk_slow: false,
        lean_left: cmd.wbuttons & msg::WBUTTON_LEAN_LEFT != 0,
        lean_right: cmd.wbuttons & msg::WBUTTON_LEAN_RIGHT != 0,
        attack: cmd.buttons & msg::BUTTON_ATTACK != 0,
        melee: cmd.buttons & msg::BUTTON_MELEE != 0,
        reload: cmd.wbuttons & msg::WBUTTON_RELOAD != 0,
        ads: cmd.buttons & msg::BUTTON_ADS != 0,
        use_button: cmd.buttons & msg::BUTTON_USE != 0,
        weapon: cmd.weapon,
        // Raw, the way `PM_AdjustAimSpreadScale` reads them: the turn term is
        // a delta between two cmds, so the view offset both carry cancels.
        angles: [cmd.angles[0], cmd.angles[1]],
        sprint: cmd.buttons & msg::BUTTON_SPRINT_V22 != 0,
        server_time: cmd.server_time,
    }
}

/// `PmoveSingle`'s talk arm, ahead of every `pm_type` dispatch: a cmd with
/// `BUTTON_TALK` (the console, a menu or the chat line up) keeps only the
/// talk bit on 1.1 (`buttons = 2`, game.mp.i386.so 0x34004), the talk and
/// sight bits on 1.5 and UO (`buttons &= 0x12`, 1.5b 0x27f5e, UO 0x2d5d7),
/// keeps the stance bits of `wbuttons` (`&= 0xc2`) and zeroes the three
/// moves. Only `pm->cmd` is masked: `pm->oldcmd` stays the raw cmd
/// (docs/research/cod15-movement.md 5, "The talk button").
pub fn talk_masked(cmd: &UserCmd, rules: MoveRules) -> UserCmd {
    if cmd.buttons & msg::BUTTON_TALK == 0 {
        return *cmd;
    }
    let keep = match rules {
        MoveRules::V1_1 => msg::BUTTON_TALK,
        MoveRules::V1_5 | MoveRules::Uo => msg::BUTTON_TALK | msg::BUTTON_ADS,
    };
    UserCmd {
        buttons: cmd.buttons & keep,
        wbuttons: cmd.wbuttons
            & (msg::WBUTTON_CROUCH | msg::WBUTTON_PRONE | msg::WBUTTON_STANCE_HELD),
        forward: 0,
        right: 0,
        up: 0,
        ..*cmd
    }
}

/// `PM_UpdateViewAngles`: `ps.viewangles[i] = SHORT2ANGLE(cmd.angles[i] +
/// delta_angles[i])`, per axis, wire convention (pitch positive down).
/// docs/protocol-1.1.md, "View angles".
pub fn view_angles(cmd_angles: [i32; 3], delta_angles: [i32; 3]) -> [f32; 3] {
    [
        short_deg(cmd_angles[0] + delta_angles[0]),
        short_deg(cmd_angles[1] + delta_angles[1]),
        short_deg(cmd_angles[2] + delta_angles[2]),
    ]
}

/// [`view_angles`] written into the sim's yaw and pitch, whose pitch is the
/// camera's convention (positive up). Returns the wire-convention view.
pub fn apply_view(ps: &mut PlayerState, cmd_angles: [i32; 3], delta_angles: [i32; 3]) -> [f32; 3] {
    let view = view_angles(cmd_angles, delta_angles);
    // `AngleVectors` takes each angle to radians by the double pi / 180 and
    // rounds it to a float (game.mp 0x3b234).
    let rad = |deg: f32| (f64::from(deg) * (std::f64::consts::PI / 180.0)) as f32;
    ps.prev_yaw = ps.yaw;
    ps.yaw = rad(view[1]);
    ps.pitch = -rad(view[0]);
    view
}

/// `PM_UpdateViewAngles`' clamps (UO game 0x2b15c, cgame 0x3000c8e0;
/// coduo-vehicles.md 3): the pitch held at 16000 shorts, 12000 in a
/// vehicle seat, by rewriting `delta_angles[0]`, then a seat's arcs around
/// the vehicle's angles, each excess pushed onto `delta_angles` and the
/// view set on the arc's edge.
fn clamp_view(
    ps: &mut PlayerState,
    cmd_angles: [i32; 3],
    delta_angles: &mut [i32; 3],
    view: &mut [f32; 3],
) {
    let limit = if ps.seat.is_some() { 12000 } else { 16000 };
    let pitch = i32::from(cmd_angles[0].wrapping_add(delta_angles[0]) as i16);
    let mut changed = false;
    if pitch.abs() > limit {
        let held = limit * pitch.signum();
        delta_angles[0] = (held - cmd_angles[0]) & 0xffff;
        view[0] = short_deg(held);
        changed = true;
    }
    if let Some(seat) = ps.seat {
        for (i, arc) in seat.arcs().into_iter().enumerate() {
            if arc.abs() < 1.0 {
                continue;
            }
            // A seat is UO's, whose difference is stored.
            let d = super::angle_delta(seat.angles[i], view[i], true);
            if arc.abs() >= d.abs() {
                continue;
            }
            let excess = if arc < d { d - arc } else { d + arc };
            delta_angles[i] = (delta_angles[i] + super::angle_shorts(excess)) & 0xffff;
            view[i] = super::normalize360_accurate(if excess > 0.0 {
                seat.angles[i] - arc
            } else {
                seat.angles[i] + arc
            });
            changed = true;
        }
    }
    if changed {
        let rad = |deg: f32| (f64::from(deg) * (std::f64::consts::PI / 180.0)) as f32;
        ps.yaw = rad(view[1]);
        ps.pitch = -rad(view[0]);
    }
}

/// The four-slot event ring a playerstate or an entity carries: written at
/// `events[seq & 3]` with the counter bumped after it, so the new slots of a
/// frame are the ones *below* the sequence
/// (`docs/research/cod11-combat.md` section 7).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct EventRing {
    pub events: [i32; 4],
    pub parms: [i32; 4],
    /// `eventSequence`, kept in the wire's eight bits.
    pub seq: i32,
}

impl EventRing {
    /// `G_AddEvent`: the slot first, the counter after.
    pub fn add(&mut self, event: i32, parm: i32) {
        let slot = (self.seq & 3) as usize;
        self.events[slot] = event;
        self.parms[slot] = parm;
        self.seq = (self.seq + 1) & 0xff;
    }

    pub fn clear(&mut self) {
        *self = EventRing::default();
    }

    /// `eventSequence` and the four slots, through whatever setter the
    /// caller writes its entity or playerstate fields with.
    pub fn write(&self, set: &mut impl FnMut(&str, i32)) {
        set("eventSequence", self.seq & 0xff);
        for (i, (ev, parm)) in self.events.iter().zip(&self.parms).enumerate() {
            set(&format!("events[{i}]"), *ev);
            set(&format!("eventParms[{i}]"), *parm);
        }
    }
}

/// `pml.frametime`: `msec` times the float 0.001, rounded once into the float
/// store (`PmoveSingle` 0x340b8). It is not `msec / 1000` for 32 of the 66
/// lengths a step can have, 18 ms among them.
pub fn frametime(msec: i32) -> f32 {
    (f64::from(msec) * f64::from(0.001f32)) as f32
}

/// `Pmove`'s walk from `command_time` up to the cmd's clock: steps of at most
/// `MAX_FRAME_MS`, each its own `PmoveSingle` on a cmd stamped at the step's
/// end (0x344e4, and 0x34074 for what lands in `commandTime`), with the
/// arrears past `MAX_PMOVE_ARREARS_MS` dropped. Runs `step` on each step's
/// cmd and its length in seconds, and nothing for a cmd already run
/// (docs/protocol-1.1.md, "How long a cmd is simulated for"). `step` returns
/// the held-jump latch (`pm_flags` 0x8) it left: while that is set `Pmove`
/// writes 20 into the cmd's `upmove` (0x344f2-0x344f8, 1.5b 0x28ad5, UO
/// 0x2e29a), so every later step's air and ladder wish reads 20, not the
/// key's 127 (cod11-mantle.md, "The jump").
pub fn chop(command_time: i32, cmd: &UserCmd, mut step: impl FnMut(&UserCmd, f32) -> bool) {
    let mut cmd = *cmd;
    let dt_ms = cmd.server_time.wrapping_sub(command_time);
    let mut base = if dt_ms <= 0 {
        cmd.server_time
    } else if dt_ms > MAX_PMOVE_ARREARS_MS {
        cmd.server_time - MAX_PMOVE_ARREARS_MS
    } else {
        command_time
    };
    while base != cmd.server_time {
        let msec = (cmd.server_time - base).min(MAX_FRAME_MS as i32);
        base += msec;
        let at = UserCmd {
            server_time: base,
            ..cmd
        };
        if step(&at, frametime(msec)) {
            cmd.up = 20;
        }
    }
}

/// What a live player's step left beside the playerstate.
#[derive(Clone, Debug)]
pub struct PlayerStep {
    /// Every event the step raised, in the ring too. A grenade's fire event
    /// still carries the fuse here (`player_step`).
    pub events: Vec<PmEvent>,
    /// `ps.viewangles` after the prone caps, wire convention.
    pub view: [f32; 3],
    /// `viewHeightLerpTime`: the serverTime the eye's running leg began, 0
    /// once it settles.
    pub view_lerp_start: i32,
}

/// One `PmoveSingle` for a free-flying spectator (`pm_type` 4), on one of
/// [`chop`]'s steps; returns the view. `PM_CheckDuck`'s `pm_type` 4 arm
/// (0x31749-0x31767) takes a prone key off the cmd and raises
/// `EV_STANCE_FORCE_STAND`. `world` `None` flies unclipped (a host with no
/// map); `spectator` false skips the prone arm, for a player flying there.
pub fn spectator_step(
    ps: &mut PlayerState,
    delta_angles: [i32; 3],
    ring: &mut EventRing,
    cmd: &UserCmd,
    dt: f32,
    world: Option<&MoveWorld>,
    spectator: bool,
) -> [f32; 3] {
    let view = apply_view(ps, cmd.angles, delta_angles);
    let cmd = &talk_masked(cmd, ps.rules);
    if spectator && cmd.wbuttons & msg::WBUTTON_PRONE != 0 {
        ring.add(crate::net::event_ids::EV_STANCE_FORCE_STAND, 0);
    }
    let (f, r, u) = (
        f32::from(cmd.forward),
        f32::from(cmd.right),
        f32::from(cmd.up),
    );
    for e in super::spectator_move(ps, world, f, r, u, cmd.wbuttons, dt) {
        ring.add(e.event, e.parm);
    }
    view
}

/// One `PmoveSingle` for a dead body, on one of [`chop`]'s steps:
/// [`super::dead_move`] and its landing events into the ring for `pm_type`
/// 6, [`super::dead_linked_move`] for 7 (`ps.linked`, the caller's), and
/// `PM_Weapon`'s `pm_type > 5` arm, which drops the weapon behind its
/// `PMF_RESPAWNED` return (0x390ee..0x390fe). The server's dead arm and the
/// client's prediction of its own corpse both run it
/// (cod11-player-clip.md 1.1). `PmoveSingle`'s head runs ahead of the
/// dispatch for every `pm_type`: `PM_AdjustAimSpreadScale` on the held
/// weapon, which empties the cone once the weapon is 0, and the backpedal
/// flag off the cmd's keys (UO 0x3d530, cod11-player-clip.md 1.1).
pub fn dead_step(
    ps: &mut PlayerState,
    ring: &mut EventRing,
    cmd: &UserCmd,
    dt: f32,
    world: &MoveWorld,
    weapons: &[Option<WeaponDef>],
    respawned: bool,
) {
    let input = pm_input(&talk_masked(cmd, ps.rules));
    // Its airborne test is `pm_type != 1`, which a linked corpse (7) passes.
    let linked = std::mem::replace(&mut ps.linked, false);
    weapon::adjust_aim_spread_scale(
        ps,
        &input,
        weapons.get(ps.weapon as usize).and_then(Option::as_ref),
        dt,
    );
    ps.linked = linked;
    ps.last_cmd_angles = input.angles;
    if input.forward < 0.0 {
        ps.backwards_run = true;
    } else if input.forward > 0.0 || input.right != 0.0 {
        ps.backwards_run = false;
    }
    if ps.linked {
        super::dead_linked_move(ps, world, dt);
    } else {
        for e in super::dead_move(ps, world, dt) {
            ring.add(e.event, e.parm);
        }
    }
    if !respawned {
        ps.weapon = 0;
    }
}

/// One `PmoveSingle` for a live player, on one of [`chop`]'s steps.
/// `delta_angles` are the raw wire values the cmd's angles are offset by, and
/// `ps.linked` is the caller's to set.
pub fn player_step(
    ps: &mut PlayerState,
    delta_angles: &mut [i32; 3],
    ring: &mut EventRing,
    cmd: &UserCmd,
    dt: f32,
    world: &MoveWorld,
    weapons: &[Option<WeaponDef>],
) -> PlayerStep {
    super::weapon::prone_move_key(ps, cmd, weapons);
    let mut view = apply_view(ps, cmd.angles, *delta_angles);
    clamp_view(ps, cmd.angles, delta_angles, &mut view);
    let events = super::pmove(
        ps,
        &pm_input(&talk_masked(cmd, ps.rules)),
        world,
        dt,
        weapons,
    );
    // `pm->oldcmd` is the raw cmd: a talking cmd's sight still counts for
    // the next one's prone crawl test.
    ps.last_cmd_ads = cmd.buttons & msg::BUTTON_ADS != 0;
    ps.last_cmd_moves = [cmd.forward, cmd.right];
    // Retail holds a prone view inside the cone around the body and the pitch
    // cap off the ground by pushing `delta_angles`, so the client's own
    // prediction lands in the same place, and the view the snapshot and the
    // aim carry is the capped one (docs/research/cod11-mantle.md, "Prone").
    // Each push is the excess times 182.04445 on the x87 stack, truncated.
    let corrections = [ps.view_pitch_correction, ps.view_yaw_correction];
    let capped = [
        ps.view_capped[0].unwrap_or(-ps.pitch.to_degrees()),
        ps.view_capped[1].unwrap_or(ps.yaw.to_degrees()),
    ];
    for (i, c) in corrections.into_iter().enumerate() {
        if c != 0.0 {
            delta_angles[i] = (delta_angles[i] + super::angle_shorts(c)) & 0xffff;
            view[i] = capped[i];
        }
    }
    // 1.5's pushes come truncated one by one.
    if let Some(shorts) = ps.view_yaw_shorts {
        delta_angles[1] = (delta_angles[1] + shorts) & 0xffff;
        view[1] = capped[1];
    }
    // The fire event's parm is vcod's internal fuse channel, and
    // `eventParms[i]` is 8 bits: a 4000 ms fuse would reach a client as 160
    // where retail writes 0.
    for e in &events {
        let parm = match e.event {
            weapon::EV_FIRE_WEAPON | weapon::EV_FIRE_WEAPON_LASTSHOT => 0,
            _ => e.parm,
        };
        ring.add(e.event, parm);
    }
    PlayerStep {
        view_lerp_start: ps.view_lerp_stamp(cmd.server_time),
        events,
        view,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cmd(server_time: i32) -> UserCmd {
        UserCmd {
            server_time,
            ..Default::default()
        }
    }

    fn steps(command_time: i32, server_time: i32) -> Vec<(i32, f32)> {
        let mut out = Vec::new();
        chop(command_time, &cmd(server_time), |c, dt| {
            out.push((c.server_time, dt));
            false
        });
        out
    }

    /// The bit table measured off a retail 1.1 client on 2026-09-01, one case
    /// per movement verb. Evidence and the full table:
    /// docs/protocol-1.1.md, "Usercmd input bits".
    #[test]
    fn wire_bits_map_to_movement_verbs() {
        let of = |buttons: u8, wbuttons: u8, up: i8| {
            pm_input(&UserCmd {
                buttons,
                wbuttons,
                up,
                ..Default::default()
            })
        };
        assert!(of(0, 0, 127).jump);
        let crouch = of(0, msg::WBUTTON_CROUCH, -127);
        assert!(crouch.crouch && !crouch.prone);
        let prone = of(0, msg::WBUTTON_PRONE, -127);
        assert!(prone.prone && !prone.crouch);
        assert!(of(0, msg::WBUTTON_LEAN_LEFT, 0).lean_left);
        assert!(of(0, msg::WBUTTON_LEAN_RIGHT, 0).lean_right);
        // A crouched or prone client holds `up` at -127 for as long as it
        // stays down, so only a positive `up` is a jump.
        assert!(!crouch.jump && !prone.jump);
    }

    /// The weapon bits reach the weapon half of pmove, and nothing else: CoD 1
    /// has a single move speed with no walk key, so no input reaches pmove's
    /// walk scale. UO's sprint bit (0x8) rides beside them; only UO's rules
    /// read it.
    #[test]
    fn weapon_bits_reach_the_weapon_input_only() {
        let all = pm_input(&UserCmd {
            buttons: 0xff,
            wbuttons: msg::WBUTTON_RELOAD,
            weapon: 7,
            ..Default::default()
        });
        assert!(all.attack && all.melee && all.reload && all.ads && all.use_button && all.sprint);
        assert_eq!(all.weapon, 7);
        assert!(!all.walk_slow);
        assert_eq!(
            PmInput {
                attack: false,
                melee: false,
                reload: false,
                ads: false,
                use_button: false,
                sprint: false,
                weapon: 0,
                ..all
            },
            PmInput::default()
        );
    }

    #[test]
    fn a_cmd_already_run_takes_no_step() {
        assert!(steps(5000, 5000).is_empty());
        assert!(steps(5000, 4990).is_empty());
    }

    #[test]
    fn a_long_cmd_is_chopped_at_66_and_stamped_at_each_end() {
        assert_eq!(
            steps(5000, 5150),
            [
                (5066, frametime(66)),
                (5132, frametime(66)),
                (5150, frametime(18))
            ]
        );
    }

    #[test]
    fn a_latched_jump_reads_upmove_20_on_the_later_steps() {
        let held = UserCmd {
            server_time: 5150,
            up: 127,
            ..Default::default()
        };
        let mut ups = Vec::new();
        chop(5000, &held, |c, _| {
            ups.push(c.up);
            true
        });
        assert_eq!(ups, [127, 20, 20]);
        ups.clear();
        chop(5000, &held, |c, _| {
            ups.push(c.up);
            false
        });
        assert_eq!(ups, [127, 127, 127]);
    }

    #[test]
    fn the_frametime_is_msec_times_the_float_thousandth() {
        assert_eq!(frametime(17), 0.017);
        assert_eq!(frametime(18).to_bits(), 0.018f32.to_bits() + 1);
    }

    #[test]
    fn arrears_past_a_second_are_dropped() {
        let s = steps(5000, 7500);
        assert_eq!(s.first().map(|s| s.0), Some(6566));
        assert_eq!(s.last().map(|s| s.0), Some(7500));
        let ms: f32 = s.iter().map(|s| s.1).sum();
        assert!((ms - 1.0).abs() < 1e-4, "{ms}");
    }

    #[test]
    fn the_ring_counts_in_eight_bits() {
        let mut r = EventRing {
            seq: 255,
            ..Default::default()
        };
        r.add(7, 1);
        assert_eq!((r.seq, r.events[3], r.parms[3]), (0, 7, 1));
    }
}
