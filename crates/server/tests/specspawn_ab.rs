//! A spectator's spawn think, ours against retail's. `client-probes/probe_specspawn`
//! spawns a spectator who holds forward again as a spectator every 2 s.
//! 1.5b's and UO's `ClientSpawn` run the client's last cmd for a spectator
//! as for a player, so the spawn frame's snapshot has him flown 100 ms off
//! the spot; 1.1d's null cmd leaves him on it (cod15-movement.md 4, "The
//! spawn's lie-down"). The fixtures' `EDGE origin` lines are the probe
//! client's snapshots on the spawn frames.
//!
//! The script's own read of `self.origin` straight after the `spawn` (the
//! `PROBE spec` lines) shows the think's origin too: retail's think runs
//! inside the builtin.
//!
//! The tests need `COD_DIR` at the install each names and return early
//! without.

use crate::common;

use common::{Queues, step};
use std::cell::RefCell;
use std::rc::Rc;
use std::time::{Duration, Instant};
use vcod_common::net::NetClient;
use vcod_common::net::msg::UserCmd;
use vcod_common::net::protocol::PROTOCOL_V6;

const SCRIPT: &str =
    include_str!("../../gsc/tests/fixtures/semantics/client-probes/probe_specspawn.gsc");

/// The capture prints origins to the thousandth.
const NEAR: f32 = 0.002;

/// The spawn frames' snapshot origins.
fn retail_spawns(fixture: &str) -> Vec<[f32; 3]> {
    fixture
        .lines()
        .filter_map(|l| l.strip_prefix("EDGE origin "))
        .map(|l| {
            let o = l.split_once("origin=").unwrap().1;
            let v: Vec<f32> = o.split(',').map(|x| x.parse().unwrap()).collect();
            [v[0], v[1], v[2]]
        })
        .collect()
}

/// The origins the `PROBE spec` lines print, past the time and number.
fn script_origins<'a>(lines: impl Iterator<Item = &'a str>) -> Vec<String> {
    lines
        .filter_map(|l| l.strip_prefix("PROBE spec "))
        .map(|l| l.splitn(3, ' ').nth(2).unwrap().to_string())
        .collect()
}

/// The `PROBE spec` lines' times.
fn spawn_times(log: &[String]) -> Vec<i32> {
    log.iter()
        .filter_map(|l| l.strip_prefix("PROBE spec "))
        .map(|l| l.split(' ').next().unwrap().parse().unwrap())
        .collect()
}

fn check(fs: vcod_common::pk3::Pk3Fs, fixture: &str) {
    let proto = vcod_server::server::install_protocol(&fs);
    let map = "mp_carentan";
    let bsp_path = fs.resolve_map(map).expect("map in the paks");
    let bsp = vcod_common::bsp::parse(&fs.read(&bsp_path).unwrap()).unwrap();
    let mut now = Instant::now();
    let mut sv = vcod_server::Server::new(common::cfg(map, "probe_specspawn"), now);
    sv.set_protocol(proto);
    sv.overlay_script("maps/mp/gametypes/probe_specspawn", SCRIPT);
    sv.load_world(vcod_server::world::World::from_bsp(&bsp, Some(&fs)));
    sv.load_scripts(Rc::new(fs)).expect("load the scripts");
    let q = Rc::new(RefCell::new(Queues::default()));
    let mut cl = NetClient::start_with_qport(common::ClientEnd(q.clone()), now, 0x2001);
    let frame = Duration::from_millis(common::FRAME_MS as u64);
    let held = UserCmd {
        forward: 127,
        ..Default::default()
    };
    let mut ours = Vec::new();
    for _ in 0..200 {
        now += frame;
        cl.send_frame(&held);
        step(&mut sv, &q, &mut cl, now);
        let times = spawn_times(sv.script_log());
        if let Some(s) = cl.snapshots().newest()
            && times.contains(&s.server_time)
            && ours.last().is_none_or(|&(t, _)| t != s.server_time)
        {
            let f = |n: &str| f32::from_bits(s.ps.field_i32(cl.protocol(), n) as u32);
            ours.push((
                s.server_time,
                [f("origin[0]"), f("origin[1]"), f("origin[2]")],
            ));
        }
        if ours.len() >= 2 {
            break;
        }
    }
    let retail = retail_spawns(fixture);
    assert!(ours.len() >= 2, "no spectator spawn: {:?}", sv.script_log());
    for (t, o) in &ours {
        assert!(
            (0..3).all(|i| (o[i] - retail[0][i]).abs() <= NEAR),
            "retail's spawn frame has him at {:?}, ours at {t} at {o:?}",
            retail[0]
        );
    }
    let theirs = script_origins(fixture.lines());
    let mine = script_origins(sv.script_log().iter().map(String::as_str));
    assert!(!mine.is_empty());
    for o in &mine {
        assert_eq!(o, &theirs[0], "the script's read after the spawn");
    }
}

#[test]
fn a_spectator_spawn_runs_a_null_cmd_like_retail() {
    let Some(fs) = vcod_common::testing::game_fs_11() else {
        return;
    };
    check(fs, include_str!("fixtures/specspawn/mp_carentan.txt"));
}

#[test]
fn a_spectator_spawn_runs_the_last_cmd_like_15() {
    let Some(fs) = vcod_common::testing::game_fs() else {
        return;
    };
    if vcod_server::server::install_protocol(&fs).transport.version != PROTOCOL_V6.transport.version
    {
        return;
    }
    check(fs, include_str!("fixtures/specspawn/mp_carentan-1.5.txt"));
}

#[test]
fn a_spectator_spawn_runs_the_last_cmd_like_uo() {
    let Some(fs) = vcod_common::testing::uo_fs() else {
        return;
    };
    check(fs, include_str!("fixtures/specspawn/mp_carentan-uo.txt"));
}
