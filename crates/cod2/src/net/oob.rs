//! CoD2's connectionless packets (docs/research/cod2-protocol.md 1): `FF FF FF
//! FF` and a text line. Unlike 1.1, `connect` goes as plain text (1.2).

pub use vcod_common::net::connectionless::{info_pairs, info_value_for_key};

use super::ops::PROTOCOL;

const PREFIX: [u8; 4] = [0xff; 4];

/// `FF FF FF FF` then `text`.
pub fn packet(text: &str) -> Vec<u8> {
    let mut v = PREFIX.to_vec();
    v.extend(text.chars().map(|c| c as u8));
    v
}

/// The payload after the prefix, or `None` for a sequenced packet.
pub fn payload(p: &[u8]) -> Option<&[u8]> {
    p.strip_prefix(&PREFIX)
}

/// `getchallenge 0 "<md5>"` (`CL_CheckForResend`); `None` sends the bare
/// word, which a LAN server answers just the same (9).
pub fn getchallenge(cd_key_md5: Option<&str>) -> Vec<u8> {
    match cd_key_md5 {
        Some(md5) => packet(&format!("getchallenge 0 \"{md5}\"")),
        None => packet("getchallenge"),
    }
}

/// `getinfo <challenge>`.
pub fn getinfo(challenge: &str) -> Vec<u8> {
    packet(&format!("getinfo {challenge}"))
}

/// `getstatus <challenge>`.
pub fn getstatus(challenge: &str) -> Vec<u8> {
    packet(&format!("getstatus {challenge}"))
}

/// `connect "<userinfo>"`, plain text, with `protocol`, `challenge` and
/// `qport` set after the caller's keys.
pub fn connect(userinfo: &str, challenge: i32, qport: u16) -> Vec<u8> {
    let ui = format!("{userinfo}\\protocol\\{PROTOCOL}\\challenge\\{challenge}\\qport\\{qport}");
    packet(&format!("connect \"{ui}\""))
}

/// A server's out-of-band reply.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Reply {
    Challenge(i32),
    Connect,
    /// `infoResponse\n<info>`.
    Info(String),
    /// `statusResponse\n<serverinfo>\n` and one `score ping "name"` line a player.
    Status {
        info: String,
        players: Vec<(i32, i32, String)>,
    },
    /// `error\n<reason>`.
    Error(String),
    Disconnect,
    /// `print\n<text>`, an rcon answer.
    Print(String),
    LoadingNewMap(String),
    /// `needcdkey`: the server's authorize server does not know the
    /// client's key (`CL_ConnectionlessPacket`, CoD2MP_s.exe 0x40f047).
    NeedCdKey,
    Other(String),
}

/// Parse an out-of-band packet's payload (after the prefix).
pub fn parse_reply(payload: &[u8]) -> Reply {
    let text = vcod_common::text::latin1(payload);
    let (head, rest) = text.split_once('\n').unwrap_or((&text, ""));
    let word = head.split_whitespace().next().unwrap_or("");
    match word {
        "challengeResponse" => head
            .split_whitespace()
            .nth(1)
            .and_then(|n| n.parse().ok())
            .map(Reply::Challenge)
            .unwrap_or_else(|| Reply::Other(text.clone())),
        "connectResponse" => Reply::Connect,
        "infoResponse" => Reply::Info(rest.to_string()),
        "statusResponse" => {
            let mut lines = rest.split('\n');
            let info = lines.next().unwrap_or("").to_string();
            let players = lines
                .filter_map(|l| {
                    let mut it = l.splitn(3, ' ');
                    let score = it.next()?.parse().ok()?;
                    let ping = it.next()?.parse().ok()?;
                    let name = it.next()?.trim_matches('"').to_string();
                    Some((score, ping, name))
                })
                .collect();
            Reply::Status { info, players }
        }
        "error" => Reply::Error(rest.trim_end_matches('\n').to_string()),
        "disconnect" => Reply::Disconnect,
        "print" => Reply::Print(rest.to_string()),
        "loadingnewmap" => Reply::LoadingNewMap(rest.to_string()),
        "needcdkey" => Reply::NeedCdKey,
        _ => Reply::Other(text.clone()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retail_replies_parse() {
        // Captured from cod2_lnxded on loopback (cod2-protocol.md 9).
        assert_eq!(parse_reply(b"needcdkey"), Reply::NeedCdKey);
        assert_eq!(
            parse_reply(b"challengeResponse 525045477"),
            Reply::Challenge(525045477)
        );
        let Reply::Info(info) = parse_reply(
            b"infoResponse\n\\challenge\\xyz\\protocol\\118\\hostname\\CoD2Host\\mapname\\mp_toujane\\sv_maxclients\\8\\gametype\\dm\\kc\\1\\hw\\1\\mod\\1\\voice\\0\\pb\\0",
        ) else {
            panic!()
        };
        assert_eq!(info_value_for_key(&info, "mapname"), Some("mp_toujane"));
        let Reply::Status { info, players } = parse_reply(
            b"statusResponse\n\\g_gametype\\dm\\sv_maxclients\\8\\challenge\\xyz\\pswrd\\0\\mod\\1\n0 999 \"bot0\"\n",
        ) else {
            panic!()
        };
        assert_eq!(info_value_for_key(&info, "mod"), Some("1"));
        assert_eq!(players, vec![(0, 999, "bot0".to_string())]);
        assert_eq!(
            parse_reply(b"error\nEXE_BAD_CHALLENGE"),
            Reply::Error("EXE_BAD_CHALLENGE".into())
        );
        assert_eq!(
            connect("\\name\\x", 7, 4660),
            packet("connect \"\\name\\x\\protocol\\118\\challenge\\7\\qport\\4660\"")
        );
    }
}
