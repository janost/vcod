//! The server browser's half of the master protocol: the `getservers` query,
//! the `getserversResponse` address list and the `infoResponse` fields a
//! list row shows. Measured in docs/research/cod11-front-end.md, "Master
//! query".

use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, SocketAddrV4};
use std::sync::{Mutex, OnceLock};
use std::time::Instant;

use super::connectionless::info_value_for_key;

/// The master vcod lists on and asks, for every game: cod.pm's, used with
/// its operator's permission. Retail's own masters (`codmaster`,
/// `coduomaster`, `cod2master.activision.com`) are documented in the
/// research docs and never contacted.
pub const MASTER_HOST: &str = "master.cod.pm";

/// A master server: its host and the port one game's list lives on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Master {
    pub host: &'static str,
    pub port: u16,
}

/// cod.pm's ports: CoD1 1.1 to 1.5 (protocols 1 to 6), United Offensive
/// (21, 22), CoD2 (115, 117, 118), CoD4.
pub const COD1_MASTER: Master = Master {
    host: MASTER_HOST,
    port: 20510,
};
pub const UO_MASTER: Master = Master {
    host: MASTER_HOST,
    port: 20610,
};
pub const COD2_MASTER: Master = Master {
    host: MASTER_HOST,
    port: 20710,
};
pub const COD4_MASTER: Master = Master {
    host: MASTER_HOST,
    port: 20810,
};

/// The query the stock browser's refresh ends up sending: the UI runs
/// `globalservers 0 <protocol> full empty` (ui_mp_x86.dll 0x4000ebe4; CoD2
/// `0x76`, cod2-protocol.md "The master") and `CL_GlobalServers_f` sends
/// `getservers` with the arguments past the master number. vcod asks for
/// exactly one protocol, the install's; protocol 0 would list every game's
/// servers and is never sent.
pub fn getservers(protocol: u32) -> String {
    assert_ne!(protocol, 0, "getservers 0 lists every protocol");
    format!("getservers {protocol} full empty")
}

/// The fewest milliseconds between two packets to one master host.
pub const MASTER_INTERVAL_MS: u64 = 1000;

/// At most one packet per [`MASTER_INTERVAL_MS`] to each master host, on a
/// clock the caller passes in milliseconds.
#[derive(Debug, Default)]
pub struct RateLimiter {
    last: HashMap<IpAddr, u64>,
}

impl RateLimiter {
    /// Whether a packet to `host` may go at `now_ms`; a yes counts as sent.
    pub fn allow(&mut self, host: IpAddr, now_ms: u64) -> bool {
        match self.last.get(&host) {
            Some(&t) if now_ms < t.saturating_add(MASTER_INTERVAL_MS) => false,
            _ => {
                self.last.insert(host, now_ms);
                true
            }
        }
    }
}

/// [`RateLimiter::allow`] on the process's one limiter and the monotonic
/// clock: every master packet the client sends asks this first.
pub fn master_send_allowed(host: IpAddr) -> bool {
    static START: OnceLock<Instant> = OnceLock::new();
    static LIMIT: OnceLock<Mutex<RateLimiter>> = OnceLock::new();
    let now = Instant::now().duration_since(*START.get_or_init(Instant::now));
    LIMIT
        .get_or_init(Mutex::default)
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .allow(host, now.as_millis() as u64)
}

/// The ping request (`CL_Ping_f`'s `getinfo xxx`, CoDMP.exe 0x5660e0).
pub const GETINFO: &str = "getinfo xxx";

/// `CL_LocalServers_f` (CoDMP.exe 0x413710) broadcasts `getinfo xxx` to
/// each of these ports, in two rounds.
pub const LAN_PORTS: [u16; 4] = [28960, 28961, 28962, 28963];
pub const LAN_ROUNDS: usize = 2;

/// The server info popup's request (`CL_ServerStatus`, CoDMP.exe
/// 0x4133eb), sent with no argument.
pub const GETSTATUS: &str = "getstatus";

/// `PORT_SERVER`, the port an address without one gets.
pub const PORT_SERVER: u16 = 28960;

/// `NET_StringToAdr` for an IPv4 address or host name with an optional
/// `:port`. A name resolves here, blocking, as it does in retail.
pub fn parse_address(s: &str) -> Option<SocketAddrV4> {
    use std::net::ToSocketAddrs;
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    let (host, port) = match s.rsplit_once(':') {
        Some((h, p)) => (h, p.parse().ok()?),
        None => (s, PORT_SERVER),
    };
    if let Ok(ip) = host.parse::<Ipv4Addr>() {
        return Some(SocketAddrV4::new(ip, port));
    }
    (host, port).to_socket_addrs().ok()?.find_map(|a| match a {
        std::net::SocketAddr::V4(v4) => Some(v4),
        _ => None,
    })
}

/// `CL_ServersResponsePacket` (CoDMP.exe 0x4107b0) over the bytes after the
/// command word: every `\` followed by six address bytes and another `\` is
/// one server, port big-endian. It stops at `\EOT`, at 256 entries, or when
/// fewer than seven bytes are left; the live master ends its list with
/// `\EOF`, which the length check catches.
pub fn parse_servers_response(body: &[u8]) -> Vec<SocketAddrV4> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < body.len() {
        if body[i] != b'\\' {
            i += 1;
            continue;
        }
        if i + 7 >= body.len() {
            break;
        }
        let b = &body[i + 1..i + 7];
        if body[i + 7] != b'\\' {
            break;
        }
        let ip = Ipv4Addr::new(b[0], b[1], b[2], b[3]);
        let port = u16::from_be_bytes([b[4], b[5]]);
        out.push(SocketAddrV4::new(ip, port));
        if out.len() >= 256 || body.get(i + 8..i + 11) == Some(b"EOT") {
            break;
        }
        i += 7;
    }
    out
}

/// The `infoResponse` keys a browser row shows; CoDMP.exe's info packet
/// handler reads the same names (string refs at 0x412aa1..0x412b0d).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ServerInfo {
    pub hostname: String,
    pub mapname: String,
    pub gametype: String,
    pub clients: u32,
    pub max_clients: u32,
    pub protocol: u32,
    pub password: bool,
    /// 1.5's `pb`: the server runs PunkBuster and refuses a client without it.
    pub punkbuster: bool,
    /// 1.5's `hw`: 1 a Linux, 2 a Windows dedicated server.
    pub hardware: i32,
    /// 1.5's `mod`.
    pub modded: bool,
    /// CoD2's `voice`: the server runs voice chat.
    pub voice: bool,
    /// CoD2's `pure`, absent on an unpure server (cod2-protocol.md 9).
    pub pure: bool,
}

impl ServerInfo {
    /// The info string after `infoResponse\n`.
    pub fn parse(info: &str) -> ServerInfo {
        let s = |k| info_value_for_key(info, k).unwrap_or("").to_string();
        let n = |k| info_value_for_key(info, k).and_then(|v| v.parse().ok());
        ServerInfo {
            hostname: s("hostname"),
            mapname: s("mapname"),
            gametype: s("gametype"),
            clients: n("clients").unwrap_or(0),
            max_clients: n("sv_maxclients").unwrap_or(0),
            protocol: n("protocol").unwrap_or(0),
            password: n("pswrd").unwrap_or(0) != 0,
            punkbuster: n("pb").unwrap_or(0) != 0,
            hardware: info_value_for_key(info, "hw")
                .and_then(|v| v.parse().ok())
                .unwrap_or(0),
            modded: n("mod").unwrap_or(0) != 0,
            voice: n("voice").unwrap_or(0) != 0,
            pure: n("pure").unwrap_or(0) != 0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn addresses_take_the_server_port_by_default() {
        assert_eq!(
            parse_address("10.1.2.3"),
            Some("10.1.2.3:28960".parse().unwrap())
        );
        assert_eq!(
            parse_address(" 10.1.2.3:29661 "),
            Some("10.1.2.3:29661".parse().unwrap())
        );
        assert_eq!(parse_address("10.1.2.3:x"), None);
        assert_eq!(parse_address(""), None);
    }

    #[test]
    fn address_list_stops_at_the_master_trailer() {
        // The live master's shape: a `\n\0` after the command word, then
        // `\` + 4 + 2 bytes per server, then `\EOF`.
        let mut body = b"\n\0".to_vec();
        for (ip, port) in [([203, 0, 113, 10], 20600u16), ([198, 51, 100, 20], 28960)] {
            body.push(b'\\');
            body.extend_from_slice(&ip);
            body.extend_from_slice(&port.to_be_bytes());
        }
        body.extend_from_slice(b"\\EOF");
        let list = parse_servers_response(&body);
        assert_eq!(
            list,
            vec![
                "203.0.113.10:20600".parse().unwrap(),
                "198.51.100.20:28960".parse().unwrap()
            ]
        );
    }

    #[test]
    fn eot_ends_the_list_early() {
        let mut body = vec![b'\\', 1, 2, 3, 4, 0, 80];
        body.extend_from_slice(b"\\EOT\0\0\0\\");
        assert_eq!(parse_servers_response(&body).len(), 1);
        assert!(parse_servers_response(b"\\\x01\x02").is_empty());
    }

    #[test]
    fn one_packet_a_second_to_each_master_host() {
        let (a, b): (IpAddr, IpAddr) = ("10.0.0.1".parse().unwrap(), "10.0.0.2".parse().unwrap());
        let mut r = RateLimiter::default();
        assert!(r.allow(a, 5000));
        assert!(!r.allow(a, 5000));
        assert!(!r.allow(a, 5999));
        // Another host keeps its own budget.
        assert!(r.allow(b, 5500));
        assert!(r.allow(a, 6000));
        assert!(!r.allow(b, 6499));
        assert!(r.allow(b, 6500));
    }

    #[test]
    fn the_query_names_one_protocol() {
        assert_eq!(getservers(118), "getservers 118 full empty");
        assert_eq!(getservers(1), "getservers 1 full empty");
    }

    #[test]
    #[should_panic]
    fn protocol_0_is_never_asked() {
        getservers(0);
    }

    #[test]
    fn info_fields() {
        let i = ServerInfo::parse(
            "\\challenge\\xxx\\protocol\\1\\hostname\\^1Sample TDM\\mapname\\mp_powcamp\\clients\\12\\sv_maxclients\\64\\gametype\\tdm\\pure\\0\\pswrd\\1",
        );
        assert_eq!(i.hostname, "^1Sample TDM");
        assert_eq!((i.clients, i.max_clients, i.protocol), (12, 64, 1));
        assert_eq!(i.gametype, "tdm");
        assert!(i.password);
        assert!(!i.punkbuster && !i.modded);
        // The 1.5 server under wine with PunkBuster on, 2026-10-13.
        let i = ServerInfo::parse(
            "\\challenge\\x\\protocol\\6\\hostname\\CoDHost\\mapname\\mp_carentan\\sv_maxclients\\20\\gametype\\tdm\\kc\\1\\hw\\2\\pb\\1\\mod\\1",
        );
        assert_eq!((i.protocol, i.hardware), (6, 2));
        assert!(i.punkbuster && i.modded && !i.password);
    }
}
