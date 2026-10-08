//! The server browser's half of the master protocol: the `getservers` query,
//! the `getserversResponse` address list and the `infoResponse` fields a
//! list row shows. Measured in docs/research/cod11-front-end.md, "Master
//! query".

use std::net::{Ipv4Addr, SocketAddrV4};

use super::connectionless::info_value_for_key;

/// `CL_GlobalServers_f` (CoDMP.exe 0x413890) always asks this host, on
/// `PORT_MASTER`.
pub const MASTER_HOST: &str = "codmaster.activision.com";
pub const MASTER_PORT: u16 = 20510;

/// What the stock browser's refresh ends up sending: the UI runs
/// `globalservers 0 <protocol> full empty` (ui_mp_x86.dll 0x4000ebe4) and
/// `CL_GlobalServers_f` sends `getservers` with the arguments past the master
/// number.
pub const GETSERVERS: &str = "getservers 1 full empty";

/// The ping request (`CL_Ping_f`'s `getinfo xxx`, CoDMP.exe 0x5660e0).
pub const GETINFO: &str = "getinfo xxx";

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
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn address_list_stops_at_the_master_trailer() {
        // The live master's shape: a `\n\0` after the command word, then
        // `\` + 4 + 2 bytes per server, then `\EOF`.
        let mut body = b"\n\0".to_vec();
        for (ip, port) in [([15, 206, 117, 161], 20600u16), ([51, 195, 89, 86], 28960)] {
            body.push(b'\\');
            body.extend_from_slice(&ip);
            body.extend_from_slice(&port.to_be_bytes());
        }
        body.extend_from_slice(b"\\EOF");
        let list = parse_servers_response(&body);
        assert_eq!(
            list,
            vec![
                "15.206.117.161:20600".parse().unwrap(),
                "51.195.89.86:28960".parse().unwrap()
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
    fn info_fields() {
        let i = ServerInfo::parse(
            "\\challenge\\xxx\\protocol\\1\\hostname\\^1Revive TDM\\mapname\\mp_powcamp\\clients\\12\\sv_maxclients\\64\\gametype\\tdm\\pure\\0\\pswrd\\1",
        );
        assert_eq!(i.hostname, "^1Revive TDM");
        assert_eq!((i.clients, i.max_clients, i.protocol), (12, 64, 1));
        assert_eq!(i.gametype, "tdm");
        assert!(i.password);
    }
}
