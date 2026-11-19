//! `addTestClient` on a whole server: `probe_testclient` run on mp_pavlov
//! against its retail section, the spawned test client's origin included,
//! which only a server with the map's collision and its own frame loop
//! produces (docs/research/cod11-gsc-object-model.md 25).
//!
//! Needs `COD_DIR`; without the paks it returns early.

use crate::common;

#[test]
fn probe_testclient_matches_retail_on_a_server() {
    let Some(fs) = vcod_common::testing::game_fs_11() else {
        eprintln!("COD_DIR unset or not a 1.1 install: skipping");
        return;
    };
    let (mut sv, mut now) = common::Boot {
        script: Some(include_str!(
            "../../gsc/tests/fixtures/semantics/probe_testclient.gsc"
        )),
        ..common::Boot::new("mp_pavlov", "probe_testclient")
    }
    .start(fs);
    common::run_alone(&mut sv, &mut now, 8000);
    let ours: Vec<&str> = sv
        .script_log()
        .iter()
        .filter(|l| l.starts_with("PROBE "))
        .map(String::as_str)
        .collect();
    let text = std::fs::read_to_string("../gsc/tests/fixtures/semantics/retail-captures.txt")
        .expect("read retail-captures.txt");
    let retail: Vec<&str> = text
        .split("# probe_testclient\n")
        .nth(1)
        .expect("a probe_testclient section")
        .lines()
        .take_while(|l| !l.is_empty())
        .collect();
    assert_eq!(ours, retail);
}

/// `status` over rcon with `probe_testclient`'s three test clients in, as
/// retail 1.1d answered it on mp_pavlov (2026-10-18): a bot's type-0
/// address prints IPX-style with its counter byte-swapped, ping 999, qport
/// 0, the userinfo's rate 5000 (cod11-server-handshake.md, "Console
/// commands over rcon"). `lastmsg` is the time since the connect and is
/// masked.
#[test]
fn status_lists_test_clients_as_retail() {
    let Some(fs) = vcod_common::testing::game_fs_11() else {
        eprintln!("COD_DIR unset or not a 1.1 install: skipping");
        return;
    };
    let (mut sv, mut now) = common::Boot {
        script: Some(include_str!(
            "../../gsc/tests/fixtures/semantics/probe_testclient.gsc"
        )),
        ..common::Boot::new("mp_pavlov", "probe_testclient")
    }
    .start(fs);
    sv.set_cvar("rconPassword", "pw");
    common::run_alone(&mut sv, &mut now, 8000);
    sv.take_outgoing();
    let from = std::net::SocketAddr::from(([127, 0, 0, 1], 40000));
    sv.handle_packet(from, b"\xff\xff\xff\xffrcon pw status", now);
    let reply: String = sv
        .take_outgoing()
        .into_iter()
        .filter(|(to, _)| *to == from)
        .map(|(_, p)| String::from_utf8_lossy(&p[4..]).into_owned())
        .collect();
    let rows: Vec<String> = reply
        .lines()
        .filter(|l| l.contains("bot"))
        .map(|l| format!("{}{}", &l[..31], &l[39..]))
        .collect();
    assert_eq!(
        rows,
        [
            "  0     0  999 bot1            00000000.000000000000:256    0  5000",
            "  1     0  999 bot2            00000000.000000000000:512    0  5000",
            "  2     0  999 bot3            00000000.000000000000:768    0  5000",
        ]
    );
}

/// `probe_tcrestart` on mp_pavlov against retail 1.1d (2026-11-14): a test
/// client added past `sv_reconnectlimit` survives a `map_restart(false)`,
/// absent from the new level's player list until its `ClientConnect` runs
/// again 300 ms on, and the name counter carries on across the restart
/// (docs/research/cod11-gsc-object-model.md 25).
#[test]
fn a_test_client_rejoins_after_a_map_restart_as_on_retail() {
    let Some(fs) = vcod_common::testing::game_fs_11() else {
        return;
    };
    let retail = [
        "PROBE level  t 0",
        "PROBE connect 0 bot0 state spectator t 5050",
        "PROBE added num 0 name bot0",
        "PROBE begin 0 t 5050",
        "PROBE restart",
        "PROBE level 1 t 6050",
        "PROBE players 0",
        "PROBE connect 0 bot0 state spectator t 6350",
        "PROBE begin 0 t 6400",
        "PROBE connect 1 bot1 state spectator t 11150",
        "PROBE added2 num 1 name bot1",
        "PROBE done",
        "PROBE begin 1 t 11150",
    ];
    let (mut sv, mut now) = common::Boot {
        script: Some(include_str!(
            "../../gsc/tests/fixtures/semantics/client-probes/probe_tcrestart.gsc"
        )),
        ..common::Boot::new("mp_pavlov", "probe_tcrestart")
    }
    .start(fs);
    // A restart starts a new script log, so the lines are kept per level.
    let mut lines: Vec<String> = Vec::new();
    let mut seen = 0;
    for _ in 0..400 {
        common::run_alone(&mut sv, &mut now, 50);
        let log = sv.script_log();
        if log.len() < seen {
            seen = 0;
        }
        lines.extend(
            log[seen..]
                .iter()
                .filter(|l| l.starts_with("PROBE "))
                .cloned(),
        );
        seen = log.len();
        if lines.iter().any(|l| l.starts_with("PROBE begin 1")) {
            break;
        }
    }
    assert_eq!(lines, retail);
}

/// `probe_tcmapchange` on mp_pavlov against retail 1.1d (2026-11-15): the
/// same test client across `exitLevel(false)` into mp_carentan. Its
/// `ClientConnect` runs again 300 ms into the new level, as after a
/// restart, but its `begin` never comes: the load puts every client back to
/// connected and a test client sends nothing to enter the world again
/// (docs/research/cod11-gsc-object-model.md 25).
#[test]
fn a_test_client_reconnects_but_never_begins_after_a_map_change_as_on_retail() {
    let Some(fs) = vcod_common::testing::game_fs_11() else {
        return;
    };
    let retail = [
        "PROBE level  map mp_pavlov t 0",
        "PROBE connect 0 bot0 state spectator t 5050",
        "PROBE added num 0 name bot0",
        "PROBE begin 0 t 5050",
        "PROBE exit",
        "PROBE level 1 map mp_carentan t 6050",
        "PROBE players 0",
        "PROBE connect 0 bot0 state spectator t 6350",
        "PROBE connect 1 bot1 state spectator t 11150",
        "PROBE added2 num 1 name bot1",
        "PROBE done",
        "PROBE begin 1 t 11150",
    ];
    let (mut sv, mut now) = common::Boot {
        cvars: &[(
            "sv_mapRotation",
            "gametype probe_tcmapchange map mp_carentan",
        )],
        script: Some(include_str!(
            "../../gsc/tests/fixtures/semantics/client-probes/probe_tcmapchange.gsc"
        )),
        ..common::Boot::new("mp_pavlov", "probe_tcmapchange")
    }
    .start(fs);
    // A map change starts a new script log, so the lines are kept per level.
    let mut lines: Vec<String> = Vec::new();
    let mut seen = 0;
    let mut id = sv.server_id();
    let mut begun_at = None;
    for i in 0..400 {
        common::run_alone(&mut sv, &mut now, 50);
        if sv.server_id() != id {
            id = sv.server_id();
            seen = 0;
        }
        let log = sv.script_log();
        lines.extend(
            log[seen.min(log.len())..]
                .iter()
                .filter(|l| l.starts_with("PROBE "))
                .cloned(),
        );
        seen = log.len();
        // Five seconds past the second begin for a late `begin 0`.
        if begun_at.is_none() && lines.iter().any(|l| l.starts_with("PROBE begin 1")) {
            begun_at = Some(i);
        }
        if begun_at.is_some_and(|at| i >= at + 100) {
            break;
        }
    }
    assert_eq!(lines, retail);
}
