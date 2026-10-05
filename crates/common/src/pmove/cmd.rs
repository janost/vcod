//! One usercmd through the mover: the input words, the view, `Pmove`'s chop
//! and a live player's step with its event ring. The server's
//! `ClientSim::step` and the client's predictor (`super::predict`) both run
//! a playing client's cmds through here, so a prediction replays the
//! server's own code.

use super::{MAX_FRAME_MS, PlayerState, PmEvent, PmInput, weapon};
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
        jump: cmd.up > 0,
        crouch: cmd.wbuttons & msg::WBUTTON_CROUCH != 0,
        prone: cmd.wbuttons & msg::WBUTTON_PRONE != 0,
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
    ps.yaw = view[1].to_radians();
    ps.pitch = -view[0].to_radians();
    view
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

/// `Pmove`'s walk from `command_time` up to the cmd's clock: steps of at most
/// `MAX_FRAME_MS`, each its own `PmoveSingle` on a cmd stamped at the step's
/// end (0x344e4, and 0x34074 for what lands in `commandTime`), with the
/// arrears past `MAX_PMOVE_ARREARS_MS` dropped. Yields each step's cmd and
/// its length in seconds; nothing for a cmd already run (docs/protocol-1.1.md,
/// "How long a cmd is simulated for").
pub fn chop(command_time: i32, cmd: &UserCmd) -> impl Iterator<Item = (UserCmd, f32)> + use<> {
    let cmd = *cmd;
    let dt_ms = cmd.server_time.wrapping_sub(command_time);
    let mut base = if dt_ms <= 0 {
        cmd.server_time
    } else if dt_ms > MAX_PMOVE_ARREARS_MS {
        cmd.server_time - MAX_PMOVE_ARREARS_MS
    } else {
        command_time
    };
    std::iter::from_fn(move || {
        (base != cmd.server_time).then(|| {
            let msec = (cmd.server_time - base).min(MAX_FRAME_MS as i32);
            base += msec;
            let step = UserCmd {
                server_time: base,
                ..cmd
            };
            (step, msec as f32 / 1000.0)
        })
    })
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
    let mut view = apply_view(ps, cmd.angles, *delta_angles);
    let events = super::pmove(ps, &pm_input(cmd), world, dt, weapons);
    // Retail holds a prone view inside the cone around the body and the pitch
    // cap off the ground by pushing `delta_angles`, so the client's own
    // prediction lands in the same place, and the view the snapshot and the
    // aim carry is the capped one (docs/research/cod11-mantle.md, "Prone").
    let corrections = [ps.view_pitch_correction, ps.view_yaw_correction];
    let capped = [-ps.pitch.to_degrees(), ps.yaw.to_degrees()];
    for (i, c) in corrections.into_iter().enumerate() {
        if c != 0.0 {
            delta_angles[i] = (delta_angles[i] + (c * ANGLE2SHORT) as i32) & 0xffff;
            view[i] = capped[i];
        }
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
        chop(command_time, &cmd(server_time))
            .map(|(c, dt)| (c.server_time, dt))
            .collect()
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
    /// walk scale.
    #[test]
    fn weapon_bits_reach_the_weapon_input_only() {
        let all = pm_input(&UserCmd {
            buttons: 0xff,
            wbuttons: msg::WBUTTON_RELOAD,
            weapon: 7,
            ..Default::default()
        });
        assert!(all.attack && all.melee && all.reload && all.ads && all.use_button);
        assert_eq!(all.weapon, 7);
        assert!(!all.walk_slow);
        assert_eq!(
            PmInput {
                attack: false,
                melee: false,
                reload: false,
                ads: false,
                use_button: false,
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
            [(5066, 0.066), (5132, 0.066), (5150, 0.018)]
        );
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
