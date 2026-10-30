//! CoD2 1.3 movement capture: joins a server running the
//! `probe_moves` gametype (crates/cod2/tests/fixtures/moves/), waits for its
//! spawn, then plays a scripted cmd run with a retail client's timing and
//! writes every cmd beside every snapshot's playerstate, the fixture
//! `crates/cod2/tests/moves_ab.rs` replays on vcod's pmove.
//!
//!     cargo run -p vcod-cod2 --example move_probe -- 127.0.0.1:28970 \
//!         --script walk --out capture.txt [--game-dir DIR]
//!
//! Timing is `com_maxfps 85`, `cl_maxpackets 30`, `cl_packetdup 1`, the
//! retail defaults: a cmd every 11 or 12 ms, a packet every 33 to 36 ms
//! carrying its own new cmds and the previous packet's. The cmd clock is the
//! spawn snapshot's `serverTime` plus the wall time since it arrived, less
//! `--lag MS` when given. The view holds the spawn snapshot's (`MSG_SetDefaultUserCmd`'s angles), each
//! phase's yaw and pitch added to it. tools/run_probe_cod2.sh runs it beside
//! a retail server.
//!
//! The output is text: `#` comments, then in time order
//! `c <serverTime> <buttons> <weapon> <offHand> <a0> <a1> <a2> <fwd> <right>`
//! for each cmd as made, `p <n>` for each packet (its last `n` cmds), and
//! `s <messageNum> <serverTime> <field>=<hex>...` for each snapshot with the
//! playerstate netfields that changed since the last `s` line (the first
//! has every non-zero one), raw 32-bit words as the wire carries them,
//! then `S[<i>]=`, `A[<i>]=` and `C[<i>]=` for the stats, reserve and
//! clip arrays the same way. The gamestate's `CS_WEAPONS` is a
//! `# weapons` line. Each snapshot's entities follow its `s` line: `e
//! <number> <field>=<hex>...` for one that changed since the snapshot
//! logged before (every non-zero field when new), `e- <number>` for one
//! that left; then its client states the same way as `r <number> ...` and
//! `r- <number>`. Each server command is a `# cmd <serverTime of the last
//! snapshot> <text>` line; an `a <index>` (`G_SelectWeaponIndex`) selects
//! that weapon as the cgame's `CG_SelectWeaponIndex` does, until a later
//! phase names one.

mod common;

use std::fmt::Write as _;
use std::time::{Duration, Instant};

use anyhow::{Context, bail};
use common::Session;
use vcod_cod2::net::delta::ClientState2;
use vcod_cod2::net::fields::{CLIENT_STATE_FIELDS, ENTITY_STATE_FIELDS, PLAYER_STATE_FIELDS};
use vcod_cod2::net::usercmd::default_usercmd;
use vcod_cod2::net::{EntityState2, PlayerState2, UserCmd2};

const ATTACK: i32 = 0x1;
/// `+activate`; the server's use test reads 0x28 (`useButtonPressed`).
const USE: i32 = 0x20;
const LEAN_LEFT: i32 = 0x40;
const LEAN_RIGHT: i32 = 0x80;
const PRONE: i32 = 0x100;
const CROUCH: i32 = 0x200;
const JUMP: i32 = 0x400;
const MELEE: i32 = 0x4;
const RELOAD: i32 = 0x10;
const ADS: i32 = 0x1000;
const FRAG: i32 = 0x1_0000;
const BREATH: i32 = 0x8000;

/// One stretch of held input: buttons, the move keys, and the view's turn
/// off the spawn view in degrees.
#[derive(Clone, Copy)]
struct Phase {
    ms: i32,
    buttons: i32,
    fwd: i8,
    right: i8,
    yaw: f32,
    pitch: f32,
    /// The weapon the cmds select from this phase on, by name (`None`
    /// keeps the selection).
    weapon: Option<&'static str>,
}

const fn ph(ms: i32, buttons: i32, fwd: i8, right: i8) -> Phase {
    Phase {
        ms,
        buttons,
        fwd,
        right,
        yaw: 0.0,
        pitch: 0.0,
        weapon: None,
    }
}

/// `p` with the cmds selecting `weapon` from its start.
const fn select(p: Phase, weapon: &'static str) -> Phase {
    Phase {
        weapon: Some(weapon),
        ..p
    }
}

const fn turn(p: Phase, yaw: f32) -> Phase {
    Phase { yaw, ..p }
}

/// The view pitched off the spawn's, positive down.
const fn look(p: Phase, pitch: f32) -> Phase {
    Phase { pitch, ..p }
}

const F: i8 = 127;
const B: i8 = -127;

/// The scripts, each opening on a still second so the replay starts from a
/// settled body.
fn script(name: &str) -> Option<Vec<Phase>> {
    let idle = |ms| ph(ms, 0, 0, 0);
    Some(match name {
        // Run each way and stop, a diagonal, a turn while running.
        "walk" => vec![
            idle(1000),
            ph(1500, 0, F, 0),
            idle(1000),
            ph(1000, 0, B, 0),
            idle(800),
            ph(1000, 0, 0, F),
            idle(800),
            ph(1000, 0, 0, B),
            idle(800),
            ph(1000, 0, F, F),
            idle(1000),
            turn(ph(700, 0, F, 0), 45.0),
            turn(idle(1000), 45.0),
        ],
        // In place, running, and a held key (one jump on its edge).
        "jump" => vec![
            idle(1000),
            ph(100, JUMP, 0, 0),
            idle(1400),
            ph(600, 0, F, 0),
            ph(100, JUMP, F, 0),
            ph(900, 0, F, 0),
            idle(1500),
            ph(1500, JUMP, 0, 0),
            idle(1000),
        ],
        // Crouch, crawl, prone, a prone turn, back up through crouch, and
        // straight from standing to prone and up.
        "stance" => vec![
            idle(800),
            ph(1500, CROUCH, 0, 0),
            ph(1000, CROUCH, F, 0),
            ph(500, CROUCH, 0, 0),
            ph(2000, PRONE, 0, 0),
            ph(1500, PRONE, F, 0),
            ph(400, PRONE, 0, 0),
            turn(ph(1000, PRONE, 0, 0), 50.0),
            turn(ph(500, PRONE, 0, 0), 50.0),
            turn(ph(1500, CROUCH, 0, 0), 50.0),
            turn(idle(1500), 50.0),
            turn(ph(2000, PRONE, 0, 0), 50.0),
            turn(idle(2000), 50.0),
        ],
        // Hard reversals: the inertia clamp.
        "reverse" => vec![
            idle(800),
            ph(800, 0, 0, F),
            ph(800, 0, 0, B),
            ph(600, 0, 0, F),
            ph(600, 0, 0, B),
            ph(500, 0, F, 0),
            ph(500, 0, B, 0),
            idle(1000),
            ph(700, 0, F, F),
            ph(700, 0, F, B),
            idle(1000),
        ],
        // Each way standing and crouched, and while moving.
        "lean" => vec![
            idle(800),
            ph(1000, LEAN_LEFT, 0, 0),
            idle(600),
            ph(1000, LEAN_RIGHT, 0, 0),
            idle(600),
            ph(800, LEAN_RIGHT, F, 0),
            idle(600),
            ph(800, CROUCH | LEAN_LEFT, 0, 0),
            ph(400, CROUCH, 0, 0),
            idle(1000),
        ],
        // Forward into whatever the spot faces, then back out: a wall at an
        // angle, stairs, a slope, a ledge.
        "forward" => vec![
            idle(1000),
            ph(2500, 0, F, 0),
            idle(1000),
            ph(2500, 0, B, 0),
            idle(1000),
        ],
        // A jump at a ledge the spot faces.
        "mantle" => vec![
            idle(1000),
            ph(700, 0, F, 0),
            ph(100, JUMP, F, 0),
            ph(1500, 0, F, 0),
            idle(1500),
        ],
        // The same jump, the view turned 90 off the wall as the body
        // climbs: past `mantle_view_yawcap`.
        "mantle_turn" => vec![
            idle(1000),
            ph(700, 0, F, 0),
            ph(100, JUMP, F, 0),
            turn(ph(1500, 0, F, 0), 90.0),
            turn(idle(1500), 90.0),
        ],
        // Walk to a ladder the spot faces and up it looking up, a hang, a
        // strafe on the rungs, down looking down, up again, the push-off,
        // then back on from the ground and over the top.
        "ladder" => vec![
            idle(1000),
            look(ph(1500, 0, F, 0), -30.0),
            look(idle(800), -30.0),
            look(ph(400, 0, 0, F), -30.0),
            look(ph(800, 0, F, 0), 60.0),
            look(ph(500, 0, F, 0), -30.0),
            look(ph(100, JUMP, 0, 0), -30.0),
            look(idle(1500), -30.0),
            look(ph(2500, 0, F, 0), -30.0),
            look(idle(1000), -30.0),
        ],
        // The weapon scripts run under `probe_combat.gsc`, which hands
        // out `probe_weapon` (primary), `probe_weapon2` (primaryb) and
        // three frags. A bolt rifle: a hip shot and its rechamber, a
        // sighted shot, a reload, a melee, a shot after it.
        "rifle" => vec![
            idle(1000),
            ph(100, ATTACK, 0, 0),
            idle(1500),
            ph(500, ADS, 0, 0),
            ph(100, ADS | ATTACK, 0, 0),
            ph(1300, ADS, 0, 0),
            idle(600),
            ph(100, RELOAD, 0, 0),
            idle(3200),
            ph(100, MELEE, 0, 0),
            idle(1300),
            ph(100, ATTACK, 0, 0),
            idle(1500),
        ],
        // An automatic: a held burst at the hip and down the sight, a
        // reload, a melee, then the trigger held through an empty clip.
        "smg" => vec![
            idle(1000),
            ph(600, ATTACK, 0, 0),
            idle(600),
            ph(500, ADS, 0, 0),
            ph(400, ADS | ATTACK, 0, 0),
            idle(600),
            ph(100, RELOAD, 0, 0),
            idle(2800),
            ph(100, MELEE, 0, 0),
            idle(1000),
            ph(2200, ATTACK, 0, 0),
            idle(400),
            ph(100, ATTACK, 0, 0),
            idle(3000),
        ],
        // A semi-automatic pistol: taps, a held trigger (one round), a
        // sighted tap, taps to empty and the reload that follows.
        "pistol" => vec![
            idle(1000),
            ph(100, ATTACK, 0, 0),
            idle(300),
            ph(100, ATTACK, 0, 0),
            idle(300),
            ph(800, ATTACK, 0, 0),
            idle(300),
            ph(400, ADS, 0, 0),
            ph(100, ADS | ATTACK, 0, 0),
            ph(300, ADS, 0, 0),
            idle(300),
            ph(100, ATTACK, 0, 0),
            idle(300),
            ph(100, ATTACK, 0, 0),
            idle(300),
            ph(100, ATTACK, 0, 0),
            idle(300),
            ph(100, ATTACK, 0, 0),
            idle(300),
            ph(100, ATTACK, 0, 0),
            idle(3500),
        ],
        // Primary to primaryb and back, a switch out of a reload, and a
        // shot right after a raise.
        "switch" => vec![
            idle(1000),
            select(idle(2000), "colt_mp"),
            ph(100, ATTACK, 0, 0),
            idle(800),
            select(idle(2000), "kar98k_mp"),
            ph(100, RELOAD, 0, 0),
            idle(500),
            select(idle(2000), "colt_mp"),
            select(idle(1500), "kar98k_mp"),
            ph(100, ATTACK, 0, 0),
            idle(1500),
        ],
        // The frag key held, then released; a tap; one sighted.
        "frag" => vec![
            idle(1000),
            ph(1500, FRAG, 0, 0),
            idle(2500),
            ph(100, FRAG, 0, 0),
            idle(2500),
            ph(400, ADS, 0, 0),
            ph(600, ADS | FRAG, 0, 0),
            ph(100, ADS, 0, 0),
            idle(4000),
        ],
        // A sighted rifle round into the target `probe_combat.gsc` aims
        // the spawn's view at, twice, the bolt between.
        "shoot" => vec![
            idle(1500),
            ph(600, ADS, 0, 0),
            ph(100, ADS | ATTACK, 0, 0),
            ph(1500, ADS, 0, 0),
            ph(100, ADS | ATTACK, 0, 0),
            ph(1500, ADS, 0, 0),
            idle(500),
        ],
        // A sighted burst.
        "burst" => vec![
            idle(1500),
            ph(600, ADS, 0, 0),
            ph(500, ADS | ATTACK, 0, 0),
            ph(1000, ADS, 0, 0),
            idle(500),
        ],
        // A scoped rifle (`probe_weapon kar98k_sniper_mp`) holding its
        // breath: a hold let go early, the timer back to 0, a hold broken
        // by a shot and taken up again, and a hold past the gasp.
        "breath" => vec![
            idle(1000),
            ph(1000, ADS, 0, 0),
            ph(2000, ADS | BREATH, 0, 0),
            ph(2500, ADS, 0, 0),
            ph(1000, ADS | BREATH, 0, 0),
            ph(100, ADS | BREATH | ATTACK, 0, 0),
            ph(1500, ADS | BREATH, 0, 0),
            ph(2500, ADS, 0, 0),
            ph(5500, ADS | BREATH, 0, 0),
            ph(2000, ADS, 0, 0),
            idle(1000),
        ],
        // A frag thrown along the spawn's view.
        "throw" => vec![idle(1500), ph(1200, FRAG, 0, 0), idle(5000)],
        // The target of a shooting script: twelve still seconds.
        "still" => vec![idle(12000)],
        // The hit pose capture's target (hitpose/probe_hitpose.gsc):
        // standing, crouched, prone, standing again, a strafe and a stop.
        "poses" => vec![
            idle(6000),
            ph(3000, CROUCH, 0, 0),
            ph(3500, PRONE, 0, 0),
            idle(3000),
            ph(1000, 0, 0, F),
            idle(2000),
        ],
        // The antilag target (probe_antilag.gsc): still, then strafing
        // each way at a run, then still.
        "weave" => vec![
            idle(4000),
            ph(600, 0, 0, F),
            ph(600, 0, 0, -F),
            ph(600, 0, 0, F),
            ph(600, 0, 0, -F),
            ph(600, 0, 0, F),
            ph(600, 0, 0, -F),
            ph(600, 0, 0, F),
            ph(600, 0, 0, -F),
            idle(3000),
        ],
        // Its shooter: the trigger held down the sight, reloads and all,
        // the gametype steering the view.
        "hose" => vec![
            idle(1500),
            ph(600, ADS, 0, 0),
            ph(17000, ADS | ATTACK, 0, 0),
        ],
        // The killcam pair (probe_killcam.gsc): "shoot" ten seconds in, so
        // the archive holds the stock 9 s, then a strafe each way inside
        // the replay window and a long wait; the target stands through its
        // death, the killcam and the dead frames after it.
        "shootlate" => vec![
            idle(10000),
            ph(600, ADS, 0, 0),
            ph(100, ADS | ATTACK, 0, 0),
            ph(1500, ADS, 0, 0),
            ph(2000, 0, 0, F),
            ph(2000, 0, 0, B),
            idle(14000),
        ],
        "killcam" => vec![idle(31000)],
        // Behind the turret with a target in its arc (probe_combat.gsc,
        // `probe_turret`): the use tap mounts, a held burst, a tap.
        "mgfire" => vec![
            idle(1000),
            ph(100, USE, 0, 0),
            idle(900),
            ph(1500, ATTACK, 0, 0),
            idle(1000),
            ph(60, ATTACK, 0, 0),
            idle(1500),
        ],
        // The use key on an mp40 with both slots full (probe_pickup.gsc):
        // the swap's `a`, the raise, then a shot from the new weapon.
        "pickup" => vec![
            idle(1000),
            ph(100, USE, 0, 0),
            idle(2500),
            ph(100, ATTACK, 0, 0),
            idle(1500),
        ],
        // Behind a turret (probe_turret.gsc): the use tap mounts, a look
        // inside the arcs and past them each way, a held trigger, a tap,
        // the use tap that lets go, and a look about once off the gun.
        "turret" => vec![
            idle(1000),
            ph(100, USE, 0, 0),
            idle(900),
            turn(idle(600), 20.0),
            look(turn(idle(600), 20.0), 10.0),
            turn(idle(600), 70.0),
            turn(idle(600), -70.0),
            look(idle(600), 40.0),
            look(idle(600), -40.0),
            idle(600),
            ph(1000, ATTACK, 0, 0),
            idle(500),
            ph(60, ATTACK, 0, 0),
            idle(800),
            turn(ph(600, ATTACK, 0, 0), 30.0),
            turn(idle(800), 30.0),
            turn(ph(100, USE, 0, 0), 30.0),
            turn(idle(900), 30.0),
            turn(idle(600), 80.0),
            idle(1000),
        ],
        // Killed on the gun (probe_turret.gsc, `probe_die`): the use tap
        // mounts, the trigger held through the suicide, the dead frames, the
        // respawn behind the gun and a look about.
        "turretdie" => vec![
            idle(1000),
            ph(100, USE, 0, 0),
            idle(900),
            turn(idle(600), 20.0),
            turn(ph(2000, ATTACK, 0, 0), 20.0),
            idle(3500),
            turn(idle(1000), 40.0),
        ],
        // A gunner shot off the gun while firing (probe_turret.gsc,
        // `probe_shooter`, beside `mgkill`): the use tap mounts, the
        // trigger held through the death, the dead frames and the respawn.
        "turretfire" => vec![
            idle(1000),
            ph(100, USE, 0, 0),
            idle(900),
            ph(4000, ATTACK, 0, 0),
            idle(3500),
            turn(idle(1000), 40.0),
        ],
        // Its shooter, joined two seconds before it: three sighted shots at
        // the gunner once he fires.
        "mgkill" => vec![
            idle(5000),
            ph(600, ADS, 0, 0),
            ph(100, ADS | ATTACK, 0, 0),
            ph(1400, ADS, 0, 0),
            ph(100, ADS | ATTACK, 0, 0),
            ph(1400, ADS, 0, 0),
            ph(100, ADS | ATTACK, 0, 0),
            idle(3000),
        ],
        // Two players at one gun (probe_turret.gsc, `probe_side`): this one
        // mounts, fires a burst, lets go and backs off the gun's spot;
        // `turretwait`, joined two seconds before it, taps use while the
        // gun is manned and again once it is free.
        "turretshare" => vec![
            idle(1000),
            ph(100, USE, 0, 0),
            idle(1900),
            ph(300, ATTACK, 0, 0),
            idle(1200),
            ph(100, USE, 0, 0),
            idle(300),
            ph(600, 0, B, 0),
            idle(4100),
        ],
        "turretwait" => vec![
            idle(4500),
            ph(100, USE, 0, 0),
            idle(3500),
            ph(100, USE, 0, 0),
            idle(1000),
            ph(300, ATTACK, 0, 0),
            idle(700),
            ph(100, USE, 0, 0),
            idle(1500),
        ],
        // The weapon's torso events: two shots and their rechambers, the
        // reload, a melee swing, a crouched and a prone shot, a reload
        // interrupted by standing out of prone, a sighted shot, a shot on
        // the run.
        "fire" => vec![
            idle(1000),
            ph(100, ATTACK, 0, 0),
            idle(1400),
            ph(100, ATTACK, 0, 0),
            idle(1400),
            ph(100, RELOAD, 0, 0),
            idle(3000),
            ph(100, MELEE, 0, 0),
            idle(1500),
            ph(800, CROUCH, 0, 0),
            ph(100, CROUCH | ATTACK, 0, 0),
            ph(1500, CROUCH, 0, 0),
            ph(1500, PRONE, 0, 0),
            ph(100, PRONE | ATTACK, 0, 0),
            ph(1500, PRONE, 0, 0),
            ph(100, PRONE | RELOAD, 0, 0),
            ph(500, PRONE, 0, 0),
            ph(2500, CROUCH, 0, 0),
            idle(1000),
            ph(800, ADS, 0, 0),
            ph(100, ADS | ATTACK, 0, 0),
            ph(1400, ADS, 0, 0),
            ph(600, 0, F, 0),
            ph(100, ATTACK, F, 0),
            ph(900, 0, F, 0),
            idle(1000),
        ],
        // Under `probe_hurt`'s hits (probe_moves.gsc, one every 0.7 s, each
        // 45 degrees on from the last): standing still, running each way,
        // crouched still and running, and a sighted walk.
        "hurt" => vec![
            idle(4000),
            ph(2000, 0, F, 0),
            ph(2000, 0, B, 0),
            ph(1500, CROUCH, 0, 0),
            ph(2000, CROUCH, F, 0),
            ph(2000, CROUCH, B, 0),
            idle(1000),
            ph(600, ADS, 0, 0),
            ph(2000, ADS, F, 0),
            ph(1500, ADS, B, 0),
            idle(1000),
        ],
        _ => return None,
    })
}

/// `ANGLE2SHORT`.
fn angle_short(deg: f32) -> i32 {
    ((deg * 65536.0 / 360.0) as i32) & 0xffff
}

fn ps_line(out: &mut String, prev: &PlayerState2, ps: &PlayerState2, full: bool) {
    for (i, f) in PLAYER_STATE_FIELDS.iter().enumerate() {
        let (a, b) = (prev.f[i], ps.f[i]);
        if (full && b != 0) || (!full && a != b) {
            let _ = write!(out, " {}={b:x}", f.name);
        }
    }
    // The array blocks as words: `S[i]` the stats, `A[i]` the reserve
    // and `C[i]` the clip rounds.
    for (tag, a, b) in [
        ("S", &prev.stats[..], &ps.stats[..]),
        ("A", &prev.ammo[..], &ps.ammo[..]),
        ("C", &prev.ammoclip[..], &ps.ammoclip[..]),
    ] {
        for (i, (&x, &y)) in a.iter().zip(b).enumerate() {
            if (full && y != 0) || (!full && x != y) {
                let _ = write!(out, " {tag}[{i}]={y:x}");
            }
        }
    }
}

/// `e <number> <field>=<hex>...` for each entity that changed since the
/// last snapshot logged (all its non-zero fields when new), `e- <number>`
/// for each that left.
fn entity_lines(out: &mut String, prev: &[EntityState2], now: &[EntityState2]) {
    for e in now {
        let old = prev.iter().find(|p| p.number == e.number);
        let fields: String = ENTITY_STATE_FIELDS
            .iter()
            .enumerate()
            .filter(|&(i, _)| match old {
                Some(o) => o.f[i] != e.f[i],
                None => e.f[i] != 0,
            })
            .map(|(i, f)| format!(" {}={:x}", f.name, e.f[i]))
            .collect();
        if old.is_none() || !fields.is_empty() {
            let _ = writeln!(out, "e {}{fields}", e.number);
        }
    }
    for p in prev {
        if !now.iter().any(|e| e.number == p.number) {
            let _ = writeln!(out, "e- {}", p.number);
        }
    }
}

/// `r <number> <field>=<hex>...` for each client state that changed
/// since the last snapshot logged (all its non-zero fields when new),
/// `r- <number>` for each that left.
fn client_lines(out: &mut String, prev: &[ClientState2], now: &[ClientState2]) {
    for c in now {
        let old = prev.iter().find(|p| p.number == c.number);
        let fields: String = CLIENT_STATE_FIELDS
            .iter()
            .enumerate()
            .filter(|&(i, _)| match old {
                Some(o) => o.f[i] != c.f[i],
                None => c.f[i] != 0,
            })
            .map(|(i, f)| format!(" {}={:x}", f.name, c.f[i]))
            .collect();
        if old.is_none() || !fields.is_empty() {
            let _ = writeln!(out, "r {}{fields}", c.number);
        }
    }
    for p in prev {
        if !now.iter().any(|c| c.number == p.number) {
            let _ = writeln!(out, "r- {}", p.number);
        }
    }
}

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let addr = args
        .first()
        .context("usage: move_probe <ip:port> --script NAME --out FILE")?;
    let opt = |k: &str| {
        args.iter()
            .position(|a| a == k)
            .and_then(|i| args.get(i + 1))
    };
    let name = opt("--script").context("--script")?;
    let phases = script(name).with_context(|| format!("no script {name}"))?;
    let out_path = opt("--out").context("--out")?;
    let fs = common::game_fs(opt("--game-dir").cloned())?;

    let mut s = Session::open(addr, false, 0)?;
    let name_opt = opt("--name").map_or("vcodmoves", |n| n.as_str());
    // `--lag MS`: every cmd's clock held that far behind the spawn
    // anchor's, a client seeing the world late (the antilag capture).
    let lag: i32 = opt("--lag").map_or(Ok(0), |v| v.parse()).context("--lag")?;
    let mut cl = s.connect(&format!(
        "\\name\\{name_opt}\\rate\\25000\\snaps\\20\\cl_punkbuster\\0"
    ))?;
    let mut out =
        format!("# move_probe --script {name}: com_maxfps 85, cl_maxpackets 30, cl_packetdup 1\n");

    let start = Instant::now();
    let mut last_send = Instant::now() - Duration::from_secs(1);
    let mut snap_at = Instant::now();
    // Set at the spawn: (snapshot serverTime, when it came, the held cmd).
    let mut anchor: Option<(i32, Instant, UserCmd2)> = None;
    let mut logged: Option<PlayerState2> = None;
    let mut logged_ents: Vec<EntityState2> = Vec::new();
    let mut logged_clients: Vec<ClientState2> = Vec::new();
    // CS_WEAPONS, once the gamestate holds it.
    let mut weapon_names: Vec<String> = Vec::new();
    let mut cmds: Vec<UserCmd2> = Vec::new();
    // The last `a <index>` the server sent, and when (ms past the anchor).
    let mut selected: Option<(u8, i32)> = None;
    // cmds.len() at each of the last two packets.
    let mut sent_at = [0usize; 2];
    let mut next_frame = Instant::now();
    let total: i32 = phases.iter().map(|p| p.ms).sum();

    loop {
        if start.elapsed() > Duration::from_secs(60) {
            bail!("timed out");
        }
        while let Some(d) = s.recv() {
            if vcod_cod2::net::oob::payload(&d).is_some() {
                continue;
            }
            let Some(p) = cl.receive(&d)? else { continue };
            let last_time = cl.parser.snapshot().map_or(0, |f| f.server_time);
            for (_, text) in &p.commands {
                let text = vcod_cod2::net::latin1(text);
                let _ = writeln!(out, "# cmd {last_time} {text}");
                if let Some(w) = text.strip_prefix("a ").and_then(|w| w.trim().parse().ok())
                    && let Some((_, at0, _)) = anchor
                {
                    selected = Some((w, Instant::now().duration_since(at0).as_millis() as i32));
                }
            }
            if p.gamestate {
                let gs = cl.parser.gamestate.as_ref().unwrap();
                let cp = common::cp_for(fs.as_ref(), gs);
                if weapon_names.is_empty() {
                    let cs = gs.configstring(vcod_cod2::net::protocol::CS_WEAPONS as u16);
                    let _ = writeln!(out, "# weapons {cs}");
                    weapon_names = cs.split(' ').map(str::to_string).collect();
                }
                cl.add_reliable(&cp);
            }
            let Some(n) = p.snapshot else { continue };
            snap_at = Instant::now();
            let snap = cl.parser.frame(n).unwrap();
            // The spawn: playing (pm_type 0) with the rifle in hand.
            if anchor.is_none() && snap.ps.i32("pm_type") == 0 && snap.ps.i32("weapon") != 0 {
                let held = default_usercmd(&snap.ps);
                anchor = Some((snap.server_time, snap_at, held));
                let o = snap.ps.origin();
                let _ = writeln!(
                    out,
                    "# spawn ({} {} {}) view ({} {})",
                    o[0],
                    o[1],
                    o[2],
                    snap.ps.f32("viewangles[0]"),
                    snap.ps.f32("viewangles[1]")
                );
            }
            if anchor.is_some() {
                let _ = write!(out, "s {} {}", snap.message_num, snap.server_time);
                match &logged {
                    Some(prev) => ps_line(&mut out, prev, &snap.ps, false),
                    None => ps_line(&mut out, &snap.ps, &snap.ps, true),
                }
                out.push('\n');
                logged = Some(snap.ps.clone());
                entity_lines(&mut out, &logged_ents, &snap.entities);
                logged_ents = snap.entities.clone();
                client_lines(&mut out, &logged_clients, &snap.clients);
                logged_clients = snap.clients.clone();
            }
        }
        let Some((t0, at0, held)) = anchor else {
            // Not spawned yet: an idle cmd every 33 ms, as the spectator does.
            if cl.parser.gamestate.is_some() && last_send.elapsed() >= Duration::from_millis(33) {
                last_send = Instant::now();
                let base = cl.parser.snapshot().map_or(0, |f| f.server_time);
                let cmd = cl.idle_cmd(base + snap_at.elapsed().as_millis() as i32);
                let pkt = cl.build_packet(&[cmd])?;
                s.send(&pkt)?;
            } else if cl.parser.gamestate.is_none()
                && last_send.elapsed() >= Duration::from_millis(500)
            {
                last_send = Instant::now();
                let pkt = cl.build_packet(&[])?;
                s.send(&pkt)?;
            }
            std::thread::sleep(Duration::from_millis(1));
            continue;
        };
        let now = Instant::now();
        if now < next_frame {
            std::thread::sleep(Duration::from_micros(200));
            continue;
        }
        // com_maxfps 85: a frame once 1000 / 85 = 11 whole ms have passed.
        next_frame = now + Duration::from_millis(11);
        let t = (now - at0).as_millis() as i32;
        if t >= total + 500 {
            break;
        }
        // The phase this frame falls in; past the end, the last one's view, still.
        let mut acc = 0;
        let phase = phases
            .iter()
            .find(|p| {
                acc += p.ms;
                t < acc
            })
            .copied()
            .unwrap_or(ph(0, 0, 0, 0));
        let last = phases.last().unwrap();
        // The selection: the last phase up to this one that names a weapon.
        // A server `a` stands until a later phase names a weapon.
        let mut upto = 0;
        let named = phases
            .iter()
            .take_while(|p| {
                let before = upto <= t;
                upto += p.ms;
                before
            })
            .scan(0, |start, p| {
                let at = *start;
                *start += p.ms;
                Some((at, p.weapon))
            })
            .filter_map(|(at, w)| Some((at, w?)))
            .last()
            .and_then(|(at, w)| Some((at, weapon_names.iter().position(|n| n == w)? as u8 + 1)));
        let weapon = match (named, selected) {
            (Some((at, w)), Some((s, sat))) => {
                if sat > at {
                    s
                } else {
                    w
                }
            }
            (Some((_, w)), None) => w,
            (None, Some((s, _))) => s,
            (None, None) => held.weapon,
        };
        let (yaw, pitch) = if t < acc {
            (phase.yaw, phase.pitch)
        } else {
            (last.yaw, last.pitch)
        };
        let cmd = UserCmd2 {
            server_time: t0 + t - lag,
            buttons: phase.buttons,
            forward_move: phase.fwd,
            right_move: phase.right,
            angles: [
                (held.angles[0] + angle_short(pitch)) & 0xffff,
                (held.angles[1] + angle_short(yaw)) & 0xffff,
                held.angles[2],
            ],
            weapon,
            ..held
        };
        if cmds
            .last()
            .is_some_and(|c| c.server_time >= cmd.server_time)
        {
            continue;
        }
        let _ = writeln!(
            out,
            "c {} {:x} {} {} {} {} {} {} {}",
            cmd.server_time,
            cmd.buttons,
            cmd.weapon,
            cmd.off_hand_index,
            cmd.angles[0],
            cmd.angles[1],
            cmd.angles[2],
            cmd.forward_move,
            cmd.right_move
        );
        cmds.push(cmd);
        // cl_maxpackets 30: a packet once 1000 / 30 = 33 ms have passed,
        // carrying the cmds since the packet before last (cl_packetdup 1).
        if last_send.elapsed() >= Duration::from_millis(33) {
            last_send = Instant::now();
            let from = sent_at[0].min(cmds.len() - 1);
            let batch = &cmds[from..];
            let batch = &batch[batch.len().saturating_sub(32)..];
            let pkt = cl.build_packet(batch)?;
            s.send(&pkt)?;
            let _ = writeln!(out, "p {}", batch.len());
            sent_at = [sent_at[1], cmds.len()];
        }
    }
    cl.add_reliable("disconnect");
    for _ in 0..3 {
        let pkt = cl.build_packet(&[])?;
        s.send(&pkt)?;
    }
    std::fs::write(out_path, out)?;
    println!("wrote {out_path}: {} cmds", cmds.len());
    Ok(())
}
