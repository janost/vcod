//! The server console: the three commands the map cycle is made of, and
//! `sv_mapRotation`'s token grammar. Numbers and addresses are in
//! docs/research/cod11-map-cycle.md sections 3 to 5.

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

    #[test]
    fn console_lines_parse() {
        assert_eq!(
            Command::parse("map mp_ship\n"),
            Command::Map("mp_ship".into())
        );
        assert_eq!(Command::parse("map_restart"), Command::MapRestart);
        assert_eq!(Command::parse("MAP_ROTATE"), Command::MapRotate);
        assert_eq!(
            Command::parse("vstr nextmap"),
            Command::Unknown("vstr nextmap".into())
        );
    }
}
