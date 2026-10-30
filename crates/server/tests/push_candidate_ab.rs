//! `G_MoverPush`'s candidate trace against the retail capture
//! `movers/mp_carentan-dm-pushcand.txt` (its header carries the recipe):
//! `client-probes/probe_pushcand.gsc` runs the slab into one player standing
//! clear, two stacked on one spot, two overlapping by half a box and one
//! sunk into the floor. The trace is a full `trap_Trace` that must name the
//! pusher, so the world or another player met first keeps a player off the
//! list and the slab passes through him (docs/research/cod11-movers.md 12,
//! "The candidate trace"). A spectator joins first, as on retail, which
//! keeps `StuckInClient` off and the stacked pair stacked. Where the slab
//! names itself first a pair it cannot place stalls it on every frame,
//! the last frame of its move included (section 12, "A blocked mover").
//!
//! Both allied clients replay the capture's cmds (`ProbeHold::framed`), one
//! packet a cmd, the two clients' packets in the order their sends' wall
//! clocks (the `CMDS` lines' `w=`) put them:
//! where a `setOrigin` drop rests rides the cmd split (movers doc 21), and
//! which of a stalled pair moves first, while the blocked push leaves the
//! other out of the tree, rides the packet order.
//!
//! The same recipe on 1.5b (`-1.5.txt`) and UO (`-uo.txt`): there the
//! spectator does not keep `StuckInClient` off, so the stacked and half
//! pairs are shoved apart before the slab moves (movers doc 12, "The
//! candidate trace"). Which way rides retail's `rand()`, so ours draws each
//! push's jitter from glibc's `rand()` under the seed recovered off the
//! capture's `RIDE` lines (`Server::pin_stuck_rand`, player-clip doc 13.4).
//!
//! `PUSHCAND_REPORT=1` prints both logs side by side.
//!
//! Needs `COD_DIR`; without the paks the test returns early.

use crate::common;

use common::{ADDR, ADDR_B, ADDR_C, ClientEnd, FRAME_MS, Join, ProbeHold, Queues, holding};
use std::cell::RefCell;
use std::rc::Rc;
use std::time::{Duration, Instant};
use vcod_common::net::protocol::PROTOCOL_V6;
use vcod_common::net::{NetClient, NetEvent};

const MAP: &str = "mp_carentan";
const FIXTURE: &str = "tests/fixtures/movers/mp_carentan-dm-pushcand.txt";
const FIXTURE_15: &str = "tests/fixtures/movers/mp_carentan-dm-pushcand-1.5.txt";
const FIXTURE_UO: &str = "tests/fixtures/movers/mp_carentan-dm-pushcand-uo.txt";

/// How long before its first snapshot a capture's client starts joining.
const FIRST_SNAP_AFTER_JOIN_MS: i64 = 400;
/// The spectator joined this long before the first allied client.
const SPECTATOR_AHEAD_MS: i64 = 2000;
/// The capture's allied clients, slots 1 and 2.
const WHO: [&str; 2] = ["p1", "p2"];

/// Every logged origin, `tag slot (x, y, z)`, the slab's as slot `bz`, the
/// frame tags with their level times.
fn rows<'a>(lines: impl Iterator<Item = &'a str>) -> Vec<String> {
    lines
        .filter_map(|l| l.find("PROBE ").map(|i| l[i + 6..].trim_end()))
        .filter(|l| {
            l.splitn(3, ' ')
                .nth(2)
                .is_some_and(|at| at.starts_with('('))
        })
        .map(str::to_string)
        .collect()
}

/// The glibc `srand` seed of the capture's server and the `rand()` draws
/// before its first `StuckInClient` push, recovered off the `RIDE` lines
/// (player-clip doc 13.4); 1.1's spectator keeps the push off.
fn stuck_seed(fixture: &str) -> Option<(u32, usize)> {
    match fixture {
        FIXTURE_15 => Some((428, 8)),
        FIXTURE_UO => Some((439, 8)),
        _ => None,
    }
}

struct Ours {
    log: Vec<String>,
    /// Each allied client's first snapshot time.
    first: [Option<i32>; 2],
}

/// The probe on ours: the spectator, then both allied clients joined so
/// their first snapshots fall on retail's, `late` starting each that much
/// earlier.
fn run_once(
    fs: vcod_common::pk3::Pk3Fs,
    retail: &str,
    seed: Option<(u32, usize)>,
    late: [i64; 2],
) -> Ours {
    let p = vcod_server::server::install_protocol(&fs);
    let bsp_path = fs.resolve_map(MAP).expect("map in the mounted paks");
    let bsp = vcod_common::bsp::parse(&fs.read(&bsp_path).unwrap()).unwrap();
    let mut now = Instant::now();
    let mut sv = vcod_server::Server::new(common::cfg(MAP, "probe_pushcand"), now);
    sv.set_protocol(p);
    if let Some((seed, skip)) = seed {
        sv.pin_stuck_rand(seed, skip);
    }
    sv.overlay_script(
        "maps/mp/gametypes/probe_pushcand",
        include_str!("../../gsc/tests/fixtures/semantics/client-probes/probe_pushcand.gsc"),
    );
    sv.load_world(vcod_server::world::World::from_bsp(&bsp, Some(&fs)));
    sv.load_scripts(Rc::new(fs)).expect("load the scripts");

    let mut holds: [ProbeHold; 2] = WHO.map(|who| ProbeHold {
        base: Some([0.0; 3]),
        framed: true,
        times: common::cmds_times(retail, who),
        ran: common::cmds_ran(retail, who),
        ..Default::default()
    });
    // Each client's cmd times to the wall clock of their sends.
    let walls: [std::collections::BTreeMap<i32, u64>; 2] =
        WHO.map(|who| common::cmds_walls(retail, who).into_iter().collect());
    let starts: [i64; 2] = std::array::from_fn(|i| {
        holds[i].ran.first().expect("the capture's CMDS lines").0 as i64
            - FIRST_SNAP_AFTER_JOIN_MS
            - late[i]
    });
    let spec_start = starts[0] - SPECTATOR_AHEAD_MS;
    let addrs = [ADDR, ADDR_B, ADDR_C];
    let q: [Rc<RefCell<Queues>>; 3] = Default::default();
    let mut cls: [Option<NetClient<ClientEnd>>; 3] = Default::default();
    let mut joins = [
        Join::new("spectator", "m1carbine_mp"),
        Join::new("allies", "m1carbine_mp"),
        Join::new("allies", "m1carbine_mp"),
    ];
    let mut first = [None; 2];
    for _ in 0..3000 {
        now += Duration::from_millis(FRAME_MS as u64);
        let t = sv.game_time_ms() as i64;
        for (k, start) in [spec_start, starts[0], starts[1]].into_iter().enumerate() {
            if cls[k].is_none() && t >= start {
                cls[k] = Some(NetClient::start_with_qport(
                    ClientEnd(q[k].clone()),
                    now,
                    0x2001 + k as u16,
                ));
            }
        }
        // The spectator's frame, then the allied clients' cmds one packet
        // each, in the order of their times.
        if let Some(cl) = cls[0].as_mut() {
            cl.send_frame(&holding(cl));
        }
        let mut cmds = Vec::new();
        for i in 0..2 {
            let Some(cl) = cls[i + 1].as_mut() else {
                continue;
            };
            let live = cl
                .snapshots()
                .newest()
                .is_some_and(|s| s.ps.field_i32(p, "pm_type") <= 1);
            if live {
                cmds.extend(holds[i].cmds(cl, 0).into_iter().map(|c| {
                    let wall = walls[i].get(&c.server_time).copied().unwrap_or(u64::MAX);
                    (wall, c.server_time, i, c)
                }));
            } else if let Some(c) = cl.send_frame(&holding(cl)) {
                holds[i].last = c.server_time;
            }
        }
        // Captured cmds in the order retail read them, any other after.
        cmds.sort_by_key(|c| (c.0, c.1, c.2));
        let drain = |k: usize| -> Vec<(usize, Vec<u8>)> {
            q[k].borrow_mut()
                .to_server
                .drain(..)
                .map(|p| (k, p))
                .collect()
        };
        let mut packets = drain(0);
        packets.extend(drain(1));
        packets.extend(drain(2));
        for (_, _, i, c) in cmds {
            cls[i + 1].as_mut().unwrap().send_cmds(&[c]);
            packets.extend(drain(i + 1));
        }
        for (k, p) in packets {
            sv.handle_packet(addrs[k], &p, now);
        }
        sv.tick(now);
        for (to, p) in sv.take_outgoing() {
            if let Some(k) = addrs.iter().position(|a| *a == to) {
                q[k].borrow_mut().to_client.push_back(p);
            }
        }
        for k in 0..3 {
            let Some(cl) = cls[k].as_mut() else { continue };
            for e in cl.pump_at(now) {
                match e {
                    NetEvent::ServerCommand(tokens) => joins[k].on_server_command(&tokens, cl, now),
                    NetEvent::Dropped(r) => panic!("client {k} dropped: {r}"),
                    _ => {}
                }
            }
            if k > 0
                && let Some(s) = cl.snapshots().newest()
            {
                first[k - 1].get_or_insert(s.server_time);
            }
        }
        if sv.script_log().iter().any(|l| l.contains("PROBE done")) {
            break;
        }
    }
    assert_eq!(sv.script_aborts(), Vec::<String>::new());
    Ours {
        log: sv.script_log().to_vec(),
        first,
    }
}

/// Runs the probe on ours under `fs`'s protocol and diffs every logged
/// origin with `fixture`'s.
fn check(fs: fn() -> Option<vcod_common::pk3::Pk3Fs>, fixture: &str) {
    let Some(first_fs) = fs() else {
        eprintln!("no install of this version under COD_DIR: skipping");
        return;
    };
    let retail_text = std::fs::read_to_string(fixture).unwrap();
    let mut ours = run_once(first_fs, &retail_text, stuck_seed(fixture), [0, 0]);
    // A join that took a frame more or less than retail's runs again,
    // shifted by the difference.
    let want: [i32; 2] = WHO.map(|who| common::cmds_ran(&retail_text, who)[0].0);
    if ours.first.iter().zip(want).any(|(f, w)| *f != Some(w)) {
        let late = std::array::from_fn(|i| ours.first[i].map_or(0, |f| (f - want[i]) as i64));
        ours = run_once(fs().unwrap(), &retail_text, stuck_seed(fixture), late);
    }
    assert!(
        ours.log.iter().any(|l| l.contains("PROBE done")),
        "the probe never finished on ours"
    );
    let ours = rows(ours.log.iter().map(String::as_str));
    let retail = rows(retail_text.lines());
    if std::env::var_os("PUSHCAND_REPORT").is_some() {
        for (r, o) in retail.iter().zip(&ours) {
            println!("{} {r:<44} | {o}", if r == o { "  " } else { "!!" });
        }
    }
    assert!(retail.len() > 50, "the fixture has no rows");
    assert_eq!(ours, retail, "\nours:\n{}", ours.join("\n"));
}

/// The 1.5b install's paks, `None` elsewhere (a UO install's `main/` is
/// 1.5's, so this runs there too).
fn fs_15() -> Option<vcod_common::pk3::Pk3Fs> {
    let fs = vcod_common::testing::game_fs()?;
    (vcod_server::server::install_protocol(&fs).transport.version == PROTOCOL_V6.transport.version)
        .then_some(fs)
}

#[test]
fn a_push_lists_only_players_its_trace_names_the_pusher_for() {
    check(vcod_common::testing::game_fs_11, FIXTURE);
}

#[test]
fn a_push_lists_only_players_its_trace_names_the_pusher_for_on_15() {
    check(fs_15, FIXTURE_15);
}

#[test]
fn a_push_lists_only_players_its_trace_names_the_pusher_for_on_uo() {
    check(vcod_common::testing::uo_fs, FIXTURE_UO);
}
