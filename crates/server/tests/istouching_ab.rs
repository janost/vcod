//! `isTouching` against a standing, crouched, prone and dead player,
//! against retail 1.1d, 1.5b and UO 1.51b (`client-probes/
//! probe_istouching.gsc` on mp_carentan, `tools/run_probe.sh`,
//! 2026-10-21; captures in `fixtures/istouching/`, recipe in the client
//! probes' README). The partner is the map's trigger_hurt brush, swept
//! past the player's box face by face (cod11-gsc-object-model.md 22.3).
//!
//! Each test needs `COD_DIR` at its install; otherwise it returns early.

use crate::common;

use common::{Join, Queues};
use std::cell::RefCell;
use std::rc::Rc;
use std::time::{Duration, Instant};
use vcod_common::net::msg::UserCmd;
use vcod_common::net::{NetClient, NetEvent};

/// The `PROBE` lines of a log.
fn probe_lines<'a>(lines: impl Iterator<Item = &'a str>) -> Vec<&'a str> {
    lines.filter(|l| l.starts_with("PROBE")).collect()
}

/// The `wbuttons` of a `f "PROBE_IN <buttons> <wbuttons> <up>"` command.
fn direct_wbuttons(tokens: &[String]) -> Option<u8> {
    if tokens.first().map(String::as_str) != Some("f") {
        return None;
    }
    let mut t = tokens.get(1)?.strip_prefix("PROBE_IN ")?.split_whitespace();
    t.next()?;
    t.next()?.parse().ok()
}

/// Runs the probe on ours with one axis `--probe-uo-weapon direct` client
/// until it logs `PROBE done`; its `PROBE` lines.
fn ours(fs: vcod_common::pk3::Pk3Fs) -> Vec<String> {
    let proto = vcod_server::server::install_protocol(&fs);
    let (map, probe) = ("mp_carentan", "probe_istouching");
    let bsp_path = fs.resolve_map(map).expect("map in the paks");
    let bsp = vcod_common::bsp::parse(&fs.read(&bsp_path).unwrap()).unwrap();
    let mut now = Instant::now();
    let mut sv = vcod_server::Server::new(common::cfg(map, probe), now);
    sv.set_protocol(proto);
    let src = std::fs::read_to_string(format!(
        "../gsc/tests/fixtures/semantics/client-probes/{probe}.gsc"
    ))
    .expect("the probe");
    sv.overlay_script(&format!("maps/mp/gametypes/{probe}"), &src);
    sv.load_world(vcod_server::world::World::from_bsp(&bsp, Some(&fs)));
    sv.load_scripts(Rc::new(fs)).expect("load the scripts");
    common::run_alone(&mut sv, &mut now, 3000);
    let q = Rc::new(RefCell::new(Queues::default()));
    let mut cl = NetClient::start_with_qport(common::ClientEnd(q.clone()), now, 0x2001);
    let mut join = Join::new("axis", "kar98k_mp");
    let mut hold = common::ProbeHold {
        aim: true,
        ..Default::default()
    };
    let (mut wbuttons, mut selected) = (0u8, None::<u8>);
    let frame = Duration::from_millis(common::FRAME_MS as u64);
    for _ in 0..3000 {
        if sv.script_log().iter().any(|l| l.contains("PROBE done")) {
            break;
        }
        now += frame;
        let live = cl
            .snapshots()
            .newest()
            .is_some_and(|s| s.ps.field_i32(cl.protocol(), "pm_type") == 0);
        if live {
            let mut cmds = hold.cmds(&cl, 0);
            for c in &mut cmds {
                c.wbuttons = wbuttons;
                if let Some(w) = selected {
                    c.weapon = w;
                }
            }
            cl.send_cmds(&cmds);
        } else if let Some(c) = cl.send_frame(&UserCmd::default()) {
            hold.last = c.server_time;
        }
        let pending: Vec<Vec<u8>> = q.borrow_mut().to_server.drain(..).collect();
        for p in pending {
            sv.handle_packet(common::ADDR, &p, now);
        }
        sv.tick(now);
        for (_, p) in sv.take_outgoing() {
            q.borrow_mut().to_client.push_back(p);
        }
        for e in cl.pump_at(now) {
            if let NetEvent::ServerCommand(tokens) = e {
                if tokens.first().map(String::as_str) == Some("a") {
                    selected = tokens.get(1).and_then(|w| w.parse().ok());
                }
                if let Some(w) = direct_wbuttons(&tokens) {
                    wbuttons = w;
                }
                join.on_server_command(&tokens, &mut cl, now);
            }
        }
    }
    sv.script_log()
        .iter()
        .map(|l| l.trim().to_string())
        .filter(|l| l.starts_with("PROBE"))
        .collect()
}

fn check(fs: Option<vcod_common::pk3::Pk3Fs>, fixture: &str) {
    let Some(fs) = fs else {
        return;
    };
    let ours = ours(fs);
    if std::env::var_os("ISTOUCHING_REPORT").is_some() {
        for l in &ours {
            eprintln!("{l}");
        }
    }
    let retail = std::fs::read_to_string(format!("tests/fixtures/istouching/{fixture}"))
        .expect("the capture");
    assert_eq!(
        probe_lines(ours.iter().map(String::as_str)),
        probe_lines(retail.lines())
    );
}

/// Needs `COD_DIR` at a 1.1 install.
#[test]
fn is_touching_follows_the_stance_box_like_retail_1_1() {
    check(vcod_common::testing::game_fs_11(), "mp_carentan-1.1.txt");
}

/// Needs `COD_DIR` at a 1.5 install.
#[test]
fn is_touching_follows_the_stance_box_like_retail_1_5() {
    let fs = vcod_common::testing::game_fs().filter(|fs| {
        vcod_server::server::install_protocol(fs).transport.version
            == vcod_common::net::protocol::PROTOCOL_V6.transport.version
    });
    check(fs, "mp_carentan-1.5.txt");
}

/// Needs `COD_DIR` at a UO install.
#[test]
fn is_touching_follows_the_stance_box_like_retail_uo() {
    check(vcod_common::testing::uo_fs(), "mp_carentan-uo.txt");
}
