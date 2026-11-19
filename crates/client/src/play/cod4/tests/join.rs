//! Live joins of vcod-server: map changes, mods, restarts, rcon, names,
//! stats and the first snapshots' timing.

use vcod_cod4::net::conn::Cp;

use super::*;
use crate::play::cod4_menus::Join as MenuJoin;

/// vcod-server's `map mp_backlot` under a joined, spawned client: the
/// session reads `loadingnewmap`, answers `requeststats`, takes the new
/// gamestate and its `cp`, names the new map once for the caller to
/// load, and the menus' answers spawn it there (cod4-map-cycle.md 6).
#[test]
fn cod4_live_map_change_names_the_new_map_and_spawns_there() {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use vcod_server::cod4::{Cod4Config, Cod4Game, Cod4Server};
    use vcod_server::engine::GameModule;
    let Some(install) = vcod_cod4::testing::cod4_install() else {
        return;
    };
    let _live = live_slot();
    let mut sv = Cod4Server::new(
        Cod4Config {
            map: "mp_crash".into(),
            gametype: "war".into(),
            hostname: "vcod".into(),
            max_clients: 4,
        },
        7,
    );
    sv.load(Some(&install)).unwrap();
    let sock = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
    sock.set_nonblocking(true).unwrap();
    let addr = sock.local_addr().unwrap();
    let spawned_once = Arc::new(AtomicBool::new(false));
    let flag = spawned_once.clone();
    let clock = sim::Clock::new();
    let client = sim::spawn(&clock, move || {
        let join = MenuJoin {
            team: Some("allies".into()),
            class: Some("assault_mp,0".into()),
        };
        let mut s = Cod4Session::connect(addr, "joiner", Cp::Bare, join).unwrap();
        let t0 = clk_now();
        let (mut changes, mut spawns) = (Vec::new(), Vec::new());
        while since(t0) < Duration::from_secs(60) && spawns.len() < 2 {
            s.step(since(t0).as_secs_f64() * 1000.0).unwrap();
            s.take_commands();
            changes.extend(s.take_map_change());
            let up = s.spawn().is_some();
            if up && spawns.len() == changes.len() {
                spawns.push(s.map_name());
                flag.store(true, Ordering::Relaxed);
            }
            clk_sleep(Duration::from_millis(16));
        }
        s.disconnect();
        (changes, spawns)
    });
    let mut asked = false;
    serve(&mut sv, &sock, &clock, &client, |sv, now, next| {
        if !asked && spawned_once.load(Ordering::Relaxed) {
            asked = true;
            sv.core.console.push_back("map mp_backlot".into());
            Cod4Game::drain_console(sv, now);
            *next = now;
        }
        true
    });
    let (changes, spawns) = client.join().unwrap();
    assert_eq!(changes, ["mp_backlot"]);
    assert_eq!(
        spawns,
        [Some("mp_crash".to_string()), Some("mp_backlot".to_string())]
    );
}

/// A server running a mod answers `connectResponse mods/x`: the join
/// sends the profile's stats under `mods/x` and the session writes
/// back there, the file the main menu reads once the server's
/// systeminfo has set `fs_game` (cod4-front-end.md 6).
#[test]
fn cod4_live_a_modded_server_reads_the_mods_stats() {
    use crate::play::cod4_profile::ProfileStats;
    use vcod_server::cod4::{Cod4Config, Cod4Server};
    let Some(install) = vcod_cod4::testing::cod4_install() else {
        return;
    };
    let root = tempfile::tempdir().unwrap();
    let root = root.path().to_path_buf();
    std::fs::create_dir_all(root.join("players/profiles/tester")).unwrap();
    std::fs::write(root.join("players/profiles/active.txt"), "tester").unwrap();
    for (fs_game, v) in [("", 1), ("mods/x", 2)] {
        let (mut p, mut stats) = ProfileStats::load_from(&root, fs_game).unwrap();
        stats.set(2301, v);
        p.save(&stats, true);
    }
    let mut sv = Cod4Server::new(
        Cod4Config {
            map: "mp_crash".into(),
            gametype: "war".into(),
            hostname: "vcod".into(),
            max_clients: 4,
        },
        7,
    );
    sv.load(Some(&install)).unwrap();
    sv.set_cvar("fs_game", "mods/x");
    let sock = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
    sock.set_nonblocking(true).unwrap();
    let addr = sock.local_addr().unwrap();
    let clock = sim::Clock::new();
    let menu_root = root.clone();
    let client = sim::spawn(&clock, move || {
        let profile = ProfileStats::load_from(&root, "");
        let mut s = Cod4Session::connect_profile(
            addr,
            "modder",
            "",
            Cp::Bare,
            MenuJoin::default(),
            None,
            profile,
        )
        .unwrap();
        let path = s.profile.as_ref().map(|p| p.path().to_path_buf());
        let sent = s.menus.stats().and_then(|st| st.get(2301));
        s.disconnect();
        (path, sent)
    });
    serve(&mut sv, &sock, &clock, &client, |_, _, _| true);
    let (path, sent) = client.join().unwrap();
    let (menu, _) = ProfileStats::load_from(&menu_root, "mods/x").unwrap();
    assert_eq!(path.as_deref(), Some(menu.path()));
    assert_eq!(sent, Some(2));
}

/// vcod-server's `fast_restart` under a joined, spawned client: no
/// gamestate comes, only `B` (or `n`), and the session puts a moved
/// dynamic entity back at its def's pose, as `CG_MapRestart` runs
/// `DynEntCl_InitEntities` (cod4-world.md 13.7).
#[test]
fn cod4_live_fast_restart_puts_the_dynents_back() {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use vcod_server::cod4::{Cod4Config, Cod4Game, Cod4Server};
    use vcod_server::engine::GameModule;
    let Some(install) = vcod_cod4::testing::cod4_install() else {
        return;
    };
    let world = Arc::new(crate::cod4_fly::load_world(install.root(), "mp_crash").unwrap());
    let home = world.dynents[0].origin;
    let mut sv = Cod4Server::new(
        Cod4Config {
            map: "mp_crash".into(),
            gametype: "war".into(),
            hostname: "vcod".into(),
            max_clients: 4,
        },
        7,
    );
    sv.load(Some(&install)).unwrap();
    let sock = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
    sock.set_nonblocking(true).unwrap();
    let addr = sock.local_addr().unwrap();
    let moved = Arc::new(AtomicBool::new(false));
    let flag = moved.clone();
    let clock = sim::Clock::new();
    let client = sim::spawn(&clock, move || {
        let join = MenuJoin {
            team: Some("allies".into()),
            class: Some("assault_mp,0".into()),
        };
        let mut s = Cod4Session::connect(addr, "restarter", Cp::Bare, join).unwrap();
        s.set_world(world);
        let t0 = clk_now();
        let (mut restarted, mut changes) = (false, 0);
        while since(t0) < Duration::from_secs(40) && !restarted {
            s.step(since(t0).as_secs_f64() * 1000.0).unwrap();
            s.take_commands();
            changes += usize::from(s.take_map_change().is_some());
            let up = s.spawn().is_some();
            let d = s.dynents.as_mut().unwrap();
            if !flag.load(Ordering::Relaxed) && up {
                d.ents[0].origin += Vec3::Z * 100.0;
                flag.store(true, Ordering::Relaxed);
            } else if flag.load(Ordering::Relaxed) {
                restarted = d.ents[0].origin.distance(home) < 0.01;
            }
            clk_sleep(Duration::from_millis(16));
        }
        s.disconnect();
        (restarted, changes)
    });
    let mut asked = false;
    serve(&mut sv, &sock, &clock, &client, |sv, now, next| {
        if !asked && moved.load(Ordering::Relaxed) {
            asked = true;
            sv.core.console.push_back("fast_restart".into());
            Cod4Game::drain_console(sv, now);
            *next = now;
        }
        true
    });
    let (restarted, changes) = client.join().unwrap();
    assert_eq!(changes, 0, "a fast_restart sends no gamestate");
    assert!(restarted, "the moved dynent stayed moved");
}

/// `rcon` from a client joined to vcod-server (cod4-handshake.md 14.2):
/// the console line through the shell and `CL_Rcon_f`'s rules, out of
/// band on the connection's socket, the server's `print`s back as
/// console lines. Before `rcon login` nothing goes; a wrong password
/// is refused; the right one runs `status`.
#[test]
fn cod4_live_rcon_status_from_a_joined_client() {
    use crate::console::rcon::{Cod4Rcon, Cod4RconOut};
    use crate::console::shell::{Effect, Shell};
    use vcod_server::cod4::{Cod4Config, Cod4Server};
    let Some(install) = vcod_cod4::testing::cod4_install() else {
        return;
    };
    let _live = live_slot();
    let mut sv = Cod4Server::new(
        Cod4Config {
            map: "mp_crash".into(),
            gametype: "war".into(),
            hostname: "vcod".into(),
            max_clients: 4,
        },
        7,
    );
    sv.set_cvar("rcon_password", "test");
    sv.load(Some(&install)).unwrap();
    let sock = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
    sock.set_nonblocking(true).unwrap();
    let addr = sock.local_addr().unwrap();
    let clock = sim::Clock::new();
    let client = sim::spawn(&clock, move || {
        let join = MenuJoin {
            team: Some("spectator".into()),
            class: None,
        };
        let mut s = Cod4Session::connect(addr, "admin", Cp::Bare, join).unwrap();
        let mut shell = Shell::new();
        let mut rcon = Cod4Rcon::default();
        let t0 = clk_now();
        let mut answers = Vec::new();
        for line in [
            "rcon status",
            "rcon login wrong",
            "rcon status",
            "rcon login test",
            "rcon status",
        ] {
            let effects = shell.execute(line);
            let [Effect::Rcon(args)] = effects.as_slice() else {
                panic!("{line}");
            };
            let mut got = match rcon.command(args, s.is_live()) {
                Cod4RconOut::Lines(l) => l,
                Cod4RconOut::Server(text) => {
                    s.send_oob(&text);
                    Vec::new()
                }
                Cod4RconOut::Host(..) => panic!("joined, yet sent to the host"),
            };
            // Past the server's 500 ms between two rcons.
            let end = since(t0) + Duration::from_millis(700);
            while since(t0) < end {
                s.step(since(t0).as_secs_f64() * 1000.0).unwrap();
                s.take_commands();
                s.take_fired();
                got.extend(s.take_lines());
                clk_sleep(Duration::from_millis(16));
            }
            answers.push(got);
        }
        s.disconnect();
        answers
    });
    serve(&mut sv, &sock, &clock, &client, |_, _, _| true);
    let a = client.join().unwrap();
    assert_eq!(
        a[0],
        ["You need to log in with 'rcon login <password>' before using rcon."]
    );
    assert!(a[1].is_empty(), "{:?}", a[1]);
    assert_eq!(a[2], ["Invalid password."]);
    assert!(a[3].is_empty(), "{:?}", a[3]);
    let status = a[4].join("\n");
    assert!(
        status.starts_with("map: mp_crash\nnum score ping"),
        "{status}"
    );
    assert!(status.contains(" admin^7"), "{status}");
}

/// `name` typed in a joined client's console (cod4-gsc.md 12): the
/// userinfo goes again, and under `war`'s `auto_change` the client
/// state the scoreboard and the kill feed read carries the new name
/// within a second; the same name again sends nothing.
#[test]
fn cod4_live_name_change_reaches_the_client_states() {
    use crate::console::shell::{Effect, Shell};
    use vcod_server::cod4::{Cod4Config, Cod4Server};
    let Some(install) = vcod_cod4::testing::cod4_install() else {
        return;
    };
    let _live = live_slot();
    let mut sv = Cod4Server::new(
        Cod4Config {
            map: "mp_crash".into(),
            gametype: "war".into(),
            hostname: "vcod".into(),
            max_clients: 4,
        },
        7,
    );
    sv.load(Some(&install)).unwrap();
    let sock = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
    sock.set_nonblocking(true).unwrap();
    let addr = sock.local_addr().unwrap();
    let clock = sim::Clock::new();
    let client = sim::spawn(&clock, move || {
        let join = MenuJoin {
            team: Some("allies".into()),
            class: Some("assault_mp,0".into()),
        };
        let mut s = Cod4Session::connect(addr, "before", Cp::Bare, join).unwrap();
        let mut shell = Shell::new();
        let own = |s: &Cod4Session| {
            let n = s.gamestate()?.client_num as u32;
            let snap = s.client().snapshots.last()?;
            snap.clients.iter().find(|c| c.0[0] == n).map(client_name)
        };
        let t0 = clk_now();
        let mut seen = Vec::new();
        let mut sent = 0;
        while since(t0) < Duration::from_secs(6) {
            s.step(since(t0).as_secs_f64() * 1000.0).unwrap();
            s.take_commands();
            s.take_fired();
            if since(t0) > Duration::from_secs(2) && sent == 0 {
                let effects = shell.execute("name after");
                assert!(effects.contains(&Effect::Userinfo), "{effects:?}");
                s.set_name(&shell.userinfo().name);
                sent = s.source_commands();
                // The same name again queues nothing.
                s.set_name("after");
                assert_eq!(s.source_commands(), sent);
            }
            if let Some(n) = own(&s)
                && seen.last() != Some(&n)
            {
                seen.push(n);
            }
            clk_sleep(Duration::from_millis(16));
        }
        s.disconnect();
        seen
    });
    serve(&mut sv, &sock, &clock, &client, |_, _, _| true);
    let seen = client.join().unwrap();
    assert_eq!(seen, ["before", "after"]);
}

/// The level time of the newest snapshot once `ui_score_bar "0"` has
/// come, among the commands since the last call.
fn score_bar_off(s: &mut Cod4Session) -> Option<i32> {
    let off = s
        .take_commands()
        .iter()
        .any(|c| String::from_utf8_lossy(c).contains("ui_score_bar \"0\""));
    off.then(|| s.client().snapshots.last().map(|f| f.server_time.raw()))?
}

/// What `sv_fps` shows a client: the `serverTime` steps between
/// snapshots past the first (which follows the gamestate), and the
/// level time from the spawn's snapshot (`otherFlags` 4) to `off`.
fn frame_timing(s: &Cod4Session, off: i32) -> (std::collections::BTreeSet<i32>, i32) {
    let snaps = &s.client().snapshots;
    let steps = snaps
        .windows(2)
        .skip(1)
        .map(|w| w[1].server_time - w[0].server_time)
        .collect();
    let spawn = snaps
        .iter()
        .find(|f| f.ps.get("otherFlags").unwrap_or(0) & 4 != 0)
        .expect("a spawn")
        .server_time
        .raw();
    (steps, off - spawn)
}

/// vcod-server at `sv_fps` 40 against retail's capture at 40
/// (cod4-server-frame.md 8): each frame steps 25 and a snapshot goes
/// out every frame, and `showClientScoreBar(5.0)`'s `wait` counts 100
/// frames, 2500 ms of level time.
#[test]
fn cod4_live_sv_fps_40_frames_and_waits_as_retail() {
    let text = std::fs::read_to_string(format!("{FIXTURES}/play-1.7-svfps40.txt")).unwrap();
    let mut s = Cod4Session::open_capture(&text).unwrap();
    let (mut ms, mut off) = (0.0, None);
    while !s.finished() && off.is_none() {
        ms += 1000.0 / 60.0;
        s.step(ms).unwrap();
        off = score_bar_off(&mut s);
    }
    let retail = frame_timing(&s, off.expect("retail's ui_score_bar 0"));
    assert_eq!(retail, (std::collections::BTreeSet::from([25]), 2500));
    let Some(ours) = live_join_on(
        "mp_crash",
        None,
        &[("sv_fps", "40")],
        |s, spawned, t0, _| {
            assert!(spawned.is_some());
            let mut off = None;
            while since(t0) < Duration::from_secs(20) && off.is_none() {
                s.step(since(t0).as_secs_f64() * 1000.0).unwrap();
                off = score_bar_off(s);
                clk_sleep(Duration::from_millis(16));
            }
            frame_timing(s, off.expect("ui_score_bar 0"))
        },
    ) else {
        return;
    };
    assert_eq!(ours, retail);
}

/// What the first snapshot after the gamestate carries: the commands
/// the gamestate already held, the playerstate's words but its
/// `commandTime`, and the entities by number.
fn first_snapshot(s: &Cod4Session) -> (i32, Vec<(&'static str, u32)>, Vec<u32>) {
    let c = s.client();
    let f = c.snapshots.first().expect("a snapshot");
    let ps = vcod_cod4::net::fields::PLAYER_STATE
        .iter()
        .filter(|d| d.name != "commandTime")
        .map(|d| (d.name, f.ps.words[d.word()]))
        .filter(|&(_, v)| v != 0)
        .collect();
    let gs = c.gamestate.as_ref().expect("a gamestate");
    (
        gs.server_command_sequence,
        ps,
        f.entities.iter().map(|e| e.number()).collect(),
    )
}

/// The first snapshot after the gamestate as retail's
/// (`play-1.7-svfps40.txt`, cod4-server-frame.md 8): the connect's two
/// `v` commands ride the gamestate, and the team menu's spectator reads
/// every playerstate word retail sends, `moveSpeedScaleMultiplier` 1
/// among them.
#[test]
fn cod4_live_first_snapshot_as_retail() {
    let text = std::fs::read_to_string(format!("{FIXTURES}/play-1.7-svfps40.txt")).unwrap();
    let mut s = Cod4Session::open_capture(&text).unwrap();
    let mut ms = 0.0;
    while !s.finished() && s.client().snapshots.is_empty() {
        ms += 1000.0 / 60.0;
        s.step(ms).unwrap();
    }
    let retail = first_snapshot(&s);
    assert_eq!(retail.0, 2);
    let Some(ours) = live_join_on("mp_crash", None, &[("sv_fps", "40")], |s, _, _, _| {
        let (seq, ps, ents) = first_snapshot(s);
        (seq, ps, ents)
    }) else {
        return;
    };
    assert_eq!(ours, retail);
}

/// `rcon kick "[BOT] Major Lag"`: the others' game message reads the
/// whole name and the localized reason, as 1.7's quoted
/// `e "\x15<name>^7 \x14EXE_PLAYERKICKED"` does (cod4-handshake.md 2).
#[test]
fn cod4_live_kick_message_keeps_the_whole_name() {
    let Some(r) = live_join_ctl(|s, spawned, t0, ctl| {
        spawned.as_ref()?;
        let (tx, rx) = std::sync::mpsc::channel();
        ctl.send(Ctl::AddBot(tx)).unwrap();
        let mut name = None;
        let mut seen = Vec::new();
        let end = since(t0) + Duration::from_secs(10);
        while since(t0) < end {
            s.step(since(t0).as_secs_f64() * 1000.0).unwrap();
            seen.extend(s.take_commands());
            if name.is_none()
                && let Ok(n) = rx.try_recv()
            {
                ctl.send(Ctl::Console(format!("kick \"{n}\""))).unwrap();
                name = Some(n);
            }
            if seen.iter().any(|c| c.starts_with(b"K ")) {
                break;
            }
            clk_sleep(Duration::from_millis(16));
        }
        Some((name?, seen))
    })
    .flatten() else {
        return;
    };
    let (name, seen) = r;
    assert!(name.contains(' '), "{name}");
    let Some(vcod_cod4::hud::Command::GameMessage(msg)) = seen
        .iter()
        .filter_map(|c| vcod_cod4::hud::Command::parse(c))
        .find(|c| matches!(c, vcod_cod4::hud::Command::GameMessage(_)))
    else {
        panic!("no game message in {seen:?}");
    };
    let install = vcod_cod4::testing::cod4_install().unwrap();
    let strings = install.code_strings("english").unwrap();
    let kicked = &strings["EXE_PLAYERKICKED"];
    // The HUD's own path (`Cod4Hud::message`).
    let mut loc = vcod_common::localize::Localized::default();
    for (k, v) in &strings {
        loc.insert(k, v);
    }
    let shown = loc.message(&msg);
    assert_eq!(shown, format!("{name}^7 {kicked}"), "{msg:?}");
}

/// The stock scripts' `setStat`s on a ranked (`sv_pure 1`) join reach
/// the client as `N <index> <value>`: its profile's block ends equal
/// to the server's, which the rank scripts moved off a fresh one's.
#[test]
fn cod4_live_set_stat_reaches_the_clients_profile() {
    use vcod_cod4::net::stats::Stats;
    let Some((client, server)) = live_join_ctl(|s, spawned, t0, ctl| {
        spawned.as_ref()?;
        let end = since(t0) + Duration::from_millis(1500);
        while since(t0) < end {
            s.step(since(t0).as_secs_f64() * 1000.0).unwrap();
            clk_sleep(Duration::from_millis(16));
        }
        let (tx, rx) = std::sync::mpsc::channel();
        ctl.send(Ctl::Stats(tx)).unwrap();
        let server = loop {
            s.step(since(t0).as_secs_f64() * 1000.0).unwrap();
            clk_sleep(Duration::from_millis(16));
            if let Ok(b) = rx.try_recv() {
                break b;
            }
        };
        let client: Vec<i32> = (0..3498).map(|i| s.menus.stat(i)).collect();
        Some((client, server))
    })
    .flatten() else {
        return;
    };
    let fresh = Stats::fresh();
    let at = |b: &Stats, i: i32| b.get(i as u16).unwrap_or(0) as i32;
    let moved: Vec<i32> = (4..3498)
        .filter(|&i| at(&server, i) != at(&fresh, i))
        .collect();
    assert!(
        !moved.is_empty(),
        "the scripts set no stat off a fresh profile's"
    );
    let differ: Vec<(i32, i32, i32)> = (4..3498)
        .filter(|&i| client[i as usize] != at(&server, i))
        .map(|i| (i, client[i as usize], at(&server, i)))
        .collect();
    assert!(
        differ.is_empty(),
        "client, server: {differ:?} (moved {moved:?})"
    );
}

/// A map change's `requeststats` sends the block as the `N`s left it,
/// not the join's: 300 XP the client took before `map mp_crash` reach
/// the new level's `_rank.gsc`, which ranks the player 3 off them.
#[test]
fn cod4_live_map_change_sends_the_stats_the_server_set() {
    let Some(server) = live_join_ctl(|s, spawned, t0, ctl| {
        spawned.as_ref()?;
        let step = |s: &mut Cod4Session, ms: u64| {
            let end = since(t0) + Duration::from_millis(ms);
            while since(t0) < end {
                s.step(since(t0).as_secs_f64() * 1000.0).unwrap();
                s.take_commands();
                s.take_map_change();
                clk_sleep(Duration::from_millis(16));
            }
        };
        ctl.send(Ctl::Command("N 2301 300".into())).unwrap();
        step(s, 500);
        ctl.send(Ctl::Console("map mp_crash".into())).unwrap();
        let mut last = None;
        for _ in 0..40 {
            step(s, 500);
            let (tx, rx) = std::sync::mpsc::channel();
            ctl.send(Ctl::Stats(tx)).unwrap();
            let got = loop {
                step(s, 16);
                if let Ok(b) = rx.try_recv() {
                    break b;
                }
            };
            let rank = (got.get(2301), got.get(2350));
            last = Some(got);
            if rank == (Some(300), Some(3)) {
                break;
            }
        }
        last
    })
    .flatten() else {
        return;
    };
    assert_eq!((server.get(2301), server.get(2350)), (Some(300), Some(3)));
}
