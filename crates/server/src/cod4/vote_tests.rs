//! `callvote` and `vote` on [`Cod4Server`] (cod4-map-cycle.md 4): two
//! in-process clients on mp_crash `war`, one calls, the other votes, and
//! the passed `map_restart` restarts the level 3 s on; and the menu
//! answers' one-packet rule (5). Needs `COD4_DIR`.

use std::time::{Duration, Instant};

use super::wire_ab::Probe;
use super::*;
use crate::follow::SessionState;

/// The two on mp_crash with their teams and classes answered, run until
/// both play.
pub(super) fn two_playing() -> Option<(Cod4Server, Vec<Probe>, Instant)> {
    two_playing_with(&[])
}

/// [`two_playing`] with `sets` (`+set` lines) ahead of the load.
pub(super) fn two_playing_with(sets: &[(&str, &str)]) -> Option<(Cod4Server, Vec<Probe>, Instant)> {
    two_playing_in(8, sets)
}

/// [`two_playing_with`] on a server of `slots` clients.
fn two_playing_in(
    slots: usize,
    sets: &[(&str, &str)],
) -> Option<(Cod4Server, Vec<Probe>, Instant)> {
    let install = vcod_cod4::testing::cod4_install()?;
    let mut sv = Cod4Server::new(
        Cod4Config {
            map: "mp_crash".into(),
            gametype: "war".into(),
            hostname: "CoD4Host".into(),
            max_clients: slots,
        },
        7,
    );
    sv.set_cvar("sv_pure", "0");
    for (n, v) in sets {
        sv.set_cvar(n, v);
    }
    sv.load(Some(&install)).unwrap();
    let mut probes = vec![
        Probe::new(
            "target",
            0,
            28961,
            43350,
            &[
                (1050, "mr {sid} 8 allies"),
                (2550, "mr {sid} 15 assault_mp,0"),
            ],
        ),
        Probe::new(
            "shooter",
            1,
            28962,
            44300,
            &[
                (1050, "mr {sid} 8 axis"),
                (2550, "mr {sid} 15 assault_mp,0"),
            ],
        ),
    ];
    let mut now = Instant::now();
    let playing = |sv: &mut Cod4Server, s: usize| {
        let level = sv.game.level.as_mut().unwrap();
        level.rt.client_session(s).map(|x| x.state) == Some(SessionState::Playing)
    };
    while !(playing(&mut sv, 0) && playing(&mut sv, 1)) {
        assert!(sv.core.sv_time_ms < 120_000, "the two never played");
        step(&mut sv, &mut probes, &mut now);
    }
    Some((sv, probes, now))
}

/// One frame: the console, each probe's packet, the tick, the replies.
pub(super) fn step(sv: &mut Cod4Server, probes: &mut [Probe], now: &mut Instant) {
    let svs = sv.core.sv_time_ms;
    for p in probes.iter_mut() {
        if !p.joined && svs >= p.connect_at {
            p.join(sv, *now);
        }
    }
    for p in probes.iter_mut().filter(|p| p.joined) {
        let pkt = p.packet(svs);
        sv.handle_packet(p.addr, &pkt, *now);
    }
    *now += Duration::from_millis(FRAME_MS as u64);
    sv.drain_console();
    sv.tick_frames(*now, 1);
    let svs = sv.core.sv_time_ms;
    for (to, pkt) in sv.take_outgoing() {
        if let Some(p) = probes.iter_mut().find(|p| p.joined && p.addr == to) {
            p.receive(svs, &pkt);
        }
    }
    for p in probes.iter_mut() {
        p.answer_stats(sv, *now);
        p.lines.clear();
    }
}

pub(super) fn run_ms(sv: &mut Cod4Server, probes: &mut [Probe], now: &mut Instant, ms: i32) {
    let until = sv.core.sv_time_ms + ms;
    while sv.core.sv_time_ms < until {
        step(sv, probes, now);
    }
}

/// A `map_restart` vote: the call's configstrings (13 the end and the
/// server id, 14 the text, 15 and 16 the counts), the second client's yes
/// passing it at once, the counts and `GAME_VOTEPASSED`, then the
/// `fast_restart` 3 s on (the low nibble steps); a bad kind is refused
/// with the usage line.
#[test]
fn cod4_a_restart_vote_passes_and_restarts() {
    let Some((mut sv, mut probes, mut now)) = two_playing() else {
        return;
    };
    let id = sv.server_id();
    probes[0].command("callvote bogus");
    run_ms(&mut sv, &mut probes, &mut now, 500);
    let seen = probes[0].server_commands();
    assert!(
        seen.iter().any(|c| c == "e \"GAME_INVALIDVOTESTRING\"")
            && seen
                .iter()
                .any(|c| c.starts_with("e \"GAME_VOTECOMMANDSARE\u{15} map_restart")),
        "{seen:?}"
    );
    probes[0].command("callvote map_restart");
    run_ms(&mut sv, &mut probes, &mut now, 500);
    let level_time = sv.game.level.as_ref().unwrap().level_time();
    let cs13 = probes[1].configstring(13);
    let (end, sid) = cs13.split_once(' ').expect("13 is `<end> <id>`");
    let end: i32 = end.parse().unwrap();
    assert!((level_time..level_time + 30_000).contains(&end), "{cs13}");
    assert_eq!(sid, id.to_string());
    assert_eq!(probes[1].configstring(14), "GAME_VOTE_MAPRESTART");
    assert_eq!(
        (probes[1].configstring(15), probes[1].configstring(16)),
        ("1".into(), "0".into())
    );
    assert!(
        probes[1]
            .server_commands()
            .iter()
            .any(|c| c == "e \"GAME_CALLEDAVOTE\u{15}target\""),
        "{:?}",
        probes[1].server_commands()
    );
    let level = sv.game.level.as_ref().unwrap();
    assert_ne!(
        level.playerstate(0).unwrap().get("eFlags").unwrap() & 0x10_0000,
        0
    );
    assert_eq!(
        level.playerstate(1).unwrap().get("eFlags").unwrap() & 0x10_0000,
        0
    );
    probes[0].command("vote yes");
    probes[1].command("vote yes");
    run_ms(&mut sv, &mut probes, &mut now, 500);
    assert!(
        probes[0]
            .server_commands()
            .iter()
            .any(|c| c == "e \"GAME_VOTEALREADYCAST\""),
        "{:?}",
        probes[0].server_commands()
    );
    assert!(
        probes[1]
            .server_commands()
            .iter()
            .any(|c| c == "e \"GAME_VOTEPASSED\""),
        "{:?}",
        probes[1].server_commands()
    );
    assert_eq!(probes[1].configstring(15), "2");
    assert_eq!(probes[1].configstring(13), "");
    assert_eq!(sv.server_id(), id, "the command waits 3 s");
    run_ms(&mut sv, &mut probes, &mut now, 3500);
    assert_eq!(sv.server_id(), super::cycle::restart_server_id(id));
}

/// A `map` vote that passes runs `map <name>`: a spawn, the high nibble
/// stepped, and both back in on the new gamestate.
#[test]
fn cod4_a_map_vote_changes_the_map() {
    let Some((mut sv, mut probes, mut now)) = two_playing() else {
        return;
    };
    let id = sv.server_id();
    probes[1].command("callvote map mp_crash");
    run_ms(&mut sv, &mut probes, &mut now, 500);
    assert_eq!(probes[0].configstring(14), "GAME_VOTE_MAP\u{15}mp_crash");
    probes[0].command("vote yes");
    run_ms(&mut sv, &mut probes, &mut now, 4000);
    assert_ne!(sv.server_id() & 0xf0, id & 0xf0);
    run_ms(&mut sv, &mut probes, &mut now, 8000);
    for p in &probes {
        assert_eq!(p.server_id(), Some(sv.server_id()));
    }
    let active = sv
        .core
        .clients
        .iter()
        .flatten()
        .filter(|c| c.state == State::Active)
        .count();
    assert_eq!(active, 2);
}

/// A passed `map` vote on a full server while both clients take 15 s to
/// load: a loading client sends nothing, and the 50 bots reconnecting
/// send it their prints. A client not `CS_ACTIVE` is queued only its
/// reliable commands (`SV_AddServerCommand` 0x81769b2), so neither is
/// dropped and both are back in on the new gamestate. (CoD4's ring held
/// 52 here with every command kept, so it never overflowed; CoD2's did,
/// `a_voted_map_change_keeps_a_slow_loading_client`.)
#[test]
fn cod4_a_map_vote_keeps_a_slow_loading_client() {
    let Some((mut sv, mut probes, mut now)) = two_playing_in(52, &[]) else {
        return;
    };
    assert_eq!(sv.add_bots(50, false).len(), 50);
    run_ms(&mut sv, &mut probes, &mut now, 4000);
    let id = sv.server_id();
    probes[1].command("callvote map mp_crash");
    run_ms(&mut sv, &mut probes, &mut now, 500);
    probes[0].command("vote yes");
    while sv.server_id() == id {
        assert!(sv.core.sv_time_ms < 200_000, "the vote never passed");
        step(&mut sv, &mut probes, &mut now);
    }
    // The load: no packet either way.
    for _ in 0..15_000 / FRAME_MS {
        now += Duration::from_millis(FRAME_MS as u64);
        sv.drain_console();
        sv.tick_frames(now, 1);
        sv.take_outgoing();
    }
    // The bots' prints went nowhere: what waits is reliable.
    for s in 0..2 {
        let c = sv.core.clients[s].as_ref().unwrap();
        assert_eq!(c.state, State::Connected);
        let mut waiting = c.link.reliable_ack + 1..=c.link.netchan.reliable_sequence as i32;
        assert!(!waiting.is_empty());
        assert!(waiting.all(|q| c.reliable_kept[q as usize & (MAX_RELIABLE_COMMANDS - 1)]));
    }
    run_ms(&mut sv, &mut probes, &mut now, 8000);
    for p in &probes {
        assert_eq!(p.server_id(), Some(sv.server_id()));
    }
    let active = (0..2)
        .filter(|&s| {
            sv.core.clients[s]
                .as_ref()
                .is_some_and(|c| c.state == State::Active)
        })
        .count();
    assert_eq!(active, 2);
}

/// `SV_SpawnServer`'s `loadingnewmap` leaves through the socket before the
/// load, one to each client in the world, then the loop sleeps 250 ms
/// (`cod4_lnxded` 0x81751f0, 0x817525a; cod4-map-cycle.md 1).
#[test]
fn cod4_a_map_change_sends_loadingnewmap_then_sleeps() {
    use std::cell::RefCell;
    use std::rc::Rc;
    let Some((mut sv, _probes, _now)) = two_playing() else {
        return;
    };
    let events = Rc::new(RefCell::new(Vec::<String>::new()));
    let (sent, slept) = (events.clone(), events.clone());
    sv.set_net_now(crate::cod2::NetNow {
        send: Box::new(move |_, pkt| {
            let text = String::from_utf8_lossy(&pkt[4..]).replace('\n', " ");
            sent.borrow_mut().push(text);
        }),
        sleep: Box::new(move |d| slept.borrow_mut().push(format!("sleep {}", d.as_millis()))),
    });
    sv.exec("map mp_crash", &mut crate::rcon::Redirect::default());
    assert_eq!(
        *events.borrow(),
        [
            "loadingnewmap mp_crash war",
            "loadingnewmap mp_crash war",
            "sleep 250"
        ]
    );
    let left = sv
        .core
        .outbox
        .iter()
        .filter(|(_, p)| p.starts_with(b"\xff\xff\xff\xffloadingnewmap"));
    assert_eq!(left.count(), 0);
}

/// The team menu's answer and the class menu's in one packet: the class
/// is lost, on a first join and back from the spectators, so the player
/// never spawns. Retail does the same (`play-1.7-menus-together.txt`):
/// the team answer's notify wakes `onMenuResponse` for the next thread
/// pass, and the class answer's, read in the same message, finds no
/// thread waiting (cod4-map-cycle.md 5).
#[test]
fn cod4_a_class_answered_with_the_team_is_lost() {
    use vcod_cod4::net::capture::{Item, Session};
    use vcod_cod4::net::clmsg::ClOp;
    // Retail: commands 5 and 6 in one packet, then no spawn.
    let lines = super::wire_ab::retail("menus-together");
    let mut session = Session::new();
    let (mut joint_at, mut spawned_after) = (None, false);
    for l in &lines {
        match session.feed(l) {
            Ok(Some(Item::Client { msg, .. })) => {
                let seqs: Vec<i32> = msg
                    .ops
                    .iter()
                    .filter_map(|o| match o {
                        ClOp::Command { sequence, .. } => Some(*sequence),
                        _ => None,
                    })
                    .collect();
                if joint_at.is_none() && seqs.contains(&5) {
                    assert!(seqs.contains(&6), "5 went without 6: {seqs:?}");
                    joint_at = Some(l.ms);
                }
            }
            Ok(Some(Item::Server { .. })) if joint_at.is_some() => {
                let pm_type = session
                    .client
                    .snapshots
                    .last()
                    .and_then(|s| s.ps.get("pm_type"));
                spawned_after |= pm_type == Some(0);
            }
            _ => {}
        }
    }
    assert!(joint_at.is_some() && !spawned_after);
    // Ours.
    let Some((mut sv, mut probes, mut now)) = two_playing() else {
        return;
    };
    let state = |sv: &mut Cod4Server| {
        let l = sv.game.level.as_mut().unwrap();
        (
            l.rt.client_field(0, "sessionteam"),
            l.rt.client_field(0, "sessionstate"),
        )
    };
    probes[0].command("mr {sid} 8 spectator");
    run_ms(&mut sv, &mut probes, &mut now, 3000);
    probes[0].command("mr {sid} 8 allies");
    probes[0].command("mr {sid} 15 specops_mp,0");
    run_ms(&mut sv, &mut probes, &mut now, 6000);
    assert_eq!(
        state(&mut sv),
        (Some("allies".into()), Some("spectator".into()))
    );
    // The class on its own takes.
    probes[0].command("mr {sid} 15 specops_mp,0");
    run_ms(&mut sv, &mut probes, &mut now, 6000);
    assert_eq!(state(&mut sv).1.as_deref(), Some("playing"));
}
