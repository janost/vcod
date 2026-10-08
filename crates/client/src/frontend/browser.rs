//! The server list behind `joinserver.menu`'s list box: one master query,
//! then a `getinfo` ping to every address it returns. Nothing blocks the
//! frame: the master's name resolves on a helper thread and the socket is
//! non-blocking (docs/research/cod11-front-end.md, "Master query").

use std::net::{SocketAddr, SocketAddrV4, ToSocketAddrs, UdpSocket};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use vcod_common::net::connectionless::{build_oob, parse_oob};
use vcod_common::net::master::{
    GETINFO, GETSERVERS, MASTER_HOST, MASTER_PORT, ServerInfo, parse_servers_response,
};

/// Pings in flight at once, and how long one waits for its reply. vcod's
/// numbers, not measured.
const MAX_IN_FLIGHT: usize = 32;
const PING_TIMEOUT: Duration = Duration::from_millis(1500);
/// How long the master gets before the refresh gives up on it.
const MASTER_TIMEOUT: Duration = Duration::from_secs(5);

pub struct Server {
    pub addr: SocketAddrV4,
    pub info: Option<ServerInfo>,
    /// Round trip of the `getinfo`, once it came back.
    pub ping: Option<u32>,
    sent: Option<Instant>,
    timed_out: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Idle,
    /// Resolving the master or waiting on its list.
    Master,
    Pinging,
    Done,
    Failed,
}

pub struct Browser {
    socket: Option<UdpSocket>,
    resolve: Option<mpsc::Receiver<Option<SocketAddr>>>,
    master: Option<SocketAddr>,
    started: Option<Instant>,
    pub servers: Vec<Server>,
    pub status: Status,
    /// `ServerSort` column, `None` for arrival order.
    pub sort: Option<usize>,
}

impl Default for Browser {
    fn default() -> Self {
        Browser {
            socket: None,
            resolve: None,
            master: None,
            started: None,
            servers: Vec::new(),
            status: Status::Idle,
            sort: None,
        }
    }
}

impl Browser {
    /// `RefreshServers`: drops the list and asks the master again.
    pub fn refresh(&mut self) {
        let socket = match UdpSocket::bind("0.0.0.0:0").and_then(|s| {
            s.set_nonblocking(true)?;
            Ok(s)
        }) {
            Ok(s) => s,
            Err(e) => {
                log::warn!("browser: cannot open a socket: {e}");
                self.status = Status::Failed;
                return;
            }
        };
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let addr = (MASTER_HOST, MASTER_PORT)
                .to_socket_addrs()
                .ok()
                .and_then(|mut a| a.find(SocketAddr::is_ipv4));
            let _ = tx.send(addr);
        });
        *self = Browser {
            socket: Some(socket),
            resolve: Some(rx),
            started: Some(Instant::now()),
            status: Status::Master,
            sort: self.sort,
            ..Browser::default()
        };
    }

    /// `RefreshFilter` (the Quick Refresh button): pings the servers already
    /// listed again without asking the master.
    pub fn requery(&mut self) {
        if self.servers.is_empty() {
            return self.refresh();
        }
        let socket = match UdpSocket::bind("0.0.0.0:0").and_then(|s| {
            s.set_nonblocking(true)?;
            Ok(s)
        }) {
            Ok(s) => s,
            Err(e) => {
                log::warn!("browser: cannot open a socket: {e}");
                return;
            }
        };
        for s in &mut self.servers {
            (s.info, s.ping, s.sent, s.timed_out) = (None, None, None, false);
        }
        self.socket = Some(socket);
        self.resolve = None;
        self.status = Status::Pinging;
    }

    /// `stopRefresh` / `closeJoin`: stops pinging, keeps what came back.
    pub fn stop(&mut self) {
        self.socket = None;
        self.resolve = None;
        if matches!(self.status, Status::Master | Status::Pinging) {
            self.status = Status::Done;
        }
    }

    /// Reads replies and sends the next pings; once a frame.
    pub fn poll(&mut self, now: Instant) {
        if let Some(rx) = &self.resolve
            && let Ok(addr) = rx.try_recv()
        {
            self.resolve = None;
            match (addr, &self.socket) {
                (Some(addr), Some(socket)) => {
                    log::info!("browser: requesting servers from {addr}");
                    let _ = socket.send_to(&build_oob(GETSERVERS), addr);
                    self.master = Some(addr);
                }
                _ => {
                    log::warn!("browser: cannot resolve {MASTER_HOST}");
                    self.status = Status::Failed;
                    self.socket = None;
                }
            }
        }
        let Some(socket) = &self.socket else { return };
        let mut buf = [0u8; 16384];
        while let Ok((n, from)) = socket.recv_from(&mut buf) {
            let Some((cmd, rest)) = parse_oob(&buf[..n]) else {
                continue;
            };
            match cmd {
                "getserversResponse" if Some(from) == self.master => {
                    for addr in parse_servers_response(rest) {
                        if !self.servers.iter().any(|s| s.addr == addr) {
                            self.servers.push(Server {
                                addr,
                                info: None,
                                ping: None,
                                sent: None,
                                timed_out: false,
                            });
                        }
                    }
                    self.status = Status::Pinging;
                }
                "infoResponse" => {
                    let SocketAddr::V4(from) = from else { continue };
                    if let Some(s) = self.servers.iter_mut().find(|s| s.addr == from)
                        && let Some(sent) = s.sent
                        && s.info.is_none()
                    {
                        // Latin-1, as the font indexes glyphs by byte.
                        let text: String = rest.iter().map(|&b| b as char).collect();
                        s.info = Some(ServerInfo::parse(&text));
                        s.ping = Some((now - sent).as_millis().max(1) as u32);
                    }
                }
                _ => {}
            }
        }
        if self.status == Status::Master && self.started.is_some_and(|t| now - t > MASTER_TIMEOUT) {
            log::warn!("browser: no answer from the master");
            self.status = Status::Failed;
            return;
        }
        if self.status != Status::Pinging {
            return;
        }
        let mut in_flight = 0;
        for s in &mut self.servers {
            if let Some(sent) = s.sent
                && s.info.is_none()
                && !s.timed_out
            {
                if now - sent > PING_TIMEOUT {
                    s.timed_out = true;
                } else {
                    in_flight += 1;
                }
            }
        }
        for s in self.servers.iter_mut().filter(|s| s.sent.is_none()) {
            if in_flight >= MAX_IN_FLIGHT {
                break;
            }
            let _ = socket.send_to(&build_oob(GETINFO), s.addr);
            s.sent = Some(now);
            in_flight += 1;
        }
        if in_flight == 0 {
            self.status = Status::Done;
            self.socket = None;
        }
    }

    /// Servers that answered, in display order.
    pub fn rows(&self) -> Vec<&Server> {
        let mut rows: Vec<&Server> = self.servers.iter().filter(|s| s.info.is_some()).collect();
        if let Some(col) = self.sort {
            rows.sort_by(|a, b| {
                let (ia, ib) = (a.info.as_ref().unwrap(), b.info.as_ref().unwrap());
                match col {
                    1 => strip(&ia.hostname)
                        .to_lowercase()
                        .cmp(&strip(&ib.hostname).to_lowercase()),
                    2 => ia.mapname.to_lowercase().cmp(&ib.mapname.to_lowercase()),
                    3 => ib.clients.cmp(&ia.clients),
                    4 => strip(&ia.gametype).cmp(&strip(&ib.gametype)),
                    5 => a.ping.cmp(&b.ping),
                    _ => std::cmp::Ordering::Equal,
                }
            });
        }
        rows
    }

    /// How many addresses the master gave and how many answered.
    pub fn counts(&self) -> (usize, usize) {
        let answered = self.servers.iter().filter(|s| s.info.is_some()).count();
        (self.servers.len(), answered)
    }
}

/// `text` without `^N` colour codes.
pub fn strip(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '^' && chars.peek().is_some_and(|d| d.is_ascii_digit()) {
            chars.next();
            continue;
        }
        out.push(c);
    }
    out
}

/// A list column's text, as the UI module's server feeder builds it
/// (ui_mp_x86.dll 0x4000c8ad, research doc "Server list columns").
pub fn column_text(s: &Server, col: usize) -> String {
    let Some(info) = &s.info else {
        return String::new();
    };
    let ping = s.ping.unwrap_or(0);
    match col {
        0 if info.password => "X".into(),
        0 => String::new(),
        1 if ping > 0 => info.hostname.clone(),
        1 => s.addr.to_string(),
        2 => info.mapname.clone(),
        3 => format!("{} ({})", info.clients, info.max_clients),
        4 if info.gametype.is_empty() => "?".into(),
        4 => info.gametype.clone(),
        5 if ping > 0 => ping.to_string(),
        5 => "...".into(),
        _ => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn server(name: &str, clients: u32, ping: u32) -> Server {
        Server {
            addr: "1.2.3.4:28960".parse().unwrap(),
            info: Some(ServerInfo {
                hostname: name.into(),
                mapname: "mp_harbor".into(),
                gametype: "^7dm".into(),
                clients,
                max_clients: 20,
                protocol: 1,
                password: true,
            }),
            ping: Some(ping),
            sent: None,
            timed_out: false,
        }
    }

    #[test]
    fn columns_follow_the_feeder() {
        let s = server("^1Red", 3, 42);
        let cols: Vec<String> = (0..6).map(|c| column_text(&s, c)).collect();
        assert_eq!(cols, ["X", "^1Red", "mp_harbor", "3 (20)", "^7dm", "42"]);
    }

    #[test]
    fn sort_by_players_puts_the_fullest_first() {
        let mut b = Browser {
            servers: vec![server("a", 1, 50), server("b", 9, 80), server("c", 4, 20)],
            sort: Some(3),
            ..Browser::default()
        };
        let names: Vec<_> = b
            .rows()
            .iter()
            .map(|s| s.info.as_ref().unwrap().hostname.clone())
            .collect();
        assert_eq!(names, ["b", "c", "a"]);
        b.sort = Some(5);
        assert_eq!(b.rows()[0].info.as_ref().unwrap().hostname, "c");
    }

    #[test]
    fn info_replies_land_on_their_server_with_a_ping() {
        let server_sock = UdpSocket::bind("127.0.0.1:0").unwrap();
        let SocketAddr::V4(addr) = server_sock.local_addr().unwrap() else {
            unreachable!()
        };
        let mut b = Browser {
            socket: Some(UdpSocket::bind("127.0.0.1:0").unwrap()),
            status: Status::Pinging,
            ..Browser::default()
        };
        b.socket.as_ref().unwrap().set_nonblocking(true).unwrap();
        b.servers.push(Server {
            addr,
            info: None,
            ping: None,
            sent: None,
            timed_out: false,
        });
        let t0 = Instant::now();
        b.poll(t0);
        let mut buf = [0u8; 256];
        let (n, from) = server_sock.recv_from(&mut buf).unwrap();
        assert_eq!(&buf[..n], b"\xff\xff\xff\xffgetinfo xxx");
        let mut reply = build_oob("infoResponse\n");
        reply.extend_from_slice(b"\\hostname\\test\\clients\\2\\sv_maxclients\\8");
        server_sock.send_to(&reply, from).unwrap();
        // Loopback delivery is quick but not instant.
        for _ in 0..100 {
            b.poll(t0 + Duration::from_millis(30));
            if b.status == Status::Done {
                break;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(b.status, Status::Done);
        assert_eq!(b.rows().len(), 1);
        assert_eq!(b.servers[0].ping, Some(30));
        assert_eq!(column_text(&b.servers[0], 3), "2 (8)");
    }
}
