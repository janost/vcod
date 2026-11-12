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
use crate::play::cod2_recoil::{Cod2Recoil, KickDef};
use crate::play::cod2_sound::{Cod2Sounds, LoopSounds, SoundCommand};
use crate::play::input::Action;
use std::collections::{HashMap, VecDeque};
use vcod_cod2::net::{PlayerView, SnapshotView};
use vcod_common::collision::CollisionWorld;
use vcod_common::net::protocol::CsRange;

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

/// What the session answers the stock script menus with
/// (`maps/mp/gametypes/_menus.gsc`'s `menuresponse` loop).
#[derive(Clone, Debug)]
pub struct Join {
    /// `allies`, `axis`, `autoassign` or `spectator`.
    pub team: String,
    /// A weapon file name the weapon menu allows; `None` takes the
    /// nation's bolt-action rifle.
    pub weapon: Option<String>,
}

impl Join {
    /// The response to script menu `menu` (its configstring name), or
    /// `None` for one a joining player leaves alone.
    pub fn response(&self, menu: &str) -> Option<String> {
        if menu.starts_with("serverinfo_") {
            return Some("close".into());
        }
        if menu.starts_with("team_") {
            return Some(self.team.clone());
        }
        let nation = menu.strip_prefix("weapon_")?;
        if let Some(w) = &self.weapon {
            return Some(w.clone());
        }
        Some(
            match nation {
                "german" => "kar98k_mp",
                "british" => "enfield_mp",
                "american" => "m1garand_mp",
                "russian" => "mosin_nagant_mp",
                _ => return None,
            }
            .into(),
        )
    }
}

/// A connect running on its own thread ([`Cod2Session::connect_in_background`]).
pub struct Connecting {
    pub addr: String,
    rx: std::sync::mpsc::Receiver<anyhow::Result<Cod2Session>>,
    progress: Arc<std::sync::Mutex<Option<Progress>>>,
}

/// What the client sends and fetches on a connect: its name, and where
/// and whether it downloads the server's iwds (`cl_allowDownload`,
/// `cl_wwwDownload`).
#[derive(Clone, Debug)]
pub struct ConnectOpts {
    pub name: String,
    /// `fs_homepath`, which on the Windows and Mac clients is the install
    /// (`Sys_DefaultHomePath` returns nothing, cod2-protocol.md 17.5):
    /// `main/` and the mod directories sit in it. `None` downloads nothing.
    pub home: Option<PathBuf>,
    pub allow_download: bool,
    pub www_download: bool,
    /// The `rate` and `snaps` userinfo dvars, and `password` (left out
    /// when empty).
    pub rate: String,
    pub snaps: String,
    pub password: String,
    /// `cl_voice` (default 1, flags 0x1003: archived, userinfo;
    /// `CL_InitOnceForAllClients`, Mac 0x1485e2): the server relays voice
    /// only to a client sending it on (cod2-voice.md 3).
    pub voice: bool,
}

impl ConnectOpts {
    /// Retail's defaults (both cvars 1) with `fs`'s install as the homepath.
    pub fn new(name: &str, fs: &Pk3Fs) -> Self {
        ConnectOpts {
            name: name.to_string(),
            home: home_of(fs),
            ..Self::default()
        }
    }

    /// The userinfo string: CoD2's stock keys (cod2-protocol.md 1.2), the
    /// values stripped of `\`, `;` and `"`, which an info string cannot
    /// carry. `connect` adds `protocol`, `challenge` and `qport`.
    pub fn userinfo(&self) -> String {
        let clean = |v: &str| -> String {
            v.chars()
                .filter(|c| !matches!(c, '\\' | ';' | '"'))
                .collect()
        };
        let mut ui = format!(
            "\\name\\{}\\rate\\{}\\snaps\\{}\\cl_punkbuster\\0\\cl_voice\\{}\\cl_wwwDownload\\{}",
            clean(&self.name),
            clean(&self.rate),
            clean(&self.snaps),
            u8::from(self.voice),
            u8::from(self.www_download)
        );
        if !self.password.is_empty() {
            ui.push_str(&format!("\\password\\{}", clean(&self.password)));
        }
        ui
    }
}

impl ConnectOpts {
    /// These options with the console's userinfo dvars; an empty `rate` or
    /// `snaps` keeps the default.
    pub fn with_userinfo(self, info: &vcod_common::net::Userinfo) -> Self {
        let or = |v: &str, d: String| if v.is_empty() { d } else { v.to_string() };
        let d = ConnectOpts::default();
        ConnectOpts {
            name: info.name.clone(),
            rate: or(&info.rate, d.rate),
            snaps: or(&info.snaps, d.snaps),
            password: info.password.clone(),
            www_download: info
                .www_download
                .trim()
                .parse::<f32>()
                .is_ok_and(|v| v != 0.0),
            ..self
        }
    }
}

impl Default for ConnectOpts {
    fn default() -> Self {
        ConnectOpts {
            name: String::new(),
            home: None,
            allow_download: true,
            www_download: true,
            rate: "25000".into(),
            snaps: "20".into(),
            password: String::new(),
            voice: true,
        }
    }
}

/// The install `fs`'s `main/` sits in.
pub fn home_of(fs: &Pk3Fs) -> Option<PathBuf> {
    fs.dirs()
        .iter()
        .find(|d| {
            d.file_name()
                .is_some_and(|n| n.eq_ignore_ascii_case("main"))
        })
        .and_then(|d| d.parent())
        .map(Path::to_path_buf)
}

impl Connecting {
    /// The file the connect is downloading, for the connect screen.
    pub fn progress(&self) -> Option<Progress> {
        self.progress.lock().ok()?.clone()
    }

    /// The session once the gamestate is in, or why the connect failed;
    /// `None` while it runs.
    pub fn poll(&self) -> Option<anyhow::Result<Cod2Session>> {
        match self.rx.try_recv() {
            Ok(r) => Some(r),
            Err(std::sync::mpsc::TryRecvError::Empty) => None,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                Some(Err(anyhow::anyhow!("the connect thread ended")))
            }
        }
    }
}

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
    /// `moveSpeedScale` by weapon name.
    move_scales: HashMap<String, f32>,
    /// The level's weapon table the prediction's `PM_Weapon` reads, and
    /// the `CS_WEAPONS` it was built off.
    weapon_defs: (String, Vec<Option<vcod_cod2::pmove::weapon::WeaponParams>>),
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
    /// Each gun weapon's `vertViewJitter` and `horizViewJitter`, by name.
    gun_jitters: HashMap<String, [f32; 2]>,
    /// This frame's view jitter on a firing gun, pitch and yaw degrees.
    jitter: [f32; 2],
    jitter_rng: crate::fx::sim::Rng,
    /// The own body's view kick and gun spring, and the gamestate and
    /// client they run for.
    recoil: Cod2Recoil,
    recoil_for: (u32, i32),
    /// Each weapon's kick keys by name (`None`: the file did not load).
    kick_defs: HashMap<String, Option<KickDef>>,
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
    /// Challenge, plain-text connect, then wait for the gamestate and its
    /// downloads; the `cp` waits for the first [`Self::fly`], over the
    /// search path the map loads from (`CL_DownloadsComplete` sends it
    /// after `CL_InitCGame`). Blocks for at most `timeout` until the
    /// gamestate.
    pub fn connect(addr: &str, opts: &ConnectOpts, timeout: Duration) -> anyhow::Result<Self> {
        Self::connect_with(addr, opts, timeout, None)
    }

    /// [`Self::connect`] on a thread of its own, for the front end's
    /// `connect`; [`Connecting::progress`] follows its downloads.
    pub fn connect_in_background(addr: &str, opts: ConnectOpts, timeout: Duration) -> Connecting {
        let (tx, rx) = std::sync::mpsc::channel();
        let a = addr.to_string();
        let progress = Arc::new(std::sync::Mutex::new(None));
        let shown = progress.clone();
        std::thread::spawn(move || {
            let _ = tx.send(Self::connect_with(&a, &opts, timeout, Some(shown)));
        });
        Connecting {
            addr: addr.to_string(),
            rx,
            progress,
        }
    }

    /// The handshake, then the gamestate and every download it asks for:
    /// the session comes back once the map can load (`CL_DownloadsComplete`).
    fn connect_with(
        addr: &str,
        opts: &ConnectOpts,
        timeout: Duration,
        progress: Option<Arc<std::sync::Mutex<Option<Progress>>>>,
    ) -> anyhow::Result<Self> {
        let sock = UdpSocket::bind("0.0.0.0:0")?;
        sock.connect(addr)
            .with_context(|| format!("cannot reach {addr}"))?;
        sock.set_read_timeout(Some(Duration::from_millis(5)))?;
        let key = KeyAuth {
            key: cdkey::lookup(KeyGame::Cod2),
            server: sock.peer_addr().ok(),
        };
        let hash = key.key.as_ref().map(|k| k.hash(KeyGame::Cod2));
        let challenge = ask_keyed(
            &sock,
            &oob::getchallenge(hash.as_deref()),
            Some(&key),
            |r| match r {
                Reply::Challenge(c) => Some(*c),
                _ => None,
            },
        )?;
        // Below 0x8000: retail's `SV_VoicePacket` reads the qport signed and
        // never matches a higher one (cod2-voice.md 3).
        let qport = std::process::id() as u16 & 0x7fff;
        // `cl_wwwDownload` is a userinfo dvar (flags 0x1003,
        // `CL_InitOnceForAllClients`, Mac 0x1485e2).
        let ui = opts.userinfo();
        ask(&sock, &oob::connect(&ui, challenge, qport), |r| {
            matches!(r, Reply::Connect).then_some(())
        })?;
        log::info!("cod2: connected to {addr}, challenge {challenge}");
        let mut s = Cod2Session::new(Some(sock), Client::new(qport, challenge));
        s.opts = opts.clone();
        s.progress = progress;
        let until = Instant::now() + timeout;
        // Past the first gamestate only `cl_timeout` gives up: a download
        // takes as long as it takes.
        while !s.settled() {
            if s.view.gamestates == 0 && Instant::now() > until {
                bail!("no gamestate from {addr}");
            }
            s.poll(None);
            if let Some(why) = s.dropped.take() {
                // `Com_Error`'s `CL_Disconnect`: the server hears of it.
                s.disconnect();
                return Err(ServerError(why).into());
            }
            s.send_due()?;
            std::thread::sleep(Duration::from_millis(2));
        }
        s.level_gamestates = s.view.gamestates;
        s.loaded_map = s.map_name().unwrap_or_default().to_string();
        Ok(s)
    }

    /// A session on `sock` past `connectResponse`, before the gamestate.
    fn new(sock: Option<UdpSocket>, client: Client) -> Self {
        let now = Instant::now();
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
            epoch: crate::frame_clock::now(),
            clock: ServerClock::cod2(),
            cmd_clock: CmdClock::default(),
            sent: VecDeque::new(),
            predictor: Cod2Predictor::default(),
            predict_bodies: true,
            world: None,
            move_scales: HashMap::new(),
            weapon_defs: Default::default(),
            predicted: None,
            predicted_events: Vec::new(),
            predict_log: PredictLog::default(),
            gun_tags: HashMap::new(),
            gun_jitters: HashMap::new(),
            jitter: [0.0; 2],
            jitter_rng: crate::fx::sim::Rng::new(0x6a69_7474),
            recoil: Cod2Recoil::default(),
            recoil_for: (0, -1),
            kick_defs: HashMap::new(),
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
        let mut buf = vec![0u8; 0x10000];
        while let Some(n) = self.sock.as_mut().and_then(|s| s.try_recv(&mut buf)) {
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
                    Reply::Disconnect if self.last_msg.elapsed() >= OOB_DISCONNECT_QUIET => {
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
            self.meter.packet(fragment, lost as u32, Instant::now());
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
        if self.last_msg.elapsed() > self.timeout {
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
        self.last_msg = Instant::now();
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
            self.move_scales.clear();
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
            self.snap_at = Instant::now();
        }
        self.view.update(&self.client.parser, &p);
        self.meter.message(Instant::now());
        if p.snapshot.is_some()
            && let Some(s) = &self.view.snap
        {
            self.meter.snapshot(s.ps.command_time, Instant::now());
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
        Some(snap.server_time - gap + self.snap_at.elapsed().as_millis() as i32)
    }

    /// The server time a cmd made now carries.
    fn cmd_time(&self) -> i32 {
        let snap = self.view.snap.as_ref().map_or(0, |s| s.server_time);
        if self.clock.started() {
            return self.clock.cmd_time(snap);
        }
        snap + self.snap_at.elapsed().as_millis() as i32
    }

    /// Local ms since the session started, on the client's frame clock
    /// (`cls.realtime`, which `cl_avidemo` steps).
    fn real_ms(&self) -> f64 {
        crate::frame_clock::now()
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
        let (eye, angles) = self
            .view()
            .unwrap_or_else(|| (Vec3::from(f.eye()), crate::cod2_viewmodel::view_angles(&f)));
        let forward = vcod_common::x87::angle_vectors(Vec3::from_array(angles))[0];
        let world = self.world.as_ref().map(|(_, w)| w);
        crate::cod2_names::scan(&f, eye, forward, world)
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
            match gun.and_then(|e| f.configstrings.weapon_name(e.weapon)) {
                Some(n) => n.to_string(),
                None => return [0.0; 2],
            }
        };
        let [v, h] = *self.gun_jitters.entry(weapon).or_insert_with_key(|n| {
            vcod_cod2::weapon::load(fs, n).map_or([0.0; 2], |w| {
                [
                    w.def.float("vertViewJitter"),
                    w.def.float("horizViewJitter"),
                ]
            })
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
        let r = self.meter.rates(Instant::now());
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
    /// the load handed it over ([`Self::install_world`]).
    fn ensure_world(&mut self, fs: &Pk3Fs) {
        let Some(map) = self.map_name().map(str::to_string) else {
            return;
        };
        if self.world.as_ref().is_some_and(|(m, _)| *m == map) {
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
            }
            Err(e) => {
                log::warn!("cod2: no collision for {map}, not predicting: {e:#}");
                self.world = None;
            }
        }
    }

    /// The held weapon's `moveSpeedScale`, 0 (none) when its file is
    /// missing.
    fn move_scale(&mut self, weapon: i32, fs: &Pk3Fs) -> f32 {
        let Some(name) = self.view.configstrings.weapon_name(weapon) else {
            return 0.0;
        };
        if let Some(&s) = self.move_scales.get(name) {
            return s;
        }
        let s = vcod_cod2::weapon::load(fs, name)
            .map(|p| p.def.float("moveSpeedScale"))
            .unwrap_or(0.0);
        self.move_scales.insert(name.to_string(), s);
        s
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
            && self.last_cmd.elapsed() >= Duration::from_millis(CMD_MS)
        {
            self.last_cmd = Instant::now();
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
        if self.last_packet.elapsed() < Duration::from_millis(wait) {
            return Ok(());
        }
        // No new cmd: the clock stands (a cmd is held within 5 s of the
        // snapshot, `CL_CreateCmd` Mac 0x189f0e). Retail still makes one a
        // frame and sends it, so the newest goes again, at most at the
        // off-LAN pace: the server keeps hearing from us.
        if self.view.gamestates > 0
            && self.pending.is_empty()
            && !self.cp_owed
            && (self.sent.is_empty()
                || self.last_packet.elapsed() < Duration::from_millis(PACKET_MS))
        {
            return Ok(());
        }
        let Some(sock) = &mut self.sock else {
            self.pending.clear();
            return Ok(());
        };
        self.last_packet = Instant::now();
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
        let log_due = self.last_log.elapsed() >= Duration::from_secs(10);
        if let Some((eye, _)) = self.view() {
            cam.pos = eye;
        } else if let Some(f) = self.frame() {
            // A followed gunner's eye is his gun's too.
            let view = crate::cod2_viewmodel::view_angles(&f);
            cam.pos = self.gunner_eye(view).unwrap_or(Vec3::from(f.eye()));
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
            self.last_log = Instant::now();
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
        let scale = self.move_scale(weapon, fs);
        let cs7 = self
            .view
            .configstrings
            .get(vcod_cod2::net::protocol::CS_WEAPONS);
        if self.weapon_defs.0 != cs7 || self.weapon_defs.1.is_empty() {
            let names: Vec<&str> = cs7.split(' ').collect();
            self.weapon_defs = (cs7.to_string(), vcod_cod2::pmove::weapon::table(fs, &names));
        }
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
            Some(&self.weapon_defs.1),
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
        Some(self.fx.level_effects(&self.view.configstrings, fs))
    }

    /// The events of the snapshot that became current since the last call
    /// (the older of [`Self::frame`]'s two, as the cgame walks them), each
    /// once, for [`Self::sound_cues`] and [`Self::effects`].
    pub fn take_events(&mut self) -> Vec<Event2> {
        if self.events_for != self.view.gamestates {
            self.events_for = self.view.gamestates;
            self.events = EventTracker::default();
            self.sounds.clear();
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
        let def = f.configstrings.weapon_name(weapon).and_then(|name| {
            self.kick_defs
                .entry(name.to_string())
                .or_insert_with(|| match vcod_cod2::weapon::load(fs, name) {
                    Ok(p) => Some(KickDef::new(&p.def)),
                    Err(e) => {
                        log::warn!("cod2 recoil: {e:#}");
                        None
                    }
                })
                .as_ref()
        });
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
                    .cues(e, cur, &self.view.configstrings, Some(fs), view)
            })
            .collect();
        (cues, ambient)
    }

    /// The loop sounds and the missiles' projectile sounds at the render
    /// time ([`Cod2Sounds::loops`]).
    pub fn loop_sounds(&mut self, fs: &Pk3Fs) -> (LoopSounds, LoopSounds) {
        let Some(t) = self.render_time() else {
            return Default::default();
        };
        match self.view.frame(t) {
            Some(f) => self.sounds.loops(&f, Some(fs)),
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
        let Some(t) = self.render_time() else {
            return Vec::new();
        };
        match self.view.frame(t) {
            Some(f) => self.fx.frame(&f, events, view, Some(fs), scene),
            None => Vec::new(),
        }
    }

    /// The `viewFlashEffect` of CS 7 weapon `weapon`, what a held view
    /// flash plays on the gun placed (`play::cod2_fx::Play::ViewFlash`).
    pub fn view_flash_effect(&mut self, weapon: i32, fs: &dyn AssetSource) -> Option<String> {
        self.fx.view_flash(&self.view.configstrings, fs, weapon)
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

/// `demo` / `timedemo` on a CoD2 install (docs/research/cod2-demos.md):
/// `CL_PlayDemo_f`'s `clc` fields (Mac 0x14c288) and `CL_SetCGameTime`'s
/// clock (0x17ba76).
struct DemoPlay {
    reader: vcod_common::net::demo::DemoReader,
    /// `clc.firstDemoFrameSkipped` (+0x40194): only a new playback clears it.
    skipped: bool,
    /// `timedemo` (+0x4018c) and its count, kept for the whole demo.
    timedemo: Option<crate::play::demos::TimeDemo>,
    /// `timedemo`'s csv, when the caller asked for one
    /// ([`Cod2Session::write_timedemo_csv`]).
    csv: Option<TimedemoCsv>,
    /// `cl_freezeDemo`: the clock holds.
    freeze: bool,
    /// `CA_ACTIVE`: a snapshot since the last gamestate seeded the clock.
    active: bool,
    /// `cl.serverTimeDelta`, `cl.serverTime`, `cl.oldServerTime`.
    delta: i32,
    server_time: i32,
    old_server_time: i32,
    /// `clc.timeDemoBaseTime` (+0x401ac): the first snapshot's time.
    base: i32,
    /// The file ran out (or was cut short).
    ended: bool,
}

/// `timedemo`'s csv (`CL_SetCGameTime`, Mac 0x17ba76): `clc` +0x4019c, the
/// file, and +0x401a8, the last timed frame's `Sys_Milliseconds`.
struct TimedemoCsv {
    dir: PathBuf,
    r_mode: i32,
    out: Option<std::io::BufWriter<std::fs::File>>,
    last_ms: i32,
}

impl TimedemoCsv {
    /// One timed frame, before it counts: the file is opened while it is
    /// not (`FS_FOpenFileWrite`, so every frame until one opens), then each
    /// frame after the first writes `<frames counted>,<ms since the last>`.
    fn frame(&mut self, map: &str, td: &crate::play::demos::TimeDemo, now_ms: i32) {
        use std::io::Write;
        if self.out.is_none() {
            let path = self.dir.join(timedemo_csv_name(map, self.r_mode));
            let file =
                std::fs::create_dir_all(&self.dir).and_then(|_| std::fs::File::create(&path));
            match file {
                Ok(f) => self.out = Some(std::io::BufWriter::new(f)),
                Err(e) => log::warn!("timedemo: {}: {e}", path.display()),
            }
        }
        if td.started()
            && let Some(out) = self.out.as_mut()
        {
            let _ = writeln!(out, "{},{}", td.frames(), now_ms - self.last_ms);
        }
        self.last_ms = now_ms;
    }
}

/// `timedemo_<map>_mode_<r_mode>.csv`: the map is `cl.mapname`
/// (`maps/mp/<mapname>.d3dbsp`, `CL_InitCGame`, Mac 0x17b412) past its last
/// slash and short of its last dot (`CL_SetCGameTime`, 0x17ba76).
fn timedemo_csv_name(map: &str, r_mode: i32) -> String {
    let full = format!("maps/mp/{map}.d3dbsp");
    let base = full.rsplit(['/', '\\']).next().unwrap_or("");
    let base = base.rsplit_once('.').map_or(base, |(b, _)| b);
    format!("timedemo_{base}_mode_{r_mode}.csv")
}

impl Cod2Session {
    /// `demo <name>` / `timedemo <name>` (`CL_PlayDemo_f`, Mac 0x14c288):
    /// a session over the file's bytes, read up to its gamestate. The
    /// caller loads the map and draws it as a live session's.
    pub fn play_demo(data: Vec<u8>, timedemo: bool) -> anyhow::Result<Self> {
        let mut s = Cod2Session::new(None, Client::new(0, 0));
        s.demo = Some(Box::new(DemoPlay {
            reader: vcod_cod2::net::demo::reader(data),
            skipped: false,
            timedemo: timedemo.then(Default::default),
            csv: None,
            freeze: false,
            active: false,
            delta: 0,
            server_time: 0,
            old_server_time: 0,
            base: 0,
            ended: false,
        }));
        while s.view.gamestates == 0 {
            if !s.read_demo_message() {
                if let Some(why) = s.dropped.take() {
                    bail!("{why}");
                }
                bail!("the demo ends before its gamestate");
            }
        }
        s.level_gamestates = s.view.gamestates;
        s.loaded_map = s.map_name().unwrap_or_default().to_string();
        Ok(s)
    }

    /// Under `timedemo`, write its csv into `dir` (the game's `demos/`)
    /// with `r_mode`'s index in its name.
    pub fn write_timedemo_csv(&mut self, dir: PathBuf, r_mode: i32) {
        if let Some(d) = self.demo.as_mut().filter(|d| d.timedemo.is_some()) {
            d.csv = Some(TimedemoCsv {
                dir,
                r_mode,
                out: None,
                last_ms: 0,
            });
        }
    }

    /// Whether this session plays a demo.
    pub fn is_demo(&self) -> bool {
        self.demo.is_some()
    }

    /// `cl_freezeDemo` for the next frames.
    pub fn set_freeze_demo(&mut self, freeze: bool) {
        if let Some(d) = self.demo.as_mut() {
            d.freeze = freeze;
        }
    }

    /// The demo ran out: the caller runs `CL_DemoCompleted` (Mac
    /// 0x14c096): [`Self::timedemo_report`], the disconnect, `nextdemo`.
    pub fn demo_over(&self) -> bool {
        self.demo.as_ref().is_some_and(|d| d.ended)
    }

    /// `CL_DemoCompleted`'s `%i frames, %3.1f seconds: %3.1f fps` under
    /// `timedemo`, when any time passed.
    pub fn timedemo_report(&self) -> Option<String> {
        let td = self.demo.as_ref()?.timedemo.as_ref()?;
        td.report(self.real_ms() as i32)
    }

    /// `CL_ReadDemoMessage` (Mac 0x14c132): one record parsed; false once
    /// the file has none, `Demo file was truncated.` printed for a cut
    /// record and a record past `MAX_MSGLEN` dropping the session.
    fn read_demo_message(&mut self) -> bool {
        use vcod_common::net::demo::DemoEnd;
        let Some(d) = self.demo.as_mut() else {
            return false;
        };
        if d.ended {
            return false;
        }
        match d.reader.next_message() {
            Ok((seq, msg)) => {
                match self.client.demo_message(seq as i32, &msg) {
                    Ok(Some(p)) => self.on_message(p),
                    Ok(None) => {}
                    Err(e) => {
                        self.stats.parse_errors += 1;
                        log::warn!("cod2: demo message parse failed: {e:#}");
                    }
                }
                true
            }
            Err(end) => {
                d.ended = true;
                // `CL_DemoCompleted` closes the csv (the drop flushes it).
                d.csv = None;
                match end {
                    DemoEnd::Completed => {}
                    DemoEnd::Truncated => crate::console::log::print("Demo file was truncated."),
                    DemoEnd::TooLong(_) => {
                        self.dropped = Some("CL_ReadDemoMessage: demoMsglen > MAX_MSGLEN".into())
                    }
                }
                false
            }
        }
    }

    /// `CL_SetCGameTime`'s demo arm (Mac 0x17ba76). Primed, the first
    /// frame is skipped, then one message a frame until a snapshot not
    /// flagged 2 seeds the clock and the base; active, the clock runs on
    /// the wall clock (held under `cl_freezeDemo`, never adjusted:
    /// `CL_AdjustTimeDelta` returns, 0x17b544) or, under `timedemo`, 50 ms
    /// a frame past the base, and messages are read while the newest
    /// snapshot is no later than it. A gamestate primes it again.
    fn demo_frame(&mut self) {
        let real = self.real_ms() as i32;
        let gamestates = self.view.gamestates;
        let Some(d) = self.demo.as_mut() else {
            return;
        };
        if d.ended {
            return;
        }
        if !d.active {
            if !d.skipped {
                d.skipped = true;
                return;
            }
            let snaps = self.stats.snapshots;
            if !self.read_demo_message() {
                return;
            }
            let fresh = self.stats.snapshots > snaps;
            let Some((time, flags)) = self
                .client
                .parser
                .snapshot()
                .map(|s| (s.server_time, s.snap_flags))
            else {
                return;
            };
            if !fresh || flags & 2 != 0 || self.view.gamestates != gamestates {
                return;
            }
            let d = self.demo.as_mut().expect("playing");
            d.active = true;
            d.delta = time - real;
            d.old_server_time = time;
            d.server_time = time;
            d.base = time;
        }
        let d = self.demo.as_mut().expect("playing");
        if !d.freeze {
            d.server_time = (real + d.delta).max(d.old_server_time);
            d.old_server_time = d.server_time;
        }
        if let Some(td) = d.timedemo.as_mut() {
            if let Some(csv) = d.csv.as_mut() {
                let map = self.view.configstrings.map_name().unwrap_or_default();
                csv.frame(map, td, real);
            }
            d.server_time = d.base + td.step(real) * 50;
        }
        let until = d.server_time;
        while self
            .client
            .parser
            .snapshot()
            .is_some_and(|s| s.server_time <= until)
        {
            if !self.read_demo_message() {
                return;
            }
            if self.view.gamestates != gamestates {
                if let Some(d) = self.demo.as_mut() {
                    d.active = false;
                }
                return;
            }
        }
    }

    /// A demo's frame: read on its clock, the camera on the recorder's
    /// eye, which [`Self::frame`] lerps between the snapshots. Nothing is
    /// predicted or sent (`cg.demoPlayback`, Mac 0x1ddafa).
    fn demo_fly(&mut self, cam: &mut FlyCamera) {
        self.demo_frame();
        if let Some(f) = self.frame() {
            cam.pos = Vec3::from(f.eye());
            let [pitch, yaw, _] = f.snap.ps.viewangles;
            cam.pitch = -pitch.to_radians();
            cam.yaw = yaw.to_radians();
        }
    }

    /// `record [name]` (`CL_Record_f`, Mac 0x149856) into `dir` (the
    /// game's `demos/`): `<name>.dm_1`, or the first free `demoNNNN`.
    /// Returns the console line.
    pub fn start_recording(&mut self, dir: &Path, name: Option<&str>) -> String {
        use vcod_cod2::net::demo::EXTENSION;
        if self.is_recording() {
            return "Already recording.".into();
        }
        // `cls.state` 8, `CA_ACTIVE`: a snapshot is in.
        if self.view.snap.is_none() {
            return "You must be in a level to record.".into();
        }
        let file = |n: &str| dir.join(format!("{n}.dm_{EXTENSION}"));
        // `clc.demoName` holds 63 characters.
        let name = match name {
            Some(n) => n.chars().take(63).collect(),
            None => vcod_common::net::demo::auto_name(|n| file(n).exists()),
        };
        let qpath = format!("demos/{name}.dm_{EXTENSION}");
        let out = std::fs::create_dir_all(dir).and_then(|_| std::fs::File::create(file(&name)));
        let Ok(out) = out else {
            return "ERROR: couldn't open.".into();
        };
        match self.client.start_recording(
            Box::new(std::io::BufWriter::new(out)),
            &name,
            self.view.configstrings.all(),
        ) {
            Ok(()) => format!("recording to {qpath}."),
            Err(e) => format!("{e:#}"),
        }
    }

    /// `stoprecord` (`CL_StopRecord_f`, Mac 0x14724e); the console line.
    pub fn stop_recording(&mut self) -> String {
        match self.client.stop_recording() {
            None => "Not recording a demo.".into(),
            Some(Ok(_)) => "Stopped demo.".into(),
            Some(Err(e)) => format!("ERROR: {e}"),
        }
    }

    pub fn is_recording(&self) -> bool {
        self.client.recording_status().is_some()
    }

    /// The demo's name and its bytes so far, for the `RECORDING` line.
    pub fn recording_status(&self) -> Option<(&str, u64)> {
        self.client.recording_status()
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
        let handle = std::thread::spawn(move || {
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
        fetched || !same
    }
}

/// The CD key a `getchallenge` goes out beside, and the server it asks.
struct KeyAuth {
    key: Option<CdKey>,
    server: Option<std::net::SocketAddr>,
}

impl KeyAuth {
    /// `CL_RequestAuthorization` (CoD2MP_s.exe 0x40d130), fire and forget.
    fn request(&self) {
        if let (Some(key), Some(server)) = (&self.key, self.server) {
            cdkey::request_authorization(KeyGame::Cod2, key, server);
        }
    }
}

/// Send `p` until an out-of-band reply `want` accepts comes back.
fn ask<T>(sock: &UdpSocket, p: &[u8], want: impl Fn(&Reply) -> Option<T>) -> anyhow::Result<T> {
    ask_keyed(sock, p, None, want)
}

/// [`ask`], each send after the key's authorization when `auth` is given
/// (`CL_CheckForResend`, CoD2MP_s.exe 0x40e316). A `needcdkey` without a
/// key ends it with `EXE_ERR_INVALID_CD_KEY`, where retail stops before it
/// sends; with one it sends the key again and keeps asking (0x40f07b),
/// and a timeout then reads `EXE_AWAITINGCDKEYAUTH`, the connect
/// screen's line.
fn ask_keyed<T>(
    sock: &UdpSocket,
    p: &[u8],
    auth: Option<&KeyAuth>,
    want: impl Fn(&Reply) -> Option<T>,
) -> anyhow::Result<T> {
    let mut buf = vec![0u8; 0x10000];
    let mut awaiting_key = false;
    for _ in 0..5 {
        if let Some(a) = auth {
            a.request();
        }
        sock.send(p)?;
        let until = Instant::now() + Duration::from_secs(1);
        while Instant::now() < until {
            let Ok(n) = sock.recv(&mut buf) else {
                continue;
            };
            let Some(pl) = oob::payload(&buf[..n]) else {
                continue;
            };
            let r = oob::parse_reply(pl);
            if let Reply::Error(e) = &r {
                return Err(ServerError(e.clone()).into());
            }
            if let (Reply::NeedCdKey, Some(a)) = (&r, auth) {
                if a.key.is_none() {
                    cdkey::warn_no_key();
                    return Err(ServerError("EXE_ERR_INVALID_CD_KEY".into()).into());
                }
                awaiting_key = true;
                a.request();
            }
            if let Some(t) = want(&r) {
                return Ok(t);
            }
        }
    }
    if awaiting_key {
        return Err(ServerError("EXE_AWAITINGCDKEYAUTH".into()).into());
    }
    bail!("no answer")
}

/// The scripted player of `vcod --net-probe ADDR --game cod2 --probe-team
/// T`: a 16 s loop of a walk, a strafe while turning, a jump, a back-up, a
/// crouched walk, a stand, three semi-automatic taps, a sighted tap, a
/// weapon switch and back, a reload and a still second, through the same
/// binds a player presses ([`Cod2Input`]); the switch presses 2, the way
/// back turns the wheel up, both through CoD2's stock binds. With
/// `VCOD_PROBE_REWEAPON=<weapon>` it presses X (`openscriptmenu ingame
/// changeweapon`) 17 s in and answers the weapon menu that opens with it.
/// `VCOD_PROBE_INGAME=<response>` instead answers the `ingame` menu's
/// button with that response 17 s in, as its click does. Every `t`, `u`,
/// `K` and `v` left to the player prints as a `cod2: menu command` line.
struct ProbeScript {
    /// The impulses already sent this loop, by their time.
    done: Vec<u32>,
    lap: u32,
    shell: crate::console::shell::Shell,
    reweapon: Option<String>,
    reweapon_asked: bool,
    ingame: Option<String>,
}

impl Default for ProbeScript {
    fn default() -> Self {
        let mut shell = crate::console::shell::Shell::new();
        shell.use_cod2_binds();
        Self {
            done: Vec::new(),
            lap: 0,
            shell,
            reweapon: std::env::var("VCOD_PROBE_REWEAPON").ok(),
            reweapon_asked: false,
            ingame: std::env::var("VCOD_PROBE_INGAME").ok(),
        }
    }
}

impl ProbeScript {
    fn step(&mut self, s: &mut Cod2Session, cam: &mut FlyCamera, t: f32) {
        let lap = (t / 16.0) as u32;
        if lap != self.lap {
            self.lap = lap;
            self.done.clear();
        }
        let p = t % 16.0;
        let within = |a: f32, b: f32| p >= a && p < b;
        let held = [
            (Action::Forward, within(0.0, 4.5) || within(6.0, 7.5)),
            (Action::Right, within(3.0, 4.5)),
            (Action::Back, within(5.0, 6.0)),
            (Action::Jump, within(4.5, 4.6) || within(7.5, 7.6)),
            (
                Action::Attack,
                within(8.0, 8.1) || within(8.6, 8.7) || within(9.2, 9.3) || within(10.6, 10.7),
            ),
            (Action::Reload, within(13.0, 13.1)),
        ];
        for (a, down) in held {
            s.input.key(a, down);
        }
        if within(3.0, 4.5) {
            cam.look([0.0, 0.3]);
        }
        let ps = s.view.snap.as_ref().map(|v| v.ps.raw.clone());
        for (at, a) in [
            (6.0, Action::Crouch),
            (10.0, Action::ToggleAds),
            (11.0, Action::ToggleAds),
        ] {
            let key = (at * 10.0) as u32;
            if p >= at && !self.done.contains(&key) {
                self.done.push(key);
                s.input.impulse(a, ps.as_ref());
            }
        }
        // The keys as `main.rs` routes them: bind, effect, input.
        for (at, k) in [(14.5, "2"), (15.5, "MWHEELUP")] {
            let key = (at * 10.0) as u32;
            if p >= at && !self.done.contains(&key) {
                self.done.push(key);
                let ps = s.frame().map(|f| f.snap.ps.raw.clone());
                for e in self.shell.key_event(k, true) {
                    s.input.effect(&e, ps.as_ref());
                }
                println!("cod2: pressed {k}, selected {}", s.input.weapons().0);
            }
        }
        if self.reweapon.is_some() && !self.reweapon_asked && t >= 17.0 {
            self.reweapon_asked = true;
            for e in self.shell.key_event("X", true) {
                if let Effect::OpenScriptMenu { menu, response } = e {
                    s.menu_response(&menu, &response);
                }
            }
        }
        if !self.reweapon_asked
            && t >= 17.0
            && let Some(response) = self.ingame.clone()
        {
            self.reweapon_asked = true;
            s.menu_response("ingame", &response);
        }
        // The weapon menu the X opened, answered as a player clicks it.
        for words in s.take_menu_commands() {
            println!("cod2: menu command {}", words.join(" "));
            if let (["t", n, ..], Some(w)) = (
                words
                    .iter()
                    .map(String::as_str)
                    .collect::<Vec<_>>()
                    .as_slice(),
                &self.reweapon,
            ) && let Some(menu) = n
                .parse()
                .ok()
                .and_then(|n| s.view.configstrings.script_menu(n))
                .filter(|m| m.starts_with("weapon_"))
                .map(str::to_string)
            {
                println!("cod2: the player's {menu}: {w}");
                s.menu_response(&menu, &w.clone());
            }
        }
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

/// Reads `map`'s world with `s` on the pump, as the windowed client
/// loads; `stall` seconds are added to the load. Prints how it went.
fn probe_load(s: Box<Cod2Session>, map: &str, fs: &Pk3Fs, stall: u64) -> Box<Cod2Session> {
    let pump = s.pump();
    let t0 = Instant::now();
    // the map parsed once, the prediction's collision built off it
    let world = vcod_cod2::map::MapFile::find(fs, map).ok().map(|f| {
        let w = f.world();
        let cw = f.clip().map(|c| prediction_world(&w, Arc::new(c)));
        (w, cw)
    });
    std::thread::sleep(Duration::from_secs(stall));
    let mut s = pump.finish();
    println!(
        "cod2: loaded {map} ({}) under the pump in {:.1} s",
        if world.is_some() { "ok" } else { "failed" },
        t0.elapsed().as_secs_f32()
    );
    if let Some((_, cw)) = world {
        s.install_world(fs, map, cw, t0);
    }
    s
}

/// `vcod --game cod2 --probe-demo FILE [--probe-timedemo]`: play a `.dm_1`
/// headless through the session's demo path (the same parse, clock, view,
/// events, cues, effects and scene builders as the window's), a line a
/// second, then the totals. Fails on a parse error or a demo with no
/// snapshot.
pub fn demo_probe(path: &Path, fs: &Pk3Fs, timedemo: bool) -> anyhow::Result<()> {
    let data = std::fs::read(path).with_context(|| format!("cannot read {}", path.display()))?;
    let bytes = data.len();
    let s = Cod2Session::play_demo(data, timedemo)?;
    let map = s.map_name().unwrap_or("?").to_string();
    println!(
        "cod2: demo {} ({bytes} bytes): gamestate for {map}, client {}, {} configstrings, {} baselines",
        path.display(),
        s.view.client_num,
        s.view
            .configstrings
            .all()
            .iter()
            .filter(|c| !c.is_empty())
            .count(),
        s.client
            .parser
            .gamestate
            .as_ref()
            .map_or(0, |g| g.baselines.len()),
    );
    let mut s = probe_load(Box::new(s), &map, fs, 0);
    let mut draw = crate::cod2_scene::Cod2Draw::new();
    let mut cam = FlyCamera::new(Vec3::ZERO, 0.0);
    let start = Instant::now();
    let mut last_print = start;
    let mut last = s.stats;
    let (mut events_all, mut cues_all, mut fx_all, mut frames) = (0usize, 0usize, 0usize, 0u64);
    let (mut first_time, mut last_time, mut most_ents) = (None, 0, 0);
    while !s.demo_over() {
        s.fly(&mut cam, fs);
        frames += 1;
        if let Some(why) = s.dropped.take() {
            bail!("{why}");
        }
        if let Some(m) = s.take_map_change() {
            println!("cod2: map change to {m}");
            s = probe_load(s, &m, fs, 0);
            draw = crate::cod2_scene::Cod2Draw::new();
        }
        let events = s.take_events();
        let (cues, _) = s.sound_cues(&events, fs, cam.pos, draw.muzzles());
        let plays = s.effects(&events, fs, cam.pos, draw.fx_scene(true));
        events_all += events.len();
        cues_all += cues.len();
        fx_all += plays.len();
        if let Some(f) = s.frame() {
            first_time.get_or_insert(f.time_ms);
            last_time = f.time_ms;
            most_ents = most_ents.max(f.snap.entities.len());
        }
        if last_print.elapsed() >= Duration::from_secs(1)
            && let Some(f) = s.frame()
        {
            last_print = Instant::now();
            let st = s.stats;
            let c = draw.count(&f, fs);
            println!(
                "t={:>3}s time {} snap {} msgs {} snaps {} delta {} ents {} clients {} \
                 drawn models {} bodies {} eye {:?} weapon {} parse errors {}",
                start.elapsed().as_secs(),
                f.time_ms,
                f.snap.server_time,
                st.messages - last.messages,
                st.snapshots - last.snapshots,
                st.deltas - last.deltas,
                f.snap.entities.len(),
                f.snap.clients.len(),
                c.models,
                c.players,
                f.eye(),
                f.configstrings.weapon_name(f.snap.ps.weapon).unwrap_or("-"),
                st.parse_errors,
            );
            last = st;
        }
        if !timedemo {
            std::thread::sleep(Duration::from_millis(2));
        }
    }
    let st = s.stats;
    println!(
        "cod2: demo done: {} messages, {} snapshots ({} deltas), {} parse errors, \
         {} gamestates, server time {:?}..{last_time}, most entities {most_ents}, \
         {frames} frames in {:.1} s",
        st.messages,
        st.snapshots,
        st.deltas,
        st.parse_errors,
        s.gamestates(),
        first_time,
        start.elapsed().as_secs_f32(),
    );
    println!("cod2: events {events_all} sound cues {cues_all} effects started {fx_all}");
    if let Some(line) = s.timedemo_report() {
        println!("cod2: {line}");
    }
    if st.parse_errors > 0 {
        bail!("{} messages failed to parse", st.parse_errors);
    }
    if st.snapshots == 0 {
        bail!("the demo holds no snapshot");
    }
    Ok(())
}

/// `vcod --net-probe ADDR --game cod2`: spectate for `secs`, or with
/// `join` answer the script menus and play, walking forward on a slow
/// circle through usercmds; print a line a second (messages, snapshots,
/// deltas, entities, the eye, `pm_type`, parse errors, and what the scene
/// and the player poser built off the frame, CPU side:
/// `cod2_scene::Cod2Draw::count`). The map is read under
/// [`Cod2Session::pump`] as the windowed client does, and so is the map of
/// a map change; `VCOD_COD2_LOAD_STALL=<secs>` stretches the first load,
/// to check the pump against a short `sv_timeout`, and
/// `VCOD_COD2_TIMEOUT=<secs>` shortens `cl_timeout`. Fails on a parse
/// error or a drop.
pub fn probe(addr: &str, secs: u64, fs: &Pk3Fs, join: Option<Join>) -> anyhow::Result<()> {
    let name = if join.is_some() {
        "vcodplay"
    } else {
        "vcodspec"
    };
    let mut s = Cod2Session::connect(addr, &ConnectOpts::new(name, fs), Duration::from_secs(15))?;
    let reopened;
    let fs = match (s.take_reopen(fs_game_of(fs).as_deref()), home_of(fs)) {
        (true, Some(home)) => {
            reopened = open_search(&home, s.fs_game())?;
            println!(
                "cod2: search path opened again, fs_game {:?}",
                s.fs_game().unwrap_or("")
            );
            &reopened
        }
        _ => fs,
    };
    s.join = join;
    s.predict_bodies = std::env::var_os("VCOD_PROBE_NO_BODIES").is_none();
    if let Some(t) = std::env::var("VCOD_COD2_TIMEOUT")
        .ok()
        .and_then(|v| v.parse().ok())
    {
        s.timeout = Duration::from_secs(t);
    }
    let map = s.map_name().unwrap_or("?").to_string();
    println!("cod2: gamestate for {map}, client {}", s.view.client_num);
    let stall: u64 = std::env::var("VCOD_COD2_LOAD_STALL")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    let mut s = probe_load(Box::new(s), &map, fs, stall);
    let mut draw = crate::cod2_scene::Cod2Draw::new();
    let mut most = crate::cod2_scene::DrawCounts::default();
    let start = Instant::now();
    let mut last_print = Instant::now();
    let mut last = s.stats;
    let mut cam = FlyCamera::new(Vec3::ZERO, 0.0);
    let scripted = s.join.is_some();
    let still = std::env::var_os("VCOD_PROBE_STILL").is_some();
    let mut script = ProbeScript::default();
    let mut first_eye = None;
    let mut first_origin = None;
    let mut playing_since: Option<u64> = None;
    // The events' sounds and effects, CPU side: cues counted (no audio
    // device), effects spawned into an fx system nothing draws.
    let mut fx = crate::fx::sim::FxSystem::new();
    let mut fx_last = start;
    let (mut cues_sec, mut fx_sec, mut events_sec) = (0usize, 0usize, 0usize);
    let (mut cues_all, mut fx_all, mut events_all) = (0usize, 0usize, 0usize);
    let mut predict_all = PredictLog::default();
    // The obituaries as the HUD resolves them.
    let mut hud = crate::cod2_hud::Cod2Hud::default();
    let loc = vcod_common::localize::Localized::load(fs);
    let hud_dump = std::env::var("VCOD_COD2_HUD_DUMP").ok();
    let mut hud_at: Vec<f32> = std::env::var("VCOD_COD2_HUD_AT")
        .unwrap_or_default()
        .split(',')
        .filter_map(|t| t.trim().parse().ok())
        .collect();
    // A player asks for the board 15 s and 25 s in, as Tab does, and with
    // `VCOD_PROBE_KILL=<secs>` sends `kill` that far in.
    let mut score_asked = 0;
    let mut kill_at = std::env::var("VCOD_PROBE_KILL")
        .ok()
        .and_then(|v| v.parse::<f32>().ok());
    // `VCOD_PROBE_CMDS="<secs>:<command>|..."` sends each command that far
    // in and prints every server command that comes back.
    let mut cmds: Vec<(f32, String)> = std::env::var("VCOD_PROBE_CMDS")
        .map(|v| {
            v.split('|')
                .filter_map(|c| c.split_once(':'))
                .filter_map(|(t, c)| Some((t.trim().parse().ok()?, c.to_string())))
                .collect()
        })
        .unwrap_or_default();
    if !cmds.is_empty() {
        s.command_log = Some(Vec::new());
    }
    // `VCOD_COD2_TRACE_WEAPON=1`: a line per snapshot with the weapon's
    // state, the clip and the reserve.
    let trace_weapon = std::env::var_os("VCOD_COD2_TRACE_WEAPON").is_some();
    let mut traced_time = 0;
    // What the HUD's ammo counter reads, off the level's weapon table.
    let mut player_hud = crate::cod2_player_hud::PlayerHud::default();
    // `VCOD_COD2_RECORD=<dir>/<name>`: `record <name>` into `<dir>` once
    // the first snapshot is in, `stoprecord` as the probe ends.
    let mut record = std::env::var_os("VCOD_COD2_RECORD").map(PathBuf::from);
    let mut first_fly_ms = None;
    let mut first_count = true;
    // `VCOD_PROBE_SNAPS_AT=<secs>:<snaps>`: change the `snaps` userinfo
    // mid-game, as the console's `snaps` does.
    let mut snaps_at = std::env::var("VCOD_PROBE_SNAPS_AT").ok().and_then(|v| {
        let (t, n) = v.split_once(':')?;
        Some((t.parse::<u64>().ok()?, n.to_string()))
    });
    while start.elapsed() < Duration::from_secs(secs) {
        if let Some((_, n)) = snaps_at.take_if(|(t, _)| start.elapsed().as_secs() >= *t) {
            let info = vcod_common::net::Userinfo {
                name: s.opts.name.clone(),
                rate: s.opts.rate.clone(),
                snaps: n,
                password: s.opts.password.clone(),
                www_download: u8::from(s.opts.www_download).to_string(),
            };
            println!(
                "cod2: snaps {} at t={}s",
                info.snaps,
                start.elapsed().as_secs()
            );
            s.set_userinfo(&info);
        }
        if s.view.snap.is_some()
            && let Some(path) = record.take()
        {
            let dir = path.parent().unwrap_or(Path::new("."));
            let name = path.file_name().map(|n| n.to_string_lossy().into_owned());
            let name = name.as_deref().map(|n| n.trim_end_matches(".dm_1"));
            println!("cod2: {}", s.start_recording(dir, name));
        }
        if still {
            // `VCOD_PROBE_STILL`: a body that stands where it spawned.
        } else if scripted {
            script.step(&mut s, &mut cam, start.elapsed().as_secs_f32());
        } else {
            s.input.key(Action::Forward, true);
            // About 30 degrees a second, so the spectator circles.
            cam.look([0.0, 0.06]);
        }
        let fly_t0 = Instant::now();
        s.fly(&mut cam, fs);
        if first_fly_ms.is_none() && s.frame().is_some() {
            let ms = fly_t0.elapsed().as_secs_f64() * 1000.0;
            first_fly_ms = Some(ms);
            println!("cod2: first frame's fly in {ms:.1} ms");
        }
        for path in s.take_fetched() {
            println!("cod2: downloaded {}", path.display());
        }
        let events = s.take_events();
        if let (Some(_), Some(f)) = (&hud_dump, s.frame()) {
            hud.on_events(&events, &f, fs, &loc, f.time_ms);
        }
        if let Some(f) = s.frame() {
            for ev in events
                .iter()
                .filter(|e| e.event == vcod_cod2::net::events::EV_OBITUARY)
            {
                match hud.obit(ev, &f, fs) {
                    Some(o) => println!(
                        "cod2: obituary {} parm {:#x}: {:?} [{}] {:?} center {:?}",
                        ev.other_entity,
                        ev.parm,
                        o.line.attacker.as_ref().map(|a| &a.0),
                        o.line.icon,
                        o.line.victim.0,
                        o.center.map(|c| loc.message(&c)),
                    ),
                    None => println!("cod2: obituary {} unresolved", ev.other_entity),
                }
            }
        }
        // `VCOD_COD2_HUD_DUMP=<prefix>` with `VCOD_COD2_HUD_AT=<secs>,...`:
        // the HUD a 1280 x 720 window would draw that far in, world sprites
        // first, as `<prefix>-<secs>.quads` for `renderer/hud_shot.rs`.
        if let (Some(prefix), Some(f)) = (&hud_dump, s.frame())
            && let Some(i) = hud_at
                .iter()
                .position(|&t| start.elapsed().as_secs_f32() >= t)
        {
            let t = hud_at.remove(i);
            draw.count(&f, fs);
            let size = (1280.0, 720.0);
            let (eye, [pitch, yaw, roll]) = s
                .view()
                .unwrap_or_else(|| (Vec3::from(f.eye()), crate::cod2_viewmodel::view_angles(&f)));
            let (yaw, pitch, roll) = (yaw.to_radians(), -pitch.to_radians(), roll.to_radians());
            let fov = crate::camera::view_fov_x(crate::camera::DEFAULT_FOV_DEG, 16.0 / 9.0, true);
            let view_proj = crate::camera::view_proj_from(eye, yaw, pitch, roll, fov, 16.0 / 9.0);
            let up = crate::camera::up_hint(yaw, pitch, roll);
            let fwd = crate::camera::basis(yaw, pitch).0;
            let right = fwd.cross(up).normalize();
            let view = crate::cod2_world_hud::WorldView {
                view_proj,
                right,
                up: right.cross(fwd),
                size,
            };
            let mut quads = crate::cod2_world_hud::build(&f, draw.heads(), &view, f.time_ms);
            hud.crosshair_hit = s.crosshair_entity();
            quads.extend(hud.build(&f, fs, &loc, size, f.time_ms, start.elapsed().as_secs_f32()));
            let path = format!("{prefix}-{t}.quads");
            std::fs::write(&path, crate::cod2_hud::dump_quads(&quads))?;
            println!(
                "cod2: hud dump {path}: {} quads, pm_type {} pm_flags {:#x} team {:?}",
                quads.len(),
                f.snap.ps.pm_type,
                f.snap.ps.pm_flags,
                f.snap
                    .clients
                    .iter()
                    .find(|c| c.number as i32 == f.snap.ps.client_num)
                    .map(|c| c.f[vcod_cod2::net::delta::field_index(
                        &vcod_cod2::net::fields::CLIENT_STATE_FIELDS,
                        "team"
                    )]),
            );
        }
        let (cues, _) = s.sound_cues(&events, fs, cam.pos, draw.muzzles());
        let plays = s.effects(&events, fs, cam.pos, draw.fx_scene(true));
        let now = start.elapsed().as_secs_f32();
        fx.set_view(cam.pos);
        for p in &plays {
            p.spawn(&mut fx, fs, now);
        }
        fx.step(fx_last.elapsed().as_secs_f32(), now, None);
        fx.spawn_chained(fs, now);
        fx_last = Instant::now();
        events_sec += events.len();
        cues_sec += cues.len();
        fx_sec += plays.len();
        if let Some(why) = &s.dropped {
            bail!("dropped: {why}");
        }
        if let Some(m) = s.take_map_change() {
            println!("cod2: map change to {m}");
            s = probe_load(s, &m, fs, 0);
            draw = crate::cod2_scene::Cod2Draw::new();
        }
        if s.take_restart() {
            println!("cod2: fast_restart, serverId {}", s.client.server_id());
            fx.clear();
        }
        while cmds.first().is_some_and(|(t, _)| now >= *t) {
            let (_, c) = cmds.remove(0);
            println!("cod2: t={now:.1} sending {c:?}");
            s.send_command(&c);
        }
        if let Some(log) = s.command_log.as_mut() {
            for c in log.drain(..) {
                println!("cod2: t={now:.1} server command {c:?}");
            }
        }
        if scripted && kill_at.is_some_and(|t| now >= t) {
            s.send_command("kill");
            kill_at = None;
        }
        if scripted && score_asked < 2 && now >= [15.0, 25.0][score_asked] {
            s.send_command("score");
            score_asked += 1;
        }
        if let Some(b) = s.take_scores() {
            println!("cod2: score reply {:?}", b.join(" "));
            match crate::hud::scoreboard::parse_scores(&b) {
                Ok(m) => {
                    // Each row's name and team as the drawn board resolves it.
                    let frame = s.frame();
                    let named = |c: u32| {
                        frame
                            .as_ref()
                            .and_then(|f| crate::cod2_hud::board_client(&f.snap.clients, c))
                    };
                    println!(
                        "cod2: scores rows {} axis {} allies {} {:?}",
                        m.entries.len(),
                        m.axis,
                        m.allies,
                        m.entries
                            .iter()
                            .map(|e| (e.client, e.score, e.ping, e.deaths, named(e.client)))
                            .collect::<Vec<_>>()
                    )
                }
                Err(e) => println!("cod2: bad scores {e}: {b:?}"),
            }
        }
        if trace_weapon
            && let Some(snap) = &s.view.snap
            && snap.server_time != traced_time
        {
            traced_time = snap.server_time;
            let ps = &snap.ps;
            let nz = |a: &[i32]| -> Vec<(usize, i32)> {
                a.iter()
                    .enumerate()
                    .filter(|e| *e.1 != 0)
                    .map(|(i, &v)| (i, v))
                    .collect()
            };
            let shown = s
                .frame()
                .and_then(|f| player_hud.counter(&f, fs, s.input.weapons().0));
            println!(
                "weap t={} weapon {} weaponstate {} weapAnim {:#x} weaponTime {} clip {:?} ammo {:?} buttons {:#x} hud {:?}",
                snap.server_time,
                ps.weapon,
                ps.raw.i32("weaponstate"),
                ps.raw.i32("weapAnim"),
                ps.raw.i32("weaponTime"),
                nz(&ps.raw.ammoclip),
                nz(&ps.raw.ammo),
                s.sent.back().map_or(0, |c| c.buttons),
                shown,
            );
        }
        if last_print.elapsed() >= Duration::from_secs(1) {
            last_print = Instant::now();
            let st = s.stats;
            let pl = s.take_predict_stats();
            println!(
                "link t={:>3}s {} | interrupted {}",
                start.elapsed().as_secs(),
                s.debug_lines().join(" | "),
                s.interrupted()
            );
            if let Some(f) = s.frame() {
                let eye = f.eye();
                first_eye.get_or_insert(eye);
                let count_t0 = Instant::now();
                let c = draw.count(&f, fs);
                if std::mem::take(&mut first_count) {
                    println!(
                        "cod2: first scene count in {:.1} ms",
                        count_t0.elapsed().as_secs_f64() * 1000.0
                    );
                }
                most.models = most.models.max(c.models);
                most.submodels = most.submodels.max(c.submodels);
                most.players = most.players.max(c.players);
                most.player_parts = most.player_parts.max(c.player_parts);
                most.guns_posed = most.guns_posed.max(c.guns_posed);
                let ps = &f.snap.ps;
                // pm_type 0: PM_NORMAL, the client's own body in the world.
                if ps.pm_type == 0 && ps.followed().is_none() {
                    playing_since.get_or_insert(start.elapsed().as_secs());
                    first_origin.get_or_insert(ps.origin);
                }
                println!(
                    "t={:>3}s serverTime {} msgs {} snaps {} delta {} cmds-sent {} ents {} clients {} \
                     drawn models {} submodels {} bodies {} body-parts {} guns {} \
                     events {} sound-cues {} effects {} fx-particles {} \
                     eye ({:.1} {:.1} {:.1}) origin ({:.1} {:.1} {:.1}) pm_type {} weapon {} ({}) \
                     follow {:?} map {} serverId {} parse-errors {} \
                     predict frames {} misses {} corrected {} over-0.1 {} max-error {:.4} \
                     selected {} pm_flags {:#x} weaponstate {} rounds {} buttons {:#x}",
                    start.elapsed().as_secs(),
                    f.snap.server_time,
                    st.messages - last.messages,
                    st.snapshots - last.snapshots,
                    st.deltas - last.deltas,
                    st.packets_sent - last.packets_sent,
                    f.snap.entities.len(),
                    f.snap.clients.len(),
                    c.models,
                    c.submodels,
                    c.players,
                    c.player_parts,
                    c.guns_posed,
                    events_sec,
                    cues_sec,
                    fx_sec,
                    fx.counts().0,
                    eye[0],
                    eye[1],
                    eye[2],
                    ps.origin[0],
                    ps.origin[1],
                    ps.origin[2],
                    ps.pm_type,
                    ps.weapon,
                    f.configstrings.weapon_name(ps.weapon).unwrap_or("-"),
                    ps.followed(),
                    f.configstrings.map_name().unwrap_or("?"),
                    s.client.server_id(),
                    st.parse_errors,
                    pl.frames,
                    pl.misses,
                    pl.corrected,
                    pl.missed,
                    pl.max_correction,
                    s.input.weapons().0,
                    ps.pm_flags,
                    ps.raw.i32("weaponstate"),
                    ps.raw.ammoclip.iter().sum::<i32>() + ps.raw.ammo.iter().sum::<i32>(),
                    s.sent.back().map_or(0, |c| c.buttons),
                );
                predict_all.frames += pl.frames;
                predict_all.misses += pl.misses;
                predict_all.missed += pl.missed;
                predict_all.corrected += pl.corrected;
                predict_all.max_correction = predict_all.max_correction.max(pl.max_correction);
            }
            last = st;
            events_all += events_sec;
            cues_all += cues_sec;
            fx_all += fx_sec;
            (events_sec, cues_sec, fx_sec) = (0, 0, 0);
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    if let Some((name, bytes)) = s.recording_status() {
        println!("cod2: RECORDING {name}: {}k", bytes >> 10);
        println!("cod2: {}", s.stop_recording());
    }
    s.disconnect();
    let st = s.stats;
    println!(
        "cod2: {} messages, {} snapshots ({} deltas), {} packets sent, {} parse errors, \
         {} new gamestates, {} fast_restarts, {} menu replies",
        st.messages,
        st.snapshots,
        st.deltas,
        st.packets_sent,
        st.parse_errors,
        st.new_gamestates,
        st.restarts,
        st.menu_replies
    );
    println!(
        "cod2: most drawn in a frame: models {} submodels {} bodies {} body-parts {} guns {}",
        most.models, most.submodels, most.players, most.player_parts, most.guns_posed
    );
    println!(
        "cod2: events {} sound cues {} effects started {}",
        events_all + events_sec,
        cues_all + cues_sec,
        fx_all + fx_sec
    );
    println!(
        "cod2: predicted frames {} misses {} snapshots corrected {} past 0.1 {} \
         longest correction {:.4}",
        predict_all.frames,
        predict_all.misses,
        predict_all.corrected,
        predict_all.missed,
        predict_all.max_correction
    );
    if st.parse_errors > 0 {
        bail!("{} messages failed to parse", st.parse_errors);
    }
    if st.deltas == 0 {
        bail!("no delta snapshot applied");
    }
    let moved = first_eye
        .zip(s.frame().map(|f| f.eye()))
        .is_some_and(|(a, b)| a != b);
    println!("cod2: eye moved: {moved}");
    if s.join.is_some() {
        let walked = first_origin
            .zip(s.frame().map(|f| f.snap.ps.origin))
            .map(|(a, b)| Vec3::from(a).distance(Vec3::from(b)));
        println!(
            "cod2: playing from t={:?}s, walked {:?} units",
            playing_since, walked
        );
    }
    Ok(())
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

    /// `timedemo_<map>_mode_<r_mode>.csv`, the map short of its directory
    /// and last extension.
    #[test]
    fn the_timedemo_csv_is_named_for_the_map_and_mode() {
        assert_eq!(
            timedemo_csv_name("mp_toujane", 3),
            "timedemo_mp_toujane_mode_3.csv"
        );
        assert_eq!(timedemo_csv_name("a.b", -1), "timedemo_a.b_mode_-1.csv");
        assert_eq!(timedemo_csv_name("x/y", 0), "timedemo_y_mode_0.csv");
        assert_eq!(timedemo_csv_name("", 0), "timedemo__mode_0.csv");
    }

    /// Retail's own demo under `timedemo`: the csv opens on the first timed
    /// frame and takes `<frame>,<ms>` from the second, the frames counted
    /// before it, one up a row, and is whole once the demo ends; a plain
    /// `demo` writes none.
    #[test]
    fn timedemo_writes_a_row_a_frame_after_the_first() {
        let data = include_bytes!("../../../cod2/tests/fixtures/demos/mp_toujane-retail.dm_1");
        let dir = std::env::temp_dir().join(format!("vcod-timedemo-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let fs = Pk3Fs::empty();
        let mut cam = FlyCamera::new(Vec3::ZERO, 0.0);
        let mut plain = Cod2Session::play_demo(data.to_vec(), false).unwrap();
        plain.write_timedemo_csv(dir.clone(), 3);
        assert!(plain.demo.as_ref().unwrap().csv.is_none());
        let mut s = Cod2Session::play_demo(data.to_vec(), true).unwrap();
        s.write_timedemo_csv(dir.clone(), 3);
        let mut frames = 0;
        while !s.demo_over() && frames < 5000 {
            s.fly(&mut cam, &fs);
            frames += 1;
        }
        assert!(s.demo_over(), "the demo never ended");
        let counted = s.demo.as_ref().unwrap().timedemo.as_ref().unwrap().frames();
        let text = std::fs::read_to_string(dir.join("timedemo_mp_toujane_mode_3.csv")).unwrap();
        let _ = std::fs::remove_dir_all(&dir);
        let rows: Vec<(i32, i32)> = text
            .lines()
            .map(|l| {
                let (f, ms) = l.split_once(',').unwrap();
                (f.parse().unwrap(), ms.parse().unwrap())
            })
            .collect();
        // A start stamped at 0 ms reads as none, so a frame in the
        // session's first millisecond writes no row.
        assert!(counted > 800, "{counted} frames");
        assert!(rows.len() as i32 >= counted - 3, "{} rows", rows.len());
        assert_eq!(rows.last().map(|r| r.0), Some(counted - 1));
        for w in rows.windows(2) {
            assert_eq!(w[1].0, w[0].0 + 1);
        }
        assert!(rows.iter().all(|r| r.1 >= 0));
    }

    /// A loopback server answering each `getchallenge` with `reply`: what
    /// the challenge ask comes back with, with and without a key.
    fn challenge_with(reply: &'static str, key: Option<CdKey>) -> anyhow::Result<i32> {
        let server = UdpSocket::bind("127.0.0.1:0").unwrap();
        let addr = server.local_addr().unwrap();
        std::thread::spawn(move || {
            let mut buf = [0u8; 256];
            server
                .set_read_timeout(Some(Duration::from_secs(6)))
                .unwrap();
            while let Ok((n, from)) = server.recv_from(&mut buf) {
                if buf[..n].starts_with(b"\xff\xff\xff\xffgetchallenge") {
                    let _ = server.send_to(&oob::packet(reply), from);
                }
            }
        });
        let sock = UdpSocket::bind("127.0.0.1:0").unwrap();
        sock.connect(addr).unwrap();
        sock.set_read_timeout(Some(Duration::from_millis(5)))
            .unwrap();
        let auth = KeyAuth {
            server: sock.peer_addr().ok(),
            key,
        };
        ask_keyed(&sock, &oob::getchallenge(None), Some(&auth), |r| match r {
            Reply::Challenge(c) => Some(*c),
            _ => None,
        })
    }

    #[test]
    fn a_key_leaves_a_loopback_challenge_unchanged() {
        let key = || CdKey::parse("AAAA-AAAA-AAAA-AAAA");
        assert_eq!(challenge_with("challengeResponse 9", None).unwrap(), 9);
        assert_eq!(challenge_with("challengeResponse 9", key()).unwrap(), 9);
        let why = |r: anyhow::Result<i32>| match r.unwrap_err().downcast::<ServerError>() {
            Ok(ServerError(w)) => w,
            Err(e) => format!("{e:#}"),
        };
        assert_eq!(
            why(challenge_with("needcdkey", None)),
            "EXE_ERR_INVALID_CD_KEY"
        );
        assert_eq!(
            why(challenge_with("needcdkey", key())),
            "EXE_AWAITINGCDKEYAUTH"
        );
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
