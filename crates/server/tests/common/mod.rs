//! vcod's own NetClient against the server, in process, over two queues.
//! `now` is advanced by hand. Shared by the tests that drive a client; each
//! of them uses part of it, hence the crate-level allow.
#![allow(dead_code)]

use std::cell::RefCell;
use std::collections::VecDeque;
use std::net::SocketAddr;
use std::rc::Rc;
use std::time::{Duration, Instant};
use vcod_common::net::{NetClient, NetEvent, Transport};
use vcod_server::Server;

pub const ADDR: SocketAddr =
    SocketAddr::new(std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST), 31337);

#[derive(Default)]
pub struct Queues {
    pub to_server: VecDeque<Vec<u8>>,
    pub to_client: VecDeque<Vec<u8>>,
}

pub struct ClientEnd(pub Rc<RefCell<Queues>>);

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

/// One exchange each way, then one client pump.
pub fn step(
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

/// Like `step`, but the server's reply never reaches the client — a lost
/// snapshot packet, not a lost ack. The client's next real ack then names a
/// frame it received without having received the one right before it, the
/// only case where a server-side message_num that has drifted from the
/// packet's actual netchan sequence is observable on the wire (see
/// `frames_delta_against_the_acked_base_and_fall_back_when_acks_stop`).
pub fn step_dropping_reply(
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
    sv.take_outgoing();
    cl.pump_at(now)
}

pub fn connect(
    sv: &mut Server,
    q: &Rc<RefCell<Queues>>,
    now: &mut Instant,
) -> NetClient<ClientEnd> {
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
