//! `fs_game mods/ModWarfare`, the install's stock example mod, against
//! retail: `handshake-1.7-mod.txt` is `tools/cod4_handshake_probe.py` on a
//! retail `cod4_lnxded` 1.7 started with `+set fs_game mods/ModWarfare`
//! (recipe in cod4-fastfile.md 13). The level boots its gametype scripts
//! out of the mod's iwd over `common_mp`'s, reads the weapon files off
//! the iwds, and configstrings 0 and 1 name the mod as retail's do, the
//! referenced iwds included. Passes vacuously without `COD4_DIR` or without
//! the mod in it.

use super::test_harness::test_cfg;
use super::*;
use vcod_cod4::net::capture;
use vcod_common::net::connectionless::info_pairs;

static FIXTURE: vcod_common::manifest::ManifestPath =
    vcod_common::manifest_const!("/../cod4/tests/fixtures/net/handshake-1.7-mod.txt");

fn pairs(s: &str) -> Vec<(String, String)> {
    info_pairs(s)
        .into_iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

#[test]
fn modwarfare_boots_its_own_scripts_and_names_the_mod_as_retail() {
    let Some(install) = vcod_cod4::testing::cod4_install() else {
        return;
    };
    if install
        .clone()
        .with_fs_game(Some("mods/ModWarfare"))
        .mod_zone()
        .is_none()
    {
        return;
    }
    let mut sv = Cod4Server::new(test_cfg("mp_crash", "war"), 7);
    sv.set_cvar("fs_game", "mods/ModWarfare");
    sv.load(Some(&install)).unwrap();
    let level = sv.game.level.as_mut().unwrap();
    let aborts = level.rt.aborts();
    assert!(
        aborts.is_empty(),
        "bootstrap errors:\n{}",
        aborts.join("\n")
    );
    // `_modwarfare::init`, which only the mod's `_globallogic` threads,
    // set its class limits.
    assert_eq!(level.rt.host.cvars.get("class_assault_limit"), "64");

    let lines = capture::parse(&std::fs::read_to_string(&FIXTURE).unwrap()).unwrap();
    let mut s = capture::Session::new();
    for l in lines.iter().filter(|l| l.dir != "auth") {
        let _ = s.feed(l);
    }
    let gs = s.client.gamestate.expect("a gamestate");
    let cs = |i: u16| String::from_utf8_lossy(gs.configstring(i)).into_owned();
    let retail0 = pairs(&cs(0));
    let ours0 = pairs(&sv.serverinfo());
    assert_eq!(
        ours0.first(),
        retail0.first(),
        "configstring 0 leads with fs_game"
    );
    let retail1 = pairs(&cs(1));
    let ours1 = pairs(&sv.systeminfo());
    let keys = |p: &[(String, String)]| p.iter().map(|(k, _)| k.clone()).collect::<Vec<_>>();
    assert_eq!(keys(&ours1), keys(&retail1), "configstring 1's keys");
    for ((k, ours), (_, theirs)) in ours1.iter().zip(&retail1) {
        if k == "sv_serverid" {
            continue;
        }
        assert_eq!(ours, theirs, "configstring 1's {k}");
    }
    // The weapon files came off the iwds, `main/iw_11`'s, as retail's.
    assert!(
        sv.game.zones4.as_ref().unwrap().weapon_files.len() > 100,
        "the weapon files"
    );
}

/// A message whose reliableAcknowledge is i32::MIN, or a `clc_move` whose
/// messageAcknowledge is, is dropped on a bare level: the server takes
/// neither ack nor a delta base from it and its next frames go out as
/// before. The first is retail's clamp, which keeps the sequence
/// (cod2-protocol.md 2.2).
#[test]
fn cod4_a_message_with_an_acknowledge_near_i32_min_is_ignored() {
    use super::test_harness::Probe;
    use super::vote_tests::run_ms;
    use std::time::Duration;
    let mut sv = Cod4Server::new(
        Cod4Config {
            map: "mp_crash".into(),
            gametype: "war".into(),
            hostname: "vcod".into(),
            max_clients: 4,
        },
        7,
    );
    sv.set_cvar("sv_pure", "0");
    sv.load(None).unwrap();
    let mut probes = vec![Probe::new("p", 0, 28961, 3600, &[])];
    let mut now = Instant::now();
    run_ms(&mut sv, &mut probes, &mut now, 6000);
    let state = |sv: &Cod4Server| {
        let c = sv.core.clients[0].as_ref().unwrap();
        (
            c.link.netchan.incoming_sequence,
            c.link.reliable_ack,
            c.delta_message,
        )
    };
    let c = sv.core.clients[0].as_ref().unwrap();
    assert!(c.state == State::Active && c.delta_message > 0);

    let p = &mut probes[0];
    let mut want = state(&sv);
    // reliableAcknowledge is the plain header's last long.
    let mut pkt = p.packet(sv.core.sv_time_ms);
    pkt[11..15].copy_from_slice(&i32::MIN.to_le_bytes());
    want.0 = u32::from_le_bytes(pkt[..4].try_into().unwrap());
    sv.handle_packet(p.addr, &pkt, now);
    assert_eq!(state(&sv), want, "reliableAcknowledge i32::MIN");
    now += Duration::from_millis(FRAME_MS as u64);
    sv.tick_frames(now, 1);
    assert!(!sv.take_outgoing().is_empty());

    // The move is keyed and scrambled under the bad messageAcknowledge, as
    // a client sending it would.
    let ack = std::mem::replace(&mut p.ack, i32::MIN);
    let pkt = p.packet(sv.core.sv_time_ms);
    p.ack = ack;
    let before = state(&sv);
    sv.handle_packet(p.addr, &pkt, now);
    assert_eq!(state(&sv), before, "messageAcknowledge i32::MIN");
    now += Duration::from_millis(FRAME_MS as u64);
    sv.tick_frames(now, 1);
    assert!(!sv.take_outgoing().is_empty());

    // The same move under good acks is taken.
    let pkt = p.packet(sv.core.sv_time_ms);
    sv.handle_packet(p.addr, &pkt, now);
    assert_eq!(state(&sv).2, ack);
}
