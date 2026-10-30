//! A client between its connect and its first spawn, ours against retail's.
//! `client-probes/probe_noclass` logs the client's fields at the connect
//! callback, 3 s later and straight after a `spawn`; the fixtures are its
//! `games_mp.log` and the probe client's snapshot origins on 1.1d, 1.5b and
//! UO 1.51b (cod11-gametypes-re-bel.md 8.3).
//!
//! The tests need `COD_DIR` at the install each names and return early
//! without.

use crate::common;

use common::{Queues, step};
use std::cell::RefCell;
use std::rc::Rc;
use std::time::{Duration, Instant};
use vcod_common::net::NetClient;
use vcod_common::net::msg::NULL_USERCMD;
use vcod_common::net::protocol::PROTOCOL_V6;

const SCRIPT: &str =
    include_str!("../../gsc/tests/fixtures/semantics/client-probes/probe_noclass.gsc");

fn probe_lines<'a>(lines: impl Iterator<Item = &'a str>) -> Vec<String> {
    lines
        .map(str::trim_end)
        .filter(|l| l.starts_with("PROBE "))
        .map(str::to_string)
        .collect()
}

fn check(fs: vcod_common::pk3::Pk3Fs, fixture: &str) {
    let proto = vcod_server::server::install_protocol(&fs);
    let map = "mp_carentan";
    let bsp_path = fs.resolve_map(map).expect("map in the paks");
    let bsp = vcod_common::bsp::parse(&fs.read(&bsp_path).unwrap()).unwrap();
    let mut now = Instant::now();
    let mut sv = vcod_server::Server::new(common::cfg(map, "probe_noclass"), now);
    sv.set_protocol(proto);
    sv.overlay_script("maps/mp/gametypes/probe_noclass", SCRIPT);
    sv.load_world(vcod_server::world::World::from_bsp(&bsp, Some(&fs)));
    sv.load_scripts(Rc::new(fs)).expect("load the scripts");
    let q = Rc::new(RefCell::new(Queues::default()));
    let mut cl = NetClient::start_with_qport(common::ClientEnd(q.clone()), now, 0x2001);
    let frame = Duration::from_millis(common::FRAME_MS as u64);
    let spawned = |log: &[String]| log.iter().any(|l| l.starts_with("PROBE spawned"));
    // Retail's probe client reads `ps.origin` 0 on every snapshot until the
    // spawn (the fixture's SNAP lines).
    let mut before_spawn = 0;
    for _ in 0..200 {
        now += frame;
        cl.send_frame(&NULL_USERCMD);
        step(&mut sv, &q, &mut cl, now);
        if spawned(sv.script_log()) {
            break;
        }
        if let Some(s) = cl.snapshots().newest() {
            let f = |n: &str| f32::from_bits(s.ps.field_i32(cl.protocol(), n) as u32);
            let o = [f("origin[0]"), f("origin[1]"), f("origin[2]")];
            assert_eq!(
                o, [0.0; 3],
                "snapshot at {} before the spawn",
                s.server_time
            );
            before_spawn += 1;
        }
    }
    assert!(
        before_spawn > 20,
        "{before_spawn} snapshots before the spawn"
    );
    let retail = probe_lines(fixture.lines());
    let ours = probe_lines(sv.script_log().iter().map(String::as_str));
    assert_eq!(ours, retail);
}

#[test]
fn a_connected_client_reads_as_retail_11() {
    let Some(fs) = vcod_common::testing::game_fs_11() else {
        return;
    };
    check(fs, include_str!("fixtures/noclass/mp_carentan.txt"));
}

#[test]
fn a_connected_client_reads_as_retail_15() {
    let Some(fs) = vcod_common::testing::game_fs() else {
        return;
    };
    if vcod_server::server::install_protocol(&fs).transport.version != PROTOCOL_V6.transport.version
    {
        return;
    }
    check(fs, include_str!("fixtures/noclass/mp_carentan-1.5.txt"));
}

#[test]
fn a_connected_client_reads_as_retail_uo() {
    let Some(fs) = vcod_common::testing::uo_fs() else {
        return;
    };
    check(fs, include_str!("fixtures/noclass/mp_carentan-uo.txt"));
}
