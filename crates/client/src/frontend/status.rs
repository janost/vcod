//! The server info popup (`serverinfo_popmenu`): a `getstatus` to the
//! selected server and the four-column rows `FEEDER_SERVERSTATUS` lists, as
//! ui_mp_x86.dll builds them (0x4000bcc0, 0x4000bbb0;
//! docs/research/cod11-front-end.md, "Server info").

use std::net::{SocketAddr, SocketAddrV4, UdpSocket};
use std::time::{Duration, Instant};

use vcod_common::net::connectionless::{build_oob, parse_oob};
use vcod_common::net::master::GETSTATUS;

/// `cl_serverStatusResendTime` and `ui_serverStatusTimeOut`'s defaults
/// (CoDMP.exe 0x566938, ui_mp_x86.dll 0x40036e0c).
const RESEND: Duration = Duration::from_millis(750);
const TIMEOUT: Duration = Duration::from_millis(7000);

/// The serverinfo keys pulled to the top, with their labels and whether the
/// value reads as Yes/No (ui_mp_x86.dll table 0x40036ec8).
const KNOWN: [(&str, &str, bool); 16] = [
    ("sv_hostname", "@EXE_SV_INFO_SERVERNAME", false),
    ("address", "@EXE_SV_INFO_ADDRESS", false),
    ("pswrd", "@EXE_SV_INFO_PASSWORD", true),
    ("gamename", "@EXE_SV_INFO_GAMENAME", false),
    ("g_gametype", "@EXE_SV_INFO_GAMETYPE", false),
    ("sv_pure", "@EXE_SV_INFO_PURE", true),
    ("mapname", "@EXE_SV_INFO_MAP", false),
    ("shortversion", "@EXE_SV_INFO_VERSION", false),
    ("protocol", "@EXE_SV_INFO_PROTOCOL", false),
    ("sv_maxping", "@EXE_SV_INFO_MAXPING", false),
    ("sv_minping", "@EXE_SV_INFO_MINPING", false),
    ("sv_maxrate", "@EXE_SV_INFO_MAXRATE", false),
    ("sv_floodprotect", "@EXE_SV_INFO_FLOODPROTECT", false),
    ("sv_allowanonymous", "@EXE_SV_INFO_ALLOWANON", false),
    ("sv_maxclients", "@EXE_SV_INFO_MAXCLIENTS", false),
    ("sv_privateclients", "@EXE_SV_INFO_PRIVATECLIENTS", false),
];

/// The UI's line cap (`MAX_SERVERSTATUS_LINES`).
const MAX_LINES: usize = 128;

pub type Row = [String; 4];

/// One `ServerStatus` request in flight, resent until it is answered or
/// times out.
pub struct StatusQuery {
    pub addr: SocketAddrV4,
    socket: Option<UdpSocket>,
    started: Instant,
    sent: Instant,
    pub rows: Vec<Row>,
}

impl StatusQuery {
    pub fn start(addr: SocketAddrV4, now: Instant) -> StatusQuery {
        let socket = UdpSocket::bind("0.0.0.0:0")
            .and_then(|s| {
                s.set_nonblocking(true)?;
                Ok(s)
            })
            .map_err(|e| log::warn!("serverinfo: cannot open a socket: {e}"))
            .ok();
        if let Some(s) = &socket {
            let _ = s.send_to(&build_oob(GETSTATUS), addr);
        }
        StatusQuery {
            addr,
            socket,
            started: now,
            sent: now,
            rows: Vec::new(),
        }
    }

    pub fn poll(&mut self, now: Instant) {
        let Some(socket) = &self.socket else { return };
        let mut buf = [0u8; 16384];
        while let Ok((n, from)) = socket.recv_from(&mut buf) {
            if from != SocketAddr::V4(self.addr) {
                continue;
            }
            if let Some(("statusResponse", rest)) = parse_oob(&buf[..n]) {
                let text: String = rest.iter().map(|&b| b as char).collect();
                self.rows = status_rows(&self.addr.to_string(), &text);
                self.socket = None;
                return;
            }
        }
        if now - self.started > TIMEOUT {
            self.socket = None;
        } else if now - self.sent > RESEND {
            let _ = socket.send_to(&build_oob(GETSTATUS), self.addr);
            self.sent = now;
        }
    }
}

/// The popup's rows for a `statusResponse` body: `address`, the
/// serverinfo pairs, a blank row, the column headers and one row per player
/// (index, score, ping, the rest of the line), then the known keys moved to
/// the top in table order under their labels.
pub fn status_rows(addr: &str, body: &str) -> Vec<Row> {
    let row =
        |a: &str, b: &str, c: &str, d: &str| -> Row { [a.into(), b.into(), c.into(), d.into()] };
    let mut lines = body.trim_start_matches('\n').split('\n');
    let info = lines.next().unwrap_or("");
    let mut rows = vec![row("address", "", "", addr)];
    let mut parts = info.strip_prefix('\\').unwrap_or(info).split('\\');
    while let (Some(k), Some(v)) = (parts.next(), parts.next()) {
        if rows.len() >= MAX_LINES {
            break;
        }
        rows.push(row(k, "", "", v));
    }
    if rows.len() < 0x7d {
        rows.push(row("", "", "", ""));
        rows.push(row(
            "@EXE_SV_INFO_NUM",
            "@EXE_SV_INFO_SCORE",
            "@EXE_SV_INFO_PING",
            "@EXE_SV_INFO_NAME",
        ));
        for (n, line) in lines.filter(|l| !l.is_empty()).enumerate() {
            if rows.len() >= MAX_LINES {
                break;
            }
            let mut f = line.splitn(3, ' ');
            let (Some(score), Some(ping), Some(name)) = (f.next(), f.next(), f.next()) else {
                break;
            };
            rows.push(row(&n.to_string(), score, ping, name));
        }
    }
    // `UI_SortServerStatusInfo`: each known key's rows swap their key and
    // value into the next slot from the top.
    let mut at = 0;
    for (key, label, yes_no) in KNOWN {
        for j in 0..rows.len() {
            if rows[j][1].is_empty() && rows[j][0].eq_ignore_ascii_case(key) {
                let (k, v) = (rows[j][0].clone(), rows[j][3].clone());
                rows[j][0] = std::mem::replace(&mut rows[at][0], k);
                rows[j][3] = std::mem::replace(&mut rows[at][3], v);
                rows[at][0] = label.into();
                if yes_no {
                    let on = rows[at][3].trim().parse::<i64>().unwrap_or(0) != 0;
                    rows[at][3] = if on { "@EXE_YES" } else { "@EXE_NO" }.into();
                }
                at += 1;
            }
        }
    }
    rows
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The retail 1.1d server's reply (cod11-server-handshake.md,
    /// `getstatus xyz`), with one player.
    const RETAIL: &str = "\n\\g_gametype\\dm\\gamename\\main\\mapname\\mp_pavlov\\protocol\\1\
        \\shortversion\\1.1\\sv_allowAnonymous\\0\\sv_floodProtect\\1\\sv_hostname\\CoDHost\
        \\sv_maxclients\\8\\sv_maxPing\\0\\sv_maxRate\\0\\sv_minPing\\0\\sv_privateClients\\0\
        \\sv_pure\\1\\pswrd\\0\n5 48 \"Bob\"\n";

    #[test]
    fn known_keys_lead_under_their_labels() {
        let rows = status_rows("1.2.3.4:28960", RETAIL);
        let head: Vec<(&str, &str)> = rows
            .iter()
            .take(5)
            .map(|r| (r[0].as_str(), r[3].as_str()))
            .collect();
        assert_eq!(
            head,
            [
                ("@EXE_SV_INFO_SERVERNAME", "CoDHost"),
                ("@EXE_SV_INFO_ADDRESS", "1.2.3.4:28960"),
                ("@EXE_SV_INFO_PASSWORD", "@EXE_NO"),
                ("@EXE_SV_INFO_GAMENAME", "main"),
                ("@EXE_SV_INFO_GAMETYPE", "dm"),
            ]
        );
        assert_eq!(rows[5][3], "@EXE_YES", "sv_pure 1");
        let last = rows.last().unwrap();
        assert_eq!(last, &["0", "5", "48", "\"Bob\""].map(String::from));
        assert_eq!(rows[rows.len() - 2][0], "@EXE_SV_INFO_NUM");
        // Every pair is listed once: 15 known keys, the address, the blank
        // row, the header and the player.
        assert_eq!(rows.len(), 1 + 15 + 3);
    }

    #[test]
    fn a_query_reads_the_reply_and_resends_until_then() {
        let server = UdpSocket::bind("127.0.0.1:0").unwrap();
        let SocketAddr::V4(addr) = server.local_addr().unwrap() else {
            unreachable!()
        };
        let t0 = Instant::now();
        let mut q = StatusQuery::start(addr, t0);
        let mut buf = [0u8; 64];
        let (n, from) = server.recv_from(&mut buf).unwrap();
        assert_eq!(&buf[..n], b"\xff\xff\xff\xffgetstatus");
        q.poll(t0 + RESEND + Duration::from_millis(1));
        assert!(server.recv_from(&mut buf).is_ok(), "resent");
        let mut reply = build_oob("statusResponse");
        reply.extend_from_slice(b"\n\\sv_hostname\\x\n");
        server.send_to(&reply, from).unwrap();
        for _ in 0..100 {
            q.poll(t0);
            if !q.rows.is_empty() {
                break;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(
            q.rows[0],
            ["@EXE_SV_INFO_SERVERNAME", "", "", "x"].map(String::from)
        );
    }
}
