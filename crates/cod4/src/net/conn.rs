//! A 1.7 client's connection to a retail server: the join the 1.7 client
//! makes (challenge, connect, the seven stats packets), the `cp` answer to
//! the gamestate, then move messages keyed and scrambled as the client's
//! and server messages parsed into [`Client`]
//! (docs/research/cod4-handshake.md, cod4-netfields.md 5 and 8). On an
//! `xproto` challenge it joins as a CoD4x 21 client instead: protocol 21,
//! the stats and the gamestate over the side channel ([`reliable::Link`]),
//! the 16-byte header (docs/research/cod4x-protocol.md 6 to 11, 18.3).
//! `examples/cod4_probe.rs` and the vcod client both join through it.

use super::clmsg::{self, ClOp, ClientHeader, CmdWire, UserCmd};
use super::demo::Archive;
use super::parse::{Client, Illegible, Op, Parsed};
use super::pure;
use super::reliable::{self, Link, Packet, StatsMsg};
use super::stats::{STAT_PACKETS, Stats};
use super::voice;
use super::wire::Dialect;
use anyhow::{Context, Result, bail};
use std::net::{SocketAddr, UdpSocket};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};
use vcod_common::net::cdkey::{self, CdKey, KeyGame};
use vcod_common::net::netchan::{Netchan, NetchanParams, scramble_client_ops};

/// Acknowledged reliable commands kept: `MAX_RELIABLE_COMMANDS`, the ring
/// the server keys a message off.
const KEPT_COMMANDS: usize = 128;

/// `cl_packetdup`'s default and range (`iw3mp.exe` 0x46dfa0..0x46e02c):
/// each packet carries the cmds of this many packets before it again.
pub const PACKET_DUP: u32 = 1;
const PACKET_DUP_MAX: u32 = 5;
/// `cl_maxpackets`' default and range (`iw3mp.exe` 0x46df21..0x46df3a):
/// off the LAN a packet goes at most every `1000 / cl_maxpackets` ms.
pub const MAX_PACKETS: i32 = 30;
const MAX_PACKETS_RANGE: (i32, i32) = (15, 100);
/// `PACKET_BACKUP` and `CMD_BACKUP`: the sent packets and the cmds kept.
const PACKET_BACKUP: usize = 32;
const CMD_BACKUP: usize = 128;

/// What `CL_WritePacket` (`iw3mp.exe` 0x463f50) reads its cmds from:
/// `cl.cmds`, `cl.cmdNumber` and each sent packet's `p_cmdNumber`
/// (cod4-netfields.md 14).
#[derive(Clone, Debug, Default)]
struct CmdRing {
    cmds: std::collections::VecDeque<UserCmd>,
    made: u32,
    sent_at: [u32; PACKET_BACKUP],
    /// Each row's `p_realtime`: when that sequence went out.
    sent_time: [Option<Instant>; PACKET_BACKUP],
}

impl CmdRing {
    fn push(&mut self, cmds: &[UserCmd]) {
        for c in cmds {
            if self.cmds.len() == CMD_BACKUP {
                self.cmds.pop_front();
            }
            self.cmds.push_back(*c);
            self.made = self.made.wrapping_add(1);
        }
    }

    /// The cmds of sequence `seq`: those made since sequence `seq - dup -
    /// 1` went out, the newest `MAX_PACKET_USERCMDS` of them.
    fn packet(&self, seq: u32, dup: u32) -> impl Iterator<Item = &UserCmd> {
        let old = self.sent_at[seq.wrapping_sub(dup).wrapping_sub(1) as usize % PACKET_BACKUP];
        let n = (self.made.wrapping_sub(old) as i32).clamp(0, clmsg::MAX_PACKET_USERCMDS as i32);
        let n = (n as usize).min(self.cmds.len());
        self.cmds.iter().skip(self.cmds.len() - n)
    }

    /// Sequence `seq` went out at `at`.
    fn sent(&mut self, seq: u32, at: Instant) {
        self.sent_at[seq as usize % PACKET_BACKUP] = self.made;
        self.sent_time[seq as usize % PACKET_BACKUP] = Some(at);
    }
}

/// What `CL_ReadyToSendPacket` (`iw3mp.exe` 0x463e60) reads, the times in
/// ms (`u64::MAX` for never) (cod4-netfields.md 14.1).
#[derive(Clone, Copy, Debug)]
struct SendClock {
    /// A gamestate is in and no `loadingnewmap` since: `CA_PRIMED` or
    /// `CA_ACTIVE`.
    in_world: bool,
    downloading: bool,
    /// Since any packet went (`clc.lastPacketSentTime`).
    since_sent: u64,
    /// Since the previous sequence went (its packet row's realtime).
    since_row: u64,
    /// The server's address passes `Sys_IsLANAddress`.
    lan: bool,
    /// `cl_maxpackets`, in its range.
    max_packets: i32,
}

impl SendClock {
    fn due(self) -> bool {
        if self.downloading && self.since_sent < 50 {
            return false;
        }
        if !self.in_world && !self.downloading && self.since_sent < 1000 {
            return false;
        }
        self.lan || self.since_row >= (1000 / self.max_packets) as u64
    }
}

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
    /// The stats under the `fs_game` the server's `connectResponse` names,
    /// which take `stats`' place when it gives any: 1.7 loads that
    /// `fs_game`'s stats before it sends them (0x46b93c).
    pub stats_for: Option<StatsFor>,
}

/// [`Join::stats_for`]: the profile's stats block under an `fs_game`.
pub type StatsFor = Box<dyn FnMut(&str) -> Option<Stats> + Send>;

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
            stats_for: None,
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
    /// Reliable client commands, index = sequence - 1 - `commands_base`:
    /// the server's acknowledged ones past the newest [`KEPT_COMMANDS`] are
    /// dropped, as retail's ring overwrites them.
    commands: Vec<Vec<u8>>,
    /// How many of the oldest commands were dropped.
    commands_base: usize,
    /// [`Self::recv`]'s datagram buffer, kept between calls.
    recv_buf: Vec<u8>,
    /// The newest the server executed (each server message's first long).
    commands_acked: i32,
    /// [`Self::command`] found the ring full: the connection is dropped.
    overflowed: bool,
    last_cmd: UserCmd,
    /// The cmds the next packets carry; a gamestate clears it, as
    /// `CL_ClearState` clears `cl` (0x473d1a).
    cmd_ring: CmdRing,
    /// `cl_packetdup`.
    packet_dup: u32,
    /// `cl_maxpackets`.
    max_packets: i32,
    /// The server's address passes `Sys_IsLANAddress`: every frame sends.
    lan: bool,
    /// `clc.lastPacketSentTime`; `None` where retail stores -9999.
    last_sent: Option<Instant>,
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
    /// CoD4x 21: the side channel, `None` on 1.7.
    side: Option<Link>,
    /// CoD4x 21: when the side channel's stats went, for
    /// [`GAMESTATE_WAIT`].
    side_stats: Option<Instant>,
    /// What the side channel's millisecond clock counts from.
    born: Instant,
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
            commands_base: 0,
            recv_buf: Vec::new(),
            commands_acked: 0,
            overflowed: false,
            last_cmd: UserCmd::default(),
            cmd_ring: CmdRing::default(),
            packet_dup: PACKET_DUP,
            max_packets: MAX_PACKETS,
            lan: vcod_common::net::lan::is_lan_cod2(server.ip()),
            last_sent: None,
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
            side: None,
            side_stats: None,
            born: now(),
        };
        c.note(JoinNote::Challenging);
        let key = cdkey::lookup(KeyGame::Cod4);
        let hash = key.as_ref().map(|k| k.hash(KeyGame::Cod4));
        let ask = format!("getchallenge 0 \"{}\"", hash.as_deref().unwrap_or(KEY_HASH));
        let chal = c.challenge_reply(ask.as_bytes(), key.as_ref())?;
        let dialect = match cod4x_proto(&chal) {
            None => Dialect::V17,
            Some(XPROTO) => Dialect::X21,
            Some(x) => return Err(Refused(cod4x_refusal(x)).into()),
        };
        c.challenge = String::from_utf8_lossy(&chal)
            .split_whitespace()
            .nth(1)
            .context("challenge")?
            .parse()?;
        let www = if opts.www { "\\cl_wwwDownload\\1" } else { "" };
        let vc = if opts.voice { "\\cl_voice\\1" } else { "" };
        let mut ui = format!(
            "{USERINFO}{www}{vc}\\name\\{}\\protocol\\{}\\challenge\\{}\\qport\\{}",
            opts.name,
            dialect.wire().transport.version,
            c.challenge,
            c.qport
        );
        if !opts.password.is_empty() {
            ui.push_str(&format!("\\password\\{}", opts.password));
        }
        c.userinfo = format!("{USERINFO}{www}{vc}\\name\\{}", opts.name);
        if !opts.password.is_empty() {
            c.userinfo
                .push_str(&format!("\\password\\{}", opts.password));
        }
        let reply = c
            .oob_reply(
                format!("connect \"{ui}\"").as_bytes(),
                b"connectResponse",
                10,
            )
            .map_err(cod4x_mismatch)?
            .context("no connectResponse")?;
        c.note(JoinNote::Connected);
        let fs_game = String::from_utf8_lossy(&reply[b"connectResponse".len()..]);
        if let Some(stats) = opts.stats_for.and_then(|mut f| f(fs_game.trim())) {
            c.stats = stats;
        }
        c.chan = Netchan::with_params(c.qport, c.challenge, NetchanParams::COD2);
        if dialect == Dialect::X21 {
            // The stats go when the side channel asks (`SV_RequestStats`).
            c.client = Client::with_dialect(dialect);
            c.side = Some(Link::new(c.qport));
            c.born = now();
            return Ok(c);
        }

        // Stats: the lowest index the server still wants, until its mask is 0.
        let mut want = (1u32 << STAT_PACKETS) - 1;
        let deadline = now() + Duration::from_secs(15);
        while want != 0 && now() < deadline {
            let idx = want.trailing_zeros() as usize;
            c.send_stat(idx)?;
            let stop = now() + Duration::from_millis(500);
            while let Some(r) = c.recv(stop)? {
                if let Some(rest) = r.get(4..).and_then(|b| b.strip_prefix(b"statResponse ")) {
                    let mask: u32 = String::from_utf8_lossy(rest).trim().parse()?;
                    want = mask & ((1u32 << STAT_PACKETS) - 1);
                    break;
                }
            }
        }
        if want != 0 {
            bail!("stats not accepted (mask {want})");
        }
        Ok(c)
    }

    pub fn server(&self) -> SocketAddr {
        self.server
    }

    /// Queues a reliable command; returns its sequence. Past
    /// [`KEPT_COMMANDS`] unacknowledged, `CL_AddReliableCommand` drops the
    /// connection with `EXE_ERR_CLIENT_CMD_OVERFLOW` (cod4-handshake.md 4):
    /// nothing queues and the next [`Self::send`] or [`Self::poll`] fails.
    pub fn command(&mut self, text: impl Into<Vec<u8>>) -> usize {
        if i64::from(self.reliable_sequence()) - i64::from(self.commands_acked)
            > KEPT_COMMANDS as i64
        {
            self.overflowed = true;
        } else {
            self.commands.push(text.into());
        }
        self.reliable_sequence() as usize
    }

    /// The reliable commands still held, oldest first: every one not yet
    /// acknowledged and the newest [`KEPT_COMMANDS`] that were. The first
    /// is sequence `reliable_sequence() - len + 1`.
    pub fn commands(&self) -> &[Vec<u8>] {
        &self.commands
    }

    /// `clc.reliableSequence`: the newest reliable command queued.
    pub fn reliable_sequence(&self) -> i32 {
        (self.commands_base + self.commands.len()) as i32
    }

    /// The text of reliable command `sequence`, empty when it was dropped
    /// or never queued.
    fn command_text(&self, sequence: i32) -> &[u8] {
        usize::try_from(sequence - 1)
            .ok()
            .and_then(|i| i.checked_sub(self.commands_base))
            .and_then(|i| self.commands.get(i))
            .map_or(&[], Vec::as_slice)
    }

    /// Drops the acknowledged commands older than the newest
    /// [`KEPT_COMMANDS`], in steps of that many so the drain is amortised.
    fn drop_acked_commands(&mut self) {
        let keep_from = usize::try_from(self.commands_acked)
            .unwrap_or(0)
            .saturating_sub(KEPT_COMMANDS);
        let n = keep_from.saturating_sub(self.commands_base);
        if n >= KEPT_COMMANDS {
            let n = n.min(self.commands.len());
            self.commands.drain(..n);
            self.commands_base += n;
        }
    }

    /// `clc.serverMessageSequence`: the newest server message read.
    pub fn message_sequence(&self) -> i32 {
        self.ack
    }

    fn check_overflow(&self) -> Result<()> {
        if self.overflowed {
            bail!("EXE_ERR_CLIENT_CMD_OVERFLOW");
        }
        Ok(())
    }

    /// The newest usercmd sent (its `server_time` the floor for the next).
    pub fn last_cmd(&self) -> &UserCmd {
        &self.last_cmd
    }

    /// `cl_packetdup` from now on, clamped to its range 0..=5.
    pub fn set_packet_dup(&mut self, n: i32) {
        self.packet_dup = n.clamp(0, PACKET_DUP_MAX as i32) as u32;
    }

    /// `cl_maxpackets` from now on, clamped to its range 15..=100.
    pub fn set_max_packets(&mut self, n: i32) {
        self.max_packets = n.clamp(MAX_PACKETS_RANGE.0, MAX_PACKETS_RANGE.1);
    }

    /// `CL_ReadyToSendPacket` (`iw3mp.exe` 0x463e60): whether a packet
    /// goes this frame. Before the gamestate or after `loadingnewmap`, one
    /// a second; while `downloading`, none within 50 ms of the last; then
    /// every frame to a LAN server, else `1000 / cl_maxpackets` ms after
    /// the previous sequence (cod4-netfields.md 14.1).
    pub fn ready_to_send(&self, downloading: bool) -> bool {
        let now = now();
        let since = |t: Option<Instant>| {
            t.map_or(u64::MAX, |t| {
                now.saturating_duration_since(t).as_millis() as u64
            })
        };
        let row = self.cmd_ring.sent_time[self.seq.wrapping_sub(1) as usize % PACKET_BACKUP];
        SendClock {
            in_world: self.client.gamestate.is_some() && self.loading.is_none(),
            downloading,
            since_sent: since(self.last_sent),
            since_row: since(row),
            lan: self.lan,
            max_packets: self.max_packets,
        }
        .due()
    }

    /// One move message: the unacked commands and the cmds made since the
    /// sequence `cl_packetdup` + 1 back went out, `cmds` the new ones (the
    /// newest 32 at most), the first on the long branch with every field,
    /// the rest the fewest bits off the one before (no move op without
    /// cmds). Before the gamestate it is the
    /// bare header the client sends while it waits. The move carries
    /// `origin`'s predicted origin and time and files it in
    /// `cl.clientArchive` (`CL_WritePacket`, `iw3mp.exe` 0x4641cd..0x46428c);
    /// without one it carries an origin far off at time 0, so the server
    /// always sends origin and velocity (cod4-netfields.md 13).
    pub fn send(&mut self, cmds: &[UserCmd], origin: Option<Archive>) -> Result<()> {
        self.check_overflow()?;
        let dialect = self.client.dialect;
        let mut head = ClientHeader {
            sequence: self.seq,
            qport: self.qport,
            message_ack: self.ack,
            config_ack: self.client.config_data_ack,
            ..ClientHeader::default()
        };
        let seq = self.seq;
        self.seq += 1;
        let mut pkt = Vec::new();
        // CoD4x's client sends its commands and `clc_EOF` before the
        // gamestate too, under serverId 0 and feed 0 (cod4x-protocol.md 6).
        let ids = match (&self.client.gamestate, dialect) {
            (Some(gs), _) => Some((
                gs.systeminfo("sv_serverid")
                    .context("sv_serverid")?
                    .parse::<i32>()?,
                gs.checksum_feed,
            )),
            (None, Dialect::X21) => Some((0, 0)),
            (None, Dialect::V17) => None,
        };
        match ids {
            None => head.write(dialect, &mut pkt),
            Some((server_id, feed)) => {
                let last = self.client.last_server_command().to_vec();
                let key = clmsg::usercmd_key(feed, self.ack, &last);
                // The ack is the server's word, any `i32`.
                let mut ops: Vec<ClOp> = (self.commands_acked.saturating_add(1)
                    ..=self.reliable_sequence())
                    .map(|sequence| ClOp::Command {
                        sequence,
                        text: self.command_text(sequence).to_vec(),
                    })
                    .collect();
                self.cmd_ring.push(cmds);
                if let Some(c) = cmds.last() {
                    self.last_cmd = *c;
                }
                let mut wired: Vec<(UserCmd, CmdWire)> = Vec::new();
                for c in self.cmd_ring.packet(seq, self.packet_dup) {
                    let wire = match wired.last() {
                        None => CmdWire::full(&UserCmd::default(), c),
                        Some((prev, _)) => CmdWire::natural(prev, c),
                    };
                    wired.push((*c, wire));
                }
                if !wired.is_empty() && self.client.gamestate.is_some() {
                    let (at, time) = origin.map_or(([1.0e6; 3], 0), |a| (a.origin, a.server_time));
                    ops.push(ClOp::Move {
                        delta: !self.client.snapshots.is_empty() && !self.no_delta,
                        cmds: wired,
                        origin: at.map(f32::to_bits),
                        origin_time: time,
                    });
                    if let Some(a) = origin {
                        self.client.archive.add(a);
                    }
                }
                ops.push(ClOp::Eof);
                let mut comp =
                    clmsg::compress_ops(&self.client.huff, &ops, key, &UserCmd::default());
                head.server_id = server_id;
                head.reliable_ack = self.client.server_command_sequence;
                scramble_client_ops(head.seed(self.challenge), &last, &mut comp);
                head.write(dialect, &mut pkt);
                pkt.extend_from_slice(&comp);
            }
        }
        let at = now();
        self.cmd_ring.sent(seq, at);
        self.last_sent = Some(at);
        self.send_raw(&pkt)
    }

    /// `disconnect` as a reliable command, sent in three moves as
    /// `CL_Disconnect` sends it (`iw3mp.exe` 0x469776..0x46978d); the
    /// server frees the slot at once. A CoD4x server takes it before the
    /// gamestate too (cod4x-protocol.md 19.1).
    pub fn disconnect(&mut self) -> Result<()> {
        if self.client.gamestate.is_none() && self.client.dialect == Dialect::V17 {
            return Ok(());
        }
        self.command("disconnect");
        for _ in 0..3 {
            self.send(&[], None)?;
        }
        Ok(())
    }

    /// The server messages read until `until` (at least everything already
    /// queued). A gamestate queues the `cp` ahead of any move, as the client
    /// sends it once the level is loaded, before its first usercmd.
    pub fn poll(&mut self, until: Instant) -> Result<Vec<Received>> {
        self.poll_at_most(until, vcod_common::net::MAX_PACKETS_PER_PUMP)
    }

    /// [`Conn::poll`] reading at most `max` datagrams, so a flood cannot
    /// hold the frame.
    fn poll_at_most(&mut self, until: Instant, max: usize) -> Result<Vec<Received>> {
        self.check_overflow()?;
        let mut out = Vec::new();
        let mut read = 0;
        while read < max
            && let Some(d) = self.recv(until)?
        {
            read += 1;
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
            if self.side.is_some() && Packet::is_side(&d) {
                out.extend(self.side_packet(&d)?);
                continue;
            }
            let Some((s, mut msg)) = self.chan.frame(&d)? else {
                continue;
            };
            if msg.len() < 4 {
                continue;
            }
            let rack = i32::from_le_bytes(msg[..4].try_into().unwrap());
            let kept = (self.ack, self.commands_acked);
            self.ack = s as i32;
            // `CL_PacketEvent` drops a message acknowledging less than the
            // ring and takes every command as acknowledged (`iw3mp.exe`
            // 0x46c502..0x46c50e, cod4-handshake.md 10).
            if rack < self.reliable_sequence() - KEPT_COMMANDS as i32 {
                self.commands_acked = self.reliable_sequence();
                self.drop_acked_commands();
                continue;
            }
            let key = self.command_text(rack).to_vec();
            self.chan.decode(s, &key, &mut msg);
            self.commands_acked = self.commands_acked.max(rack);
            // `CL_PacketEvent` warns `ignoring illegible message` and goes
            // on (`iw3mp.exe` 0x46c53c..0x46c548, cod4-handshake.md 4); one
            // it marked overflowed puts both acknowledges back (0x46c550,
            // 0x46c556), so the next move acknowledges the message before
            // it and resends the commands it acknowledged.
            let parsed = match self.client.parse(s, &msg[4..]) {
                Ok(p) => p,
                Err(e) => {
                    if e.downcast_ref::<Illegible>().is_some() {
                        (self.ack, self.commands_acked) = kept;
                    }
                    log::warn!("cod4: ignoring illegible message {s}: {e:#}");
                    continue;
                }
            };
            self.drop_acked_commands();
            self.apply_configstrings(&parsed);
            if parsed.ops.iter().any(|o| matches!(o, Op::Gamestate)) {
                self.cmd_ring = CmdRing::default();
                self.answer_gamestate()?;
            }
            out.push(Received {
                sequence: s,
                parsed,
                msg,
            });
        }
        self.side_frame()?;
        if self.client.gamestate.is_none()
            && self
                .side_stats
                .is_some_and(|t| now().saturating_duration_since(t) > GAMESTATE_WAIT)
        {
            return Err(self.refuse(NO_GAMESTATE));
        }
        Ok(out)
    }

    /// Ends a CoD4x join before its gamestate: `disconnect` goes, so the
    /// server frees the slot, and the error is `why`.
    fn refuse(&mut self, why: &str) -> anyhow::Error {
        if let Err(e) = self.disconnect() {
            log::warn!("cod4x: disconnect: {e:#}");
        }
        Refused(why.into()).into()
    }

    /// One side channel packet: its acknowledge, and the messages it
    /// completed answered. The stats request gets the profile's block in
    /// mode 2 (cod4x-protocol.md 7.1); the gamestate goes into the client
    /// and comes back as a [`Received`] holding [`Op::Gamestate`] alone,
    /// with no message for a demo. The authentication library's message
    /// ahead of the stats request ends the join with [`AUTH_REFUSAL`]
    /// (cod4x-protocol.md 19.2). Other messages (downloads) are logged and
    /// left.
    fn side_packet(&mut self, d: &[u8]) -> Result<Option<Received>> {
        let p = match Packet::read(d) {
            Ok(p) => p,
            Err(e) => {
                log::warn!("cod4x: ignoring side channel packet: {e:#}");
                return Ok(None);
            }
        };
        let side = self.side.as_mut().context("side channel")?;
        let mut got = None;
        for body in side.receive(&p)? {
            let cmd = body
                .get(..4)
                .map_or(0, |w| i32::from_le_bytes(w.try_into().unwrap()));
            match cmd {
                reliable::CMD_AUTH
                    if self.client.gamestate.is_none() && self.side_stats.is_none() =>
                {
                    log::info!("cod4x: the server authenticates clients");
                    return Err(self.refuse(AUTH_REFUSAL));
                }
                reliable::CMD_STATS => {
                    self.side_stats.get_or_insert_with(now);
                    let m = StatsMsg::mode2(&self.stats.0, self.challenge);
                    self.side.as_mut().context("side channel")?.send(&m.write());
                    log::info!("cod4x: stats requested, sent in mode 2");
                }
                reliable::CMD_GAMESTATE => {
                    self.client.take_side_gamestate(&body)?;
                    self.side_gamestate();
                    self.cmd_ring = CmdRing::default();
                    self.answer_gamestate()?;
                    got = Some(Received {
                        sequence: self.ack as u32,
                        parsed: Parsed {
                            ops: vec![Op::Gamestate],
                            eof_at: 0,
                            exact: true,
                        },
                        msg: Vec::new(),
                    });
                }
                cmd => log::info!(
                    "cod4x: side channel message cmd {cmd}, {} bytes, left",
                    body.len()
                ),
            }
        }
        Ok(got)
    }

    /// A side channel gamestate's table takes the server commands from its
    /// sequence on: the netchan may have brought newer ones while the
    /// fragments came, and the reset of [`apply_configstrings`] on a
    /// netchan gamestate is this one's.
    fn side_gamestate(&mut self) {
        let Some(from) = self
            .client
            .gamestate
            .as_ref()
            .map(|g| g.server_command_sequence)
        else {
            return;
        };
        self.applied = from;
        let newer = Parsed {
            ops: self
                .client
                .server_commands
                .range(from.saturating_add(1)..)
                .map(|(s, t)| Op::ServerCommand {
                    sequence: *s,
                    text: t.clone(),
                })
                .collect(),
            eof_at: 0,
            exact: true,
        };
        self.apply_configstrings(&newer);
    }

    /// The side channel packets due now ([`Link::frame`]).
    fn side_frame(&mut self) -> Result<()> {
        let ms = now().saturating_duration_since(self.born).as_millis() as i32;
        let due = match self.side.as_mut() {
            Some(side) => side.frame(ms),
            None => return Ok(()),
        };
        for p in due {
            self.send_raw(&p.write())?;
        }
        Ok(())
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
    /// 0x46af95; cod4-netfields.md 10.2).
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
                // `CA_SENDINGSTATS` and -9999 (0x46bfcd, 0x46c007).
                self.last_sent = None;
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
                // Any `statResponse` stores -9999 (0x46ba43).
                self.last_sent = None;
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
        let mut buf = std::mem::take(&mut self.recv_buf);
        buf.resize(65536, 0);
        let got = self.recv_into(until, &mut buf);
        self.recv_buf = buf;
        got
    }

    /// [`Self::recv`] into the connection's reused buffer.
    fn recv_into(&mut self, until: Instant, buf: &mut [u8]) -> Result<Option<Vec<u8>>> {
        loop {
            if self
                .cancel
                .as_ref()
                .is_some_and(|c| c.load(Ordering::Relaxed))
            {
                return Err(Cancelled.into());
            }
            for i in 0..self.auth.len() {
                if let Ok((n, _)) = self.auth[i].recv_from(buf) {
                    let local = self.auth[i].local_addr()?;
                    let d = buf[..n].to_vec();
                    self.tap("auth", local, &d)?;
                }
            }
            match self.sock.recv_from(buf) {
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
                // Another sender's datagram, which must not hold the loop
                // past `until`.
                Ok(_) if now() < until => continue,
                Ok(_) => return Ok(None),
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

/// The `xproto` of the CoD4x server vcod joins as a protocol 21 client
/// (`SV_GetChallenge`'s format at 0x10e700, cod4x-protocol.md 2).
pub const XPROTO: &str = "18";

/// Why a CoD4x join ends at the authentication library's first message
/// (cod4x-protocol.md 19.2).
pub const AUTH_REFUSAL: &str =
    "This CoD4x server requires CoD4x client authentication, which vcod does not support.";

/// How long a CoD4x join waits for the gamestate once its stats went: the
/// largest (0x20000 bytes, 110 fragments) takes 5.5 s at 20 fragments a
/// second, the release's 44 came 2.5 s after the stats (cod4x-protocol.md
/// 7.4, 18.4).
pub const GAMESTATE_WAIT: Duration = Duration::from_secs(15);

/// Why a CoD4x join ends when no gamestate came within [`GAMESTATE_WAIT`].
pub const NO_GAMESTATE: &str = "This CoD4x server sent no gamestate within 15 s of the stats; \
     it may require CoD4x client authentication, which vcod does not support.";

/// Why vcod does not go on into a CoD4x server whose `xproto` is not
/// [`XPROTO`]: its side channel or protocol may not be the one vcod
/// speaks.
pub fn cod4x_refusal(xproto: &str) -> String {
    format!(
        "This server runs a CoD4x vcod does not know (xproto {xproto}); vcod joins CoD4x \
         servers of xproto {XPROTO} (protocol 21) and stock 1.7 servers."
    )
}

/// A CoD4x server's refusal of the protocol vcod sent, which ends in
/// `{OOBErrorParser protocolmismatch CoD4X- <n>}` (0x2c1538,
/// cod4x-protocol.md 3), in plain words; any other error as it came.
fn cod4x_mismatch(e: anyhow::Error) -> anyhow::Error {
    let Some(Refused(why)) = e.downcast_ref() else {
        return e;
    };
    let Some(n) = why
        .split("protocolmismatch CoD4X-")
        .nth(1)
        .and_then(|t| t.split('}').next())
        .map(str::trim)
    else {
        return e;
    };
    Refused(format!(
        "This server needs CoD4x protocol {n}; vcod speaks CoD4x protocol 21."
    ))
    .into()
}

/// `ANGLE2SHORT`.
pub fn angle2short(deg: f32) -> u16 {
    ((deg * 65536.0 / 360.0) as i32 & 0xffff) as u16
}

/// A cmd's angles as the 1.7 client makes them off a view net of the
/// playerstate's float `delta_angles`: `(int)` of the x87 double `deg *
/// 182.04445f + 0.5`, masked (`iw3mp.exe` 0x463aa9..0x463acf,
/// cod4-client-view.md 14.1). The difference wraps into 0..360 first, as
/// the truncation would carry a negative one a short toward 0 each time a
/// caller feeds the server's view back in.
pub fn cmd_angles(view: [f32; 3], delta: [f32; 3]) -> [u16; 3] {
    let k = f64::from(vcod_common::x87::SHORT_PER_DEG);
    std::array::from_fn(|i| {
        let deg = (f64::from(view[i]) - f64::from(delta[i])).rem_euclid(360.0);
        ((deg * k + 0.5) as i32 & 0xffff) as u16
    })
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
            .filter(|&i| u32::from(i) < gs.dialect.max_configstrings())
        {
            gs.configstrings
                .insert(i, rest.get(at + 1..).unwrap_or(&[]).to_vec());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The half rounds the float difference up, where two truncated shorts
    /// lose it, and a negative difference rounds as its wrap does.
    #[test]
    fn cmd_angles_round_the_float_difference() {
        let c = cmd_angles([0.003, -0.004, 0.0], [-0.003, 0.0, 0.0]);
        assert_eq!(c, [1, 65535, 0]);
        assert_eq!(angle2short(0.003).wrapping_sub(angle2short(-0.003)), 0);
        assert_eq!(cmd_angles([90.0, -90.0, 0.0], [0.0; 3]), [16384, 49152, 0]);
    }

    /// The server's view fed back in (`PM_UpdateViewAngles`' wrap to
    /// -180..180) gives the cmd that made it, either sign.
    #[test]
    fn cmd_angles_hold_the_server_view() {
        let delta = [3.7f32, -151.3, 0.0];
        for cmd in [0u16, 1, 5000, 32767, 32768, 60000, 65535] {
            let view = delta.map(|d| {
                let a = f64::from(d) + f64::from(cmd) * 360.0 / 65536.0;
                ((a + 180.0).rem_euclid(360.0) - 180.0) as f32
            });
            assert_eq!(cmd_angles(view, delta)[..2], [cmd, cmd], "cmd {cmd}");
        }
    }

    /// A joined connection to `server`, as [`Conn::join`] leaves one.
    fn joined(server: SocketAddr) -> Conn {
        let sock = UdpSocket::bind("127.0.0.1:0").unwrap();
        sock.set_nonblocking(true).unwrap();
        Conn {
            stats: Stats::fresh(),
            sock,
            server,
            auth: Vec::new(),
            tap: None,
            qport: 1,
            challenge: 0,
            cp: Cp::Bare,
            chan: Netchan::with_params(1, 0, NetchanParams::COD2),
            client: Client::new(),
            seq: 1,
            ack: 0,
            commands: Vec::new(),
            commands_base: 0,
            recv_buf: Vec::new(),
            commands_acked: 0,
            overflowed: false,
            last_cmd: UserCmd::default(),
            cmd_ring: CmdRing::default(),
            packet_dup: PACKET_DUP,
            max_packets: MAX_PACKETS,
            lan: true,
            last_sent: None,
            cp_feed: None,
            applied: 0,
            loading: None,
            stats_sent: None,
            disconnect: None,
            cancel: None,
            notes: None,
            no_delta: false,
            voice_in: Vec::new(),
            prints: Vec::new(),
            userinfo: String::new(),
            side: None,
            side_stats: None,
            born: Instant::now(),
        }
    }

    fn waiting(c: &Conn) -> usize {
        let mut buf = [0u8; 16];
        std::iter::from_fn(|| c.sock.recv_from(&mut buf).ok()).count()
    }

    /// A flood from the server is read `max` datagrams a poll; one from
    /// another sender ends a wait at its deadline.
    #[test]
    fn a_flood_cannot_hold_the_receive_loop() {
        let server = UdpSocket::bind("127.0.0.1:0").unwrap();
        let mut c = joined(server.local_addr().unwrap());
        let me = c.sock.local_addr().unwrap();
        for _ in 0..50 {
            server.send_to(b"x", me).unwrap();
        }
        assert!(c.poll_at_most(Instant::now(), 8).unwrap().is_empty());
        assert_eq!(waiting(&c), 42);

        let other = UdpSocket::bind("127.0.0.1:0").unwrap();
        for _ in 0..50 {
            other.send_to(b"x", me).unwrap();
        }
        assert!(c.recv(Instant::now()).unwrap().is_none());
        assert_eq!(waiting(&c), 49);
    }

    /// A message that does not parse is skipped and acknowledged, as
    /// retail's `ignoring illegible message`; the ones around it come
    /// through in the same poll.
    #[test]
    fn an_illegible_message_is_skipped_not_fatal() {
        use super::super::huffman::Huffman;
        use super::super::parse::{SVC_EOF, SVC_NOP};
        use vcod_common::net::netchan::ServerNetchan;
        let server = UdpSocket::bind("127.0.0.1:0").unwrap();
        let mut c = joined(server.local_addr().unwrap());
        let me = c.sock.local_addr().unwrap();
        let mut chan = ServerNetchan::with_params(1, 0, NetchanParams::COD2);
        let huff = Huffman::new();
        for ops in [&[SVC_NOP, SVC_EOF][..], &[0xc8], &[SVC_EOF]] {
            for p in chan.transmit_block(0, &huff.compress(ops)) {
                server.send_to(&p, me).unwrap();
            }
        }
        let got = c.poll(Instant::now() + Duration::from_millis(200)).unwrap();
        assert_eq!(got.iter().map(|r| r.sequence).collect::<Vec<_>>(), [1, 3]);
        assert!(matches!(got[0].parsed.ops[..], [Op::Nop]));
        assert_eq!(c.ack, 3);
    }

    /// A message the parse marks overflowed (an unknown op, a read past
    /// the end) puts `serverMessageSequence` and `reliableAcknowledge`
    /// back (`iw3mp.exe` 0x46c550, 0x46c556): the next move acknowledges
    /// the message before it and resends the command it acknowledged.
    #[test]
    fn an_illegible_message_rolls_its_acknowledges_back() {
        use super::super::huffman::Huffman;
        use super::super::parse::{SVC_EOF, SVC_SERVERCOMMAND};
        use vcod_common::net::netchan::ServerNetchan;
        let server = UdpSocket::bind("127.0.0.1:0").unwrap();
        let mut c = joined(server.local_addr().unwrap());
        c.command("say hi");
        let me = c.sock.local_addr().unwrap();
        let mut chan = ServerNetchan::with_params(1, 0, NetchanParams::COD2);
        let huff = Huffman::new();
        let mut send = |rack: i32, ops: &[u8]| {
            for p in chan.transmit_block(rack, &huff.compress(ops)) {
                server.send_to(&p, me).unwrap();
            }
        };
        let until = || Instant::now() + Duration::from_millis(200);
        send(0, &[SVC_EOF]);
        assert_eq!(c.poll(until()).unwrap().len(), 1);
        assert_eq!((c.ack, c.commands_acked), (1, 0));
        // The server keys its messages on the command it acknowledges.
        chan.last_client_command_string = "say hi".into();
        let mut send = |rack: i32, ops: &[u8]| {
            for p in chan.transmit_block(rack, &huff.compress(ops)) {
                server.send_to(&p, me).unwrap();
            }
        };
        for ops in [&[0xc8][..], &[SVC_SERVERCOMMAND, 1]] {
            send(1, ops);
            assert!(c.poll(until()).unwrap().is_empty());
            assert_eq!((c.ack, c.commands_acked), (1, 0), "{ops:?}");
        }
        send(1, &[SVC_EOF]);
        assert_eq!(c.poll(until()).unwrap().len(), 1);
        assert_eq!((c.ack, c.commands_acked), (4, 1));
    }

    /// A `reliableAcknowledge` at either end of `int`: `i32::MIN` is below
    /// the ring and its message dropped, as `CL_PacketEvent` drops it
    /// (`iw3mp.exe` 0x46c502); `i32::MAX` acks every command, and the next
    /// move resends none.
    #[test]
    fn an_acknowledge_at_the_ends_of_int_is_survived() {
        use super::super::huffman::Huffman;
        use super::super::parse::{Gamestate, MAX_CONFIGSTRINGS, SVC_EOF};
        use std::collections::BTreeMap;
        use vcod_common::net::netchan::ServerNetchan;
        let server = UdpSocket::bind("127.0.0.1:0").unwrap();
        let mut c = joined(server.local_addr().unwrap());
        let mut table = vec![String::new(); MAX_CONFIGSTRINGS as usize];
        table[1] = "\\sv_serverid\\16".into();
        c.client.gamestate = Some(Gamestate::from_table(0, &table, &BTreeMap::new(), 0, 0));
        c.command("say hi");
        let me = c.sock.local_addr().unwrap();
        let mut chan = ServerNetchan::with_params(1, 0, NetchanParams::COD2);
        let huff = Huffman::new();
        for ack in [i32::MIN, i32::MAX] {
            for p in chan.transmit_block(ack, &huff.compress(&[SVC_EOF])) {
                server.send_to(&p, me).unwrap();
            }
        }
        let got = c.poll(Instant::now() + Duration::from_millis(200)).unwrap();
        assert_eq!(got.iter().map(|r| r.sequence).collect::<Vec<_>>(), [2]);
        assert_eq!(c.commands_acked, i32::MAX);
        c.send(&[], None).unwrap();
    }

    /// A `reliableAcknowledge` more than the ring behind drops its message
    /// and acknowledges every command, as `CL_PacketEvent` stores
    /// `reliableSequence` over it (`iw3mp.exe` 0x46c50e): the next move
    /// resends none.
    #[test]
    fn an_acknowledge_behind_the_ring_acknowledges_every_command() {
        use super::super::huffman::Huffman;
        use super::super::parse::{Gamestate, MAX_CONFIGSTRINGS, SVC_EOF};
        use std::collections::BTreeMap;
        use vcod_common::net::netchan::ServerNetchan;
        let server = UdpSocket::bind("127.0.0.1:0").unwrap();
        let mut c = joined(server.local_addr().unwrap());
        let mut table = vec![String::new(); MAX_CONFIGSTRINGS as usize];
        table[1] = "\\sv_serverid\\16".into();
        c.client.gamestate = Some(Gamestate::from_table(0, &table, &BTreeMap::new(), 0, 0));
        c.command("say hi");
        c.command("say there");
        let me = c.sock.local_addr().unwrap();
        let mut chan = ServerNetchan::with_params(1, 0, NetchanParams::COD2);
        let huff = Huffman::new();
        for p in chan.transmit_block(-1000, &huff.compress(&[SVC_EOF])) {
            server.send_to(&p, me).unwrap();
        }
        let got = c.poll(Instant::now() + Duration::from_millis(200)).unwrap();
        assert!(got.is_empty(), "the message is dropped");
        assert_eq!(c.ack, 1, "its sequence is read");
        assert_eq!(c.commands_acked, c.reliable_sequence());
        assert_eq!(c.commands_acked, 2);
    }

    /// Acknowledged commands past the reliable ring are dropped; the
    /// sequence numbering and the newest ring's texts hold.
    #[test]
    fn acknowledged_commands_are_dropped_past_the_ring() {
        let server = UdpSocket::bind("127.0.0.1:0").unwrap();
        let mut c = joined(server.local_addr().unwrap());
        for i in 1..=100_000 {
            assert_eq!(c.command(format!("c{i}")), i);
            c.commands_acked = i as i32 - 3;
            c.drop_acked_commands();
        }
        assert_eq!(c.reliable_sequence(), 100_000);
        assert!(c.commands().len() <= 3 * KEPT_COMMANDS);
        assert_eq!(c.command_text(100_000), b"c100000");
        assert_eq!(c.command_text(99_997 - KEPT_COMMANDS as i32 + 1), b"c99870");
        assert_eq!(c.command_text(1), b"");
    }

    /// More than the ring unacknowledged refuses the next command and
    /// drops the connection, as `CL_AddReliableCommand` does.
    #[test]
    fn unacknowledged_commands_past_the_ring_overflow() {
        let server = UdpSocket::bind("127.0.0.1:0").unwrap();
        let mut c = joined(server.local_addr().unwrap());
        for i in 1..=KEPT_COMMANDS + 1 {
            assert_eq!(c.command("x"), i);
        }
        c.send(&[], None).unwrap();
        assert_eq!(c.command("x"), KEPT_COMMANDS + 1);
        let e = c.send(&[], None).unwrap_err();
        assert_eq!(e.to_string(), "EXE_ERR_CLIENT_CMD_OVERFLOW");
        assert!(c.poll(Instant::now()).is_err());
    }

    /// `c` with a gamestate in `dialect`, and the move ops of each packet
    /// it sends to `server` decoded as the server reads them.
    fn moving(server: &UdpSocket, dialect: Dialect) -> Conn {
        use super::super::parse::{Gamestate, MAX_CONFIGSTRINGS};
        use std::collections::BTreeMap;
        let mut c = joined(server.local_addr().unwrap());
        c.client = Client::with_dialect(dialect);
        let mut table = vec![String::new(); MAX_CONFIGSTRINGS as usize];
        table[1] = "\\sv_serverid\\16".into();
        let mut gs = Gamestate::from_table(0, &table, &BTreeMap::new(), 0, 0x1234);
        gs.dialect = dialect;
        c.client.gamestate = Some(gs);
        c
    }

    /// The ops of the packet `c` sent last, read off `server`.
    fn sent_ops(server: &UdpSocket, c: &Conn) -> Vec<ClOp> {
        let mut buf = [0u8; 2048];
        let n = server.recv(&mut buf).unwrap();
        let (h, block) = ClientHeader::read(&buf[..n], c.client.dialect).unwrap();
        let last = c.client.server_command(h.reliable_ack).to_vec();
        let mut block = block.to_vec();
        scramble_client_ops(h.seed(c.challenge), &last, &mut block);
        let key = clmsg::usercmd_key(0x1234, h.message_ack, &last);
        let plain = c.client.huff.decompress(&block);
        clmsg::read_ops(plain, key, &UserCmd::default()).unwrap()
    }

    fn move_times(ops: &[ClOp]) -> Vec<i32> {
        match ops {
            [ClOp::Move { cmds, .. }, ClOp::Eof] => {
                cmds.iter().map(|(c, _)| c.server_time).collect()
            }
            [ClOp::Eof] => Vec::new(),
            o => panic!("{o:?}"),
        }
    }

    /// `cl_packetdup` N: a packet carries the cmds made since the packet
    /// N + 1 sequences back went out, the newest 32 at most, the first on
    /// the long branch (`CL_WritePacket`, cod4-netfields.md 14), on 1.7's
    /// header and CoD4x 21's.
    #[test]
    fn each_packet_resends_the_cmds_of_the_last_packet_dup_packets() {
        let cmd = |t: i32| UserCmd {
            server_time: t,
            buttons: if t == 200 { 0x1_0000 } else { 0 },
            ..UserCmd::default()
        };
        for dialect in [Dialect::V17, Dialect::X21] {
            let server = UdpSocket::bind("127.0.0.1:0").unwrap();
            server
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let mut c = moving(&server, dialect);
            assert_eq!(c.packet_dup, PACKET_DUP);
            let send = |c: &mut Conn, times: &[i32], dup: i32| {
                c.set_packet_dup(dup);
                let cmds: Vec<UserCmd> = times.iter().map(|&t| cmd(t)).collect();
                c.send(&cmds, None).unwrap();
                sent_ops(&server, c)
            };
            assert_eq!(move_times(&send(&mut c, &[100], 1)), [100]);
            assert_eq!(move_times(&send(&mut c, &[133, 150], 1)), [100, 133, 150]);
            let ops = send(&mut c, &[200], 1);
            assert_eq!(move_times(&ops), [133, 150, 200]);
            // The wire is the writer's over those three cmds.
            let (b, d, e) = (cmd(133), cmd(150), cmd(200));
            let want = vec![
                ClOp::Move {
                    delta: false,
                    cmds: vec![
                        (b, CmdWire::full(&UserCmd::default(), &b)),
                        (d, CmdWire::natural(&b, &d)),
                        (e, CmdWire::natural(&d, &e)),
                    ],
                    origin: [1.0e6f32.to_bits(); 3],
                    origin_time: 0,
                },
                ClOp::Eof,
            ];
            assert_eq!(ops, want, "{dialect:?}");
            // Nothing new: the last packet's cmds again, then none.
            assert_eq!(move_times(&send(&mut c, &[], 1)), [200]);
            assert_eq!(move_times(&send(&mut c, &[], 1)), Vec::<i32>::new());
            assert_eq!(move_times(&send(&mut c, &[250], 0)), [250]);
            // Six packets back: the second's on.
            assert_eq!(
                move_times(&send(&mut c, &[300], 5)),
                [133, 150, 200, 250, 300]
            );
            // Past the range: 5.
            assert_eq!(move_times(&send(&mut c, &[350], 9)), [200, 250, 300, 350]);
            let many: Vec<i32> = (0..40).map(|i| 400 + i).collect();
            assert_eq!(move_times(&send(&mut c, &many, 1)), many[8..]);
            assert_eq!(c.last_cmd().server_time, 439);
        }
    }

    /// `CL_ReadyToSendPacket` (cod4-netfields.md 14.1): in the world a LAN
    /// server takes every frame's packet and one off it a packet
    /// `1000 / cl_maxpackets` ms after the previous sequence; out of it one
    /// a second, LAN or not; downloading, 50 ms and then the same test.
    #[test]
    fn a_lan_server_gets_every_frame_and_one_off_it_cl_maxpackets() {
        let clock = |in_world, downloading, since_sent, since_row, lan, max_packets| {
            SendClock {
                in_world,
                downloading,
                since_sent,
                since_row,
                lan,
                max_packets,
            }
            .due()
        };
        let world = |since, lan, max| clock(true, false, since, since, lan, max);
        assert!(world(0, true, 30));
        assert!(!world(32, false, 30));
        assert!(world(33, false, 30));
        assert!(!world(65, false, 15));
        assert!(world(66, false, 15));
        assert!(world(10, false, 100));
        // A cleared row (a gamestate) is due at once.
        assert!(clock(true, false, 5, u64::MAX, false, 30));
        let out = |since, lan| clock(false, false, since, since, lan, 30);
        assert!(!out(999, true));
        assert!(out(1000, true));
        assert!(!out(999, false));
        assert!(out(u64::MAX, false));
        let dl = |since_sent, since_row, lan| clock(false, true, since_sent, since_row, lan, 30);
        assert!(!dl(49, 49, true));
        assert!(dl(50, 50, true));
        assert!(!dl(50, 20, false));
        assert!(dl(50, 50, false));
    }

    /// The gate on a connection: the row a send leaves holds the next
    /// packet back off the LAN; `loadingnewmap` puts it on the second's
    /// wait, a `statResponse` clears that wait; `cl_maxpackets` clamps to
    /// 15..=100.
    #[test]
    fn a_conn_paces_its_packets_by_the_server_and_its_state() {
        let server = UdpSocket::bind("127.0.0.1:0").unwrap();
        let mut c = moving(&server, Dialect::V17);
        assert_eq!(c.max_packets, MAX_PACKETS);
        c.set_max_packets(1);
        assert_eq!(c.max_packets, 15);
        c.set_max_packets(1000);
        assert_eq!(c.max_packets, 100);
        c.set_max_packets(30);
        c.lan = false;
        assert!(c.ready_to_send(false), "no packet yet");
        c.send(&[], None).unwrap();
        assert!(!c.ready_to_send(false), "33 ms off the LAN");
        c.lan = true;
        assert!(c.ready_to_send(false), "every frame on it");
        c.loading = Some("mp_crash".into());
        assert!(!c.ready_to_send(false), "a second while loading");
        c.connectionless(b"statResponse 0").unwrap();
        assert!(c.ready_to_send(false));
        c.loading = None;
        // A gamestate clears the rows.
        c.lan = false;
        c.send(&[], None).unwrap();
        c.cmd_ring = CmdRing::default();
        assert!(c.ready_to_send(false));
    }

    /// A side channel gamestate's table takes the `d` commands the
    /// netchan brought past its sequence while its fragments came, and
    /// none from before it.
    #[test]
    fn a_side_gamestate_takes_the_newer_configstring_commands() {
        use super::super::parse::{Gamestate, MAX_CONFIGSTRINGS};
        use std::collections::BTreeMap;
        let server = UdpSocket::bind("127.0.0.1:0").unwrap();
        let mut c = joined(server.local_addr().unwrap());
        c.client = Client::with_dialect(Dialect::X21);
        let table = vec![String::new(); MAX_CONFIGSTRINGS as usize];
        let mut gs = Gamestate::from_table(5, &table, &BTreeMap::new(), 0, 0);
        gs.dialect = Dialect::X21;
        c.client.gamestate = Some(gs);
        for (seq, text) in [(4, "d 9 old"), (6, "d 9 new"), (7, "d 4000 high")] {
            c.client
                .server_commands
                .insert(seq, text.as_bytes().to_vec());
        }
        c.side_gamestate();
        let gs = c.client.gamestate.as_ref().unwrap();
        assert_eq!(
            (gs.configstring(9), gs.configstring(4000)),
            (&b"new"[..], &b"high"[..])
        );
        assert_eq!(c.applied, 7);
    }

    /// A CoD4x server's protocol refusal reads plainly; another error
    /// stays as it came (cod4x-protocol.md 3).
    #[test]
    fn a_cod4x_protocol_mismatch_reads_plainly() {
        let why = "This server requires protocol version: 22\nPlease restart CoD4 \
                   {OOBErrorParser protocolmismatch CoD4X- 22}";
        let e = cod4x_mismatch(Refused(why.into()).into());
        assert_eq!(
            e.to_string(),
            "This server needs CoD4x protocol 22; vcod speaks CoD4x protocol 21."
        );
        let e = cod4x_mismatch(Refused("GAME_INVALIDPASSWORD".into()).into());
        assert_eq!(e.to_string(), "GAME_INVALIDPASSWORD");
    }

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
