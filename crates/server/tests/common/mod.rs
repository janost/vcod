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

// --------------------------------------------------------------- the fixtures

/// Every `# key value` clause in a fixture's header block with this key. The
/// callers insist on exactly one: the header is prose as well as data, so a
/// second clause opening with the same word would otherwise decide the join
/// silently, by being first.
pub fn header_values<'a>(text: &'a str, key: &str) -> Vec<&'a str> {
    text.lines()
        .take_while(|l| l.starts_with('#'))
        .flat_map(|l| l.trim_start_matches('#').split(','))
        .map(str::trim)
        .filter_map(|clause| clause.strip_prefix(key)?.strip_prefix(' '))
        .map(|v| v.trim_end_matches('.'))
        .collect()
}

/// The one clause with this key, or a panic naming `path` and what it found.
pub fn header_value<'a>(text: &'a str, key: &str, path: &str) -> &'a str {
    let found = header_values(text, key);
    assert_eq!(
        found.len(),
        1,
        "{path}: the header has {} clauses opening with {key:?}, expected 1: {found:?}",
        found.len()
    );
    found[0]
}

/// The team and weapon the retail playerstate capture was taken with, so a
/// test that drives the stock menus asks for the same two things the gate
/// does rather than carrying its own copy of them.
pub fn captured_join(map: &str, gametype: &str) -> (String, String) {
    let path = format!("tests/fixtures/playerstate/{map}-{gametype}.txt");
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {path}: {e}"));
    (
        header_value(&text, "joined", &path).to_string(),
        header_value(&text, "weapon", &path).to_string(),
    )
}

// ------------------------------------------------------------------- the join

/// The fixture header's "3 s after the weapon menu was answered".
pub const SPAWN_SETTLE: Duration = Duration::from_secs(3);

/// Answers the stock team and weapon menus. `v g_scriptMainMenu <menu>` names
/// the menu, `t <index>` opens it and `mr <serverId> <index> <response>`
/// answers it (docs/research/cod11-hud-protocol.md, section 0.1).
pub struct Join {
    team: String,
    weapon: String,
    main_menu: String,
    answered: Vec<i32>,
    answered_team: bool,
    answered_weapon_at: Option<Instant>,
    log: Vec<String>,
}

impl Join {
    pub fn new(team: &str, weapon: &str) -> Self {
        Join {
            team: team.to_string(),
            weapon: weapon.to_string(),
            main_menu: String::new(),
            answered: Vec::new(),
            answered_team: false,
            answered_weapon_at: None,
            log: Vec::new(),
        }
    }

    pub fn on_server_command(
        &mut self,
        tokens: &[String],
        cl: &mut NetClient<ClientEnd>,
        now: Instant,
    ) {
        match tokens.first().map(String::as_str) {
            Some("v") if tokens.get(1).map(String::as_str) == Some("g_scriptMainMenu") => {
                self.main_menu = tokens.get(2).cloned().unwrap_or_default();
            }
            Some("t") => {
                let Some(idx) = tokens.get(1).and_then(|t| t.parse::<i32>().ok()) else {
                    return;
                };
                if self.answered.contains(&idx) {
                    return;
                }
                let menu = self.main_menu.clone();
                let reply = if menu.starts_with("team_") {
                    self.team.clone()
                } else if menu.starts_with("weapon_") {
                    self.weapon.clone()
                } else {
                    self.log.push(format!("menu {idx} ({menu:?}) has no reply"));
                    return;
                };
                cl.send_reliable(&format!("mr {} {idx} {reply}", cl.server_id()));
                self.answered.push(idx);
                self.log
                    .push(format!("answered menu {idx} ({menu}) with {reply}"));
                if menu.starts_with("weapon_") {
                    self.answered_weapon_at = Some(now);
                } else {
                    self.answered_team = true;
                }
            }
            _ => {}
        }
    }

    pub fn settled(&self, now: Instant) -> bool {
        self.answered_weapon_at
            .is_some_and(|t| now.duration_since(t) >= SPAWN_SETTLE)
    }

    /// What the join did, for a failure message.
    pub fn summary(&self) -> String {
        let log = match self.log.is_empty() {
            true => "the server opened no script menu at all".to_string(),
            false => self.log.join("; "),
        };
        format!("join: begin sent, {log}")
    }

    /// The path, not the state it ended in: a player that reached the
    /// playerstate without being asked which team and which weapon did not
    /// join through the stock menus, whatever its 103 fields say.
    pub fn findings(&self) -> Vec<String> {
        let mut out = Vec::new();
        if !self.answered_team {
            out.push("no team menu opened, so the client never answered one".to_string());
        }
        if self.answered_weapon_at.is_none() {
            out.push("no weapon menu opened, so the client never spawned".to_string());
        }
        out
    }
}

/// Connects a client, sends `begin`, answers both stock menus with `team` and
/// `weapon`, and runs on until the spawn has settled. 20 s of simulated time
/// at sv_fps 20, well past the settle a completed join needs; a join that
/// never happens burns the lot.
pub fn join(
    sv: &mut Server,
    q: &Rc<RefCell<Queues>>,
    now: &mut Instant,
    team: &str,
    weapon: &str,
) -> (NetClient<ClientEnd>, Join) {
    let mut cl = connect(sv, q, now);
    // `begin` is what releases `Callback_PlayerConnect`'s `waittill`; the join
    // menus follow from it.
    cl.send_reliable("begin");
    let mut j = Join::new(team, weapon);
    for _ in 0..400 {
        *now += Duration::from_millis(50);
        cl.send_frame(&vcod_common::net::msg::NULL_USERCMD);
        for e in step(sv, q, &mut cl, *now) {
            match e {
                NetEvent::ServerCommand(tokens) => j.on_server_command(&tokens, &mut cl, *now),
                NetEvent::Dropped(r) => panic!("dropped mid-join: {r}"),
                _ => {}
            }
        }
        if j.settled(*now) {
            break;
        }
    }
    (cl, j)
}
