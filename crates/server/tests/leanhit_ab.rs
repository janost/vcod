//! A leaning target's hit pose and a leaning shooter's rounds, ours against
//! retail 1.1d's. `client-probes/probe_leanhit` holds an allies shooter and
//! an axis target 256 units apart and drives both clients' stance, lean and
//! trigger through `PROBE_IN` lines (`--probe-uo-weapon direct`). Each
//! phase it crosses the target with a grid of `bulletTrace`s, whose cells
//! read the posed body (docs/research/cod11-combat.md 3, 16.4), then the
//! sighted springfield fires once from the shooter's eye, leaned in the
//! last three phases (2.1); the damage callback logs where it struck.
//!
//! `probe_bones 1` captures also link a mark to eleven bones before each
//! grid and log them with the body's origin, which splits the pose from
//! the boxes. One capture each from 1.1d, 1.5b and UO with them, and the
//! first 1.1d one without.
//!
//! `LH_REPORT=1` prints both logs and each phase's median cell. Needs
//! `COD_DIR` at a 1.1, 1.5 or UO install.

use crate::common;

use common::{Join, Queues};
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;
use std::time::{Duration, Instant};
use vcod_common::net::msg::UserCmd;
use vcod_common::net::{NetClient, NetEvent};

const SCRIPT: &str =
    include_str!("../../gsc/tests/fixtures/semantics/client-probes/probe_leanhit.gsc");

/// How far a grid cell may sit from retail's, units along the ray.
const CELL_NEAR: f32 = 0.5;
/// How far a bone or the body's origin may sit from retail's, units on
/// each axis; retail logs an offset to six figures.
const BONE_NEAR: f32 = 0.005;
/// How far a round's point may sit from retail's: the scoped idle sway
/// moves 0.45 degrees a second at most, and retail logs the point to the
/// hundredth.
const POINT_NEAR: f32 = 0.5;
/// How far a round's direction may sit from retail's, on each axis:
/// retail logs six figures, and the aim block's turns round as the game
/// module's `AngleVectors` (docs/research/cod11-movers.md 22, "pmove's
/// turns").
const DIR_NEAR: f32 = 0.0000003;

/// One retail capture of the probe and the join times its recipe gave.
struct Case {
    fixture: &'static str,
    /// Server time before ours' target connects, and how long after it the
    /// shooter does, set so ours' target spawns and takes each stance on
    /// retail's frames: his idle clip runs on the level time and his slot,
    /// and the setup's prone and stand wait for the shooter.
    join_ms: i64,
    late_ms: i64,
    /// The level's `holdrand` seed: the script's draws dress the target
    /// (`randomInt(3)` his variant, then `randomInt(20)` his head) as the
    /// capture's `PROBE models` line logs him. Retail's own seed is its
    /// clock (map-cycle doc 4.8); this is the smallest that draws the same.
    script_seed: u32,
}

/// Draws variant 0 (`load1`) and head `a_01`, the set a `probe_pin`
/// capture puts on the target anyway.
const SEED_LOAD1_A01: u32 = 1;

/// The join times of the `probe_bones` captures.
const JOIN_11: i64 = 8550;
const LATE_11: i64 = 2950;
const JOIN_UO: i64 = 8600;
const LATE_UO: i64 = 3000;
/// The `-cmds` captures' join times: each header's target snapshot less
/// 800 (1.1d), 1000 (1.5b) or 1150 (UO) ms, and the shooter's after it.
const CMDS_JOIN_11: i64 = 8350;
const CMDS_LATE_11: i64 = 3200;
const CMDS_JOIN_15: i64 = 9050;
const CMDS_LATE_15: i64 = 2700;
const CMDS_JOIN_UO: i64 = 9800;
const CMDS_LATE_UO: i64 = 2950;
/// The second 1.1d `-cmds` run's (`-cmds-ct`), joined the same way.
const CT_JOIN_11: i64 = 8100;
const CT_LATE_11: i64 = 3050;

/// The buttons, `wbuttons` and `up` of a `f "PROBE_IN ..."` command
/// (`vehicle_probe::direct_input`).
fn direct_input(tokens: &[String]) -> Option<(u8, u8, i8)> {
    if tokens.first().map(String::as_str) != Some("f") {
        return None;
    }
    let mut t = tokens.get(1)?.strip_prefix("PROBE_IN ")?.split_whitespace();
    Some((
        t.next()?.parse().ok()?,
        t.next()?.parse().ok()?,
        t.next()?.parse().ok()?,
    ))
}

fn ours(fs: vcod_common::pk3::Pk3Fs, c: &Case) -> Vec<String> {
    let proto = vcod_server::server::install_protocol(&fs);
    let map = "mp_carentan";
    let bsp_path = fs.resolve_map(map).expect("map in the paks");
    let bsp = vcod_common::bsp::parse(&fs.read(&bsp_path).unwrap()).unwrap();
    let mut now = Instant::now();
    let mut sv = vcod_server::Server::new(common::cfg(map, "probe_leanhit"), now);
    sv.set_protocol(proto);
    sv.overlay_script("maps/mp/gametypes/probe_leanhit", SCRIPT);
    // The recipe's `+set`s.
    for l in c.fixture.lines().filter(|l| l.starts_with('#')) {
        let mut t = l.split_whitespace();
        while let Some(w) = t.next() {
            if w == "+set"
                && let (Some(name), Some(value)) = (t.next(), t.next())
            {
                sv.set_cvar(name, value);
            }
        }
    }
    sv.load_world(vcod_server::world::World::from_bsp(&bsp, Some(&fs)));
    sv.load_scripts(Rc::new(fs)).expect("load the scripts");
    sv.pin_script_rand(c.script_seed);
    // The target connects first, into slot 0.
    let pin = |who: &str| spawn_at(c.fixture, who).map(Some).into_iter().collect();
    sv.pin_spawns(vec![pin("target"), pin("shooter")]);
    common::run_alone(&mut sv, &mut now, c.join_ms);
    let (qa, qb) = (
        Rc::new(RefCell::new(Queues::default())),
        Rc::new(RefCell::new(Queues::default())),
    );
    let mut ca: Option<NetClient<common::ClientEnd>> = None;
    let mut cb = NetClient::start_with_qport(common::ClientEnd(qb.clone()), now, 0x2002);
    let (mut ja, mut jb) = (
        Join::new("allies", "springfield_mp"),
        Join::new("axis", "kar98k_mp"),
    );
    let aim = || common::ProbeHold {
        aim: true,
        framed: true,
        ..Default::default()
    };
    let (mut ha, mut hb) = (aim(), aim());
    ha.times = common::cmds_times(c.fixture, "shooter");
    hb.times = common::cmds_times(c.fixture, "target");
    ha.ran = common::cmds_ran(c.fixture, "shooter");
    hb.ran = common::cmds_ran(c.fixture, "target");
    let (mut pa, mut pb) = (Vec::new(), Vec::new());
    if !ha.ran.is_empty() {
        // A capture that logs each snapshot's `commandTime` places every
        // cmd on retail's side of a frame, so it presses on the cmds too;
        // the others press on the `PROBE_IN` lines.
        pa = common::cmds_inputs(c.fixture, "shooter");
        pb = common::cmds_inputs(c.fixture, "target");
    }
    // Each client's last `PROBE_IN`, and the shooter's `a` select.
    let (mut ia, mut ib) = ((0u8, 0u8, 0i8), (0u8, 0u8, 0i8));
    let mut selected: Option<u8> = None;
    let mut spawned: Vec<&str> = Vec::new();
    let frame = Duration::from_millis(common::FRAME_MS as u64);
    let live = |cl: &NetClient<common::ClientEnd>| {
        cl.snapshots()
            .newest()
            .is_some_and(|s| s.ps.field_i32(cl.protocol(), "pm_type") == 0)
    };
    // Each cmd's place in retail's read order, keyed (0 target, 1 shooter;
    // their slots) and `serverTime`, where the capture logs it.
    let read: std::collections::HashMap<(usize, i32), usize> =
        common::cmds_read(c.fixture, &["target", "shooter"])
            .into_iter()
            .enumerate()
            .map(|(i, k)| (k, i))
            .collect();
    // A live client's cmds for this frame, unsent; a client not yet live
    // sends its frame here.
    let take = |cl: &mut NetClient<common::ClientEnd>,
                hold: &mut common::ProbeHold,
                input: (u8, u8, i8),
                pressed: &[(i32, (u8, u8, i8))],
                weapon: Option<u8>| {
        if live(cl) {
            let mut cmds = hold.cmds(cl, input.0);
            for c in &mut cmds {
                // The captured input of the cmd, where the capture logs it.
                let input = match pressed.partition_point(|e| e.0 <= c.server_time) {
                    0 if pressed.is_empty() => input,
                    0 => (0, 0, 0),
                    i => pressed[i - 1].1,
                };
                c.buttons = input.0;
                c.wbuttons = input.1;
                c.up = input.2;
                if let Some(w) = weapon {
                    c.weapon = w;
                }
            }
            Some(cmds)
        } else {
            if let Some(c) = cl.send_frame(&UserCmd::default()) {
                hold.last = c.server_time;
            }
            None
        }
    };
    for i in 0..4000i64 {
        if sv.script_log().iter().any(|l| l.contains("PROBE done")) {
            break;
        }
        now += frame;
        if ca.is_none() && i * common::FRAME_MS >= c.late_ms {
            ca = Some(NetClient::start_with_qport(
                common::ClientEnd(qa.clone()),
                now,
                0x2001,
            ));
        }
        if std::env::var_os("LH_REPORT").is_some() {
            for (name, cl) in [("shooter", ca.as_ref()), ("target", Some(&cb))] {
                if let Some(cl) = cl
                    && live(cl)
                    && !spawned.contains(&name)
                {
                    spawned.push(name);
                    let s = cl.snapshots().newest().unwrap();
                    let p = cl.protocol();
                    eprintln!(
                        "{name} live at {} origin {:?} view {:?}",
                        s.server_time,
                        s.ps.origin(p),
                        s.ps.viewangles(p)
                    );
                }
            }
        }
        let sa = ca
            .as_mut()
            .and_then(|ca| take(ca, &mut ha, ia, &pa, selected));
        let sb = take(&mut cb, &mut hb, ib, &pb, None);
        let mut batch: Vec<(usize, UserCmd)> = sb.iter().flatten().map(|c| (0, *c)).collect();
        batch.extend(sa.iter().flatten().map(|c| (1, *c)));
        if !batch.is_empty()
            && batch
                .iter()
                .all(|(k, c)| read.contains_key(&(*k, c.server_time)))
        {
            // One cmd a packet, as the net probe sent them, in the order
            // retail read them (docs/research/cod11-combat.md 3.7).
            batch.sort_by_key(|(k, c)| read[&(*k, c.server_time)]);
            for (k, cmd) in batch {
                let (cl, addr, q) = match k {
                    0 => (&mut cb, common::ADDR_B, &qb),
                    _ => (ca.as_mut().unwrap(), common::ADDR, &qa),
                };
                cl.send_cmds(&[cmd]);
                let pending: Vec<Vec<u8>> = q.borrow_mut().to_server.drain(..).collect();
                for p in pending {
                    sv.handle_packet(addr, &p, now);
                }
            }
        } else {
            // The shooter's packet first.
            if let (Some(ca), Some(cmds)) = (ca.as_mut(), &sa) {
                ca.send_cmds(cmds);
            }
            if let Some(cmds) = &sb {
                cb.send_cmds(cmds);
            }
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
                    if let Some(input) = direct_input(&tokens) {
                        ia = input;
                    }
                    ja.on_server_command(&tokens, ca, now);
                }
            }
        }
        for e in cb.pump_at(now) {
            if let NetEvent::ServerCommand(tokens) = e {
                if let Some(input) = direct_input(&tokens) {
                    ib = input;
                }
                jb.on_server_command(&tokens, &mut cb, now);
            }
        }
    }
    sv.script_log().to_vec()
}

/// A `PROBE spawnat` row's spawn point and angles for `who`.
fn spawn_at(fixture: &str, who: &str) -> Option<([f32; 3], [f32; 3])> {
    let l = fixture
        .lines()
        .filter_map(|l| l.strip_prefix("PROBE spawnat "))
        .find(|l| l.split(' ').nth(1) == Some(who))?;
    let v: Vec<f32> = l
        .split(' ')
        .skip(2)
        .map(|t| t.trim_matches(|c| c == '(' || c == ')' || c == ','))
        .map(|t| t.parse().unwrap())
        .collect();
    Some(([v[0], v[1], v[2]], [v[3], v[4], v[5]]))
}

fn probe_lines<'a>(lines: impl IntoIterator<Item = &'a str>) -> Vec<String> {
    lines
        .into_iter()
        .map(str::trim)
        .filter(|l| l.starts_with("PROBE"))
        .map(str::to_string)
        .collect()
}

/// Each `PROBE bone` and `PROBE body` row's offset off the target's spot,
/// keyed `(phase, tag, frame)`, the body's under the tag `origin` and the
/// shooter's `PROBE eye` off his under `eye`, the frame
/// counted from the phase's first row of that tag.
fn bone_rows(lines: &[String]) -> BTreeMap<(String, String, usize), [f32; 3]> {
    let mut out = BTreeMap::new();
    let mut seen: BTreeMap<(String, String), usize> = BTreeMap::new();
    for l in lines {
        let body = |r: &str, tag: &str| {
            let (t, rest) = r.split_once(' ').unwrap();
            let (phase, d) = rest.split_once(' ').unwrap();
            format!("{t} {phase} {tag} {d}")
        };
        let rest = if let Some(r) = l.strip_prefix("PROBE bone ") {
            r.to_string()
        } else if let Some(r) = l.strip_prefix("PROBE body ") {
            body(r, "origin")
        } else if let Some(r) = l.strip_prefix("PROBE eye ") {
            body(r, "eye")
        } else {
            continue;
        };
        let t: Vec<&str> = rest.split(' ').collect();
        let n = t.len();
        let tag = t[2..n - 3].join(" ");
        let d: Vec<f32> = t[n - 3..].iter().map(|v| v.parse().unwrap()).collect();
        let frame = seen.entry((t[1].into(), tag.clone())).or_default();
        out.insert((t[1].into(), tag, *frame), [d[0], d[1], d[2]]);
        *frame += 1;
    }
    out
}

/// Each `PROBE fall` row's height, keyed `(who, row)`.
fn fall_rows(lines: &[String]) -> BTreeMap<(String, usize), f32> {
    let mut out = BTreeMap::new();
    let mut seen: BTreeMap<String, usize> = BTreeMap::new();
    for l in lines {
        let Some(r) = l.strip_prefix("PROBE fall ") else {
            continue;
        };
        let t: Vec<&str> = r.split(' ').collect();
        let n = seen.entry(t[1].into()).or_default();
        out.insert((t[1].to_string(), *n), t[2].parse().unwrap());
        *n += 1;
    }
    out
}

/// Each phase's grid rows, keyed `(phase, view, height)`.
fn grids(lines: &[String]) -> BTreeMap<(String, String, String), Vec<Option<f32>>> {
    let mut out = BTreeMap::new();
    for l in lines {
        let t: Vec<&str> = l.split(' ').collect();
        if t[1] != "grid" {
            continue;
        }
        let cells = t[6..].iter().map(|c| c.parse().ok()).collect();
        out.insert((t[3].into(), t[4].into(), t[5].into()), cells);
    }
    out
}

/// The round each phase landed: location, damage and point.
fn hits(lines: &[String]) -> BTreeMap<String, (String, String, [f32; 3])> {
    let mut out = BTreeMap::new();
    for l in lines {
        let Some(rest) = l.strip_prefix("PROBE hit ") else {
            continue;
        };
        let t: Vec<&str> = rest.splitn(8, ' ').collect();
        let point: Vec<f32> = t[7]
            .trim_start_matches("point ")
            .trim_matches(|c| c == '(' || c == ')')
            .split(", ")
            .map(|c| c.parse().unwrap())
            .collect();
        out.insert(
            t[1].to_string(),
            (
                t[6].to_string(),
                t[4].to_string(),
                [point[0], point[1], point[2]],
            ),
        );
    }
    out
}

/// Each round's `PROBE dir` direction, by phase.
fn dirs(lines: &[String]) -> BTreeMap<String, [f32; 3]> {
    let mut out = BTreeMap::new();
    for l in lines {
        let Some(r) = l.strip_prefix("PROBE dir ") else {
            continue;
        };
        let t: Vec<&str> = r.split(' ').collect();
        let d: Vec<f32> = t[2..5].iter().map(|v| v.parse().unwrap()).collect();
        out.insert(t[1].to_string(), [d[0], d[1], d[2]]);
    }
    out
}

fn report(retail: &[String], ours: &[String]) {
    if std::env::var_os("LH_REPORT").is_some() {
        eprintln!("retail:\n{}\nours:\n{}", retail.join("\n"), ours.join("\n"));
    }
}

/// Each phase's median cell distance from retail's, for `LH_REPORT`.
fn report_cells(
    rg: &BTreeMap<(String, String, String), Vec<Option<f32>>>,
    og: &BTreeMap<(String, String, String), Vec<Option<f32>>>,
) {
    if std::env::var_os("LH_REPORT").is_none() {
        return;
    }
    let mut per: BTreeMap<&str, Vec<f32>> = BTreeMap::new();
    for (k, r) in rg {
        let d = per.entry(&k.0).or_default();
        for (r, o) in r.iter().zip(&og[k]) {
            if let (Some(r), Some(o)) = (r, o) {
                d.push((r - o).abs());
            }
        }
    }
    for (phase, mut d) in per {
        d.sort_by(f32::total_cmp);
        eprintln!("{phase:16} median cell {:.4}", d[d.len() / 2]);
    }
}

fn check(fs: vcod_common::pk3::Pk3Fs, c: &Case) {
    let retail = probe_lines(c.fixture.lines());
    let ours = ours(fs, c);
    let ours = probe_lines(ours.iter().map(String::as_str));
    report(&retail, &ours);
    let site = |l: &[String]| l.iter().find(|l| l.starts_with("PROBE site")).cloned();
    let strip = |s: Option<String>| s.map(|s| s.splitn(4, ' ').nth(3).unwrap().to_string());
    assert_eq!(strip(site(&retail)), strip(site(&ours)), "the pair site");
    let models = |l: &[String]| l.iter().find(|l| l.starts_with("PROBE models")).cloned();
    if let Some(r) = models(&retail) {
        assert_eq!(Some(r), models(&ours), "the target's head and gear");
    }
    let mut bad = Vec::new();

    let (rb, mut ob) = (bone_rows(&retail), bone_rows(&ours));
    if !rb.keys().any(|k| k.1 == "eye") {
        ob.retain(|k, _| k.1 != "eye");
    }
    assert_eq!(
        rb.keys().collect::<Vec<_>>(),
        ob.keys().collect::<Vec<_>>(),
        "the bone rows"
    );
    for (k, r) in &rb {
        let o = ob[k];
        if (0..3).any(|i| (r[i] - o[i]).abs() > BONE_NEAR) {
            bad.push(format!("bone {k:?}: retail {r:?} ours {o:?}"));
        }
    }

    let (rf, of) = (fall_rows(&retail), fall_rows(&ours));
    for (k, r) in &rf {
        if of.get(k).is_none_or(|o| (r - o).abs() > BONE_NEAR) {
            bad.push(format!("fall {k:?}: retail {r} ours {:?}", of.get(k)));
        }
    }

    let (rg, og) = (grids(&retail), grids(&ours));
    assert_eq!(
        rg.keys().collect::<Vec<_>>(),
        og.keys().collect::<Vec<_>>(),
        "the grid rows"
    );
    report_cells(&rg, &og);
    for (k, r) in &rg {
        let o = &og[k];
        let off = r.iter().zip(o).any(|(r, o)| match (r, o) {
            (Some(r), Some(o)) => (r - o).abs() > CELL_NEAR,
            (None, None) => false,
            _ => true,
        });
        if off {
            bad.push(format!("cells {k:?}:\n  retail {r:?}\n  ours   {o:?}"));
        }
    }

    let (rh, oh) = (hits(&retail), hits(&ours));
    for (phase, r) in &rh {
        let o = oh.get(phase);
        let near = o.is_some_and(|o| {
            o.0 == r.0 && o.1 == r.1 && (0..3).all(|i| (o.2[i] - r.2[i]).abs() <= POINT_NEAR)
        });
        if !near {
            bad.push(format!("round {phase}: retail {r:?} ours {o:?}"));
        }
    }
    let (rd, od) = (dirs(&retail), dirs(&ours));
    for (phase, r) in &rd {
        let o = od.get(phase).copied();
        if o.is_none_or(|o| (0..3).any(|i| (o[i] - r[i]).abs() > DIR_NEAR)) {
            bad.push(format!("round {phase} direction: retail {r:?} ours {o:?}"));
        }
    }
    for phase in oh.keys().filter(|p| !rh.contains_key(*p)) {
        bad.push(format!(
            "round {phase}: ours hit {:?}, retail none",
            oh[phase]
        ));
    }
    assert!(bad.is_empty(), "off retail's:\n{}", bad.join("\n"));
}

#[test]
fn a_leaning_body_and_a_leaning_shooters_rounds_land_as_on_retail() {
    let Some(fs) = vcod_common::testing::game_fs_11() else {
        return;
    };
    check(
        fs,
        &Case {
            fixture: include_str!("fixtures/leanhit/mp_carentan-leanhit.txt"),
            join_ms: 8550,
            late_ms: 3450,
            // Logs no models; the 1.1d captures that do wear `load1`.
            script_seed: SEED_LOAD1_A01,
        },
    );
}

#[test]
fn a_leaning_bodys_bones_sit_as_on_retail() {
    let Some(fs) = vcod_common::testing::game_fs_11() else {
        return;
    };
    check(
        fs,
        &Case {
            fixture: include_str!("fixtures/leanhit/mp_carentan-leanhit-bones.txt"),
            join_ms: JOIN_11,
            late_ms: LATE_11,
            script_seed: SEED_LOAD1_A01,
        },
    );
}

/// The 1.5b install's paks, `None` elsewhere (a UO install's `main/` is
/// 1.5's, so this runs there too).
fn fs_15() -> Option<vcod_common::pk3::Pk3Fs> {
    let fs = vcod_common::testing::game_fs()?;
    let v6 = vcod_common::net::protocol::PROTOCOL_V6.transport.version;
    (vcod_server::server::install_protocol(&fs).transport.version == v6).then_some(fs)
}

/// The 1.5b `probe_bones` capture logs both clients' cmds and `ct=`, so
/// the hold's `setOrigin` drop lands where retail's did (`ProbeHold::ran`;
/// docs/research/cod11-combat.md 3.6).
#[test]
fn a_leaning_body_on_15_poses_as_on_retail() {
    let Some(fs) = fs_15() else {
        return;
    };
    check(
        fs,
        &Case {
            fixture: include_str!("fixtures/leanhit/mp_carentan-leanhit-1.5.txt"),
            join_ms: 9450,
            late_ms: 2750,
            // `load1`, head `b_07`.
            script_seed: 19,
        },
    );
}

#[test]
fn a_leaning_body_on_uo_poses_as_on_retail() {
    let Some(fs) = vcod_common::testing::uo_fs() else {
        return;
    };
    check(
        fs,
        &Case {
            fixture: include_str!("fixtures/leanhit/mp_carentan-leanhit-uo.txt"),
            join_ms: JOIN_UO,
            late_ms: LATE_UO,
            // `load3`, head `a_01`.
            script_seed: 12,
        },
    );
}

/// The `-cmds` and `-press` captures, runs of one recipe (two on 1.5b and
/// UO, three on 1.1d): `probe_fall 1` and `probe_tapwait 1`, both clients' cmd times
/// and input changes, and each snapshot's `commandTime` (`ct=`). The
/// replay runs each cmd on the side of a frame retail ran it on and
/// presses on the logged inputs (`ProbeHold::ran`). The first 1.1d
/// `-cmds` run's `ct=` was joined from its `VEH ... ps` lines.
fn cmds_case(fixture: &'static str, join_ms: i64, late_ms: i64) -> Case {
    Case {
        fixture,
        join_ms,
        late_ms,
        script_seed: SEED_LOAD1_A01,
    }
}

#[test]
fn a_dropped_body_lands_and_poses_as_on_retail_on_its_cmds() {
    let Some(fs) = vcod_common::testing::game_fs_11() else {
        return;
    };
    let fixture = include_str!("fixtures/leanhit/mp_carentan-leanhit-cmds.txt");
    check(fs, &cmds_case(fixture, CMDS_JOIN_11, CMDS_LATE_11));
}

/// The 1.1d `-cmds` recipe's second run, its `ct=` printed by the probe.
/// Every cmd before a trigger ran ahead of the frame here; in the first
/// run two ran after it (`t_prone`, `t_right`).
#[test]
fn a_dropped_body_lands_and_poses_as_on_retail_on_its_logged_frames() {
    let Some(fs) = vcod_common::testing::game_fs_11() else {
        return;
    };
    let fixture = include_str!("fixtures/leanhit/mp_carentan-leanhit-cmds-ct.txt");
    check(fs, &cmds_case(fixture, CT_JOIN_11, CT_LATE_11));
}

#[test]
fn a_dropped_body_on_15_lands_and_poses_as_on_retail_on_its_cmds() {
    let Some(fs) = fs_15() else {
        return;
    };
    let fixture = include_str!("fixtures/leanhit/mp_carentan-leanhit-cmds-1.5.txt");
    check(fs, &cmds_case(fixture, CMDS_JOIN_15, CMDS_LATE_15));
}

#[test]
fn a_dropped_body_on_uo_lands_and_poses_as_on_retail_on_its_cmds() {
    let Some(fs) = vcod_common::testing::uo_fs() else {
        return;
    };
    let fixture = include_str!("fixtures/leanhit/mp_carentan-leanhit-cmds-uo.txt");
    check(fs, &cmds_case(fixture, CMDS_JOIN_UO, CMDS_LATE_UO));
}

/// The `-press` captures' join times, as the `-cmds` ones'. Every round
/// leaves within [`DIR_NEAR`] of retail's direction: whether the cmd that
/// fires aims along the view the hold's `setPlayerAngles` left or along
/// the one a cmd after it quantized rides which side of the frame each
/// cmd ran on (docs/research/cod11-combat.md 15.1).
const PRESS_JOIN_11: i64 = 9150;
const PRESS_LATE_11: i64 = 3100;
const PRESS_JOIN_15: i64 = 9450;
const PRESS_LATE_15: i64 = 2950;
const PRESS_JOIN_UO: i64 = 8700;
const PRESS_LATE_UO: i64 = 3200;

#[test]
fn a_round_leaves_along_retails_aim_on_its_press() {
    let Some(fs) = vcod_common::testing::game_fs_11() else {
        return;
    };
    let fixture = include_str!("fixtures/leanhit/mp_carentan-leanhit-press.txt");
    check(fs, &cmds_case(fixture, PRESS_JOIN_11, PRESS_LATE_11));
}

#[test]
fn a_round_on_15_leaves_along_retails_aim_on_its_press() {
    let Some(fs) = fs_15() else {
        return;
    };
    let fixture = include_str!("fixtures/leanhit/mp_carentan-leanhit-press-1.5.txt");
    check(fs, &cmds_case(fixture, PRESS_JOIN_15, PRESS_LATE_15));
}

#[test]
fn a_round_on_uo_leaves_along_retails_aim_on_its_press() {
    let Some(fs) = vcod_common::testing::uo_fs() else {
        return;
    };
    let fixture = include_str!("fixtures/leanhit/mp_carentan-leanhit-press-uo.txt");
    check(fs, &cmds_case(fixture, PRESS_JOIN_UO, PRESS_LATE_UO));
}

/// The `-turn` and `-turnonce` captures' join times (their headers' first
/// playing snapshots, joined as [`cmds_case`]'s are).
const TURN_JOIN_11: i64 = 7800;
const TURN_LATE_11: i64 = 3000;
const ONCE_JOIN_11: i64 = 9100;
const ONCE_LATE_11: i64 = 2300;
const TURN_JOIN_15: i64 = 8050;
const TURN_LATE_15: i64 = 2750;
const TURN_JOIN_UO: i64 = 8550;
const TURN_LATE_UO: i64 = 3000;
const ONCE_JOIN_15: i64 = 8400;
const ONCE_LATE_15: i64 = 3800;
const ONCE_JOIN_UO: i64 = 9150;
const ONCE_LATE_UO: i64 = 4600;

/// `probe_turn 1` (docs/research/cod11-combat.md 3.7): the target's view
/// turns at each phase's start, and nothing poses him until the shooter's
/// round 1.5 s later, then the grid 0.3 s on and again a frame after.
/// The round's cmd and one of the target's carry the same time, and
/// whether it met the tilt rides which packet retail read first: the
/// captures' `RECV` lines say, and the replay sends both clients' cmds one
/// a packet in that order (`common::cmds_read`).
fn turn_case(fixture: &'static str, join_ms: i64, late_ms: i64) -> Case {
    cmds_case(fixture, join_ms, late_ms)
}

/// The hold sets the turn every frame, which tilts the whole body by the
/// view's pitch until the target's next cmd: every grid, run after the
/// hold in the thread pass, reads the tilted body, and the grid a frame
/// later the upright one.
#[test]
fn a_body_stands_in_the_angles_setplayerangles_gave_it_until_his_next_cmd() {
    let Some(fs) = vcod_common::testing::game_fs_11() else {
        return;
    };
    let fixture = include_str!("fixtures/leanhit/mp_carentan-leanhit-turn.txt");
    check(fs, &turn_case(fixture, TURN_JOIN_11, TURN_LATE_11));
}

#[test]
fn a_body_stands_in_the_angles_setplayerangles_gave_it_until_his_next_cmd_on_15() {
    let Some(fs) = fs_15() else {
        return;
    };
    let fixture = include_str!("fixtures/leanhit/mp_carentan-leanhit-turn-1.5.txt");
    check(fs, &turn_case(fixture, TURN_JOIN_15, TURN_LATE_15));
}

#[test]
fn a_body_stands_in_the_angles_setplayerangles_gave_it_until_his_next_cmd_on_uo() {
    let Some(fs) = vcod_common::testing::uo_fs() else {
        return;
    };
    let fixture = include_str!("fixtures/leanhit/mp_carentan-leanhit-turn-uo.txt");
    check(fs, &turn_case(fixture, TURN_JOIN_UO, TURN_LATE_UO));
}

/// `probe_turnonce 1`: the turn is set for 0.2 s only, so the round, the
/// first thing to pose the target since, meets him upright. On 1.5 and UO
/// each skeleton build eases his controls one step from where the last
/// build left them, and the round's packet is a build of its own: the
/// round reads one step past the last phase's second grid, the grid two,
/// not the settled turn (cod11-movers.md 15). Eased once an end frame
/// instead, the `t_down2` round lands 12 units off retail's; not eased by
/// the round, every round misses by 2 to 20.
/// 1.1d sets its controls outright, so its rounds read the settled turn.
#[test]
fn a_round_poses_a_turned_body_at_its_turn() {
    let Some(fs) = vcod_common::testing::game_fs_11() else {
        return;
    };
    let fixture = include_str!("fixtures/leanhit/mp_carentan-leanhit-turnonce.txt");
    check(fs, &turn_case(fixture, ONCE_JOIN_11, ONCE_LATE_11));
}

#[test]
fn a_round_on_15_poses_a_turned_body_one_build_on() {
    let Some(fs) = fs_15() else {
        return;
    };
    let fixture = include_str!("fixtures/leanhit/mp_carentan-leanhit-turnonce-1.5.txt");
    check(fs, &turn_case(fixture, ONCE_JOIN_15, ONCE_LATE_15));
}

#[test]
fn a_round_on_uo_poses_a_turned_body_one_build_on() {
    let Some(fs) = vcod_common::testing::uo_fs() else {
        return;
    };
    let fixture = include_str!("fixtures/leanhit/mp_carentan-leanhit-turnonce-uo.txt");
    check(fs, &turn_case(fixture, ONCE_JOIN_UO, ONCE_LATE_UO));
}
