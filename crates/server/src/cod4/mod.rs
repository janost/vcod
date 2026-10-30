//! The engine side of a CoD4 1.7 server (protocol 6 of `Game::Cod4`), its
//! first milestone: a bare level a retail
//! client joins as a spectator. The handshake (challenge, connect, the seven
//! stats packets, the gamestate, the pure check) and the snapshots follow
//! docs/research/cod4-handshake.md; the frame order is 1.7's `SV_Frame`
//! (cod4-server-frame.md) through [`Cod4Game`]'s constants.
//!
//! The load walks the five common zones and the map's, and runs the level
//! (`game::cod4::level::Cod4Level`): the gametype's bootstrap, whose
//! configstrings and mirrored dvars go out in the gamestate, then each
//! client's `ClientConnect`, `ClientBegin` on its first move, its `mr` menu
//! answers, the spawns the script makes, and each cmd's pmove as it
//! arrives. A snapshot carries the level's playerstate, the script's
//! entities but the hidden ones and the playing clients' bodies, unculled
//! as retail's, and the roster (docs/research/cod4-pmove.md 15). Without an
//! install the level is bare: spectators float at the origin.

mod bots;
mod cycle;
mod download;
pub use crate::engine::gamelog;
mod oob;
mod voice;

use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use vcod_cod4::install::Install;
use vcod_cod4::net::clmsg::{self, ClOp, UserCmd};
use vcod_cod4::net::delta::{ClientState, ENTITYNUM_NONE, EntityState, PlayerState};
use vcod_cod4::net::fields::{CLIENT_STATE, PLAYER_STATE};
use vcod_cod4::net::huffman::Huffman;
use vcod_cod4::net::parse::{Gamestate, SVC_SERVERCOMMAND, UNUSED_HUFF};
use vcod_cod4::net::stats::{STAT_PACKETS, Stats};
use vcod_cod4::net::wire::{Cod4Wire, EOF, Frame4, PROTOCOL_17};
use vcod_common::net::codec::MsgWriter;
use vcod_common::net::netchan::{NetchanParams, ServerNetchan};
use vcod_common::net::protocol::{SnapshotHead, WireGame};

use crate::engine::link;

/// The frame at `sv_fps` 20, the dvar's default; a server steps
/// [`Cod4Server::frame_ms`].
pub const FRAME_MS: i32 = 50;
/// `MAX_RELIABLE_COMMANDS`: the netchan's 128-command rings (CoD2's,
/// `& 0x7f` in `SV_AddServerCommand`); the walk capture's walker
/// is sent 74 commands in 26 s.
const MAX_RELIABLE_COMMANDS: usize = NetchanParams::COD2.reliable_commands;
const PACKET_BACKUP: usize = crate::engine::slot::PACKET_BACKUP;
/// A frame older than this many messages is not deltaed from.
const MAX_DELTA_AGE: i32 = PACKET_BACKUP as i32 - 3;
/// `TEAM_SPECTATOR` in a client state's `team`, as the spectator capture's
/// roster reads (VERIFIED live, `play-1.7-spec.txt`).
const TEAM_SPECTATOR: u32 = 3;
/// `PM_SPECTATOR`: the spectator capture's `pm_type` (VERIFIED live).
const PM_SPECTATOR: u32 = 4;
/// `SV_SpawnServer`'s three 100 ms frames before `SV_CreateBaseline`.
const SPAWN_FRAMES: i32 = 3;
const SPAWN_FRAME_MS: i32 = 100;
/// `SNAPFLAG_SERVERCOUNT`, flipped by every spawn.
const SNAPFLAG_SERVERCOUNT: u8 = 4;
/// `SV_CheckTimeouts`' stale calls before the drop (0x8175830).
const TIMEOUT_CALLS: u32 = 5;
/// The zones `com_startup` loads ahead of the map's (`HUD_ZONES` plus
/// `ui_mp`), in load order.
pub const COMMON_ZONES: [&str; 5] = [
    "code_post_gfx_mp",
    "localized_code_post_gfx_mp",
    "ui_mp",
    "common_mp",
    "localized_common_mp",
];

/// What `vcod-server <map> --game cod4` starts with.
#[derive(Clone, Debug)]
pub struct Cod4Config {
    pub map: String,
    pub gametype: String,
    pub hostname: String,
    pub max_clients: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum State {
    /// `CS_ZOMBIE`: dropped, still sent its last commands until
    /// `sv_zombietime` passes without a packet.
    Zombie,
    /// `connectResponse` sent: the stats come in, bare messages go out.
    Connected,
    /// `CS_CLIENTLOADING`: the gamestate sent, not yet in the world.
    Primed,
    Active,
}

pub struct Client {
    link: crate::engine::slot::Link<(i32, Frame4)>,
    state: State,
    /// The message the gamestate went in; an older acknowledge is ignored.
    gamestate_message: i32,
    /// The frame the client's last `clc_move` acknowledged, -1 for none.
    delta_message: i32,
    /// `lastUsercmd.serverTime`.
    last_cmd_time: i32,
    /// The playerstate's `commandTime`: the newest cmd thought.
    command_time: i32,
    /// The 0x2000-byte stats block and which of its seven packets came.
    stats: Stats,
    stat_packets: u8,
    /// The `getchallenge` hex the client sent, its GUID (VERIFIED live, the
    /// console's `guid "<hex>"`).
    guid: String,
    /// `pureAuthentic`: `None` until a `cp`.
    pure: Option<bool>,
    rates: link::Rates,
    /// `Sys_IsLANAddress` of its address: its snapshots go out every frame,
    /// unpaced ([`Self::transmit`]).
    lan: bool,
    /// One of vcod's bots (`bots.rs`): no address, no messages.
    bot: bool,
    last_connect_ms: i32,
    /// Each reliable ring slot's `svscmd_type`: false for
    /// `SV_CMD_CAN_IGNORE` ([`add_server_command`]).
    reliable_kept: [bool; MAX_RELIABLE_COMMANDS],
    /// The file in flight (cod4-handshake.md 11).
    download: download::Download,
    /// Voice chat (`voice.rs`): the frames queued for this client this
    /// frame, its mute list and `sendVoice`.
    voice: voice::ClientVoice,
}

/// The pure lists: `sv_iwds`/`sv_iwdNames` in search order and the keyed
/// sums a `cp` is checked against.
#[derive(Default)]
struct Iwds {
    sums: Vec<i32>,
    names: Vec<String>,
    keyed: Vec<i32>,
    /// `sv_referencedIwdNames` and `sv_referencedIwds`: the iwds the level
    /// read a file from, `<game>/<name>`, set only under an `fs_game`.
    referenced: Vec<(String, i32)>,
}

/// The CoD4 game module beside the [`crate::engine::EngineCore`].
pub struct Cod4Game {
    cfg: Cod4Config,
    cvars: crate::cvars::Cvars,
    huff: Huffman,
    server_id: i32,
    snap_flags: u8,
    /// Where every client floats: `mp_global_intermission`'s origin and
    /// angles.
    spot: ([f32; 3], [f32; 3]),
    iwds: Iwds,
    /// `sv_referencedFFNames` and their sizes, `sv_referencedFFCheckSums`.
    fastfiles: Vec<(String, String)>,
    /// The walked zones, for the level the gsc boot runs over.
    pub zones: Vec<Arc<vcod_cod4::assets::Assets>>,
    /// Entity baselines; empty until entities spawn.
    baselines: BTreeMap<u32, EntityState>,
    /// `svs.time` at the last heartbeat.
    last_beat_ms: Option<i32>,
    /// The `getchallenge` GUID hex by address, for the connect.
    guids: std::collections::HashMap<SocketAddr, String>,
    /// The running level; `None` on a bare server, whose cvars are
    /// [`Self::cvars`]'s own. A level owns the one cvar table while it runs.
    pub level: Option<crate::game::cod4::level::Cod4Level>,
    /// The install every level loads from; `None` on a bare server.
    install: Option<Install>,
    /// The five common zones, walked once: a map change walks only the
    /// map's zone, as retail keeps them loaded.
    common: Vec<Arc<vcod_cod4::assets::Assets>>,
    /// The level's zones, which a `fast_restart` runs the scripts over
    /// again.
    zones4: Option<std::rc::Rc<crate::game::cod4::Cod4Zones>>,
    /// `sv.start_frameTime`: the frame the level last spawned or restarted
    /// on; a second restart in it does nothing (cod4-map-cycle.md 2).
    start_frame: Option<i32>,
    /// `svs.tempBans`: the GUIDs `tempBanClient` kicked and when.
    temp_bans: Vec<(String, i32)>,
    /// The socket and the wait `SV_SpawnServer` uses mid-command; without
    /// it (a test) its OOB lines wait in the core's `outbox`.
    net_now: Option<crate::cod2::NetNow>,
    /// The seed each level's `srand` takes when a test pinned one
    /// ([`Cod4Server::pin_game_seed`]); otherwise a draw off the engine's own.
    game_seed: Option<u32>,
    /// A directory whose loose `.gsc` each level reads over the zones'
    /// ([`Cod4Server::set_script_overrides`]).
    script_overrides: Option<std::path::PathBuf>,
    /// vcod's bots (`bots.rs`), once one is added.
    bots: Option<crate::bots::cod4::Cod4Bots>,
    /// `level.logFile`: `games_mp.log` while a level runs with `g_log` set
    /// (`gamelog`).
    log: gamelog::LevelLog,
    /// `killserver` ran: no level, no packet answered (VERIFIED live:
    /// retail answers neither rcon nor `getinfo` after it).
    killed: bool,
    /// The domains `dvar_bool`, `dvar_int` and `dvar_float` gave the dvars
    /// they made, keyed by the folded name (`oob::Domain`).
    domains: std::collections::HashMap<String, oob::Domain>,
}

impl Cod4Game {
    /// The cvar table: the level's while one runs (its scripts and the
    /// engine share it, as retail's one dvar table), else the engine's.
    pub fn cvars(&self) -> &crate::cvars::Cvars {
        match &self.level {
            Some(l) => &l.rt.host.cvars,
            None => &self.cvars,
        }
    }

    pub fn cvars_mut(&mut self) -> &mut crate::cvars::Cvars {
        match &mut self.level {
            Some(l) => &mut l.rt.host.cvars,
            None => &mut self.cvars,
        }
    }
}

pub type Cod4Server = crate::engine::Engine<Cod4Game>;

/// Every snapshot of a turn is built from one roster.
type Cod4Snapshots = Option<(Vec<ClientState>, BTreeMap<u32, EntityState>)>;

/// 1.7's `SV_FrameInternal` (0x8177010, cod4-server-frame.md): pings, then
/// `SV_PreFrame` (bots, configstrings 0 and 1, the mirror), then per owed
/// frame `G_RunFrame`, `SV_CheckTimeouts`, `SV_SendClientMessages` (the
/// fragments of every slot first, then the new messages, the archive at its
/// tail) and the heartbeat. Each cmd is thought as its packet arrives.
impl crate::engine::GameModule for Cod4Game {
    type Client = Client;
    type Wire = Cod4Wire;
    type Snapshots = Cod4Snapshots;

    const THINK: crate::engine::ThinkTiming = crate::engine::ThinkTiming::OnArrival;
    const HEAD: crate::engine::HeadOrder = crate::engine::HeadOrder::PingsBotsCs;
    const POST_EACH_FRAME: bool = true;
    const MIN_FRAMES: u32 = 0;
    const ARCHIVE_AFTER_MESSAGES: bool = true;
    const FRAGMENTS_FIRST: bool = true;

    fn wire(_e: &Cod4Server) -> &'static Cod4Wire {
        &PROTOCOL_17
    }
    fn status(
        e: &mut Cod4Server,
        q: crate::engine::StatusQuery,
        challenge: &str,
    ) -> Option<String> {
        if e.game.killed {
            return None;
        }
        Some(match q {
            crate::engine::StatusQuery::Info => e.svc_info(challenge),
            crate::engine::StatusQuery::Status => e.svc_status(challenge),
        })
    }
    fn connectionless(
        e: &mut Cod4Server,
        from: SocketAddr,
        pkt: &[u8],
        at: crate::engine::Arrival,
    ) {
        if !e.game.killed {
            e.connectionless(from, &pkt[4..], at.now);
        }
    }
    fn client_packet(
        e: &mut Cod4Server,
        from: SocketAddr,
        pkt: &[u8],
        _at: crate::engine::Arrival,
    ) {
        if !e.game.killed {
            e.client_packet(from, pkt);
        }
    }
    fn drain_console(e: &mut Cod4Server, _now: Instant) {
        let _p = crate::tick_profile::phase("console");
        e.drain_console();
    }
    fn configstrings_head(e: &mut Cod4Server) {
        e.broadcast_configstrings();
    }
    fn bots(e: &mut Cod4Server) {
        e.step_bots();
    }
    fn calc_pings(e: &mut Cod4Server) {
        for c in e.core.clients.iter_mut().flatten() {
            c.link.ping = if c.bot {
                0
            } else if c.state == State::Active {
                link::calc_ping(&c.link.ping_ring)
            } else {
                999
            };
        }
    }
    fn frame_step_ms(e: &mut Cod4Server) -> i32 {
        e.frame_ms()
    }
    fn wall_frame_ms(e: &Cod4Server) -> i32 {
        e.frame_ms()
    }
    fn run_frame(e: &mut Cod4Server) -> bool {
        crate::tick_trace::step!("G_RunFrame");
        e.run_level_frame();
        true
    }
    /// No killcam without a level: nothing reads the archive yet.
    fn archive(_e: &mut Cod4Server) {}
    fn check_timeouts(e: &mut Cod4Server) {
        e.check_timeouts();
    }
    fn snapshots_begin(_e: &mut Cod4Server) -> Cod4Snapshots {
        None
    }
    fn message_due(e: &Cod4Server, slot: usize) -> bool {
        let svs = e.core.sv_time_ms;
        e.core.clients[slot]
            .as_ref()
            .is_some_and(|c| !c.bot && c.link.next_message_ms <= svs)
    }
    fn next_fragment(e: &mut Cod4Server, slot: usize) -> Option<(SocketAddr, Vec<u8>)> {
        let svs = e.core.sv_time_ms;
        let max_rate = e.int("sv_maxRate");
        let c = e.core.clients[slot].as_mut()?;
        let pkt = c.link.unsent.pop_front()?;
        let left = pkt.len() - 10 + c.link.unsent.iter().map(|p| p.len() - 10).sum::<usize>();
        c.link.next_message_ms = svs + link::rate_msec(left, c.rates.send_rate(max_rate));
        Some((c.link.addr, pkt))
    }
    fn send_client_snapshot(e: &mut Cod4Server, slot: usize, snaps: &mut Cod4Snapshots) {
        let _p = crate::tick_profile::phase("messages/snapshot");
        if snaps.is_none() {
            let _p = crate::tick_profile::phase("messages/snapshot world");
            let roster = e.roster();
            // The roster's tag indices (`client_attachments`) are
            // `G_UpdateClientInfo`'s `G_TagIndex` inside retail's frame, so
            // their `d` goes out ahead of the snapshot that names them
            // (cod4-player-anims.md 5).
            e.flush_game_commands();
            let world = e.game.level.as_mut().map_or_else(BTreeMap::new, |l| {
                // The angles as 1.7's writer rounds them (`angle16_sent`).
                let w: BTreeMap<_, _> = l
                    .entities()
                    .into_iter()
                    .map(|(n, e)| (n, e.sent()))
                    .collect();
                l.archive_entities(&w);
                w
            });
            *snaps = Some((roster, world));
        }
        let (roster, world) = snaps.as_ref().unwrap();
        let (roster, mut world) = (roster.clone(), world.clone());
        // A killcam's follower sees the world as it was at the look back.
        if let Some(old) = e.game.level.as_ref().and_then(|l| l.killcam_entities(slot)) {
            world = old;
        }
        e.send_message(slot, &roster, &world);
        e.send_voice(slot);
    }
    fn heartbeat(e: &mut Cod4Server) {
        e.heartbeat();
    }
}

impl Cod4Server {
    /// The loop's socket and wait for `SV_SpawnServer`'s send and sleep.
    pub fn set_net_now(&mut self, net: crate::cod2::NetNow) {
        self.game.net_now = Some(net);
    }

    /// A server with no level: [`Self::load`] brings one up.
    pub fn new(mut cfg: Cod4Config, seed: u64) -> Self {
        cfg.max_clients = cfg.max_clients.clamp(1, crate::server::MAX_CLIENTS);
        let mut cvars = crate::cvars::Cvars::new();
        for (name, value) in oob::ENGINE_CVARS {
            cvars.register(name, value);
            cvars.set(name, value);
        }
        cvars.remirror(&PROTOCOL_17.cs, oob::MIRRORED);
        // 1.7's archived dvars, not the CoD1 table's (`writeconfig`).
        cvars.clear_flags(crate::cvars::flag::ARCHIVE);
        for name in oob::ARCHIVED {
            cvars.respell(name);
            cvars.add_flags(name, crate::cvars::flag::ARCHIVE);
        }
        cvars.set("sv_hostname", &cfg.hostname);
        cvars.set("ui_hostname", &cfg.hostname);
        cvars.set("sv_maxclients", &cfg.max_clients.to_string());
        cvars.set("g_gametype", &cfg.gametype);
        cvars.set("mapname", &cfg.map);
        let mut core = crate::engine::EngineCore::new(
            &PROTOCOL_17.transport,
            cfg.max_clients,
            seed | 1,
            // D6: never beaten unless `dedicated 2` names one ([`Self::heartbeat`]).
            crate::master::Masters::new(vcod_common::net::master::COD4_MASTER),
        );
        core.configstrings = vec![String::new(); PROTOCOL_17.transport.max_configstrings];
        let mut s = crate::engine::Engine {
            core,
            game: Cod4Game {
                cfg,
                cvars,
                huff: Huffman::new(),
                server_id: 0,
                snap_flags: 0,
                spot: ([0.0; 3], [0.0; 3]),
                iwds: Iwds::default(),
                fastfiles: Vec::new(),
                zones: Vec::new(),
                baselines: BTreeMap::new(),
                last_beat_ms: None,
                guids: Default::default(),
                level: None,
                install: None,
                common: Vec::new(),
                zones4: None,
                start_frame: None,
                temp_bans: Vec::new(),
                net_now: None,
                game_seed: None,
                script_overrides: None,
                bots: None,
                log: Default::default(),
                killed: false,
                // `sv_fps`, registered by `SV_Init` (0x81735c8).
                domains: [(
                    "sv_fps".to_string(),
                    oob::Domain::Int(
                        *crate::game::cod4::SV_FPS_DOMAIN.start(),
                        *crate::game::cod4::SV_FPS_DOMAIN.end(),
                    ),
                )]
                .into(),
            },
        };
        s.core.checksum_feed = s.rand();
        s.write_engine_configstrings();
        s.core.sent_configstrings = s.core.configstrings.clone();
        s
    }

    /// Test-facing: every level's `srand` (`G_InitGame`, 0x80b56ee) takes
    /// `seed`, as retail's took its wall clock's milliseconds, so a gate
    /// replays a capture's `rand()` from the seed recovered off it
    /// (cod4-combat.md 10.9).
    pub fn pin_game_seed(&mut self, seed: u32) {
        self.game.game_seed = Some(seed);
    }

    /// Each level reads the loose `.gsc` under `dir` over the zones'
    /// scripts, as retail reads a mod's under an `fs_game` (a probe gsc).
    pub fn set_script_overrides(&mut self, dir: std::path::PathBuf) {
        self.game.script_overrides = Some(dir);
    }

    /// The seed of the next level's `srand`.
    pub(crate) fn game_seed(&mut self) -> u32 {
        match self.game.game_seed {
            Some(s) => s,
            None => self.rand() as u32,
        }
    }

    /// `SV_SpawnServer`: the zones (the five common ones, then the map's),
    /// `default_mp.cfg`, the level (the gametype's bootstrap over the
    /// zones, the engine's cvars handed to it), three 100 ms frames, the
    /// baselines, the intermission spot off the map's entities and the
    /// pure lists. `None` runs a bare server: no script, the spectators
    /// float at the origin and nothing is pure.
    pub fn load(&mut self, install: Option<&Install>) -> anyhow::Result<()> {
        // `fs_game` (a `+set` ahead of the load) picks the mod's zone and
        // iwds (cod4-fastfile.md 13).
        // The Linux server keeps `fs_game` lowercased (VERIFIED live,
        // cod4-fastfile.md 13).
        let fs_game = self.game.cvars().get("fs_game").to_ascii_lowercase();
        self.game.cvars_mut().set("fs_game", &fs_game);
        self.game.install = install.map(|i| i.clone().with_fs_game(Some(&fs_game)));
        self.load_level(Default::default())
    }

    /// [`Self::load`]'s spawn over the kept install, the last level's
    /// `carry` handed to the new one.
    fn load_level(&mut self, carry: crate::game::script::Carry) -> anyhow::Result<()> {
        use anyhow::Context;
        self.game.killed = false;
        self.shutdown_game_log();
        // The last level hands the one cvar table back.
        if let Some(mut old) = self.game.level.take() {
            self.game.cvars = std::mem::take(&mut old.rt.host.cvars);
        }
        self.game.server_id = spawn_server_id(self.game.server_id);
        self.game.snap_flags ^= SNAPFLAG_SERVERCOUNT;
        self.game.start_frame = Some(self.core.sv_time_ms);
        self.game.zones4 = None;
        let server_id = self.game.server_id.to_string();
        self.game.cvars.set("sv_serverid", &server_id);
        // The level's `usermaps/<map>/` iwds join the search path.
        let install = self
            .game
            .install
            .clone()
            .map(|i| i.with_map(Some(&self.game.cfg.map)));
        let Some(install) = install.as_ref() else {
            // `SV_SpawnServer`'s frames ahead of the baselines.
            self.core.sv_time_ms += SPAWN_FRAMES * SPAWN_FRAME_MS;
            self.write_engine_configstrings();
            self.core.sent_configstrings = self.core.configstrings.clone();
            return Ok(());
        };
        let t0 = Instant::now();
        let lang = "english";
        if self.game.common.is_empty() {
            let mod_zone = install.mod_zone().map(|_| "mod");
            for name in COMMON_ZONES.into_iter().chain(mod_zone) {
                let path = install.zone_path(lang, name);
                let zone = vcod_cod4::zone::Zone::open_path(&path)
                    .and_then(|z| vcod_cod4::walk::walk(z, Default::default()))
                    .with_context(|| path.display().to_string())?;
                self.game.common.push(Arc::new(zone.assets));
            }
        }
        let mut zones = self.game.common.clone();
        let (map, walked) = vcod_cod4::map::Map::load(install, lang, &self.game.cfg.map)
            .with_context(|| format!("loading {}", self.game.cfg.map))?;
        zones.push(Arc::new(walked.assets));
        self.game.zones = zones;
        // The engine's `default_mp.cfg` under the command line's sets, as
        // CoD2's server runs its own (cod2-protocol.md 15.4). Retail execs
        // it and its `exec`s "from disk", out of the iwds, before any zone
        // loads (cod4-gsc.md 7).
        let dirs = install.iwd_dirs();
        let iwds = vcod_common::pk3::Pk3Fs::open_iwd_layers(&dirs[0], &dirs[1..])?;
        let cfg = iwd_cfg(&iwds, "default_mp.cfg", 0);
        let (gametype, mapname) = (self.game.cfg.gametype.clone(), self.game.cfg.map.clone());
        self.game.cvars_mut().exec_cfg_under_sets(&cfg);
        self.game.cvars_mut().set("g_gametype", &gametype);
        self.game.cvars_mut().set("mapname", &mapname);
        if let Some(spot) = intermission_spot(&map.entities) {
            self.game.spot = spot;
        }
        // The scripts read `code_post_gfx_mp`, `common_mp` and the map's
        // zone (cod4-gsc.md 4), and a mod's `mod.ff` over all three.
        let script_zones: Vec<Arc<vcod_cod4::assets::Assets>> = COMMON_ZONES
            .iter()
            .zip(&self.game.zones)
            .filter(|(n, _)| crate::game::cod4::BASE_ZONES.contains(n))
            .map(|(_, z)| z.clone())
            .chain(self.game.zones.last().cloned())
            .collect();
        let mod_zone = self.game.common.get(COMMON_ZONES.len()).cloned();
        let zones4 =
            crate::game::cod4::Cod4Zones::from_zones(install, script_zones, mod_zone, map)?;
        let now = self.core.sv_time_ms;
        let info = self.serverinfo();
        self.init_game_log(&info, now);
        // A level owns the one cvar table while it runs.
        let cvars = std::mem::take(&mut self.game.cvars);
        let seed = self.game_seed();
        let mut level = crate::game::cod4::level::Cod4Level::load_with(
            &zones4,
            install,
            &mapname,
            &gametype,
            cvars,
            false,
            self.game.script_overrides.clone(),
            now,
            carry,
            None,
            seed,
        )?;
        self.game.zones4 = Some(std::rc::Rc::new(zones4));
        // `SV_SpawnServer`'s frames ahead of the baselines.
        for _ in 0..SPAWN_FRAMES {
            self.core.sv_time_ms += SPAWN_FRAME_MS;
            level.run_frame(self.core.sv_time_ms);
        }
        level.rt.host.configstrings.take_writes();
        level.rt.take_client_commands_stamped();
        // `SV_CreateBaseline`.
        self.game.baselines = level
            .baselines()
            .into_iter()
            .map(|(n, e)| (n, e.sent()))
            .collect();
        for (i, v) in level.rt.configstrings().iter().enumerate().skip(2) {
            if let Some(slot) = self.core.configstrings.get_mut(i) {
                slot.clone_from(v);
            }
        }
        self.game.level = Some(level);
        self.game.fastfiles = referenced_fastfiles(install, lang, &self.game.cfg.map);
        self.game.iwds = pure_lists(&install.iwd_dirs(), self.core.checksum_feed)?;
        if let (Some(g), Some(z)) = (install.fs_game(), &self.game.zones4) {
            let usermap = install.usermap_dir(&self.game.cfg.map);
            self.game.iwds.referenced =
                referenced_iwds(z.source.iwds(), g, usermap.as_deref(), &self.game.cfg.map);
        }
        self.write_engine_configstrings();
        self.core.sent_configstrings = self.core.configstrings.clone();
        log::info!(
            "cod4: {} {} up in {:.0} ms, {} iwds, {} baselines, intermission at {:?}",
            self.game.cfg.map,
            self.game.cfg.gametype,
            t0.elapsed().as_secs_f64() * 1000.0,
            self.game.iwds.names.len(),
            self.game.baselines.len(),
            self.game.spot.0
        );
        Ok(())
    }

    /// Keeps `ban.txt` at `path` (`--ban-file`); without it the bans last
    /// for the run.
    pub fn set_ban_file(&mut self, path: std::path::PathBuf) {
        self.core.bans = crate::engine::bans::BanFile::open(path);
    }

    /// `+set name value` before the level loads, or an rcon `set`.
    pub fn set_cvar(&mut self, name: &str, value: &str) {
        self.game.cvars_mut().set(name, value);
        // CoD1's name sets a dvar 1.7 never reads (cod4-handshake.md 14.2).
        if name.eq_ignore_ascii_case("rconPassword") {
            log::warn!("CoD4 reads rcon_password, not rconPassword: rcon stays off");
        }
        if name.eq_ignore_ascii_case("sv_hostname") {
            self.game.cfg.hostname = value.to_string();
            self.game.cvars_mut().set("ui_hostname", value);
        }
        // `+set g_gametype` ahead of the first load is that level's, as
        // `SV_SpawnServer` takes the latched dvar; once a level runs the
        // latch waits for the next `map` (`cycle.rs`).
        if name.eq_ignore_ascii_case("g_gametype") && self.game.level.is_none() {
            self.game.cfg.gametype = value.to_string();
        }
        self.write_engine_configstrings();
    }

    pub fn cvar(&self, name: &str) -> &str {
        self.game.cvars().get(name)
    }

    /// `1000 / sv_fps`: how far each frame steps `svs.time` and the wall
    /// time it takes. Read every `SV_Frame`, as 1.7 does: an rcon
    /// `sv_fps 40` steps the next frame 25 (cod4-server-frame.md 8).
    pub fn frame_ms(&self) -> i32 {
        crate::game::cod4::frame_ms(self.game.cvars())
    }

    pub fn configstring(&self, i: usize) -> &str {
        &self.core.configstrings[i]
    }

    pub fn server_id(&self) -> i32 {
        self.game.server_id
    }

    fn int(&self, name: &str) -> i32 {
        crate::game::builtins::cvar::atoi(self.game.cvars().get(name))
    }

    fn pure(&self) -> bool {
        self.int("sv_pure") != 0
    }

    /// Configstrings 0, 1 and 2 and the dvar mirror (20..147, 148..275).
    fn write_engine_configstrings(&mut self) {
        self.core.configstrings[0] = self.serverinfo();
        self.core.configstrings[1] = self.systeminfo();
        // `CS_GAME_VERSION`: `cod` on every 1.7 capture.
        self.core.configstrings[2] = "cod".into();
        if let Err(e) = self.game.cvars().write_mirror(&mut self.core.configstrings) {
            log::warn!("cod4: {e:?}");
        }
    }

    /// `Dvar_InfoString(0x404)`: configstring 0 and `getstatus`'s info,
    /// case-folded order.
    fn serverinfo(&self) -> String {
        serverinfo_of(self.game.cvars())
    }

    /// `G_InitGame`'s log (0x80b5782..0x80b5ac7): `g_log` empty prints
    /// `Not logging to disk.`; else the file opens (`g_logSync`'s mode)
    /// and takes the separator and `InitGame: <serverinfo>` at `now`, the
    /// new `level.time` (VERIFIED live, games_mp.log). With no home path
    /// (a test's server) nothing is written.
    fn init_game_log(&mut self, info: &str, now: i32) {
        let cv = match &self.game.level {
            Some(l) => &l.rt.host.cvars,
            None => &self.game.cvars,
        };
        self.game.log.init(cv, cv.get("fs_game"), info, now);
    }

    /// The level's `G_LogPrintf` lines since the last call into the file;
    /// without one they are dropped, as retail's `G_LogPrintf` returns.
    fn drain_game_log(&mut self) {
        let Some(level) = self.game.level.as_mut() else {
            return;
        };
        let lines = std::mem::take(&mut level.rt.host.game_log);
        let _p = crate::tick_profile::phase("game log");
        self.game.log.write(lines);
    }

    /// `G_ShutdownGame` (0x80b5ce8): `ShutdownGame:` and the separator at
    /// the level's last `level.time`, then the file closed (VERIFIED live).
    fn shutdown_game_log(&mut self) {
        let (lines, at) = match self.game.level.as_mut() {
            Some(l) => (
                std::mem::take(&mut l.rt.host.game_log),
                l.rt.host.level_time_ms,
            ),
            None => (Vec::new(), self.core.sv_time_ms),
        };
        self.game.log.shutdown(lines, at, gamelog::SHUTDOWN);
    }

    /// Configstring 1: the systeminfo dvars, the pure lists (none off
    /// pure), the fastfiles, `sv_serverid`, and `\fs_game\\` last with
    /// `fs_game` empty (VERIFIED live on all four captures).
    fn systeminfo(&self) -> String {
        let mut pairs: Vec<(String, String)> = oob::SYSTEMINFO
            .iter()
            .map(|n| (n.to_string(), self.game.cvars().get(n).to_string()))
            .collect();
        if self.pure() {
            pairs.push((
                "sv_iwds".into(),
                self.game
                    .iwds
                    .sums
                    .iter()
                    .map(|s| format!("{s} "))
                    .collect(),
            ));
            pairs.push(("sv_iwdNames".into(), self.game.iwds.names.join(" ")));
        }
        pairs.push((
            "sv_referencedFFCheckSums".into(),
            self.game
                .fastfiles
                .iter()
                .map(|(_, s)| s.as_str())
                .collect::<Vec<_>>()
                .join(" "),
        ));
        pairs.push((
            "sv_referencedFFNames".into(),
            self.game
                .fastfiles
                .iter()
                .map(|(n, _)| n.as_str())
                .collect::<Vec<_>>()
                .join(" "),
        ));
        let refs = &self.game.iwds.referenced;
        pairs.push((
            "sv_referencedIwdNames".into(),
            refs.iter()
                .map(|(n, _)| n.as_str())
                .collect::<Vec<_>>()
                .join(" "),
        ));
        pairs.push((
            "sv_referencedIwds".into(),
            refs.iter().map(|(_, s)| format!("{s} ")).collect(),
        ));
        pairs.push(("sv_serverid".into(), self.game.server_id.to_string()));
        pairs.push(("sv_pure".into(), u8::from(self.pure()).to_string()));
        // A set `fs_game` sorts with the rest; an empty one trails
        // (VERIFIED live, cod4-fastfile.md 13).
        let fs_game = self.game.cvars().get("fs_game").to_string();
        if !fs_game.is_empty() {
            pairs.push(("fs_game".into(), fs_game.clone()));
        }
        let mut s = info_sorted(pairs.into_iter());
        if fs_game.is_empty() {
            s.push_str("\\fs_game\\\\");
        }
        s
    }

    /// Runs the queued console lines.
    fn drain_console(&mut self) {
        let lines: Vec<String> = std::mem::take(&mut self.core.console)
            .iter()
            .flat_map(|l| crate::console::split_commands(l))
            .collect();
        for line in lines {
            let mut out = crate::rcon::Redirect::default();
            self.exec(&line, &mut out);
            for p in out.finish() {
                let text = p.trim_start_matches("print\n");
                if !text.is_empty() {
                    log::info!("{}", text.trim_end());
                }
            }
        }
    }

    fn send_oob(&mut self, to: SocketAddr, text: &str) {
        let pkt = PROTOCOL_17.oob_packet(text);
        self.core.outbox.push((to, pkt));
    }

    /// `SV_MasterHeartbeat("COD-4")` (0x81780e4): at `dedicated 2` only,
    /// every 180 s, and only to a master `sv_master1` names (D6: no CoD4
    /// list vcod may beat by default).
    fn heartbeat(&mut self) {
        let master = self.game.cvars().get("sv_master1").to_string();
        if self.int("dedicated") != 2 || master.is_empty() {
            return;
        }
        let now = self.core.sv_time_ms;
        if self.game.last_beat_ms.is_some_and(|t| now - t < 180_000) {
            return;
        }
        self.game.last_beat_ms = Some(now);
        let with_port = if master.contains(':') {
            master
        } else {
            format!("{master}:20810")
        };
        if let Some(to) = (self.core.resolver)(&with_port) {
            log::info!("Sending heartbeat to {with_port}");
            self.send_oob(to, "heartbeat COD-4\n");
        }
    }

    /// A netchan packet: its slot by address and qport, the netchan, then
    /// `SV_ExecuteClientMessage`.
    fn client_packet(&mut self, from: SocketAddr, pkt: &[u8]) {
        if pkt.len() < 6 {
            return;
        }
        let qport = u16::from_le_bytes([pkt[4], pkt[5]]);
        let Some(slot) = self.slot_of(from, qport) else {
            return;
        };
        let svs = self.core.sv_time_ms;
        let c = self.core.clients[slot].as_mut().unwrap();
        let Some(m) = c.link.netchan.process_in(pkt) else {
            return;
        };
        let seq = c.link.netchan.reliable_sequence as i32;
        if m.reliable_ack > seq || seq - m.reliable_ack >= MAX_RELIABLE_COMMANDS as i32 {
            return;
        }
        c.link.netchan.accept(&m);
        c.link.addr = from;
        c.link.reliable_ack = m.reliable_ack;
        if c.state == State::Zombie {
            return;
        }
        c.link.last_packet_ms = svs;
        if c.state == State::Connected {
            // `SV_SendClientGameState`: only once all seven stats
            // packets are in; with none yet, the client is asked again.
            if c.stat_packets == (1 << STAT_PACKETS) - 1 {
                self.send_gamestate(slot);
            } else if c.stat_packets == 0 {
                self.send_oob(from, "requeststats\n");
            }
            return;
        }
        let stale = i32::from(m.server_id) != self.game.server_id & 0xff;
        if stale && i32::from(m.server_id) & 0xf0 == self.game.server_id & 0xf0 {
            // A message from before a `fast_restart` (0x816c466 kept the
            // high nibble): nothing read, a loading client into the world
            // (`SV_ExecuteClientMessage`, cod4-map-cycle.md 2).
            if c.state == State::Primed {
                self.enter_world(slot);
            }
            return;
        }
        let plain = self.game.huff.decompress(&m.ops);
        if stale {
            // Another level's message: only its commands, which run as from
            // the old server (its usercmds are keyed to the last feed), then
            // the gamestate again once the client has acknowledged past the
            // last one (VERIFIED live).
            for op in clmsg::read_commands(plain) {
                if let ClOp::Command { sequence, text } = op
                    && !self.client_command(slot, sequence, &text, true)
                {
                    return;
                }
            }
            let c = self.core.clients[slot].as_ref().unwrap();
            if m.message_ack > c.gamestate_message {
                self.send_gamestate(slot);
            }
            return;
        }
        let key = clmsg::usercmd_key(
            self.core.checksum_feed,
            m.message_ack,
            c.link.netchan.reliable[m.reliable_ack as usize & (MAX_RELIABLE_COMMANDS - 1)]
                .as_bytes(),
        );
        let ops = match clmsg::read_ops(plain, key, &UserCmd::default()) {
            Ok(ops) => ops,
            Err(e) => {
                log::debug!("cod4: client {slot}: {e:#}");
                return;
            }
        };
        for op in ops {
            match op {
                ClOp::Command { sequence, text } => {
                    if !self.client_command(slot, sequence, &text, false) {
                        return;
                    }
                }
                ClOp::Move { delta, cmds, .. } => {
                    let cmds: Vec<UserCmd> = cmds.into_iter().map(|(c, _)| c).collect();
                    self.user_move(slot, m.message_ack, delta, &cmds);
                    return;
                }
                ClOp::Eof => return,
            }
        }
    }

    /// The slot holding `from`'s address and `qport`.
    fn slot_of(&self, from: SocketAddr, qport: u16) -> Option<usize> {
        self.core.clients.iter().position(|c| {
            c.as_ref()
                .is_some_and(|c| c.link.addr.ip() == from.ip() && c.link.netchan.qport == qport)
        })
    }

    /// One `clc_clientCommand`. False when the client was dropped. One
    /// from the last level's messages (`from_old`) runs only the engine's
    /// `disconnect` and `userinfo` (Q3's `SV_ExecuteClientCommand` rule,
    /// INFERRED).
    fn client_command(&mut self, slot: usize, seq: i32, text: &[u8], from_old: bool) -> bool {
        let Some(c) = self.core.clients[slot].as_mut() else {
            return false;
        };
        if seq <= c.link.last_client_command {
            return true;
        }
        if seq > c.link.last_client_command + 1 {
            self.drop_client(slot, "EXE_LOSTRELIABLECOMMANDS");
            return false;
        }
        let text: String = text.iter().map(|&b| char::from(b)).collect();
        c.link.last_client_command = seq;
        c.link.netchan.last_client_command_string = text.clone();
        log::debug!("cod4: client {slot} command {seq} {text:?}");
        let args = crate::game::say::tokenize(&text);
        match args.first().map(String::as_str) {
            Some(c) if from_old && c != "disconnect" && c != "userinfo" => {}
            Some("cp") => {
                let argv: Vec<&str> = args.iter().map(String::as_str).collect();
                let ok = vcod_cod4::net::pure::verify_cp(
                    &argv,
                    &self.game.iwds.keyed,
                    self.core.checksum_feed,
                );
                log::info!("cod4: client {slot} cp {}", if ok { "ok" } else { "bad" });
                c.pure = Some(ok);
            }
            Some("disconnect") => {
                self.drop_client(slot, "EXE_DISCONNECTED");
                return false;
            }
            Some("download" | "nextdl" | "stopdl" | "donedl" | "retransdl" | "wwwdl") => {
                return self.download_command(slot, &args);
            }
            Some(m @ ("muteplayer" | "unmuteplayer")) => {
                let mute = m == "muteplayer";
                c.voice
                    .mute_command(args.get(1).map_or("", String::as_str), mute);
            }
            // `SV_UpdateUserinfo_f` (0x8170536): the engine's copy, then
            // `ClientUserinfoChanged` (0x80a8068) for the game's name.
            Some("userinfo") => {
                c.link.userinfo = args.get(1).cloned().unwrap_or_default();
                // Never the LAN rate: `SV_UserinfoChanged` gives it only
                // below `dedicated 2` (0x817038f), which cod4_lnxded's
                // read-only `dedicated` never is (cod4-handshake.md 15).
                c.rates = link::Rates::of(&c.link.userinfo, false, 1);
                c.voice.send = voice::wants_voice(&c.link.userinfo);
                let name = client_name(&c.link.userinfo);
                if let Some(level) = self.game.level.as_mut() {
                    level.userinfo_changed(slot, &name);
                }
            }
            Some(_) => self.game_command(slot, &args, &text),
            None => {}
        }
        self.core.clients[slot]
            .as_ref()
            .is_some_and(|c| c.state != State::Zombie)
    }

    /// The engine's download commands (the table at 0x826d398, cod4-handshake.md
    /// 11). False when the client was dropped.
    fn download_command(&mut self, slot: usize, args: &[String]) -> bool {
        let svs = self.core.sv_time_ms;
        let arg = args.get(1).map_or("", String::as_str);
        let atoi = crate::game::builtins::cvar::atoi;
        let Some(c) = self.core.clients[slot].as_mut() else {
            return false;
        };
        match args[0].as_str() {
            "download" => c.download.begin(arg),
            "nextdl" => {
                if !c.download.next_dl(atoi(arg), svs) {
                    self.drop_client(slot, "broken download");
                    return false;
                }
            }
            "stopdl" => {
                if c.download.active() {
                    log::info!(
                        "clientDownload: {slot} : file {:?} aborted",
                        c.download.name
                    );
                }
                c.download.close();
            }
            // 0x816fd00: the transfer closed, then `SV_SendClientGameState`.
            "donedl" => {
                log::info!("clientDownload: {slot} Done");
                c.download.close();
                self.send_gamestate(slot);
            }
            "retransdl" => c.download.retransmit(atoi(arg)),
            _ => match c.download.wwwdl(arg) {
                download::Www::None => {}
                download::Www::Gamestate => self.send_gamestate(slot),
                download::Www::Drop(reason) => {
                    self.drop_client(slot, reason);
                    return false;
                }
            },
        }
        true
    }

    /// `ClientCommand` for what the engine leaves to the game: `mr`
    /// (`mr <serverId> <menu> <response>`, the walk capture's `mr 16 8
    /// allies`, cod4-netfields.md 8), `say`, `say_team`, `score`, `kill`,
    /// the votes and `setviewpos`.
    fn game_command(&mut self, slot: usize, args: &[String], text: &str) {
        let server_id = self.game.server_id;
        let pings: Vec<Option<i32>> = self
            .core
            .clients
            .iter()
            .map(|c| c.as_ref().map(|c| c.link.ping))
            .collect();
        let Some(level) = self.game.level.as_mut() else {
            return;
        };
        let cmd = args[0].to_ascii_lowercase();
        match cmd.as_str() {
            // `ClientCommand`: at the intermission only `say`,
            // `say_team` and `score`.
            c if !matches!(c, "say" | "say_team" | "score") && level.at_intermission(slot) => {}
            "mr" => {
                if let Some((menu, response)) = crate::server::parse_menu_response(text, server_id)
                {
                    level.menu_response(slot, &menu, &response);
                }
            }
            "say" | "say_team" => {
                let mode = if cmd == "say" {
                    crate::game::say::SayMode::All
                } else {
                    crate::game::say::SayMode::Team
                };
                let rest = text
                    .trim_start()
                    .split_once(char::is_whitespace)
                    .map_or("", |(_, r)| r);
                if let Some(line) = crate::game::say::concat_args(rest) {
                    for (to, line) in level.rt.say(slot, None, mode, &line) {
                        self.send_server_command(to, &line);
                    }
                }
            }
            "score" => {
                let text = level.scoreboard(&pings);
                self.send_server_command(slot, &text);
            }
            "callvote" | "vote" => {
                for (to, line) in level.vote_command(slot, args) {
                    match to {
                        Some(to) => self.send_server_command(to, &line),
                        None => self.send_to_all(&line),
                    }
                }
            }
            "kill" => {
                if let Some(reply) = level.kill_command(slot) {
                    self.send_server_command(slot, &reply);
                }
            }
            "setviewpos"
                if crate::game::builtins::cvar::atoi(level.rt.host.cvars.get("sv_cheats")) != 0 =>
            {
                let n = |i: usize| {
                    args.get(i)
                        .map_or(0.0, |a| crate::game::builtins::cvar::atof(a))
                };
                if (4..=6).contains(&args.len()) {
                    let mut angles = [0.0; 3];
                    if args.len() >= 5 {
                        angles[1] = n(4);
                    }
                    if args.len() == 6 {
                        angles[0] = n(5);
                    }
                    level.set_view_pos(slot, [n(1), n(2), n(3)], angles);
                }
            }
            _ => log::debug!("cod4: client {slot}: game command {cmd:?} not handled"),
        }
        self.flush_game_commands();
    }

    /// `G_RunFrame` on the frame clock, then the commands and
    /// configstrings it raised out to their clients.
    fn run_level_frame(&mut self) {
        let now = self.core.sv_time_ms;
        let fps = crate::game::cod4::sv_fps(self.game.cvars());
        if let Some(level) = self.game.level.as_mut() {
            level.set_sv_fps(fps);
            level.rt.set_network_time(now);
            level.run_frame(now);
        }
        let _p = crate::tick_profile::phase("frame/flush");
        self.flush_game_commands();
    }

    /// The configstrings the script wrote and the server commands it
    /// queued, in the order they were raised, as retail's
    /// `SV_SetConfigstring` and `SV_GameSendServerCommand` send each at
    /// once. The dvar mirror's changes wait for the next frame's head.
    fn flush_game_commands(&mut self) {
        let Some(level) = self.game.level.as_mut() else {
            return;
        };
        let writes = level.rt.host.configstrings.take_writes_stamped();
        let commands = level.rt.take_client_commands_stamped();
        let console = level.rt.take_console();
        let mut writes = writes.into_iter().peekable();
        for (seq, to, cmd) in commands {
            while let Some((_, i, v)) = writes.next_if(|w| w.0 < seq) {
                self.write_configstring(i, &v);
            }
            if to == crate::game::commands::ALL_CLIENTS {
                self.send_to_all(&cmd);
            } else {
                self.send_server_command(to, &cmd);
            }
        }
        for (_, i, v) in writes {
            self.write_configstring(i, &v);
        }
        self.core.console.extend(console);
        self.drain_game_log();
    }

    /// `SV_SetConfigstring` of a game write: the table and `d <index>
    /// <string>` to every client past `CS_CONNECTED`. 0 and 1 are the
    /// engine's.
    fn write_configstring(&mut self, i: usize, value: &str) {
        if i < 2 || i >= self.core.configstrings.len() || self.core.configstrings[i] == value {
            return;
        }
        self.core.configstrings[i] = value.to_string();
        if let Some(sent) = self.core.sent_configstrings.get_mut(i) {
            *sent = value.to_string();
        }
        self.send_to_all(&format!("d {i} {value}"));
    }

    /// `SV_UserMove` (0x8172560): the delta base and ping, a primed client
    /// into the world, the pure check (0x81727b8: no `cp` drops with
    /// `EXE_CANNOTVALIDATEPURECLIENT`, a failed one `EXE_UNPURECLIENTDETECTED`,
    /// cod4-handshake.md 8), then each cmd newer than the last thought.
    fn user_move(&mut self, slot: usize, message_ack: i32, delta: bool, cmds: &[UserCmd]) {
        crate::tick_trace::step!("SV_UserMove slot {slot}");
        let pure = self.pure();
        let now = self.core.sv_time_ms;
        let c = self.core.clients[slot].as_mut().unwrap();
        c.delta_message = if delta { message_ack } else { -1 };
        if message_ack >= 0 {
            c.link.ping_ring[message_ack as usize % PACKET_BACKUP].1 = now;
        }
        if c.state == State::Primed {
            self.enter_world(slot);
        }
        let c = self.core.clients[slot].as_mut().unwrap();
        if pure {
            match c.pure {
                None => return self.drop_client(slot, "EXE_CANNOTVALIDATEPURECLIENT"),
                Some(false) => return self.drop_client(slot, "EXE_UNPURECLIENTDETECTED"),
                Some(true) => {}
            }
        }
        let newest = cmds.last().map_or(0, |c| c.server_time);
        for cmd in cmds {
            let Some(c) = self.core.clients[slot].as_mut() else {
                return;
            };
            if cmd.server_time > newest || cmd.server_time <= c.last_cmd_time {
                continue;
            }
            // `SV_ClientThink` refuses a cmd 20 s or more ahead (0x816deca).
            if cmd.server_time - now >= 20_000 {
                continue;
            }
            c.last_cmd_time = cmd.server_time;
            // No pmove yet: the cmd moves the clock and the view.
            c.command_time = cmd.server_time;
            if let Some(level) = self.game.level.as_mut() {
                use crate::game::cod4::level::Inactivity;
                match level.inactivity_timer(slot, cmd) {
                    Inactivity::Drop => return self.drop_client(slot, "GAME_DROPPEDFORINACTIVITY"),
                    // `SV_GameSendServerCommand(.., "%c \"GAME_INACTIVEDROPWARNING\"", 'c')`
                    // (0x80a4e10), VERIFIED live.
                    Inactivity::Warn => level
                        .rt
                        .host
                        .client_commands
                        .push((slot, "c \"GAME_INACTIVEDROPWARNING\"".to_string())),
                    Inactivity::Keep => {}
                }
                level.client_think(slot, cmd);
            }
        }
        self.flush_game_commands();
    }

    /// `SV_ClientEnterWorld`: `CS_ACTIVE` and `ClientBegin`.
    fn enter_world(&mut self, slot: usize) {
        let Some(c) = self.core.clients[slot].as_mut() else {
            return;
        };
        c.state = State::Active;
        log::info!("cod4: client {slot} entered the world");
        if let Some(level) = self.game.level.as_mut() {
            level.begin(slot);
        }
    }

    /// `SV_SendClientGameState`: the unacknowledged commands, the
    /// configstrings by the default rule, the baselines, then
    /// `CS_CLIENTLOADING`.
    fn send_gamestate(&mut self, slot: usize) {
        let feed = self.core.checksum_feed;
        let c = self.core.clients[slot].as_mut().unwrap();
        let mut w = MsgWriter::new(&UNUSED_HUFF);
        write_pending_commands(&mut w, c);
        let gs = Gamestate::from_table(
            c.link.netchan.reliable_sequence as i32,
            &self.core.configstrings,
            &self.game.baselines,
            slot as i32,
            feed,
        );
        PROTOCOL_17.write_gamestate(&mut w, &gs);
        c.gamestate_message = c.link.netchan.outgoing_sequence as i32;
        c.state = State::Primed;
        c.delta_message = -1;
        c.pure = None;
        log::info!(
            "cod4: client {slot} gamestate, message {}",
            c.gamestate_message
        );
        let ops = w.into_ops();
        self.transmit(slot, ops);
    }

    /// `SV_SendMessageToClient`: the message through the netchan, its first
    /// packet now and any other one a turn, then `nextSnapshotTime`.
    fn transmit(&mut self, slot: usize, mut ops: Vec<u8>) {
        let svs = self.core.sv_time_ms;
        let max_rate = self.int("sv_maxRate");
        let Some(c) = self.core.clients[slot].as_mut() else {
            return;
        };
        ops.push(EOF);
        let block = self.game.huff.compress(&ops);
        let mut pkts = c
            .link
            .netchan
            .transmit_block(c.link.last_client_command, &block);
        let bytes = match pkts.as_slice() {
            [one] => one.len() - 4,
            many => many.iter().map(|p| p.len() - 10).sum(),
        };
        // `SV_SendMessageToClient` (0x8178b3e..0x8178bed): a LAN client's
        // next message is due the next frame, anyone else's when its rate
        // allows, and a second on for one neither in the world nor
        // downloading (cod4-handshake.md 15). The loopback walker was sent
        // its 359-byte spawn frame and the next snapshot 50 ms apart at
        // `sv_maxRate` 5000, and a client not yet in the world every 50 ms
        // (VERIFIED live, cod4-pmove.md 15, cod4-handshake.md 3). The
        // fragments that follow a message's first are rate-paced for every
        // client (`next_fragment`; the gamestate's 350 ms apart, VERIFIED
        // live).
        let idle = c.state != State::Active && !c.download.active();
        match link::pace(
            svs,
            bytes,
            c.lan,
            c.rates.send_rate(max_rate),
            c.rates.snapshot_msec,
            idle,
        ) {
            link::Pace::Lan => c.link.next_message_ms = svs - 1,
            link::Pace::At { next, delayed } => {
                c.link.next_message_ms = next;
                c.link.rate_delayed = delayed;
            }
        }
        let to = c.link.addr;
        if !pkts.is_empty() {
            self.core.outbox.push((to, pkts.remove(0)));
        }
        c.link.unsent = pkts.into();
    }

    /// `SV_AddServerCommand` for one game or engine command, its type read
    /// off its letter ([`ignorable`]); a full ring drops the client.
    pub fn send_server_command(&mut self, slot: usize, text: &str) {
        let Some(c) = self.core.clients[slot].as_mut() else {
            return;
        };
        if !add_server_command(c, text, !ignorable(text)) {
            self.drop_client(slot, "EXE_SERVERCOMMANDOVERFLOW");
        }
    }

    /// Every client past `CS_CONNECTED`, zombies left out.
    fn send_to_all(&mut self, cmd: &str) {
        for slot in 0..self.core.clients.len() {
            if self.core.clients[slot]
                .as_ref()
                .is_some_and(|c| c.state > State::Connected)
            {
                self.send_server_command(slot, cmd);
            }
        }
    }

    /// `SV_DropClient` (0x817096e): the slot turns `CS_ZOMBIE`; everyone
    /// else reads `e "\x15<name>^7 \x14<reason>"` (`EXE_LEFTGAME` for a
    /// disconnect; no `\x14` for a reason the server's strings lack,
    /// cod4-handshake.md 2) and `K <slot>`; a client in the world its reason as
    /// `w "<reason>"` on the zombie's next message (VERIFIED live for
    /// `EXE_CANNOTVALIDATEPURECLIENT`), one still loading as an OOB
    /// `disconnect <reason>`.
    fn drop_client(&mut self, slot: usize, reason: &str) {
        let Some(c) = self.core.clients[slot].as_mut() else {
            return;
        };
        if c.state == State::Zombie {
            return;
        }
        let was = c.state;
        c.state = State::Zombie;
        let name = client_name(&c.link.userinfo);
        let addr = c.link.addr;
        log::info!("{slot}:{name} {reason}");
        self.core.challenges.release(addr);
        if let Some(level) = self.game.level.as_mut() {
            level.disconnect(slot);
        }
        self.flush_game_commands();
        let shown = if reason.eq_ignore_ascii_case("EXE_DISCONNECTED") {
            "EXE_LEFTGAME"
        } else {
            reason
        };
        let mark = if self.code_string(shown) {
            "\u{14}"
        } else {
            ""
        };
        self.send_to_all(&format!("e \"\u{15}{name}^7 {mark}{shown}\""));
        self.send_to_all(&format!("K {slot}"));
        if was == State::Active {
            self.send_server_command(slot, &format!("w \"{reason}\""));
        } else {
            self.send_oob(addr, &format!("disconnect {reason}"));
        }
    }

    /// Whether `localized_code_post_gfx_mp`, the one localized zone the
    /// dedicated server loads, holds `key` (`SEH_StringEd_GetString`,
    /// 0x817f4b0).
    fn code_string(&self, key: &str) -> bool {
        let at = COMMON_ZONES
            .iter()
            .position(|&z| z == "localized_code_post_gfx_mp");
        at.and_then(|i| self.game.common.get(i))
            .is_some_and(|z| z.localize.iter().any(|l| l.name.eq_ignore_ascii_case(key)))
    }

    /// `SV_CheckTimeouts` (0x8175830, cod4-server-frame.md 3): a future
    /// `lastPacketTime` clamped, a zombie silent `sv_zombietime` freed, a
    /// client silent past `sv_timeout` (`sv_connectTimeout` while loading)
    /// dropped on its sixth stale call.
    fn check_timeouts(&mut self) {
        let svs = self.core.sv_time_ms;
        let active = svs - self.int("sv_timeout") * 1000;
        let loading = svs - self.int("sv_connectTimeout") * 1000;
        let zombie = svs - self.int("sv_zombietime") * 1000;
        for slot in 0..self.core.clients.len() {
            let Some(c) = self.core.clients[slot].as_mut() else {
                continue;
            };
            c.link.last_packet_ms = c.link.last_packet_ms.min(svs);
            let last = c.link.last_packet_ms;
            match c.state {
                State::Zombie if last < zombie => {
                    log::debug!("Going from CS_ZOMBIE to CS_FREE for client #{slot}");
                    self.core.clients[slot] = None;
                }
                State::Zombie => {}
                s => {
                    let limit = if s == State::Active { active } else { loading };
                    if last >= limit {
                        c.link.timeout_count = 0;
                    } else {
                        c.link.timeout_count += 1;
                        if c.link.timeout_count > TIMEOUT_CALLS {
                            self.drop_client(slot, "EXE_TIMEDOUT");
                        }
                    }
                }
            }
        }
    }

    /// Every changed configstring as `d <index> <string>` to the clients
    /// holding a table (`SV_SetConfigstring`, Q3's form).
    fn broadcast_configstrings(&mut self) {
        self.write_engine_configstrings();
        let changed: Vec<usize> = (0..self.core.configstrings.len())
            .filter(|&i| self.core.sent_configstrings.get(i) != Some(&self.core.configstrings[i]))
            .collect();
        if changed.is_empty() {
            return;
        }
        self.core.sent_configstrings = self.core.configstrings.clone();
        for i in changed {
            let cmd = format!("d {i} {}", self.core.configstrings[i]);
            self.send_to_all(&cmd);
        }
    }

    /// One client state per client with a gamestate: a spectator with its
    /// userinfo's name, as the spectator capture's roster reads (team 3,
    /// `attachedVehEntNum` 1023, VERIFIED live).
    fn roster(&mut self) -> Vec<ClientState> {
        if let Some(level) = self.game.level.as_mut() {
            return self
                .core
                .clients
                .iter()
                .enumerate()
                .filter_map(|(i, c)| {
                    c.as_ref().filter(|c| c.state != State::Connected)?;
                    // The game's name, which `ClientUserinfoChanged` keeps.
                    let name = level.client_name(i).to_string();
                    Some(level.client_state(i, &name))
                })
                .collect();
        }
        let word = |name: &str| CLIENT_STATE.iter().find(|f| f.name == name).unwrap().word();
        self.core
            .clients
            .iter()
            .enumerate()
            .filter_map(|(i, c)| {
                let c = c.as_ref().filter(|c| c.state != State::Connected)?;
                let mut cs = ClientState::default();
                cs.0[0] = i as u32;
                cs.0[word("team")] = TEAM_SPECTATOR;
                cs.0[word("attachedVehEntNum")] = ENTITYNUM_NONE;
                let mut name = [0u8; 16];
                for (d, s) in name
                    .iter_mut()
                    .zip(client_name(&c.link.userinfo).bytes().take(15))
                {
                    *d = s;
                }
                let at = word("name[0]");
                for k in 0..4 {
                    cs.0[at + k] = u32::from_le_bytes(name[k * 4..k * 4 + 4].try_into().unwrap());
                }
                Some(cs)
            })
            .collect()
    }

    /// `SV_SendClientSnapshot`: an active client or a zombie gets its
    /// server commands and a snapshot, any other a bare message.
    fn send_message(
        &mut self,
        slot: usize,
        roster: &[ClientState],
        world: &BTreeMap<u32, EntityState>,
    ) {
        let svs = self.core.sv_time_ms;
        let frame = self.frame_ms();
        let (origin, angles) = self.game.spot;
        let map_center = vcod_cod4::net::wire::map_center(
            self.core.configstrings[vcod_cod4::net::parse::CS_MAPCENTER as usize].as_bytes(),
        );
        let flags_base = self.game.snap_flags;
        let max_rate = self.int("sv_maxRate");
        let dl_cvars = download::DownloadCvars {
            allow: self.int("sv_allowDownload") != 0,
            pure: self.pure(),
            www: self.int("sv_wwwDownload") != 0,
            www_base_url: self.game.cvars().get("sv_wwwBaseURL").to_string(),
            www_disconnected: self.int("sv_wwwDlDisconnected") != 0,
        };
        let level_ps = self
            .game
            .level
            .as_ref()
            .and_then(|l| l.playerstate(slot))
            .cloned();
        // The temp entities a single-client scope keeps from the client
        // the playerstate views as (`r.clientMask`, cod4-combat.md 4).
        let viewer_num = level_ps
            .as_ref()
            .and_then(|p| p.get("clientNum"))
            .unwrap_or(slot as u32);
        let hidden = self
            .game
            .level
            .as_ref()
            .map_or_else(Vec::new, |l| l.hidden_from(viewer_num as usize));
        let Some(c) = self.core.clients[slot].as_mut() else {
            return;
        };
        let mut w = MsgWriter::new(&UNUSED_HUFF);
        if matches!(c.state, State::Active | State::Zombie) {
            write_pending_commands(&mut w, c);
            let message = c.link.netchan.outgoing_sequence as i32;
            let mut ps = level_ps.unwrap_or_else(|| {
                spectator_ps(slot, origin, angles, c.command_time.max(svs - frame))
            });
            ps.sent_angles();
            // Every entity but the one the playerstate views from: retail
            // culls none by position (cod4-pmove.md 15).
            let viewer = ps.get("clientNum").unwrap_or(slot as u32);
            let to = Frame4 {
                server_time: svs,
                map_center,
                ps,
                entities: world
                    .values()
                    .filter(|e| e.number() != viewer && !hidden.contains(&e.number()))
                    .copied()
                    .collect(),
                clients: roster.to_vec(),
            };
            let from = (c.state == State::Active
                && c.delta_message > 0
                && c.delta_message > c.gamestate_message
                && message - c.delta_message < MAX_DELTA_AGE)
                .then(|| {
                    c.link.frames[c.delta_message as usize % PACKET_BACKUP]
                        .as_ref()
                        .filter(|(n, _)| *n == c.delta_message)
                        .map(|(_, f)| f)
                })
                .flatten();
            if c.state == State::Active {
                c.link.was_active = true;
            }
            let flags =
                flags_base | u8::from(c.link.rate_delayed) | if c.link.was_active { 0 } else { 2 };
            let head = SnapshotHead {
                delta_num: from.map_or(0, |_| (message - c.delta_message) as u8),
                snap_flags: u32::from(flags),
            };
            PROTOCOL_17.write_snapshot(&mut w, from, &to, head, &self.game.baselines);
            c.link.frames[message as usize % PACKET_BACKUP] = Some((message, to));
        }
        if c.download.active() && c.state != State::Zombie {
            let rate = c.rates.send_rate(max_rate);
            let budget = vcod_cod4::net::download::blocks_per_message(rate, c.rates.snapshot_msec);
            let wants_www =
                vcod_common::net::info_value_for_key(&c.link.userinfo, "cl_wwwDownload")
                    .is_some_and(|v| v != "0");
            let root = self.game.install.as_ref().map(|i| i.root().to_path_buf());
            let open = |name: &str| {
                download::servable(root.as_deref()?, name).and_then(|p| std::fs::read(p).ok())
            };
            c.download
                .write(&mut w, svs, budget, &dl_cvars, wants_www, &open);
        }
        c.link.ping_ring[c.link.netchan.outgoing_sequence as usize % PACKET_BACKUP] = (svs, -1);
        let ops = w.into_ops();
        self.transmit(slot, ops);
    }

    /// A new client in `slot`, a spectator in `CS_CONNECTED`.
    fn admit(
        &mut self,
        slot: usize,
        from: SocketAddr,
        qport: u16,
        challenge: i32,
        guid: String,
        userinfo: &str,
    ) {
        let svs = self.core.sv_time_ms;
        let mut link = crate::engine::slot::Link::new(
            from,
            ServerNetchan::with_params(qport, challenge, NetchanParams::COD2),
            userinfo.to_string(),
        );
        link.next_message_ms = svs;
        link.last_packet_ms = svs;
        if let Some(level) = self.game.level.as_mut() {
            level.connect(slot, &client_name(userinfo), self.game.spot);
            level.set_guid(slot, &guid);
        }
        self.core.clients[slot] = Some(Client {
            link,
            state: State::Connected,
            gamestate_message: -1,
            delta_message: -1,
            last_cmd_time: 0,
            command_time: svs,
            stats: Stats(Box::new([0; vcod_cod4::net::stats::STATS_BYTES])),
            stat_packets: 0,
            guid,
            pure: None,
            rates: link::Rates::of(userinfo, false, 1),
            // `Sys_IsLANAddress` with the subnet table (0x81d59c8), the
            // pacing's and the ping limits' test; only the challenge takes
            // the subnet-blind one (cod4-handshake.md 15).
            lan: link::Lan::Cod2.is_lan(from.ip()),
            bot: false,
            last_connect_ms: svs,
            reliable_kept: [true; MAX_RELIABLE_COMMANDS],
            download: download::Download::default(),
            voice: voice::ClientVoice::of(userinfo),
        });
    }

    pub fn client_count(&self) -> usize {
        self.core
            .clients
            .iter()
            .flatten()
            .filter(|c| c.state >= State::Connected)
            .count()
    }

    /// A client's stats block as the server holds it.
    pub fn stats(&self, slot: usize) -> Option<&Stats> {
        self.core.clients.get(slot)?.as_ref().map(|c| &c.stats)
    }

    /// The GUID a client gave with its `getchallenge`.
    pub fn guid(&self, slot: usize) -> Option<&str> {
        self.core
            .clients
            .get(slot)?
            .as_ref()
            .map(|c| c.guid.as_str())
    }
}

/// A config file out of the iwds with each `exec <name>` line replaced by
/// that file's text, as `Cbuf` runs it; a file no iwd holds reads as
/// empty. `server_map.cfg` (in `iw_00.iwd`) sets `sv_mapRotation`. `iw_00.iwd` holds two
/// `default_mp_gamesettings.cfg`, and retail reads the later one: no
/// `scr_game_playerwaittime` line, so the prematch waits the script's 15 s
/// (VERIFIED live, rcon and the game end time 619800).
/// `Dvar_InfoString(0x404)` over `cvars`: configstring 0, `getstatus`'s
/// info and the log's `InitGame` line, case-folded order.
fn serverinfo_of(cvars: &crate::cvars::Cvars) -> String {
    info_sorted(
        oob::SERVERINFO
            .iter()
            .map(|n| (n.to_string(), cvars.get(n).to_string())),
    )
}

fn iwd_cfg(fs: &vcod_common::pk3::Pk3Fs, name: &str, depth: u32) -> String {
    let Some(raw) = fs.read(name) else {
        return String::new();
    };
    let mut out = String::new();
    for line in String::from_utf8_lossy(&raw).lines() {
        let t = crate::game::say::tokenize(line);
        match t.as_slice() {
            [exec, file, ..] if exec.eq_ignore_ascii_case("exec") && depth < 8 => {
                out.push_str(&iwd_cfg(fs, file, depth + 1));
            }
            _ => {
                out.push_str(line);
                out.push('\n');
            }
        }
    }
    out
}

/// `sv_serverid` steps 16 a spawn, past 0 in the high nibble;
/// the first spawn's 16 is VERIFIED live.
fn spawn_server_id(id: i32) -> i32 {
    let mut n = (id + 16) & 0xff;
    if n & 0xf0 == 0 {
        n += 16;
    }
    n
}

/// The unacknowledged server commands, each `svc_serverCommand`.
fn write_pending_commands(w: &mut MsgWriter, c: &mut Client) {
    c.link.reliable_sent = c.link.netchan.reliable_sequence as i32;
    for seq in c.link.reliable_ack + 1..=c.link.netchan.reliable_sequence as i32 {
        let text = &c.link.netchan.reliable[seq as usize & (MAX_RELIABLE_COMMANDS - 1)];
        w.write_byte(SVC_SERVERCOMMAND);
        w.write_long(seq);
        w.write_string_bytes(text.as_bytes());
    }
}

/// Whether a command goes `SV_CMD_CAN_IGNORE`, read off its letter: the
/// menu notify `J` (the walk capture's four `J "1"` in a row, never
/// squashed, VERIFIED live), the team and player scores `G`, `H`, `I`, the
/// game messages `c`, `e`, `f`, `g` and the `iPrintLn` whose letter a
/// player argument cost it (the capture's ` "MP_CONNECTED\x15walker^7"`),
/// the chat lines `h`, `i`. Everything else is `SV_CMD_RELIABLE`.
fn ignorable(text: &str) -> bool {
    matches!(
        text.as_bytes().first(),
        Some(b'J' | b'G' | b'H' | b'I' | b'c' | b'e' | b'f' | b'g' | b'h' | b'i' | b' ')
    )
}

/// `SV_AddServerCommand` (0x81769b2) on one client's ring:
/// - A client not `CS_ACTIVE`, or more than 63 commands past its ack
///   (0x81769df), first loses its unsent ignorable commands, and an
///   ignorable command is then not queued at all.
/// - A reliable command takes the place of the first unsent reliable one
///   it [`supersedes`]: that one leaves the ring and the new one goes last
///   (the walk capture's `u L u L` read `u L`, and its `C 40` from the
///   frag's give follows the next `J`, VERIFIED live).
///
/// False when the ring would hold more than 128 unacked commands
/// (retail's `== 0x81` test); the caller drops the client.
fn add_server_command(c: &mut Client, text: &str, reliable: bool) -> bool {
    let pending = c.link.netchan.reliable_sequence as i32 - c.link.reliable_ack;
    if c.state != State::Active || pending > 63 {
        edit_unsent(c, |cmds| cmds.retain(|(_, kept)| *kept));
        if !reliable {
            return true;
        }
    }
    let stale = unsent(c)
        .map(ring_slot)
        .position(|s| c.reliable_kept[s] && supersedes(text, &c.link.netchan.reliable[s]));
    if let Some(i) = stale {
        edit_unsent(c, |cmds| drop(cmds.remove(i)));
    }
    let seq = c.link.netchan.reliable_sequence as i32 + 1;
    if seq - c.link.reliable_ack > MAX_RELIABLE_COMMANDS as i32 {
        return false;
    }
    c.link.netchan.reliable_sequence = seq as u32;
    let slot = ring_slot(seq);
    c.link.netchan.reliable[slot] = text.to_string();
    c.reliable_kept[slot] = reliable;
    true
}

/// Whether reliable `new` replaces the unsent reliable `old`
/// (0x8176a76..0x8176bff): one letter, not a configstring chunk `x`..`z`
/// (0x8176abe), and then the same text after it (0x8176adb), or a letter
/// any later one replaces (`C D a b o p q r t`, the jump table at
/// 0x82326a8), or a `d`/`v` naming the same first word (0x8176b85).
fn supersedes(new: &str, old: &str) -> bool {
    let (new, old) = (new.as_bytes(), old.as_bytes());
    let Some((&letter, rest)) = new.split_first() else {
        return false;
    };
    if old.first() != Some(&letter) || (b'x'..=b'z').contains(&letter) {
        return false;
    }
    fn first_word(t: &[u8]) -> Option<&[u8]> {
        t.get(2..).and_then(|r| r.split(|&b| b == b' ').next())
    }
    rest == &old[1..]
        || match letter {
            b'C' | b'D' | b'a' | b'b' | b'o' | b'p' | b'q' | b'r' | b't' => true,
            b'd' | b'v' => first_word(new) == first_word(old),
            _ => false,
        }
}

/// A sequence number's slot in the 128-command ring.
fn ring_slot(seq: i32) -> usize {
    seq as usize & (MAX_RELIABLE_COMMANDS - 1)
}

/// The sequence numbers no message has carried yet:
/// `reliableSent + 1 ..= reliableSequence`.
fn unsent(c: &Client) -> std::ops::RangeInclusive<i32> {
    c.link.reliable_sent + 1..=c.link.netchan.reliable_sequence as i32
}

/// Takes the unsent commands (text, reliable) out of the ring, lets `edit`
/// drop some, and packs the rest back from `reliableSent + 1`, the
/// sequence ending on the last of them.
fn edit_unsent(c: &mut Client, edit: impl FnOnce(&mut Vec<(String, bool)>)) {
    let mut cmds: Vec<(String, bool)> = unsent(c)
        .map(ring_slot)
        .map(|s| {
            (
                std::mem::take(&mut c.link.netchan.reliable[s]),
                c.reliable_kept[s],
            )
        })
        .collect();
    edit(&mut cmds);
    let mut seq = c.link.reliable_sent;
    for (text, kept) in cmds {
        seq += 1;
        c.link.netchan.reliable[ring_slot(seq)] = text;
        c.reliable_kept[ring_slot(seq)] = kept;
    }
    c.link.netchan.reliable_sequence = seq as u32;
}

/// A userinfo's `name`.
fn client_name(userinfo: &str) -> String {
    vcod_common::net::info_value_for_key(userinfo, "name")
        .unwrap_or("")
        .to_string()
}

/// A spectator floating at the intermission spot: the fields the retail
/// spectator's first snapshot carries (`play-1.7-spec.txt`, VERIFIED live:
/// `pm_type` 4, `eFlags` 2, gravity 800, `moveSpeedScaleMultiplier` 1, the
/// depth of field's 6 and 1.8, `throwBackGrenadeOwner` and
/// `viewlocked_entNum` 1023, `stats[3]` 63, `stats[4]` 1).
fn spectator_ps(slot: usize, origin: [f32; 3], angles: [f32; 3], command_time: i32) -> PlayerState {
    let mut ps = PlayerState::default();
    let mut set = |name: &str, v: u32| {
        let f = PLAYER_STATE
            .iter()
            .find(|f| f.name == name)
            .unwrap_or_else(|| panic!("no playerstate field {name}"));
        ps.words[f.word()] = v;
    };
    set("commandTime", command_time as u32);
    for i in 0..3 {
        set(&format!("origin[{i}]"), origin[i].to_bits());
        // The wire's half-short rounding reads the capture's yaw 65 as
        // 65.00061 (`angle16_sent`, VERIFIED live).
        let a = angles[i];
        set(&format!("viewangles[{i}]"), a.to_bits());
        set(&format!("delta_angles[{i}]"), a.to_bits());
    }
    set("eFlags", 2);
    set("moveSpeedScaleMultiplier", 1.0f32.to_bits());
    set("throwBackGrenadeOwner", ENTITYNUM_NONE);
    set("viewlocked_entNum", ENTITYNUM_NONE);
    set("gravity", 800);
    set("dofNearBlur", 6.0f32.to_bits());
    set("dofFarBlur", 1.8f32.to_bits());
    set("clientNum", slot as u32);
    set("pm_type", PM_SPECTATOR);
    ps.stats[3] = 63;
    ps.stats[4] = 1;
    ps
}

/// `mp_global_intermission`'s origin and angles.
fn intermission_spot(ents: &[Vec<(String, String)>]) -> Option<([f32; 3], [f32; 3])> {
    let e = ents.iter().find(|e| {
        e.iter()
            .any(|(k, v)| k == "classname" && v == "mp_global_intermission")
    })?;
    let vec = |k: &str| {
        e.iter()
            .find(|(a, _)| a == k)
            .and_then(|(_, v)| vcod_common::bsp::parse_vec3(v))
    };
    Some((vec("origin")?, vec("angles").unwrap_or([0.0; 3])))
}

/// `sv_referencedFFNames` and their byte sizes: `code_post_gfx_mp`, the
/// map, `common_mp` on every 1.7 capture (VERIFIED, cod4-handshake.md 8),
/// then a mod's `<fs_game>/mod`; a usermap as `usermaps/<map>` and, when
/// it has one, `usermaps/<map>_load`, both sizes under its slot. The mod's
/// and the usermap's shapes are `DB_ReferencedFastFiles`', INFERRED
/// (cod4-fastfile.md 13).
fn referenced_fastfiles(install: &Install, lang: &str, map: &str) -> Vec<(String, String)> {
    use vcod_cod4::install::ZoneOrigin;
    let size = |p: &Path| std::fs::metadata(p).map_or(0, |m| m.len());
    let mut out = Vec::new();
    for n in ["code_post_gfx_mp", map, "common_mp"] {
        let Some(f) = install.zone_file(lang, n) else {
            out.push((n.to_string(), "0".to_string()));
            continue;
        };
        if f.origin != ZoneOrigin::Usermaps {
            out.push((n.to_string(), size(&f.path).to_string()));
            continue;
        }
        let load = install
            .zone_file(lang, &format!("{n}_load"))
            .filter(|l| l.origin == ZoneOrigin::Usermaps);
        match load {
            Some(l) => {
                let sums = format!("{} {}", size(&f.path), size(&l.path));
                out.push((format!("usermaps/{n} usermaps/{n}_load"), sums));
            }
            None => out.push((format!("usermaps/{n}"), size(&f.path).to_string())),
        }
    }
    if let (Some(g), Some(p)) = (install.fs_game(), install.mod_zone()) {
        out.push((format!("{g}/mod"), size(&p).to_string()));
    }
    out
}

/// The non-localized iwds of `iwds` a file was read from, lowest priority
/// first (`main/iw_11 mods/modwarfare/z_modwarfare` on retail, VERIFIED
/// live), each as `<game>/<name>` and its checksum; the `fs_game`
/// directory's named under `fs_game`, a usermap's (`usermap`) under
/// `usermaps/<map>` (`FS_AddGameDirectory`'s name for it,
/// INFERRED).
fn referenced_iwds(
    iwds: &vcod_common::pk3::Pk3Fs,
    fs_game: &str,
    usermap: Option<&Path>,
    map: &str,
) -> Vec<(String, i32)> {
    let mut out: Vec<(String, i32)> = iwds
        .paks_read()
        .filter(|(p, read)| *read && !p.localized)
        .map(|(p, _)| {
            let game = if p.game.eq_ignore_ascii_case("main") {
                "main".to_string()
            } else if usermap.is_some_and(|u| p.path.parent() == Some(u)) {
                format!("usermaps/{map}")
            } else {
                fs_game.to_string()
            };
            let sum = vcod_cod4::net::pure::iwd_checksum(&p.crcs, 0);
            (format!("{game}/{}", p.name.to_ascii_lowercase()), sum)
        })
        .collect();
    out.reverse();
    out
}

/// `FS_LoadedIwdChecksums` and `FS_LoadedIwdNames` over the non-localized
/// iwds of [`Install::iwd_dirs`] (lowest priority first), search order
/// first: the mod's, a usermap's, then `main`'s last name first (`iw_13`
/// leads, VERIFIED live), and each sum keyed with the gamestate's feed
/// for the `cp` check.
fn pure_lists(dirs: &[PathBuf], feed: i32) -> anyhow::Result<Iwds> {
    let mut out = Iwds::default();
    for dir in dirs.iter().rev() {
        pure_lists_in(dir, feed, &mut out)?;
    }
    Ok(out)
}

fn pure_lists_in(main: &Path, feed: i32, out: &mut Iwds) -> anyhow::Result<()> {
    use anyhow::Context;
    let mut names: Vec<String> = std::fs::read_dir(main)
        .with_context(|| main.display().to_string())?
        .filter_map(|e| e.ok()?.file_name().into_string().ok())
        .filter(|n| {
            let l = n.to_ascii_lowercase();
            l.ends_with(".iwd") && !l.starts_with("localized_")
        })
        .collect();
    names.sort_by_key(|n| std::cmp::Reverse(n.to_ascii_lowercase()));
    for n in names {
        let path = main.join(&n);
        let f = std::fs::File::open(&path).with_context(|| path.display().to_string())?;
        let crcs = vcod_common::pak_checksum::entry_crcs(std::io::BufReader::new(f))
            .with_context(|| path.display().to_string())?;
        out.sums.push(vcod_cod4::net::pure::iwd_checksum(&crcs, 0));
        out.keyed
            .push(vcod_cod4::net::pure::iwd_checksum(&crcs, feed));
        out.names.push(n.trim_end_matches(".iwd").to_string());
    }
    Ok(())
}

/// An info string in `Dvar`'s case-folded order; empty values left out.
fn info_sorted(pairs: impl Iterator<Item = (String, String)>) -> String {
    let mut pairs: Vec<(String, String)> = pairs.collect();
    pairs.sort_by_key(|(k, _)| k.to_ascii_lowercase());
    let mut info = vcod_common::net::connectionless::Info::new();
    for (k, v) in &pairs {
        info.set(k, v);
    }
    info.to_string()
}

#[cfg(test)]
mod blast_tests;
#[cfg(test)]
mod combat_ab;
#[cfg(test)]
mod console_tests;
#[cfg(test)]
mod damage_soak;
#[cfg(test)]
mod destructible_tests;
#[cfg(test)]
mod disconnect_tests;
#[cfg(test)]
mod download_tests;
#[cfg(test)]
mod gamelog_tests;
#[cfg(test)]
mod gametype_tests;
#[cfg(test)]
mod hardpoint_ab;
#[cfg(test)]
mod hardpoint_tests;
#[cfg(test)]
mod join_tests;
#[cfg(test)]
mod mod_tests;
#[cfg(test)]
mod oob_ab;
#[cfg(test)]
mod pen_pin_tests;
#[cfg(test)]
mod perk_tests;
#[cfg(test)]
mod planefx_ab;
#[cfg(test)]
mod prematch_ab;
#[cfg(test)]
mod shock_tests;
#[cfg(test)]
mod shot_aim_ab;
#[cfg(test)]
mod stat_tests;
#[cfg(test)]
mod vote_tests;
#[cfg(test)]
mod wire_ab;
