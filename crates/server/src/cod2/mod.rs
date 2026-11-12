//! The engine side of a CoD2 1.3 server (protocol 118): the
//! connectionless exchange, the netchan, the gamestate and the delta
//! snapshots, written from docs/research/cod2-protocol.md and checked
//! against the retail session it records (`vcod_cod2::net::write`'s
//! tests).
//!
//! The server is `Engine<Cod2Game>`: `engine::tick` and `engine::packet`
//! run [`Cod2Game`]'s hooks. The game behind it is [`Cod2Level`], driven in
//! `SV_Frame`'s order (cod2-protocol.md 14.1): `ClientConnect` on `connect`, `ClientBegin` on
//! the first `clc_move`, `ClientThink` per cmd as its packet arrives, the
//! client commands (`ClientCommand`, the votes), then each frame `G_RunFrame` and the
//! snapshots: every client's playerstate and HUD off the level, the
//! object table's entities and the playing clients' bodies culled by the
//! map's PVS, the roster, and the server commands and configstrings the
//! script queued. Without game data (`load(None)`) the engine flies bare
//! spectators at the origin.
//!
//! The map cycle (`map`, `map_restart`, `fast_restart`, `map_rotate`) is
//! `cycle.rs` (docs/research/cod2-map-cycle.md).
//!
//! Housekeeping (timeouts and zombies, private slots, `g_password`, ping
//! limits, downloads, the heartbeat) is `house.rs` (cod2-protocol.md 17);
//! the rate, pacing, ping and LAN test are `crate::engine::link`, shared
//! with CoD1's server.
//!
//! Not here: voice, PunkBuster, the authorize server, the status the
//! master gets every 10 minutes. The heartbeat goes to cod.pm's CoD2
//! list, only at `dedicated 2`.
//! Shots, damage, deaths and missiles are the level's
//! (`game::cod2_combat`); this side sends their corpses, temp entities
//! and missiles.

mod bots;
mod cycle;
mod house;
mod oob;

pub use cycle::{Rotation, rotate};

use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::rc::Rc;
use std::time::Instant;
use vcod_common::asset_source::RawFile;

use glam::Vec3;
use vcod_cod2::net::PROTOCOL_118;
use vcod_cod2::net::delta::{ClientState2, EntityState2, PlayerState2, field_index};
use vcod_cod2::net::fields::{CLIENT_STATE_FIELDS, PLAYER_STATE_FIELDS};
use vcod_cod2::net::ops::{CLC_BITS, Clc, ENTITYNUM_NONE, MAX_RELIABLE_COMMANDS, Svc};
use vcod_cod2::net::usercmd::{MAX_PACKET_USERCMDS, default_usercmd, read_delta_usercmd};
use vcod_cod2::net::write::{Frame2, latin1_bytes, write_server_command};
use vcod_cod2::net::{Gamestate2, latin1};
use vcod_cod2::pmove::{PlayerState, pm_type};
use vcod_common::mapworld::MapWorld;
use vcod_common::net::codec::{MsgReader, MsgWriter};
use vcod_common::net::com_hash_key_bytes;
use vcod_common::net::msg::EntityState;
use vcod_common::net::netchan::{NetchanParams, ServerNetchan};
use vcod_common::net::protocol::{SnapshotHead, WireGame};
use vcod_common::pk3::Pk3Fs;

use crate::engine::{Lan, link};
use crate::game::cod2::Cod2Level;
use crate::game::cod2_wire::to_cod2_entity;

/// `sv_fps 20`.
pub const FRAME_MS: i32 = 50;
/// `PACKET_BACKUP`: the frames a client can delta from.
const PACKET_BACKUP: usize = crate::engine::slot::PACKET_BACKUP;
/// A frame older than this many messages is not deltaed from (3.3).
const MAX_DELTA_AGE: i32 = PACKET_BACKUP as i32 - 3;
/// `TEAM_SPECTATOR` in a client state's `team`.
const TEAM_SPECTATOR: u32 = 3;
/// The playerstate's `pm_flags` and `eFlags` a retail spectator carries
/// (cod2-protocol.md 9).
const SPECTATOR_PM_FLAGS: u32 = 0x100_0000;
const SPECTATOR_EFLAGS: u32 = 0x2;
/// `SV_SpawnServer`'s frames before `SV_CreateBaseline`: three of 100 ms
/// (0x8093520, VERIFIED).
const SPAWN_FRAMES: usize = 3;
const SPAWN_FRAME_MS: i32 = 100;

/// What `vcod-server <map> --game cod2` starts with.
#[derive(Clone, Debug)]
pub struct Cod2Config {
    pub map: String,
    pub gametype: String,
    pub hostname: String,
    pub max_clients: usize,
    /// `--gametype-script`: the canonical path it overlays
    /// (`maps/mp/gametypes/<stem>`) and its text, run whenever that
    /// gametype loads.
    pub gametype_script: Option<(String, String)>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum State {
    /// `CS_ZOMBIE`: dropped, still sent its last commands until
    /// `sv_zombietime` passes without a packet (17.3).
    Zombie,
    /// `connectResponse` sent; bare messages until the client asks.
    Connected,
    /// Gamestate sent, not yet in the world.
    Primed,
    Active,
}

pub struct Client {
    /// The connection half CoD1's client keeps too (`engine::slot`); its
    /// frames are the message number each went in and the frame.
    link: crate::engine::slot::Link<(i32, Frame2)>,
    state: State,
    /// The message the gamestate went in; an older acknowledge is ignored.
    gamestate_message: i32,
    /// The frame the client's last `clc_move` acknowledged, -1 for none.
    delta_message: i32,
    /// `lastUsercmd.serverTime`: a cmd at or before it is not thought again.
    last_cmd_time: i32,
    /// A bare server's spectator; with a level the playerstate is the
    /// level's (`Cod2Level::players`).
    ps: PlayerState,
    /// `SV_VerifyIwds_f`'s verdict: `None` until a `cp`.
    pure: Option<bool>,
    last_heard: Instant,
    /// A bot (`bots.rs`): no address, no packets, its cmds the brain's.
    bot: bool,
    /// `SV_UserinfoChanged`'s rate, snapshot interval and www flag.
    rates: link::Rates,
    /// Whether the address is on the LAN (`Sys_IsLANAddress`).
    lan: bool,
    /// `lastConnectTime`, on `svs.time`.
    last_connect_ms: i32,
    download: house::Download,
    /// An `addTestClient` client: the bots' pass thinks for it on a
    /// random cmd, where a vcod bot (`bot` alone) has its brain.
    test: bool,
}

/// The pure lists of `SV_SpawnServer` (cod2-protocol.md 11.3).
#[derive(Default)]
struct Iwds {
    /// `sv_iwds`, `sv_iwdNames`: every non-localized iwd, search order.
    sums: Vec<i32>,
    names: Vec<String>,
    /// The keyed checksums `SV_VerifyIwds_f` accepts.
    keyed: Vec<i32>,
    /// `sv_referencedIwds`, `sv_referencedIwdNames`.
    referenced: Vec<(i32, String)>,
}

/// The CoD2 game module: what `Cod2Server` keeps beside its
/// [`EngineCore`](crate::engine::EngineCore).
pub struct Cod2Game {
    cfg: Cod2Config,
    fs: Option<Rc<Pk3Fs>>,
    /// Engine cvars; the script keeps its own table.
    cvars: crate::cvars::Cvars,
    /// The loaded level, `None` on a bare server.
    level: Option<Cod2Level>,
    /// Where a connecting client floats until the script places it:
    /// `mp_global_intermission`'s spot and view.
    spot: (Vec3, [f32; 3]),
    baselines: BTreeMap<u32, EntityState2>,
    server_id: i32,
    /// `SNAPFLAG_SERVERCOUNT` (4), flipped by every spawn and restart
    /// (`DAT_08423088 ^= 4`, cod2-map-cycle.md 1).
    snap_flags: u8,
    iwds: Iwds,
    start: Instant,
    /// [`Self::test_frame_clock`].
    frame_clock: bool,
    /// The bots' brains and graph, once `--bots` asked for any.
    bots: Option<crate::bots::cod2::Cod2Bots>,
    /// `SV_AddTestClient`'s static `botport`, which no level resets: a
    /// test client added after a map change is `bot1` (VERIFIED live, tdm
    /// past its score limit).
    bot_port: u32,
    /// The socket and the wait `SV_SpawnServer` uses mid-command; without
    /// it (a test) its OOB lines wait in the core's `outbox`.
    net_now: Option<NetNow>,
    /// `games_mp.log` while a level runs with `g_log` set.
    game_log: crate::engine::gamelog::LevelLog,
    /// `SV_Cmd_Argv(1)` of the last line tokenized (a connectionless
    /// packet, a client command, a console line, `SVC_Status`'s own iwd
    /// list): the `challenge` the master's status carries
    /// (cod2-protocol.md 17.6).
    argv1: String,
}

/// The CoD2 server: the engine over [`Cod2Game`].
pub type Cod2Server = crate::engine::Engine<Cod2Game>;

/// CoD2's `SV_Frame` (cod2_lnxded 0x80969b0, cod2-protocol.md 14.1): the
/// serverinfo and systeminfo dvars' changes as configstrings 0 and 1 and the
/// mirror's block (a `fast_restart`'s `d 1` after its `B`, before the
/// connect callbacks' commands, cod2-map-cycle.md 2), the bots (0x8096952),
/// The roster and the world every CoD2 snapshot of a turn is built from,
/// once the first client needs them.
type Cod2Snapshots = Option<(Vec<ClientState2>, BTreeMap<u32, (EntityState, bool)>)>;

/// `SV_CalcPings` (0x80963c8), the frames owed, each `G_RunFrame`
/// (0x810a13a) through its `ClientEndFrame`s to `CheckVote`, the archive
/// between frames (0x809b016); then `SV_CheckTimeouts` (0x809657e, which
/// frees the zombies), `SV_SendClientMessages` (0x809bcce), the last frame's
/// archive (Mac 0x15bf3c) and `SV_MasterHeartbeat` (0x8096ed6).
impl crate::engine::GameModule for Cod2Game {
    type Client = Client;
    type Wire = vcod_cod2::net::Cod2Wire;
    type Snapshots = Cod2Snapshots;

    const THINK: crate::engine::ThinkTiming = crate::engine::ThinkTiming::OnArrival;
    const HEAD: crate::engine::HeadOrder = crate::engine::HeadOrder::CsBotsPings;
    const MIN_FRAMES: u32 = 0;
    const ARCHIVE_AFTER_MESSAGES: bool = true;

    fn wire(_e: &Cod2Server) -> &'static vcod_cod2::net::Cod2Wire {
        &PROTOCOL_118
    }
    fn status(
        e: &mut Cod2Server,
        query: crate::engine::StatusQuery,
        challenge: &str,
    ) -> Option<String> {
        match query {
            crate::engine::StatusQuery::Info => e.svc_info(challenge),
            crate::engine::StatusQuery::Status => e.svc_status(challenge),
        }
    }
    fn connectionless(
        e: &mut Cod2Server,
        from: SocketAddr,
        pkt: &[u8],
        at: crate::engine::Arrival,
    ) {
        e.connectionless(from, &pkt[4..], at.now);
    }

    fn client_packet(e: &mut Cod2Server, from: SocketAddr, pkt: &[u8], at: crate::engine::Arrival) {
        e.client_packet(from, pkt, at.now);
    }

    fn drain_console(e: &mut Cod2Server, now: Instant) {
        e.drain_console(now);
    }

    fn configstrings_head(e: &mut Cod2Server) {
        // `SV_SetConfig(0x8e, 0x60, 0x100)`: the dvar mirror's block.
        e.copy_script_configstrings();
        e.broadcast_configstrings();
    }

    fn bots(e: &mut Cod2Server) {
        // On the clock before the step, as retail's test clients and CoD1's
        // bots.
        crate::tick_trace::step!("SV_BotFrame time {}", e.core.sv_time_ms);
        e.step_bots();
    }

    fn calc_pings(e: &mut Cod2Server) {
        e.calc_pings();
    }

    fn frame_step_ms(_e: &mut Cod2Server) -> i32 {
        FRAME_MS
    }

    fn run_frame(e: &mut Cod2Server) -> bool {
        e.run_level_frame();
        true
    }

    fn archive(e: &mut Cod2Server) {
        crate::tick_trace::step!("SV_ArchiveSnapshot");
        e.archive_frame();
    }

    fn frame_out(e: &mut Cod2Server) {
        crate::tick_trace::step!("flush_game_commands");
        e.flush_game_commands();
    }

    fn check_timeouts(e: &mut Cod2Server) {
        e.check_timeouts();
    }

    fn snapshots_begin(_e: &mut Cod2Server) -> Cod2Snapshots {
        None
    }
    fn message_due(e: &Cod2Server, slot: usize) -> bool {
        e.message_due(slot)
    }
    fn next_fragment(e: &mut Cod2Server, slot: usize) -> Option<(SocketAddr, Vec<u8>)> {
        e.next_fragment(slot)
    }
    fn send_client_snapshot(e: &mut Cod2Server, slot: usize, snaps: &mut Cod2Snapshots) {
        e.send_client_snapshot(slot, snaps);
    }

    fn heartbeat(e: &mut Cod2Server) {
        e.drain_game_log();
        // `SV_Frame`'s last step (cod2-protocol.md 1.6, 14.1).
        e.heartbeat(false);
    }
}

pub type NetSend = Box<dyn FnMut(SocketAddr, &[u8])>;

/// What a command that blocks the frame loop sends through and waits on:
/// `NET_OutOfBandPrint` at once, and `NET_Sleep(ms)` (cod2_lnxded
/// 0x80d5cc4: a `select` on the socket, so a packet ends it early).
pub struct NetNow {
    pub send: NetSend,
    pub sleep: Box<dyn FnMut(std::time::Duration)>,
}

impl Cod2Server {
    /// A server with no level: [`Self::load`] brings one up.
    pub fn new(mut cfg: Cod2Config, seed: u64, now: Instant) -> Self {
        // `sv_maxclients` tops out at 64, the level's client slots.
        cfg.max_clients = cfg.max_clients.clamp(1, crate::server::MAX_CLIENTS);
        let mut cvars = crate::cvars::Cvars::new();
        // Set as well as registered: 1.1's table already holds `gamename`,
        // `protocol` and `shortversion` at its own values.
        for (name, value) in oob::ENGINE_CVARS {
            cvars.register(name, value);
            cvars.set(name, value);
        }
        cvars.remirror(&PROTOCOL_118.cs, oob::MIRRORED);
        cvars.set("sv_hostname", &cfg.hostname);
        cvars.set("sv_maxclients", &cfg.max_clients.to_string());
        cvars.set("g_gametype", &cfg.gametype);
        cvars.set("mapname", &cfg.map);
        let mut core =
            crate::engine::EngineCore::new(&PROTOCOL_118.transport, cfg.max_clients, seed | 1, {
                // The one master `SV_MasterHeartbeat` (0x8096ed6) beats, with
                // no `sv_master` cvars.
                crate::master::Masters::new(vcod_common::net::master::COD2_MASTER)
            });
        // The script's configstrings, with 0 and 1 the engine's.
        core.configstrings = vec![String::new(); vcod_cod2::net::ops::MAX_CONFIGSTRINGS];
        let mut s = crate::engine::Engine {
            core,
            game: Cod2Game {
                cfg,
                fs: None,
                cvars,
                level: None,
                spot: (Vec3::ZERO, [0.0; 3]),
                baselines: BTreeMap::new(),
                server_id: 0,
                snap_flags: 0,
                iwds: Iwds::default(),
                start: now,
                frame_clock: false,
                bot_port: 0,
                bots: None,
                net_now: None,
                argv1: String::new(),
                game_log: Default::default(),
            },
        };
        // The command line's `map <name>` is the first line tokenized.
        s.game.argv1 = s.game.cfg.map.clone();
        // `sv_serverid` starts at 0 and the first spawn makes it 16
        // (cod2-map-cycle.md 1).
        s.core.checksum_feed = s.rand();
        s.write_engine_configstrings();
        s.core.sent_configstrings = s.core.configstrings.clone();
        s
    }

    /// `+set name value` before the level loads, or an rcon `set`.
    pub fn set_cvar(&mut self, name: &str, value: &str) {
        self.game.cvars.set(name, value);
        // Retail has one dvar table; the level reads its own copy.
        if let Some(level) = self.game.level.as_mut() {
            level.rt.host.cvars.set(name, value);
        }
        if name.eq_ignore_ascii_case("sv_hostname") {
            self.game.cfg.hostname = value.to_string();
        }
        self.write_engine_configstrings();
    }

    /// Keeps `ban.txt` at `path` (`--ban-file`); without it the bans last
    /// for the run.
    pub fn set_ban_file(&mut self, path: std::path::PathBuf) {
        self.core.bans = crate::engine::bans::BanFile::open(path);
    }

    pub fn cvar(&self, name: &str) -> &str {
        self.game.cvars.get(name)
    }

    /// `SV_SpawnServer`: the map, its collision, the script bootstrap and
    /// three 100 ms frames, the baselines, then the pure lists over what
    /// the load read. `fs` `None` runs a bare server: no world, no script,
    /// the spectators float at the origin.
    pub fn load(&mut self, fs: Option<Rc<Pk3Fs>>) -> anyhow::Result<()> {
        self.load_with(fs, crate::game::cod2::Cod2Init::default())
    }

    /// [`Self::load`] with what the last level hands on.
    fn load_with(
        &mut self,
        fs: Option<Rc<Pk3Fs>>,
        init: crate::game::cod2::Cod2Init,
    ) -> anyhow::Result<()> {
        self.game.fs = fs.clone();
        // `SV_SetGametype`: the load runs the `g_gametype` set now
        // (`--set g_gametype=`, a rotation's).
        let gametype = self.game.cvars.get("g_gametype");
        if !gametype.is_empty() {
            self.game.cfg.gametype = gametype.to_string();
        }
        self.game.server_id = cycle::spawn_server_id(self.game.server_id);
        self.game.snap_flags ^= 4;
        let Some(fs) = fs else {
            self.write_engine_configstrings();
            self.core.sent_configstrings = self.core.configstrings.clone();
            return Ok(());
        };
        // `default_mp.cfg`, which the engine execs at startup, under the
        // `+set`s: the stock `scr_*` limits (cod2-protocol.md 15.4).
        if let Ok(raw) = fs.load::<RawFile>("default_mp.cfg") {
            self.game.cvars.exec_cfg_under_sets(&raw.text());
            // Its `g_gametype random` and kin lose to the command line.
            self.game.cvars.set("g_gametype", &self.game.cfg.gametype);
            self.game.cvars.set("mapname", &self.game.cfg.map);
        }
        fs.clear_references();
        fs.set_surfs_unflagged(true);
        // `SV_SpawnServer` loads the sound aliases before the game
        // (`Com_LoadSoundAliases` 0x80ad0aa): every `soundaliases/*.csv`
        // is read, which flags its iwd (cod2-protocol.md 11.3).
        for name in fs.list_prefix("soundaliases/") {
            if name.ends_with(".csv") {
                fs.touch(&name);
            }
        }
        let t0 = std::time::Instant::now();
        self.load_level(Some(fs.clone()), init)?;
        let level = self.game.level.as_mut().expect("a loaded level");
        // The load's writes ride the gamestate.
        level.rt.host.configstrings.take_writes();
        // `SV_CreateBaseline`: every linked entity as it stands now.
        self.game.baselines = level
            .rt
            .baseline_entities(PROTOCOL_118.as_cod1_shim())
            .values()
            .filter_map(to_cod2_entity)
            .map(|e| (e.number, e))
            .collect();
        self.game.iwds = pure_lists(&fs, self.core.checksum_feed);
        self.copy_script_configstrings();
        self.write_engine_configstrings();
        self.core.sent_configstrings = self.core.configstrings.clone();
        log::info!(
            "cod2: {} {} up in {:.0} ms, {} configstrings, {} baselines, {} iwds referenced",
            self.game.cfg.map,
            self.game.cfg.gametype,
            t0.elapsed().as_secs_f64() * 1000.0,
            self.core
                .configstrings
                .iter()
                .filter(|s| !s.is_empty())
                .count(),
            self.game.baselines.len(),
            self.game.iwds.referenced.len()
        );
        Ok(())
    }

    fn copy_script_configstrings(&mut self) {
        if let Some(level) = &self.game.level {
            for (i, s) in level.rt.configstrings().iter().enumerate().skip(2) {
                if let Some(slot) = self.core.configstrings.get_mut(i) {
                    slot.clone_from(s);
                }
            }
            // The dvar mirror over the script's cvars (cod2-protocol.md 15.4).
            if let Err(e) = level.rt.cvars().write_mirror(&mut self.core.configstrings) {
                log::warn!("cod2: {e:?}");
            }
        }
    }

    /// Configstrings 0 and 1: `Dvar_InfoString(0x404)` and
    /// `Dvar_InfoString_Big(8)` (cod2-protocol.md 6, 9, 11.3).
    /// The level's `G_LogPrintf` lines since the last call into the file.
    fn drain_game_log(&mut self) {
        if let Some(level) = self.game.level.as_mut() {
            let lines = std::mem::take(&mut level.rt.host.game_log);
            self.game.game_log.write(lines);
        }
    }

    /// `G_ShutdownGame`: the level's last lines, `ShutdownGame:` and the
    /// separator at its last `level.time`, the file closed.
    fn shutdown_game_log(&mut self) {
        if let Some(level) = self.game.level.as_mut() {
            let lines = std::mem::take(&mut level.rt.host.game_log);
            let at = level.rt.host.level_time_ms;
            self.game
                .game_log
                .shutdown(lines, at, crate::engine::gamelog::SHUTDOWN);
        }
    }

    fn write_engine_configstrings(&mut self) {
        self.core.configstrings[0] = self.serverinfo();
        self.core.configstrings[1] = self.systeminfo();
    }

    fn pure(&self) -> bool {
        self.game.cvars.get("sv_pure") != "0"
    }

    /// The `getstatus` and configstring 0 serverinfo, sorted as `Dvar`'s
    /// list is (case folded).
    fn serverinfo(&self) -> String {
        let mut pairs: Vec<(String, String)> = oob::SERVERINFO
            .iter()
            .map(|n| (n.to_string(), self.game.cvars.get(n).to_string()))
            .collect();
        pairs.push(("mapname".into(), self.game.cfg.map.clone()));
        pairs.push(("g_gametype".into(), self.game.cfg.gametype.clone()));
        pairs.push(("sv_hostname".into(), self.game.cfg.hostname.clone()));
        pairs.push((
            "sv_maxclients".into(),
            self.game.cfg.max_clients.to_string(),
        ));
        info_sorted(pairs)
    }

    fn systeminfo(&self) -> String {
        let mut pairs: Vec<(String, String)> = oob::SYSTEMINFO
            .iter()
            .map(|n| (n.to_string(), self.game.cvars.get(n).to_string()))
            .collect();
        let join = |v: Vec<String>| v.join(" ");
        let (sums, names) = if self.pure() {
            (
                self.game
                    .iwds
                    .sums
                    .iter()
                    .map(|s| format!("{s} "))
                    .collect(),
                join(self.game.iwds.names.clone()),
            )
        } else {
            (String::new(), String::new())
        };
        pairs.push(("sv_iwds".into(), sums));
        pairs.push(("sv_iwdNames".into(), names));
        pairs.push((
            "sv_referencedIwds".into(),
            self.game
                .iwds
                .referenced
                .iter()
                .map(|(s, _)| format!("{s} "))
                .collect(),
        ));
        pairs.push((
            "sv_referencedIwdNames".into(),
            join(
                self.game
                    .iwds
                    .referenced
                    .iter()
                    .map(|(_, n)| n.clone())
                    .collect(),
            ),
        ));
        pairs.push(("sv_serverid".into(), self.game.server_id.to_string()));
        pairs.push(("sv_pure".into(), u8::from(self.pure()).to_string()));
        info_sorted(pairs)
    }

    pub fn configstring(&self, i: usize) -> &str {
        &self.core.configstrings[i]
    }

    pub fn server_id(&self) -> i32 {
        self.game.server_id
    }

    pub fn time_ms(&self) -> i32 {
        self.core.sv_time_ms
    }

    pub fn client_count(&self) -> usize {
        self.core
            .clients
            .iter()
            .flatten()
            .filter(|c| c.state >= State::Connected)
            .count()
    }

    /// An engine cvar as `atoi` reads it.
    fn int(&self, name: &str) -> i32 {
        crate::game::builtins::cvar::atoi(self.game.cvars.get(name))
    }

    /// The loop's socket and wait for `SV_SpawnServer`'s send and sleep.
    pub fn set_net_now(&mut self, net: NetNow) {
        self.game.net_now = Some(net);
    }

    /// `SV_MasterHeartbeat(text)` at `dedicated 2`; the flatline ignores
    /// the timer.
    fn heartbeat(&mut self, flatline: bool) {
        let dedicated = self.game.cvars.get("dedicated").parse().unwrap_or(0);
        let mut resolve = self.core.resolver;
        let beats = if flatline {
            self.core
                .masters
                .shutdown(dedicated, self.core.sv_time_ms, &mut resolve)
        } else {
            self.core.masters.heartbeat(
                dedicated,
                self.core.sv_time_ms,
                crate::master::HEARTBEAT_GAME_COD2,
                &mut resolve,
            )
        };
        self.core.outbox.extend(beats);
        if !flatline {
            let argv1 = self.game.argv1.clone();
            for to in self.core.masters.status(dedicated, self.core.sv_time_ms) {
                crate::engine::answer_status(self, to, crate::engine::StatusQuery::Status, &argv1);
            }
        }
    }

    /// Runs the queued console lines (`quit` from Ctrl-C), each split on
    /// `;` as `Cbuf_ExecuteText` does: a passed `g_gametype` vote queues
    /// `g_gametype tdm; map_restart`.
    pub fn drain_console(&mut self, _now: Instant) {
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
        self.core
            .outbox
            .push((to, vcod_cod2::net::oob::packet(text)));
    }

    fn client_packet(&mut self, from: SocketAddr, pkt: &[u8], now: Instant) {
        if pkt.len() < 6 {
            return;
        }
        let qport = u16::from_le_bytes([pkt[4], pkt[5]]);
        let Some(slot) = self.core.clients.iter().position(|c| {
            c.as_ref()
                .is_some_and(|c| c.link.addr.ip() == from.ip() && c.link.netchan.qport == qport)
        }) else {
            // `SV_PacketEvent` (0x80960e2): a sender no slot holds is told
            // to go away (17.3).
            self.send_oob(from, "disconnect");
            return;
        };
        let svs = self.core.sv_time_ms;
        let c = self.core.clients[slot].as_mut().unwrap();
        let Some(m) = c.link.netchan.process_in(pkt) else {
            return;
        };
        // An acknowledge past what we sent, or so far behind that its key is
        // gone, is a forgery or garbage (`SV_ExecuteClientMessage`).
        let seq = c.link.netchan.reliable_sequence as i32;
        if m.reliable_ack > seq || seq - m.reliable_ack >= MAX_RELIABLE_COMMANDS as i32 {
            return;
        }
        c.link.netchan.accept(&m);
        c.link.addr = from;
        c.link.reliable_ack = m.reliable_ack;
        // A zombie's packets go through the netchan and nothing else: its
        // `lastPacketTime` stands, so it is freed `sv_zombietime` after the
        // last one before the drop.
        if c.state == State::Zombie {
            return;
        }
        c.last_heard = now;
        c.link.last_packet_ms = svs;
        let ops = self.core.huff.decompress_block(&m.ops);
        let id = i32::from(m.server_id);
        // A downloading client's messages are read whatever their serverId
        // (`SV_ExecuteClientMessage` 0x809114e).
        if id != self.game.server_id && c.download.name.is_empty() {
            // `SV_ExecuteClientMessage` (0x809114e): an id of this spawn
            // from before a `fast_restart` only lets a primed client in;
            // any other stale id gets the gamestate again once the client
            // has acknowledged past the last one (cod2-map-cycle.md 1).
            if id & 0xf0 == self.game.server_id & 0xf0 {
                if c.state == State::Primed {
                    c.state = State::Active;
                    if let Some(level) = self.game.level.as_mut() {
                        level.begin(slot);
                    }
                }
            } else if m.message_ack > c.gamestate_message {
                self.send_gamestate(slot);
            }
            return;
        }
        let mut r = MsgReader::from_plain(ops);
        loop {
            if r.is_overflowed() {
                return;
            }
            let op = r.read_bits(CLC_BITS);
            match op {
                x if x == Clc::ClientCommand as i32 => {
                    let seq = r.read_long();
                    let text = latin1(&r.read_string_bytes());
                    if !self.client_command(slot, seq, text) {
                        return;
                    }
                }
                x => {
                    // Any op past the commands: a client whose `cp` failed is
                    // dropped and sent its last message at once.
                    if self.pure()
                        && self.core.clients[slot]
                            .as_ref()
                            .is_some_and(|c| c.pure == Some(false))
                    {
                        self.drop_client(slot, "EXE_UNPURECLIENTDETECTED");
                        self.send_client_message(slot);
                        return;
                    }
                    if x == Clc::Move as i32 || x == Clc::MoveNoDelta as i32 {
                        self.user_move(slot, &mut r, m.message_ack, x == Clc::Move as i32);
                    }
                    return;
                }
            }
        }
    }

    /// One `clc_clientCommand`. False when the client was dropped.
    fn client_command(&mut self, slot: usize, seq: i32, text: String) -> bool {
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
        c.link.last_client_command = seq;
        log::debug!("cod2: client {slot} command {seq} {text:?}");
        c.link.netchan.last_client_command_string = text.clone();
        let args = crate::game::say::tokenize(&text);
        self.game.argv1 = args.get(1).cloned().unwrap_or_default();
        let Some(cmd) = args.first() else {
            return true;
        };
        // `SV_ClientCommand`'s flood rule (lnxded 0x8090bac): past the
        // first, a command other than `team `, `score ` and `mr ` inside
        // 800 ms of the last reaches no game while `sv_floodProtect` is on,
        // and each pushes the window on. VERIFIED.
        // `svs.time`, the frame clock.
        let now = self.core.sv_time_ms;
        let flood = self.game.cvars.get("sv_floodProtect") != "0";
        let c = self.core.clients[slot].as_mut().unwrap();
        let exempt = ["team ", "score ", "mr "].iter().any(|p| {
            text.len() >= p.len() && text.as_bytes()[..p.len()].eq_ignore_ascii_case(p.as_bytes())
        });
        let game_ok =
            !(c.state == State::Active && flood && now < c.link.next_reliable_ms && !exempt);
        if !exempt {
            c.link.next_reliable_ms = now + 800;
        }
        // `SV_ExecuteClientCommand` (lnxded 0x8090b14): the engine's `ucmds`
        // (the table at 0x817d9e0), matched case for case, then the game's
        // `ClientCommand` for anything else. VERIFIED.
        match cmd.as_str() {
            "cp" => {
                let argv: Vec<&str> = args.iter().map(String::as_str).collect();
                let ok = vcod_cod2::net::pure::verify_cp(
                    &argv,
                    &self.game.iwds.keyed,
                    self.core.checksum_feed,
                );
                log::info!("cod2: client {slot} cp {}", if ok { "ok" } else { "bad" });
                c.pure = Some(ok);
            }
            "disconnect" => {
                self.drop_client(slot, "EXE_DISCONNECTED");
                return false;
            }
            "userinfo" => {
                c.link.userinfo = args
                    .get(1)
                    .map_or("", String::as_str)
                    .chars()
                    .take(1023)
                    .collect();
                self.userinfo_changed(slot);
            }
            "vdr" | "download" | "nextdl" | "stopdl" | "donedl" | "retransdl" | "wwwdl"
            | "muteplayer" | "unmuteplayer" => self.download_command(slot, &args),
            _ if game_ok => self.game_command(slot, &args, &text),
            _ => log::debug!("cod2: client {slot}: text ignored: {cmd}"),
        }
        self.core.clients[slot]
            .as_ref()
            .is_some_and(|c| c.state != State::Zombie)
    }

    /// The engine's `vdr`, download and voice commands (`.data` 0x817d9e0,
    /// cod2-protocol.md 17.5).
    fn download_command(&mut self, slot: usize, args: &[String]) {
        let arg = |i: usize| args.get(i).map_or("", String::as_str);
        let atoi = crate::game::builtins::cvar::atoi;
        let svs = self.core.sv_time_ms;
        let Some(c) = self.core.clients[slot].as_mut() else {
            return;
        };
        match args[0].as_str() {
            // `vdr` (0x80907ba): the pure verdict back to unknown.
            "vdr" => c.pure = None,
            "download" => c.download.begin(arg(1)),
            "nextdl" => {
                if !c.download.next_dl(atoi(arg(1)), svs) {
                    self.drop_client(slot, "broken download");
                }
            }
            "stopdl" => {
                if !c.download.name.is_empty() {
                    log::info!(
                        "clientDownload: {slot} : file {:?} aborted",
                        c.download.name
                    );
                }
                c.download.close();
            }
            "donedl" => self.send_gamestate(slot),
            "retransdl" => c.download.retransmit(atoi(arg(1))),
            "wwwdl" => match c.download.wwwdl(arg(1)) {
                house::Www::None => {}
                house::Www::Gamestate => self.send_gamestate(slot),
                house::Www::Drop(why) => self.drop_client(slot, why),
            },
            // Voice: nothing to mute.
            _ => {}
        }
    }

    /// `SV_UserinfoChanged` (0x80907cc): the rate, the snapshot interval
    /// and the www flag.
    fn userinfo_changed(&mut self, slot: usize) {
        let dedicated = self.int("dedicated");
        if let Some(c) = self.core.clients[slot].as_mut() {
            c.rates = link::Rates::of(&c.link.userinfo, c.lan, dedicated);
        }
    }

    /// `ClientCommand` (Mac 0x1a7684): `say`, `say_team` and `score`
    /// always; at the intermission nothing else; then `mr`, `kill`, the
    /// votes and the rest (`Cod2Level::client_command`), in retail's
    /// compare order (cod2-protocol.md 3.4.1).
    fn game_command(&mut self, slot: usize, args: &[String], text: &str) {
        let svs = self.svs_time_ms();
        let Some(level) = self.game.level.as_mut() else {
            return;
        };
        let cmd = args[0].to_ascii_lowercase();
        let mut sends: crate::game::vote::Sends = Vec::new();
        match cmd.as_str() {
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
                        sends.push((Some(to), line));
                    }
                }
            }
            // `Cmd_Score_f` (Mac 0x1a5186): the scoreboard to the asker.
            "score" => sends.push((Some(slot), level.scoreboard())),
            _ if level.at_intermission(slot) => {}
            // `mr <serverId> <menu> <response>`, 1.1's shapes (the handler
            // at 0x8100d16).
            "mr" => {
                if let Some((menu, response)) =
                    crate::server::parse_menu_response(text, self.game.server_id)
                {
                    level.menu_response(slot, &menu, &response);
                }
            }
            // `Cmd_Kill_f` (in `ClientCommand`, Mac 0x1a7684): a playing
            // client's `player_die` with `MOD_SUICIDE`.
            "kill" => level.kill(slot),
            // `Cmd_CallVote_f` and `Cmd_Vote_f`: the engine's vote under
            // `g_oldVoting` 1, the script's under 0 (`game::vote`).
            "callvote" | "vote" => sends = level.rt.vote_command(slot, args, svs),
            _ => {
                let out = level.client_command(slot, args);
                for line in out.prints {
                    log::info!("{line}");
                }
                sends = out.sends;
            }
        }
        for (to, cmd) in sends {
            match to {
                Some(to) => self.send_server_command(to, &cmd),
                None => self.send_to_all(&cmd),
            }
        }
        self.flush_game_commands();
    }

    /// `SV_SendServerCommand(NULL, ..)` (0x8094a10): every client past
    /// `CS_CONNECTED`, zombies left out.
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

    /// The configstrings the script wrote and the server commands and
    /// console lines it queued, out to their clients in the order they
    /// were written, as retail's `SV_SetConfigstring` and
    /// `SV_GameSendServerCommand` send each at once (cod2-map-cycle.md 3:
    /// the intermission's `f` lines lead its `d 5` / `d 6`). The dvar
    /// mirror's changes wait for the next `SV_Frame`'s head.
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
    }

    /// `SV_SetConfigstring` (0x8092780) of a game write: the table, and
    /// `d` (or its `x`/`y`/`z` pieces) to every client past
    /// `CS_CONNECTED`. 0 and 1 are the engine's. VERIFIED, the `2 <`
    /// test.
    fn write_configstring(&mut self, i: usize, value: &str) {
        if i < 2 || i >= self.core.configstrings.len() {
            return;
        }
        self.core.configstrings[i] = value.to_string();
        if let Some(sent) = self.core.sent_configstrings.get_mut(i) {
            *sent = value.to_string();
        }
        let cmds = vcod_common::net::configstring_commands(i, value);
        for slot in 0..self.core.clients.len() {
            if self.core.clients[slot]
                .as_ref()
                .is_some_and(|c| c.state > State::Connected)
            {
                for cmd in &cmds {
                    self.send_server_command(slot, cmd);
                }
            }
        }
    }

    /// `SV_UserMove`: the keyed cmds, the pure verdict, entering the world,
    /// then each cmd through the spectator's pmove.
    fn user_move(&mut self, slot: usize, r: &mut MsgReader, message_ack: i32, delta: bool) {
        crate::tick_trace::step!("SV_UserMove slot {slot}");
        let pure = self.pure();
        let c = self.core.clients[slot].as_mut().unwrap();
        c.delta_message = if delta { message_ack } else { -1 };
        let count = usize::from(r.read_byte());
        if count == 0 || count > MAX_PACKET_USERCMDS {
            return;
        }
        let key_cmd = latin1_bytes(
            &c.link.netchan.reliable[c.link.reliable_ack as usize & (MAX_RELIABLE_COMMANDS - 1)],
        );
        let key = (self.core.checksum_feed ^ message_ack ^ com_hash_key_bytes(&key_cmd, 32)) as u32;
        let base = match self.game.level.as_ref() {
            Some(level) => level
                .wire_playerstate(slot, 0)
                .unwrap_or_else(|| spectator_wire(&c.ps, slot)),
            None => spectator_wire(&c.ps, slot),
        };
        let mut from = default_usercmd(&base);
        let mut cmds = Vec::with_capacity(count);
        for _ in 0..count {
            let cmd = read_delta_usercmd(r, key, &from);
            cmds.push(cmd);
            from = cmd;
        }
        if r.is_overflowed() {
            return;
        }
        // `svs.time` steps a frame at a time, so a loopback client that
        // answers inside the frame reads 0, as retail's does (live,
        // cod2-protocol.md 3.4.1).
        let now = self.core.sv_time_ms;
        let c = self.core.clients[slot].as_mut().unwrap();
        if message_ack >= 0 {
            c.link.ping_ring[message_ack as usize % PACKET_BACKUP].1 = now;
        }
        // `SV_UserMove` (0x8090e34): the client enters the world on its
        // first move, before the pure check.
        if c.state == State::Primed {
            c.state = State::Active;
            log::info!("cod2: client {slot} entered the world");
            if let Some(level) = self.game.level.as_mut() {
                level.begin(slot);
            }
        }
        let c = self.core.clients[slot].as_mut().unwrap();
        // A failed `cp` was dropped before the move; no `cp` at all drops
        // here, after the client entered the world.
        if pure && c.pure.is_none() {
            return self.drop_client(slot, "EXE_CANNOTVALIDATEPURECLIENT");
        }
        // A cmd past the packet's newest or at or before the last thought
        // is skipped.
        let newest = cmds.last().map_or(0, |c| c.server_time);
        if let Some(level) = self.game.level.as_mut() {
            level.packet();
        }
        for cmd in cmds {
            if cmd.server_time > newest || cmd.server_time <= c.last_cmd_time {
                continue;
            }
            c.last_cmd_time = cmd.server_time;
            match self.game.level.as_mut() {
                Some(level) => level.client_think(slot, &cmd),
                // A bare server: the cmds advance the clock and nothing else.
                None => c.ps.command_time = c.ps.command_time.max(cmd.server_time),
            }
        }
        self.flush_game_commands();
    }

    /// `SV_SendClientGameState`: the unacknowledged commands, the whole
    /// configstring table and the baselines, then `CS_PRIMED`.
    fn send_gamestate(&mut self, slot: usize) {
        let gs = Gamestate2 {
            server_command_sequence: 0,
            configstrings: self
                .core
                .configstrings
                .iter()
                .enumerate()
                .filter(|(_, s)| !s.is_empty())
                .map(|(i, s)| (i as u16, s.clone()))
                .collect(),
            baselines: self.game.baselines.clone(),
            client_num: slot as i32,
            checksum_feed: self.core.checksum_feed,
        };
        let c = self.core.clients[slot].as_mut().unwrap();
        let mut w = MsgWriter::new(&self.core.huff);
        write_pending_commands(&mut w, c);
        PROTOCOL_118.write_gamestate(
            &mut w,
            &Gamestate2 {
                server_command_sequence: c.link.netchan.reliable_sequence as i32,
                ..gs
            },
        );
        c.gamestate_message = c.link.netchan.outgoing_sequence as i32;
        c.state = State::Primed;
        c.delta_message = -1;
        c.pure = None;
        log::info!(
            "cod2: client {slot} gamestate, message {}",
            c.gamestate_message
        );
        let ops = w.into_ops();
        self.transmit(slot, ops);
    }

    /// `SV_SendMessageToClient` (0x809aba2): the message through the
    /// netchan, its first packet now and any other one a turn
    /// ([`crate::engine::send_messages`]), then `nextSnapshotTime` (17.2).
    fn transmit(&mut self, slot: usize, ops: Vec<u8>) {
        let svs = self.core.sv_time_ms;
        let max_rate = self.int("sv_maxRate");
        let Some(c) = self.core.clients[slot].as_mut() else {
            return;
        };
        let mut pkts = c.link.netchan.transmit_eof(
            c.link.last_client_command,
            &ops,
            Svc::Eof as u8,
            &self.core.huff,
        );
        // The compressed message with its acknowledge: one packet less its
        // sequence, or each fragment less its 10-byte header.
        let bytes = match pkts.as_slice() {
            [one] => one.len() - 4,
            many => many.iter().map(|p| p.len() - 10).sum(),
        };
        let idle = c.state != State::Active && c.download.name.is_empty();
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

    /// Queues a reliable server command (`SV_AddServerCommand`); a client
    /// whose ring is full of unacknowledged ones is dropped.
    pub fn send_server_command(&mut self, slot: usize, text: &str) {
        let Some(c) = self.core.clients[slot].as_mut() else {
            return;
        };
        // `SV_AddServerCommand` (0x8094750) adds nothing for a test
        // client (`client+0xae70c`, which `SV_AddTestClient` sets), so its
        // never-acked ring cannot overflow. VERIFIED.
        if c.test {
            return;
        }
        log::trace!("cod2: to client {slot}: {text:?}");
        if c.link.netchan.reliable_sequence as i32 - c.link.reliable_ack
            >= MAX_RELIABLE_COMMANDS as i32
        {
            self.drop_client(slot, "EXE_SERVERCOMMANDOVERFLOW");
            return;
        }
        push_command(c, text);
    }

    /// `SV_DropClient` (0x808f02e): the game lets go of the slot, which
    /// turns `CS_ZOMBIE`; everyone else in the world reads the reason as
    /// `e` (a localized one behind `\x14`, a plain disconnect not at all)
    /// and the slot's `J`, the client its `w`, carried by the zombie's next
    /// messages (17.3). A bot's slot is free at once.
    fn drop_client(&mut self, slot: usize, reason: &str) {
        let Some(c) = self.core.clients[slot].as_mut() else {
            return;
        };
        if c.state == State::Zombie {
            return;
        }
        log::info!("cod2: client {slot} dropped: {reason}");
        c.state = State::Zombie;
        c.download.close();
        let bot = c.bot;
        let name = vcod_common::net::info_value_for_key(&c.link.userinfo, "name")
            .unwrap_or("")
            .chars()
            .take(31)
            .collect::<String>();
        let addr = c.link.addr;
        // The first slot with the client's address (`SV_DropClient`
        // 0x808f02e), skipped for a bot.
        if !bot {
            self.core.challenges.release(addr);
        }
        if let Some(level) = self.game.level.as_mut() {
            level.disconnect(slot);
        }
        self.flush_game_commands();
        if bot {
            self.core.clients[slot] = None;
            if let Some(b) = self.game.bots.as_mut() {
                b.remove(slot);
            }
        }
        let localized = ["EXE_", "GAME_", "PC_PATCH_"]
            .iter()
            .any(|p| reason.starts_with(p));
        if !reason.eq_ignore_ascii_case("EXE_DISCONNECTED") {
            let mark = if localized { "\u{14}" } else { "" };
            self.send_to_all(&format!("e \"\u{15}{name}^7 {mark}{reason}\""));
        }
        self.send_to_all(&format!("J {slot}"));
        if let Some(c) = self.core.clients[slot].as_mut() {
            let w = if localized {
                format!("w \"{reason}\"")
            } else {
                format!("w \"{name}^7 {reason}\" PB")
            };
            push_command(c, &w);
        }
        if self.client_count() == 0 {
            self.core.masters.force();
        }
        self.flush_game_commands();
    }

    /// `G_RunFrame` on the stepped clock, then `SV_AddTestClient` for the
    /// slots it connected.
    fn run_level_frame(&mut self) {
        // `svs.time`, the frame clock `SV_DirectConnect`'s reconnect
        // check reads for `addTestClient`'s bot address too.
        let svs = self.core.sv_time_ms;
        if let Some(level) = self.game.level.as_mut() {
            level.rt.set_network_time(svs);
            let port = &mut level.rt.host.test_clients.counter;
            *port = (*port).max(self.game.bot_port);
            crate::tick_trace::step!("G_RunFrame time {}", self.core.sv_time_ms);
            level.run_frame(self.core.sv_time_ms);
            self.game.bot_port = level.rt.host.test_clients.counter;
        }
        crate::tick_trace::step!("install_test_clients");
        self.install_test_clients();
    }

    /// The frame into the level's killcam archive, while script has it on.
    fn archive_frame(&mut self) {
        if !self.game.level.as_ref().is_some_and(|l| l.rt.archive_on()) {
            return;
        }
        let roster = self.client_states();
        let world = self.world_entities();
        if let Some(level) = self.game.level.as_mut() {
            level.archive_frame(&world, &roster);
        }
    }

    /// `SV_CheckTimeouts` (0x809657e): a zombie silent `sv_zombietime` is
    /// freed; anyone else silent `sv_timeout` is dropped on the sixth such
    /// frame and freed at once, with no zombie turn (17.3).
    fn check_timeouts(&mut self) {
        let svs = self.core.sv_time_ms;
        let drop_before = svs - self.int("sv_timeout") * 1000;
        let free_before = svs - self.int("sv_zombietime") * 1000;
        for slot in 0..self.core.clients.len() {
            let Some(c) = self.core.clients[slot].as_mut() else {
                continue;
            };
            if c.bot {
                continue;
            }
            c.link.last_packet_ms = c.link.last_packet_ms.min(svs);
            if c.state == State::Zombie && c.link.last_packet_ms < free_before {
                log::debug!("cod2: client {slot} from CS_ZOMBIE to CS_FREE");
                self.core.clients[slot] = None;
            } else if c.state == State::Zombie || c.link.last_packet_ms >= drop_before {
                c.link.timeout_count = 0;
            } else {
                c.link.timeout_count += 1;
                if c.link.timeout_count > house::TIMEOUT_FRAMES {
                    self.drop_client(slot, "EXE_TIMEDOUT");
                    self.core.clients[slot] = None;
                }
            }
        }
    }

    /// `SV_CalcPings`: each client's ping into the level's roster, 999 for
    /// one not yet in the world.
    fn calc_pings(&mut self) {
        for c in self.core.clients.iter_mut().flatten() {
            // `SV_CalcPings` (0x80963c8) has no bot arm: a bot acks no
            // frame, so it reads 999 (`!count`). VERIFIED.
            c.link.ping = if c.bot {
                999
            } else if c.state == State::Active {
                link::calc_ping(&c.link.ping_ring)
            } else {
                999
            };
        }
        let Some(level) = self.game.level.as_mut() else {
            return;
        };
        for (slot, c) in self.core.clients.iter().enumerate() {
            if let Some(c) = c {
                level.set_ping(slot, c.link.ping);
            }
        }
    }

    /// Every changed configstring as `d` to the clients that hold a table.
    fn broadcast_configstrings(&mut self) {
        let changed: Vec<usize> = (0..self.core.configstrings.len())
            .filter(|&i| self.core.sent_configstrings.get(i) != Some(&self.core.configstrings[i]))
            .collect();
        if changed.is_empty() {
            return;
        }
        self.core.sent_configstrings = self.core.configstrings.clone();
        for slot in 0..self.core.clients.len() {
            if !self.core.clients[slot]
                .as_ref()
                .is_some_and(|c| c.state > State::Connected)
            {
                continue;
            }
            for &i in &changed {
                for cmd in vcod_common::net::configstring_commands(i, &self.core.configstrings[i]) {
                    self.send_server_command(slot, &cmd);
                }
            }
        }
    }

    /// Whether `slot` takes a turn in `SV_SendClientMessages` (0x809bcce,
    /// [`crate::engine::send_messages`]): a client with a socket whose
    /// `nextSnapshotTime` came (17.2).
    fn message_due(&self, slot: usize) -> bool {
        let svs = self.core.sv_time_ms;
        self.core.clients[slot]
            .as_ref()
            .is_some_and(|c| !c.bot && c.link.next_message_ms <= svs)
    }

    /// The turn's next fragment, paced on the bytes left (17.2).
    fn next_fragment(&mut self, slot: usize) -> Option<(SocketAddr, Vec<u8>)> {
        let svs = self.core.sv_time_ms;
        let max_rate = self.int("sv_maxRate");
        let c = self.core.clients[slot].as_mut()?;
        let pkt = c.link.unsent.pop_front()?;
        let left = pkt.len() - 10 + c.link.unsent.iter().map(|p| p.len() - 10).sum::<usize>();
        c.link.next_message_ms = svs + link::rate_msec(left, c.rates.send_rate(max_rate));
        Some((c.link.addr, pkt))
    }

    /// A new message for `slot`, the roster and the world built on the
    /// first client that needs them.
    fn send_client_snapshot(&mut self, slot: usize, shared: &mut Cod2Snapshots) {
        let (clients, world) =
            shared.get_or_insert_with(|| (self.client_states(), self.world_entities()));
        let ops = self.client_message(slot, clients, world);
        self.transmit(slot, ops);
    }

    /// One client's message now, outside the pacing: a drop for a bad `cp`
    /// sends the zombie its `w` at once.
    fn send_client_message(&mut self, slot: usize) {
        let clients = self.client_states();
        let world = self.world_entities();
        let ops = self.client_message(slot, &clients, &world);
        self.transmit(slot, ops);
    }

    /// `SV_SendClientSnapshot` (0x809adea): an active client or a zombie
    /// gets its server commands and a snapshot, any other just the
    /// acknowledge; everyone but a zombie its download's next blocks.
    fn client_message(
        &mut self,
        slot: usize,
        clients: &[ClientState2],
        world: &BTreeMap<u32, (EntityState, bool)>,
    ) -> Vec<u8> {
        let svs = self.core.sv_time_ms;
        let team_of = |slot: usize| {
            clients
                .iter()
                .find(|cs| cs.number as usize == slot)
                .map_or(TEAM_SPECTATOR as i32, |cs| {
                    cs.f[field_index(&CLIENT_STATE_FIELDS, "team")] as i32
                })
        };
        let dl_cvars = house::DownloadCvars {
            allow: self.game.cvars.get("sv_allowDownload") != "0",
            pure: self.pure(),
            www: self.game.cvars.get("sv_wwwDownload") != "0",
            www_base_url: self.game.cvars.get("sv_wwwBaseURL").to_string(),
            www_disconnected: self.game.cvars.get("sv_wwwDlDisconnected") != "0",
        };
        let home = self.game.cvars.get("fs_homepath").to_string();
        let base = self.game.cvars.get("fs_basepath").to_string();
        let max_rate = self.int("sv_maxRate");
        let Some(c) = self.core.clients[slot].as_ref() else {
            return Vec::new();
        };
        let in_world = matches!(c.state, State::Active | State::Zombie);
        // A killcam's snapshot carries the archived frame's roster
        // (cod2-killcam.md 4).
        let (ps, entities, archived) = match self.game.level.as_mut() {
            Some(level) if in_world => {
                let ps = level
                    .wire_playerstate(slot, team_of(slot))
                    .unwrap_or_else(|| spectator_wire(&c.ps, slot));
                let (entities, archived) = level.snapshot_entities(world, slot, &ps);
                (ps, entities, archived)
            }
            _ => (spectator_wire(&c.ps, slot), Vec::new(), None),
        };
        let c = self.core.clients[slot].as_mut().unwrap();
        let mut w = MsgWriter::new(&self.core.huff);
        if in_world {
            write_pending_commands(&mut w, c);
            let message = c.link.netchan.outgoing_sequence as i32;
            let to = Frame2 {
                server_time: svs,
                ps,
                entities,
                clients: archived.map_or_else(|| clients.to_vec(), |r| r.to_vec()),
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
            // `SV_WriteSnapshotToClient` (0x80985c8): the server count bit,
            // 1 while the rate holds messages back, 2 for a zombie never
            // sent one active.
            if c.state == State::Active {
                c.link.was_active = true;
            }
            let flags = self.game.snap_flags
                | u8::from(c.link.rate_delayed)
                | if c.link.was_active { 0 } else { 2 };
            let baselines = &self.game.baselines;
            let head = SnapshotHead {
                delta_num: (message - c.delta_message) as u8,
                snap_flags: u32::from(flags),
            };
            PROTOCOL_118.write_snapshot(&mut w, from, &to, head, baselines);
            c.link.frames[message as usize % PACKET_BACKUP] = Some((message, to));
        }
        if c.state != State::Zombie {
            let budget =
                house::blocks_per_message(c.rates.send_rate(max_rate), c.rates.snapshot_msec);
            let open = |name: &str| house::open_download(&home, &base, name);
            c.download
                .write(&mut w, svs, budget, &dl_cvars, c.rates.wants_www, &open);
        }
        c.link.ping_ring[c.link.netchan.outgoing_sequence as usize % PACKET_BACKUP] = (svs, -1);
        w.into_ops()
    }

    /// The frame's entities before any client's cull
    /// ([`Cod2Level::world_entities`]).
    fn world_entities(&mut self) -> BTreeMap<u32, (EntityState, bool)> {
        let svs = self.svs_time_ms();
        match self.game.level.as_mut() {
            Some(level) => level.world_entities(svs),
            None => BTreeMap::new(),
        }
    }

    /// One client state per client with a gamestate: its team, its
    /// userinfo's name, and with a level the body model and attachments
    /// the script set.
    fn client_states(&mut self) -> Vec<ClientState2> {
        let mut level = self.game.level.as_mut();
        self.core
            .clients
            .iter()
            .enumerate()
            .filter_map(|(i, c)| {
                let c = c.as_ref().filter(|c| c.state != State::Connected)?;
                let name =
                    vcod_common::net::info_value_for_key(&c.link.userinfo, "name").unwrap_or("");
                Some(match level.as_deref_mut() {
                    Some(level) => level.client_state(i, name),
                    None => crate::game::cod2_wire::bare_client_state(i, name),
                })
            })
            .collect()
    }

    /// A new client in `slot`: a spectator at the intermission spot, and
    /// with a level `ClientConnect` (`SV_DirectConnect` calls it).
    fn admit(&mut self, slot: usize, from: SocketAddr, qport: u16, challenge: i32, userinfo: &str) {
        let (origin, angles) = self.game.spot;
        if let Some(level) = self.game.level.as_mut() {
            // A reconnect frees the old client first (`SV_FreeClient`); a
            // zombie's game side is gone already.
            if self.core.clients[slot]
                .as_ref()
                .is_some_and(|c| c.state > State::Zombie)
            {
                level.disconnect(slot);
            }
            let name = vcod_common::net::info_value_for_key(userinfo, "name").unwrap_or("");
            level.rt.host.client_names[slot] = name.to_string();
            level.connect_at(slot, name, origin, angles);
        }
        self.core.clients[slot] = Some(self.new_client(slot, from, qport, challenge, userinfo));
    }

    /// A client connected in `slot`: a spectator at the intermission spot.
    fn new_client(
        &self,
        slot: usize,
        from: SocketAddr,
        qport: u16,
        challenge: i32,
        userinfo: &str,
    ) -> Client {
        let (origin, angles) = self.game.spot;
        let mut ps = PlayerState::spawn(origin, angles[1], self.core.sv_time_ms);
        ps.view_angles = angles;
        ps.delta_angles = angles.map(angle_to_short);
        ps.pm_type = pm_type::SPECTATOR;
        ps.speed = 400;
        ps.gravity = 0;
        ps.pm_flags = SPECTATOR_PM_FLAGS;
        ps.e_flags = SPECTATOR_EFLAGS;
        ps.client_num = slot as u32;
        let svs = self.core.sv_time_ms;
        let lan = Lan::Cod2.is_lan(from.ip());
        let rates = link::Rates::of(userinfo, lan, self.int("dedicated"));
        let mut link = crate::engine::slot::Link::new(
            from,
            ServerNetchan::with_params(qport, challenge, NetchanParams::COD2),
            userinfo.to_string(),
        );
        link.next_message_ms = svs;
        link.last_packet_ms = svs;
        Client {
            link,
            state: State::Connected,
            gamestate_message: -1,
            delta_message: -1,
            last_cmd_time: 0,
            ps,
            pure: None,
            last_heard: Instant::now(),
            bot: false,
            rates,
            lan,
            last_connect_ms: svs,
            download: house::Download::default(),
            test: false,
        }
    }

    /// `SV_AddTestClient` (lnxded 0x80913da): each slot `addTestClient`
    /// connected gets an active client with a bot address and the stock
    /// userinfo, which sends nothing and is sent nothing.
    fn install_test_clients(&mut self) {
        let Some(level) = self.game.level.as_mut() else {
            return;
        };
        for (slot, port, name) in level.take_test_clients() {
            let userinfo = format!(
                "\\cg_predictItems\\1\\cl_punkbuster\\0\\cl_anonymous\\0\\color\\4\\head\\default\\model\\multi\\snaps\\20\\rate\\5000\\name\\{name}\\protocol\\{}",
                vcod_cod2::net::ops::PROTOCOL
            );
            let addr = SocketAddr::from(([0, 0, 0, 0], port));
            let mut c = self.new_client(slot, addr, 0, 0, &userinfo);
            c.bot = true;
            c.state = State::Active;
            c.test = true;
            self.core.clients[slot] = Some(c);
            log::info!("cod2: test client {name} in slot {slot}");
        }
    }

    /// Seconds since the server started, `svs.time`.
    fn svs_time_ms(&self) -> i32 {
        if self.game.frame_clock {
            return self.core.sv_time_ms;
        }
        self.game.start.elapsed().as_millis() as i32
    }

    /// `svs.time` on the frame clock instead of the wall's, so a harness
    /// that runs frames faster than real time sends the same bytes each
    /// run (`bot_match`'s `NETDIGEST`).
    pub fn test_frame_clock(&mut self) {
        self.game.frame_clock = true;
    }
}

/// One more server command in the ring, past any check.
fn push_command(c: &mut Client, text: &str) {
    // `SV_AddServerCommand` (0x8094750) takes an unsent pending command
    // out before it appends (`SV_FindPendingCommand` 0x8094530, 1.1's
    // shape, protocol-1.1.md "Squash"): an equal `C` or `a` (a frame's
    // give and `switchToOffhand` reach the client as one `C`,
    // cod2-protocol.md 15.5), and a `d` of the same index (0x80944aa
    // compares the first word), so a restart's fog cleared and set back
    // goes out once, where the second write stood. VERIFIED, the switch.
    // vcod squashes the type 1 letters it raises.
    let same = |pending: &str| match text.as_bytes().first() {
        Some(b'C' | b'a') => pending == text,
        Some(b'd') => {
            let word = |t: &str| {
                t.get(2..)
                    .and_then(|r| r.split(' ').next())
                    .map(str::to_owned)
            };
            pending.starts_with('d') && word(pending) == word(text)
        }
        _ => false,
    };
    {
        let ring = |i: i32| i as usize & (MAX_RELIABLE_COMMANDS - 1);
        let seq = c.link.netchan.reliable_sequence as i32;
        let from = c.link.reliable_sent.max(c.link.reliable_ack) + 1;
        if let Some(at) = (from..=seq).find(|&i| same(&c.link.netchan.reliable[ring(i)])) {
            for i in at..seq {
                c.link.netchan.reliable[ring(i)] =
                    std::mem::take(&mut c.link.netchan.reliable[ring(i + 1)]);
            }
            c.link.netchan.reliable_sequence -= 1;
        }
    }
    c.link.netchan.reliable_sequence += 1;
    let at = c.link.netchan.reliable_sequence as usize & (MAX_RELIABLE_COMMANDS - 1);
    c.link.netchan.reliable[at] = text.to_string();
}

/// The unacknowledged server commands, each `svc_serverCommand`.
fn write_pending_commands(w: &mut MsgWriter, c: &mut Client) {
    c.link.reliable_sent = c.link.netchan.reliable_sequence as i32;
    for seq in c.link.reliable_ack + 1..=c.link.netchan.reliable_sequence as i32 {
        let text = &c.link.netchan.reliable[seq as usize & (MAX_RELIABLE_COMMANDS - 1)];
        write_server_command(w, seq, &latin1_bytes(text));
    }
}

/// `ANGLE2SHORT`.
fn angle_to_short(a: f32) -> i32 {
    ((a * 65536.0 / 360.0) as i32) & 0xffff
}

/// A spectator's playerstate on the wire: the fields a retail spectator's
/// carries (cod2-protocol.md 9), from the pmove state.
fn spectator_wire(ps: &PlayerState, slot: usize) -> PlayerState2 {
    let mut out = PlayerState2::default();
    let mut set = |name: &str, v: u32| out.f[field_index(&PLAYER_STATE_FIELDS, name)] = v;
    set("commandTime", ps.command_time as u32);
    for i in 0..3 {
        set(&format!("origin[{i}]"), ps.origin[i].to_bits());
        set(&format!("velocity[{i}]"), ps.velocity[i].to_bits());
        set(&format!("viewangles[{i}]"), ps.view_angles[i].to_bits());
        set(
            &format!("delta_angles[{i}]"),
            ps.delta_angles[i] as u32 & 0xffff,
        );
        set(&format!("mins[{i}]"), ps.mins[i].to_bits());
        set(&format!("maxs[{i}]"), ps.maxs[i].to_bits());
    }
    set("pm_type", ps.pm_type as u32);
    set("pm_flags", ps.pm_flags);
    set("pm_time", ps.pm_time as u32);
    set("eFlags", ps.e_flags);
    set("speed", ps.speed as u32);
    set("gravity", ps.gravity as u32);
    set("clientNum", slot as u32);
    set("viewlocked_entNum", ENTITYNUM_NONE);
    // `stats[3]` and `stats[5]` as a retail spectator's read (9).
    out.stats[3] = 63;
    out.stats[5] = 1;
    out
}

/// `mp_global_intermission`'s origin and angles, else the first spawn's,
/// else the world's middle.
fn intermission_spot(world: &MapWorld) -> (Vec3, [f32; 3]) {
    let blocks = vcod_common::bsp::entity_blocks(&world.entities);
    let find = |want: &dyn Fn(&str) -> bool| {
        blocks.iter().find_map(|b| {
            let class = b.get("classname")?;
            if !want(class) {
                return None;
            }
            let o = vcod_common::bsp::parse_vec3(b.get("origin")?)?;
            let a = b
                .get("angles")
                .and_then(|a| vcod_common::bsp::parse_vec3(a))
                .unwrap_or([0.0; 3]);
            Some((Vec3::from_array(o), a))
        })
    };
    find(&|c| c == "mp_global_intermission")
        .or_else(|| find(&|c| c.starts_with("mp_") && c.ends_with("_spawn")))
        .unwrap_or_else(|| {
            let m = world.models.first();
            let mid = m.map_or(Vec3::ZERO, |m| {
                (Vec3::from_array(m.mins) + Vec3::from_array(m.maxs)) * 0.5
            });
            (mid, [0.0; 3])
        })
}

/// `SV_SpawnServer`'s iwd lists over what the load read.
fn pure_lists(fs: &Pk3Fs, feed: i32) -> Iwds {
    let mut out = Iwds::default();
    for p in fs.paks().filter(|p| !p.localized) {
        out.sums.push(p.checksum);
        out.names.push(p.name.clone());
        out.keyed.push(p.pure_checksum(feed));
    }
    out.referenced = fs
        .referenced_paks()
        .into_iter()
        .map(|p| (p.checksum, p.qualified_name()))
        .collect();
    out
}

/// An info string in `Dvar`'s case-folded order; empty values left out.
fn info_sorted(mut pairs: Vec<(String, String)>) -> String {
    pairs.sort_by_key(|(k, _)| k.to_ascii_lowercase());
    let mut info = vcod_common::net::connectionless::Info::new();
    for (k, v) in &pairs {
        info.set(k, v);
    }
    info.to_string()
}

#[cfg(test)]
mod cycle_tests;
#[cfg(test)]
mod house_tests;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod wire_ab;
