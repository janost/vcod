//! The connect handshake, the client roster and the snapshot stream, driven
//! through the in-process client harness in `common`.

use crate::common;

use common::{ClientEnd, Queues, connect, step, step_dropping_reply};
use std::cell::RefCell;
use std::rc::Rc;
use std::time::{Duration, Instant};
use vcod_common::net::connectionless::info_value_for_key;
use vcod_common::net::msg::{EntityState, NULL_USERCMD};
use vcod_common::net::protocol::PROTOCOL_V1;
use vcod_common::net::{NetClient, NetEvent, NetState};
use vcod_server::{Server, ServerConfig};

fn server() -> Server {
    server_with_entities(0)
}

fn server_with_entities(test_entities: usize) -> Server {
    Server::new(
        ServerConfig {
            hostname: "loop".into(),
            max_clients: 2,
            test_entities,
            ..common::cfg("mp_carentan", "dm")
        },
        Instant::now(),
    )
}

#[test]
fn client_connects_and_loads_the_map_from_the_gamestate() {
    let q = Rc::new(RefCell::new(Queues::default()));
    let mut sv = server();
    let mut now = Instant::now();
    let cl = connect(&mut sv, &q, &mut now);
    assert_eq!(cl.state(), NetState::Active);
    assert_eq!(
        info_value_for_key(cl.configstring(0), "mapname"),
        Some("mp_carentan")
    );
    assert_eq!(
        info_value_for_key(cl.configstring(1), "sv_serverid"),
        Some("16")
    );
    assert!(cl.configstring(7).starts_with("bar_mp "));
    assert_eq!(sv.client_count(), 1);
}

#[test]
fn disconnect_frees_the_slot_and_a_fresh_connect_gets_one() {
    let q = Rc::new(RefCell::new(Queues::default()));
    let mut sv = server();
    let mut now = Instant::now();
    let mut cl = connect(&mut sv, &q, &mut now);
    cl.disconnect();
    now += Duration::from_millis(250);
    step(&mut sv, &q, &mut cl, now);
    assert_eq!(sv.client_count(), 0);

    // The slot is a zombie sending the drop notice to this same socket; the
    // connect takes it back, and the new client ignores those frames until
    // its `connectResponse`.
    let cl2 = connect(&mut sv, &q, &mut now);
    assert_eq!(cl2.state(), NetState::Active);
    assert_eq!(sv.client_count(), 1);
}

#[test]
fn a_reconnect_waits_out_sv_reconnectlimit_then_takes_the_slot() {
    let qa = Rc::new(RefCell::new(Queues::default()));
    let mut sv = server();
    let mut now = Instant::now();
    let _a = connect(&mut sv, &qa, &mut now);
    let a_settled = now;

    // B is the same peer to the server, same address and the same pid-derived
    // qport. Its own queue pair keeps A's netchan out of the exchange. A is
    // not stepped again, so its slot falls silent and B may reclaim it.
    let qb = Rc::new(RefCell::new(Queues::default()));
    let mut b = NetClient::start(ClientEnd(qb.clone()), now);
    for _ in 0..4 {
        now += Duration::from_millis(250);
        let events = step(&mut sv, &qb, &mut b, now);
        assert!(!events.contains(&NetEvent::GamestateReady), "{events:?}");
    }
    // Bounds the exchange above inside the 3 s limit.
    assert!(now.duration_since(a_settled) < Duration::from_secs(2));
    // No reply at all, so B is still resending the connect.
    assert_eq!(b.state(), NetState::Connecting);
    assert_eq!(sv.client_count(), 1);

    now += Duration::from_secs(3);
    let mut ready = false;
    for _ in 0..20 {
        now += Duration::from_millis(250);
        ready = step(&mut sv, &qb, &mut b, now).contains(&NetEvent::GamestateReady);
        if ready {
            break;
        }
    }
    assert!(ready, "no gamestate past the limit; state {:?}", b.state());
    // The reconnect took A's slot.
    assert_eq!(sv.client_count(), 1);
}

#[test]
fn a_live_client_is_not_replaced_by_a_reconnect_with_a_new_challenge() {
    let qa = Rc::new(RefCell::new(Queues::default()));
    let mut sv = server();
    let mut now = Instant::now();
    let mut a = connect(&mut sv, &qa, &mut now);

    // Same peer as A, fresh challenge. A keeps sending moves the whole time,
    // so B never gets past the reconnect limit into A's slot.
    let qb = Rc::new(RefCell::new(Queues::default()));
    let mut b = NetClient::start(ClientEnd(qb.clone()), now);
    for _ in 0..24 {
        now += Duration::from_millis(250);
        a.send_frame(&NULL_USERCMD);
        step(&mut sv, &qa, &mut a, now);
        let events = step(&mut sv, &qb, &mut b, now);
        assert!(!events.contains(&NetEvent::GamestateReady), "{events:?}");
    }
    assert_eq!(a.state(), NetState::Active);
    assert_ne!(b.state(), NetState::Active);
    assert_eq!(sv.client_count(), 1);
}

/// `SV_CheckTimeouts` (cod_lnxded 0x808cbc0) runs on the server clock: a
/// last packet more than `sv_timeout` seconds behind it counts a frame, and
/// the sixth such frame in a row drops the client before that frame's
/// message. Retail, measured with `--probe-edge silence` (handshake doc,
/// "Zombie slots"): the last snapshot is `sv_timeout * 1000 + 250` ms past
/// the frame the last packet arrived after, at `sv_timeout` 1, 2 and 3.
#[test]
fn a_silent_client_s_last_snapshot_is_250_ms_past_its_timeout() {
    let q = Rc::new(RefCell::new(Queues::default()));
    let mut sv = server();
    sv.set_cvar("sv_timeout", "1");
    let mut now = Instant::now();
    let mut cl = connect(&mut sv, &q, &mut now);
    for _ in 0..20 {
        now += Duration::from_millis(50);
        cl.send_frame(&NULL_USERCMD);
        step(&mut sv, &q, &mut cl, now);
    }
    // The last packet goes out straight after this snapshot, so the server
    // stamps it with this frame's time.
    let last_packet_frame = cl.snapshots().newest().unwrap().server_time;
    cl.send_frame(&NULL_USERCMD);
    let mut frames = 0;
    while sv.client_count() > 0 {
        now += Duration::from_millis(50);
        step(&mut sv, &q, &mut cl, now);
        frames += 1;
        assert!(frames < 100);
    }
    let last = cl.snapshots().newest().unwrap().server_time;
    assert_eq!(last - last_packet_frame, 1250);
}

/// A client that disconnects before its first usercmd is a zombie that was
/// never active: `SV_WriteSnapshotToClient` (0x808dee0) ors in
/// `SNAPFLAG_NOT_ACTIVE`, and the slot goes `sv_zombietime` (2 s) past the
/// drop's packet. Retail, `--probe-edge zombie` on loopback (protocol doc,
/// `snapFlags`): 40 messages 50 ms apart, each `w "EXE_DISCONNECTED"` and a
/// snapshot with `snapFlags` 6, no delta and a null playerstate.
#[test]
fn a_never_active_zombie_is_sent_not_active_snapshots_for_two_seconds() {
    let q = Rc::new(RefCell::new(Queues::default()));
    let mut sv = server();
    let mut now = Instant::now();
    let mut cl = connect(&mut sv, &q, &mut now);
    cl.listen_after_drop();
    cl.send_reliable("disconnect");
    cl.send_cmds(&[]);
    let mut seen = Vec::new();
    for _ in 0..60 {
        now += Duration::from_millis(50);
        step(&mut sv, &q, &mut cl, now);
        if let Some(s) = cl.snapshots().newest()
            && seen.last() != Some(&s.message_num)
        {
            assert_eq!(s.snap_flags, 6);
            assert_eq!(s.delta_num, -1);
            assert_eq!(s.ps.field_i32(&PROTOCOL_V1, "commandTime"), 0);
            seen.push(s.message_num);
        }
    }
    assert_eq!(seen.len(), 40);
    assert_eq!(sv.client_count(), 0);
}

/// Once the client acks a frame, later frames delta against it; when acks
/// stop for longer than the ring, the server falls back to uncompressed and
/// recovers when they resume.
#[test]
fn frames_delta_against_the_acked_base_and_fall_back_when_acks_stop() {
    let q = Rc::new(RefCell::new(Queues::default()));
    let mut sv = server();
    let mut now = Instant::now();
    let mut cl = connect(&mut sv, &q, &mut now);
    // The loop's first usercmd puts the client in the world; only a client in
    // the world is sent snapshots.

    let mut seen_delta = false;
    let mut seen_uncompressed_during_silence = false;
    let mut seen_delta_after_recovery = false;
    // A single mid-stream lost reply, well before the long ack silence below.
    // The client's ack the tick after this jumps straight past the sequence
    // it never received, so the base the server picks must line up with the
    // packet's real netchan sequence rather than any renumbering of its own:
    // a uniform off-by-one between the two cancels on every other frame in
    // this test, but not across this gap. See `step_dropping_reply`.
    // Keep this inside 0..40 (acking) and away from the acking/silence
    // boundary at 40 and the loop's end at 80; ticks 38-40 and 78-79 are dead
    // zones where the test silently stops catching what it names.
    const DROPPED_REPLY_TICK: i32 = 10;

    for tick in 0..80 {
        now += Duration::from_millis(50);
        cl.send_frame(&NULL_USERCMD);
        // Ack every frame for the first 40 ticks, then go silent for 35
        // (past the 32-deep ring), then resume.
        let acking = !(40..75).contains(&tick);
        if !acking {
            // The client still sends; the server just never sees it, as if
            // every ack packet were lost in flight.
            q.borrow_mut().to_server.clear();
        }
        if tick == DROPPED_REPLY_TICK {
            step_dropping_reply(&mut sv, &q, &mut cl, now);
        } else {
            step(&mut sv, &q, &mut cl, now);
        }

        if let Some(s) = cl.snapshots().newest() {
            if s.delta_num > 0 {
                seen_delta = true;
                if tick >= 75 {
                    seen_delta_after_recovery = true;
                }
            } else if (40..75).contains(&tick) {
                seen_uncompressed_during_silence = true;
            }
        }
    }

    assert!(seen_delta, "no delta frame while the client was acking");
    assert!(
        seen_uncompressed_during_silence,
        "server kept deltaing against a base the client never acked"
    );
    assert!(
        seen_delta_after_recovery,
        "deltas never resumed once acks came back"
    );
    assert!(
        cl.snapshots().last_invalid().is_none(),
        "client failed to resolve a delta base"
    );
}

/// Scripted entities end to end, asserted against the client's own parsed
/// ring: they appear at their gamestate numbers, entity 32's trajectory
/// restarts (the only field `TestEntities::at` actually varies with time)
/// between two frames while the client is acking, so the delta writer must
/// re-encode it against the base-frame entity, and the cycling entity
/// disappears (the removal bit) and returns (a re-add through the baseline
/// path).
#[test]
fn scripted_entities_move_cycle_out_and_return() {
    let q = Rc::new(RefCell::new(Queues::default()));
    let mut sv = server_with_entities(2);
    let mut now = Instant::now();
    let mut cl = connect(&mut sv, &q, &mut now);
    // The loop's first usercmd puts the client in the world; only a client in
    // the world is sent snapshots.

    let p = &PROTOCOL_V1;
    let time_idx = EntityState::field_index(p, "pos.trTime").unwrap();

    let mut saw_both_entities = false;
    let mut first_time_at_32: Option<i32> = None;
    let mut saw_time_change = false;
    let mut saw_gone = false;
    let mut saw_return_after_gone = false;

    // 50 ms/tick (sv_fps 20); 260 ticks span 13 s, past a full 8 s cycle of
    // the entity that vanishes for 2 s of every cycle, with room to spare.
    for _ in 0..260 {
        now += Duration::from_millis(50);
        cl.send_frame(&NULL_USERCMD);
        step(&mut sv, &q, &mut cl, now);

        let Some(s) = cl.snapshots().newest() else {
            continue;
        };
        if s.entities.contains_key(&32) && s.entities.contains_key(&33) {
            saw_both_entities = true;
        }
        if let Some(e32) = s.entities.get(&32) {
            let t = e32.fields[time_idx];
            match first_time_at_32 {
                None => first_time_at_32 = Some(t),
                Some(first) if t != first => saw_time_change = true,
                _ => {}
            }
        }
        if !s.entities.contains_key(&33) {
            saw_gone = true;
        } else if saw_gone {
            saw_return_after_gone = true;
        }
    }

    assert!(
        saw_both_entities,
        "the scripted entities never appeared at 32 and 33"
    );
    assert!(
        saw_time_change,
        "entity 32's trajectory never re-encoded against its base"
    );
    assert!(saw_gone, "the cycling entity never disappeared");
    assert!(
        saw_return_after_gone,
        "the cycling entity never returned after cycling out"
    );
    assert!(
        cl.snapshots().last_invalid().is_none(),
        "client failed to resolve a delta base"
    );
}

/// A userinfo rename after entering the game changes an existing roster
/// entry in place. None of the other snapshot tests touch this: a lone
/// client whose name never changes always compares equal to its own base, so
/// the roster's delta-against-a-base-entry arm (an entry present on both
/// sides that actually differs) is otherwise never taken.
///
/// The second rename matters as much as the first: "vcod" -> "robertson"
/// fills `name[4]` and `name[8]` (the name packs 4 bytes per netfield), then
/// "robertson" -> "abcd" clears them again. A base that only ever compares
/// against null cannot tell "always zero" from "just cleared", so shrinking
/// the name is what actually exercises a wrong base: a single short-to-short
/// rename (e.g. "vcod" -> "bob") never leaves the null-compared and
/// true-base-compared writers disagreeing, since both names live entirely in
/// `name[0]`, which is non-zero either way.
#[test]
fn a_renamed_client_deltas_against_its_roster_base() {
    let q = Rc::new(RefCell::new(Queues::default()));
    let mut sv = server();
    let mut now = Instant::now();
    let mut cl = connect(&mut sv, &q, &mut now);
    // The loop's first usercmd puts the client in the world; only a client in
    // the world is sent snapshots.

    let p = &PROTOCOL_V1;
    let mut saw_original_name = false;
    let mut saw_long_name = false;
    let mut last_name: Option<String> = None;

    for tick in 0..40 {
        now += Duration::from_millis(50);
        cl.send_frame(&NULL_USERCMD);
        if tick == 5 {
            cl.send_reliable("userinfo \\name\\robertson");
        }
        if tick == 20 {
            cl.send_reliable("userinfo \\name\\abcd");
        }
        step(&mut sv, &q, &mut cl, now);

        let Some(s) = cl.snapshots().newest() else {
            continue;
        };
        let name = s.clients.get(&0).map(|c| c.name(p));
        match name.as_deref() {
            Some("vcod") => saw_original_name = true,
            Some("robertson") => saw_long_name = true,
            _ => {}
        }
        last_name = name;
    }

    assert!(saw_original_name, "never saw the original name");
    assert!(saw_long_name, "the long rename never reached a snapshot");
    assert_eq!(
        last_name.as_deref(),
        Some("abcd"),
        "shrinking the name must clear name[4] and name[8], not leave a stale tail"
    );
    assert!(
        cl.snapshots().last_invalid().is_none(),
        "client failed to resolve a delta base"
    );
}

/// The retail client never sends `begin`. It is not in the engine's client
/// command table (CoDExtended, `src/sv_client.c`, `ucmds`), and Quake III
/// Arena's table (which CoD inherited) does not have it either. Entry
/// into the world is the first usercmd after the gamestate (Quake III Arena,
/// `code/server/sv_client.c`, `SV_UserMove`), so a client that only acks and
/// moves must be promoted and start receiving snapshots. Without that
/// trigger the retail client loads to 100% and hangs on the loading screen
/// forever.
#[test]
fn a_client_that_never_sends_begin_enters_the_world_on_its_first_usercmd() {
    let q = Rc::new(RefCell::new(Queues::default()));
    let mut sv = server();
    let mut now = Instant::now();
    let mut cl = connect(&mut sv, &q, &mut now);

    for _ in 0..8 {
        now += Duration::from_millis(50);
        cl.send_frame(&NULL_USERCMD);
        step(&mut sv, &q, &mut cl, now);
    }

    assert!(
        cl.snapshots().newest().is_some(),
        "no snapshot: the client never left CS_PRIMED without sending `begin`"
    );
}

/// Steps until the client is in the world and holding snapshots.
fn enter(
    sv: &mut Server,
    q: &Rc<RefCell<Queues>>,
    cl: &mut NetClient<ClientEnd>,
    now: &mut Instant,
) {
    for _ in 0..8 {
        *now += Duration::from_millis(50);
        cl.send_frame(&NULL_USERCMD);
        step(sv, q, cl, *now);
    }
    assert!(cl.snapshots().newest().is_some());
}

/// Whether the client parsed a valid frame out of the last message it took.
fn last_frame_valid(cl: &NetClient<ClientEnd>) -> bool {
    cl.snapshots().get(cl.incoming_sequence()).is_some()
}

/// A client reads a server command's `%` as `.` (protocol-1.1.md,
/// divergence 14) and keys its moves on that copy, so the server's ring
/// keys on the same: the yaw a move carries after the ack lands.
#[test]
fn a_move_after_a_percent_server_command_decodes() {
    let q = Rc::new(RefCell::new(Queues::default()));
    let mut sv = server();
    let mut now = Instant::now();
    let mut cl = connect(&mut sv, &q, &mut now);
    enter(&mut sv, &q, &mut cl, &mut now);
    sv.push_console("say a%b");
    let mut cmd = NULL_USERCMD;
    let mut acked_at = None;
    for i in 0..12 {
        now += Duration::from_millis(50);
        if acked_at.is_some_and(|at| i > at + 1) {
            cmd.angles[1] = 0x4000; // 90 degrees
        }
        cl.send_frame(&cmd);
        let events = step(&mut sv, &q, &mut cl, now);
        if events
            .iter()
            .any(|e| matches!(e, NetEvent::Chat { text, .. } if text.ends_with("a.b")))
        {
            acked_at = Some(i);
        }
    }
    assert!(acked_at.is_some_and(|at| at < 6), "the say never came");
    let s = cl.snapshots().newest().expect("a frame");
    assert!(last_frame_valid(&cl));
    assert!((s.ps.viewangles(&PROTOCOL_V1)[1] - 90.0).abs() < 0.1);
}

/// A reliable command keeps the byte the wire carried: a 0x92 inside a
/// UTF-8 char goes out as `.`, which is what the server keys its messages
/// on, so the client decodes them on `.` too.
#[test]
fn messages_after_a_command_with_byte_0x92_decode() {
    let q = Rc::new(RefCell::new(Queues::default()));
    let mut sv = server();
    let mut now = Instant::now();
    let mut cl = connect(&mut sv, &q, &mut now);
    enter(&mut sv, &q, &mut cl, &mut now);
    // U+0412 is D0 92 in UTF-8.
    cl.send_reliable("say \u{412}");
    for _ in 0..6 {
        now += Duration::from_millis(50);
        cl.send_frame(&NULL_USERCMD);
        let events = step(&mut sv, &q, &mut cl, now);
        assert!(!events.iter().any(|e| matches!(e, NetEvent::Dropped(_))));
        assert!(last_frame_valid(&cl), "message {}", cl.incoming_sequence());
    }
}

/// A snapshot the client cannot parse is never a base: its next move goes
/// out as `clc_moveNoDelta` (cod11-demos.md 4), the server answers with a
/// full frame and the deltas resume from there.
#[test]
fn a_lost_frame_is_followed_by_a_no_delta_move_and_a_full_frame() {
    let q = Rc::new(RefCell::new(Queues::default()));
    let mut sv = server();
    let mut now = Instant::now();
    let mut cl = connect(&mut sv, &q, &mut now);
    enter(&mut sv, &q, &mut cl, &mut now);
    assert!(cl.snapshots().newest().unwrap().delta_num > 0);

    // One message cut in half on its way to the client.
    now += Duration::from_millis(50);
    cl.send_frame(&NULL_USERCMD);
    let pending: Vec<Vec<u8>> = q.borrow_mut().to_server.drain(..).collect();
    for p in pending {
        sv.handle_packet(common::ADDR, &p, now);
    }
    sv.tick(now);
    for (_, mut p) in sv.take_outgoing() {
        p.truncate(8 + (p.len() - 8) / 2);
        q.borrow_mut().to_client.push_back(p);
    }
    cl.pump_at(now);
    assert!(!last_frame_valid(&cl));

    let mut kinds = Vec::new();
    for _ in 0..4 {
        now += Duration::from_millis(50);
        cl.send_frame(&NULL_USERCMD);
        step(&mut sv, &q, &mut cl, now);
        assert!(last_frame_valid(&cl), "message {}", cl.incoming_sequence());
        kinds.push(cl.snapshots().newest().unwrap().delta_num > 0);
    }
    assert_eq!(kinds, [false, true, true, true]);
}

/// `client-probes/<probe>` on ours, mp_carentan, the install's protocol.
/// `None` without paks.
fn probe_server(probe: &str) -> Option<Server> {
    let fs = vcod_common::testing::game_fs()?;
    let proto = vcod_server::server::install_protocol(&fs);
    let fs = Rc::new(fs);
    let src = std::fs::read_to_string(format!(
        "../gsc/tests/fixtures/semantics/client-probes/{probe}.gsc"
    ))
    .expect("the probe");
    let (sv, _) = common::Boot {
        proto: Some(proto),
        script: Some(&src),
        ..common::Boot::new("mp_carentan", probe)
    }
    .start(fs);
    Some(sv)
}

/// Steps `sv` and `cl` 50 ms at a time for `ms`, the client sending a cmd
/// each step when `moving`; returns the client's events.
fn run_for(
    sv: &mut Server,
    q: &Rc<RefCell<Queues>>,
    cl: &mut NetClient<ClientEnd>,
    now: &mut Instant,
    ms: u64,
    moving: bool,
) -> Vec<NetEvent> {
    let mut events = Vec::new();
    for _ in 0..ms / 50 {
        *now += Duration::from_millis(50);
        if moving {
            cl.send_frame(&NULL_USERCMD);
        }
        events.extend(step(sv, q, cl, *now));
    }
    events
}

/// `client-probes/probe_svinfoquote` on retail 1.1d, 1.5b and UO 1.51b,
/// mp_carentan, 2026-10-20, one plain `--net-probe` client and getinfo /
/// getstatus over a socket (`fixtures/handshake/svinfo-quote.txt`): the cvar keeps
/// `"`, `;` and `\`, the serverinfo drops all three, and a value with
/// nothing left drops its key from configstring 0 and from both replies.
/// All three servers sent the same. cod11-server-handshake.md, "The
/// challenge key".
#[test]
fn a_quote_in_a_serverinfo_value_is_stripped_like_retail() {
    let Some(mut sv) = probe_server("probe_svinfoquote") else {
        return;
    };
    let q = Rc::new(RefCell::new(Queues::default()));
    let mut now = Instant::now();
    let mut cl = connect(&mut sv, &q, &mut now);
    let query = |sv: &mut Server, now: Instant| {
        let at: std::net::SocketAddr = "127.0.0.1:27999".parse().unwrap();
        let mut got = Vec::new();
        for (ask, key) in [("getinfo xxx", "hostname"), ("getstatus", "sv_hostname")] {
            sv.handle_packet(at, &vcod_common::net::connectionless::build_oob(ask), now);
            for (_, p) in sv.take_outgoing().into_iter().filter(|(to, _)| *to == at) {
                let (_, rest) = vcod_common::net::connectionless::parse_oob(&p).unwrap();
                let text = String::from_utf8_lossy(rest).into_owned();
                let info = text.lines().next().unwrap_or("").to_string();
                got.push(info_value_for_key(&info, key).map(str::to_string));
            }
        }
        got
    };
    let mut seen = vec![info_value_for_key(cl.configstring(0), "sv_hostname").map(str::to_string)];
    let mut replies = vec![query(&mut sv, now)];
    // The probe's hostnames change at 8, 11 and 14 s; sample between.
    for until in [9500u64, 3000, 3000, 3000] {
        for e in run_for(&mut sv, &q, &mut cl, &mut now, until, true) {
            if e == NetEvent::ConfigstringChanged(0) {
                seen.push(
                    info_value_for_key(cl.configstring(0), "sv_hostname").map(str::to_string),
                );
            }
        }
        replies.push(query(&mut sv, now));
    }
    let host = |s: Option<&str>| s.map(str::to_string);
    assert_eq!(
        seen,
        [
            host(Some("qhsb e")),
            host(Some("lead")),
            None,
            host(Some("tail"))
        ],
        "configstring 0"
    );
    let replies: Vec<Option<String>> = replies
        .into_iter()
        .map(|r| {
            assert_eq!(r.len(), 2, "one getinfo and one getstatus reply");
            assert_eq!(r[0], r[1]);
            r[0].clone()
        })
        .collect();
    assert_eq!(
        replies,
        [
            host(Some("qhsb e")),
            host(Some("lead")),
            None,
            host(Some("tail")),
            host(Some("tail"))
        ],
        "getinfo and getstatus"
    );
    let log: Vec<&str> = sv
        .script_log()
        .iter()
        .filter_map(|l| l.trim().strip_prefix("PROBE "))
        .filter_map(|l| l.split_once(' ').map(|(_, r)| r))
        .filter(|l| l.starts_with("hostname"))
        .collect();
    assert_eq!(
        log,
        [
            "hostname [q\"h;s\\b e]",
            "hostname [\"lead]",
            "hostname [\"]",
            "hostname [tail\"]"
        ],
        "the cvar itself"
    );
}

/// `client-probes/probe_primedprint` on retail 1.1d, 1.5b and UO 1.51b,
/// mp_carentan, 2026-10-20, one plain `--net-probe` client behind a relay
/// that dropped its sequenced packets for 3 s after the gamestate
/// (`fixtures/handshake/primed-print.txt`). Every print sent while the client was
/// connected or primed, its own or the broadcast, never arrived; every
/// client cvar did, the first inside the gamestate message and the rest
/// with the first snapshot, then the print sent at `begin`. All three
/// servers sent the same. docs/protocol-1.1.md, "The server command
/// queue".
#[test]
fn prints_sent_while_primed_are_dropped_and_cvars_kept() {
    let Some(mut sv) = probe_server("probe_primedprint") else {
        return;
    };
    let q = Rc::new(RefCell::new(Queues::default()));
    let mut now = Instant::now();
    let mut cl = NetClient::start(ClientEnd(q.clone()), now);
    let mut events = Vec::new();
    while !events.contains(&NetEvent::GamestateReady) {
        assert!(events.len() < 10_000 && cl.state() != NetState::Disconnected);
        events.extend(run_for(&mut sv, &q, &mut cl, &mut now, 50, false));
    }
    // Primed: the client acks nothing and sends no cmd.
    events.extend(run_for(&mut sv, &q, &mut cl, &mut now, 3000, false));
    events.extend(run_for(&mut sv, &q, &mut cl, &mut now, 2000, true));
    let got: Vec<String> = events
        .iter()
        .filter_map(|e| match e {
            NetEvent::GamestateReady => Some("gamestate".to_string()),
            NetEvent::ServerCommand(t) if t[0] == "v" && t[1].starts_with("pp_") => {
                Some(format!("v {}", t[1]))
            }
            // `iPrintLn` is `f`, type 0; the client cvar `v`, type 1.
            NetEvent::ServerCommand(t) if t[0] == "f" && t[1].starts_with("pp ") => {
                Some(format!("print {}", t[1]))
            }
            _ => None,
        })
        .collect();
    let sent = sv
        .script_log()
        .iter()
        .filter(|l| l.contains(" sent "))
        .count();
    assert!(sent > 4, "the probe sent {sent}");
    let mut want = vec!["v pp_0".to_string(), "gamestate".to_string()];
    want.extend((1..sent).map(|n| format!("v pp_{n}")));
    want.push("print pp entered".to_string());
    assert_eq!(got, want);
}
