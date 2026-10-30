//! 1.7's server browser on the wire (docs/research/cod4-handshake.md 12):
//! the master's `getservers` and its `getserversResponse`, the `getinfo`
//! broadcast and pings and their `infoResponse`, `getstatus` and its
//! `statusResponse`, and `servercache.dat`, where `iw3mp.exe` keeps the
//! Internet list and the favourites between runs.

use std::net::{Ipv4Addr, SocketAddrV4};

use vcod_common::net::connectionless::{info_pairs, info_value_for_key};

/// The protocol 1.7 speaks and asks the master for (`globalservers 0 6
/// full empty`, iw3mp.exe 0x54ac77).
pub const PROTOCOL: u32 = 6;
/// The master 1.7 registers in `masterServerName` and asks
/// (`CL_GlobalServers_f` 0x47188b), and its port (`masterPort`, 0x4718c5).
/// vcod never contacts it.
pub const RETAIL_MASTER: &str = "cod4master.activision.com";
pub const MASTER_PORT: u16 = 20810;
/// `getservers 6 full empty`.
pub fn getservers() -> String {
    vcod_common::net::master::getservers(PROTOCOL)
}

/// `serverInfo_t`, 156 bytes (`CL_SetServerInfo` 0x470ad0).
pub const RECORD: usize = 0x9c;
/// The three lists' caps: Internet (0x95bcdc..0xc5585c), Local and
/// favourites (`MAX_OTHER_SERVERS`).
pub const MAX_GLOBAL: usize = 20000;
pub const MAX_LOCAL: usize = 0x80;
pub const MAX_FAVORITES: usize = 0x80;
/// `servercache.dat`'s version word and its blocks' size word.
pub const CACHE_VERSION: i32 = 3;
pub const CACHE_FILE: &str = "servercache.dat";
const BLOCKS: usize = (MAX_GLOBAL + MAX_FAVORITES) * RECORD;
const HEADER: usize = 16;
pub const CACHE_LEN: usize = HEADER + BLOCKS;
const FAVORITES_AT: usize = HEADER + MAX_GLOBAL * RECORD;
/// `netadrtype_t`'s `NA_IP` (the type `CL_ServersResponsePacket` stores,
/// 0x471668).
const NA_IP: i32 = 4;
/// A ping not yet measured.
pub const NO_PING: u16 = 0xffff;

/// What a list row knows of a server: `infoResponse`'s keys as
/// `CL_SetServerInfo` stores them, numbers through `atoi` into a byte.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ServerInfo {
    pub hostname: String,
    pub mapname: String,
    pub game: String,
    pub gametype: String,
    pub clients: u8,
    pub max_clients: u8,
    pub nettype: u8,
    pub min_ping: u16,
    pub max_ping: u16,
    pub allow_anonymous: u8,
    pub con_disabled: u8,
    pub password: u8,
    pub pure: u8,
    pub ff: u8,
    pub kc: u8,
    pub hc: u8,
    pub od: u8,
    pub hw: u8,
    pub modded: u8,
    pub voice: u8,
    pub pb: bool,
    /// vcod's own: the server answered as CoD4x ([`info_is_cod4x`],
    /// [`Status::is_cod4x`]). Not a `serverInfo_t` field, so the cache
    /// does not keep it; a cached row learns it again from its next ping.
    pub cod4x: bool,
}

/// C's `atoi`: leading blanks, a sign, digits; 0 for anything else.
fn atoi(s: &str) -> i32 {
    let s = s.trim_start();
    let (neg, digits) = match s.as_bytes().first() {
        Some(b'-') => (true, &s[1..]),
        Some(b'+') => (false, &s[1..]),
        _ => (false, s),
    };
    let n = digits
        .bytes()
        .take_while(u8::is_ascii_digit)
        .fold(0i64, |n, d| {
            (n * 10 + i64::from(d - b'0')).min(i64::from(i32::MAX))
        });
    (if neg { -n } else { n }) as i32
}

/// `Q_strncpyz` into a field of `len` bytes: at most `len - 1`. The text
/// is Latin-1, a char a byte.
fn field(s: &str, len: usize) -> String {
    s.chars().take(len - 1).collect()
}

impl ServerInfo {
    /// The info string after `infoResponse\n`.
    pub fn parse(info: &str) -> ServerInfo {
        let s = |k| info_value_for_key(info, k).unwrap_or("");
        let n = |k| atoi(s(k));
        ServerInfo {
            clients: n("clients") as u8,
            hostname: field(s("hostname"), 32),
            mapname: field(s("mapname"), 32),
            max_clients: n("sv_maxclients") as u8,
            game: field(s("game"), 24),
            gametype: field(s("gametype"), 16),
            nettype: n("nettype") as u8,
            min_ping: n("minping") as u16,
            max_ping: n("maxping") as u16,
            allow_anonymous: n("sv_allowAnonymous") as u8,
            con_disabled: n("con_disabled") as u8,
            password: n("pswrd") as u8,
            pure: n("pure") as u8,
            ff: n("ff") as u8,
            kc: n("kc") as u8,
            hc: n("hc") as u8,
            od: n("od") as u8,
            hw: n("hw") as u8,
            modded: n("mod") as u8,
            voice: n("voice") as u8,
            pb: n("pb") > 0,
            cod4x: info_is_cod4x(info),
        }
    }
}

/// Whether an `infoResponse` comes from a CoD4x server: CoD4x sends
/// `shortversion` (`x21`) there, where 1.7's `getinfo` has no such key
/// (cod4-handshake.md 13).
pub fn info_is_cod4x(info: &str) -> bool {
    info_value_for_key(info, "shortversion").is_some_and(|v| v != "1.7")
}

/// `infoResponse`'s `protocol`, which `CL_ServerInfoPacket` (0x470d3f)
/// compares with 6 before it reads anything else.
pub fn info_protocol(info: &str) -> i32 {
    atoi(info_value_for_key(info, "protocol").unwrap_or(""))
}

/// `CL_ServersResponsePacket` (0x4714b0) over the bytes after the command
/// word: from the first `\`, six address bytes (port big-endian) and a
/// `\` per server, at most 256, ending at `\EOT` or `\EOF` or when fewer
/// than seven bytes are left.
pub fn parse_servers_response(body: &[u8]) -> Vec<SocketAddrV4> {
    let mut out = Vec::new();
    let Some(mut i) = body.iter().position(|&b| b == b'\\') else {
        return out;
    };
    i += 1;
    while i + 6 < body.len() {
        let b = &body[i..i + 6];
        if body[i + 6] != b'\\' {
            break;
        }
        out.push(SocketAddrV4::new(
            Ipv4Addr::new(b[0], b[1], b[2], b[3]),
            u16::from_be_bytes([b[4], b[5]]),
        ));
        i += 7;
        if out.len() >= 256 || matches!(body.get(i..i + 3), Some(b"EOT" | b"EOF")) {
            break;
        }
    }
    out
}

/// A player line of `statusResponse`: `<score> <ping> "<name>"`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StatusPlayer {
    pub score: i32,
    pub ping: i32,
    pub name: String,
}

/// `statusResponse`: the serverinfo's pairs in order, then the players.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Status {
    pub info: Vec<(String, String)>,
    pub players: Vec<StatusPlayer>,
}

impl Status {
    /// The text after `statusResponse\n`.
    pub fn parse(text: &str) -> Status {
        let mut lines = text.split('\n');
        let info = lines
            .next()
            .map(|l| {
                info_pairs(l)
                    .into_iter()
                    .map(|(k, v)| (k.to_string(), v.to_string()))
                    .collect()
            })
            .unwrap_or_default();
        let players = lines
            .filter_map(|l| {
                let mut w = l.splitn(3, ' ');
                let score = w.next()?.parse().ok()?;
                let ping = w.next()?.parse().ok()?;
                let name = w.next()?.trim().trim_matches('"').to_string();
                Some(StatusPlayer { score, ping, name })
            })
            .collect();
        Status { info, players }
    }

    /// Whether the status comes from a CoD4x server: `protocol` other
    /// than 6 (CoD4x sends 21) or a `version` naming `CoD4 X`
    /// (cod4-handshake.md 13).
    pub fn is_cod4x(&self) -> bool {
        let get = |key: &str| {
            self.info
                .iter()
                .find(|(k, _)| k.eq_ignore_ascii_case(key))
                .map(|(_, v)| v.as_str())
        };
        get("protocol").is_some_and(|p| atoi(p) != PROTOCOL as i32)
            || get("version").is_some_and(|v| v.starts_with("CoD4 X"))
    }
}

/// One `serverInfo_t` as `servercache.dat` holds it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Record {
    /// `netadr_t`: type, then the IPv4 address at +4 and the port
    /// (network order) at +8; 20 bytes.
    pub adr_type: i32,
    pub addr: SocketAddrV4,
    pub info: ServerInfo,
    /// +0x1b: the list's refresh marks it.
    pub visible: bool,
    /// +0x24: `LAN_SaveServersToCache` drops an Internet entry at 3 or
    /// more (0x476592).
    pub misses: u8,
    /// +0x2a, [`NO_PING`] until measured.
    pub ping: u16,
    /// +0x30: an `infoResponse` filled the row.
    pub answered: bool,
}

impl Record {
    pub fn new(addr: SocketAddrV4) -> Record {
        Record {
            adr_type: NA_IP,
            addr,
            info: ServerInfo::default(),
            visible: true,
            misses: 0,
            ping: NO_PING,
            answered: false,
        }
    }

    pub fn read(b: &[u8]) -> Record {
        let s = |at: usize, len: usize| {
            let raw = &b[at..at + len];
            let end = raw.iter().position(|&c| c == 0).unwrap_or(len);
            raw[..end].iter().map(|&c| c as char).collect::<String>()
        };
        let u16_at = |at: usize| u16::from_le_bytes([b[at], b[at + 1]]);
        Record {
            adr_type: i32::from_le_bytes(b[0..4].try_into().unwrap()),
            addr: SocketAddrV4::new(
                Ipv4Addr::new(b[4], b[5], b[6], b[7]),
                u16::from_be_bytes([b[8], b[9]]),
            ),
            info: ServerInfo {
                allow_anonymous: b[0x14],
                password: b[0x15],
                pure: b[0x16],
                con_disabled: b[0x17],
                nettype: b[0x18],
                clients: b[0x19],
                max_clients: b[0x1a],
                ff: b[0x1c],
                kc: b[0x1d],
                hc: b[0x1e],
                od: b[0x1f],
                hw: b[0x20],
                modded: b[0x21],
                voice: b[0x22],
                pb: b[0x23] != 0,
                cod4x: false,
                min_ping: u16_at(0x26),
                max_ping: u16_at(0x28),
                hostname: s(0x31, 32),
                mapname: s(0x51, 32),
                game: s(0x71, 24),
                gametype: s(0x89, 16),
            },
            visible: b[0x1b] != 0,
            misses: b[0x24],
            ping: u16_at(0x2a),
            answered: b[0x30] != 0,
        }
    }

    pub fn write(&self) -> [u8; RECORD] {
        let mut b = [0u8; RECORD];
        b[0..4].copy_from_slice(&self.adr_type.to_le_bytes());
        b[4..8].copy_from_slice(&self.addr.ip().octets());
        b[8..10].copy_from_slice(&self.addr.port().to_be_bytes());
        let i = &self.info;
        for (at, v) in [
            (0x14, i.allow_anonymous),
            (0x15, i.password),
            (0x16, i.pure),
            (0x17, i.con_disabled),
            (0x18, i.nettype),
            (0x19, i.clients),
            (0x1a, i.max_clients),
            (0x1b, u8::from(self.visible)),
            (0x1c, i.ff),
            (0x1d, i.kc),
            (0x1e, i.hc),
            (0x1f, i.od),
            (0x20, i.hw),
            (0x21, i.modded),
            (0x22, i.voice),
            (0x23, u8::from(i.pb)),
            (0x24, self.misses),
            (0x30, u8::from(self.answered)),
        ] {
            b[at] = v;
        }
        for (at, v) in [(0x26, i.min_ping), (0x28, i.max_ping), (0x2a, self.ping)] {
            b[at..at + 2].copy_from_slice(&v.to_le_bytes());
        }
        for (at, len, s) in [
            (0x31, 32, &i.hostname),
            (0x51, 32, &i.mapname),
            (0x71, 24, &i.game),
            (0x89, 16, &i.gametype),
        ] {
            let s = field(s, len);
            let bytes: Vec<u8> = s.chars().map(|c| c as u32 as u8).collect();
            b[at..at + bytes.len()].copy_from_slice(&bytes);
        }
        b
    }
}

/// `servercache.dat` (`LAN_LoadCachedServers` 0x4764d0,
/// `LAN_SaveServersToCache` 0x476540): the version 3, the Internet and
/// favourite counts, the blocks' size 0x2fe980, then 20000 and 128
/// records. The loader refuses a file whose words or block reads differ.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ServerCache {
    pub global: Vec<Record>,
    pub favorites: Vec<Record>,
}

impl ServerCache {
    /// `None` for a file the loader refuses.
    pub fn read(bytes: &[u8]) -> Option<ServerCache> {
        if bytes.len() != CACHE_LEN {
            return None;
        }
        let word = |at: usize| i32::from_le_bytes(bytes[at..at + 4].try_into().unwrap());
        let (globals, favorites) = (word(4) as u32 as usize, word(8) as u32 as usize);
        if word(0) != CACHE_VERSION
            || globals >= MAX_GLOBAL
            || favorites >= MAX_FAVORITES
            || word(12) as usize != BLOCKS
        {
            return None;
        }
        let rec = |at: usize| Record::read(&bytes[at..at + RECORD]);
        Some(ServerCache {
            global: (0..globals).map(|n| rec(HEADER + n * RECORD)).collect(),
            favorites: (0..favorites)
                .map(|n| rec(FAVORITES_AT + n * RECORD))
                .collect(),
        })
    }

    /// The whole file, unused records zero.
    pub fn write(&self) -> Vec<u8> {
        let mut out = vec![0u8; CACHE_LEN];
        self.write_into(&mut out);
        out
    }

    /// Writes `favorites` and their count over `file`, a valid cache or
    /// zeros of [`CACHE_LEN`], every other byte as it was: the Internet
    /// list stays the one retail last saved.
    pub fn write_favorites(favorites: &[Record], file: &mut [u8]) {
        let n = favorites.len().min(MAX_FAVORITES - 1);
        for (at, v) in [(0, CACHE_VERSION), (8, n as i32), (12, BLOCKS as i32)] {
            file[at..at + 4].copy_from_slice(&v.to_le_bytes());
        }
        for (i, r) in favorites.iter().take(n).enumerate() {
            let at = FAVORITES_AT + i * RECORD;
            file[at..at + RECORD].copy_from_slice(&r.write());
        }
    }

    /// Writes the header and both lists over `file`, a valid cache or
    /// zeros of [`CACHE_LEN`]; the records past each count keep their
    /// bytes.
    pub fn write_into(&self, file: &mut [u8]) {
        let globals = self.global.len().min(MAX_GLOBAL - 1);
        let favorites = self.favorites.len().min(MAX_FAVORITES - 1);
        for (at, v) in [
            (0, CACHE_VERSION),
            (4, globals as i32),
            (8, favorites as i32),
            (12, BLOCKS as i32),
        ] {
            file[at..at + 4].copy_from_slice(&v.to_le_bytes());
        }
        for (n, r) in self.global.iter().take(globals).enumerate() {
            let at = HEADER + n * RECORD;
            file[at..at + RECORD].copy_from_slice(&r.write());
        }
        for (n, r) in self.favorites.iter().take(favorites).enumerate() {
            let at = FAVORITES_AT + n * RECORD;
            file[at..at + RECORD].copy_from_slice(&r.write());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The 1.7 server's `infoResponse` from `handshake-1.7.txt`.
    const INFO: &str = "\\challenge\\xxx\\protocol\\6\\hostname\\CoD4Host\\mapname\\mp_crash\\sv_maxclients\\8\\gametype\\war\\pure\\1\\kc\\1\\hw\\1\\mod\\0\\voice\\0\\pb\\0";

    #[test]
    fn info_reads_as_cl_set_server_info_stores_it() {
        assert_eq!(info_protocol(INFO), 6);
        let i = ServerInfo::parse(INFO);
        assert_eq!(
            (i.hostname.as_str(), i.mapname.as_str(), i.gametype.as_str()),
            ("CoD4Host", "mp_crash", "war")
        );
        assert_eq!(
            (i.clients, i.max_clients, i.pure, i.kc, i.hw),
            (0, 8, 1, 1, 1)
        );
        assert!(!i.pb);
        assert!(!i.cod4x);
        // The fields' sizes cut what does not fit.
        let long = ServerInfo::parse(&format!("\\gametype\\{}", "g".repeat(20)));
        assert_eq!(long.gametype.len(), 15);
    }

    /// CoD4x's `getinfo` and `getstatus` answers as cod4-handshake.md 13
    /// lists them; retail 1.7's status sends `shortversion\\1.7`.
    #[test]
    fn cod4x_answers_are_told_apart() {
        let x = "\\protocol\\6\\hostname\\X\\shortversion\\x21\\build\\1";
        assert_eq!(info_protocol(x), 6);
        assert!(info_is_cod4x(x) && ServerInfo::parse(x).cod4x);
        assert!(!info_is_cod4x(INFO));
        let st = Status::parse("\\protocol\\21\\version\\CoD4 X - linux-i386-custom build 1\n");
        assert!(st.is_cod4x());
        let retail = Status::parse("\\gamename\\Call of Duty 4\\protocol\\6\\shortversion\\1.7\n");
        assert!(!retail.is_cod4x());
    }

    #[test]
    fn address_list_stops_at_the_trailer() {
        let mut body = b"\0".to_vec();
        for (ip, port) in [([127, 0, 0, 1], 28960u16), ([127, 0, 0, 1], 28961)] {
            body.push(b'\\');
            body.extend_from_slice(&ip);
            body.extend_from_slice(&port.to_be_bytes());
        }
        body.extend_from_slice(b"\\EOT\0\0\0");
        assert_eq!(
            parse_servers_response(&body),
            [
                "127.0.0.1:28960".parse::<SocketAddrV4>().unwrap(),
                "127.0.0.1:28961".parse().unwrap()
            ]
        );
    }

    #[test]
    fn status_lists_the_players() {
        let s =
            Status::parse("\\sv_hostname\\x\\g_gametype\\war\n0 48 \"walker\"\n12 999 \"a b\"\n");
        assert_eq!(s.info[1], ("g_gametype".into(), "war".into()));
        assert_eq!(s.players.len(), 2);
        assert_eq!(s.players[1].name, "a b");
        assert_eq!(s.players[1].score, 12);
    }

    #[test]
    fn the_cache_round_trips() {
        let mut fav = Record::new("127.0.0.1:28960".parse().unwrap());
        fav.info = ServerInfo::parse(INFO);
        fav.ping = 12;
        fav.answered = true;
        let cache = ServerCache {
            global: vec![Record::new("10.0.0.1:28961".parse().unwrap())],
            favorites: vec![fav],
        };
        let bytes = cache.write();
        assert_eq!(bytes.len(), 3_139_984);
        assert_eq!(&bytes[12..16], &0x2fe980i32.to_le_bytes());
        // The port in network order at +8.
        let f = FAVORITES_AT;
        assert_eq!(&bytes[f + 4..f + 10], &[127, 0, 0, 1, 0x71, 0x20]);
        assert_eq!(ServerCache::read(&bytes), Some(cache));
        let mut bad = bytes.clone();
        bad[0] = 2;
        assert_eq!(ServerCache::read(&bad), None);
        // The favourites alone: the Internet list stays as it was.
        let mut file = bytes.clone();
        ServerCache::write_favorites(&[], &mut file);
        let back = ServerCache::read(&file).unwrap();
        assert_eq!((back.global.len(), back.favorites.len()), (1, 0));
    }
}
