//! Two gunners in each other's arc, ours against retail's.
//! `client-probes/probe_gunduel` puts an allied gunner on mp_carentan's gun
//! at (1712 1830 8) and moves the map's other gun 300 units in front of it,
//! facing back, for an axis gunner; both are `--probe-uo-weapon direct`
//! clients, mounted by `useby`, their views and triggers set by the script.
//! One sweeps his gun while the other holds the trigger on his chest, then
//! the roles swap, then both fire at once and, 0.5 s in, every hit kills.
//! Each hit logs its point and a grid of `bulletTrace`s round it, which
//! reads the pose the round met; every frame logs each gunner's origin
//! (docs/research/cod11-turrets.md 13.10).
//!
//! `GD_REPORT=1` prints every hit and grid row, worst first. Needs `COD_DIR` at a 1.1, 1.5 or UO
//! install.

use crate::common;

use common::{Join, Queues};
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;
use std::time::{Duration, Instant};
use vcod_common::net::msg::UserCmd;
use vcod_common::net::{NetClient, NetEvent};

const SCRIPT: &str =
    include_str!("../../gsc/tests/fixtures/semantics/client-probes/probe_gunduel.gsc");

/// The two clients' names in the capture's `CMDS` lines, in slot order.
const WHO: [&str; 2] = ["allies", "axis"];

/// One retail capture and the join times its recipe gave: server time
/// before ours' allied client connects, and how long after it the axis one
/// does.
struct Case {
    fixture: &'static str,
    join_ms: i64,
    late_ms: i64,
}

/// The buttons, `wbuttons` and `up` of a `f "PROBE_IN ..."` command.
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

/// A `PROBE spawnat` row's spawn point and angles for entity `num`.
fn spawn_at(fixture: &str, num: usize) -> Option<([f32; 3], [f32; 3])> {
    let l = fixture
        .lines()
        .filter_map(|l| l.strip_prefix("PROBE spawnat "))
        .find(|l| l.split(' ').next() == Some(&num.to_string()))?;
    let v: Vec<f32> = l.split(' ').skip(1).map(|t| t.parse().unwrap()).collect();
    Some(([v[0], v[1], v[2]], [v[3], v[4], v[5]]))
}

fn ours(fs: vcod_common::pk3::Pk3Fs, c: &Case) -> Vec<String> {
    let proto = vcod_server::server::install_protocol(&fs);
    let map = "mp_carentan";
    let bsp_path = fs.resolve_map(map).expect("map in the paks");
    let bsp = vcod_common::bsp::parse(&fs.read(&bsp_path).unwrap()).unwrap();
    let mut now = Instant::now();
    let mut sv = vcod_server::Server::new(common::cfg(map, "probe_gunduel"), now);
    sv.set_protocol(proto);
    sv.overlay_script("maps/mp/gametypes/probe_gunduel", SCRIPT);
    sv.load_world(vcod_server::world::World::from_bsp(&bsp, Some(&fs)));
    sv.load_scripts(Rc::new(fs)).expect("load the scripts");
    let pin = |num: usize| spawn_at(c.fixture, num).map(Some).into_iter().collect();
    sv.pin_spawns(vec![pin(0), pin(1)]);
    common::run_alone(&mut sv, &mut now, c.join_ms);
    let q = [
        Rc::new(RefCell::new(Queues::default())),
        Rc::new(RefCell::new(Queues::default())),
    ];
    let addr = [common::ADDR, common::ADDR_B];
    let mut cl: [Option<NetClient<common::ClientEnd>>; 2] = [
        Some(NetClient::start_with_qport(
            common::ClientEnd(q[0].clone()),
            now,
            0x2001,
        )),
        None,
    ];
    let mut join = [
        Join::new("allies", "m1carbine_mp"),
        Join::new("axis", "kar98k_mp"),
    ];
    let mut hold: [common::ProbeHold; 2] = std::array::from_fn(|k| common::ProbeHold {
        aim: true,
        framed: true,
        times: common::cmds_times(c.fixture, WHO[k]),
        ran: common::cmds_ran(c.fixture, WHO[k]),
        ..Default::default()
    });
    let pressed: [Vec<(i32, common::CmdInput)>; 2] =
        std::array::from_fn(|k| common::cmds_inputs(c.fixture, WHO[k]));
    let mut input = [(0u8, 0u8, 0i8); 2];
    let mut selected: [Option<u8>; 2] = [None; 2];
    let frame = Duration::from_millis(common::FRAME_MS as u64);
    let live = |cl: &NetClient<common::ClientEnd>| {
        cl.snapshots()
            .newest()
            .is_some_and(|s| s.ps.field_i32(cl.protocol(), "pm_type") == 0)
    };
    // Each cmd's place in retail's read order, keyed by slot and
    // `serverTime`.
    let read: std::collections::HashMap<(usize, i32), usize> = common::cmds_read(c.fixture, &WHO)
        .into_iter()
        .enumerate()
        .map(|(i, k)| (k, i))
        .collect();
    let take = |cl: &mut NetClient<common::ClientEnd>,
                hold: &mut common::ProbeHold,
                input: (u8, u8, i8),
                pressed: &[(i32, common::CmdInput)],
                weapon: Option<u8>| {
        if live(cl) {
            let mut cmds = hold.cmds(cl, input.0);
            for c in &mut cmds {
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
        if cl[1].is_none() && i * common::FRAME_MS >= c.late_ms {
            cl[1] = Some(NetClient::start_with_qport(
                common::ClientEnd(q[1].clone()),
                now,
                0x2002,
            ));
        }
        let sent: Vec<Option<Vec<UserCmd>>> = (0..2)
            .map(|k| {
                let (h, p) = (&mut hold[k], &pressed[k]);
                cl[k]
                    .as_mut()
                    .and_then(|cl| take(cl, h, input[k], p, selected[k]))
            })
            .collect();
        let batch: Vec<(usize, UserCmd)> = (0..2)
            .flat_map(|k| sent[k].iter().flatten().map(move |c| (k, *c)))
            .collect();
        if !batch.is_empty()
            && batch
                .iter()
                .all(|(k, c)| read.contains_key(&(*k, c.server_time)))
        {
            // One cmd a packet, in the order retail read them.
            let mut batch = batch;
            batch.sort_by_key(|(k, c)| read[&(*k, c.server_time)]);
            for (k, cmd) in batch {
                cl[k].as_mut().unwrap().send_cmds(&[cmd]);
                let pending: Vec<Vec<u8>> = q[k].borrow_mut().to_server.drain(..).collect();
                for p in pending {
                    sv.handle_packet(addr[k], &p, now);
                }
            }
        } else {
            for k in 0..2 {
                if let (Some(cl), Some(cmds)) = (cl[k].as_mut(), &sent[k]) {
                    cl.send_cmds(cmds);
                }
            }
        }
        for k in 0..2 {
            let pending: Vec<Vec<u8>> = q[k].borrow_mut().to_server.drain(..).collect();
            for p in pending {
                sv.handle_packet(addr[k], &p, now);
            }
        }
        sv.tick(now);
        for (to, p) in sv.take_outgoing() {
            let k = usize::from(to != common::ADDR);
            q[k].borrow_mut().to_client.push_back(p);
        }
        for k in 0..2 {
            let Some(c) = cl[k].as_mut() else { continue };
            for e in c.pump_at(now) {
                if let NetEvent::ServerCommand(tokens) = e {
                    if tokens.first().map(String::as_str) == Some("a") {
                        selected[k] = tokens.get(1).and_then(|w| w.parse().ok());
                    }
                    if let Some(i) = direct_input(&tokens) {
                        input[k] = i;
                    }
                    join[k].on_server_command(&tokens, c, now);
                }
            }
        }
    }
    sv.script_log().to_vec()
}

fn probe_lines<'a>(lines: impl IntoIterator<Item = &'a str>) -> Vec<String> {
    lines
        .into_iter()
        .map(str::trim)
        .filter(|l| l.starts_with("PROBE"))
        .map(str::to_string)
        .collect()
}

/// `fine1`'s `<int>:<thousandths>:<millionths of a thousandth>`.
fn fine(c: &str) -> f64 {
    let neg = c.starts_with('-');
    let mut t = c.trim_start_matches('-').split(':');
    let n: f64 = t.next().unwrap().parse().unwrap();
    let m: f64 = t.next().unwrap().parse().unwrap();
    let r: f64 = t.next().unwrap().parse().unwrap();
    let v = n + m / 1000.0 + r / 1e9;
    if neg { -v } else { v }
}

fn fine3(v: &str) -> [f64; 3] {
    let p: Vec<f64> = v.split(',').map(fine).collect();
    [p[0], p[1], p[2]]
}

/// A hit's location, damage, point and direction.
type Hit = (String, String, [f64; 3], [f64; 3]);

/// Every `PROBE hit` keyed `(time, victim, attacker)`.
fn hits(lines: &[String]) -> BTreeMap<(i32, u32, i32), Hit> {
    let mut out = BTreeMap::new();
    for l in lines {
        let Some(r) = l.strip_prefix("PROBE hit ") else {
            continue;
        };
        let t: Vec<&str> = r.split(' ').collect();
        out.insert(
            (
                t[0].parse().unwrap(),
                t[1].parse().unwrap(),
                t[2].parse().unwrap(),
            ),
            (t[3].into(), t[4].into(), fine3(t[5]), fine3(t[6])),
        );
    }
    out
}

/// Every `PROBE grid` row keyed `(time, victim, row)`.
fn grids(lines: &[String]) -> BTreeMap<(i32, u32, i32), Vec<Option<f64>>> {
    let mut out = BTreeMap::new();
    for l in lines {
        let Some(r) = l.strip_prefix("PROBE grid ") else {
            continue;
        };
        let t: Vec<&str> = r.split(' ').collect();
        let cells = t[3..]
            .iter()
            .map(|c| (*c != "-").then(|| fine(c)))
            .collect();
        out.insert(
            (
                t[0].parse().unwrap(),
                t[1].parse().unwrap(),
                t[2].parse().unwrap(),
            ),
            cells,
        );
    }
    out
}

/// Every `PROBE at` origin keyed `(time, entity)`.
fn origins(lines: &[String]) -> BTreeMap<(i32, u32), [f64; 3]> {
    let mut out = BTreeMap::new();
    for l in lines {
        let Some(r) = l.strip_prefix("PROBE at ") else {
            continue;
        };
        let t: Vec<&str> = r.split(' ').collect();
        out.insert((t[0].parse().unwrap(), t[1].parse().unwrap()), fine3(t[2]));
    }
    out
}

fn off3(a: [f64; 3], b: [f64; 3]) -> f64 {
    (0..3).map(|i| (a[i] - b[i]).abs()).fold(0.0, f64::max)
}

/// The rows past their bound, worst first, for `GD_REPORT`.
fn report(rows: &[(f64, String)]) {
    if std::env::var_os("GD_REPORT").is_none() {
        return;
    }
    let mut rows = rows.to_vec();
    rows.sort_by(|a, b| b.0.total_cmp(&a.0));
    for (_, r) in rows.iter().take(200) {
        eprintln!("{r}");
    }
}

/// Rows a gate tolerates, each with the research section that explains it:
/// the hit points and grid cells on `victim` from `from` to `to` ms.
struct Gap {
    victim: u32,
    from: i32,
    to: i32,
    why: &'static str,
}

fn check(fs: vcod_common::pk3::Pk3Fs, c: &Case, near: Near, gaps: &[Gap]) {
    let retail = probe_lines(c.fixture.lines());
    let ours = ours(fs, c);
    let ours = probe_lines(ours.iter().map(String::as_str));
    let fixed = |l: &[String]| -> Vec<String> {
        l.iter()
            .filter(|l| {
                ["PROBE gun ", "PROBE site ", "PROBE player ", "PROBE phase "]
                    .iter()
                    .any(|p| l.starts_with(p))
            })
            .cloned()
            .collect()
    };
    assert_eq!(
        fixed(&retail),
        fixed(&ours),
        "the guns, the site and the phases"
    );
    let mut rows = Vec::new();
    let mut bad = Vec::new();
    // Each gap's worst row, which must stay past the bound.
    let mut gap_off = vec![0.0f64; gaps.len()];
    let mut gapped = |t: i32, v: u32, d: f64| -> bool {
        let Some(k) = gaps
            .iter()
            .position(|g| g.victim == v && (g.from..=g.to).contains(&t))
        else {
            return false;
        };
        gap_off[k] = gap_off[k].max(d);
        true
    };

    let (ro, oo) = (origins(&retail), origins(&ours));
    assert_eq!(
        ro.keys().collect::<Vec<_>>(),
        oo.keys().collect::<Vec<_>>(),
        "the origin rows"
    );
    for (k, r) in &ro {
        let d = off3(*r, oo[k]);
        if d > near.origin {
            bad.push(format!("origin {k:?}: retail {r:?} ours {:?}", oo[k]));
        }
    }

    // When, on whom and by whom each round struck: a gunner a lower slot's
    // round kills fires nothing on his own turn (turrets doc 13.10).
    let (rh, oh) = (hits(&retail), hits(&ours));
    assert_eq!(
        rh.keys().collect::<Vec<_>>(),
        oh.keys().collect::<Vec<_>>(),
        "the hits: when, on whom, by whom"
    );
    for (k, r) in &rh {
        let o = &oh[k];
        assert_eq!((&r.0, &r.1), (&o.0, &o.1), "hit {k:?}: location and damage");
        let dd = off3(r.3, o.3);
        if dd > near.dir {
            bad.push(format!(
                "hit {k:?} direction: retail {:?} ours {:?}",
                r.3, o.3
            ));
        }
        let d = off3(r.2, o.2);
        rows.push((
            d,
            format!("hit {k:?}: retail {:?} ours {:?} off {d:.6}", r.2, o.2),
        ));
        if d > near.point && !gapped(k.0, k.1, d) {
            bad.push(format!(
                "hit {k:?}: retail {:?} ours {:?} off {d}",
                r.2, o.2
            ));
        }
    }

    let (rg, og) = (grids(&retail), grids(&ours));
    assert_eq!(
        rg.keys().collect::<Vec<_>>(),
        og.keys().collect::<Vec<_>>(),
        "the grid rows"
    );
    for (k, r) in &rg {
        let o = &og[k];
        let d = r
            .iter()
            .zip(o)
            .map(|(r, o)| match (r, o) {
                (Some(r), Some(o)) => (r - o).abs(),
                (None, None) => 0.0,
                _ => f64::INFINITY,
            })
            .fold(0.0, f64::max);
        rows.push((
            d,
            format!("grid {k:?}: retail {r:?}\n   ours {o:?} off {d:.6}"),
        ));
        if d > near.cell && !gapped(k.0, k.1, d) {
            bad.push(format!("grid {k:?}:\n  retail {r:?}\n  ours   {o:?}"));
        }
    }
    report(&rows);
    for (g, off) in gaps.iter().zip(&gap_off) {
        if *off <= near.point.max(near.cell) {
            bad.push(format!(
                "gap on {} {}..{} reads retail's now ({off}): drop it ({})",
                g.victim, g.from, g.to, g.why
            ));
        }
    }
    assert!(bad.is_empty(), "off retail's:\n{}", bad.join("\n"));
}

/// How far a row may sit from retail's.
#[derive(Clone, Copy)]
struct Near {
    origin: f64,
    point: f64,
    dir: f64,
    cell: f64,
}

/// One float step at 1024..2048, where both guns' gunners stand, and the
/// millionth of a thousandth `fine1` prints to: the origins and points
/// ours reads off retail's (cod11-turrets.md 13.10).
const STEP: f64 = 1.0 / 8192.0 + 0.000001;

/// 1.5b and UO: origins to the step, points to two (the hit trace's
/// turned normal and fraction each round once), directions as retail
/// prints them, and grid cells within the 0.0011 the turn phases read.
const NEAR_15: Near = Near {
    origin: STEP,
    point: 2.0 * STEP,
    dir: 0.000001,
    cell: 0.0011,
};

/// The rounds on a firing gunner (the duel), from the first frame both
/// fire: cells to 0.0013 on it, points 0.01 to 0.37 and cells to 7.8
/// from the second on both victims (cod11-turrets.md 13.10, "Open").
const DUEL_GAPS: &[Gap] = &[
    Gap {
        victim: 0,
        from: 26250,
        to: 26700,
        why: "turrets doc 13.10, a firing gunner",
    },
    Gap {
        victim: 1,
        from: 26250,
        to: 26700,
        why: "turrets doc 13.10, a firing gunner",
    },
];

/// 1.1d: every gunner's points and cells (cod11-turrets.md 13.10, "Open"):
/// 0.0004 to 0.011 on a body his own end frame placed, 0.08 to 0.2 in
/// the arc's middle on one it has not, and cells to 0.06 and 0.3.
const POSE_GAPS_11: &[Gap] = &[
    Gap {
        victim: 0,
        from: 0,
        to: i32::MAX,
        why: "turrets doc 13.10, the 1.1d gunner pose",
    },
    Gap {
        victim: 1,
        from: 0,
        to: i32::MAX,
        why: "turrets doc 13.10, the 1.1d gunner pose",
    },
];

#[test]
fn two_gunners_in_each_others_arc_match_retail() {
    let Some(fs) = vcod_common::testing::game_fs_11() else {
        return;
    };
    let c = Case {
        fixture: include_str!("fixtures/gunduel/mp_carentan-gunduel.txt"),
        join_ms: 7950,
        late_ms: 3100,
    };
    check(fs, &c, NEAR_15, POSE_GAPS_11);
}

fn fs_15() -> Option<vcod_common::pk3::Pk3Fs> {
    let fs = vcod_common::testing::game_fs()?;
    let v6 = vcod_common::net::protocol::PROTOCOL_V6.transport.version;
    (vcod_server::server::install_protocol(&fs).transport.version == v6).then_some(fs)
}

#[test]
fn two_gunners_in_each_others_arc_match_15b() {
    let Some(fs) = fs_15() else {
        return;
    };
    let c = Case {
        fixture: include_str!("fixtures/gunduel/mp_carentan-gunduel-1.5.txt"),
        join_ms: 7100,
        late_ms: 3300,
    };
    check(fs, &c, NEAR_15, DUEL_GAPS);
}

#[test]
fn two_gunners_in_each_others_arc_match_uo() {
    let Some(fs) = vcod_common::testing::uo_fs() else {
        return;
    };
    let c = Case {
        fixture: include_str!("fixtures/gunduel/mp_carentan-gunduel-uo.txt"),
        join_ms: 7550,
        late_ms: 3250,
    };
    check(fs, &c, NEAR_15, DUEL_GAPS);
}
