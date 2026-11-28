//! A United Offensive jeep running over a player, ours against retail's
//! (`fixtures/vehicles/mp_kharkov-jeep-runover-*-uo.txt`,
//! `client-probes/probe_vehicle_ram` with `probe_victim`, recipes in their
//! headers): mp_kharkov's second Horch, driven straight ahead by
//! `--probe-vehicle ram`, meets client 0 150 units ahead of it.
//! `VEH_PlayerCollision`'s run-over damage, knock and `vehicle_hit_person`
//! (docs/research/coduo-vehicles.md 13), and a prone body under the hull
//! taking `ClientThink_real`'s 10 points a cmd (11.6). The victim's fall
//! from his first placement is compared too (`compare_first_drop`). The
//! `bail` rows' driver gets out a second into the drive and the jeep rolls
//! on driverless. A capture's `CMDS` lines replay both clients' cmds on
//! retail's times.
//!
//! `RUNOVER_REPORT=1` prints both sides' lines, `RUNOVER_DUMP=<path>`
//! writes ours. Needs `COD_DIR` at a UO
//! install; without one it returns early.

use crate::common;

use common::{ClientEnd, Join, Queues, step_pair};
use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;
use vcod_common::net::event_ids::EV_SOUND_ALIAS;
use vcod_common::net::msg::{BUTTON_USE, UserCmd};
use vcod_common::net::protocol::PROTOCOL_V22;
use vcod_common::net::{NetClient, NetEvent, NetState};

const MAP: &str = "mp_kharkov";
/// `vehicle_probe.rs` `RAM`, from the driver's placement.
const SETTLE_MS: i64 = 2000;
const TAP_MS: i64 = 150;
const SEATED_MS: i64 = 2000;
const AHEAD_MS: i64 = 6000;
/// `BAIL`: the throttle's time before the exit, which keeps it held.
const BAIL_MS: i64 = 1000;
/// How long after the victim connects ours places him, run alone for
/// `common::PROBE_JOIN_MS` before: the capture's `PROBE victim` time less
/// this is how much longer ours runs alone, so both clocks agree and the
/// victim's cmds go out on the times the capture's `CMDS` lines logged.
const VICTIM_PLACED_MS: i64 = 1400;

/// Our PROBE lines, and the `EV_SOUND_ALIAS` temp entities the victim's
/// snapshots carried, each once as `VEH <time> temp <n> ev 172 parm <p>
/// other 0 pos <x> <y> <z>` in the probe client's format. The victim is
/// allied, the driver on `driver_team`; `cvars` go on top of the probe's.
/// With `bail` the driver drives `vehicle_probe.rs` `BAIL`, out 1 s in.
/// The clients join on the capture's clock: the victim as late as
/// `PROBE victim` puts him, the driver as much later as `PROBE place`
/// came, and the victim sends his cmds on the times of the capture's
/// `victim CMDS` lines.
fn ours(
    victim: &str,
    driver_team: &str,
    cvars: &[(&str, &str)],
    bail: bool,
    capture: &str,
) -> Option<Vec<String>> {
    let fs = vcod_common::testing::uo_fs()?;
    let at = |head: &str| {
        capture
            .lines()
            .find(|l| l.starts_with(head))
            .and_then(time)
            .unwrap_or_else(|| panic!("a {head}line in the capture")) as i64
    };
    let late_ms = at("PROBE victim ") - common::PROBE_JOIN_MS - VICTIM_PLACED_MS;
    let driver_late_ms = at("PROBE place ") - at("PROBE victim ");
    let p = &PROTOCOL_V22;
    let mut sets = vec![
        ("probe_vehicletype", "horch_mp"),
        ("probe_index", "1"),
        ("probe_ahead", "680"),
        ("probe_victim", victim),
        ("probe_victim_ahead", "150"),
    ];
    sets.extend_from_slice(cvars);
    let (mut sv, mut now) = common::Boot {
        proto: Some(p),
        cvars: &sets,
        script: Some(include_str!(
            "../../gsc/tests/fixtures/semantics/client-probes/probe_vehicle_ram.gsc"
        )),
        ..common::Boot::new(MAP, "probe_vehicle_ram")
    }
    .start(fs);
    common::run_alone(&mut sv, &mut now, common::PROBE_JOIN_MS + late_ms);
    let (qa, qb) = (
        Rc::new(RefCell::new(Queues::default())),
        Rc::new(RefCell::new(Queues::default())),
    );
    // The victim connects first, so he is client 0; the driver connects
    // `DRIVER_LATE_MS` after him, as the recipes' second client did, so the
    // two never join in one frame.
    let mut ca = NetClient::start_with_qport(ClientEnd(qa.clone()), now, 0x2001);
    let mut cb: Option<NetClient<ClientEnd>> = None;
    let mut ja = Join::new("allies", "mosin_nagant_mp");
    let weapon = if driver_team == "axis" {
        "kar98k_mp"
    } else {
        "mosin_nagant_mp"
    };
    let mut jb = Join::new(driver_team, weapon);
    let frame = Duration::from_millis(common::FRAME_MS as u64);
    // The victim, `--probe-uo-weapon direct`, holds the view his placement
    // gave him, three cmds a frame; the driver, `--probe-vehicle ram`,
    // holds his from the use tap on. Each takes it off the placement's
    // line: the join's cmds, sent until then, keep the world view at 0.
    let mut victim_hold = common::ProbeHold {
        framed: true,
        ..Default::default()
    };
    let mut hold = common::ProbeHold::default();
    // A capture that logged the driver's `CMDS` lines replays them: his cmds
    // on their times, the use taps on the cmds that carried them, and the
    // throttle from `SEATED_MS` past the first tap's end, as
    // `vehicle_probe.rs` holds it.
    let driver_in = common::cmds_inputs(capture, "driver");
    let replay_driver = !driver_in.is_empty();
    if replay_driver {
        hold.framed = true;
        hold.times = common::cmds_times(capture, "driver");
        hold.ran = common::cmds_ran(capture, "driver");
    }
    let throttle_from = driver_in
        .windows(2)
        .find(|w| w[0].1.0 & BUTTON_USE != 0)
        .map(|w| w[1].0 + SEATED_MS as i32);
    victim_hold.times = common::cmds_times(capture, "victim");
    victim_hold.ran = common::cmds_ran(capture, "victim");
    // The velocity the victim's placement kept: it shares his spawn's frame,
    // and the cmds run at the spawn point before it leave him moving
    // (coduo-vehicles.md 11.6, "The first drop").
    let spawn_vel: Option<[f32; 3]> = capture
        .lines()
        .find_map(|l| l.strip_prefix("victim SPAWNED "))
        .and_then(|l| l.split_whitespace().find_map(|w| w.strip_prefix("vel=")))
        .map(|v| {
            let v: Vec<f32> = v
                .split(',')
                .map(|c| c.parse().expect("a velocity"))
                .collect();
            [v[0], v[1], v[2]]
        });
    // A capture from before the probe logged CMDS lines holds the victim
    // prone from his placement, as he lay down on the gsc's `PROBE_IN`.
    let mut victim_in = common::cmds_inputs(capture, "victim");
    if victim_hold.times.is_empty() && victim == "prone" {
        victim_in.push((i32::MIN, (0, 0x40, -127)));
    }
    let mut placed_at: Option<i64> = None;
    let mut sounds = Vec::new();
    let mut seen = std::collections::BTreeSet::new();
    for i in 0..3000i64 {
        now += frame;
        if cb.is_none() && i * common::FRAME_MS >= driver_late_ms {
            cb = Some(NetClient::start_with_qport(
                ClientEnd(qb.clone()),
                now,
                0x2002,
            ));
        }
        let since = placed_at.map(|at| (i - at) * common::FRAME_MS);
        let ahead = SETTLE_MS + TAP_MS + SEATED_MS;
        if since.is_some_and(|ms| ms >= ahead + AHEAD_MS) {
            break;
        }
        let tapping = |from: i64| since.is_some_and(|ms| (from..from + TAP_MS).contains(&ms));
        let throttle = if bail { BAIL_MS + TAP_MS } else { AHEAD_MS };
        let tap = tapping(SETTLE_MS) || (bail && tapping(ahead + BAIL_MS));
        hold.forward = if since.is_some_and(|ms| (ahead..ahead + throttle).contains(&ms)) {
            127
        } else {
            0
        };
        if victim_hold.base.is_none() {
            victim_hold.base = placed_yaw(sv.script_log(), "PROBE victim ").map(|y| [0.0, y, 0.0]);
            if let (Some(_), Some(v)) = (victim_hold.base, spawn_vel) {
                sv.test_set_client_velocity(0, v);
            }
        }
        if hold.base.is_none() {
            hold.base = placed_yaw(sv.script_log(), "PROBE place ").map(|y| [10.0, y, 0.0]);
        }
        let clients = [
            Some((&mut ca, &mut victim_hold, 0, true)),
            cb.as_mut()
                .map(|cb| (cb, &mut hold, if tap { BUTTON_USE } else { 0 }, false)),
        ];
        for (cl, h, buttons, is_victim) in clients.into_iter().flatten() {
            if h.base.is_none() || cl.state() != NetState::Active {
                if let Some(c) = cl.send_frame(&UserCmd::default()) {
                    h.last = c.server_time;
                }
                continue;
            }
            let mut cmds = h.cmds(cl, buttons);
            // The victim's buttons, prone key and up move as his capture's
            // `in=` changes set them: he lies down on the cmd retail's
            // probe client pressed it on, the `PROBE_IN` it answers. The
            // driver's the same way when his are replayed.
            let ins = if is_victim { &victim_in } else { &driver_in };
            if h.framed {
                for c in &mut cmds {
                    let at = ins.partition_point(|(st, _)| *st <= c.server_time);
                    if let Some(&(_, (b, wb, up))) = at.checked_sub(1).map(|k| &ins[k]) {
                        (c.buttons, c.wbuttons, c.up) = (b, wb, up);
                    }
                    if let (false, Some(from)) = (is_victim, throttle_from) {
                        let on = (from..from + throttle as i32).contains(&c.server_time);
                        c.forward = if on { 127 } else { 0 };
                    }
                }
            }
            cl.send_cmds(&cmds);
        }
        let ea = if let Some(cb) = cb.as_mut() {
            let (ea, eb) = step_pair(&mut sv, (&qa, &mut ca), (&qb, cb), now);
            for e in eb {
                if let NetEvent::ServerCommand(tokens) = e {
                    jb.on_server_command(&tokens, cb, now);
                }
            }
            ea
        } else {
            common::step_at(&mut sv, common::ADDR, &qa, &mut ca, now)
        };
        for e in ea {
            if let NetEvent::ServerCommand(tokens) = e {
                ja.on_server_command(&tokens, &mut ca, now);
            }
        }
        if let Some(s) = ca.snapshots().newest() {
            for (num, e) in &s.entities {
                let i = |n: &str| e.field_i32(p, n);
                if p.event_of(i("eType")) != Some(EV_SOUND_ALIAS) {
                    continue;
                }
                let o = e.origin(p);
                let line = format!(
                    "temp {num} ev {EV_SOUND_ALIAS} parm {} other {} pos {:.1} {:.1} {:.1}",
                    i("eventParm"),
                    i("otherEntityNum"),
                    o[0],
                    o[1],
                    o[2]
                );
                if seen.insert(line.clone()) {
                    sounds.push(format!("VEH {} {line}", s.server_time));
                }
            }
        }
        if placed_at.is_none()
            && sv
                .script_log()
                .iter()
                .any(|l| l.starts_with("PROBE place "))
        {
            placed_at = Some(i);
        }
    }
    let mut out: Vec<String> = sv
        .script_log()
        .iter()
        .filter(|l| l.starts_with("PROBE "))
        .map(|l| l.trim_end().to_string())
        .collect();
    out.extend(sounds);
    if let Some(path) = std::env::var_os("RUNOVER_DUMP") {
        std::fs::write(path, out.join("\n")).expect("write RUNOVER_DUMP");
    }
    Some(out)
}

/// The yaw of the first line opening `head` (`PROBE place <t> <n> <spot>
/// <yaw>`).
fn placed_yaw(log: &[String], head: &str) -> Option<f32> {
    log.iter()
        .find(|l| l.starts_with(head))
        .and_then(|l| l.split_whitespace().last()?.parse().ok())
}

fn read(path: &str) -> String {
    std::fs::read_to_string(path).unwrap_or_else(|e| panic!("read {path}: {e}"))
}

fn retail(text: &str) -> Vec<String> {
    text.lines()
        .filter(|l| l.starts_with("PROBE ") || l.starts_with("VEH "))
        .map(str::to_string)
        .collect()
}

/// A line's time: the third word of a PROBE line, the second of a VEH one.
fn time(l: &str) -> Option<i32> {
    let w: Vec<&str> = l.split(' ').collect();
    match w[0] {
        "VEH" => w.get(1)?.parse().ok(),
        _ => w.get(2)?.parse().ok(),
    }
}

/// The victim's lines (`v`, `dmg`) and the hit sounds (alias 8,
/// `vehicle_hit_person`) from 100 ms before the first hit to `upto` ms past
/// it, each time made relative to the jeep's first move: the drive's start
/// rides the probe client's own clock from its spawn.
fn around_hit(lines: &[String], upto: i32, origins: bool) -> Vec<String> {
    let placed = lines
        .iter()
        .filter(|l| l.starts_with("PROBE a "))
        .find(|l| l.rsplit(' ').next() != Some("0"))
        .and_then(|l| time(l))
        .expect("the jeep moved");
    let hit = lines
        .iter()
        .find(|l| l.starts_with("PROBE dmg "))
        .and_then(|l| time(l))
        .expect("the victim was hit");
    let mut out: Vec<String> = lines
        .iter()
        .filter(|l| {
            l.starts_with("PROBE v ")
                || l.starts_with("PROBE dmg ")
                || (origins && l.starts_with("PROBE dmgorigin "))
                || (l.starts_with("VEH ") && l.contains(" parm 8 "))
        })
        .filter_map(|l| {
            let t = time(l)?;
            (hit - 100..=hit + upto).contains(&t).then(|| {
                let mut w: Vec<String> = l.split(' ').map(str::to_string).collect();
                let at = if w[0] == "VEH" { 1 } else { 2 };
                w[at] = format!("+{}", t - placed);
                // A temp entity's number rides the free list.
                if w[0] == "VEH" {
                    w[3] = "-".into();
                }
                w.join(" ")
            })
        })
        .collect();
    out.sort_by_key(|l| (time_rel(l), l.starts_with("VEH")));
    out
}

/// The jeep's ram sounds (alias 7, `vehicle_impact`, as its Horch strikes
/// the Panzer) up to `upto` ms past the jeep's first move, each time made
/// relative to that move and the temp entity's number dropped.
fn impacts(lines: &[String], upto: i32) -> Vec<String> {
    let placed = lines
        .iter()
        .filter(|l| l.starts_with("PROBE a "))
        .find(|l| l.rsplit(' ').next() != Some("0"))
        .and_then(|l| time(l))
        .expect("the jeep moved");
    lines
        .iter()
        .filter(|l| l.starts_with("VEH ") && l.contains(" parm 7 "))
        .filter_map(|l| {
            let t = time(l)? - placed;
            let w: Vec<&str> = l.split(' ').skip(4).collect();
            (t <= upto).then(|| format!("+{t} {}", w.join(" ")))
        })
        .collect()
}

/// The ram sounds on ours against retail's, one for one: same times from
/// the jeep's first move, same alias and same snapped origin.
fn compare_impacts(retail: &[String], ours: &[String]) {
    let upto = ours
        .iter()
        .filter_map(|l| time(l))
        .max()
        .expect("our lines")
        - ours
            .iter()
            .filter(|l| l.starts_with("PROBE a "))
            .find(|l| l.rsplit(' ').next() != Some("0"))
            .and_then(|l| time(l))
            .expect("our jeep moved");
    let (r, o) = (impacts(retail, upto), impacts(ours, upto));
    assert!(!r.is_empty(), "retail's jeep struck nothing by +{upto}");
    assert_eq!(r, o, "the ram sounds");
}

fn time_rel(l: &str) -> i32 {
    l.split(' ')
        .find_map(|w| w.strip_prefix('+')?.parse().ok())
        .unwrap_or(0)
}

/// Lines past `after` ms from the first hit let differ from retail's by
/// at most `within` a number, for `why`; at least one must.
struct Drift {
    after: i32,
    within: f32,
    why: &'static str,
}

/// A prone corpse creeping off the hull's line: retail's +1150 repeats
/// +1100 (a frame no cmd reached), the creep a cmd at a time.
const CREEP: Drift = Drift {
    after: 200,
    within: 0.1,
    why: "the probe client's cmd clock in the corpse's creep",
};

/// Where a prone teammate's victim settles and creeps: with both clients'
/// cmds, his prone press and spawn velocity replayed he lies on retail's
/// spot to the hundredth, and his corpse creeps up to 0.13 apart after the
/// jeep has passed (coduo-vehicles.md 11.6, "The corpse's creep").
const LIE_DOWN: Drift = Drift {
    after: -1000,
    within: 0.15,
    why: "the prone victim's creep under the jeep",
};

fn words(l: &str) -> Vec<String> {
    l.split([' ', '(', ')', ','])
        .filter(|w| !w.is_empty())
        .map(str::to_string)
        .collect()
}

/// Two lines' words equal, decimals within `tol`.
fn same(a: &str, b: &str, tol: f32) -> bool {
    let (wa, wb) = (words(a), words(b));
    wa.len() == wb.len()
        && wa.iter().zip(&wb).all(|(x, y)| {
            match (x.contains('.'), x.parse::<f32>(), y.parse::<f32>()) {
                (true, Ok(x), Ok(y)) => (x - y).abs() <= tol,
                _ => x == y,
            }
        })
}

/// The victim's lines from his first placement up to the second
/// (`PROBE victim`, `v`), each time made relative to the placement.
fn first_drop(lines: &[String]) -> Vec<String> {
    let at = |head: &str| {
        lines
            .iter()
            .find(|l| l.starts_with(head))
            .and_then(|l| time(l))
            .unwrap_or_else(|| panic!("a {head}line"))
    };
    let (placed, replaced) = (at("PROBE victim "), at("PROBE replace "));
    lines
        .iter()
        .filter(|l| l.starts_with("PROBE victim ") || l.starts_with("PROBE v "))
        .filter_map(|l| {
            let t = time(l)?;
            (placed..replaced).contains(&t).then(|| {
                let mut w: Vec<&str> = l.split(' ').collect();
                let rel = format!("+{}", t - placed);
                w[2] = &rel;
                w.join(" ")
            })
        })
        .collect()
}

/// The first drops ours misses, each with its reason; none since the
/// driverless prone run's lie-down waits for the ground as retail's does
/// (coduo-vehicles.md 11.6, "The first drop").
const FIRST_DROP_GAPS: &[(&str, &str)] = &[];

/// The victim's first drop on ours against retail's, line for line to the
/// hundredth, unless [`FIRST_DROP_GAPS`] names `capture`: then at least one
/// line must differ.
fn compare_first_drop(capture: &str, retail: &[String], ours: &[String]) {
    let (r, o) = (first_drop(retail), first_drop(ours));
    assert_eq!(
        r.len(),
        o.len(),
        "first drop line counts\nretail {r:#?}\nours {o:#?}"
    );
    let differ: Vec<String> = r
        .iter()
        .zip(&o)
        .filter(|(a, b)| !same(a, b, 0.01))
        .map(|(a, b)| format!("retail {a}\nours   {b}"))
        .collect();
    match FIRST_DROP_GAPS.iter().find(|(c, _)| *c == capture) {
        None => assert!(differ.is_empty(), "the first drop\n{}", differ.join("\n")),
        Some((_, why)) => assert!(
            !differ.is_empty(),
            "the first drop matches retail's; drop its gap ({why})"
        ),
    }
}

/// Retail's lines against ours one for one: whole words (times, damage,
/// health, alias indices) equal, decimals within 0.01 up to `drift.after`
/// ms past the first hit and within `drift.within` from there; every one
/// within 0.01 without a drift.
fn compare(retail: &[String], ours: &[String], upto: i32, drift: Option<Drift>) {
    let Some(drift) = drift else {
        let (r, o) = (around_hit(retail, upto, true), around_hit(ours, upto, true));
        let differ: Vec<String> = r
            .iter()
            .zip(&o)
            .filter(|(a, b)| !same(a, b, 0.01))
            .map(|(a, b)| format!("retail {a}\nours   {b}"))
            .collect();
        assert!(differ.is_empty(), "{}", differ.join("\n"));
        assert_eq!(r.len(), o.len(), "line counts\nretail {r:#?}\nours {o:#?}");
        return;
    };
    // Captures from before the probe logged `dmgorigin` carry none.
    let origins = retail.iter().any(|l| l.starts_with("PROBE dmgorigin "));
    let (r, o) = (
        around_hit(retail, upto, origins),
        around_hit(ours, upto, origins),
    );
    if std::env::var_os("RUNOVER_REPORT").is_some() {
        for l in &r {
            eprintln!("retail {l}");
        }
        for l in &o {
            eprintln!("ours   {l}");
        }
    }
    let hit = r
        .iter()
        .find(|l| l.starts_with("PROBE dmg "))
        .map(|l| time_rel(l))
        .expect("retail's first hit");
    if std::env::var_os("RUNOVER_REPORT").is_some() {
        let off = |a: &str, b: &str| -> f32 {
            words(a)
                .iter()
                .zip(words(b))
                .filter_map(|(x, y)| Some((x.parse::<f32>().ok()? - y.parse::<f32>().ok()?).abs()))
                .filter(|d| d.is_finite())
                .fold(0.0, f32::max)
        };
        let worst = r.iter().zip(&o).map(|(a, b)| off(a, b)).fold(0.0, f32::max);
        eprintln!("drift: {worst} at most ({})", drift.why);
    }
    let mut drifted = false;
    for (a, b) in r.iter().zip(&o) {
        if same(a, b, 0.01) {
            continue;
        }
        assert!(
            time_rel(a) > hit + drift.after && same(a, b, drift.within),
            "past {} ({})\nretail {a}\nours   {b}",
            drift.within,
            drift.why
        );
        drifted = true;
    }
    assert_eq!(r.len(), o.len(), "line counts\nretail {r:#?}\nours {o:#?}");
    assert!(
        drifted,
        "every line matches; drop the drift ({})",
        drift.why
    );
}

#[test]
fn a_jeep_runs_over_a_standing_player_twice() {
    let text = read("tests/fixtures/vehicles/mp_kharkov-jeep-runover-stand-uo.txt");
    let r = retail(&text);
    let Some(o) = ours("stand", "axis", &[], false, &text) else {
        return;
    };
    compare_first_drop("stand", &r, &o);
    compare(&r, &o, 1000, None);
    compare_impacts(&r, &o);
}

#[test]
fn a_prone_player_under_a_jeep_takes_ten_a_cmd() {
    let text = read("tests/fixtures/vehicles/mp_kharkov-jeep-runover-prone-uo.txt");
    let r = retail(&text);
    let Some(o) = ours("prone", "axis", &[], false, &text) else {
        return;
    };
    compare_first_drop("prone", &r, &o);
    compare(&r, &o, 1000, Some(CREEP));
    compare_impacts(&r, &o);
}

/// A teammate's jeep under friendly fire 0: each cmd's crush is refused,
/// so the prone victim deals himself the second 10 (`MOD_SUICIDE`, his own
/// weapon), and once dead keeps taking both until his end frame.
#[test]
fn a_teammate_s_jeep_makes_a_prone_player_hurt_himself() {
    let text = read("tests/fixtures/vehicles/mp_kharkov-jeep-runover-prone-team-uo.txt");
    let r = retail(&text);
    let Some(o) = ours(
        "prone",
        "allies",
        &[("scr_friendlyfire", "0")],
        false,
        &text,
    ) else {
        return;
    };
    compare_first_drop("prone-team", &r, &o);
    compare(&r, &o, 1000, Some(LIE_DOWN));
    compare_impacts(&r, &o);
}

/// A driverless jeep (its driver out at full throttle a second into the
/// drive) rolls on into a standing victim, and its one strike kills him.
/// `VEH_PlayerDamage` passes the jeep as the attacker too, and the body's
/// origin as the point; the kill lands before the cmd's pmove, so the
/// callback reads the origin the cmd started from (coduo-vehicles.md 11.6).
#[test]
fn a_driverless_jeep_runs_over_a_standing_player() {
    let text = read("tests/fixtures/vehicles/mp_kharkov-jeep-runover-bail-uo.txt");
    let r = retail(&text);
    let Some(o) = ours(
        "stand",
        "axis",
        &[("probe_victim_ahead", "300")],
        true,
        &text,
    ) else {
        return;
    };
    compare_first_drop("bail", &r, &o);
    compare(&r, &o, 1000, None);
    compare_impacts(&r, &o);
}

/// A driverless jeep over a prone victim: 10 a cmd, the jeep both
/// inflictor and attacker, until its probes' strike kills him; the corpse
/// takes that strike's push alone and creeps (coduo-vehicles.md 11.6).
#[test]
fn a_driverless_jeep_runs_over_a_prone_player() {
    let text = read("tests/fixtures/vehicles/mp_kharkov-jeep-runover-bail-prone-uo.txt");
    let r = retail(&text);
    let Some(o) = ours(
        "prone",
        "axis",
        &[("probe_victim_ahead", "300")],
        true,
        &text,
    ) else {
        return;
    };
    compare_first_drop("bail-prone", &r, &o);
    compare(&r, &o, 1000, None);
    compare_impacts(&r, &o);
}
