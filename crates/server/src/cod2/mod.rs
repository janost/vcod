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
//! Voice chat's relay is `voice.rs` (docs/research/cod2-voice.md).
//!
//! Not here: PunkBuster, the authorize server. The heartbeat and the
//! 10-minute status go to cod.pm's CoD2 list, only at `dedicated 2`
//! (cod2-protocol.md 17.6).
//!
//! Shots, damage, deaths and missiles are the level's
//! (`game::cod2_combat`); this side sends their corpses, temp entities
//! and missiles.

mod bots;
mod cycle;
mod house;
mod oob;
mod voice;

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
use vcod_cod2::net::{Gamestate2, UserCmd2, latin1};
use vcod_cod2::pmove::{PlayerState, pm_type};
use vcod_common::mapworld::MapWorld;
use vcod_common::net::codec::{MsgReader, MsgWriter};
use vcod_common::net::com_hash_key_bytes;
use vcod_common::net::msg::EntityState;
use vcod_common::net::netchan::{ClientMessage, NetchanParams, ServerNetchan};
use vcod_common::net::protocol::WireGame;
use vcod_common::pk3::Pk3Fs;

use crate::engine::{Lan, conn, link};
use crate::game::cod2::Cod2Level;
use crate::game::cod2_wire::to_cod2_entity;

/// `sv_fps 20`.
pub const FRAME_MS: i32 = 50;
/// `PACKET_BACKUP`: the frames a client can delta from.
const PACKET_BACKUP: usize = crate::engine::slot::PACKET_BACKUP;
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

use crate::engine::slot::State;

/// A CoD2 client slot (`engine::slot::Conn`); its frames are the message
/// number each went in and the frame.
pub type Client = crate::engine::slot::Conn<(i32, Frame2), Cod2Client>;

/// What a CoD2 slot keeps beside the engine's connection.
pub struct Cod2Client {
    /// A bare server's spectator; with a level the playerstate is the
    /// level's (`Cod2Level::players`).
    ps: PlayerState,
    last_heard: Instant,
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
    /// The level a load replaced, dropped at the next `Cbuf_Execute` (the
    /// next tick's head), so the load's tick does not pay for it
    /// (cod4-server-frame.md 9.1).
    retired: Option<Cod2Level>,
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
    /// A load that failed with the level torn down, for the loop to end the
    /// process on ([`crate::engine::GameModule::take_fatal`]).
    fatal: Option<anyhow::Error>,
    /// The seed each level's `G_InitGame` takes when a test pinned one
    /// ([`Cod2Server::pin_game_seed`]); otherwise a draw off the engine's.
    game_seed: Option<u32>,
    /// The world the last snapshot round built, for the archive that
    /// follows it with no frame between ([`Cod2Server::archive_frame`]).
    snapshot_world: Option<BTreeMap<u32, (EntityState, bool)>>,
}

/// The CoD2 server: the engine over [`Cod2Game`].
pub type Cod2Server = crate::engine::Engine<Cod2Game>;

/// The roster and the world every CoD2 snapshot of a turn is built from,
/// once the first client needs them.
type Cod2Snapshots = Option<(
    Vec<ClientState2>,
    BTreeMap<u32, (EntityState, bool)>,
    Vec<crate::game::cod2_wire::Culled>,
)>;

/// CoD2's `SV_Frame` (cod2_lnxded 0x80969b0, cod2-protocol.md 14.1): the
/// serverinfo and systeminfo dvars' changes as configstrings 0 and 1 and the
/// mirror's block (a `fast_restart`'s `d 1` after its `B`, before the
/// connect callbacks' commands, cod2-map-cycle.md 2), the bots (0x8096952),
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
            crate::engine::StatusQuery::Info => Some(e.svc_info(challenge)),
            crate::engine::StatusQuery::Status => {
                let text = e.svc_status(challenge);
                e.status_argv1();
                Some(text)
            }
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

    fn drop_sender(e: &mut Cod2Server, from: SocketAddr, pkt: &[u8], reason: &str) {
        if let Some(slot) = conn::sender_slot(&e.core, from, pkt) {
            e.drop_client(slot, reason);
        }
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
    fn snapshots_end(e: &mut Cod2Server, snaps: Cod2Snapshots) {
        e.game.snapshot_world = snaps.map(|(_, world, _)| world);
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
    fn take_fatal(e: &mut Cod2Server) -> Option<anyhow::Error> {
        e.game.fatal.take()
    }
}

impl conn::ConnGame for Cod2Game {
    type Frame = (i32, Frame2);
    type Extra = Cod2Client;

    fn cvar<'a>(e: &'a Cod2Server, name: &str) -> &'a str {
        e.game.cvars.get(name)
    }
    fn ignorable(text: &str) -> bool {
        ignorable(text)
    }
    /// `SV_AddServerCommand` (0x8094750) adds nothing for a test client
    /// (`client+0xae70c`, which `SV_AddTestClient` sets), so its
    /// never-acked ring cannot overflow. VERIFIED.
    fn takes_commands(c: &Client) -> bool {
        !c.extra.test
    }
    fn downloading(c: &Client) -> bool {
        !c.extra.download.name.is_empty()
    }
    fn drop_client(e: &mut Cod2Server, slot: usize, reason: &str) {
        e.drop_client(slot, reason);
    }
    fn script(e: &mut Cod2Server) -> Option<&mut crate::game::script::ScriptRuntime> {
        e.game.level.as_mut().map(|l| &mut l.rt)
    }
    fn write_configstring(e: &mut Cod2Server, i: usize, value: &str) {
        e.write_configstring(i, value);
    }
    fn write_server_command(w: &mut MsgWriter, seq: i32, text: &str) {
        write_server_command(w, seq, &latin1_bytes(text));
    }
    /// `SV_Netchan_Transmit` (17.2): the ops and `svc_EOF` compressed.
    fn netchan_out(e: &mut Cod2Server, slot: usize, ops: Vec<u8>) -> Vec<Vec<u8>> {
        let core = &mut e.core;
        let c = core.clients[slot].as_mut().unwrap();
        c.link
            .netchan
            .transmit_eof(c.link.last_client_command, &ops, Svc::Eof as u8, &core.huff)
    }
    fn exec(e: &mut Cod2Server, line: &str, out: &mut crate::rcon::Redirect) {
        e.exec(line, out);
    }

    const PROTOCOL: i32 = vcod_cod2::net::ops::PROTOCOL;
    const VERSION: &'static str = "1.3";
    const FF_KC: [&'static str; 2] = ["scr_friendlyfire", "scr_killcam"];
    fn names(e: &Cod2Server) -> [&str; 3] {
        let cfg = &e.game.cfg;
        [&cfg.hostname, &cfg.map, &cfg.gametype]
    }
    fn pure(e: &Cod2Server) -> bool {
        e.pure()
    }
    fn modded(e: &Cod2Server) -> bool {
        e.modded()
    }
    /// The public slots plus the private ones in use (17.1).
    fn shown_slots(e: &Cod2Server) -> usize {
        let private = (e.int("sv_privateClients").max(0) as usize).min(e.core.clients.len());
        let private_used = e.core.clients[..private]
            .iter()
            .flatten()
            .filter(|c| c.state >= State::Connected)
            .count();
        e.core.clients.len() - (private - private_used)
    }
    fn status_name(c: &Client) -> String {
        vcod_common::net::info_value_for_key(&c.link.userinfo, "name")
            .unwrap_or("")
            .to_string()
    }
    /// `password` against `sv_privatePassword` (`strcmp`, so an empty one
    /// matches an empty one) starts at 0, else past the private slots.
    fn first_slot(e: &Cod2Server, password: &str) -> usize {
        if password == e.game.cvars.get("sv_privatePassword") {
            0
        } else {
            e.int("sv_privateClients").max(0) as usize
        }
    }
    /// `ClientConnect` (0x80f8e7a): `g_password` unless empty or `none`,
    /// compared case-sensitively.
    fn password_refused(e: &Cod2Server, password: &str) -> bool {
        let gp = e.game.cvars.get("g_password");
        !gp.is_empty() && !gp.eq_ignore_ascii_case("none") && gp != password
    }
    fn free_level_slot(e: &mut Cod2Server, slot: usize) {
        if let Some(level) = e.game.level.as_mut() {
            level.disconnect(slot);
        }
    }
    fn find_player(
        e: &Cod2Server,
        by_name: bool,
        who: &str,
        out: &mut crate::rcon::Redirect,
    ) -> Option<usize> {
        if by_name {
            e.player_by_name(who, out)
        } else {
            e.player_by_num(who, out)
        }
    }
    fn ban_client(e: &mut Cod2Server, slot: usize, out: &mut crate::rcon::Redirect) {
        e.ban_client(slot, out);
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
                // Retail beats one constant master (0x8096ed6); vcod adds
                // `sv_master1..5` (`Masters::sync_cvars`).
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
                retired: None,
                snapshot_world: None,
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
                fatal: None,
                game_log: Default::default(),
                game_seed: None,
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
        self.game.server_id = conn::spawn_server_id(self.game.server_id);
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
        let p = crate::tick_profile::phase("load/baselines and pure");
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
        drop(p);
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

    /// Configstrings 0 and 1: `Dvar_InfoString(0x404)` and
    /// `Dvar_InfoString_Big(8)` (cod2-protocol.md 6, 9, 11.3).
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

    /// Test-facing: every level's `G_InitGame` seeds `srand` and the
    /// script's draws with `seed`, as retail's took its wall clock's
    /// milliseconds (cod2-combat.md 9.6).
    pub fn pin_game_seed(&mut self, seed: u32) {
        self.game.game_seed = Some(seed);
    }

    /// The seed of the next level's `G_InitGame`.
    pub(crate) fn game_seed(&mut self) -> u32 {
        match self.game.game_seed {
            Some(s) => s,
            None => self.rand() as u32,
        }
    }

    /// The loop's socket and wait for `SV_SpawnServer`'s send and sleep.
    pub fn set_net_now(&mut self, net: NetNow) {
        self.game.net_now = Some(net);
    }

    /// `SV_MasterHeartbeat(text)` at `dedicated 2`; the flatline ignores
    /// the timer.
    fn heartbeat(&mut self, flatline: bool) {
        let dedicated = self.game.cvars.get("dedicated").parse().unwrap_or(0);
        let cvars = &self.game.cvars;
        self.core
            .masters
            .sync_cvars(vcod_common::net::master::COD2_MASTER.host, |n| cvars.get(n));
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
        if self.game.retired.is_some() {
            let _p = crate::tick_profile::phase("drop level");
            self.game.retired = None;
        }
        let _p = crate::tick_profile::phase("console");
        self.run_console();
    }

    fn client_packet(&mut self, from: SocketAddr, pkt: &[u8], now: Instant) {
        let (slot, m) = match conn::incoming(&self.core, from, pkt) {
            conn::Incoming::Message(slot, m) => (slot, m),
            // `SV_PacketEvent` (0x80960e2): a sender no slot holds is told
            // to go away (17.3).
            conn::Incoming::NoSlot => return self.send_oob(from, "disconnect"),
            conn::Incoming::Dropped => return,
        };
        let svs = self.core.sv_time_ms;
        let ops = self.core.huff.decompress_block(&m.ops);
        let id = i32::from(m.server_id);
        let c = self.core.clients[slot].as_ref().unwrap();
        // A downloading client's messages are read whatever their serverId
        // (`SV_ExecuteClientMessage` 0x809114e).
        let stale = id != self.game.server_id && c.extra.download.name.is_empty();
        if c.state != State::Zombie && !stale && !self.reads_to_end(slot, &m, &ops) {
            log::debug!("cod2: client {slot}: message from {from} does not read");
            return;
        }
        let c = self.core.clients[slot].as_mut().unwrap();
        conn::commit(c, from, &m);
        // A zombie's packets go through the netchan and nothing else: its
        // `lastPacketTime` stands, so it is freed `sv_zombietime` after the
        // last one before the drop.
        if c.state == State::Zombie {
            return;
        }
        c.extra.last_heard = now;
        c.link.last_packet_ms = svs;
        if stale {
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

    /// Whether a message of the running level reads to its end: its
    /// commands, then `clc_EOF`, or a move whose cmds all read. Retail
    /// runs each op as it reads it; vcod reads the whole block before the
    /// netchan takes the message, as CoD1's server does, so a forged or
    /// garbled one runs nothing. A move's cmds read the same off any base
    /// (the key alone steers the bits), so the commands ahead of it, which
    /// may move the playerstate, need not run first.
    fn reads_to_end(&self, slot: usize, m: &ClientMessage, ops: &[u8]) -> bool {
        let mut r = MsgReader::from_plain(ops.to_vec());
        loop {
            let op = r.read_bits(CLC_BITS);
            if r.is_overflowed() {
                return false;
            }
            match op {
                x if x == Clc::ClientCommand as i32 => {
                    r.read_long();
                    r.read_string_bytes();
                }
                x if x == Clc::Eof as i32 => return true,
                x if x == Clc::Move as i32 || x == Clc::MoveNoDelta as i32 => {
                    let count = usize::from(r.read_byte());
                    if count == 0 || count > MAX_PACKET_USERCMDS {
                        return false;
                    }
                    let key = self.move_key(slot, m.message_ack, m.reliable_ack);
                    let mut from = self.move_base(slot);
                    for _ in 0..count {
                        from = read_delta_usercmd(&mut r, key, &from);
                    }
                    return !r.is_overflowed();
                }
                _ => return false,
            }
        }
    }

    /// The key a move's cmds are read with: the feed, the message it
    /// acknowledges and the server command it last saw.
    fn move_key(&self, slot: usize, message_ack: i32, reliable_ack: i32) -> u32 {
        let c = self.core.clients[slot].as_ref().unwrap();
        let key_cmd = latin1_bytes(
            &c.link.netchan.reliable[reliable_ack as usize & (MAX_RELIABLE_COMMANDS - 1)],
        );
        (self.core.checksum_feed ^ message_ack ^ com_hash_key_bytes(&key_cmd, 32)) as u32
    }

    /// The cmd a move's first one is a delta from, off the playerstate.
    fn move_base(&self, slot: usize) -> UserCmd2 {
        let c = self.core.clients[slot].as_ref().unwrap();
        let base = match self.game.level.as_ref() {
            Some(level) => level
                .wire_playerstate(slot, 0)
                .unwrap_or_else(|| spectator_wire(&c.extra.ps, slot)),
            None => spectator_wire(&c.extra.ps, slot),
        };
        default_usercmd(&base)
    }

    /// One `clc_clientCommand`. False when the client was dropped.
    fn client_command(&mut self, slot: usize, seq: i32, text: String) -> bool {
        match self.take_client_command(slot, seq, &text) {
            conn::CommandSeq::Old => return true,
            conn::CommandSeq::Lost => return false,
            conn::CommandSeq::Next => {}
        }
        log::debug!("cod2: client {slot} command {seq} {text:?}");
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
            "vdr" | "download" | "nextdl" | "stopdl" | "donedl" | "retransdl" | "wwwdl" => {
                self.download_command(slot, &args)
            }
            // `SV_MutePlayer_f` (0x8090a5a), `SV_UnmutePlayer_f` (0x8090aaa).
            m @ ("muteplayer" | "unmuteplayer") => {
                let arg = args.get(1).map_or("", String::as_str);
                c.voice.mute_command(arg, m == "muteplayer");
            }
            _ if game_ok => self.game_command(slot, &args, &text),
            _ => log::debug!("cod2: client {slot}: text ignored: {cmd}"),
        }
        self.core.clients[slot]
            .as_ref()
            .is_some_and(|c| c.state != State::Zombie)
    }

    /// The engine's `vdr` and download commands (`.data` 0x817d9e0,
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
            "download" => c.extra.download.begin(arg(1)),
            "nextdl" => {
                if !c.extra.download.next_dl(atoi(arg(1)), svs) {
                    self.drop_client(slot, "broken download");
                }
            }
            "stopdl" => {
                if !c.extra.download.name.is_empty() {
                    log::info!(
                        "clientDownload: {slot} : file {:?} aborted",
                        c.extra.download.name
                    );
                }
                c.extra.download.close();
            }
            // One gamestate a message turn, however many `donedl`s ask.
            "donedl" if conn::gamestate_last(c) => {}
            "donedl" => self.send_gamestate(slot),
            "retransdl" => c.extra.download.retransmit(atoi(arg(1))),
            "wwwdl" => match c.extra.download.wwwdl(arg(1)) {
                house::Www::None => {}
                house::Www::Gamestate if conn::gamestate_last(c) => {}
                house::Www::Gamestate => self.send_gamestate(slot),
                house::Www::Drop(why) => self.drop_client(slot, why),
            },
            _ => {}
        }
    }

    /// `SV_UserinfoChanged` (0x80907cc): the rate, the snapshot interval,
    /// `sendVoice` off the rate and `cl_voice`, and the www flag.
    fn userinfo_changed(&mut self, slot: usize) {
        let dedicated = self.int("dedicated");
        if let Some(c) = self.core.clients[slot].as_mut() {
            c.rates = link::Rates::of(&c.link.userinfo, c.lan, dedicated);
            c.voice.send = crate::voice::wants(&c.link.userinfo, c.rates.rate);
        }
    }

    /// `ClientCommand` (Mac 0x1a7684): `say`, `say_team` and `score`
    /// always; at the intermission nothing else; then `mr`, `kill`, the
    /// votes and the rest (`Cod2Level::client_command`), in retail's
    /// compare order (cod2-protocol.md 3.4.1).
    fn game_command(&mut self, slot: usize, args: &[String], text: &str) {
        let svs = self.core.sv_time_ms;
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
    /// then each cmd through the level's `ClientThink`
    /// (`Cod2Level::client_think`).
    fn user_move(&mut self, slot: usize, r: &mut MsgReader, message_ack: i32, delta: bool) {
        crate::tick_trace::step!("SV_UserMove slot {slot}");
        let pure = self.pure();
        let c = self.core.clients[slot].as_mut().unwrap();
        c.delta_message = if delta { message_ack } else { -1 };
        let count = usize::from(r.read_byte());
        if count == 0 || count > MAX_PACKET_USERCMDS {
            return;
        }
        let reliable_ack = c.link.reliable_ack;
        let key = self.move_key(slot, message_ack, reliable_ack);
        let mut from = self.move_base(slot);
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
        // `SV_ClientThink` (0x8090dac) stores the cmd and runs `ClientThink`
        // only for an active client (cod2-protocol.md 14.1).
        let active = c.state == State::Active;
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
            if !active {
                continue;
            }
            match self.game.level.as_mut() {
                Some(level) => level.client_think(slot, &cmd),
                // A bare server: the cmds advance the clock and nothing else.
                None => c.extra.ps.command_time = c.extra.ps.command_time.max(cmd.server_time),
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
        conn::write_pending_commands::<Cod2Game>(&mut w, c);
        PROTOCOL_118.write_gamestate(
            &mut w,
            &Gamestate2 {
                server_command_sequence: c.link.netchan.reliable_sequence as i32,
                ..gs
            },
        );
        self.transmit_gamestate(slot, w.into_ops());
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
        c.extra.download.close();
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
        if self.core.clients[slot].is_some() {
            let w = if localized {
                format!("w \"{reason}\"")
            } else {
                format!("w \"{name}^7 {reason}\" PB")
            };
            self.send_server_command(slot, &w);
        }
        if self.client_count() == 0 {
            self.core.masters.force();
        }
        self.flush_game_commands();
    }

    /// `G_RunFrame` on the stepped clock, then `SV_AddTestClient` for the
    /// slots it connected.
    fn run_level_frame(&mut self) {
        self.game.snapshot_world = None;
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
        // The messages change no entity, so their world is this one.
        let world = match self.game.snapshot_world.take() {
            Some(world) => world,
            None => self.world_entities(),
        };
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

    /// A new message for `slot`, the roster and the world built on the
    /// first client that needs them.
    fn send_client_snapshot(&mut self, slot: usize, shared: &mut Cod2Snapshots) {
        let (clients, _, live) = shared.get_or_insert_with(|| {
            let clients = self.client_states();
            let world = self.world_entities();
            let live = self.prepare_cull(&world);
            (clients, world, live)
        });
        let ops = self.client_message(slot, clients, live);
        self.transmit(slot, ops);
        self.send_voice(slot);
    }

    /// One client's message now, outside the pacing: a drop for a bad `cp`
    /// sends the zombie its `w` at once.
    fn send_client_message(&mut self, slot: usize) {
        let clients = self.client_states();
        let world = self.world_entities();
        let live = self.prepare_cull(&world);
        let ops = self.client_message(slot, &clients, &live);
        self.transmit(slot, ops);
    }

    /// The live world as every client's cull reads it this round
    /// ([`crate::game::cod2_wire::Culled`]).
    fn prepare_cull(
        &self,
        world: &BTreeMap<u32, (EntityState, bool)>,
    ) -> Vec<crate::game::cod2_wire::Culled> {
        self.game.level.as_ref().map_or_else(Vec::new, |level| {
            let ents = world.iter().map(|(n, (e, b))| (*n, e, *b));
            level.prepare_cull(ents, level.level_time())
        })
    }

    /// `SV_SendClientSnapshot` (0x809adea): an active client or a zombie
    /// gets its server commands and a snapshot, any other just the
    /// acknowledge; everyone but a zombie its download's next blocks.
    fn client_message(
        &mut self,
        slot: usize,
        clients: &[ClientState2],
        live: &[crate::game::cod2_wire::Culled],
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
                    .unwrap_or_else(|| spectator_wire(&c.extra.ps, slot));
                let (entities, archived) = level.snapshot_entities(live, slot, &ps);
                (ps, entities, archived)
            }
            _ => (spectator_wire(&c.extra.ps, slot), Vec::new(), None),
        };
        let c = self.core.clients[slot].as_mut().unwrap();
        let mut w = MsgWriter::new(&self.core.huff);
        if in_world {
            conn::write_pending_commands::<Cod2Game>(&mut w, c);
            let to = Frame2 {
                server_time: svs,
                ps,
                entities,
                clients: archived.map_or_else(|| clients.to_vec(), |r| r.to_vec()),
            };
            conn::write_snapshot(
                &PROTOCOL_118,
                &mut w,
                c,
                to,
                self.game.snap_flags,
                &self.game.baselines,
            );
        }
        if c.state != State::Zombie {
            let budget =
                house::blocks_per_message(c.rates.send_rate(max_rate), c.rates.snapshot_msec);
            let open = |name: &str| house::open_download(&home, &base, name);
            c.extra
                .download
                .write(&mut w, svs, budget, &dl_cvars, c.rates.wants_www, &open);
        }
        c.link.ping_ring[c.link.netchan.outgoing_sequence as usize % PACKET_BACKUP] = (svs, -1);
        w.into_ops()
    }

    /// The frame's entities before any client's cull
    /// ([`Cod2Level::world_entities`]).
    fn world_entities(&mut self) -> BTreeMap<u32, (EntityState, bool)> {
        // `r.broadcastTime` is stamped on level time, so it expires on the
        // frame clock (cod2-protocol.md 15.7).
        let svs = self.core.sv_time_ms;
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
    /// with a level `ClientConnect` (`SV_DirectConnect` calls it, a
    /// reconnect's old client freed already).
    fn admit(&mut self, slot: usize, from: SocketAddr, qport: u16, challenge: i32, userinfo: &str) {
        let (origin, angles) = self.game.spot;
        if let Some(level) = self.game.level.as_mut() {
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
        Client::connected(
            link,
            svs,
            rates,
            lan,
            voice::client_voice(userinfo, rates.rate),
            Cod2Client {
                ps,
                last_heard: Instant::now(),
                download: house::Download::default(),
                test: false,
            },
        )
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
            c.extra.test = true;
            let Some(free) = self.core.clients.get_mut(slot) else {
                log::warn!("cod2: test client {name} past sv_maxclients in slot {slot}");
                continue;
            };
            *free = Some(c);
            log::info!("cod2: test client {name} in slot {slot}");
        }
    }

    /// Wall milliseconds since the server started, which only salt the
    /// challenge; `svs.time` is `core.sv_time_ms`.
    fn wall_ms(&self) -> i32 {
        if self.game.frame_clock {
            return self.core.sv_time_ms;
        }
        self.game.start.elapsed().as_millis() as i32
    }

    /// The challenge's salt on the frame clock instead of the wall's, so a harness
    /// that runs frames faster than real time sends the same bytes each
    /// run (`bot_match`'s `NETDIGEST`).
    #[cfg(any(test, feature = "testing"))]
    pub fn test_frame_clock(&mut self) {
        self.game.frame_clock = true;
    }
}

/// Whether a command is type 0, read off its letter: every caller of
/// `SV_SendServerCommand` (0x8094a10) and the game's
/// `SV_GameSendServerCommand` (0x80917aa) pushes the type beside the
/// letter it formats, one type a letter (cod2-protocol.md 3.4). Type 0 are
/// the game messages `c`, `e`, `f`, `g` and the `iPrintLn` whose letter a
/// player argument cost it (0x8110a60), the chat lines `h`, `i`, the
/// local sound `s`, the team scores `G`, `H` and the menu notify `I`.
fn ignorable(text: &str) -> bool {
    matches!(
        text.as_bytes().first(),
        Some(b'c' | b'e' | b'f' | b'g' | b'h' | b'i' | b's' | b'G' | b'H' | b'I' | b' ')
    )
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
pub(crate) fn intermission_spot(world: &MapWorld) -> (Vec3, [f32; 3]) {
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
