//! The transport the two server browsers (CoD1 and CoD2's
//! [`super::browser`], CoD4's `play::cod4_menus::browser`) and CoD1's server
//! info popup share: one non-blocking broadcast socket, the master's name
//! resolved on a helper thread, its `getservers` held until
//! [`master_send_allowed`], the LAN broadcast, and the connectionless
//! replies sorted by kind. Each browser keeps its lists, ping pacing,
//! protocol filter, sort and cache codec (cod11-front-end.md "Master
//! query", "Local servers"; cod4-handshake.md 12).

use std::io;
use std::net::{SocketAddr, SocketAddrV4, ToSocketAddrs, UdpSocket};
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::Duration;

use vcod_common::net::connectionless::{build_oob, parse_oob};
use vcod_common::net::master::{GETINFO, LAN_ROUNDS, getservers, master_send_allowed};

/// How long the master gets before a refresh stops waiting on it. vcod's
/// own: retail waits for as long as the refresh runs.
pub const MASTER_TIMEOUT: Duration = Duration::from_secs(5);

/// A connectionless reply the scan's socket read.
#[derive(Debug, PartialEq, Eq)]
pub enum Reply {
    /// `getserversResponse` from the master the scan asked: the body, for
    /// the game's own parser.
    Servers(Vec<u8>),
    /// `infoResponse`, as Latin-1 text (the font indexes glyphs by byte).
    Info(SocketAddrV4, String),
    /// `statusResponse`, as Latin-1 text.
    Status(SocketAddrV4, String),
}

pub struct ServerScan {
    /// The log lines' prefix.
    tag: &'static str,
    /// Opens the socket on the first send (CoD4's browser keeps one for the
    /// session); else only [`Self::reopen`] opens it (CoD1's opens one a
    /// refresh and closes it when the refresh ends).
    lazy: bool,
    socket: Option<UdpSocket>,
    resolve: Option<mpsc::Receiver<Option<SocketAddr>>>,
    /// The master's name as the last [`Self::ask_master`] gave it.
    host: String,
    /// The master's resolved address, waiting on the rate limit until its
    /// `getservers` goes.
    due: Option<SocketAddr>,
    /// The address the `getservers` went to; its answers are taken.
    master: Option<SocketAddr>,
}

fn open_socket(tag: &str) -> Option<UdpSocket> {
    UdpSocket::bind("0.0.0.0:0")
        .and_then(|s| {
            s.set_nonblocking(true)?;
            s.set_broadcast(true)?;
            Ok(s)
        })
        .map_err(|e| log::warn!("{tag}: cannot open a socket: {e}"))
        .ok()
}

impl ServerScan {
    /// A scan whose socket only [`Self::reopen`] opens.
    pub fn new(tag: &'static str) -> ServerScan {
        ServerScan {
            tag,
            lazy: false,
            socket: None,
            resolve: None,
            host: String::new(),
            due: None,
            master: None,
        }
    }

    /// A scan that opens its socket when it first sends.
    pub fn lazy(tag: &'static str) -> ServerScan {
        ServerScan {
            lazy: true,
            ..ServerScan::new(tag)
        }
    }

    /// A fresh socket in place of the old one, which stays when none opens.
    pub fn reopen(&mut self) -> bool {
        match open_socket(self.tag) {
            Some(s) => {
                self.socket = Some(s);
                true
            }
            None => false,
        }
    }

    /// Opens a socket unless one is open.
    pub fn ensure(&mut self) -> bool {
        if self.socket.is_none() {
            self.socket = open_socket(self.tag);
        }
        self.socket.is_some()
    }

    pub fn close(&mut self) {
        self.socket = None;
    }

    pub fn is_open(&self) -> bool {
        self.socket.is_some()
    }

    fn sender(&mut self) -> Option<&UdpSocket> {
        if self.lazy {
            self.ensure();
        }
        self.socket.as_ref()
    }

    /// `cmd` out of band to `to`; a failed send is dropped, as a lost
    /// packet would be.
    pub fn send(&mut self, cmd: &str, to: SocketAddrV4) {
        if let Some(s) = self.sender() {
            let _ = s.send_to(&build_oob(cmd), to);
        }
    }

    /// `CL_LocalServers_f`'s broadcast: `getinfo xxx` to each target,
    /// [`LAN_ROUNDS`] times; a failed send is logged at `level`.
    pub fn broadcast(&mut self, targets: &[SocketAddrV4], level: log::Level) {
        let tag = self.tag;
        let Some(s) = self.sender() else {
            return;
        };
        for _ in 0..LAN_ROUNDS {
            for &to in targets {
                if let Err(e) = s.send_to(&build_oob(GETINFO), to) {
                    log::log!(level, "{tag}: broadcast to {to}: {e}");
                }
            }
        }
    }

    /// Starts resolving the master `host` on a helper thread; the last
    /// master's answers are no longer taken.
    pub fn ask_master(&mut self, host: &str, port: u16) {
        let (tx, rx) = mpsc::channel();
        let name = host.to_string();
        std::thread::spawn(move || {
            let addr = (name.as_str(), port)
                .to_socket_addrs()
                .ok()
                .and_then(|mut a| a.find(SocketAddr::is_ipv4));
            let _ = tx.send(addr);
        });
        self.host = host.to_string();
        self.resolve = Some(rx);
        self.master = None;
    }

    /// Stops resolving and holds back a `getservers` not sent yet.
    pub fn stop_master(&mut self) {
        self.resolve = None;
        self.due = None;
    }

    /// [`Self::stop_master`], and the last master's answers are no longer
    /// taken.
    pub fn forget_master(&mut self) {
        self.stop_master();
        self.master = None;
    }

    /// Resolving the master or holding its `getservers`.
    pub fn master_pending(&self) -> bool {
        self.resolve.is_some() || self.due.is_some()
    }

    /// Whether a `getservers` went out since the last forget.
    pub fn asked_master(&self) -> bool {
        self.master.is_some()
    }

    /// Once a frame: the master's resolve, then its `getservers` for
    /// `protocol` (`None` holds it) once the master's host is out of its
    /// last second. One a refresh: a list in several packets is not asked
    /// again. False when the name did not resolve.
    pub fn poll_master(&mut self, protocol: Option<u32>) -> bool {
        if let Some(rx) = &self.resolve
            && let Ok(addr) = rx.try_recv()
        {
            self.resolve = None;
            match addr {
                Some(addr) => self.due = Some(addr),
                None => {
                    log::warn!("{}: cannot resolve {}", self.tag, self.host);
                    return false;
                }
            }
        }
        // A lazy scan consults the rate limit before it opens a socket; the
        // other sends only on the socket its refresh opened.
        let tag = self.tag;
        if let Some(addr) = self.due
            && let Some(protocol) = protocol
            && (self.lazy || self.socket.is_some())
            && master_send_allowed(addr.ip())
            && let Some(s) = self.sender()
        {
            log::info!("{tag}: requesting protocol {protocol} servers from {addr}");
            let _ = s.send_to(&build_oob(&getservers(protocol)), addr);
            self.due = None;
            self.master = Some(addr);
        }
        true
    }

    /// The replies waiting on the socket, in arrival order: the master's
    /// list (from the master asked only), `infoResponse`s and
    /// `statusResponse`s. Anything else is dropped.
    pub fn recv(&self) -> Vec<Reply> {
        let mut out = Vec::new();
        let Some(s) = &self.socket else {
            return out;
        };
        let mut buf = [0u8; 16384];
        while let Ok((n, from)) = s.recv_from(&mut buf) {
            let SocketAddr::V4(from4) = from else {
                continue;
            };
            let Some((cmd, rest)) = parse_oob(&buf[..n]) else {
                continue;
            };
            out.push(match cmd {
                "getserversResponse" if Some(from) == self.master => Reply::Servers(rest.to_vec()),
                "infoResponse" => Reply::Info(from4, vcod_common::text::latin1(rest)),
                "statusResponse" => Reply::Status(from4, vcod_common::text::latin1(rest)),
                _ => continue,
            });
        }
        out
    }
}

/// Writes a server cache whole or not at all: `bytes` into `<path>.tmp`,
/// renamed over `path`. The temp file goes when either step fails.
pub fn write_cache(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".tmp");
    let tmp = PathBuf::from(tmp);
    std::fs::write(&tmp, bytes)
        .and_then(|()| std::fs::rename(&tmp, path))
        .inspect_err(|_| {
            let _ = std::fs::remove_file(&tmp);
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_cache_write_replaces_the_file_whole_or_leaves_it() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("servercache.dat");
        std::fs::write(&path, b"old bytes, longer than the new").unwrap();
        write_cache(&path, b"new").unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"new");
        assert!(!dir.path().join("servercache.dat.tmp").exists());
        // A directory in the cache's place: the rename fails, the directory
        // stays and the temp file goes.
        let blocked = dir.path().join("blocked.dat");
        std::fs::create_dir(&blocked).unwrap();
        std::fs::write(blocked.join("inside"), b"kept").unwrap();
        assert!(write_cache(&blocked, b"new").is_err());
        assert_eq!(std::fs::read(blocked.join("inside")).unwrap(), b"kept");
        assert!(!dir.path().join("blocked.dat.tmp").exists());
    }

    /// Only the master asked has its list taken; info and status replies
    /// come from anyone, in arrival order.
    #[test]
    fn replies_sort_by_kind_and_the_master_list_needs_the_master() {
        let peer = UdpSocket::bind("127.0.0.1:0").unwrap();
        let SocketAddr::V4(peer_addr) = peer.local_addr().unwrap() else {
            unreachable!()
        };
        let mut scan = ServerScan::new("test");
        assert!(scan.reopen());
        let me = scan.socket.as_ref().unwrap().local_addr().unwrap().port();
        let me = SocketAddrV4::new(std::net::Ipv4Addr::LOCALHOST, me);
        let send = |cmd: &str, body: &[u8]| {
            let mut p = build_oob(cmd);
            p.extend_from_slice(body);
            peer.send_to(&p, me).unwrap();
        };
        let drain = |scan: &ServerScan, n: usize| {
            let mut got = Vec::new();
            for _ in 0..100 {
                got.extend(scan.recv());
                if got.len() >= n {
                    break;
                }
                std::thread::sleep(Duration::from_millis(5));
            }
            got
        };
        send("getserversResponse\n", b"\\abcdef\\EOT");
        send("infoResponse\n", b"\\hostname\\a");
        send("print\n", b"ignored");
        send("statusResponse\n", b"\\sv_hostname\\b\n");
        assert_eq!(
            drain(&scan, 2),
            [
                Reply::Info(peer_addr, "\\hostname\\a".into()),
                Reply::Status(peer_addr, "\\sv_hostname\\b\n".into()),
            ]
        );
        scan.master = Some(SocketAddr::V4(peer_addr));
        send("getserversResponse\n", b"\\abcdef\\EOT");
        assert_eq!(drain(&scan, 1), [Reply::Servers(b"\\abcdef\\EOT".to_vec())]);
    }
}
