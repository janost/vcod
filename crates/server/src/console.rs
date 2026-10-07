//! The server console: the three commands the map cycle is made of, the
//! handful rcon is for, and `sv_mapRotation`'s token grammar. Numbers and
//! addresses are in docs/research/cod11-map-cycle.md sections 3 to 5 and
//! docs/research/cod11-server-handshake.md, "rcon".

use std::collections::VecDeque;

/// `SNAPFLAG_SERVERCOUNT`, toggled by every map load and restart (doc 3
/// step 13, doc 4 step 4).
pub const SNAPFLAG_SERVERCOUNT: u32 = 4;

/// `sv_serverid` after a map load: the high nibble climbs by one and skips
/// zero on wrap, the low nibble (restarts) is kept (doc 3, step 16).
pub fn next_map_id(id: u8) -> u8 {
    let mut n = id.wrapping_add(0x10);
    if n & 0xf0 == 0 {
        n = n.wrapping_add(0x10);
    }
    n
}

/// `sv_serverid` after a restart: only the low nibble climbs (doc 4, step 5).
pub fn next_restart_id(id: u8) -> u8 {
    (id & 0xf0) | (id.wrapping_add(1) & 0x0f)
}

#[derive(Debug, PartialEq, Eq)]
pub enum Command {
    Map(String),
    MapRestart,
    MapRotate,
    /// `SV_Status_f` (0x80846b4).
    Status,
    /// `SV_KickNum_f` (0x8084be4); `None` when the argument count is not
    /// exactly one, which prints the usage line.
    ClientKick(Option<String>),
    /// `SV_Heartbeat_f` (0x8084bd0).
    Heartbeat,
    /// `quit`: `SV_Shutdown`, which flatlines the masters, then exit.
    Quit,
    Unknown(String),
}

impl Command {
    /// `Cmd_TokenizeString` plus the dispatch `SV_Map_f` /
    /// `SV_MapRestart_f` / `SV_MapRotate_f` register themselves under.
    pub fn parse(line: &str) -> Command {
        let line = line.trim();
        let mut it = line.split_whitespace();
        match it.next().map(|w| w.to_ascii_lowercase()).as_deref() {
            Some("map") => match it.next() {
                Some(m) => Command::Map(m.to_string()),
                None => Command::Unknown(line.to_string()),
            },
            Some("map_restart") => Command::MapRestart,
            Some("map_rotate") => Command::MapRotate,
            Some("status") => Command::Status,
            Some("clientkick") => match (it.next(), it.next()) {
                (Some(n), None) => Command::ClientKick(Some(n.to_string())),
                _ => Command::ClientKick(None),
            },
            Some("heartbeat") => Command::Heartbeat,
            Some("quit") => Command::Quit,
            _ => Command::Unknown(line.to_string()),
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum Rotate {
    Map(String),
    Restart,
}

/// `sv_mapRotationCurrent`: a queue eaten one token at a time, refilled from
/// `sv_mapRotation` when empty (doc 5).
#[derive(Default, Debug)]
pub struct Rotation {
    current: String,
    pub warnings: Vec<String>,
}

impl Rotation {
    /// `NextToken` (doc 5.1): destructive, one token per call.
    fn next_token(&mut self) -> Option<String> {
        let trimmed = self.current.trim_start();
        let mut it = trimmed.splitn(2, char::is_whitespace);
        let tok = it.next().filter(|t| !t.is_empty())?.to_string();
        self.current = it.next().unwrap_or("").to_string();
        Some(tok)
    }

    /// `SV_MapRotate_f`'s loop (doc 5.2): returns the map to load, or
    /// `Restart` when the rotation names none. A `gametype` token writes
    /// `gametype` and the caller clears its stashed `savePersist` when the
    /// value changed.
    pub fn rotate(&mut self, full: &str, gametype: &mut String) -> Rotate {
        if self.current.trim().is_empty() {
            self.current = full.to_string();
        }
        let mut tok = self.next_token();
        if tok.is_none() {
            self.current = full.to_string();
            tok = self.next_token();
        }
        let Some(mut tok) = tok else {
            self.warnings
                .push("No map specified in sv_mapRotation - forcing map_restart.".into());
            return Rotate::Restart;
        };
        loop {
            match tok.to_ascii_lowercase().as_str() {
                "gametype" => match self.next_token() {
                    Some(g) => *gametype = g,
                    None => {
                        self.warnings.push(
                            "No gametype specified after 'gametype' keyword in sv_mapRotation - forcing map_restart."
                                .into(),
                        );
                        return Rotate::Restart;
                    }
                },
                "map" => match self.next_token() {
                    Some(m) => return Rotate::Map(m),
                    None => {
                        self.warnings.push(
                            "No map specified after 'map' keyword in sv_mapRotation - forcing map_restart."
                                .into(),
                        );
                        return Rotate::Restart;
                    }
                },
                _ => self
                    .warnings
                    .push(format!("Unknown keyword '{tok}' in sv_mapRotation.")),
            }
            match self.next_token() {
                Some(t) => tok = t,
                None => {
                    self.warnings
                        .push("No map specified in sv_mapRotation - forcing map_restart.".into());
                    return Rotate::Restart;
                }
            }
        }
    }
}

/// `SV_Status_f`'s ping column.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ping {
    Connecting,
    Zombie,
    Ms(i32),
}

/// One row of `status`, in `SV_Status_f`'s format (0x80846b4, strings at
/// 0x80d3f5c..0x80d3fa8).
pub struct StatusRow<'a> {
    pub num: usize,
    pub score: i32,
    pub ping: Ping,
    pub name: &'a str,
    pub last_msg_ms: i64,
    pub addr: std::net::SocketAddr,
    pub qport: u16,
    pub rate: i32,
}

impl std::fmt::Display for StatusRow<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:3} {:5} ", self.num, self.score)?;
        match self.ping {
            Ping::Connecting => f.write_str("CNCT ")?,
            Ping::Zombie => f.write_str("ZMBI ")?,
            Ping::Ms(ms) => write!(f, "{:4} ", ms.min(9999))?,
        }
        // `NET_AdrToString` prints the port as a signed short.
        let addr = format!("{}:{}", self.addr.ip(), self.addr.port() as i16);
        writeln!(
            f,
            "{:<16}{:7} {:<22}{:5} {:5}",
            self.name, self.last_msg_ms, addr, self.qport, self.rate
        )
    }
}

/// The lines a builtin queued and `Server::drain_console` eats, one per
/// `tick` (`Cbuf_Execute`).
pub type Console = VecDeque<String>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_map_id_bumps_the_high_nibble_and_skips_zero() {
        assert_eq!(next_map_id(0x10), 0x20);
        assert_eq!(next_map_id(0x13), 0x23); // restarts in the low nibble survive
        assert_eq!(next_map_id(0xf0), 0x10); // 0xf0 + 0x10 wraps to 0x00, then +0x10
    }

    #[test]
    fn a_restart_id_bumps_only_the_low_nibble() {
        assert_eq!(next_restart_id(0x10), 0x11);
        assert_eq!(next_restart_id(0x1f), 0x10);
    }

    #[test]
    fn map_rotate_consumes_one_map_per_call_and_wraps() {
        let mut r = Rotation::default();
        let mut gt = "dm".to_string();
        let full = "gametype tdm map mp_carentan map mp_brecourt";
        assert_eq!(r.rotate(full, &mut gt), Rotate::Map("mp_carentan".into()));
        assert_eq!(gt, "tdm");
        assert_eq!(r.rotate(full, &mut gt), Rotate::Map("mp_brecourt".into()));
        // The queue is empty: refill once from the full list.
        assert_eq!(r.rotate(full, &mut gt), Rotate::Map("mp_carentan".into()));
    }

    #[test]
    fn map_rotate_without_a_map_falls_back_to_restart() {
        let mut r = Rotation::default();
        let mut gt = "dm".to_string();
        assert_eq!(r.rotate("", &mut gt), Rotate::Restart);
        assert_eq!(r.rotate("gametype tdm", &mut gt), Rotate::Restart);
        assert_eq!(gt, "tdm");
    }

    #[test]
    fn map_rotate_warns_on_an_unknown_keyword_and_continues() {
        let mut r = Rotation::default();
        let mut gt = "dm".to_string();
        assert_eq!(
            r.rotate("bogus map mp_ship", &mut gt),
            Rotate::Map("mp_ship".into())
        );
        assert_eq!(
            r.warnings,
            vec!["Unknown keyword 'bogus' in sv_mapRotation.".to_string()]
        );
    }

    /// Rows from a retail capture (docs/research/cod11-server-handshake.md,
    /// "rcon").
    #[test]
    fn status_rows_match_retail() {
        let row = |ping, name, last_msg_ms| {
            StatusRow {
                num: 0,
                score: 0,
                ping,
                name,
                last_msg_ms,
                addr: "127.0.0.1:48014".parse().unwrap(),
                qport: 12038,
                rate: 25000,
            }
            .to_string()
        };
        assert_eq!(
            row(Ping::Ms(0), "vcod", 0),
            "  0     0    0 vcod                  0 127.0.0.1:-17522      12038 25000\n"
        );
        assert_eq!(
            row(Ping::Zombie, "", 1550),
            "  0     0 ZMBI                    1550 127.0.0.1:-17522      12038 25000\n"
        );
    }

    #[test]
    fn console_lines_parse() {
        assert_eq!(
            Command::parse("map mp_ship\n"),
            Command::Map("mp_ship".into())
        );
        assert_eq!(Command::parse("map_restart"), Command::MapRestart);
        assert_eq!(Command::parse("MAP_ROTATE"), Command::MapRotate);
        assert_eq!(
            Command::parse("clientkick 3 "),
            Command::ClientKick(Some("3".into()))
        );
        assert_eq!(Command::parse("clientkick"), Command::ClientKick(None));
        assert_eq!(Command::parse("clientkick 1 2"), Command::ClientKick(None));
        assert_eq!(
            Command::parse("vstr nextmap"),
            Command::Unknown("vstr nextmap".into())
        );
    }
}
