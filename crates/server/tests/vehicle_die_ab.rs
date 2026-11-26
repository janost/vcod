//! A United Offensive vehicle's death with riders in it, on ours against
//! retail's (`fixtures/vehicles/*-die-*-uo.txt`, `client-probes/probe_vehdie`,
//! recipes in their headers): `Scr_Vehicle_Die`'s 5-point probe hit on each
//! rider through the damage callback, who is put out where he sits and who
//! looks for a dismount spot, and the stock death scripts after it
//! (docs/research/coduo-vehicles.md 11.2). Ours runs the same probe with
//! two clients joined as axis on the capture's clock, each sending the cmds
//! and use taps its probe client's `CMDS` lines logged, and every line from
//! the kill on matches retail's word for word. The `*-free-*` captures put
//! `freeVehicle()` in the kill's place (coduo-vehicles.md 17.4.1).
//!
//! `DIE_REPORT=1` prints both sides' lines. Needs `COD_DIR` at a UO
//! install; without one it returns early.

use crate::common;

use common::{ClientEnd, Join, Queues, step_pair};
use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;
use vcod_common::net::NetClient;
use vcod_common::net::NetEvent;
use vcod_common::net::msg::UserCmd;
use vcod_common::net::protocol::PROTOCOL_V22;

/// The yaw the probe's `PROBE place <t> <n> <spot> <yaw>` line gave client
/// `n`.
fn placed_yaw(log: &[String], n: usize) -> Option<f32> {
    log.iter()
        .filter(|l| l.starts_with("PROBE place "))
        .find(|l| l.split(' ').nth(3) == Some(&n.to_string()))
        .and_then(|l| l.split_whitespace().last()?.parse().ok())
}

/// How long after a client connects ours places him, with the server run
/// alone for `common::PROBE_JOIN_MS` before: the capture's `PROBE place`
/// time for client 0 less this is how much longer ours runs alone, so both
/// clocks agree and the cmds go out on the times the capture logged.
const PLACED_MS: i64 = 1400;

/// The `getTime` of the capture's `PROBE place <t> <n>` line for client `n`.
fn placed_time(capture: &str, n: usize) -> Option<i64> {
    capture
        .lines()
        .filter(|l| l.starts_with("PROBE place "))
        .find(|l| l.split(' ').nth(3) == Some(&n.to_string()))
        .and_then(|l| l.split(' ').nth(2)?.parse().ok())
}

/// Our `PROBE` lines: `probe_vehdie` on `map` with `cvars`, two clients
/// joined as axis on the capture's clock, client 1 as much later as its
/// placement came, each sending the cmds and use taps of the capture's
/// `CMDS` lines, run on until 3 s past the kill.
fn ours(map: &str, cvars: &[(&str, &str)], capture: &str) -> Option<Vec<String>> {
    ours_aborts(map, cvars, capture).map(|(lines, _)| lines)
}

/// [`ours`] with the script errors the run raised.
fn ours_aborts(
    map: &str,
    cvars: &[(&str, &str)],
    capture: &str,
) -> Option<(Vec<String>, Vec<String>)> {
    let fs = vcod_common::testing::uo_fs()?;
    let placed = |n: usize| {
        placed_time(capture, n).unwrap_or_else(|| panic!("the capture's PROBE place for {n}"))
    };
    let late_ms = placed(0) - common::PROBE_JOIN_MS - PLACED_MS;
    let gunner_late_ms = placed(1) - placed(0);
    let p = &PROTOCOL_V22;
    let (mut sv, mut now) = common::Boot {
        proto: Some(p),
        cvars,
        script: Some(include_str!(
            "../../gsc/tests/fixtures/semantics/client-probes/probe_vehdie.gsc"
        )),
        ..common::Boot::new(map, "probe_vehdie")
    }
    .start(fs);
    common::run_alone(&mut sv, &mut now, common::PROBE_JOIN_MS + late_ms);
    let (qa, qb) = (
        Rc::new(RefCell::new(Queues::default())),
        Rc::new(RefCell::new(Queues::default())),
    );
    let mut ca = NetClient::start_with_qport(ClientEnd(qa.clone()), now, 0x2001);
    let mut cb: Option<NetClient<ClientEnd>> = None;
    let (mut ja, mut jb) = (
        Join::new("axis", "kar98k_mp"),
        Join::new("axis", "kar98k_mp"),
    );
    let frame = Duration::from_millis(common::FRAME_MS as u64);
    // Each probe client's cmds on the times, and its use taps on the cmds,
    // of the capture's `CMDS` lines (`driver`, `gunner`): a corpse in the
    // hull gets out of it a cmd at a time (coduo-vehicles.md 11.6), and the
    // gunner's turn at the kill rides how many cmds his seat has run.
    let mut holds: [common::ProbeHold; 2] = Default::default();
    let mut inputs: [Vec<(i32, common::CmdInput)>; 2] = Default::default();
    for (k, who) in ["driver", "gunner"].into_iter().enumerate() {
        holds[k].framed = true;
        holds[k].times = common::cmds_times(capture, who);
        holds[k].ran = common::cmds_ran(capture, who);
        inputs[k] = common::cmds_inputs(capture, who);
        assert!(!holds[k].times.is_empty(), "the capture's {who} CMDS lines");
    }
    let mut killed = None::<i64>;
    for i in 0..3000i64 {
        now += frame;
        if cb.is_none() && i * common::FRAME_MS >= gunner_late_ms {
            cb = Some(NetClient::start_with_qport(
                ClientEnd(qb.clone()),
                now,
                0x2002,
            ));
        }
        let clients = [Some(&mut ca), cb.as_mut()];
        for (k, cl) in clients.into_iter().enumerate() {
            let Some(cl) = cl else { continue };
            let h = &mut holds[k];
            if h.base.is_none() {
                h.base = placed_yaw(sv.script_log(), k).map(|y| [10.0, y, 0.0]);
            }
            if h.base.is_none() || cl.state() != vcod_common::net::NetState::Active {
                if let Some(c) = cl.send_frame(&UserCmd::default()) {
                    h.last = c.server_time;
                }
                continue;
            }
            let mut cmds = h.cmds(cl, 0);
            for c in &mut cmds {
                let at = inputs[k].partition_point(|(st, _)| *st <= c.server_time);
                if let Some(&(_, (b, wb, up))) = at.checked_sub(1).map(|j| &inputs[k][j]) {
                    (c.buttons, c.wbuttons, c.up) = (b, wb, up);
                }
            }
            cl.send_cmds(&cmds);
        }
        let ea = match cb.as_mut() {
            Some(cb) => {
                let (ea, eb) = step_pair(&mut sv, (&qa, &mut ca), (&qb, cb), now);
                for e in eb {
                    if let NetEvent::ServerCommand(tokens) = e {
                        jb.on_server_command(&tokens, cb, now);
                    }
                }
                ea
            }
            None => common::step_at(&mut sv, common::ADDR, &qa, &mut ca, now),
        };
        for e in ea {
            if let NetEvent::ServerCommand(tokens) = e {
                ja.on_server_command(&tokens, &mut ca, now);
            }
        }
        if killed.is_none() && sv.script_log().iter().any(|l| l.starts_with("PROBE kill ")) {
            killed = Some(i);
        }
        if killed.is_some_and(|k| i - k > 60) {
            break;
        }
    }
    assert!(
        killed.is_some(),
        "the vehicle was never killed:\n{}",
        sv.script_log().join("")
    );
    Some((
        sv.script_log()
            .iter()
            .filter(|l| l.starts_with("PROBE "))
            .map(|l| l.trim_end().to_string())
            .collect(),
        sv.script_aborts(),
    ))
}

fn retail(text: &str) -> Vec<String> {
    text.lines()
        .filter(|l| l.starts_with("PROBE "))
        .map(str::to_string)
        .collect()
}

/// The lines from the kill on, within a second of it, each time made
/// relative to the kill's.
fn from_kill(lines: &[String]) -> Vec<String> {
    let time = |l: &str| l.split(' ').nth(2).and_then(|t| t.parse::<i32>().ok());
    let Some(kill) = lines
        .iter()
        .find(|l| l.starts_with("PROBE kill "))
        .and_then(|l| time(l))
    else {
        return Vec::new();
    };
    lines
        .iter()
        .skip_while(|l| !l.starts_with("PROBE kill "))
        .filter_map(|l| {
            let t = time(l)?;
            (t - kill <= 1000).then(|| {
                let mut w: Vec<String> = l.split(' ').map(str::to_string).collect();
                w[2] = format!("+{}", t - kill);
                w.join(" ")
            })
        })
        .collect()
}

/// Retail's lines against ours from the kill on, one for one and word for
/// word.
fn compare(retail: &[String], ours: &[String]) {
    let (r, o) = (from_kill(retail), from_kill(ours));
    if std::env::var_os("DIE_REPORT").is_some() {
        for l in &r {
            eprintln!("retail {l}");
        }
        for l in &o {
            eprintln!("ours   {l}");
        }
    }
    assert_eq!(r, o);
}

#[test]
fn the_tank_killed_by_itself_puts_both_riders_out_where_they_sit() {
    let text = std::fs::read_to_string("tests/fixtures/vehicles/mp_cassino-tank-die-self-uo.txt")
        .expect("the capture");
    let r = retail(&text);
    let Some(o) = ours("mp_cassino", &[("probe_vehicletype", "panzeriv_mp")], &text) else {
        return;
    };
    compare(&r, &o);
}

#[test]
fn the_tank_killed_by_its_gunner_dismounts_the_driver() {
    let text = std::fs::read_to_string("tests/fixtures/vehicles/mp_cassino-tank-die-gunner-uo.txt")
        .expect("the capture");
    let r = retail(&text);
    let cvars = [
        ("probe_vehicletype", "panzeriv_mp"),
        ("probe_die_by", "gunner"),
    ];
    let Some(o) = ours("mp_cassino", &cvars, &text) else {
        return;
    };
    compare(&r, &o);
}

#[test]
fn the_jeep_killed_by_a_teammate_dismounts_its_driver() {
    let text = std::fs::read_to_string("tests/fixtures/vehicles/mp_peaks-jeep-die-teammate-uo.txt")
        .expect("the capture");
    let r = retail(&text);
    let cvars = [
        ("probe_vehicletype", "willyjeep_mp"),
        ("probe_die_by", "gunner"),
        ("probe_side", "75"),
        ("scr_friendlyfire", "0"),
        ("probe_riders", "1"),
    ];
    let Some(o) = ours("mp_peaks", &cvars, &text) else {
        return;
    };
    compare(&r, &o);
}

/// `freeVehicle()` on the tank with both riders in it (coduo-vehicles.md
/// 17.4.1): each rider's stock `tank_death_player_damage` wakes on the
/// `death` and reads `last_attacker` off the freed handle, the script error
/// that ends retail's server.
#[test]
fn freeing_the_occupied_tank_trips_the_stock_rider_thread() {
    let text = std::fs::read_to_string("tests/fixtures/vehicles/mp_cassino-tank-free-uo.txt")
        .expect("the capture");
    // run_probe.sh's: the error ends the thread (retail's whole server).
    let cvars = [
        ("probe_vehicletype", "panzeriv_mp"),
        ("probe_kill", "free"),
        ("developer", "1"),
    ];
    let Some((o, aborts)) = ours_aborts("mp_cassino", &cvars, &text) else {
        return;
    };
    let (r, o) = (from_kill(&retail(&text)), from_kill(&o));
    assert_eq!(r, o[..r.len().min(o.len())]);
    assert_eq!(
        aborts,
        [
            "maps/mp/_tankdrive_gmi::tank_death_player_damage:461: BadType(\"dead entity is not an object\")"
        ]
    );
}

/// The same with the stock rider threads ended first: retail's next client
/// think reads the riders' freed record and segfaults, so its lines end at
/// the `death`. Ours puts each rider out where he sits instead.
#[test]
fn freeing_the_occupied_tank_puts_the_riders_out_where_retail_faults() {
    let text = std::fs::read_to_string("tests/fixtures/vehicles/mp_cassino-tank-free-quiet-uo.txt")
        .expect("the capture");
    let cvars = [
        ("probe_vehicletype", "panzeriv_mp"),
        ("probe_kill", "free_quiet"),
    ];
    let Some((o, aborts)) = ours_aborts("mp_cassino", &cvars, &text) else {
        return;
    };
    assert_eq!(aborts, Vec::<String>::new());
    let (r, o) = (from_kill(&retail(&text)), from_kill(&o));
    assert_eq!(r, o[..r.len()]);
    let frame: Vec<&str> = o[r.len()..]
        .iter()
        .filter(|l| l.contains(" frame "))
        .map(String::as_str)
        .collect();
    assert_eq!(frame.len(), 2, "{o:?}");
    assert!(
        frame.iter().all(|l| l.contains(" veh 0 health 100 ")),
        "{frame:?}"
    );
}
