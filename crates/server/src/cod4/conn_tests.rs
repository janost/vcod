//! The netchan's acceptance and the `donedl` limit on a bare level, one
//! in-process probe (`test_harness::Probe`); no install needed.

use std::time::{Duration, Instant};

use super::test_harness::Probe;
use super::*;

/// A bare server and a probe in the world past its first snapshots.
fn joined() -> (Cod4Server, Probe, Instant) {
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
    let mut p = Probe::new("conn", 0, 28990, 0, &[]);
    let mut now = Instant::now();
    p.join(&mut sv, now);
    for _ in 0..40 {
        step(&mut sv, &mut p, &mut now);
    }
    assert_eq!(sv.core.clients[0].as_ref().unwrap().state, State::Active);
    (sv, p, now)
}

/// One probe message, one frame, the probe reads what came.
fn step(sv: &mut Cod4Server, p: &mut Probe, now: &mut Instant) {
    let pkt = p.packet(sv.core.sv_time_ms);
    sv.handle_packet(p.addr, &pkt, *now);
    *now += Duration::from_millis(FRAME_MS as u64);
    sv.tick_frames(*now, 1);
    let svs = sv.core.sv_time_ms;
    for (to, pkt) in sv.take_outgoing() {
        if to == p.addr {
            p.receive(svs, &pkt);
        }
    }
}

/// A message short of `clc_EOF` (no ops at all) under a sequence far
/// ahead, and a whole one acknowledging a message the server never sent,
/// are dropped before the netchan takes them: from another port of the
/// probe's address under its qport they move neither the sequence nor the
/// address, and the probe's own next message is taken.
#[test]
fn a_cod4_message_that_does_not_read_leaves_the_client_untouched() {
    let (mut sv, mut p, mut now) = joined();
    let state = |sv: &Cod4Server| {
        let l = &sv.core.clients[0].as_ref().unwrap().link;
        (l.netchan.incoming_sequence, l.reliable_ack, l.addr)
    };
    let before = state(&sv);
    let forger = SocketAddr::from(([127, 0, 0, 1], 40000));

    let mut pkt = 1_000_000u32.to_le_bytes().to_vec();
    pkt.extend_from_slice(&p.qport.to_le_bytes());
    pkt.push(sv.game.server_id as u8);
    pkt.extend_from_slice(&p.ack.to_le_bytes());
    pkt.extend_from_slice(&before.1.to_le_bytes());
    sv.handle_packet(forger, &pkt, now);
    assert_eq!(state(&sv), before, "a message of no ops");

    let ack = p.ack;
    p.ack = sv.core.clients[0]
        .as_ref()
        .unwrap()
        .link
        .netchan
        .outgoing_sequence as i32;
    let pkt = p.packet(sv.core.sv_time_ms);
    p.ack = ack;
    sv.handle_packet(forger, &pkt, now);
    assert_eq!(state(&sv), before, "an acknowledge of a message never sent");

    step(&mut sv, &mut p, &mut now);
    let l = &sv.core.clients[0].as_ref().unwrap().link;
    assert!(l.netchan.incoming_sequence > before.0);
    assert_eq!(l.addr, p.addr);
}

/// Fifty `donedl`s in one message build one gamestate, not fifty: another
/// waits until a message went out after it.
#[test]
fn a_cod4_donedl_flood_sends_one_gamestate() {
    let (mut sv, mut p, now) = joined();
    let sent = |sv: &Cod4Server| {
        let c = sv.core.clients[0].as_ref().unwrap();
        c.link.netchan.outgoing_sequence
    };
    let before = sent(&sv);
    for _ in 0..50 {
        assert!(p.command("donedl"));
    }
    let pkt = p.packet(sv.core.sv_time_ms);
    sv.handle_packet(p.addr, &pkt, now);
    assert_eq!(sent(&sv), before + 1);
    let c = sv.core.clients[0].as_ref().unwrap();
    assert_eq!(c.gamestate_message, before as i32);
}

/// Frames with no message from the probe, as a client whose game went
/// down.
fn silent(sv: &mut Cod4Server, now: &mut Instant, ms: i32) {
    for _ in 0..ms / FRAME_MS {
        *now += Duration::from_millis(FRAME_MS as u64);
        sv.tick_frames(*now, 1);
        sv.take_outgoing();
    }
}

/// `getchallenge` then `connect` from `from` under `qport`: the replies.
fn connect_from(sv: &mut Cod4Server, from: SocketAddr, qport: u16, now: Instant) -> Vec<Vec<u8>> {
    let mut d = b"\xff\xff\xff\xff".to_vec();
    d.extend_from_slice(
        format!("getchallenge 0 \"{}\"", vcod_cod4::net::conn::KEY_HASH).as_bytes(),
    );
    sv.handle_packet(from, &d, now);
    let reply = sv.take_outgoing();
    let text = String::from_utf8_lossy(&reply[0].1[4..]).into_owned();
    let challenge: i32 = text.split_whitespace().nth(1).unwrap().parse().unwrap();
    let ui = format!("\\name\\conn\\protocol\\6\\challenge\\{challenge}\\qport\\{qport}");
    let mut d = b"\xff\xff\xff\xff".to_vec();
    d.extend_from_slice(format!("connect \"{ui}\"").as_bytes());
    sv.handle_packet(from, &d, now);
    sv.take_outgoing()
        .into_iter()
        .filter(|(to, _)| *to == from)
        .map(|(_, p)| p)
        .collect()
}

/// A client connecting again from its address and qport frees its old
/// slot (`SV_FreeClient`, cod4-handshake.md 9); a wrong `g_password` then
/// refuses the connect and leaves the slot free, not held by the old
/// client the level already let go of.
#[test]
fn a_cod4_reconnect_refused_by_g_password_frees_the_slot() {
    let (mut sv, p, mut now) = joined();
    // Past `sv_reconnectlimit`'s 3 s since the first connect, and silent
    // as long: the new challenge may take the slot.
    silent(&mut sv, &mut now, 3_100);
    sv.set_cvar("g_password", "secret");
    let reply = connect_from(&mut sv, p.addr, p.qport, now);
    assert_eq!(reply.len(), 1);
    assert!(reply[0].ends_with(b"error\nGAME_INVALIDPASSWORD"));
    assert!(sv.core.clients[0].is_none());
}

/// A connect from the client's ip under its qport with another challenge
/// is refused while the client is still heard from (vcod's guard,
/// cod4-handshake.md 9, "Reconnects"), with or without `g_password`: the
/// client keeps its slot and its next message is taken.
#[test]
fn a_cod4_connect_with_a_foreign_challenge_leaves_a_live_client() {
    let (mut sv, mut p, mut now) = joined();
    for _ in 0..70 {
        step(&mut sv, &mut p, &mut now);
    }
    let neighbour = SocketAddr::from(([127, 0, 0, 1], 40001));
    for password in ["", "secret"] {
        sv.set_cvar("g_password", password);
        assert!(connect_from(&mut sv, neighbour, p.qport, now).is_empty());
        let c = sv.core.clients[0].as_ref().unwrap();
        assert_eq!((c.state, c.link.addr), (State::Active, p.addr));
    }
    let before = sv.core.clients[0]
        .as_ref()
        .unwrap()
        .link
        .netchan
        .incoming_sequence;
    step(&mut sv, &mut p, &mut now);
    let l = &sv.core.clients[0].as_ref().unwrap().link;
    assert!(l.netchan.incoming_sequence > before);
}

/// `stats` and `v` from another port of the client's address under its
/// qport change nothing: no stats chunk, no keep-alive. A `stats` from the
/// client itself once it is in the world is answered with its mask and
/// leaves the block the scripts read alone.
#[test]
fn a_cod4_stats_or_voice_packet_off_another_port_changes_no_slot() {
    use vcod_cod4::net::stats::{STAT_CHUNK, Stats};
    let (mut sv, mut p, now) = joined();
    let forged = |idx: u8| {
        let mut d = b"\xff\xff\xff\xffstats\0".to_vec();
        d.extend_from_slice(&p.qport.to_le_bytes());
        d.push(idx);
        d.extend_from_slice(&[0xab; STAT_CHUNK]);
        d
    };
    let block = |sv: &Cod4Server| sv.core.clients[0].as_ref().unwrap().extra.stats.0.to_vec();
    let fresh = Stats::fresh().0.to_vec();
    assert_eq!(block(&sv), fresh);
    sv.core.clients[0].as_mut().unwrap().link.last_packet_ms = -5;
    let forger = SocketAddr::from(([127, 0, 0, 1], 40000));
    sv.handle_packet(forger, &forged(0), now);
    let voice = vcod_common::net::voice::client_packet(p.qport, &[vec![1, 2, 3]]);
    sv.handle_packet(forger, &voice, now);
    assert!(sv.take_outgoing().iter().all(|(to, _)| *to != forger));
    let c = sv.core.clients[0].as_ref().unwrap();
    assert_eq!((c.link.last_packet_ms, c.link.addr), (-5, p.addr));
    assert_eq!(block(&sv), fresh);
    let reply = p.oob(&mut sv, &forged(0)[4..], now);
    assert_eq!(reply, [b"\xff\xff\xff\xffstatResponse 0".to_vec()]);
    assert_eq!(block(&sv), fresh);
}

/// `SV_UserMove` refuses a move message of more than 32 cmds or none
/// (0x81725bd `cmdCount < 1`, 0x81725c3 `cmdCount > MAX_PACKET_USERCMDS`)
/// before it reads one (BUG-102).
#[test]
fn a_cod4_move_of_33_cmds_or_none_is_refused() {
    let (mut sv, _p, _now) = joined();
    let last = |sv: &Cod4Server| sv.core.clients[0].as_ref().unwrap().last_cmd_time;
    let t0 = last(&sv);
    let cmds = |n: i32| -> Vec<UserCmd> {
        (1..=n)
            .map(|i| UserCmd {
                server_time: t0 + i,
                ..UserCmd::default()
            })
            .collect()
    };
    sv.user_move(0, -1, false, &cmds(33));
    assert_eq!(last(&sv), t0);
    sv.user_move(0, -1, false, &[]);
    assert_eq!(last(&sv), t0);
    sv.user_move(0, -1, false, &cmds(32));
    assert_eq!(last(&sv), t0 + 32);
}

/// Hostile datagrams on both entry points, connectionless and netchan,
/// from the client's own address and a stranger's: none panics, none takes
/// a slot or moves the client's, and the client's next message is taken.
#[test]
fn a_cod4_server_shrugs_off_malformed_packets() {
    let (mut sv, mut p, mut now) = joined();
    sv.set_cvar("sv_voice", "1");
    sv.set_cvar("rcon_password", "pw");
    let q = p.qport.to_le_bytes();
    let oob = |parts: &[&[u8]]| {
        let mut d = vec![0xff; 4];
        for part in parts {
            d.extend_from_slice(part);
        }
        d
    };
    let mut cases: Vec<Vec<u8>> = vec![
        Vec::new(),
        vec![1, 2, 3],
        vec![0xff; 4],
        oob(&[b"stats\0", &q[..1]]),
        oob(&[b"stats\0", &q]),
        oob(&[b"stats\0", &q, &[7], &[0xab; 64]]),
        oob(&[b"stats\0", &q, &[255], &[0xab; 2000]]),
        oob(&[b"stats\0", &q, &[0], &[0xab; 3]]),
        oob(&[b"v"]),
        oob(&[b"v\0", &q[..1]]),
        oob(&[b"v\0", &q, &[255]]),
        oob(&[b"v\0", &q, &[3, 255, 1]]),
        oob(&[b"rcon"]),
        oob(&[b"rcon pw"]),
        oob(&[b"rcon pw ", &[b'x'; 2000]]),
        oob(&[b"connect"]),
        oob(&[b"connect \"\\protocol\\6\\challenge\\-1\\qport\\0\""]),
        oob(&[b"getchallenge 0 \"", &[b'a'; 2000], b"\""]),
    ];
    // The netchan header's extremes: sequence, then the message and
    // reliable acknowledges, with and without a body.
    for (seq, ack, rack) in [
        (u32::MAX, i32::MIN, i32::MIN),
        (0x8000_0000, -1, -1),
        (p.ack as u32 + 1, i32::MAX, i32::MAX),
        (p.ack as u32 + 1, 0, i32::MIN),
    ] {
        let mut d = seq.to_le_bytes().to_vec();
        d.extend_from_slice(&q);
        d.push(sv.game.server_id as u8);
        d.extend_from_slice(&ack.to_le_bytes());
        d.extend_from_slice(&rack.to_le_bytes());
        cases.push(d.clone());
        d.extend_from_slice(&[0x55; 40]);
        cases.push(d);
    }
    let stranger = SocketAddr::from(([10, 0, 0, 9], 40002));
    let state = |sv: &Cod4Server| {
        let c = sv.core.clients[0].as_ref().unwrap();
        (c.state, c.link.addr, c.link.reliable_ack)
    };
    let before = state(&sv);
    for d in &cases {
        for from in [stranger, p.addr] {
            sv.handle_packet(from, d, now);
            sv.take_outgoing();
        }
    }
    assert_eq!(state(&sv), before);
    assert_eq!(sv.core.clients.iter().flatten().count(), 1);
    let seq = sv.core.clients[0]
        .as_ref()
        .unwrap()
        .link
        .netchan
        .incoming_sequence;
    step(&mut sv, &mut p, &mut now);
    let c = sv.core.clients[0].as_ref().unwrap();
    assert!(c.link.netchan.incoming_sequence > seq);
    assert_eq!(c.state, State::Active);
}

/// `sv_floodprotect` 4 (`cod4_lnxded` 0x8170c97..0x8170f97): an active
/// client's fifth game command inside the window reaches no game; `team `,
/// `score` and `mr ` never count, case for case; a refused one moves the
/// window on, and one 800 ms past the last counted one refills the count.
#[test]
fn cod4_flood_protect_counts_game_commands_in_the_window() {
    let (mut sv, _, _) = joined();
    let c = sv.core.clients[0].as_mut().unwrap();
    c.link.next_reliable_ms = 0;
    let mut got = Vec::new();
    for (t, text) in [
        (1000, "say a"),
        (1001, "say b"),
        (1002, "say c"),
        (1003, "say d"),
        (1004, "say e"),
        (1005, "score"),
        (1005, "mr 3 1 x"),
        (1005, "team axis"),
        (1006, "Score"),
        (1805, "say f"),
        (2605, "say g"),
        (2606, "say h"),
    ] {
        got.push((text, flood_check(c, 4, t, text)));
    }
    let refused: Vec<&str> = got.iter().filter(|(_, ok)| !ok).map(|(t, _)| *t).collect();
    assert_eq!(refused, ["say e", "Score", "say f"]);
    assert_eq!(c.extra.flood_count, 2, "{got:?}");
    assert!(flood_check(c, 0, 2607, "say i"), "sv_floodprotect 0");
    c.state = State::Primed;
    assert!(flood_check(c, 4, 2607, "say j"), "not in the world");
}
