//! CoD4 1.7 in-game probe: joins a retail `cod4_lnxded` the way the 1.7
//! client does (challenge, connect, the seven stats packets), decodes the
//! gamestate, then answers it with move messages carrying its `serverId`,
//! `messageAcknowledge` and reliable ack, and logs every datagram both ways
//! as raw hex, the line format of `tools/cod4_handshake_probe.py`. The join
//! and the move messages are `vcod_cod4::net::conn`'s, the client's own.
//!
//!     cargo run -p vcod-cod4 --example cod4_probe -- HOST:PORT OUT.txt [--secs 15] [--auth 127.0.0.2]
//!         [--team allies|axis|spectator] [--class assault_mp,0] [--script walk|idle|follow|shoot|fly|mantle|frag|flash|melee|lunge|auto|saw|throwback|sway|poses|prone|rocket|push]
//!         [--name NAME] [--qport N] [--iwds MAIN_DIR] [--no-cp] [--goto "X Y Z"] [--scores] [--weapon N] [--pitch P]
//!         [--download auto|FILE,FILE...] [--dl-dir DIR] [--cmd "SECS TEXT"]...
//!
//! Without `--team` it stays a spectator in the team menu
//! (docs/research/cod4-netfields.md, "The snapshot capture"). With it, it
//! answers the team menu (`mr`) a second after the first snapshot and the
//! class menu 1.5 s later, and from its spawn plays `--script`: `walk`
//! walks, strafes, jumps, crouches, goes prone, fires the rifle in taps,
//! throws a frag, says a line and turns (section 8); `idle` stands; `follow`
//! (a spectator) taps attack every 3 s; `shoot` aims down its sights at the
//! nearest other player in its snapshot and taps attack every 250 ms; `fly`
//! (a spectator) flies its keys from its first snapshot (cod4-pmove.md 13);
//! `mantle` faces yaw 0, walks into what is ahead and taps jump at 2 s,
//! turning 120 to the left over the mantle, then faces 180 and does the
//! same back at 6 s, then leans left at 9 s and right at 10.5 s
//! (cod4-pmove.md 17; use it with `--goto`); `frag` and `melee` view the
//! nearest other player as `shoot` does and, from 5 s after the spawn (past
//! the prematch freeze),
//! hold the frag key 0.8 s once, or tap the melee key every second
//! (cod4-combat.md 10); `lunge` views the nearest other player and
//! presses the melee key once at 5 s, the cmd carrying 1.7's charge pick
//! (`pmove::charge::client_pick`, cod4-pmove.md 19); `auto` holds attack from 7 to 9 s and from 11 to
//! 13 s after the spawn, the view where the spawn left it (the fire rate
//! capture, cod4-combat.md 2.1). `saw` faces `--face YAW` level, taps use
//! at 2 s, fires 3 to 5 s, swings the view 70 right and 70 left of it 5 to
//! 8 s, taps use again at 8.5 s and fires 9.5 to 10.5 s (a mounted SAW,
//! cod4-gsc.md 9.2; use it with `--goto` behind the gun). `throwback`
//! views the nearest other player and holds the frag key while its
//! playerstate's `throwBackGrenadeTimeLeft` is set and no owner is
//! named, so a live frag under its cursor hint goes back (9.3). `sway`
//! views the nearest other player as `shoot` does and from 6 s after the
//! spawn taps attack 250 ms apart while strafing, leaning, swinging the
//! view and crouching, sighted and from the hip (the shot aim capture,
//! cod4-combat.md 3.2). `poses` plays one state a life, by the life's
//! number modulo 7: standing, crouched, prone, firing taps 300 ms apart
//! from 1 s, a tap at 1 s and the reload key at 2 s, a jump at 2.4 s, a
//! switch to the Beretta at 2.2 s (the corpse captures, cod4-combat.md 5).
//! `prone` holds the prone key from 1 s past the spawn (`poses`' prone
//! life, holding it from the spawn, never lay down). `rocket` selects weapon
//! `--weapon N` from 1.5 s, taps attack at 4.5 s along `--face` and
//! `--pitch` (-15 without it), then views the nearest other player from
//! 9 s and taps again at 10 s (the rocket captures, cod4-combat.md 12).
//! `push` faces `--face` level, walks forward 3 to 6 s, strafes right 6
//! to 7 s, backs off 7 to 8.5 s and walks forward again 8.5 to 10 s: into
//! a player standing ahead (the touch capture, cod4-pmove.md 15.2).
//! `--respawn` restarts the script's clock and `--goto` on every spawn;
//! `--fire-after SECS` holds attack back until another player has been
//! within 400 units for SECS; `--aim-z Z` aims Z over the nearest
//! player's origin in place of its stance's 40, 28 or 6.
//! `--goto`
//! sends `setviewpos X Y Z` half a second after the first spawn (the server
//! must run `devmap` for cheats). `--scores` sends `score` every 2 s from
//! the spawn, as the cgame's `UpdateScores` does while the scoreboard is
//! up (cod4-hud.md 7). Each packet carries three usercmds
//! 16 ms apart, the first on the long branch with every field, the others
//! the fewest bits off it; every cmd echoes `ps.weapon` and
//! `ps.offHandIndex`. `--secs` counts from the first snapshot.
//!
//! The stats packets carry a fresh profile's block (`net::stats`). The first
//! reliable command after the gamestate is `cp`, the pure checksums of the
//! iwds in `--iwds` that the server lists, or the bare key without it; both
//! pass a pure server's check (cod4-handshake.md, "The pure check").
//! `--no-cp` leaves it out. `--answers "SECS team|class RESPONSE|..."`
//! answers the team or class menu at each time past the first snapshot in
//! place of `--team`'s two; answers at one time go in one packet.
//! `--cmd "SECS TEXT"` (repeatable) sends TEXT as a client command SECS
//! past the first snapshot (`say`, `kill`, `disconnect`). `--snaps N`
//! sends a userinfo with `snaps N` on the first snapshot (the connect's
//! says 20). Every change of `pm_type` is logged.
//!
//! `--download` fetches files over `svc_download` on the first gamestate,
//! sending no usercmds meanwhile, as 1.7's `CL_InitDownloads` does: `auto`
//! asks for every file `net::download::missing_files` lists (nothing held),
//! a list asks for those names as given. Each block is answered with
//! `nextdl`, each finished file logged with its size and CRC-32 (written
//! under `--dl-dir` when given), a refusal or redirect logged and passed
//! over; `donedl` follows the last (cod4-handshake.md 11). Run it only against a server on loopback inside
//! `unshare -rnm` (recipe in cod4-handshake.md, section 7).

use anyhow::{Context, Result, bail};
use std::io::Write;
use std::net::{SocketAddr, UdpSocket};
use std::time::{Duration, Instant};
use vcod_cod4::net::clmsg::{UserCmd, buttons};
use vcod_cod4::net::conn::{Conn, Cp, Join, angle2short};
use vcod_cod4::net::delta::{EntityState, PlayerState};
use vcod_cod4::net::download::{DownloadMsg, missing_files};
use vcod_cod4::net::events::event_name;
use vcod_cod4::net::parse::{Gamestate, Op};
use vcod_cod4::net::pure::block_checksum_key32;

/// `CS_SCRIPT_MENUS`: `mr` names a menu by its index past this slot
/// (INFERRED).
const CS_MENUS: u16 = 1970;
const PM_NORMAL: u32 = 0;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Script {
    Walk,
    Idle,
    Follow,
    Shoot,
    Fly,
    Mantle,
    Frag,
    /// `frag`'s throw on the secondary offhand's key (0x8000).
    Flash,
    Melee,
    /// One melee press at 5 s with 1.7's charge pick
    /// (`pmove::charge::client_pick`) in the cmd.
    Lunge,
    Auto,
    /// Mans a turret ahead and lets it go (`--face`).
    Saw,
    /// Throws back a live frag under its cursor hint.
    Throwback,
    /// `shoot` while strafing, leaning, crouching and swinging the view.
    Sway,
    /// One state a life (`--respawn`), the shot due 2.5 s past the spawn.
    Poses,
    /// The prone key held from 1 s.
    Prone,
    /// `--weapon` up, one round along `--face` and `--pitch`, one at the
    /// nearest player.
    Rocket,
    /// Walks along `--face` into what is ahead, strafes, backs off and
    /// walks in again.
    Push,
}

/// The fixed aims `saw` and `rocket` read: `--face`, and `rocket`'s
/// `--weapon` byte and first round's `--pitch`.
#[derive(Clone, Copy, Default)]
struct Aim {
    face: f32,
    weapon: u8,
    pitch: f32,
}

/// `+activate`: the use key's button.
const USE: u32 = 0x8;
/// `+reload` (`BUTTON_RELOAD`).
const RELOAD: u32 = 0x10;
/// The kill capture's class's M16 and the Beretta `weapnext` selects.
const M16: u32 = 27;
const BERETTA: u32 = 11;

fn ps_f32(ps: &PlayerState, name: &str) -> f32 {
    ps.get_f32(name).unwrap_or(0.0)
}

fn ps_u32(ps: &PlayerState, name: &str) -> u32 {
    ps.get(name).unwrap_or(0)
}

/// The script's cmd at `s` seconds after the spawn (or the first snapshot
/// before one), its view `view` (pitch, yaw) over the playerstate's
/// `delta_angles`.
fn script_cmd(
    script: Script,
    s: Option<f32>,
    view: [f32; 2],
    ps: &PlayerState,
    target: Option<[f32; 2]>,
    aim: Aim,
    life: u32,
) -> UserCmd {
    let face = aim.face;
    let mut c = UserCmd {
        weapon: ps_u32(ps, "weapon") as u8,
        offhand: ps_u32(ps, "offHandIndex") as u8,
        ..UserCmd::default()
    };
    let mut yaw = view[1];
    let mut pitch = view[0];
    let within = |a: f32, b: f32| s.is_some_and(|s| s >= a && s < b);
    match script {
        Script::Idle => {}
        Script::Poses => match life % 7 {
            // Standing, crouched, prone; firing taps 300 ms apart from
            // 1 s; one tap at 1 s and the reload key at 2 s; a jump at
            // 2.4 s; the Beretta's switch at 2.2 s.
            1 => c.buttons |= buttons::CROUCH,
            2 => c.buttons |= buttons::PRONE,
            3 if s.is_some_and(|s| s >= 1.0 && (s - 1.0) % 0.3 < 0.05) => {
                c.buttons |= buttons::ATTACK
            }
            4 if within(1.0, 1.05) => c.buttons |= buttons::ATTACK,
            4 if within(2.0, 2.1) => c.buttons |= RELOAD,
            5 if within(2.4, 2.5) => c.buttons |= buttons::JUMP,
            6 if within(2.2, 99.0) && c.weapon as u32 == M16 => c.weapon = BERETTA as u8,
            _ => {}
        },
        Script::Prone => {
            if within(1.0, 999.0) {
                c.buttons |= buttons::PRONE;
            }
        }
        Script::Rocket => {
            if within(1.5, 999.0) {
                c.weapon = aim.weapon;
            }
            if s.is_some_and(|s| s < 9.0) {
                [pitch, yaw] = [aim.pitch, face];
            } else if let Some(t) = target {
                [pitch, yaw] = t;
            }
            if within(4.5, 4.55) || within(10.0, 10.05) {
                c.buttons |= buttons::ATTACK;
            }
        }
        Script::Follow => {
            if s.is_some_and(|s| s % 3.0 < 0.06) {
                c.buttons |= buttons::ATTACK;
            }
        }
        Script::Frag | Script::Flash | Script::Melee => {
            if let (Some(t), Some(s)) = (target, s) {
                [pitch, yaw] = t;
                if script == Script::Frag && (5.0..5.8).contains(&s) {
                    c.buttons |= buttons::FRAG;
                }
                if script == Script::Flash && (5.0..5.8).contains(&s) {
                    c.buttons |= buttons::SMOKE;
                }
                if script == Script::Melee && s > 5.0 && s % 1.0 < 0.06 {
                    c.buttons |= buttons::MELEE;
                }
            }
        }
        Script::Lunge => {
            if let (Some(t), Some(s)) = (target, s) {
                [pitch, yaw] = t;
                if (5.0..5.1).contains(&s) {
                    c.buttons |= buttons::MELEE;
                }
            }
        }
        Script::Saw => {
            // Faces the gun, taps use at 2 s, fires 3..5 s, swings the view
            // 70 right of the gun and 70 left (past its 45 arcs) 5..8 s,
            // taps use at 8.5 s to let go, fires its own weapon 9.5..10.5 s.
            pitch = 0.0;
            yaw = face;
            if let Some(s) = s {
                if (5.0..6.5).contains(&s) {
                    yaw = face - 70.0 * ((s - 5.0) / 1.5);
                } else if (6.5..8.0).contains(&s) {
                    yaw = face - 70.0 + 140.0 * ((s - 6.5) / 1.5);
                } else if (8.0..8.5).contains(&s) {
                    yaw = face + 70.0 * (1.0 - (s - 8.0) / 0.5);
                }
            }
            if within(2.0, 2.1) || within(8.5, 8.6) {
                c.buttons |= USE;
            }
            if within(3.0, 5.0) || within(9.5, 10.5) {
                c.buttons |= buttons::ATTACK;
            }
        }
        Script::Throwback => {
            if let Some(t) = target {
                [pitch, yaw] = t;
            }
            // The frag key while a live grenade is under the hint, held
            // until the pickup names its owner.
            let left = ps_u32(ps, "throwBackGrenadeTimeLeft") as i32;
            let owner = ps_u32(ps, "throwBackGrenadeOwner");
            if left > 0 && owner == 1023 {
                c.buttons |= buttons::FRAG;
            }
        }
        Script::Auto => {
            if within(7.0, 9.0) || within(11.0, 13.0) {
                c.buttons |= buttons::ATTACK;
            }
        }
        Script::Shoot => {
            if let (Some(t), Some(s)) = (target, s) {
                [pitch, yaw] = t;
                c.buttons |= buttons::ADS;
                // Sights up for a second, then taps 250 ms apart.
                if s % 0.25 < 0.05 && s > 1.0 {
                    c.buttons |= buttons::ATTACK;
                }
            }
        }
        Script::Sway => {
            // From 6 s past the spawn (the prematch freeze over), each
            // phase taps attack 250 ms apart at the nearest player:
            // sighted strafing 6..9, sighted leaning left 9..10.5 and
            // right 10.5..12, the sight down 12..13, from the hip
            // strafing 13..16, sighted swinging the view 16..18, then
            // crouched, sighted and strafing 18..21.
            if let (Some(t), Some(s)) = (target, s) {
                [pitch, yaw] = t;
                let phase = |a: f32, b: f32| (a..b).contains(&s);
                let strafe = if (s % 1.0) < 0.5 { 127 } else { -127 };
                if phase(6.0, 12.0) || phase(16.0, 21.0) {
                    c.buttons |= buttons::ADS;
                }
                if phase(6.0, 9.0) || phase(13.0, 16.0) || phase(18.0, 21.0) {
                    c.rightmove = strafe;
                }
                if phase(9.0, 10.5) {
                    c.buttons |= 0x40;
                }
                if phase(10.5, 12.0) {
                    c.buttons |= 0x80;
                }
                if phase(16.0, 18.0) {
                    yaw += 3.0 * (s * std::f32::consts::TAU).sin();
                    pitch += 1.5 * (s * 1.7 * std::f32::consts::TAU).sin();
                }
                if phase(18.0, 21.0) {
                    c.buttons |= buttons::CROUCH;
                }
                let firing = phase(6.5, 12.0) || phase(13.0, 21.0);
                if firing && s % 0.25 < 0.05 {
                    c.buttons |= buttons::ATTACK;
                }
            }
        }
        Script::Fly => {
            // A free spectator from its first snapshot (the team menu's
            // `mr` goes at 1 s): forward, strafe, up and down on the lean
            // keys, forward pitched down, back-left while turning, the
            // crouch and prone keys, then forward into whatever is ahead.
            if within(3.0, 5.0) || within(9.0, 11.0) || within(15.0, 19.0) {
                c.forwardmove = 127;
            }
            if within(5.0, 6.5) {
                c.rightmove = 127;
            }
            if within(6.5, 7.5) {
                c.buttons |= 0x80;
            }
            if within(7.5, 8.5) {
                c.buttons |= 0x40;
            }
            if within(9.0, 11.0) {
                pitch = 45.0;
            }
            if within(11.0, 13.0) {
                c.forwardmove = -127;
                c.rightmove = -127;
            }
            if let Some(s) = s.filter(|&s| s >= 11.0) {
                yaw += 90.0 * ((s - 11.0) / 2.0).min(1.0);
            }
            if within(13.0, 13.5) {
                c.buttons |= buttons::CROUCH;
            }
            if within(13.5, 14.0) {
                c.buttons |= buttons::PRONE;
            }
            if within(14.0, 14.5) {
                c.buttons |= buttons::JUMP;
            }
        }
        Script::Push => {
            pitch = 0.0;
            yaw = face;
            if within(3.0, 6.0) || within(8.5, 10.0) {
                c.forwardmove = 127;
            }
            if within(6.0, 7.0) {
                c.rightmove = 127;
            }
            if within(7.0, 8.5) {
                c.forwardmove = -127;
            }
        }
        Script::Mantle => {
            pitch = 0.0;
            yaw = 0.0;
            if let Some(s) = s {
                // Over the first mantle the view swings past the yaw cap
                // and back.
                if (2.1..2.9).contains(&s) {
                    yaw = 120.0 * (1.0 - ((s - 2.5) / 0.4).abs());
                }
                if s >= 5.0 {
                    yaw = 180.0;
                }
            }
            if within(1.5, 3.3) || within(5.5, 8.0) {
                c.forwardmove = 127;
            }
            if within(2.0, 2.25) || within(6.0, 6.25) {
                c.buttons |= buttons::JUMP;
            }
            // Then the leans: left, back, right, back.
            if within(9.0, 10.0) {
                c.buttons |= 0x40;
            }
            if within(10.5, 11.5) {
                c.buttons |= 0x80;
            }
        }
        Script::Walk => {
            if within(0.5, 3.0) || within(22.0, 25.0) {
                c.forwardmove = 127;
            }
            if within(3.0, 4.5) {
                c.rightmove = 127;
            }
            if within(5.0, 5.25) {
                c.buttons |= buttons::JUMP;
            }
            if within(6.5, 8.5) {
                c.buttons |= buttons::CROUCH;
            }
            if within(9.0, 12.0) {
                c.buttons |= buttons::PRONE;
            }
            // Taps 500 ms apart, past any stock rifle's fire time.
            if within(14.0, 17.0) && s.is_some_and(|s| (s - 14.0) % 0.5 < 0.05) {
                c.buttons |= buttons::ATTACK;
            }
            if within(18.0, 18.8) {
                c.buttons |= buttons::FRAG;
            }
            if let Some(s) = s.filter(|&s| s >= 22.0) {
                yaw += 45.0 * ((s - 22.0) / 3.0).min(1.0);
            }
        }
    }
    let delta = |i: usize| angle2short(ps_f32(ps, &format!("delta_angles[{i}]")));
    c.angles = [
        angle2short(pitch).wrapping_sub(delta(0)),
        angle2short(yaw).wrapping_sub(delta(1)),
        0u16.wrapping_sub(delta(2)),
    ];
    c
}

/// The distance to the nearest other live player in the snapshot.
fn nearest_dist(ps: &PlayerState, ents: &[EntityState]) -> Option<f32> {
    let me = ps_u32(ps, "clientNum");
    let at = [
        ps_f32(ps, "origin[0]"),
        ps_f32(ps, "origin[1]"),
        ps_f32(ps, "origin[2]"),
    ];
    ents.iter()
        .filter(|e| e.etype() == 1 && e.number() != me)
        .map(|e| {
            let o = e.origin();
            ((o[0] - at[0]).powi(2) + (o[1] - at[1]).powi(2) + (o[2] - at[2]).powi(2)).sqrt()
        })
        .min_by(f32::total_cmp)
}

/// The view (pitch, yaw) from our eye to the chest of the nearest other
/// player in the snapshot.
fn aim_at(ps: &PlayerState, ents: &[EntityState], aim_z: Option<f32>) -> Option<[f32; 2]> {
    let me = ps_u32(ps, "clientNum");
    let eye = [
        ps_f32(ps, "origin[0]"),
        ps_f32(ps, "origin[1]"),
        ps_f32(ps, "origin[2]") + ps_f32(ps, "viewHeightCurrent"),
    ];
    let d = ents
        .iter()
        .filter(|e| e.etype() == 1 && e.number() != me)
        .map(|e| {
            let o = e.origin();
            // `EF_CROUCHING` 0x4, `EF_PRONE` 0x8.
            let flags = EntityState::table(e.etype())
                .iter()
                .find(|f| f.name == "lerp.eFlags")
                .map_or(0, |f| e.0[f.word()]);
            let z = aim_z.unwrap_or(if flags & 0x8 != 0 {
                6.0
            } else if flags & 0x4 != 0 {
                28.0
            } else {
                40.0
            });
            [o[0] - eye[0], o[1] - eye[1], o[2] + z - eye[2]]
        })
        .min_by(|a, b| {
            let n = |v: &[f32; 3]| v[0] * v[0] + v[1] * v[1] + v[2] * v[2];
            n(a).total_cmp(&n(b))
        })?;
    let flat = (d[0] * d[0] + d[1] * d[1]).sqrt();
    Some([
        -d[2].atan2(flat).to_degrees(),
        d[1].atan2(d[0]).to_degrees(),
    ])
}

/// 1.7's charge pick over the snapshot's other players, each box from
/// its origin to a standing helmet's 70 (`pmove::charge::client_pick`),
/// the eye at the playerstate's origin and view height, the view the
/// cmd's (over `delta_angles`), `cg_fov` 65 on 4:3.
fn charge_at(ps: &PlayerState, ents: &[EntityState], c: &UserCmd) -> (u16, u8) {
    use vcod_cod4::pmove::charge::{AimBody, AimEye, client_pick};
    let v = |n: &str| glam::Vec3::from_array([0, 1, 2].map(|k| ps_f32(ps, &format!("{n}[{k}]"))));
    let own = v("origin");
    let delta = v("delta_angles");
    let short = |s: u16| f32::from(s) * (360.0 / 65536.0);
    let t = 32.5f32.to_radians().tan();
    let eye = AimEye {
        origin: own + glam::Vec3::Z * ps_f32(ps, "viewHeightCurrent"),
        angles: glam::Vec3::new(
            short(c.angles[0]) + delta.x,
            short(c.angles[1]) + delta.y,
            0.0,
        ),
        tan_half_fov: [t, t * 0.75],
        fov_scale_inv: 1.0,
    };
    let me = ps_u32(ps, "clientNum");
    let bodies: Vec<AimBody> = ents
        .iter()
        .filter(|e| e.etype() == 1 && e.number() != me)
        .map(|e| {
            let origin = glam::Vec3::from_array(e.origin());
            AimBody {
                origin,
                yaw: 0.0,
                aim: origin,
                top: 70.0,
            }
        })
        .collect();
    let (yaw, dist) = client_pick(c.buttons, ps_u32(ps, "pm_flags"), own, &eye, &bodies);
    eprintln!("lunge pick yaw {yaw} dist {dist}");
    (angle2short(yaw), dist)
}

fn menu_index(gs: &Gamestate, name: &str) -> Result<usize> {
    (0..32)
        .find(|&i| gs.configstring(CS_MENUS + i as u16) == name.as_bytes())
        .with_context(|| format!("no menu {name}"))
}

/// The probe's download queue.
#[derive(Default)]
struct Fetch {
    queue: Vec<String>,
    current: Option<(String, i32, Vec<u8>)>,
    started: bool,
    /// `donedl` went: no usercmds until the gamestate it brings.
    awaiting_gamestate: bool,
    dir: Option<std::path::PathBuf>,
}

impl Fetch {
    fn busy(&self) -> bool {
        self.current.is_some() || !self.queue.is_empty() || self.awaiting_gamestate
    }

    /// The next `download`, or `donedl` past the last.
    fn next(&mut self, conn: &mut Conn) {
        if self.queue.is_empty() {
            if self.started {
                self.started = false;
                self.awaiting_gamestate = true;
                eprintln!("command {}: donedl", conn.done_downloading());
            }
            return;
        }
        let f = self.queue.remove(0);
        self.started = true;
        eprintln!(
            "command {}: download {f}",
            conn.command(format!("download {f}"))
        );
        self.current = Some((f, 0, Vec::new()));
    }

    fn on_message(&mut self, conn: &mut Conn, msg: &DownloadMsg) -> Result<()> {
        eprintln!(
            "download message {}",
            match msg {
                DownloadMsg::Block { block, size, data } =>
                    format!("block {block} size {size:?} len {}", data.len()),
                m => format!("{m:?}"),
            }
        );
        let Some((name, due, data)) = self.current.as_mut() else {
            return Ok(());
        };
        match msg {
            DownloadMsg::Block {
                block, data: bytes, ..
            } => {
                if block != due {
                    eprintln!("expected block {due}, got {block}");
                    return Ok(());
                }
                data.extend_from_slice(bytes);
                eprintln!(
                    "command {}: nextdl {block}",
                    conn.command(format!("nextdl {block}"))
                );
                *due += 1;
                if bytes.is_empty() {
                    eprintln!(
                        "downloaded {name}: {} bytes, crc32 {:08x}",
                        data.len(),
                        block_checksum_key32(data, 0)
                    );
                    if let Some(dir) = &self.dir {
                        let path = dir.join(name.replace('/', "_"));
                        std::fs::write(&path, &*data)?;
                    }
                    self.current = None;
                    self.next(conn);
                }
            }
            DownloadMsg::Refused(_) | DownloadMsg::Redirect { .. } => {
                self.current = None;
                self.next(conn);
            }
        }
        Ok(())
    }
}

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let server: SocketAddr = args.first().context("usage: HOST:PORT OUT.txt")?.parse()?;
    let out = args.get(1).context("usage: HOST:PORT OUT.txt")?;
    let opt = |k: &str| {
        args.iter()
            .position(|a| a == k)
            .and_then(|i| args.get(i + 1))
    };
    let secs: u64 = opt("--secs").map_or(Ok(15), |s| s.parse())?;
    let team = opt("--team").cloned();
    let class = opt("--class").map_or("assault_mp,0", String::as_str);
    let script = match opt("--script").map(String::as_str) {
        None | Some("idle") => Script::Idle,
        Some("walk") => Script::Walk,
        Some("follow") => Script::Follow,
        Some("shoot") => Script::Shoot,
        Some("frag") => Script::Frag,
        Some("flash") => Script::Flash,
        Some("melee") => Script::Melee,
        Some("lunge") => Script::Lunge,
        Some("fly") => Script::Fly,
        Some("mantle") => Script::Mantle,
        Some("auto") => Script::Auto,
        Some("saw") => Script::Saw,
        Some("throwback") => Script::Throwback,
        Some("sway") => Script::Sway,
        Some("poses") => Script::Poses,
        Some("prone") => Script::Prone,
        Some("rocket") => Script::Rocket,
        Some("push") => Script::Push,
        Some(s) => bail!("unknown script {s}"),
    };
    let name = opt("--name").map_or("vcodprobe", String::as_str);
    let qport: u16 = opt("--qport").map_or(Ok(0x1234), |s| s.parse())?;
    let cp = match (args.iter().any(|a| a == "--no-cp"), opt("--iwds")) {
        (true, _) => Cp::None,
        (false, Some(dir)) => Cp::Iwds(dir.into()),
        (false, None) => Cp::Bare,
    };
    let goto = opt("--goto").cloned();
    let respawn = args.iter().any(|a| a == "--respawn");
    let aim_z: Option<f32> = opt("--aim-z").map(|s| s.parse()).transpose()?;
    let fire_after: Option<f32> = opt("--fire-after").map(|s| s.parse()).transpose()?;
    let aim = Aim {
        face: opt("--face").map_or(Ok(0.0), |s| s.parse())?,
        weapon: opt("--weapon").map_or(Ok(0), |s| s.parse())?,
        pitch: opt("--pitch").map_or(Ok(-15.0), |s| s.parse())?,
    };
    let scores = args.iter().any(|a| a == "--scores");
    let answers = opt("--answers").cloned();
    let snaps = opt("--snaps").cloned();
    // `--cmd "SECS TEXT"`, any number: TEXT as a client command SECS past
    // the first snapshot.
    let mut extra: Vec<(f32, String)> = Vec::new();
    for (i, a) in args.iter().enumerate() {
        if a == "--cmd" {
            let v = args.get(i + 1).context("--cmd takes \"SECS TEXT\"")?;
            let (at, text) = v.split_once(' ').context("--cmd takes \"SECS TEXT\"")?;
            extra.push((at.parse()?, text.to_string()));
        }
    }
    let mut download = opt("--download").cloned();
    let mut fetch = Fetch {
        dir: opt("--dl-dir").map(Into::into),
        ..Fetch::default()
    };
    let mut scores_asked: Option<f32> = None;
    let mut auth = Vec::new();
    if let Some(ip) = opt("--auth") {
        for port in [20800, 20810] {
            auth.push(UdpSocket::bind((ip.as_str(), port))?);
        }
    }
    // Every datagram both ways, `<ms> <dir> <peer> <hex>`.
    let mut file = std::fs::File::create(out)?;
    let t0 = Instant::now();
    let tap = Box::new(move |tag: &str, peer: SocketAddr, data: &[u8]| {
        let hex: String = data.iter().map(|b| format!("{b:02x}")).collect();
        writeln!(file, "{} {tag} {peer} {hex}", t0.elapsed().as_millis())?;
        Ok(())
    });
    let mut conn = Conn::join(
        server,
        Join {
            name: name.into(),
            password: String::new(),
            qport,
            cp,
            auth,
            tap: Some(tap),
            www: false,
            cancel: None,
            voice: false,
            notes: None,
            stats: None,
        },
    )?;

    let mut first_snap: Option<Instant> = None;
    let mut last_snap_at = Instant::now();
    let mut spawned: Option<Instant> = None;
    let mut view = [0f32; 2];
    let mut queued: Vec<(f32, String)> = extra;
    let (mut menus_queued, mut said, mut went) = (false, false, false);
    let mut last_events = [0u32; 4];
    let mut last_pm_type = None;
    let mut life = 0u32;
    let mut near_since: Option<Instant> = None;
    // Our own client number, off the first spawn: a killcam's playerstate
    // is the killer's.
    let mut me: Option<u32> = None;
    let mut next_report = 0u64;
    let hard_stop = Instant::now() + Duration::from_secs(secs + 30);
    loop {
        if Instant::now() > hard_stop {
            bail!("no snapshots");
        }
        if first_snap.is_some_and(|t| t.elapsed() > Duration::from_secs(secs)) {
            break;
        }
        // Commands due on the first snapshot's clock (team, class) or the
        // spawn's (`say`).
        if let (Some(t), Some(gs)) = (first_snap, conn.client.gamestate.as_ref()) {
            let sid = gs.systeminfo("sv_serverid").context("sv_serverid")?;
            if !menus_queued && let Some(list) = &answers {
                menus_queued = true;
                let tm = menu_index(gs, "team_marinesopfor")?;
                let cm = menu_index(gs, "changeclass")?;
                for a in list.split('|') {
                    let mut w = a.split_whitespace();
                    let (Some(at), Some(which), Some(r)) = (w.next(), w.next(), w.next()) else {
                        bail!("--answers takes `SECS team|class RESPONSE`, got {a:?}");
                    };
                    let menu = if which == "team" { tm } else { cm };
                    queued.push((at.parse()?, format!("mr {sid} {menu} {r}")));
                }
            }
            if !menus_queued && let Some(team) = &team {
                menus_queued = true;
                let tm = menu_index(gs, "team_marinesopfor")?;
                queued.push((1.0, format!("mr {sid} {tm} {team}")));
                if team != "spectator" {
                    let cm = menu_index(gs, "changeclass")?;
                    queued.push((2.5, format!("mr {sid} {cm} {class}")));
                }
            }
            let since = t.elapsed().as_secs_f32();
            let spawn_s = spawned.map(|s| s.elapsed().as_secs_f32());
            for (_, text) in queued.extract_if(.., |(at, _)| since >= *at) {
                eprintln!("command {}: {text}", conn.command(text.clone()));
            }
            if script == Script::Walk && spawn_s.is_some_and(|s| s >= 21.0) && !said {
                said = true;
                eprintln!("command {}: say", conn.command("say \"vcod probe here\""));
            }
            if scores
                && let Some(s) = spawn_s
                && scores_asked.is_none_or(|t| s >= t + 2.0)
            {
                scores_asked = Some(s);
                eprintln!("command {}: score", conn.command("score"));
            }
            if let Some(g) = goto.as_ref().filter(|_| !went)
                && spawn_s.is_some_and(|s| s >= 0.5)
                && team.as_deref() != Some("spectator")
            {
                went = true;
                let n = conn.command(format!("setviewpos {g}"));
                eprintln!("command {n}: setviewpos {g}");
            }
        }
        let mut cmds = Vec::new();
        if conn.client.gamestate.is_some() && !fetch.busy() {
            let snap = conn.client.snapshots.last();
            let ps = snap.map(|s| s.ps.clone()).unwrap_or_default();
            let now_time = snap.map_or(0, |s| {
                s.server_time + last_snap_at.elapsed().as_millis() as i32
            });
            let spawn_s = spawned.map(|s| s.elapsed().as_secs_f32());
            let target = snap.and_then(|s| aim_at(&s.ps, &s.entities, aim_z));
            let mut last_time = conn.last_cmd().server_time;
            for k in [2, 1, 0] {
                let t = now_time - 16 * k;
                if t <= last_time && !cmds.is_empty() {
                    continue;
                }
                let mut c = script_cmd(
                    script,
                    spawn_s.map(|s| s - 0.016 * k as f32),
                    view,
                    &ps,
                    target,
                    aim,
                    life,
                );
                if fire_after
                    .is_some_and(|f| near_since.is_none_or(|t| t.elapsed().as_secs_f32() < f))
                {
                    c.buttons &= !buttons::ATTACK;
                }
                if c.buttons & buttons::MELEE != 0 {
                    (c.melee_charge_yaw, c.melee_charge_dist) =
                        snap.map_or((0, 0), |s| charge_at(&s.ps, &s.entities, &c));
                }
                c.server_time = t.max(last_time + 1);
                last_time = c.server_time;
                cmds.push(c);
            }
        }
        conn.send(&cmds)?;
        for got in conn.poll(Instant::now() + Duration::from_millis(50))? {
            let s = got.sequence;
            for op in &got.parsed.ops {
                match op {
                    Op::Download(m) => fetch.on_message(&mut conn, m)?,
                    Op::Gamestate => {
                        eprintln!("gamestate at message {s}");
                        fetch.awaiting_gamestate = false;
                        if let Some(d) = download.take() {
                            let gs = conn.client.gamestate.as_ref().context("gamestate")?;
                            let si = String::from_utf8_lossy(gs.configstring(1)).into_owned();
                            fetch.queue = match d.as_str() {
                                "auto" => missing_files(&si, |_| false, |_| None)
                                    .into_iter()
                                    .map(|w| w.remote)
                                    .collect(),
                                list => list.split(',').map(str::to_string).collect(),
                            };
                            eprintln!("downloads {:?}", fetch.queue);
                            fetch.next(&mut conn);
                        }
                        if let Some(cp) = conn.commands().iter().position(|c| c.starts_with(b"cp "))
                        {
                            let text = String::from_utf8_lossy(&conn.commands()[cp]);
                            eprintln!("command {}: {text}", cp + 1);
                        }
                    }
                    Op::ServerCommand { sequence, text } => {
                        eprintln!(
                            "server command {sequence}: {}",
                            String::from_utf8_lossy(text)
                        )
                    }
                    Op::Snapshot(i) => {
                        last_snap_at = Instant::now();
                        if first_snap.is_none() {
                            eprintln!("first snapshot at message {s}");
                            first_snap = Some(Instant::now());
                            if let Some(n) = &snaps {
                                conn.set_userinfo("snaps", n);
                            }
                        }
                        let ps = &conn.client.snapshots[*i].ps;
                        let pm_type = ps_u32(ps, "pm_type");
                        if last_pm_type != Some(pm_type) {
                            last_pm_type = Some(pm_type);
                            eprintln!("pm_type {pm_type} at message {s}");
                            if respawn && pm_type != PM_NORMAL && spawned.is_some() {
                                spawned = None;
                                went = false;
                                life += 1;
                                eprintln!("life {life} at message {s}");
                            }
                        }
                        let near = nearest_dist(ps, &conn.client.snapshots[*i].entities)
                            .is_some_and(|d| d < 400.0);
                        near_since = near.then(|| near_since.unwrap_or_else(Instant::now));
                        for (k, last) in last_events.iter_mut().enumerate() {
                            let ev = ps_u32(ps, &format!("events[{k}]"));
                            if ev != *last && ev != 0 {
                                eprintln!(
                                    "event {ev} {} at message {s}",
                                    event_name(ev).unwrap_or("?")
                                );
                            }
                            *last = ev;
                        }
                        if spawned.is_none()
                            && team.as_deref().is_some_and(|t| t != "spectator")
                            && ps_u32(ps, "pm_type") == PM_NORMAL
                            && me.is_none_or(|n| n == ps_u32(ps, "clientNum"))
                        {
                            me = Some(ps_u32(ps, "clientNum"));
                            eprintln!("spawned at message {s}");
                            spawned = Some(Instant::now());
                            view = [ps_f32(ps, "viewangles[0]"), ps_f32(ps, "viewangles[1]")];
                        }
                        if team.as_deref() == Some("spectator") && spawned.is_none() {
                            spawned = first_snap;
                        }
                    }
                    _ => {}
                }
            }
        }
        if let Some(snap) = conn.client.snapshots.last()
            && t0.elapsed().as_secs() >= next_report
        {
            next_report = t0.elapsed().as_secs() + 1;
            let ps = &snap.ps;
            let types: Vec<u32> = snap.entities.iter().map(|e| e.etype()).collect();
            eprintln!(
                "t={:.1} pm_type={} origin=({:.0} {:.0} {:.0}) pm_flags={:#x} eFlags={:#x} weapon={} clientNum={} shock={}/{}/{} hint={}/{}/{} lock={}/{} tb={}/{} ents={types:?}",
                t0.elapsed().as_secs_f32(),
                ps_u32(ps, "pm_type"),
                ps_f32(ps, "origin[0]"),
                ps_f32(ps, "origin[1]"),
                ps_f32(ps, "origin[2]"),
                ps_u32(ps, "pm_flags"),
                ps_u32(ps, "eFlags"),
                ps_u32(ps, "weapon"),
                ps_u32(ps, "clientNum"),
                ps_u32(ps, "shellshockIndex"),
                ps_u32(ps, "shellshockTime"),
                ps_u32(ps, "shellshockDuration"),
                ps_u32(ps, "cursorHint"),
                ps_u32(ps, "cursorHintString"),
                ps_u32(ps, "cursorHintEntIndex"),
                ps_u32(ps, "viewlocked"),
                ps_u32(ps, "viewlocked_entNum"),
                ps_u32(ps, "throwBackGrenadeTimeLeft"),
                ps_u32(ps, "throwBackGrenadeOwner"),
            );
        }
    }
    eprintln!("{} snapshots", conn.client.snapshots.len());
    Ok(())
}
