//! 1.7's server browser behind `pc_join_unranked` (cod4-handshake.md 12):
//! three lists as `ui_netSource` numbers them, Local (a `getinfo`
//! broadcast to 28960..28963), Internet (the master's `getserversResponse`)
//! and Favorites (`servercache.dat`); every listed server pinged with
//! `getinfo xxx` a few a frame; `getstatus` for the server info popup.
//! Nothing blocks a frame: the master's name resolves on a helper thread
//! and the socket is non-blocking.

use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4, ToSocketAddrs, UdpSocket};
use std::path::PathBuf;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use vcod_cod4::net::browser::{
    MAX_FAVORITES, MAX_GLOBAL, MAX_LOCAL, NO_PING, PROTOCOL, Record, ServerCache, ServerInfo,
    Status, getservers, info_protocol, parse_servers_response,
};
use vcod_common::net::connectionless::{build_oob, parse_oob};
use vcod_common::net::master::{GETINFO, GETSTATUS, LAN_PORTS, LAN_ROUNDS, master_send_allowed};

/// `CL_UpdateVisiblePings` (0x4703c0) stops once nothing was sent and
/// 2000 ms passed since the newest ping went out.
const PING_WAIT: Duration = Duration::from_millis(2000);
/// How long the master gets before an Internet refresh gives up on it.
const MASTER_TIMEOUT: Duration = Duration::from_secs(5);
/// How long the status popup's `getstatus` waits before it is sent again.
const STATUS_RESEND: Duration = Duration::from_secs(1);

/// `ui_netSource`'s values.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    Local = 0,
    Internet = 1,
    Favorites = 2,
}

impl Source {
    pub fn of(v: i32) -> Source {
        match v {
            1 => Source::Internet,
            2 => Source::Favorites,
            _ => Source::Local,
        }
    }
}

/// A listed server: its cached row, and when its `getinfo` went.
#[derive(Clone, Debug)]
pub struct Server {
    pub rec: Record,
    sent: Option<Instant>,
}

impl Server {
    fn new(addr: SocketAddrV4) -> Server {
        Server {
            rec: Record::new(addr),
            sent: None,
        }
    }

    pub fn info(&self) -> &ServerInfo {
        &self.rec.info
    }
}

/// What `addFavorite` and `createFavorite` report
/// (`ui_favorite_message`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AddFavorite {
    Added,
    InList,
    Full,
    BadAddress,
}

pub struct Browser {
    socket: Option<UdpSocket>,
    resolve: Option<mpsc::Receiver<Option<SocketAddr>>>,
    /// The master's address, waiting on the rate limit until its
    /// `getservers` goes, then the address its answers come from.
    master_due: Option<SocketAddr>,
    master: Option<SocketAddr>,
    /// `masterServerName` and `masterPort` as the last refresh read them.
    pub master_name: String,
    pub master_port: u16,
    pub lists: [Vec<Server>; 3],
    /// The list the pings and replies go to (`cls.pingUpdateSource`).
    pub source: Source,
    /// A refresh running, and when it started.
    pub refreshing: bool,
    started: Option<Instant>,
    /// The newest `getinfo` sent.
    last_ping: Option<Instant>,
    /// `getinfo`s a frame: `cl_maxppf`'s default (iw3mp.exe 0x46e4df).
    pub pings_per_frame: usize,
    /// Where Local sends its broadcasts; tests point it at loopback.
    pub lan_targets: Vec<SocketAddrV4>,
    /// `servercache.dat`; `None` keeps the lists in memory.
    cache: Option<PathBuf>,
    /// The status popup's server, its answer, and when it was asked.
    pub status: Option<(SocketAddrV4, Option<Status>)>,
    status_sent: Option<Instant>,
}

impl Default for Browser {
    fn default() -> Self {
        Browser {
            socket: None,
            resolve: None,
            master_due: None,
            master: None,
            master_name: vcod_common::net::master::COD4_MASTER.host.into(),
            master_port: vcod_common::net::master::COD4_MASTER.port,
            lists: Default::default(),
            source: Source::Local,
            refreshing: false,
            started: None,
            last_ping: None,
            pings_per_frame: 5,
            lan_targets: LAN_PORTS
                .iter()
                .map(|&p| SocketAddrV4::new(Ipv4Addr::BROADCAST, p))
                .collect(),
            cache: None,
            status: None,
            status_sent: None,
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
        .map_err(|e| log::warn!("cod4 browser: cannot open a socket: {e}"))
        .ok()
}

impl Browser {
    /// A browser over `cache`, read now as `LAN_LoadCachedServers` reads
    /// it at UI start: the Internet list and the favourites.
    pub fn with_cache(cache: PathBuf) -> Browser {
        let mut b = Browser::default();
        if let Some(c) = std::fs::read(&cache)
            .ok()
            .and_then(|bytes| ServerCache::read(&bytes))
        {
            let take = |v: Vec<Record>| {
                v.into_iter()
                    .filter(|r| r.adr_type == 4)
                    .map(|rec| Server { rec, sent: None })
                    .collect()
            };
            b.lists[Source::Internet as usize] = take(c.global);
            b.lists[Source::Favorites as usize] = take(c.favorites);
        }
        b.cache = Some(cache);
        b
    }

    pub fn list(&self, source: Source) -> &[Server] {
        &self.lists[source as usize]
    }

    fn socket(&mut self) -> Option<&UdpSocket> {
        if self.socket.is_none() {
            self.socket = open_socket();
        }
        self.socket.as_ref()
    }

    /// `UI_StartServerRefresh` (0x54aaf0): a full refresh marks every
    /// server of the source and forgets its pings (`LAN_ResetPings`), then
    /// Local broadcasts (`localservers`, clearing the list first), Internet
    /// asks the master (`globalservers 0 6 full empty`, the list kept and
    /// added to), and Favorites only pings. A quick refresh
    /// (`full` false) only forgets the pings.
    pub fn refresh(&mut self, source: Source, full: bool) {
        let now = Instant::now();
        self.source = source;
        self.refreshing = true;
        self.started = Some(now);
        self.last_ping = None;
        for s in &mut self.lists[source as usize] {
            s.rec.visible = full || s.rec.visible;
            (s.rec.ping, s.sent) = (NO_PING, None);
        }
        if !full {
            return;
        }
        match source {
            Source::Local => {
                self.lists[0].clear();
                let targets = self.lan_targets.clone();
                if let Some(sock) = self.socket() {
                    for _ in 0..LAN_ROUNDS {
                        for &to in &targets {
                            if let Err(e) = sock.send_to(&build_oob(GETINFO), to) {
                                log::debug!("cod4 browser: broadcast to {to}: {e}");
                            }
                        }
                    }
                }
                // The broadcast's answers are timed from the broadcast.
                self.last_ping = Some(now);
            }
            Source::Internet => {
                let (tx, rx) = mpsc::channel();
                let (host, port) = (self.master_name.clone(), self.master_port);
                std::thread::spawn(move || {
                    let addr = (host.as_str(), port)
                        .to_socket_addrs()
                        .ok()
                        .and_then(|mut a| a.find(SocketAddr::is_ipv4));
                    let _ = tx.send(addr);
                });
                self.resolve = Some(rx);
                self.master = None;
            }
            Source::Favorites => {}
        }
    }

    /// `closeJoin`: the refresh stops; the lists stay.
    pub fn stop(&mut self) {
        self.refreshing = false;
        self.resolve = None;
        self.master_due = None;
    }

    /// Reads the replies and sends the next pings; once a frame.
    pub fn pump(&mut self, now: Instant) {
        if let Some(rx) = &self.resolve
            && let Ok(addr) = rx.try_recv()
        {
            self.resolve = None;
            match addr {
                Some(a) => self.master_due = Some(a),
                None => {
                    log::warn!("cod4 browser: cannot resolve {}", self.master_name);
                    self.refreshing = false;
                }
            }
        }
        if let Some(addr) = self.master_due
            && master_send_allowed(addr.ip())
            && let Some(sock) = self.socket()
        {
            log::info!("cod4 browser: requesting protocol {PROTOCOL} servers from {addr}");
            let _ = sock.send_to(&build_oob(&getservers()), addr);
            self.master_due = None;
            self.master = Some(addr);
        }
        self.receive(now);
        self.status_resend(now);
        if !self.refreshing {
            return;
        }
        let waiting_master = self.resolve.is_some()
            || self.master_due.is_some()
            || (self.source == Source::Internet
                && self.master.is_some()
                && self.lists[1].is_empty()
                && self.started.is_some_and(|t| now - t < MASTER_TIMEOUT));
        let sent = self.send_pings(now);
        let last = self.last_ping.or(self.started);
        if sent == 0 && !waiting_master && last.is_none_or(|t| now - t >= PING_WAIT) {
            self.finish();
        }
    }

    /// `CL_UpdateVisiblePings`: up to `pings_per_frame` `getinfo xxx`s to
    /// marked servers with no ping and none in flight.
    fn send_pings(&mut self, now: Instant) -> usize {
        let src = self.source as usize;
        let n = self.pings_per_frame;
        let targets: Vec<usize> = (0..self.lists[src].len())
            .filter(|&i| {
                let s = &self.lists[src][i];
                s.rec.visible && s.rec.ping == NO_PING && s.sent.is_none()
            })
            .take(n)
            .collect();
        if targets.is_empty() {
            return 0;
        }
        let Some(sock) = self.socket() else {
            return 0;
        };
        let sock = sock.try_clone().ok();
        for &i in &targets {
            let s = &mut self.lists[src][i];
            if let Some(sock) = &sock {
                let _ = sock.send_to(&build_oob(GETINFO), s.rec.addr);
            }
            s.sent = Some(now);
        }
        self.last_ping = Some(now);
        targets.len()
    }

    /// The refresh is done; a Favorites refresh saves what the
    /// favourites answered.
    fn finish(&mut self) {
        self.refreshing = false;
        if self.source == Source::Favorites {
            self.save();
        }
    }

    fn receive(&mut self, now: Instant) {
        let Some(sock) = self.socket.take() else {
            return;
        };
        let mut buf = [0u8; 16384];
        while let Ok((n, from)) = sock.recv_from(&mut buf) {
            let SocketAddr::V4(from) = from else {
                continue;
            };
            let Some((cmd, rest)) = parse_oob(&buf[..n]) else {
                continue;
            };
            match cmd {
                "getserversResponse" if Some(SocketAddr::V4(from)) == self.master => {
                    let list = &mut self.lists[1];
                    for addr in parse_servers_response(rest) {
                        if list.len() < MAX_GLOBAL && !list.iter().any(|s| s.rec.addr == addr) {
                            list.push(Server::new(addr));
                        }
                    }
                }
                "infoResponse" => {
                    let text = vcod_common::text::latin1(rest);
                    self.info_response(from, &text, now);
                }
                "statusResponse" => {
                    if let Some((addr, st)) = &mut self.status
                        && *addr == from
                    {
                        let status = Status::parse(&vcod_common::text::latin1(rest));
                        if status.is_cod4x() {
                            for s in self.lists.iter_mut().flatten() {
                                s.rec.info.cod4x |= s.rec.addr == from;
                            }
                        }
                        *st = Some(status);
                    }
                }
                _ => {}
            }
        }
        self.socket = Some(sock);
    }

    /// `CL_ServerInfoPacket` (0x470ce0): another protocol is dropped; the
    /// server of the source being pinged takes the info and its ping; a
    /// Local answer from a server not listed is added, up to 128.
    fn info_response(&mut self, from: SocketAddrV4, text: &str, now: Instant) {
        let text = text.trim_start_matches('\n');
        if info_protocol(text) != PROTOCOL as i32 {
            log::debug!("cod4 browser: different protocol info packet from {from}");
            return;
        }
        let src = self.source as usize;
        let local = self.source == Source::Local;
        let list = &mut self.lists[src];
        if local && !list.iter().any(|s| s.rec.addr == from) && list.len() < MAX_LOCAL {
            let mut s = Server::new(from);
            s.sent = self.last_ping;
            list.push(s);
        }
        let Some(s) = list.iter_mut().find(|s| s.rec.addr == from) else {
            return;
        };
        let Some(sent) = s.sent else {
            return;
        };
        // A CoD4x mark its status gave stays.
        let cod4x = s.rec.info.cod4x;
        s.rec.info = ServerInfo::parse(text);
        s.rec.info.cod4x |= cod4x;
        s.rec.answered = true;
        s.rec.misses = 0;
        s.rec.ping = (now - sent).as_millis().clamp(0, 999) as u16;
    }

    /// `ServerStatus`: asks `addr` for its status (`getstatus`, 0x4722ef).
    pub fn ask_status(&mut self, addr: SocketAddrV4) {
        self.status = Some((addr, None));
        self.status_sent = None;
        self.status_resend(Instant::now());
    }

    fn status_resend(&mut self, now: Instant) {
        let Some((addr, None)) = self.status else {
            return;
        };
        if self.status_sent.is_some_and(|t| now - t < STATUS_RESEND) {
            return;
        }
        if let Some(sock) = self.socket() {
            let _ = sock.send_to(&build_oob(GETSTATUS), addr);
        }
        self.status_sent = Some(now);
    }

    /// `LAN_AddServer` into the favourites.
    pub fn add_favorite(&mut self, addr: Option<SocketAddrV4>, name: &str) -> AddFavorite {
        let Some(addr) = addr else {
            return AddFavorite::BadAddress;
        };
        let favs = &mut self.lists[Source::Favorites as usize];
        if favs.iter().any(|s| s.rec.addr == addr) {
            return AddFavorite::InList;
        }
        if favs.len() >= MAX_FAVORITES - 1 {
            return AddFavorite::Full;
        }
        let mut s = Server::new(addr);
        s.rec.info.hostname = name.chars().take(31).collect();
        favs.push(s);
        self.save();
        AddFavorite::Added
    }

    /// `LAN_RemoveServer` from the favourites.
    pub fn remove_favorite(&mut self, addr: SocketAddrV4) {
        self.lists[Source::Favorites as usize].retain(|s| s.rec.addr != addr);
        self.save();
    }

    /// `LAN_SaveServersToCache`'s favourites over the file's old bytes:
    /// vcod leaves the Internet block as retail last wrote it.
    pub fn save(&self) {
        let Some(path) = &self.cache else {
            return;
        };
        let mut file = std::fs::read(path)
            .ok()
            .filter(|b| ServerCache::read(b).is_some())
            .unwrap_or_else(|| vec![0; vcod_cod4::net::browser::CACHE_LEN]);
        let favorites: Vec<Record> = self.lists[2].iter().map(|s| s.rec.clone()).collect();
        ServerCache::write_favorites(&favorites, &mut file);
        if let Err(e) = std::fs::write(path, &file) {
            log::warn!("cod4 browser: cannot write {}: {e}", path.display());
        }
    }
}

/// The join menu's list boxes: the servers (`FEEDER_SERVERS`) and the
/// info popup's status (`FEEDER_SERVERSTATUS`).
pub const FEEDER_SERVERS: f32 = 2.0;
pub const FEEDER_SERVERSTATUS: f32 = 13.0;
/// The join menu's owner draws: the source, the refresh line, the game
/// type filter.
pub const UI_NETSOURCE: i32 = 220;
pub const UI_SERVERREFRESHDATE: i32 = 247;
pub const UI_JOINGAMETYPE: i32 = 253;
/// The menu `JoinGame` opens.
pub const JOIN_MENU: &str = "pc_join_unranked";

/// The UI's browser dvars and their registered defaults (iw3mp.exe
/// 0x541d5b, 0x54205b..0x542342; cod4-handshake.md 12.3).
pub const UI_DVARS: [(&str, &str); 15] = [
    ("ui_netSource", "1"),
    ("ui_browserShowFull", "1"),
    ("ui_browserShowEmpty", "1"),
    ("ui_browserShowPassword", "-1"),
    ("ui_browserShowPure", "1"),
    ("ui_browserShowDedicated", "0"),
    ("ui_browserMod", "0"),
    ("ui_browserFriendlyfire", "-1"),
    ("ui_browserKillcam", "-1"),
    ("ui_browserHardcore", "-1"),
    ("ui_browserOldSchool", "-1"),
    ("ui_browserShowPunkBuster", "-1"),
    ("ui_joinGametype", "0"),
    ("ui_favoriteAddress", ""),
    ("password", ""),
];

/// vcod's own dvar, no retail one: 1 lists CoD4x servers, 0 (the
/// default) leaves them out of every list. A shell cvar, archived.
pub const SHOW_COD4X: &str = "ui_browserShowCoD4x";
/// What a CoD4x row's name column starts with, so the refused join is
/// no surprise.
const COD4X_TAG: &str = "[CoD4x] ";

/// A dvar the browser's menus own: drawn and set though vcod has no cvar.
pub fn ui_dvar(name: &str) -> Option<&'static str> {
    UI_DVARS
        .iter()
        .find(|(d, _)| d.eq_ignore_ascii_case(name))
        .map(|&(_, v)| v)
}

/// The months `ui_lastServerRefresh_<n>` names (iw3mp.exe 0x723dc0).
const MONTHS: [&str; 12] = [
    "EXE_MONTH_ABV_JANUARY",
    "EXE_MONTH_ABV_FEBRUARY",
    "EXE_MONTH_ABV_MARCH",
    "EXE_MONTH_ABV_APRIL",
    "EXE_MONTH_ABV_MAY",
    "EXE_MONTH_ABV_JUN",
    "EXE_MONTH_ABV_JULY",
    "EXE_MONTH_ABV_AUGUST",
    "EXE_MONTH_ABV_SEPTEMBER",
    "EXE_MONTH_ABV_OCTOBER",
    "EXE_MONTH_ABV_NOVEMBER",
    "EXE_MONTH_ABV_DECEMBER",
];
/// `ui_netSource`'s names (0x723df0).
const SOURCES: [&str; 3] = ["EXE_LOCAL", "EXE_INTERNET", "EXE_FAVORITES"];
/// The status popup's labelled keys, in the order they lead the list,
/// and how each value reads: 0 as sent, 1 yes or no, 2 a game type,
/// 3 a map (0x723e00).
const STATUS_KEYS: [(&str, &str, u8); 25] = [
    ("sv_hostname", "@EXE_SV_INFO_SERVERNAME", 0),
    ("address", "@EXE_SV_INFO_ADDRESS", 0),
    ("pswrd", "@EXE_SV_INFO_PASSWORD", 1),
    ("gamename", "@EXE_SV_INFO_GAMENAME", 0),
    ("g_gametype", "@EXE_SV_INFO_GAMETYPE", 2),
    ("sv_pure", "@EXE_SV_INFO_PURE", 1),
    ("mapname", "@EXE_SV_INFO_MAP", 3),
    ("shortversion", "@EXE_SV_INFO_VERSION", 0),
    ("protocol", "@EXE_SV_INFO_PROTOCOL", 0),
    ("sv_maxping", "@EXE_SV_INFO_MAXPING", 0),
    ("sv_minping", "@EXE_SV_INFO_MINPING", 0),
    ("sv_maxrate", "@EXE_SV_INFO_MAXRATE", 0),
    ("sv_floodprotect", "@EXE_SV_INFO_FLOODPROTECT", 1),
    ("sv_allowanonymous", "@EXE_SV_INFO_ALLOWANON", 0),
    ("sv_maxclients", "@EXE_SV_INFO_MAXCLIENTS", 0),
    ("sv_privateclients", "@EXE_SV_INFO_PRIVATECLIENTS", 0),
    ("scr_friendlyFire", "@EXE_SV_INFO_FRIENDLY_FIRE", 0),
    ("fs_game", "@EXE_SV_INFO_MOD", 0),
    ("mod", "@MENU_MODS", 1),
    ("scr_killcam", "@EXE_SV_INFO_KILLCAM", 1),
    ("g_antilag", "@EXE_SV_INFO_ANTILAG", 1),
    ("g_compassShowEnemies", "@EXE_SV_INFO_COMPASS_ENEMIES", 1),
    ("sv_voice", "@EXE_SV_INFO_VOICE", 1),
    ("sv_punkbuster", "@MPUI_PUNKBUSTER", 1),
    ("sv_disableClientConsole", "@EXE_SV_INFO_CLIENT_CONSOLE", 1),
];
/// The hardware column's icons by `hw` (0x549183..0x54924b).
const HARDWARE: [(u8, &str); 6] = [
    (0, "server_hardware_unknown"),
    (1, "server_hardware_linux_dedicated"),
    (2, "server_hardware_win_dedicated"),
    (3, "server_hardware_mac_dedicated"),
    (6, "server_hardware_win_listen"),
    (7, "server_hardware_mac_listen"),
];

/// What `JoinServer` asks the host to do: connect to `addr` with the
/// `password` dvar in the userinfo.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct JoinRequest {
    pub addr: SocketAddrV4,
    pub password: String,
}

/// The browser's display state: the rows shown (indexes into the
/// source's list), the picked row, the sort and the join asked for.
#[derive(Clone, Debug, Default)]
pub struct Display {
    pub rows: Vec<usize>,
    pub selected: usize,
    /// `ServerSort`'s column and its flip (0xcaf79b4, 0xcaf79b8).
    pub sort: Option<i32>,
    pub reverse: bool,
    pub join: Option<JoinRequest>,
}

use super::ScriptMenus;
use vcod_cod4::menuexp::atoi;
use vcod_cod4::net::parse::Gamestate;

impl ScriptMenus {
    /// Join Game: `pc_join_unranked` opens over the menu up, its
    /// `onOpen` (`UpdateFilter`) refreshing a Local source.
    #[cfg(test)]
    pub fn open_join(&mut self, gs: &Gamestate) -> Vec<String> {
        let mut out = Vec::new();
        self.push_menu(JOIN_MENU, gs, &mut out, 0);
        out
    }

    /// The browser's work for a frame: replies read, pings sent, the
    /// display rebuilt.
    pub fn pump_browser(&mut self, now: Instant) {
        self.browser.pump(now);
        self.build_display();
    }

    /// The join `JoinServer` asked for, once.
    pub fn take_join(&mut self) -> Option<JoinRequest> {
        self.display.join.take()
    }

    fn net_source(&self) -> Source {
        Source::of(atoi(&self.dvar("ui_netSource").unwrap_or_default()))
    }

    /// The servers of the current source, as shown.
    #[cfg(test)]
    pub fn shown_servers(&self) -> Vec<&Server> {
        let list = self.browser.list(self.net_source());
        self.display
            .rows
            .iter()
            .filter_map(|&i| list.get(i))
            .collect()
    }

    fn selected_server(&self) -> Option<&Server> {
        let i = *self.display.rows.get(self.display.selected)?;
        self.browser.list(self.net_source()).get(i)
    }

    /// The game types `ui_joinGametype` walks: "All", then the zones'.
    fn join_gametype(&self) -> Option<&(String, String)> {
        let at = atoi(&self.dvar("ui_joinGametype").unwrap_or_default());
        usize::try_from(at - 1)
            .ok()
            .and_then(|i| self.lists.gametypes.get(i))
    }

    /// `UI_BuildServerDisplayList` (0x5472b0): the source's marked rows
    /// with a ping (any favourite), through the filter dvars, sorted.
    /// vcod also leaves out CoD4x servers, which never let 1.7 play
    /// (cod4-handshake.md 13), unless [`SHOW_COD4X`] is set.
    pub(super) fn build_display(&mut self) {
        let src = self.net_source();
        let d = |m: &Self, n: &str| atoi(&m.dvar(n).unwrap_or_default());
        let (full, empty, pw, pure, ded, md) = (
            d(self, "ui_browserShowFull"),
            d(self, "ui_browserShowEmpty"),
            d(self, "ui_browserShowPassword"),
            d(self, "ui_browserShowPure"),
            d(self, "ui_browserShowDedicated"),
            d(self, "ui_browserMod"),
        );
        let exact = [
            d(self, "ui_browserFriendlyfire"),
            d(self, "ui_browserKillcam"),
            d(self, "ui_browserHardcore"),
            d(self, "ui_browserOldSchool"),
            d(self, "ui_browserShowPunkBuster"),
        ];
        let cod4x = d(self, SHOW_COD4X) != 0;
        let gametype = self.join_gametype().map(|(g, _)| g.clone());
        let list = self.browser.list(src);
        let mut rows: Vec<usize> = (0..list.len())
            .filter(|&i| {
                let s = &list[i];
                let r = &s.rec;
                let i = &r.info;
                let ping = r.ping as i16;
                r.visible
                    && (ping > 0 || src == Source::Favorites)
                    && (empty != 0 || i.clients != 0)
                    && (full != 0 || i.clients != i.max_clients)
                    && (pw < 0 || i32::from(i.password) == pw)
                    && (pure == 0 || i.pure != 0)
                    && (ded == 0 || matches!(i.hw, 1..=3))
                    && (md < 0 || i32::from(i.modded) == md)
                    && (cod4x || !i.cod4x)
                    && [i.ff, i.kc, i.hc, i.od, u8::from(i.pb)]
                        .iter()
                        .zip(exact)
                        .all(|(&v, want)| want < 0 || i32::from(v) == want)
                    && gametype
                        .as_ref()
                        .is_none_or(|g| g.eq_ignore_ascii_case(&i.gametype))
            })
            .collect();
        if let Some(col) = self.display.sort {
            rows.sort_by(|&a, &b| {
                let o = self.compare(&list[a], &list[b], col);
                if self.display.reverse { o.reverse() } else { o }
            });
        }
        self.display.rows = rows;
    }

    /// `LAN_CompareServers` (0x475b20): the column's key, then the ping,
    /// the game type and the host name.
    fn compare(&self, a: &Server, b: &Server, col: i32) -> std::cmp::Ordering {
        let (ia, ib) = (a.info(), b.info());
        let host = |s: &Server| super::controls::clean(&s.info().hostname).to_lowercase();
        let ping = |s: &Server| s.rec.ping as i16;
        let key = match col {
            0 => ia.password.cmp(&ib.password),
            // A `hw` of 0 sorts last.
            1 => (ia.hw == 0, ia.hw).cmp(&(ib.hw == 0, ib.hw)),
            2 => host(a).cmp(&host(b)),
            3 => self.map_name(&ia.mapname).cmp(&self.map_name(&ib.mapname)),
            4 => ia.clients.cmp(&ib.clients),
            5 => self
                .gametype_name(&ia.gametype)
                .cmp(&self.gametype_name(&ib.gametype)),
            6 => ia.voice.cmp(&ib.voice),
            7 => ia.pure.cmp(&ib.pure),
            8 => ia.modded.cmp(&ib.modded),
            9 => ia.pb.cmp(&ib.pb),
            _ => ping(a).cmp(&ping(b)),
        };
        key.then(ping(a).cmp(&ping(b)))
            .then_with(|| ia.gametype.cmp(&ib.gametype))
            .then_with(|| host(a).cmp(&host(b)))
    }

    fn localized(&self, key: &str) -> String {
        match &self.strings {
            Some(l) => l.get(key).unwrap_or(key).to_string(),
            None => key.to_string(),
        }
    }

    /// A map's display name: its arena's long name, else itself.
    fn map_name(&self, map: &str) -> String {
        self.lists
            .maps
            .iter()
            .find(|a| a.map.eq_ignore_ascii_case(map))
            .map_or_else(|| map.to_string(), |a| self.localized(&a.longname))
    }

    /// A game type's display name, `?` for one the zones do not list.
    fn gametype_name(&self, g: &str) -> String {
        self.lists
            .gametypes
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(g))
            .map_or_else(|| "?".into(), |(_, k)| self.localized(k))
    }

    pub(super) fn browser_feeder_count(&self, feeder: f32) -> Option<usize> {
        if feeder == FEEDER_SERVERS {
            Some(self.display.rows.len())
        } else if feeder == FEEDER_SERVERSTATUS {
            Some(self.status_rows().len())
        } else {
            None
        }
    }

    /// `UI_FeederItemText` (0x5483e0) for the two browser feeders.
    pub(super) fn browser_feeder_text(
        &self,
        feeder: f32,
        row: usize,
        col: usize,
    ) -> Option<String> {
        if feeder == FEEDER_SERVERSTATUS {
            return Some(
                self.status_rows()
                    .get(row)
                    .and_then(|r| r.get(col).cloned())
                    .unwrap_or_default(),
            );
        }
        if feeder != FEEDER_SERVERS {
            return None;
        }
        let list = self.browser.list(self.net_source());
        let Some(s) = self.display.rows.get(row).and_then(|&i| list.get(i)) else {
            return Some(String::new());
        };
        let i = s.info();
        let x = |on: bool| if on { "X".to_string() } else { String::new() };
        let ping = s.rec.ping as i16;
        Some(match col {
            0 => x(i.password != 0),
            2 if ping <= 0 => s.rec.addr.to_string(),
            2 if i.cod4x => format!("{COD4X_TAG}{}", i.hostname)
                .chars()
                .take(37)
                .collect(),
            2 => i.hostname.chars().take(37).collect(),
            3 => self.map_name(&i.mapname),
            4 => format!("{} ({})", i.clients, i.max_clients),
            5 => self.gametype_name(&i.gametype),
            6 => x(i.voice != 0),
            7 => x(i.pure != 0),
            8 => x(i.modded == 0),
            9 => x(i.pb),
            10 if ping <= 0 => "...".into(),
            10 => ping.to_string(),
            _ => String::new(),
        })
    }

    /// `UI_FeederItemImage`: the hardware column's icon.
    pub fn feeder_image(&self, feeder: f32, row: usize, col: usize) -> Option<&'static str> {
        if feeder != FEEDER_SERVERS || col != 1 {
            return None;
        }
        let list = self.browser.list(self.net_source());
        let s = self.display.rows.get(row).and_then(|&i| list.get(i))?;
        HARDWARE
            .iter()
            .find(|(hw, _)| *hw == s.info().hw)
            .map(|&(_, m)| m)
    }

    pub(super) fn browser_feeder_select(&mut self, feeder: f32, row: usize) -> bool {
        if feeder == FEEDER_SERVERS {
            self.display.selected = row;
            true
        } else {
            feeder == FEEDER_SERVERSTATUS
        }
    }

    /// The status popup's rows, four columns each (`UI_BuildServerStatus`
    /// 0x548160): the address and the info, the labelled keys first in
    /// their table's order, a blank row and the header, then the players.
    pub fn status_rows(&self) -> Vec<Vec<String>> {
        let Some((addr, Some(st))) = &self.browser.status else {
            return Vec::new();
        };
        let mut pairs: Vec<(String, String)> = vec![("address".into(), addr.to_string())];
        pairs.extend(st.info.iter().cloned());
        let mut rows = Vec::new();
        for (key, label, kind) in STATUS_KEYS {
            if let Some((_, v)) = pairs.iter().find(|(k, _)| k.eq_ignore_ascii_case(key)) {
                let v = match kind {
                    1 if atoi(v) != 0 => "@EXE_YES".into(),
                    1 => "@EXE_NO".into(),
                    2 => self.gametype_name(v),
                    3 => self.map_name(v),
                    _ => v.clone(),
                };
                rows.push(vec![label.to_string(), String::new(), String::new(), v]);
            }
        }
        for (k, v) in &pairs {
            if !STATUS_KEYS
                .iter()
                .any(|(key, _, _)| key.eq_ignore_ascii_case(k))
            {
                rows.push(vec![k.clone(), String::new(), String::new(), v.clone()]);
            }
        }
        rows.push(vec![String::new(); 4]);
        rows.push(
            [
                "@EXE_SV_INFO_NUM",
                "@EXE_SV_INFO_SCORE",
                "@EXE_SV_INFO_PING",
                "@EXE_SV_INFO_NAME",
            ]
            .map(String::from)
            .to_vec(),
        );
        for (n, p) in st.players.iter().enumerate() {
            rows.push(vec![
                n.to_string(),
                p.score.to_string(),
                p.ping.to_string(),
                p.name.clone(),
            ]);
        }
        rows
    }

    /// The text of the join menu's owner draws: the source, the game type
    /// filter and the refresh line (0x543a60, 0x543940, 0x543ef0).
    pub(super) fn browser_owner_text(&self, owner_draw: i32) -> Option<String> {
        let message = |text: &str| match &self.strings {
            Some(l) => l.message(text),
            None => text.to_string(),
        };
        let src = self.net_source() as usize;
        Some(match owner_draw {
            UI_NETSOURCE => message(&format!("EXE_NETSOURCE\u{14}{}", SOURCES[src])),
            UI_JOINGAMETYPE => match self.join_gametype() {
                Some((_, key)) => format!("@{key}"),
                None => "@EXE_ALL".into(),
            },
            UI_SERVERREFRESHDATE => {
                let b = &self.browser;
                if b.refreshing && b.waiting_master() {
                    "@EXE_WAITINGFORMASTERSERVERRESPONSE".into()
                } else if b.refreshing {
                    let total = b.list(self.net_source()).len();
                    let sent = b
                        .list(self.net_source())
                        .iter()
                        .filter(|s| s.sent.is_some())
                        .count();
                    message("EXE_GETTINGINFOFORSERVERS").replace("&&1", &format!("{sent}/{total}"))
                } else {
                    let when = self
                        .dvar(&format!("ui_lastServerRefresh_{src}"))
                        .unwrap_or_default();
                    message("EXE_REFRESHTIME").replace("&&1", &when)
                }
            }
            _ => return None,
        })
    }

    /// A click or Enter (`forward`) or a back step on the source or the
    /// game type filter (0x544d00, 0x544c80); false for other items.
    pub(super) fn browser_owner_key(&mut self, owner_draw: i32, forward: bool) -> bool {
        match owner_draw {
            UI_NETSOURCE => {
                let cur = self.net_source() as i32;
                let next = (cur + if forward { 1 } else { 2 }) % 3;
                self.set_dvar("ui_netSource", &next.to_string());
                if next != 1 {
                    self.refresh(Source::of(next), true);
                }
            }
            UI_JOINGAMETYPE => {
                let n = self.lists.gametypes.len().min(31) as i32 + 1;
                let cur = atoi(&self.dvar("ui_joinGametype").unwrap_or_default());
                let next = (cur + if forward { 1 } else { n - 1 }).rem_euclid(n);
                self.set_dvar("ui_joinGametype", &next.to_string());
            }
            _ => return false,
        }
        self.build_display();
        true
    }

    /// `UI_StartServerRefresh`, the refresh date first.
    fn refresh(&mut self, src: Source, full: bool) {
        let now = jiff::Zoned::now();
        let month = self.localized(MONTHS[(now.month() - 1) as usize]);
        let date = format!(
            "{month} {}, {}   {}:{:02}",
            now.day(),
            now.year(),
            now.hour(),
            now.minute()
        );
        self.ui
            .dvars
            .insert(format!("ui_lastserverrefresh_{}", src as usize), date);
        let name = self.dvar("masterServerName");
        if let Some(n) = name.filter(|n| !n.is_empty()) {
            self.browser.master_name = n;
        }
        if let Some(p) = self.dvar("masterPort").and_then(|p| p.parse().ok()) {
            self.browser.master_port = p;
        }
        self.browser.refresh(src, full);
    }

    /// The browser's `uiScript`s (`UI_RunMenuScript`, 0x546186..0x546a77);
    /// false for another script.
    pub(super) fn browser_script(
        &mut self,
        name: &str,
        args: &[String],
        gs: &Gamestate,
        out: &mut Vec<String>,
    ) -> bool {
        let src = self.net_source();
        match name.to_ascii_lowercase().as_str() {
            "refreshservers" => self.refresh(src, true),
            "refreshfilter" => self.refresh(src, false),
            "updatefilter" => {
                if src == Source::Local {
                    self.refresh(src, true);
                }
                self.build_display();
                self.display.selected = 0;
            }
            "serverstatus" => {
                if let Some(addr) = self.selected_server().map(|s| s.rec.addr) {
                    self.browser.ask_status(addr);
                }
            }
            "joinserver" => {
                let Some((addr, pb)) = self.selected_server().map(|s| (s.rec.addr, s.info().pb))
                else {
                    return true;
                };
                // vcod runs no PunkBuster (`cl_punkbuster` 0).
                if pb && !self.joinpb_answered {
                    self.push_menu("joinpb_popmenu", gs, out, 1);
                    return true;
                }
                self.joinpb_answered = false;
                let password = self.dvar("password").unwrap_or_default();
                log::info!("cod4 browser: joining {addr}");
                self.display.join = Some(JoinRequest { addr, password });
            }
            "serversort" => {
                let col = args.first().map_or(0, |a| atoi(a));
                if self.display.sort == Some(col) {
                    self.display.reverse = !self.display.reverse;
                }
                self.display.sort = Some(col);
                self.build_display();
            }
            "addfavorite" => {
                if src != Source::Favorites
                    && let Some(s) = self.selected_server()
                {
                    let (addr, host) = (s.rec.addr, s.info().hostname.clone());
                    let r = self.browser.add_favorite(Some(addr), &host);
                    self.favorite_message(r);
                }
            }
            "deletefavorite" => {
                if src == Source::Favorites
                    && let Some(addr) = self.selected_server().map(|s| s.rec.addr)
                {
                    self.browser.remove_favorite(addr);
                }
            }
            "createfavorite" => {
                if src == Source::Favorites {
                    let text = self.dvar("ui_favoriteAddress").unwrap_or_default();
                    let r = if text.trim().is_empty() {
                        None
                    } else {
                        Some(self.browser.add_favorite(
                            vcod_common::net::master::parse_address(&text),
                            "default",
                        ))
                    };
                    match r {
                        Some(r) => self.favorite_message(r),
                        None => self.set_dvar("ui_favorite_message", "@EXE_FAVORITEADDRESSEMPTY"),
                    }
                    self.refresh(src, true);
                }
            }
            "closejoin" => {
                if self.browser.refreshing {
                    self.browser.stop();
                    self.build_display();
                } else {
                    let cmds = [
                        vcod_cod4::scriptmenu::Cmd::Close("joinserver".into()),
                        vcod_cod4::scriptmenu::Cmd::Open("main".into()),
                    ];
                    self.run(JOIN_MENU, None, &cmds, gs, out, 1);
                }
            }
            "setpbclstatus" => {
                // `joinpb_popmenu`'s Yes: vcod joins without PunkBuster.
                self.joinpb_answered = true;
            }
            _ => return false,
        }
        true
    }

    fn favorite_message(&mut self, r: AddFavorite) {
        let key = match r {
            AddFavorite::Added => "@EXE_FAVORITEADDED",
            AddFavorite::InList => "@EXE_FAVORITEINLIST",
            AddFavorite::Full => "@EXE_FAVORITELISTFULL",
            AddFavorite::BadAddress => "@EXE_BADSERVERADDRESS",
        };
        self.set_dvar("ui_favorite_message", key);
    }
}

impl Browser {
    /// Waiting on the master's name or its first answer.
    pub fn waiting_master(&self) -> bool {
        self.resolve.is_some()
            || self.master_due.is_some()
            || (self.source == Source::Internet
                && self.master.is_some()
                && self.lists[1].is_empty())
    }
}

/// vcod-server's CoD4 `war` on mp_crash on a loopback port, served on a
/// thread for tests (the browser's, the front end's) until it is dropped:
/// `password` its `g_password`; [`TestServer::command`] sends client 0 a
/// server command.
#[cfg(test)]
pub(crate) struct TestServer {
    pub(crate) addr: SocketAddrV4,
    run: std::sync::Arc<std::sync::atomic::AtomicBool>,
    commands: mpsc::Sender<String>,
    handle: Option<std::thread::JoinHandle<()>>,
}

#[cfg(test)]
impl TestServer {
    pub(crate) fn start(password: &str) -> Option<TestServer> {
        use std::sync::atomic::{AtomicBool, Ordering};
        use vcod_server::cod4::{Cod4Config, Cod4Server, FRAME_MS};
        let install = vcod_cod4::testing::cod4_install()?;
        let run = std::sync::Arc::new(AtomicBool::new(true));
        let flag = run.clone();
        let password = password.to_string();
        let (tx, rx) = mpsc::channel();
        let (cmd_tx, cmd_rx) = mpsc::channel::<String>();
        // The server is built on its own thread: it is not `Send`.
        let h = std::thread::spawn(move || {
            let mut sv = Cod4Server::new(
                Cod4Config {
                    map: "mp_crash".into(),
                    gametype: "war".into(),
                    hostname: "^1vcod ^7browser".into(),
                    max_clients: 6,
                },
                7,
            );
            sv.load(Some(&install)).unwrap();
            sv.set_cvar("g_password", &password);
            let sock = UdpSocket::bind("127.0.0.1:0").unwrap();
            sock.set_nonblocking(true).unwrap();
            tx.send(sock.local_addr().unwrap()).unwrap();
            let mut next = Instant::now();
            let mut buf = [0u8; 4096];
            while flag.load(Ordering::Relaxed) {
                while let Ok((n, from)) = sock.recv_from(&mut buf) {
                    sv.handle_packet(from, &buf[..n], Instant::now());
                }
                while let Ok(c) = cmd_rx.try_recv() {
                    sv.send_server_command(0, &c);
                }
                let now = Instant::now();
                if now >= next {
                    sv.tick_frames(now, 1);
                    next += Duration::from_millis(FRAME_MS as u64);
                }
                for (to, p) in sv.take_outgoing() {
                    let _ = sock.send_to(&p, to);
                }
                std::thread::sleep(Duration::from_millis(1));
            }
        });
        let SocketAddr::V4(addr) = rx.recv().ok()? else {
            return None;
        };
        Some(TestServer {
            addr,
            run,
            commands: cmd_tx,
            handle: Some(h),
        })
    }

    pub(crate) fn command(&self, text: &str) {
        let _ = self.commands.send(text.to_string());
    }
}

#[cfg(test)]
impl Drop for TestServer {
    fn drop(&mut self) {
        self.run.store(false, std::sync::atomic::Ordering::Relaxed);
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cod4_hud::{Cod4Hud, MenuAssets};
    use crate::cod4_menu::paint;
    use crate::hud::cod2elem::ScreenPlacement;
    use crate::hud::font;
    use crate::play::cod4_menus::{Join, MenuKey};

    const SIZE: (f32, f32) = (640.0, 480.0);

    /// The front end's menus: `ui_mp`'s over the HUD zones, as the main
    /// menu loads them, with the zones' game types, maps and strings.
    fn front_end(hud: &Cod4Hud) -> ScriptMenus {
        let (defs, tables) = hud.script_menus().unwrap();
        let mut m = ScriptMenus::new(Join::default());
        m.set_defs(defs.clone(), tables.clone());
        m.set_lists(hud.ui_lists().unwrap().clone());
        m.strings = hud.strings();
        m
    }

    fn texts(m: &ScriptMenus, a: &MenuAssets) -> Vec<(String, [f32; 2])> {
        paint(m, a, SIZE, 0, true).1
    }

    fn click_at(m: &mut ScriptMenus, gs: &Gamestate, p: [f32; 2]) -> Vec<String> {
        m.mouse_move(None, Some(p), SIZE, gs);
        m.click(SIZE, gs)
    }

    /// Moves the cursor onto the drawn text `label` and clicks.
    fn click(m: &mut ScriptMenus, a: &MenuAssets, gs: &Gamestate, label: &str) -> Vec<String> {
        let t = texts(m, a);
        let p = t
            .iter()
            .find(|(s, _)| s == label)
            .unwrap_or_else(|| panic!("no {label:?} in {t:?}"))
            .1;
        click_at(m, gs, [p[0] + 2.0, p[1] - 4.0])
    }

    /// Pumps the browser a frame at a time until `done` or 5 s pass, the
    /// frame's sleep first: a Local reply read in the broadcast's own
    /// millisecond has ping 0, which the display list hides (retail's
    /// `time - start` at 0x470df3 has no `+ 1` either).
    fn pump_until(m: &mut ScriptMenus, done: impl Fn(&ScriptMenus) -> bool) -> bool {
        let t0 = Instant::now();
        while t0.elapsed() < Duration::from_secs(5) {
            std::thread::sleep(Duration::from_millis(16));
            m.pump_browser(Instant::now());
            if done(m) {
                return true;
            }
        }
        false
    }

    fn open_name(m: &ScriptMenus) -> &str {
        m.open().map_or("", |o| o.name.as_str())
    }

    /// Join Game against vcod-server on loopback, through the zone's
    /// `pc_join_unranked`: Local finds the server (its broadcast pointed
    /// at the server's port), the row draws the cleaned host name, the
    /// map's display name, `0 (6)`, the game type and a ping; Server Info
    /// lists `getstatus`'s keys over the join menu and Esc uncovers it;
    /// the password popup's field takes typed text into `password`; Join
    /// asks the host to connect with it, and the server lets a client with
    /// that password in and refuses one without.
    #[test]
    fn cod4_join_game_lists_a_lan_server_and_joins_it_with_its_password() {
        let Some(install) = vcod_cod4::testing::cod4_install() else {
            return;
        };
        let Some(server) = TestServer::start("secret") else {
            return;
        };
        let addr = server.addr;
        let mut hud = Cod4Hud::default();
        hud.load(&install.root().join("main"), "ui_mp");
        let a = hud.menu_assets().unwrap();
        let gs = Gamestate::default();
        let mut m = front_end(&hud);
        m.browser.lan_targets = vec![addr];
        m.set_dvar("ui_netSource", "0");
        m.open_join(&gs);
        assert_eq!(open_name(&m), JOIN_MENU);
        assert!(
            m.open().unwrap().layout.is_some(),
            "the join menu evaluates"
        );
        assert!(
            pump_until(&mut m, |m| !m.shown_servers().is_empty()
                && !m.browser.refreshing),
            "Local found no server"
        );
        let t: Vec<String> = texts(&m, &a).into_iter().map(|(s, _)| s).collect();
        let row = m.shown_servers()[0].clone();
        assert_eq!(row.rec.addr, addr);
        assert!(row.rec.ping > 0 && row.rec.ping < 999, "{}", row.rec.ping);
        for want in ["^1vcod ^7browser", "Crash", "0 (6)", "Team Deathmatch", "X"] {
            assert!(t.iter().any(|s| s == want), "{want:?} not in {t:?}");
        }
        assert!(t.iter().any(|s| s == &row.rec.ping.to_string()), "{t:?}");
        // The source line and the refresh line localize.
        assert!(t.iter().any(|s| s.contains("Local")), "{t:?}");
        assert!(!t.iter().any(|s| s.contains("EXE_")), "{t:?}");
        // The hardware icon draws in its column.
        let tex = hud.texture_of("server_hardware_linux_dedicated").unwrap();
        assert!(
            paint(&m, &a, SIZE, 0, true)
                .0
                .iter()
                .any(|q| q.texture == tex)
        );

        // The row picked, Server Info opens over the join menu.
        click_at(&mut m, &gs, [100.0, 89.0]);
        assert_eq!(m.display.selected, 0);
        click(&mut m, &a, &gs, "Server Info");
        assert_eq!(open_name(&m), "serverinfo_popmenu");
        assert!(pump_until(&mut m, |m| !m.status_rows().is_empty()));
        let t: Vec<String> = texts(&m, &a).into_iter().map(|(s, _)| s).collect();
        for want in ["Crash", "1.7", "Call of Duty 4", "Password Protected"] {
            assert!(t.iter().any(|s| s.contains(want)), "{want:?} not in {t:?}");
        }
        let mut out = Vec::new();
        m.key(MenuKey::Escape, &gs, &mut out);
        assert_eq!(open_name(&m), JOIN_MENU);

        // The password popup: its field takes the text, Enter accepts.
        click(&mut m, &a, &gs, "Password");
        assert_eq!(open_name(&m), "password_popmenu");
        assert!(m.editing());
        m.type_text("secrex");
        m.key(MenuKey::Char(127), &gs, &mut out);
        m.type_text("t");
        m.key(MenuKey::Enter, &gs, &mut out);
        assert_eq!(open_name(&m), JOIN_MENU);
        assert_eq!(m.dvar("password").as_deref(), Some("secret"));

        click(&mut m, &a, &gs, "Join Server");
        let join = m.take_join().expect("Join Server asks for a join");
        assert_eq!(
            join,
            JoinRequest {
                addr,
                password: "secret".into()
            }
        );
        use crate::play::cod4::Cod4Session;
        use vcod_cod4::net::conn::Cp;
        let to = SocketAddr::V4(addr);
        let s =
            Cod4Session::connect_password(to, "joiner", "secret", Cp::Bare, Join::default(), None);
        assert!(s.is_ok(), "{:?}", s.err());
        let refused =
            Cod4Session::connect_password(to, "nopass", "", Cp::Bare, Join::default(), None);
        let why = format!("{:#}", refused.err().expect("no password, no join"));
        assert!(why.contains("GAME_INVALIDPASSWORD"), "{why}");
        drop(server);
    }

    /// Favorites and Internet: New Favorite's popup takes an address,
    /// `CreateFavorite` adds it, says so in `fav_message_popmenu` and
    /// pings it; `servercache.dat` keeps it. The source steps to Internet,
    /// whose refresh asks a fake master on loopback (`masterServerName`,
    /// `masterPort`) for `getservers 6 full empty`; the server it lists
    /// is pinged and shown.
    #[test]
    fn cod4_favorites_and_a_fake_masters_list() {
        let Some(install) = vcod_cod4::testing::cod4_install() else {
            return;
        };
        let Some(server) = TestServer::start("") else {
            return;
        };
        let addr = server.addr;
        let mut hud = Cod4Hud::default();
        hud.load(&install.root().join("main"), "ui_mp");
        let a = hud.menu_assets().unwrap();
        let gs = Gamestate::default();
        let dir = std::env::temp_dir().join(format!("vcod-cod4-browser-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let cache = dir.join(vcod_cod4::net::browser::CACHE_FILE);
        let _ = std::fs::remove_file(&cache);
        let mut m = front_end(&hud);
        m.browser = Browser::with_cache(cache.clone());
        m.browser.lan_targets.clear();
        m.set_dvar("ui_netSource", "2");
        m.open_join(&gs);
        click(&mut m, &a, &gs, "New Favorite");
        assert_eq!(open_name(&m), "createfavorite_popmenu");
        assert!(m.editing());
        m.type_text(&addr.to_string());
        let mut out = Vec::new();
        m.key(MenuKey::Enter, &gs, &mut out);
        assert_eq!(open_name(&m), "fav_message_popmenu");
        assert_eq!(
            m.dvar("ui_favorite_message").as_deref(),
            Some("@EXE_FAVORITEADDED")
        );
        let t: Vec<String> = texts(&m, &a).into_iter().map(|(s, _)| s).collect();
        assert!(!t.iter().any(|s| s.contains("EXE_")), "{t:?}");
        click(&mut m, &a, &gs, "OK");
        assert_eq!(open_name(&m), JOIN_MENU);
        assert!(pump_until(&mut m, |m| {
            m.shown_servers()
                .first()
                .is_some_and(|s| s.rec.ping != NO_PING && !m.browser.refreshing)
        }));
        let t: Vec<String> = texts(&m, &a).into_iter().map(|(s, _)| s).collect();
        assert!(t.iter().any(|s| s == "^1vcod ^7browser"), "{t:?}");
        let saved = ServerCache::read(&std::fs::read(&cache).unwrap()).unwrap();
        assert_eq!(saved.favorites.len(), 1);
        assert_eq!(saved.favorites[0].addr, addr);
        assert_eq!(saved.favorites[0].info.hostname, "^1vcod ^7browser");
        assert_eq!(
            Browser::with_cache(cache.clone())
                .list(Source::Favorites)
                .len(),
            1
        );

        // A fake master on loopback lists the server.
        let master = UdpSocket::bind("127.0.0.1:0").unwrap();
        master
            .set_read_timeout(Some(Duration::from_secs(10)))
            .unwrap();
        let port = master.local_addr().unwrap().port();
        let answer = std::thread::spawn(move || {
            let mut buf = [0u8; 256];
            let (n, from) = master.recv_from(&mut buf).ok()?;
            let mut reply = b"\xff\xff\xff\xffgetserversResponse\n\0".to_vec();
            reply.push(b'\\');
            reply.extend_from_slice(&addr.ip().octets());
            reply.extend_from_slice(&addr.port().to_be_bytes());
            reply.extend_from_slice(b"\\EOT\0\0\0");
            master.send_to(&reply, from).ok()?;
            Some(buf[..n].to_vec())
        });
        m.set_dvar("masterServerName", "127.0.0.1");
        m.set_dvar("masterPort", &port.to_string());
        // Favorites -> Local -> Internet.
        for _ in 0..2 {
            let t = texts(&m, &a);
            let p = t.iter().find(|(s, _)| s.starts_with("Source")).unwrap().1;
            click_at(&mut m, &gs, [p[0] + 2.0, p[1] - 4.0]);
        }
        assert_eq!(m.dvar("ui_netSource").as_deref(), Some("1"));
        click(&mut m, &a, &gs, "Refresh List");
        assert!(pump_until(&mut m, |m| m
            .shown_servers()
            .first()
            .is_some_and(|s| s.rec.addr == addr && !m.browser.refreshing)));
        assert_eq!(
            answer.join().unwrap().as_deref(),
            Some(&b"\xff\xff\xff\xffgetservers 6 full empty"[..])
        );
        drop(server);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A fake server on loopback: it answers `getinfo` with `info` until
    /// `run` drops.
    fn fake_server(
        info: &'static str,
        run: &std::sync::Arc<std::sync::atomic::AtomicBool>,
    ) -> (SocketAddrV4, std::thread::JoinHandle<()>) {
        let sock = UdpSocket::bind("127.0.0.1:0").unwrap();
        sock.set_read_timeout(Some(Duration::from_millis(50)))
            .unwrap();
        let SocketAddr::V4(addr) = sock.local_addr().unwrap() else {
            unreachable!()
        };
        let run = run.clone();
        let h = std::thread::spawn(move || {
            let mut buf = [0u8; 256];
            while run.load(std::sync::atomic::Ordering::Relaxed) {
                if let Ok((n, from)) = sock.recv_from(&mut buf)
                    && buf[..n].starts_with(b"\xff\xff\xff\xffgetinfo")
                {
                    let reply = format!("\u{ff}\u{ff}\u{ff}\u{ff}infoResponse\n{info}");
                    let bytes: Vec<u8> = reply.chars().map(|c| c as u32 as u8).collect();
                    let _ = sock.send_to(&bytes, from);
                }
            }
        });
        (addr, h)
    }

    /// CoD4x servers stay out of the lists (cod4-handshake.md 13): a fake
    /// master on loopback lists a stock 1.7 server and a CoD4x one
    /// (`shortversion\x21` in its `infoResponse`); Internet shows only the
    /// stock one until `ui_browserShowCoD4x` is 1, then both, the CoD4x
    /// row's name marked. Local's broadcast finds both and does the same.
    #[test]
    fn cod4_browser_hides_cod4x_servers() {
        use std::sync::atomic::{AtomicBool, Ordering};
        let Some(install) = vcod_cod4::testing::cod4_install() else {
            return;
        };
        let mut hud = Cod4Hud::default();
        hud.load(&install.root().join("main"), "ui_mp");
        let a = hud.menu_assets().unwrap();
        let gs = Gamestate::default();
        let run = std::sync::Arc::new(AtomicBool::new(true));
        let (stock, h1) = fake_server(
            "\\protocol\\6\\hostname\\stock host\\mapname\\mp_crash\\clients\\1\\sv_maxclients\\8\\gametype\\war\\pure\\1\\hw\\1",
            &run,
        );
        let (x, h2) = fake_server(
            "\\protocol\\6\\hostname\\x host\\mapname\\mp_crash\\clients\\1\\sv_maxclients\\8\\gametype\\war\\pure\\1\\hw\\1\\shortversion\\x21\\build\\1",
            &run,
        );
        let master = UdpSocket::bind("127.0.0.1:0").unwrap();
        master
            .set_read_timeout(Some(Duration::from_secs(10)))
            .unwrap();
        let port = master.local_addr().unwrap().port();
        let answer = std::thread::spawn(move || {
            let mut buf = [0u8; 256];
            let (_, from) = master.recv_from(&mut buf).ok()?;
            let mut reply = b"\xff\xff\xff\xffgetserversResponse\n\0".to_vec();
            for addr in [stock, x] {
                reply.push(b'\\');
                reply.extend_from_slice(&addr.ip().octets());
                reply.extend_from_slice(&addr.port().to_be_bytes());
            }
            reply.extend_from_slice(b"\\EOT\0\0\0");
            master.send_to(&reply, from).ok()
        });
        let mut m = front_end(&hud);
        m.browser.lan_targets.clear();
        m.set_dvar("ui_netSource", "1");
        m.set_dvar("masterServerName", "127.0.0.1");
        m.set_dvar("masterPort", &port.to_string());
        m.open_join(&gs);
        click(&mut m, &a, &gs, "Refresh List");
        let answered = |m: &ScriptMenus, src: Source| {
            let l = m.browser.list(src);
            l.len() == 2 && l.iter().all(|s| s.rec.answered) && !m.browser.refreshing
        };
        assert!(pump_until(&mut m, |m| answered(m, Source::Internet)));
        assert!(answer.join().unwrap().is_some());
        let shown = |m: &ScriptMenus| -> Vec<SocketAddrV4> {
            m.shown_servers().iter().map(|s| s.rec.addr).collect()
        };
        assert_eq!(shown(&m), [stock]);
        let t: Vec<String> = texts(&m, &a).into_iter().map(|(s, _)| s).collect();
        assert!(t.iter().any(|s| s == "stock host"), "{t:?}");
        assert!(!t.iter().any(|s| s.contains("x host")), "{t:?}");

        m.set_dvar(SHOW_COD4X, "1");
        m.pump_browser(Instant::now());
        let mut both = shown(&m);
        both.sort();
        let mut want = vec![stock, x];
        want.sort();
        assert_eq!(both, want);
        let t: Vec<String> = texts(&m, &a).into_iter().map(|(s, _)| s).collect();
        assert!(t.iter().any(|s| s == "[CoD4x] x host"), "{t:?}");

        // Local: the broadcast pointed at both.
        m.set_dvar(SHOW_COD4X, "0");
        m.browser.lan_targets = vec![stock, x];
        m.set_dvar("ui_netSource", "0");
        m.refresh(Source::Local, true);
        assert!(pump_until(&mut m, |m| answered(m, Source::Local)));
        assert_eq!(shown(&m), [stock]);
        m.set_dvar(SHOW_COD4X, "1");
        m.pump_browser(Instant::now());
        assert_eq!(shown(&m).len(), 2);
        run.store(false, Ordering::Relaxed);
        let _ = (h1.join(), h2.join());
    }

    /// A double-click on a PunkBuster server's row in Internet: the
    /// list's `doubleClick` runs `JoinServer`, which opens
    /// `joinpb_popmenu`, and the click's `Item_Action` then runs the list
    /// item's own `action`, not an item of the popup (the playtest's
    /// panic: item 58 of a 15-item menu).
    #[test]
    fn cod4_double_click_on_a_pb_server_opens_the_pb_popup() {
        use std::sync::atomic::{AtomicBool, Ordering};
        let Some(install) = vcod_cod4::testing::cod4_install() else {
            return;
        };
        let mut hud = Cod4Hud::default();
        hud.load(&install.root().join("main"), "ui_mp");
        let a = hud.menu_assets().unwrap();
        let gs = Gamestate::default();
        let run = std::sync::Arc::new(AtomicBool::new(true));
        let (pb, h) = fake_server(
            "\\protocol\\6\\hostname\\pb host\\mapname\\mp_crash\\clients\\1\\sv_maxclients\\8\\gametype\\war\\pure\\1\\hw\\1\\pb\\1",
            &run,
        );
        let master = UdpSocket::bind("127.0.0.1:0").unwrap();
        master
            .set_read_timeout(Some(Duration::from_secs(10)))
            .unwrap();
        let port = master.local_addr().unwrap().port();
        let answer = std::thread::spawn(move || {
            let mut buf = [0u8; 256];
            let (_, from) = master.recv_from(&mut buf).ok()?;
            let mut reply = b"\xff\xff\xff\xffgetserversResponse\n\0\\".to_vec();
            reply.extend_from_slice(&pb.ip().octets());
            reply.extend_from_slice(&pb.port().to_be_bytes());
            reply.extend_from_slice(b"\\EOT\0\0\0");
            master.send_to(&reply, from).ok()
        });
        let mut m = front_end(&hud);
        m.browser.lan_targets.clear();
        m.set_dvar("ui_netSource", "1");
        m.set_dvar("masterServerName", "127.0.0.1");
        m.set_dvar("masterPort", &port.to_string());
        m.open_join(&gs);
        click(&mut m, &a, &gs, "Refresh List");
        assert!(pump_until(&mut m, |m| {
            m.shown_servers()
                .first()
                .is_some_and(|s| s.rec.answered && !m.browser.refreshing)
        }));
        assert!(answer.join().unwrap().is_some());
        click(&mut m, &a, &gs, "pb host");
        click(&mut m, &a, &gs, "pb host");
        assert_eq!(open_name(&m), "joinpb_popmenu");
        assert!(m.take_join().is_none());
        run.store(false, Ordering::Relaxed);
        let _ = h.join();
    }

    /// `joinpb_popmenu`'s message is `autowrapped` (static flag
    /// 0x800000): it draws as lines inside the popup's 280-unit text
    /// rect, not one line running off its right edge (the playtest).
    #[test]
    fn cod4_pb_popup_text_wraps_inside_the_popup() {
        let Some(install) = vcod_cod4::testing::cod4_install() else {
            return;
        };
        let mut hud = Cod4Hud::default();
        hud.load(&install.root().join("main"), "ui_mp");
        let a = hud.menu_assets().unwrap();
        let gs = Gamestate::default();
        let mut m = front_end(&hud);
        m.open_by_name("joinpb_popmenu", &gs);
        assert_eq!(open_name(&m), "joinpb_popmenu");
        let layout = m.open().unwrap().layout.as_ref().unwrap();
        let msg = layout
            .items
            .iter()
            .find(|it| it.static_flags & 0x80_0000 != 0)
            .expect("an autowrapped item");
        let scale = msg.text_scale;
        let (texts, f) = (
            texts(&m, &a),
            a.fonts
                .pick(1, scale, &ScreenPlacement::new(SIZE.0, SIZE.1)),
        );
        let full = crate::cod4_menu::localize(&a, "@MPUI_ENABLEPUNKBUSTERTOCONNECT");
        let lines: Vec<_> = texts
            .iter()
            .filter(|(t, _)| full.contains(t.as_str()) && t.len() > 1)
            .collect();
        assert!(lines.len() >= 2, "{full:?} in {texts:?}");
        // The popup's text rect: x -138..142 off the centre, on 640 wide.
        let (sx, left) = (SIZE.0 / 640.0, SIZE.0 / 2.0 - 138.0 * SIZE.0 / 640.0);
        for (t, p) in &lines {
            let w = font::measure(f, t, scale / f.unit_scale()) * sx;
            assert!(p[0] >= left - 0.5, "{t:?} at {p:?}");
            assert!(p[0] + w <= left + 280.0 * sx + 0.5, "{t:?} {w} at {p:?}");
        }
        assert!(lines.windows(2).all(|l| l[1].1[1] > l[0].1[1]));
    }

    /// The browser against a server outside the process
    /// (`COD4_BROWSER_SERVER=ip:port`, retail `cod4_lnxded` in its
    /// namespace): Local with its broadcast sent to that address's
    /// `COD4_BROWSER_BROADCAST` (a broadcast address, ports 28960..28963)
    /// or to the server itself finds it; Server Info lists its status.
    /// Prints both lists. Skipped without the variable.
    #[test]
    fn cod4_browser_against_a_named_server() {
        let Some(addr) = std::env::var("COD4_BROWSER_SERVER")
            .ok()
            .and_then(|a| a.parse::<SocketAddrV4>().ok())
        else {
            return;
        };
        let Some(install) = vcod_cod4::testing::cod4_install() else {
            return;
        };
        let mut hud = Cod4Hud::default();
        hud.load(&install.root().join("main"), "ui_mp");
        let a = hud.menu_assets().unwrap();
        let gs = Gamestate::default();
        let mut m = front_end(&hud);
        m.browser.lan_targets = match std::env::var("COD4_BROWSER_BROADCAST") {
            Ok(b) => {
                let ip: Ipv4Addr = b.parse().unwrap();
                LAN_PORTS
                    .iter()
                    .map(|&p| SocketAddrV4::new(ip, p))
                    .collect()
            }
            Err(_) => vec![addr],
        };
        m.set_dvar("ui_netSource", "0");
        m.open_join(&gs);
        assert!(pump_until(&mut m, |m| m
            .shown_servers()
            .iter()
            .any(|s| s.rec.addr == addr)
            && !m.browser.refreshing));
        let t: Vec<String> = texts(&m, &a).into_iter().map(|(s, _)| s).collect();
        eprintln!("list: {t:?}");
        let row = m
            .shown_servers()
            .iter()
            .position(|s| s.rec.addr == addr)
            .unwrap();
        eprintln!("row: {:?}", m.shown_servers()[row].rec);
        m.display.selected = row;
        click(&mut m, &a, &gs, "Server Info");
        assert!(pump_until(&mut m, |m| !m.status_rows().is_empty()));
        for r in m.status_rows() {
            eprintln!("status: {r:?}");
        }
    }

    /// The join menu with a found server drawn offscreen through the
    /// renderer's HUD pass, as the window draws it (lavapipe will do; no
    /// adapter, no test): the server row's band holds lit text and the
    /// hardware column its icon. `VCOD_C4_SHOTS=<dir>` writes the frame
    /// as `join.tga`.
    #[test]
    fn cod4_join_menu_draws_offscreen() {
        let Some(install) = vcod_cod4::testing::cod4_install() else {
            return;
        };
        let fs = crate::cod4_fly::open_fs(install.root(), None, None).unwrap();
        let Some((mut r, errors)) =
            crate::renderer::Renderer::headless_sized(&fs, false, 1, (640, 480))
        else {
            return;
        };
        let Some(server) = TestServer::start("") else {
            return;
        };
        let addr = server.addr;
        let mut hud = Cod4Hud::default();
        hud.load(&install.root().join("main"), "ui_mp");
        let a = hud.menu_assets().unwrap();
        let gs = Gamestate::default();
        let mut m = front_end(&hud);
        m.browser.lan_targets = vec![addr];
        m.set_dvar("ui_netSource", "0");
        m.open_join(&gs);
        assert!(pump_until(&mut m, |m| !m.shown_servers().is_empty()
            && !m.browser.refreshing));
        drop(server);
        r.set_gamma(1.0, crate::gamma::overbright_bits(fs.game(), false, true));
        r.set_hud_quads(&fs, paint(&m, &a, SIZE, 0, false).0);
        let (eye, fwd, up) = (glam::Vec3::ZERO, glam::Vec3::X, glam::Vec3::Z);
        r.render(
            crate::renderer::Frame {
                view_proj: crate::camera::view_proj_from(eye, 0.0, 0.0, 0.0, 80.0, r.aspect()),
                eye,
                fwd,
                up,
                time: 0.0,
                fov_x: 80.0,
                cull: crate::renderer::CullMode::On,
                hud_lines: Vec::new(),
            },
            None,
        );
        let (w, h, bgra) = r.read_offscreen().expect("offscreen target");
        let errors = errors.lock().unwrap();
        assert!(errors.is_empty(), "{errors:?}");
        let lit = |x0: u32, x1: u32, y0: u32, y1: u32| {
            (y0..y1)
                .flat_map(|y| (x0..x1).map(move |x| (x, y)))
                .filter(|&(x, y)| {
                    let p = &bgra[((y * w + x) * 4) as usize..][..3];
                    p.iter().any(|&c| c > 96)
                })
                .count()
        };
        // The first row: y 81..97 on the 640x480 grid; the host name from
        // x 52, the hardware icon in x 27..43.
        if let Some(dir) = std::env::var_os("VCOD_C4_SHOTS") {
            let rgba: Vec<u8> = bgra
                .chunks(4)
                .flat_map(|p| [p[2], p[1], p[0], 255])
                .collect();
            let path = std::path::Path::new(&dir).join("join.tga");
            image::save_buffer(path, &rgba, w, h, image::ExtendedColorType::Rgba8).unwrap();
        }
        let text = lit(52, 250, 81, 97);
        let icon = lit(27, 43, 81, 97);
        let above = lit(52, 250, 120, 136);
        eprintln!(
            "row text {text}, icon {icon}, empty row {above}, all {}",
            lit(0, w, 0, h)
        );
        assert!(text > 40 && icon > 10 && above < text / 4);
    }
}
