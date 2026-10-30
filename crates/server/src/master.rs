//! `SV_MasterHeartbeat` (cod_lnxded 0x808ba0c): the `heartbeat COD-1` a
//! `dedicated 2` server sends the master list, and the `flatline` it sends on
//! the way down; `SV_MasterGameCompleteStatus` (0x808bbc4), the console's
//! `gameCompleteStatus`. Numbers and addresses are in
//! docs/research/cod11-server-handshake.md, "Master heartbeat" and "Console
//! commands over rcon".
//!
//! vcod lists on cod.pm's master ([`vcod_common::net::master::MASTER_HOST`])
//! instead of retail's, on the protocol's port, and sends each master host
//! at most one packet a second ([`RateLimiter`]).

use std::net::{SocketAddr, ToSocketAddrs};
use vcod_common::net::connectionless::build_oob;
use vcod_common::net::master::{COD1_MASTER, Master, RateLimiter};

/// `HEARTBEAT_MSEC` (`add eax,0x2bf20` at 0x808ba3d).
pub const HEARTBEAT_MS: i32 = 180_000;
/// CoD2's status interval: `SV_MasterHeartbeat` (cod2_lnxded 0x8096ed6)
/// sends `SVC_Status` to the master each 600000 ms. VERIFIED.
pub const STATUS_MS: i32 = 600_000;
/// `sv_master1`'s default until the protocol's ([`Transport::master`],
/// `Server::set_protocol`); `sv_master2..5` default empty. Retail's is
/// `codmaster.activision.com` on `PORT_MASTER` 20510 (`push 0x501e` at
/// 0x808baff).
///
/// [`Transport::master`]: vcod_common::net::wire::Transport::master
pub const DEFAULT_MASTER: Master = COD1_MASTER;
pub const HEARTBEAT_GAME: &str = "COD-1";
/// CoD2's `SV_MasterHeartbeat("COD-2")` (cod2_lnxded 0x8096ed6).
pub const HEARTBEAT_GAME_COD2: &str = "COD-2";
pub const FLATLINE: &str = "flatline";
pub const MAX_MASTERS: usize = 5;

/// `sv_master1..5`, their resolved addresses and `svs.nextHeartbeatTime`.
pub struct Masters {
    names: [String; MAX_MASTERS],
    /// `cvar->modified`: resolve again before the next send.
    dirty: [bool; MAX_MASTERS],
    addrs: [Option<SocketAddr>; MAX_MASTERS],
    /// `SV_MasterGameCompleteStatus`'s own address table (0x8160b20, the
    /// heartbeat's is 0x8160aa0). Whichever of the two sees `dirty` first
    /// resolves into its table alone.
    status_addrs: [Option<SocketAddr>; MAX_MASTERS],
    next_ms: i32,
    /// CoD2's status timer (`DAT_084230d8`), and the masters a due status
    /// still owes because the one-a-second limit held it back.
    status_next_ms: i32,
    status_owed: [bool; MAX_MASTERS],
    /// The port every master gets, the protocol's: retail's
    /// `strstr(":", name)` has its arguments swapped, so a port in the cvar
    /// is parsed and then overwritten.
    port: u16,
    limit: RateLimiter,
}

impl Default for Masters {
    fn default() -> Self {
        Masters {
            names: [
                DEFAULT_MASTER.host.to_string(),
                String::new(),
                String::new(),
                String::new(),
                String::new(),
            ],
            dirty: [true; MAX_MASTERS],
            addrs: [None; MAX_MASTERS],
            status_addrs: [None; MAX_MASTERS],
            next_ms: 0,
            status_next_ms: 0,
            status_owed: [false; MAX_MASTERS],
            port: DEFAULT_MASTER.port,
            limit: RateLimiter::default(),
        }
    }
}

impl Masters {
    /// The masters a server with only `master` in `sv_master1` heartbeats.
    pub fn new(master: Master) -> Masters {
        let mut m = Masters::default();
        m.set_master(master);
        m
    }

    /// The protocol's master: `sv_master1`'s host and every master's port.
    pub fn set_master(&mut self, master: Master) {
        self.set(0, master.host);
        self.set_port(master.port);
    }

    /// Every master's port; each resolves again.
    pub fn set_port(&mut self, port: u16) {
        if self.port != port {
            self.port = port;
            self.dirty = [true; MAX_MASTERS];
        }
    }

    /// Whether a packet to `to` may go now, at most one a second to a host.
    fn allow(&mut self, to: SocketAddr, now_ms: i32) -> bool {
        let ok = self.limit.allow(to.ip(), now_ms.max(0) as u64);
        if !ok {
            log::info!("master {to}: held to one packet a second, dropped");
        }
        ok
    }

    /// `sv_master<n>` for `n` in 1..=5; anything else is not one of them.
    pub fn index(cvar: &str) -> Option<usize> {
        let n = cvar
            .to_ascii_lowercase()
            .strip_prefix("sv_master")?
            .parse::<usize>()
            .ok()?;
        (1..=MAX_MASTERS).contains(&n).then(|| n - 1)
    }

    /// `sv_master<i + 1>`'s host.
    #[cfg(test)]
    pub(crate) fn name(&self, i: usize) -> &str {
        &self.names[i]
    }

    /// An unchanged name keeps its resolved address, as an unchanged cvar
    /// stays unmodified.
    pub fn set(&mut self, i: usize, name: &str) {
        if self.names[i] != name {
            self.names[i] = name.to_string();
            self.dirty[i] = true;
        }
    }

    /// `SV_Heartbeat_f` (0x8084bd0): the next frame sends.
    pub fn force(&mut self) {
        self.next_ms = -9_999_999;
    }

    /// One `SV_MasterHeartbeat(text)`: nothing unless `dedicated` is 2 and the
    /// timer is due. A name that fails to resolve is cleared, as retail's
    /// `Cvar_Set(name, "")` does, so it is never tried again.
    pub fn heartbeat(
        &mut self,
        dedicated: i32,
        now_ms: i32,
        text: &str,
        resolve: &mut dyn FnMut(&str) -> Option<SocketAddr>,
    ) -> Vec<(SocketAddr, Vec<u8>)> {
        if dedicated != 2 || now_ms < self.next_ms {
            return Vec::new();
        }
        self.next_ms = now_ms.saturating_add(HEARTBEAT_MS);
        let mut out = Vec::new();
        for i in 0..MAX_MASTERS {
            if self.names[i].is_empty() {
                continue;
            }
            if self.dirty[i] {
                self.dirty[i] = false;
                log::info!("Resolving {}", self.names[i]);
                match resolve(&self.names[i]) {
                    Some(mut a) => {
                        a.set_port(self.port);
                        log::info!("{} resolved to {a}", self.names[i]);
                        self.addrs[i] = Some(a);
                    }
                    None => {
                        log::info!("Couldn't resolve address: {}", self.names[i]);
                        self.names[i].clear();
                        self.addrs[i] = None;
                        continue;
                    }
                }
            }
            if let Some(a) = self.addrs[i]
                && self.allow(a, now_ms)
            {
                log::info!("Sending heartbeat to {}", self.names[i]);
                out.push((a, build_oob(&format!("heartbeat {text}\n"))));
            }
        }
        out
    }

    /// CoD2's second arm of `SV_MasterHeartbeat` (0x8096ed6): with
    /// `dedicated` 2, each [`STATUS_MS`] a `statusResponse` to every
    /// master the heartbeat resolved. Run after [`Self::heartbeat`] in the
    /// same frame, as retail's order is; where the heartbeat took the
    /// master's one packet that second, the status waits for the next
    /// second instead of going out with it (retail sends both at once).
    pub fn status(&mut self, dedicated: i32, now_ms: i32) -> Vec<SocketAddr> {
        if dedicated != 2 {
            return Vec::new();
        }
        if now_ms >= self.status_next_ms {
            self.status_next_ms = now_ms.saturating_add(STATUS_MS);
            self.status_owed = [true; MAX_MASTERS];
        }
        let mut out = Vec::new();
        for i in 0..MAX_MASTERS {
            if !self.status_owed[i] {
                continue;
            }
            match self.addrs[i] {
                Some(a) if !self.names[i].is_empty() => {
                    if self.limit.allow(a.ip(), now_ms.max(0) as u64) {
                        log::info!("Sending status to {}", self.names[i]);
                        out.push(a);
                        self.status_owed[i] = false;
                    }
                }
                // `SV_MasterAddress` reads `NA_BAD`: nothing goes.
                _ => self.status_owed[i] = false,
            }
        }
        out
    }

    /// `SV_MasterGameCompleteStatus` (0x808bbc4): nothing unless
    /// `dedicated` is 2. Returns what it prints and the masters to send the
    /// `gameCompleteStatus` packet to: a master the heartbeat resolved first
    /// has no address in this table, so it is named and sent nothing.
    pub fn game_complete_status(
        &mut self,
        dedicated: i32,
        now_ms: i32,
        resolve: &mut dyn FnMut(&str) -> Option<SocketAddr>,
    ) -> (String, Vec<SocketAddr>) {
        let (mut text, mut out) = (String::new(), Vec::new());
        if dedicated != 2 {
            return (text, out);
        }
        for i in 0..MAX_MASTERS {
            if self.names[i].is_empty() {
                continue;
            }
            if self.dirty[i] {
                self.dirty[i] = false;
                text.push_str(&format!("Resolving {}\n", self.names[i]));
                match resolve(&self.names[i]) {
                    Some(mut a) => {
                        a.set_port(self.port);
                        text.push_str(&format!("{} resolved to {a}\n", self.names[i]));
                        self.status_addrs[i] = Some(a);
                    }
                    None => {
                        text.push_str(&format!("Couldn't resolve address: {}\n", self.names[i]));
                        self.names[i].clear();
                        self.status_addrs[i] = None;
                        continue;
                    }
                }
            }
            text.push_str(&format!(
                "Sending gameCompleteStatus to {}\n",
                self.names[i]
            ));
            if let Some(a) = self.status_addrs[i]
                && self.allow(a, now_ms)
            {
                out.push(a);
            }
        }
        (text, out)
    }

    /// `SV_MasterShutdown` (0x808d268): one `flatline`, sent at once.
    pub fn shutdown(
        &mut self,
        dedicated: i32,
        now_ms: i32,
        resolve: &mut dyn FnMut(&str) -> Option<SocketAddr>,
    ) -> Vec<(SocketAddr, Vec<u8>)> {
        self.next_ms = -9999;
        self.heartbeat(dedicated, now_ms, FLATLINE, resolve)
    }
}

/// `NET_StringToAdr` for a master name: the first IPv4 address it resolves
/// to. [`Masters`] replaces the port with the protocol's either way.
pub fn resolve(name: &str) -> Option<SocketAddr> {
    let host = name.rsplit_once(':').map_or(name, |(h, _)| h);
    (host, 0).to_socket_addrs().ok()?.find(SocketAddr::is_ipv4)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn local(_: &str) -> Option<SocketAddr> {
        Some("127.0.0.1:29463".parse().unwrap())
    }

    /// The heartbeat and `gameCompleteStatus` keep separate address tables
    /// behind one `modified` flag: whichever resolves a master first is the
    /// only one that can reach it.
    #[test]
    fn game_complete_status_reaches_only_masters_it_resolved_itself() {
        let mut m = Masters::default();
        m.set(0, "127.0.0.1");
        assert_eq!(m.heartbeat(2, 0, HEARTBEAT_GAME, &mut local).len(), 1);
        m.set(1, "127.0.0.1");
        let (text, to) = m.game_complete_status(2, 1000, &mut local);
        assert_eq!(
            text,
            "Sending gameCompleteStatus to 127.0.0.1\nResolving 127.0.0.1\n\
             127.0.0.1 resolved to 127.0.0.1:20510\n\
             Sending gameCompleteStatus to 127.0.0.1\n"
        );
        assert_eq!(to, vec!["127.0.0.1:20510".parse().unwrap()]);
        m.force();
        assert_eq!(m.heartbeat(2, 2000, HEARTBEAT_GAME, &mut local).len(), 1);
        assert_eq!(
            m.game_complete_status(1, 3000, &mut local),
            (String::new(), vec![])
        );
    }

    #[test]
    fn heartbeats_only_on_dedicated_2_every_three_minutes() {
        let mut m = Masters::default();
        assert!(m.heartbeat(1, 50, HEARTBEAT_GAME, &mut local).is_empty());
        let out = m.heartbeat(2, 50, HEARTBEAT_GAME, &mut local);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].0, "127.0.0.1:20510".parse().unwrap());
        assert_eq!(out[0].1, b"\xff\xff\xff\xffheartbeat COD-1\n");
        assert!(
            m.heartbeat(2, 50 + HEARTBEAT_MS - 1, HEARTBEAT_GAME, &mut local)
                .is_empty()
        );
        assert_eq!(
            m.heartbeat(2, 50 + HEARTBEAT_MS, HEARTBEAT_GAME, &mut local)
                .len(),
            1
        );
        m.force();
        assert_eq!(
            m.heartbeat(2, 50 + HEARTBEAT_MS + 1000, HEARTBEAT_GAME, &mut local)
                .len(),
            1
        );
    }

    /// A forced heartbeat inside a second of the last packet to that host
    /// is dropped, and the timer still runs from it.
    #[test]
    fn one_packet_a_second_to_a_master() {
        let mut m = Masters::default();
        assert_eq!(m.heartbeat(2, 0, HEARTBEAT_GAME, &mut local).len(), 1);
        m.force();
        assert!(m.heartbeat(2, 999, HEARTBEAT_GAME, &mut local).is_empty());
        assert!(m.shutdown(2, 999, &mut local).is_empty());
        assert!(m.game_complete_status(2, 999, &mut local).1.is_empty());
        assert_eq!(m.shutdown(2, 1000, &mut local).len(), 1);
    }

    #[test]
    fn the_protocols_master_sets_every_port() {
        let mut m = Masters::new(vcod_common::net::master::UO_MASTER);
        m.set(1, "127.0.0.2");
        let mut r = |n: &str| {
            Some(SocketAddr::new(
                n.parse().unwrap_or([127, 0, 0, 1].into()),
                0,
            ))
        };
        let out = m.heartbeat(2, 0, HEARTBEAT_GAME, &mut r);
        let to: Vec<String> = out.iter().map(|(a, _)| a.to_string()).collect();
        assert_eq!(to, ["127.0.0.1:20610", "127.0.0.2:20610"]);
        assert_eq!(m.name(0), "master.cod.pm");
    }

    #[test]
    fn flatline_ignores_the_timer_but_not_dedicated() {
        let mut m = Masters::default();
        assert_eq!(m.heartbeat(2, 50, HEARTBEAT_GAME, &mut local).len(), 1);
        let out = m.shutdown(2, 1050, &mut local);
        assert_eq!(out[0].1, b"\xff\xff\xff\xffheartbeat flatline\n");
        assert!(Masters::default().shutdown(1, 0, &mut local).is_empty());
    }

    #[test]
    fn an_unresolvable_master_is_cleared_and_not_retried() {
        let mut m = Masters::default();
        let mut calls = 0;
        let mut fail = |_: &str| {
            calls += 1;
            None
        };
        assert!(m.heartbeat(2, 0, HEARTBEAT_GAME, &mut fail).is_empty());
        m.force();
        assert!(m.heartbeat(2, 50, HEARTBEAT_GAME, &mut fail).is_empty());
        assert_eq!(calls, 1);
    }

    #[test]
    fn every_master_slot_is_sent_and_only_a_set_one_resolves_again() {
        let mut m = Masters::default();
        m.set(Masters::index("SV_MASTER3").unwrap(), "127.0.0.2:1234");
        let mut calls = Vec::new();
        let mut r = |n: &str| {
            calls.push(n.to_string());
            let ip = if n == DEFAULT_MASTER.host {
                [127, 0, 0, 1]
            } else {
                [127, 0, 0, 2]
            };
            Some(SocketAddr::from((ip, 29463)))
        };
        assert_eq!(m.heartbeat(2, 0, HEARTBEAT_GAME, &mut r).len(), 2);
        m.force();
        assert_eq!(m.heartbeat(2, 1000, HEARTBEAT_GAME, &mut r).len(), 2);
        assert_eq!(calls, [DEFAULT_MASTER.host, "127.0.0.2:1234"]);
        assert_eq!(Masters::index("sv_master6"), None);
        assert_eq!(Masters::index("sv_masterx"), None);
    }
}
