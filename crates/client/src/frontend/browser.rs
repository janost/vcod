//! The server lists behind `joinserver.menu`'s list box, one per source as
//! `ui_netSource` numbers them: Local (a `getinfo` broadcast), Internet (the
//! master's list) and Favorites (`servercache.dat`). Every listed address
//! gets a `getinfo` ping. Nothing blocks the frame: the master's name
//! resolves on a helper thread and the socket is non-blocking
//! (docs/research/cod11-front-end.md, "Master query", "Local servers",
//! "Favourites").

use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4, ToSocketAddrs, UdpSocket};
use std::path::PathBuf;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use vcod_common::net::connectionless::{build_oob, parse_oob};
use vcod_common::net::master::{
    GETINFO, GETSERVERS, LAN_PORTS, LAN_ROUNDS, MASTER_HOST, MASTER_PORT, ServerInfo,
    parse_address, parse_servers_response,
};
use vcod_common::net::server_cache::{self, Entry, MAX_FAVORITES};

/// Pings in flight at once, and how long one waits for its reply. vcod's
/// numbers, not measured.
const MAX_IN_FLIGHT: usize = 32;
const PING_TIMEOUT: Duration = Duration::from_millis(1500);
/// How long the master gets before the refresh gives up on it.
const MASTER_TIMEOUT: Duration = Duration::from_secs(5);
/// How long a Local refresh listens for broadcast answers: the UI's refresh
/// time for source 0 (ui_mp_x86.dll 0x4000eb5b).
const LAN_WINDOW: Duration = Duration::from_millis(1000);
/// `MAX_OTHER_SERVERS`, the Local list's cap.
const MAX_LOCAL: usize = 0x80;

/// `ui_netSource`'s values, in the order the source owner draw cycles.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    Local = 0,
    Internet = 1,
    Favorites = 2,
}

impl Source {
    /// The names `UI_NETSOURCE` prints (ui_mp_x86.dll table 0x40036ad8).
    pub const NAMES: [&'static str; 3] = ["@EXE_LOCAL", "@EXE_INTERNET", "@EXE_FAVORITES"];

    /// `ui_netSource`'s value; out of range reads as Local, as
    /// `UI_NetSource_HandleKey` wraps it.
    pub fn from_cvar(v: &str) -> Source {
        match v.trim().parse::<f32>().map(|f| f as i32) {
            Ok(1) => Source::Internet,
            Ok(2) => Source::Favorites,
            _ => Source::Local,
        }
    }

    /// The next source on a click (ui_mp_x86.dll 0x40009b90).
    pub fn next(self) -> Source {
        match self {
            Source::Local => Source::Internet,
            Source::Internet => Source::Favorites,
            Source::Favorites => Source::Local,
        }
    }
}

/// The filter popup's four switches (`UI_BuildServerDisplayList`,
/// ui_mp_x86.dll 0x4000b800). All on by default.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Filter {
    pub show_full: bool,
    pub show_empty: bool,
    pub show_password: bool,
    pub show_no_password: bool,
}

impl Default for Filter {
    fn default() -> Self {
        Filter {
            show_full: true,
            show_empty: true,
            show_password: true,
            show_no_password: true,
        }
    }
}

impl Filter {
    fn passes(&self, info: &ServerInfo) -> bool {
        (self.show_empty || info.clients != 0)
            && (self.show_full || info.clients != info.max_clients)
            && (self.show_password || !info.password)
            && (self.show_no_password || info.password)
    }
}

pub struct Server {
    pub addr: SocketAddrV4,
    /// The favourite's stored name; a reply's hostname replaces it.
    pub name: String,
    pub info: Option<ServerInfo>,
    /// Round trip of the `getinfo`, once it came back.
    pub ping: Option<u32>,
    sent: Option<Instant>,
    timed_out: bool,
}

impl Server {
    fn new(addr: SocketAddrV4) -> Server {
        Server {
            addr,
            name: String::new(),
            info: None,
            ping: None,
            sent: None,
            timed_out: false,
        }
    }
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

/// `LAN_AddServer`'s answers (CoDMP.exe 0x417640), in the order it checks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AddFavorite {
    Added,
    Full,
    BadAddress,
    InList,
}

pub struct Browser {
    socket: Option<UdpSocket>,
    resolve: Option<mpsc::Receiver<Option<SocketAddr>>>,
    master: Option<SocketAddr>,
    started: Option<Instant>,
    pub source: Source,
    /// Indexed by [`Source`].
    lists: [Vec<Server>; 3],
    pub status: Status,
    /// `ServerSort` column, `None` for arrival order.
    pub sort: Option<usize>,
    pub filter: Filter,
    /// Where a Local refresh sends its broadcasts; tests empty it.
    pub(super) lan_targets: Vec<SocketAddrV4>,
    /// `servercache.dat`; `None` keeps favourites in memory only.
    cache: Option<PathBuf>,
    /// Favourites vcod cannot address (IPX, loopback), written back as read.
    foreign: Vec<Entry>,
}

impl Default for Browser {
    fn default() -> Self {
        Browser {
            socket: None,
            resolve: None,
            master: None,
            started: None,
            source: Source::Local,
            lists: Default::default(),
            status: Status::Idle,
            sort: None,
            filter: Filter::default(),
            lan_targets: LAN_PORTS
                .iter()
                .map(|&p| SocketAddrV4::new(Ipv4Addr::BROADCAST, p))
                .collect(),
            cache: None,
            foreign: Vec::new(),
        }
    }
}

fn open_socket() -> Option<UdpSocket> {
    UdpSocket::bind("0.0.0.0:0")
        .and_then(|s| {
            s.set_nonblocking(true)?;
            s.set_broadcast(true)?;
            Ok(s)
        })
        .map_err(|e| log::warn!("browser: cannot open a socket: {e}"))
        .ok()
}

impl Browser {
    /// A browser whose favourites live in `cache`, read now as
    /// `LAN_LoadCachedServers` reads it at UI start.
    pub fn with_cache(cache: PathBuf) -> Browser {
        let mut b = Browser::default();
        if let Ok(bytes) = std::fs::read(&cache) {
            for e in server_cache::read_favorites(&bytes) {
                match e.addr() {
                    Some(addr) => b.lists[Source::Favorites as usize].push(Server {
                        name: e.hostname(),
                        ..Server::new(addr)
                    }),
                    None => b.foreign.push(e),
                }
            }
        }
        b.cache = Some(cache);
        b
    }

    fn list(&self) -> &Vec<Server> {
        &self.lists[self.source as usize]
    }

    fn list_mut(&mut self) -> &mut Vec<Server> {
        &mut self.lists[self.source as usize]
    }

    /// A new source: stops the refresh running for the old one, and starts
    /// one for Local and Favorites as `UI_NetSource_HandleKey` does.
    pub fn set_source(&mut self, source: Source) {
        if source == self.source {
            return;
        }
        self.stop();
        self.source = source;
        self.status = Status::Idle;
        if source != Source::Internet {
            self.refresh();
        }
    }

    /// `RefreshServers`: drops the source's list and asks again: the master
    /// for Internet, a broadcast for Local; favourites are pinged again.
    pub fn refresh(&mut self) {
        let Some(socket) = open_socket() else {
            self.status = Status::Failed;
            return;
        };
        let now = Instant::now();
        self.resolve = None;
        self.master = None;
        self.started = Some(now);
        match self.source {
            Source::Internet => {
                let (tx, rx) = mpsc::channel();
                std::thread::spawn(move || {
                    let addr = (MASTER_HOST, MASTER_PORT)
                        .to_socket_addrs()
                        .ok()
                        .and_then(|mut a| a.find(SocketAddr::is_ipv4));
                    let _ = tx.send(addr);
                });
                self.list_mut().clear();
                self.resolve = Some(rx);
                self.status = Status::Master;
            }
            Source::Local => {
                self.list_mut().clear();
                for _ in 0..LAN_ROUNDS {
                    for &to in &self.lan_targets {
                        if let Err(e) = socket.send_to(&build_oob(GETINFO), to) {
                            log::warn!("browser: broadcast to {to}: {e}");
                        }
                    }
                }
                self.status = Status::Pinging;
            }
            Source::Favorites => {
                self.reset_pings();
                self.status = Status::Pinging;
            }
        }
        self.socket = Some(socket);
    }

    fn reset_pings(&mut self) {
        for s in self.list_mut() {
            (s.info, s.ping, s.sent, s.timed_out) = (None, None, None, false);
        }
    }

    /// `RefreshFilter` (the Quick Refresh button): pings the servers already
    /// listed again without asking the master.
    pub fn requery(&mut self) {
        if self.list().is_empty() {
            return self.refresh();
        }
        let Some(socket) = open_socket() else { return };
        self.reset_pings();
        self.socket = Some(socket);
        self.resolve = None;
        self.started = Some(Instant::now());
        self.status = Status::Pinging;
    }

    /// `stopRefresh` / `closeJoin`: stops pinging, keeps what came back.
    pub fn stop(&mut self) {
        self.socket = None;
        self.resolve = None;
        if matches!(self.status, Status::Master | Status::Pinging) {
            self.finish();
        }
    }

    fn finish(&mut self) {
        self.status = Status::Done;
        self.socket = None;
        if self.source == Source::Favorites {
            self.save();
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
        let Some(socket) = self.socket.take() else {
            return;
        };
        let mut buf = [0u8; 16384];
        while let Ok((n, from)) = socket.recv_from(&mut buf) {
            let Some((cmd, rest)) = parse_oob(&buf[..n]) else {
                continue;
            };
            let SocketAddr::V4(from4) = from else {
                continue;
            };
            match cmd {
                "getserversResponse" if Some(from) == self.master => {
                    for addr in parse_servers_response(rest) {
                        if !self.list().iter().any(|s| s.addr == addr) {
                            self.list_mut().push(Server::new(addr));
                        }
                    }
                    self.status = Status::Pinging;
                }
                "infoResponse" => {
                    // Latin-1, as the font indexes glyphs by byte.
                    let text: String = rest.iter().map(|&b| b as char).collect();
                    let info = ServerInfo::parse(&text);
                    let started = self.started;
                    let local = self.source == Source::Local;
                    let list = self.list_mut();
                    // A broadcast's answers are new servers, timed from the
                    // broadcast (`CL_ServerInfoPacket` adds them to the
                    // Local list).
                    if local && !list.iter().any(|s| s.addr == from4) && list.len() < MAX_LOCAL {
                        let mut s = Server::new(from4);
                        s.sent = started;
                        list.push(s);
                    }
                    if let Some(s) = list.iter_mut().find(|s| s.addr == from4)
                        && let Some(sent) = s.sent
                        && s.info.is_none()
                    {
                        s.name.clone_from(&info.hostname);
                        s.info = Some(info);
                        s.ping = Some((now - sent).as_millis().max(1) as u32);
                    }
                }
                _ => {}
            }
        }
        self.socket = Some(socket);
        let since = self.started.map_or(Duration::ZERO, |t| now - t);
        if self.status == Status::Master && since > MASTER_TIMEOUT {
            log::warn!("browser: no answer from the master");
            self.status = Status::Failed;
            self.socket = None;
            return;
        }
        if self.status != Status::Pinging {
            return;
        }
        let socket = self.socket.take().unwrap();
        let mut in_flight = 0;
        for s in self.list_mut().iter_mut() {
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
        for s in self.list_mut().iter_mut().filter(|s| s.sent.is_none()) {
            if in_flight >= MAX_IN_FLIGHT {
                break;
            }
            let _ = socket.send_to(&build_oob(GETINFO), s.addr);
            s.sent = Some(now);
            in_flight += 1;
        }
        self.socket = Some(socket);
        let listening = self.source == Source::Local && since < LAN_WINDOW;
        if in_flight == 0 && !listening {
            self.finish();
        }
    }

    /// The servers the list shows, in display order: those that answered
    /// (every favourite, answered or not) that pass the filter.
    pub fn rows(&self) -> Vec<&Server> {
        let all = self.source == Source::Favorites;
        let blank = ServerInfo::default();
        let mut rows: Vec<&Server> = self
            .list()
            .iter()
            .filter(|s| {
                (all || s.info.is_some()) && self.filter.passes(s.info.as_ref().unwrap_or(&blank))
            })
            .collect();
        if let Some(col) = self.sort {
            rows.sort_by(|a, b| {
                let ia = a.info.as_ref().unwrap_or(&blank);
                let ib = b.info.as_ref().unwrap_or(&blank);
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

    /// The source's list length (`LAN_GetServerCount`) and how many
    /// answered.
    pub fn counts(&self) -> (usize, usize) {
        let answered = self.list().iter().filter(|s| s.info.is_some()).count();
        (self.list().len(), answered)
    }

    /// `LAN_AddServer(AS_FAVORITES, name, address)`: the cap, then the
    /// address, then a duplicate. The cache is written at once.
    pub fn add_favorite(&mut self, name: &str, address: &str) -> AddFavorite {
        let favs = &mut self.lists[Source::Favorites as usize];
        if favs.len() + self.foreign.len() >= MAX_FAVORITES {
            return AddFavorite::Full;
        }
        let Some(addr) = parse_address(address) else {
            return AddFavorite::BadAddress;
        };
        if favs.iter().any(|s| s.addr == addr) {
            return AddFavorite::InList;
        }
        favs.push(Server {
            name: name.chars().take(31).collect(),
            ..Server::new(addr)
        });
        self.save();
        AddFavorite::Added
    }

    /// `LAN_RemoveServer(AS_FAVORITES, address)`.
    pub fn remove_favorite(&mut self, addr: SocketAddrV4) {
        let favs = &mut self.lists[Source::Favorites as usize];
        let before = favs.len();
        favs.retain(|s| s.addr != addr);
        if favs.len() != before {
            self.save();
        }
    }

    /// Writes the favourites into `servercache.dat`, keeping the rest of
    /// the file.
    fn save(&self) {
        let Some(path) = &self.cache else { return };
        let mut entries: Vec<Entry> = self.lists[Source::Favorites as usize]
            .iter()
            .map(|s| {
                let mut e = Entry::new(s.addr, &s.name);
                if let Some(info) = &s.info {
                    e.set_info(info, s.ping.map_or(-1, |p| p as i32));
                }
                e
            })
            .collect();
        entries.extend(self.foreign.iter().cloned());
        let old = std::fs::read(path).ok();
        let bytes = server_cache::write_favorites(old.as_deref(), &entries);
        if let Err(e) = std::fs::write(path, bytes) {
            log::warn!("browser: cannot write {}: {e}", path.display());
        }
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
/// (ui_mp_x86.dll 0x4000c8ad, research doc "Server list columns"). A
/// favourite that has not answered reads as zeros with its address.
pub fn column_text(s: &Server, col: usize) -> String {
    let blank = ServerInfo::default();
    let info = s.info.as_ref().unwrap_or(&blank);
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
            ..Server::new("1.2.3.4:28960".parse().unwrap())
        }
    }

    fn names(b: &Browser) -> Vec<String> {
        b.rows()
            .iter()
            .map(|s| s.info.as_ref().unwrap().hostname.clone())
            .collect()
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
            sort: Some(3),
            ..Browser::default()
        };
        b.lists[0] = vec![server("a", 1, 50), server("b", 9, 80), server("c", 4, 20)];
        assert_eq!(names(&b), ["b", "c", "a"]);
        b.sort = Some(5);
        assert_eq!(b.rows()[0].info.as_ref().unwrap().hostname, "c");
    }

    #[test]
    fn the_filter_drops_empty_full_and_locked_servers() {
        let mut b = Browser::default();
        let mut open = server("open", 3, 10);
        open.info.as_mut().unwrap().password = false;
        b.lists[0] = vec![server("empty", 0, 10), server("full", 20, 10), open];
        assert_eq!(names(&b).len(), 3);
        b.filter.show_empty = false;
        b.filter.show_full = false;
        assert_eq!(names(&b), ["open"]);
        b.filter = Filter {
            show_no_password: false,
            ..Filter::default()
        };
        assert_eq!(names(&b), ["empty", "full"]);
    }

    /// A loopback "server" that answers `getinfo` with `info`.
    fn answer(sock: &UdpSocket, info: &str) -> SocketAddr {
        let mut buf = [0u8; 256];
        let (n, from) = sock.recv_from(&mut buf).unwrap();
        assert_eq!(&buf[..n], b"\xff\xff\xff\xffgetinfo xxx");
        let mut reply = build_oob("infoResponse\n");
        reply.extend_from_slice(info.as_bytes());
        sock.send_to(&reply, from).unwrap();
        from
    }

    fn poll_until_done(b: &mut Browser, t: Instant) {
        // Loopback delivery is quick but not instant.
        for _ in 0..100 {
            b.poll(t);
            if b.status == Status::Done {
                return;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn info_replies_land_on_their_server_with_a_ping() {
        let server_sock = UdpSocket::bind("127.0.0.1:0").unwrap();
        let SocketAddr::V4(addr) = server_sock.local_addr().unwrap() else {
            unreachable!()
        };
        let mut b = Browser {
            socket: open_socket(),
            status: Status::Pinging,
            ..Browser::default()
        };
        b.lists[0].push(Server::new(addr));
        let t0 = Instant::now();
        b.started = Some(t0 - LAN_WINDOW);
        b.poll(t0);
        answer(
            &server_sock,
            "\\hostname\\test\\clients\\2\\sv_maxclients\\8",
        );
        poll_until_done(&mut b, t0 + Duration::from_millis(30));
        assert_eq!(b.status, Status::Done);
        assert_eq!(b.rows().len(), 1);
        assert_eq!(b.list()[0].ping, Some(30));
        assert_eq!(column_text(&b.list()[0], 3), "2 (8)");
    }

    #[test]
    fn a_local_refresh_lists_whoever_answers_the_broadcast() {
        let a = UdpSocket::bind("127.0.0.1:0").unwrap();
        let b_sock = UdpSocket::bind("127.0.0.1:0").unwrap();
        let target = |s: &UdpSocket| match s.local_addr().unwrap() {
            SocketAddr::V4(v) => v,
            _ => unreachable!(),
        };
        let mut b = Browser {
            lan_targets: vec![target(&a), target(&b_sock)],
            ..Browser::default()
        };
        b.refresh();
        // Two rounds to each port.
        answer(&a, "\\hostname\\alpha\\clients\\1\\sv_maxclients\\8");
        answer(&b_sock, "\\hostname\\beta\\clients\\0\\sv_maxclients\\8");
        let mut buf = [0u8; 64];
        assert!(a.recv_from(&mut buf).is_ok() && b_sock.recv_from(&mut buf).is_ok());
        let t0 = b.started.unwrap();
        b.poll(t0 + Duration::from_millis(100));
        std::thread::sleep(Duration::from_millis(20));
        b.poll(t0 + Duration::from_millis(100));
        assert_eq!(b.status, Status::Pinging, "still listening");
        poll_until_done(&mut b, t0 + LAN_WINDOW);
        b.sort = Some(1);
        assert_eq!(names(&b), ["alpha", "beta"]);
    }

    #[test]
    fn favourites_persist_in_the_server_cache() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("servercache.dat");
        let mut b = Browser::with_cache(path.clone());
        assert_eq!(b.add_favorite("Home", "10.0.0.1"), AddFavorite::Added);
        assert_eq!(
            b.add_favorite("Again", "10.0.0.1:28960"),
            AddFavorite::InList
        );
        assert_eq!(
            b.add_favorite("Bad", "10.0.0.1:port"),
            AddFavorite::BadAddress
        );
        assert_eq!(
            b.add_favorite("Other", "10.0.0.2:29661"),
            AddFavorite::Added
        );
        assert_eq!(
            std::fs::metadata(&path).unwrap().len() as usize,
            server_cache::FILE_LEN
        );
        let mut b = Browser::with_cache(path.clone());
        b.source = Source::Favorites;
        // Unanswered favourites still list, by address.
        let rows: Vec<String> = b.rows().iter().map(|s| column_text(s, 1)).collect();
        assert_eq!(rows, ["10.0.0.1:28960", "10.0.0.2:29661"]);
        assert_eq!(b.list()[0].name, "Home");
        b.remove_favorite("10.0.0.1:28960".parse().unwrap());
        let b = Browser::with_cache(path);
        assert_eq!(b.lists[2].len(), 1);
        assert_eq!(b.lists[2][0].name, "Other");
    }

    /// The LAN scan against real servers: run ours and the retail one on
    /// 29661 and 29662 first (docs/research/cod11-front-end.md, "Local
    /// servers"), then `cargo test -p vcod lan_scan_live -- --ignored
    /// --nocapture`.
    #[test]
    #[ignore]
    fn lan_scan_live() {
        let mut b = Browser {
            lan_targets: [29661, 29662]
                .map(|p| SocketAddrV4::new(Ipv4Addr::BROADCAST, p))
                .to_vec(),
            ..Browser::default()
        };
        b.refresh();
        let t0 = Instant::now();
        while b.status == Status::Pinging && t0.elapsed() < Duration::from_secs(3) {
            b.poll(Instant::now());
            std::thread::sleep(Duration::from_millis(10));
        }
        for s in b.rows() {
            println!("{} {:?} ping {:?}", s.addr, s.info, s.ping);
        }
        assert!(!b.rows().is_empty());
    }
}
