//! The connect handshake, the client roster and the snapshot stream, driven
//! through the in-process client harness in `common`.

mod common;

use common::{connect, step, step_dropping_reply, ClientEnd, Queues};
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
            map: "mp_carentan".into(),
            hostname: "loop".into(),
            max_clients: 2,
            gametype: "dm".into(),
            test_entities,
            trace: false,
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

    // An ordinary connect, not a reconnect; `sv_reconnectlimit` only looks at
    // clients that still hold a slot.
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

#[test]
fn silent_client_times_out() {
    let q = Rc::new(RefCell::new(Queues::default()));
    let mut sv = server();
    let mut now = Instant::now();
    let _cl = connect(&mut sv, &q, &mut now);
    sv.tick(now + Duration::from_secs(241));
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
    // packet's real netchan sequence rather than any renumbering of its own
    // — a uniform off-by-one between the two cancels on every other frame in
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
/// the name is what actually exercises a wrong base — a single short-to-short
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
/// Arena's table -- which CoD inherited -- does not have it either. Entry
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
