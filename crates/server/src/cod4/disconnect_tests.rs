//! Clients leaving a `war` level on mp_crash, in process: two probes
//! spawn (allies and axis) and leave one after the other, one probe
//! reconnects into its own live slot before it leaves, and one takes
//! auto-assign beside four shooting bots and leaves. The scripts run the
//! whole time without an error. Needs `COD4_DIR`.

use std::time::{Duration, Instant};

use super::wire_ab::{Probe, side};
use super::*;

/// Runs `probes` against a fresh `war` level until `run_ms`, each probe
/// going silent from its `(index, ms)` in `leave` on (its `disconnect`
/// sent a half second earlier through its answers) and joining again,
/// from the same address and qport, at each `rejoin`. Returns the script
/// errors.
fn run(
    map: &str,
    cvars: &[(&str, &str)],
    probes: &mut [Probe],
    rejoin: &[(usize, i32)],
    leave: &[(usize, i32)],
    run_ms: i32,
    bots: usize,
) -> (Cod4Server, Vec<String>) {
    let install = vcod_cod4::testing::cod4_install().unwrap();
    let mut sv = Cod4Server::new(
        Cod4Config {
            map: map.into(),
            gametype: "war".into(),
            hostname: "vcod".into(),
            max_clients: 8,
        },
        7,
    );
    sv.set_cvar("sv_pure", "0");
    for (k, v) in cvars {
        sv.set_cvar(k, v);
    }
    sv.load(Some(&install)).unwrap();
    if bots > 0 {
        sv.add_bots(bots, true);
    }
    let mut now = Instant::now();
    while sv.core.sv_time_ms < run_ms {
        let svs = sv.core.sv_time_ms;
        for p in probes.iter_mut() {
            if !p.joined && p.connect_at >= 0 && svs >= p.connect_at {
                p.join(&mut sv, now);
            }
        }
        for &(i, at) in rejoin {
            if (at..at + FRAME_MS).contains(&svs) {
                probes[i].join(&mut sv, now);
            }
        }
        for (i, p) in probes.iter_mut().enumerate() {
            if leave.iter().any(|&(s, at)| s == i && svs >= at) {
                p.joined = false;
                p.connect_at = -1;
                continue;
            }
            if p.joined {
                let pkt = p.packet(svs);
                p.log("c2s", svs, &pkt);
                sv.handle_packet(p.addr, &pkt, now);
            }
        }
        now += Duration::from_millis(FRAME_MS as u64);
        sv.tick_frames(now, 1);
        let svs = sv.core.sv_time_ms;
        for (to, pkt) in sv.take_outgoing() {
            if let Some(p) = probes.iter_mut().find(|p| p.addr == to) {
                p.receive(svs, &pkt);
            }
        }
    }
    let aborts = sv.game.level.as_ref().unwrap().rt.aborts();
    (sv, aborts)
}

#[test]
fn players_leave_a_war_level_without_script_errors() {
    if vcod_cod4::testing::cod4_install().is_none() {
        return;
    }
    let start = 2_000;
    let stay = Probe::new(
        "stay",
        0,
        28971,
        start + 3_600,
        &[
            (1_000, "mr {sid} 8 allies"),
            (2_500, "mr {sid} 15 assault_mp,0"),
            (40_000, "disconnect"),
        ],
    );
    let go = Probe::new(
        "go",
        1,
        28972,
        start + 3_700,
        &[
            (1_000, "mr {sid} 8 axis"),
            (2_500, "mr {sid} 15 assault_mp,0"),
            (28_000, "disconnect"),
        ],
    );
    let mut probes = vec![stay, go];
    let first = |p: &Probe| p.connect_at + 3_600;
    let leave = [
        (1, first(&probes[1]) + 28_500),
        (0, first(&probes[0]) + 40_500),
    ];
    let end = first(&probes[0]) + 52_000;
    let (sv, aborts) = run("mp_crash", &[], &mut probes, &[], &leave, end, 0);
    assert!(sv.core.clients.iter().all(Option::is_none));
    assert!(aborts.is_empty(), "script errors:\n{}", aborts.join("\n"));
    // Both spawned before they left.
    for p in &probes {
        let s = side(&p.lines);
        assert!(
            s.snaps.iter().any(|f| f.ps.get("pm_type") == Some(0)),
            "{} never spawned",
            p.addr
        );
    }
}

/// A client whose `connect` comes again from its own address and qport
/// while it is in the world: `SV_DirectConnect` frees the old client first
/// (`ClientDisconnect`, cod4-handshake.md 9), so the scripts see one
/// player leave and one join, not a second `level.players` entry.
#[test]
fn a_reconnect_into_a_live_slot_frees_the_old_player() {
    if vcod_cod4::testing::cod4_install().is_none() {
        return;
    }
    let answers = [
        (1_000, "mr {sid} 8 allies"),
        (2_500, "mr {sid} 15 assault_mp,0"),
        (9_000, "mr {sid} 8 allies"),
        (10_500, "mr {sid} 15 assault_mp,0"),
        (30_000, "disconnect"),
    ];
    let mut probes = vec![Probe::new("solo", 0, 28971, 5_600, &answers)];
    let first = probes[0].connect_at + 3_600;
    let (_sv, aborts) = run(
        "mp_crash",
        &[],
        &mut probes,
        &[(0, first + 6_000)],
        &[(0, first + 30_500)],
        first + 42_000,
        0,
    );
    assert!(aborts.is_empty(), "script errors:\n{}", aborts.join("\n"));
}

/// A client that goes silent without a `disconnect` (its window closed)
/// is timed out past `sv_timeout` and its `level.players` entry goes with
/// it: `spawnPerFrameUpdate` reads every entry's `sessionstate` each frame.
#[test]
fn a_timed_out_player_leaves_without_script_errors() {
    if vcod_cod4::testing::cod4_install().is_none() {
        return;
    }
    let start = 2_000;
    let mut probes = vec![
        Probe::new(
            "stay",
            0,
            28971,
            start + 3_600,
            &[
                (1_000, "mr {sid} 8 allies"),
                (2_500, "mr {sid} 15 assault_mp,0"),
            ],
        ),
        Probe::new(
            "gone",
            1,
            28972,
            start + 3_700,
            &[
                (1_000, "mr {sid} 8 axis"),
                (2_500, "mr {sid} 15 assault_mp,0"),
            ],
        ),
    ];
    let first = |p: &Probe| p.connect_at + 3_600;
    let leave = [(1, first(&probes[1]) + 12_000)];
    let end = first(&probes[0]) + 30_000;
    let (sv, aborts) = run(
        "mp_crash",
        &[("sv_timeout", "5"), ("sv_pure", "1")],
        &mut probes,
        &[],
        &leave,
        end,
        0,
    );
    assert!(sv.core.clients[1].is_none(), "never timed out");
    assert!(aborts.is_empty(), "script errors:\n{}", aborts.join("\n"));
}

/// Two players on mp_citystreets `war`: every spawn point the frame
/// update walks reads as one.
#[test]
fn citystreets_runs_two_players_without_script_errors() {
    if vcod_cod4::testing::cod4_install().is_none() {
        return;
    }
    let start = 2_000;
    let mut probes = vec![
        Probe::new(
            "a",
            0,
            28971,
            start + 3_600,
            &[
                (1_000, "mr {sid} 8 allies"),
                (2_500, "mr {sid} 15 assault_mp,0"),
            ],
        ),
        Probe::new(
            "b",
            1,
            28972,
            start + 3_700,
            &[
                (1_000, "mr {sid} 8 axis"),
                (2_500, "mr {sid} 15 assault_mp,0"),
            ],
        ),
    ];
    let end = probes[0].connect_at + 3_600 + 20_000;
    let (_sv, aborts) = run(
        "mp_citystreets",
        &[("sv_pure", "1")],
        &mut probes,
        &[],
        &[],
        end,
        0,
    );
    assert!(aborts.is_empty(), "script errors:\n{}", aborts.join("\n"));
}

/// One player beside four shooting bots, as `--bots 4 --bots-shoot` runs
/// it, who takes the team menu's auto-assign and then leaves after
/// `leave_ms`, with a `disconnect` first when `clean`.
/// `_teams::CountPlayers` writes `"allies"` and `"axis"` into its copy of
/// `level.players`; were the copy the same array, `level.players.size`
/// would count both keys and `spawnPerFrameUpdate` would read an
/// undefined player every frame (`probe_array_copy`, cod4-gsc.md 10).
fn autoassign_then_leave(clean: bool) -> (Cod4Server, Vec<String>) {
    let mut answers = vec![
        (1_000, "mr {sid} 8 autoassign"),
        (2_500, "mr {sid} 15 assault_mp,0"),
    ];
    if clean {
        answers.push((14_000, "disconnect"));
    }
    let mut probes = vec![Probe::new("gone", 0, 28971, 5_600, &answers)];
    let first = probes[0].connect_at + 3_600;
    run(
        "mp_crash",
        &[("sv_timeout", "5")],
        &mut probes,
        &[],
        &[(0, first + 14_500)],
        first + 30_000,
        4,
    )
}

/// The auto-assigned player's window closed: no `disconnect`, so the
/// timeout drops him.
#[test]
fn an_autoassigned_player_beside_bots_times_out_without_script_errors() {
    if vcod_cod4::testing::cod4_install().is_none() {
        return;
    }
    let (sv, aborts) = autoassign_then_leave(false);
    assert!(
        sv.core.clients.iter().flatten().all(|c| c.bot),
        "never timed out"
    );
    assert!(aborts.is_empty(), "script errors:\n{}", aborts.join("\n"));
}

/// The same player leaving through `disconnect`.
#[test]
fn an_autoassigned_player_beside_bots_disconnects_without_script_errors() {
    if vcod_cod4::testing::cod4_install().is_none() {
        return;
    }
    let (sv, aborts) = autoassign_then_leave(true);
    assert!(
        sv.core.clients.iter().flatten().all(|c| c.bot),
        "never left"
    );
    assert!(aborts.is_empty(), "script errors:\n{}", aborts.join("\n"));
}
