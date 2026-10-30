//! vcod's own CoD2 client (`vcod_cod2::net::Client`, what `vcod
//! --net-probe --game cod2` and `examples/spectate.rs` run) against the
//! server in process: challenge, connect, gamestate, then a minute of delta
//! snapshots, every message through the client's parser.

use std::net::SocketAddr;
use std::rc::Rc;
use std::time::{Duration, Instant};

use vcod_cod2::net::oob::{self, Reply};
use vcod_cod2::net::{Client, UserCmd2};

use super::*;

pub(super) fn cfg(map: &str) -> Cod2Config {
    Cod2Config {
        map: map.into(),
        gametype: "dm".into(),
        hostname: "vcod cod2".into(),
        max_clients: 4,
        gametype_script: None,
    }
}

fn addr() -> SocketAddr {
    SocketAddr::from(([127, 0, 0, 1], 28961))
}

/// The one out-of-band reply `pkt` gets.
fn ask(sv: &mut Cod2Server, pkt: &[u8], now: Instant) -> Reply {
    sv.handle_packet(addr(), pkt, now);
    let out = sv.take_outgoing();
    assert_eq!(out.len(), 1, "{out:?}");
    oob::parse_reply(oob::payload(&out[0].1).expect("an oob reply"))
}

fn connect(sv: &mut Cod2Server, now: Instant) -> Client {
    let Reply::Challenge(c) = ask(sv, &oob::getchallenge(None), now) else {
        panic!("no challenge");
    };
    let qport = 4660;
    let ui = "\\name\\vcodspec\\rate\\25000\\snaps\\20\\cl_punkbuster\\0";
    assert_eq!(ask(sv, &oob::connect(ui, c, qport), now), Reply::Connect);
    Client::new(qport, c)
}

#[derive(Default, Debug)]
struct Seen {
    messages: usize,
    snapshots: usize,
    deltas: usize,
    parse_errors: usize,
    gamestates: usize,
}

/// `secs` of 50 ms frames: the client sends a packet every frame (a cmd
/// holding forward once it has a gamestate and has queued its `cp`) and
/// reads everything the server sent.
fn run(sv: &mut Cod2Server, cl: &mut Client, fs: Option<&Pk3Fs>, secs: u32) -> Seen {
    let mut seen = Seen::default();
    let mut now = Instant::now();
    for _ in 0..secs * 20 {
        let pkt = if cl.parser.gamestate.is_some() {
            let base = cl.parser.snapshot().map_or(0, |s| s.server_time);
            let mut cmd: UserCmd2 = cl.idle_cmd(base + FRAME_MS);
            cmd.forward_move = 127;
            cl.build_packet(&[cmd]).unwrap()
        } else {
            cl.build_packet(&[]).unwrap()
        };
        sv.handle_packet(addr(), &pkt, now);
        now += Duration::from_millis(FRAME_MS as u64);
        sv.tick_frames(now, 1);
        for (to, p) in sv.take_outgoing() {
            assert_eq!(to, addr());
            match cl.receive(&p) {
                Ok(Some(parsed)) => {
                    seen.messages += 1;
                    if parsed.gamestate {
                        seen.gamestates += 1;
                        let gs = cl.parser.gamestate.as_ref().unwrap();
                        let si = gs.configstring(1).to_string();
                        let cp = match fs {
                            Some(fs) => {
                                fs.clear_references();
                                fs.touch(&format!("maps/mp/{}.d3dbsp", sv.game.cfg.map));
                                vcod_cod2::net::pure::pure_command(fs, gs.checksum_feed, &si)
                            }
                            None => vcod_cod2::net::pure::cp_command(&[], gs.checksum_feed),
                        };
                        cl.add_reliable(&cp);
                    }
                    if let Some(n) = parsed.snapshot {
                        seen.snapshots += 1;
                        if cl.parser.frame(n).is_some_and(|f| f.delta_num >= 0) {
                            seen.deltas += 1;
                        }
                    }
                }
                Ok(None) => {}
                Err(e) => {
                    eprintln!("parse error: {e:#}");
                    seen.parse_errors += 1;
                }
            }
        }
    }
    seen
}

/// The gamestate the client parsed is the server's table, byte for byte
/// through the Latin-1 strings, and its systeminfo carries the serverid
/// the client keys its header on.
fn check_gamestate(sv: &Cod2Server, cl: &Client) {
    let gs = cl.parser.gamestate.as_ref().unwrap();
    for (i, s) in sv.core.configstrings.iter().enumerate() {
        assert_eq!(gs.configstring(i as u16), s, "configstring {i}");
    }
    assert_eq!(gs.server_id(), Some(sv.server_id()));
    assert_eq!(gs.checksum_feed, sv.core.checksum_feed);
    assert_eq!(gs.client_num, 0);
}

/// A server with no game data: the wire alone, as CI runs it.
#[test]
fn a_cod2_spectator_joins_a_bare_server_and_takes_delta_snapshots() {
    let now = Instant::now();
    let mut sv = Cod2Server::new(cfg("mp_toujane"), 7, now);
    sv.load(None).unwrap();
    let Reply::Info(info) = ask(&mut sv, &oob::getinfo("xyz"), now) else {
        panic!("no infoResponse");
    };
    assert_eq!(
        info,
        "\\challenge\\xyz\\protocol\\118\\hostname\\vcod cod2\\mapname\\mp_toujane\
         \\sv_maxclients\\4\\gametype\\dm\\pure\\1\\kc\\1\\hw\\1\\mod\\0\\voice\\0\\pb\\0"
    );
    let mut cl = connect(&mut sv, now);
    let seen = run(&mut sv, &mut cl, None, 10);
    assert_eq!(seen.parse_errors, 0, "{seen:?}");
    assert_eq!(seen.gamestates, 1, "{seen:?}");
    check_gamestate(&sv, &cl);
    // Every frame after the gamestate is a snapshot, all but the first few
    // deltas from the frame the client acknowledged.
    assert!(seen.snapshots > 180, "{seen:?}");
    assert!(seen.deltas > 170, "{seen:?}");
    let snap = cl.parser.snapshot().unwrap();
    assert_eq!(snap.snap_flags, 4);
    assert_eq!(snap.ps.i32("pm_type"), 4);
    assert_eq!(snap.ps.i32("clientNum"), 0);
    assert_eq!(snap.clients.len(), 1);
    assert_eq!(snap.clients[0].name(), "vcodspec");
    // No world: the cmds advance the clock and nothing else.
    assert_eq!(snap.ps.i32("commandTime"), snap.server_time);

    let Reply::Status { info, players } = ask(&mut sv, &oob::getstatus("s"), now) else {
        panic!("no statusResponse");
    };
    assert!(info.contains("\\challenge\\s\\pswrd\\0\\mod\\0"), "{info}");
    assert_eq!(players, vec![(0, 0, "vcodspec".to_string())]);

    cl.add_reliable("disconnect");
    sv.handle_packet(addr(), &cl.build_packet(&[]).unwrap(), now);
    assert_eq!(sv.client_count(), 0);
}

/// `heartbeat COD-2` to cod.pm's CoD2 port at `dedicated 2` only, every
/// three minutes, and the flatline on `quit`; a loopback fake stands in
/// for the master.
#[test]
fn a_cod2_server_heartbeats_only_at_dedicated_2() {
    let now = Instant::now();
    let mut sv = Cod2Server::new(cfg("mp_toujane"), 7, now);
    sv.set_master_resolver(|_| Some(SocketAddr::from(([127, 0, 0, 1], 0))));
    sv.load(None).unwrap();
    let master = SocketAddr::from(([127, 0, 0, 1], 20710));
    let beats = |sv: &mut Cod2Server, frames: u32| -> Vec<String> {
        sv.tick_frames(now, frames);
        sv.take_outgoing()
            .into_iter()
            .filter(|(to, _)| *to == master)
            .map(|(_, p)| String::from_utf8_lossy(&p[4..]).into_owned())
            .collect()
    };
    assert!(beats(&mut sv, 1).is_empty(), "dedicated 1 stays quiet");
    sv.set_cvar("dedicated", "2");
    assert_eq!(beats(&mut sv, 1), ["heartbeat COD-2\n"]);
    assert!(beats(&mut sv, 1).is_empty());
    // A forced beat inside a second of the last packet is dropped.
    sv.push_console("heartbeat");
    sv.drain_console(now);
    assert!(beats(&mut sv, 1).is_empty());
    let frames = (crate::master::HEARTBEAT_MS / FRAME_MS) as u32;
    assert_eq!(beats(&mut sv, frames), ["heartbeat COD-2\n"]);
    sv.push_console("quit");
    sv.drain_console(now);
    let out: Vec<String> = sv
        .take_outgoing()
        .into_iter()
        .filter(|(to, _)| *to == master)
        .map(|(_, p)| String::from_utf8_lossy(&p[4..]).into_owned())
        .collect();
    assert!(
        out.is_empty(),
        "the flatline waits out the last beat's second"
    );
    assert!(sv.quit_requested());
}

/// A wrong protocol, then a pure server refusing a client that never sent
/// `cp`.
#[test]
fn a_cod2_server_refuses_the_wrong_protocol_and_an_unpure_client() {
    let now = Instant::now();
    let mut sv = Cod2Server::new(cfg("mp_toujane"), 7, now);
    sv.load(None).unwrap();
    let Reply::Challenge(c) = ask(&mut sv, &oob::getchallenge(None), now) else {
        panic!();
    };
    let pkt = oob::packet(&format!(
        "connect \"\\name\\x\\protocol\\115\\challenge\\{c}\\qport\\1\""
    ));
    assert_eq!(
        ask(&mut sv, &pkt, now),
        Reply::Error("EXE_SERVER_IS_DIFFERENT_VER\u{15}1.3".into())
    );

    let mut cl = connect(&mut sv, now);
    sv.handle_packet(addr(), &cl.build_packet(&[]).unwrap(), now);
    for (_, p) in sv.take_outgoing() {
        cl.receive(&p).unwrap();
    }
    assert!(cl.parser.gamestate.is_some());
    let cmd = cl.idle_cmd(100);
    sv.handle_packet(addr(), &cl.build_packet(&[cmd]).unwrap(), now);
    assert_eq!(sv.client_count(), 0);
    // The zombie's next message carries the reason (cod2-protocol.md 17.3).
    sv.tick_frames(now + Duration::from_millis(50), 1);
    let mut why = None;
    for (_, p) in sv.take_outgoing() {
        if let Some(parsed) = cl.receive(&p).unwrap() {
            why = parsed.commands.last().map(|(_, t)| latin1(t));
        }
    }
    assert_eq!(why.as_deref(), Some("w \"EXE_CANNOTVALIDATEPURECLIENT\""));
}

/// mp_toujane with its script: a minute of snapshots, the `cp` the pure
/// server checks, and the spectator flying from the intermission spot.
/// Needs `COD2_DIR`.
#[test]
fn a_cod2_spectator_flies_a_minute_over_mp_toujane() {
    let Some(fs) = vcod_common::testing::cod2_fs() else {
        return;
    };
    let fs = Rc::new(fs);
    let now = Instant::now();
    let mut sv = Cod2Server::new(cfg("mp_toujane"), 7, now);
    sv.load(Some(fs.clone())).unwrap();
    let si = sv.configstring(1).to_string();
    let sums = vcod_cod2::net::pure::loaded_iwds(&si).expect("a pure server lists its iwds");
    assert_eq!(sums.len(), 16, "{si}");
    assert!(
        vcod_common::net::info_value_for_key(&si, "sv_referencedIwdNames")
            .is_some_and(|n| n.contains("main/iw_04")),
        "{si}"
    );
    let mut cl = connect(&mut sv, now);
    let start = sv.game.spot.0;
    let seen = run(&mut sv, &mut cl, Some(&fs), 60);
    assert_eq!(seen.parse_errors, 0, "{seen:?}");
    assert_eq!(seen.gamestates, 1, "{seen:?}");
    assert!(seen.snapshots >= 60 * 20 - 5, "{seen:?}");
    assert!(seen.deltas >= 60 * 20 - 10, "{seen:?}");
    check_gamestate(&sv, &cl);
    let snap = cl.parser.snapshot().unwrap();
    let o = Vec3::from_array(snap.ps.origin());
    assert!(o.distance(start) > 100.0, "{start} to {o}");
    assert!(o.is_finite());
    eprintln!("{seen:?}, from {start} to {o}");
}

/// `rcon` under `rcon_password`: 1.3's refusals (`cod2_lnxded`
/// 0x8097188) with none set, with none given and with a wrong one, then a
/// query and a write.
#[test]
fn cod2_rcon_answers_under_rcon_password() {
    let t0 = Instant::now();
    let mut sv = Cod2Server::new(cfg("mp_toujane"), 7, t0);
    sv.load(None).unwrap();
    let rcon = |sv: &mut Cod2Server, line: &str, at: Instant| {
        ask(sv, &oob::packet(&format!("rcon {line}")), at)
    };
    assert_eq!(
        rcon(&mut sv, "pw status", t0),
        Reply::Print("The server must set 'rcon_password' for clients to use 'rcon'.\n".into())
    );
    sv.set_cvar("rcon_password", "pw");
    let now = t0 + Duration::from_secs(1);
    assert_eq!(
        rcon(&mut sv, "\"\" status", now),
        Reply::Print("You must log in with 'rcon login <password>' before using 'rcon'.\n".into())
    );
    let now = now + Duration::from_secs(1);
    assert_eq!(
        rcon(&mut sv, "nope status", now),
        Reply::Print("Invalid password.\n".into())
    );
    let later = now + Duration::from_secs(1);
    let Reply::Print(text) = rcon(&mut sv, "pw serverinfo", later) else {
        panic!();
    };
    assert!(text.contains("sv_hostname         vcod cod2\n"), "{text}");
    rcon(
        &mut sv,
        "pw sv_hostname renamed",
        later + Duration::from_secs(1),
    );
    assert_eq!(
        vcod_common::net::info_value_for_key(sv.configstring(0), "sv_hostname"),
        Some("renamed")
    );
}

/// The console's replies, as retail's rcon printed them live
/// (cod2-protocol.md 3.4.2).
#[test]
fn cod2_console_replies_read_as_retail() {
    let now = Instant::now();
    let mut sv = Cod2Server::new(cfg("mp_toujane"), 7, now);
    sv.load(None).unwrap();
    let exec = |sv: &mut Cod2Server, line: &str| {
        let mut out = crate::rcon::Redirect::default();
        sv.exec(line, &mut out);
        out.finish().join("")
    };
    let info = exec(&mut sv, "serverinfo");
    assert!(info.starts_with("print\nServer info settings:\n"), "{info}");
    assert!(
        info.contains("gamename            Call of Duty 2\n"),
        "{info}"
    );
    assert!(info.contains("shortversion        1.3\n"), "{info}");
    for (line, reply) in [
        ("clientkick x", "Bad slot number: x\n"),
        ("clientkick 9", "Bad client slot: 9\n"),
        ("clientkick 2", "Client 2 is not active\n"),
        ("kick nobody", "Player nobody is not on the server\n"),
        (
            "onlykick",
            "Usage: onlykick <player name>\nonlykick all = kick everyone\n",
        ),
        ("dumpuser", "Usage: info <userid>\n"),
        (
            "map mp_nonexist",
            "Can't find map maps/mp/mp_nonexist.d3dbsp\n",
        ),
        ("banana", ""),
        ("say \"hi all\"", ""),
    ] {
        assert_eq!(exec(&mut sv, line), format!("print\n{reply}"), "{line}");
    }
    connect(&mut sv, now);
    let status = exec(&mut sv, "status");
    let row = status.lines().nth(4).unwrap_or_default();
    assert!(
        row.starts_with("  0     0 CNCT      0 vcodspec^7        "),
        "{row:?}"
    );
    assert!(
        row.ends_with(" 127.0.0.1:28961        4660 25000"),
        "{row:?}"
    );
    let user = exec(&mut sv, "dumpuser vcodspec");
    assert!(
        user.starts_with(
            "print\nuserinfo\n--------\nname                vcodspec\nrate                25000\n"
        ),
        "{user}"
    );
    assert_eq!(
        exec(&mut sv, "clientkick 0"),
        "print\n0:vcodspec EXE_PLAYERKICKED\n"
    );
    assert_eq!(sv.client_count(), 0);
}

/// One joining player: its address, its client and what it saw of the
/// other player.
struct Player {
    addr: SocketAddr,
    cl: Client,
    team: &'static str,
    asked_team: bool,
    parse_errors: usize,
    /// The other player's body: snapshots carrying it, and the origins it
    /// stood at.
    seen: usize,
    other_origins: Vec<Vec3>,
    /// Each snapshot's own `legsAnim`, `torsoAnim`, `legsTimer`,
    /// `torsoTimer`, `groundEntityNum`, and the other's body's `legsAnim`
    /// and `torsoAnim` when it carried it.
    anims: Vec<([i32; 5], Option<[i32; 2]>)>,
    /// The `b` scoreboards and the `h` chat lines that arrived.
    boards: Vec<String>,
    chat: Vec<String>,
}

/// The weapon menu's answer (`examples/spectate.rs`'s): the class's rifle.
fn weapon_reply(cl: &Client, text: &str) -> Option<String> {
    let n: u16 = text.strip_prefix("t ")?.trim().parse().ok()?;
    let menu = cl.parser.gamestate.as_ref()?.configstring(1246 + n);
    let weapon = match menu.strip_prefix("weapon_")? {
        "german" => "kar98k_mp",
        "british" => "enfield_mp",
        "american" => "m1garand_mp",
        "russian" => "mosin_nagant_mp",
        _ => return None,
    };
    Some(format!("mr {} {n} {weapon}", cl.server_id()))
}

/// Two clients join mp_toujane's dm through the team and weapon menus,
/// spawn and run 30 s; each one's snapshots carry the other's body
/// (`eType` 1) moving, every message parses and no script thread dies.
/// Needs `COD2_DIR`.
#[test]
fn two_cod2_players_join_spawn_and_see_each_other_on_mp_toujane() {
    let Some(fs) = vcod_common::testing::cod2_fs() else {
        return;
    };
    let fs = Rc::new(fs);
    let mut now = Instant::now();
    let mut sv = Cod2Server::new(cfg("mp_toujane"), 7, now);
    sv.set_cvar("developer", "1");
    sv.load(Some(fs.clone())).unwrap();
    assert!(!sv.game.baselines.is_empty(), "no baselines");
    let mut players: Vec<Player> = [("allies", 28961u16), ("axis", 28962)]
        .into_iter()
        .enumerate()
        .map(|(i, (team, port))| {
            let addr = SocketAddr::from(([127, 0, 0, 1], port));
            sv.handle_packet(addr, &oob::getchallenge(None), now);
            let out = sv.take_outgoing();
            let Reply::Challenge(c) = oob::parse_reply(oob::payload(&out[0].1).unwrap()) else {
                panic!("no challenge");
            };
            let qport = 4660 + i as u16;
            let ui = format!("\\name\\p{i}\\rate\\25000\\snaps\\20\\cl_punkbuster\\0");
            sv.handle_packet(addr, &oob::connect(&ui, c, qport), now);
            sv.take_outgoing();
            Player {
                addr,
                cl: Client::new(qport, c),
                team,
                asked_team: false,
                parse_errors: 0,
                seen: 0,
                other_origins: Vec::new(),
                anims: Vec::new(),
                boards: Vec::new(),
                chat: Vec::new(),
            }
        })
        .collect();
    for frame in 0..40 * 20 {
        // Late on, player 0 asks for the board and player 1 talks.
        if frame == 700 {
            players[0].cl.add_reliable("score");
            players[1].cl.add_reliable("say hello there");
        }
        for p in players.iter_mut() {
            let pkt = if p.cl.parser.gamestate.is_some() {
                let base = p.cl.parser.snapshot().map_or(0, |s| s.server_time);
                let mut cmd: UserCmd2 = p.cl.idle_cmd(base + FRAME_MS);
                // Ten seconds to join, then run, turning slowly.
                if frame > 200 {
                    cmd.forward_move = 127;
                    cmd.angles[1] = (cmd.angles[1] + 150) & 0xffff;
                }
                p.cl.build_packet(&[cmd]).unwrap()
            } else {
                p.cl.build_packet(&[]).unwrap()
            };
            sv.handle_packet(p.addr, &pkt, now);
        }
        now += Duration::from_millis(FRAME_MS as u64);
        sv.tick_frames(now, 1);
        for (to, pkt) in sv.take_outgoing() {
            let me = players.iter().position(|p| p.addr == to).unwrap();
            let p = &mut players[me];
            let parsed = match p.cl.receive(&pkt) {
                Ok(Some(parsed)) => parsed,
                Ok(None) => continue,
                Err(e) => {
                    eprintln!("player {me}: parse error: {e:#}");
                    p.parse_errors += 1;
                    continue;
                }
            };
            if parsed.gamestate {
                let gs = p.cl.parser.gamestate.as_ref().unwrap();
                let si = gs.configstring(1).to_string();
                fs.clear_references();
                fs.touch("maps/mp/mp_toujane.d3dbsp");
                let cp = vcod_cod2::net::pure::pure_command(&fs, gs.checksum_feed, &si);
                p.cl.add_reliable(&cp);
            }
            for (_, text) in &parsed.commands {
                let text_s = latin1(text);
                if text_s.starts_with("b ") {
                    p.boards.push(text_s.clone());
                } else if text_s.starts_with("h ") {
                    p.chat.push(text_s.clone());
                }
                if let Some(reply) = weapon_reply(&p.cl, &latin1(text)) {
                    p.cl.add_reliable(&reply);
                }
            }
            if parsed.snapshot.is_some() && !p.asked_team {
                p.asked_team = true;
                let gs = p.cl.parser.gamestate.as_ref().unwrap();
                let menu = (0..32)
                    .find(|&i| gs.configstring(1246 + i).starts_with("team_"))
                    .expect("a team menu");
                let mr = format!("mr {} {menu} {}", p.cl.server_id(), p.team);
                p.cl.add_reliable(&mr);
            }
            if let Some(snap) = p.cl.parser.snapshot() {
                let other = 1 - me as u32;
                let body = snap.entities.iter().find(|e| e.number == other);
                if let Some(e) = body {
                    assert_eq!(e.i32("eType"), 1, "player {me} sees {other} as {e:?}");
                    p.seen += 1;
                    p.other_origins.push(Vec3::from_array(e.origin()));
                }
                if parsed.snapshot.is_some() {
                    let w = |n: &str| snap.ps.i32(n);
                    p.anims.push((
                        [
                            w("legsAnim"),
                            w("torsoAnim"),
                            w("legsTimer"),
                            w("torsoTimer"),
                            w("groundEntityNum"),
                        ],
                        body.map(|e| [e.i32("legsAnim"), e.i32("torsoAnim")]),
                    ));
                }
            }
        }
    }
    let game = sv.game.level.as_ref().unwrap();
    for (i, p) in players.iter().enumerate() {
        let snap = p.cl.parser.snapshot().unwrap();
        assert_eq!(p.parse_errors, 0, "player {i}");
        assert_eq!(snap.ps.i32("pm_type"), 0, "player {i} never spawned");
        assert_eq!(snap.ps.i32("clientNum"), i as i32);
        assert!(game.playing(i));
        // The loadout and the health the script gave, and the HUD it built.
        assert_ne!(snap.ps.i32("weapon"), 0, "player {i}");
        assert_eq!(snap.ps.stats[0], 100, "player {i}");
        assert!(
            snap.ps
                .hud_current
                .iter()
                .any(|h| h.iter().any(|&w| w != 0))
        );
        let moved = p
            .other_origins
            .iter()
            .map(|o| o.distance(p.other_origins[0]))
            .fold(0.0, f32::max);
        eprintln!(
            "player {i}: {} snapshots carried the other, which moved {moved:.0} units",
            p.seen
        );
        assert!(
            p.seen > 20,
            "player {i} saw the other in {} snapshots",
            p.seen
        );
        assert!(moved > 64.0, "player {i} saw the other move {moved} units");
        check_walker_anims(i, &p.anims);
    }
    // `score`: `b <rows> <axis> <allies>` then client, score, ping,
    // deaths and status icon a row, both clients in slot order (no kills,
    // so no score splits them).
    assert_eq!(players[0].boards.len(), 1, "{:?}", players[0].boards);
    assert!(players[1].boards.is_empty());
    let b: Vec<i32> = players[0].boards[0]
        .split_whitespace()
        .skip(1)
        .map(|w| w.parse().unwrap())
        .collect();
    assert_eq!(b.len(), 3 + 2 * 5, "{:?}", players[0].boards[0]);
    assert_eq!(&b[..3], &[2, 0, 0]);
    for (row, slot) in b[3..].chunks(5).zip(0..) {
        assert_eq!(row[0], slot);
        assert_eq!((row[1], row[3], row[4]), (0, 0, 0), "{row:?}");
        assert!((0..999).contains(&row[2]), "ping {row:?}");
    }
    // `say` reaches both clients, the speaker too.
    for p in &players {
        assert_eq!(p.chat.len(), 1, "{:?}", p.chat);
        assert!(p.chat[0].contains("p1^7: ^7hello there"), "{:?}", p.chat);
    }
    let aborts = game.rt.aborts();
    assert!(aborts.is_empty(), "{}", aborts.join("\n"));
}

/// The legs anims retail's walker plays with no event timer running (its
/// continuous picks: the run, the idle, and before the rifle is up the
/// unarmed run and idle),
/// off `tests/fixtures/cod2/walker-anims.txt`, toggles masked. An anim
/// that ever runs with a timer (the deaths) is left out.
fn retail_walker_legs() -> std::collections::BTreeSet<i32> {
    let text = include_str!("../../tests/fixtures/cod2/walker-anims.txt");
    let rows: Vec<[i32; 4]> = text
        .lines()
        .filter_map(|l| l.strip_prefix("w a "))
        .map(|l| {
            let n: Vec<i32> = l
                .split_whitespace()
                .skip(2)
                .map(|t| t.parse().unwrap())
                .collect();
            [n[0], n[1], n[2], n[3]]
        })
        .collect();
    let timed: std::collections::BTreeSet<i32> = rows
        .iter()
        .filter(|r| r[2] != 0)
        .map(|r| r[0] & !ANIM_TOGGLE)
        .collect();
    rows.iter()
        .map(|r| r[0] & !ANIM_TOGGLE)
        .filter(|l| *l != 0 && !timed.contains(l))
        .collect()
}

const ANIM_TOGGLE: i32 = vcod_cod2::pmove::anim::ANIM_TOGGLEBIT;

/// A running, turning player's anims on our wire against retail's walker
/// (cod2-player-anims.md 9): on the ground with no event timer the legs play only what
/// retail's did, the run among them, the torso no anim; the toggle flips
/// only with the anim or under an event; and the other client's view of
/// the body carries the same words (`BG_PlayerStateToEntityState`).
fn check_walker_anims(player: usize, anims: &[([i32; 5], Option<[i32; 2]>)]) {
    let retail = retail_walker_legs();
    assert_eq!(
        retail,
        [127, 136, 138, 153].into(),
        "retail's continuous legs: the run, the unarmed run and idle \
         (`playerAnimType none` before the rifle is up), the idle"
    );
    let mut ran = false;
    for (k, &([legs, torso, lt, tt, ground], _)) in anims.iter().enumerate() {
        if legs == 0 {
            continue;
        }
        // In the air the takeoff holds the legs past its timer.
        if lt == 0 && tt == 0 && ground != 1023 {
            assert!(
                retail.contains(&(legs & !ANIM_TOGGLE)),
                "player {player} snapshot {k}: legs {legs} is not one of retail's {retail:?}"
            );
            assert_eq!(torso & !ANIM_TOGGLE, 0, "player {player} snapshot {k}");
        }
        ran |= legs & !ANIM_TOGGLE == 127;
        if let Some(&([prev, ..], _)) = k.checked_sub(1).map(|j| &anims[j])
            && prev != 0
            && (prev ^ legs) == ANIM_TOGGLE
        {
            assert!(
                lt > 0,
                "player {player} snapshot {k}: legs {prev} -> {legs} with no event"
            );
        }
    }
    assert!(ran, "player {player} never ran");
    // Each body as the other client saw it, against that client's own
    // snapshot words: the same set of legs anims.
    let seen: std::collections::BTreeSet<i32> = anims
        .iter()
        .filter_map(|(_, b)| b.map(|[l, _]| l))
        .collect();
    assert!(
        !seen.is_empty() && !seen.contains(&0),
        "player {player} saw {seen:?}"
    );
}

/// `crates/server/tests/fixtures/cod2-mp_toujane-mapfx.bin`: retail
/// cod2_lnxded 1.3 on mp_toujane (dm, `sv_pure 1`, `developer 1`) with
/// one client, `COD2_DIR=<install> examples/spectate.rs 127.0.0.1:28970
/// --secs 5 --save ... --save-messages 30`, both inside `unshare -rn` on
/// loopback, the spectator started 12 s after the server, whose line is
/// `+set fs_basepath <install> +set net_ip 127.0.0.1 +set net_port 28970
/// +set dedicated 1 +set sv_punkbuster 0 +set developer 1 +set
/// sv_maxclients 8 +set sv_pure 1 +map mp_toujane`. The spectator's one
/// client command is its `cp`, which keys the server's later messages.
const MAPFX_FIXTURE: &[u8] = include_bytes!("../../tests/fixtures/cod2-mp_toujane-mapfx.bin");

/// The parser after every server message of a spectator capture.
fn replay_capture(bytes: &[u8]) -> vcod_cod2::net::Parser {
    use vcod_cod2::net::capture::{Capture, Dir};
    use vcod_common::net::huffman::Huffman;
    use vcod_common::net::netchan::{Netchan, NetchanParams};
    let cap = Capture::decode(bytes).unwrap();
    let huff = Huffman::new();
    let mut challenge = None;
    let mut nc: Option<Netchan> = None;
    let mut parser = vcod_cod2::net::Parser::new();
    // The client's reliable commands, which key the server's messages.
    let mut commands = vec![Vec::new(); MAX_RELIABLE_COMMANDS];
    for (dir, p) in &cap.packets {
        if let Some(pl) = oob::payload(p) {
            match (dir, oob::parse_reply(pl)) {
                (Dir::FromServer, Reply::Challenge(c)) => challenge = Some(c),
                (Dir::ToServer, _) => {
                    let text = String::from_utf8_lossy(pl);
                    let qport = text
                        .strip_prefix("connect \"")
                        .and_then(|ui| oob::info_value_for_key(ui.trim_end_matches('"'), "qport"));
                    if let Some(q) = qport {
                        nc = Some(Netchan::with_params(
                            q.parse().unwrap(),
                            challenge.unwrap(),
                            NetchanParams::COD2,
                        ));
                    }
                }
                _ => {}
            }
            continue;
        }
        if *dir == Dir::ToServer {
            // `SV_Netchan_Decode`: the server's command at the ack keys
            // the block; the client commands lead it.
            let challenge = nc.as_ref().unwrap().challenge;
            let msg_ack = i32::from_le_bytes(p[7..11].try_into().unwrap());
            let rel_ack = i32::from_le_bytes(p[11..15].try_into().unwrap());
            let key = parser.server_command(rel_ack).to_vec();
            let mut block = p[15..].to_vec();
            vcod_common::net::netchan::scramble_client_ops(
                (challenge ^ i32::from(p[6]) ^ msg_ack) as u8,
                &key,
                &mut block,
            );
            let mut r = MsgReader::from_plain(huff.decompress_block(&block));
            while r.read_bits(CLC_BITS) == Clc::ClientCommand as i32 {
                let seq = r.read_long();
                commands[seq as usize % MAX_RELIABLE_COMMANDS] = r.read_string_bytes();
            }
            continue;
        }
        if *dir == Dir::FromServer {
            let nc = nc.as_mut().unwrap();
            let Some((seq, mut body)) = nc.frame(p).unwrap() else {
                continue;
            };
            let ack = i32::from_le_bytes(body[0..4].try_into().unwrap());
            let key = commands[ack as usize % MAX_RELIABLE_COMMANDS].clone();
            nc.decode(seq, &key, &mut body);
            let plain = huff.decompress_block(&body[4..]);
            parser.parse(seq as i32, &plain).unwrap();
        }
    }
    parser
}

/// Every `ET_FX` entity's state but its number, in entity-number order.
fn looped_fx(ents: &[vcod_cod2::net::delta::EntityState2]) -> Vec<Vec<u32>> {
    let mut fx: Vec<_> = ents
        .iter()
        .filter(|e| e.i32("eType") == vcod_cod2::net::events::ET_FX)
        .collect();
    fx.sort_by_key(|e| e.number);
    fx.iter().map(|e| e.f.to_vec()).collect()
}

/// mp_toujane's `_fx` script on ours puts out what retail's does: the four
/// `loadFx` paths at configstrings 847..850 and the nineteen `loopfx`
/// `ET_FX` entities, `scale` the effect id, field for field and in the
/// same order, in the baselines and in a snapshot. vcod numbers them
/// lower (258.. against retail's 619..), as it does every map entity.
/// Needs `COD2_DIR`.
#[test]
fn mp_toujane_ambient_fx_match_retail() {
    let Some(fs) = vcod_common::testing::cod2_fs() else {
        return;
    };
    let retail = replay_capture(MAPFX_FIXTURE);
    let rgs = retail.gamestate.as_ref().unwrap();
    let rsnap = retail.snapshot().unwrap();

    let fs = Rc::new(fs);
    let now = Instant::now();
    let mut sv = Cod2Server::new(cfg("mp_toujane"), 7, now);
    sv.load(Some(fs.clone())).unwrap();
    let mut cl = connect(&mut sv, now);
    let seen = run(&mut sv, &mut cl, Some(&fs), 3);
    assert_eq!(seen.parse_errors, 0, "{seen:?}");
    let gs = cl.parser.gamestate.as_ref().unwrap();

    let (first, last) = PROTOCOL_118
        .cs
        .bounds(vcod_common::net::protocol::CsRange::Effect);
    for i in first - 1..=last {
        assert_eq!(
            gs.configstring(i as u16),
            rgs.configstring(i as u16),
            "cs {i}"
        );
    }
    assert_eq!(rgs.configstring(850), "fx/smoke/smoke_plumeBG_toujane.efx");

    let want = looped_fx(&rsnap.entities);
    assert_eq!(want.len(), 19);
    assert_eq!(looped_fx(&cl.parser.snapshot().unwrap().entities), want);
    let bases = |gs: &Gamestate2| -> Vec<_> { gs.baselines.values().cloned().collect() };
    assert_eq!(looped_fx(&bases(gs)), looped_fx(&bases(rgs)));
    assert_eq!(looped_fx(&bases(rgs)), want);
}

/// `addTestClient` on CoD2 (cod2-protocol.md 16.5): the engine gets a
/// client in the slot, active and sending nothing, `status` lists it at
/// the bot address with ping 999 as retail's did, no server command is
/// queued for it, and the bots' pass moves it on random cmds (a spectator
/// here).
#[test]
fn a_cod2_test_client_joins_and_thinks_on_random_cmds() {
    let Some(fs) = vcod_common::testing::cod2_fs() else {
        return;
    };
    let mut cfg = cfg("mp_toujane");
    cfg.gametype_script = Some((
        "maps/mp/gametypes/dm".into(),
        "main() { maps\\mp\\gametypes\\_callbacksetup::SetupCallbacks(); \
         level.callbackStartGameType = ::start; \
         level.callbackPlayerConnect = ::connect; \
         level.callbackPlayerDisconnect = ::nop; }\n\
         start() { level thread add(); }\n\
         add() { wait 4; level.bot = addTestClient(); \
         level.botnum = level.bot getEntityNumber(); }\n\
         connect() { iprintln(\"c15\"); self.sessionstate = \"spectator\"; \
         self spawn((0, 0, 0), (0, 0, 0)); }\n\
         nop() {}\n"
            .into(),
    ));
    let now = Instant::now();
    let mut sv = Cod2Server::new(cfg, 7, now);
    sv.load(Some(Rc::new(fs))).unwrap();
    sv.tick_frames(now, 100);
    let slot = {
        let game = sv.game.level.as_mut().unwrap();
        game.rt.level_field_str("botnum")
    };
    assert_eq!(slot, "0");
    let c = sv.core.clients[0].as_ref().expect("an engine client");
    assert!(c.test && c.bot && c.state == State::Active);
    // Nothing is queued for it, or its unacked ring would overflow.
    assert_eq!(c.link.netchan.reliable_sequence, 0);
    let origin = |sv: &Cod2Server| {
        sv.game.level.as_ref().unwrap().players[0]
            .as_ref()
            .unwrap()
            .ps
            .origin
    };
    let before = origin(&sv);
    // One cmd a `SV_Frame`, however many frames it owes.
    for _ in 0..60 {
        sv.tick_frames(now, 1);
    }
    assert_ne!(origin(&sv), before, "the random cmds fly the spectator");
    let mut out = crate::rcon::Redirect::default();
    sv.exec("status", &mut out);
    let status = out.finish().join("");
    let row = status.lines().find(|l| l.contains("bot0")).expect(&status);
    assert!(
        row.contains(" 999 ") && row.contains("00000000.000000000000:0"),
        "{row}"
    );
    // `botport` is the engine's: the next level's test client is `bot1`.
    sv.map_command("mp_toujane", false).unwrap();
    sv.tick_frames(now, 100);
    let mut out = crate::rcon::Redirect::default();
    sv.exec("status", &mut out);
    let status = out.finish().join("");
    assert!(status.contains("bot1^7"), "{status}");
}

/// A `SV_Frame` owing two frames runs in retail's order (lnxded 0x80969b0,
/// cod2-protocol.md 14.1): configstrings 0, 1 and the dvar mirror, the bots
/// once, the pings, then each frame's `G_RunFrame` ending in `CheckVote`,
/// then the timeouts, the messages and the heartbeat.
#[test]
fn a_cod2_frame_runs_in_retails_order() {
    let Some(fs) = vcod_common::testing::cod2_fs() else {
        return;
    };
    let now = Instant::now();
    let mut sv = Cod2Server::new(cfg("mp_toujane"), 7, now);
    sv.load(Some(Rc::new(fs))).unwrap();
    let path = std::env::temp_dir().join(format!("vcod-cod2-order-{}.trace", std::process::id()));
    crate::tick_trace::install(&path).unwrap();
    sv.tick_frames(now, 2);
    crate::tick_trace::finish().unwrap();
    let trace = std::fs::read_to_string(&path).unwrap();
    let _ = std::fs::remove_file(&path);
    let steps: Vec<&str> = trace
        .lines()
        .filter_map(|l| {
            [
                "SV_SetConfigstring",
                "SV_BotFrame",
                "SV_CalcPings",
                "G_RunFrame time",
                "CheckVote",
                "SV_CheckTimeouts",
                "SV_SendClientMessages",
                "SV_MasterHeartbeat",
            ]
            .into_iter()
            .find(|s| l.starts_with(s))
        })
        .collect();
    assert_eq!(
        steps,
        [
            "SV_SetConfigstring",
            "SV_BotFrame",
            "SV_CalcPings",
            "G_RunFrame time",
            "CheckVote",
            "G_RunFrame time",
            "CheckVote",
            "SV_CheckTimeouts",
            "SV_SendClientMessages",
            "SV_MasterHeartbeat",
        ],
        "{trace}"
    );
}

/// `games_mp.log` over a `map_restart`, a map load and `quit`, as the
/// retail 1.3 run wrote it (cod11-map-cycle.md 9): each level opens with
/// the separator and `InitGame:` and closes with `ShutdownGame:` and the
/// separator, a restart too (CoD2 has no `RestartGame:`).
#[test]
fn a_cod2_server_writes_games_mp_log() {
    let Some(fs) = vcod_common::testing::cod2_fs() else {
        return;
    };
    let home = std::env::temp_dir().join(format!("vcod-cod2-log-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&home);
    let now = Instant::now();
    let mut sv = Cod2Server::new(cfg("mp_toujane"), 7, now);
    sv.set_cvar("fs_homepath", &home.display().to_string());
    sv.load(Some(Rc::new(fs))).unwrap();
    for line in ["map_restart", "map mp_carentan", "quit"] {
        sv.tick_frames(now, 1);
        sv.push_console(line);
        sv.drain_console(now);
    }
    let text = std::fs::read_to_string(home.join("main/games_mp.log")).unwrap();
    let sep = "-".repeat(60);
    let lines: Vec<&str> = text
        .lines()
        .map(|l| {
            let rest = &l[7..];
            rest.split_once(" \\").map_or(rest, |(w, _)| w)
        })
        .collect();
    let mut want = Vec::new();
    for _ in 0..3 {
        want.extend([sep.as_str(), "InitGame:", "ShutdownGame:", &sep]);
    }
    assert_eq!(lines, want, "{text}");
    let _ = std::fs::remove_dir_all(&home);
}

/// A tag `attach` names takes its configstring in the frame
/// (`ClientEndFrame`'s 0x80f72f8, cod2-player-anims.md 1), so the `d`
/// reaches the client ahead of the first snapshot whose client state names
/// it: through the client's own view (`NetView`, `Character`) every
/// snapshot that carries the flag hangs it off `j_spine4`. Before, the
/// snapshot build indexed the tag after the frame's writes went out, and
/// one snapshot hung `flag_carry`'s nameless root bone off nothing.
/// Needs `COD2_DIR`.
#[test]
fn a_cod2_attachments_tag_reaches_the_client_before_its_snapshot() {
    let Some(fs) = vcod_common::testing::cod2_fs() else {
        return;
    };
    let mut cfg = cfg("mp_toujane");
    cfg.gametype_script = Some((
        "maps/mp/gametypes/dm".into(),
        "main() { maps\\mp\\gametypes\\_callbacksetup::SetupCallbacks(); \
         level.callbackStartGameType = ::start; \
         level.callbackPlayerConnect = ::connect; \
         level.callbackPlayerDisconnect = ::nop; \
         level.callbackPlayerDamage = ::nop; level.callbackPlayerKilled = ::nop; }\n\
         start() { precacheModel(\"xmodel/playerbody_german_africa01\"); \
         precacheModel(\"xmodel/prop_flag_american_carry\"); }\n\
         connect() { self.sessionstate = \"playing\"; \
         self spawn((0, 0, 0), (0, 0, 0)); \
         self setModel(\"xmodel/playerbody_german_africa01\"); \
         wait 2; self attach(\"xmodel/prop_flag_american_carry\", \"J_Spine4\", true); }\n\
         nop() {}\n"
            .into(),
    ));
    let mut now = Instant::now();
    let mut sv = Cod2Server::new(cfg, 7, now);
    sv.set_cvar("developer", "1");
    let fs = Rc::new(fs);
    sv.load(Some(fs.clone())).unwrap();
    let mut cl = connect(&mut sv, now);
    let mut view = vcod_cod2::net::NetView::default();
    let mut carried = 0;
    for _ in 0..6 * 20 {
        let pkt = if cl.parser.gamestate.is_some() {
            let base = cl.parser.snapshot().map_or(0, |s| s.server_time);
            cl.build_packet(&[cl.idle_cmd(base + FRAME_MS)]).unwrap()
        } else {
            cl.build_packet(&[]).unwrap()
        };
        sv.handle_packet(addr(), &pkt, now);
        now += Duration::from_millis(FRAME_MS as u64);
        sv.tick_frames(now, 1);
        for (_, p) in sv.take_outgoing() {
            let Some(parsed) = cl.receive(&p).unwrap() else {
                continue;
            };
            if parsed.gamestate {
                let gs = cl.parser.gamestate.as_ref().unwrap();
                let si = gs.configstring(1).to_string();
                fs.clear_references();
                fs.touch("maps/mp/mp_toujane.d3dbsp");
                let cp = vcod_cod2::net::pure::pure_command(&fs, gs.checksum_feed, &si);
                cl.add_reliable(&cp);
            }
            view.update(&cl.parser, &parsed);
            if parsed.snapshot.is_none() {
                continue;
            }
            let snap = view.snap.as_ref().unwrap();
            let Some(c) = snap.clients.iter().find(|c| c.number == 0) else {
                continue;
            };
            let cs = &view.configstrings;
            let Some(ch) =
                vcod_cod2::player::Character::from_client_state(c, |i| Some(cs.get(i.into())))
            else {
                continue;
            };
            if let Some((_, tag)) = ch.attachments.iter().find(|(m, _)| m.ends_with("_carry")) {
                assert_eq!(
                    tag.as_deref(),
                    Some("j_spine4"),
                    "snapshot {}",
                    snap.message_num
                );
                carried += 1;
            }
        }
    }
    assert!(carried > 20, "{carried} snapshots carried the flag");
    let aborts = sv.game.level.as_ref().unwrap().rt.aborts();
    assert!(aborts.is_empty(), "{}", aborts.join("\n"));
}
