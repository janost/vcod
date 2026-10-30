//! `ban.txt` and the temporary bans. 1.5b, UO 1.51b and CoD2 1.3 keep
//! the same file, a `%i %s\r\n` guid and cleaned name a line, read whole
//! at every check, and the same 16-entry temporary table `tempBan*`
//! (CoD2's `kick`) fills (cod11-server-handshake.md, "1.5 permanent
//! bans"; cod2-protocol.md 17.7). Addresses are CoD2 1.3's, 1.5b's
//! beside them. 1.1d keeps no file: its bans go to the authorize server,
//! and vcod's stand-in for that list is this file too.
//!
//! Retail's guid comes from the authorize server's `ipAuthorize accept`,
//! which vcod never asks: an off-LAN client's IPv4 address stands in for it
//! ([`guid_of`]), and a LAN client has 0, as on retail.

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::PathBuf;

use crate::engine::Lan;

/// The temporary table's size (CoD2 `FUN_0808d6ce`'s `0xf <` bound, 1.5b
/// 0x8089096's loop over 0x83e1e0c).
pub const TEMP_BANS: usize = 16;

/// The guid vcod gives a client from `addr`: 0 on the game's LAN, where
/// retail never asks the authorize server (CoD2 `SV_GetChallenge`
/// 0x808d18e, 1.5b 0x80889d0), else the IPv4 address read as a big-endian
/// `i32`.
pub fn guid_of(lan: Lan, addr: SocketAddr) -> i32 {
    match addr.ip() {
        IpAddr::V4(v4) if !lan.is_lan(addr.ip()) => u32::from(v4) as i32,
        _ => 0,
    }
}

/// `ban.txt`. With a path every operation reads and writes the file, as
/// retail does; without one the text lives for the run. `None` text is
/// retail's missing file.
#[derive(Default)]
pub struct BanFile {
    path: Option<PathBuf>,
    text: Option<String>,
}

impl BanFile {
    pub fn at(path: PathBuf) -> BanFile {
        BanFile {
            path: Some(path),
            text: None,
        }
    }

    /// [`BanFile::at`], after rewriting the lines of vcod's old CoD1 list
    /// (`<ip> <name>`, before 2026-10-23) as `%i %s\r\n` with the address's
    /// guid, so they keep holding. Other lines are left as they are. The
    /// old file is kept beside it as `<name>.old` before the first rewrite.
    pub fn open(path: PathBuf) -> BanFile {
        let mut f = BanFile::at(path);
        if let Some(text) = f.read()
            && let Some(new) = migrate(&text)
        {
            if let Some(p) = &f.path {
                let mut old = p.clone().into_os_string();
                old.push(".old");
                if let Err(e) = std::fs::copy(p, &old) {
                    log::error!("keeping {} as {}: {e}", p.display(), old.to_string_lossy());
                    return f;
                }
            }
            log::info!("ban file: rewrote vcod's old `<ip> <name>` lines as guids");
            f.write(new);
        }
        f
    }

    fn read(&self) -> Option<String> {
        match &self.path {
            Some(p) => std::fs::read(p)
                .ok()
                .map(|b| b.iter().map(|&c| char::from(c)).collect()),
            None => self.text.clone(),
        }
    }

    fn write(&mut self, text: String) {
        match &self.path {
            Some(p) => {
                let bytes: Vec<u8> = text.chars().map(|c| c as u32 as u8).collect();
                if let Err(e) = std::fs::write(p, bytes) {
                    log::error!("writing {}: {e}", p.display());
                }
            }
            None => self.text = Some(text),
        }
    }

    /// `SV_IsBannedGuid` (0x808d630): 0 is never banned; otherwise each
    /// line's first token, `atoi`'d, against `guid`, the rest of the line
    /// skipped.
    pub fn is_banned(&self, guid: i32) -> bool {
        if guid == 0 {
            return false;
        }
        let Some(text) = self.read() else {
            return false;
        };
        let b = text.as_bytes();
        let mut pos = 0;
        while let Some(tok) = parse_token(b, &mut pos) {
            if crate::game::builtins::cvar::atoi(tok) == guid {
                return true;
            }
            skip_rest_of_line(b, &mut pos);
        }
        false
    }

    /// CoD4 1.7's `SV_IsBannedGuid` (0x816ec34): an empty GUID is never
    /// banned; otherwise each line's first token against `guid` with
    /// `strcmp`, the rest of the line skipped.
    pub fn lists_guid(&self, guid: &str) -> bool {
        if guid.is_empty() {
            return false;
        }
        let Some(text) = self.read() else {
            return false;
        };
        let b = text.as_bytes();
        let mut pos = 0;
        while let Some(tok) = parse_token(b, &mut pos) {
            if tok == guid {
                return true;
            }
            skip_rest_of_line(b, &mut pos);
        }
        false
    }

    /// `SV_BanClient`'s append (0x808d75a): `%i %s\r\n`, the name cleaned.
    pub fn append(&mut self, guid: i32, name: &str) {
        self.append_line(&guid.to_string(), name);
    }

    /// The append with the GUID as text: CoD4 1.7's `%s %s\r\n`
    /// (0x823137f).
    pub fn append_line(&mut self, guid: &str, name: &str) {
        let mut text = self.read().unwrap_or_default();
        text.push_str(&format!("{guid} {name}\r\n"));
        self.write(text);
    }

    /// `SV_UnbanClient` (0x808d886): each line whose text past the guid and
    /// the blanks after it is `name`, ignoring case, up to a `\r` or `\n`,
    /// is cut out; the file is written back either way. `None` without a
    /// file, else the count cut.
    pub fn unban(&mut self, name: &str) -> Option<usize> {
        let text = self.read()?;
        let mut b = text.into_bytes();
        let want = name.as_bytes();
        let (mut pos, mut n) = (0, 0);
        loop {
            let start = pos;
            if parse_token(&b, &mut pos).is_none() {
                break;
            }
            while pos < b.len() && b[pos] < b'!' {
                pos += 1;
            }
            let rest = &b[pos..];
            let hit = rest.len() > want.len()
                && rest[..want.len()].eq_ignore_ascii_case(want)
                && matches!(rest[want.len()], b'\r' | b'\n');
            skip_rest_of_line(&b, &mut pos);
            if hit {
                n += 1;
                b.drain(start..pos);
                pos = start;
            }
        }
        self.write(b.iter().map(|&c| char::from(c)).collect());
        Some(n)
    }
}

/// `text` with each line that starts with a dotted quad written as the
/// guid line [`BanFile::append`] makes; `None` when there is none.
fn migrate(text: &str) -> Option<String> {
    let mut out = String::with_capacity(text.len());
    let mut any = false;
    for line in text.split_inclusive('\n') {
        let body = line.trim();
        let (first, name) = body.split_once(char::is_whitespace).unwrap_or((body, ""));
        match first.parse::<Ipv4Addr>() {
            Ok(ip) => {
                any = true;
                out.push_str(&format!("{} {}\r\n", u32::from(ip) as i32, name.trim()));
            }
            Err(_) => out.push_str(line),
        }
    }
    any.then_some(out)
}

/// `Com_Parse` over `b` from `pos`: blanks and `//` comments skipped, then
/// a quoted string or a run of printable characters.
fn parse_token<'a>(b: &'a [u8], pos: &mut usize) -> Option<&'a str> {
    loop {
        while *pos < b.len() && b[*pos] <= b' ' {
            *pos += 1;
        }
        if b[*pos..].starts_with(b"//") {
            skip_rest_of_line(b, pos);
            continue;
        }
        break;
    }
    if *pos >= b.len() {
        return None;
    }
    let start = *pos;
    let tok = if b[start] == b'"' {
        *pos += 1;
        let s = *pos;
        while *pos < b.len() && b[*pos] != b'"' {
            *pos += 1;
        }
        let t = &b[s..*pos];
        *pos = (*pos + 1).min(b.len());
        t
    } else {
        while *pos < b.len() && b[*pos] > b' ' {
            *pos += 1;
        }
        &b[start..*pos]
    };
    std::str::from_utf8(tok).ok().or(Some(""))
}

/// `Com_SkipRestOfLine` (0x80b6f90): past the next `\n`.
fn skip_rest_of_line(b: &[u8], pos: &mut usize) {
    while *pos < b.len() {
        *pos += 1;
        if b[*pos - 1] == b'\n' {
            break;
        }
    }
}

/// The temporary bans at 0x844010c: a guid and the `svs.time` it was put
/// in, 0 a free entry.
#[derive(Default)]
pub struct TempBans {
    entries: [(i32, i32); TEMP_BANS],
}

impl TempBans {
    /// `FUN_0808d72e`: the first free entry, else the oldest
    /// (`FUN_0808d6ce`).
    pub fn add(&mut self, guid: i32, now: i32) {
        let mut oldest = 0;
        let mut at = None;
        for (i, &(g, t)) in self.entries.iter().enumerate() {
            if g == 0 {
                at = Some(i);
                break;
            }
            if t < self.entries[oldest].1 {
                oldest = i;
            }
        }
        self.entries[at.unwrap_or(oldest)] = (guid, now);
    }

    /// `FUN_0808d5ac`: an entry for a non-zero `guid` whose age is not
    /// above `sv_kickBanTime` seconds, the dvar read now.
    pub fn banned(&self, guid: i32, now: i32, kick_ban_time: f32) -> bool {
        guid != 0
            && self
                .entries
                .iter()
                .any(|&(g, t)| g == guid && now.wrapping_sub(t) as f32 <= 1000.0 * kick_ban_time)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_file_holds_guids_and_unbans_by_name() {
        let mut f = BanFile::default();
        assert!(!f.is_banned(12345));
        assert_eq!(f.unban("x"), None);
        f.append(12345, "Foo");
        f.append(67890, "Bar");
        assert!(f.is_banned(12345) && f.is_banned(67890) && !f.is_banned(0));
        assert_eq!(f.unban("foo"), Some(1));
        assert_eq!(f.text.as_deref(), Some("67890 Bar\r\n"));
        assert!(!f.is_banned(12345));
        assert_eq!(f.unban("foo"), Some(0));
    }

    /// The 1.5b walk this one shares (handshake doc, "1.5 permanent
    /// bans"): both dups go, a last line with no line end is missed, blanks
    /// after the guid are skipped.
    #[test]
    fn unban_walks_the_lines_as_retail() {
        let mut f = BanFile {
            path: None,
            text: Some("1 Dup\n2 dup\r\n5   Spaced Name\r\n4 Baz".into()),
        };
        assert_eq!(f.unban("dup"), Some(2));
        assert_eq!(f.unban("spaced name"), Some(1));
        assert_eq!(f.unban("baz"), Some(0));
        assert_eq!(f.text.as_deref(), Some("4 Baz"));
        assert!(f.is_banned(4));
    }

    #[test]
    fn temp_bans_age_out_and_reuse_the_oldest() {
        let mut t = TempBans::default();
        t.add(7, 1000);
        assert!(t.banned(7, 301_000, 300.0));
        assert!(!t.banned(7, 301_001, 300.0));
        assert!(!t.banned(0, 1000, 300.0));
        for g in 100..115 {
            t.add(g, 2000 + g);
        }
        // Full: the next goes over guid 7's, the oldest.
        t.add(200, 5000);
        assert!(!t.banned(7, 5000, 300.0) && t.banned(200, 5000, 300.0));
        assert!(t.banned(100, 5000, 300.0));
    }

    #[test]
    fn a_lan_client_has_no_guid() {
        for lan in [Lan::Cod1, Lan::Cod2] {
            assert_eq!(guid_of(lan, "192.168.1.5:28960".parse().unwrap()), 0);
            assert_eq!(guid_of(lan, "127.0.0.1:28960".parse().unwrap()), 0);
            assert_eq!(
                guid_of(lan, "203.0.113.7:28960".parse().unwrap()),
                0xcb00_7107u32 as i32
            );
        }
    }

    /// Retail's `ban.txt` round trip: what `append` writes, a fresh
    /// `BanFile` on the same path reads, `\r\n` lines included.
    #[test]
    fn the_file_round_trips_on_disk() {
        let dir = std::env::temp_dir().join(format!("vcod-banfile-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("ban.txt");
        let _ = std::fs::remove_file(&path);
        let mut f = BanFile::open(path.clone());
        assert_eq!(f.unban("x"), None);
        f.append(-889163513, "Some One");
        f.append(42, "Two");
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "-889163513 Some One\r\n42 Two\r\n"
        );
        let mut again = BanFile::open(path.clone());
        assert!(again.is_banned(-889163513) && again.is_banned(42));
        assert_eq!(again.unban("some one"), Some(1));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "42 Two\r\n");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// A list vcod wrote as `<ip> <name>` before the guid file is read on,
    /// its addresses as guids; comments and guid lines stay as they are.
    #[test]
    fn an_old_address_list_becomes_guid_lines() {
        let dir = std::env::temp_dir().join(format!("vcod-banmig-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("ban.txt");
        std::fs::write(
            &path,
            "// banned\n10.1.2.3\n203.0.113.7  Some One\r\n42 Guid\r\n",
        )
        .unwrap();
        let f = BanFile::open(path.clone());
        let ip = |s: &str| guid_of(Lan::Cod1, format!("{s}:1").parse().unwrap());
        assert!(f.is_banned(ip("203.0.113.7")) && f.is_banned(167838211) && f.is_banned(42));
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "// banned\n167838211 \r\n-889163513 Some One\r\n42 Guid\r\n"
        );
        assert_eq!(
            std::fs::read_to_string(dir.join("ban.txt.old")).unwrap(),
            "// banned\n10.1.2.3\n203.0.113.7  Some One\r\n42 Guid\r\n"
        );
        // Read on, the rewritten file needs no second pass.
        assert!(migrate(&std::fs::read_to_string(&path).unwrap()).is_none());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
