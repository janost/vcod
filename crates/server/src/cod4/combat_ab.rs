//! `cod4_combat_ab`: the kill capture (`play-1.7-kill-target.txt`,
//! `-shooter.txt`, cod4-netfields.md 10) on ours. Its two `cod4_probe`
//! clients join [`Cod4Server`] in process on retail's level clock as
//! `wire_ab`'s do (`target` slot 0 on allies, `shooter` slot 1 on axis,
//! each on its retail spawn spot, each sending its `setviewpos` when its
//! twin did) and from their spawns send their captures' own cmds. Then the
//! shooter's weapon playerstate, the target's damage feedback, health and
//! death, the round's flesh events, the corpse, the dropped rifle, the
//! obituary and the killcam window are compared with retail's, snapshot
//! time by snapshot time (docs/research/cod4-combat.md 8). What still
//! differs is [`GAPS`]. Needs `COD4_DIR`.
//!
//! `COMBAT_REPORT=1` prints every row.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use vcod_cod4::net::parse::Snapshot;

use super::wire_ab::{Probe, Side, replay_schedule, retail, side};
use super::*;

/// The run's end: the capture's last snapshot (63350) and a frame.
const RUN_MS: i32 = 63_400;
/// The two spawns the replays start after.
const TARGET_SPAWN_MS: i32 = 45_950;
const SHOOTER_SPAWN_MS: i32 = 46_900;

/// The kill run's `srand` seed (`G_InitGame`, 0x80b56ee), found off its
/// two challenges as `wire_ab::WALK_SEED` was (cod4-combat.md 10.9).
const KILL_SEED: u32 = 1_807_258_603;

/// Each tolerated row (a prefix of the report line) and why, by
/// cod4-combat.md's section.
const GAPS: &[(&str, &str)] = &[];

pub(super) fn kill(name: &str) -> Vec<vcod_cod4::net::capture::Line> {
    retail(&format!("kill-{name}"))
}

/// The two clients against `Cod4Server` to [`RUN_MS`].
fn ours(install: &vcod_cod4::install::Install, retail_cs0: &str) -> (Vec<Probe>, Vec<String>) {
    ours_with(install, retail_cs0, None, &[])
}

/// [`ours`], the shooter's cmds `shooter`'s when given.
fn ours_with(
    install: &vcod_cod4::install::Install,
    retail_cs0: &str,
    shooter: Option<VecDeque<(i32, Vec<UserCmd>)>>,
    target_extra: &[(i32, &'static str)],
) -> (Vec<Probe>, Vec<String>) {
    let (probes, aborts, _) = ours_hooked(
        install,
        retail_cs0,
        shooter,
        target_extra,
        &mut |_, _, _| {},
    );
    (probes, aborts)
}

/// A hook run before each tick's packets: the server, the probes and the
/// server time.
pub(super) type Hook<'a> = dyn FnMut(&mut Cod4Server, &mut [Probe], i32) + 'a;

/// [`ours_with`] with a [`Hook`]; the server comes back too.
pub(super) fn ours_hooked(
    install: &vcod_cod4::install::Install,
    retail_cs0: &str,
    shooter: Option<VecDeque<(i32, Vec<UserCmd>)>>,
    target_extra: &[(i32, &'static str)],
    hook: &mut Hook,
) -> (Vec<Probe>, Vec<String>, Cod4Server) {
    let classes = ["mr {sid} 15 assault_mp,0"; 2];
    ours_set_up(
        install,
        retail_cs0,
        &[],
        classes,
        shooter,
        target_extra,
        hook,
    )
}

/// [`ours_hooked`] with `cvars` set over the capture's serverinfo and each
/// side's class menu answer (target, shooter).
pub(super) fn ours_set_up(
    install: &vcod_cod4::install::Install,
    retail_cs0: &str,
    cvars: &[(&str, &str)],
    classes: [&'static str; 2],
    shooter: Option<VecDeque<(i32, Vec<UserCmd>)>>,
    target_extra: &[(i32, &'static str)],
    hook: &mut Hook,
) -> (Vec<Probe>, Vec<String>, Cod4Server) {
    // Past the capture's end when a request falls later.
    let run_ms = target_extra
        .iter()
        .map(|a| 43_350 + a.0 + 500)
        .max()
        .unwrap_or(0)
        .max(RUN_MS);
    let mut sv = Cod4Server::new(
        Cod4Config {
            map: "mp_crash".into(),
            gametype: "war".into(),
            hostname: "CoD4Host".into(),
            max_clients: 8,
        },
        7,
    );
    for (k, v) in vcod_common::net::connectionless::info_pairs(retail_cs0) {
        if !matches!(k, "protocol" | "shortversion") {
            sv.set_cvar(k, v);
        }
    }
    // `+devmap`: cheats on, so `setviewpos` runs.
    sv.set_cvar("sv_cheats", "1");
    for (k, v) in cvars {
        sv.set_cvar(k, v);
    }
    sv.pin_game_seed(KILL_SEED);
    sv.load(Some(install)).unwrap();
    let pin =
        |o: [u32; 3], yaw: u32| Some((o.map(f32::from_bits), [0.0, f32::from_bits(yaw), 0.0]));
    let mut pins = vec![VecDeque::new(); crate::server::MAX_CLIENTS];
    pins[0].push_back(pin([0x442e_4000, 0xc4fe_c000, 0x4308_2af3], 0x42b4_0000));
    pins[1].push_back(pin([0xc454_8000, 0x4509_c000, 0x437e_2000], 0xc2b3_fd30));
    sv.game.level.as_mut().unwrap().rt.host.spawn_pins =
        Some(std::rc::Rc::new(std::cell::RefCell::new(pins)));
    let mut target_answers = vec![
        (1050, "mr {sid} 8 allies"),
        (2550, classes[0]),
        (3050, "setviewpos 207 2291 296"),
    ];
    target_answers.extend_from_slice(target_extra);
    target_answers.sort_by_key(|a| a.0);
    let mut probes = vec![
        Probe::new("target", 0, 28961, 43350, &target_answers),
        Probe::new(
            "shooter",
            1,
            28962,
            44300,
            &[
                (1050, "mr {sid} 8 axis"),
                (2550, classes[1]),
                (3050, "setviewpos 123 2110 300"),
            ],
        ),
    ];
    probes[0].replay = replay_schedule(&kill("target"), TARGET_SPAWN_MS);
    probes[0].replay_from = TARGET_SPAWN_MS;
    probes[1].replay =
        shooter.unwrap_or_else(|| replay_schedule(&kill("shooter"), SHOOTER_SPAWN_MS));
    probes[1].replay_from = SHOOTER_SPAWN_MS;
    let mut now = Instant::now();
    while sv.core.sv_time_ms < run_ms {
        let svs = sv.core.sv_time_ms;
        for p in probes.iter_mut() {
            if !p.joined && svs >= p.connect_at {
                p.join(&mut sv, now);
            }
        }
        hook(&mut sv, &mut probes, svs);
        for p in probes.iter_mut().filter(|p| p.joined) {
            let pkt = p.packet(svs);
            p.log("c2s", svs, &pkt);
            sv.handle_packet(p.addr, &pkt, now);
        }
        now += Duration::from_millis(FRAME_MS as u64);
        sv.tick_frames(now, 1);
        let svs = sv.core.sv_time_ms;
        for (to, pkt) in sv.take_outgoing() {
            if let Some(p) = probes.iter_mut().find(|p| p.addr == to) {
                p.receive(svs, &pkt);
            }
        }
    }
    let aborts = sv.game.level.as_ref().unwrap().rt.aborts();
    (probes, aborts, sv)
}

/// The shooter's weapon words, compared from its `setviewpos` on.
const SHOOTER_FIELDS: &[&str] = &[
    "weapon",
    "weaponstate",
    "weaponTime",
    "weaponDelay",
    "weaponShotCount",
    "weapAnim",
    "fWeaponPosFrac",
    "aimSpreadScale",
    "eventSequence",
];

/// The target's damage words up to its death.
const TARGET_FIELDS: &[&str] = &[
    "damageEvent",
    "damageCount",
    "damageYaw",
    "damagePitch",
    "damageTimer",
    "pm_type",
    "pm_time",
    "weapon",
];

fn at(s: &Side, t: i32) -> Option<&Snapshot> {
    s.snaps.iter().find(|f| f.server_time == t)
}

/// The entities of `t`'s snapshot with `etype`, as (number, the field).
fn of_type(f: &Snapshot, etype: u32, field: &str) -> Vec<(u32, u32)> {
    f.entities
        .iter()
        .filter(|e| e.etype() == etype)
        .map(|e| {
            let w = vcod_cod4::net::delta::EntityState::table(etype)
                .iter()
                .find(|t| t.name == field)
                .map_or(0, |t| e.0[t.word()]);
            (e.number(), w)
        })
        .collect()
}

/// Every compared row that differs, as one line each.
fn diffs(rt: &Side, rs: &Side, ot: &Side, os: &Side) -> Vec<String> {
    let mut out = Vec::new();
    let word = |f: &Snapshot, n: &str| f.ps.get(n).unwrap_or(0);
    // The shooter's weapon half, every snapshot from its teleport.
    for r in rs.snaps.iter().filter(|f| f.server_time >= 47_550) {
        let Some(o) = at(os, r.server_time) else {
            out.push(format!("shooter {}: no snapshot of ours", r.server_time));
            continue;
        };
        for n in SHOOTER_FIELDS {
            let (a, b) = (word(o, n), word(r, n));
            if a != b {
                out.push(format!(
                    "shooter {n} {}: ours {a:#x} retail {b:#x}",
                    r.server_time
                ));
            }
        }
    }
    // The target's damage, health and death, to its respawn.
    for r in rt
        .snaps
        .iter()
        .filter(|f| (47_500..=48_400).contains(&f.server_time))
    {
        let Some(o) = at(ot, r.server_time) else {
            out.push(format!("target {}: no snapshot of ours", r.server_time));
            continue;
        };
        for n in TARGET_FIELDS {
            let (a, b) = (word(o, n), word(r, n));
            if a != b {
                out.push(format!(
                    "target {n} {}: ours {a:#x} retail {b:#x}",
                    r.server_time
                ));
            }
        }
        if o.ps.stats[0] != r.ps.stats[0] {
            out.push(format!(
                "target health {}: ours {} retail {}",
                r.server_time, o.ps.stats[0], r.ps.stats[0]
            ));
        }
        // The flesh event (17 + 43), the corpse (2), the item (3) and the
        // obituary (17 + 66) by their fields, numbers aside.
        for (etype, field) in [
            (60, "eventParm"),
            (2, "clientNum"),
            (2, "legsAnim"),
            (2, "torsoAnim"),
            (2, "lerp.eFlags"),
            (2, "lerp.pos.trType"),
            (2, "groundEntityNum"),
            (3, "index"),
            (83, "eventParm"),
        ] {
            let (a, b) = (of_type(o, etype, field), of_type(r, etype, field));
            let vals = |v: &[(u32, u32)]| v.iter().map(|x| x.1).collect::<Vec<_>>();
            if vals(&a) != vals(&b) {
                out.push(format!(
                    "target entity type {etype} {field} {}: ours {a:?} retail {b:?}",
                    r.server_time
                ));
            } else if a.iter().map(|x| x.0).ne(b.iter().map(|x| x.0)) {
                out.push(format!(
                    "target entity type {etype} number {}: ours {a:?} retail {b:?}",
                    r.server_time
                ));
            }
        }
    }
    // The shooter's view of the rounds: `EV_BULLET_HIT` (17 + 41).
    for r in rs
        .snaps
        .iter()
        .filter(|f| (47_900..=48_100).contains(&f.server_time))
    {
        if let Some(o) = at(os, r.server_time) {
            let (a, b) = (of_type(o, 58, "eventParm"), of_type(r, 58, "eventParm"));
            if a.iter().map(|x| x.1).ne(b.iter().map(|x| x.1)) {
                out.push(format!(
                    "shooter bullet hit {}: ours {a:?} retail {b:?}",
                    r.server_time
                ));
            }
        }
    }
    // The killcam: the first snapshot that views the shooter, its look
    // back and its `commandTime`, shifted to the frame's (cod4-combat.md 7).
    let killcam = |s: &Side| {
        s.snaps
            .iter()
            .find(|f| f.server_time > 48_000 && f.ps.get("clientNum") == Some(1))
            .map(|f| {
                (
                    f.server_time,
                    f.ps.get("deltaTime").unwrap_or(0),
                    f.ps.get("commandTime").unwrap_or(0),
                )
            })
    };
    let (a, b) = (killcam(ot), killcam(rt));
    if a != b {
        out.push(format!("target killcam: ours {a:?} retail {b:?}"));
    }
    // The killcam's world: the players and corpses each killcam frame
    // carries, by number and type (the archive's, not the live one).
    let bodies = |f: &Snapshot| {
        f.entities
            .iter()
            .filter(|e| matches!(e.etype(), 1 | 2))
            .map(|e| (e.number(), e.etype()))
            .collect::<Vec<_>>()
    };
    for r in rt
        .snaps
        .iter()
        .filter(|f| f.server_time > 48_000 && f.ps.get("clientNum") == Some(1))
    {
        if let Some(o) = at(ot, r.server_time)
            && bodies(o) != bodies(r)
        {
            out.push(format!(
                "target killcam bodies {}: ours {:?} retail {:?}",
                r.server_time,
                bodies(o),
                bodies(r)
            ));
        }
    }
    out
}

#[test]
fn cod4_combat_ab() {
    let Some(install) = vcod_cod4::testing::cod4_install() else {
        return;
    };
    let (rt, rs) = (side(&kill("target")), side(&kill("shooter")));
    let cs0 = String::from_utf8_lossy(rt.gs.configstring(0)).into_owned();
    let (probes, aborts) = ours(&install, &cs0);
    for a in &aborts {
        eprintln!("script error: {a}");
    }
    for p in &probes {
        super::join_tests::reencodes(&p.lines);
    }
    let (ot, os) = (side(&probes[0].lines), side(&probes[1].lines));
    let all = diffs(&rt, &rs, &ot, &os);
    let report = std::env::var_os("COMBAT_REPORT").is_some();
    let gap = |d: &str| GAPS.iter().find(|(g, _)| d.starts_with(g));
    let mut unexplained = Vec::new();
    for d in &all {
        match gap(d) {
            Some((_, why)) if report => eprintln!("  {d} (gap: {why})"),
            Some(_) => {}
            None => {
                if report {
                    eprintln!("  {d}");
                }
                unexplained.push(d.clone());
            }
        }
    }
    eprintln!(
        "cod4_combat_ab: {} rows differ, {} outside GAPS",
        all.len(),
        unexplained.len()
    );
    assert!(
        unexplained.is_empty(),
        "rows outside GAPS:\n{}",
        unexplained.join("\n")
    );
    // A row no difference matches is a gap closed: it goes.
    let unused: Vec<&str> = GAPS
        .iter()
        .filter(|(g, _)| !all.iter().any(|d| d.starts_with(g)))
        .map(|(g, _)| *g)
        .collect();
    assert!(unused.is_empty(), "GAPS rows matching nothing: {unused:?}");
}

/// The live client's three weapon inputs on ours, as `vcod --game cod4`
/// sends them (`play::cod4_input`): the sight key (0x800) brings
/// `fWeaponPosFrac` to 1 and `PMF_SIGHT`, the attack key (1) fires
/// the M16's burst out of its clip with `EV_FIRE_WEAPON`, and a weapon
/// byte of the held pistol (`weapnext`'s `cg_weaponSelect`) drops the rifle
/// and raises it (cod4-combat.md 2).
#[test]
fn the_sight_the_trigger_and_a_switch_drive_the_weapon() {
    let Some(install) = vcod_cod4::testing::cod4_install() else {
        return;
    };
    let rt = side(&kill("target"));
    let cs0 = String::from_utf8_lossy(rt.gs.configstring(0)).into_owned();
    // `assault_mp,0`: the M16 is 27, the Beretta 11 (cod4-gsc.md 7).
    let phase = |t: i32| {
        if t < 47_500 {
            (0, 27)
        } else if t < 48_500 {
            (0x800, 27)
        } else if t < 49_500 {
            (0x801, 27)
        } else if t < 50_500 {
            (0, 27)
        } else {
            (0, 11)
        }
    };
    let cmds: VecDeque<(i32, Vec<UserCmd>)> = (SHOOTER_SPAWN_MS / FRAME_MS + 1..53_000 / FRAME_MS)
        .map(|k| {
            let t = k * FRAME_MS;
            let (buttons, weapon) = phase(t);
            let c = UserCmd {
                server_time: t - 40,
                buttons,
                weapon,
                offhand: 40,
                ..Default::default()
            };
            (t - FRAME_MS, vec![c])
        })
        .collect();
    let (probes, _) = ours_with(&install, &cs0, Some(cmds), &[]);
    let os = side(&probes[1].lines);
    let w = |t: i32, n: &str| at(&os, t).and_then(|f| f.ps.get(n));
    let clip = |t: i32| at(&os, t).map(|f| f.ps.ammoclip[27]);
    assert_eq!(
        w(48_400, "fWeaponPosFrac"),
        Some(1.0f32.to_bits()),
        "sighted"
    );
    assert_ne!(w(48_400, "pm_flags").unwrap() & 0x10, 0, "PMF_SIGHT");
    let fired = os
        .snaps
        .iter()
        .filter(|f| (48_500..49_600).contains(&f.server_time))
        .any(|f| (0..4).any(|k| f.ps.get(&format!("events[{k}]")) == Some(26)));
    assert!(fired, "EV_FIRE_WEAPON");
    assert!(
        clip(49_600) < clip(48_400),
        "rounds left the clip: {:?} {:?}",
        clip(48_400),
        clip(49_600)
    );
    assert_eq!(w(52_900, "weapon"), Some(11), "the pistol came up");
    eprintln!(
        "sight {:?}, clip {:?} -> {:?}, weapon at 52900 {:?}",
        w(48_400, "fWeaponPosFrac").map(f32::from_bits),
        clip(48_400),
        clip(49_600),
        w(52_900, "weapon")
    );
}

/// The `score` reply (the scoreboard reply) against the score capture's twelve
/// (`play-1.7-score-target.txt`, the kill recipe with the target's probe
/// sending `score` every 2 s, cod4-hud.md 7): the kill pair replays as
/// [`cod4_combat_ab`] does and the target sends `score` at the capture's
/// times past its first snapshot. Each reply is compared word for word but
/// the pings, which are the link's.
#[test]
fn the_score_command_answers_as_retail() {
    let Some(install) = vcod_cod4::testing::cod4_install() else {
        return;
    };
    let rt = side(&kill("target"));
    let cs0 = String::from_utf8_lossy(rt.gs.configstring(0)).into_owned();
    // The capture's requests, by the server time of the snapshot before
    // each, past its first snapshot.
    let score = retail("score-target");
    let mut s = vcod_cod4::net::capture::Session::new();
    let (mut first, mut last, mut asks, mut replies) = (None, 0, Vec::new(), Vec::new());
    for l in score.iter().filter(|l| l.dir != "auth") {
        match s.feed(l) {
            Ok(Some(vcod_cod4::net::capture::Item::Client { msg, .. })) => {
                for op in &msg.ops {
                    if let vcod_cod4::net::clmsg::ClOp::Command { text, .. } = op
                        && text.as_slice() == b"score"
                        && !asks.contains(&(last - first.unwrap_or(last)))
                    {
                        asks.push(last - first.unwrap_or(last));
                    }
                }
            }
            Ok(Some(vcod_cod4::net::capture::Item::Server { parsed, .. })) => {
                for op in &parsed.ops {
                    match op {
                        vcod_cod4::net::parse::Op::Snapshot(i) => {
                            last = s.client.snapshots[*i].server_time;
                            first.get_or_insert(last);
                        }
                        vcod_cod4::net::parse::Op::ServerCommand { text, .. }
                            if text.starts_with(b"b ") =>
                        {
                            let t = String::from_utf8_lossy(text).into_owned();
                            if !replies.contains(&t) {
                                replies.push(t);
                            }
                        }
                        _ => {}
                    }
                }
            }
            _ => {}
        }
    }
    assert_eq!(asks.len(), 12, "the capture's requests");
    let extra: Vec<(i32, &'static str)> = asks.iter().map(|&t| (t + 50, "score")).collect();
    let (probes, _) = ours_with(&install, &cs0, None, &extra);
    let ot = side(&probes[0].lines);
    let ours: Vec<&String> = ot
        .commands
        .iter()
        .map(|(_, c)| c)
        .filter(|c| c.starts_with("b "))
        .collect();
    // Every word but each row's third (the ping).
    let masked = |t: &str| {
        t.split(' ')
            .enumerate()
            .map(|(i, w)| if i >= 5 && (i - 5) % 7 == 2 { "*" } else { w })
            .collect::<Vec<_>>()
            .join(" ")
    };
    // Reply 1 came 4.6 s past the score run's first snapshot, before its
    // kill; the kill capture's shooter (a run of its own) killed 4.65 s
    // past its first, so ours answers after the kill. A difference of the
    // two retail runs' timing, not of the board.
    const TIMING: &[usize] = &[1];
    let mut wrong = Vec::new();
    for (k, r) in replies.iter().enumerate() {
        let o = ours.get(k).map(|o| masked(o));
        if o.as_deref() != Some(masked(r).as_str()) && !TIMING.contains(&k) {
            wrong.push(format!("reply {k}: ours {o:?} retail {:?}", masked(r)));
        }
    }
    eprintln!(
        "score replies: ours {} retail {}, {} differ",
        ours.len(),
        replies.len(),
        wrong.len()
    );
    for w in &wrong {
        eprintln!("  {w}");
    }
    assert_eq!(ours.len(), replies.len(), "one reply a request");
    assert!(wrong.is_empty(), "replies differ:\n{}", wrong.join("\n"));
}

/// A shot aimed at the head scores the head: the kill pair's target stands
/// where its `setviewpos` put it, and from 47450 the shooter's cmd views
/// the centre of the target's posed head box (its eye and `delta_angles`
/// as the shooter's playerstate holds them, the sight up), then taps the
/// trigger once. The M16's first round lands in the head (hit location 2).
#[test]
fn a_shot_at_the_head_scores_the_head() {
    let Some(install) = vcod_cod4::testing::cod4_install() else {
        return;
    };
    let rt = side(&kill("target"));
    let cs0 = String::from_utf8_lossy(rt.gs.configstring(0)).into_owned();
    let mut hook = |sv: &mut Cod4Server, probes: &mut [Probe], svs: i32| {
        if !(47_450..48_200).contains(&svs) {
            return;
        }
        let level = sv.game.level.as_mut().unwrap();
        let (Some(head), Some(ps)) = (level.hit_box_center(0, 2), level.pm_state(1)) else {
            return;
        };
        let eye = crate::game::cod4::combat::view_origin(&ps, ps.command_time);
        let d = head - eye;
        let flat = (d.x * d.x + d.y * d.y).sqrt();
        let view = [-d.z.atan2(flat).to_degrees(), d.y.atan2(d.x).to_degrees()];
        let short = |a: f32| ((a * 65536.0 / 360.0).round() as i32 & 0xffff) as u16;
        let angles = [
            short(view[0]).wrapping_sub(short(ps.delta_angles[0])),
            short(view[1]).wrapping_sub(short(ps.delta_angles[1])),
            0u16.wrapping_sub(short(ps.delta_angles[2])),
        ];
        let attack = if (47_800..47_850).contains(&svs) {
            1
        } else {
            0
        };
        let c = UserCmd {
            server_time: svs + 10,
            buttons: 0x800 | attack,
            weapon: 27,
            offhand: 40,
            angles,
            ..Default::default()
        };
        probes[1].replay.clear();
        probes[1].replay.push_back((svs, vec![c]));
    };
    let (_, _, sv) = ours_hooked(&install, &cs0, None, &[], &mut hook);
    let hits = &sv.game.level.as_ref().unwrap().combat_hits();
    eprintln!("hits: {hits:?}");
    let first = hits.iter().find(|h| h.0 == 1 && h.1 == 0).copied();
    assert_eq!(first.map(|h| h.2), Some(2), "{hits:?}");
}

/// The melee key swings and hurts: the target is put 40 units ahead of the
/// shooter, who views its chest and presses the melee key (0x4) once; the
/// swing (`EV_MELEE_SWIPE`, then `EV_FIRE_MELEE` at `meleeDelay`) scores a
/// hit on the target (cod4-combat.md 10.4).
#[test]
fn the_melee_key_swings_and_hits() {
    let Some(install) = vcod_cod4::testing::cod4_install() else {
        return;
    };
    let rt = side(&kill("target"));
    let cs0 = String::from_utf8_lossy(rt.gs.configstring(0)).into_owned();
    let mut events = Vec::new();
    let mut hook = |sv: &mut Cod4Server, probes: &mut [Probe], svs: i32| {
        if !(47_450..48_200).contains(&svs) {
            return;
        }
        let level = sv.game.level.as_mut().unwrap();
        let (Some(target), Some(ps)) = (level.pm_state(0), level.pm_state(1)) else {
            return;
        };
        events.extend((0..4).map(|k| ps.events[k]));
        let eye = crate::game::cod4::combat::view_origin(&ps, ps.command_time);
        let d = target.origin + glam::Vec3::Z * 45.0 - eye;
        let flat = (d.x * d.x + d.y * d.y).sqrt();
        let view = [-d.z.atan2(flat).to_degrees(), d.y.atan2(d.x).to_degrees()];
        let short = |a: f32| ((a * 65536.0 / 360.0).round() as i32 & 0xffff) as u16;
        let angles = [
            short(view[0]).wrapping_sub(short(ps.delta_angles[0])),
            short(view[1]).wrapping_sub(short(ps.delta_angles[1])),
            0u16.wrapping_sub(short(ps.delta_angles[2])),
        ];
        let melee = if (47_800..47_850).contains(&svs) {
            4
        } else {
            0
        };
        let c = UserCmd {
            server_time: svs + 10,
            buttons: melee,
            weapon: 27,
            offhand: 40,
            angles,
            ..Default::default()
        };
        probes[1].replay.clear();
        probes[1].replay.push_back((svs, vec![c]));
    };
    let (_, _, sv) = ours_hooked(
        &install,
        &cs0,
        None,
        &[(3600, "setviewpos 140 2146 296")],
        &mut hook,
    );
    let hits = sv.game.level.as_ref().unwrap().combat_hits();
    assert!(events.contains(&30), "EV_MELEE_SWIPE: {events:?}");
    assert!(events.contains(&31), "EV_FIRE_MELEE: {events:?}");
    assert!(hits.iter().any(|h| h.0 == 1 && h.1 == 0), "{hits:?}");
}

/// `kill` under `sv_cheats` (`+devmap`) kills its sender: the target sends
/// it 3.6 s past its first snapshot, before the shooter's first round; its
/// playerstate goes dead and its corpse (`ET_PLAYER_CORPSE`, the body queue
/// from 64) carries a death anim.
#[test]
fn the_kill_command_kills_its_sender() {
    let Some(install) = vcod_cod4::testing::cod4_install() else {
        return;
    };
    let rt = side(&kill("target"));
    let cs0 = String::from_utf8_lossy(rt.gs.configstring(0)).into_owned();
    let (probes, aborts) = ours_with(&install, &cs0, None, &[(3600, "kill")]);
    let ot = side(&probes[0].lines);
    let dead = ot
        .snaps
        .iter()
        .find(|f| f.server_time < 47_900 && f.ps.get("pm_type") == Some(7));
    assert!(dead.is_some(), "aborts {aborts:?}");
    let corpse = ot
        .snaps
        .iter()
        .flat_map(|f| f.entities.iter())
        .find(|e| e.etype() == 2)
        .copied();
    let legs = corpse.and_then(|e| {
        EntityState::table(2)
            .iter()
            .find(|f| f.name == "legsAnim")
            .map(|f| e.0[f.word()])
    });
    assert!(legs.is_some_and(|l| l & 0x1ff != 0), "{legs:?}");
}

/// A dropped rifle hands its ammo over: once the target's M16 lies at rest,
/// the shooter (an M16 too) is put on it; it takes the stock and clip
/// `dropWeaponForDeath` set into its reserve (`EV_AMMO_PICKUP`, 9) and the
/// item goes (cod4-combat.md 6).
#[test]
fn a_dropped_rifle_hands_its_ammo_over() {
    let Some(install) = vcod_cod4::testing::cod4_install() else {
        return;
    };
    let rt = side(&kill("target"));
    let cs0 = String::from_utf8_lossy(rt.gs.configstring(0)).into_owned();
    let mut before = None;
    let mut hook = |sv: &mut Cod4Server, _: &mut [Probe], svs: i32| {
        let level = sv.game.level.as_mut().unwrap();
        if svs == 49_000 {
            let at = level.items_at_rest().first().copied();
            let ps = level.pm_state(1).unwrap();
            before = Some(ps.ammo);
            // 10 units off the item, its box still over it: the rifle
            // lies against a wall, and a body on top of it starts in solid.
            if let Some(o) = at {
                level.set_view_pos(1, [o.x, o.y - 10.0, o.z + 61.0], [0.0; 3]);
            }
        }
    };
    let (probes, _, sv) = ours_hooked(&install, &cs0, None, &[], &mut hook);
    let level = sv.game.level.as_ref().unwrap();
    assert!(level.items_at_rest().is_empty(), "the item is still there");
    let os = side(&probes[1].lines);
    let got = os
        .snaps
        .iter()
        .filter(|f| (49_000..49_500).contains(&f.server_time))
        .any(|f| (0..4).any(|k| f.ps.get(&format!("events[{k}]")) == Some(9)));
    assert!(got, "EV_AMMO_PICKUP");
    let after = level.pm_state(1).unwrap().ammo;
    assert!(after.iter().sum::<i32>() > before.unwrap().iter().sum::<i32>());
}

/// The grenade launcher fires its grenade: the shooter switches to the
/// M16's alt (`gl_m16_mp`, the weapon byte the alt's number), and its
/// trigger puts an `ET_MISSILE` of that weapon in the world (cod4-combat.md
/// 10.3).
#[test]
fn the_grenade_launcher_fires_its_grenade() {
    let Some(install) = vcod_cod4::testing::cod4_install() else {
        return;
    };
    let rt = side(&kill("target"));
    let cs0 = String::from_utf8_lossy(rt.gs.configstring(0)).into_owned();
    let mut seen = Vec::new();
    let mut hook = |sv: &mut Cod4Server, probes: &mut [Probe], svs: i32| {
        if !(47_450..49_000).contains(&svs) {
            return;
        }
        let level = sv.game.level.as_mut().unwrap();
        let Some(ps) = level.pm_state(1) else {
            return;
        };
        let alt = level.rt.host.cod4().table.defs[27].alt as u8;
        for m in &level.rt.host.cod4().missiles {
            seen.push((m.weapon, m.pos.delta.length()));
        }
        let short = |a: f32| ((a * 65536.0 / 360.0).round() as i32 & 0xffff) as u16;
        let c = UserCmd {
            server_time: svs + 10,
            buttons: if (48_400..48_450).contains(&svs) {
                1
            } else {
                0
            },
            weapon: alt,
            offhand: 40,
            angles: [
                0u16.wrapping_sub(short(ps.delta_angles[0])),
                short(ps.view_angles[1]).wrapping_sub(short(ps.delta_angles[1])),
                0u16.wrapping_sub(short(ps.delta_angles[2])),
            ],
            ..Default::default()
        };
        probes[1].replay.clear();
        probes[1].replay.push_back((svs, vec![c]));
    };
    let (_, _, sv) = ours_hooked(&install, &cs0, None, &[], &mut hook);
    let alt = sv.game.level.as_ref().unwrap().rt.host.cod4().table.defs[27].alt as u32;
    assert!(alt != 0);
    // It strikes inside its 375-unit arming distance and bounces on as a
    // dud, slower.
    assert!(seen.iter().any(|m| m.0 == alt), "no launcher grenade");
}

/// The corpse's `startRagdoll` (cod4-combat.md 5.1): each time its `pos`
/// and `apos` types change, on both clients, live and in the killcam's
/// replay. Retail turns 1 and 0 into 11 and 9 at 49550, the stock
/// `delayStartRagdoll`'s 0.2 s and `start_ragdoll` fraction of the death
/// anim past the clone.
#[test]
fn the_corpse_turns_ragdoll_when_retails_did() {
    let Some(install) = vcod_cod4::testing::cod4_install() else {
        return;
    };
    let (rt, rs) = (side(&kill("target")), side(&kill("shooter")));
    let cs0 = String::from_utf8_lossy(rt.gs.configstring(0)).into_owned();
    let (probes, _) = ours(&install, &cs0);
    let (ot, os) = (side(&probes[0].lines), side(&probes[1].lines));
    let changes = |s: &Side| {
        let mut out: Vec<(i32, Vec<u32>, Vec<u32>)> = Vec::new();
        for f in &s.snaps {
            let types = |n: &str| of_type(f, 2, n).iter().map(|x| x.1).collect::<Vec<_>>();
            let v = (types("lerp.pos.trType"), types("lerp.apos.trType"));
            if out.last().is_none_or(|l| (&l.1, &l.2) != (&v.0, &v.1)) {
                out.push((f.server_time, v.0, v.1));
            }
        }
        out
    };
    let ragdoll = (49_550, vec![11], vec![9]);
    assert!(changes(&rt).contains(&ragdoll), "{:?}", changes(&rt));
    assert_eq!(changes(&ot), changes(&rt), "target");
    assert_eq!(changes(&os), changes(&rs), "shooter");
}

/// The corpse's own move (cod4-combat.md 5.2): `clonePlayer` launches it
/// under gravity, the frame's corpse pass lands it and its death anim's
/// root motion drags it on until the per-frame move falls to a unit. Its
/// `pos.trBase` as the wire rounds it, each time it changes, on the
/// target's client: retail's drifts from (208, 2294) to (214, 2310) over
/// 48000..48650 and again in the killcam's replay.
#[test]
fn the_corpse_drifts_on_its_death_anim_as_on_retail() {
    let Some(install) = vcod_cod4::testing::cod4_install() else {
        return;
    };
    let rt = side(&kill("target"));
    let cs0 = String::from_utf8_lossy(rt.gs.configstring(0)).into_owned();
    let (probes, _) = ours(&install, &cs0);
    let ot = side(&probes[0].lines);
    let path = |s: &Side| {
        let mut out: Vec<(i32, Vec<[i32; 3]>)> = Vec::new();
        for f in &s.snaps {
            let at = |k: usize| {
                of_type(f, 2, &format!("lerp.pos.trBase[{k}]"))
                    .iter()
                    .map(|x| f32::from_bits(x.1).round() as i32)
                    .collect::<Vec<_>>()
            };
            let (x, y, z) = (at(0), at(1), at(2));
            let v: Vec<[i32; 3]> = (0..x.len()).map(|i| [x[i], y[i], z[i]]).collect();
            if out.last().is_none_or(|l| l.1 != v) {
                out.push((f.server_time, v));
            }
        }
        out
    };
    let want = path(&rt);
    assert!(
        want.iter().any(|r| r.1 == vec![[214, 2310, 236]]),
        "{want:?}"
    );
    assert_eq!(path(&ot), want);
}

/// `self.killcamentity` rides the killcam's playerstate as
/// `killCamEntity`, 1023 for -1 (cod4_lnxded 0x80a4a9b, cod4-combat.md 7):
/// the stock killcam writes -1 for a rifle kill, and a value held on it
/// from then on reaches the follower's frames.
#[test]
fn a_killcam_entity_rides_the_killcam_playerstate() {
    let Some(install) = vcod_cod4::testing::cod4_install() else {
        return;
    };
    let rt = side(&kill("target"));
    let cs0 = String::from_utf8_lossy(rt.gs.configstring(0)).into_owned();
    let mut hook = |sv: &mut Cod4Server, _: &mut [Probe], svs: i32| {
        if svs >= 50_500 {
            let level = sv.game.level.as_mut().unwrap();
            level
                .rt
                .set_client_field_for_test(0, "killcamentity", vcod_gsc::Value::Int(70));
        }
    };
    let (probes, aborts, _) = ours_hooked(&install, &cs0, None, &[], &mut hook);
    assert!(aborts.is_empty(), "{aborts:?}");
    let ot = side(&probes[0].lines);
    let cam: Vec<(i32, u32)> = ot
        .snaps
        .iter()
        .filter(|f| f.server_time > 48_000 && f.ps.get("clientNum") == Some(1))
        .map(|f| (f.server_time, f.ps.get("killCamEntity").unwrap_or(0)))
        .collect();
    assert!(cam.iter().any(|c| c.0 < 50_500 && c.1 == 1023), "{cam:?}");
    assert!(cam.iter().any(|c| c.0 > 50_600 && c.1 == 70), "{cam:?}");
    // Retail's rifle killcam reads none.
    assert!(
        rt.snaps
            .iter()
            .filter(|f| f.server_time > 48_000 && f.ps.get("clientNum") == Some(1))
            .all(|f| f.ps.get("killCamEntity") == Some(1023))
    );
}
