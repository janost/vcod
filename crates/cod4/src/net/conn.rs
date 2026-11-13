//! A 1.7 client's connection to a retail server: the join the 1.7 client
//! makes (challenge, connect, the seven stats packets), the `cp` answer to
//! the gamestate, then move messages keyed and scrambled as the client's
//! and server messages parsed into [`Client`]
//! (docs/research/cod4-handshake.md, cod4-netfields.md 5 and 8).
//! `examples/cod4_probe.rs` and the vcod client both join through it.

use super::clmsg::{self, ClOp, CmdWire, UserCmd};
use super::parse::{Client, Op, Parsed};
use super::pure;
use super::stats::{STAT_PACKETS, Stats};
use super::voice;
use anyhow::{Context, Result, bail};
use std::net::{SocketAddr, UdpSocket};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};
use vcod_common::net::cdkey::{self, CdKey, KeyGame};
use vcod_common::net::netchan::{Netchan, NetchanParams, scramble_client_ops};

/// md5("vcodprobe"), the cd-key hash `getchallenge` carries without a key.
pub const KEY_HASH: &str = "97c8407fc937bc7c1eddecf36de89e75";

/// The clock a [`Conn`] on this thread waits and times out on: the
/// wall's, or one a test drives ([`set_thread_clock`]).
type ThreadClock = (fn() -> Instant, fn(Duration));

thread_local! {
    static CLOCK: std::cell::Cell<Option<ThreadClock>> = const { std::cell::Cell::new(None) };
}

/// Puts this thread's connections on `now` and `sleep` instead of the
/// wall clock, so a test that runs its server on a clock of its own
/// joins in step with it: every wait and deadline of the join on that
/// clock, none on how fast the machine runs.
pub fn set_thread_clock(now: fn() -> Instant, sleep: fn(Duration)) {
    CLOCK.with(|c| c.set(Some((now, sleep))));
}

/// Now on this thread's clock.
fn now() -> Instant {
    CLOCK
        .with(|c| c.get())
        .map_or_else(Instant::now, |(now, _)| now())
}

/// Sleeps on this thread's clock.
fn sleep(d: Duration) {
    match CLOCK.with(|c| c.get()) {
        Some((_, sleep)) => sleep(d),
        None => std::thread::sleep(d),
    }
}

/// The server's refusal of the challenge: an `EXE_*` reason to localize,
/// or its `error` text.
#[derive(Debug)]
pub struct Refused(pub String);

impl std::fmt::Display for Refused {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Refused {}

/// The join was called off (Esc on the connect screen) through
/// [`Join::cancel`].
#[derive(Debug)]
pub struct Cancelled;

impl std::fmt::Display for Cancelled {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("connect cancelled")
    }
}

impl std::error::Error for Cancelled {}

/// What the join tells a connect screen as it goes (cod4-hud.md 18).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum JoinNote {
    /// `CL_Connect`: a join starts, `clc.serverMessage` cleared
    /// (`iw3mp.exe` 0x46959f).
    Challenging,
    /// `clc.serverMessage`: a `print`'s text (0x46bc4f), or
    /// `EXE_AWAITINGCDKEYAUTH` on `needcdkey` (0x46be56), a key the
    /// screen localizes.
    Message(String),
    /// `connectResponse`: `CA_CONNECTED`, past the lines' state.
    Connected,
}

/// Told each [`JoinNote`]; shared across a download's reconnects.
pub type Notes = Arc<dyn Fn(JoinNote) + Send + Sync>;
/// The server's own test-client userinfo (cod4-handshake.md 3) with
/// `rate` 25000; `name`, `protocol`, `challenge` and `qport` follow.
const USERINFO: &str = "\\cg_predictItems\\1\\cl_punkbuster\\0\\cl_anonymous\\0\\color\\4\
\\head\\default\\model\\multi\\snaps\\20\\rate\\25000";

/// What the first reliable command after a gamestate is
/// (cod4-handshake.md 8).
#[derive(Clone, Debug, Default)]
pub enum Cp {
    /// None: a pure server drops the client.
    None,
    /// `cp @ <checksumFeed>`, which passes the server's check.
    #[default]
    Bare,
    /// Every listed iwd's pure checksum, read off this `main/`.
    Iwds(PathBuf),
}

/// Sees every datagram: `c2s`, `s2c`, or `auth` (a datagram on one of
/// [`Join::auth`]'s sockets), the peer and the bytes.
pub type Tap = Box<dyn FnMut(&str, SocketAddr, &[u8]) -> Result<()> + Send>;

pub struct Join {
    pub name: String,
    /// The userinfo's `password` (`g_password`), left out when empty.
    pub password: String,
    pub qport: u16,
    pub cp: Cp,
    /// Sockets standing in for the authorize servers, read alongside the
    /// server's so the tap logs what the server sends them.
    pub auth: Vec<UdpSocket>,
    pub tap: Option<Tap>,
    /// `cl_wwwDownload`, a userinfo dvar (flags 3, iw3mp.exe 0x46e304):
    /// on, the userinfo says so and a `sv_wwwDownload` server redirects.
    pub www: bool,
    /// Set from another thread, every read after it fails with
    /// [`Cancelled`], so a join waiting on a silent server ends at once.
    pub cancel: Option<Arc<AtomicBool>>,
    /// The connect screen's [`JoinNote`]s.
    pub notes: Option<Notes>,
    /// `cl_voice`, a userinfo dvar (default 1, flags 3, `iw3mp.exe`
    /// 0x46d74c): the server relays voice only to a client that sets it
    /// (cod4-voice.md 3).
    pub voice: bool,
    /// The profile's stats block the seven `stats` packets carry
    /// (`vcod_mpdata`); a fresh profile's without one.
    pub stats: Option<Stats>,
}

impl Default for Join {
    fn default() -> Self {
        Join {
            name: "vcod".into(),
            password: String::new(),
            qport: 0x1234,
            cp: Cp::Bare,
            auth: Vec::new(),
            tap: None,
            www: false,
            cancel: None,
            notes: None,
            voice: false,
            stats: None,
        }
    }
}

/// One server message read: its netchan sequence and its ops (the
/// snapshots and gamestate land in [`Conn::client`]).
pub struct Received {
    pub sequence: u32,
    pub parsed: Parsed,
    /// The message as a demo records it: the plain `reliableAcknowledge`,
    /// then the unscrambled, still compressed block (cod4-demos.md 2).
    pub msg: Vec<u8>,
}

/// A joined connection: [`Conn::send`] moves, [`Conn::poll`] the server.
pub struct Conn {
    /// What the `stats` packets carry ([`Join::stats`]).
    stats: Stats,
    sock: UdpSocket,
    server: SocketAddr,
    auth: Vec<UdpSocket>,
    tap: Option<Tap>,
    qport: u16,
    challenge: i32,
    cp: Cp,
    chan: Netchan,
    pub client: Client,
    /// The next outgoing netchan sequence.
    seq: u32,
    /// The newest server message read (`messageAcknowledge`).
    ack: i32,
    /// Reliable client commands, index = sequence - 1.
    commands: Vec<Vec<u8>>,
    /// The newest the server executed (each server message's first long).
    commands_acked: i32,
    last_cmd: UserCmd,
    /// The checksum feed of the gamestate the last `cp` answered.
    cp_feed: Option<i32>,
    /// The newest server command applied to the gamestate's table.
    applied: i32,
    /// `loadingnewmap`'s map, until its gamestate comes.
    pub loading: Option<String>,
    /// When `requeststats` last sent the seven packets.
    stats_sent: Option<Instant>,
    /// An out-of-band `disconnect` from the server and its reason (empty
    /// for none), until the session takes it ([`Conn::take_disconnect`]).
    disconnect: Option<String>,
    /// [`Join::cancel`].
    cancel: Option<Arc<AtomicBool>>,
    /// [`Join::notes`].
    notes: Option<Notes>,
    /// `clc.demowaiting`: moves go as `clc_moveNoDelta` (`CL_WritePacket`,
    /// `iw3mp.exe` 0x4640e1), so the server sends a full frame.
    pub no_delta: bool,
    /// The server's voice packets read since the last [`Conn::take_voice`].
    voice_in: voice::Talked,
    /// The server's out-of-band `print`s once joined (rcon's answers),
    /// until [`Conn::take_prints`].
    prints: Vec<String>,
    /// The userinfo dvars as the connect sent them, which a change sends
    /// again ([`Conn::set_name`]).
    userinfo: String,
}

impl Conn {
    /// Challenge, connect and stats, as the 1.7 client joins; returns once
    /// the server wants no more stats. Resends each ask every 3 s.
    pub fn join(server: SocketAddr, opts: Join) -> Result<Conn> {
        let sock = UdpSocket::bind(if server.is_ipv4() {
            "0.0.0.0:0"
        } else {
            "[::]:0"
        })?;
        sock.set_nonblocking(true)?;
        for a in &opts.auth {
            a.set_nonblocking(true)?;
        }
        let mut c = Conn {
            sock,
            server,
            auth: opts.auth,
            tap: opts.tap,
            qport: opts.qport,
            challenge: 0,
            cp: opts.cp,
            chan: Netchan::with_params(opts.qport, 0, NetchanParams::COD2),
            client: Client::new(),
            seq: 1,
            ack: 0,
            commands: Vec::new(),
            commands_acked: 0,
            last_cmd: UserCmd::default(),
            cp_feed: None,
            applied: 0,
            loading: None,
            stats_sent: None,
            stats: opts.stats.unwrap_or_else(Stats::fresh),
            disconnect: None,
            cancel: opts.cancel,
            notes: opts.notes,
            no_delta: false,
            voice_in: Vec::new(),
            prints: Vec::new(),
            userinfo: String::new(),
        };
        c.note(JoinNote::Challenging);
        let key = cdkey::lookup(KeyGame::Cod4);
        let hash = key.as_ref().map(|k| k.hash(KeyGame::Cod4));
        let ask = format!("getchallenge 0 \"{}\"", hash.as_deref().unwrap_or(KEY_HASH));
        let chal = c.challenge_reply(ask.as_bytes(), key.as_ref())?;
        if let Some(x) = cod4x_proto(&chal) {
            return Err(Refused(cod4x_refusal(x)).into());
        }
        c.challenge = String::from_utf8_lossy(&chal)
            .split_whitespace()
            .nth(1)
            .context("challenge")?
            .parse()?;
        let www = if opts.www { "\\cl_wwwDownload\\1" } else { "" };
        let vc = if opts.voice { "\\cl_voice\\1" } else { "" };
        let mut ui = format!(
            "{USERINFO}{www}{vc}\\name\\{}\\protocol\\6\\challenge\\{}\\qport\\{}",
            opts.name, c.challenge, c.qport
        );
        if !opts.password.is_empty() {
            ui.push_str(&format!("\\password\\{}", opts.password));
        }
        c.userinfo = format!("{USERINFO}{www}{vc}\\name\\{}", opts.name);
        if !opts.password.is_empty() {
            c.userinfo
                .push_str(&format!("\\password\\{}", opts.password));
        }
        c.oob_reply(
            format!("connect \"{ui}\"").as_bytes(),
            b"connectResponse",
            10,
        )?
        .context("no connectResponse")?;
        c.note(JoinNote::Connected);

        // Stats: the lowest index the server still wants, until its mask is 0.
        let mut want = (1u32 << STAT_PACKETS) - 1;
        let deadline = now() + Duration::from_secs(15);
        while want != 0 && now() < deadline {
            let idx = want.trailing_zeros() as usize;
            c.send_stat(idx)?;
            let stop = now() + Duration::from_millis(500);
            while let Some(r) = c.recv(stop)? {
                if let Some(rest) = r.get(4..).and_then(|b| b.strip_prefix(b"statResponse ")) {
                    want = String::from_utf8_lossy(rest).trim().parse()?;
                    break;
                }
            }
        }
        if want != 0 {
            bail!("stats not accepted (mask {want})");
        }
        c.chan = Netchan::with_params(c.qport, c.challenge, NetchanParams::COD2);
        Ok(c)
    }

    pub fn server(&self) -> SocketAddr {
        self.server
    }

    /// Queues a reliable command; returns its sequence.
    pub fn command(&mut self, text: impl Into<Vec<u8>>) -> usize {
        self.commands.push(text.into());
        self.commands.len()
    }

    /// Every reliable command queued, oldest first.
    pub fn commands(&self) -> &[Vec<u8>] {
        &self.commands
    }

    /// `clc.reliableSequence`: the newest reliable command queued.
    pub fn reliable_sequence(&self) -> i32 {
        self.commands.len() as i32
    }

    /// `clc.serverMessageSequence`: the newest server message read.
    pub fn message_sequence(&self) -> i32 {
        self.ack
    }

    /// The newest usercmd sent (its `server_time` the floor for the next).
    pub fn last_cmd(&self) -> &UserCmd {
        &self.last_cmd
    }

    /// One move message: the unacked commands and `cmds`, the first on the
    /// long branch with every field, the rest the fewest bits off the one
    /// before (no move op without cmds). Before the gamestate it is the
    /// bare header the client sends while it waits.
    pub fn send(&mut self, cmds: &[UserCmd]) -> Result<()> {
        let mut pkt = self.seq.to_le_bytes().to_vec();
        self.seq += 1;
        pkt.extend_from_slice(&self.qport.to_le_bytes());
        match &self.client.gamestate {
            None => {
                pkt.push(0);
                pkt.extend_from_slice(&self.ack.to_le_bytes());
                pkt.extend_from_slice(&0i32.to_le_bytes());
            }
            Some(gs) => {
                let server_id: i32 = gs
                    .systeminfo("sv_serverid")
                    .context("sv_serverid")?
                    .parse()?;
                let last = self.client.last_server_command().to_vec();
                let key = clmsg::usercmd_key(gs.checksum_feed, self.ack, &last);
                let mut ops: Vec<ClOp> = (self.commands_acked as usize..self.commands.len())
                    .map(|i| ClOp::Command {
                        sequence: i as i32 + 1,
                        text: self.commands[i].clone(),
                    })
                    .collect();
                let mut wired: Vec<(UserCmd, CmdWire)> = Vec::new();
                for c in cmds {
                    let wire = match wired.last() {
                        None => CmdWire::full(&UserCmd::default(), c),
                        Some((prev, _)) => CmdWire::natural(prev, c),
                    };
                    wired.push((*c, wire));
                    self.last_cmd = *c;
                }
                if !wired.is_empty() {
                    ops.push(ClOp::Move {
                        delta: !self.client.snapshots.is_empty() && !self.no_delta,
                        cmds: wired,
                        // Far off on purpose, so the server always sends
                        // origin and velocity.
                        origin: [1.0e6f32.to_bits(); 3],
                        origin_time: 0,
                    });
                }
                ops.push(ClOp::Eof);
                let mut comp =
                    clmsg::compress_ops(&self.client.huff, &ops, key, &UserCmd::default());
                let seed = self.challenge as u32 ^ server_id as u32 ^ self.ack as u32;
                scramble_client_ops(seed as u8, &last, &mut comp);
                pkt.push(server_id as u8);
                pkt.extend_from_slice(&self.ack.to_le_bytes());
                pkt.extend_from_slice(&self.client.server_command_sequence.to_le_bytes());
                pkt.extend_from_slice(&comp);
            }
        }
        self.send_raw(&pkt)
    }

    /// `disconnect` as a reliable command, sent three times as Q3's
    /// `CL_Disconnect` does; the server frees the slot at once.
    pub fn disconnect(&mut self) -> Result<()> {
        if self.client.gamestate.is_none() {
            return Ok(());
        }
        self.command("disconnect");
        for _ in 0..3 {
            self.send(&[])?;
        }
        Ok(())
    }

    /// The server messages read until `until` (at least everything already
    /// queued). A gamestate queues the `cp` ahead of any move, as the client
    /// sends it once the level is loaded, before its first usercmd.
    pub fn poll(&mut self, until: Instant) -> Result<Vec<Received>> {
        let mut out = Vec::new();
        while let Some(d) = self.recv(until)? {
            if d.len() >= 4 && d[..4] == [0xff; 4] {
                // `CL_ConnectionlessPacket` hands `v` to `CL_VoicePacket`
                // (`iw3mp.exe` 0x46b60b).
                if let Some(body) = d[4..].strip_prefix(b"v\0") {
                    self.voice_in.extend(voice::parse_server(body));
                    continue;
                }
                self.connectionless(&d[4..])?;
                continue;
            }
            if d.len() < 4 {
                continue;
            }
            let Some((s, mut msg)) = self.chan.frame(&d)? else {
                continue;
            };
            if msg.len() < 4 {
                continue;
            }
            let rack = i32::from_le_bytes(msg[..4].try_into().unwrap());
            let key = match rack {
                1.. => self
                    .commands
                    .get(rack as usize - 1)
                    .cloned()
                    .unwrap_or_default(),
                _ => Vec::new(),
            };
            self.chan.decode(s, &key, &mut msg);
            self.commands_acked = self.commands_acked.max(rack);
            let parsed = self
                .client
                .parse(s, &msg[4..])
                .with_context(|| format!("message {s}"))?;
            self.ack = s as i32;
            self.apply_configstrings(&parsed);
            if parsed.ops.iter().any(|o| matches!(o, Op::Gamestate)) {
                self.answer_gamestate()?;
            }
            out.push(Received {
                sequence: s,
                parsed,
                msg,
            });
        }
        Ok(out)
    }

    /// The voice packets the server relayed since the last call, (talker,
    /// Speex frame).
    pub fn take_voice(&mut self) -> voice::Talked {
        std::mem::take(&mut self.voice_in)
    }

    /// `CL_WriteVoicePacket`: the batched frames out of band.
    pub fn send_voice(&mut self, frames: &[Vec<u8>]) -> Result<()> {
        let p = voice::client_packet(self.qport, frames);
        self.send_raw(&p)
    }

    /// The server's out-of-band `disconnect` since the last call, with its
    /// reason (`CL_DisconnectPacket`, `iw3mp.exe` 0x46b510, honours it only
    /// 3 s past the last sequenced message; the caller tests that).
    pub fn take_disconnect(&mut self) -> Option<String> {
        self.disconnect.take()
    }

    /// A new `name` (a userinfo dvar): `CL_CheckUserinfo` (`iw3mp.exe`
    /// 0x46c680) sends `userinfo "<info>"` on the frame a userinfo dvar
    /// changed, from `CA_CHALLENGING` on. The info is the dvars', without
    /// the connect's `protocol`, `challenge` and `qport`. Nothing goes
    /// when the name is the same.
    pub fn set_name(&mut self, name: &str) {
        self.set_userinfo("name", name);
    }

    /// Any userinfo dvar the connect sent (`name`, `snaps`, `rate`), sent
    /// as [`Conn::set_name`] sends a name.
    pub fn set_userinfo(&mut self, key: &str, value: &str) {
        let pairs = vcod_common::net::connectionless::info_pairs(&self.userinfo);
        if pairs.iter().any(|&(k, v)| k == key && v == value) {
            return;
        }
        let mut ui = String::new();
        for (k, v) in pairs {
            let v = if k == key { value } else { v };
            ui.push_str(&format!("\\{k}\\{v}"));
        }
        self.userinfo = ui;
        self.command(format!("userinfo \"{}\"", self.userinfo));
    }

    /// The server's out-of-band `print` texts since the last call:
    /// `CL_ConnectionlessPacket` prints them to the console in any state.
    pub fn take_prints(&mut self) -> Vec<String> {
        std::mem::take(&mut self.prints)
    }

    /// An out-of-band packet to the server (`rcon`'s, `CL_Rcon_f`,
    /// `iw3mp.exe` 0x471af0).
    pub fn send_oob(&mut self, text: &[u8]) -> Result<()> {
        self.oob(text)
    }

    /// The block a later `requeststats` sends: the profile's as the
    /// server's `N`s and the menus' `statset`s left it. Retail's packets
    /// read the live `LiveStorage` buffer those write (`iw3mp.exe`
    /// 0x46af95; cod4-netfields.md, "The stats file").
    pub fn set_stats(&mut self, stats: &Stats) {
        if self.stats.0[..] != stats.0[..] {
            self.stats.0.copy_from_slice(&stats.0[..]);
        }
    }

    /// Stats packet `idx` of the profile's block.
    fn send_stat(&mut self, idx: usize) -> Result<()> {
        let mut p = b"stats\0".to_vec();
        p.extend_from_slice(&self.qport.to_le_bytes());
        p.push(idx as u8);
        p.extend_from_slice(self.stats.packet(idx));
        self.oob(&p)
    }

    /// The server's out-of-band lines once joined: `loadingnewmap` (a map
    /// change, its map kept until the gamestate), `requeststats` (the
    /// seven stats packets again, as `CL_ConnectionlessPacket`
    /// starts `CA_SENDINGSTATS`), `statResponse` (the packets its mask
    /// still wants).
    fn connectionless(&mut self, text: &[u8]) -> Result<()> {
        let line = String::from_utf8_lossy(text);
        if let Some(body) = line.strip_prefix("print\n") {
            let body = body.split('\0').next().unwrap_or_default();
            self.prints.push(body.to_string());
            return Ok(());
        }
        let mut lines = line.split('\n');
        match lines.next().unwrap_or("").trim_end_matches('\0') {
            "loadingnewmap" => {
                self.loading = lines.next().map(str::to_string);
                log::info!("cod4: server loading {:?}", self.loading);
            }
            // The server asks once a message until the stats are in
            // (nine times on retail's map change, VERIFIED live); one
            // asking while the last burst is under a second old is that
            // burst's, as `CA_SENDINGSTATS` ignores it.
            "requeststats"
                if self.stats_sent.is_none_or(|t| {
                    now().saturating_duration_since(t) > Duration::from_secs(1)
                }) =>
            {
                self.stats_sent = Some(now());
                for idx in 0..STAT_PACKETS {
                    self.send_stat(idx)?;
                }
            }
            "fastrestart" => log::debug!("cod4: server fast restart"),
            l if l == "disconnect" || l.starts_with("disconnect ") => {
                self.disconnect = Some(l["disconnect".len()..].trim().to_string());
            }
            l if l.starts_with("statResponse ") => {
                let want: u32 = l["statResponse ".len()..].trim().parse().unwrap_or(0);
                for idx in (0..STAT_PACKETS).filter(|i| want & (1 << i) != 0) {
                    self.send_stat(idx)?;
                }
            }
            _ => {}
        }
        Ok(())
    }

    fn apply_configstrings(&mut self, parsed: &Parsed) {
        apply_configstrings(&mut self.client, &mut self.applied, parsed);
    }

    /// `donedl`: the server sends the gamestate again, and that one is
    /// answered with its own `cp` though the feed is the same.
    pub fn done_downloading(&mut self) -> usize {
        self.cp_feed = None;
        self.command("donedl")
    }

    /// `wwwdl fail`: the server answers with the gamestate again
    /// (cod4-handshake.md 11), which, like `donedl`'s, needs its own `cp`.
    pub fn www_failed(&mut self) -> usize {
        self.cp_feed = None;
        self.command("wwwdl fail")
    }

    fn answer_gamestate(&mut self) -> Result<()> {
        let gs = self.client.gamestate.as_ref().context("gamestate")?;
        if self.cp_feed == Some(gs.checksum_feed) {
            return Ok(());
        }
        self.cp_feed = Some(gs.checksum_feed);
        self.loading = None;
        // The first move's writer builds it otherwise, in the first frame
        // after the load (cod4-client-perf.md 17).
        std::sync::LazyLock::force(&super::parse::UNUSED_HUFF);
        let si = String::from_utf8_lossy(gs.configstring(1)).into_owned();
        let cp = match &self.cp {
            Cp::None => return Ok(()),
            Cp::Bare => pure::cp_command(&[], gs.checksum_feed),
            Cp::Iwds(dir) => pure::pure_command(dir, &si, gs.checksum_feed)?,
        };
        self.command(cp);
        Ok(())
    }

    fn note(&self, n: JoinNote) {
        if let Some(f) = &self.notes {
            f(n);
        }
    }

    fn tap(&mut self, tag: &str, peer: SocketAddr, data: &[u8]) -> Result<()> {
        match &mut self.tap {
            Some(t) => t(tag, peer, data),
            None => Ok(()),
        }
    }

    fn send_raw(&mut self, data: &[u8]) -> Result<()> {
        self.sock.send_to(data, self.server)?;
        self.tap("c2s", self.server, data)
    }

    fn oob(&mut self, text: &[u8]) -> Result<()> {
        let mut d = vec![0xff; 4];
        d.extend_from_slice(text);
        self.send_raw(&d)
    }

    /// The next datagram from the server before `until`, else `None`; one
    /// already waiting is returned even past it. Taps authorize traffic on
    /// the way.
    fn recv(&mut self, until: Instant) -> Result<Option<Vec<u8>>> {
        let mut buf = vec![0u8; 65536];
        loop {
            if self
                .cancel
                .as_ref()
                .is_some_and(|c| c.load(Ordering::Relaxed))
            {
                return Err(Cancelled.into());
            }
            for i in 0..self.auth.len() {
                if let Ok((n, _)) = self.auth[i].recv_from(&mut buf) {
                    let local = self.auth[i].local_addr()?;
                    let d = buf[..n].to_vec();
                    self.tap("auth", local, &d)?;
                }
            }
            match self.sock.recv_from(&mut buf) {
                Ok((n, peer)) if peer == self.server => {
                    let d = buf[..n].to_vec();
                    self.tap("s2c", peer, &d)?;
                    // `CL_ConnectionlessPacket`'s `print` in any state.
                    if let Some(text) = d.strip_prefix(b"\xff\xff\xff\xffprint\n") {
                        let text = text.split(|&b| b == 0).next().unwrap_or_default();
                        self.note(JoinNote::Message(
                            String::from_utf8_lossy(text).into_owned(),
                        ));
                    }
                    return Ok(Some(d));
                }
                Ok(_) => continue,
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
                Err(e) if e.kind() == std::io::ErrorKind::ConnectionRefused => {}
                Err(e) => return Err(e.into()),
            }
            let now = now();
            if now >= until {
                return Ok(None);
            }
            sleep((until - now).min(Duration::from_millis(1)));
        }
    }

    /// `CL_CheckForResend`'s challenge (iw3mp.exe 0x46b1d7): every 3 s the
    /// key to the authorize server ([`cdkey::request_authorization`],
    /// nothing for a LAN server), then `ask`, for 30 s. `needcdkey`
    /// without a key refuses with `EXE_ERR_INVALID_CD_KEY`, where retail
    /// stops before it sends; with one the key goes again, and a timeout
    /// then reads `EXE_AWAITINGCDKEYAUTH` (docs/research/cod4-handshake.md).
    fn challenge_reply(&mut self, ask: &[u8], key: Option<&CdKey>) -> Result<Vec<u8>> {
        let end = now() + Duration::from_secs(30);
        let mut awaiting_key = false;
        while now() < end {
            if let Some(k) = key {
                cdkey::request_authorization(KeyGame::Cod4, k, self.server);
            }
            self.oob(ask)?;
            let stop = now() + Duration::from_secs(3);
            while let Some(d) = self.recv(stop)? {
                let Some(text) = d.strip_prefix(&[0xff; 4]) else {
                    continue;
                };
                if text.starts_with(b"challengeResponse") {
                    return Ok(text.to_vec());
                }
                if let Some(why) = text.strip_prefix(b"error\n") {
                    let why = String::from_utf8_lossy(why);
                    return Err(Refused(why.trim_end_matches(['\n', '\0']).to_string()).into());
                }
                if text.starts_with(b"needcdkey") {
                    self.note(JoinNote::Message("EXE_AWAITINGCDKEYAUTH".into()));
                    let Some(k) = key else {
                        cdkey::warn_no_key();
                        return Err(Refused("EXE_ERR_INVALID_CD_KEY".into()).into());
                    };
                    awaiting_key = true;
                    cdkey::request_authorization(KeyGame::Cod4, k, self.server);
                }
            }
        }
        if awaiting_key {
            return Err(Refused("EXE_AWAITINGCDKEYAUTH".into()).into());
        }
        bail!("no challengeResponse")
    }

    /// Resends `ask` every 3 s until an OOB reply starting with `prefix`.
    fn oob_reply(&mut self, ask: &[u8], prefix: &[u8], secs: u64) -> Result<Option<Vec<u8>>> {
        let end = now() + Duration::from_secs(secs);
        while now() < end {
            self.oob(ask)?;
            let stop = now() + Duration::from_secs(3);
            while let Some(d) = self.recv(stop)? {
                if d.len() > 4 && d[..4] == [0xff; 4] && d[4..].starts_with(prefix) {
                    return Ok(Some(d[4..].to_vec()));
                }
                // A refusal (`GAME_INVALIDPASSWORD`, `EXE_SERVERISFULL`)
                // ends the join, as `CL_ConnectionlessPacket`'s `error`.
                if let Some(why) = d
                    .strip_prefix(&[0xff; 4])
                    .and_then(|t| t.strip_prefix(b"error\n"))
                {
                    let why = String::from_utf8_lossy(why);
                    return Err(Refused(why.trim_end_matches(['\n', '\0']).to_string()).into());
                }
            }
        }
        Ok(None)
    }
}

/// The `xproto` a CoD4x server appends to its `challengeResponse`
/// (`challengeResponse <challenge> <client challenge> 0 xproto 18`); a
/// stock 1.7 server sends none (cod4-handshake.md 13).
pub fn cod4x_proto(reply: &[u8]) -> Option<&str> {
    let text = std::str::from_utf8(reply).ok()?;
    let mut words = text.split_whitespace();
    words.find(|w| *w == "xproto")?;
    Some(words.next().unwrap_or(""))
}

/// Why vcod does not go on into a CoD4x server: it parks a protocol 6
/// client and relays it to CoD4x's client-update backend, which never
/// lets a 1.7 client play (cod4-handshake.md 13).
pub fn cod4x_refusal(xproto: &str) -> String {
    format!(
        "This server runs CoD4x (xproto {xproto}). CoD4x hands a 1.7 client to its own client \
         update, which vcod does not install; vcod joins stock 1.7 servers only."
    )
}

/// `ANGLE2SHORT`.
pub fn angle2short(deg: f32) -> u16 {
    ((deg * 65536.0 / 360.0) as i32 & 0xffff) as u16
}

/// `CL_ConfigstringModified`: each `d <index> <string>` newer than the
/// table into it; one read ahead of a gamestate in the same message is
/// the last level's. `applied` is the newest command already applied.
pub fn apply_configstrings(client: &mut Client, applied: &mut i32, parsed: &Parsed) {
    let from = parsed
        .ops
        .iter()
        .rposition(|o| matches!(o, Op::Gamestate))
        .map_or(0, |i| i + 1);
    if from > 0 {
        *applied = client
            .gamestate
            .as_ref()
            .map_or(0, |g| g.server_command_sequence);
    }
    for op in &parsed.ops[from..] {
        let Op::ServerCommand { sequence, text } = op else {
            continue;
        };
        if *sequence <= *applied {
            continue;
        }
        *applied = *sequence;
        let (Some(rest), Some(gs)) = (text.strip_prefix(b"d "), client.gamestate.as_mut()) else {
            continue;
        };
        let at = rest.iter().position(|&b| b == b' ').unwrap_or(rest.len());
        if let Some(i) = std::str::from_utf8(&rest[..at])
            .ok()
            .and_then(|n| n.parse::<u16>().ok())
        {
            gs.configstrings
                .insert(i, rest.get(at + 1..).unwrap_or(&[]).to_vec());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The challenge replies of a CoD4x server and of retail 1.7
    /// (cod4-handshake.md 3, 13).
    #[test]
    fn a_cod4x_challenge_carries_xproto() {
        assert_eq!(
            cod4x_proto(b"challengeResponse 1357851985 123 0 xproto 18"),
            Some("18")
        );
        assert_eq!(cod4x_proto(b"challengeResponse -1257716235"), None);
    }
}
