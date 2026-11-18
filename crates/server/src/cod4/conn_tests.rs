//! The netchan's acceptance and the `donedl` limit on a bare level, one
//! in-process probe (`wire_ab::Probe`); no install needed.

use std::time::{Duration, Instant};

use super::wire_ab::Probe;
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

/// A client connecting again from its address and qport frees its old
/// slot (`SV_FreeClient`, cod4-handshake.md 9); a wrong `g_password` then
/// refuses the connect and leaves the slot free, not held by the old
/// client the level already let go of.
#[test]
fn a_cod4_reconnect_refused_by_g_password_frees_the_slot() {
    let (mut sv, mut p, mut now) = joined();
    // Past `sv_reconnectlimit`'s 3 s since the first connect.
    for _ in 0..40 {
        step(&mut sv, &mut p, &mut now);
    }
    sv.set_cvar("g_password", "secret");
    let reply = p.oob(
        &mut sv,
        format!("getchallenge 0 \"{}\"", vcod_cod4::net::conn::KEY_HASH).as_bytes(),
        now,
    );
    let text = String::from_utf8_lossy(&reply[0][4..]).into_owned();
    let challenge: i32 = text.split_whitespace().nth(1).unwrap().parse().unwrap();
    let ui = format!(
        "\\name\\conn\\protocol\\6\\challenge\\{challenge}\\qport\\{}",
        p.qport
    );
    let reply = p.oob(&mut sv, format!("connect \"{ui}\"").as_bytes(), now);
    assert_eq!(reply.len(), 1);
    assert!(reply[0].ends_with(b"error\nGAME_INVALIDPASSWORD"));
    assert!(sv.core.clients[0].is_none());
}
