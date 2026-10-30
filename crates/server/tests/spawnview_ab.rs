//! What a script reads off a player straight after `spawn`, ours against
//! retail's. `client-probes/probe_spawnview` spawns the client on
//! mp_carentan's first deathmatch spot at several heights and angles, then
//! as a spectator and an intermission camera; the fixtures are its
//! `games_mp.log` on 1.1d, 1.5b and UO 1.51b (cod11-gametypes-re-bel.md 8.5).
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
    include_str!("../../gsc/tests/fixtures/semantics/client-probes/probe_spawnview.gsc");

/// The probe's lines less the `next` ones, which ride the probe client's
/// own cmds. Off 1.1 the spawn's think runs the client's last cmd, whose
/// keys moved retail's body sideways, so an origin keeps only its z there.
fn probe_lines<'a>(lines: impl Iterator<Item = &'a str>, v15: bool) -> Vec<String> {
    lines
        .map(str::trim_end)
        .filter(|l| l.starts_with("PROBE ") && !l.contains(" next "))
        .map(|l| match l.split_once(" origin ") {
            Some((head, xyz)) if v15 => {
                format!("{head} origin z {}", xyz.rsplit(' ').next().unwrap())
            }
            _ => l.to_string(),
        })
        .collect()
}

fn check(fs: vcod_common::pk3::Pk3Fs, fixture: &str) {
    let proto = vcod_server::server::install_protocol(&fs);
    let v15 = proto.transport.version != vcod_common::net::protocol::PROTOCOL_V1.transport.version;
    let map = "mp_carentan";
    let bsp_path = fs.resolve_map(map).expect("map in the paks");
    let bsp = vcod_common::bsp::parse(&fs.read(&bsp_path).unwrap()).unwrap();
    let mut now = Instant::now();
    let mut sv = vcod_server::Server::new(common::cfg(map, "probe_spawnview"), now);
    sv.set_protocol(proto);
    sv.overlay_script("maps/mp/gametypes/probe_spawnview", SCRIPT);
    sv.load_world(vcod_server::world::World::from_bsp(&bsp, Some(&fs)));
    sv.load_scripts(Rc::new(fs)).expect("load the scripts");
    let q = Rc::new(RefCell::new(Queues::default()));
    let mut cl = NetClient::start_with_qport(common::ClientEnd(q.clone()), now, 0x2001);
    let frame = Duration::from_millis(common::FRAME_MS as u64);
    let done = |log: &[String]| log.iter().any(|l| l.starts_with("PROBE inter angles"));
    for _ in 0..600 {
        now += frame;
        cl.send_frame(&NULL_USERCMD);
        step(&mut sv, &q, &mut cl, now);
        if done(sv.script_log()) {
            break;
        }
    }
    let retail = probe_lines(fixture.lines(), v15);
    let ours = probe_lines(sv.script_log().iter().map(String::as_str), v15);
    assert_eq!(ours, retail);
}

#[test]
fn a_spawned_player_reads_as_retail_11() {
    let Some(fs) = vcod_common::testing::game_fs_11() else {
        return;
    };
    check(fs, include_str!("fixtures/spawnview/mp_carentan.txt"));
}

#[test]
fn a_spawned_player_reads_as_retail_15() {
    let Some(fs) = vcod_common::testing::game_fs() else {
        return;
    };
    if vcod_server::server::install_protocol(&fs).transport.version != PROTOCOL_V6.transport.version
    {
        return;
    }
    check(fs, include_str!("fixtures/spawnview/mp_carentan-1.5.txt"));
}

#[test]
fn a_spawned_player_reads_as_retail_uo() {
    let Some(fs) = vcod_common::testing::uo_fs() else {
        return;
    };
    check(fs, include_str!("fixtures/spawnview/mp_carentan-uo.txt"));
}
