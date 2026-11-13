//! A script model in a locational trace's way, ours against retail's.
//! `client-probes/probe_scriptmodel` spawns an `xmodel/barrel_black1`
//! script model and logs a `bulletTrace` through it shown, hidden,
//! `notSolid()`ed and deleted (1.1d and UO alike); on UO it also stands one
//! between a `--probe-uo-weapon aim` shooter and an empty Willys, shown for
//! 3 s, hidden for 3 s, then deleted, and logs the jeep's `damage`
//! notifies. Retail stops the trace and every round on the model until it
//! is deleted, hidden or not (docs/research/cod11-combat.md 2.7).
//!
//! On 1.1d the pair parts (`probe_part`) put a player behind a barrel: a
//! Thompson held on him (`rounds`), a frag thrown at the barrel and then
//! with it gone at him (`grenade`), and a frag resting with a barrel
//! between it and both players (`blast`). The `prone` part throws a frag
//! onto a prone player's back and, once it rests and he is moved off, a
//! second where he lay: on mp_carentan the first rests on him with his
//! number as its ground, on mp_brecourt the second rolls down terrain.
//! On 1.5b and UO the same part checks where each frag leaves from against
//! the thrower's logged origin.
//!
//! `SM_REPORT=1` prints both logs. The 1.1d tests need `COD_DIR` at a 1.1
//! install, the 1.5b one at a 1.5 install, the UO ones at a UO one; each
//! returns early without.

use crate::common;

use common::{Join, Queues, step};
use std::cell::RefCell;
use std::rc::Rc;
use std::time::{Duration, Instant};
use vcod_common::net::msg::{BUTTON_ADS, BUTTON_ATTACK, UserCmd};
use vcod_common::net::protocol::{PROTOCOL_V1, PROTOCOL_V6, PROTOCOL_V22};
use vcod_common::net::{NetClient, NetEvent};

/// The shooter's first rounds leave before its sight is up, at the hip's
/// spread, and ours' RNG sends one or two past the barrel's sides where
/// retail's sent none; the shown phase is counted from this far past the
/// placement.
const SIGHT_MS: i32 = 500;

const SCRIPT: &str =
    include_str!("../../gsc/tests/fixtures/semantics/client-probes/probe_scriptmodel.gsc");

/// Our run of the probe on `map`; with `shooter`, client 0 joins and holds
/// the trigger from its placement for `ms` past it.
fn ours(fs: vcod_common::pk3::Pk3Fs, map: &str, uo: bool, shooter: bool, ms: i64) -> Vec<String> {
    let bsp_path = fs.resolve_map(map).expect("map in the paks");
    let bsp = vcod_common::bsp::parse(&fs.read(&bsp_path).unwrap()).unwrap();
    let mut now = Instant::now();
    let mut sv = vcod_server::Server::new(common::cfg(map, "probe_scriptmodel"), now);
    if uo {
        sv.set_protocol(&PROTOCOL_V22);
    }
    sv.overlay_script("maps/mp/gametypes/probe_scriptmodel", SCRIPT);
    sv.load_world(vcod_server::world::World::from_bsp(&bsp, Some(&fs)));
    sv.load_scripts(Rc::new(fs)).expect("load the scripts");
    if !shooter {
        common::run_alone(&mut sv, &mut now, 3000);
        return sv.script_log().to_vec();
    }
    common::run_alone(&mut sv, &mut now, common::PROBE_JOIN_MS);
    let q = Rc::new(RefCell::new(Queues::default()));
    let mut cl = NetClient::start_with_qport(common::ClientEnd(q.clone()), now, 0x2001);
    let mut join = Join::new("allies", "thompson_mp");
    let frame = Duration::from_millis(common::FRAME_MS as u64);
    let mut hold = common::ProbeHold {
        aim: true,
        ..Default::default()
    };
    let mut placed_at: Option<i64> = None;
    for i in 0..3000i64 {
        now += frame;
        if placed_at.is_some_and(|at| (i - at) * common::FRAME_MS > ms) {
            break;
        }
        if placed_at.is_some_and(|at| i > at) && cl.state() == vcod_common::net::NetState::Active {
            cl.send_cmds(&hold.cmds(&cl, BUTTON_ATTACK | BUTTON_ADS));
        } else if let Some(c) = cl.send_frame(&UserCmd::default()) {
            hold.last = c.server_time;
        }
        for e in step(&mut sv, &q, &mut cl, now) {
            if let NetEvent::ServerCommand(tokens) = e {
                join.on_server_command(&tokens, &mut cl, now);
            }
        }
        if placed_at.is_none() && sv.script_log().iter().any(|l| l.contains("PROBE place")) {
            placed_at = Some(i);
        }
    }
    sv.script_log().to_vec()
}

fn probe_lines(text: impl IntoIterator<Item = String>) -> Vec<String> {
    text.into_iter()
        .map(|l| l.trim().to_string())
        .filter(|l| l.starts_with("PROBE"))
        .collect()
}

fn report(retail: &[String], ours: &[String]) {
    if std::env::var_os("SM_REPORT").is_some() {
        for l in ours {
            eprintln!("ours   {l}");
        }
        for l in retail {
            eprintln!("retail {l}");
        }
    }
}

/// Each `mtrace` line's phase, whether it stopped on the model, its
/// fraction and surface type.
fn traces(lines: &[String]) -> Vec<(String, bool, f32, String)> {
    let model = lines
        .iter()
        .find_map(|l| l.strip_prefix("PROBE model "))
        .and_then(|r| r.split(' ').next())
        .expect("the model was spawned")
        .to_string();
    lines
        .iter()
        .filter_map(|l| l.strip_prefix("PROBE mtrace "))
        .map(|l| {
            let t: Vec<&str> = l.split(' ').collect();
            (
                t[1].to_string(),
                t[2] == model,
                t[3].parse().unwrap(),
                t[4].to_string(),
            )
        })
        .collect()
}

fn compare_traces(fixture: &str, log: Vec<String>) {
    let retail = probe_lines(fixture.lines().map(str::to_string));
    let ours = probe_lines(log);
    report(&retail, &ours);
    let (r, o) = (traces(&retail), traces(&ours));
    assert_eq!(r.len(), 4, "{retail:?}");
    assert_eq!(o.len(), r.len(), "{ours:?}");
    for (r, o) in r.iter().zip(&o) {
        assert_eq!(
            (&o.0, o.1, &o.3),
            (&r.0, r.1, &r.3),
            "retail {r:?}, ours {o:?}"
        );
        assert!((o.2 - r.2).abs() < 0.005, "retail {r:?}, ours {o:?}");
    }
}

/// mp_carentan on 1.1d: the barrel stops the trace shown, hidden and
/// `notSolid`; deleted, the trace runs on to the world.
#[test]
fn a_script_model_stops_bullettrace_until_deleted() {
    let Some(fs) = vcod_common::testing::game_fs_11() else {
        return;
    };
    let log = ours(fs, "mp_carentan", false, false, 0);
    compare_traces(
        include_str!("fixtures/scriptmodel/mp_carentan-scriptmodel.txt"),
        log,
    );
}

/// The jeep's damage notifies in each phase of the barrel: shown, hidden,
/// deleted. The shown phase starts [`SIGHT_MS`] past the placement.
fn phase_counts(lines: &[String]) -> [usize; 3] {
    let time = |l: &str| -> i32 { l.split(' ').nth(2).unwrap().parse().unwrap() };
    let at = |p: &str| {
        lines
            .iter()
            .find(|l| l.starts_with("PROBE rphase") && l.split(' ').nth(3) == Some(p))
            .map(|l| time(l))
            .unwrap_or_else(|| panic!("no {p} phase in {lines:?}"))
    };
    let (shown, hidden, deleted) = (at("shown"), at("hidden"), at("deleted"));
    let mut out = [0; 3];
    for l in lines.iter().filter(|l| l.starts_with("PROBE vdamage")) {
        let t = time(l);
        if t < shown + SIGHT_MS {
            continue;
        }
        out[(t >= hidden) as usize + (t >= deleted) as usize] += 1;
    }
    out
}

/// mp_peaks on UO: the barrel stops every round shown and hidden; deleted,
/// the jeep takes them, 22 each.
#[test]
fn a_script_model_stops_rounds_until_deleted_on_uo() {
    let Some(fs) = vcod_common::testing::uo_fs() else {
        return;
    };
    let fixture = include_str!("fixtures/scriptmodel/mp_peaks-scriptmodel-uo.txt");
    let log = ours(fs, "mp_peaks", true, true, 9000);
    let retail = probe_lines(fixture.lines().map(str::to_string));
    let ours = probe_lines(log);
    report(&retail, &ours);
    let (rt, ot) = (traces(&retail), traces(&ours));
    assert_eq!(rt.len(), ot.len(), "{ours:?}");
    for (r, o) in rt.iter().zip(&ot) {
        assert_eq!(
            (&o.0, o.1, &o.3),
            (&r.0, r.1, &r.3),
            "retail {r:?}, ours {o:?}"
        );
        assert!((o.2 - r.2).abs() < 0.005, "retail {r:?}, ours {o:?}");
    }
    // Retail's deleted phase ran to the server's end; ours stops 3 s past
    // the delete, so only its first 3 s count.
    let deleted = |lines: &[String]| {
        lines
            .iter()
            .find(|l| l.starts_with("PROBE rphase") && l.ends_with("deleted"))
            .and_then(|l| l.split(' ').nth(2)?.parse::<i32>().ok())
            .expect("the barrel was deleted")
    };
    let rd = deleted(&retail);
    let retail_window: Vec<String> = retail
        .iter()
        .filter(|l| {
            !l.starts_with("PROBE vdamage")
                || l.split(' ')
                    .nth(2)
                    .and_then(|t| t.parse::<i32>().ok())
                    .is_some_and(|t| t < rd + 3000)
        })
        .cloned()
        .collect();
    let (r, o) = (phase_counts(&retail_window), phase_counts(&ours));
    assert_eq!((r[0], r[1]), (0, 0), "retail's barrel let rounds by: {r:?}");
    assert_eq!((o[0], o[1]), (0, 0), "ours' barrel let rounds by: {o:?}");
    assert!(
        r[2] > 10 && o[2].abs_diff(r[2]) * 5 <= r[2],
        "rounds past the deleted barrel in 3 s: retail {}, ours {}",
        r[2],
        o[2]
    );
    assert!(
        ours.iter()
            .filter(|l| l.starts_with("PROBE vdamage"))
            .all(|l| l.split(' ').nth(3) == Some("22")),
        "{ours:?}"
    );
}

/// Server time before ours' target connects, and how long after it the
/// shooter does: the fixtures' recipe, the target in slot 0 and the
/// barrel placed near retail's 12000. The target's idle clip runs on the
/// level time and his slot, and a frag off his bones reads its pose.
const TARGET_JOIN_MS: i64 = 7000;
const SHOOTER_LATE_MS: i64 = 3000;

/// The pair parts on mp_carentan (1.1d): `probe_part` set to `part`, an
/// axis target and then an allies shooter or thrower joined, run until `ms`
/// past the script's first `rphase` or `rest` line. The shooter holds the
/// trigger at the snapshot's view (`--probe-uo-weapon aim`) or, with
/// `throw`, runs `--probe-uo-weapon throw`'s two pullbacks. The `prone`
/// part's target holds prone (`--probe-fall-prone`). Each snapshot's
/// missiles follow the script's lines as the thrower's `VEH missile` lines.
fn ours_pair(part: &str, throw: bool, ms: i64) -> Vec<String> {
    ours_pair_on("mp_carentan", part, throw, ms, None)
}

/// [`ours_pair`] on `map`; with `prone_yaw` the target holds prone at that
/// world yaw on every cmd from the first, as `--probe-fall-prone` does (the
/// prone heading is the yaw the body lay down at).
fn ours_pair_on(
    map: &str,
    part: &str,
    throw: bool,
    ms: i64,
    prone_yaw: Option<f32>,
) -> Vec<String> {
    let fs = vcod_common::testing::game_fs_11().expect("checked by the caller");
    ours_pair_in(fs, map, part, throw, ms, prone_yaw, None, None)
}

/// [`ours_pair_on`] on `fs`'s install, its server speaking the install's
/// protocol. With `thrower_at` (where retail's spawn left him) the thrower
/// spawns on the spawn point under it instead of the script's pick.
///
/// With `target_log` (a capture's `target`-prefixed `--probe-fall` lines)
/// the target spawns on the spawn point under his first playing snapshot,
/// both clients join on the capture's clock and the target sends the
/// capture's cmds (`ProbeHold::times`): where his drop onto the grade
/// settles, and the prone body it leaves, rides them.
#[allow(clippy::too_many_arguments)]
fn ours_pair_in(
    fs: vcod_common::pk3::Pk3Fs,
    map: &str,
    part: &str,
    throw: bool,
    ms: i64,
    prone_yaw: Option<f32>,
    thrower_at: Option<[f32; 3]>,
    target_log: Option<&str>,
) -> Vec<String> {
    let fs = Rc::new(fs);
    let run = |shift| {
        ours_pair_run(
            fs.clone(),
            map,
            part,
            throw,
            ms,
            prone_yaw,
            thrower_at,
            target_log,
            shift,
        )
    };
    let (lines, played) = run(0);
    // Ours' join takes as long as its gamestate's fragments: a map or a
    // protocol whose join runs a frame off rejoins that much earlier, so
    // the target plays on the capture's frame and his cmds land on it.
    match (target_log.map(first_play), played) {
        (Some((t, _)), Some(p)) if p != t => run(t - p).0,
        _ => lines,
    }
}

/// One run of [`ours_pair_in`], the clients joining `shift` ms later than
/// [`TARGET_PLAYS_AFTER_JOIN_MS`] says, and the target's first playing
/// snapshot time.
#[allow(clippy::too_many_arguments)]
fn ours_pair_run(
    fs: Rc<vcod_common::pk3::Pk3Fs>,
    map: &str,
    part: &str,
    throw: bool,
    ms: i64,
    prone_yaw: Option<f32>,
    thrower_at: Option<[f32; 3]>,
    target_log: Option<&str>,
    shift: i64,
) -> (Vec<String>, Option<i64>) {
    let prone = prone_yaw.is_some();
    let mut missiles = Vec::new();
    let mut last_snap = None;
    let proto = vcod_server::server::install_protocol(&fs);
    let bsp_path = fs.resolve_map(map).expect("map in the paks");
    let bsp = vcod_common::bsp::parse(&fs.read(&bsp_path).unwrap()).unwrap();
    let mut now = Instant::now();
    let mut sv = vcod_server::Server::new(common::cfg(map, "probe_scriptmodel"), now);
    sv.set_protocol(proto);
    sv.overlay_script("maps/mp/gametypes/probe_scriptmodel", SCRIPT);
    sv.set_cvar("probe_part", part);
    let world = vcod_server::world::World::from_bsp(&bsp, Some(&fs));
    let target_play = target_log.map(first_play);
    if let Some(at) = thrower_at {
        // The target joins first, slot 0; the thrower is slot 1.
        let spot = placed_spawn_under(&world, common::spawn_points(&fs, map), at);
        let target = target_play
            .map(|(_, at)| {
                vec![Some(placed_spawn_under(
                    &world,
                    common::spawn_points(&fs, map),
                    at,
                ))]
            })
            .unwrap_or_default();
        sv.pin_spawns(vec![target, vec![Some(spot)]]);
    }
    sv.load_world(world);
    sv.load_scripts(fs).expect("load the scripts");
    // The script draws the target's variant (`randomInt(3)`) and head
    // (`randomInt(20)`) off `holdrand`, which retail seeds from its clock
    // (map-cycle doc 4.8). This seed draws variant 0, the `load1` gear the
    // 1.1d prone captures' rests sit on (INFERRED, off ours matching them
    // in it and not in variant 1), and head `a_01`.
    sv.pin_script_rand(1);
    let (join_ms, late_ms) = match (target_play, target_log) {
        (Some((t, _)), Some(log)) => {
            let thrower = thrower_spawn(log);
            (
                t - TARGET_PLAYS_AFTER_JOIN_MS + shift,
                thrower - t + TARGET_PLAYS_AFTER_JOIN_MS - THROWER_SPAWNS_AFTER_JOIN_MS,
            )
        }
        _ => (TARGET_JOIN_MS, SHOOTER_LATE_MS),
    };
    common::run_alone(&mut sv, &mut now, join_ms);
    let (qa, qb) = (
        Rc::new(RefCell::new(Queues::default())),
        Rc::new(RefCell::new(Queues::default())),
    );
    let mut ca: Option<NetClient<common::ClientEnd>> = None;
    let mut cb = NetClient::start_with_qport(common::ClientEnd(qb.clone()), now, 0x2002);
    let (mut ja, mut jb) = (
        Join::new("allies", "thompson_mp"),
        Join::new("axis", "kar98k_mp"),
    );
    // 1.5's and UO's thrower sends the net probe's cmds, three a frame from
    // each snapshot's time: how far he falls before the script's next
    // `setOrigin` check decides whether it puts him back (cod11-combat.md
    // 2.1, "The thrower's slide"). 1.1's captures log no thrower.
    let aim = || common::ProbeHold {
        aim: true,
        framed: proto.transport.version != PROTOCOL_V1.transport.version,
        ..Default::default()
    };
    let (mut ha, mut hb) = (aim(), aim());
    if let Some(yaw) = prone_yaw {
        hb = common::ProbeHold {
            base: Some([0.0, yaw, 0.0]),
            // 1.5's and UO's target keeps the body his drop's last airborne
            // cmd wrote; 1.1's lies flat after any drop, and its captures
            // settle on the grade where the even cadence puts him.
            framed: proto.transport.version != PROTOCOL_V1.transport.version,
            ..Default::default()
        };
        if let Some(log) = target_log {
            hb.times = common::cmds_times(log, "target");
            hb.ran = common::cmds_ran(log, "target");
            ha.times = common::cmds_times(log, "thrower");
            ha.ran = common::cmds_ran(log, "thrower");
        }
    }
    let mut selected: Option<u8> = None;
    let mut spawned: Option<i64> = None;
    let mut played: Option<i64> = None;
    let mut started: Option<i64> = None;
    let frame = Duration::from_millis(common::FRAME_MS as u64);
    let live = |cl: &NetClient<common::ClientEnd>| {
        cl.snapshots()
            .newest()
            .is_some_and(|s| s.ps.field_i32(cl.protocol(), "pm_type") == 0)
    };
    // One cmd round for a probe: the hold once it plays, a bare frame
    // before (the join's menus answer off it).
    let send = |cl: &mut NetClient<common::ClientEnd>,
                hold: &mut common::ProbeHold,
                buttons: u8,
                weapon: Option<u8>| {
        if live(cl) {
            let mut cmds = hold.cmds(cl, buttons);
            if let Some(w) = weapon {
                cmds.iter_mut().for_each(|c| c.weapon = w);
            }
            cl.send_cmds(&cmds);
        } else if let Some(c) = cl.send_frame(&UserCmd::default()) {
            hold.last = c.server_time;
        }
    };
    for i in 0..3000i64 {
        if started.is_some_and(|at| (i - at) * common::FRAME_MS > ms) {
            break;
        }
        now += frame;
        if ca.is_none() && i * common::FRAME_MS >= late_ms {
            ca = Some(NetClient::start_with_qport(
                common::ClientEnd(qa.clone()),
                now,
                0x2001,
            ));
        }
        if spawned.is_none() && ca.as_ref().is_some_and(live) {
            spawned = Some(i);
        }
        let buttons = match (throw, spawned) {
            (_, None) => 0,
            (false, Some(_)) => BUTTON_ATTACK | BUTTON_ADS,
            (true, Some(at)) => {
                // `vehicle_probe::THROW`: settle 3 s, pull back 1 s, 6 s
                // thrown, pull back 1 s.
                let t = (i - at) * common::FRAME_MS;
                if (3000..4000).contains(&t) || (10_000..11_000).contains(&t) {
                    BUTTON_ATTACK
                } else {
                    0
                }
            }
        };
        if let Some(ca) = ca.as_mut() {
            send(ca, &mut ha, buttons, selected);
        }
        if prone && live(&cb) {
            played.get_or_insert(cb.snapshots().newest().unwrap().server_time as i64);
            let mut cmds = hb.cmds(&cb, 0);
            for c in &mut cmds {
                c.wbuttons |= vcod_common::net::msg::WBUTTON_PRONE;
                c.up = -127;
            }
            cb.send_cmds(&cmds);
        } else {
            send(&mut cb, &mut hb, 0, None);
        }
        for (addr, q) in [(common::ADDR, &qa), (common::ADDR_B, &qb)] {
            let pending: Vec<Vec<u8>> = q.borrow_mut().to_server.drain(..).collect();
            for p in pending {
                sv.handle_packet(addr, &p, now);
            }
        }
        sv.tick(now);
        for (to, p) in sv.take_outgoing() {
            let q = if to == common::ADDR { &qa } else { &qb };
            q.borrow_mut().to_client.push_back(p);
        }
        if let Some(ca) = ca.as_mut() {
            for e in ca.pump_at(now) {
                if let NetEvent::ServerCommand(tokens) = e {
                    if tokens.first().map(String::as_str) == Some("a") {
                        selected = tokens.get(1).and_then(|w| w.parse().ok());
                    }
                    ja.on_server_command(&tokens, ca, now);
                }
            }
        }
        for e in cb.pump_at(now) {
            if let NetEvent::ServerCommand(tokens) = e {
                jb.on_server_command(&tokens, &mut cb, now);
            }
        }
        if let Some(snap) = ca.as_ref().and_then(|c| c.snapshots().newest())
            && last_snap != Some(snap.message_num)
        {
            last_snap = Some(snap.message_num);
            let p = ca.as_ref().unwrap().protocol();
            for (num, e) in &snap.entities {
                let i = |n: &str| e.field_i32(p, n);
                if i("eType") == 4 {
                    missiles.push(format!(
                        "VEH {} missile {num} type {} bits {:#x} {:#x} {:#x} ground {}",
                        snap.server_time,
                        i("pos.trType"),
                        i("pos.trBase[0]"),
                        i("pos.trBase[1]"),
                        i("pos.trBase[2]"),
                        i("groundEntityNum"),
                    ));
                }
            }
        }
        if started.is_none()
            && sv
                .script_log()
                .iter()
                .any(|l| l.contains("PROBE rphase") || l.contains("PROBE rest"))
        {
            started = Some(i);
        }
    }
    let mut out = sv.script_log().to_vec();
    out.extend(missiles);
    (out, played)
}

/// How long after ours' client connects its first playing snapshot comes,
/// the target's, and the thrower's spawn line (`PROBE thrower`).
const TARGET_PLAYS_AFTER_JOIN_MS: i64 = 1100;
const THROWER_SPAWNS_AFTER_JOIN_MS: i64 = 1100;

/// The time and origin of the first `target FALL` line that plays.
fn first_play(log: &str) -> (i64, [f32; 3]) {
    log.lines()
        .filter_map(|l| l.strip_prefix("target FALL "))
        .filter(|l| l.contains(" pm_type=0 "))
        .find_map(|l| {
            let t = l.strip_prefix("t=")?.split(' ').next()?.parse().ok()?;
            let o: Vec<f32> = l
                .split_once(" origin=")?
                .1
                .split(' ')
                .next()?
                .split(',')
                .map(|x| x.parse().unwrap())
                .collect();
            Some((t, [o[0], o[1], o[2]]))
        })
        .expect("a playing target FALL line")
}

/// The time of a fixture's first `PROBE thrower` line, his spawn.
fn thrower_spawn(text: &str) -> i64 {
    text.lines()
        .find_map(|l| {
            l.strip_prefix("PROBE thrower ")?
                .split(' ')
                .next()?
                .parse()
                .ok()
        })
        .expect("a PROBE thrower line")
}

/// The spawn point under `at`, where a spawn's think left a body, as the
/// gametype hands it to `spawn`: dropped onto the floor by
/// `placeSpawnpoint`'s player box (its ceiling check, then the fall).
fn placed_spawn_under(
    world: &vcod_server::world::World,
    points: Vec<([f32; 3], [f32; 3])>,
    at: [f32; 3],
) -> ([f32; 3], [f32; 3]) {
    use glam::Vec3;
    use vcod_server::game::trigger::{PLAYER_MAXS, PLAYER_MINS};
    let (origin, angles) = points
        .into_iter()
        .filter(|(p, _)| {
            (p[0] - at[0]).hypot(p[1] - at[1]) < 2.0 && (-2.0..128.0).contains(&(p[2] - at[2]))
        })
        .min_by(|a, b| a.0[2].total_cmp(&b.0[2]))
        .expect("a spawn point under the body");
    let (mins, maxs) = (Vec3::from(PLAYER_MINS), Vec3::from(PLAYER_MAXS));
    let start = Vec3::from(origin);
    let c = &world.collision;
    let up = c.box_trace(start, start + Vec3::new(0.0, 0.0, 128.0), mins, maxs);
    let down = c.box_trace(
        up.endpos,
        up.endpos - Vec3::new(0.0, 0.0, 262144.0),
        mins,
        maxs,
    );
    (down.endpos.into(), angles)
}

fn line_time(l: &str) -> i32 {
    l.split(' ').nth(2).unwrap().parse().unwrap()
}

/// The target's hits in each phase of the rounds part's barrel: shown,
/// hidden, deleted, the shown phase from [`SIGHT_MS`] past its start and
/// the deleted one for its first 3 s.
fn target_phases(lines: &[String]) -> [usize; 3] {
    let at = |p: &str| {
        lines
            .iter()
            .find(|l| l.starts_with("PROBE rphase") && l.split(' ').nth(3) == Some(p))
            .map(|l| line_time(l))
            .unwrap_or_else(|| panic!("no {p} phase in {lines:?}"))
    };
    let (shown, hidden, deleted) = (at("shown"), at("hidden"), at("deleted"));
    let mut out = [0; 3];
    for l in lines.iter().filter(|l| l.starts_with("PROBE hit")) {
        let t = line_time(l);
        if t < shown + SIGHT_MS || t >= deleted + 3000 {
            continue;
        }
        out[(t >= hidden) as usize + (t >= deleted) as usize] += 1;
    }
    out
}

/// mp_carentan on 1.1d: a barrel 26 units in front of a player stops every
/// Thompson round shown and hidden, none passes or carries through; deleted,
/// the rounds land.
#[test]
fn a_script_model_stops_rounds_until_deleted() {
    if vcod_common::testing::game_fs_11().is_none() {
        return;
    }
    let fixture = include_str!("fixtures/scriptmodel/mp_carentan-scriptmodel-rounds.txt");
    let retail = probe_lines(fixture.lines().map(str::to_string));
    let ours = probe_lines(ours_pair("rounds", false, 9000));
    report(&retail, &ours);
    let (r, o) = (target_phases(&retail), target_phases(&ours));
    assert_eq!((r[0], r[1]), (0, 0), "retail's barrel let rounds by: {r:?}");
    assert_eq!((o[0], o[1]), (0, 0), "ours' barrel let rounds by: {o:?}");
    assert!(
        r[2] > 10 && o[2].abs_diff(r[2]) * 5 <= r[2],
        "rounds past the deleted barrel in 3 s: retail {}, ours {}",
        r[2],
        o[2]
    );
}

/// Each grenade's path, frame by frame from its first, and every hit as
/// (team, damage) in order.
#[allow(clippy::type_complexity)]
fn throws(lines: &[String]) -> (Vec<Vec<[f32; 3]>>, Vec<(String, i32)>) {
    let mut paths: Vec<(String, Vec<[f32; 3]>)> = Vec::new();
    for l in lines.iter().filter_map(|l| l.strip_prefix("PROBE gren ")) {
        let (num, rest) = l.split_once(' ').unwrap().1.split_once(' ').unwrap();
        let v: Vec<f32> = rest
            .trim_matches(|c| c == '(' || c == ')')
            .split(", ")
            .map(|x| x.parse().unwrap())
            .collect();
        match paths.iter_mut().find(|(n, _)| n == num) {
            Some((_, p)) => p.push([v[0], v[1], v[2]]),
            None => paths.push((num.to_string(), vec![[v[0], v[1], v[2]]])),
        }
    }
    let hits = lines
        .iter()
        .filter(|l| l.starts_with("PROBE hit"))
        .map(|l| {
            let t: Vec<&str> = l.split(' ').collect();
            (t[3].to_string(), t[5].parse().unwrap())
        })
        .collect();
    (paths.into_iter().map(|(_, p)| p).collect(), hits)
}

/// Retail's paths and hits against ours: every frame within 0.1 of
/// retail's (a unit once the explode snaps it), every hit within 2. A frag
/// off the target's bones reads his idle clip's pose: two retail runs part
/// by 0.01 there.
fn compare_throws(fixture: &str, ours: Vec<String>) {
    compare_throws_within(fixture, ours, 0.1);
}

/// [`compare_throws`] with `near` for the frames before the explode.
fn compare_throws_within(fixture: &str, ours: Vec<String>, near: f32) {
    let retail = probe_lines(fixture.lines().map(str::to_string));
    let ours = probe_lines(ours);
    report(&retail, &ours);
    let ((rp, rh), (op, oh)) = (throws(&retail), throws(&ours));
    assert_eq!(rp.len(), 2, "{retail:?}");
    assert_eq!(op.len(), rp.len(), "{ours:?}");
    for (g, (r, o)) in rp.iter().zip(&op).enumerate() {
        assert!(r.len().abs_diff(o.len()) <= 1, "grenade {g}: {r:?} {o:?}");
        for (f, (a, b)) in r.iter().zip(o).enumerate() {
            let snapped = a.iter().all(|v| v.fract() == 0.0);
            let tol = if snapped { 1.0f32.max(near) } else { near };
            assert!(
                (0..3).all(|i| (a[i] - b[i]).abs() <= tol),
                "grenade {g} frame {f}: retail {a:?}, ours {b:?}"
            );
        }
    }
    assert_eq!(oh.len(), rh.len(), "retail {rh:?}, ours {oh:?}");
    for (r, o) in rh.iter().zip(&oh) {
        assert!(
            r.0 == o.0 && (r.1 - o.1).abs() <= 2,
            "retail {rh:?}, ours {oh:?}"
        );
    }
}

/// mp_carentan on 1.1d: a frag thrown at a barrel bounces off its mesh;
/// thrown again with the barrel gone it strikes the target behind, whose
/// posed bones it bounces off at an eighth of its speed
/// (docs/research/cod11-combat.md 12.4).
#[test]
fn a_frag_bounces_off_a_script_model_and_a_players_bones() {
    if vcod_common::testing::game_fs_11().is_none() {
        return;
    }
    compare_throws(
        include_str!("fixtures/scriptmodel/mp_carentan-scriptmodel-grenade.txt"),
        ours_pair("grenade", true, 16_000),
    );
}

/// mp_carentan on 1.1d: a barrel between a resting frag and both players
/// keeps the whole blast off them; the same blast in the open hits both.
#[test]
fn a_script_model_shelters_players_from_a_blast() {
    if vcod_common::testing::game_fs_11().is_none() {
        return;
    }
    compare_throws(
        include_str!("fixtures/scriptmodel/mp_carentan-scriptmodel-blast.txt"),
        ours_pair("blast", true, 12_000),
    );
}

/// One snapshot's missile: trType, `trBase` and groundEntityNum.
type MissileRow = (i32, [f32; 3], i32);

/// Each grenade's `VEH missile` lines per snapshot, in order of first
/// appearance.
fn missile_grounds(lines: &[String]) -> Vec<Vec<MissileRow>> {
    let mut out: Vec<(String, Vec<MissileRow>)> = Vec::new();
    for l in lines
        .iter()
        .filter(|l| l.starts_with("VEH ") && l.contains(" missile "))
    {
        let t: Vec<&str> = l.split(' ').collect();
        let after = |k: &str, n: usize| t[t.iter().position(|x| *x == k).unwrap() + n];
        let bits = |n| {
            let raw = after("bits", n).trim_start_matches("0x");
            f32::from_bits(u32::from_str_radix(raw, 16).unwrap())
        };
        let row = (
            after("type", 1).parse().unwrap(),
            [bits(1), bits(2), bits(3)],
            after("ground", 1).parse().unwrap(),
        );
        let num = t[3].to_string();
        match out.iter_mut().find(|(n, _)| *n == num) {
            Some((_, v)) => v.push(row),
            None => out.push((num, vec![row])),
        }
    }
    out.into_iter().map(|(_, v)| v).collect()
}

/// How far ours' frames may sit from retail's off a prone body: his pose
/// strikes half a unit off retail's.
const PRONE_NEAR: f32 = 1.0;

/// mp_carentan on 1.1d: a frag thrown onto a prone player's back rests
/// there with his entity number as its ground (`G_BounceMissile` writes
/// `tr.entityNum`); the second rests on the floor where he lay
/// (docs/research/cod11-combat.md 12.4).
#[test]
fn a_frag_rests_on_a_prone_players_back() {
    if vcod_common::testing::game_fs_11().is_none() {
        return;
    }
    let fixture = include_str!("fixtures/scriptmodel/mp_carentan-scriptmodel-prone.txt");
    // The site's yaw (45) turned round, as the recipe's `--probe-fall-prone`.
    let ours = ours_pair_on("mp_carentan", "prone", true, 12_000, Some(225.0));
    let retail: Vec<String> = fixture.lines().map(str::to_string).collect();
    compare_throws_within(fixture, ours.clone(), PRONE_NEAR);
    let (r, o) = (missile_grounds(&retail), missile_grounds(&ours));
    if std::env::var_os("SM_REPORT").is_some() {
        eprintln!("retail {r:?}\nours   {o:?}");
    }
    assert_eq!(r.len(), 2);
    assert_eq!(o.len(), r.len(), "{o:?}");
    for (g, (r, o)) in r.iter().zip(&o).enumerate() {
        let rest = |v: &[MissileRow]| *v.last().unwrap();
        let (rr, or) = (rest(r), rest(o));
        assert_eq!(or.0, 0, "grenade {g} never came to rest: {o:?}");
        assert_eq!(
            or.2, rr.2,
            "grenade {g}'s ground: retail {rr:?}, ours {or:?}"
        );
    }
}

/// mp_brecourt on 1.1d, the target lying still on the field's grade: the
/// first frag strikes his back on retail's frame and rests on him within
/// [`PRONE_NEAR`] of retail's spot, his number its ground; the second rolls
/// down the terrain (docs/research/cod11-combat.md 12.4,
/// player-model-anim-system.md "The prone body on a grade").
#[test]
fn a_frag_rests_on_a_prone_players_back_on_a_grade() {
    if vcod_common::testing::game_fs_11().is_none() {
        return;
    }
    let fixture = include_str!("fixtures/scriptmodel/mp_brecourt-scriptmodel-prone-rest.txt");
    let ours = ours_pair_on("mp_brecourt", "prone", true, 12_000, Some(270.0));
    let retail: Vec<String> = fixture.lines().map(str::to_string).collect();
    compare_throws_within(fixture, ours.clone(), PRONE_NEAR);
    let (r, o) = (missile_grounds(&retail), missile_grounds(&ours));
    if std::env::var_os("SM_REPORT").is_some() {
        eprintln!("retail {r:?}\nours   {o:?}");
    }
    assert_eq!(o.len(), r.len(), "{o:?}");
    for (g, ground) in [(0, 0), (1, 1022)] {
        let (rr, or) = (*r[g].last().unwrap(), *o[g].last().unwrap());
        assert_eq!((rr.0, rr.2), (0, ground), "retail's grenade {g}: {rr:?}");
        assert_eq!((or.0, or.2), (0, ground), "ours' grenade {g}: {or:?}");
        let off = (0..3)
            .map(|i| (rr.1[i] - or.1[i]).abs())
            .fold(0.0, f32::max);
        assert!(
            off <= PRONE_NEAR,
            "grenade {g} rests {off} off: {rr:?} {or:?}"
        );
    }
}

/// mp_brecourt on 1.1d: the prone part's second frag bounces down the
/// field's terrain and rolls to rest on it, each contact on retail's
/// snapshot with the world as its ground, every base retail's to the bit:
/// the point clip (0x8052894), `G_BounceMissile` and the gravity arc keep
/// 1.1d's x87 arithmetic (12.4, 12.5). The first rests on the target's back, his
/// number its ground; the fixture was taken while the part dropped him onto
/// the grade every frame, which put that frag where the cmd timing gave, so
/// only its ground is compared (docs/research/cod11-combat.md 12.4, 12.5).
#[test]
fn a_frag_rolls_down_terrain_like_retail() {
    if vcod_common::testing::game_fs_11().is_none() {
        return;
    }
    let fixture = include_str!("fixtures/scriptmodel/mp_brecourt-scriptmodel-prone.txt");
    // The site's yaw (90) turned round.
    let ours = ours_pair_on("mp_brecourt", "prone", true, 12_000, Some(270.0));
    let retail: Vec<String> = fixture.lines().map(str::to_string).collect();
    let (r, o) = (missile_grounds(&retail), missile_grounds(&ours));
    if std::env::var_os("SM_REPORT").is_some() {
        eprintln!("retail {r:?}\nours   {o:?}");
    }
    assert_eq!(r.len(), 2);
    assert_eq!(o.len(), r.len(), "{o:?}");
    assert_eq!(o[0].last().map(|g| g.2), r[0].last().map(|g| g.2));
    // Each distinct snapshot state: a resting frag repeats its last.
    let changes = |v: &[MissileRow]| {
        let mut out: Vec<MissileRow> = Vec::new();
        for &g in v {
            if out.last() != Some(&g) {
                out.push(g);
            }
        }
        out
    };
    let (rc, oc) = (changes(&r[1]), changes(&o[1]));
    assert_eq!(oc.len(), rc.len(), "retail {rc:?}\nours   {oc:?}");
    let bits = |v: &[MissileRow]| -> Vec<_> {
        v.iter()
            .map(|g| (g.0, g.1.map(f32::to_bits), g.2))
            .collect()
    };
    assert_eq!(bits(&oc), bits(&rc), "retail {rc:?}\nours   {oc:?}");
}

/// mp_brecourt on 1.5b: both frags miss the prone target and bounce up
/// the field's terrain to rest on it, every contact on retail's snapshot
/// with retail's ground and every base retail's to the bit: the arc, the
/// bounce and the ground snap take 1.5b's float stores (12.5, "The
/// arithmetic on 1.5 and UO"). The thrower spawns on the spawn point under
/// retail's first `PROBE thrower` line. Runs only with `COD_DIR` at a 1.5
/// install.
#[test]
fn a_frag_rolls_up_terrain_like_retail_on_15() {
    let Some(fs) = vcod_common::testing::game_fs() else {
        return;
    };
    if vcod_server::server::install_protocol(&fs).transport.version != PROTOCOL_V6.transport.version
    {
        return;
    }
    compare_rolls(
        fs,
        include_str!("fixtures/scriptmodel/mp_brecourt-scriptmodel-roll-1.5.txt"),
        None,
    );
}

/// [`a_frag_rolls_up_terrain_like_retail_on_15`] on United Offensive
/// 1.51b, whose bounce keeps 0.4 of the reflection: the second frag rolls
/// up the grade over four contacts to retail's bit. The first strikes the
/// target's prone body on the grade before the terrain, and that strike
/// lands 0.007 off retail's: this capture logged neither client's cmds,
/// and his body rides where his drop onto the grade settled
/// (cod11-combat.md 3.1, "The pose on 1.5 and UO"), so its states are held
/// to retail's type and ground and to 0.01.
/// [`a_frag_strikes_a_prone_body_on_a_grade_like_retail_on_uo`] replays a
/// rerun's cmds to the bit. Runs only with `COD_DIR` at a UO install.
#[test]
fn a_frag_rolls_up_terrain_like_retail_on_uo() {
    let Some(fs) = vcod_common::testing::uo_fs() else {
        return;
    };
    compare_rolls(
        fs,
        include_str!("fixtures/scriptmodel/mp_brecourt-scriptmodel-roll-uo.txt"),
        Some(0),
    );
}

/// [`a_frag_rolls_up_terrain_like_retail_on_uo`] on a capture that logged
/// the target's cmds: with his drop replayed on them, his prone body lies
/// on retail's bits (`pronepose_ab`'s `probe_rays` gates), the first frag
/// strikes his legs on retail's float and rests on the grade by them, and
/// the second rolls up the grade to retail's bit, every state of both.
#[test]
fn a_frag_strikes_a_prone_body_on_a_grade_like_retail_on_uo() {
    let Some(fs) = vcod_common::testing::uo_fs() else {
        return;
    };
    compare_rolls(
        fs,
        include_str!("fixtures/scriptmodel/mp_brecourt-scriptmodel-roll-uo-cmds.txt"),
        None,
    );
}

/// The prone part on mp_brecourt against `fixture`: each frag's distinct
/// snapshot states up to its explode (`trType`, base bits, ground)
/// retail's, in order; `struck`'s states only within 0.01.
fn compare_rolls(fs: vcod_common::pk3::Pk3Fs, fixture: &str, struck: Option<usize>) {
    // Ours logs a missile until its explode.
    let retail: Vec<String> = fixture
        .lines()
        .filter(|l| !l.contains(" etype 0 "))
        .map(str::to_string)
        .collect();
    let landed = thrower_lines(&retail)
        .first()
        .expect("a PROBE thrower line")
        .0;
    // The site's yaw (90) turned round.
    let ours = ours_pair_in(
        fs,
        "mp_brecourt",
        "prone",
        true,
        12_000,
        Some(270.0),
        Some(landed),
        fixture.contains("target CMDS").then_some(fixture),
    );
    report(&retail, &ours);
    let (r, o) = (missile_grounds(&retail), missile_grounds(&ours));
    assert_eq!(r.len(), 2);
    assert_eq!(o.len(), r.len(), "{o:?}");
    let changes = |v: &[MissileRow]| {
        let mut out: Vec<(i32, [u32; 3], i32)> = Vec::new();
        for g in v {
            let row = (g.0, g.1.map(f32::to_bits), g.2);
            if out.last() != Some(&row) {
                out.push(row);
            }
        }
        out
    };
    for g in 0..2 {
        let (rc, oc) = (changes(&r[g]), changes(&o[g]));
        if struck != Some(g) {
            assert_eq!(oc, rc, "grenade {g}: retail {:?}\nours   {:?}", r[g], o[g]);
            continue;
        }
        assert_eq!(
            oc.len(),
            rc.len(),
            "grenade {g}: retail {rc:?}\nours   {oc:?}"
        );
        for (a, b) in rc.iter().zip(&oc) {
            let near =
                (0..3).all(|i| (f32::from_bits(a.1[i]) - f32::from_bits(b.1[i])).abs() <= 0.01);
            assert!(
                (a.0, a.2) == (b.0, b.2) && near,
                "grenade {g}: retail {:?}\nours   {:?}",
                r[g],
                o[g]
            );
        }
    }
}

/// Each `PROBE shooter` line's thrower origin, in order.
fn shooter_origins(lines: &[String]) -> Vec<[f32; 3]> {
    lines
        .iter()
        .filter_map(|l| l.strip_prefix("PROBE shooter "))
        .map(|l| {
            let v: Vec<f32> = l.split(' ').skip(2).map(|x| x.parse().unwrap()).collect();
            [v[0], v[1], v[2]]
        })
        .collect()
}

/// mp_carentan on 1.5b: each frag leaves from the thrower's unsnapped
/// origin plus the eye's 60, untruncated, where 1.1d truncates the origin
/// and the point (docs/research/cod11-combat.md 2.1, "1.5 and United
/// Offensive"). The thrower spawns where retail's did, lands on its grade
/// moving and slides off the site to retail's float (2.1, "The thrower's
/// slide"), so both frags leave from retail's point. Each frag rests with
/// retail's ground (12.6). Runs only with `COD_DIR` at a 1.5 install.
#[test]
fn a_frag_leaves_15s_untruncated_muzzle() {
    let Some(fs) = vcod_common::testing::game_fs() else {
        return;
    };
    if vcod_server::server::install_protocol(&fs).transport.version != PROTOCOL_V6.transport.version
    {
        return;
    }
    compare_throw_starts(
        fs,
        include_str!("fixtures/scriptmodel/mp_carentan-scriptmodel-throw-1.5.txt"),
    );
}

/// Where a thrower's spawn left him on retail, for the captures that log
/// no `PROBE thrower` line, read off the throw captures' runs
/// (cod11-combat.md 2.1, "The thrower's slide"): 1.5b's spawn at (976,
/// -864) lands moving and slides him to (1368.03, 2608.14, -23.8748), as
/// `-prone-rest-1.5.txt`'s `PROBE shooter` reads; a level spawn lands him
/// still, and he settles at -23.852, as `-prone-1.5.txt`'s and
/// `-prone-rest-uo.txt`'s frags (36.148 up) and shooter line read.
const SLIDING_SPAWN_15: [f32; 3] = [976.016, -863.939, -5.89044];
const LEVEL_SPAWN_15: [f32; 3] = [1080.0, 3488.0, -23.875];
const LEVEL_SPAWN_UO: [f32; 3] = [1800.0, 2696.0, -23.875];

/// mp_carentan on 1.5b, the target lying still with the body his drop
/// left (cod15-movement.md 4, "The airborne prone body"): the first frag
/// strikes his back and comes to rest on it, his number its ground, each
/// on retail's bits, not snapped 1.5 up onto his bones
/// as 1.1's would be (12.2); moved off, he leaves it to 1.5's fall test,
/// which drops it to the floor (12.6), where it bounces at 1.5's half of
/// the reflection (12.4) and rests. Every snapshot of both frags carries
/// retail's `trType` and ground and retail's base. The older capture,
/// whose target's body went unlogged, holds the same. Runs only with `COD_DIR` at a 1.5
/// install.
#[test]
fn a_frag_rests_on_a_prone_players_back_on_15() {
    for (fixture, thrower_at) in [
        (
            include_str!("fixtures/scriptmodel/mp_carentan-scriptmodel-prone-rest-1.5.txt"),
            SLIDING_SPAWN_15,
        ),
        (
            include_str!("fixtures/scriptmodel/mp_carentan-scriptmodel-prone-1.5.txt"),
            LEVEL_SPAWN_15,
        ),
    ] {
        let Some(fs) = vcod_common::testing::game_fs() else {
            return;
        };
        if vcod_server::server::install_protocol(&fs).transport.version
            != PROTOCOL_V6.transport.version
        {
            return;
        }
        prone_back_gate(fs, fixture, thrower_at, true);
    }
}

/// [`a_frag_rests_on_a_prone_players_back_on_15`] on a capture that logged
/// both clients' cmds: with the target's drop and the thrower's cmds
/// replayed, every snapshot of both frags carries retail's `trType`,
/// ground and base bits, the strike on him too.
#[test]
fn a_frag_rests_on_a_prone_players_back_like_retail_on_15() {
    let Some(fs) = vcod_common::testing::game_fs() else {
        return;
    };
    if vcod_server::server::install_protocol(&fs).transport.version != PROTOCOL_V6.transport.version
    {
        return;
    }
    let fixture =
        include_str!("fixtures/scriptmodel/mp_carentan-scriptmodel-prone-rest-1.5-cmds.txt");
    prone_back_gate(fs, fixture, landed(fixture), true);
}

/// [`a_frag_strikes_a_prone_players_back_on_uo`] on a capture that logged
/// both clients' cmds.
#[test]
fn a_frag_strikes_a_prone_players_back_like_retail_on_uo() {
    let Some(fs) = vcod_common::testing::uo_fs() else {
        return;
    };
    let fixture =
        include_str!("fixtures/scriptmodel/mp_carentan-scriptmodel-prone-rest-uo-cmds.txt");
    prone_back_gate(fs, fixture, landed(fixture), false);
}

/// Where a fixture's thrower spawned: its first `PROBE thrower` origin.
fn landed(fixture: &str) -> [f32; 3] {
    let lines: Vec<String> = fixture.lines().map(str::to_string).collect();
    thrower_lines(&lines)
        .first()
        .expect("a PROBE thrower line")
        .0
}

/// [`a_frag_rests_on_a_prone_players_back_on_15`] on United Offensive
/// 1.51b: the frag strikes his back on retail's spot and, its bounce off a
/// body keeping a tenth of the reflection (coduo-diff.md 21), strikes him
/// again lower down and slides off to the floor without resting on him. Runs only with `COD_DIR` at a UO
/// install.
#[test]
fn a_frag_strikes_a_prone_players_back_on_uo() {
    let Some(fs) = vcod_common::testing::uo_fs() else {
        return;
    };
    prone_back_gate(
        fs,
        include_str!("fixtures/scriptmodel/mp_carentan-scriptmodel-prone-rest-uo.txt"),
        LEVEL_SPAWN_UO,
        false,
    );
}

/// The prone part on `fs`'s install against `fixture`: the first frag's
/// strike on the target (his number its ground), with `rests` its rest on
/// him, and its first four bases, each retail's bits; both frags end on
/// the floor, and with `rests` every snapshot of both carries retail's
/// `trType`, ground and base.
fn prone_back_gate(fs: vcod_common::pk3::Pk3Fs, fixture: &str, thrower_at: [f32; 3], rests: bool) {
    let ours = ours_pair_in(
        fs,
        "mp_carentan",
        "prone",
        true,
        12_000,
        Some(225.0),
        Some(thrower_at),
        fixture.contains("target CMDS").then_some(fixture),
    );
    // Ours logs a missile until its explode.
    let retail: Vec<String> = fixture
        .lines()
        .filter(|l| !l.contains(" etype 0 "))
        .map(str::to_string)
        .collect();
    report(&retail, &ours);
    let (r, o) = (missile_grounds(&retail), missile_grounds(&ours));
    if std::env::var_os("SM_REPORT").is_some() {
        eprintln!("retail {r:?}\nours   {o:?}");
    }
    assert_eq!((r.len(), o.len()), (2, 2), "{o:?}");
    // How many float steps apart two bases lie, the worst axis.
    let off = |a: [f32; 3], b: [f32; 3]| {
        (0..3)
            .map(|i| (a[i].to_bits() as i64 - b[i].to_bits() as i64).unsigned_abs() as u32)
            .max()
            .unwrap()
    };
    let on_him = |v: &[MissileRow], rest: bool| {
        v.iter()
            .find(|g| g.2 == 0 && (g.0 == 0) == rest)
            .map(|g| g.1)
    };
    for rest in [false, true] {
        let (rb, ob) = (on_him(&r[0], rest), on_him(&o[0], rest));
        if !rest || rests {
            let (Some(rb), Some(ob)) = (rb, ob) else {
                panic!("rest {rest}: retail {rb:?}, ours {ob:?}: {:?}", o[0]);
            };
            assert!(
                off(rb, ob) == 0,
                "rest {rest}: retail {rb:?}, ours {ob:?}, {} steps off",
                off(rb, ob)
            );
        } else {
            assert_eq!((rb, ob), (None, None), "a rest on him");
        }
    }
    // The first frag's first four bases: the throw, the strike on him, then
    // the rest on him (1.5) or the second strike lower down (UO), then the
    // next.
    let bases = |v: &[MissileRow]| {
        let mut out: Vec<[f32; 3]> = Vec::new();
        for g in v {
            if out.last() != Some(&g.1) {
                out.push(g.1);
            }
        }
        out.truncate(4);
        out
    };
    let (rb, ob) = (bases(&r[0]), bases(&o[0]));
    assert_eq!(rb.len(), ob.len());
    for (a, b) in rb.iter().zip(&ob) {
        assert!(off(*a, *b) == 0, "retail {rb:?}\nours   {ob:?}");
    }
    for g in 0..2 {
        let (rr, or) = (*r[g].last().unwrap(), *o[g].last().unwrap());
        assert_eq!((rr.0, rr.2), (0, 1022), "retail's grenade {g}: {rr:?}");
        assert_eq!((or.0, or.2), (0, 1022), "ours' grenade {g}: {or:?}");
    }
    if rests {
        // 1.5: every snapshot of both frags, the fall off his back and each
        // floor and wall bounce after it, on retail's frame with retail's
        // `trType` and ground (12.4, "1.5 and United Offensive").
        for g in 0..2 {
            assert_eq!(r[g].len(), o[g].len(), "grenade {g}'s snapshots");
            for (s, (a, b)) in r[g].iter().zip(&o[g]).enumerate() {
                assert!(
                    a.0 == b.0 && a.2 == b.2 && off(a.1, b.1) == 0,
                    "grenade {g} snapshot {s}: retail {a:?}, ours {b:?}"
                );
            }
        }
    }
    if std::env::var_os("SM_REPORT").is_some() {
        let worst = r[0]
            .iter()
            .chain(&r[1])
            .zip(o[0].iter().chain(&o[1]))
            .map(|(a, b)| off(a.1, b.1))
            .max();
        eprintln!("worst {worst:?} steps");
    }
}

/// [`a_frag_leaves_15s_untruncated_muzzle`] on United Offensive 1.51b.
/// Runs only with `COD_DIR` at a UO install.
#[test]
fn a_frag_leaves_uos_untruncated_muzzle() {
    let Some(fs) = vcod_common::testing::uo_fs() else {
        return;
    };
    compare_throw_starts(
        fs,
        include_str!("fixtures/scriptmodel/mp_carentan-scriptmodel-throw-uo.txt"),
    );
}

/// Each `PROBE thrower` line's origin and its offset from the site in
/// thousandths, in order.
fn thrower_lines(lines: &[String]) -> Vec<([f32; 3], [f32; 3])> {
    lines
        .iter()
        .filter_map(|l| l.strip_prefix("PROBE thrower "))
        .map(|l| {
            let v: Vec<f32> = l.split(' ').skip(1).map(|x| x.parse().unwrap()).collect();
            ([v[0], v[1], v[2]], [v[3], v[4], v[5]])
        })
        .collect()
}

/// The prone part on `fs`'s install against `fixture`, the thrower spawned
/// on the spawn point under retail's first `PROBE thrower` line: every
/// frame of his first second on retail's float, both frags' first base on
/// retail's (each side's his origin plus 60), and where each comes to rest.
fn compare_throw_starts(fs: vcod_common::pk3::Pk3Fs, fixture: &str) {
    let retail: Vec<String> = fixture.lines().map(str::to_string).collect();
    let rt = thrower_lines(&retail);
    let landed = rt.first().expect("a PROBE thrower line").0;
    let ours = ours_pair_in(
        fs,
        "mp_carentan",
        "prone",
        true,
        12_000,
        Some(225.0),
        Some(landed),
        None,
    );
    report(&retail, &ours);
    let (r, o) = (missile_grounds(&retail), missile_grounds(&ours));
    if std::env::var_os("SM_REPORT").is_some() {
        eprintln!("retail {r:?}\nours   {o:?}");
    }
    // The origins print at six significant digits, the offsets at six too.
    let near = |a: [f32; 3], b: [f32; 3], d: f32| (0..3).all(|i| (a[i] - b[i]).abs() <= d);
    let ot = thrower_lines(&ours);
    assert_eq!(ot.len(), rt.len(), "thrower frames: ours {ot:?}");
    assert!(
        near(ot[0].0, rt[0].0, 0.001),
        "the spawn's landing: retail {:?}, ours {:?}",
        rt[0].0,
        ot[0].0
    );
    for (f, (a, b)) in rt.iter().zip(&ot).enumerate().skip(1) {
        assert!(
            near(a.1, b.1, 0.002),
            "thrower frame {f}, off the site in thousandths: retail {:?}, ours {:?}",
            a.1,
            b.1
        );
    }
    let (rs, os) = (shooter_origins(&retail), shooter_origins(&ours));
    assert_eq!(r.len(), 2);
    assert_eq!(o.len(), r.len(), "{o:?}");
    assert_eq!((rs.len(), os.len()), (2, 2), "retail {rs:?}, ours {os:?}");
    for g in 0..2 {
        let (rb, ob) = (r[g][0].1, o[g][0].1);
        let eye = |s: [f32; 3]| [s[0], s[1], s[2] + 60.0];
        assert!(
            near(rb, eye(rs[g]), 0.01),
            "retail grenade {g}: {rb:?} from {:?}",
            rs[g]
        );
        assert!(
            near(ob, eye(os[g]), 0.01),
            "ours grenade {g}: {ob:?} from {:?}",
            os[g]
        );
        assert_eq!(ob, rb, "grenade {g}'s start");
        let (rr, or) = (*r[g].last().unwrap(), *o[g].last().unwrap());
        assert_eq!(or.0, 0, "grenade {g} never came to rest: {:?}", o[g]);
        assert_eq!(
            or.2, rr.2,
            "grenade {g}'s ground: retail {rr:?}, ours {or:?}"
        );
    }
}
