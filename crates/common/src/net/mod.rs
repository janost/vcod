//! CoD 1.1 network client, headless (no wgpu/winit types). Wire reference:
//! docs/protocol-1.1.md.

pub mod connectionless;
pub use connectionless::info_value_for_key;
pub mod download;
pub mod events;
pub mod fields_v1;
pub mod gamestate;
pub mod huffman;
pub mod msg;
pub mod netchan;
pub mod protocol;
pub use protocol::{FogParams, CS_FOG_V1};
pub mod snapshot;
pub mod trajectory;

use gamestate::Gamestate;
use huffman::Huffman;
use msg::{
    write_delta_usercmd, MsgReader, MsgWriter, UserCmd, NULL_USERCMD, SVC_DOWNLOAD, SVC_EOF,
    SVC_GAMESTATE, SVC_NOP, SVC_SERVER_COMMAND, SVC_SNAPSHOT,
};
use netchan::Netchan;
use protocol::{Protocol, PROTOCOL_V1};
use snapshot::SnapshotRing;
use std::net::UdpSocket;
use std::time::{Duration, Instant};

/// `MAX_DOWNLOAD_BLKSIZE`, 8192 on CoD 1.1, not RTCW's 2048 (docs/protocol-1.1.md,
/// svc_download).
const MAX_DOWNLOAD_BLKSIZE: usize = 8192;

/// Largest pk3 a server may announce; the biggest stock pak is under 300 MiB.
const MAX_DOWNLOAD_SIZE: u32 = 512 << 20;

/// Packets read per `pump`, matching the server's `MAX_PACKETS_PER_FRAME`, so a
/// flood cannot stall the caller's frame.
const MAX_PACKETS_PER_PUMP: usize = 256;

/// `clc_ops_e`, 2 bits on the wire (cod_lnxded 0x8087454).
const CLC_MOVE: i32 = 0;
const CLC_MOVE_NO_DELTA: i32 = 1;
const CLC_CLIENT_COMMAND: i32 = 2;
const CLC_EOF: i32 = 3;

const CONNECT_RESEND: Duration = Duration::from_secs(2);
const CONNECT_TRIES: u32 = 5;
const GAMESTATE_POKE: Duration = Duration::from_millis(200);
const GAMESTATE_TIMEOUT: Duration = Duration::from_secs(20);
/// Snapshot silence that drops an active connection.
const ACTIVE_TIMEOUT: Duration = Duration::from_secs(30);

/// Socket abstraction so the state machine runs in tests without UDP.
/// `try_recv` is non-blocking.
pub trait Transport {
    fn try_recv(&mut self, buf: &mut [u8]) -> Option<usize>;
    fn send(&mut self, data: &[u8]);
}

pub struct UdpTransport(UdpSocket);

impl UdpTransport {
    pub fn connect(addr: &str) -> anyhow::Result<Self> {
        let sock = UdpSocket::bind("0.0.0.0:0")?;
        sock.connect(addr)?;
        sock.set_nonblocking(true)?;
        Ok(UdpTransport(sock))
    }
}

impl Transport for UdpTransport {
    fn try_recv(&mut self, buf: &mut [u8]) -> Option<usize> {
        self.0.recv(buf).ok()
    }
    fn send(&mut self, data: &[u8]) {
        let _ = self.0.send(data);
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NetState {
    Disconnected,
    Challenging,
    Connecting,
    LoadingGamestate,
    Active,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NetEvent {
    GamestateReady,
    /// Colour codes left in; `team` for `tchat`.
    Chat {
        text: String,
        team: bool,
    },
    Print(String),
    Dropped(String),
    /// Changed via the `d` serverCommand.
    ConfigstringChanged(usize),
    /// The pk3 is in place; carries the requested remote path (`main/foo.pk3`).
    DownloadComplete(String),
    /// Any serverCommand not handled here, tokenized. The HUD reads `b` from it.
    ServerCommand(Vec<String>),
}

/// CoD 1.1 spectate client, generic over its [`Transport`] so the state
/// machine is testable without a socket.
pub struct NetClient<T: Transport> {
    transport: T,
    huff: Huffman,
    proto: &'static Protocol,

    state: NetState,
    challenge: i32,
    qport: u16,
    userinfo: String,
    netchan: Netchan,

    gamestate: Option<Gamestate>,
    snapshots: SnapshotRing,
    configstrings: Vec<String>,
    server_id: i32,
    checksum_feed: i32,
    client_num: i32,
    /// Highest serverCommand sequence executed; the `reliableAcknowledge` we send.
    command_sequence: i32,

    /// Server clock estimate and the pump time it was taken.
    server_time: i32,
    server_time_at: Instant,

    /// The last usercmd put on the wire; the delta base for the next move
    /// (retail's outCmd chaining). Reset at every gamestate, matching the
    /// client's own outCmd reset on map entry.
    last_sent_cmd: UserCmd,

    // Timers, all measured against the `now` handed to `pump_at`.
    now: Instant,
    tries: u32,
    last_send: Instant,
    connect_deadline: Instant,
    last_snapshot: Instant,
    /// Last OOB `print` during the handshake; the rejection reason if the
    /// connect stalls.
    handshake_print: Option<String>,

    download: Option<download::Download>,
    /// `stopdl` already sent for an unrequested transfer.
    stopdl_sent: bool,

    events: Vec<NetEvent>,

    /// Raw snapshot capture for parser fixtures; off unless a probe asks.
    capture: Option<SnapshotCapture>,
    captured_gamestate: Option<Vec<u8>>,
}

/// Raw snapshot messages as `[u32 message_num][u32 len][bytes]` triples, the
/// format the snapshot parser tests replay.
#[derive(Default)]
pub struct SnapshotCapture {
    pub triples: Vec<u8>,
    pub count: usize,
    pub times: Vec<i32>,
}

impl NetClient<UdpTransport> {
    pub fn connect(addr: &str) -> anyhow::Result<Self> {
        let transport = UdpTransport::connect(addr)?;
        Ok(Self::start(transport, Instant::now()))
    }
}

impl<T: Transport> NetClient<T> {
    pub fn start(transport: T, now: Instant) -> Self {
        let qport: u16 = 0x2000 | (std::process::id() as u16 & 0x0fff);
        let mut c = NetClient {
            transport,
            huff: Huffman::new(),
            proto: &PROTOCOL_V1,
            state: NetState::Disconnected,
            challenge: 0,
            qport,
            userinfo: String::new(),
            netchan: Netchan::new(qport, 0),
            gamestate: None,
            snapshots: SnapshotRing::new(),
            configstrings: vec![String::new(); PROTOCOL_V1.max_configstrings],
            server_id: 0,
            checksum_feed: 0,
            client_num: 0,
            command_sequence: 0,
            server_time: 0,
            server_time_at: now,
            last_sent_cmd: NULL_USERCMD,
            now,
            tries: 0,
            last_send: now,
            connect_deadline: now + GAMESTATE_TIMEOUT,
            last_snapshot: now,
            handshake_print: None,
            download: None,
            stopdl_sent: false,
            events: Vec::new(),
            capture: None,
            captured_gamestate: None,
        };
        c.send_getchallenge();
        c.state = NetState::Challenging;
        c.tries = 1;
        c.last_send = now;
        c
    }

    pub fn state(&self) -> NetState {
        self.state
    }
    pub fn gamestate(&self) -> Option<&Gamestate> {
        self.gamestate.as_ref()
    }
    pub fn snapshots(&self) -> &SnapshotRing {
        &self.snapshots
    }
    pub fn packet_drops(&self) -> u64 {
        self.netchan.total_dropped
    }
    pub fn snapshot_age(&self) -> std::time::Duration {
        self.last_snapshot.elapsed()
    }
    pub fn configstring(&self, i: usize) -> &str {
        self.configstrings.get(i).map(String::as_str).unwrap_or("")
    }

    pub fn configstrings(&self) -> &[String] {
        &self.configstrings
    }

    pub fn server_id(&self) -> i32 {
        self.server_id
    }

    pub fn capture_count(&self) -> Option<usize> {
        self.capture.as_ref().map(|c| c.count)
    }

    pub fn take_capture(&mut self) -> Option<SnapshotCapture> {
        self.capture.take()
    }

    pub fn captured_gamestate(&self) -> Option<&[u8]> {
        self.captured_gamestate.as_deref()
    }

    pub fn enable_capture(&mut self) {
        self.capture = Some(SnapshotCapture::default());
    }

    /// Drain the socket, advance the state machine, return the events.
    pub fn pump(&mut self) -> Vec<NetEvent> {
        self.pump_at(Instant::now())
    }

    /// [`Self::pump`] with the clock injected.
    pub fn pump_at(&mut self, now: Instant) -> Vec<NetEvent> {
        self.now = now;
        let mut buf = [0u8; 65536];
        for _ in 0..MAX_PACKETS_PER_PUMP {
            let Some(n) = self.transport.try_recv(&mut buf) else {
                break;
            };
            let pkt = buf[..n].to_vec();
            self.handle_packet(&pkt);
            if self.state == NetState::Disconnected {
                break;
            }
        }
        self.tick();
        std::mem::take(&mut self.events)
    }

    /// Send one usercmd; a no-op unless active. Called every frame, which is
    /// also the keepalive.
    pub fn send_frame(&mut self, cmd: &UserCmd) {
        if self.state != NetState::Active {
            return;
        }
        let mut to = *cmd;
        to.server_time = self.estimated_server_time();
        // The compact move encoding carries only -127/0/127 per axis.
        to.forward = quantize_move(cmd.forward);
        to.right = quantize_move(cmd.right);
        // The server builds viewangles as SHORT2ANGLE(cmd.angles + ps.delta_angles).
        // Subtract delta_angles or the move basis is rotated (strafe reads as
        // forward/back) and a stale pitch leaks a vertical component.
        if let Some(s) = self.snapshots.newest() {
            for (i, name) in DELTA_ANGLE_FIELDS.iter().enumerate() {
                let da = s.ps.field_i32(self.proto, name);
                to.angles[i] = (to.angles[i] - da) & 0xffff;
            }
        }
        self.send_message(Some(to));
    }

    /// Queue a reliable `clc_clientCommand`, resent until acked.
    pub fn send_reliable(&mut self, cmd: &str) {
        // A full ring means the server stopped acking; overwriting unacked slots
        // desyncs the scramble key. The reference client treats it as fatal.
        if self
            .netchan
            .reliable_sequence
            .wrapping_sub(self.netchan.reliable_acknowledge)
            >= self.netchan.reliable.len() as u32
        {
            self.drop("reliable command overflow");
            return;
        }
        self.netchan.reliable_sequence += 1;
        let slot = self.netchan.reliable_sequence as usize & (self.netchan.reliable.len() - 1);
        self.netchan.reliable[slot] = cmd.to_string();
    }

    /// Ask the server for `remote` (`main/foo.pk3`), spooling into `dest`. One
    /// transfer at a time; a refusal surfaces as `Dropped` with the reason.
    pub fn begin_download(&mut self, remote: &str, dest: &std::path::Path) -> anyhow::Result<()> {
        if self.download.is_some() {
            anyhow::bail!("a download is already in progress");
        }
        let dl = download::Download::create(remote, dest)?;
        self.download = Some(dl);
        self.stopdl_sent = false;
        self.send_reliable(&format!("download {remote}"));
        self.send_message(None);
        self.last_send = self.now;
        Ok(())
    }

    /// `(received, total)` bytes; total is 0 until block 0 arrives.
    pub fn download_progress(&self) -> Option<(u64, u32)> {
        self.download.as_ref().map(|d| (d.received, d.size))
    }

    /// `donedl` makes the server resend the gamestate, so walk back to
    /// `LoadingGamestate`.
    pub fn finish_downloads(&mut self) {
        self.send_reliable("donedl");
        self.send_message(None);
        self.state = NetState::LoadingGamestate;
        self.connect_deadline = self.now + GAMESTATE_TIMEOUT;
        self.last_send = self.now;
    }

    pub fn disconnect(&mut self) {
        if matches!(self.state, NetState::LoadingGamestate | NetState::Active) {
            self.send_reliable("disconnect");
            self.send_message(None);
        }
        self.state = NetState::Disconnected;
    }

    // ---- internals ----

    fn send_getchallenge(&mut self) {
        self.transport
            .send(&connectionless::build_oob("getchallenge"));
    }

    fn send_connect(&mut self) {
        self.userinfo = format!(
            "\\cg_predictItems\\1\\cl_anonymous\\0\\handicap\\100\\color\\4\\head\\default\
             \\model\\multi\\snaps\\20\\rate\\25000\\name\\vcod\
             \\protocol\\{}\\qport\\{}\\challenge\\{}",
            self.proto.version, self.qport, self.challenge
        );
        self.transport
            .send(&connectionless::build_connect(&self.userinfo));
    }

    fn tick(&mut self) {
        match self.state {
            NetState::Challenging | NetState::Connecting => {
                if self.now.duration_since(self.last_send) >= CONNECT_RESEND {
                    if self.tries >= CONNECT_TRIES {
                        let reason = match self.handshake_print.take() {
                            Some(msg) => format!("no response from server (server said: {msg})"),
                            None => "no response from server".to_string(),
                        };
                        self.drop(&reason);
                        return;
                    }
                    if self.state == NetState::Challenging {
                        self.send_getchallenge();
                    } else {
                        self.send_connect();
                    }
                    self.tries += 1;
                    self.last_send = self.now;
                }
            }
            NetState::LoadingGamestate => {
                if self.now >= self.connect_deadline {
                    self.drop("no gamestate from server");
                    return;
                }
                if self.now.duration_since(self.last_send) >= GAMESTATE_POKE {
                    self.send_message(None);
                    self.last_send = self.now;
                }
            }
            NetState::Active => {
                if self.now.duration_since(self.last_snapshot) >= ACTIVE_TIMEOUT {
                    self.drop("server timed out");
                } else if self.download.is_some()
                    && self.now.duration_since(self.last_send) >= GAMESTATE_POKE
                {
                    // Keep reliable resends flowing during a download; no render
                    // loop is sending frames yet.
                    self.send_message(None);
                    self.last_send = self.now;
                }
            }
            NetState::Disconnected => {}
        }
    }

    fn drop(&mut self, reason: &str) {
        if let Some(dl) = self.download.take() {
            dl.abort();
        }
        self.state = NetState::Disconnected;
        self.events.push(NetEvent::Dropped(reason.to_string()));
    }

    fn handle_packet(&mut self, pkt: &[u8]) {
        if let Some((cmd, rest)) = connectionless::parse_oob(pkt) {
            self.handle_oob(cmd, rest);
            return;
        }
        match self.netchan.process_in(pkt, &self.huff) {
            Ok(Some(msg)) => self.handle_message(&msg),
            Ok(None) => {}
            Err(_) => {} // not a netchan packet
        }
    }

    fn handle_oob(&mut self, cmd: &str, rest: &[u8]) {
        log::debug!(
            "oob [{:?}] {cmd} {:?}",
            self.state,
            String::from_utf8_lossy(rest)
        );
        match cmd {
            "challengeResponse" if self.state == NetState::Challenging => {
                // Only the first field is the challenge; a second may follow.
                let Some(ch) = String::from_utf8_lossy(rest)
                    .split_whitespace()
                    .next()
                    .and_then(|s| s.parse::<i32>().ok())
                else {
                    return;
                };
                self.challenge = ch;
                self.netchan = Netchan::new(self.qport, ch);
                self.send_connect();
                self.state = NetState::Connecting;
                self.tries = 1;
                self.last_send = self.now;
            }
            "connectResponse" if self.state == NetState::Connecting => {
                self.state = NetState::LoadingGamestate;
                self.connect_deadline = self.now + GAMESTATE_TIMEOUT;
                self.last_send = self.now - GAMESTATE_POKE; // poke immediately
            }
            "disconnect" => self.drop("server refused the connection"),
            // A rejected connect is `error\n<reason>`; surface the reason instead
            // of stalling to the connect timeout.
            "error" => {
                let reason = strip_colors(String::from_utf8_lossy(rest).trim());
                self.drop(if reason.is_empty() {
                    "server rejected the connection"
                } else {
                    &reason
                });
            }
            // A handshake `print` is usually an MOTD and the connect still
            // completes. A rejection ("Server is full") arrives the same way but
            // then stalls, so keep the text for the timeout drop to report.
            "print" if matches!(self.state, NetState::Challenging | NetState::Connecting) => {
                let text = strip_colors(String::from_utf8_lossy(rest).trim());
                if !text.is_empty() {
                    self.events.push(NetEvent::Print(text.clone()));
                    self.handshake_print = Some(text);
                }
            }
            _ => {}
        }
    }

    /// One decoded server message: `[u32 reliableAcknowledge]` then the
    /// unscrambled, still compressed op block.
    fn handle_message(&mut self, msg: &[u8]) {
        if msg.len() < 4 {
            return;
        }
        self.netchan.reliable_acknowledge = u32::from_le_bytes(msg[..4].try_into().unwrap());

        let message_num = self.netchan.incoming_sequence;
        let mut r = MsgReader::new(&msg[4..], &self.huff);
        let mut has_snapshot = false;
        let mut got_download = false;
        loop {
            if r.is_overflowed() {
                break;
            }
            match r.read_byte() {
                SVC_NOP => {}
                SVC_SERVER_COMMAND => {
                    let seq = r.read_long();
                    let s = r.read_big_string();
                    self.handle_server_command(seq, s);
                }
                SVC_GAMESTATE => {
                    match gamestate::parse_body(&mut r, self.proto) {
                        Ok(gs) => {
                            // A map change re-sends the gamestate with the netchan up
                            // (protocol-1.1.md, "map_restart and sv_serverid").
                            if matches!(self.state, NetState::LoadingGamestate | NetState::Active) {
                                if self.capture.is_some() {
                                    self.captured_gamestate = Some(msg.to_vec());
                                }
                                self.on_gamestate(gs);
                            }
                        }
                        // Fatal; drop now rather than wait for the snapshot timeout.
                        Err(e) => {
                            log::warn!("gamestate parse failed: {e}");
                            self.drop(&format!("gamestate parse failed: {e}"));
                            return;
                        }
                    }
                    // parse_body ran to svc_EOF and the trailing longs.
                    break;
                }
                SVC_SNAPSHOT => {
                    if let Err(e) = self.snapshots.parse_into(&mut r, self.proto, message_num) {
                        // Drop this frame; a later keyframe recovers.
                        log::warn!("snapshot {message_num} parse failed: {e}");
                        break;
                    }
                    has_snapshot = true;
                    self.last_snapshot = self.now;
                    if let Some(s) = self.snapshots.newest() {
                        self.server_time = s.server_time;
                        self.server_time_at = self.now;
                    }
                }
                SVC_EOF => break,
                SVC_DOWNLOAD => match self.parse_download(&mut r) {
                    Ok(progressed) => got_download |= progressed,
                    Err(()) => return, // fatal, already dropped
                },
                _ => break, // unknown op: the rest is unreadable
            }
        }

        if got_download {
            // Download data counts as liveness, and the acks go out now: nothing
            // else sends while the render loop is idle.
            self.last_snapshot = self.now;
            self.send_message(None);
            self.last_send = self.now;
        }

        if has_snapshot {
            if let Some(cap) = self.capture.as_mut() {
                cap.triples.extend_from_slice(&message_num.to_le_bytes());
                cap.triples
                    .extend_from_slice(&(msg.len() as u32).to_le_bytes());
                cap.triples.extend_from_slice(msg);
                cap.count += 1;
                if let Some(s) = self.snapshots.newest() {
                    cap.times.push(s.server_time);
                }
            }
        }
    }

    /// One `svc_download` op (docs/protocol-1.1.md, svc_download). Returns
    /// whether the transfer moved; `Err` means the connection was dropped.
    fn parse_download(&mut self, r: &mut MsgReader) -> Result<bool, ()> {
        let block = r.read_short() as u16;
        let mut size = 0i32;
        if block == 0 {
            size = r.read_long();
            if size < 0 {
                let why = strip_colors(r.read_string().trim());
                self.drop(if why.is_empty() {
                    "server refused the download"
                } else {
                    &why
                });
                return Err(());
            }
        }
        let chunk = r.read_short();
        if !(0..=MAX_DOWNLOAD_BLKSIZE as i16).contains(&chunk) {
            self.drop(&format!("bad download block size {chunk}"));
            return Err(());
        }
        let mut data = vec![0u8; chunk as usize];
        for b in &mut data {
            *b = r.read_byte();
        }
        if r.is_overflowed() {
            self.drop("truncated download block");
            return Err(());
        }

        if self.download.is_none() {
            // Unrequested data; tell the server to stop, once.
            if !self.stopdl_sent {
                self.stopdl_sent = true;
                self.send_reliable("stopdl");
                return Ok(true);
            }
            return Ok(false);
        }
        let dl = self.download.as_mut().unwrap();
        if block == 0 {
            dl.size = size as u32;
        }
        if block != dl.next_block {
            return Ok(false); // a retransmit
        }
        if dl.size > MAX_DOWNLOAD_SIZE {
            let why = format!("{} is too large ({} bytes announced)", dl.remote, dl.size);
            self.drop(&why);
            return Err(());
        }
        if let Err(e) = dl.accept_block(&data) {
            self.drop(&format!("download failed: {e}"));
            return Err(());
        }
        if dl.received > dl.size as u64 {
            let why = format!(
                "{} sent {} bytes, {} announced",
                dl.remote, dl.received, dl.size
            );
            self.drop(&why);
            return Err(());
        }
        self.send_reliable(&format!("nextdl {block}"));
        if chunk == 0 {
            // A zero-length block is EOF.
            let dl = self.download.take().unwrap();
            let remote = dl.remote.clone();
            if let Err(e) = dl.finish() {
                self.drop(&format!("download failed: {e}"));
                return Err(());
            }
            self.events.push(NetEvent::DownloadComplete(remote));
        }
        Ok(true)
    }

    fn on_gamestate(&mut self, gs: Gamestate) {
        self.configstrings = gs.configstrings.clone();
        self.snapshots.clear();
        self.snapshots.set_baselines(gs.baselines.clone());
        self.server_id = server_id_from_gamestate(&gs);
        self.checksum_feed = gs.checksum_feed;
        self.client_num = gs.client_num;
        self.command_sequence = gs.server_command_sequence;
        self.gamestate = Some(gs);
        // A map change re-sends the gamestate on the live netchan; the cmd
        // stream starts over, so the delta base does too.
        self.last_sent_cmd = NULL_USERCMD;
        self.state = NetState::Active;
        self.last_snapshot = self.now;
        self.events.push(NetEvent::GamestateReady);
    }

    /// Dedupe, store in the XOR key ring, map to an event.
    fn handle_server_command(&mut self, seq: i32, cmd: String) {
        if seq <= self.command_sequence && self.command_sequence != 0 {
            return; // already executed
        }
        // The scramble and the usercmd key hash this string at this slot, so it
        // has to land verbatim.
        let slot = seq as usize & (self.netchan.server_commands.len() - 1);
        self.netchan.server_commands[slot] = cmd.clone();
        self.command_sequence = seq;

        let tokens = tokenize(&cmd);
        match tokens.first().map(String::as_str) {
            // Drop notice, SV_DropClient cod_lnxded 0x8085cf4; the reason is a
            // localized key (EXE_TIMEDOUT).
            Some("w") => {
                let reason = tokens.get(1).map_or("dropped", |s| s.as_str());
                self.drop(&strip_colors(reason));
            }
            // Q3's spelling; no CoD server sends it.
            Some("disconnect") => self.drop("server closed the connection"),
            Some("chat") | Some("tchat") => {
                if let Some(text) = tokens.get(1) {
                    self.events.push(NetEvent::Chat {
                        text: text.clone(),
                        team: tokens[0] == "tchat",
                    });
                }
            }
            Some("print") | Some("cp") => {
                if let Some(text) = tokens.get(1) {
                    self.events.push(NetEvent::Print(strip_colors(text)));
                }
            }
            // Configstring update (docs/research/cod11-hud-protocol.md, section 0).
            // The text is unquoted and runs to end of line, so read it off the raw
            // command, not the tokens.
            Some("d") => {
                if let Some((i, val)) = parse_configstring_update(&cmd) {
                    if i < self.configstrings.len() {
                        self.configstrings[i] = val;
                        if i == 1 {
                            // map_restart bumps sv_serverid and the server drops
                            // messages carrying the old one (docs/protocol-1.1.md,
                            // "map_restart and sv_serverid").
                            self.server_id = server_id_from_systeminfo(&self.configstrings[1]);
                        }
                        self.events.push(NetEvent::ConfigstringChanged(i));
                    }
                }
            }
            _ => {
                log::debug!("serverCommand {seq}: {cmd}");
                self.events.push(NetEvent::ServerCommand(tokens));
            }
        }
    }

    /// Client message: unacked reliables, an optional move, `clc_EOF`.
    fn send_message(&mut self, cmd: Option<UserCmd>) {
        let message_ack = self.netchan.incoming_sequence as i32;
        let reliable_ack = self.command_sequence;

        let mut w = MsgWriter::new(&self.huff);
        // `reliable_acknowledge` comes off the wire; an all-ones ack must not
        // overflow the add.
        let from = self.netchan.reliable_acknowledge.wrapping_add(1);
        for seq in from..=self.netchan.reliable_sequence {
            let slot = seq as usize & (self.netchan.reliable.len() - 1);
            let s = self.netchan.reliable[slot].clone();
            if s.is_empty() {
                continue;
            }
            w.write_bits(CLC_CLIENT_COMMAND, 2);
            w.write_long(seq as i32);
            w.write_string(&s);
        }
        let mut sent = None;
        if let Some(to) = cmd {
            let key = self.usercmd_key(message_ack, reliable_ack);
            // clc_moveNoDelta sets our deltaMessage to -1 and every snapshot back
            // is a full keyframe; once we hold a snapshot, clc_move lets the server
            // delta-compress. Keyframes on a populated server fragment and drop.
            let clc = if self.snapshots.newest().is_some() {
                CLC_MOVE
            } else {
                CLC_MOVE_NO_DELTA
            };
            w.write_bits(clc, 2);
            w.write_byte(1); // one usercmd
            write_delta_usercmd(&mut w, key, &self.last_sent_cmd, &to);
            sent = Some(to);
        }
        w.write_bits(CLC_EOF, 2);
        let ops = w.into_ops();

        if let Ok(pkt) =
            self.netchan
                .build_out(self.server_id, message_ack, reliable_ack, &ops, &self.huff)
        {
            self.transport.send(&pkt);
            if let Some(to) = sent {
                self.last_sent_cmd = to;
            }
        }
    }

    /// `SV_UserMove`'s delta key (cod_lnxded 0x8087043).
    fn usercmd_key(&self, message_ack: i32, reliable_ack: i32) -> i32 {
        let slot = reliable_ack as usize & (self.netchan.server_commands.len() - 1);
        let cmd = &self.netchan.server_commands[slot];
        self.checksum_feed ^ message_ack ^ com_hash_key(cmd, 32)
    }

    fn estimated_server_time(&self) -> i32 {
        let elapsed = self.now.duration_since(self.server_time_at).as_millis() as i32;
        self.server_time.wrapping_add(elapsed)
    }
}

/// In usercmd angle order [pitch, yaw, roll].
const DELTA_ANGLE_FIELDS: [&str; 3] = ["delta_angles[0]", "delta_angles[1]", "delta_angles[2]"];

/// The compact usercmd encoding carries only -127, 0 or 127 per axis.
fn quantize_move(v: i8) -> i8 {
    if v as i32 > 63 {
        127
    } else if (v as i32) < -63 {
        -127
    } else {
        0
    }
}

/// cl_parse.c:478; `CS_SYSTEMINFO` is configstring 1.
pub fn server_id_from_gamestate(gs: &Gamestate) -> i32 {
    gs.configstrings
        .get(1)
        .map_or(0, |si| server_id_from_systeminfo(si))
}

fn server_id_from_systeminfo(systeminfo: &str) -> i32 {
    info_value_for_key(systeminfo, "sv_serverid")
        .and_then(|v| v.parse::<i32>().ok())
        .unwrap_or(0)
}

/// Split `d <index> <text>`; a trailing newline is the line terminator, not
/// part of the value.
fn parse_configstring_update(cmd: &str) -> Option<(usize, String)> {
    let rest = cmd.strip_prefix('d')?.trim_start();
    let (idx, val) = rest.split_once(' ').unwrap_or((rest, ""));
    let i = idx.parse::<usize>().ok()?;
    Some((i, val.trim_end_matches('\n').to_string()))
}

/// `Com_HashKey` (cod_lnxded 0x806810c). Empty string hashes to 0.
pub fn com_hash_key(s: &str, maxlen: usize) -> i32 {
    let mut hash: i32 = 0;
    for (i, &b) in s.as_bytes().iter().take(maxlen).enumerate() {
        if b == 0 {
            break;
        }
        hash = hash.wrapping_add((b as i8 as i32).wrapping_mul(i as i32 + 0x77));
    }
    (hash >> 10) ^ hash ^ (hash >> 20)
}

/// Strip `^N` colour codes and control characters (a hostile server could put
/// ANSI escapes in chat/print text bound for the terminal); tab survives.
pub fn strip_colors(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '^' && chars.peek().is_some_and(|n| n.is_ascii_digit()) {
            chars.next();
        } else if c.is_control() && c != '\t' {
            // dropped
        } else {
            out.push(c);
        }
    }
    out
}

/// Split on whitespace; a double-quoted run is one token.
fn tokenize(s: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut chars = s.chars().peekable();
    while let Some(&c) = chars.peek() {
        if c.is_whitespace() {
            chars.next();
        } else if c == '"' {
            chars.next();
            let mut tok = String::new();
            for c in chars.by_ref() {
                if c == '"' {
                    break;
                }
                tok.push(c);
            }
            tokens.push(tok);
        } else {
            let mut tok = String::new();
            while let Some(&c) = chars.peek() {
                if c.is_whitespace() {
                    break;
                }
                tok.push(c);
                chars.next();
            }
            tokens.push(tok);
        }
    }
    tokens
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;

    #[derive(Default)]
    struct FakeTransport {
        incoming: VecDeque<Vec<u8>>,
        sent: Vec<Vec<u8>>,
    }

    impl Transport for FakeTransport {
        fn try_recv(&mut self, buf: &mut [u8]) -> Option<usize> {
            let pkt = self.incoming.pop_front()?;
            buf[..pkt.len()].copy_from_slice(&pkt);
            Some(pkt.len())
        }
        fn send(&mut self, data: &[u8]) {
            self.sent.push(data.to_vec());
        }
    }

    fn oob(cmd: &str, tail: &str) -> Vec<u8> {
        let mut p = connectionless::build_oob(cmd);
        p.extend_from_slice(tail.as_bytes());
        p
    }

    /// `[u32 sequence][payload]`. The tests pick challenge == sequence and leave
    /// the command ring empty, so the netchan decode is a no-op.
    fn server_packet(seq: u32, payload: &[u8]) -> Vec<u8> {
        let mut p = seq.to_le_bytes().to_vec();
        p.extend_from_slice(payload);
        p
    }

    fn last_oob(t: &FakeTransport) -> Option<String> {
        let pkt = t.sent.last()?;
        connectionless::parse_oob(pkt).map(|(c, _)| c.to_string())
    }

    #[test]
    fn connect_sends_getchallenge() {
        let t0 = Instant::now();
        let c = NetClient::start(FakeTransport::default(), t0);
        assert_eq!(c.state(), NetState::Challenging);
        assert_eq!(last_oob(&c.transport).as_deref(), Some("getchallenge"));
    }

    #[test]
    fn challenge_resends_after_two_seconds_then_drops() {
        let t0 = Instant::now();
        let mut c = NetClient::start(FakeTransport::default(), t0);
        assert_eq!(c.transport.sent.len(), 1);
        c.pump_at(t0 + Duration::from_millis(1900));
        assert_eq!(c.transport.sent.len(), 1);
        for i in 1..=5 {
            let ev = c.pump_at(t0 + Duration::from_secs(2 * i));
            if let Some(NetEvent::Dropped(_)) = ev.first() {
                assert_eq!(c.state(), NetState::Disconnected);
                assert_eq!(i, 5, "should give up after 5 sends");
                return;
            }
            assert_eq!(last_oob(&c.transport).as_deref(), Some("getchallenge"));
        }
        panic!("never dropped");
    }

    #[test]
    fn challenge_response_sends_connect() {
        let t0 = Instant::now();
        let mut c = NetClient::start(FakeTransport::default(), t0);
        c.transport
            .incoming
            .push_back(oob("challengeResponse", " 42"));
        c.pump_at(t0);
        assert_eq!(c.state(), NetState::Connecting);
        assert_eq!(c.challenge, 42);
        assert_eq!(last_oob(&c.transport).as_deref(), Some("connect"));
    }

    #[test]
    fn connect_response_moves_to_loading() {
        let t0 = Instant::now();
        let mut c = NetClient::start(FakeTransport::default(), t0);
        c.transport
            .incoming
            .push_back(oob("challengeResponse", " 7"));
        c.pump_at(t0);
        c.transport.incoming.push_back(oob("connectResponse", ""));
        c.pump_at(t0);
        assert_eq!(c.state(), NetState::LoadingGamestate);
    }

    #[test]
    fn gamestate_fixture_drives_to_active() {
        let gs = crate::testing::fixture("net/gamestate.bin");
        let seq = 100u32;
        let challenge = seq as i32; // challenge ^ sequence == 0

        let t0 = Instant::now();
        let mut c = NetClient::start(FakeTransport::default(), t0);
        c.transport
            .incoming
            .push_back(oob("challengeResponse", &format!(" {challenge}")));
        c.pump_at(t0);
        c.transport.incoming.push_back(oob("connectResponse", ""));
        c.pump_at(t0);
        assert_eq!(c.state(), NetState::LoadingGamestate);

        c.transport.incoming.push_back(server_packet(seq, &gs));
        let events = c.pump_at(t0);

        assert_eq!(c.state(), NetState::Active);
        assert!(events.contains(&NetEvent::GamestateReady));
        assert!(c.gamestate().is_some());
        assert!(c.configstring(0).contains("mapname"));
        assert_ne!(c.server_id, 0);
    }

    #[test]
    fn a_gamestate_while_active_reloads() {
        let gs = crate::testing::fixture("net/gamestate.bin");
        let seq = 100u32;
        let challenge = seq as i32;
        let t0 = Instant::now();
        let mut c = NetClient::start(FakeTransport::default(), t0);
        c.transport
            .incoming
            .push_back(oob("challengeResponse", &format!(" {challenge}")));
        c.pump_at(t0);
        c.transport.incoming.push_back(oob("connectResponse", ""));
        c.pump_at(t0);
        c.transport.incoming.push_back(server_packet(seq, &gs));
        c.pump_at(t0);
        assert_eq!(c.state(), NetState::Active);
        // the server switches maps: the same gamestate again, one sequence later.
        // +256 keeps `challenge ^ sequence` zero in the scramble seed's low byte,
        // so the decode stays a no-op like the comment on `server_packet` says.
        c.transport
            .incoming
            .push_back(server_packet(seq + 256, &gs));
        let events = c.pump_at(t0);
        assert!(events.contains(&NetEvent::GamestateReady));
        assert_eq!(c.state(), NetState::Active);
        assert!(c.snapshots().newest().is_none());
        assert!(c.configstring(0).contains("mapname"));
    }

    #[test]
    fn active_times_out_without_snapshots() {
        let gs = crate::testing::fixture("net/gamestate.bin");
        let seq = 100u32;
        let t0 = Instant::now();
        let mut c = NetClient::start(FakeTransport::default(), t0);
        c.transport
            .incoming
            .push_back(oob("challengeResponse", &format!(" {seq}")));
        c.pump_at(t0);
        c.transport.incoming.push_back(oob("connectResponse", ""));
        c.pump_at(t0);
        c.transport.incoming.push_back(server_packet(seq, &gs));
        c.pump_at(t0);
        assert_eq!(c.state(), NetState::Active);

        c.pump_at(t0 + Duration::from_secs(29));
        assert_eq!(c.state(), NetState::Active);
        let ev = c.pump_at(t0 + Duration::from_secs(31));
        assert_eq!(c.state(), NetState::Disconnected);
        assert!(matches!(ev.first(), Some(NetEvent::Dropped(_))));
    }

    /// An all-ones reliableAcknowledge off the wire must not overflow the next
    /// send.
    #[test]
    fn max_ack_does_not_overflow_send() {
        let gs = crate::testing::fixture("net/gamestate.bin");
        let seq = 100u32;
        let t0 = Instant::now();
        let mut c = NetClient::start(FakeTransport::default(), t0);
        c.transport
            .incoming
            .push_back(oob("challengeResponse", &format!(" {seq}")));
        c.pump_at(t0);
        c.transport.incoming.push_back(oob("connectResponse", ""));
        c.pump_at(t0);
        c.transport.incoming.push_back(server_packet(seq, &gs));
        c.pump_at(t0);
        assert_eq!(c.state(), NetState::Active);

        c.handle_message(&[0xff, 0xff, 0xff, 0xff]);
        assert_eq!(c.netchan.reliable_acknowledge, u32::MAX);

        c.send_frame(&UserCmd::default());
        c.send_message(None);
    }

    fn active_client() -> NetClient<FakeTransport> {
        let gs = crate::testing::fixture("net/gamestate.bin");
        let seq = 100u32;
        let t0 = Instant::now();
        let mut c = NetClient::start(FakeTransport::default(), t0);
        c.transport
            .incoming
            .push_back(oob("challengeResponse", &format!(" {seq}")));
        c.pump_at(t0);
        c.transport.incoming.push_back(oob("connectResponse", ""));
        c.pump_at(t0);
        c.transport.incoming.push_back(server_packet(seq, &gs));
        c.pump_at(t0);
        assert_eq!(c.state(), NetState::Active);
        c
    }

    /// `[u32 reliableAck=0][compressed ops]`, the ops written by `f` plus `svc_EOF`.
    fn ops_message(f: impl Fn(&mut MsgWriter)) -> Vec<u8> {
        let h = Huffman::new();
        let mut w = MsgWriter::new(&h);
        f(&mut w);
        w.write_byte(SVC_EOF);
        let mut msg = 0u32.to_le_bytes().to_vec();
        msg.extend(w.finish());
        msg
    }

    fn reliable_count(c: &NetClient<FakeTransport>, cmd: &str) -> usize {
        c.netchan.reliable.iter().filter(|r| *r == cmd).count()
    }

    fn write_data(w: &mut MsgWriter, data: &[u8]) {
        for &b in data {
            w.write_byte(b);
        }
    }

    #[test]
    fn download_happy_path() {
        let dir = std::env::temp_dir().join(format!("vcod-dl-happy-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let dest = dir.join("main/foo.pk3");

        let mut c = active_client();
        c.begin_download("main/foo.pk3", &dest).unwrap();
        assert_eq!(reliable_count(&c, "download main/foo.pk3"), 1);

        // Block 0, block 1 and the EOF block in one message, as the server
        // batches its window.
        let zip = download::test_zip_bytes();
        let (a, b) = zip.split_at(10);
        c.handle_message(&ops_message(|w| {
            w.write_byte(SVC_DOWNLOAD);
            w.write_short(0);
            w.write_long(zip.len() as i32);
            w.write_short(a.len() as i16);
            write_data(w, a);
            w.write_byte(SVC_DOWNLOAD);
            w.write_short(1);
            w.write_short(b.len() as i16);
            write_data(w, b);
            w.write_byte(SVC_DOWNLOAD);
            w.write_short(2);
            w.write_short(0);
        }));

        assert_eq!(std::fs::read(&dest).unwrap(), zip);
        for ack in ["nextdl 0", "nextdl 1", "nextdl 2"] {
            assert_eq!(reliable_count(&c, ack), 1, "{ack} missing");
        }
        assert!(c
            .events
            .contains(&NetEvent::DownloadComplete("main/foo.pk3".to_string())));
        assert_eq!(c.download_progress(), None);

        c.finish_downloads();
        assert_eq!(reliable_count(&c, "donedl"), 1);
        assert_eq!(c.state(), NetState::LoadingGamestate);
        let gs = crate::testing::fixture("net/gamestate.bin");
        c.handle_message(&gs);
        assert_eq!(c.state(), NetState::Active);

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn download_refusal_drops_with_reason() {
        let dir = std::env::temp_dir().join(format!("vcod-dl-refuse-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let dest = dir.join("main/foo.pk3");

        let mut c = active_client();
        c.begin_download("main/foo.pk3", &dest).unwrap();
        c.handle_message(&ops_message(|w| {
            w.write_byte(SVC_DOWNLOAD);
            w.write_short(0);
            w.write_long(-1);
            w.write_string("File \"main/foo.pk3\" not found on server");
        }));

        assert_eq!(c.state(), NetState::Disconnected);
        assert!(c
            .events
            .iter()
            .any(|e| matches!(e, NetEvent::Dropped(r) if r.contains("not found on server"))));
        assert!(!dest.exists());
        assert!(std::fs::read_dir(dir.join("main"))
            .unwrap()
            .next()
            .is_none());

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn download_ignores_out_of_order_and_duplicate_blocks() {
        let dir = std::env::temp_dir().join(format!("vcod-dl-order-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let dest = dir.join("main/foo.pk3");

        let mut c = active_client();
        c.begin_download("main/foo.pk3", &dest).unwrap();

        c.handle_message(&ops_message(|w| {
            w.write_byte(SVC_DOWNLOAD);
            w.write_short(2);
            w.write_short(3);
            write_data(w, b"xyz");
        }));
        assert_eq!(c.download_progress(), Some((0, 0)));
        assert_eq!(reliable_count(&c, "nextdl 2"), 0);

        let block0 = ops_message(|w| {
            w.write_byte(SVC_DOWNLOAD);
            w.write_short(0);
            w.write_long(11);
            w.write_short(6);
            write_data(w, b"hello ");
        });
        c.handle_message(&block0);
        assert_eq!(c.download_progress(), Some((6, 11)));

        c.handle_message(&block0);
        assert_eq!(c.download_progress(), Some((6, 11)));
        assert_eq!(reliable_count(&c, "nextdl 0"), 1);

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn download_of_garbage_drops_and_leaves_no_pak() {
        let dir = std::env::temp_dir().join(format!("vcod-dl-garbage-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let dest = dir.join("main/foo.pk3");

        let mut c = active_client();
        c.begin_download("main/foo.pk3", &dest).unwrap();
        c.handle_message(&ops_message(|w| {
            w.write_byte(SVC_DOWNLOAD);
            w.write_short(0);
            w.write_long(5);
            w.write_short(5);
            write_data(w, b"junk!");
            w.write_byte(SVC_DOWNLOAD);
            w.write_short(1);
            w.write_short(0);
        }));

        assert_eq!(c.state(), NetState::Disconnected);
        assert!(c
            .events
            .iter()
            .any(|e| matches!(e, NetEvent::Dropped(r) if r.contains("not a valid pk3"))));
        assert!(!dest.exists());
        assert!(std::fs::read_dir(dir.join("main"))
            .unwrap()
            .next()
            .is_none());

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn download_exceeding_the_announced_size_drops() {
        let dir = std::env::temp_dir().join(format!("vcod-dl-oversize-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let dest = dir.join("main/foo.pk3");

        let mut c = active_client();
        c.begin_download("main/foo.pk3", &dest).unwrap();
        c.handle_message(&ops_message(|w| {
            w.write_byte(SVC_DOWNLOAD);
            w.write_short(0);
            w.write_long(4);
            w.write_short(3);
            write_data(w, b"abc");
            w.write_byte(SVC_DOWNLOAD);
            w.write_short(1);
            w.write_short(3);
            write_data(w, b"def");
        }));

        assert_eq!(c.state(), NetState::Disconnected);
        assert!(c
            .events
            .iter()
            .any(|e| matches!(e, NetEvent::Dropped(r) if r.contains("6 bytes") && r.contains("4 announced"))));
        assert!(!dest.exists());
        assert!(std::fs::read_dir(dir.join("main"))
            .unwrap()
            .next()
            .is_none());

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn download_with_an_absurd_announced_size_drops() {
        let dir = std::env::temp_dir().join(format!("vcod-dl-absurd-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let dest = dir.join("main/foo.pk3");

        let mut c = active_client();
        c.begin_download("main/foo.pk3", &dest).unwrap();
        c.handle_message(&ops_message(|w| {
            w.write_byte(SVC_DOWNLOAD);
            w.write_short(0);
            w.write_long(MAX_DOWNLOAD_SIZE as i32 + 1);
            w.write_short(3);
            write_data(w, b"abc");
        }));

        assert_eq!(c.state(), NetState::Disconnected);
        assert!(c
            .events
            .iter()
            .any(|e| matches!(e, NetEvent::Dropped(r) if r.contains("too large"))));
        assert!(!dest.exists());
        assert!(std::fs::read_dir(dir.join("main"))
            .unwrap()
            .next()
            .is_none());

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn pump_reads_a_bounded_number_of_packets_per_call() {
        let t0 = Instant::now();
        let mut c = NetClient::start(FakeTransport::default(), t0);
        for _ in 0..(MAX_PACKETS_PER_PUMP + 10) {
            c.transport.incoming.push_back(vec![0xff; 4]);
        }
        c.pump_at(t0);
        assert_eq!(c.transport.incoming.len(), 10);
        c.pump_at(t0);
        assert!(c.transport.incoming.is_empty());
    }

    #[test]
    fn unrequested_download_gets_one_stopdl_and_parsing_continues() {
        let mut c = active_client();
        let seq = c.command_sequence + 1;
        c.handle_message(&ops_message(|w| {
            w.write_byte(SVC_DOWNLOAD);
            w.write_short(0);
            w.write_long(5);
            w.write_short(3);
            write_data(w, b"abc");
            w.write_byte(SVC_DOWNLOAD);
            w.write_short(1);
            w.write_short(2);
            write_data(w, b"de");
            // An op after the download data proves the parse stayed aligned.
            w.write_byte(SVC_SERVER_COMMAND);
            w.write_long(seq);
            w.write_string("print \"still here\"");
        }));
        assert_eq!(c.state(), NetState::Active);
        assert_eq!(reliable_count(&c, "stopdl"), 1);
        assert!(c
            .events
            .contains(&NetEvent::Print("still here".to_string())));
    }

    #[test]
    fn reliable_ring_overflow_drops() {
        let t0 = Instant::now();
        let mut c = NetClient::start(FakeTransport::default(), t0);
        for i in 0..64 {
            c.send_reliable(&format!("c{i}"));
        }
        assert_ne!(c.state(), NetState::Disconnected);
        c.send_reliable("one too many");
        assert_eq!(c.state(), NetState::Disconnected);
    }

    /// Decode packet `i` the way the server would: unscramble, decompress,
    /// walk to the clc_move, delta-decode its first cmd against `base`.
    fn decoded_move_cmd(
        c: &NetClient<FakeTransport>,
        h: &Huffman,
        i: usize,
        base: &UserCmd,
    ) -> Option<UserCmd> {
        let pkt = &c.transport.sent[i];
        let body = &pkt[6..];
        let server_id = body[0] as i32;
        let message_ack = i32::from_le_bytes(body[1..5].try_into().unwrap());
        let reliable_ack = i32::from_le_bytes(body[5..9].try_into().unwrap());
        let mut comp = body[9..].to_vec();
        let string = c.netchan.server_commands[reliable_ack as usize & 63].as_bytes();
        let mut key = (c.netchan.challenge as u32 ^ server_id as u32 ^ message_ack as u32) as u8;
        let mut index = 0usize;
        for (n, b) in comp.iter_mut().enumerate() {
            if index >= string.len() {
                index = 0;
            }
            key ^= string.get(index).copied().unwrap_or(0) << (n & 1);
            index += 1;
            *b ^= key;
        }
        // MsgReader::new block-decompresses; the body only needed unscrambling.
        let key = c.usercmd_key(message_ack, reliable_ack);
        let mut r = MsgReader::new(&comp, h);
        loop {
            match r.read_bits(2) {
                CLC_MOVE | CLC_MOVE_NO_DELTA => {
                    r.read_byte(); // count
                    return msg::read_delta_usercmd(&mut r, key, base).ok();
                }
                CLC_CLIENT_COMMAND => {
                    r.read_long();
                    r.read_string();
                }
                _ => return None,
            }
        }
    }

    /// Moves chain from the last sent cmd, retail outCmd-style. From a null
    /// base every cmd looks unchanged to itself and a release (up back to 0)
    /// encodes compact, which cannot carry `up`; the server then replays the
    /// jump forever.
    #[test]
    fn sent_moves_chain_from_the_last_sent_cmd() {
        let mut c = active_client();
        let base_pkt = c.transport.sent.len();
        c.send_frame(&UserCmd {
            up: 127,
            ..Default::default()
        });
        c.send_frame(&UserCmd {
            up: 0,
            ..Default::default()
        });
        assert_eq!(c.transport.sent.len(), base_pkt + 2);

        let h = Huffman::new();
        let press = decoded_move_cmd(&c, &h, base_pkt, &NULL_USERCMD).expect("first move decodes");
        assert_eq!(press.up, 127);
        let release = decoded_move_cmd(&c, &h, base_pkt + 1, &press).expect("second move decodes");
        assert_eq!(release.up, 0, "release must decode as 0, not replay 127");
    }

    #[test]
    fn strip_colors_removes_caret_codes() {
        assert_eq!(strip_colors("^1red ^7white"), "red white");
        assert_eq!(strip_colors("no codes"), "no codes");
        assert_eq!(strip_colors("^^2"), "^"); // a caret then a code
        assert_eq!(strip_colors("a\x1b[31mb\r\x7fc\td"), "a[31mbc\td");
    }

    #[test]
    fn oob_error_drops_with_reason() {
        let t0 = Instant::now();
        let mut c = NetClient::start(FakeTransport::default(), t0);
        c.transport
            .incoming
            .push_back(oob("error", "\nEXE_SERVER_IS_DIFFERENT_VER 1.1"));
        let ev = c.pump_at(t0);
        assert_eq!(c.state(), NetState::Disconnected);
        assert_eq!(
            ev.first(),
            Some(&NetEvent::Dropped(
                "EXE_SERVER_IS_DIFFERENT_VER 1.1".to_string()
            ))
        );
    }

    #[test]
    fn handshake_print_is_motd_not_rejection() {
        let t0 = Instant::now();
        let mut c = NetClient::start(FakeTransport::default(), t0);
        c.transport
            .incoming
            .push_back(oob("challengeResponse", " 42"));
        c.pump_at(t0);
        c.transport
            .incoming
            .push_back(oob("print", "\n^1Welcome to Revive TDM\n"));
        let ev = c.pump_at(t0);
        assert_eq!(c.state(), NetState::Connecting);
        assert_eq!(
            ev.first(),
            Some(&NetEvent::Print("Welcome to Revive TDM".to_string()))
        );
        c.transport.incoming.push_back(oob("connectResponse", ""));
        c.pump_at(t0);
        assert_eq!(c.state(), NetState::LoadingGamestate);
    }

    #[test]
    fn handshake_print_reported_when_connect_times_out() {
        let t0 = Instant::now();
        let mut c = NetClient::start(FakeTransport::default(), t0);
        c.transport
            .incoming
            .push_back(oob("challengeResponse", " 42"));
        c.pump_at(t0);
        c.transport
            .incoming
            .push_back(oob("print", "\nServer is full.\n"));
        c.pump_at(t0);
        for i in 1..=6 {
            let ev = c.pump_at(t0 + Duration::from_secs(2 * i));
            if let Some(NetEvent::Dropped(r)) = ev.first() {
                assert!(r.contains("Server is full."), "reason was {r:?}");
                return;
            }
        }
        panic!("never dropped");
    }

    #[test]
    fn tokenize_handles_quotes() {
        assert_eq!(tokenize("cs 5 \"\\a\\b\""), vec!["cs", "5", "\\a\\b"]);
        assert_eq!(tokenize("disconnect"), vec!["disconnect"]);
    }

    #[test]
    fn server_commands_map_to_events() {
        let t0 = Instant::now();
        let mut c = NetClient::start(FakeTransport::default(), t0);

        c.handle_server_command(1, "chat \"^1hi ^7there\"".to_string());
        assert_eq!(
            c.events,
            vec![NetEvent::Chat {
                text: "^1hi ^7there".to_string(),
                team: false
            }]
        );
        // Stored verbatim in the XOR key ring at seq & 63.
        assert_eq!(c.netchan.server_commands[1], "chat \"^1hi ^7there\"");
        assert_eq!(c.command_sequence, 1);
        c.events.clear();

        c.handle_server_command(2, "print \"^3loading\"".to_string());
        assert_eq!(c.events, vec![NetEvent::Print("loading".to_string())]);
        c.events.clear();

        c.handle_server_command(3, "d 12 0 1 0.00025 0.32 0.36 0.4 0\n".to_string());
        assert_eq!(c.configstring(12), "0 1 0.00025 0.32 0.36 0.4 0");
        assert_eq!(c.events, vec![NetEvent::ConfigstringChanged(12)]);
        c.events.clear();

        c.handle_server_command(2, "print \"dup\"".to_string());
        assert!(c.events.is_empty(), "dedup should drop a stale seq");
        assert_eq!(c.command_sequence, 3);

        c.handle_server_command(4, "disconnect".to_string());
        assert!(matches!(c.events.first(), Some(NetEvent::Dropped(_))));
        assert_eq!(c.state(), NetState::Disconnected);
    }

    #[test]
    fn drop_notice_carries_the_reason() {
        let t0 = Instant::now();
        let mut c = NetClient::start(FakeTransport::default(), t0);
        c.handle_server_command(1, "w \"^1EXE_TIMEDOUT\"".to_string());
        assert_eq!(
            c.events,
            vec![NetEvent::Dropped("EXE_TIMEDOUT".to_string())]
        );
        assert_eq!(c.state(), NetState::Disconnected);
    }

    #[test]
    fn systeminfo_update_refreshes_server_id() {
        let t0 = Instant::now();
        let mut c = NetClient::start(FakeTransport::default(), t0);
        c.server_id = 220;
        c.handle_server_command(1, "d 1 \\sv_pure\\0\\sv_serverid\\221\\timescale\\1".into());
        assert_eq!(c.server_id, 221);
        assert_eq!(c.events, vec![NetEvent::ConfigstringChanged(1)]);
    }

    #[test]
    fn unhandled_server_commands_surface_as_tokens() {
        let t0 = Instant::now();
        let mut c = NetClient::start(FakeTransport::default(), t0);
        c.handle_server_command(7, "b 2 0 1 0 5 60 1 12 80".into());
        assert_eq!(
            c.events.last().unwrap(),
            &NetEvent::ServerCommand(
                "b 2 0 1 0 5 60 1 12 80"
                    .split(' ')
                    .map(String::from)
                    .collect()
            )
        );
    }

    #[test]
    fn chat_keeps_color_codes_and_team_flag() {
        let t0 = Instant::now();
        let mut c = NetClient::start(FakeTransport::default(), t0);
        c.handle_server_command(8, "tchat \"^1Bob^7: go go\"".into());
        assert_eq!(
            c.events.last().unwrap(),
            &NetEvent::Chat {
                text: "^1Bob^7: go go".into(),
                team: true
            }
        );
    }
}
