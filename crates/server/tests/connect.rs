//! vcod's own NetClient against the server, in process, over two queues.
//! `now` is advanced by hand.

use std::cell::RefCell;
use std::collections::VecDeque;
use std::net::SocketAddr;
use std::rc::Rc;
use std::time::{Duration, Instant};
use vcod_common::net::connectionless::info_value_for_key;
use vcod_common::net::msg::NULL_USERCMD;
use vcod_common::net::{NetClient, NetEvent, NetState, Transport};
use vcod_server::{Server, ServerConfig};

const ADDR: SocketAddr =
    SocketAddr::new(std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST), 31337);

#[derive(Default)]
struct Queues {
    to_server: VecDeque<Vec<u8>>,
    to_client: VecDeque<Vec<u8>>,
}

struct ClientEnd(Rc<RefCell<Queues>>);

impl Transport for ClientEnd {
    fn try_recv(&mut self, buf: &mut [u8]) -> Option<usize> {
        let p = self.0.borrow_mut().to_client.pop_front()?;
        buf[..p.len()].copy_from_slice(&p);
        Some(p.len())
    }
    fn send(&mut self, data: &[u8]) {
        self.0.borrow_mut().to_server.push_back(data.to_vec());
    }
}

fn server() -> Server {
    Server::new(
        ServerConfig {
            map: "mp_carentan".into(),
            hostname: "loop".into(),
            max_clients: 2,
            gametype: "dm".into(),
        },
        Instant::now(),
    )
}

/// One exchange each way, then one client pump.
fn step(
    sv: &mut Server,
    q: &Rc<RefCell<Queues>>,
    cl: &mut NetClient<ClientEnd>,
    now: Instant,
) -> Vec<NetEvent> {
    let pending: Vec<Vec<u8>> = q.borrow_mut().to_server.drain(..).collect();
    for p in pending {
        sv.handle_packet(ADDR, &p, now);
    }
    sv.tick(now);
    for (to, p) in sv.take_outgoing() {
        assert_eq!(to, ADDR);
        q.borrow_mut().to_client.push_back(p);
    }
    cl.pump_at(now)
}

fn connect(sv: &mut Server, q: &Rc<RefCell<Queues>>, now: &mut Instant) -> NetClient<ClientEnd> {
    let mut cl = NetClient::start(ClientEnd(q.clone()), *now);
    for _ in 0..40 {
        *now += Duration::from_millis(250);
        let events = step(sv, q, &mut cl, *now);
        if events.contains(&NetEvent::GamestateReady) {
            return cl;
        }
        assert!(
            !events.iter().any(|e| matches!(e, NetEvent::Dropped(_))),
            "{events:?}"
        );
    }
    panic!(
        "no gamestate within 10 s of simulated time; state {:?}",
        cl.state()
    );
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

    let mut seen_delta = false;
    let mut seen_uncompressed_during_silence = false;
    let mut seen_delta_after_recovery = false;

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
        step(&mut sv, &q, &mut cl, now);

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
