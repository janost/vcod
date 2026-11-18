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
    GETINFO, LAN_PORTS, LAN_ROUNDS, Master, ServerInfo, getservers, master_send_allowed,
    parse_address, parse_servers_response,
};
use vcod_common::net::server_cache::{self, Entry, Layout};
use vcod_common::net::wire::Transport;

/// `MAX_PINGREQUESTS`: the ping slots (CoDMP.exe 0x15ca660, 16 of
/// `0x41c` bytes).
const MAX_IN_FLIGHT: usize = 16;
/// `cl_maxPing`'s default and floor (CoDMP.exe 0x41228d, 0x413a7d), ms.
pub const MAX_PING_DEFAULT: u32 = 800;
const MAX_PING_FLOOR: u32 = 100;
/// How long the master gets before a refresh gives up on it. vcod's own:
/// retail waits for as long as the refresh runs.
const MASTER_TIMEOUT: Duration = Duration::from_secs(5);
/// `MAX_OTHER_SERVERS`, the Local list's cap.
const MAX_LOCAL: usize = 0x80;

/// The Internet list's cap for a master protocol: 1.1's 2048 entries and
/// the `0x800` addresses its overflow block holds (`CL_ServersResponsePacket`
/// 0x4107b0), else the 20000 of 1.5, UO and CoD2 (front-end doc 8).
fn max_internet(protocol: u32) -> usize {
    if protocol == 1 { 2048 + 0x800 } else { 20000 }
}

/// The master an Internet refresh asks and the one protocol it asks for:
/// the install's, never another the client could also join.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MasterQuery {
    pub master: Master,
    pub protocol: u32,
}

impl MasterQuery {
    pub fn of(t: &Transport) -> MasterQuery {
        MasterQuery {
            master: t.master(),
            protocol: t.version,
        }
    }
}

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

    /// The previous source on a right click (0x40009b90).
    pub fn prev(self) -> Source {
        self.next().next()
    }
}

/// The filter popup's four switches and the game type filter
/// (`UI_BuildServerDisplayList`, ui_mp_x86.dll 0x4000b800). All on and no
/// game type by default.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Filter {
    pub show_full: bool,
    pub show_empty: bool,
    pub show_password: bool,
    pub show_no_password: bool,
    /// The `ui_joinGametype` entry's script name; `None` for "All".
    pub game_type: Option<String>,
}

impl Default for Filter {
    fn default() -> Self {
        Filter {
            show_full: true,
            show_empty: true,
            show_password: true,
            show_no_password: true,
            game_type: None,
        }
    }
}

impl Filter {
    fn passes(&self, info: &ServerInfo) -> bool {
        (self.show_empty || info.clients != 0)
            && (self.show_full || info.clients != info.max_clients)
            && (self.show_password || !info.password)
            && (self.show_no_password || info.password)
            // `Q_stricmp` (0x4000bac5): a `^7dm` matches no filter.
            && self
                .game_type
                .as_ref()
                .is_none_or(|g| g.eq_ignore_ascii_case(&info.gametype))
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
    pub(super) fn new(addr: SocketAddrV4) -> Server {
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
    /// The master's resolved address, waiting on the rate limit
    /// ([`master_send_allowed`]) until its `getservers` goes.
    master_due: Option<SocketAddr>,
    /// The address the `getservers` went to; its answers are taken.
    master: Option<SocketAddr>,
    /// The install's master and protocol; `None` asks no master.
    pub master_query: Option<MasterQuery>,
    started: Option<Instant>,
    pub source: Source,
    /// Indexed by [`Source`].
    pub(super) lists: [Vec<Server>; 3],
    pub status: Status,
    /// `ServerSort` column, `None` for arrival order.
    pub sort: Option<usize>,
    /// What each list column shows, set off the loaded `joinserver.menu`.
    pub columns: &'static [Column],
    pub filter: Filter,
    /// Where a Local refresh sends its broadcasts; tests empty it.
    pub(super) lan_targets: Vec<SocketAddrV4>,
    /// The game's server cache and its layout; `None` keeps favourites in
    /// memory only, as does a file that is not a cache of that layout.
    cache: Option<(PathBuf, Layout)>,
    /// The cached favourites as read, by address: a save writes their
    /// bytes back with what this run learnt.
    cached: std::collections::HashMap<SocketAddrV4, Entry>,
    /// Favourites vcod cannot address (IPX, loopback), written back as read.
    foreign: Vec<Entry>,
    /// When each source's last refresh started, local time
    /// (`ui_lastServerRefresh_<source>`).
    pub last_refresh: [Option<jiff::civil::DateTime>; 3],
    /// `cl_maxPing`, ms: how long a ping waits for its reply.
    pub max_ping: u32,
}

impl Default for Browser {
    fn default() -> Self {
        Browser {
            socket: None,
            resolve: None,
            master_due: None,
            master: None,
            master_query: Some(MasterQuery::of(
                &vcod_common::net::protocol::PROTOCOL_V1.transport,
            )),
            started: None,
            source: Source::Local,
            lists: Default::default(),
            status: Status::Idle,
            sort: None,
            columns: &COLUMNS_V1,
            filter: Filter::default(),
            lan_targets: LAN_PORTS
                .iter()
                .map(|&p| SocketAddrV4::new(Ipv4Addr::BROADCAST, p))
                .collect(),
            cache: None,
            cached: Default::default(),
            foreign: Vec::new(),
            last_refresh: [None; 3],
            max_ping: MAX_PING_DEFAULT,
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
    /// A browser whose favourites live in `cache`, a server cache of
    /// `layout`, read now as `LAN_LoadCachedServers` reads it at UI start.
    /// A file that is not one, or that cannot be read, is never written.
    pub fn with_cache(cache: PathBuf, layout: Layout) -> Browser {
        let mut b = Browser::default();
        let entries = match std::fs::read(&cache) {
            Ok(bytes) => server_cache::read_favorites(layout, &bytes),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Some(Vec::new()),
            Err(_) => None,
        };
        let Some(entries) = entries else {
            log::warn!(
                "browser: {} is not a {layout:?} server cache; favourites stay in memory",
                cache.display()
            );
            return b;
        };
        for e in entries {
            match e.addr() {
                Some(addr) => {
                    b.lists[Source::Favorites as usize].push(Server {
                        name: e.hostname(),
                        ..Server::new(addr)
                    });
                    b.cached.insert(addr, e);
                }
                None => b.foreign.push(e),
            }
        }
        b.cache = Some((cache, layout));
        b
    }

    /// How many favourites the cache can hold and still load.
    fn max_favorites(&self) -> usize {
        self.cache
            .as_ref()
            .map_or(server_cache::MAX_FAVORITES, |(_, l)| l.max_favorites())
    }

    fn list(&self) -> &Vec<Server> {
        &self.lists[self.source as usize]
    }

    fn list_mut(&mut self) -> &mut Vec<Server> {
        &mut self.lists[self.source as usize]
    }

    /// A master's addresses not listed yet, up to [`max_internet`].
    fn add_master_servers(&mut self, addrs: impl IntoIterator<Item = SocketAddrV4>) {
        let cap = max_internet(self.master_query.map_or(1, |q| q.protocol));
        for addr in addrs {
            if self.list().len() >= cap {
                break;
            }
            if !self.list().iter().any(|s| s.addr == addr) {
                self.list_mut().push(Server::new(addr));
            }
        }
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
        self.stamp();
        let Some(socket) = open_socket() else {
            self.status = Status::Failed;
            return;
        };
        let now = Instant::now();
        self.resolve = None;
        self.master_due = None;
        self.master = None;
        self.started = Some(now);
        match (self.source, self.master_query) {
            (Source::Internet, None) => {
                log::info!("browser: no master query for this game; use Local or a favourite");
                self.list_mut().clear();
                self.status = Status::Failed;
                return;
            }
            (Source::Internet, Some(q)) => {
                let (tx, rx) = mpsc::channel();
                std::thread::spawn(move || {
                    let addr = (q.master.host, q.master.port)
                        .to_socket_addrs()
                        .ok()
                        .and_then(|mut a| a.find(SocketAddr::is_ipv4));
                    let _ = tx.send(addr);
                });
                self.list_mut().clear();
                self.resolve = Some(rx);
                self.status = Status::Master;
            }
            (Source::Local, _) => {
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
            (Source::Favorites, _) => {
                self.reset_pings();
                self.status = Status::Pinging;
            }
        }
        self.socket = Some(socket);
    }

    /// `UI_StartServerRefresh` (ui_mp_x86.dll 0x4000ea90) dates the source
    /// before it asks anyone, on a full and a quick refresh alike.
    fn stamp(&mut self) {
        self.last_refresh[self.source as usize] = Some(jiff::Zoned::now().datetime());
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
        self.stamp();
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
        self.master_due = None;
        if matches!(self.status, Status::Master | Status::Pinging) {
            self.finish();
        }
    }

    /// `UI_StopServerRefresh` (ui_mp_x86.dll 0x4000e9b0): the counts go
    /// to the console, not the browser.
    fn finish(&mut self) {
        let rows = self.rows();
        let listed = rows.len();
        let players: u32 = rows
            .iter()
            .filter_map(|s| s.info.as_ref())
            .map(|i| i.clients)
            .sum();
        let hidden = self.list().len().saturating_sub(listed);
        crate::console::log::print(&format!(
            "{listed} servers listed in browser with {players} players."
        ));
        if hidden > 0 {
            crate::console::log::print(&format!(
                "{hidden} servers not listed (filtered out by game browser settings)"
            ));
        }
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
            match addr {
                Some(addr) => self.master_due = Some(addr),
                None => {
                    let host = self.master_query.map_or("", |q| q.master.host);
                    log::warn!("browser: cannot resolve {host}");
                    self.status = Status::Failed;
                    self.socket = None;
                }
            }
        }
        // One `getservers` per refresh, sent once the master's host is out
        // of its last second; a list in several packets is not asked again.
        if let (Some(addr), Some(socket), Some(q)) =
            (self.master_due, &self.socket, self.master_query)
            && master_send_allowed(addr.ip())
        {
            log::info!(
                "browser: requesting protocol {} servers from {addr}",
                q.protocol
            );
            let _ = socket.send_to(&build_oob(&getservers(q.protocol)), addr);
            self.master_due = None;
            self.master = Some(addr);
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
                    self.add_master_servers(parse_servers_response(rest));
                    self.status = Status::Pinging;
                }
                "infoResponse" => {
                    // Latin-1, as the font indexes glyphs by byte.
                    let text = vcod_common::text::latin1(rest);
                    let info = ServerInfo::parse(&text);
                    // The master lists by what a server told it; one that
                    // answers with another protocol is dropped.
                    if self.source == Source::Internet
                        && let Some(q) = self.master_query
                        && info.protocol != 0
                        && info.protocol != q.protocol
                    {
                        self.list_mut().retain(|s| s.addr != from4);
                        continue;
                    }
                    let local = self.source == Source::Local;
                    let list = self.list_mut();
                    // An answer no ping waits on is a broadcast's: on Local,
                    // `CL_ServerInfoPacket` adds its address unanswered, to
                    // be pinged as any other (CoDMP.exe, front-end doc 7).
                    let pinged = list
                        .iter()
                        .any(|s| s.addr == from4 && s.sent.is_some() && s.info.is_none());
                    if local && !pinged {
                        if !list.iter().any(|s| s.addr == from4) && list.len() < MAX_LOCAL {
                            list.push(Server::new(from4));
                        }
                        continue;
                    }
                    // The round trip plus one, as `CL_ServerInfoPacket`
                    // stores it (0x412d4a).
                    if let Some(s) = list.iter_mut().find(|s| s.addr == from4)
                        && let Some(sent) = s.sent
                        && s.info.is_none()
                        && !s.timed_out
                    {
                        s.name.clone_from(&info.hostname);
                        s.info = Some(info);
                        s.ping = Some((now - sent).as_millis() as u32 + 1);
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
        // `CL_UpdateVisiblePings` (0x413de0), once a frame: unpinged
        // servers into the free slots, then each slot's reply or timeout
        // (`CL_GetPing`: past `cl_maxPing` the server keeps ping 0 and
        // stays hidden, 0x413a8d). It ends the refresh once a call finds
        // nothing to do; Local waits while its list is empty
        // (`UI_DoServerRefresh` 0x4000ea00).
        let socket = self.socket.take().unwrap();
        let waiting = |s: &Server| s.sent.is_some() && s.info.is_none() && !s.timed_out;
        let busy = self.list().iter().filter(|s| waiting(s)).count();
        let free = MAX_IN_FLIGHT.saturating_sub(busy);
        let sent = (self.list_mut().iter_mut())
            .filter(|s| s.sent.is_none())
            .take(free)
            .map(|s| {
                let _ = socket.send_to(&build_oob(GETINFO), s.addr);
                s.sent = Some(now);
            })
            .count();
        let acted = busy + sent > 0;
        let max_ping = Duration::from_millis(self.max_ping.max(MAX_PING_FLOOR).into());
        for s in self.list_mut().iter_mut().filter(|s| waiting(s)) {
            if now - s.sent.unwrap() >= max_ping {
                s.timed_out = true;
            }
        }
        self.socket = Some(socket);
        let wait = self.source == Source::Local && self.list().is_empty();
        if !acted && !wait {
            self.finish();
        }
    }

    /// The servers the list shows, in display order: those that answered
    /// (every favourite, answered or not) that pass the filter.
    /// The servers the filter keeps, in the list's order.
    fn filtered(&self) -> impl Iterator<Item = &Server> {
        let all = self.source == Source::Favorites;
        let blank = ServerInfo::default();
        self.list().iter().filter(move |s| {
            (all || s.info.is_some()) && self.filter.passes(s.info.as_ref().unwrap_or(&blank))
        })
    }

    /// `rows().len()`, without the sort.
    pub fn row_count(&self) -> usize {
        self.filtered().count()
    }

    /// The row for `addr`, without the sort.
    pub fn row(&self, addr: SocketAddrV4) -> Option<&Server> {
        self.filtered().find(|s| s.addr == addr)
    }

    pub fn rows(&self) -> Vec<&Server> {
        let blank = ServerInfo::default();
        let mut rows: Vec<&Server> = self.filtered().collect();
        let Some(&col) = self.sort.and_then(|c| self.columns.get(c)) else {
            return rows;
        };
        fn of<'a>(s: &'a Server, blank: &'a ServerInfo) -> &'a ServerInfo {
            s.info.as_ref().unwrap_or(blank)
        }
        // A string key is built once a row, not twice a comparison; both
        // sorts are stable, so ties keep the list's order either way.
        match col {
            Column::Name => {
                rows.sort_by_cached_key(|s| strip(&of(s, &blank).hostname).to_lowercase())
            }
            Column::Map => rows.sort_by_cached_key(|s| of(s, &blank).mapname.to_lowercase()),
            Column::GameType => rows.sort_by_cached_key(|s| strip(&of(s, &blank).gametype)),
            _ => rows.sort_by(|a, b| {
                let (ia, ib) = (of(a, &blank), of(b, &blank));
                match col {
                    Column::Players => ib.clients.cmp(&ia.clients),
                    Column::Ping => a.ping.cmp(&b.ping),
                    // 1.5's `LAN_CompareServers` (CoDMP.exe 0x419c00) keys 1, 6, 7;
                    // an unknown `hw` (0) after every known one (0x419c4d).
                    Column::Hardware => {
                        let (ha, hb) = (ia.hardware as u8, ib.hardware as u8);
                        (ha == 0).cmp(&(hb == 0)).then(ha.cmp(&hb))
                    }
                    Column::Hardware2 => ia.hardware.cmp(&ib.hardware),
                    Column::PunkBuster | Column::PunkBusterText => {
                        ia.punkbuster.cmp(&ib.punkbuster)
                    }
                    Column::Voice => ia.voice.cmp(&ib.voice),
                    Column::Pure => ia.pure.cmp(&ib.pure),
                    Column::Mod => ia.modded.cmp(&ib.modded),
                    Column::Name | Column::Map | Column::GameType | Column::Password => {
                        std::cmp::Ordering::Equal
                    }
                }
            }),
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
        let max = self.max_favorites();
        let favs = &mut self.lists[Source::Favorites as usize];
        if favs.len() + self.foreign.len() >= max {
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
        self.cached.remove(&addr);
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

    /// Writes the favourites into the server cache, keeping the rest of
    /// the file. A file that changed into something not a cache, or cannot
    /// be read, is left alone; the new one goes in whole or not at all.
    fn save(&self) {
        let Some((path, layout)) = &self.cache else {
            return;
        };
        let mut entries: Vec<Entry> = self.lists[Source::Favorites as usize]
            .iter()
            .map(|s| {
                let mut e = match self.cached.get(&s.addr) {
                    Some(e) => e.clone(),
                    None => Entry::new(*layout, s.addr, &s.name),
                };
                if let Some(info) = &s.info {
                    e.set_info(info, s.ping.map_or(-1, |p| p as i32));
                }
                e
            })
            .collect();
        entries.extend(self.foreign.iter().cloned());
        let old = match std::fs::read(path) {
            Ok(bytes) => Some(bytes),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(e) => {
                log::warn!(
                    "browser: cannot read {}: {e}; not writing it",
                    path.display()
                );
                return;
            }
        };
        let Some(bytes) = server_cache::write_favorites(*layout, old.as_deref(), &entries) else {
            log::warn!(
                "browser: {} is no longer a {layout:?} server cache; not writing it",
                path.display()
            );
            return;
        };
        let tmp = path.with_extension("dat.tmp");
        if let Err(e) = std::fs::write(&tmp, bytes).and_then(|()| std::fs::rename(&tmp, path)) {
            log::warn!("browser: cannot write {}: {e}", path.display());
            let _ = std::fs::remove_file(&tmp);
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

/// What a server list column shows. `ServerSort` numbers the columns the
/// same way (research doc "Server list columns").
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Column {
    Password,
    /// 1.5: the `hw` icon.
    Hardware,
    Name,
    Map,
    Players,
    GameType,
    /// 1.5: the PunkBuster logo on a `pb` server.
    PunkBuster,
    /// 1.5: `X` on a `mod` server.
    Mod,
    Ping,
    /// CoD2: the `hw` icon out of CoD2's own materials.
    Hardware2,
    /// CoD2: `X` on a `voice` server.
    Voice,
    /// CoD2: `X` on a `pure` server.
    Pure,
    /// CoD2: `X` on a `pb` server, no logo.
    PunkBusterText,
}

/// 1.1's `joinserver.menu`, six columns (ui_mp_x86.dll 0x4000c8ad).
pub const COLUMNS_V1: [Column; 6] = [
    Column::Password,
    Column::Name,
    Column::Map,
    Column::Players,
    Column::GameType,
    Column::Ping,
];

/// 1.5's, nine (`paka.pk3`; ui_mp_x86.dll 1.5 `UI_FeederItemText` 0x4000ce00,
/// jump table 0x4000d22c).
pub const COLUMNS_V5: [Column; 9] = [
    Column::Password,
    Column::Hardware,
    Column::Name,
    Column::Map,
    Column::Players,
    Column::GameType,
    Column::PunkBuster,
    Column::Mod,
    Column::Ping,
];

/// CoD2 1.3's, eleven (`UI_FeederItemText`, Mac 0x152e8a: the `hw` icon,
/// then `X` for `voice`, `pure`, `mod` and `pb`).
pub const COLUMNS_V118: [Column; 11] = [
    Column::Password,
    Column::Hardware2,
    Column::Name,
    Column::Map,
    Column::Players,
    Column::GameType,
    Column::Voice,
    Column::Pure,
    Column::Mod,
    Column::PunkBusterText,
    Column::Ping,
];

/// The layout a list box of `count` columns is: CoD2's eleven, 1.5's nine,
/// else 1.1's.
pub fn columns_for(count: usize) -> &'static [Column] {
    match count {
        11 => &COLUMNS_V118,
        9 => &COLUMNS_V5,
        _ => &COLUMNS_V1,
    }
}

/// `hw` 0..5 to the icon the 1.5 UI registers for it (0x4000ddce on).
const HARDWARE_ICONS: [&str; 6] = [
    "ui_mp/assets/server_hardware_unknown",
    "ui_mp/assets/server_hardware_linux_dedicated",
    "ui_mp/assets/server_hardware_win_dedicated",
    "ui_mp/assets/server_hardware_mac_dedicated",
    "ui_mp/assets/server_hardware_win_listen",
    "ui_mp/assets/server_hardware_mac_listen",
];

/// `hw` 0..6 to the material CoD2's `UI_Init` registers for it (Mac 0x1506ba;
/// 4 registers nothing).
const HARDWARE_ICONS_V118: [&str; 7] = [
    "server_hardware_unknown",
    "server_hardware_linux_dedicated",
    "server_hardware_win_dedicated",
    "server_hardware_mac_dedicated",
    "",
    "server_hardware_win_listen",
    "server_hardware_mac_listen",
];

/// One list cell: an icon, or text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Cell {
    Text(String),
    Image(&'static str),
}

/// A list column's cell, as the UI module's server feeder builds it. A
/// favourite that has not answered reads as zeros with its address.
pub fn column_cell(s: &Server, col: Column) -> Cell {
    let blank = ServerInfo::default();
    let info = s.info.as_ref().unwrap_or(&blank);
    let ping = s.ping.unwrap_or(0);
    let text = |t: &str| Cell::Text(t.to_string());
    match col {
        Column::Password if info.password => text("X"),
        Column::Hardware => match usize::try_from(info.hardware) {
            Ok(i) if i < HARDWARE_ICONS.len() => Cell::Image(HARDWARE_ICONS[i]),
            _ => text(""),
        },
        Column::Name if ping > 0 => text(&info.hostname),
        Column::Name => Cell::Text(s.addr.to_string()),
        Column::Map => text(&info.mapname),
        Column::Players => Cell::Text(format!("{} ({})", info.clients, info.max_clients)),
        Column::GameType if info.gametype.is_empty() => text("?"),
        Column::GameType => text(&info.gametype),
        Column::PunkBuster if info.punkbuster => Cell::Image("ui_mp/assets/punkbusterlogo"),
        Column::Mod if info.modded => text("X"),
        Column::Ping if ping > 0 => Cell::Text(ping.to_string()),
        Column::Ping => text("..."),
        Column::Hardware2 => match usize::try_from(info.hardware) {
            Ok(i) if i < HARDWARE_ICONS_V118.len() && !HARDWARE_ICONS_V118[i].is_empty() => {
                Cell::Image(HARDWARE_ICONS_V118[i])
            }
            _ => text(""),
        },
        Column::Voice if info.voice => text("X"),
        Column::Pure if info.pure => text("X"),
        Column::PunkBusterText if info.punkbuster => text("X"),
        Column::Password
        | Column::PunkBuster
        | Column::Mod
        | Column::Voice
        | Column::Pure
        | Column::PunkBusterText => text(""),
    }
}

/// [`column_cell`] of 1.1's column `col` as text.
#[cfg(test)]
pub fn column_text(s: &Server, col: usize) -> String {
    match COLUMNS_V1.get(col).map(|&c| column_cell(s, c)) {
        Some(Cell::Text(t)) => t,
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
                ..Default::default()
            }),
            ping: Some(ping),
            ..Server::new("203.0.113.4:28960".parse().unwrap())
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

    #[test]
    fn the_game_type_filter_matches_the_key_without_case_or_colour_stripping() {
        let mut b = Browser::default();
        let mut tdm = server("tdm", 3, 10);
        tdm.info.as_mut().unwrap().gametype = "TDM".into();
        let mut dm = server("dm", 3, 10);
        dm.info.as_mut().unwrap().gametype = "dm".into();
        b.lists[0] = vec![server("coloured", 3, 10), dm, tdm];
        b.filter.game_type = Some("tdm".into());
        assert_eq!(names(&b), ["tdm"]);
        b.filter.game_type = Some("dm".into());
        assert_eq!(names(&b), ["dm"]);
    }

    /// A master that keeps answering fills the Internet list to the size
    /// retail's arrays hold and no further.
    #[test]
    fn the_internet_list_stops_at_retails_size() {
        let addrs = |from: u32, n: u32| {
            (from..from + n).map(|i| SocketAddrV4::new(Ipv4Addr::from(0x0a00_0000 + i), 28960))
        };
        for (protocol, cap) in [(1, 4096), (6, 20000), (118, 20000)] {
            let mut b = Browser {
                source: Source::Internet,
                master_query: Some(MasterQuery {
                    master: vcod_common::net::master::COD1_MASTER,
                    protocol,
                }),
                ..Browser::default()
            };
            b.add_master_servers(addrs(0, 10));
            b.add_master_servers(addrs(5, 10));
            assert_eq!(b.list().len(), 15);
            for k in 0..100 {
                b.add_master_servers(addrs(k * 256, 256));
            }
            assert_eq!(b.list().len(), cap);
        }
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
        b.poll(t0);
        answer(
            &server_sock,
            "\\hostname\\test\\clients\\2\\sv_maxclients\\8",
        );
        poll_until_done(&mut b, t0 + Duration::from_millis(30));
        assert_eq!(b.status, Status::Done);
        assert_eq!(b.rows().len(), 1);
        // The round trip plus one.
        assert_eq!(b.list()[0].ping, Some(31));
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
        let t0 = b.started.unwrap();
        // With nobody answering yet the refresh waits, however long.
        b.poll(t0 + Duration::from_secs(10));
        assert_eq!(b.status, Status::Pinging);
        // Two rounds to each port; a broadcast's answer only lists the
        // address.
        answer(&a, "\\hostname\\stale\\clients\\1\\sv_maxclients\\8");
        answer(&b_sock, "\\hostname\\stale\\clients\\0\\sv_maxclients\\8");
        let mut buf = [0u8; 64];
        assert!(a.recv_from(&mut buf).is_ok() && b_sock.recv_from(&mut buf).is_ok());
        std::thread::sleep(Duration::from_millis(20));
        let t1 = t0 + Duration::from_millis(100);
        b.poll(t1);
        assert_eq!(b.list().len(), 2);
        assert!(b.rows().is_empty());
        // Then each is pinged on its own, which brings its info and ping.
        answer(&a, "\\hostname\\alpha\\clients\\1\\sv_maxclients\\8");
        answer(&b_sock, "\\hostname\\beta\\clients\\0\\sv_maxclients\\8");
        poll_until_done(&mut b, t1 + Duration::from_millis(40));
        assert_eq!(b.status, Status::Done);
        b.sort = Some(1);
        assert_eq!(names(&b), ["alpha", "beta"]);
        assert!(b.list().iter().all(|s| s.ping == Some(41)));
    }

    /// Sixteen pings at once; one unanswered past `cl_maxPing` keeps no
    /// ping and stays off the list, and the refresh ends.
    #[test]
    fn sixteen_pings_fly_at_once_and_a_silent_server_times_out() {
        let mut b = Browser {
            socket: open_socket(),
            status: Status::Pinging,
            source: Source::Favorites,
            ..Browser::default()
        };
        for i in 0..20u16 {
            b.lists[2].push(Server::new(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 9 + i)));
        }
        let t0 = Instant::now();
        b.poll(t0);
        assert_eq!(b.lists[2].iter().filter(|s| s.sent.is_some()).count(), 16);
        b.poll(t0 + Duration::from_millis(799));
        assert_eq!(b.lists[2].iter().filter(|s| s.timed_out).count(), 0);
        b.poll(t0 + Duration::from_millis(800));
        assert_eq!(b.lists[2].iter().filter(|s| s.timed_out).count(), 16);
        let t1 = t0 + Duration::from_millis(801);
        b.poll(t1);
        assert_eq!(b.lists[2].iter().filter(|s| s.sent.is_some()).count(), 20);
        b.poll(t1 + Duration::from_millis(800));
        b.poll(t1 + Duration::from_millis(801));
        assert_eq!(b.status, Status::Done);
        assert!(b.lists[2].iter().all(|s| s.ping.is_none()));
    }

    #[test]
    fn favourites_persist_in_the_server_cache() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("servercache.dat");
        let mut b = Browser::with_cache(path.clone(), Layout::Cod11);
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
            Layout::Cod11.file_len()
        );
        let mut b = Browser::with_cache(path.clone(), Layout::Cod11);
        b.source = Source::Favorites;
        // Unanswered favourites still list, by address.
        let rows: Vec<String> = b.rows().iter().map(|s| column_text(s, 1)).collect();
        assert_eq!(rows, ["10.0.0.1:28960", "10.0.0.2:29661"]);
        assert_eq!(b.list()[0].name, "Home");
        b.remove_favorite("10.0.0.1:28960".parse().unwrap());
        let b = Browser::with_cache(path, Layout::Cod11);
        assert_eq!(b.lists[2].len(), 1);
        assert_eq!(b.lists[2][0].name, "Other");
    }

    /// Each game's own cache: 1.5's and UO's keep a favourite in their
    /// shape and name, and a file that is not that game's cache is never
    /// touched.
    #[test]
    fn favourites_keep_each_games_cache_and_spare_a_foreign_file() {
        let dir = tempfile::tempdir().unwrap();
        for layout in [Layout::Cod15, Layout::Uo, Layout::Cod2] {
            let path = dir.path().join(layout.file_name());
            let _ = std::fs::remove_file(&path);
            let mut b = Browser::with_cache(path.clone(), layout);
            assert_eq!(b.add_favorite("Home", "10.0.0.1"), AddFavorite::Added);
            assert_eq!(
                std::fs::metadata(&path).unwrap().len() as usize,
                layout.file_len()
            );
            let b = Browser::with_cache(path.clone(), layout);
            assert_eq!(b.lists[2][0].name, "Home");
        }
        // UO's file read as 1.5's: not a 1.5 cache, so left as it was.
        let uo = std::fs::read(dir.path().join(Layout::Uo.file_name())).unwrap();
        let path = dir.path().join("other.dat");
        std::fs::write(&path, &uo).unwrap();
        let mut b = Browser::with_cache(path.clone(), Layout::Cod15);
        assert!(b.lists[2].is_empty());
        assert_eq!(b.add_favorite("Away", "10.0.0.2"), AddFavorite::Added);
        assert_eq!(std::fs::read(&path).unwrap(), uo);
        // Nor is a short file truncated or replaced.
        std::fs::write(&path, b"not a cache").unwrap();
        let mut b = Browser::with_cache(path.clone(), Layout::Cod11);
        assert_eq!(b.add_favorite("Away", "10.0.0.2"), AddFavorite::Added);
        assert_eq!(std::fs::read(&path).unwrap(), b"not a cache");
    }

    /// With no master an Internet refresh asks nobody and ends at once.
    #[test]
    fn no_master_asks_no_master() {
        let mut b = Browser {
            master_query: None,
            source: Source::Internet,
            ..Browser::default()
        };
        b.refresh();
        assert_eq!(b.status, Status::Failed);
        assert!(b.resolve.is_none() && b.socket.is_none());
    }

    /// The Internet list against cod.pm's live master: one `getservers 118`
    /// on the CoD2 port, then a `getinfo` to every server it lists. Run with
    /// `cargo test -p vcod --bins master_list_live -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn master_list_live() {
        let mut b = Browser {
            master_query: Some(MasterQuery::of(&vcod_cod2::net::PROTOCOL_118.transport)),
            source: Source::Internet,
            ..Browser::default()
        };
        b.refresh();
        let t0 = Instant::now();
        while matches!(b.status, Status::Master | Status::Pinging)
            && t0.elapsed() < Duration::from_secs(60)
        {
            b.poll(Instant::now());
            std::thread::sleep(Duration::from_millis(10));
        }
        let rows = b.rows();
        for s in &rows {
            let i = s.info.as_ref().unwrap();
            println!(
                "{} {:?} {} {} ping {:?}",
                s.addr, i.hostname, i.mapname, i.protocol, s.ping
            );
        }
        println!("listed {} answered {}", b.list().len(), rows.len());
        assert!(
            rows.iter()
                .all(|s| s.info.as_ref().unwrap().protocol == 118)
        );
        assert!(!rows.is_empty());
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

    #[test]
    fn the_15_layout_shows_and_sorts_hardware_punkbuster_and_mod() {
        assert_eq!(columns_for(6), &COLUMNS_V1[..]);
        assert_eq!(columns_for(9), &COLUMNS_V5[..]);
        let mut a = server("a", 1, 40);
        let info = a.info.as_mut().unwrap();
        (info.punkbuster, info.hardware, info.modded) = (true, 2, true);
        let b = server("b", 2, 20);
        assert_eq!(
            column_cell(&a, Column::Hardware),
            Cell::Image("ui_mp/assets/server_hardware_win_dedicated")
        );
        assert_eq!(
            column_cell(&a, Column::PunkBuster),
            Cell::Image("ui_mp/assets/punkbusterlogo")
        );
        assert_eq!(column_cell(&a, Column::Mod), Cell::Text("X".into()));
        assert_eq!(
            column_cell(&b, Column::PunkBuster),
            Cell::Text(String::new())
        );
        let mut br = Browser {
            columns: &COLUMNS_V5,
            ..Browser::default()
        };
        br.lists[0] = vec![a, b];
        // `ServerSort 6` is the PunkBuster column on 1.5's menu, 2 the name.
        br.sort = Some(6);
        assert_eq!(names(&br), ["b", "a"]);
        br.sort = Some(2);
        assert_eq!(names(&br), ["a", "b"]);
        br.sort = Some(8);
        assert_eq!(names(&br), ["b", "a"]);
        // `ServerSort 1`: an unknown `hw` after every known one.
        for (s, hw) in br.lists[0].iter_mut().zip([0, 3]) {
            s.info.as_mut().unwrap().hardware = hw;
        }
        br.sort = Some(1);
        assert_eq!(names(&br), ["b", "a"]);
    }
}
