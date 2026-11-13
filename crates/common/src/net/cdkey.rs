//! CD key authorization, as each retail client does it beside
//! `getchallenge`: `getKeyAuthorize` to the game's authorize server, fire
//! and forget, skipped for a LAN server (docs/research/cod11-server-handshake.md,
//! "CD key authorization").
//!
//! The key never reaches a log: [`CdKey`]'s `Debug` hides it, and nothing
//! here prints the packet.

use std::net::{SocketAddr, ToSocketAddrs, UdpSocket};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

/// The env var that overrides the install's key.
pub const ENV: &str = "VCOD_CDKEY";

/// Which retail client's authorization to copy.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeyGame {
    /// CoD 1.1 and 1.5: one key, one authorize server.
    Cod1,
    /// United Offensive's own key.
    Uo,
    Cod2,
    Cod4,
}

impl KeyGame {
    /// The authorize server (`CL_RequestAuthorization`'s
    /// `NET_StringToAdr` and `BigShort` port).
    pub fn authorize_server(self) -> (&'static str, u16) {
        match self {
            KeyGame::Cod1 => ("codauthorize.activision.com", 20500),
            KeyGame::Uo => ("coduoauthorize.activision.com", 20600),
            KeyGame::Cod2 => ("cod2master.activision.com", 20700),
            KeyGame::Cod4 => ("cod4master.activision.com", 20800),
        }
    }
}

/// A key: its first 16 characters, upper case.
#[derive(Clone, PartialEq, Eq)]
pub struct CdKey {
    key: [u8; 16],
}

impl std::fmt::Debug for CdKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("CdKey(..)")
    }
}

impl CdKey {
    /// A key as the box prints it: dashes and spaces dropped, upper
    /// cased, the first 16 characters kept and the rest not sent. `None`
    /// under 16.
    pub fn parse(text: &str) -> Option<CdKey> {
        let chars: Vec<u8> = text
            .bytes()
            .filter(u8::is_ascii_alphanumeric)
            .map(|b| b.to_ascii_uppercase())
            .collect();
        let key: [u8; 16] = chars.get(..16)?.try_into().ok()?;
        Some(CdKey { key })
    }

    fn text(&self) -> &str {
        std::str::from_utf8(&self.key).unwrap_or_default()
    }

    /// `CL_BuildMd5StrFromCDKey`: MD5 from Activision's own initial state
    /// over the key, upper case on CoD2 and lower case on CoD4, in hex.
    /// The hash the game server reads in `getchallenge 0 "<hash>"` and the
    /// authorize server in the `PB` field.
    pub fn hash(&self, game: KeyGame) -> String {
        let mut k = self.key;
        if game == KeyGame::Cod4 {
            k.make_ascii_lowercase();
        }
        md5_with_state(&k, COD_MD5_STATE)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect()
    }

    /// `getKeyAuthorize <cl_anonymous> <key>`, with ` PB <hash>` on CoD2
    /// and CoD4; `cl_anonymous` is 0.
    pub fn key_authorize(&self, game: KeyGame) -> Vec<u8> {
        let text = match game {
            KeyGame::Cod1 | KeyGame::Uo => format!("getKeyAuthorize 0 {}", self.text()),
            KeyGame::Cod2 | KeyGame::Cod4 => {
                format!("getKeyAuthorize 0 {} PB {}", self.text(), self.hash(game))
            }
        };
        let mut p = vec![0xff; 4];
        p.extend_from_slice(text.as_bytes());
        p
    }
}

static INSTALL: OnceLock<PathBuf> = OnceLock::new();

/// The key a CD key menu verified this session (`CL_SetCDKey`'s
/// `cl_cdkey`), with its game and check; [`lookup`] takes it first.
static SESSION: Mutex<Option<(KeyGame, String, String)>> = Mutex::new(None);

/// `CL_CDKeyValidate`'s check over a key's first 16 bytes (`CoDMP.exe` 1.1
/// 0x414260, `iw3mp.exe` 0x4712e0): each byte, sign-extended, xors into the
/// sum, then 8 rounds of `sum ^= 0x14002` when odd and `sum >>= 1`; the
/// check is the sum as `%04x`. `None` for a key under 16 bytes.
pub fn check(key: &str) -> Option<String> {
    let bytes = key.as_bytes().get(..16)?;
    let mut sum = 0u32;
    for &b in bytes {
        sum ^= b as i8 as i32 as u32;
        for _ in 0..8 {
            if sum & 1 != 0 {
                sum ^= 0x14002;
            }
            sum >>= 1;
        }
    }
    Some(format!("{sum:04x}"))
}

/// `CL_CDKeyValidate`: the 16-character key's check matches `check`,
/// case folded. The menus join four 4-character fields into the key and
/// read the fifth as the check.
pub fn valid(key: &str, check_text: &str) -> bool {
    key.len() == 16 && check(key).is_some_and(|c| c.eq_ignore_ascii_case(check_text))
}

/// What a CD key menu's `getCDKey` shows: the session's verified key,
/// else `VCOD_CDKEY`'s or `main/codkey`'s as [`lookup`] reads it, with its
/// check when the source holds one (the 17th to 20th characters). Empty
/// strings with no key.
pub fn menu_key(game: KeyGame) -> (String, String) {
    if let Some((g, key, check)) = SESSION.lock().ok().and_then(|s| s.clone())
        && g == game
    {
        return (key, check);
    }
    let text = match std::env::var(ENV) {
        Ok(v) if !v.trim().is_empty() => v,
        _ if game == KeyGame::Cod1 => INSTALL
            .get()
            .and_then(|i| std::fs::read(i.join("main").join("codkey")).ok())
            .map(|b| String::from_utf8_lossy(b.get(..20).unwrap_or(&b)).into_owned())
            .unwrap_or_default(),
        _ => String::new(),
    };
    let chars: String = text
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .map(|c| c.to_ascii_uppercase())
        .collect();
    match chars.len() {
        0..16 => (String::new(), String::new()),
        16..20 => (chars[..16].to_string(), String::new()),
        _ => (chars[..16].to_string(), chars[16..20].to_string()),
    }
}

/// `CL_SetCDKey` after a menu's `verifyCDKey` found the key valid: the
/// key serves this session's authorizations from now on. 1.1 (`write_file`)
/// also writes `main/codkey` (`Com_WriteCDKey`, `CoDMP.exe` 0x436bc0); the
/// other clients keep theirs in the registry, which vcod leaves alone.
pub fn store(game: KeyGame, key: &str, check_text: &str, write_file: bool) {
    if !valid(key, check_text) {
        return;
    }
    if let Ok(mut s) = SESSION.lock() {
        *s = Some((game, key.to_string(), check_text.to_string()));
    }
    if write_file && let Some(install) = INSTALL.get() {
        let path = install.join("main").join("codkey");
        if std::fs::write(&path, codkey_file(key, check_text)).is_err() {
            log::warn!("Couldn't write {}.", path.display());
        }
    }
}

/// Drops `game`'s session key; tests that verify one through a menu put
/// the process back as it was.
pub fn forget(game: KeyGame) {
    if let Ok(mut s) = SESSION.lock()
        && s.as_ref().is_some_and(|(g, ..)| *g == game)
    {
        *s = None;
    }
}

/// `Com_WriteCDKey`'s file: the key and check upper cased, then its three
/// comment lines.
fn codkey_file(key: &str, check_text: &str) -> Vec<u8> {
    let mut out = format!(
        "{}{}",
        key.to_ascii_uppercase(),
        check_text.to_ascii_uppercase()
    );
    out.push_str("\r\n// generated by CoD, do not modify\r\n");
    out.push_str("// Do not give this file to ANYONE.\r\n");
    out.push_str("// Infinity Ward and Activision will NOT ask you to send this file to them.\r\n");
    out.into_bytes()
}

/// The install directory (the one holding `main/`), set once at startup,
/// where 1.1's `main/codkey` is read.
pub fn set_install(dir: &Path) {
    let _ = INSTALL.set(dir.to_path_buf());
}

/// `Com_ReadCDKey` (CoDMP.exe 1.1 0x436930): the file's first 16 bytes
/// are the key.
fn read_codkey(install: &Path) -> Option<String> {
    let bytes = std::fs::read(install.join("main").join("codkey")).ok()?;
    Some(String::from_utf8_lossy(bytes.get(..16)?).into_owned())
}

/// The key for `game`: a menu's verified key this session, else
/// `VCOD_CDKEY`, else on CoD 1 the install's
/// `main/codkey`. The other clients read only the registry, which vcod
/// does not. One under 16 characters is not used.
pub fn lookup(game: KeyGame) -> Option<CdKey> {
    if let Some((g, key, _)) = SESSION.lock().ok().and_then(|s| s.clone())
        && g == game
    {
        return CdKey::parse(&key);
    }
    let (text, from) = match std::env::var(ENV) {
        Ok(v) if !v.trim().is_empty() => (v, ENV),
        _ if game == KeyGame::Cod1 => (read_codkey(INSTALL.get()?)?, "main/codkey"),
        _ => return None,
    };
    let key = CdKey::parse(&text);
    if key.is_none() {
        log::warn!("the CD key in {from} is under 16 characters; not sent");
    }
    key
}

/// `needcdkey` without a key: the console line beside retail's message.
pub fn warn_no_key() {
    log::warn!(
        "this server checks CD keys: enter yours in the CD key menu or set {ENV} \
         (docs/usage.md, \"CD keys\")"
    );
}

/// Whether retail asks the authorize server before challenging `server`:
/// not for a LAN address (`net_lanauthorize` 0, `Sys_IsLANAddress`).
pub fn authorizes(server: SocketAddr) -> bool {
    !super::lan::is_lan(server.ip())
}

/// [`authorizes`] against a fixed address list instead of this host's.
#[cfg(test)]
fn authorizes_with(server: SocketAddr, locals: &[std::net::Ipv4Addr]) -> bool {
    match server.ip() {
        std::net::IpAddr::V4(v4) => !super::lan::is_lan_with(v4, locals),
        std::net::IpAddr::V6(v6) => !v6.is_loopback(),
    }
}

/// Sends `key`'s `getKeyAuthorize` to `game`'s authorize server, on a
/// thread so the name lookup never holds up the join. Nothing for a LAN
/// `server`.
pub fn request_authorization(game: KeyGame, key: &CdKey, server: SocketAddr) {
    if !authorizes(server) {
        return;
    }
    let packet = key.key_authorize(game);
    let (host, port) = game.authorize_server();
    std::thread::spawn(move || {
        let to = (host, port)
            .to_socket_addrs()
            .ok()
            .and_then(|mut a| a.find(SocketAddr::is_ipv4));
        let Some(to) = to else {
            log::debug!("cannot resolve {host}");
            return;
        };
        match UdpSocket::bind("0.0.0.0:0").and_then(|s| s.send_to(&packet, to)) {
            Ok(_) => log::debug!("getKeyAuthorize sent to {host} ({to})"),
            Err(e) => log::debug!("getKeyAuthorize to {host}: {e}"),
        }
    });
}

/// The four state words `CL_BuildMd5StrFromCDKey` starts MD5 from in place
/// of the standard ones (CoD2MP_s.exe 0x40d053, iw3mp.exe 0x469a37): the
/// hash CoD2 and CoD4 servers and authorize servers expect.
const COD_MD5_STATE: [u32; 4] = [0x6f1cd602, 0x226c74be, 0xb31c088d, 0x555a9639];

/// RFC 1321's initial state.
#[cfg(test)]
const MD5_STATE: [u32; 4] = [0x67452301, 0xefcdab89, 0x98badcfe, 0x10325476];

/// MD5 of `data` from `state`.
fn md5_with_state(data: &[u8], mut state: [u32; 4]) -> [u8; 16] {
    const S: [u32; 64] = [
        7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 5, 9, 14, 20, 5, 9, 14, 20, 5,
        9, 14, 20, 5, 9, 14, 20, 4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 6, 10,
        15, 21, 6, 10, 15, 21, 6, 10, 15, 21, 6, 10, 15, 21,
    ];
    let k: [u32; 64] =
        std::array::from_fn(|i| ((i as f64 + 1.0).sin().abs() * 4294967296.0) as u32);
    let mut msg = data.to_vec();
    msg.push(0x80);
    while msg.len() % 64 != 56 {
        msg.push(0);
    }
    msg.extend_from_slice(&((data.len() as u64) * 8).to_le_bytes());
    for block in msg.as_chunks::<64>().0 {
        let m: [u32; 16] = std::array::from_fn(|i| {
            u32::from_le_bytes(block[4 * i..4 * i + 4].try_into().unwrap())
        });
        let [mut a, mut b, mut c, mut d] = state;
        for i in 0..64 {
            let (f, g) = match i / 16 {
                0 => ((b & c) | (!b & d), i),
                1 => ((d & b) | (!d & c), (5 * i + 1) % 16),
                2 => (b ^ c ^ d, (3 * i + 5) % 16),
                _ => (c ^ (b | !d), (7 * i) % 16),
            };
            let f = f.wrapping_add(a).wrapping_add(k[i]).wrapping_add(m[g]);
            a = d;
            d = c;
            c = b;
            b = b.wrapping_add(f.rotate_left(S[i]));
        }
        for (s, v) in state.iter_mut().zip([a, b, c, d]) {
            *s = s.wrapping_add(v);
        }
    }
    let mut out = [0u8; 16];
    for (i, s) in state.iter().enumerate() {
        out[4 * i..4 * i + 4].copy_from_slice(&s.to_le_bytes());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(b: [u8; 16]) -> String {
        b.iter().map(|b| format!("{b:02x}")).collect()
    }

    #[test]
    fn md5_matches_rfc_1321() {
        assert_eq!(
            hex(md5_with_state(b"", MD5_STATE)),
            "d41d8cd98f00b204e9800998ecf8427e"
        );
        assert_eq!(
            hex(md5_with_state(b"abc", MD5_STATE)),
            "900150983cd24fb0d6963f7d28e17f72"
        );
        let long =
            b"12345678901234567890123456789012345678901234567890123456789012345678901234567890";
        assert_eq!(
            hex(md5_with_state(long, MD5_STATE)),
            "57edf4a22be3c955ac49da2e2107b67a"
        );
    }

    /// A dummy key, all 'A's.
    const DUMMY: &str = "AAAA-AAAA-AAAA-AAAA";

    #[test]
    fn parse_keeps_sixteen_characters() {
        assert!(CdKey::parse(DUMMY).is_some());
        assert_eq!(CdKey::parse("aaaa aaaa-aaaa-aaaa"), CdKey::parse(DUMMY));
        assert_eq!(CdKey::parse(&format!("{DUMMY}-1234")), CdKey::parse(DUMMY));
        assert!(CdKey::parse("AAAA-AAAA").is_none());
        assert_eq!(format!("{:?}", CdKey::parse(DUMMY).unwrap()), "CdKey(..)");
    }

    #[test]
    fn packets_follow_each_client() {
        let k = CdKey::parse(DUMMY).unwrap();
        let text = |g| String::from_utf8(k.key_authorize(g)[4..].to_vec()).unwrap();
        assert_eq!(text(KeyGame::Cod1), "getKeyAuthorize 0 AAAAAAAAAAAAAAAA");
        assert_eq!(text(KeyGame::Uo), "getKeyAuthorize 0 AAAAAAAAAAAAAAAA");
        let cod2 = md5_with_state(b"AAAAAAAAAAAAAAAA", COD_MD5_STATE);
        let cod4 = md5_with_state(b"aaaaaaaaaaaaaaaa", COD_MD5_STATE);
        assert_eq!(k.hash(KeyGame::Cod2), hex(cod2));
        assert_eq!(k.hash(KeyGame::Cod4), hex(cod4));
        assert_ne!(hex(cod2), hex(cod4));
        assert_eq!(
            text(KeyGame::Cod2),
            format!("getKeyAuthorize 0 AAAAAAAAAAAAAAAA PB {}", hex(cod2))
        );
        assert_eq!(
            text(KeyGame::Cod4),
            format!("getKeyAuthorize 0 AAAAAAAAAAAAAAAA PB {}", hex(cod4))
        );
        assert_eq!(&k.key_authorize(KeyGame::Cod1)[..4], &[0xff; 4]);
    }

    #[test]
    fn lan_servers_are_not_authorized() {
        // A host on 192.168.1.5: its block is the LAN, wherever the test runs.
        let locals = ["127.0.0.1".parse().unwrap(), "192.168.1.5".parse().unwrap()];
        assert!(!authorizes("127.0.0.1:28960".parse().unwrap()));
        assert!(!authorizes_with(
            "192.168.1.20:28960".parse().unwrap(),
            &locals
        ));
        assert!(authorizes_with(
            "203.0.113.7:28960".parse().unwrap(),
            &locals
        ));
    }

    #[test]
    fn the_check_is_the_menus_crc() {
        assert_eq!(check("AAAAAAAAAAAAAAAA").as_deref(), Some("7d1d"));
        assert_eq!(check("ABCD2345EFGH6789").as_deref(), Some("0882"));
        assert_eq!(check("ABCD"), None);
        assert!(valid("ABCD2345EFGH6789", "0882"));
        assert!(valid("AAAAAAAAAAAAAAAA", "7D1D"));
        assert!(!valid("AAAAAAAAAAAAAAAA", "7d1e"));
        assert!(!valid("AAAAAAAAAAAAAAA", "7d1d"));
        // the sum runs over the bytes as typed
        assert!(!valid("aaaaaaaaaaaaaaaa", "7d1d"));
    }

    #[test]
    fn a_written_codkey_reads_back() {
        let file = codkey_file("abcd2345efgh6789", "0882");
        assert!(file.starts_with(b"ABCD2345EFGH67890882\r\n// generated by CoD"));
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("main")).unwrap();
        std::fs::write(dir.path().join("main/codkey"), file).unwrap();
        let text = read_codkey(dir.path()).unwrap();
        assert_eq!(CdKey::parse(&text), CdKey::parse("ABCD2345EFGH6789"));
    }

    #[test]
    fn codkey_file_reads_the_key() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("main")).unwrap();
        std::fs::write(
            dir.path().join("main/codkey"),
            "AAAAAAAAAAAAAAAA1234\r\n// generated by CoD, do not modify\r\n",
        )
        .unwrap();
        let text = read_codkey(dir.path()).unwrap();
        assert_eq!(CdKey::parse(&text), CdKey::parse(DUMMY));
    }
}
