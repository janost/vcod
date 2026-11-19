//! A CoD2 1.3 connection (protocol 118, docs/research/cod2-protocol.md): the
//! socket, the handshake, the wire state ([`vcod_cod2::net::Client`]) and
//! the snapshot view the rest of the client draws from. CoD1's
//! `net::NetClient` path is not involved.
//!
//! The interface for whatever draws a CoD2 server's world:
//!
//! - `App::session`'s `Cod2Client` holds the session while `--connect` runs on a CoD2
//!   install; the mode is `Mode::Cod2`, the camera placed each frame by
//!   [`Cod2Session::fly`]. From the gamestate to the first redraw, and
//!   through a renderer build, the session runs on its own thread
//!   ([`Cod2Session::pump`]) so a slow load does not time it out.
//! - [`Cod2Session::frame`] is this render frame's
//!   [`vcod_cod2::net::Frame`]: `snap` (the newest
//!   [`vcod_cod2::net::SnapshotView`]: `entities`, `ps`, `clients`), `prev`,
//!   `time_ms` and `frac` between them, and `configstrings`
//!   ([`vcod_cod2::net::ConfigStrings`]). `crate::cod2_scene::Cod2Draw`
//!   draws it: the entities and bodies, placed by `frame.entity_place(e)`.
//!   Everything is in CoD2's own numbering (`vcod_cod2::net::{events,
//!   flags}`).
//! - After each gamestate the `cp` a pure server checks is queued before
//!   any cmd (cod2-protocol.md 11.4).
//! - A gamestate for another map (`map`, `map_rotate`) is the caller's to
//!   load: [`Cod2Session::take_map_change`] names it once, and the caller
//!   reads it under [`Cod2Session::pump`]. One for the same map (`map_restart`)
//!   and a `fast_restart`'s `B` / `n` (a script `map_restart`, an sd round)
//!   keep the map; [`Cod2Session::take_restart`] reports the second
//!   (cod2-protocol.md 1.8, 1.9).
//! - With a [`Join`] the session answers the stock script menus as a
//!   player does (`mr <serverId> <menu> <response>`): the server info's
//!   `close`, the team, the weapon.
//! - [`Cod2Session::fly`] builds one cmd every [`super::cmds::CMD_MS`] of
//!   the client clock ([`super::clock::ServerClock`], `CL_AdjustTimeDelta`)
//!   from [`Cod2Session::input`] (`play::cod2_input`, fed by the console
//!   binds) and the camera's view, keeps every cmd sent, and predicts the
//!   local body off the newest snapshot over the map's collision world
//!   (`play::cod2_predict`). [`Cod2Session::frame`] then carries the
//!   predicted playerstate in `snap.ps`, and [`Cod2Session::view`] is the
//!   eye and angles the camera draws from. The move's own events come off
//!   the prediction, once each, in [`Cod2Session::take_events`].
//! - [`Cod2Session::connect_in_background`] joins from the front end
//!   without blocking the window.
//! - [`Cod2Session::play_demo`] reads a `.dm_1` instead of a socket
//!   (docs/research/cod2-demos.md): the same parse, view, events and HUD,
//!   paced by `CL_SetCGameTime`'s demo arm, nothing predicted or sent;
//!   [`Cod2Session::start_recording`] writes one off a live session.
//! - [`Cod2Session::dropped`] holds why the server let us go: a `w`
//!   command (kick, server shutdown), an OOB `error`, an OOB `disconnect`
//!   honoured past 3 s of silence, or [`TIMEOUT`] without a message
//!   (1.10).

use std::net::UdpSocket;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use anyhow::{Context, bail};
use glam::Vec3;
use std::path::{Path, PathBuf};
use vcod_cod2::net::download::{DownloadEvent, Downloads, Progress};
use vcod_cod2::net::oob::{self, Reply};
use vcod_cod2::net::protocol::CS_LAYOUT_118;
use vcod_cod2::net::{Client, Frame, NetView, UserCmd2};
use vcod_common::asset_source::{Asset, AssetSource};
use vcod_common::net::cdkey::{self, CdKey, KeyGame};
use vcod_common::net::{Transport, UdpTransport};
use vcod_common::pk3::Pk3Fs;

use crate::audio::cues::Cue;
use crate::camera::FlyCamera;
use crate::console::shell::Effect;
use crate::play::clock::ServerClock;
use crate::play::cmds::CmdClock;
use crate::play::cod2_events::{Event2, EventTracker};
use crate::play::cod2_fx::{Cod2Fx, FxPlay};
use crate::play::cod2_input::Cod2Input;
use crate::play::cod2_net::{Lagometer, LerpStats, NetMeter};
use crate::play::cod2_predict::{CMD_BACKUP, Cod2Predictor, Predicted2};
use crate::play::cod2_recoil::Cod2Recoil;
use crate::play::cod2_sound::{Cod2Sounds, LoopSounds, SoundCommand};
use crate::play::cod2_weapons::LevelWeapons;
use crate::play::input::Action;
use std::collections::{HashMap, VecDeque};
use vcod_cod2::net::{PlayerView, SnapshotView};
use vcod_common::collision::CollisionWorld;
use vcod_common::net::protocol::CsRange;

mod connect;
mod demo;
use demo::DemoPlay;
mod probe;
pub use connect::{ConnectOpts, Connecting, Join, home_of};
pub use probe::{demo_probe, probe};

/// Now for a session's waits, send pacing and timeouts: the wall's, or the
/// clock a live test shares with its in-process server on this thread
/// (`play::cod4::tests::sim`, the CoD2 server through `tests::run_held`).
pub(crate) fn clock_now() -> Instant {
    #[cfg(test)]
    if let Some(t) = crate::play::cod4::tests::sim::now() {
        return t;
    }
    Instant::now()
}

/// [`Instant::elapsed`] on [`clock_now`].
fn since(t: Instant) -> Duration {
    clock_now().saturating_duration_since(t)
}

/// A wait between two socket reads: `d` of the wall, or on a test's clock
/// a frame of it, which is what lets its server answer.
fn clock_sleep(d: Duration) {
    #[cfg(test)]
    if crate::play::cod4::tests::sim::sleep(Duration::from_millis(16)) {
        return;
    }
    if !d.is_zero() {
        std::thread::sleep(d);
    }
}

/// [`clock_now`] on the client's frame clock (`cls.realtime`, which
/// `cl_avidemo` steps) where no test clock runs.
fn frame_now() -> Instant {
    #[cfg(test)]
    if let Some(t) = crate::play::cod4::tests::sim::now() {
        return t;
    }
    crate::frame_clock::now()
}

/// `ANGLE2SHORT`.
fn angle_to_short(deg: f32) -> i32 {
    ((deg * (65536.0 / 360.0)) as i32) & 0xffff
}

/// `SHORT2ANGLE` of a difference, wrapped to [-180, 180).
fn short_to_angle(d: i32) -> f32 {
    f32::from(d as i16) * (360.0 / 65536.0)
}

/// Shortest gap between two of the pump's cmds ([`Cod2Session::pump`]); the
/// packet carries every cmd since the last one sent.
const CMD_MS: u64 = 16;
/// Shortest gap between two packets off the LAN: `1000 / cl_maxpackets`,
/// 30 by default (`CL_SendCmdInternal`, Mac 0x18a032).
const PACKET_MS: u64 = 1000 / 30;
/// The gap before the client is primed: one packet a second (the same).
const CONNECTING_PACKET_MS: u64 = 1000;
/// `cl_packetdup`'s default: each packet carries the cmds of this many
/// packets before it again (`CL_WritePacket`, Mac 0x188154).
const PACKET_DUP: usize = 1;
/// `cl_timeout`'s default, 200 s (`CL_InitOnceForAllClients`, Mac
/// 0x1485e2; cod2-protocol.md 1.10).
pub const TIMEOUT: Duration = Duration::from_secs(200);
/// How many of the `kept` newest cmds a packet carries: the `new` ones
/// and those of the last [`PACKET_DUP`] packets (`dup`), at most
/// `MAX_PACKET_USERCMDS` (`CL_WritePacket`, Mac 0x188154).
fn packet_cmds(new: usize, dup: &VecDeque<usize>, kept: usize) -> usize {
    (new + dup.iter().sum::<usize>())
        .min(vcod_cod2::net::usercmd::MAX_PACKET_USERCMDS)
        .min(kept)
}

/// Frames in a row past `cl_timeout` that drop the connection.
const TIMEOUT_FRAMES: u32 = 6;
/// `CL_ConnectionlessPacket` honours an OOB `disconnect` only once this
/// long passed without a sequenced message (`CL_ConnectionlessPacket`,
/// Mac 0x14c4c4).
const OOB_DISCONNECT_QUIET: Duration = Duration::from_millis(3000);

/// What the session counted; `--net-probe` prints it.
#[derive(Clone, Copy, Debug, Default)]
pub struct Stats {
    pub messages: usize,
    pub snapshots: usize,
    pub deltas: usize,
    pub commands: usize,
    pub parse_errors: usize,
    pub packets_sent: usize,
    /// Gamestates after the first: map changes and full restarts.
    pub new_gamestates: usize,
    /// `B` / `n`: `fast_restart`s.
    pub restarts: usize,
    /// `mr` responses sent.
    pub menu_replies: usize,
}

pub struct Cod2Session {
    /// `None` while a demo plays. `VCOD_NETSIM` applies here, and the
    /// server's address decides `Sys_IsLANAddress`'s packet pacing.
    sock: Option<UdpTransport>,
    /// The demo playing instead of a server.
    demo: Option<Box<DemoPlay>>,
    client: Client,
    view: NetView,
    /// When the newest snapshot came in; the render clock runs from it.
    snap_at: Instant,
    last_cmd: Instant,
    last_packet: Instant,
    /// Cmds not yet sent.
    pending: Vec<UserCmd2>,
    /// How many new cmds each of the last [`PACKET_DUP`] packets carried,
    /// newest last: the next packet sends them again.
    dup: VecDeque<usize>,
    /// The gamestate whose view the camera took its angles from.
    aimed_for: u32,
    /// The last [`Self::fly`] status line.
    last_log: Instant,
    /// A gamestate came in whose `cp` is not queued yet: no cmd goes out
    /// until it is, or a pure server drops us at the first `clc_move`.
    cp_owed: bool,
    /// The iwds the server references and the install lacks, fetched one
    /// at a time before the map loads and the `cp` goes
    /// (`CL_InitDownloads`, cod2-protocol.md 17.5); `None` when nothing is
    /// in flight.
    downloads: Option<Downloads>,
    /// Where and whether downloads go.
    opts: ConnectOpts,
    /// The newest gamestate's systeminfo `fs_game`, which
    /// `CL_SystemInfoChanged` sets on the client.
    fs_game: Option<String>,
    /// The connect screen's copy of the download's progress.
    progress: Option<Arc<std::sync::Mutex<Option<Progress>>>>,
    /// The files downloads landed: the caller opens the search path again
    /// ([`Self::take_fetched`]).
    fetched: Vec<PathBuf>,
    /// Their checksums, which count as held from then on.
    fetched_sums: Vec<i32>,
    /// `donedl` went out: the `cp` waits for the gamestate it brings.
    await_gamestate: bool,
    /// The server dropped us: its reason.
    pub dropped: Option<String>,
    pub stats: Stats,
    /// Answer the script menus, and play; `None` spectates.
    pub join: Option<Join>,
    /// When the last sequenced message came in.
    last_msg: Instant,
    /// `cl_timeout`: no message this long drops the session.
    pub timeout: Duration,
    /// Frames in a row past [`Self::timeout`] (`cl` +0x14).
    timeout_frames: u32,
    /// Ping, rates, losses and fragments, for the overlay.
    pub meter: NetMeter,
    /// `cg_drawLagometer`'s two rings (cod2-protocol.md 18.6).
    pub lagometer: Lagometer,
    /// The frames' snapshot pairs, for the F3 `lerp` line.
    pub lerp: LerpStats,
    /// This frame's `cg.time`, `CG_ProcessSnapshots`' once [`Self::fly`]
    /// ran it; `None` until then and after a gamestate.
    cg_time: Option<i32>,
    /// The gamestate [`Self::take_map_change`] last looked at, and the map
    /// the caller has loaded.
    level_gamestates: u32,
    loaded_map: String,
    /// A `B` / `n` not yet taken by [`Self::take_restart`].
    restart_pending: bool,
    /// The newest `b` (the scoreboard, the `score` reply) not yet taken
    /// by [`Self::take_scores`], split into words.
    scores: Option<Vec<String>>,
    /// Every server command's text, kept only while `Some` (the probe's
    /// `VCOD_PROBE_CMDS` log).
    pub command_log: Option<Vec<String>>,
    /// `h` / `i` chat lines not yet taken by [`Self::take_chat`]: whether
    /// team chat, and the line's argument, still to localize.
    chat: Vec<(bool, String)>,
    /// OOB `print`s (rcon's answers) not yet taken by
    /// [`Self::take_prints`].
    prints: Vec<String>,
    /// `s`, `o`, `p` and `q` commands not yet taken by
    /// [`Self::take_sound_commands`].
    sound_commands: Vec<SoundCommand>,
    /// The script menu indices [`Self::join`] answered since the last
    /// gamestate or restart: the next `t` for one is the player's.
    answered: Vec<usize>,
    /// `t`, `u`, `K` and `v` the player's UI takes, tokenized, not yet taken
    /// by [`Self::take_menu_commands`].
    menu_commands: Vec<Vec<String>>,
    /// The newest snapshot's `delta_angles`, to turn the view by a change.
    last_delta: Option<[i32; 3]>,
    /// Each snapshot's events as it becomes current, their sounds
    /// (`play::cod2_sound`) and their effects with the looping ones
    /// (`play::cod2_fx`).
    events: EventTracker,
    sounds: Cod2Sounds,
    fx: Cod2Fx,
    /// The gamestate `events`, `sounds` and `fx` were filled under.
    events_for: u32,
    /// The gamestate [`Self::take_level_effects`] last listed effects for.
    fx_for: u32,
    /// What the keys hold: buttons, stance, the weapon selection.
    pub input: Cod2Input,
    /// The console or a chat line is up: the cmd's talk bit.
    pub talking: bool,
    /// Voice chat's heard frames, talkers and outgoing batch
    /// (`crate::voice`, cod2-voice.md 4).
    pub voice: crate::voice::VoiceLink,
    /// `s_playerMute`: the clients muted from the mute list, whose frames
    /// `CL_VoicePacket` drops. `CL_Disconnect` clears it (Mac 0x14b8ea), as
    /// a new session starts empty.
    pub muted: [bool; 64],
    /// The client clock and the cmd clock over it; local ms run from `epoch`.
    epoch: Instant,
    clock: ServerClock,
    cmd_clock: CmdClock,
    /// Every cmd built, oldest first, at most [`CMD_BACKUP`].
    sent: VecDeque<UserCmd2>,
    predictor: Cod2Predictor,
    /// Whether the prediction clips other bodies; the probe's
    /// `VCOD_PROBE_NO_BODIES` turns it off to measure what they change.
    pub predict_bodies: bool,
    /// The map's collision world the prediction runs over, and its map.
    world: Option<(String, CollisionWorld)>,
    /// The map whose collision failed to build and the gamestate count it
    /// failed under: not tried again until a new gamestate.
    world_failed: Option<(String, u32)>,
    /// The level's weapon files, which every reader below takes its keys
    /// from; dropped at a gamestate and a search-path reopen.
    weapons: LevelWeapons,
    /// This frame's prediction and the newest snapshot with its
    /// playerstate replaced by the predicted one.
    predicted: Option<(Predicted2, SnapshotView)>,
    /// The prediction's events not yet taken.
    predicted_events: Vec<Event2>,
    /// The prediction counters since the last [`Self::take_predict_stats`].
    predict_log: PredictLog,
    /// Each ridden gun model's `tag_aim` and `tag_player` bind positions,
    /// the gunner's eye (`None`: the model or a tag did not load).
    gun_tags: HashMap<String, Option<(Vec3, Vec3)>>,
    /// This frame's view jitter on a firing gun, pitch and yaw degrees.
    jitter: [f32; 2],
    jitter_rng: crate::fx::sim::Rng,
    /// The own body's view kick and gun spring, and the gamestate and
    /// client they run for.
    recoil: Cod2Recoil,
    recoil_for: (u32, i32),
    /// The own body's last landing and this frame's dips of the eye and
    /// the gun (`play::cod2_view::Land`).
    land: crate::play::cod2_view::Land,
    land_dips: (f32, f32),
    /// The own view's last step and this frame's offset of the eye
    /// (`play::cod2_view::Step`).
    step: crate::play::cod2_view::Step,
    step_dip: f32,
}

/// What `--net-probe` prints of the prediction each second.
#[derive(Clone, Copy, Debug, Default)]
pub struct PredictLog {
    pub frames: u64,
    pub misses: u64,
    /// Snapshots that moved the prediction at all, and past 0.1.
    pub corrected: u64,
    pub missed: u64,
    pub max_correction: f32,
}

impl Cod2Session {
    /// A session on `sock` past `connectResponse`, before the gamestate.
    fn new(sock: Option<UdpSocket>, client: Client) -> Self {
        let now = clock_now();
        Cod2Session {
            sock: sock.and_then(|s| UdpTransport::from_socket(s).ok()),
            demo: None,
            client,
            view: NetView::default(),
            snap_at: now,
            last_cmd: now,
            last_packet: now - Duration::from_secs(1),
            pending: Vec::new(),
            dup: VecDeque::new(),
            aimed_for: 0,
            last_log: now - Duration::from_secs(10),
            cp_owed: false,
            downloads: None,
            opts: ConnectOpts {
                allow_download: false,
                www_download: false,
                ..ConnectOpts::default()
            },
            fs_game: None,
            progress: None,
            fetched: Vec::new(),
            fetched_sums: Vec::new(),
            await_gamestate: false,
            dropped: None,
            stats: Stats::default(),
            join: None,
            last_msg: now,
            timeout: TIMEOUT,
            timeout_frames: 0,
            meter: NetMeter::default(),
            lagometer: Lagometer::default(),
            lerp: LerpStats::default(),
            cg_time: None,
            level_gamestates: 0,
            loaded_map: String::new(),
            restart_pending: false,
            scores: None,
            command_log: None,
            chat: Vec::new(),
            prints: Vec::new(),
            sound_commands: Vec::new(),
            answered: Vec::new(),
            menu_commands: Vec::new(),
            last_delta: None,
            events: EventTracker::default(),
            sounds: Cod2Sounds::default(),
            fx: Cod2Fx::default(),
            events_for: 0,
            fx_for: 0,
            input: Cod2Input::default(),
            talking: false,
            voice: crate::voice::VoiceLink::cod2(),
            muted: [false; 64],
            epoch: frame_now(),
            clock: ServerClock::cod2(),
            cmd_clock: CmdClock::default(),
            sent: VecDeque::new(),
            predictor: Cod2Predictor::default(),
            predict_bodies: true,
            world: None,
            world_failed: None,
            weapons: LevelWeapons::default(),
            predicted: None,
            predicted_events: Vec::new(),
            predict_log: PredictLog::default(),
            gun_tags: HashMap::new(),
            jitter: [0.0; 2],
            jitter_rng: crate::fx::sim::Rng::new(0x6a69_7474),
            recoil: Cod2Recoil::default(),
            recoil_for: (0, -1),
            land: Default::default(),
            land_dips: (0.0, 0.0),
            step: Default::default(),
            step_dip: 0.0,
        }
    }

    /// The map of a gamestate that replaced the loaded one's map, once:
    /// the caller loads it, the session under [`Self::pump`]. A gamestate
    /// for the same map (`map_restart`, the same map again) keeps it.
    pub fn take_map_change(&mut self) -> Option<String> {
        if self.view.gamestates == self.level_gamestates || !self.settled() {
            return None;
        }
        self.level_gamestates = self.view.gamestates;
        let map = self.map_name()?.to_string();
        if map == self.loaded_map {
            log::info!("cod2: new gamestate for {map}, the map stays loaded");
            return None;
        }
        log::info!("cod2: map change {} -> {map}", self.loaded_map);
        self.loaded_map = map.clone();
        Some(map)
    }

    /// The newest `b` reply since the last call.
    pub fn take_scores(&mut self) -> Option<Vec<String>> {
        self.scores.take()
    }

    /// The chat lines since the last call, oldest first.
    pub fn take_chat(&mut self) -> Vec<(bool, String)> {
        std::mem::take(&mut self.chat)
    }

    /// The OOB `print` texts since the last call, oldest first.
    pub fn take_prints(&mut self) -> Vec<String> {
        std::mem::take(&mut self.prints)
    }

    /// An out-of-band packet to the server joined (`rcon`): `text` is the
    /// payload, NUL included, with no 0xff header. A demo sends nothing.
    pub fn send_oob(&mut self, text: &[u8]) {
        if let Some(sock) = self.sock.as_mut() {
            let mut p = vec![0xff; 4];
            p.extend_from_slice(text);
            sock.send(&p);
        }
    }

    /// The systeminfo's `sv_voice` and `sv_voiceQuality`, both systeminfo
    /// dvars the client copies (`SV_Init`, Mac 0x141d62, flags 0x100d and
    /// 0x1008); off and 1 before a gamestate.
    pub fn voice_settings(&self) -> (bool, i32) {
        use vcod_cod2::net::voice::SV_VOICE_QUALITY;
        let si = self.view.configstrings.get(1);
        let int =
            |k: &str| vcod_common::net::info_value_for_key(si, k).map(vcod_common::text::atoi);
        (
            int("sv_voice").unwrap_or(0) != 0,
            int("sv_voiceQuality").unwrap_or(SV_VOICE_QUALITY),
        )
    }

    /// The talker list's clients (`UI_OwnerDraw` 267..270): every client
    /// `CL_IsPlayerTalking` (Mac 0x1479dc) names, by number, a frame of
    /// his heard in the last 300 ms; our own only under the server's
    /// `voice_localEcho`.
    pub fn talkers(&self) -> Vec<u32> {
        self.voice.talkers(None, self.real_ms())
    }

    /// `CL_MutePlayer` (Mac 0x161d2e): flips `client`'s mute and tells the
    /// server, `muteplayer <n>` or `unmuteplayer <n>`.
    pub fn toggle_mute(&mut self, client: u32) {
        let Some(m) = self.muted.get_mut(client as usize) else {
            return;
        };
        *m = !*m;
        let cmd = if *m { "muteplayer" } else { "unmuteplayer" };
        self.client.add_reliable(&format!("{cmd} {client}"));
    }

    /// `CL_VoiceTransmit` (Mac 0x147a9e) on the batch, sent out of band
    /// on our qport (`CL_WriteVoicePacket`, 0x187fac). A demo sends
    /// nothing.
    pub fn transmit_voice(&mut self) {
        let now = self.real_ms();
        let Some(batch) = self.voice.due(now) else {
            return;
        };
        if let Some(sock) = self.sock.as_mut() {
            let qport = self.client.netchan.qport;
            sock.send(&vcod_cod2::net::voice::client_packet(qport, &batch));
        }
    }

    /// A client command on the reliable stream (`score`, `say`, a
    /// console line the client does not run itself).
    pub fn send_command(&mut self, line: &str) {
        self.client.add_reliable(line);
    }

    /// Whether a `B` / `n` came in since the last call: the level restarted
    /// on the loaded map, its entities spawned again.
    pub fn take_restart(&mut self) -> bool {
        std::mem::take(&mut self.restart_pending)
    }

    /// Serverinfo's map, once the gamestate is in.
    pub fn map_name(&self) -> Option<&str> {
        self.view.configstrings.map_name()
    }

    /// Read every waiting datagram. A message that fails to parse is
    /// counted and logged, and the next one tried. A gamestate's `cp` is
    /// queued here when `fs` is given, else on the next call that gives it.
    pub fn poll(&mut self, fs: Option<&Pk3Fs>) {
        self.poll_at_most(fs, vcod_common::net::MAX_PACKETS_PER_PUMP);
    }

    /// [`Self::poll`] reading at most `max` datagrams, so a flood cannot
    /// hold the frame.
    fn poll_at_most(&mut self, fs: Option<&Pk3Fs>, max: usize) {
        let mut buf = vec![0u8; 0x10000];
        let mut read = 0;
        while read < max
            && let Some(n) = self.sock.as_mut().and_then(|s| s.try_recv(&mut buf))
        {
            read += 1;
            let d = &buf[..n];
            if let Some(pl) = oob::payload(d) {
                // `CL_ConnectionlessPacket`'s `v` (Mac 0x14c4c4) into
                // `CL_VoicePacket` (0x1478f2).
                if let Some(body) = vcod_cod2::net::voice::oob_body(pl) {
                    let heard = vcod_cod2::net::voice::parse_server(body);
                    let muted = self.muted;
                    let now = self.real_ms();
                    self.voice.hear(heard, now, |c| muted[c as usize]);
                    continue;
                }
                match oob::parse_reply(pl) {
                    Reply::Error(e) => self.dropped = Some(e),
                    Reply::Disconnect if since(self.last_msg) >= OOB_DISCONNECT_QUIET => {
                        self.dropped = Some("EXE_SERVER_DISCONNECTED".into())
                    }
                    // `CL_ConnectionlessPacket` prints a `print` from any
                    // sender in any state (Mac 0x14c4c4).
                    Reply::Print(text) => {
                        let text = text.trim_end_matches(['\n', '\0']);
                        if !text.is_empty() {
                            self.prints.push(text.to_string());
                        }
                    }
                    Reply::LoadingNewMap(m) => log::info!(
                        "cod2: loadingnewmap {} at {:.0}",
                        m.replace('\n', " "),
                        self.real_ms()
                    ),
                    r => log::debug!("cod2 oob: {r:?}"),
                }
                continue;
            }
            // The netchan header's high bit marks a fragment.
            let fragment = d.get(3).is_some_and(|b| b & 0x80 != 0);
            let lost = self.client.netchan.total_dropped;
            let got = self.client.receive(d);
            let lost = self.client.netchan.total_dropped - lost;
            self.meter.packet(fragment, lost as u32, clock_now());
            match got {
                Ok(Some(p)) => self.on_message(p),
                Ok(None) => {}
                Err(e) => {
                    self.stats.parse_errors += 1;
                    log::warn!("cod2: message parse failed: {e:#}");
                }
            }
        }
        if self.demo.is_some() {
            return;
        }
        if let Some(dls) = self.downloads.as_mut() {
            let evs = dls.poll();
            self.download_events(evs);
        }
        if let Some(p) = &self.progress
            && let Ok(mut p) = p.lock()
        {
            *p = self.downloads.as_ref().and_then(Downloads::progress);
        }
        // `CL_Frame` (Mac 0x14b182): six frames in a row past `cl_timeout`
        // without a sequenced packet drop the connection.
        if since(self.last_msg) > self.timeout {
            self.timeout_frames += 1;
            if self.timeout_frames >= TIMEOUT_FRAMES && self.dropped.is_none() {
                self.dropped = Some("EXE_ERR_SERVER_TIMEOUT".into());
            }
        } else {
            self.timeout_frames = 0;
        }
        if self.cp_owed
            && let Some(fs) = fs
        {
            self.queue_cp(fs);
        }
        if let Some(fs) = fs
            && let Some(model) = self.ridden_gun().map(|(m, _)| m)
            && !self.gun_tags.contains_key(&model)
        {
            let tags = vcod_common::model::load_bones(fs, &model)
                .ok()
                .and_then(|b| {
                    let tag = |n: &str| b.iter().find(|x| x.name == n).map(|x| x.pos);
                    Some((tag("tag_aim")?, tag("tag_player")?))
                });
            self.gun_tags.insert(model, tags);
        }
    }

    /// What one parsed server message brings, off the socket or a demo.
    fn on_message(&mut self, p: vcod_cod2::net::Parsed) {
        self.last_msg = clock_now();
        self.stats.messages += 1;
        self.stats.commands += p.commands.len();
        if p.gamestate && self.view.gamestates > 0 {
            self.stats.new_gamestates += 1;
            self.last_delta = None;
        }
        if p.gamestate {
            log::debug!("cod2 gamestate {:.0}", self.real_ms());
            self.clock = ServerClock::cod2();
            self.cmd_clock.reset();
            self.sent.clear();
            self.pending.clear();
            self.dup.clear();
            self.predictor.reset();
            self.predicted = None;
            self.cg_time = None;
            self.input.reset();
            self.weapons.clear();
            self.answered.clear();
        }
        if let Some(n) = p.snapshot {
            self.stats.snapshots += 1;
            if self
                .client
                .parser
                .frame(n)
                .is_some_and(|f| f.delta_num >= 0)
            {
                self.stats.deltas += 1;
            }
            self.snap_at = clock_now();
        }
        self.view.update(&self.client.parser, &p);
        self.meter.message(clock_now());
        if p.snapshot.is_some()
            && let Some(s) = &self.view.snap
        {
            self.meter.snapshot(s.ps.command_time, clock_now());
            self.lagometer
                .parsed(s.message_num, self.meter.ping, s.snap_flags);
        }
        // A demo sends nothing, so it owes no `cp`.
        self.cp_owed |= p.gamestate && self.demo.is_none();
        if p.gamestate {
            self.on_gamestate();
        }
        if !p.commands.is_empty() {
            self.client.set_systeminfo(self.view.configstrings.get(1));
        }
        for (_, text) in &p.commands {
            self.server_command(text);
        }
        for m in &p.downloads {
            self.download_message(m);
        }
    }

    /// The gun the newest snapshot's playerstate rides (`eFlags & 0x300`,
    /// `viewlocked_entNum`): its model, `xmodel/` stripped, and number.
    fn ridden_gun(&self) -> Option<(String, u32)> {
        let f = self.frame()?;
        let ps = &f.snap.ps;
        if ps.eflags & vcod_cod2::net::flags::EF_TURRET_STANCE == 0 {
            return None;
        }
        let n = ps.raw.i32("viewlocked_entNum") as u32;
        let e = f.snap.entity(n)?;
        let model = f.configstrings.model(e.model_index)?;
        let model = model.strip_prefix("xmodel/").unwrap_or(model);
        Some((model.to_string(), n))
    }

    /// The engine's and the session's share of a server command, after the
    /// message's configstrings applied: `w` drops, `B` / `n` restart
    /// (`CL_GetServerCommand`), `t` opens a script menu a [`Join`] answers.
    fn server_command(&mut self, text: &[u8]) {
        if let Some(log) = self.command_log.as_mut() {
            log.push(vcod_cod2::net::latin1(text));
        }
        // The map-cycle captures (cod2-map-cycle.md) read these.
        log::debug!(
            "cod2 cmd {:.0} {}",
            self.real_ms(),
            vcod_cod2::net::latin1(text)
        );
        if let Some(why) = text.strip_prefix(b"w ") {
            let why = vcod_cod2::net::latin1(why);
            self.dropped = Some(why.trim_matches('"').to_string());
            return;
        }
        // `b`: the scoreboard (cod2-protocol.md 5, 1.1's shape).
        if text == b"b" || text.starts_with(b"b ") {
            let words = vcod_cod2::net::latin1(text);
            self.scores = Some(words.split_whitespace().map(str::to_string).collect());
            return;
        }
        // `h` chat, `i` team chat (`CG_ServerCommand` Mac 0x1e2ea4):
        // localized, then `CG_AddToTeamChat` and the console.
        if let Some(team) = match text.get(..2) {
            Some(b"h ") => Some(false),
            Some(b"i ") => Some(true),
            _ => None,
        } {
            let words = crate::console::shell::tokenize(&vcod_cod2::net::latin1(text));
            if let Some(line) = words.get(1) {
                self.chat.push((team, line.clone()));
            }
            return;
        }
        if let Some(c) = crate::play::cod2_sound::sound_command(
            &vcod_cod2::net::latin1(text),
            &self.view.configstrings,
        ) {
            self.sound_commands.push(c);
            return;
        }
        // `a <index>` (`G_SelectWeaponIndex`, sent by the use key's weapon
        // swap): `CG_ServerCommand`'s 0x61 selects it, so the next cmd asks
        // for it and pmove raises it (cod2-combat.md 6.3).
        if let Some(w) = text.strip_prefix(b"a ") {
            if let Ok(w) = vcod_cod2::net::latin1(w).trim().parse::<i32>() {
                self.input.select(w as u8);
            }
            return;
        }
        if text == b"B" || text == b"n" {
            log::info!("cod2: fast_restart ({})", text[0] as char);
            self.stats.restarts += 1;
            self.restart_pending = true;
            self.answered.clear();
            // The level's entities spawn again: nothing lerps across.
            self.view.reset_lerp();
            return;
        }
        let words = crate::console::shell::tokenize(&vcod_cod2::net::latin1(text));
        match words.first().map(String::as_str) {
            Some("t") => {}
            // `u` closes the script popup, `K` the in-game menu
            // (`CL_CloseInGameMenu`, Mac 0x17afbe), `v` sets a client cvar
            // (`CG_SetClientDvarFromServer`, Mac 0x1e2676): the UI's.
            Some("u" | "K" | "v") => {
                self.menu_commands.push(words);
                return;
            }
            _ => return,
        }
        let Some(n) = words.get(1).and_then(|t| t.parse::<usize>().ok()) else {
            self.menu_commands.push(words);
            return;
        };
        let answer = self
            .view
            .configstrings
            .script_menu(n)
            .filter(|_| !self.answered.contains(&n))
            .and_then(|menu| Some((menu, self.join.as_ref()?.response(menu)?)));
        let Some((menu, response)) = answer else {
            // Answered once already, or one no join answers: the player's.
            self.menu_commands.push(words);
            return;
        };
        self.answered.push(n);
        let mr = format!("mr {} {n} {response}", self.client.server_id());
        log::info!("cod2: {menu}: {mr}");
        self.client.add_reliable(&mr);
        self.stats.menu_replies += 1;
    }

    /// The `t`, `u`, `K` and `v` server commands the player's script menus
    /// take ([`crate::play::join::Join::on_server_command`]), tokenized:
    /// every `t` but a [`Join`]'s first answer for its menu since the
    /// gamestate or a restart.
    pub fn take_menu_commands(&mut self) -> Vec<Vec<String>> {
        std::mem::take(&mut self.menu_commands)
    }

    /// A script menu's response (`Script_ScriptMenuResponse`, Mac
    /// 0x165c44) or `openscriptmenu`'s (`CL_OpenScriptMenu_f`, 0x1484fe):
    /// `mr <serverId> <index> <response>`, the index `menu`'s slot in
    /// configstrings 1246..1277 matched without case, -1 when none.
    pub fn menu_response(&mut self, menu: &str, response: &str) {
        let (first, last) = CS_LAYOUT_118.bounds(CsRange::Menu);
        let index = (first..=last)
            .position(|i| {
                let name = self.view.configstrings.get(i);
                !name.is_empty() && name.eq_ignore_ascii_case(menu)
            })
            .map_or(-1, |i| i as i32);
        let mr = format!("mr {} {index} {response}", self.client.server_id());
        log::info!("cod2: {mr}");
        self.client.add_reliable(&mr);
    }

    /// The gamestates parsed this connection.
    pub fn gamestates(&self) -> u32 {
        self.view.gamestates
    }

    /// The `sv_serverId` script menu responses carry.
    pub fn server_id(&self) -> i32 {
        i32::from(self.client.server_id())
    }

    /// Every configstring slot, for the script menus' names and the
    /// server cvar mirror.
    pub fn configstrings(&self) -> &[String] {
        self.view.configstrings.all()
    }

    /// The `cp` a pure server checks before the first `clc_move` after a
    /// gamestate (cod2-protocol.md 11.4): the iwd the level's `.d3dbsp`
    /// came from, keyed with the gamestate's feed, as the server accepted
    /// live. An unpure server ignores it.
    fn queue_cp(&mut self, fs: &Pk3Fs) {
        if !self.settled() {
            return;
        }
        let Some(gs) = &self.client.parser.gamestate else {
            return;
        };
        fs.clear_references();
        if let Some(map) = self.view.configstrings.map_name() {
            fs.touch(&format!("maps/mp/{map}.d3dbsp"));
        }
        let cp = vcod_cod2::net::pure::pure_command(fs, gs.checksum_feed, gs.configstring(1));
        log::info!("cod2: {}", cp.trim_end());
        self.client.add_reliable(&cp);
        self.cp_owed = false;
    }

    /// A gamestate is in and no download runs or is owed one: the map
    /// can load and the `cp` go (`CL_DownloadsComplete`).
    pub fn settled(&self) -> bool {
        self.view.gamestates > 0 && self.downloads.is_none() && !self.await_gamestate
    }

    /// `CL_SystemInfoChanged`'s `fs_game` and `CL_InitDownloads` (Mac
    /// 0x14d84c) for a new gamestate: under `cl_allowDownload` the
    /// referenced iwds the homepath lacks are asked for; without it the
    /// join goes on and the console says what is missing.
    fn on_gamestate(&mut self) {
        use vcod_cod2::net::download::{fs_game, held_checksums, missing_iwds};
        self.await_gamestate = false;
        // `CL_SystemInfoChanged` returns past `sv_serverid` while a demo
        // plays (Mac 0x1d535a): no `fs_game`, no downloads.
        if self.demo.is_some() {
            return;
        }
        let si = self.view.configstrings.get(1).to_string();
        self.fs_game = fs_game(&si);
        if self.downloads.is_some() {
            return;
        }
        let Some(home) = self.opts.home.clone() else {
            return;
        };
        let held = held_checksums(&home, self.fs_game.as_deref());
        let missing = missing_iwds(&si, |sum| {
            self.fetched_sums.contains(&sum) || held.contains(&sum)
        });
        if missing.is_empty() {
            return;
        }
        if !self.opts.allow_download {
            log::warn!(
                "cod2: You are missing some files referenced by the server: {}. \
                 You might not be able to join the game (cl_allowDownload is 0)",
                missing.join(" ")
            );
            return;
        }
        log::info!("cod2: Need iwds: {}", missing.join(" "));
        let mut dls = Downloads::new(&home, &si, missing).with_www(self.opts.www_download);
        let first = dls.next_download();
        self.downloads = Some(dls);
        self.download_events(first);
        // Nothing could be asked for: no `donedl` is owed.
        if self.downloads.as_ref().is_some_and(Downloads::is_done) {
            self.downloads = None;
        }
    }

    /// `CL_ParseDownload` for one `svc_download`.
    fn download_message(&mut self, m: &vcod_cod2::net::download::DownloadMsg) {
        let Some(dls) = self.downloads.as_mut() else {
            return;
        };
        let evs = dls.on_message(m);
        self.download_events(evs);
    }

    fn download_events(&mut self, evs: Vec<DownloadEvent>) {
        for e in evs {
            match e {
                DownloadEvent::Command(c) => {
                    // `donedl` ends the queue, `wwwdl fail` has the server
                    // fall back: either way a gamestate comes next.
                    if c == "donedl" || c == "wwwdl fail" {
                        self.downloads = None;
                        self.await_gamestate = true;
                    }
                    self.client.add_reliable(&c);
                }
                DownloadEvent::Finished(path) => {
                    log::info!("cod2: downloaded {}", path.display());
                    if let Ok(s) = vcod_common::pak_checksum::pak_checksums(&path, 0) {
                        self.fetched_sums.push(s.checksum);
                    }
                    self.fetched.push(path);
                }
                DownloadEvent::Skipped(why) => log::warn!("cod2: {why}"),
                // The refusal's `Com_Error` ends the connection with the
                // server's own text (Mac 0x1d59c8).
                DownloadEvent::Failed(why) => {
                    log::warn!("cod2: download failed: {why}");
                    self.downloads = None;
                    self.dropped = Some(why);
                }
            }
        }
    }

    /// The download in flight, for a progress line.
    pub fn download_progress(&self) -> Option<Progress> {
        self.downloads.as_ref()?.progress()
    }

    /// The systeminfo's `fs_game` of the newest gamestate.
    pub fn fs_game(&self) -> Option<&str> {
        self.fs_game.as_deref()
    }

    /// The files downloads landed since the last call: the caller opens its
    /// search path again before it loads the map.
    pub fn take_fetched(&mut self) -> Vec<std::path::PathBuf> {
        std::mem::take(&mut self.fetched)
    }

    /// The server time the client draws at: the client clock once
    /// [`Self::fly`] runs it, else one snapshot interval behind the newest,
    /// advancing on the wall clock since it came in.
    fn render_time(&self) -> Option<i32> {
        let snap = self.view.snap.as_ref()?;
        if let Some(d) = &self.demo {
            return d.active.then_some(d.server_time);
        }
        if self.clock.started() {
            return Some(self.cg_time.unwrap_or_else(|| self.clock.server_time()));
        }
        let gap = self
            .view
            .prev
            .as_ref()
            .map_or(0, |p| snap.server_time - p.server_time);
        Some(snap.server_time - gap + since(self.snap_at).as_millis() as i32)
    }

    /// The server time a cmd made now carries.
    fn cmd_time(&self) -> i32 {
        let snap = self.view.snap.as_ref().map_or(0, |s| s.server_time);
        if self.clock.started() {
            return self.clock.cmd_time(snap);
        }
        snap + since(self.snap_at).as_millis() as i32
    }

    /// Local ms since the session started, on the client's frame clock
    /// (`cls.realtime`, which `cl_avidemo` steps).
    fn real_ms(&self) -> f64 {
        frame_now()
            .saturating_duration_since(self.epoch)
            .as_secs_f64()
            * 1000.0
    }

    /// `CL_SetCGameTime`'s clock for this frame, off the newest snapshot.
    fn advance_clock(&mut self) {
        let real = self.real_ms() as i32;
        if let Some(s) = &self.view.snap {
            let prev = self.view.prev.as_ref().map(|p| p.server_time);
            self.clock
                .update_cod2(real, s.server_time, prev, s.message_num as u32);
        }
    }

    /// `CG_DrawActiveFrame`'s start (Mac 0x1d4a50) on the client clock:
    /// the lagometer's frame, then `CG_ProcessSnapshots` picks the pair the
    /// frame draws between (cod2-protocol.md 18.5).
    fn process_snapshots(&mut self) {
        if self.demo.is_some() || !self.clock.started() {
            return;
        }
        let t = self.clock.server_time();
        self.lagometer.frame(t);
        let (time, read) = self.view.process(t);
        self.lagometer.read(&read);
        let Some(newest) = &self.view.snap else {
            return;
        };
        self.lagometer.latest = newest.server_time;
        self.cg_time = Some(time);
        if let Some(c) = self.view.cursor()
            && let Some(f) = self.view.frame(time)
        {
            let next = f.snap.server_time;
            let cur = f.prev.map_or(next, |p| p.server_time);
            debug_assert_eq!(c.next == c.cur, f.prev.is_none());
            let prev = self
                .view
                .prev
                .as_ref()
                .map_or(newest.server_time, |p| p.server_time);
            self.lerp
                .frame(t, time, (cur, next), (prev, newest.server_time));
        }
    }

    /// This render frame's view; `None` before the first snapshot. While
    /// the body is predicted `snap.ps` is the predicted playerstate.
    pub fn frame(&self) -> Option<Frame<'_>> {
        let t = self.render_time()?;
        let mut f = self.view.frame(t)?;
        if let Some((_, snap)) = &self.predicted {
            f.snap = snap;
        }
        Some(f)
    }

    /// Whether this frame's own playerstate is predicted (its sight's
    /// `fWeaponPosFrac` is the frame's own, not a snapshot's to lerp).
    pub fn predicting(&self) -> bool {
        self.predicted.is_some()
    }

    /// The predicted eye and view angles (degrees, pitch down-positive),
    /// what the camera draws from; `None` while the body is not predicted.
    pub fn view(&self) -> Option<(Vec3, [f32; 3])> {
        let (p, _) = self.predicted.as_ref()?;
        let dips = (self.land_dips.0, self.step_dip);
        let mut eye = crate::play::cod2_predict::first_person_eye(&p.drawn, p.origin, dips);
        if p.drawn.e_flags & 0x300 != 0
            && let Some(at) = self.gunner_eye(p.drawn.view_angles)
        {
            eye = at;
        }
        let mut angles = p.drawn.view_angles;
        angles[0] += self.jitter[0];
        angles[1] += self.jitter[1];
        Some((eye, angles))
    }

    /// The client entity under the crosshair this frame, traced from the
    /// drawn eye and view (`CG_DrawCrosshairNames`' scan, crate::cod2_names).
    pub fn crosshair_entity(&self) -> Option<u32> {
        let f = self.frame()?;
        let (eye, angles) = self.view().unwrap_or_else(|| {
            (
                self.snapshot_eye(&f),
                crate::cod2_viewmodel::view_angles(&f),
            )
        });
        let forward = vcod_common::x87::angle_vectors(Vec3::from_array(angles))[0];
        let world = self.world.as_ref().map(|(_, w)| w);
        crate::cod2_names::scan(&f, eye, forward, world)
    }

    /// A demo's or a followed player's eye: `CG_CalcViewValues`' first
    /// person arm (Mac 0x1d3b28), which tests neither, over the snapshots'
    /// lerped playerstate ([`cod2_predict::interpolated`]): the bob, the
    /// landing dip and the step [`Self::recoil_frame`] keeps (none in a
    /// demo), the lean. A gunner's is his gun's; an intermission's
    /// (`pm_type` 5, which the arm skips) the lerped eye.
    fn snapshot_eye(&self, f: &Frame) -> Vec3 {
        let view = crate::cod2_viewmodel::view_angles(f);
        if let Some(at) = self.gunner_eye(view) {
            return at;
        }
        if f.snap.ps.pm_type == 5 {
            return Vec3::from(f.eye());
        }
        let ps = crate::play::cod2_predict::interpolated(f);
        let dips = (self.land_dips.0, self.step_dip);
        crate::play::cod2_predict::first_person_eye(&ps, ps.origin, dips)
    }

    /// A gunner sees from the gun's `tag_player` (`CG_CalcViewValues`,
    /// Mac 0x1d3b28, cod2-turrets.md 9), the barrel turned to `view`: the
    /// predicted body's, or a followed gunner's from his playerstate, the
    /// `eFlags & 0x300` arm reading `cg.predictedPlayerState` either way.
    /// `None` off a gun or before its tags load.
    fn gunner_eye(&self, view: [f32; 3]) -> Option<Vec3> {
        let (model, n) = self.ridden_gun()?;
        let Some(Some((aim, player))) = self.gun_tags.get(&model) else {
            return None;
        };
        let f = self.frame()?;
        let (origin, angles) = f.entity_place(f.snap.entity(n)?);
        Some(crate::play::cod2_predict::gunner_eye(
            *aim, *player, origin, angles, view,
        ))
    }

    /// `CG_CalcViewValues`' firing gunner (Mac 0x1d3b28): on `viewlocked`
    /// 2 the drawn view's pitch and yaw take `crandom()` times the gun
    /// weapon's `vertViewJitter` and `horizViewJitter`, fresh each frame
    /// and never in the cmd (cod2-turrets.md 9).
    fn gun_jitter(&mut self, fs: &Pk3Fs) -> [f32; 2] {
        let weapon = {
            let Some(f) = self.frame() else {
                return [0.0; 2];
            };
            let ps = &f.snap.ps;
            if ps.eflags & 0x300 == 0 || ps.raw.i32("viewlocked") != 2 {
                return [0.0; 2];
            }
            let gun = f.snap.entity(ps.raw.i32("viewlocked_entNum") as u32);
            match gun.filter(|e| f.configstrings.weapon_name(e.weapon).is_some()) {
                Some(e) => e.weapon,
                None => return [0.0; 2],
            }
        };
        self.weapons.ensure(fs, &self.view.configstrings);
        let [v, h] = self.weapons.get(weapon).map_or([0.0; 2], |w| {
            [
                w.def.float("vertViewJitter"),
                w.def.float("horizViewJitter"),
            ]
        });
        let mut crandom = || 2.0 * self.jitter_rng.next_f32() - 1.0;
        [crandom() * v, crandom() * h]
    }

    /// The counters [`PredictLog`] keeps, since the last call.
    pub fn take_predict_stats(&mut self) -> PredictLog {
        let s = self.predictor.stats;
        let max = self.predictor.take_max();
        let log = PredictLog {
            frames: s.frames,
            misses: s.misses,
            corrected: s.corrected,
            missed: s.missed,
            max_correction: max,
        };
        let last = std::mem::replace(&mut self.predict_log, log);
        PredictLog {
            frames: log.frames - last.frames,
            misses: log.misses - last.misses,
            corrected: log.corrected - last.corrected,
            missed: log.missed - last.missed,
            max_correction: max,
        }
    }

    /// `CG_DrawDisconnect` (Mac 0x1cd848): the server has not run the
    /// oldest of the last 128 cmds (`CL_GetCurrentCmdNumber() - 0x7f`)
    /// though its time has come: `Connection Interrupted` and the blinking
    /// `net_disconnect` icon (cod2-protocol.md 18.2). Never before 128
    /// cmds were built, as that slot still reads 0.
    pub fn interrupted(&self) -> bool {
        if self.demo.is_some() || self.sent.len() < CMD_BACKUP {
            return false;
        }
        let Some(f) = self.render_time().and_then(|t| self.view.frame(t)) else {
            return false;
        };
        let t = f.time_ms;
        // `cg.snap`: the older of the two the frame draws between.
        let ps = &f.prev.unwrap_or(f.snap).ps;
        let oldest = self.sent[self.sent.len() - CMD_BACKUP].server_time;
        ps.command_time < oldest && oldest <= t
    }

    /// The F3 overlay's link lines: ping, the last second's snapshots,
    /// messages, packets out, losses and fragments; the client clock; the
    /// frames held or extrapolated ([`LerpStats`]); the prediction's
    /// corrections.
    pub fn debug_lines(&mut self) -> Vec<String> {
        let r = self.meter.rates(clock_now());
        let lan = self.sock.as_ref().is_some_and(UdpTransport::is_lan);
        let mut lines = vec![format!(
            "net: ping {:3}  snaps {:2}/s  msgs {:2}/s  out {:2}/s  lost {} ({})  frag {}/s ({})  rate {} snaps {}{}",
            self.meter.ping,
            r.snaps,
            r.messages,
            r.sent,
            r.lost,
            self.meter.lost_total,
            r.fragments,
            self.meter.fragments,
            self.opts.rate,
            self.opts.snaps,
            if lan { "  lan" } else { "" },
        )];
        if let (Some(snap), true) = (&self.view.snap, self.clock.started()) {
            let st = self.clock.stats;
            lines.push(format!(
                "clock: behind {:3}  reset {} fast {} +{} -{}  extrap {}",
                snap.server_time - self.clock.server_time(),
                st.resets,
                st.fast,
                st.ahead,
                st.back,
                st.extrapolated
            ));
        }
        if self.cg_time.is_some() {
            let l = self.lerp;
            lines.push(format!(
                "lerp: frames {}  held {}  extrap {}  (newest two: held {} extrap {})",
                l.frames, l.held, l.extrapolated, l.two_held, l.two_extrapolated
            ));
        }
        let p = self.predictor.stats;
        lines.push(format!(
            "pred {}  miss {}  corrected {}  missed {}",
            if self.predicted.is_some() {
                "on"
            } else {
                "off"
            },
            p.misses,
            p.corrected,
            p.missed
        ));
        lines
    }

    /// The map's collision world, built on first use of each map unless
    /// the load handed it over ([`Self::install_world`]). A failed build
    /// waits for a new gamestate, and a map change the caller has yet to
    /// take ([`Self::take_map_change`]) is left to its load.
    fn ensure_world(&mut self, fs: &Pk3Fs) {
        let Some(map) = self.map_name().map(str::to_string) else {
            return;
        };
        let gamestates = self.view.gamestates;
        if self.world.as_ref().is_some_and(|(m, _)| *m == map)
            || self.world_failed.as_ref() == Some(&(map.clone(), gamestates))
            || (gamestates != self.level_gamestates && map != self.loaded_map)
        {
            return;
        }
        let t0 = Instant::now();
        let built = (|| -> anyhow::Result<CollisionWorld> {
            let map = vcod_cod2::map::LoadedMap::find(fs, &map)?;
            Ok(prediction_world(&map.world, map.clip))
        })();
        self.install_world(fs, &map, built, t0);
    }

    /// `map`'s collision for prediction, `built` since `t0`; the load
    /// builds it beside the renderer's upload (`cod2_front`) and hands it
    /// over here, so the first predicted frame reads no map again.
    pub fn install_world(
        &mut self,
        fs: &Pk3Fs,
        map: &str,
        built: anyhow::Result<CollisionWorld>,
        t0: Instant,
    ) {
        // The mantle's anims, which the predictor climbs along.
        self.predictor.rules.mantle_anims = vcod_cod2::pmove::mantle::anims(fs);
        match built {
            Ok(cw) => {
                log::info!(
                    "cod2: {map}'s collision for prediction in {:.0} ms",
                    t0.elapsed().as_secs_f64() * 1000.0
                );
                self.world = Some((map.to_string(), cw));
                self.world_failed = None;
            }
            Err(e) => {
                log::warn!("cod2: no collision for {map}, not predicting: {e:#}");
                self.world = None;
                self.world_failed = Some((map.to_string(), self.view.gamestates));
            }
        }
    }

    /// The held weapon's `moveSpeedScale`, 0 (none) when its file is
    /// missing.
    fn move_scale(&self, weapon: i32) -> f32 {
        self.weapons
            .get(weapon)
            .map_or(0.0, |w| w.def.float("moveSpeedScale"))
    }

    /// One cmd at `time` from the keys and `view`, the view kick added to
    /// its angles as `CL_CreateCmd` (Mac 0x189f0e) adds the cgame's aim
    /// values (`play::cod2_recoil`).
    fn play_cmd(&self, time: i32, view: [f32; 2]) -> UserCmd2 {
        let mut cmd = self.client.idle_cmd(time);
        let Some(snap) = &self.view.snap else {
            return cmd;
        };
        let kick = self.recoil.view_kick();
        for (i, v) in view.iter().enumerate() {
            cmd.angles[i] = (angle_to_short(*v + kick[i]) - snap.ps.delta_angles[i]) & 0xffff;
        }
        cmd.angles[2] = (cmd.angles[2] + angle_to_short(kick[2])) & 0xffff;
        (cmd.forward_move, cmd.right_move) = self.input.moves();
        cmd.buttons = self.input.buttons(snap.ps.pm_type, self.talking);
        let (weapon, offhand) = self.input.weapons();
        if weapon != 0 {
            cmd.weapon = weapon;
        }
        if offhand != 0 {
            cmd.off_hand_index = offhand;
        }
        cmd
    }

    /// Keeps a built cmd for the next packet and the prediction.
    fn keep(&mut self, cmd: UserCmd2) {
        self.pending.push(cmd);
        self.sent.push_back(cmd);
        while self.sent.len() > CMD_BACKUP {
            self.sent.pop_front();
        }
    }

    /// Queue a cmd looking along `view` (pitch down-positive, yaw, degrees;
    /// `None` holds the playerstate's) with `forward`/`right` keys and
    /// `buttons` over the base cmd's, at most one each [`CMD_MS`], and send
    /// a packet when one is due. Nothing is queued while a gamestate's `cp`
    /// is owed.
    pub fn send_move(
        &mut self,
        view: Option<[f32; 2]>,
        forward: i8,
        right: i8,
        buttons: i32,
    ) -> anyhow::Result<()> {
        self.advance_clock();
        if self.view.gamestates > 0
            && !self.cp_owed
            && since(self.last_cmd) >= Duration::from_millis(CMD_MS)
        {
            self.last_cmd = clock_now();
            let mut cmd = self.client.idle_cmd(self.cmd_time());
            if self
                .sent
                .back()
                .is_some_and(|c| c.server_time >= cmd.server_time)
            {
                return self.send_due();
            }
            // Before the first snapshot the base cmd's angles stand.
            if let Some(snap) = &self.view.snap
                && let Some(view) = view
            {
                for (i, v) in view.iter().enumerate() {
                    cmd.angles[i] = (angle_to_short(*v) - snap.ps.delta_angles[i]) & 0xffff;
                }
            }
            cmd.forward_move = forward;
            cmd.right_move = right;
            cmd.buttons |= buttons;
            self.keep(cmd);
        }
        self.send_due()
    }

    /// A packet when `CL_SendCmdInternal` (Mac 0x18a032) would send one:
    /// once a second before the gamestate, then every frame to a LAN
    /// server and every [`PACKET_MS`] off it. It carries the new cmds and,
    /// as `cl_packetdup` 1 does, the last packet's again, at most
    /// `MAX_PACKET_USERCMDS`.
    fn send_due(&mut self) -> anyhow::Result<()> {
        let lan = self.sock.as_ref().is_some_and(UdpTransport::is_lan);
        let wait = if self.view.gamestates == 0 {
            CONNECTING_PACKET_MS
        } else if lan {
            0
        } else {
            PACKET_MS
        };
        if since(self.last_packet) < Duration::from_millis(wait) {
            return Ok(());
        }
        // No new cmd: the clock stands (a cmd is held within 5 s of the
        // snapshot, `CL_CreateCmd` Mac 0x189f0e). Retail still makes one a
        // frame and sends it, so the newest goes again, at most at the
        // off-LAN pace: the server keeps hearing from us.
        if self.view.gamestates > 0
            && self.pending.is_empty()
            && !self.cp_owed
            && (self.sent.is_empty() || since(self.last_packet) < Duration::from_millis(PACKET_MS))
        {
            return Ok(());
        }
        let Some(sock) = &mut self.sock else {
            self.pending.clear();
            return Ok(());
        };
        self.last_packet = clock_now();
        let new = std::mem::take(&mut self.pending).len();
        let n =
            packet_cmds(new, &self.dup, self.sent.len()).max(usize::from(!self.sent.is_empty()));
        let cmds: Vec<UserCmd2> = self
            .sent
            .iter()
            .skip(self.sent.len() - n)
            .copied()
            .collect();
        self.dup.push_back(new);
        while self.dup.len() > PACKET_DUP {
            self.dup.pop_front();
        }
        let pkt = self.client.build_packet(&cmds)?;
        sock.send(&pkt);
        self.meter
            .sent(cmds.last().map(|c| c.server_time), self.last_packet);
        self.stats.packets_sent += 1;
        Ok(())
    }

    /// One frame of a spectator or a player: read the server, run the
    /// client clock, build the frame's cmds from [`Self::input`] and the
    /// camera's view, send what is due, predict, then put the camera at the
    /// predicted eye (or, unpredicted, the lerped one). The first snapshot
    /// of a level turns the camera to the server's view, and a
    /// `delta_angles` change (a spawn, the pitch cap) turns it by the
    /// change, as the client's view angles ride `delta_angles`.
    pub fn fly(&mut self, cam: &mut FlyCamera, fs: &Pk3Fs) {
        if self.demo.is_some() {
            return self.demo_fly(cam);
        }
        self.poll(Some(fs));
        self.advance_clock();
        self.process_snapshots();
        if let Some(s) = &self.view.snap {
            let delta = s.ps.delta_angles;
            if self.aimed_for != self.view.gamestates {
                self.aimed_for = self.view.gamestates;
                let [pitch, yaw, _] = s.ps.viewangles;
                cam.pitch = -pitch.to_radians();
                cam.yaw = yaw.to_radians();
            } else if let Some(old) = self.last_delta
                && old != delta
            {
                cam.pitch -= short_to_angle(delta[0] - old[0]).to_radians();
                cam.yaw += short_to_angle(delta[1] - old[1]).to_radians();
            }
            self.last_delta = Some(delta);
            self.input.sync(&s.ps.raw);
        }
        let view = [-cam.pitch.to_degrees(), cam.yaw.to_degrees()];
        let mut draw_cmd = None;
        if self.view.gamestates > 0 && !self.cp_owed && self.view.snap.is_some() {
            let now = self.cmd_time();
            let last = self.sent.back().map_or(i32::MIN, |c| c.server_time);
            // Past what the pump sent while the map loaded.
            for t in self.cmd_clock.due(now).into_iter().filter(|&t| t > last) {
                let c = self.play_cmd(t, view);
                self.keep(c);
            }
            if self.sent.back().is_none_or(|c| c.server_time < now) {
                draw_cmd = Some(self.play_cmd(now, view));
            }
        }
        // A primed client is sent no snapshot until it moves (cod2-protocol.md
        // 17.2): after a gamestate the map did not reload for (a download's,
        // a same-map one) idle cmds go out as the load pump's do.
        let primed = self.view.gamestates > 0 && !self.cp_owed && self.view.snap.is_none();
        let sent = if primed {
            self.send_move(None, 0, 0, 0)
        } else {
            self.send_due()
        };
        if let Err(e) = sent {
            log::warn!("cod2: send failed: {e:#}");
        }
        self.predict(fs, draw_cmd);
        self.jitter = self.gun_jitter(fs);
        let log_due = since(self.last_log) >= Duration::from_secs(10);
        if let Some((eye, _)) = self.view() {
            cam.pos = eye;
        } else if let Some(f) = self.frame() {
            cam.pos = self.snapshot_eye(&f);
        }
        if log_due && let Some(f) = self.frame() {
            let st = self.stats;
            log::info!(
                "cod2: serverTime {} snapshots {} ({} deltas) ents {} eye {:?} parse errors {} \
                 predicted {}",
                f.snap.server_time,
                st.snapshots,
                st.deltas,
                f.snap.entities.len(),
                cam.pos,
                st.parse_errors,
                self.predicted.is_some(),
            );
        }
        if log_due {
            self.last_log = clock_now();
        }
    }

    /// Predicts the local body for this frame, `draw_cmd` the cmd being
    /// built (drawn, not kept); the move's new events are queued for
    /// [`Self::take_events`].
    fn predict(&mut self, fs: &Pk3Fs, draw_cmd: Option<UserCmd2>) {
        self.predicted = None;
        self.ensure_world(fs);
        let Some(weapon) = self.view.snap.as_ref().map(|s| s.ps.weapon) else {
            return;
        };
        self.weapons.ensure(fs, &self.view.configstrings);
        let scale = self.move_scale(weapon);
        let now = self.real_ms();
        // `cg.nextSnap`'s solid entities where this frame draws them
        // (`CG_CalcEntityLerpPositions` runs before `CG_PredictPlayerState`).
        let bodies = match self.render_time().and_then(|t| self.view.frame(t)) {
            Some(f) if self.predict_bodies => {
                super::cod2_predict::solid_bodies(&f, f.snap.ps.client_num)
            }
            _ => Vec::new(),
        };
        let (Some(snap), Some((_, world))) = (&self.view.snap, &self.world) else {
            return;
        };
        if snap.ps.followed().is_some() {
            self.predictor.reset();
            return;
        }
        let Some(p) = self.predictor.predict(
            &snap.ps.raw,
            &self.sent,
            draw_cmd.as_ref(),
            world,
            &bodies,
            Some(self.weapons.params()),
            scale,
            now,
        ) else {
            return;
        };
        let time = self.clock.server_time();
        for &(event, parm) in &p.events {
            self.predicted_events.push(Event2 {
                entity: snap.ps.client_num as u32,
                event,
                parm,
                etype: vcod_cod2::net::events::ET_PLAYER,
                eflags: p.ps.e_flags as i32,
                weapon: i32::from(p.ws.weapon),
                surf: 0,
                other_entity: 0,
                time,
                scale: 0,
                angles2: [0.0; 2],
                pos: p.ps.origin,
                angles: Vec3::from(p.ps.view_angles),
                from_ps: true,
            });
        }
        // The playerstate drawn over `cg.nextSnap`'s entities.
        let mut shown = self
            .render_time()
            .and_then(|t| self.view.frame(t))
            .map_or_else(|| snap.clone(), |f| f.snap.clone());
        let mut raw = p.drawn.to_wire(&snap.ps.raw);
        p.drawn_ws.write_wire(&mut raw);
        shown.ps = PlayerView::new(&raw, snap.ps.own_client_num);
        shown.ps.origin = p.origin.to_array();
        self.predicted = Some((p, shown));
    }

    /// Once a gamestate, after [`Self::take_events`] started its effects
    /// over: every effect the level registers (`Cod2Fx::level_effects`),
    /// for the client to build ahead of their first play.
    pub fn take_level_effects(&mut self, fs: &dyn AssetSource) -> Option<Vec<String>> {
        if self.view.gamestates == 0 || self.fx_for == self.view.gamestates {
            return None;
        }
        self.fx_for = self.view.gamestates;
        self.weapons.ensure(fs, &self.view.configstrings);
        Some(
            self.fx
                .level_effects(&self.view.configstrings, &self.weapons, fs),
        )
    }

    /// The events of the snapshot that became current since the last call
    /// (the older of [`Self::frame`]'s two, as the cgame walks them), each
    /// once, for [`Self::sound_cues`] and [`Self::effects`].
    pub fn take_events(&mut self) -> Vec<Event2> {
        if self.events_for != self.view.gamestates {
            self.events_for = self.view.gamestates;
            self.events = EventTracker::default();
            self.fx = Cod2Fx::default();
        }
        // The move's own events come off the prediction instead.
        self.events.predicting = self.predicted.is_some();
        // `CG_SetNextSnap` (Mac 0x1dee08) checks a snapshot's events as it
        // becomes `cg.nextSnap`.
        let next = self
            .render_time()
            .and_then(|t| self.view.frame(t))
            .map(|f| f.snap);
        let mut out = match next {
            Some(cur) => self.events.transition(cur),
            None => Vec::new(),
        };
        out.append(&mut self.predicted_events);
        out
    }

    /// The own body's recoil, landing and steps for this frame: each of
    /// `events` (this frame's [`Self::take_events`]) that is its own shot
    /// kicks, then the kicks step to the frame's time; its own landing dips
    /// the eye ([`Self::view`]) and the gun ([`Self::land_gun`]), its
    /// `EV_STEP_VIEW` the eye, though not in a demo (`CG_EntityEvent`, Mac
    /// 0x1e4650, tests `cg + 8`, the demo flag). The next cmds
    /// carry the view kick ([`Self::play_cmd`]); [`Self::gun_kick`] turns
    /// the view weapon.
    pub fn recoil_frame(&mut self, events: &[Event2], fs: &Pk3Fs) {
        self.weapons.ensure(fs, &self.view.configstrings);
        let Some(t) = self.render_time() else {
            return;
        };
        let Some(mut f) = self.view.frame(t) else {
            return;
        };
        if let Some((_, snap)) = &self.predicted {
            f.snap = snap;
        }
        let ps = &f.snap.ps;
        let owner = (self.view.gamestates, ps.client_num);
        if self.recoil_for != owner {
            self.recoil_for = owner;
            self.recoil.reset();
            self.land = Default::default();
            self.step = Default::default();
        }
        let rules = &self.predictor.rules;
        let fall = (rules.fall_damage_min_height, rules.fall_damage_max_height);
        for e in events.iter().filter(|e| e.entity as i32 == ps.client_num) {
            if let Some(c) = crate::play::cod2_view::land_change(e.event, e.parm, fall) {
                self.land.land(c, f.time_ms);
            }
            if e.event == vcod_cod2::net::events::EV_STEP_VIEW && self.demo.is_none() {
                self.step.step(e.parm, f.time_ms);
            }
        }
        let eye = self.land.eye(f.time_ms);
        self.land_dips = (eye, self.land.gun(f.time_ms));
        self.step_dip = self.step.eye(f.time_ms);
        let weapon = crate::play::cod2_recoil::kick_weapon(ps);
        let def = self.weapons.get(weapon).map(|w| &w.kick);
        self.recoil.frame(events, ps, def, f.time_ms);
    }

    /// The gun spring's pitch and yaw, degrees, which turn the view weapon
    /// (`play::cod2_gun::GunInput::recoil`).
    pub fn gun_kick(&self) -> [f32; 2] {
        self.recoil.gun_kick()
    }

    /// This frame's landing drop of the view weapon on its world z
    /// (`play::cod2_view::Land::gun`).
    pub fn land_gun(&self) -> f32 {
        self.land_dips.1
    }

    /// What `events` play heard at `view`, their fire sounds and whizbys
    /// starting at `muzzles` (`cod2_scene::Cod2Draw::muzzles`), and
    /// configstring 3 as `cod2_sound::ambient` reads it.
    pub fn sound_cues(
        &mut self,
        events: &[Event2],
        fs: &Pk3Fs,
        view: Vec3,
        muzzles: &HashMap<u32, Vec3>,
    ) -> (Vec<Cue>, (String, Option<String>, i32)) {
        self.sounds.muzzles.clone_from(muzzles);
        self.weapons.ensure(fs, &self.view.configstrings);
        let cur = self
            .render_time()
            .and_then(|t| self.view.frame(t))
            .map(|f| f.snap);
        let ambient = crate::play::cod2_sound::ambient(
            &self.view.configstrings,
            cur.map_or(0, |s| s.server_time),
        );
        let Some(cur) = cur else {
            return (Vec::new(), ambient);
        };
        let cues = events
            .iter()
            .flat_map(|e| {
                self.sounds
                    .cues(e, cur, &self.view.configstrings, &self.weapons, view)
            })
            .collect();
        (cues, ambient)
    }

    /// The loop sounds and the missiles' projectile sounds at the render
    /// time ([`Cod2Sounds::loops`]).
    pub fn loop_sounds(&mut self, fs: &Pk3Fs) -> (LoopSounds, LoopSounds) {
        self.weapons.ensure(fs, &self.view.configstrings);
        let Some(t) = self.render_time() else {
            return Default::default();
        };
        match self.view.frame(t) {
            Some(f) => self.sounds.loops(&f, &self.weapons),
            None => Default::default(),
        }
    }

    /// The sound blend entities at the render time
    /// ([`Cod2Sounds::blends`]).
    pub fn sound_blends(&self) -> Vec<crate::audio::blend::BlendSource> {
        let Some(t) = self.render_time() else {
            return Vec::new();
        };
        self.view
            .frame(t)
            .map_or_else(Vec::new, |f| Cod2Sounds::blends(&f))
    }

    /// The sound server commands since the last call, oldest first.
    pub fn take_sound_commands(&mut self) -> Vec<SoundCommand> {
        std::mem::take(&mut self.sound_commands)
    }

    /// The effects `events` play this frame, and the looping fx entities
    /// due at the render time, seen from `view` (`play::cod2_fx`); `scene`
    /// is the drawn bodies' tags (`cod2_scene::Cod2Draw::fx_scene`).
    pub fn effects(
        &mut self,
        events: &[Event2],
        fs: &dyn AssetSource,
        view: Vec3,
        scene: crate::play::cod2_fx::FxScene,
    ) -> Vec<FxPlay> {
        self.weapons.ensure(fs, &self.view.configstrings);
        let Some(t) = self.render_time() else {
            return Vec::new();
        };
        match self.view.frame(t) {
            Some(f) => self
                .fx
                .frame(&f, events, view, Some(fs), &self.weapons, scene),
            None => Vec::new(),
        }
    }

    /// The `viewFlashEffect` of CS 7 weapon `weapon`, what a held view
    /// flash plays on the gun placed (`play::cod2_fx::Play::ViewFlash`).
    pub fn view_flash_effect(&mut self, weapon: i32, fs: &dyn AssetSource) -> Option<String> {
        self.weapons.ensure(fs, &self.view.configstrings);
        self.weapons.get(weapon)?.fx.view_flash.clone()
    }

    /// The level's weapon files, read off `fs` unless held already
    /// ([`LevelWeapons::ensure`]).
    pub fn ensure_weapons(&mut self, fs: &dyn AssetSource) {
        self.weapons.ensure(fs, &self.view.configstrings);
    }

    /// The level's weapon files as [`Self::ensure_weapons`] last read them.
    pub fn weapons(&self) -> &LevelWeapons {
        &self.weapons
    }

    /// A userinfo dvar changed (`name`, `rate`, `snaps`, `password`,
    /// `cl_wwwDownload`): `CL_Frame` (Mac 0x14b182) sends the whole
    /// userinfo as `userinfo "<info>"` on the reliable stream the frame
    /// after, once connected (cod2-protocol.md 18.3). Nothing goes when
    /// nothing changed.
    pub fn set_userinfo(&mut self, info: &vcod_common::net::Userinfo) {
        let opts = self.opts.clone().with_userinfo(info);
        let ui = opts.userinfo();
        if ui == self.opts.userinfo() {
            return;
        }
        self.opts = opts;
        if self.sock.is_some() {
            self.client.add_reliable(&format!("userinfo \"{ui}\""));
        }
    }

    /// `cl_voice` changed: the userinfo goes again, as [`Self::set_userinfo`]
    /// sends it, and frames heard from now on are dropped while it is off
    /// (`CL_VoicePacket`).
    pub fn set_cl_voice(&mut self, on: bool) {
        self.voice.enabled = on;
        if self.opts.voice == on {
            return;
        }
        self.opts.voice = on;
        if self.sock.is_some() {
            let ui = self.opts.userinfo();
            self.client.add_reliable(&format!("userinfo \"{ui}\""));
        }
    }

    /// `disconnect`, sent three times as `CL_Disconnect` does.
    /// `CL_Disconnect` (Mac 0x14b8ea) stops a recording first.
    pub fn disconnect(&mut self) {
        if self.is_recording() {
            crate::console::log::print(&self.stop_recording());
        }
        let Some(sock) = &mut self.sock else {
            return;
        };
        self.client.add_reliable("disconnect");
        for _ in 0..3 {
            if let Ok(p) = self.client.build_packet(&[]) {
                sock.send(&p);
            }
        }
        // Under `VCOD_NETSIM` they would die in the simulator's queue.
        sock.drain();
    }
}

/// A session read and kept alive on a thread of its own while the main
/// thread loads: the map's materials can take longer than `sv_timeout` on a
/// software renderer. [`Pump::finish`] hands the session back.
pub struct Pump {
    stop: Arc<AtomicBool>,
    handle: Option<JoinHandle<Box<Cod2Session>>>,
}

impl Cod2Session {
    /// Moves the session onto a thread that reads every snapshot and sends
    /// a cmd holding the playerstate's view each [`PACKET_MS`].
    pub fn pump(self: Box<Self>) -> Pump {
        let stop = Arc::new(AtomicBool::new(false));
        let flag = stop.clone();
        // A live test's clock holds while the map loads: this thread reads
        // it but sleeps on the wall.
        #[cfg(test)]
        let clock = crate::play::cod4::tests::sim::current();
        let handle = std::thread::spawn(move || {
            #[cfg(test)]
            if clock.is_some() {
                crate::play::cod4::tests::sim::install(clock);
            }
            let mut s = self;
            let (start, messages) = (Instant::now(), s.stats.messages);
            while !flag.load(Ordering::Relaxed) {
                // A panic here would lose the session and take the main
                // thread's join down with it: the game ends with an error
                // instead.
                let step = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    s.poll(None);
                    s.send_move(None, 0, 0, 0)
                }));
                match step {
                    Ok(Ok(())) => {}
                    Ok(Err(e)) => log::warn!("cod2: send failed: {e:#}"),
                    Err(_) => {
                        s.dropped = Some("cod2: the connection failed while the map loaded".into());
                        break;
                    }
                }
                std::thread::sleep(Duration::from_millis(2));
            }
            log::info!(
                "cod2: kept the connection through {:.1} s of loading, {} messages",
                start.elapsed().as_secs_f32(),
                s.stats.messages - messages
            );
            s
        });
        Pump {
            stop,
            handle: Some(handle),
        }
    }
}

impl Pump {
    pub fn finish(mut self) -> Box<Cod2Session> {
        self.stop.store(true, Ordering::Relaxed);
        let handle = self.handle.take().expect("finished once");
        handle.join().expect("the cod2 pump thread panicked")
    }
}

impl Drop for Pump {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

/// Why a connection ended in the server's (or the download's) own words,
/// an `EXE_*` key the error popup localizes, as retail's `Com_Error` shows it.
#[derive(Debug)]
pub struct ServerError(pub String);

impl std::fmt::Display for ServerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for ServerError {}

/// The search path over `home`'s `main/` and, when set, the `fs_game`
/// directory beside it, as `FS_Startup` builds it.
pub fn open_search(home: &Path, fs_game: Option<&str>) -> anyhow::Result<Pk3Fs> {
    let game = fs_game.map(|g| home.join(g));
    Pk3Fs::open_iwd(&home.join("main"), game.as_deref())
}

/// The mod directory `fs` searches over `main/`, if any.
pub fn fs_game_of(fs: &Pk3Fs) -> Option<String> {
    fs.dirs()
        .get(1)
        .and_then(|d| d.file_name())
        .map(|n| n.to_string_lossy().into_owned())
}

impl Cod2Session {
    /// `FS_ConditionalRestart` and `CL_DownloadsComplete`'s `FS_Restart`:
    /// whether the search path must open again before the map loads,
    /// because a download landed or the gamestate's `fs_game` is not
    /// `current`.
    pub fn take_reopen(&mut self, current: Option<&str>) -> bool {
        // A demo keeps the player's search path (`CL_SystemInfoChanged`).
        if self.demo.is_some() {
            return false;
        }
        let fetched = !self.take_fetched().is_empty();
        let same = match (self.fs_game.as_deref(), current) {
            (Some(a), Some(b)) => a.eq_ignore_ascii_case(b),
            (a, b) => a.is_none() && b.is_none(),
        };
        let reopen = fetched || !same;
        // The files came off the old search path.
        if reopen {
            self.weapons.clear();
        }
        reopen
    }
}

/// The prediction's collision world over a map's world and ClipMap, with
/// the server's stored floats (play::cod2_predict).
pub fn prediction_world(
    w: &vcod_common::mapworld::MapWorld,
    clip: Arc<vcod_cod2::collision::ClipMap>,
) -> CollisionWorld {
    let cw = vcod_cod2::collision::collision_world(w, clip, &[]);
    cw.set_arith(vcod_common::patch::Arith::Stored);
    cw
}

/// What Enter on the `messagemode` / `messagemode2` field sends: `say` or
/// `say_team` and the line quoted, as the console's field does
/// (`Con_ChatModePublic_f` / `Con_ChatModeTeam_f` pick the word); nothing
/// for an empty line. CoD1's field sends the same.
pub fn chat_command(team: bool, line: &str) -> Option<String> {
    if line.is_empty() {
        return None;
    }
    let word = if team { "say_team" } else { "say" };
    Some(format!("{word} \"{line}\""))
}
#[cfg(test)]
mod tests {
    use super::*;

    /// A map whose collision failed is not read again until a new
    /// gamestate, and a map change the caller has yet to take is left to
    /// its load. Needs `COD2_DIR`.
    #[test]
    fn prediction_builds_a_maps_collision_once_per_gamestate() {
        let Some(stock) = vcod_common::testing::cod2_fs() else {
            return;
        };
        let bsp = stock.read("maps/mp/mp_toujane.d3dbsp").unwrap();
        let mut s = Cod2Session::new(None, Client::new(0, 0));
        let gs = |map: &str| vcod_cod2::net::Gamestate2 {
            server_command_sequence: 0,
            configstrings: [(0u16, format!("\\mapname\\{map}"))].into_iter().collect(),
            baselines: Default::default(),
            client_num: 0,
            checksum_feed: 0,
        };
        s.view.configstrings = vcod_cod2::net::ConfigStrings::from_gamestate(&gs("mp_x"));
        s.view.gamestates = 1;
        s.level_gamestates = 1;
        s.loaded_map = "mp_x".into();
        // Not in the search path: the build fails.
        let mut fs = Pk3Fs::empty();
        s.ensure_world(&fs);
        assert!(s.world.is_none());
        // There now, but the same gamestate does not read it again.
        fs.overlay("maps/mp/mp_x.d3dbsp", bsp);
        s.ensure_world(&fs);
        assert!(s.world.is_none());
        // A new gamestate for another map waits for the caller's load.
        s.view.configstrings = vcod_cod2::net::ConfigStrings::from_gamestate(&gs("mp_y"));
        s.view.gamestates = 2;
        s.ensure_world(&fs);
        assert!(s.world.is_none());
        // Back on mp_x, a new gamestate builds it.
        s.view.configstrings = vcod_cod2::net::ConfigStrings::from_gamestate(&gs("mp_x"));
        s.ensure_world(&fs);
        assert!(s.world.as_ref().is_some_and(|(m, _)| m == "mp_x"));
    }

    /// A front-end connect dropped once vcod-server holds its slot and has
    /// sent the gamestate (Esc, or a second `connect`) says `disconnect`:
    /// the slot frees at once instead of timing out. One dropped before
    /// the gamestate carries no `sv_serverid`, and the server reads
    /// nothing of it, as retail's does. Needs `COD2_DIR`.
    #[test]
    fn cod2_live_a_dropped_connect_frees_the_server_slot() {
        use vcod_server::cod2::{Cod2Config, Cod2Server};
        let Some(fs) = vcod_common::testing::cod2_fs() else {
            return;
        };
        let fs = std::rc::Rc::new(fs);
        let mut sv = Cod2Server::new(
            Cod2Config {
                map: "mp_toujane".into(),
                gametype: "dm".into(),
                hostname: "drop".into(),
                max_clients: 4,
                gametype_script: None,
            },
            7,
            Instant::now(),
        );
        sv.exec_default_cfg(&*fs);
        sv.set_cvar("sv_pure", "0");
        sv.load(Some(fs)).unwrap();
        let opts = ConnectOpts {
            name: "dropper".into(),
            allow_download: false,
            www_download: false,
            ..ConnectOpts::default()
        };
        // The server's client count at each frame.
        let count = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let seen = count.clone();
        let (connected, freed) = crate::play::cod4::tests::run_held(
            &mut sv,
            move |addr| {
                let mut connecting = Some(Cod2Session::connect_in_background(
                    &addr.to_string(),
                    opts,
                    Duration::from_secs(15),
                ));
                let t0 = clock_now();
                let mut joined = None;
                while since(t0) < Duration::from_secs(30) {
                    let n = seen.load(Ordering::Relaxed);
                    if joined.is_none() && n == 1 {
                        joined = Some(clock_now());
                    }

                    // The gamestate is a few frames out on loopback.
                    if joined.is_some_and(|t| since(t) > Duration::from_millis(500)) {
                        connecting = None;
                    }
                    if connecting.is_none() && n == 0 {
                        return (true, true);
                    }
                    // One thread moves the clock at a time: the join while
                    // it runs, this one after.
                    if connecting.as_ref().is_some_and(Connecting::running) {
                        std::thread::sleep(Duration::from_millis(1));
                    } else {
                        crate::play::cod4::tests::clk_sleep(Duration::from_millis(16));
                    }
                }
                (connecting.is_none(), false)
            },
            |sv, _, _| {
                count.store(sv.client_count(), Ordering::Relaxed);
                true
            },
        );
        assert!(connected, "the connect never took a slot");
        assert!(freed, "the server kept the dropped connect's slot");
    }

    /// `vcod --connect` on CoD2 joins through `connect_in_background`, as
    /// the console's `connect` does: the call returns before the server
    /// has read a packet, and the session it hands over stands where the
    /// blocking connect left one. Needs `COD2_DIR`.
    #[test]
    fn cod2_live_a_background_connect_hands_over_the_blocking_connects_session() {
        const CONNECT_SECS: Duration = Duration::from_secs(15);
        use vcod_server::cod2::{Cod2Config, Cod2Server, FRAME_MS};
        let Some(fs) = vcod_common::testing::cod2_fs() else {
            return;
        };
        let fs = std::rc::Rc::new(fs);
        let mut sv = Cod2Server::new(
            Cod2Config {
                map: "mp_toujane".into(),
                gametype: "dm".into(),
                hostname: "bg".into(),
                max_clients: 4,
                gametype_script: None,
            },
            7,
            Instant::now(),
        );
        sv.exec_default_cfg(&*fs);
        sv.set_cvar("sv_pure", "0");
        sv.load(Some(fs)).unwrap();
        let sock = UdpSocket::bind("127.0.0.1:0").unwrap();
        sock.set_nonblocking(true).unwrap();
        let addr = sock.local_addr().unwrap().to_string();
        let opts = ConnectOpts {
            name: "joiner".into(),
            allow_download: false,
            www_download: false,
            ..ConnectOpts::default()
        };
        let mut next = Instant::now();
        // Serves the server until `done` holds, at most 30 s.
        let mut serve = |sv: &mut Cod2Server, done: &mut dyn FnMut(&Cod2Server) -> bool| {
            let mut buf = [0u8; 4096];
            let t0 = Instant::now();
            while !done(sv) {
                assert!(t0.elapsed() < Duration::from_secs(30), "served 30 s");
                while let Ok((n, from)) = sock.recv_from(&mut buf) {
                    sv.handle_packet(from, &buf[..n], Instant::now());
                }
                let now = Instant::now();
                if now >= next {
                    next = now + Duration::from_millis(FRAME_MS as u64);
                    sv.tick_frames(now, 1);
                }
                for (to, p) in sv.take_outgoing() {
                    let _ = sock.send_to(&p, to);
                }
                std::thread::sleep(Duration::from_millis(1));
            }
        };
        let state = |s: &Cod2Session| {
            (
                s.settled(),
                s.map_name().map(str::to_string),
                s.view.gamestates,
                s.level_gamestates,
                s.loaded_map.clone(),
                s.fs_game().map(str::to_string),
            )
        };
        let (a, o) = (addr.clone(), opts.clone());
        let blocking = std::thread::spawn(move || Cod2Session::connect(&a, &o, CONNECT_SECS));
        serve(&mut sv, &mut |_| blocking.is_finished());
        let mut first = blocking.join().unwrap().unwrap();
        let want = state(&first);
        first.disconnect();
        serve(&mut sv, &mut |sv| sv.client_count() == 0);
        let connecting = Cod2Session::connect_in_background(&addr, opts, CONNECT_SECS);
        assert!(connecting.poll().is_none(), "the connect ran on the caller");
        let mut got = None;
        serve(&mut sv, &mut |_| {
            got = got.take().or_else(|| connecting.poll());
            got.is_some()
        });
        let mut s = got.unwrap().unwrap();
        assert_eq!(state(&s), want);
        assert_eq!(
            want,
            (
                true,
                Some("mp_toujane".into()),
                1,
                1,
                "mp_toujane".into(),
                None
            )
        );
        s.disconnect();
    }

    /// Voice on the session's socket: the systeminfo's `sv_voice` and
    /// `sv_voiceQuality`, a server `v` stamping its talkers but a muted
    /// one, the mute flip's `muteplayer` / `unmuteplayer`, and the batch out
    /// on our qport the moment the first frame waits (cod2-voice.md 4, 5).
    #[test]
    fn cod2_voice_reaches_the_session_and_leaves_on_its_qport() {
        use vcod_cod2::net::voice;
        let server = UdpSocket::bind("127.0.0.1:0").unwrap();
        server
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        let sock = UdpSocket::bind("127.0.0.1:0").unwrap();
        sock.connect(server.local_addr().unwrap()).unwrap();
        let me = sock.local_addr().unwrap();
        let mut s = Cod2Session::new(Some(sock), Client::new(0x1234, 2));
        assert_eq!(s.voice_settings(), (false, 1), "no gamestate: off, 1");
        let gs = vcod_cod2::net::Gamestate2 {
            server_command_sequence: 0,
            configstrings: [(1u16, "\\sv_voice\\1\\sv_voiceQuality\\4".to_string())]
                .into_iter()
                .collect(),
            baselines: Default::default(),
            client_num: 0,
            checksum_feed: 0,
        };
        s.view.configstrings = vcod_cod2::net::ConfigStrings::from_gamestate(&gs);
        assert_eq!(s.voice_settings(), (true, 4));

        let seq = s.client.reliable_sequence;
        s.toggle_mute(5);
        assert_eq!(s.client.reliable_command(seq + 1), b"muteplayer 5");
        let heard = voice::server_packet(&[(3, vec![1, 2]), (5, vec![3])]);
        server.send_to(&heard, me).unwrap();
        let until = Instant::now() + Duration::from_secs(2);
        while s.talkers().is_empty() && Instant::now() < until {
            s.poll(None);
        }
        assert_eq!(s.talkers(), vec![3], "a muted talker's frame is dropped");
        assert_eq!(s.voice.take_heard(), vec![(3, vec![1, 2])]);
        s.toggle_mute(5);
        assert_eq!(s.client.reliable_command(seq + 2), b"unmuteplayer 5");
        assert!(!s.muted[5]);

        s.voice.queue(vec![9; 10]);
        s.transmit_voice();
        let mut buf = [0u8; 512];
        let n = server.recv(&mut buf).unwrap();
        assert_eq!(&buf[..n], voice::client_packet(0x1234, &[vec![9; 10]]));
    }

    /// A session on a gamestate whose systeminfo is `si`, downloading
    /// into `home`.
    fn session_on(si: &str, home: &Path, allow: bool) -> Cod2Session {
        let sock = UdpSocket::bind("127.0.0.1:0").unwrap();
        let mut s = Cod2Session::new(Some(sock), Client::new(1, 2));
        s.opts = ConnectOpts {
            name: "t".into(),
            home: Some(home.to_path_buf()),
            allow_download: allow,
            www_download: false,
            ..ConnectOpts::default()
        };
        let gs = vcod_cod2::net::Gamestate2 {
            server_command_sequence: 0,
            configstrings: [(1u16, si.to_string())].into_iter().collect(),
            baselines: Default::default(),
            client_num: 0,
            checksum_feed: 0,
        };
        s.view.configstrings = vcod_cod2::net::ConfigStrings::from_gamestate(&gs);
        s.view.gamestates = 1;
        s.on_gamestate();
        s
    }

    fn write_iwd(path: &Path, entries: &[(&str, &[u8])]) -> i32 {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, vcod_common::testing::pk3_with(entries)).unwrap();
        vcod_common::pak_checksum::pak_checksums(path, 0)
            .unwrap()
            .checksum
    }

    /// `CL_InitDownloads` on the public server's lists: the held iwd and
    /// the stock one are passed over, the mod's is asked for and the map
    /// waits; without `cl_allowDownload` the map loads at once.
    #[test]
    fn a_gamestate_downloads_what_the_homepath_lacks_before_the_map() {
        let home = tempfile::tempdir().unwrap();
        let held = write_iwd(&home.path().join("main/held.iwd"), &[("a.txt", b"a")]);
        let si = format!(
            "\\fs_game\\veteran\\sv_referencedIwds\\{held} 5 6 \\sv_referencedIwdNames\\main/held main/iw_03 veteran/rt"
        );
        let s = session_on(&si, home.path(), true);
        assert_eq!(s.fs_game(), Some("veteran"));
        assert!(!s.settled());
        let seq = s.client.reliable_sequence;
        assert_eq!(s.client.reliable_command(seq), b"download veteran/rt.iwd");
        assert!(home.path().join("veteran").is_dir());

        let s = session_on(&si, home.path(), false);
        assert!(s.settled());
    }

    /// `FS_ConditionalRestart` and the restart after downloads: the search
    /// path opens again when a file landed or `fs_game` moved, and the
    /// reopened one finds the map inside the mod's iwd.
    #[test]
    fn the_search_path_reopens_over_the_mod_and_its_download() {
        let home = tempfile::tempdir().unwrap();
        write_iwd(&home.path().join("main/iw_00.iwd"), &[("a.txt", b"a")]);
        let fs = open_search(home.path(), None).unwrap();
        assert_eq!(fs_game_of(&fs), None);
        assert!(fs.read("maps/mp/mp_dltest.d3dbsp").is_none());

        let mut s = session_on("\\sv_pure\\0", home.path(), true);
        assert!(!s.take_reopen(None));
        assert!(s.take_reopen(Some("veteran")));
        s.fs_game = Some("Veteran".into());
        assert!(!s.take_reopen(Some("veteran")));
        s.fs_game = Some("veteran".into());
        let rt = home.path().join("veteran/rt.iwd");
        write_iwd(&rt, &[("maps/mp/mp_dltest.d3dbsp", b"IBSP")]);
        s.fetched.push(rt);
        assert!(s.take_reopen(Some("veteran")));
        assert!(!s.take_reopen(Some("veteran")));

        let fs = open_search(home.path(), s.fs_game()).unwrap();
        assert_eq!(fs_game_of(&fs).as_deref(), Some("veteran"));
        assert_eq!(
            fs.read("maps/mp/mp_dltest.d3dbsp").as_deref(),
            Some(&b"IBSP"[..])
        );
    }

    /// A flood of datagrams is read `max` at a time; the rest wait.
    #[test]
    fn a_poll_reads_a_bounded_number_of_datagrams() {
        let sock = UdpSocket::bind("127.0.0.1:0").unwrap();
        let me = sock.local_addr().unwrap();
        let mut s = Cod2Session::new(Some(sock), Client::new(1, 2));
        let flood = UdpSocket::bind("127.0.0.1:0").unwrap();
        for _ in 0..50 {
            flood.send_to(b"x", me).unwrap();
        }
        s.poll_at_most(None, 8);
        let mut buf = [0u8; 16];
        let left = std::iter::from_fn(|| s.sock.as_mut().unwrap().try_recv(&mut buf)).count();
        assert_eq!(left, 42);
    }

    fn session(join: Option<Join>) -> Cod2Session {
        let sock = UdpSocket::bind("127.0.0.1:0").unwrap();
        let mut s = Cod2Session::new(Some(sock), Client::new(1, 2));
        s.join = join;
        s.client.set_systeminfo("\\sv_pure\\1\\sv_serverid\\32");
        let gs = vcod_cod2::net::Gamestate2 {
            server_command_sequence: 0,
            configstrings: Default::default(),
            baselines: Default::default(),
            client_num: 0,
            checksum_feed: 0,
        };
        s.view.configstrings = vcod_cod2::net::ConfigStrings::from_gamestate(&gs);
        for (slot, menu) in [
            (1247, "team_americangerman"),
            (1248, "weapon_american"),
            (1250, "serverinfo_dm"),
        ] {
            s.view
                .configstrings
                .apply_command(format!("d {slot} {menu}").as_bytes());
        }
        s
    }

    fn allies(weapon: Option<&str>) -> Option<Join> {
        Some(Join {
            team: "allies".into(),
            weapon: weapon.map(Into::into),
        })
    }

    /// The view kick rides the cmd's three angles, over the view the keys
    /// and mouse set (`CL_CreateCmd`, Mac 0x189f0e).
    #[test]
    fn the_cmd_carries_the_view_kick() {
        let mut s = session(None);
        s.view.snap = Some(SnapshotView {
            message_num: 1,
            server_time: 1000,
            snap_flags: 0,
            ps: PlayerView::new(&vcod_cod2::net::PlayerState2::default(), 0),
            entities: Vec::new(),
            clients: Vec::new(),
        });
        let still = s.play_cmd(1000, [10.0, 20.0]);
        assert_eq!(
            still.angles,
            [angle_to_short(10.0), angle_to_short(20.0), 0]
        );
        s.recoil = crate::play::cod2_recoil::kicked();
        let kick = s.recoil.view_kick();
        assert!(kick[0] < -0.5, "the view kicked up, {kick:?}");
        let cmd = s.play_cmd(1000, [10.0, 20.0]);
        assert_eq!(
            cmd.angles,
            [
                angle_to_short(10.0 + kick[0]),
                angle_to_short(20.0 + kick[1]),
                angle_to_short(kick[2]),
            ]
        );
    }

    #[test]
    fn a_join_answers_the_stock_menus_in_turn() {
        let mut s = session(allies(None));
        let seq = s.client.reliable_sequence;
        // serverinfo, team, weapon: the order _menus.gsc opens them in.
        for t in [&b"t 4"[..], b"t 1", b"t 2"] {
            s.server_command(t);
        }
        assert_eq!(s.stats.menu_replies, 3);
        assert_eq!(s.client.reliable_sequence, seq + 3);
        // A menu no join answers (quick chat) is left alone.
        s.view.configstrings.apply_command(b"d 1253 quickcommands");
        s.server_command(b"t 7");
        assert_eq!(s.stats.menu_replies, 3);
    }

    /// Retail's use-key swap (`vcod-cod2`'s `pickup/carentan-swap.txt`):
    /// a kar98k (4) in hand beside a thompson (5), the press takes an mp40
    /// (6). The snapshot that carries `a 6` has the hand empty and 5 and 6
    /// held; the selection follows the `a`, so the next cmd asks for the
    /// mp40 and pmove raises it, where the old selection named a weapon no
    /// longer held and left the hand empty.
    #[test]
    fn the_swaps_a_selects_the_new_weapon() {
        use vcod_cod2::net::delta::field_index;
        use vcod_cod2::net::fields::PLAYER_STATE_FIELDS;
        let ps = |weapon: u32, held: u32, slots: u32| {
            let mut p = vcod_cod2::net::PlayerState2::default();
            let mut set = |n: &str, v: u32| p.f[field_index(&PLAYER_STATE_FIELDS, n)] = v;
            set("weapon", weapon);
            set("weapons[0]", held);
            set("weaponslots[0]", slots);
            p.stats[5] = 1;
            p
        };
        let mut s = session(None);
        s.input.sync(&ps(4, 0x30, 0x50400));
        assert_eq!(s.input.weapons().0, 4);
        s.server_command(b"a 6");
        s.input.sync(&ps(0, 0x60, 0x50600));
        assert_eq!(s.input.weapons().0, 6);
        s.input.sync(&ps(6, 0x60, 0x50600));
        assert_eq!(s.input.weapons().0, 6);
    }

    /// A typed line goes out as `say` (T) or `say_team` (Y), quoted, on
    /// the reliable stream; an empty one sends nothing.
    #[test]
    fn a_chat_line_routes_to_say() {
        assert_eq!(
            chat_command(false, "hi all").as_deref(),
            Some("say \"hi all\"")
        );
        assert_eq!(
            chat_command(true, "go b").as_deref(),
            Some("say_team \"go b\"")
        );
        assert_eq!(chat_command(false, ""), None);
        let mut s = session(None);
        let seq = s.client.reliable_sequence;
        s.send_command(&chat_command(false, "hi all").unwrap());
        assert_eq!(s.client.reliable_sequence, seq + 1);
    }

    /// `h` and `i` (G_Say's lines) land in the chat queue with their team
    /// flag, quotes off; a `b` does not.
    #[test]
    fn chat_server_commands_are_kept() {
        let mut s = session(None);
        s.server_command("h \"\u{15}p1^7: ^7hello there\"".as_bytes());
        s.server_command("i \"\u{15}(\u{14}GAME_ALLIES\u{15})p2^7: ^5go\"".as_bytes());
        s.server_command(b"b 0 0 0");
        assert_eq!(
            s.take_chat(),
            vec![
                (false, "\u{15}p1^7: ^7hello there".to_string()),
                (
                    true,
                    "\u{15}(\u{14}GAME_ALLIES\u{15})p2^7: ^5go".to_string()
                ),
            ]
        );
        assert!(s.take_chat().is_empty());
    }

    /// A join answers each menu once; the next `t` for it (the weapon
    /// menu `openscriptmenu ingame changeweapon` brings back), `u` and
    /// `v` are the player's UI's.
    #[test]
    fn a_menu_answered_once_goes_to_the_player_next_time() {
        let mut s = session(allies(None));
        s.server_command(b"t 2");
        assert_eq!(s.stats.menu_replies, 1);
        assert!(s.take_menu_commands().is_empty());
        s.server_command(b"t 2");
        s.server_command(b"u");
        s.server_command(b"K");
        s.server_command(b"v g_scriptMainMenu \"ingame\"");
        assert_eq!(s.stats.menu_replies, 1);
        assert_eq!(
            s.take_menu_commands(),
            [
                vec!["t".to_string(), "2".into()],
                vec!["u".to_string()],
                vec!["K".to_string()],
                vec!["v".to_string(), "g_scriptMainMenu".into(), "ingame".into()],
            ]
        );
        // A restart rearms the join.
        s.server_command(b"B");
        s.server_command(b"t 2");
        assert_eq!(s.stats.menu_replies, 2);
    }

    /// `openscriptmenu` and a menu's response name the menu's slot from
    /// 1246 without case, -1 for none (`CL_OpenScriptMenu_f`).
    #[test]
    fn a_menu_response_names_the_menus_slot() {
        let mut s = session(None);
        let seq = s.client.reliable_sequence;
        s.menu_response("WEAPON_american", "thompson_mp");
        s.menu_response("nosuch", "x");
        assert_eq!(s.client.reliable_sequence, seq + 2);
        let id = s.server_id();
        let sent: Vec<String> = [seq + 1, seq + 2]
            .map(|n| String::from_utf8_lossy(s.client.reliable_command(n)).into_owned())
            .into();
        assert_eq!(
            sent,
            [format!("mr {id} 2 thompson_mp"), format!("mr {id} -1 x")]
        );
    }

    #[test]
    fn a_spectator_answers_no_menu() {
        let mut s = session(None);
        s.server_command(b"t 1");
        assert_eq!(s.stats.menu_replies, 0);
    }

    #[test]
    fn join_responses() {
        let j = allies(None).unwrap();
        assert_eq!(j.response("serverinfo_sd").as_deref(), Some("close"));
        assert_eq!(j.response("team_britishgerman").as_deref(), Some("allies"));
        assert_eq!(j.response("weapon_german").as_deref(), Some("kar98k_mp"));
        assert_eq!(
            j.response("weapon_russian").as_deref(),
            Some("mosin_nagant_mp")
        );
        assert_eq!(j.response("callvote"), None);
        let j = allies(Some("thompson_mp")).unwrap();
        assert_eq!(
            j.response("weapon_american").as_deref(),
            Some("thompson_mp")
        );
    }

    #[test]
    fn fast_restart_and_drops() {
        let mut s = session(None);
        s.server_command(b"B");
        assert!(s.take_restart());
        assert!(!s.take_restart());
        s.server_command(b"n");
        assert_eq!(s.stats.restarts, 2);
        // A configstring that only starts with n is no restart.
        s.server_command(b"nx");
        assert_eq!(s.stats.restarts, 2);
        s.server_command(b"w \"EXE_PLAYERKICKED\"");
        assert_eq!(s.dropped.as_deref(), Some("EXE_PLAYERKICKED"));
    }

    #[test]
    fn a_systeminfo_update_moves_the_server_id() {
        let mut s = session(None);
        assert_eq!(s.client.server_id(), 32);
        s.client.set_systeminfo("\\sv_serverid\\33\\sv_pure\\1");
        assert_eq!(s.client.server_id(), 33);
        // No sv_serverid key: the id stands.
        s.client.set_systeminfo("\\sv_pure\\1");
        assert_eq!(s.client.server_id(), 33);
    }

    #[test]
    fn short_differences_wrap() {
        assert_eq!(short_to_angle(0x4000), 90.0);
        assert_eq!(short_to_angle(-0x4000), -90.0);
        assert_eq!(short_to_angle(0xc000), -90.0);
    }

    #[test]
    fn a_packet_carries_the_last_packets_cmds_again() {
        let mut dup = VecDeque::new();
        assert_eq!(packet_cmds(2, &dup, 2), 2);
        dup.push_back(2);
        assert_eq!(packet_cmds(3, &dup, 5), 5);
        assert_eq!(packet_cmds(3, &dup, 4), 4, "only what is kept");
        dup[0] = 40;
        assert_eq!(packet_cmds(3, &dup, 128), 32);
    }

    #[test]
    fn interrupted_once_the_oldest_kept_cmd_is_unrun() {
        let mut s = session(None);
        let mut ps = PlayerView::new(&vcod_cod2::net::PlayerState2::default(), 0);
        ps.command_time = 980;
        s.view.snap = Some(SnapshotView {
            message_num: 1,
            server_time: 1000,
            snap_flags: 0,
            ps,
            entities: Vec::new(),
            clients: Vec::new(),
        });
        for t in 863..990 {
            s.keep(s.client.idle_cmd(t));
        }
        assert!(!s.interrupted(), "127 cmds: the slot reads 0");
        s.keep(s.client.idle_cmd(990));
        // The oldest of 128 (863) the server ran: still fine.
        assert!(!s.interrupted());
        for t in 991..1001 {
            s.keep(s.client.idle_cmd(t));
        }
        // Oldest 873: run. Push past 980.
        for t in 1001..1110 {
            s.keep(s.client.idle_cmd(t));
        }
        // Oldest is 982 now, past the server's 980, and due.
        assert!(s.interrupted());
    }

    #[test]
    fn a_userinfo_change_goes_out_once() {
        let mut s = session(None);
        let mut info = vcod_common::net::Userinfo {
            name: "a".into(),
            rate: "25000".into(),
            snaps: "20".into(),
            password: String::new(),
            www_download: "0".into(),
        };
        s.opts = s.opts.clone().with_userinfo(&info);
        let seq = s.client.reliable_sequence;
        s.set_userinfo(&info);
        assert_eq!(s.client.reliable_sequence, seq, "nothing changed");
        info.name = "b;\\\"c".into();
        info.rate = "5000".into();
        s.set_userinfo(&info);
        assert_eq!(s.client.reliable_sequence, seq + 1);
        assert_eq!(
            s.client.reliable_command(seq + 1),
            b"userinfo \"\\name\\bc\\rate\\5000\\snaps\\20\\cl_punkbuster\\0\\cl_voice\\1\\cl_wwwDownload\\0\""
        );
    }
}
