//! The server console on [`Cod4Server`] (cod4-handshake.md 14): `status`
//! and `getstatus`' player lines with the script's scores, `say`, `tell`
//! `kick all` and the bans, and the off-LAN admission (`sv_minPing`, the
//! idle client's second). Two in-process clients on mp_crash `war`
//! (`vote_tests::two_playing`). Needs `COD4_DIR`.

use super::vote_tests::{run_ms, two_playing};
use super::*;
use crate::rcon::Redirect;

/// The console's output for `line`, its `print\n` heads stripped.
fn console(sv: &mut Cod4Server, line: &str) -> String {
    let mut out = Redirect::default();
    sv.exec(line, &mut out);
    out.finish()
        .iter()
        .map(|t| t.strip_prefix("print\n").unwrap_or(t).to_string())
        .collect()
}

/// `status` prints 1.7's separator line and each client's script score,
/// its name closed by `^7` and padded to 16; `getstatus` carries the
/// score too.
#[test]
fn cod4_status_lines_carry_the_script_score() {
    let Some((mut sv, _, _)) = two_playing() else {
        return;
    };
    let level = sv.game.level.as_mut().unwrap();
    level
        .rt
        .set_client_field_for_test(0, "score", vcod_gsc::Value::Int(7));
    let text = console(&mut sv, "status");
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines[0], "map: mp_crash", "{text}");
    assert!(lines[1].starts_with("num score ping guid"), "{text}");
    assert_eq!(
        lines[2],
        "--- ----- ---- -------------------------------- --------------- ------- --------------------- ----- -----"
    );
    let row = lines.iter().find(|l| l.starts_with("  0 ")).expect(&text);
    assert!(row.starts_with("  0     7 "), "{row}");
    assert!(row.contains(" target^7          "), "{row}");
    assert!(lines.iter().any(|l| l.starts_with("  1     0 ")), "{text}");
    let status = sv.svc_status("x");
    assert!(
        status
            .lines()
            .any(|l| l.starts_with("7 ") && l.ends_with(" \"target\"")),
        "{status}"
    );
}

/// `status`'s `lastmsg` is `svs.time` less the client's `lastPacketTime`
/// (`SV_Status_f` 0x816c824), the server's clock, not the wall clock: a
/// client last heard 4321 ms of server time ago reads 4321 however little
/// wall time passed.
#[test]
fn cod4_status_lastmsg_reads_the_server_clock() {
    let Some((mut sv, _, _)) = two_playing() else {
        return;
    };
    let svs = sv.core.sv_time_ms;
    sv.core.clients[1].as_mut().unwrap().link.last_packet_ms = svs - 4321;
    let text = console(&mut sv, "status");
    let row = text.lines().find(|l| l.starts_with("  1 ")).expect(&text);
    // lastmsg is the first column after the name's `^7` and its padding.
    let after = &row[row.find("^7").unwrap() + 2..];
    assert_eq!(after.split_whitespace().next(), Some("4321"), "{row}");
}

/// `say` reaches both clients as a console chat line, `tell` only its
/// client; `kick` with no name prints the usage, and `kick all` drops
/// everyone, the bot and both players, each with `SV_DropClient`'s line
/// (retail 1.7 live, cod4-handshake.md 14.1).
#[test]
fn cod4_console_say_tell_and_kick_all() {
    let Some((mut sv, mut probes, mut now)) = two_playing() else {
        return;
    };
    let bot = sv.add_bots(1, false)[0];
    let bot_name = client_name(&sv.core.clients[bot].as_ref().unwrap().link.userinfo);
    console(&mut sv, "say hello there");
    console(&mut sv, "tell 1 just you");
    run_ms(&mut sv, &mut probes, &mut now, 300);
    let said = "h \"\u{15}console: hello there\"";
    let told = "h \"\u{15}console: just you\"";
    for (p, gets_tell) in probes.iter().zip([false, true]) {
        let seen = p.server_commands();
        assert!(seen.iter().any(|c| c == said), "{seen:?}");
        assert_eq!(seen.iter().any(|c| c == told), gets_tell, "{seen:?}");
    }
    assert_eq!(
        console(&mut sv, "kick"),
        "Usage: kick <player name>\nkick all = kick everyone\n"
    );
    assert_eq!(
        console(&mut sv, "kick all"),
        format!(
            "Player all is not on the server\n0:target EXE_PLAYERKICKED\n\
             1:shooter EXE_PLAYERKICKED\n{bot}:{bot_name} EXE_PLAYERKICKED\n"
        )
    );
    // The zombies left behind print nothing a second time.
    assert_eq!(
        console(&mut sv, "kick all"),
        "Player all is not on the server\n"
    );
    run_ms(&mut sv, &mut probes, &mut now, 300);
    for p in &probes {
        let seen = p.server_commands();
        assert!(
            seen.iter().any(|c| c == "w \"EXE_PLAYERKICKED\""),
            "{seen:?}"
        );
    }
}

/// `banUser` writes the GUID and the cleaned name to `ban.txt` and drops
/// the client; the second client's same GUID is refused as already
/// banned; a `getchallenge` from the GUID is refused; `unbanUser` cuts
/// the line.
#[test]
fn cod4_ban_user_refuses_the_guid_until_unbanned() {
    let Some((mut sv, mut probes, mut now)) = two_playing() else {
        return;
    };
    let guid = sv.guid(0).unwrap().to_string();
    assert!(!guid.is_empty());
    assert_eq!(
        console(&mut sv, "banUser"),
        "Usage: banUser <player name>\n"
    );
    assert_eq!(
        console(&mut sv, "banUser target"),
        "0:target EXE_PLAYERKICKED\n"
    );
    assert!(sv.core.bans.lists_guid(&guid));
    assert_eq!(
        console(&mut sv, "banClient 1"),
        format!("This GUID ({guid}) is already banned\n")
    );
    run_ms(&mut sv, &mut probes, &mut now, 300);
    let seen = probes[0].server_commands();
    assert!(
        seen.iter().any(|c| c == "w \"EXE_PLAYERKICKED\""),
        "{seen:?}"
    );
    // The refusal answers the `getchallenge` itself (0x816f3f8).
    let challenge = format!("getchallenge 0 \"{}\"", vcod_cod4::net::conn::KEY_HASH);
    let replies = probes[0].oob(&mut sv, challenge.as_bytes(), now);
    let oob: Vec<String> = replies
        .iter()
        .filter_map(|p| p.strip_prefix(&[0xff; 4][..]))
        .map(|t| String::from_utf8_lossy(t).into_owned())
        .collect();
    assert_eq!(
        oob,
        ["error\n\u{15}You are permanently banned from this server"]
    );
    assert_eq!(
        console(&mut sv, "unbanUser target"),
        "unbanned 1 user(s) named target\n"
    );
    assert!(!sv.core.bans.lists_guid(&guid));
}

/// A bot has no GUID, as 1.7's test clients (`Connecting player #1 has an
/// unknown GUID`): `status` leaves its column blank and `banUser` refuses
/// it with `Can't ban user, GUID is unknown` (retail 1.7 live,
/// cod4-handshake.md 14.2).
#[test]
fn cod4_a_bot_has_no_guid_to_ban() {
    let Some((mut sv, _, _)) = two_playing() else {
        return;
    };
    let bot = sv.add_bots(1, false)[0];
    let name = client_name(&sv.core.clients[bot].as_ref().unwrap().link.userinfo);
    let text = console(&mut sv, "status");
    let row = text
        .lines()
        .find(|l| l.starts_with(&format!("{bot:3} ")))
        .expect(&text);
    // `%3i %5i %4i ` then the GUID right-aligned in 32.
    assert_eq!(&row[15..48], &" ".repeat(33), "{row}");
    for line in [format!("banUser \"{name}\""), format!("banClient {bot}")] {
        assert_eq!(
            console(&mut sv, &line),
            "Can't ban user, GUID is unknown\n",
            "{line}"
        );
    }
    assert!(sv.core.clients[bot].is_some());
}

/// A scratch home path of this process's.
fn scratch(tag: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(format!("vcod-cod4-console-{tag}-{}", std::process::id()))
}

/// A bare server named and sized as the retail run's (`CoD4Host`, 8
/// slots), its home path a scratch directory.
fn bare(home: &std::path::Path) -> Cod4Server {
    let _ = std::fs::remove_dir_all(home);
    std::fs::create_dir_all(home).unwrap();
    let mut sv = Cod4Server::new(
        Cod4Config {
            map: "mp_crash".into(),
            gametype: "war".into(),
            hostname: "CoD4Host".into(),
            max_clients: 8,
        },
        7,
    );
    sv.set_cvar("fs_homepath", &home.display().to_string());
    sv
}

/// `writeconfig` writes retail's file under `players/`, line for line
/// (the fixture is the run's `players/vtest.cfg`), and a `seta` adds its
/// dvar, a registered one too.
#[test]
fn cod4_writeconfig_matches_retail() {
    let home = scratch("wc");
    let mut sv = bare(home.as_path());
    assert_eq!(
        console(&mut sv, "writeconfig"),
        "Usage: writeconfig <filename>\n"
    );
    assert_eq!(
        console(&mut sv, "writeconfig vtest.cfg"),
        "Writing vtest.cfg.\n"
    );
    let retail = include_str!("../../../cod4/tests/fixtures/net/writeconfig-1.7.cfg");
    let ours = std::fs::read_to_string(home.as_path().join("players/vtest.cfg")).unwrap();
    assert_eq!(ours, retail);
    console(&mut sv, "seta g_inactivity 5");
    console(&mut sv, "seta vnew 7");
    console(&mut sv, "set vplain 3");
    assert_eq!(
        console(&mut sv, "writeconfig sub/vw3"),
        "Writing sub/vw3.cfg.\n"
    );
    let text = std::fs::read_to_string(home.as_path().join("players/sub/vw3.cfg")).unwrap();
    assert!(
        text.contains("\nseta g_inactivity \"5\"\nseta g_log "),
        "{text}"
    );
    assert!(
        text.contains("\nseta vnew \"7\"\nseta voice_deadChat "),
        "{text}"
    );
    assert!(!text.contains("vplain"), "{text}");
}

/// `dvar_bool`, `dvar_int` and `dvar_float` make a dvar the query prints
/// with its domain; `setdvartotime` and `touchFile` print their usage
/// (VERIFIED live).
#[test]
fn cod4_typed_dvar_commands() {
    let home = scratch("dv");
    let mut sv = bare(home.as_path());
    assert_eq!(
        console(&mut sv, "dvar_bool vb2 0 extra"),
        "USAGE: dvar_bool <name> <default>\n"
    );
    assert_eq!(
        console(&mut sv, "dvar_int vi 5"),
        "USAGE: dvar_int <name> <default> <min> <max>\n"
    );
    assert_eq!(console(&mut sv, "dvar_bool vb 1"), "");
    assert_eq!(
        console(&mut sv, "vb"),
        "\"vb\" is: \"1^7\" default: \"1^7\"\n  Domain is 0 or 1\n"
    );
    console(&mut sv, "dvar_int vi 5 0 10");
    assert_eq!(
        console(&mut sv, "vi"),
        "\"vi\" is: \"5^7\" default: \"5^7\"\n  Domain is any integer from 0 to 10\n"
    );
    console(&mut sv, "dvar_float vf 0.5 0 1");
    assert_eq!(
        console(&mut sv, "vf"),
        "\"vf\" is: \"0.5^7\" default: \"0.5^7\"\n  Domain is any number from 0 to 1\n"
    );
    assert_eq!(console(&mut sv, "setdvartotime"), "USAGE: set <variable>\n");
    assert_eq!(console(&mut sv, "setdvartotime vtime"), "");
    assert_eq!(console(&mut sv, "touchFile"), "Usage: touchFile <file>\n");
    assert_eq!(console(&mut sv, "touchFile vtouch.txt"), "");
}

/// `exec` (`Cmd_Exec_f` 0x81121a2): a zone's rawfile before the disk,
/// `config_mp.cfg` off the disk
/// alone, a miss printed as typed (the retail run's answers).
#[test]
fn cod4_exec_reads_fastfiles_then_the_disk() {
    let home = scratch("ex");
    let mut sv = bare(home.as_path());
    let main = home.as_path().join("main");
    std::fs::create_dir_all(&main).unwrap();
    std::fs::write(main.join("vt_loose.cfg"), "set vtl loose\n").unwrap();
    std::fs::write(main.join("ragdoll.cfg"), "set vtr loose\n").unwrap();
    assert_eq!(
        console(&mut sv, "exec vt_loose"),
        "execing vt_loose.cfg from disk\n"
    );
    assert_eq!(sv.cvar("vtl"), "loose");
    // An iwd beside a loose file wins (`pk3::read_search`'s test).
    assert_eq!(
        console(&mut sv, "exec nothere"),
        "^1Error: couldn't exec nothere\n"
    );
    assert_eq!(console(&mut sv, "exec config_mp"), "");
    assert_eq!(
        console(&mut sv, "exec a b"),
        "exec <filename> : execute a script file\n"
    );
    // The zones' rawfile beats the loose `ragdoll.cfg`.
    let Some(install) = vcod_cod4::testing::cod4_install() else {
        return;
    };
    sv.set_cvar("sv_pure", "0");
    sv.load(Some(&install)).unwrap();
    assert_eq!(
        console(&mut sv, "exec ragdoll"),
        "execing ragdoll.cfg from fastfile\n"
    );
    assert_eq!(sv.cvar("vtr"), "");
    assert_eq!(
        console(&mut sv, "exec hardcore_settings"),
        "execing hardcore_settings.cfg from fastfile\n"
    );
}

/// `sv_minPing` and `sv_maxPing` hold an off-LAN connect only: the
/// in-process challenge ping is 0, under a `sv_minPing` of 100
/// (`SV_DirectConnect`, 0x8171847).
#[test]
fn cod4_min_ping_refuses_only_off_the_lan() {
    let Some((mut sv, mut probes, now)) = two_playing() else {
        return;
    };
    sv.set_cvar("sv_minPing", "100");
    let connect = |p: &mut super::wire_ab::Probe, sv: &mut Cod4Server| {
        let challenge = format!("getchallenge 0 \"{}\"", vcod_cod4::net::conn::KEY_HASH);
        let reply = p.oob(sv, challenge.as_bytes(), now);
        let text = String::from_utf8_lossy(&reply[0][4..]).into_owned();
        let c: i32 = text.split_whitespace().nth(1).unwrap().parse().unwrap();
        let line = format!(
            "connect \"\\name\\far\\protocol\\6\\challenge\\{c}\\qport\\{}\"",
            p.qport
        );
        let replies = p.oob(sv, line.as_bytes(), now);
        String::from_utf8_lossy(&replies[0][4..]).into_owned()
    };
    // 203.0.113.0/24 is no private range and none of the host's.
    probes[0].addr = std::net::SocketAddr::from(([203, 0, 113, 7], 28970));
    assert_eq!(
        connect(&mut probes[0], &mut sv),
        "error\nEXE_ERR_HIGH_PING_ONLY"
    );
    probes[0].addr = std::net::SocketAddr::from(([127, 0, 0, 1], 28970));
    assert!(connect(&mut probes[0], &mut sv).starts_with("connectResponse"));
}

/// Past its gamestate an off-LAN client still out of the world is next due
/// a second on (`SV_SendMessageToClient`, 0x8178bd1; retail's second
/// fragment came 957 ms after the first off the LAN), a LAN one the next
/// frame.
#[test]
fn cod4_an_idle_off_lan_client_waits_a_second() {
    let Some((mut sv, _, now)) = two_playing() else {
        return;
    };
    for (slot, ip, next) in [(2u32, [203, 0, 113, 9], 1000), (3, [127, 0, 0, 1], -1)] {
        let mut p = super::wire_ab::Probe::new("far", slot, 28980 + slot as u16, 0, &[]);
        p.addr = std::net::SocketAddr::from((ip, 28980 + slot as u16));
        p.join(&mut sv, now);
        let svs = sv.core.sv_time_ms;
        let pkt = p.packet(svs);
        sv.handle_packet(p.addr, &pkt, now);
        let c = sv.core.clients[slot as usize].as_ref().unwrap();
        assert_eq!(c.state, State::Primed, "{ip:?}");
        assert_eq!(c.link.next_message_ms - svs, next, "{ip:?}");
    }
}
