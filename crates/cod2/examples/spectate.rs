//! Headless CoD2 1.3 spectator: challenge, plain-text connect, gamestate,
//! then `clc_move` cmds against each acknowledged snapshot, printing a
//! summary a second (docs/research/cod2-protocol.md 9).
//!
//!     cargo run -p vcod-cod2 --example spectate -- 127.0.0.1:28970 [--secs N]
//!         [--save capture.bin] [--save-messages K] [--fly] [--join [--no-vote]]
//!
//! `--fly` holds forward and turns, so the playerstate deltas move;
//! `--join` autoassigns a team and takes the rifle, so another client sees
//! a player entity move; it answers the stock menus each time a level opens
//! them (`menu_reply`), so it joins again after a map change or a restart. `--anims` prints a line per snapshot: the own
//! playerstate's anim words, then every player entity's
//! (`a <serverTime> ps <legsAnim> <torsoAnim> <legsTimer> <torsoTimer> |
//! e <num> t<eType> tr<pos.trType> <legsAnim> <torsoAnim> <eFlags hex>
//! <groundEntityNum> <x y z>`), corpses (`eType` 2) included. A joined
//! client taps use while dead, which stock dm respawns on. A joined
//! client also answers every vote with `vote yes` a second after it
//! starts (configstring 15 set: the gamestate's or a `d 15`), so it never
//! holds one short of a pass; `--no-vote` leaves votes alone.
//! After each gamestate it sends the `cp` a pure server checks
//! (`net::pure`, cod2-protocol.md 11), over the iwds of `$COD2_DIR` (or
//! `--game-dir`) with the map's `.d3dbsp` read; without either it sends the
//! bare key, which a pure server also takes. `--systeminfo` prints the pure
//! server's iwd lists; `--bad-cp` sends a wrong key, which a pure server
//! drops at the first `clc_move`.
//! Housekeeping (cod2-protocol.md 17): `--rate N`, `--snaps N`,
//! `--password P` and `--www` (`cl_wwwDownload 1`) go in the userinfo;
//! `--log-packets` prints every datagram as `pkt <ms> <bytes> <sequence>`;
//! `--silent-after S` stops sending after S seconds and keeps listening
//! (a timeout); `--linger S` keeps listening S seconds after the
//! `disconnect` (a zombie); `--say-burst` sends three `say`s 100 ms apart
//! at 5 s (flood protection), or at `--say-at S`; `--score-at S` asks
//! for the scoreboard (the reply prints as a `cmd` line); `--kill-at S` sends `kill` at S s (a death
//! and what it sends, cod2-protocol.md 15.7); `--download-dir D` fetches the referenced
//! iwds the install lacks into `D/main/` (`net::download`), or
//! `--download NAME` asks for that one file whatever the lists say;
//! `--ask NAME` sends `download NAME` as typed beside the `cp` and prints
//! the server's answer without writing anything (the name rules).
//! `--save` writes the session's raw datagrams (`net::capture`) up to the
//! K-th sequenced server message (default all). Run a retail server only in
//! a loopback-only namespace (`unshare -rn`): it heartbeats regardless of
//! `dedicated`.

mod common;

use std::time::{Duration, Instant};

use anyhow::Context;
use common::Session;
use vcod_cod2::net::oob;
use vcod_cod2::net::{Client, Parsed};

#[derive(Default)]
struct Second {
    messages: usize,
    snapshots: usize,
    deltas: usize,
    commands: usize,
    max_left: usize,
}

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let addr = args
        .first()
        .context("usage: spectate <ip:port> [--secs N]")?;
    let opt = |k: &str| {
        args.iter()
            .position(|a| a == k)
            .and_then(|i| args.get(i + 1))
    };
    let secs: u64 = opt("--secs").map_or(Ok(30), |s| s.parse())?;
    let save = opt("--save").cloned();
    let save_messages = opt("--save-messages").map_or(Ok(usize::MAX), |s| s.parse())?;
    let fly = args.iter().any(|a| a == "--fly");
    let join = args.iter().any(|a| a == "--join");
    let auto_vote = join && !args.iter().any(|a| a == "--no-vote");
    let name = opt("--name").map_or("vcodspec", |s| s.as_str());
    let show_systeminfo = args.iter().any(|a| a == "--systeminfo");
    let bad_cp = args.iter().any(|a| a == "--bad-cp");
    let anims = args.iter().any(|a| a == "--anims");
    let rate = opt("--rate").map_or("25000", |s| s.as_str());
    let snaps = opt("--snaps").map_or("20", |s| s.as_str());
    let password = opt("--password");
    let www = args.iter().any(|a| a == "--www");
    let log_packets = args.iter().any(|a| a == "--log-packets");
    let silent_after: Option<u64> = opt("--silent-after").map(|s| s.parse()).transpose()?;
    let linger: u64 = opt("--linger").map_or(Ok(0), |s| s.parse())?;
    let say_at: Option<u64> = opt("--say-at").map(|s| s.parse()).transpose()?;
    let say_burst = say_at.is_some() || args.iter().any(|a| a == "--say-burst");
    let kill_at: Option<u64> = opt("--kill-at").map(|s| s.parse()).transpose()?;
    let score_at: Option<u64> = opt("--score-at").map(|s| s.parse()).transpose()?;
    let mut killed = false;
    let mut scored = false;
    let download_dir = opt("--download-dir").map(std::path::PathBuf::from);
    let download_one = opt("--download").cloned();
    let mut ask = opt("--ask").cloned();
    let fs = common::game_fs(opt("--game-dir").cloned())?;

    let mut s = Session::open(addr, save.is_some(), save_messages)?;
    let mut ui = format!("\\name\\{name}\\rate\\{rate}\\snaps\\{snaps}\\cl_punkbuster\\0");
    if let Some(p) = password {
        ui += &format!("\\password\\{p}");
    }
    if www {
        ui += "\\cl_wwwDownload\\1";
    }
    let mut cl = s.connect(&ui)?;
    let start = Instant::now();
    let mut last_send = Instant::now() - Duration::from_secs(1);
    let mut last_print = Instant::now();
    let mut snap_at = Instant::now();
    let mut sec = Second::default();
    let mut total_msgs = 0usize;
    let mut total_deltas = 0usize;
    let mut said = false;
    let mut downloads: Option<vcod_cod2::net::download::Downloads> = None;
    let mut asked_one = false;
    // The vote answered last (configstring 15, its end time) and when the
    // answer goes out: a second on, past `sv_floodProtect`'s 800 ms.
    let mut voted_end = String::new();
    let mut vote_at: Option<Instant> = None;
    // `CA_PRIMED`: the `cp` is queued and cmds may go out. A client
    // downloading, or waiting for the gamestate after `donedl`, sends none.
    let mut primed = false;
    // Checksums of the files fetched this session (`FS_Restart` adds them).
    let mut fetched: Vec<i32> = Vec::new();

    while start.elapsed() < Duration::from_secs(secs) {
        while let Some(d) = s.recv() {
            if log_packets {
                let seq = u32::from_le_bytes(d[..4.min(d.len())].try_into().unwrap_or([0; 4]));
                println!(
                    "pkt {} {} {}",
                    start.elapsed().as_millis(),
                    d.len(),
                    seq as i32
                );
            }
            if let Some(pl) = oob::payload(&d) {
                println!("oob: {:?}", oob::parse_reply(pl));
                continue;
            }
            let Some(p): Option<Parsed> = cl.receive(&d)? else {
                continue;
            };
            s.messages += 1;
            total_msgs += 1;
            sec.messages += 1;
            sec.commands += p.commands.len();
            for (_, text) in &p.commands {
                println!(
                    "cmd {} {}",
                    start.elapsed().as_millis(),
                    String::from_utf8_lossy(text)
                );
                // `CL_SystemInfoChanged`: a `fast_restart`'s new id comes
                // as `d 1` a frame after its `B` or `n`.
                if let Some(si) = text.strip_prefix(b"d 1 ") {
                    cl.set_systeminfo(&String::from_utf8_lossy(si));
                }
            }
            for m in &p.downloads {
                match m {
                    vcod_cod2::net::download::DownloadMsg::Block { block, size, data } => println!(
                        "dl {} block {block} size {size:?} len {}",
                        start.elapsed().as_millis(),
                        data.len()
                    ),
                    other => println!("dl {} {other:?}", start.elapsed().as_millis()),
                }
                if let Some(dls) = downloads.as_mut() {
                    for ev in dls.on_message(m) {
                        println!("dl event {ev:?}");
                        match ev {
                            vcod_cod2::net::download::DownloadEvent::Command(c) => {
                                cl.add_reliable(&c)
                            }
                            vcod_cod2::net::download::DownloadEvent::Finished(path) => {
                                if let Ok(sums) = vcod_common::pak_checksum::pak_checksums(&path, 0)
                                {
                                    fetched.push(sums.checksum);
                                }
                            }
                            _ => {}
                        }
                    }
                }
            }
            if auto_vote {
                let started = p.commands.iter().find_map(|(_, t)| vote_started(t));
                let started = started.or_else(|| {
                    let gs = cl.parser.gamestate.as_ref().filter(|_| p.gamestate)?;
                    Some(gs.configstring(15).to_string()).filter(|v| !v.is_empty() && v != "0")
                });
                if let Some(end) = started.filter(|e| *e != voted_end) {
                    voted_end = end;
                    vote_at = Some(Instant::now() + Duration::from_secs(1));
                }
            }
            if join {
                for (_, text) in &p.commands {
                    if let Some(reply) = menu_reply(&cl, text) {
                        println!("menu: {reply}");
                        cl.add_reliable(&reply);
                    }
                }
            }
            sec.max_left = sec.max_left.max(p.plain_len - p.consumed);
            if p.gamestate {
                let gs = cl.parser.gamestate.as_ref().unwrap();
                println!(
                    "gamestate: {} configstrings, {} baselines, clientNum {}, feed {}, serverid {:?}",
                    gs.configstrings.len(),
                    gs.baselines.len(),
                    gs.client_num,
                    gs.checksum_feed,
                    gs.server_id()
                );
                let si = gs.configstring(1).to_string();
                let feed = gs.checksum_feed;
                if show_systeminfo {
                    for k in [
                        "sv_pure",
                        "sv_iwds",
                        "sv_iwdNames",
                        "sv_referencedIwds",
                        "sv_referencedIwdNames",
                    ] {
                        let v = vcod_common::net::info_value_for_key(&si, k).unwrap_or("");
                        println!("systeminfo {k}: {v}");
                    }
                }
                let cp = common::cp_for(fs.as_ref(), gs);
                let cp = if bad_cp {
                    vcod_cod2::net::pure::cp_command(&[], !feed)
                } else {
                    cp
                };
                println!("pure: {cp}");
                // `CL_InitDownloads`: a queue first, the `cp` once it is empty.
                let missing = match (&download_dir, &download_one) {
                    (_, Some(one)) if !asked_one => {
                        asked_one = true;
                        vec![one.clone()]
                    }
                    (Some(_), _) => vcod_cod2::net::download::missing_iwds(&si, |sum| {
                        fetched.contains(&sum)
                            || fs
                                .as_ref()
                                .is_some_and(|fs| fs.paks().any(|p| p.checksum == sum))
                    }),
                    _ => Vec::new(),
                };
                let root = download_dir.clone().unwrap_or_else(std::env::temp_dir);
                let mut dls = vcod_cod2::net::download::Downloads::new(&root, &si, missing);
                let first = dls.next_download();
                primed = !first
                    .iter()
                    .any(|e| matches!(e, vcod_cod2::net::download::DownloadEvent::Command(_)));
                if primed {
                    cl.add_reliable(&cp);
                }
                {
                    for ev in first {
                        println!("dl event {ev:?}");
                        if let vcod_cod2::net::download::DownloadEvent::Command(c) = ev {
                            cl.add_reliable(&c);
                        }
                    }
                }
                downloads = Some(dls);
                if let Some(name) = ask.take() {
                    cl.add_reliable(&format!("download {name}"));
                }
            }
            if let Some(n) = p.snapshot {
                snap_at = Instant::now();
                if anims && let Some(f) = cl.parser.frame(n) {
                    print_anims(f);
                }
                sec.snapshots += 1;
                if cl.parser.frame(n).is_some_and(|f| f.delta_num >= 0) {
                    sec.deltas += 1;
                    total_deltas += 1;
                }
            }
        }
        if vote_at.is_some_and(|t| Instant::now() >= t) && primed {
            vote_at = None;
            println!("vote: yes");
            cl.add_reliable("vote yes");
        }
        if say_burst && !said && start.elapsed() >= Duration::from_secs(say_at.unwrap_or(5)) {
            said = true;
            for (i, word) in ["one", "two", "three"].iter().enumerate() {
                if i > 0 {
                    std::thread::sleep(Duration::from_millis(100));
                    let pkt = cl.build_packet(&[])?;
                    s.send(&pkt)?;
                }
                cl.add_reliable(&format!("say {word}"));
                let pkt = cl.build_packet(&[])?;
                s.send(&pkt)?;
            }
        }
        if score_at.is_some_and(|t| start.elapsed() >= Duration::from_secs(t)) && !scored {
            scored = true;
            cl.add_reliable("score");
        }
        if !killed && kill_at.is_some_and(|t| start.elapsed() >= Duration::from_secs(t)) {
            killed = true;
            cl.add_reliable("kill");
        }
        let silent = silent_after.is_some_and(|t| start.elapsed() >= Duration::from_secs(t));
        let have_gs = cl.parser.gamestate.is_some() && primed;
        let interval = if cl.parser.gamestate.is_some() {
            33
        } else {
            500
        };
        if !silent && last_send.elapsed() >= Duration::from_millis(interval) {
            last_send = Instant::now();
            let pkt = if have_gs {
                let base = cl.parser.snapshot().map_or(0, |f| f.server_time);
                let t = base + snap_at.elapsed().as_millis() as i32;
                let mut cmd = cl.idle_cmd(t);
                let dead = cl
                    .parser
                    .snapshot()
                    .is_some_and(|f| f.ps.i32("pm_type") >= 6);
                if fly && !dead {
                    // forward, turning about 90 degrees a second
                    cmd.forward_move = 127;
                    cmd.angles[1] = (cmd.angles[1] + 500) & 0xffff;
                }
                // Dead, a joined client taps use every other cmd: stock dm
                // respawns on `useButtonPressed` (cmd buttons 0x28) unless
                // `scr_forcerespawn` is set.
                if join && dead && (t / 100) % 2 == 0 {
                    cmd.buttons |= 0x20;
                }
                cl.build_packet(&[cmd])?
            } else {
                cl.build_packet(&[])?
            };
            s.send(&pkt)?;
        }
        if last_print.elapsed() >= Duration::from_secs(1) {
            last_print = Instant::now();
            if let Some(f) = cl.parser.snapshot() {
                let o = f.ps.origin();
                println!(
                    "t={:>3}s serverTime {} msgs {} snaps {} delta {} cmds {} ents {} clients {} \
                     origin ({:.1} {:.1} {:.1}) pm_type {} flags {} drops {} left<= {}",
                    start.elapsed().as_secs(),
                    f.server_time,
                    sec.messages,
                    sec.snapshots,
                    sec.deltas,
                    sec.commands,
                    f.entities.len(),
                    f.clients.len(),
                    o[0],
                    o[1],
                    o[2],
                    f.ps.i32("pm_type"),
                    f.snap_flags,
                    cl.netchan.dropped,
                    sec.max_left
                );
            }
            if let Some(f) = cl.parser.snapshot() {
                for e in f.entities.iter().filter(|e| e.number < 64) {
                    let o = e.origin();
                    println!(
                        "    player entity {} eType {} origin ({:.1} {:.1} {:.1})",
                        e.number,
                        e.i32("eType"),
                        o[0],
                        o[1],
                        o[2]
                    );
                }
            }
            sec = Second::default();
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    cl.add_reliable("disconnect");
    for _ in 0..3 {
        let pkt = cl.build_packet(&[])?;
        s.send(&pkt)?;
    }
    let gone = Instant::now();
    while gone.elapsed() < Duration::from_secs(linger) {
        if let Some(d) = s.recv() {
            if log_packets {
                println!("pkt {} {} linger", start.elapsed().as_millis(), d.len());
            }
            if let Ok(Some(p)) = cl.receive(&d) {
                for (_, text) in &p.commands {
                    println!(
                        "cmd {} {}",
                        start.elapsed().as_millis(),
                        String::from_utf8_lossy(text)
                    );
                }
            }
        }
    }
    println!("done: {total_msgs} messages, {total_deltas} delta snapshots");
    if let (Some(path), Some(c)) = (save, &s.capture) {
        std::fs::write(&path, c.encode())?;
        println!("wrote {} packets to {path}", c.packets.len());
    }
    Ok(())
}

/// One `--anims` line for a snapshot.
fn print_anims(f: &vcod_cod2::net::Snapshot2) {
    let ps = &f.ps;
    let mut line = format!(
        "a {} ps {} {} {} {}",
        f.server_time,
        ps.i32("legsAnim"),
        ps.i32("torsoAnim"),
        ps.i32("legsTimer"),
        ps.i32("torsoTimer")
    );
    for e in f
        .entities
        .iter()
        .filter(|e| e.number < 64 || e.i32("eType") == 2)
    {
        let o = e.origin();
        line += &format!(
            " | e {} t{} tr{} {} {} {:x} {} {:.3} {:.3} {:.3}",
            e.number,
            e.i32("eType"),
            e.i32("pos.trType"),
            e.i32("legsAnim"),
            e.i32("torsoAnim"),
            e.i32("eFlags"),
            e.i32("groundEntityNum"),
            o[0],
            o[1],
            o[2]
        );
    }
    println!("{line}");
}

/// The answer to a `t <menu>` (`mr <serverId> <menu> <response>`, the
/// handler at 0x8100d16): `close` to `serverinfo_<gametype>`, which
/// `_menus.gsc` answers with the team menu, `autoassign` to that, the
/// class's rifle to a weapon menu. Stock scripts open the serverinfo menu
/// on every connect that has no `pers["team"]`: the first, a map change,
/// a `map_restart` and a `fast_restart` that sent `B`; an `n` keeps
/// `pers` and opens none.
fn menu_reply(cl: &Client, text: &[u8]) -> Option<String> {
    let text = std::str::from_utf8(text).ok()?;
    let n: u16 = text.strip_prefix("t ")?.trim().parse().ok()?;
    let menu = cl.parser.gamestate.as_ref()?.configstring(1246 + n);
    let reply = |r: &str| Some(format!("mr {} {n} {r}", cl.server_id()));
    if menu.starts_with("serverinfo_") {
        return reply("close");
    }
    if menu.starts_with("team_") {
        return reply("autoassign");
    }
    let weapon = match menu.strip_prefix("weapon_")? {
        "german" => "kar98k_mp",
        "british" => "enfield_mp",
        "american" => "m1garand_mp",
        "russian" => "mosin_nagant_mp",
        _ => return None,
    };
    reply(weapon)
}

/// The end time of a vote a `d 15 <time>` starts (`Cmd_CallVote_f`,
/// cod2-protocol.md 3.4.2); `None` for any other command or the empty
/// `d 15` that closes one.
fn vote_started(text: &[u8]) -> Option<String> {
    let text = std::str::from_utf8(text).ok()?;
    let end = text.strip_prefix("d 15 ")?.trim();
    (!end.is_empty() && end != "0").then(|| end.to_string())
}

#[cfg(test)]
mod tests {
    use super::vote_started;

    #[test]
    fn a_vote_start_is_configstring_15_set() {
        assert_eq!(vote_started(b"d 15 63150").as_deref(), Some("63150"));
        assert_eq!(vote_started(b"d 15 "), None);
        assert_eq!(vote_started(b"d 150 3"), None);
        assert_eq!(vote_started(b"e \"GAME_VOTEPASSED\""), None);
    }
}
