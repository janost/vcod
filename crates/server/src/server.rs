//! Contains routines ported from the RTCW-MP GPL source, Copyright (C) 1999-2010 id Software LLC, a ZeniMax Media company.
//! See NOTICE.
//!
//! The server state machine, transport-free. `main.rs` owns the socket, the
//! tests own a queue. Ported from RTCW-MP sv_main.c / sv_client.c; reply
//! strings come from docs/research/cod11-server-handshake.md.

use crate::client::{sanitize_name, Client, ClientState};
use crate::configstrings;
use crate::spectate::SpectatorSim;
use crate::world::{TestEntities, World};
use std::collections::{BTreeMap, HashMap};
use std::net::{IpAddr, SocketAddr};
use std::time::{Duration, Instant};
use vcod_common::net::connectionless::{build_oob, parse_connect, parse_oob, Info};
use vcod_common::net::gamestate::{self, Gamestate};
use vcod_common::net::huffman::Huffman;
use vcod_common::net::msg::{
    self, read_delta_usercmd, MsgReader, MsgWriter, UserCmd, NULL_USERCMD,
};
use vcod_common::net::netchan::{ClientMessage, ServerNetchan, MAX_RELIABLE_COMMANDS};
use vcod_common::net::protocol::{Protocol, PROTOCOL_V1};
use vcod_common::net::{com_hash_key, info_value_for_key, snapshot};
use vcod_common::pmove::MAX_FRAME_MS;

/// `MAX_CHALLENGES`, server.h:198.
const MAX_CHALLENGES: usize = 1024;
/// A challenge nobody asked about for this long is dropped when a new one
/// is issued, so a spoof flood has to keep up with real clients to evict them.
const CHALLENGE_TTL: Duration = Duration::from_secs(60);
/// `sv_timeout` default (cod_lnxded 0x80d56c0).
const TIMEOUT: Duration = Duration::from_secs(240);
/// `sv_reconnectlimit` default. Also how long a slot must have been silent
/// before a connect carrying a different challenge may reclaim it.
const RECONNECT_LIMIT: Duration = Duration::from_secs(3);
/// ioq3 `SVC_RateLimitAddress`: per source ip, a burst of 10 connectionless
/// requests, one back per second.
const ADDR_BURST: u32 = 10;
const ADDR_PERIOD: Duration = Duration::from_secs(1);
/// ioq3 `outboundLeakyBucket`: all connectionless replies, 10 per 100 ms.
const GLOBAL_BURST: u32 = 10;
const GLOBAL_PERIOD: Duration = Duration::from_millis(100);
/// Source ips tracked at once; the least recently seen is evicted.
const MAX_BUCKETS: usize = 1024;
/// clc ops, 2 bits on the wire.
const CLC_MOVE: i32 = 0;
const CLC_MOVE_NO_DELTA: i32 = 1;
const CLC_CLIENT_COMMAND: i32 = 2;
const CLC_EOF: i32 = 3;
const MAX_PACKET_USERCMDS: u8 = 32;
/// Queued-but-unreplayed usercmds per client; past this a flood drops the
/// oldest rather than building latency.
const MAX_PENDING_CMDS: usize = 64;
/// pmove steps per snapshot tick; a flood beyond this fast-forwards.
const MAX_CMDS_PER_TICK: usize = 32;
/// The tick pace (`1000 / sv_fps`), also the dt floor a fresh sim starts from.
const FRAME_MS: i32 = 50;
/// Retail's `MAX_CLIENTS`. Client slots index a 6-bit wire field
/// (clientState entries; `ps.clientNum` gets 8), so more than 64 would
/// collide silently.
const MAX_CLIENTS: usize = 64;

pub struct ServerConfig {
    pub map: String,
    pub hostname: String,
    pub max_clients: usize,
    /// `g_gametype` as text (`dm`, `tdm`, `sd`).
    pub gametype: String,
    /// Scripted entities that exercise the packet-entity wire path. 0 is off.
    pub test_entities: usize,
    /// Log one line per snapshot per client with the numbers that drive a
    /// client's prediction. Off by default; a busy server would flood.
    pub trace: bool,
}

/// `challenge_t`. Entries past `CHALLENGE_TTL` go on the next insert; the
/// oldest slot is recycled when the table is still full.
struct Challenge {
    addr: SocketAddr,
    challenge: i32,
    time: Instant,
    connected: bool,
}

/// ioq3 `leakyBucket_t`: `used` tokens, one drained per period.
struct Bucket {
    last: Instant,
    used: u32,
}

impl Bucket {
    /// `SVC_RateLimit`. True when this request is over the limit.
    fn limited(&mut self, now: Instant, burst: u32, period: Duration) -> bool {
        let interval = now.saturating_duration_since(self.last);
        let expired = interval.as_micros() / period.as_micros();
        if expired > u128::from(self.used) {
            self.used = 0;
            self.last = now;
        } else {
            self.used -= expired as u32;
            self.last += period * expired as u32;
        }
        if self.used < burst {
            self.used += 1;
            return false;
        }
        true
    }
}

/// The connectionless reply limiter, so `getstatus` cannot be used as a
/// reflector: a bucket per source ip in front of one for all replies.
struct RateLimiter {
    global: Bucket,
    addrs: HashMap<IpAddr, Bucket>,
}

impl RateLimiter {
    fn new(now: Instant) -> Self {
        RateLimiter {
            global: Bucket { last: now, used: 0 },
            addrs: HashMap::new(),
        }
    }

    /// `SVC_RateLimitAddress`. The global bucket is untouched by a request
    /// this rejects.
    fn addr_limited(&mut self, from: IpAddr, now: Instant) -> bool {
        if !self.addrs.contains_key(&from) && self.addrs.len() >= MAX_BUCKETS {
            let oldest = *self.addrs.iter().min_by_key(|(_, b)| b.last).unwrap().0;
            self.addrs.remove(&oldest);
        }
        self.addrs
            .entry(from)
            .or_insert(Bucket { last: now, used: 0 })
            .limited(now, ADDR_BURST, ADDR_PERIOD)
    }

    /// Both buckets, in ioq3's order; true when the reply must not go out.
    fn reply_limited(&mut self, from: IpAddr, now: Instant) -> bool {
        self.addr_limited(from, now) || self.global.limited(now, GLOBAL_BURST, GLOBAL_PERIOD)
    }
}

/// One op of a client message, read out before any of it is applied.
enum ClientOp {
    Command {
        seq: i32,
        text: String,
    },
    /// Every usercmd of a move block, in wire order.
    Move(Vec<UserCmd>),
}

pub struct Server {
    cfg: ServerConfig,
    huff: Huffman,
    proto: &'static Protocol,
    /// `sv_serverid`, `0x10` per map load plus the restart count in the low
    /// nibble. u8 because the client echoes it in a one-byte header field.
    server_id: u8,
    checksum_feed: i32,
    configstrings: Vec<String>,
    challenges: Vec<Challenge>,
    clients: Vec<Option<Client>>,
    outbox: Vec<(SocketAddr, Vec<u8>)>,
    limiter: RateLimiter,
    rng: u64,
    /// The map's collision and spawn, loaded by the binary; tests run without.
    world: Option<World>,
    /// `svs.time`, advanced one frame per tick.
    sv_time_ms: i32,
    /// Gamestate entity baselines a delta frame may omit an unchanged entity
    /// against; empty when `test_entities` is off.
    baselines: HashMap<u32, msg::EntityState>,
    /// Scripted entities driving the packet-entity path; `None` when
    /// `cfg.test_entities` is 0.
    test_entities: Option<TestEntities>,
    /// Wall clock of the previous tick, for the trace's send-interval column.
    last_tick: Option<Instant>,
    /// The map script, run once at load; `None` until `load_scripts` succeeds.
    script: Option<crate::game::script::ScriptRuntime>,
}

/// OOB argument text, minus a trailing line terminator.
fn oob_arg(rest: &[u8]) -> String {
    String::from_utf8_lossy(rest)
        .trim_end_matches(['\n', '\0'])
        .to_string()
}

/// `Cmd_Argv(1)`. The token goes back out inside an info string, so the info
/// separators come off it and the length is capped.
fn challenge_arg(arg: &str) -> String {
    const MAX: usize = 32;
    let mut out = String::with_capacity(MAX);
    for c in arg
        .split_whitespace()
        .next()
        .unwrap_or("")
        .chars()
        .filter(|c| *c != '\\' && *c != '"')
    {
        if out.len() + c.len_utf8() > MAX {
            break;
        }
        out.push(c);
    }
    out
}

/// `SV_UpdateServerCommandsToClient`. The caller bounds `from_ack` to
/// `0..=reliable_sequence`, so the range is empty or inside the ring.
fn write_pending_commands(w: &mut MsgWriter, nc: &ServerNetchan, from_ack: i32) {
    for seq in (from_ack + 1)..=(nc.reliable_sequence as i32) {
        msg::write_server_command(
            w,
            seq,
            &nc.reliable[seq as usize & (MAX_RELIABLE_COMMANDS - 1)],
        );
    }
}

impl Server {
    pub fn new(mut cfg: ServerConfig, now: Instant) -> Self {
        cfg.max_clients = cfg.max_clients.min(MAX_CLIENTS);
        let seed = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0x9e37_79b9_7f4a_7c15, |d| d.as_nanos() as u64)
            | 1;
        let server_id = 0x10;
        let mut sv = Server {
            configstrings: configstrings::static_configstrings(&cfg, server_id),
            clients: (0..cfg.max_clients).map(|_| None).collect(),
            cfg,
            huff: Huffman::new(),
            proto: &PROTOCOL_V1,
            server_id,
            checksum_feed: 0,
            challenges: Vec::new(),
            outbox: Vec::new(),
            limiter: RateLimiter::new(now),
            rng: seed,
            world: None,
            sv_time_ms: 0,
            baselines: HashMap::new(),
            test_entities: None,
            last_tick: None,
            script: None,
        };
        // `(rand() << 16) ^ rand() ^ Sys_Milliseconds()`, SV_SpawnServer 0x808a3e0.
        sv.checksum_feed = (sv.rand() << 16) ^ sv.rand() ^ (now.elapsed().as_millis() as i32);
        if sv.cfg.test_entities > 0 {
            let te = TestEntities::new(sv.cfg.test_entities, [0.0, 0.0, 64.0]);
            sv.baselines = te.baselines(sv.proto);
            sv.test_entities = Some(te);
        }
        sv
    }

    /// xorshift64*, masked to 31 bits like glibc's `rand()` so that
    /// `(rand() << 16) ^ rand()` wraps into the sign bit and challenges come
    /// out signed like retail's.
    fn rand(&mut self) -> i32 {
        self.rng ^= self.rng >> 12;
        self.rng ^= self.rng << 25;
        self.rng ^= self.rng >> 27;
        (self.rng.wrapping_mul(0x2545_f491_4f6c_dd1d) >> 33) as i32 & 0x7fff_ffff
    }

    pub fn configstring(&self, i: usize) -> &str {
        self.configstrings.get(i).map_or("", String::as_str)
    }

    pub fn client_count(&self) -> usize {
        self.clients.iter().flatten().count()
    }

    pub fn take_outgoing(&mut self) -> Vec<(SocketAddr, Vec<u8>)> {
        std::mem::take(&mut self.outbox)
    }

    fn send_oob(&mut self, to: SocketAddr, text: &str) {
        self.outbox.push((to, build_oob(text)));
    }

    /// `SV_PacketEvent`.
    pub fn handle_packet(&mut self, from: SocketAddr, pkt: &[u8], now: Instant) {
        match parse_oob(pkt) {
            Some((cmd, rest)) => {
                let cmd = cmd.to_string();
                self.handle_oob(from, &cmd, rest, pkt, now);
            }
            None => self.handle_client_packet(from, pkt, now),
        }
    }

    /// `connect` re-parses `raw` because its body is compressed; only the
    /// browser queries read `rest` as text. The queries are rate limited
    /// before anything is looked at; a limited request is dropped silently.
    fn handle_oob(&mut self, from: SocketAddr, cmd: &str, rest: &[u8], raw: &[u8], now: Instant) {
        match cmd {
            "getinfo" | "getstatus" | "getchallenge" => {
                if self.limiter.reply_limited(from.ip(), now) {
                    log::debug!("{from}: {cmd} rate limited");
                    return;
                }
                match cmd {
                    "getinfo" => self.svc_info(from, &oob_arg(rest)),
                    "getstatus" => self.svc_status(from, &oob_arg(rest)),
                    _ => self.svc_get_challenge(from, now),
                }
            }
            "connect" => self.svc_direct_connect(from, raw, now),
            // Retail drops by source address alone; the bucket keeps a spoofer
            // from cycling a slot faster than the client can reconnect.
            "disconnect" => {
                if self.limiter.addr_limited(from.ip(), now) {
                    log::debug!("{from}: disconnect rate limited");
                    return;
                }
                self.oob_disconnect(from)
            }
            other => log::debug!("{from}: unhandled oob {other:?}"),
        }
    }

    /// `SVC_Info` (cod_lnxded 0x808c1ac). Key order is retail's;
    /// `minPing`/`maxPing`/`game` only appear when the matching cvar is set.
    fn svc_info(&mut self, from: SocketAddr, challenge: &str) {
        let mut i = Info::new();
        i.set("challenge", challenge_arg(challenge))
            .set("protocol", PROTOCOL_V1.version)
            .set("hostname", &self.cfg.hostname)
            .set("mapname", &self.cfg.map)
            .set("clients", self.client_count())
            .set("sv_maxclients", self.cfg.max_clients)
            .set("gametype", &self.cfg.gametype)
            .set("pure", 0)
            .set("sv_allowAnonymous", 0)
            .set("pswrd", 0);
        self.send_oob(from, &format!("infoResponse\n{i}"));
    }

    /// `SVC_Status` (0x808bd50).
    fn svc_status(&mut self, from: SocketAddr, challenge: &str) {
        let mut i = configstrings::serverinfo(&self.cfg);
        i.set("challenge", challenge_arg(challenge)).set("pswrd", 0);
        let mut lines = String::new();
        for c in self.clients.iter().flatten() {
            lines.push_str(&format!("0 0 \"{}\"\n", c.name));
        }
        self.send_oob(from, &format!("statusResponse\n{i}\n{lines}"));
    }

    /// `SV_GetChallenge` without the authorize-server detour. A client still
    /// asking keeps its entry fresh; stale entries go before a new insert.
    fn svc_get_challenge(&mut self, from: SocketAddr, now: Instant) {
        let challenge = match self
            .challenges
            .iter_mut()
            .find(|c| !c.connected && c.addr == from)
        {
            Some(c) => {
                c.time = now;
                c.challenge
            }
            None => {
                self.challenges
                    .retain(|c| now.duration_since(c.time) < CHALLENGE_TTL);
                let challenge = (self.rand() << 16) ^ self.rand();
                let entry = Challenge {
                    addr: from,
                    challenge,
                    time: now,
                    connected: false,
                };
                if self.challenges.len() < MAX_CHALLENGES {
                    self.challenges.push(entry);
                } else {
                    let oldest = (0..self.challenges.len())
                        .min_by_key(|&i| self.challenges[i].time)
                        .unwrap();
                    self.challenges[oldest] = entry;
                }
                challenge
            }
        };
        self.send_oob(from, &format!("challengeResponse {challenge}"));
    }

    /// `SV_DirectConnect` (sv_client.c:252) with CoD's rejection strings.
    fn svc_direct_connect(&mut self, from: SocketAddr, raw: &[u8], now: Instant) {
        let userinfo = match parse_connect(raw) {
            Ok(u) => u,
            Err(e) => {
                log::debug!("{from}: bad connect: {e}");
                return;
            }
        };
        let val = |k: &str| info_value_for_key(&userinfo, k).and_then(|v| v.parse::<i32>().ok());
        if val("protocol") != Some(self.proto.version as i32) {
            self.send_oob(from, "error\nEXE_SERVER_IS_DIFFERENT_VER 1.1");
            return;
        }
        let (Some(challenge), Some(qport)) = (val("challenge"), val("qport")) else {
            self.send_oob(from, "error\nEXE_BAD_CHALLENGE");
            return;
        };
        let qport = qport as u16;
        let same_peer = |c: &Client| {
            c.addr.ip() == from.ip() && (c.netchan.qport == qport || c.addr.port() == from.port())
        };
        if self
            .clients
            .iter()
            .flatten()
            .any(|c| same_peer(c) && now.duration_since(c.last_connect) < RECONNECT_LIMIT)
        {
            log::debug!("{from}: reconnect rejected, too soon");
            return;
        }
        // By ip alone, as retail does: the port may have moved behind a NAT.
        let Some(ci) = self
            .challenges
            .iter()
            .position(|c| c.addr.ip() == from.ip() && c.challenge == challenge)
        else {
            self.send_oob(from, "error\nEXE_BAD_CHALLENGE");
            return;
        };
        self.challenges[ci].connected = true;
        let slot = match self
            .clients
            .iter()
            .position(|c| c.as_ref().is_some_and(same_peer))
        {
            Some(i) => {
                // Retail hands the slot to any challenge issued to this ip, so
                // a neighbour behind the same NAT who knows the qport can take
                // over a live player. Only the slot's own challenge (the
                // client's connect retry) may replace a client still heard
                // from; a silent slot (crash, lost disconnect) is reclaimable.
                let c = self.clients[i].as_ref().unwrap();
                if c.netchan.challenge != challenge
                    && now.duration_since(c.last_packet) < RECONNECT_LIMIT
                {
                    log::info!(
                        "{from}: connect with a foreign challenge refused, client {i} is live"
                    );
                    return;
                }
                log::info!("{from}: reconnect");
                i
            }
            None => match self.clients.iter().position(Option::is_none) {
                Some(i) => i,
                None => {
                    self.send_oob(from, "error\nEXE_SERVERISFULL");
                    return;
                }
            },
        };
        let client = Client::new(from, qport, challenge, userinfo, now);
        log::info!("client {slot} {:?} connected from {from}", client.name);
        self.clients[slot] = Some(client);
        self.send_oob(from, "connectResponse");
    }

    /// OOB `disconnect` (0x808c827).
    fn oob_disconnect(&mut self, from: SocketAddr) {
        if let Some(slot) = self
            .clients
            .iter()
            .position(|c| c.as_ref().is_some_and(|c| c.addr == from))
        {
            self.drop_client(slot, "EXE_DISCONNECTED");
        }
    }

    /// The netchan half of `SV_PacketEvent`, then `SV_ExecuteClientMessage`.
    /// The slot changes only once the message has passed every check the
    /// plain header and the op stream allow (`Client::accept`), so a packet
    /// forged with the client's ip and qport cannot stall it behind a huge
    /// sequence, redirect its replies or keep a dead slot alive.
    fn handle_client_packet(&mut self, from: SocketAddr, pkt: &[u8], now: Instant) {
        if pkt.len() < 6 {
            log::debug!("{from}: netchan packet too short ({} bytes)", pkt.len());
            return;
        }
        let qport = u16::from_le_bytes([pkt[4], pkt[5]]);
        let Some(slot) = self.clients.iter().position(|c| {
            c.as_ref()
                .is_some_and(|c| c.addr.ip() == from.ip() && c.netchan.qport == qport)
        }) else {
            log::debug!("{from}: netchan packet from no client (qport {qport})");
            return;
        };
        let Some(c) = self.clients[slot].as_ref() else {
            return;
        };
        let Some(m) = c.netchan.process_in(pkt) else {
            return;
        };
        // messageAcknowledge names a message we sent, so it is below
        // outgoing_sequence; retail only rejects a negative one.
        if m.message_ack < 0 || m.message_ack >= c.netchan.outgoing_sequence as i32 {
            log::debug!(
                "client {slot}: messageAcknowledge {} out of range",
                m.message_ack
            );
            return;
        }
        // 0x808ca1a drops an ack too far behind. One ahead of what we sent is
        // bogus too; `write_pending_commands` walks `reliable_ack + 1 ..=
        // reliable_sequence`, so an unbounded value overflows or loops for ages.
        let reliable_seq = c.netchan.reliable_sequence as i32;
        if m.reliable_ack < 0
            || m.reliable_ack > reliable_seq
            || reliable_seq.saturating_sub(m.reliable_ack) > MAX_RELIABLE_COMMANDS as i32 - 1
        {
            log::debug!(
                "client {slot}: reliableAcknowledge {} out of range",
                m.reliable_ack
            );
            return;
        }

        if m.server_id != self.server_id {
            let c = self.clients[slot].as_mut().unwrap();
            if m.server_id & 0xf0 != self.server_id & 0xf0 {
                // A map change from the client's view (a fresh client, serverId
                // 0, looks the same). Resend the gamestate once its ack is past
                // the last one. Until snapshots exist the gamestate is our only
                // message, so a lost one never advances the ack and the client
                // falls to its own timeout.
                if i64::from(m.message_ack) > c.gamestate_message_num {
                    c.accept(&m, now);
                    self.send_gamestate(slot);
                }
            } else if c.state == ClientState::Primed {
                // Restart path; retail promotes to CS_ACTIVE here.
                c.accept(&m, now);
                self.enter_world(slot);
            }
            return;
        }

        let base = c.last_cmd;
        let Some((ops, last_cmd)) = self.parse_client_ops(c, &m, base) else {
            log::debug!("client {slot}: message from {from} does not decode");
            return;
        };
        // The new delta base commits before any op runs; a message that fails
        // to decode left it alone above.
        self.clients[slot].as_mut().unwrap().last_cmd = last_cmd;
        // Acking the gamestate message is the enter-world trigger. Equal, not
        // strictly past: a message's fragments share one sequence, so the
        // client's first ack lands exactly on gamestate_message_num, and no
        // later server message exists yet to push it past.
        let entering = {
            let c = self.clients[slot].as_mut().unwrap();
            c.accept(&m, now);
            c.addr = from; // NAT may move the port; the qport is the identity
            c.state == ClientState::Primed && i64::from(m.message_ack) >= c.gamestate_message_num
        };
        if entering {
            self.enter_world(slot);
        }
        for op in ops {
            match op {
                ClientOp::Command { seq, text } => {
                    if !self.client_command(slot, seq, text) {
                        return;
                    }
                }
                ClientOp::Move(last) => self.user_move(slot, last),
            }
        }
    }

    /// `SV_ExecuteClientMessage`'s op walk, read to the end before any op is
    /// applied. `None` when the stream does not decode: an overflow anywhere
    /// or a bad usercmd count, which is what a forged block looks like once
    /// the scramble key is wrong. On success the new delta base comes back
    /// with the ops: the last cmd decoded, for the next move message to
    /// chain from.
    fn parse_client_ops(
        &self,
        c: &Client,
        m: &ClientMessage,
        base: UserCmd,
    ) -> Option<(Vec<ClientOp>, UserCmd)> {
        let mut r = MsgReader::new(&m.ops, &self.huff);
        let mut ops = Vec::new();
        let mut prev = base;
        loop {
            let op = r.read_bits(2);
            if r.is_overflowed() {
                return None;
            }
            match op {
                CLC_CLIENT_COMMAND => {
                    let seq = r.read_long();
                    let text = r.read_string();
                    if r.is_overflowed() {
                        return None;
                    }
                    ops.push(ClientOp::Command { seq, text });
                }
                CLC_EOF => return Some((ops, prev)),
                CLC_MOVE | CLC_MOVE_NO_DELTA => {
                    let cmds = self.parse_move(c, &mut r, m, &mut prev)?;
                    ops.push(ClientOp::Move(cmds));
                    return Some((ops, prev));
                }
                // `read_bits(2)` yields 0..=3; unreachable.
                _ => return None,
            }
        }
    }

    /// `SV_UserMove`'s parse: the whole block, in order, so the sim can
    /// replay every cmd like retail's pmove does. `prev` enters as the
    /// client's stored delta base and leaves as the last cmd of the block.
    fn parse_move(
        &self,
        c: &Client,
        r: &mut MsgReader,
        m: &ClientMessage,
        prev: &mut UserCmd,
    ) -> Option<Vec<UserCmd>> {
        let count = r.read_byte();
        if !(1..=MAX_PACKET_USERCMDS).contains(&count) {
            return None;
        }
        let cmd = &c.netchan.reliable[m.reliable_ack as usize & (MAX_RELIABLE_COMMANDS - 1)];
        let key = self.checksum_feed ^ m.message_ack ^ com_hash_key(cmd, 32);
        let mut out = Vec::with_capacity(count as usize);
        for _ in 0..count {
            match read_delta_usercmd(r, key, prev) {
                Ok(next) => {
                    out.push(next);
                    *prev = next;
                }
                Err(e) => {
                    log::debug!("usercmd not parsed: {e}");
                    return None;
                }
            }
        }
        Some(out)
    }

    /// `SV_ClientCommand`. Returns false when the client is gone.
    fn client_command(&mut self, slot: usize, seq: i32, s: String) -> bool {
        let Some(c) = self.clients[slot].as_mut() else {
            return false;
        };
        if seq <= c.last_client_command {
            return true;
        }
        if seq > c.last_client_command.saturating_add(1) {
            self.drop_client(slot, "EXE_LOSTRELIABLECOMMANDS");
            return false;
        }
        // Split rather than slice; the command may arrive with leading whitespace.
        let trimmed = s.trim_start();
        let (word, args) = trimmed
            .split_once(char::is_whitespace)
            .unwrap_or((trimmed, ""));
        match word {
            "disconnect" => {
                self.drop_client(slot, "EXE_DISCONNECTED");
                return false;
            }
            "userinfo" => {
                let ui = args.trim().trim_matches('"').to_string();
                if let Some(c) = self.clients[slot].as_mut() {
                    if let Some(name) = info_value_for_key(&ui, "name") {
                        c.name = sanitize_name(name);
                    }
                    c.userinfo = ui;
                }
            }
            // DeathmatchScoreboardMessage (.so 0x459c0); grammar in
            // docs/research/cod11-hud-protocol.md section 3. Nothing is
            // measured, so team totals use the "no score" sentinel and rows
            // zero out.
            "score" => {
                let mut text = format!("b {} -9999 -9999", self.client_count());
                for (cs_slot, c) in self.clients.iter().enumerate() {
                    if c.is_some() {
                        text.push_str(&format!(" {cs_slot} 0 0 0 0"));
                    }
                }
                self.send_server_command(slot, &text);
            }
            other => log::debug!("client {slot}: command {other:?} ignored"),
        }
        let Some(c) = self.clients[slot].as_mut() else {
            return false;
        };
        c.last_client_command = seq;
        c.netchan.last_client_command_string = s;
        true
    }

    /// `SV_UserMove` past the parse: queue the block for the next tick's
    /// replay. A flood past the cap drops the oldest cmds, not the newest.
    fn user_move(&mut self, slot: usize, cmds: Vec<UserCmd>) {
        let Some(c) = self.clients[slot].as_mut() else {
            return;
        };
        c.pending.extend(cmds);
        let excess = c.pending.len().saturating_sub(MAX_PENDING_CMDS);
        c.pending.drain(..excess);
    }

    /// `SV_SendClientGameState`.
    fn send_gamestate(&mut self, slot: usize) {
        let Some(c) = self.clients[slot].as_mut() else {
            return;
        };
        c.state = ClientState::Primed;
        c.gamestate_message_num = i64::from(c.netchan.outgoing_sequence);
        let mut w = MsgWriter::new(&self.huff);
        write_pending_commands(&mut w, &c.netchan, c.reliable_ack);
        let gs = Gamestate {
            configstrings: self.configstrings.clone(),
            baselines: self.baselines.clone(),
            client_num: slot as i32,
            checksum_feed: self.checksum_feed,
            server_command_sequence: c.netchan.reliable_sequence as i32,
        };
        gamestate::write(&mut w, self.proto, &gs);
        let ops = w.into_ops();
        log::info!("client {slot}: gamestate, {} bytes", ops.len());
        for pkt in c.netchan.transmit(c.last_client_command, &ops, &self.huff) {
            self.outbox.push((c.addr, pkt));
        }
    }

    /// `SV_AddServerCommand` plus an immediate message, so a drop notice
    /// reaches the client before its slot is freed. A client whose acks fall
    /// a whole ring behind has stopped consuming reliables; overwriting an
    /// unsacked slot would desync both ends' scramble keys, so that is fatal
    /// (`EXE_LOSTRELIABLECOMMANDS`), mirroring the reference client's own
    /// outbound guard.
    fn send_server_command(&mut self, slot: usize, cmd: &str) {
        let overflow = match self.clients[slot].as_ref() {
            Some(c) => {
                i64::from(c.netchan.reliable_sequence) - i64::from(c.reliable_ack)
                    >= MAX_RELIABLE_COMMANDS as i64
            }
            None => return,
        };
        if overflow {
            self.drop_client(slot, "EXE_LOSTRELIABLECOMMANDS");
            return;
        }
        self.write_server_command(slot, cmd);
    }

    /// The unconditional tail of [`Self::send_server_command`]. The drop
    /// notice goes out through here, so freeing the slot cannot recurse and
    /// the notice is never guarded away.
    fn write_server_command(&mut self, slot: usize, cmd: &str) {
        let Some(c) = self.clients[slot].as_mut() else {
            return;
        };
        c.netchan.reliable_sequence += 1;
        let seq = c.netchan.reliable_sequence;
        c.netchan.reliable[seq as usize & (MAX_RELIABLE_COMMANDS - 1)] = cmd.to_string();
        let mut w = MsgWriter::new(&self.huff);
        write_pending_commands(&mut w, &c.netchan, c.reliable_ack);
        // The netchan appends the `svc_EOF`.
        for pkt in c
            .netchan
            .transmit(c.last_client_command, &w.into_ops(), &self.huff)
        {
            self.outbox.push((c.addr, pkt));
        }
    }

    /// `SV_DropClient` (cod_lnxded 0x8085cf4). No zombie state; nothing here
    /// needs the grace period.
    fn drop_client(&mut self, slot: usize, reason: &str) {
        self.write_server_command(slot, &format!("w \"{reason}\""));
        let Some(c) = self.clients[slot].take() else {
            return;
        };
        // Match on ip, not `ch.addr == c.addr`; NAT may have moved the port
        // since the challenge was issued and the slot would stay `connected`.
        for ch in &mut self.challenges {
            if ch.addr.ip() == c.addr.ip() && ch.challenge == c.netchan.challenge {
                ch.connected = false;
            }
        }
        log::info!("client {slot} {:?} dropped: {reason}", c.name);
    }

    /// `SV_CheckTimeouts` with `sv_timeout` and no timeoutCount hysteresis.
    fn check_timeouts(&mut self, now: Instant) {
        let stale: Vec<usize> = self
            .clients
            .iter()
            .enumerate()
            .filter_map(|(i, c)| {
                c.as_ref()
                    .filter(|c| now.duration_since(c.last_packet) > TIMEOUT)
                    .map(|_| i)
            })
            .collect();
        for slot in stale {
            self.drop_client(slot, "EXE_TIMEDOUT");
        }
    }

    /// Swap in the map built by the binary; tests run without one.
    pub fn load_world(&mut self, world: World) {
        self.world = Some(world);
    }

    /// Loads and runs the map script. Called once at map load, before any
    /// client connects, so the configstring table is final by the time a
    /// gamestate goes out; a script write after that would need the `d`
    /// configstring-update command, which arrives in a later stage.
    pub fn load_scripts(&mut self, fs: std::rc::Rc<vcod_common::pk3::Pk3Fs>) -> anyhow::Result<()> {
        let mut rt = crate::game::script::ScriptRuntime::load(fs, &self.cfg.map)?;
        let mut host = crate::game::host::GameHost::new(std::mem::take(&mut self.configstrings));
        rt.start_map_main(&mut host, self.sv_time_ms);
        rt.run_frame(&mut host, self.sv_time_ms);
        self.configstrings = std::mem::take(&mut host.configstrings);
        self.script = Some(rt);
        Ok(())
    }

    const FALLBACK_SPAWN: ([f32; 3], f32) = ([0.0, 0.0, 64.0], 0.0);

    /// `SV_ClientEnterWorld` for a spectator: park the sim at the spawn, start
    /// snapping.
    fn enter_world(&mut self, slot: usize) {
        let spawn = self
            .world
            .as_ref()
            .map_or(Self::FALLBACK_SPAWN, |w| w.spawn);
        let Some(c) = self.clients[slot].as_mut() else {
            return;
        };
        c.state = ClientState::Active;
        // Fresh start, matching the client's own outCmd reset on map entry.
        c.last_cmd = NULL_USERCMD;
        c.sim = Some(SpectatorSim::new(spawn.0, spawn.1));
        // One frame back, so the first cmd's dt is a sane 50 ms rather than
        // the whole age of the client's clock.
        c.last_processed_st = self.sv_time_ms.wrapping_sub(FRAME_MS);
        log::info!("client {slot} {:?} begin (spectator)", c.name);
    }

    pub fn tick(&mut self, now: Instant) {
        self.check_timeouts(now);
        self.send_snapshots(now);
    }

    /// One snapshot per active client per tick, the main loop pacing calls at
    /// sv_fps: a delta against the frame the client last acked when one is
    /// still in its ring, uncompressed otherwise. The sim replays every
    /// queued usercmd first, dt off the cmd clocks, matching the client's own
    /// prediction.
    fn send_snapshots(&mut self, now: Instant) {
        self.sv_time_ms = self.sv_time_ms.wrapping_add(FRAME_MS);
        // Wall gap between ticks: sv_time always advances exactly FRAME_MS, so
        // a gap far off it means the frames the client interpolates between
        // are not arriving at the rate their timestamps claim.
        let wall_ms = self
            .last_tick
            .map(|t| now.saturating_duration_since(t).as_secs_f32() * 1000.0);
        self.last_tick = Some(now);

        // One clientState entry per online client, rebuilt each frame; slot ==
        // index. `snapshot::write` deltas this against each client's own
        // base roster, or sends it full when that client has none.
        let roster: BTreeMap<u32, msg::ClientState> = self
            .clients
            .iter()
            .enumerate()
            .filter_map(|(i, c)| {
                c.as_ref()
                    .map(|c| (i as u32, msg::ClientState::named(self.proto, 0, 3, &c.name)))
            })
            .collect();
        let entities: BTreeMap<u32, msg::EntityState> = self
            .test_entities
            .as_ref()
            .map_or_else(BTreeMap::new, |te| te.at(self.proto, self.sv_time_ms));

        for slot in 0..self.clients.len() {
            let Some(c) = self.clients[slot].as_mut() else {
                continue;
            };
            let Some(sim) = c.sim.as_mut() else {
                continue;
            };
            // SV_UserMove: one pmove step per usercmd, dt off the cmd clocks.
            // Stale cmds (dt <= 0) are skipped whole; a flood past the per-tick
            // cap resyncs to the newest cmd and keeps only the tail.
            let mut processed = 0usize;
            let (mut first_cmd_st, mut last_cmd_st) = (None::<i32>, None::<i32>);
            while !c.pending.is_empty() {
                let cmd = c.pending[0];
                if processed >= MAX_CMDS_PER_TICK {
                    // The resync keeps the newest two cmds and sets the base
                    // as if only the last replays, so the penultimate may
                    // double-count one frame; harmless for flight.
                    c.last_processed_st =
                        c.pending.last().unwrap().server_time.wrapping_sub(FRAME_MS);
                    c.pending.drain(..c.pending.len().saturating_sub(2));
                    break;
                }
                c.pending.remove(0);
                let dt_ms = cmd.server_time.wrapping_sub(c.last_processed_st);
                if dt_ms <= 0 {
                    continue;
                }
                let dt = (dt_ms as f32 / 1000.0).min(MAX_FRAME_MS / 1000.0);
                sim.step(&cmd, dt);
                c.last_processed_st = cmd.server_time;
                first_cmd_st.get_or_insert(cmd.server_time);
                last_cmd_st = Some(cmd.server_time);
                processed += 1;
            }
            // Exactly the serverTime of the last cmd the sim consumed, and
            // nothing else: the client replays everything past it, so a
            // commandTime we never simulated drops that slice of its input
            // and its prediction judders (docs/protocol-1.1.md).
            let command_time = c.last_processed_st;
            let message_num = c.netchan.outgoing_sequence;

            let frame = snapshot::Snapshot {
                server_time: self.sv_time_ms,
                message_num,
                delta_num: -1,
                snap_flags: 0,
                ps: sim.to_wire(self.proto, slot as i32, command_time),
                entities: entities.clone(),
                clients: roster.clone(),
                valid: true,
            };

            // The base is the frame the client last acked, if it is still in
            // the ring and close enough for the byte-wide deltaNum offset.
            // Safe at a full SV_PACKET_BACKUP depth (no margin, unlike
            // retail's SV_WriteSnapshotToClient, which keeps PACKET_BACKUP -
            // 3) only because both this ring and the client's own read a slot
            // before either side can overwrite it.
            let base = c
                .sent_frame(c.message_ack.max(0) as u32)
                .filter(|b| {
                    let back = message_num.saturating_sub(b.message_num);
                    (1..=255).contains(&back)
                })
                .cloned();

            let mut w = MsgWriter::new(&self.huff);
            write_pending_commands(&mut w, &c.netchan, c.reliable_ack);
            w.write_byte(snapshot::SVC_SNAPSHOT);
            snapshot::write(&mut w, self.proto, base.as_ref(), &frame, &self.baselines);
            c.record_frame(frame);

            let ops = w.into_ops();

            if self.cfg.trace {
                // The prediction contract: a client replays every usercmd
                // newer than ps.commandTime, so `lead` at or below zero means
                // we claim to have simulated past our own frame clock and the
                // client has nothing left to replay. Retail's captures run a
                // lead of 0..34 ms, never negative (examples/snapshot_timing).
                let lead = self.sv_time_ms - command_time;
                let queued = c.pending.len();
                let span = match (first_cmd_st, last_cmd_st) {
                    (Some(a), Some(b)) => format!("{a}..{b}"),
                    _ => "-".into(),
                };
                let ack_behind = message_num as i64 - i64::from(c.message_ack);
                let base_desc = match base.as_ref() {
                    Some(b) => format!("d{}", message_num - b.message_num),
                    None => "uncompressed".into(),
                };
                log::info!(
                    "trace c{slot} msg {message_num} wall {} sv {} ct {} lead {} \
cmds {processed} span {span} queued {queued} ack {} behind {ack_behind} {base_desc} {} B",
                    wall_ms.map_or("-".into(), |w| format!("{w:.1}")),
                    self.sv_time_ms,
                    command_time,
                    lead,
                    c.message_ack,
                    ops.len(),
                );
            }
            for pkt in c.netchan.transmit(c.last_client_command, &ops, &self.huff) {
                self.outbox.push((c.addr, pkt));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world::World;
    use vcod_common::collision::test_world;
    use vcod_common::net::connectionless::{
        build_connect, build_oob, info_value_for_key, parse_oob,
    };
    use vcod_common::net::msg::write_delta_usercmd;
    use vcod_common::net::netchan::Netchan;
    use vcod_common::net::snapshot::{SnapshotRing, SVC_SNAPSHOT};

    const QPORT: u16 = 0x2001;

    fn cfg() -> ServerConfig {
        ServerConfig {
            map: "mp_carentan".into(),
            hostname: "vcod test".into(),
            max_clients: 4,
            gametype: "dm".into(),
            test_entities: 0,
            trace: false,
        }
    }
    fn addr(port: u16) -> SocketAddr {
        SocketAddr::from(([127, 0, 0, 1], port))
    }
    fn oob(cmd: &str) -> Vec<u8> {
        build_oob(cmd)
    }
    fn reply(sv: &mut Server) -> (SocketAddr, String, Vec<u8>) {
        let mut out = sv.take_outgoing();
        assert_eq!(out.len(), 1, "expected one packet");
        let (to, pkt) = out.remove(0);
        let (cmd, rest) = parse_oob(&pkt).expect("oob reply");
        (to, cmd.to_string(), rest.to_vec())
    }
    fn reply_text(sv: &mut Server) -> (String, String) {
        let (_, cmd, rest) = reply(sv);
        (cmd, String::from_utf8_lossy(&rest).trim().to_string())
    }
    fn challenge_for(sv: &mut Server, from: SocketAddr, now: Instant) -> i32 {
        sv.handle_packet(from, &oob("getchallenge"), now);
        let (cmd, body) = reply_text(sv);
        assert_eq!(cmd, "challengeResponse");
        body.parse().expect("a numeric challenge")
    }
    fn connect_pkt(challenge: i32, qport: u16, protocol: u32) -> Vec<u8> {
        build_connect(&format!(
            "\\name\\vcod\\protocol\\{protocol}\\qport\\{qport}\\challenge\\{challenge}"
        ))
    }
    fn connected(sv: &mut Server, from: SocketAddr, now: Instant) -> Netchan {
        let challenge = challenge_for(sv, from, now);
        sv.handle_packet(
            from,
            &connect_pkt(challenge, QPORT, PROTOCOL_V1.version),
            now,
        );
        assert_eq!(reply_text(sv).0, "connectResponse");
        Netchan::new(QPORT, challenge)
    }
    fn server_commands(nc: &mut Netchan, pkt: &[u8], huff: &Huffman) -> Vec<String> {
        let msg = nc
            .process_in(pkt, huff)
            .expect("a netchan packet")
            .expect("a whole message");
        let mut r = MsgReader::new(&msg[4..], huff);
        let mut out = Vec::new();
        while !r.is_overflowed() {
            match r.read_byte() {
                msg::SVC_SERVER_COMMAND => {
                    r.read_long();
                    out.push(r.read_big_string());
                }
                _ => break,
            }
        }
        out
    }

    #[test]
    fn getinfo_echoes_the_challenge_and_names_the_map() {
        let now = Instant::now();
        let mut sv = Server::new(cfg(), now);
        sv.handle_packet(addr(5), &oob("getinfo 4242"), now);
        let (to, cmd, rest) = reply(&mut sv);
        assert_eq!(to, addr(5));
        assert_eq!(cmd, "infoResponse");
        let info = String::from_utf8_lossy(&rest);
        assert_eq!(info_value_for_key(&info, "challenge"), Some("4242"));
        assert_eq!(info_value_for_key(&info, "mapname"), Some("mp_carentan"));
        assert_eq!(info_value_for_key(&info, "protocol"), Some("1"));
        assert_eq!(info_value_for_key(&info, "clients"), Some("0"));
        assert_eq!(info_value_for_key(&info, "sv_maxclients"), Some("4"));
        assert_eq!(info_value_for_key(&info, "gametype"), Some("dm"));
        assert_eq!(info_value_for_key(&info, "pswrd"), Some("0"));
        // Key order is retail's; pin the whole string.
        assert_eq!(
            info,
            "\\challenge\\4242\\protocol\\1\\hostname\\vcod test\\mapname\\mp_carentan\\clients\\0\\sv_maxclients\\4\\gametype\\dm\\pure\\0\\sv_allowAnonymous\\0\\pswrd\\0"
        );
    }

    #[test]
    fn getinfo_sanitizes_the_challenge_argument() {
        let now = Instant::now();
        let mut sv = Server::new(cfg(), now);
        sv.handle_packet(addr(5), &oob("getinfo a\\pure\\1 extra"), now);
        let (_, cmd, rest) = reply(&mut sv);
        assert_eq!(cmd, "infoResponse");
        let info = String::from_utf8_lossy(&rest);
        assert_eq!(info_value_for_key(&info, "challenge"), Some("apure1"));
        assert_eq!(info_value_for_key(&info, "pure"), Some("0"));
    }

    #[test]
    fn getstatus_has_serverinfo_and_no_player_lines() {
        let now = Instant::now();
        let mut sv = Server::new(cfg(), now);
        sv.handle_packet(addr(5), &oob("getstatus 7"), now);
        let (_, cmd, rest) = reply(&mut sv);
        assert_eq!(cmd, "statusResponse");
        let text = String::from_utf8_lossy(&rest);
        let (info, players) = text.split_once('\n').unwrap();
        assert_eq!(info_value_for_key(info, "sv_hostname"), Some("vcod test"));
        assert_eq!(info_value_for_key(info, "challenge"), Some("7"));
        assert_eq!(players, "");
    }

    #[test]
    fn getchallenge_is_stable_per_address() {
        let now = Instant::now();
        let mut sv = Server::new(cfg(), now);
        sv.handle_packet(addr(5), &oob("getchallenge"), now);
        let (_, cmd, a) = reply(&mut sv);
        assert_eq!(cmd, "challengeResponse");
        sv.handle_packet(addr(5), &oob("getchallenge"), now);
        let (_, _, b) = reply(&mut sv);
        assert_eq!(a, b);
        sv.handle_packet(addr(6), &oob("getchallenge"), now);
        let (_, _, c) = reply(&mut sv);
        assert_ne!(a, c);
        assert!(String::from_utf8_lossy(&a).trim().parse::<i32>().is_ok());
    }

    #[test]
    fn serverinfo_is_configstring_zero() {
        let sv = Server::new(cfg(), Instant::now());
        let cs0 = sv.configstring(0);
        assert_eq!(info_value_for_key(cs0, "mapname"), Some("mp_carentan"));
        assert_eq!(
            info_value_for_key(sv.configstring(1), "sv_serverid"),
            Some("16")
        );
        assert!(sv.configstring(7).contains("kar98k_mp"));
    }

    #[test]
    fn a_valid_connect_takes_a_slot() {
        let now = Instant::now();
        let mut sv = Server::new(cfg(), now);
        let challenge = challenge_for(&mut sv, addr(5), now);
        sv.handle_packet(
            addr(5),
            &connect_pkt(challenge, QPORT, PROTOCOL_V1.version),
            now,
        );
        let (to, cmd, _) = reply(&mut sv);
        assert_eq!(to, addr(5));
        assert_eq!(cmd, "connectResponse");
        assert_eq!(sv.client_count(), 1);
    }

    #[test]
    fn a_connect_on_the_wrong_protocol_is_rejected() {
        let now = Instant::now();
        let mut sv = Server::new(cfg(), now);
        let challenge = challenge_for(&mut sv, addr(5), now);
        sv.handle_packet(addr(5), &connect_pkt(challenge, QPORT, 6), now);
        assert_eq!(
            reply_text(&mut sv),
            (
                "error".to_string(),
                "EXE_SERVER_IS_DIFFERENT_VER 1.1".to_string()
            )
        );
        assert_eq!(sv.client_count(), 0);
    }

    #[test]
    fn a_connect_with_a_challenge_we_never_issued_is_rejected() {
        let now = Instant::now();
        let mut sv = Server::new(cfg(), now);
        let challenge = challenge_for(&mut sv, addr(5), now);
        sv.handle_packet(
            addr(5),
            &connect_pkt(challenge.wrapping_add(1), QPORT, PROTOCOL_V1.version),
            now,
        );
        assert_eq!(
            reply_text(&mut sv),
            ("error".to_string(), "EXE_BAD_CHALLENGE".to_string())
        );
        assert_eq!(sv.client_count(), 0);
    }

    #[test]
    fn a_full_server_rejects_the_next_connect() {
        let now = Instant::now();
        let mut sv = Server::new(
            ServerConfig {
                max_clients: 1,
                ..cfg()
            },
            now,
        );
        connected(&mut sv, addr(5), now);
        assert_eq!(sv.client_count(), 1);

        // Neither qport nor port matches, so not a reconnect of the first client.
        let challenge = challenge_for(&mut sv, addr(6), now);
        sv.handle_packet(
            addr(6),
            &connect_pkt(challenge, QPORT + 1, PROTOCOL_V1.version),
            now,
        );
        assert_eq!(
            reply_text(&mut sv),
            ("error".to_string(), "EXE_SERVERISFULL".to_string())
        );
        assert_eq!(sv.client_count(), 1);
    }

    #[test]
    fn a_message_with_an_out_of_range_reliable_acknowledge_is_ignored() {
        let now = Instant::now();
        let mut sv = Server::new(cfg(), now);
        let mut nc = connected(&mut sv, addr(5), now);
        let huff = Huffman::new();
        let mut w = MsgWriter::new(&huff);
        w.write_bits(CLC_EOF, 2);
        let ops = w.into_ops();

        // serverId 0 asks for a gamestate; only the ack makes this inadmissible.
        let pkt = nc.build_out(0, 0, i32::MIN, &ops, &huff).unwrap();
        sv.handle_packet(addr(5), &pkt, now);
        assert!(sv.take_outgoing().is_empty(), "bogus ack got a reply");

        let pkt = nc.build_out(0, 0, 1, &ops, &huff).unwrap();
        sv.handle_packet(addr(5), &pkt, now);
        assert!(sv.take_outgoing().is_empty(), "ack ahead of us got a reply");

        // messageAcknowledge names a message we sent; we have sent none.
        let pkt = nc.build_out(0, 1, 0, &ops, &huff).unwrap();
        sv.handle_packet(addr(5), &pkt, now);
        assert!(
            sv.take_outgoing().is_empty(),
            "messageAcknowledge ahead of us got a reply"
        );

        let pkt = nc.build_out(0, 0, 0, &ops, &huff).unwrap();
        sv.handle_packet(addr(5), &pkt, now);
        assert!(
            !sv.take_outgoing().is_empty(),
            "no gamestate for a good ack"
        );
        assert_eq!(sv.client_count(), 1);
    }

    #[test]
    fn a_gap_in_the_client_command_sequence_drops_the_client() {
        let now = Instant::now();
        let mut sv = Server::new(cfg(), now);
        let mut nc = connected(&mut sv, addr(5), now);
        let huff = Huffman::new();

        let mut w = MsgWriter::new(&huff);
        w.write_bits(CLC_CLIENT_COMMAND, 2);
        w.write_long(2); // skips sequence 1, which we never sent
        w.write_string("say hi");
        w.write_bits(CLC_EOF, 2);
        let pkt = nc
            .build_out(i32::from(sv.server_id), 0, 0, &w.into_ops(), &huff)
            .unwrap();
        sv.handle_packet(addr(5), &pkt, now);

        assert_eq!(sv.client_count(), 0);
        let mut out = sv.take_outgoing();
        assert_eq!(out.len(), 1, "expected the drop notice");
        let (to, pkt) = out.remove(0);
        assert_eq!(to, addr(5));
        assert_eq!(
            server_commands(&mut nc, &pkt, &huff),
            vec!["w \"EXE_LOSTRELIABLECOMMANDS\"".to_string()]
        );
    }

    /// Score requests pipelined faster than the client acks their replies
    /// would wrap the reliable ring and overwrite unsacked slots, corrupting
    /// the wire and the scramble key. Past 64 unacked commands the client is
    /// dropped instead, and the slot works for a fresh connect. The whole
    /// burst rides one message: a later message would fail the incoming
    /// ack-range check before any op ran.
    #[test]
    fn score_spam_past_the_reliable_ring_drops_the_client() {
        let now = Instant::now();
        let huff = Huffman::new();
        let scores = |sv: &mut Server, nc: &mut Netchan, first: i32, last: i32| {
            let mut w = MsgWriter::new(&huff);
            for seq in first..=last {
                w.write_bits(CLC_CLIENT_COMMAND, 2);
                w.write_long(seq);
                w.write_string("score");
            }
            w.write_bits(CLC_EOF, 2);
            sv.handle_packet(
                addr(5),
                &nc.build_out(
                    i32::from(sv.server_id),
                    nc.incoming_sequence as i32,
                    0,
                    &w.into_ops(),
                    &huff,
                )
                .unwrap(),
                now,
            );
        };

        // Exactly the ring's worth of unacked commands is not yet fatal.
        let mut sv = Server::new(cfg(), now);
        let mut nc = active(&mut sv, now);
        sv.take_outgoing();
        // Each reply is scrambled with the last client command string, so the
        // receiving netchan needs its own sent-command ring filled (reply 1
        // went out before any command was stored and stays undecodable here).
        for s in 2..=70 {
            nc.reliable[(s as usize) & 63] = "score".to_string();
        }
        scores(&mut sv, &mut nc, 1, 64);
        assert_eq!(sv.client_count(), 1, "a full ring is not yet fatal");
        assert!(
            !sv.take_outgoing().is_empty(),
            "the ring-full boundary must still answer"
        );

        // One pipelined request too many drops the client, notice last.
        let mut sv = Server::new(cfg(), now);
        let mut nc = active(&mut sv, now);
        sv.take_outgoing();
        for s in 2..=70 {
            nc.reliable[(s as usize) & 63] = "score".to_string();
        }
        scores(&mut sv, &mut nc, 1, 65);
        assert_eq!(sv.client_count(), 0);
        let mut notices = Vec::new();
        for (_, pkt) in sv.take_outgoing() {
            let res = nc.process_in(&pkt, &huff);
            if let Ok(Some(msg)) = res {
                let mut r = MsgReader::new(&msg[4..], &huff);
                while !r.is_overflowed() {
                    match r.read_byte() {
                        msg::SVC_SERVER_COMMAND => {
                            r.read_long();
                            notices.push(r.read_big_string());
                        }
                        _ => break,
                    }
                }
            }
        }
        assert_eq!(
            notices.last().map(String::as_str),
            Some("w \"EXE_LOSTRELIABLECOMMANDS\"")
        );

        // The freed slot takes a fresh client.
        let t2 = now + RECONNECT_LIMIT + Duration::from_millis(100);
        connected(&mut sv, addr(5), t2);
        assert_eq!(sv.client_count(), 1);
    }

    #[test]
    fn a_score_request_gets_a_deathmatch_scoreboard() {
        let now = Instant::now();
        let mut sv = Server::new(cfg(), now);
        let mut nc = active(&mut sv, now);
        sv.take_outgoing();

        let huff = Huffman::new();
        let mut w = MsgWriter::new(&huff);
        w.write_bits(CLC_CLIENT_COMMAND, 2);
        w.write_long(1);
        w.write_string("score");
        w.write_bits(CLC_EOF, 2);
        let pkt = nc
            .build_out(
                i32::from(sv.server_id),
                nc.incoming_sequence as i32,
                0,
                &w.into_ops(),
                &huff,
            )
            .unwrap();
        sv.handle_packet(addr(5), &pkt, now);

        let out = sv.take_outgoing();
        assert_eq!(out.len(), 1, "expected one reply frame");
        assert_eq!(out[0].0, addr(5));
        let cmds = server_commands(&mut nc, &out[0].1, &huff);
        assert_eq!(cmds.len(), 1);
        assert!(
            cmds[0].starts_with("b 1 -9999 -9999 0 0 0 0 0"),
            "{:?}",
            cmds[0]
        );
    }

    fn count_replies(sv: &mut Server, to: SocketAddr) -> usize {
        sv.take_outgoing().iter().filter(|(a, _)| *a == to).count()
    }

    #[test]
    fn getstatus_is_rate_limited_per_address() {
        let now = Instant::now();
        let mut sv = Server::new(cfg(), now);
        let a = SocketAddr::from(([10, 0, 0, 1], 5));
        let b = SocketAddr::from(([10, 0, 0, 2], 5));
        for _ in 0..ADDR_BURST + 1 {
            sv.handle_packet(a, &oob("getstatus x"), now);
        }
        assert_eq!(count_replies(&mut sv, a), ADDR_BURST as usize);
        // One global period on, so the burst above is not what limits b.
        let t1 = now + GLOBAL_PERIOD;
        sv.handle_packet(b, &oob("getstatus x"), t1);
        assert_eq!(count_replies(&mut sv, b), 1);
        // The same bucket covers getinfo; one token comes back per period.
        sv.handle_packet(a, &oob("getinfo x"), t1);
        assert_eq!(count_replies(&mut sv, a), 0);
        let t2 = now + ADDR_PERIOD;
        sv.handle_packet(a, &oob("getinfo x"), t2);
        assert_eq!(count_replies(&mut sv, a), 1);
        sv.handle_packet(a, &oob("getinfo x"), t2);
        assert_eq!(count_replies(&mut sv, a), 0);
    }

    #[test]
    fn connectionless_replies_share_a_global_bucket() {
        let now = Instant::now();
        let mut sv = Server::new(cfg(), now);
        let mut answered = 0;
        for i in 0..GLOBAL_BURST + 5 {
            let from = SocketAddr::from(([10, 1, (i >> 8) as u8, i as u8], 5));
            sv.handle_packet(from, &oob("getchallenge"), now);
            answered += count_replies(&mut sv, from);
        }
        assert_eq!(answered, GLOBAL_BURST as usize);
        let late = SocketAddr::from(([10, 2, 0, 1], 5));
        sv.handle_packet(late, &oob("getchallenge"), now + GLOBAL_PERIOD);
        assert_eq!(count_replies(&mut sv, late), 1);
    }

    #[test]
    fn the_address_table_is_bounded() {
        let now = Instant::now();
        let mut sv = Server::new(cfg(), now);
        for i in 0..MAX_BUCKETS as u32 + 8 {
            let from = SocketAddr::from(([10, (i >> 16) as u8, (i >> 8) as u8, i as u8], 5));
            // Spread over time so the global bucket stays open and the
            // eviction order is defined.
            sv.handle_packet(from, &oob("getinfo x"), now + GLOBAL_PERIOD * i);
        }
        assert_eq!(sv.limiter.addrs.len(), MAX_BUCKETS);
        // The least recently seen address went first.
        assert!(!sv
            .limiter
            .addrs
            .contains_key(&std::net::IpAddr::from([10, 0, 0, 0])));
    }

    #[test]
    fn oob_disconnect_honours_the_address_limit() {
        let now = Instant::now();
        let mut sv = Server::new(cfg(), now);
        connected(&mut sv, addr(5), now);
        // The challenge took one token; spend the rest on getinfo.
        for _ in 1..ADDR_BURST {
            sv.handle_packet(addr(5), &oob("getinfo x"), now);
        }
        sv.take_outgoing();
        sv.handle_packet(addr(5), &oob("disconnect"), now);
        assert_eq!(
            sv.client_count(),
            1,
            "a rate-limited disconnect went through"
        );
        sv.handle_packet(addr(5), &oob("disconnect"), now + ADDR_PERIOD);
        assert_eq!(sv.client_count(), 0);
    }

    /// A forged packet carrying the victim's ip and qport must not advance the
    /// netchan, move the address or refresh the timeout; the real client's
    /// next packet still goes through.
    #[test]
    fn a_spoofed_packet_leaves_the_client_untouched() {
        let now = Instant::now();
        let mut sv = Server::new(cfg(), now);
        let mut nc = connected(&mut sv, addr(5), now);
        let huff = Huffman::new();
        let mut eof = MsgWriter::new(&huff);
        eof.write_bits(CLC_EOF, 2);
        let eof = eof.into_ops();
        let later = now + Duration::from_secs(5);
        let spoofer = addr(6);
        let mut forged = Netchan::new(QPORT, nc.challenge);
        let snapshot = |sv: &Server| {
            let c = sv.clients[0].as_ref().unwrap();
            (c.netchan.incoming_sequence, c.addr, c.last_packet)
        };
        let before = snapshot(&sv);

        // Header checks: an ack we never sent.
        forged.outgoing_sequence = 0x7fff_fffe;
        let pkt = forged.build_out(0, 0, 1, &eof, &huff).unwrap();
        sv.handle_packet(spoofer, &pkt, later);
        assert_eq!(snapshot(&sv), before, "a bad ack committed state");

        // A stale serverId with nothing to resend.
        forged.outgoing_sequence = 0x7fff_fffe;
        let pkt = forged
            .build_out(i32::from(sv.server_id) ^ 0x01, 0, 0, &eof, &huff)
            .unwrap();
        sv.handle_packet(spoofer, &pkt, later);
        assert_eq!(snapshot(&sv), before, "a stale serverId committed state");

        // Right header, ops that end inside a command.
        forged.outgoing_sequence = 0x7fff_fffe;
        let mut w = MsgWriter::new(&huff);
        w.write_bits(CLC_CLIENT_COMMAND, 2);
        w.write_long(1);
        let pkt = forged
            .build_out(i32::from(sv.server_id), 0, 0, &w.into_ops(), &huff)
            .unwrap();
        sv.handle_packet(spoofer, &pkt, later);
        assert_eq!(
            snapshot(&sv),
            before,
            "a truncated op stream committed state"
        );
        assert!(sv.take_outgoing().is_empty());

        // The legitimate client's first message, sequence 1, asks for the gamestate.
        let pkt = nc.build_out(0, 0, 0, &eof, &huff).unwrap();
        sv.handle_packet(addr(5), &pkt, later);
        assert_eq!(snapshot(&sv), (1, addr(5), later));
        assert!(
            !sv.take_outgoing().is_empty(),
            "no gamestate after the spoofs"
        );
    }

    #[test]
    fn a_foreign_challenge_cannot_replace_a_live_client() {
        let now = Instant::now();
        let mut sv = Server::new(cfg(), now);
        let mut nc = connected(&mut sv, addr(5), now);
        let huff = Huffman::new();
        let mut w = MsgWriter::new(&huff);
        w.write_bits(CLC_EOF, 2);
        let eof = w.into_ops();
        let own = nc.challenge;

        // The client is heard from just before the attempt.
        let t1 = now + RECONNECT_LIMIT + Duration::from_millis(100);
        let pkt = nc.build_out(0, 0, 0, &eof, &huff).unwrap();
        sv.handle_packet(addr(5), &pkt, t1);
        sv.take_outgoing();

        // A neighbour behind the same NAT holds a valid challenge for this ip
        // and knows the qport. Past sv_reconnectlimit, retail would hand it the slot.
        let foreign = challenge_for(&mut sv, addr(7), t1);
        let t2 = t1 + Duration::from_millis(100);
        sv.handle_packet(
            addr(7),
            &connect_pkt(foreign, QPORT, PROTOCOL_V1.version),
            t2,
        );
        assert!(sv.take_outgoing().is_empty(), "the takeover got a reply");
        let c = sv.clients[0].as_ref().unwrap();
        assert_eq!((c.addr, c.netchan.challenge), (addr(5), own));

        // The client's own connect retry, same challenge, still resets its slot.
        sv.handle_packet(addr(5), &connect_pkt(own, QPORT, PROTOCOL_V1.version), t2);
        assert_eq!(reply_text(&mut sv).0, "connectResponse");
        assert_eq!(sv.client_count(), 1);

        // Once the slot has been silent for sv_reconnectlimit, a crashed client
        // coming back with a new challenge may reclaim it.
        let t3 = t2 + RECONNECT_LIMIT + Duration::from_millis(100);
        sv.handle_packet(
            addr(7),
            &connect_pkt(foreign, QPORT, PROTOCOL_V1.version),
            t3,
        );
        assert_eq!(reply_text(&mut sv).0, "connectResponse");
        assert_eq!(sv.client_count(), 1);
        let c = sv.clients[0].as_ref().unwrap();
        assert_eq!((c.addr, c.netchan.challenge), (addr(7), foreign));
    }

    #[test]
    fn stale_challenges_expire_when_a_new_one_is_issued() {
        let now = Instant::now();
        let mut sv = Server::new(cfg(), now);
        let old = challenge_for(&mut sv, addr(5), now);
        // Asking again refreshes the entry rather than issuing a new one.
        let t1 = now + CHALLENGE_TTL - Duration::from_secs(1);
        assert_eq!(challenge_for(&mut sv, addr(5), t1), old);
        let t2 = now + CHALLENGE_TTL + Duration::from_secs(1);
        challenge_for(&mut sv, addr(6), t2);
        assert_eq!(sv.challenges.len(), 2, "a refreshed entry was expired");

        let t3 = t1 + CHALLENGE_TTL + Duration::from_secs(1);
        challenge_for(&mut sv, addr(8), t3);
        assert_eq!(sv.challenges.len(), 2);
        assert!(sv.challenges.iter().all(|c| c.addr != addr(5)));
        sv.handle_packet(addr(5), &connect_pkt(old, QPORT, PROTOCOL_V1.version), t3);
        assert_eq!(
            reply_text(&mut sv),
            ("error".to_string(), "EXE_BAD_CHALLENGE".to_string())
        );
    }

    /// clc_move ops carrying one usercmd, keyed the way the server decodes it.
    fn move_ops(checksum_feed: i32, message_ack: i32, cmd: UserCmd) -> Vec<u8> {
        move_ops_cmds(checksum_feed, message_ack, &[cmd])
    }

    /// clc_move ops carrying several usercmds, chained as a real client
    /// chains them: each deltas from its predecessor.
    fn move_ops_cmds(checksum_feed: i32, message_ack: i32, cmds: &[UserCmd]) -> Vec<u8> {
        let huff = Huffman::new();
        let key = checksum_feed ^ message_ack ^ com_hash_key("", 32);
        let mut w = MsgWriter::new(&huff);
        w.write_bits(CLC_MOVE, 2);
        w.write_byte(cmds.len() as u8);
        let mut prev = NULL_USERCMD;
        for cmd in cmds {
            write_delta_usercmd(&mut w, key, &prev, cmd);
            prev = *cmd;
        }
        w.write_bits(CLC_EOF, 2);
        w.into_ops()
    }

    /// Move-message encoder that keeps the delta base across messages, the
    /// way the real client chains its sent cmds. A fresh null base per
    /// message would let a changed-then-released field encode compact
    /// against nothing.
    struct MoveChain {
        checksum_feed: i32,
        prev: UserCmd,
    }

    impl MoveChain {
        fn new(checksum_feed: i32) -> Self {
            MoveChain {
                checksum_feed,
                prev: NULL_USERCMD,
            }
        }

        fn ops(&mut self, message_ack: i32, cmds: &[UserCmd]) -> Vec<u8> {
            let huff = Huffman::new();
            let key = self.checksum_feed ^ message_ack ^ com_hash_key("", 32);
            let mut w = MsgWriter::new(&huff);
            w.write_bits(CLC_MOVE, 2);
            w.write_byte(cmds.len() as u8);
            for cmd in cmds {
                write_delta_usercmd(&mut w, key, &self.prev, cmd);
                self.prev = *cmd;
            }
            w.write_bits(CLC_EOF, 2);
            w.into_ops()
        }
    }
    /// clc_move carrying one cmd whose pitch and yaw ride change bit 0
    /// (omitted, unchanged from the server's stored cmd), what a retail client
    /// sends when the mouse did not move this packet. Forward/right stay
    /// announced; the serverTime is absolute so the test isolates the angle
    /// base from the serverTime base.
    fn move_ops_omit_angles(checksum_feed: i32, message_ack: i32, st: i32) -> Vec<u8> {
        let huff = Huffman::new();
        let key = checksum_feed ^ message_ack ^ com_hash_key("", 32);
        let mut w = MsgWriter::new(&huff);
        w.write_bits(CLC_MOVE, 2);
        w.write_byte(1);
        w.write_bits(0, 1); // serverTime: 32-bit absolute
        w.write_long(st);
        // Not the whole-cmd shortcut, and the branch bit picks the compact one.
        w.write_bits((key & 1) ^ 1, 1);
        w.write_bits(key & 1, 1);
        let key = key ^ st;
        w.write_bits(key & 1, 1); // buttons bit 0, announced as 0
        w.write_bits(0, 1); // pitch omitted
        w.write_bits(0, 1); // yaw omitted
        w.write_bits(1, 1); // forward/right announced
        w.write_bits(1 ^ (key & 0xf), 4); // forward 127: bucket 1
        w.write_bits(CLC_EOF, 2);
        w.into_ops()
    }

    /// clc_move whose count byte is past the usercmd cap: the header decodes,
    /// the move block cannot.
    fn garbled_move_ops() -> Vec<u8> {
        let huff = Huffman::new();
        let mut w = MsgWriter::new(&huff);
        w.write_bits(CLC_MOVE, 2);
        w.write_byte(u8::MAX);
        w.write_bits(CLC_EOF, 2);
        w.into_ops()
    }

    /// Ack-only ops: an empty command/move run still ends in clc_EOF, which is
    /// what `parse_client_ops` demands before it commits anything.
    fn ack_ops() -> Vec<u8> {
        let huff = Huffman::new();
        let mut w = MsgWriter::new(&huff);
        w.write_bits(CLC_EOF, 2);
        w.into_ops()
    }

    /// Connect, ask for the gamestate (a fresh client sends serverId 0),
    /// drain it, ack past it. Returns the live client netchan; the server now
    /// considers slot 0 Active.
    fn active(sv: &mut Server, now: Instant) -> Netchan {
        let huff = Huffman::new();
        let mut nc = connected(sv, addr(5), now);
        let pkt = nc.build_out(0, 0, 0, &ack_ops(), &huff).unwrap();
        sv.handle_packet(addr(5), &pkt, now);
        for (_, pkt) in sv.take_outgoing() {
            let _ = nc.process_in(&pkt, &huff).unwrap();
        }
        let ack = nc.incoming_sequence as i32;
        sv.handle_packet(
            addr(5),
            &nc.build_out(0x10, ack, 0, &ack_ops(), &huff).unwrap(),
            now,
        );
        nc
    }

    /// Tick once and parse the newest queued frame as a client would.
    fn latest_snapshot(
        sv: &mut Server,
        nc: &mut Netchan,
        ring: &mut SnapshotRing,
        now: Instant,
    ) -> vcod_common::net::snapshot::Snapshot {
        let huff = Huffman::new();
        let p = &PROTOCOL_V1;
        sv.tick(now);
        let mut snap = None;
        for (_, pkt) in sv.take_outgoing() {
            if let Ok(Some(msgbytes)) = nc.process_in(&pkt, &huff) {
                let num = nc.incoming_sequence;
                let mut r = MsgReader::new(&msgbytes[4..], &huff);
                while !r.is_overflowed() {
                    if r.read_byte() == SVC_SNAPSHOT {
                        snap = Some(ring.parse_into(&mut r, p, num).unwrap().clone());
                        break;
                    }
                }
            }
        }
        snap.expect("a snapshot arrived")
    }

    /// `ps.commandTime` must name a usercmd we actually simulated, never a
    /// later moment. A client replays every cmd past commandTime, so claiming
    /// to have simulated further than we have silently drops that slice of
    /// its input on every frame and its prediction judders (retail trace,
    /// 2026-08-28: commandTime ran 11-24 ms ahead on 100% of frames).
    #[test]
    fn command_time_never_runs_ahead_of_the_last_simulated_cmd() {
        let now = Instant::now();
        let mut sv = Server::new(cfg(), now);
        sv.load_world(World {
            collision: test_world(&[]),
            spawn: ([0.0, 0.0, 64.0], 0.0),
        });
        let mut nc = active(&mut sv, now);
        let mut ring = SnapshotRing::new();
        let p = &PROTOCOL_V1;

        // A client whose cmd clock trails the server's, as a real one's does:
        // it runs its own serverTime behind so it has frames to interpolate.
        let mut at = now;
        let mut cmd_time = 0i32;
        for _ in 0..8 {
            at += std::time::Duration::from_millis(50);
            cmd_time += 40; // 10 ms per frame behind the server's 50
            let ack = nc.incoming_sequence as i32;
            sv.handle_packet(
                addr(5),
                &nc.build_out(
                    0x10,
                    ack,
                    0,
                    &move_ops(
                        sv.checksum_feed,
                        ack,
                        UserCmd {
                            server_time: cmd_time,
                            forward: 127,
                            ..Default::default()
                        },
                    ),
                    &Huffman::new(),
                )
                .unwrap(),
                at,
            );
            let s = latest_snapshot(&mut sv, &mut nc, &mut ring, at);
            let ct = s.ps.field_i32(p, "commandTime");
            assert!(
                ct <= cmd_time,
                "commandTime {ct} is ahead of the newest cmd we simulated ({cmd_time})"
            );
        }
    }

    #[test]
    fn an_acked_client_receives_snapshots_and_flies_forward() {
        let now = Instant::now();
        let mut sv = Server::new(cfg(), now);
        // A flat floor to fly over; without a world the sim freezes.
        sv.load_world(World {
            collision: test_world(&[]),
            spawn: ([0.0, 0.0, 64.0], 0.0),
        });
        let mut nc = active(&mut sv, now);

        let mut ring = SnapshotRing::new();
        let s = latest_snapshot(&mut sv, &mut nc, &mut ring, now);
        assert!(s.valid && s.delta_num == -1);
        let p = &PROTOCOL_V1;
        assert_eq!(s.ps.field_i32(p, "pm_type"), 4);
        assert_eq!(s.clients[&0].name(p), "vcod");

        // Forward nudge; yaw 0 faces +X.
        let later = now + std::time::Duration::from_millis(50);
        let ack = nc.incoming_sequence as i32;
        sv.handle_packet(
            addr(5),
            &nc.build_out(
                0x10,
                ack,
                0,
                &move_ops(
                    sv.checksum_feed,
                    ack,
                    UserCmd {
                        forward: 127,
                        ..Default::default()
                    },
                ),
                &Huffman::new(),
            )
            .unwrap(),
            now,
        );
        let s2 = latest_snapshot(&mut sv, &mut nc, &mut ring, later);
        let moved = s2.ps.origin(p)[0] - s.ps.origin(p)[0];
        assert!(moved > 1.0, "should have flown +X, dx {moved}");
    }

    /// A block whose mouse turns mid-flight: per-cmd replay must cover both
    /// headings. The old last-cmd-only step integrated once at yaw 90 and
    /// never moved along +X.
    #[test]
    fn turning_cmds_integrate_per_cmd() {
        let now = Instant::now();
        let mut sv = Server::new(cfg(), now);
        sv.load_world(World {
            collision: test_world(&[]),
            spawn: ([0.0, 0.0, 64.0], 0.0),
        });
        let mut nc = active(&mut sv, now);
        let mut ring = SnapshotRing::new();
        let t0 = now;
        let t1 = now + std::time::Duration::from_millis(50);
        let s = latest_snapshot(&mut sv, &mut nc, &mut ring, t0);

        // Forward at yaw 0 (faces +X), then the mouse swings to yaw 90 (+Y).
        let ack = nc.incoming_sequence as i32;
        let cmds = [
            UserCmd {
                server_time: 0,
                forward: 127,
                angles: [0, 0, 0],
                ..Default::default()
            },
            UserCmd {
                server_time: 50,
                forward: 127,
                angles: [0, 16384, 0],
                ..Default::default()
            },
        ];
        sv.handle_packet(
            addr(5),
            &nc.build_out(
                0x10,
                ack,
                0,
                &move_ops_cmds(sv.checksum_feed, ack, &cmds),
                &Huffman::new(),
            )
            .unwrap(),
            now,
        );
        let s2 = latest_snapshot(&mut sv, &mut nc, &mut ring, t1);
        let o1 = s.ps.origin(&PROTOCOL_V1);
        let o2 = s2.ps.origin(&PROTOCOL_V1);
        assert!(
            o2[0] - o1[0] > 2.0,
            "cmd A should have flown +X, dx {}",
            o2[0] - o1[0]
        );
        assert!(
            o2[1] - o1[1] > 2.0,
            "cmd B should have flown +Y, dy {}",
            o2[1] - o1[1]
        );
    }

    /// A cmd from before the processed clock is stale: skipped whole, no
    /// motion, no panic.
    #[test]
    fn stale_cmds_are_skipped() {
        let now = Instant::now();
        let mut sv = Server::new(cfg(), now);
        sv.load_world(World {
            collision: test_world(&[]),
            spawn: ([0.0, 0.0, 64.0], 0.0),
        });
        let mut nc = active(&mut sv, now);
        let mut ring = SnapshotRing::new();
        let mut chain = MoveChain::new(sv.checksum_feed);
        let mut tick_at = now + std::time::Duration::from_millis(50);

        let ack = nc.incoming_sequence as i32;
        sv.handle_packet(
            addr(5),
            &nc.build_out(
                0x10,
                ack,
                0,
                &chain.ops(
                    ack,
                    &[UserCmd {
                        server_time: 500,
                        forward: 127,
                        ..Default::default()
                    }],
                ),
                &Huffman::new(),
            )
            .unwrap(),
            now,
        );
        let s1 = latest_snapshot(&mut sv, &mut nc, &mut ring, tick_at);
        assert!(
            s1.ps.origin(&PROTOCOL_V1)[0] > 1.0,
            "the fresh cmd must move the sim first, at {}",
            s1.ps.origin(&PROTOCOL_V1)[0]
        );

        // A reordered/duplicated cmd from before serverTime 500.
        let ack = nc.incoming_sequence as i32;
        sv.handle_packet(
            addr(5),
            &nc.build_out(
                0x10,
                ack,
                0,
                &chain.ops(
                    ack,
                    &[UserCmd {
                        server_time: 0,
                        forward: 127,
                        ..Default::default()
                    }],
                ),
                &Huffman::new(),
            )
            .unwrap(),
            now,
        );
        tick_at += std::time::Duration::from_millis(50);
        let s2 = latest_snapshot(&mut sv, &mut nc, &mut ring, tick_at);
        assert_eq!(
            s2.ps.origin(&PROTOCOL_V1),
            s1.ps.origin(&PROTOCOL_V1),
            "a stale cmd must not move the sim"
        );
    }

    /// More cmds than a tick may replay arrive back to back; the queue stays
    /// bounded and snapshotting carries on.
    #[test]
    fn flood_is_bounded() {
        let now = Instant::now();
        let mut sv = Server::new(cfg(), now);
        sv.load_world(World {
            collision: test_world(&[]),
            spawn: ([0.0, 0.0, 64.0], 0.0),
        });
        let mut nc = active(&mut sv, now);
        let mut ring = SnapshotRing::new();
        let mut chain = MoveChain::new(sv.checksum_feed);
        let _ = latest_snapshot(&mut sv, &mut nc, &mut ring, now);

        let cmds: Vec<UserCmd> = (0..100)
            .map(|i| UserCmd {
                server_time: i * 20,
                forward: 127,
                ..Default::default()
            })
            .collect();
        for chunk in cmds.chunks(MAX_PACKET_USERCMDS as usize) {
            let ack = nc.incoming_sequence as i32;
            sv.handle_packet(
                addr(5),
                &nc.build_out(0x10, ack, 0, &chain.ops(ack, chunk), &Huffman::new())
                    .unwrap(),
                now,
            );
        }
        let s = latest_snapshot(
            &mut sv,
            &mut nc,
            &mut ring,
            now + std::time::Duration::from_millis(50),
        );
        assert!(s.valid);
        assert!(
            sv.clients[0].as_ref().unwrap().pending.len() <= 2,
            "flood left {} cmds queued",
            sv.clients[0].as_ref().unwrap().pending.len()
        );
        let s2 = latest_snapshot(
            &mut sv,
            &mut nc,
            &mut ring,
            now + std::time::Duration::from_millis(100),
        );
        assert!(s2.valid, "snapshots must continue");
    }

    /// A retail client omits unchanged angle fields (change bit 0) instead of
    /// announcing them, so the server must decode them against the cmd it last
    /// stored for this client. Against a null base the view flashes to 0.
    #[test]
    fn omitted_angles_decode_against_the_stored_last_cmd() {
        let now = Instant::now();
        let mut sv = Server::new(cfg(), now);
        sv.load_world(World {
            collision: test_world(&[]),
            spawn: ([0.0, 0.0, 64.0], 0.0),
        });
        let mut nc = active(&mut sv, now);
        let mut ring = SnapshotRing::new();
        let _ = latest_snapshot(&mut sv, &mut nc, &mut ring, now);

        // Turn to yaw -45, announced like vcod's own client always does.
        let yaw = ((-45.0f32) * 65536.0 / 360.0) as i32 & 0xffff;
        let ack = nc.incoming_sequence as i32;
        sv.handle_packet(
            addr(5),
            &nc.build_out(
                0x10,
                ack,
                0,
                &move_ops(
                    sv.checksum_feed,
                    ack,
                    UserCmd {
                        server_time: 500,
                        forward: 127,
                        angles: [0, yaw, 0],
                        ..Default::default()
                    },
                ),
                &Huffman::new(),
            )
            .unwrap(),
            now,
        );
        let t1 = now + std::time::Duration::from_millis(50);
        let s1 = latest_snapshot(&mut sv, &mut nc, &mut ring, t1);
        assert!((s1.ps.viewangles(&PROTOCOL_V1)[1] + 45.0).abs() < 0.01);

        // Next packet: the mouse did not move, so pitch and yaw ride change
        // bit 0 off the cmd the server just stored.
        let ack = nc.incoming_sequence as i32;
        sv.handle_packet(
            addr(5),
            &nc.build_out(
                0x10,
                ack,
                0,
                &move_ops_omit_angles(sv.checksum_feed, ack, 520),
                &Huffman::new(),
            )
            .unwrap(),
            t1,
        );
        let s2 = latest_snapshot(
            &mut sv,
            &mut nc,
            &mut ring,
            t1 + std::time::Duration::from_millis(50),
        );
        assert!(
            (s2.ps.viewangles(&PROTOCOL_V1)[1] + 45.0).abs() < 0.01,
            "omitted angles must keep the stored yaw, got {}",
            s2.ps.viewangles(&PROTOCOL_V1)[1]
        );
        // Guard against a vacuous pass: the second cmd carries forward 127,
        // so the sim must have moved between the snapshots.
        let d = s2.ps.origin(&PROTOCOL_V1)[0] - s1.ps.origin(&PROTOCOL_V1)[0];
        assert!(d > 0.5, "the omitted-angle cmd was not applied, dx {d}");
    }

    /// Press then release, end to end: the release must decode up=0 against
    /// the server's stored base, or the sim replays the jump forever.
    #[test]
    fn released_upmove_stops_the_climb() {
        let now = Instant::now();
        let mut sv = Server::new(cfg(), now);
        sv.load_world(World {
            collision: test_world(&[]),
            spawn: ([0.0, 0.0, 64.0], 0.0),
        });
        let mut nc = active(&mut sv, now);
        let mut ring = SnapshotRing::new();
        let mut chain = MoveChain::new(sv.checksum_feed);
        let p = &PROTOCOL_V1;

        let ack = nc.incoming_sequence as i32;
        sv.handle_packet(
            addr(5),
            &nc.build_out(
                0x10,
                ack,
                0,
                &chain.ops(
                    ack,
                    &[UserCmd {
                        server_time: 500,
                        up: 127,
                        ..Default::default()
                    }],
                ),
                &Huffman::new(),
            )
            .unwrap(),
            now,
        );
        let t1 = now + std::time::Duration::from_millis(50);
        let s1 = latest_snapshot(&mut sv, &mut nc, &mut ring, t1);
        let vz = s1.ps.field_f32(p, "velocity[2]");
        assert!(vz > 100.0, "holding up must climb, vz {vz}");

        // A burst of release frames: friction alone has to bring the climb
        // back to rest. With spectator friction 5.0 the decay is geometric
        // (scale ~0.75/tick at the stopspeed boundary); 32 frames (the wire
        // per-message cap) take a 237 u/s climb to ~0.02.
        let releases: Vec<UserCmd> = (0..32)
            .map(|i| UserCmd {
                server_time: 550 + i * 50,
                ..Default::default()
            })
            .collect();
        let ack = nc.incoming_sequence as i32;
        sv.handle_packet(
            addr(5),
            &nc.build_out(0x10, ack, 0, &chain.ops(ack, &releases), &Huffman::new())
                .unwrap(),
            t1,
        );
        let s2 = latest_snapshot(
            &mut sv,
            &mut nc,
            &mut ring,
            t1 + std::time::Duration::from_millis(50),
        );
        let vz = s2.ps.field_f32(p, "velocity[2]");
        // Q3's PM_Friction zeroes only xy below speed 1, leaving a ~0.18 u/s
        // vertical floor (retail-faithful); anything under half a unit is rest.
        assert!(
            vz.abs() < 0.5,
            "released upmove must decay to rest, vz {vz}"
        );
    }

    /// A move message that fails to decode is discarded whole: the next good
    /// one still chains from the last successfully decoded cmd.
    #[test]
    fn a_garbled_message_does_not_poison_the_base() {
        let now = Instant::now();
        let mut sv = Server::new(cfg(), now);
        sv.load_world(World {
            collision: test_world(&[]),
            spawn: ([0.0, 0.0, 64.0], 0.0),
        });
        let mut nc = active(&mut sv, now);
        let mut ring = SnapshotRing::new();
        let _ = latest_snapshot(&mut sv, &mut nc, &mut ring, now);

        let yaw = ((-45.0f32) * 65536.0 / 360.0) as i32 & 0xffff;
        let ack = nc.incoming_sequence as i32;
        sv.handle_packet(
            addr(5),
            &nc.build_out(
                0x10,
                ack,
                0,
                &move_ops(
                    sv.checksum_feed,
                    ack,
                    UserCmd {
                        server_time: 500,
                        forward: 127,
                        angles: [0, yaw, 0],
                        ..Default::default()
                    },
                ),
                &Huffman::new(),
            )
            .unwrap(),
            now,
        );
        let t1 = now + std::time::Duration::from_millis(50);
        let s1 = latest_snapshot(&mut sv, &mut nc, &mut ring, t1);
        assert!((s1.ps.viewangles(&PROTOCOL_V1)[1] + 45.0).abs() < 0.01);

        // Garbage that cannot parse, then a good packet whose angles are
        // omitted: they must come off the pre-failure base.
        let ack = nc.incoming_sequence as i32;
        sv.handle_packet(
            addr(5),
            &nc.build_out(0x10, ack, 0, &garbled_move_ops(), &Huffman::new())
                .unwrap(),
            t1,
        );
        let ack = nc.incoming_sequence as i32;
        sv.handle_packet(
            addr(5),
            &nc.build_out(
                0x10,
                ack,
                0,
                &move_ops_omit_angles(sv.checksum_feed, ack, 520),
                &Huffman::new(),
            )
            .unwrap(),
            t1,
        );
        let s2 = latest_snapshot(
            &mut sv,
            &mut nc,
            &mut ring,
            t1 + std::time::Duration::from_millis(50),
        );
        assert!(s2.valid, "snapshots must continue");
        assert!(
            (s2.ps.viewangles(&PROTOCOL_V1)[1] + 45.0).abs() < 0.01,
            "the garbled packet must not reset the base, got {}",
            s2.ps.viewangles(&PROTOCOL_V1)[1]
        );
    }

    #[test]
    fn an_unacked_client_gets_no_snapshots() {
        let now = Instant::now();
        let mut sv = Server::new(cfg(), now);
        let _nc = connected(&mut sv, addr(5), now);
        sv.take_outgoing(); // drop the gamestate frames
        sv.tick(now);
        assert!(
            sv.take_outgoing().is_empty(),
            "nothing before the gamestate is acked"
        );
    }
}
