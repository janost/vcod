//! CoD 1.1 network client, headless (no wgpu/winit types). Wire reference:
//! docs/protocol-1.1.md.

pub mod capture;
pub mod cdkey;
pub mod cod1;
pub use cod1::{event_ids, fields_v1, fields_v6, fields_v22, flags};
pub mod codec;
pub mod connectionless;
pub mod demo;
pub use connectionless::{info_pairs, info_value_for_key};
pub mod download;
pub mod events;
pub mod gamestate;
pub mod http;
pub mod huffman;
pub mod lan;
pub mod master;
pub mod msg;
pub mod netchan;
pub mod netsim;
pub mod protocol;
pub mod server_cache;
pub use protocol::{CS_FOG_V1, FogParams};
pub mod snapshot;
pub mod trajectory;
pub mod wire;

use gamestate::Gamestate;
use huffman::Huffman;
use msg::{
    CLC_BITS, CLC_CLIENT_COMMAND, CLC_EOF, CLC_MOVE, CLC_MOVE_NO_DELTA, MsgReader, MsgWriter,
    NULL_USERCMD, SVC_DOWNLOAD, SVC_EOF, SVC_GAMESTATE, SVC_NOP, SVC_SERVER_COMMAND, SVC_SNAPSHOT,
    UserCmd, write_delta_usercmd_in,
};
use netchan::Netchan;
use protocol::{PROTOCOL_V1, Protocol};
use snapshot::SnapshotRing;
use std::net::UdpSocket;
use std::time::{Duration, Instant};

/// Largest download block: retail's `CL_ParseDownload` checks no size and
/// copies into a 16 KiB stack buffer (docs/protocol-1.1.md, svc_download).
const MAX_DOWNLOAD_BLKSIZE: usize = 0x4000;

/// Largest pk3 a server may announce; the biggest stock pak is under 300 MiB.
const MAX_DOWNLOAD_SIZE: u32 = 512 << 20;

/// The www redirect's flag bits (`DL_FLAG_DISCON`, `DL_FLAG_URL`): the
/// server drops the client for the transfer; the URL is a page to open.
const WWW_FLAG_DISCON: i32 = 1;
const WWW_FLAG_URL: i32 = 2;

/// Packets read per `pump`, matching the server's `MAX_PACKETS_PER_FRAME`, so a
/// flood cannot stall the caller's frame.
const MAX_PACKETS_PER_PUMP: usize = 256;

const CONNECT_RESEND: Duration = Duration::from_secs(2);
const CONNECT_TRIES: u32 = 5;
const GAMESTATE_POKE: Duration = Duration::from_millis(200);
/// `CL_ReadyToSendPacket` (CoDMP.exe 0x40b940) sends at most one packet per
/// 50 ms while a download runs, whatever the LAN or `cl_maxpackets` say.
const DOWNLOAD_POKE: Duration = Duration::from_millis(50);
const GAMESTATE_TIMEOUT: Duration = Duration::from_secs(20);
/// Retail's `SV_ClientCommand` flood window (`svs.time + 800`).
const FLOOD_WINDOW: Duration = Duration::from_millis(800);
/// `CL_DisconnectPacket`'s guard (CoDMP.exe 0x410663, `cmp eax,0xbb8`).
const DISCONNECT_GUARD: Duration = Duration::from_millis(3000);
/// Snapshot silence that drops an active connection.
const ACTIVE_TIMEOUT: Duration = Duration::from_secs(30);

/// Socket abstraction so the state machine runs in tests without UDP.
/// `try_recv` is non-blocking.
pub trait Transport {
    fn try_recv(&mut self, buf: &mut [u8]) -> Option<usize>;
    fn send(&mut self, data: &[u8]);
    /// The server's address, where there is one: what decides whether the
    /// CD key goes to the authorize server ([`cdkey::authorizes`]).
    fn server_addr(&self) -> Option<std::net::SocketAddr> {
        None
    }
}

pub struct UdpTransport {
    /// `None` for [`UdpTransport::none`].
    sock: Option<UdpSocket>,
    /// `VCOD_NETSIM`'s bad network, when set ([`netsim`]).
    sim: Option<netsim::NetSim>,
    lan: bool,
}

impl UdpTransport {
    pub fn connect(addr: &str) -> anyhow::Result<Self> {
        let sock = UdpSocket::bind("0.0.0.0:0")?;
        sock.connect(addr)?;
        Self::from_socket(sock)
    }

    /// A transport over a socket already bound (and connected, or it counts
    /// as on the LAN): what a CoD2 session hands over after its handshake.
    pub fn from_socket(sock: UdpSocket) -> anyhow::Result<Self> {
        sock.set_nonblocking(true)?;
        let sim = netsim::NetSimConfig::from_env().map(|cfg| {
            log::info!("VCOD_NETSIM: {cfg:?}");
            let seed = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(1, |d| d.as_nanos() as u64);
            netsim::NetSim::new(cfg, seed)
        });
        // Retail's `Sys_IsLANAddress` (CoDMP.exe 0x464be0), which lets every
        // frame send a packet. A simulated network is never one.
        let lan = sim.is_none() && sock.peer_addr().ok().is_none_or(|a| lan::is_lan(a.ip()));
        Ok(UdpTransport {
            sock: Some(sock),
            sim,
            lan,
        })
    }

    /// No socket: what a demo plays through, which sends nothing.
    pub fn none() -> Self {
        UdpTransport {
            sock: None,
            sim: None,
            lan: true,
        }
    }

    /// Puts the simulator's due outgoing datagrams on the wire.
    fn flush(&mut self, now: Instant) {
        if let (Some(sim), Some(sock)) = (&mut self.sim, &self.sock) {
            while let Some(d) = sim.take_outgoing(now) {
                let _ = sock.send(&d);
            }
        }
    }

    /// Puts every datagram the simulator still holds on the wire now, so a
    /// disconnect is not lost with the session.
    pub fn drain(&mut self) {
        if let (Some(sim), Some(sock)) = (&mut self.sim, &self.sock) {
            for d in sim.drain_outgoing() {
                let _ = sock.send(&d);
            }
        }
    }

    /// Whether the server is on this machine or its LAN (`Sys_IsLANAddress`).
    pub fn is_lan(&self) -> bool {
        self.lan
    }

    /// The server's address.
    pub fn peer(&self) -> Option<std::net::SocketAddr> {
        self.sock.as_ref()?.peer_addr().ok()
    }
}

impl Transport for UdpTransport {
    fn server_addr(&self) -> Option<std::net::SocketAddr> {
        self.peer()
    }
    fn try_recv(&mut self, buf: &mut [u8]) -> Option<usize> {
        let sock = self.sock.as_ref()?;
        let Some(sim) = &mut self.sim else {
            return sock.recv(buf).ok();
        };
        let now = Instant::now();
        while let Ok(n) = sock.recv(buf) {
            sim.arrive(now, &buf[..n]);
        }
        let d = sim.take_incoming(now);
        self.flush(now);
        let d = d?;
        let n = d.len().min(buf.len());
        buf[..n].copy_from_slice(&d[..n]);
        Some(n)
    }
    fn send(&mut self, data: &[u8]) {
        let Some(sim) = &mut self.sim else {
            if let Some(sock) = &self.sock {
                let _ = sock.send(data);
            }
            return;
        };
        let now = Instant::now();
        sim.depart(now, data);
        self.flush(now);
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
    /// `h` or `i` (`team`): the message as sent, colour codes and the
    /// localization markers left in.
    Chat {
        text: String,
        team: bool,
    },
    Print(String),
    /// The session ended. The reason is in `SEH_LocalizeTextMessage` form
    /// (`Localized::message` turns it into text): a server's
    /// `EXE_`/`GAME_` key, a `\x14`/`\x15` message, or vcod's own English,
    /// which passes through.
    Dropped(String),
    /// Changed via the `d` serverCommand.
    ConfigstringChanged(usize),
    /// The pk3 is in place; carries the requested remote path (`main/foo.pk3`).
    DownloadComplete(String),
    /// Any serverCommand not handled here, tokenized. The HUD reads `b` from it.
    ServerCommand(Vec<String>),
    /// A connectionless packet the client did not act on itself (retail's
    /// `loadingnewmap` before a map change), command word and the rest.
    OutOfBand(String),
    /// The demo being played ran out (`CL_DemoCompleted`, CoDMP.exe
    /// 0x40e920); the client is disconnected.
    DemoEnded(demo::DemoEnd),
}

/// A demo being written (`clc.demorecording`, CoDMP.exe 0x15ef000).
struct Recording {
    writer: demo::DemoWriter<Box<dyn std::io::Write + Send>>,
    /// `clc.demoName` (0x15eefc0): the name `record` was given or chose.
    name: String,
    /// `clc.demowaiting` (0x15ef008): nothing is written until a snapshot
    /// that deltas from no frame, which the cmds ask for meanwhile.
    waiting: bool,
}

/// The userinfo keys the client's cvars drive (`name`, `rate`, `snaps`,
/// `password`); the rest of the string is fixed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Userinfo {
    pub name: String,
    pub rate: String,
    pub snaps: String,
    /// The browser's password popup sets it; empty leaves the key out.
    pub password: String,
    /// `cl_wwwDownload`, sent on protocols 6 and 22 only.
    pub www_download: String,
}

/// What a client sends until told otherwise, the probes included. The
/// game client overrides it from its cvars, whose `rate` default is
/// retail's 5000; a probe keeps 25000 so a retail server never throttles a
/// capture.
impl Default for Userinfo {
    fn default() -> Self {
        Userinfo {
            name: "vcod".into(),
            rate: "25000".into(),
            snaps: "20".into(),
            password: String::new(),
            www_download: "1".into(),
        }
    }
}

/// One client per process, so the process id is a fine qport. A test
/// harness running two on one process is not, and says so with
/// `start_with_qport`: a server keys a peer by ip and qport, and two
/// clients sharing one look to it like a single client reconnecting.
fn process_qport() -> u16 {
    0x2000 | (std::process::id() as u16 & 0x0fff)
}

/// The protocol an `error` refusing the connect for its version names:
/// `\nEXE_SERVER_IS_DIFFERENT_VER\x15<shortversion>\n`.
/// Whose CD key a connection speaking `proto` sends: 1.1 and 1.5 share
/// one, United Offensive has its own.
fn key_game(proto: &Protocol) -> cdkey::KeyGame {
    if proto.is_uo() {
        cdkey::KeyGame::Uo
    } else {
        cdkey::KeyGame::Cod1
    }
}

fn other_version(rest: &[u8]) -> Option<&'static Protocol> {
    let text = String::from_utf8_lossy(rest);
    let v = text
        .trim()
        .strip_prefix("EXE_SERVER_IS_DIFFERENT_VER\u{15}")?;
    Protocol::by_short_version(v.trim())
}

/// CoD 1.1 spectate client, generic over its [`Transport`] so the state
/// machine is testable without a socket.
pub struct NetClient<T: Transport> {
    transport: T,
    huff: Huffman,
    proto: &'static Protocol,
    /// The handshake already switched protocol once; a second refusal ends it.
    proto_switched: bool,
    /// The install's protocol ([`NetClient::connect_from`]): a server of
    /// another game ([`Transport::same_game`](crate::net::wire::Transport::same_game)) ends the handshake instead of
    /// switching. `None` switches to any.
    install: Option<&'static Protocol>,

    state: NetState,
    challenge: i32,
    qport: u16,
    userinfo: String,
    /// The cvar half of the userinfo; [`NetClient::set_userinfo`] changes it.
    info: Userinfo,
    netchan: Netchan,

    gamestate: Option<Gamestate>,
    snapshots: SnapshotRing,
    configstrings: Vec<String>,
    /// `bigConfigString`: an `x`..`z` update being put together.
    big_configstring: String,
    server_id: i32,
    checksum_feed: i32,
    client_num: i32,
    /// Highest serverCommand sequence executed; the `reliableAcknowledge` we send.
    command_sequence: i32,
    /// Cursor for [`NetClient::take_server_commands`]; `None` until its first
    /// call, which starts it wherever the sequence has already reached.
    commands_taken: Option<i32>,

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
    /// Until when the server's flood window, opened by the last command
    /// not exempt from it, drops another (docs/protocol-1.1.md, "Client
    /// commands are flood-protected").
    flood_until: Instant,
    connect_deadline: Instant,
    last_snapshot: Instant,
    /// `clc.lastPacketTime` (CoDMP.exe 0x15ce868): stamped at
    /// `connectResponse` and by every sequenced packet once connected. `None`
    /// before the first, which retail's zeroed stamp reads as long ago.
    last_packet: Option<Instant>,
    /// Last OOB `print` during the handshake; the rejection reason if the
    /// connect stalls.
    handshake_print: Option<String>,
    /// The CD key sent to the authorize server beside each `getchallenge`
    /// ([`cdkey::lookup`]); `None` sends nothing.
    pub cd_key: Option<cdkey::CdKey>,
    /// The server answered `needcdkey`: a connect that times out says
    /// `EXE_AWAITINGCDKEYAUTH`, as retail's connect screen does.
    awaiting_key: bool,

    download: Option<download::Download>,
    /// `stopdl` already sent for an unrequested transfer.
    stopdl_sent: bool,
    /// The current download, redirected to HTTP (`svc_download` block -1).
    www: Option<WwwDownload>,
    /// A disconnected www download finished and the handshake started over:
    /// the next gamestate comes on its own, and a request waits for it.
    www_reconnect: bool,
    /// A www transfer failed its checksum or was refused: the server
    /// resends the gamestate, which only re-asks for `queued_download`.
    www_retry: bool,
    /// `clc.badChecksumList`: paks whose redirect brought the wrong
    /// checksum; a later redirect of one is refused (`CL_ParseDownload`,
    /// UO CoDUOMP.exe 0x418f90).
    bad_checksums: Vec<String>,
    /// Paks already refused once; a server that redirects one again after
    /// the refusal (UO's never falls back to UDP) is left.
    refused: Vec<String>,
    queued_download: Option<String>,

    events: Vec<NetEvent>,

    /// Raw snapshot capture for parser fixtures; off unless a probe asks.
    capture: Option<SnapshotCapture>,
    captured_gamestate: Option<Vec<u8>>,
    /// A drop is reported but the client keeps decoding: a probe watching
    /// what a zombie slot is sent ([`NetClient::listen_after_drop`]).
    listen: bool,

    recording: Option<Recording>,
    /// The demo being played (`clc.demoplaying`); the transport is unused.
    demo: Option<demo::DemoReader>,
    /// `clc.firstDemoFrameSkipped` (CoDMP.exe 0x15ef00c): the playback's
    /// first `CA_PRIMED` frame has gone by.
    demo_frame_skipped: bool,
}

/// Raw snapshot messages as `[u32 message_num][u32 len][bytes]` triples, the
/// format the snapshot parser tests replay.
#[derive(Default)]
pub struct SnapshotCapture {
    pub triples: Vec<u8>,
    pub count: usize,
    pub times: Vec<i32>,
}

/// A download the server redirected to `url`. `disconnected` is
/// `sv_wwwDlDisconnected`'s mode: the server drops the client, which fetches
/// on its own and reconnects after.
struct WwwDownload {
    url: String,
    fetch: http::Fetch,
    disconnected: bool,
    /// The pak's checksum in `sv_referencedPaks`, read at the redirect: a
    /// disconnected fetch ends after the gamestate is gone.
    want: Option<i32>,
}

/// The fields after block -1: `MSG_WriteString` URL (`sv_wwwBaseURL/<pak>`),
/// `MSG_WriteLong` size, `MSG_WriteLong` flags (`SV_WriteDownloadToClient`,
/// UO coduo_lnxded 0x808c559).
#[derive(Debug, PartialEq, Eq)]
struct WwwRedirect {
    url: String,
    size: i32,
    flags: i32,
}

fn read_www_redirect(r: &mut MsgReader) -> WwwRedirect {
    let url = r.read_string();
    let size = r.read_long();
    let flags = r.read_long();
    WwwRedirect { url, size, flags }
}

impl NetClient<UdpTransport> {
    pub fn connect(addr: &str) -> anyhow::Result<Self> {
        let transport = UdpTransport::connect(addr)?;
        Ok(Self::start(transport, Instant::now()))
    }

    /// Connects speaking the install's protocol, and refuses a server of
    /// another game rather than playing it on the wrong assets.
    pub fn connect_from(addr: &str, install: &'static Protocol) -> anyhow::Result<Self> {
        let transport = UdpTransport::connect(addr)?;
        Ok(Self::start_from(transport, Instant::now(), install))
    }
}

impl<T: Transport> NetClient<T> {
    pub fn start(transport: T, now: Instant) -> Self {
        Self::start_with_qport(transport, now, process_qport())
    }

    pub fn start_with_qport(transport: T, now: Instant, qport: u16) -> Self {
        Self::start_with_protocol(transport, now, qport, &PROTOCOL_V1)
    }

    /// [`NetClient::connect_from`] over any transport.
    pub fn start_from(transport: T, now: Instant, install: &'static Protocol) -> Self {
        let mut c = Self::start_with_protocol(transport, now, process_qport(), install);
        c.install = Some(install);
        c
    }

    /// Connects speaking `proto` first. A server of the other version
    /// refuses the connect naming its own, and the client switches to it and
    /// starts the handshake over ([`NetClient::protocol`]).
    pub fn start_with_protocol(
        transport: T,
        now: Instant,
        qport: u16,
        proto: &'static Protocol,
    ) -> Self {
        let mut c = Self::idle(transport, now, qport, proto);
        c.send_getchallenge();
        c.state = NetState::Challenging;
        c.tries = 1;
        c.last_send = now;
        c
    }

    /// `demo <name>` (CoDMP.exe 0x40eb40): plays `data`, a demo file
    /// recorded under `proto`, through the parse a connection runs. Nothing
    /// goes out on `transport`; [`Self::pump_demo`] reads it.
    pub fn play_demo(transport: T, now: Instant, proto: &'static Protocol, data: Vec<u8>) -> Self {
        let mut c = Self::idle(transport, now, 0, proto);
        c.state = NetState::LoadingGamestate;
        c.demo = Some(demo::DemoReader::new(data, proto));
        c
    }

    fn idle(transport: T, now: Instant, qport: u16, proto: &'static Protocol) -> Self {
        NetClient {
            transport,
            huff: Huffman::new(),
            proto,
            proto_switched: false,
            install: None,
            state: NetState::Disconnected,
            challenge: 0,
            qport,
            userinfo: String::new(),
            info: Userinfo::default(),
            netchan: Netchan::with_params(qport, 0, proto.transport.netchan),
            gamestate: None,
            snapshots: SnapshotRing::new(),
            configstrings: vec![String::new(); proto.transport.max_configstrings],
            big_configstring: String::new(),
            server_id: 0,
            checksum_feed: 0,
            client_num: 0,
            command_sequence: 0,
            commands_taken: None,
            server_time: 0,
            server_time_at: now,
            last_sent_cmd: NULL_USERCMD,
            now,
            tries: 0,
            last_send: now,
            flood_until: now,
            connect_deadline: now + GAMESTATE_TIMEOUT,
            last_snapshot: now,
            last_packet: None,
            handshake_print: None,
            cd_key: cdkey::lookup(key_game(proto)),
            awaiting_key: false,
            download: None,
            stopdl_sent: false,
            www: None,
            www_reconnect: false,
            www_retry: false,
            bad_checksums: Vec::new(),
            refused: Vec::new(),
            queued_download: None,
            events: Vec::new(),
            capture: None,
            captured_gamestate: None,
            listen: false,
            recording: None,
            demo: None,
            demo_frame_skipped: false,
        }
    }

    /// Whether this client plays a demo rather than a server.
    pub fn is_demo(&self) -> bool {
        self.demo.is_some()
    }

    /// `CL_SetCGameTime`'s demo reads (CoDMP.exe 0x404d60): with no
    /// gamestate yet, up to the first one; then, once the caller's clock
    /// `until` is given, every message while the newest snapshot is no
    /// later than it. A gamestate past the first stops the read, as the
    /// client leaves `CA_ACTIVE` for the load.
    pub fn pump_demo(&mut self, now: Instant, until: Option<i32>) -> Vec<NetEvent> {
        self.now = now;
        while self.demo.is_some() && self.state != NetState::Disconnected {
            let want = match (&self.gamestate, until) {
                (None, _) => true,
                (Some(_), None) => false,
                (Some(_), Some(t)) => self.snapshots.newest().is_none_or(|s| s.server_time <= t),
            };
            if !want {
                break;
            }
            self.read_demo_message();
            if self.events.contains(&NetEvent::GamestateReady) {
                break;
            }
        }
        std::mem::take(&mut self.events)
    }

    /// `CL_SetCGameTime` in `CA_PRIMED` (CoDMP.exe 0x404d60), the map up
    /// and no snapshot yet: the playback's first such frame reads nothing,
    /// every later one reads one message. Only `CL_Disconnect`'s clear of
    /// `clc` resets the skip, so a gamestate inside the demo skips no frame.
    pub fn pump_demo_primed(&mut self, now: Instant) -> Vec<NetEvent> {
        self.now = now;
        if !self.demo_frame_skipped {
            self.demo_frame_skipped = true;
        } else if self.demo.is_some() && self.state != NetState::Disconnected {
            self.read_demo_message();
        }
        std::mem::take(&mut self.events)
    }

    /// `CL_ReadDemoMessage` (CoDMP.exe 0x40e9c0): one record through
    /// [`Self::handle_message`] under its own sequence.
    fn read_demo_message(&mut self) {
        let Some(reader) = &mut self.demo else { return };
        match reader.next_message() {
            Ok((seq, msg)) => {
                self.netchan.incoming_sequence = seq;
                self.last_packet = Some(self.now);
                self.handle_message(&msg);
            }
            Err(end) => {
                if end != demo::DemoEnd::Completed {
                    log::warn!("demo: {end:?}");
                }
                self.state = NetState::Disconnected;
                self.events.push(NetEvent::DemoEnded(end));
            }
        }
    }

    /// `record` (`CL_Record_f`, CoDMP.exe 0x40e540): opens the demo on a
    /// gamestate built from what the client holds now. Refused, with
    /// retail's text, unless in a level and not recording or playing.
    pub fn start_recording(
        &mut self,
        out: Box<dyn std::io::Write + Send>,
        name: &str,
    ) -> anyhow::Result<()> {
        anyhow::ensure!(self.recording.is_none(), "Already recording.");
        let in_level = self.demo.is_none()
            && self.state == NetState::Active
            && self.snapshots.newest().is_some();
        let Some(gs) = self.gamestate.as_ref().filter(|_| in_level) else {
            anyhow::bail!("You must be in a level to record.");
        };
        let now = Gamestate {
            configstrings: self.configstrings.clone(),
            baselines: gs.baselines.clone(),
            client_num: self.client_num,
            checksum_feed: self.checksum_feed,
            server_command_sequence: self.command_sequence,
        };
        let msg = demo::gamestate_message(
            self.proto,
            &self.huff,
            self.netchan.reliable_sequence as i32,
            &now,
        );
        let mut writer = demo::DemoWriter::new(out);
        writer.write_message(self.netchan.incoming_sequence, &msg)?;
        self.recording = Some(Recording {
            writer,
            name: name.to_string(),
            waiting: true,
        });
        Ok(())
    }

    pub fn is_recording(&self) -> bool {
        self.recording.is_some()
    }

    /// The recording's name and the bytes written so far, what
    /// `SCR_DrawDemoRecording` (CoDMP.exe 0x416b10) prints.
    pub fn recording_status(&self) -> Option<(&str, u64)> {
        self.recording
            .as_ref()
            .map(|r| (r.name.as_str(), r.writer.written()))
    }

    /// `stoprecord` (`CL_StopRecord_f`, CoDMP.exe 0x40e430): the trailer,
    /// then the file closes. `false` when nothing was being recorded.
    pub fn stop_recording(&mut self) -> std::io::Result<bool> {
        let Some(rec) = self.recording.take() else {
            return Ok(false);
        };
        rec.writer.finish()?;
        Ok(true)
    }

    /// `CL_Disconnect`'s stop (CoDMP.exe 0x40ef90), its error logged.
    fn end_recording(&mut self) {
        if let Err(e) = self.stop_recording() {
            log::warn!("demo: cannot finish the recording: {e}");
        }
    }

    pub fn transport(&self) -> &T {
        &self.transport
    }

    /// The protocol this connection speaks; it can change once, while the
    /// handshake runs, when the server turns out to be the other version.
    pub fn protocol(&self) -> &'static Protocol {
        self.proto
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

    /// Highest serverCommand sequence executed, which is the sequence of the
    /// last command [`NetClient::take_server_commands`] returned.
    pub fn command_sequence(&self) -> i32 {
        self.command_sequence
    }

    /// The netchan sequence of the newest message parsed.
    pub fn incoming_sequence(&self) -> u32 {
        self.netchan.incoming_sequence
    }

    /// Verbatim text of every serverCommand executed since the last call, in
    /// order. It reads the reliable ring `handle_server_command` already keeps
    /// verbatim for the scramble key rather than buffering a second copy, so
    /// commands older than the ring are gone. The first call after a connect or
    /// a gamestate starts the cursor where the sequence stands and reports
    /// nothing before it, since those slots hold no text this client parsed.
    pub fn take_server_commands(&mut self) -> Vec<String> {
        let cap = self.netchan.server_commands.len();
        let from = *self.commands_taken.get_or_insert(self.command_sequence);
        let first = (self.command_sequence - cap as i32 + 1).max(from + 1);
        let out = (first..=self.command_sequence)
            .map(|seq| self.netchan.server_commands[seq as usize & (cap - 1)].clone())
            .collect();
        self.commands_taken = Some(self.command_sequence);
        out
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

    /// Keep decoding after a drop notice or a timeout; the drop is still
    /// reported. For probes only: a real client stops listening.
    pub fn listen_after_drop(&mut self) {
        self.listen = true;
    }

    /// Drain the socket, advance the state machine, return the events.
    pub fn pump(&mut self) -> Vec<NetEvent> {
        self.pump_at(Instant::now())
    }

    /// [`Self::pump`] with the clock injected.
    pub fn pump_at(&mut self, now: Instant) -> Vec<NetEvent> {
        if self.demo.is_some() {
            return self.pump_demo(now, None);
        }
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
    /// also the keepalive. Returns the cmd as it went on the wire, stamped,
    /// quantized and rebased on `delta_angles`, so a capture can record what
    /// the server will replay rather than what the caller asked for.
    pub fn send_frame(&mut self, cmd: &UserCmd) -> Option<UserCmd> {
        if self.state != NetState::Active {
            return None;
        }
        let mut to = *cmd;
        to.server_time = self.server_clock_ms();
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
        self.send_message(&[to]);
        Some(to)
    }

    /// Send one message whose `clc_move` carries every cmd of `cmds` (more
    /// than [`MAX_MOVE_CMDS`] keeps the last of them), chained from
    /// `last_sent_cmd`. Unlike `send_frame`, the caller's cmds go out
    /// verbatim: no stamping, no quantizing, no `delta_angles` rebase. The
    /// server adds `delta_angles` itself (docs/protocol-1.1.md, "View
    /// angles"). A no-op unless active; an empty slice still sends queued
    /// reliable commands, like `send_frame` would with none pending.
    pub fn send_cmds(&mut self, cmds: &[UserCmd]) {
        if self.state != NetState::Active {
            return;
        }
        let start = cmds.len().saturating_sub(MAX_MOVE_CMDS);
        self.send_message(&cmds[start..]);
    }

    /// A connectionless datagram to the server, as it stands (`rcon`'s
    /// packet, CoDMP.exe 0x40f880).
    pub fn send_oob(&mut self, packet: &[u8]) {
        self.transport.send(packet);
    }

    /// Queue a reliable `clc_clientCommand`, resent until acked.
    pub fn send_reliable(&mut self, cmd: &str) {
        // A full ring means the server stopped acking; overwriting unacked slots
        // desyncs the scramble key. The reference client treats it as fatal.
        // The difference is signed, as Q3's `CL_AddReliableCommand` takes it:
        // a demo's messages ack the recorder's commands, past a playback's
        // own sequence of 0, and that is no overflow.
        if self
            .netchan
            .reliable_sequence
            .wrapping_sub(self.netchan.reliable_acknowledge) as i32
            >= self.netchan.reliable.len() as i32
        {
            self.drop("reliable command overflow");
            return;
        }
        self.netchan.reliable_sequence += 1;
        let slot = self.netchan.reliable_sequence as usize & (self.netchan.reliable.len() - 1);
        self.netchan.reliable[slot] = cmd.to_string();
        if !["team ", "score ", "mr "]
            .iter()
            .any(|x| cmd.starts_with(x))
        {
            self.flood_until = self.now + FLOOD_WINDOW;
        }
    }

    /// Whether a command not exempt from the server's flood window would
    /// land inside it and be dropped.
    pub fn flood_window_open(&self) -> bool {
        self.now < self.flood_until
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
        if self.www_reconnect {
            // Back from a disconnected download: ask once the slot is ours.
            self.queued_download = Some(remote.to_string());
            return Ok(());
        }
        self.send_reliable(&format!("download {remote}"));
        self.send_message(&[]);
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
        if self.www_reconnect {
            // The reconnect after a disconnected download brings it.
            return;
        }
        self.send_reliable("donedl");
        self.send_message(&[]);
        self.state = NetState::LoadingGamestate;
        self.connect_deadline = self.now + GAMESTATE_TIMEOUT;
        self.last_send = self.now;
    }

    /// `CL_Disconnect` (CoDMP.exe 0x40ef90): the command rides three packets,
    /// so one lost datagram does not leave the slot held until `sv_timeout`.
    pub fn disconnect(&mut self) {
        self.end_recording();
        if self.demo.is_some() {
            self.state = NetState::Disconnected;
            return;
        }
        self.www = None;
        if let Some(dl) = self.download.take() {
            dl.abort();
        }
        if matches!(self.state, NetState::LoadingGamestate | NetState::Active) {
            self.send_reliable("disconnect");
            for _ in 0..3 {
                self.send_message(&[]);
            }
        }
        self.state = NetState::Disconnected;
    }

    /// A fresh handshake on the same socket and qport, the connection's
    /// protocol and userinfo kept, with no `disconnect` sent: to the server
    /// a client that crashed and came back (`SV_DirectConnect`'s
    /// reconnect arm). The probe's `--probe-edge reconnect` only.
    pub fn reconnect(self, now: Instant) -> Self {
        let (info, listen) = (self.info.clone(), self.listen);
        let mut c = Self::start_with_protocol(self.transport, now, self.qport, self.proto);
        c.set_userinfo(info);
        c.listen = listen;
        c
    }

    // ---- internals ----

    /// `CL_CheckForResend`'s challenge (CoDMP.exe 0x4105b5): the key to
    /// the authorize server first, unless the server is on the LAN, then
    /// `getchallenge`, whatever came of the first.
    fn send_getchallenge(&mut self) {
        self.request_authorization();
        self.transport
            .send(&connectionless::build_oob("getchallenge"));
    }

    /// `CL_RequestAuthorization` (CoDMP.exe 0x40f260), fire and forget.
    fn request_authorization(&self) {
        if let (Some(key), Some(server)) = (&self.cd_key, self.transport.server_addr()) {
            cdkey::request_authorization(key_game(self.proto), key, server);
        }
    }

    /// The userinfo cvars. Once connected, a change goes to the server as a
    /// `userinfo` client command, as retail's `CL_CheckUserinfo` sends it;
    /// before that it rides the `connect`. A `\\`, `;` or `"` would break the
    /// info string or the command, so they are dropped.
    pub fn set_userinfo(&mut self, info: Userinfo) {
        let clean = |v: String| -> String {
            v.chars()
                .filter(|c| !matches!(c, '\\' | ';' | '"'))
                .collect()
        };
        let info = Userinfo {
            name: clean(info.name),
            rate: clean(info.rate),
            snaps: clean(info.snaps),
            password: clean(info.password),
            www_download: clean(info.www_download),
        };
        if info == self.info {
            return;
        }
        self.info = info;
        if matches!(self.state, NetState::LoadingGamestate | NetState::Active) {
            let info = self.base_userinfo();
            self.send_reliable(&format!("userinfo \"{info}\""));
        }
    }

    fn base_userinfo(&self) -> String {
        let Userinfo {
            name,
            rate,
            snaps,
            password,
            www_download,
        } = &self.info;
        // `password` (CVAR_USERINFO, registered after `cl_anonymous` and
        // before `cg_predictItems`, CoDMP.exe 0x4123ea) sits between them in
        // the newest-first cvar walk; an empty value sets no key.
        let password = if password.is_empty() {
            String::new()
        } else {
            format!("\\password\\{password}")
        };
        // 1.5 registers `cl_guid` and `cl_punkbuster` (CVAR_USERINFO)
        // between `cl_anonymous` and `password` (CoDMP.exe 1.5 0x413380);
        // `cl_guid` keeps its default unless PunkBuster sets it.
        let pb = if self.proto.is_v1() {
            ""
        } else {
            "\\cl_punkbuster\\0\\cl_guid\\unknown"
        };
        // `cl_wwwDownload` (default 1, flags 3) is older than `name` in
        // 1.5's `CL_Init` (0x413380), so it closes the walk; 1.1 lacks it.
        let www = if self.proto.is_v1() {
            String::new()
        } else {
            format!("\\cl_wwwDownload\\{www_download}")
        };
        format!(
            "\\cg_predictItems\\1{password}{pb}\\cl_anonymous\\0\\handicap\\100\\color\\4\\head\\default\
             \\model\\multi\\snaps\\{snaps}\\rate\\{rate}\\name\\{name}{www}"
        )
    }

    fn send_connect(&mut self) {
        self.userinfo = format!(
            "{}\\protocol\\{}\\qport\\{}\\challenge\\{}",
            self.base_userinfo(),
            self.proto.transport.version,
            self.qport,
            self.challenge
        );
        self.transport
            .send(&connectionless::build_connect(&self.userinfo));
    }

    fn tick(&mut self) {
        self.poll_www();
        match self.state {
            NetState::Challenging | NetState::Connecting => {
                if self.now.duration_since(self.last_send) >= CONNECT_RESEND {
                    if self.tries >= CONNECT_TRIES {
                        let reason = match self.handshake_print.take() {
                            _ if self.awaiting_key => "EXE_AWAITINGCDKEYAUTH".to_string(),
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
                    self.send_message(&[]);
                    self.last_send = self.now;
                }
            }
            NetState::Active => {
                if self.now.duration_since(self.last_snapshot) >= ACTIVE_TIMEOUT {
                    self.drop("server timed out");
                } else if self.download.is_some()
                    && self.now.duration_since(self.last_send) >= DOWNLOAD_POKE
                {
                    // The acks and reliable resends of a download; no render
                    // loop is sending frames yet.
                    self.send_message(&[]);
                    self.last_send = self.now;
                }
            }
            NetState::Disconnected => {}
        }
    }

    fn drop(&mut self, reason: &str) {
        if self.www.as_ref().is_some_and(|w| w.disconnected) {
            // `wwwdl bbl8r` asked for this drop (GMI_EXE_DOWNLOADDISCONNECTED).
            log::info!("dropped ({reason}); the www download goes on disconnected");
            self.state = NetState::Disconnected;
            return;
        }
        if let Some(dl) = self.download.take() {
            dl.abort();
        }
        if self.listen {
            self.last_snapshot = self.now;
            self.events.push(NetEvent::Dropped(reason.to_string()));
            return;
        }
        self.state = NetState::Disconnected;
        self.end_recording();
        self.events.push(NetEvent::Dropped(reason.to_string()));
    }

    fn handle_packet(&mut self, pkt: &[u8]) {
        if let Some((cmd, rest)) = connectionless::parse_oob(pkt) {
            self.handle_oob(cmd, rest);
            return;
        }
        // `CL_PacketEvent`: nothing sequenced is valid before
        // `connectResponse`. A reconnect from the same socket would otherwise
        // read the old slot's zombie frames, drop notice included.
        if matches!(
            self.state,
            NetState::Disconnected | NetState::Challenging | NetState::Connecting
        ) {
            return;
        }
        // CoDMP.exe 0x4110b1, ahead of the netchan's own checks.
        self.last_packet = Some(self.now);
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
                self.netchan = Netchan::with_params(self.qport, ch, self.proto.transport.netchan);
                self.send_connect();
                self.state = NetState::Connecting;
                self.tries = 1;
                self.last_send = self.now;
            }
            // The authorize server does not know the key yet (CoDMP.exe
            // 0x410f00): retail shows `EXE_AWAITINGCDKEYAUTH`, sends the key
            // again and keeps challenging. Without a key retail never gets
            // here: it stops at `EXE_ERR_INVALID_CD_KEY` (0x40f286).
            "needcdkey" if self.state == NetState::Challenging => {
                if self.cd_key.is_none() {
                    cdkey::warn_no_key();
                    self.drop("EXE_ERR_INVALID_CD_KEY");
                    return;
                }
                self.awaiting_key = true;
                self.request_authorization();
            }
            "connectResponse" if self.state == NetState::Connecting => {
                self.state = NetState::LoadingGamestate;
                self.last_packet = Some(self.now);
                self.connect_deadline = self.now + GAMESTATE_TIMEOUT;
                self.last_send = self.now - GAMESTATE_POKE; // poke immediately
            }
            // `CL_DisconnectPacket` (CoDMP.exe 0x410620): honoured only
            // while connecting or connected, and only once the server has
            // been silent for 3 s, so a forged one cannot cut a live link.
            "disconnect"
                if self.state != NetState::Disconnected
                    && self
                        .last_packet
                        .is_none_or(|t| self.now.duration_since(t) >= DISCONNECT_GUARD) =>
            {
                self.drop("EXE_SERVER_DISCONNECTED")
            }
            "disconnect" => log::debug!("oob disconnect inside the 3 s guard, ignored"),
            // A server of another game, refused before it plays on the
            // wrong install's assets.
            "error"
                if self.state == NetState::Connecting
                    && let (Some(install), Some(proto)) = (self.install, other_version(rest))
                    && !proto.transport.same_game(&install.transport) =>
            {
                self.drop(&if proto.is_uo() {
                    "This is a United Offensive server: run vcod with --mod-dir uo".to_string()
                } else {
                    format!(
                        "This is a CoD {} server: run vcod without --mod-dir uo",
                        proto.transport.short_version
                    )
                });
            }
            // A server of the other version refuses the connect naming its
            // own; speak that one and start over.
            "error"
                if self.state == NetState::Connecting
                    && !self.proto_switched
                    && let Some(proto) = other_version(rest)
                    && !std::ptr::eq(proto, self.proto) =>
            {
                log::info!(
                    "server is {}, reconnecting with protocol {}",
                    proto.transport.short_version,
                    proto.transport.version
                );
                self.proto = proto;
                self.proto_switched = true;
                self.configstrings = vec![String::new(); proto.transport.max_configstrings];
                self.send_getchallenge();
                self.state = NetState::Challenging;
                self.tries = 1;
                self.last_send = self.now;
            }
            // A rejected connect is `error\n<reason>`; surface the reason instead
            // of stalling to the connect timeout. CoDMP.exe 0x410e41 localizes
            // it as a message (`EXE_SERVER_IS_DIFFERENT_VER\x151.1`).
            "error" => {
                let reason = strip_colors_keep_markers(String::from_utf8_lossy(rest).trim());
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
            // `CL_ConnectionlessPacket`'s `print` (CoDMP.exe 0x410db1) prints
            // in any state, colour codes kept: an rcon reply, among others.
            "print" => {
                let text = String::from_utf8_lossy(rest);
                let text = text.trim_end_matches(['\n', '\0']);
                if !text.is_empty() {
                    self.events.push(NetEvent::Print(text.to_string()));
                }
            }
            _ => {
                let rest = String::from_utf8_lossy(rest);
                self.events
                    .push(NetEvent::OutOfBand(format!("{cmd}\n{}", rest.trim_end())));
            }
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
        let redirected = self.www.is_none();
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
                    match self.snapshots.parse_into(&mut r, self.proto, message_num) {
                        // `CL_ParseSnapshot` ends the demo's wait on a frame
                        // that deltas from nothing (CoDMP.exe 0x415c5f).
                        Ok(s) if s.delta_num <= 0 => {
                            if let Some(rec) = &mut self.recording {
                                rec.waiting = false;
                            }
                        }
                        Ok(_) => {}
                        Err(e) => {
                            // Drop this frame; a later keyframe recovers.
                            log::warn!("snapshot {message_num} parse failed: {e}");
                            break;
                        }
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

        if redirected && self.www.is_some() {
            // The decoded message, for a parser fixture.
            log::debug!(
                "www redirect message {}",
                msg.iter().map(|b| format!("{b:02x}")).collect::<String>()
            );
        }
        if got_download {
            // Download data counts as liveness, and the acks go out now: nothing
            // else sends while the render loop is idle.
            self.last_snapshot = self.now;
            self.send_message(&[]);
            self.last_send = self.now;
        }

        // `CL_PacketEvent` writes the message once it is parsed (CoDMP.exe
        // 0x41110e).
        if let Some(rec) = &mut self.recording
            && !rec.waiting
            && let Err(e) = rec.writer.write_message(message_num, msg)
        {
            log::warn!("demo: write failed, recording stopped: {e}");
            self.recording = None;
        }

        if has_snapshot && let Some(cap) = self.capture.as_mut() {
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

    /// One `svc_download` op (docs/protocol-1.1.md, svc_download). Returns
    /// whether the transfer moved; `Err` means the connection was dropped.
    fn parse_download(&mut self, r: &mut MsgReader) -> Result<bool, ()> {
        let block = r.read_short() as u16;
        if block == 0xffff {
            return self.parse_www_redirect(r);
        }
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
        log::trace!("download block {block}, {chunk} bytes");
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

        if self.www.is_some() || self.www_retry {
            // 1.5b opens the pak before it redirects (cod_lnxded 0x808b95d), so
            // its next frames stream blocks until `wwwdl ack` lands; after a
            // failed redirect the transfer starts over at the new request.
            return Ok(false);
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
            let want = download::listed_checksum(self.configstring(1), &remote);
            if let Err(e) = dl.finish(want) {
                self.drop(&format!("download failed: {e}"));
                return Err(());
            }
            self.events.push(NetEvent::DownloadComplete(remote));
        }
        Ok(true)
    }

    /// Block -1 of `svc_download`, the www redirect: URL, size, flags
    /// (`CL_ParseDownload`, UO CoDUOMP.exe 0x418f90; 1.5 CoDMP.exe 0x417d80 and
    /// its helper 0x417b10).
    /// The server repeats it every frame until `wwwdl ack` lands.
    fn parse_www_redirect(&mut self, r: &mut MsgReader) -> Result<bool, ()> {
        let WwwRedirect { url, size, flags } = read_www_redirect(r);
        if r.is_overflowed() {
            self.drop("truncated download redirect");
            return Err(());
        }
        if self.proto.is_v1() {
            self.drop(&format!(
                "server redirected a download to {url}, which protocol 1 has no way to do"
            ));
            return Err(());
        }
        if self.www.is_some() || self.www_retry {
            return Ok(false);
        }
        if self.download.is_none() {
            self.drop(&format!(
                "server redirected a download that was never asked for to {url}"
            ));
            return Err(());
        }
        if flags & WWW_FLAG_URL != 0 {
            // Retail opens the page in a browser and quits.
            self.send_reliable("wwwdl bbl8r");
            self.send_message(&[]);
            self.drop(&format!("server sends {url} to be opened in a web browser"));
            return Err(());
        }
        if !(0..=MAX_DOWNLOAD_SIZE as i32).contains(&size) {
            self.drop(&format!("{url} announced {size} bytes"));
            return Err(());
        }
        log::info!("server redirected download: {url} ({size} bytes)");
        self.send_reliable("wwwdl ack");
        let remote = self.download.as_ref().unwrap().remote.clone();
        if self
            .bad_checksums
            .iter()
            .any(|b| b.eq_ignore_ascii_case(&remote))
        {
            if self.refused.iter().any(|b| b.eq_ignore_ascii_case(&remote)) {
                self.drop(&format!(
                    "server keeps redirecting {remote}, whose redirect had the wrong checksum"
                ));
                return Err(());
            }
            // `wwwdl fail` sets the server's fallback flag: 1.5b streams the
            // next request over UDP (cod15-protocol-diff.md 14).
            log::warn!("refusing redirect to {url} by server (bad checksum)");
            self.refused.push(remote.clone());
            self.send_reliable("wwwdl fail");
            self.send_message(&[]);
            self.www_retry = true;
            self.queued_download = Some(remote);
            return Ok(true);
        }
        let dl = self.download.as_mut().unwrap();
        dl.size = size as u32;
        let fetch = dl
            .spool()
            .map_err(anyhow::Error::from)
            .and_then(|f| http::Fetch::spawn(&url, f, MAX_DOWNLOAD_SIZE as u64));
        let fetch = match fetch {
            Ok(f) => f,
            Err(e) => {
                self.send_reliable("wwwdl fail");
                self.send_message(&[]);
                self.drop(&format!("Failed to initialize download for '{url}': {e}"));
                return Err(());
            }
        };
        let disconnected = flags & WWW_FLAG_DISCON != 0;
        if disconnected {
            self.send_reliable("wwwdl bbl8r");
        }
        let want = download::listed_checksum(self.configstring(1), &remote);
        self.www = Some(WwwDownload {
            url,
            fetch,
            disconnected,
            want,
        });
        Ok(true)
    }

    /// `CL_WWWDownload` (UO CoDUOMP.exe 0x413420): the HTTP transfer's
    /// progress, and on its end the rename and `wwwdl done`, or in the
    /// disconnected mode a reconnect.
    fn poll_www(&mut self) {
        let Some(w) = self.www.as_mut() else {
            return;
        };
        if let Some(dl) = self.download.as_mut() {
            dl.received = w.fetch.received();
        }
        let Some(res) = w.fetch.poll() else {
            // The server sends nothing while the client fetches elsewhere.
            self.last_snapshot = self.now;
            return;
        };
        let w = self.www.take().unwrap();
        let Some(dl) = self.download.take() else {
            return;
        };
        let remote = dl.remote.clone();
        let dest = dl.dest().to_path_buf();
        let res = match res {
            Ok(()) => dl.finish(w.want),
            Err(e) => {
                dl.abort();
                Err(e)
            }
        };
        match res {
            Ok(()) if w.disconnected => {
                log::info!("{} landed; reconnecting", w.url);
                self.events.push(NetEvent::DownloadComplete(remote));
                self.restart_handshake();
            }
            Ok(()) => {
                self.send_reliable("wwwdl done");
                self.send_message(&[]);
                self.last_send = self.now;
                self.events.push(NetEvent::DownloadComplete(remote));
            }
            // Retail keeps such a pak and finds it wrong only at the next
            // gamestate (`CL_WWWBadChecksum`, UO CoDUOMP.exe 0x4136e0);
            // vcod deletes it first and says `chkfail`, which the server
            // takes as a failure and logs. Either way the name goes on the
            // bad-checksum list and is asked for again once the server
            // resends the gamestate (cod15-protocol-diff.md 14).
            Err(e) if e.is::<download::WrongChecksum>() => {
                log::warn!(
                    "file {remote} obtained through download redirect has wrong checksum \
                     ({e}); this likely means the server configuration is broken"
                );
                if self
                    .bad_checksums
                    .iter()
                    .any(|b| b.eq_ignore_ascii_case(&remote))
                {
                    self.drop(&format!("{remote} came back with the wrong checksum again"));
                    return;
                }
                self.bad_checksums.push(remote.clone());
                match download::Download::create(&remote, &dest) {
                    Ok(dl) => self.download = Some(dl),
                    Err(e) => {
                        self.drop(&format!("cannot retry {remote}: {e:#}"));
                        return;
                    }
                }
                self.www_retry = true;
                self.queued_download = Some(remote);
                if w.disconnected {
                    self.restart_handshake();
                } else {
                    self.send_reliable("wwwdl chkfail");
                    self.send_message(&[]);
                    self.last_send = self.now;
                }
            }
            Err(e) => {
                if !w.disconnected {
                    self.send_reliable("wwwdl fail");
                    self.send_message(&[]);
                }
                self.drop(&format!(
                    "Download failure while getting '{}': {e:#}",
                    w.url
                ));
            }
        }
    }

    /// Retail's `reconnect` after a disconnected download: the connect
    /// starts over from `getchallenge` on the protocol already settled.
    fn restart_handshake(&mut self) {
        self.www_reconnect = true;
        self.challenge = 0;
        self.netchan = Netchan::with_params(self.qport, 0, self.proto.transport.netchan);
        self.gamestate = None;
        self.snapshots.clear();
        self.configstrings = vec![String::new(); self.proto.transport.max_configstrings];
        self.server_id = 0;
        self.command_sequence = 0;
        self.commands_taken = None;
        self.last_sent_cmd = NULL_USERCMD;
        self.last_packet = None;
        self.handshake_print = None;
        self.stopdl_sent = false;
        self.send_getchallenge();
        self.state = NetState::Challenging;
        self.tries = 1;
        self.last_send = self.now;
    }

    fn on_gamestate(&mut self, gs: Gamestate) {
        self.configstrings = gs.configstrings.clone();
        self.snapshots.clear();
        self.snapshots.set_baselines(gs.baselines.clone());
        self.server_id = server_id_from_gamestate(&gs);
        self.checksum_feed = gs.checksum_feed;
        self.client_num = gs.client_num;
        self.command_sequence = gs.server_command_sequence;
        // The sequence jumps to the server's, past ring slots this client never
        // wrote; replanting the cursor keeps those out of the drain.
        self.commands_taken = None;
        self.gamestate = Some(gs);
        // A map change re-sends the gamestate on the live netchan; the cmd
        // stream starts over, so the delta base does too.
        self.last_sent_cmd = NULL_USERCMD;
        self.state = NetState::Active;
        self.last_snapshot = self.now;
        // A gamestate resent after a failed www transfer only re-asks for
        // the pak; the caller is still waiting on that download.
        let retry = std::mem::take(&mut self.www_retry);
        if !retry {
            self.events.push(NetEvent::GamestateReady);
        }
        let reconnect = std::mem::take(&mut self.www_reconnect);
        if (retry || reconnect)
            && let Some(remote) = self.queued_download.take()
        {
            self.send_reliable(&format!("download {remote}"));
        }
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
            // Drop notice, SV_DropClient cod_lnxded 0x8085cf4. CoDMP.exe
            // 0x40168b: a bare `w` is `EXE_SERVER_DISCONNECTED`, a reason
            // (a key such as EXE_TIMEDOUT, or text) fills
            // `EXE_SERVERDISCONNECTREASON`'s `%s`.
            Some("w") => match tokens.get(1) {
                Some(reason) => self.drop(&format!(
                    "EXE_SERVERDISCONNECTREASON\u{14}{}",
                    strip_colors(reason)
                )),
                None => self.drop("EXE_SERVER_DISCONNECTED"),
            },
            // Q3's spelling; no CoD server sends it.
            Some("disconnect") => self.drop("server closed the connection"),
            // `h` chat, `i` team chat (G_Say, docs/research/cod11-chat.md).
            Some("h") | Some("i") => {
                if let Some(text) = tokens.get(1) {
                    self.events.push(NetEvent::Chat {
                        text: text.clone(),
                        team: tokens[0] == "i",
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
            Some("d") => self.apply_configstring(&cmd),
            // A value of 1000 bytes or more comes in 999-byte chunks, `x`
            // opening it, `z` closing it (cod11-server-handshake.md,
            // "Configstring 1, systeminfo"; CoDMP.exe 0x4014e0, CoDUOMP.exe
            // 0x401630).
            Some("x") => {
                if let Some((i, chunk)) = parse_configstring_update(&cmd) {
                    self.big_configstring = format!("d {i} {chunk}");
                }
            }
            Some("y") | Some("z") => {
                if let Some((_, chunk)) = parse_configstring_update(&cmd) {
                    self.big_configstring.push_str(&chunk);
                }
                if tokens[0] == "z" {
                    let whole = std::mem::take(&mut self.big_configstring);
                    self.apply_configstring(&whole);
                }
            }
            _ => {
                log::debug!("serverCommand {seq}: {cmd}");
                self.events.push(NetEvent::ServerCommand(tokens));
            }
        }
    }

    /// A `d <index> <text>` configstring update.
    fn apply_configstring(&mut self, cmd: &str) {
        if let Some((i, val)) = parse_configstring_update(cmd)
            && i < self.configstrings.len()
        {
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

    /// Client message: unacked reliables, `cmds` as one `clc_move` (skipped
    /// when empty), `clc_EOF`. Each cmd is written full-branch, chained from
    /// the one before it under the same key, the first from `last_sent_cmd`.
    fn send_message(&mut self, cmds: &[UserCmd]) {
        // `CL_WritePacket` sends nothing while a demo plays.
        if self.demo.is_some() {
            return;
        }
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
            w.write_bits(CLC_CLIENT_COMMAND, CLC_BITS);
            w.write_long(seq as i32);
            w.write_string(&s);
        }
        if !cmds.is_empty() {
            let key = self.usercmd_key(message_ack, reliable_ack);
            // clc_moveNoDelta sets our deltaMessage to -1 and every snapshot back
            // is a full keyframe; once we hold a snapshot, clc_move lets the server
            // delta-compress. Keyframes on a populated server fragment and drop.
            // A recording waits on a full frame (CoDMP.exe 0x40bc5a).
            let waiting = self.recording.as_ref().is_some_and(|r| r.waiting);
            let clc = if self.snapshots.newest().is_some() && !waiting {
                CLC_MOVE
            } else {
                CLC_MOVE_NO_DELTA
            };
            w.write_bits(clc, CLC_BITS);
            w.write_byte(cmds.len() as u8);
            let mut from = self.last_sent_cmd;
            for to in cmds {
                write_delta_usercmd_in(&mut w, self.proto, key, &from, to);
                from = *to;
            }
        }
        w.write_bits(CLC_EOF, CLC_BITS);
        let ops = w.into_ops();

        if let Ok(pkt) =
            self.netchan
                .build_out(self.server_id, message_ack, reliable_ack, &ops, &self.huff)
        {
            self.transport.send(&pkt);
            if let Some(to) = cmds.last() {
                self.last_sent_cmd = *to;
            }
        }
    }

    /// `SV_UserMove`'s delta key (cod_lnxded 0x8086fa4, the mask at 0x8087060).
    fn usercmd_key(&self, message_ack: i32, reliable_ack: i32) -> i32 {
        let slot = reliable_ack as usize & (self.netchan.server_commands.len() - 1);
        let cmd = &self.netchan.server_commands[slot];
        self.checksum_feed ^ message_ack ^ com_hash_key(cmd, 32)
    }

    /// The server's clock, extrapolated from the last snapshot's `server_time`
    /// by wall-clock elapsed since it arrived.
    pub fn server_clock_ms(&self) -> i32 {
        let elapsed = self.now.duration_since(self.server_time_at).as_millis() as i32;
        self.server_time.wrapping_add(elapsed)
    }
}

/// Cmds in one `clc_move`, the cap on both ends: `send_cmds` keeps the most
/// recent this many, and the server's `SV_UserMove` parse refuses a count
/// above it.
pub const MAX_MOVE_CMDS: usize = 32;

/// `ps.delta_angles`, in usercmd angle order [pitch, yaw, roll].
pub const DELTA_ANGLE_FIELDS: [&str; 3] = ["delta_angles[0]", "delta_angles[1]", "delta_angles[2]"];

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

/// `SV_SetConfigstring`'s server commands for `index` set to `text`: one
/// `d <index> <text>` under 1000 bytes, else 999-byte chunks, the first
/// `x`, the last `z` and any between `y` (1.1 0x8089bf0, UO 0x8090da8).
pub fn configstring_commands(index: usize, text: &str) -> Vec<String> {
    const CHUNK: usize = 999;
    // A configstring holds one char per gamestate byte (`write_big_string`).
    let chars: Vec<char> = text.chars().collect();
    if chars.len() <= CHUNK {
        return vec![format!("d {index} {text}")];
    }
    chars
        .chunks(CHUNK)
        .enumerate()
        .map(|(i, chunk)| {
            let letter = match i {
                0 => 'x',
                _ if (i + 1) * CHUNK >= chars.len() => 'z',
                _ => 'y',
            };
            format!("{letter} {index} {}", chunk.iter().collect::<String>())
        })
        .collect()
}

/// Split `d <index> <text>` (or a chunk's `x`, `y`, `z`); a trailing
/// newline is the line terminator, not part of the value.
fn parse_configstring_update(cmd: &str) -> Option<(usize, String)> {
    let rest = cmd.get(1..)?.trim_start();
    let (idx, val) = rest.split_once(' ').unwrap_or((rest, ""));
    let i = idx.parse::<usize>().ok()?;
    Some((i, val.trim_end_matches('\n').to_string()))
}

/// `Com_HashKey` (cod_lnxded 0x806810c). Empty string hashes to 0.
pub fn com_hash_key(s: &str, maxlen: usize) -> i32 {
    com_hash_key_bytes(s.as_bytes(), maxlen)
}

/// [`com_hash_key`] over raw bytes, signed chars, stopping at a NUL
/// (cod2_lnxded 0x80acaa4 is the same function).
pub fn com_hash_key_bytes(s: &[u8], maxlen: usize) -> i32 {
    let mut hash: i32 = 0;
    for (i, &b) in s.iter().take(maxlen).enumerate() {
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
    strip(s, false)
}

/// [`strip_colors`], keeping the localization markers `\x14`..`\x16` of a
/// message bound for `Localized::message`.
fn strip_colors_keep_markers(s: &str) -> String {
    strip(s, true)
}

fn strip(s: &str, markers: bool) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '^' && chars.peek().is_some_and(|n| n.is_ascii_digit()) {
            chars.next();
        } else if markers && ('\u{14}'..='\u{16}').contains(&c) {
            out.push(c);
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

    /// Retail announces a map change out of band and sends no gamestate with
    /// it; the probe's capture needs the packet, so it becomes an event
    /// instead of being dropped on the floor.
    #[test]
    fn an_unhandled_oob_packet_becomes_an_event() {
        let t0 = Instant::now();
        let mut c = NetClient::start(FakeTransport::default(), t0);
        c.transport
            .incoming
            .push_back(oob("loadingnewmap", "\nmp_brecourt\ndm"));
        let ev = c.pump_at(t0);
        assert!(ev.contains(&NetEvent::OutOfBand(
            "loadingnewmap\nmp_brecourt\ndm".to_string()
        )));
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
        c.send_message(&[]);
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

    /// An rcon reply reaches a connected client's console as a print; the
    /// packet `send_oob` puts out goes on the wire untouched.
    #[test]
    fn a_print_while_connected_is_printed() {
        let mut c = active_client();
        c.send_oob(b"\xff\xff\xff\xffrcon pw status\0");
        assert_eq!(
            c.transport.sent.last().map(Vec::as_slice),
            Some(&b"\xff\xff\xff\xffrcon pw status\0"[..])
        );
        c.transport
            .incoming
            .push_back(oob("print", "\n^3Bad rconpassword.\n"));
        let ev = c.pump_at(Instant::now());
        assert!(ev.contains(&NetEvent::Print("^3Bad rconpassword.".to_string())));
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
    fn userinfo_rides_the_connect_then_a_userinfo_command() {
        let info = |name: &str, rate: &str| Userinfo {
            name: name.into(),
            rate: rate.into(),
            snaps: "20".into(),
            password: String::new(),
            www_download: "1".into(),
        };
        let t0 = Instant::now();
        let mut c = NetClient::start(FakeTransport::default(), t0);
        c.set_userinfo(info("Big \"Bob\";", "5000"));
        c.transport
            .incoming
            .push_back(oob("challengeResponse", " 7"));
        c.pump_at(t0);
        assert!(
            c.userinfo
                .contains("\\rate\\5000\\name\\Big Bob\\protocol\\")
        );

        let mut c = active_client();
        c.set_userinfo(info("Rob", "5000"));
        let sent = format!("userinfo \"{}\"", c.base_userinfo());
        assert!(sent.ends_with("\\rate\\5000\\name\\Rob\""));
        assert_eq!(reliable_count(&c, &sent), 1);
        c.set_userinfo(info("Rob", "5000"));
        assert_eq!(reliable_count(&c, &sent), 1);
        // A rate change alone resends the info, as a name change does.
        c.set_userinfo(info("Rob", "25000"));
        let sent = format!("userinfo \"{}\"", c.base_userinfo());
        assert!(sent.contains("\\rate\\25000\\"));
        assert_eq!(reliable_count(&c, &sent), 1);
        // A password rides after `cg_predictItems`; an empty one is absent.
        assert!(!c.base_userinfo().contains("password"));
        c.set_userinfo(Userinfo {
            password: "s3cret".into(),
            ..info("Rob", "25000")
        });
        assert!(
            c.base_userinfo()
                .starts_with("\\cg_predictItems\\1\\password\\s3cret\\cl_anonymous\\0")
        );
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
        let zip = crate::testing::pk3_bytes();
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
        assert!(
            c.events
                .contains(&NetEvent::DownloadComplete("main/foo.pk3".to_string()))
        );
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
    fn download_takes_blocks_past_8192() {
        let dir = std::env::temp_dir().join(format!("vcod-dl-big-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let dest = dir.join("main/foo.pk3");

        let mut c = active_client();
        c.begin_download("main/foo.pk3", &dest).unwrap();
        // One 11800-byte block, the size a public 1.1 server sends.
        let zip = crate::testing::pk3_with(&[("maps/mp/foo.bsp", &[7u8; 11672])]);
        assert_eq!(
            zip.len(),
            11800,
            "resize the entry to keep the block at 11800"
        );
        c.handle_message(&ops_message(|w| {
            w.write_byte(SVC_DOWNLOAD);
            w.write_short(0);
            w.write_long(zip.len() as i32);
            w.write_short(zip.len() as i16);
            write_data(w, &zip);
            w.write_byte(SVC_DOWNLOAD);
            w.write_short(1);
            w.write_short(0);
        }));

        assert_eq!(std::fs::read(&dest).unwrap(), zip);
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
        assert!(
            c.events
                .iter()
                .any(|e| matches!(e, NetEvent::Dropped(r) if r.contains("not found on server")))
        );
        assert!(!dest.exists());
        assert!(
            std::fs::read_dir(dir.join("main"))
                .unwrap()
                .next()
                .is_none()
        );

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
        assert!(
            c.events
                .iter()
                .any(|e| matches!(e, NetEvent::Dropped(r) if r.contains("not a valid pk3")))
        );
        assert!(!dest.exists());
        assert!(
            std::fs::read_dir(dir.join("main"))
                .unwrap()
                .next()
                .is_none()
        );

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
        assert!(
            std::fs::read_dir(dir.join("main"))
                .unwrap()
                .next()
                .is_none()
        );

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
        assert!(
            c.events
                .iter()
                .any(|e| matches!(e, NetEvent::Dropped(r) if r.contains("too large")))
        );
        assert!(!dest.exists());
        assert!(
            std::fs::read_dir(dir.join("main"))
                .unwrap()
                .next()
                .is_none()
        );

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
        assert!(
            c.events
                .contains(&NetEvent::Print("still here".to_string()))
        );
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

    /// Unscramble and decompress packet `i` the way the server would, and
    /// walk to its clc_move: returns the usercmd key, a reader positioned
    /// right after the cmd count, and the count itself.
    fn sent_move(
        c: &NetClient<FakeTransport>,
        h: &Huffman,
        i: usize,
    ) -> Option<(i32, MsgReader, u8)> {
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
            match r.read_bits(CLC_BITS) {
                CLC_MOVE | CLC_MOVE_NO_DELTA => {
                    let count = r.read_byte();
                    return Some((key, r, count));
                }
                CLC_CLIENT_COMMAND => {
                    r.read_long();
                    r.read_string();
                }
                _ => return None,
            }
        }
    }

    /// Decode packet `i`'s clc_move's first cmd against `base`.
    fn decoded_move_cmd(
        c: &NetClient<FakeTransport>,
        h: &Huffman,
        i: usize,
        base: &UserCmd,
    ) -> Option<UserCmd> {
        let (key, mut r, _count) = sent_move(c, h, i)?;
        msg::read_delta_usercmd(&mut r, key, base).ok()
    }

    /// Decode every cmd of packet `i`'s clc_move, chained from `base`.
    fn decoded_move_cmds(
        c: &NetClient<FakeTransport>,
        h: &Huffman,
        i: usize,
        base: &UserCmd,
    ) -> Option<Vec<UserCmd>> {
        let (key, mut r, count) = sent_move(c, h, i)?;
        let mut prev = *base;
        let mut out = Vec::with_capacity(count as usize);
        for _ in 0..count {
            prev = msg::read_delta_usercmd(&mut r, key, &prev).ok()?;
            out.push(prev);
        }
        Some(out)
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
    fn send_cmds_packs_several_cmds_the_server_decodes_in_order() {
        let mut c = active_client();
        let h = Huffman::new();

        let base_pkt = c.transport.sent.len();
        let cmds: Vec<UserCmd> = (0..3)
            .map(|i| UserCmd {
                server_time: 1000 + 8 * i,
                forward: 127,
                weapon: 4,
                angles: [0, 1000 * i, 0],
                ..Default::default()
            })
            .collect();
        c.send_cmds(&cmds);
        assert_eq!(c.transport.sent.len(), base_pkt + 1, "one message per call");

        let decoded = decoded_move_cmds(&c, &h, base_pkt, &NULL_USERCMD).expect("move decodes");
        assert_eq!(decoded.len(), 3);
        assert_eq!(decoded, cmds, "unrebased, unstamped, field for field");

        // More than 32 cmds keeps the last 32.
        let base_pkt = c.transport.sent.len();
        let many: Vec<UserCmd> = (0..40)
            .map(|i| UserCmd {
                server_time: 5000 + 8 * i,
                forward: 127,
                weapon: 4,
                angles: [0, i, 0],
                ..Default::default()
            })
            .collect();
        c.send_cmds(&many);
        let decoded = decoded_move_cmds(&c, &h, base_pkt, &NULL_USERCMD).expect("move decodes");
        assert_eq!(decoded.len(), 32);
        assert_eq!(decoded, many[8..], "the last 32, not the first");
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
            .push_back(oob("error", "\nEXE_SERVER_IS_DIFFERENT_VER\u{15}1.1\n"));
        let ev = c.pump_at(t0);
        assert_eq!(c.state(), NetState::Disconnected);
        assert_eq!(
            ev.first(),
            Some(&NetEvent::Dropped(
                "EXE_SERVER_IS_DIFFERENT_VER\u{15}1.1".to_string()
            ))
        );
    }

    /// A 1.5 server refuses a protocol-1 connect naming its version; the
    /// client starts over on protocol 6 and sends 1.5's userinfo.
    #[test]
    fn connect_refused_for_the_version_retries_as_that_version() {
        let t0 = Instant::now();
        let mut c = NetClient::start(FakeTransport::default(), t0);
        c.transport
            .incoming
            .push_back(oob("challengeResponse", " 7"));
        c.pump_at(t0);
        assert_eq!(c.state(), NetState::Connecting);
        let sent = c.transport.sent.len();
        c.transport
            .incoming
            .push_back(oob("error", "\nEXE_SERVER_IS_DIFFERENT_VER\u{15}1.5\n"));
        assert!(c.pump_at(t0).is_empty());
        assert_eq!(c.state(), NetState::Challenging);
        assert_eq!(c.protocol().transport.version, 6);
        assert_eq!(
            c.transport.sent[sent],
            connectionless::build_oob("getchallenge")
        );

        c.transport
            .incoming
            .push_back(oob("challengeResponse", " 8"));
        c.pump_at(t0);
        assert_eq!(c.state(), NetState::Connecting);
        assert!(c.userinfo.starts_with(
            "\\cg_predictItems\\1\\cl_punkbuster\\0\\cl_guid\\unknown\\cl_anonymous\\0"
        ));
        assert!(c.userinfo.contains("\\protocol\\6\\"));

        // Refused again by its own version: a real refusal, not a switch.
        c.transport
            .incoming
            .push_back(oob("error", "\nEXE_SERVER_IS_DIFFERENT_VER\u{15}1.5\n"));
        c.pump_at(t0);
        assert_eq!(c.state(), NetState::Disconnected);
    }

    /// A UO install opens on protocol 22 and drops on a 1.1 server's
    /// refusal instead of switching; a 1.1 install drops on a UO one's.
    #[test]
    fn a_server_of_another_game_ends_the_handshake() {
        let t0 = Instant::now();
        for (install, server, why) in [
            (
                &protocol::PROTOCOL_V22,
                "1.1",
                "This is a CoD 1.1 server: run vcod without --mod-dir uo",
            ),
            (
                &protocol::PROTOCOL_V1,
                "1.51",
                "This is a United Offensive server: run vcod with --mod-dir uo",
            ),
        ] {
            let mut c = NetClient::start_from(FakeTransport::default(), t0, install);
            assert_eq!(c.protocol().transport.version, install.transport.version);
            c.transport
                .incoming
                .push_back(oob("challengeResponse", " 7"));
            c.pump_at(t0);
            c.transport.incoming.push_back(oob(
                "error",
                &format!("\nEXE_SERVER_IS_DIFFERENT_VER\u{15}{server}\n"),
            ));
            assert_eq!(c.pump_at(t0), vec![NetEvent::Dropped(why.to_string())]);
            assert_eq!(c.state(), NetState::Disconnected);
        }
    }

    /// 1.1 and 1.5 still cross over from an install's connect.
    #[test]
    fn an_install_switches_between_1_1_and_1_5() {
        let t0 = Instant::now();
        let mut c = NetClient::start_from(FakeTransport::default(), t0, &protocol::PROTOCOL_V6);
        c.transport
            .incoming
            .push_back(oob("challengeResponse", " 7"));
        c.pump_at(t0);
        c.transport
            .incoming
            .push_back(oob("error", "\nEXE_SERVER_IS_DIFFERENT_VER\u{15}1.1\n"));
        c.pump_at(t0);
        assert_eq!(c.state(), NetState::Challenging);
        assert_eq!(c.protocol().transport.version, 1);
    }

    /// A PunkBuster server refuses a client without it in one `error`, the
    /// text pbsv.dll wrote (CoDMP.exe 1.5 under wine, `sv_punkbuster 1`,
    /// 2026-10-13); vcod drops with it, as it never claims PunkBuster.
    #[test]
    fn a_punkbuster_refusal_drops_with_the_servers_text() {
        let t0 = Instant::now();
        let mut c = NetClient::start_with_protocol(
            FakeTransport::default(),
            t0,
            0x2001,
            &protocol::PROTOCOL_V6,
        );
        c.transport
            .incoming
            .push_back(oob("challengeResponse", " 7"));
        c.pump_at(t0);
        assert!(c.userinfo.contains("\\cl_punkbuster\\0\\"));
        let why = "PunkBuster must be Enabled before joining this server. PunkBuster can be \
                   Enabled/Disabled in the Multiplayer Options menu. Visit www.evenbalance.com \
                   for PunkBuster support.";
        c.transport
            .incoming
            .push_back(oob("error", &format!("\n{why}\n")));
        assert_eq!(c.pump_at(t0), vec![NetEvent::Dropped(why.to_string())]);
        assert_eq!(c.state(), NetState::Disconnected);
    }

    /// `CL_DisconnectPacket` drops only once the server has been silent for
    /// 3 s since `connectResponse` or its last sequenced packet.
    #[test]
    fn oob_disconnect_waits_out_the_guard() {
        let t0 = Instant::now();
        let mut c = NetClient::start(FakeTransport::default(), t0);
        c.transport
            .incoming
            .push_back(oob("challengeResponse", " 42"));
        c.pump_at(t0);
        c.transport.incoming.push_back(oob("connectResponse", ""));
        c.pump_at(t0);
        c.transport.incoming.push_back(oob("disconnect", ""));
        let ev = c.pump_at(t0 + Duration::from_millis(2999));
        assert_eq!(c.state(), NetState::LoadingGamestate);
        assert!(!ev.iter().any(|e| matches!(e, NetEvent::Dropped(_))));
        // Any sequenced packet restarts it, before the netchan's checks.
        c.transport.incoming.push_back(vec![9, 0, 0, 0, 0, 0, 0, 0]);
        c.pump_at(t0 + Duration::from_millis(2000));
        c.transport.incoming.push_back(oob("disconnect", ""));
        c.pump_at(t0 + Duration::from_millis(4999));
        assert_eq!(c.state(), NetState::LoadingGamestate);
        c.transport.incoming.push_back(oob("disconnect", ""));
        let ev = c.pump_at(t0 + Duration::from_millis(5000));
        assert_eq!(c.state(), NetState::Disconnected);
        assert_eq!(
            ev.first(),
            Some(&NetEvent::Dropped("EXE_SERVER_DISCONNECTED".to_string()))
        );

        // Before `connectResponse` nothing has stamped the clock.
        let mut c = NetClient::start(FakeTransport::default(), t0);
        c.transport.incoming.push_back(oob("disconnect", ""));
        c.pump_at(t0);
        assert_eq!(c.state(), NetState::Disconnected);
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
            .push_back(oob("print", "\n^1Welcome to Sample TDM\n"));
        let ev = c.pump_at(t0);
        assert_eq!(c.state(), NetState::Connecting);
        assert_eq!(
            ev.first(),
            Some(&NetEvent::Print("Welcome to Sample TDM".to_string()))
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
    fn needcdkey_without_a_key_says_so() {
        let t0 = Instant::now();
        let mut c = NetClient::start(FakeTransport::default(), t0);
        c.cd_key = None;
        c.transport.incoming.push_back(oob("needcdkey", ""));
        let ev = c.pump_at(t0);
        assert_eq!(c.state(), NetState::Disconnected);
        assert_eq!(
            ev.first(),
            Some(&NetEvent::Dropped("EXE_ERR_INVALID_CD_KEY".to_string()))
        );
    }

    #[test]
    fn needcdkey_with_a_key_keeps_challenging() {
        let t0 = Instant::now();
        let mut c = NetClient::start(FakeTransport::default(), t0);
        c.cd_key = cdkey::CdKey::parse("AAAA-AAAA-AAAA-AAAA");
        c.transport.incoming.push_back(oob("needcdkey", ""));
        c.pump_at(t0);
        assert_eq!(c.state(), NetState::Challenging);
        // The fake transport has no address, so nothing goes past it.
        assert!(
            c.transport
                .sent
                .iter()
                .all(|p| last_oob_of(p) == "getchallenge")
        );
        for i in 1..=6 {
            let ev = c.pump_at(t0 + Duration::from_secs(2 * i));
            if let Some(NetEvent::Dropped(r)) = ev.first() {
                assert_eq!(r, "EXE_AWAITINGCDKEYAUTH");
                return;
            }
        }
        panic!("never dropped");
    }

    /// Over real UDP on loopback, a server that checks no key answers the
    /// same handshake with a key set as without: the key stays home.
    #[test]
    fn a_key_leaves_a_loopback_join_unchanged() {
        let run = |key: Option<cdkey::CdKey>| {
            let server = UdpSocket::bind("127.0.0.1:0").unwrap();
            server
                .set_read_timeout(Some(Duration::from_millis(500)))
                .unwrap();
            let addr = server.local_addr().unwrap().to_string();
            let t0 = Instant::now();
            let mut c = NetClient::start(UdpTransport::connect(&addr).unwrap(), t0);
            c.cd_key = key;
            // The first `getchallenge` went out before the key was set:
            // send it again with this one.
            c.send_getchallenge();
            let mut seen = Vec::new();
            let mut buf = [0u8; 2048];
            while c.state() != NetState::LoadingGamestate && seen.len() < 6 {
                let (n, from) = server.recv_from(&mut buf).unwrap();
                let cmd = last_oob_of(&buf[..n]);
                let reply = match cmd.as_str() {
                    "getchallenge" => "challengeResponse 7",
                    "connect" => "connectResponse",
                    _ => "",
                };
                seen.push(cmd);
                server.send_to(&oob(reply, ""), from).unwrap();
                for _ in 0..50 {
                    c.pump_at(Instant::now());
                    if c.state() != NetState::Challenging {
                        break;
                    }
                    std::thread::sleep(Duration::from_millis(2));
                }
            }
            assert_eq!(c.state(), NetState::LoadingGamestate);
            seen
        };
        let without = run(None);
        let with = run(cdkey::CdKey::parse("AAAA-AAAA-AAAA-AAAA"));
        assert_eq!(without, with);
        assert_eq!(with.last().map(String::as_str), Some("connect"));
    }

    fn last_oob_of(p: &[u8]) -> String {
        connectionless::parse_oob(p).map_or(String::new(), |(c, _)| c.to_string())
    }

    #[test]
    fn tokenize_handles_quotes() {
        assert_eq!(tokenize("cs 5 \"\\a\\b\""), vec!["cs", "5", "\\a\\b"]);
        assert_eq!(tokenize("disconnect"), vec!["disconnect"]);
    }

    #[test]
    fn take_server_commands_drains_verbatim_and_only_once() {
        let t0 = Instant::now();
        let mut c = NetClient::start(FakeTransport::default(), t0);

        // The first call sets the cursor, so what arrived before it is not
        // replayed; consumed commands come back like any other.
        c.handle_server_command(1, "v g_scriptMainMenu \"team_americangerman\"".to_string());
        assert_eq!(c.take_server_commands(), Vec::<String>::new());

        c.handle_server_command(2, "t 0".to_string());
        c.handle_server_command(3, "d 3 n\\ambient\\t\\0".to_string());
        assert_eq!(
            c.take_server_commands(),
            vec!["t 0".to_string(), "d 3 n\\ambient\\t\\0".to_string()]
        );
        assert!(c.take_server_commands().is_empty());
    }

    #[test]
    fn take_server_commands_replants_the_cursor_at_the_gamestate() {
        let t0 = Instant::now();
        let mut c = NetClient::start(FakeTransport::default(), t0);
        // The probe drains every loop iteration, so the cursor is planted at 0
        // before the gamestate jumps the sequence to the server's.
        assert!(c.take_server_commands().is_empty());

        c.on_gamestate(Gamestate {
            configstrings: vec![String::new(); PROTOCOL_V1.transport.max_configstrings],
            baselines: Default::default(),
            client_num: 0,
            checksum_feed: 0,
            server_command_sequence: 7,
        });
        // Without the replant these are ring slots this client never wrote,
        // and they come back as seven empty strings.
        assert!(c.take_server_commands().is_empty());

        c.handle_server_command(8, "t 0".to_string());
        assert_eq!(c.take_server_commands(), vec!["t 0".to_string()]);
    }

    #[test]
    fn server_commands_map_to_events() {
        let t0 = Instant::now();
        let mut c = NetClient::start(FakeTransport::default(), t0);

        c.handle_server_command(1, "h \"^1hi ^7there\"".to_string());
        assert_eq!(
            c.events,
            vec![NetEvent::Chat {
                text: "^1hi ^7there".to_string(),
                team: false
            }]
        );
        // Stored verbatim in the XOR key ring at seq & 63.
        assert_eq!(c.netchan.server_commands[1], "h \"^1hi ^7there\"");
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
            vec![NetEvent::Dropped(
                "EXE_SERVERDISCONNECTREASON\u{14}EXE_TIMEDOUT".to_string()
            )]
        );
        assert_eq!(c.state(), NetState::Disconnected);
        let mut c = NetClient::start(FakeTransport::default(), t0);
        c.handle_server_command(1, "w".to_string());
        assert_eq!(
            c.events,
            vec![NetEvent::Dropped("EXE_SERVER_DISCONNECTED".to_string())]
        );
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

    /// A systeminfo past 1000 bytes (UO's pure one runs to 1350) goes out
    /// as `x`, `y`, `z` chunks and comes back whole, a space at a chunk's
    /// edge included.
    #[test]
    fn a_chunked_configstring_comes_back_whole() {
        let info = format!("\\sv_paks\\{}\\sv_serverid\\7", "1 23 ".repeat(450));
        let cmds = configstring_commands(1, &info);
        let letters: String = cmds.iter().map(|c| &c[..1]).collect();
        assert_eq!(letters, "xyz");
        assert!(cmds.iter().all(|c| c.len() < 1010));
        assert_eq!(configstring_commands(1, "\\a\\b"), vec!["d 1 \\a\\b"]);
        assert_eq!(configstring_commands(1, &"a".repeat(999)).len(), 1);
        assert_eq!(configstring_commands(1, &"a".repeat(1000)).len(), 2);

        let t0 = Instant::now();
        let mut c = NetClient::start(FakeTransport::default(), t0);
        for (seq, cmd) in cmds.into_iter().enumerate() {
            c.handle_server_command(seq as i32 + 1, cmd);
        }
        assert_eq!(c.configstrings[1], info);
        assert_eq!(c.server_id, 7);
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
        c.handle_server_command(
            8,
            "i \"\u{15}(\u{14}GAME_AXIS\u{15})Bob^7: ^7go go\"".into(),
        );
        assert_eq!(
            c.events.last().unwrap(),
            &NetEvent::Chat {
                text: "\u{15}(\u{14}GAME_AXIS\u{15})Bob^7: ^7go go".into(),
                team: true
            }
        );
    }

    /// Retail's www redirect messages, decoded: the UO 1.51b server with
    /// `sv_wwwDlDisconnected` 0 and 1, and the 1.5b server with 0
    /// (tmp-captured off `--probe-download`, recipe in
    /// docs/research/cod15-protocol-diff.md, "HTTP downloads").
    #[test]
    fn retail_www_redirects_decode() {
        let url = "http://127.0.0.1:31072/main/nsf_mp_j5outpost.pk3";
        let h = Huffman::new();
        for (file, flags) in [
            ("net/www-redirect-uo.bin", 0),
            ("net/www-redirect-uo-discon.bin", WWW_FLAG_DISCON),
            ("net/www-redirect-1.5.bin", 0),
        ] {
            let msg = crate::testing::fixture(file);
            let mut r = MsgReader::new(&msg[4..], &h);
            assert_eq!(r.read_byte(), SVC_DOWNLOAD, "{file}");
            assert_eq!(r.read_short(), -1, "{file}");
            assert_eq!(
                read_www_redirect(&mut r),
                WwwRedirect {
                    url: url.into(),
                    size: 953499,
                    flags
                },
                "{file}"
            );
            assert_eq!(r.read_byte(), SVC_EOF, "{file}");
            assert!(!r.is_overflowed(), "{file}");
        }
    }

    #[test]
    fn the_www_userinfo_key_rides_protocols_6_and_22_only() {
        let mut c = active_client();
        assert!(!c.base_userinfo().contains("cl_wwwDownload"));
        c.proto = &protocol::PROTOCOL_V22;
        assert!(
            c.base_userinfo()
                .ends_with("\\name\\vcod\\cl_wwwDownload\\1")
        );
    }

    /// One HTTP response to one connection on a loopback port.
    fn http_once(body: Vec<u8>) -> u16 {
        use std::io::{Read, Write};
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        std::thread::spawn(move || {
            let (mut s, _) = l.accept().unwrap();
            let mut req = [0u8; 1024];
            let _ = s.read(&mut req);
            let head = format!("HTTP/1.0 200 OK\r\nContent-Length: {}\r\n\r\n", body.len());
            s.write_all(head.as_bytes()).unwrap();
            s.write_all(&body).unwrap();
        });
        port
    }

    fn redirect(url: &str, size: usize, flags: i32) -> Vec<u8> {
        ops_message(|w| {
            w.write_byte(SVC_DOWNLOAD);
            w.write_short(-1);
            w.write_string(url);
            w.write_long(size as i32);
            w.write_long(flags);
        })
    }

    /// Pump until the HTTP thread is done.
    fn wait_www(c: &mut NetClient<FakeTransport>) -> Vec<NetEvent> {
        let deadline = Instant::now() + Duration::from_secs(10);
        while c.www.is_some() {
            assert!(Instant::now() < deadline, "www download hung");
            std::thread::sleep(Duration::from_millis(5));
            c.tick();
        }
        std::mem::take(&mut c.events)
    }

    #[test]
    fn a_www_redirect_fetches_over_http_and_acks() {
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("main/foo.pk3");
        let zip = crate::testing::pk3_bytes();
        let port = http_once(zip.clone());

        let mut c = active_client();
        c.proto = &protocol::PROTOCOL_V22;
        c.begin_download("main/foo.pk3", &dest).unwrap();
        let url = format!("http://127.0.0.1:{port}/main/foo.pk3");
        c.handle_message(&redirect(&url, zip.len(), 0));
        assert_eq!(reliable_count(&c, "wwwdl ack"), 1);
        assert_eq!(c.download_progress().map(|p| p.1), Some(zip.len() as u32));
        // The server repeats it until the ack lands, and 1.5b streams blocks
        // meanwhile; neither touches the transfer.
        c.handle_message(&redirect(&url, zip.len(), 0));
        assert_eq!(reliable_count(&c, "wwwdl ack"), 1);
        c.handle_message(&ops_message(|w| {
            w.write_byte(SVC_DOWNLOAD);
            w.write_short(0);
            w.write_long(4);
            w.write_short(4);
            write_data(w, b"junk");
        }));
        assert_eq!(reliable_count(&c, "nextdl 0"), 0);

        let ev = wait_www(&mut c);
        assert_eq!(ev, [NetEvent::DownloadComplete("main/foo.pk3".into())]);
        assert_eq!(std::fs::read(&dest).unwrap(), zip);
        assert_eq!(reliable_count(&c, "wwwdl done"), 1);
        assert_eq!(c.state(), NetState::Active);
        c.finish_downloads();
        assert_eq!(reliable_count(&c, "donedl"), 1);
    }

    #[test]
    fn a_disconnected_www_download_survives_the_drop_and_reconnects() {
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("main/foo.pk3");
        let zip = crate::testing::pk3_bytes();
        let port = http_once(zip.clone());

        let mut c = active_client();
        c.proto = &protocol::PROTOCOL_V22;
        c.begin_download("main/foo.pk3", &dest).unwrap();
        let url = format!("http://127.0.0.1:{port}/main/foo.pk3");
        c.handle_message(&redirect(&url, zip.len(), WWW_FLAG_DISCON));
        assert_eq!(reliable_count(&c, "wwwdl ack"), 1);
        assert_eq!(reliable_count(&c, "wwwdl bbl8r"), 1);
        // The drop `bbl8r` asks for.
        c.handle_server_command(
            c.command_sequence + 1,
            "w \"GMI_EXE_DOWNLOADDISCONNECTED\"".into(),
        );
        assert_eq!(c.state(), NetState::Disconnected);
        assert!(c.events.is_empty());

        let ev = wait_www(&mut c);
        assert_eq!(ev, [NetEvent::DownloadComplete("main/foo.pk3".into())]);
        assert_eq!(std::fs::read(&dest).unwrap(), zip);
        assert_eq!(c.state(), NetState::Challenging);
        assert_eq!(
            c.transport.sent.last().unwrap(),
            &connectionless::build_oob("getchallenge")
        );
        // The reconnect brings the gamestate; a second pak waits for it.
        c.finish_downloads();
        assert_eq!(c.state(), NetState::Challenging);
        c.begin_download("main/bar.pk3", &dir.path().join("main/bar.pk3"))
            .unwrap();
        assert_eq!(reliable_count(&c, "download main/bar.pk3"), 0);
        // The fixture is a 1.1 gamestate.
        c.proto = &PROTOCOL_V1;
        c.state = NetState::LoadingGamestate;
        c.handle_message(&crate::testing::fixture("net/gamestate.bin"));
        assert_eq!(c.state(), NetState::Active);
        assert_eq!(reliable_count(&c, "download main/bar.pk3"), 1);
    }

    #[test]
    fn a_failed_www_download_says_wwwdl_fail_and_drops() {
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("main/foo.pk3");
        // Garbage instead of a zip.
        let port = http_once(b"not a zip".to_vec());
        let mut c = active_client();
        c.proto = &protocol::PROTOCOL_V6;
        c.begin_download("main/foo.pk3", &dest).unwrap();
        c.handle_message(&redirect(
            &format!("http://127.0.0.1:{port}/main/foo.pk3"),
            9,
            0,
        ));
        let ev = wait_www(&mut c);
        assert_eq!(reliable_count(&c, "wwwdl fail"), 1);
        assert!(matches!(&ev[..], [NetEvent::Dropped(r)] if r.contains("not a valid pk3")));
        assert!(!dest.exists());
    }

    /// A client mid-download whose redirect brought `main/foo.pk3` with a
    /// checksum other than the one listed: `chkfail` went out.
    fn after_chkfail(dest: &std::path::Path) -> NetClient<FakeTransport> {
        let zip = crate::testing::pk3_bytes();
        let port = http_once(zip.clone());
        let mut c = active_client();
        c.proto = &protocol::PROTOCOL_V22;
        c.configstrings[1] = "\\sv_referencedPakNames\\main/foo\\sv_referencedPaks\\1234 ".into();
        c.begin_download("main/foo.pk3", dest).unwrap();
        c.handle_message(&redirect(
            &format!("http://127.0.0.1:{port}/main/foo.pk3"),
            zip.len(),
            0,
        ));
        let ev = wait_www(&mut c);
        assert_eq!(reliable_count(&c, "wwwdl chkfail"), 1);
        assert_eq!(reliable_count(&c, "wwwdl done"), 0);
        assert!(ev.is_empty(), "{ev:?}");
        assert!(!dest.exists());
        c
    }

    /// The gamestate the server resends after `chkfail` or `fail`.
    fn resent_gamestate(c: &mut NetClient<FakeTransport>) {
        let proto = c.proto;
        c.proto = &PROTOCOL_V1; // the fixture is a 1.1 gamestate
        c.handle_message(&crate::testing::fixture("net/gamestate.bin"));
        c.proto = proto;
        let ev = std::mem::take(&mut c.events);
        assert!(!ev.contains(&NetEvent::GamestateReady), "{ev:?}");
    }

    #[test]
    fn a_www_pak_with_the_wrong_checksum_is_asked_for_again_over_udp() {
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("main/foo.pk3");
        let mut c = after_chkfail(&dest);
        // The resent gamestate re-asks for the pak and stays inside the net
        // client: the caller is still waiting on its download.
        resent_gamestate(&mut c);
        assert_eq!(reliable_count(&c, "download main/foo.pk3"), 2);
        // 1.5b's fallback flag sends it over UDP.
        let zip = crate::testing::pk3_bytes();
        c.handle_message(&ops_message(|w| {
            w.write_byte(SVC_DOWNLOAD);
            w.write_short(0);
            w.write_long(zip.len() as i32);
            w.write_short(zip.len() as i16);
            write_data(w, &zip);
            w.write_byte(SVC_DOWNLOAD);
            w.write_short(1);
            w.write_short(0);
        }));
        assert_eq!(
            c.events,
            [NetEvent::DownloadComplete("main/foo.pk3".into())]
        );
        assert_eq!(std::fs::read(&dest).unwrap(), zip);
    }

    #[test]
    fn a_disconnected_www_pak_with_the_wrong_checksum_reconnects_and_asks_again() {
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("main/foo.pk3");
        let zip = crate::testing::pk3_bytes();
        let port = http_once(zip.clone());
        let mut c = active_client();
        c.proto = &protocol::PROTOCOL_V22;
        c.configstrings[1] = "\\sv_referencedPakNames\\main/foo\\sv_referencedPaks\\1234 ".into();
        c.begin_download("main/foo.pk3", &dest).unwrap();
        c.handle_message(&redirect(
            &format!("http://127.0.0.1:{port}/main/foo.pk3"),
            zip.len(),
            WWW_FLAG_DISCON,
        ));
        c.handle_server_command(
            c.command_sequence + 1,
            "w \"GMI_EXE_DOWNLOADDISCONNECTED\"".into(),
        );
        let ev = wait_www(&mut c);
        assert!(ev.is_empty(), "{ev:?}");
        assert!(!dest.exists());
        assert_eq!(c.state(), NetState::Challenging);
        c.state = NetState::LoadingGamestate;
        let before = reliable_count(&c, "download main/foo.pk3");
        resent_gamestate(&mut c);
        assert_eq!(reliable_count(&c, "download main/foo.pk3"), before + 1);
        assert_eq!(reliable_count(&c, "wwwdl chkfail"), 0);
    }

    #[test]
    fn a_redirect_of_a_bad_checksum_pak_is_refused_once_then_left() {
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("main/foo.pk3");
        let mut c = after_chkfail(&dest);
        resent_gamestate(&mut c);
        // UO redirects the same name again: ack, then refuse.
        let msg = redirect("http://127.0.0.1:1/main/foo.pk3", 10, 0);
        c.handle_message(&msg);
        assert_eq!(reliable_count(&c, "wwwdl ack"), 2);
        assert_eq!(reliable_count(&c, "wwwdl fail"), 1);
        assert!(c.www.is_none());
        // Its repeats before the ack lands are eaten.
        c.handle_message(&msg);
        assert_eq!(reliable_count(&c, "wwwdl ack"), 2);
        assert!(c.events.is_empty(), "{:?}", c.events);
        resent_gamestate(&mut c);
        assert_eq!(reliable_count(&c, "download main/foo.pk3"), 3);
        // UO never falls back to UDP: a second redirect is the end.
        c.handle_message(&msg);
        assert!(
            matches!(&c.events[..], [NetEvent::Dropped(r)] if r.contains("keeps redirecting")),
            "{:?}",
            c.events
        );
    }

    #[test]
    fn an_unexpected_block_ffff_drops_with_a_reason() {
        let dir = tempfile::tempdir().unwrap();
        let msg = redirect("http://127.0.0.1:1/main/foo.pk3", 10, 0);
        // Protocol 1 has no redirect.
        let mut c = active_client();
        c.begin_download("main/foo.pk3", &dir.path().join("main/foo.pk3"))
            .unwrap();
        c.handle_message(&msg);
        assert!(matches!(&c.events[..], [NetEvent::Dropped(r)] if r.contains("protocol 1")));
        // Nothing asked for.
        let mut c = active_client();
        c.proto = &protocol::PROTOCOL_V22;
        c.handle_message(&msg);
        assert!(matches!(&c.events[..], [NetEvent::Dropped(r)] if r.contains("never asked")));
        // A page to open in a browser.
        let mut c = active_client();
        c.proto = &protocol::PROTOCOL_V22;
        c.begin_download("main/foo.pk3", &dir.path().join("main/foo.pk3"))
            .unwrap();
        c.handle_message(&redirect("http://127.0.0.1:1/x", 10, WWW_FLAG_URL));
        assert_eq!(reliable_count(&c, "wwwdl bbl8r"), 1);
        assert!(matches!(&c.events[..], [NetEvent::Dropped(r)] if r.contains("web browser")));
    }

    /// A demo made of retail's own messages, the committed gamestate and
    /// snapshot captures in the order they arrived, then the trailer.
    fn retail_demo(gs: &str, snaps: &str) -> Vec<u8> {
        let gs = crate::testing::fixture(gs);
        let snaps = crate::testing::fixture(snaps);
        let first = u32::from_le_bytes(snaps[..4].try_into().unwrap());
        let mut w = demo::DemoWriter::new(Vec::new());
        w.write_message(first - 1, &gs).unwrap();
        let mut out = w.finish().unwrap();
        // The captures are already `[sequence][length][message]` records.
        out.truncate(out.len() - 8);
        out.extend_from_slice(&snaps);
        out.extend_from_slice(&[0xff; 8]);
        out
    }

    /// A played demo's messages ack the recorder's reliable commands, past
    /// the playback's own sequence of 0: a command queued then (the `cp`
    /// after the load) is no overflow. A client whose server stops acking
    /// still drops once the ring is full.
    #[test]
    fn a_demo_ack_past_the_sequence_is_no_overflow() {
        let now = Instant::now();
        let demo = retail_demo("net/gamestate.bin", "net/snapshots.bin");
        let mut c = NetClient::play_demo(FakeTransport::default(), now, &PROTOCOL_V1, demo);
        assert!(c.pump_demo(now, None).contains(&NetEvent::GamestateReady));
        // The first snapshot acks the recorder's command 1.
        c.pump_demo(now, Some(i32::MIN));
        assert!(c.netchan.reliable_acknowledge > c.netchan.reliable_sequence);
        c.send_reliable("cp 1 2");
        assert!(c.events.iter().all(|e| !matches!(e, NetEvent::Dropped(_))));
        assert_ne!(c.state(), NetState::Disconnected);

        let mut c = active_client();
        let len = c.netchan.reliable.len() as u32;
        let ack = c.netchan.reliable_acknowledge;
        while c.netchan.reliable_sequence.wrapping_sub(ack) < len {
            c.send_reliable("say x");
            assert_ne!(c.state(), NetState::Disconnected);
        }
        c.send_reliable("say x");
        assert_eq!(c.state(), NetState::Disconnected);
    }

    /// Every protocol's retail capture played as a demo parses to the
    /// frames the snapshot gates parse it to, frame for frame, and ends
    /// on the trailer.
    #[test]
    fn a_demo_of_retail_messages_plays_every_frame() {
        use protocol::{PROTOCOL_V6, PROTOCOL_V22};
        for (tag, p) in [
            ("", &PROTOCOL_V1),
            ("-delta", &PROTOCOL_V1),
            ("-1.5", &PROTOCOL_V6),
            ("-uo", &PROTOCOL_V22),
            ("-uo-jeep", &PROTOCOL_V22),
        ] {
            let (gs, snaps) = (
                format!("net/gamestate{tag}.bin"),
                format!("net/snapshots{tag}.bin"),
            );
            let mut want = Vec::new();
            snapshot::tests::regate_as(p, &gs, &snaps, false, |s| {
                want.push(format!("{s:?}"));
            });
            let now = Instant::now();
            let mut c =
                NetClient::play_demo(FakeTransport::default(), now, p, retail_demo(&gs, &snaps));
            assert!(c.pump_demo(now, None).contains(&NetEvent::GamestateReady));
            let mut got = Vec::new();
            let mut ended = false;
            while !ended {
                let until = c.snapshots().newest().map_or(i32::MIN, |s| s.server_time);
                let evs = c.pump_demo(now, Some(until));
                ended = evs.contains(&NetEvent::DemoEnded(demo::DemoEnd::Completed));
                if let Some(s) = c.snapshots().newest()
                    && s.server_time > until
                {
                    got.push(format!("{s:?}"));
                }
            }
            assert_eq!(got.len(), want.len(), "{tag}: frames");
            for (i, (g, w)) in got.iter().zip(&want).enumerate() {
                assert_eq!(g, w, "{tag}: frame {i}");
            }
            assert_eq!(c.state(), NetState::Disconnected);
            assert!(c.transport.sent.is_empty(), "{tag}: a demo sent a packet");
        }
    }
}
