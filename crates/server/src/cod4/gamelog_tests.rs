//! `games_mp.log`, `g_inactivity` and the console commands 1.7 has
//! beside the kicks (cod4-server-gaps.md), on two in-process clients on
//! mp_crash `war` (`vote_tests::two_playing_with`), measured against the
//! retail run in that doc. Needs `COD4_DIR`.

use super::vote_tests::{run_ms, two_playing_with};
use super::*;
use crate::rcon::Redirect;

fn console(sv: &mut Cod4Server, line: &str) -> String {
    let mut out = Redirect::default();
    sv.exec(line, &mut out);
    out.finish()
        .iter()
        .map(|t| t.strip_prefix("print\n").unwrap_or(t).to_string())
        .collect()
}

fn temp_home(tag: &str) -> PathBuf {
    let home = std::env::temp_dir().join(format!("vcod-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&home);
    home
}

/// The text after each line's `%3i:%i%i ` stamp, which must be there.
fn unstamped(text: &str) -> Vec<String> {
    text.lines()
        .map(|l| {
            let (stamp, rest) = l.split_at(7);
            assert!(
                stamp.len() == 7 && stamp.as_bytes()[3] == b':' && stamp.ends_with(' '),
                "{l:?}"
            );
            rest.to_string()
        })
        .collect()
}

/// The log the retail run wrote, line for line past the stamps: the
/// separator and `InitGame` with configstring 0's pairs, the stock
/// scripts' `J;` lines, `G_Say`'s `say;` and `sayteam;`, `Q;` as the
/// clients go, then `ShutdownGame:` and the separator at `killserver`.
#[test]
fn cod4_games_mp_log_carries_the_engine_and_script_lines() {
    let home = temp_home("gamelog");
    let path = home.display().to_string();
    let Some((mut sv, mut probes, mut now)) =
        two_playing_with(&[("fs_homepath", &path), ("g_logSync", "1")])
    else {
        return;
    };
    let guid = sv.guid(0).unwrap().to_string();
    probes[0].command("say hello log");
    probes[1].command("say_team team log");
    run_ms(&mut sv, &mut probes, &mut now, 500);
    console(&mut sv, "killserver");
    // `SV_FinalMessage`: each client is sent its drop at once.
    let finals = sv.take_outgoing();
    for p in &probes {
        assert!(finals.iter().any(|(to, _)| *to == p.addr), "{finals:?}");
    }
    let text = std::fs::read_to_string(home.join("main/games_mp.log")).unwrap();
    let lines = unstamped(&text);
    let sep = "-".repeat(60);
    assert_eq!(lines[0], sep, "{text}");
    assert_eq!(
        lines[1],
        "InitGame: \\g_compassShowEnemies\\0\\g_gametype\\war\\gamename\\Call of Duty 4\
         \\mapname\\mp_crash\\protocol\\6\\shortversion\\1.7\\sv_allowAnonymous\\0\
         \\sv_disableClientConsole\\0\\sv_floodprotect\\4\\sv_hostname\\CoD4Host\
         \\sv_maxclients\\8\\sv_maxPing\\0\\sv_maxRate\\5000\\sv_minPing\\0\
         \\sv_privateClients\\0\\sv_punkbuster\\0\\sv_pure\\0\\sv_voice\\0\\ui_maxclients\\32",
    );
    let want = [
        format!("J;{guid};0;target"),
        format!("J;{guid};1;shooter"),
        format!("say;{guid};0;target;hello log"),
        format!("sayteam;{guid};1;shooter;team log"),
        format!("Q;{guid};0;target"),
        format!("Q;{guid};1;shooter"),
        "ShutdownGame:".to_string(),
        sep,
    ];
    let mut at = 2;
    for w in &want {
        let found = lines[at..].iter().position(|l| l == w);
        assert!(found.is_some(), "{w:?} after line {at} in\n{text}");
        at += found.unwrap() + 1;
    }
    assert_eq!(at, lines.len(), "{text}");
    // Killed, the server answers nothing.
    assert!(probes[0].oob(&mut sv, b"getinfo x", now).is_empty());
    let _ = std::fs::remove_dir_all(&home);
}

/// `g_inactivity 12`: each idle player is warned 10 s before the deadline
/// its spawn set and dropped at it; a spectator is never timed (the
/// retail run's shape, cod4-server-gaps.md "g_inactivity").
#[test]
fn cod4_idle_players_are_warned_then_dropped() {
    let Some((mut sv, mut probes, mut now)) = two_playing_with(&[("g_inactivity", "12")]) else {
        return;
    };
    let sid = sv.server_id();
    probes[1].command(&format!("mr {sid} 8 spectator"));
    let start = sv.core.sv_time_ms;
    let (mut warned, mut dropped) = (None, None);
    while sv.core.sv_time_ms < start + 20_000 && dropped.is_none() {
        run_ms(&mut sv, &mut probes, &mut now, 50);
        let seen = probes[0].server_commands();
        if warned.is_none() && seen.iter().any(|c| c == "c \"GAME_INACTIVEDROPWARNING\"") {
            warned = Some(sv.core.sv_time_ms);
        }
        if seen.iter().any(|c| c == "w \"GAME_DROPPEDFORINACTIVITY\"") {
            dropped = Some(sv.core.sv_time_ms);
        }
    }
    let (warned, dropped) = (warned.expect("no warning"), dropped.expect("no drop"));
    assert!(
        (9_800..=10_300).contains(&(dropped - warned)),
        "{warned} {dropped}"
    );
    assert!(dropped - start <= 12_500, "{start} {dropped}");
    run_ms(&mut sv, &mut probes, &mut now, 3_000);
    let seen: Vec<String> = probes[1]
        .server_commands()
        .into_iter()
        .filter(|c| c.contains("INACTIVEDROPWARNING") || c.starts_with("w "))
        .collect();
    assert!(seen.is_empty(), "the spectator was timed: {seen:?}");
    let state = sv.core.clients[1].as_ref().map(|c| c.state);
    assert_eq!(state, Some(State::Active));
    let session = sv
        .game
        .level
        .as_mut()
        .unwrap()
        .rt
        .client_session(1)
        .map(|s| s.state);
    assert_eq!(session, Some(crate::follow::SessionState::Spectator));
}

/// `dumpuser`, `kick` (which bans briefly) against `onlykick`, the dvar
/// commands and `cmdlist`, as the retail run printed them.
#[test]
fn cod4_console_dumpuser_kick_and_dvar_commands() {
    let Some((mut sv, mut probes, mut now)) = two_playing_with(&[]) else {
        return;
    };
    let dump = console(&mut sv, "dumpuser target");
    assert!(dump.starts_with("userinfo\n--------\n"), "{dump}");
    assert!(dump.contains("\nname                target\n"), "{dump}");
    assert_eq!(console(&mut sv, "dumpuser"), "Usage: info <userid>\n");
    assert_eq!(
        console(&mut sv, "dumpuser nobody"),
        "Player nobody is not on the server\n"
    );
    assert_eq!(console(&mut sv, "heartbeat"), "");
    console(&mut sv, "setu vtest 5");
    assert_eq!(
        console(&mut sv, "vtest"),
        "\"vtest\" is: \"5^7\" default: \"5^7\"\n  Domain is any text\n"
    );
    assert_eq!(
        console(&mut sv, "toggle vtest"),
        "'toggle' with no arguments makes no sense for dvar 'vtest'\n"
    );
    console(&mut sv, "toggle g_logSync");
    assert_eq!(sv.cvar("g_logSync"), "1");
    assert_eq!(
        console(&mut sv, "togglep g_logSync"),
        "g_logSync toggled to 0\n"
    );
    console(&mut sv, "toggle vtest 1 5 9");
    assert_eq!(sv.cvar("vtest"), "9");
    console(&mut sv, "set g_inactivity 40");
    console(&mut sv, "reset g_inactivity");
    assert_eq!(sv.cvar("g_inactivity"), "0");
    let list = console(&mut sv, "cmdlist");
    assert!(list.starts_with("cmdlist\nsay\nvstr\n"), "{list}");
    assert!(
        list.ends_with(&format!("{} commands\n", COMMANDS_LEN)),
        "{list}"
    );
    assert_eq!(
        console(&mut sv, "onlykick"),
        "Usage: onlykick <player name>\nonlykick all = kick everyone\n"
    );
    let guid = sv.guid(1).unwrap().to_string();
    assert_eq!(
        console(&mut sv, "kick shooter"),
        format!("1:shooter EXE_PLAYERKICKED\nshooter (guid \"{guid}\") was kicked for cheating\n")
    );
    assert!(sv.temp_banned(&guid));
    assert_eq!(
        console(&mut sv, "onlykick target"),
        "0:target EXE_PLAYERKICKED\n"
    );
    run_ms(&mut sv, &mut probes, &mut now, 300);
    assert!(
        probes[0]
            .server_commands()
            .iter()
            .any(|c| c == "w \"EXE_PLAYERKICKED\"")
    );
}

const COMMANDS_LEN: usize = super::oob::COMMANDS_LEN;
