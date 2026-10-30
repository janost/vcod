//! The prematch period against retail: `play-1.7-prematch.txt`
//! (`crates/cod4/tests/fixtures/net/`, recipe in its header) is one
//! `cod4_probe` (`--team allies --class assault_mp,0 --script idle`)
//! joining a fresh retail `cod4_lnxded` 1.7 `war` level on mp_crash 3 s
//! after the server started, so its first snapshot falls inside
//! `_globallogic::matchStartTimer`: "Waiting for teams" over a 15 s timer,
//! then "Match begins in" over 5 s from 14800, both gone at 19800
//! (cod4-hud.md 5.1). The same probe joins [`Cod4Server`] in process on
//! retail's level clock; every HUD element of every snapshot both sent at
//! one server time is diffed (`wire_ab::hud_diffs`), and the server
//! commands it read, in order (the prematch's `ui_hud_hardcore` among
//! them). `COD4_PRE_CMDS=1` prints both command lists. Needs `COD4_DIR`.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use super::wire_ab::{Probe, hud_diffs, retail, side};
use super::*;

#[test]
fn the_prematch_hud_matches_retail() {
    let Some(install) = vcod_cod4::testing::cod4_install() else {
        return;
    };
    let r = side(&retail("prematch"));
    let first = r.snaps.first().unwrap().server_time;
    let run_ms = r.snaps.last().unwrap().server_time;
    let spawn = r
        .snaps
        .iter()
        .find(|f| f.ps.get("pm_type") == Some(0))
        .unwrap();
    let origin = [0, 1, 2].map(|k| spawn.ps.get_f32(&format!("origin[{k}]")).unwrap());
    let yaw = spawn.ps.get_f32("viewangles[1]").unwrap();

    let cs0 = String::from_utf8_lossy(r.gs.configstring(0)).into_owned();
    let mut sv = Cod4Server::new(
        Cod4Config {
            map: "mp_crash".into(),
            gametype: "war".into(),
            hostname: "CoD4Host".into(),
            max_clients: 8,
        },
        7,
    );
    for (k, v) in vcod_common::net::connectionless::info_pairs(&cs0) {
        if !matches!(k, "protocol" | "shortversion") {
            sv.set_cvar(k, v);
        }
    }
    sv.set_cvar("sv_pure", "0");
    sv.load(Some(&install)).unwrap();
    let mut pins = vec![VecDeque::new(); crate::server::MAX_CLIENTS];
    pins[0].push_back(Some((origin, [0.0, yaw, 0.0])));
    sv.game.level.as_mut().unwrap().rt.host.spawn_pins =
        Some(std::rc::Rc::new(std::cell::RefCell::new(pins)));
    // `cod4_probe`'s rule: the team a second past the first snapshot, the
    // class 1.5 s after; 2550 lands the class on the frame retail's did,
    // so the spawn falls on its twin's.
    let mut p = Probe::new(
        "pre",
        0,
        28961,
        first,
        &[
            (1000, "mr {sid} 8 allies"),
            (2550, "mr {sid} 15 assault_mp,0"),
        ],
    );
    let mut now = Instant::now();
    while sv.core.sv_time_ms < run_ms {
        let svs = sv.core.sv_time_ms;
        if !p.joined && svs >= p.connect_at {
            p.join(&mut sv, now);
        }
        if p.joined {
            let pkt = p.packet(svs);
            p.log("c2s", svs, &pkt);
            sv.handle_packet(p.addr, &pkt, now);
        }
        now += Duration::from_millis(FRAME_MS as u64);
        sv.tick_frames(now, 1);
        let svs = sv.core.sv_time_ms;
        for (to, pkt) in sv.take_outgoing() {
            if to == p.addr {
                p.receive(svs, &pkt);
            }
        }
    }
    let aborts = sv.game.level.as_ref().unwrap().rt.aborts();
    assert!(aborts.is_empty(), "script errors:\n{}", aborts.join("\n"));
    let o = side(&p.lines);
    // The waiting text's timer is in the first snapshot, as retail's.
    let ty = vcod_cod4::net::fields::HUDELEM
        .iter()
        .find(|f| f.name == "type")
        .unwrap()
        .word();
    let timer = |s: &super::wire_ab::Side| {
        s.snaps
            .first()
            .is_some_and(|f| f.ps.hud_archival.iter().any(|h| h.0[ty] == 7))
    };
    assert!(
        timer(&r) && timer(&o),
        "no timer element in the first snapshot"
    );
    if std::env::var_os("COD4_PRE_CMDS").is_some() {
        for i in 0..r.commands.len().max(o.commands.len()) {
            eprintln!(
                "{i:3} {:70} | {:?}",
                format!("{:?}", r.commands.get(i)),
                o.commands.get(i)
            );
        }
    }
    let mut out = Vec::new();
    let texts = |s: &super::wire_ab::Side| -> Vec<String> {
        s.commands.iter().map(|(_, c)| c.clone()).collect()
    };
    let (rc, oc) = (texts(&r), texts(&o));
    for i in 0..rc.len().max(oc.len()) {
        if rc.get(i) != oc.get(i) {
            out.push(format!(
                "cmd {i}: ours {:?} retail {:?}",
                oc.get(i),
                rc.get(i)
            ));
        }
    }
    hud_diffs("pre", &r, &o, &mut out);
    assert!(
        out.is_empty(),
        "{} differences:\n{}",
        out.len(),
        out.join("\n")
    );
}
