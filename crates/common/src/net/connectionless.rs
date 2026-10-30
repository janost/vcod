//! Contains routines ported from the RTCW-MP GPL source, Copyright (C) 1999-2010 id Software LLC, a ZeniMax Media company.
//! See NOTICE.
//!
//! Out-of-band packets: 0xFFFFFFFF prefix and a text command, both halves of
//! the wire (RTCW net_chan.c NET_OutOfBandPrint, cl_main.c CL_ConnectionlessPacket).

const OOB_PREFIX: [u8; 4] = [0xff; 4];
/// The command word of a connect packet; huffman starts right after it.
const CONNECT_CMD: &str = "connect ";
const CONNECT_COMPRESS_START: usize = OOB_PREFIX.len() + CONNECT_CMD.len();

pub fn build_oob(cmd: &str) -> Vec<u8> {
    let mut v = Vec::with_capacity(OOB_PREFIX.len() + cmd.len());
    v.extend_from_slice(&OOB_PREFIX);
    v.extend_from_slice(cmd.as_bytes());
    v
}

/// `connect "<userinfo>"` with everything from byte 12 (the opening quote)
/// huffman-compressed, so the server matches the command word before it
/// decompresses. Plaintext gets `error\nEXE_SERVER_IS_DIFFERENT_VER`.
/// docs/protocol-1.1.md, divergence #1.
pub fn build_connect(userinfo: &str) -> Vec<u8> {
    let mut v = build_oob(CONNECT_CMD);
    v.push(b'"');
    v.extend_from_slice(userinfo.as_bytes());
    v.push(b'"');
    super::huffman::compress(&mut v, CONNECT_COMPRESS_START);
    v
}

/// Server half of [`build_connect`]; returns the userinfo without its quotes
/// (`SV_DirectConnect`'s `Cmd_Argv(1)`).
pub fn parse_connect(packet: &[u8]) -> anyhow::Result<String> {
    let start = CONNECT_COMPRESS_START;
    anyhow::ensure!(
        packet.len() > start && packet[..start] == *build_oob(CONNECT_CMD),
        "not a connect packet"
    );
    let mut body = packet.to_vec();
    super::huffman::decompress(&mut body, start);
    let s = body[start..].strip_prefix(b"\"").unwrap_or(&body[start..]);
    let end = s
        .iter()
        .position(|&b| b == b'"' || b == 0)
        .unwrap_or(s.len());
    anyhow::ensure!(end > 0, "empty userinfo");
    Ok(crate::text::latin1(&s[..end]))
}

/// `MAX_INFO_STRING`: what `Info_SetValueForKey` lets an info string grow to.
pub const MAX_INFO_STRING: usize = 1024;

/// `Info_SetValueForKey` as a builder: insertion order, a repeated key replaces
/// in place.
#[derive(Clone, Debug, Default)]
pub struct Info {
    pairs: Vec<(String, String)>,
}

/// A value as `Info_SetValueForKey` (cod_lnxded 0x80827d4) stores it: every
/// `\`, `;` and `"` dropped.
fn info_value(value: impl std::fmt::Display) -> String {
    value
        .to_string()
        .chars()
        .filter(|c| !matches!(c, '\\' | ';' | '"'))
        .collect()
}

impl Info {
    pub fn new() -> Self {
        Self::default()
    }

    /// The value loses its `\ ; "`, as `Cvar_InfoString` (0x806fc30) has
    /// `Info_SetValueForKey` do, and an empty one leaves the key out. A key
    /// holding one is a `Com_Error` there; here the pair is refused.
    pub fn set(&mut self, key: &str, value: impl std::fmt::Display) -> &mut Self {
        if key.contains(['\\', ';', '"']) {
            log::warn!("info: refusing key {key:?}");
            return self;
        }
        let value = info_value(value);
        if value.is_empty() {
            self.pairs.retain(|(k, _)| k != key);
            return self;
        }
        match self.pairs.iter_mut().find(|(k, _)| k == key) {
            Some(slot) => slot.1 = value,
            None => self.pairs.push((key.to_string(), value)),
        }
        self
    }

    /// The engine's own `Info_SetValueForKey` (cod_lnxded 0x80827d4; 1.5b
    /// and UO behave the same): the value loses every `\`, `;` and `"`,
    /// the key comes out and goes back on the end, an empty value leaves it
    /// out, and a pair that would take the string past `MAX_INFO_STRING`
    /// is refused, returning false (docs/research/cod11-server-handshake.md,
    /// "The challenge key"). The key is a constant of the caller's.
    pub fn set_value_for_key(&mut self, key: &str, value: impl std::fmt::Display) -> bool {
        let value = info_value(value);
        self.pairs.retain(|(k, _)| k != key);
        if value.is_empty() {
            return true;
        }
        if self.len() + 2 + key.len() + value.len() > MAX_INFO_STRING {
            log::warn!("Info string length exceeded: {key:?}");
            return false;
        }
        self.pairs.push((key.to_string(), value));
        true
    }

    /// The length of the string `Display` writes.
    pub fn len(&self) -> usize {
        self.pairs.iter().map(|(k, v)| 2 + k.len() + v.len()).sum()
    }

    pub fn is_empty(&self) -> bool {
        self.pairs.is_empty()
    }

    pub fn get(&self, key: &str) -> Option<&str> {
        self.pairs
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_str())
    }
}

impl std::fmt::Display for Info {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for (k, v) in &self.pairs {
            write!(f, "\\{k}\\{v}")?;
        }
        Ok(())
    }
}

/// Minimal `Info_ValueForKey`: `\key\value\key\value...`.
pub fn info_value_for_key<'a>(info: &'a str, key: &str) -> Option<&'a str> {
    let mut parts = info.trim_start_matches('\\').split('\\');
    while let (Some(k), Some(v)) = (parts.next(), parts.next()) {
        if k == key {
            return Some(v);
        }
    }
    None
}

/// `Info_NextPair` run to the end, as `CL_SystemInfoChanged` walks the
/// systeminfo (CoDMP.exe 0x415f90): one leading `\` skipped per pair, an
/// empty key ends the walk, a key with no value reads `""`.
pub fn info_pairs(info: &str) -> Vec<(&str, &str)> {
    let mut out = Vec::new();
    let mut rest = info;
    loop {
        rest = rest.strip_prefix('\\').unwrap_or(rest);
        let (key, value);
        match rest.split_once('\\') {
            None => (key, value, rest) = (rest, "", ""),
            Some((k, after)) => {
                key = k;
                // The value runs to the next `\`, which stays for the next pair.
                let end = after.find('\\').unwrap_or(after.len());
                value = &after[..end];
                rest = &after[end..];
            }
        }
        if key.is_empty() {
            return out;
        }
        out.push((key, value));
    }
}

/// `(first word, remainder after the separator)`; `None` without the -1 prefix.
pub fn parse_oob(packet: &[u8]) -> Option<(&str, &[u8])> {
    let body = packet.strip_prefix(&OOB_PREFIX[..])?;
    let end = body
        .iter()
        .position(|&b| b == b' ' || b == b'\n' || b == 0)
        .unwrap_or(body.len());
    let cmd = std::str::from_utf8(&body[..end]).ok()?;
    let rest = if end < body.len() {
        &body[end + 1..]
    } else {
        &[][..]
    };
    Some((cmd, rest))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn oob_roundtrip() {
        let pkt = build_oob("getstatus");
        assert_eq!(&pkt[..4], &[0xff, 0xff, 0xff, 0xff]);
        let (cmd, rest) = parse_oob(&pkt).unwrap();
        assert_eq!(cmd, "getstatus");
        assert!(rest.is_empty());
    }

    #[test]
    fn oob_parse_with_payload() {
        let mut pkt = build_oob("challengeResponse");
        pkt.extend_from_slice(b" 12345");
        let (cmd, rest) = parse_oob(&pkt).unwrap();
        assert_eq!(cmd, "challengeResponse");
        assert_eq!(rest, b"12345");
    }

    #[test]
    fn connect_keeps_the_command_word_plain() {
        let pkt = build_connect("\\protocol\\1\\name\\vcod");
        assert_eq!(&pkt[..12], b"\xff\xff\xff\xffconnect ");
        let (cmd, _) = parse_oob(&pkt).unwrap();
        assert_eq!(cmd, "connect");
        let mut body = pkt.clone();
        super::super::huffman::decompress(&mut body, 12);
        assert_eq!(&body[12..], b"\"\\protocol\\1\\name\\vcod\"");
    }

    #[test]
    fn oob_rejects_netchan_packet() {
        assert!(parse_oob(&[0x01, 0x00, 0x00, 0x00, 0xaa]).is_none());
    }

    #[test]
    fn parse_connect_recovers_the_userinfo() {
        let ui = "\\protocol\\1\\qport\\8193\\challenge\\-12345\\name\\vcod";
        assert_eq!(parse_connect(&build_connect(ui)).unwrap(), ui);
        assert!(parse_connect(&build_oob("getinfo")).is_err());
        assert!(parse_connect(b"\xff\xff\xff\xffconnect ").is_err());
    }

    /// The engine's setter: separators stripped, the key moved to the end,
    /// an empty value removing it, and nothing past 1024 bytes.
    #[test]
    fn set_value_for_key_strips_moves_and_caps() {
        let mut i = Info::new();
        i.set("sv_keywords", "ctf").set("protocol", 1);
        assert!(i.set_value_for_key("challenge", "a\\b;c\"d"));
        assert!(i.set_value_for_key("sv_keywords", "demo ctf"));
        assert_eq!(
            i.to_string(),
            "\\protocol\\1\\challenge\\abcd\\sv_keywords\\demo ctf"
        );
        assert!(i.set_value_for_key("challenge", ""));
        assert_eq!(i.get("challenge"), None);
        let room = MAX_INFO_STRING - i.len() - "\\k\\".len();
        assert!(!i.set_value_for_key("k", "x".repeat(room + 1)));
        assert!(i.set_value_for_key("k", "x".repeat(room)));
        assert_eq!(i.to_string().len(), MAX_INFO_STRING);
    }

    #[test]
    fn info_pairs_walk_like_cl_system_info_changed() {
        assert_eq!(
            info_pairs(r"\sv_cheats\0\rate\5000\fs_game\mymod"),
            [("sv_cheats", "0"), ("rate", "5000"), ("fs_game", "mymod")]
        );
        // No leading `\`, an empty value, and a last key with no value.
        assert_eq!(info_pairs(r"a\\b\2\c"), [("a", ""), ("b", "2"), ("c", "")]);
        // An empty key ends the walk.
        assert_eq!(info_pairs(r"\a\1\\\b\2"), [("a", "1")]);
        assert!(info_pairs("").is_empty());
    }

    /// Retail over rcon: `sv_hostname a;b\c` came back as `abc` (1.1d,
    /// 1.5b, UO 1.51b) and `"x y;z"` as `x yz` (1.1d)
    /// (cod11-server-handshake.md, "The challenge key").
    #[test]
    fn info_strips_separators_from_a_value() {
        let mut i = Info::new();
        i.set("sv_hostname", "a;b\\c").set("protocol", 1);
        assert_eq!(i.to_string(), "\\sv_hostname\\abc\\protocol\\1");
        i.set("sv_hostname", "x \"y;z");
        assert_eq!(i.get("sv_hostname"), Some("x yz"));
        i.set("sv_hostname", ";\\\"");
        assert_eq!(i.to_string(), "\\protocol\\1");
        i.set("a\\b", "x").set("a;b", "x");
        assert_eq!(i.to_string(), "\\protocol\\1");
    }

    #[test]
    fn info_builder_round_trips_and_overrides() {
        let mut i = Info::new();
        i.set("mapname", "mp_carentan")
            .set("protocol", 1)
            .set("mapname", "mp_pavlov");
        let s = i.to_string();
        assert_eq!(s, "\\mapname\\mp_pavlov\\protocol\\1");
        assert_eq!(info_value_for_key(&s, "protocol"), Some("1"));
        assert_eq!(i.get("mapname"), Some("mp_pavlov"));
        assert_eq!(Info::new().to_string(), "");
    }
}
