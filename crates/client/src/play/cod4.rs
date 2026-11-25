//! A CoD4 1.7 session the client watches or plays, renderer-free: a
//! committed datagram capture replayed on its own clock, or a live join of
//! a server (`vcod_cod4::net::conn`) that answers its script menus
//! (`play::cod4_menus`) and sends a cmd a frame off the keys
//! (`play::cod4_input`). Each step reads what is due,
//! walks the new snapshots for their events (the playerstate's ring, the
//! entities' rings, the temp entities as they enter) and the obituaries;
//! [`Cod4Session::scene`] is the view and the entities to draw, lerped
//! between the two newest snapshots. `cod4_client` puts it on screen; the
//! replay gate (`tests`) runs the same steps headless.
//!
//! Our own body, or a free spectator's view, is predicted
//! (`play::cod4_predict`) once [`Cod4Session::set_world`] hands in the
//! level's clip map; a followed view, and any view before that, is the
//! snapshots' playerstate, lerped. Players and corpses carry what posing
//! them needs ([`BodyView`]); `play::cod4_players` poses them.

use anyhow::{Context, Result};
use glam::Vec3;
use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};
use vcod_cod4::net::capture::{self, Item, Line};
use vcod_cod4::net::clmsg::{ClOp, UserCmd};
use vcod_cod4::net::conn::Conn;
use vcod_cod4::net::delta::{ClientState, EntityState, PlayerState};
use vcod_cod4::net::fields::CLIENT_STATE;
use vcod_cod4::net::parse::{Client, Gamestate, Op, Snapshot};
use vcod_common::net::Msec;

use crate::cod4_fly::EntityAssets;
use crate::play::clock::ServerClock;
use crate::play::cod4_input::Cod4Input;
use crate::play::cod4_kick;
use crate::play::cod4_menus::ScriptMenus;
use crate::play::cod4_predict::{self, Cod4Predictor, Cod4World, Ring, Solid};
use crate::play::cod4_profile;
use crate::play::cod4_third_person;
use std::collections::VecDeque;
use std::sync::Arc;
use vcod_cod4::pmove::weapon::WeaponDef;
use vcod_cod4::pmove::{self as pm, Cmd};

mod cmd;
mod demo;
mod events;
mod join;
mod scene;

use cmd::Selector;
use demo::DemoPlay;
use events::source_client;
pub use events::{Fired, angles_at, origin_of, position_at, yaw_at};
pub use join::{Fetch, localize_drop, localize_refusal, probe};
use join::{drop_reason, is_map_restart};
pub use scene::{BodyView, CrosshairHit, Drawn, Scene, VehicleView, Visual, weapon_name};

/// `CS_SERVERINFO`.
const CS_SERVERINFO: u16 = 0;
/// `n\<alias>\t\<time>`, the map's ambient track (cod4-handshake.md 5).
pub const CS_AMBIENT: u16 = 821;
/// Model `n` is configstring `CS_MODELS + n` (cod4-handshake.md 5).
pub const CS_MODELS: u16 = 830;
const MAX_MODELS: u16 = 512;
/// The weapon list: weapon `n` is its `n`th name, 1-based (0 is none).
pub const CS_WEAPONS: u16 = 2258;
/// `weapInventoryType_t`'s alt mode (`WeaponDef` +316 = 3, the test at
/// `iw3mp.exe` 0x4575c6).
const INVENTORY_ALT_MODE: i32 = 3;
/// An attachment's tag is configstring `CS_TAGS + attachTagIndex`
/// (cod4-player-anims.md 5).
pub const CS_TAGS: u16 = 2282;
/// `eType`s (cod4-netfields.md 1).
pub const ET_GENERAL: u32 = 0;
pub const ET_PLAYER: u32 = 1;
pub const ET_PLAYER_CORPSE: u32 = 2;
pub const ET_ITEM: u32 = 3;
pub const ET_MISSILE: u32 = 4;
pub const ET_SCRIPTMOVER: u32 = 6;
/// `ET_FX` (`spawnFx` / `triggerFx`) and `ET_LOOP_FX` (`playLoopedFx`)
/// (cod4-effects.md 11).
pub const ET_FX: u32 = 8;
pub const ET_LOOP_FX: u32 = 9;
/// A mounted gun (the stock maps' SAWs, cod4-gsc.md 9.2).
pub const ET_MG42: u32 = 11;
/// The hardpoint vehicles: a helicopter and an airstrike's jet
/// (cod4-client-view.md 11).
pub const ET_HELICOPTER: u32 = 12;
pub const ET_PLANE: u32 = 13;
/// A vehicle: its own row of 1.7's per-eType table (`cod4_lnxded`
/// 0x8228be0, the vehicle table at 0x8225f80, cod4-netfields.md 1). The
/// name is INFERRED.
pub const ET_VEHICLE: u32 = 14;
pub const ET_EVENTS: u32 = 17;
use vcod_cod4::net::flags::SOLID_BMODEL;
pub const EV_OBITUARY: u32 = 66;
/// `EF_TELEPORT_BIT`: toggled, the entity or view starts over instead of
/// lerping (`CG_SetNextSnap`, cod4-client-view.md 2).
const EF_TELEPORT: u32 = 2;
/// `eFlags` 0x40 on a mounted gun: its trigger held, a round out
/// (cod4-gsc.md 9.2).
const EF_FIRING: u32 = 0x40;

/// An entity field by its table name (every table keeps a name's offset).
pub fn ent_u32(e: &EntityState, name: &str) -> u32 {
    EntityState::table(e.etype())
        .iter()
        .find(|f| f.name == name)
        .map_or(0, |f| e.0[f.word()])
}

pub fn ent_f32(e: &EntityState, name: &str) -> f32 {
    f32::from_bits(ent_u32(e, name))
}

pub(crate) fn client_u32(c: &ClientState, name: &str) -> u32 {
    CLIENT_STATE
        .iter()
        .find(|f| f.name == name)
        .map_or(0, |f| c.0[f.word()])
}

/// `cgs.clientinfo[client].perks` off the snapshot's client states. A
/// client number outside 0..63 reads client 0's, as `CG_EntityEvent`'s
/// clamp does (`iw3mp.exe` 0x435dc7..0x435dce).
pub fn client_perks(s: &Snapshot, client: u32) -> u32 {
    let client = if client < 64 { client } else { 0 };
    s.clients
        .iter()
        .find(|c| c.0[0] == client)
        .map_or(0, |c| client_u32(c, "perks"))
}

/// A client state's `team` (`team_t`: 0 free, 1 axis, 2 allies, 3
/// spectator).
pub fn client_team(c: &ClientState) -> i32 {
    client_u32(c, "team") as i32
}

/// A client's name: `name[0..16]`, NUL-ended.
pub fn client_name(c: &ClientState) -> String {
    let bytes: Vec<u8> = (0..4)
        .flat_map(|k| client_u32(c, &format!("name[{}]", 4 * k)).to_le_bytes())
        .take_while(|&b| b != 0)
        .collect();
    String::from_utf8_lossy(&bytes).into_owned()
}

fn ps_f32(ps: &PlayerState, name: &str) -> f32 {
    ps.get_f32(name).unwrap_or(0.0)
}

fn vec3(f: impl Fn(usize) -> f32) -> Vec3 {
    Vec3::new(f(0), f(1), f(2))
}

/// An info string's value (`\key\value\...`). Unlike common's
/// `info_value_for_key` the key matches case-blind and the bytes read lossy.
pub fn info_value(info: &[u8], key: &str) -> Option<String> {
    let s = String::from_utf8_lossy(info);
    let parts: Vec<&str> = s.split('\\').collect();
    parts
        .iter()
        .skip(1)
        .step_by(2)
        .zip(parts.iter().skip(2).step_by(2))
        .find(|(k, _)| k.eq_ignore_ascii_case(key))
        .map(|(_, v)| v.to_string())
}

/// The snapshots a live session keeps once it sheds (one minute at
/// `sv_fps` 20); it sheds at twice that.
const KEEP_SNAPSHOTS: usize = 1200;

enum Source {
    Capture {
        lines: Vec<Line>,
        next: usize,
        session: Box<capture::Session>,
        /// The capture ms that maps to the session's 0.
        start_ms: u64,
        /// Every cmd its client sent, by time: what it pressed when
        /// (the prediction's input).
        cmds: Vec<Cmd>,
    },
    Live {
        conn: Box<Conn>,
        /// When the last sequenced message came (`clc.lastPacketTime`).
        last_msg: Instant,
    },
    /// A `.dm_1` played back (cod4-demos.md 5).
    Demo(Box<DemoPlay>),
}

/// `cl_timeout`'s default, 40 s (`CL_Init`, `iw3mp.exe` 0x46d75d: 40.0 in
/// 0..3600; cod4-handshake.md).
pub const TIMEOUT: Duration = Duration::from_secs(40);
/// `CL_CheckTimeout` (0x46c590) drops on the sixth frame in a row past
/// `cl_timeout` (`cmp eax,5; jle` at 0x46c63a).
const TIMEOUT_FRAMES: u32 = 6;
/// `CL_DisconnectPacket` (0x46b510) honours an out-of-band `disconnect`
/// only this long past the last sequenced message (`cmp ecx,0xbb8`).
const OOB_DISCONNECT_QUIET: Duration = Duration::from_millis(3000);

pub struct Cod4Session {
    source: Source,
    /// The active profile's stats file, written back as the stats change
    /// (`cod4_profile`); `None` without a profile.
    profile: Option<super::cod4_profile::ProfileStats>,
    /// Snapshots already walked for events.
    walked: usize,
    /// What has fired of our own playerstate's ring, off the snapshots or
    /// the prediction.
    ps_ring: Option<Ring>,
    /// The level's weapon defs by number, for the prediction's weapon
    /// half, and the weapon list they were built off.
    level_defs: Option<(String, Vec<WeaponDef>)>,
    /// The last scene's solid entities but our own, which the client-side
    /// rounds meet (`cod4_fx::ShotWorld`).
    pub shot_solids: Vec<Solid>,
    /// Each legs anim's movetype bits (`Cod4Players::legs_movetypes`),
    /// which another player's spread reads; without them his `eFlags`
    /// stance stands in, never sighted.
    pub legs_movetypes: Option<Arc<[u64]>>,
    rings: HashMap<u32, u32>,
    /// Event entities in the last walked snapshot.
    temps: HashSet<u32>,
    fired: Vec<Fired>,
    lines: Vec<String>,
    /// `cl.serverTimeDelta` and its adjusts (cod4-client-view.md 1).
    clock: ServerClock,
    /// `cl.serverTime` this frame, the time drawn; `None` before a
    /// snapshot.
    cg_time: Option<i32>,
    /// The view live cmds carry (pitch, yaw), whole: the cmd's angles are
    /// it less the playerstate's `delta_angles`.
    pub view: [f32; 2],
    /// The server turned our view (a spawn, `setPlayerAngles`): [`Self::view`]
    /// moved to the playerstate's, for the camera to take.
    view_jump: bool,
    /// The `delta_angles` (pitch, yaw) of the newest own playerstate.
    own_delta: Option<[f32; 2]>,
    /// The keys live cmds carry (`play::cod4_input`).
    pub input: Cod4Input,
    /// The server's script menus and their answers.
    pub menus: ScriptMenus,
    /// A menu or the console holds the keys: cmds carry only that bit.
    pub talking: bool,
    /// Voice chat's heard frames, talkers and outgoing batch
    /// (`crate::voice`).
    pub voice: crate::voice::VoiceLink,
    /// Cmds built since the last packet.
    pending: Vec<UserCmd>,
    /// A capture's newest cmd angles: its client's own view, which a
    /// snapshot leaves out unless the view is locked
    /// (cod4-netfields.md 7).
    cmd_angles: Option<[u16; 3]>,
    /// The newest server command read off the client's queue, and those
    /// read since the last [`Cod4Session::take_commands`].
    cmd_seq: i32,
    commands: Vec<Vec<u8>>,
    /// What prediction moves through; `None` draws the snapshots.
    world: Option<Arc<Cod4World>>,
    pub predictor: Cod4Predictor,
    /// A live session's cmds as sent, oldest first, at most `CMD_BACKUP`
    /// (a capture's are its source's).
    sent_cmds: VecDeque<Cmd>,
    /// Session ms the newest snapshot came in, and this step's.
    snap_at_ms: f64,
    now_ms: f64,
    /// The cmd clock's offset off session ms, slewed
    /// ([`Self::slew_cmd_clock`]), and the last time it read.
    cmd_delta: Option<f64>,
    cmd_last: i32,
    /// Snapshots the cmd clock has slewed on.
    cmd_snaps: usize,
    /// Gamestates read since the join, and the count the level last took
    /// ([`Self::take_map_change`]); the map the world was loaded for.
    gamestates: u32,
    level_gamestates: u32,
    loaded_map: Option<String>,
    /// `cl_timeout`: no message this long (six frames in a row) drops the
    /// session.
    pub timeout: Duration,
    timeout_frames: u32,
    /// The server dropped us (a `w`, an out-of-band `disconnect`) or went
    /// silent: the `EXE_*` reason, until [`Self::take_dropped`].
    dropped: Option<String>,
    /// The systeminfo last handed out by [`Self::take_systeminfo`].
    systeminfo_seen: Option<Vec<u8>>,
    /// Our predicted body's EV_NOAMMO this frame: the weapon and ammo it
    /// ran dry with, for `CG_OutOfAmmoChange` ([`Self::out_of_ammo_change`]).
    ran_dry: Option<Dry>,
    /// The location selector (`beginLocationSelection`, cod4-hud.md 15.8).
    pub selector: Selector,
    /// `record`'s file, while one is open (cod4-demos.md 1).
    recording: Option<vcod_cod4::net::demo::Recording>,
    /// The last predicted playerstate, which the archive copies.
    last_predicted: Option<pm::PlayerState>,
    /// A live packet is due: the scene sends it once its prediction has
    /// run this frame's cmd ([`Self::send_packet`]), else the next step.
    packet_due: bool,
    /// The recoil and the landing dip (`play::cod4_kick`), and how many of
    /// [`Self::fired`] they have taken.
    pub kick: cod4_kick::Cod4Kick,
    kick_seen: usize,
    /// The snapshots a live session keeps when it sheds
    /// ([`KEEP_SNAPSHOTS`]; a test sheds sooner).
    keep_snapshots: usize,
    /// The `physicsLaunch`ed parts this client flies (`play::cod4_physics`).
    pub physics: super::cod4_physics::Cod4Physics,
    /// The map's dynamic entities, made with the world and again by a
    /// new gamestate or `CG_MapRestart` (cod4-world.md 13).
    pub dynents: Option<vcod_cod4::dynent::DynEnts>,
    /// The third-person dvars the next [`Self::scene`] reads.
    pub third_person: cod4_third_person::Dvars,
}

/// What `CG_OutOfAmmoChange` reads of the predicted playerstate.
#[derive(Clone, Debug)]
struct Dry {
    pm_type: i32,
    weapon: u32,
    held: [u32; 4],
    ammo: [i32; 64],
    ammoclip: [i32; 128],
}

impl Dry {
    fn of(ps: &pm::PlayerState) -> Dry {
        Dry {
            pm_type: ps.pm_type,
            weapon: ps.weapon,
            held: ps.weapons,
            ammo: ps.ammo,
            ammoclip: ps.ammoclip,
        }
    }
}

impl Cod4Session {
    /// A capture (`examples/cod4_probe.rs`'s format) read up to its
    /// gamestate; its clock starts there.
    pub fn open_capture(text: &str) -> Result<Cod4Session> {
        let lines = capture::parse(text)?;
        let mut session = Box::new(capture::Session::new());
        let mut next = 0;
        let mut start_ms = None;
        while next < lines.len() && start_ms.is_none() {
            let l = &lines[next];
            next += 1;
            if let Some(Item::Server { parsed, .. }) = session.feed(l)?
                && parsed.ops.iter().any(|o| matches!(o, Op::Gamestate))
            {
                start_ms = Some(l.ms);
            }
        }
        let start_ms = start_ms.context("the capture carries no gamestate")?;
        let mut all = capture::Session::new();
        let mut cmds: Vec<Cmd> = Vec::new();
        for l in &lines {
            if let Ok(Some(Item::Client { msg, .. })) = all.feed(l) {
                for op in &msg.ops {
                    if let ClOp::Move { cmds: c, .. } = op {
                        cmds.extend(c.iter().map(|(c, _)| Cmd::from(c)));
                    }
                }
            }
        }
        cmds.sort_by_key(|c| c.server_time);
        cmds.dedup_by_key(|c| c.server_time);
        Ok(Self::new(Source::Capture {
            lines,
            next,
            session,
            start_ms,
            cmds,
        }))
    }

    fn new(source: Source) -> Self {
        Cod4Session {
            source,
            profile: None,
            walked: 0,
            ps_ring: None,
            level_defs: None,
            shot_solids: Vec::new(),
            legs_movetypes: None,
            rings: HashMap::new(),
            temps: HashSet::new(),
            fired: Vec::new(),
            lines: Vec::new(),
            clock: ServerClock::cod4(),
            cg_time: None,
            view: [0.0; 2],
            view_jump: false,
            own_delta: None,
            input: Cod4Input::default(),
            menus: ScriptMenus::default(),
            talking: false,
            voice: Default::default(),
            pending: Vec::new(),
            cmd_angles: None,
            cmd_seq: 0,
            commands: Vec::new(),
            world: None,
            predictor: Cod4Predictor::default(),
            sent_cmds: VecDeque::new(),
            snap_at_ms: 0.0,
            now_ms: 0.0,
            cmd_delta: None,
            cmd_last: i32::MIN,
            cmd_snaps: 0,
            gamestates: 0,
            level_gamestates: 0,
            loaded_map: None,
            ran_dry: None,
            timeout: TIMEOUT,
            timeout_frames: 0,
            dropped: None,
            systeminfo_seen: None,
            selector: Selector::default(),
            recording: None,
            last_predicted: None,
            packet_due: false,
            kick: Default::default(),
            kick_seen: 0,
            keep_snapshots: KEEP_SNAPSHOTS,
            physics: Default::default(),
            dynents: None,
            third_person: Default::default(),
        }
    }

    /// Why the server dropped us, once: a `w` command's reason, an
    /// out-of-band `disconnect`'s, or `EXE_ERR_SERVER_TIMEOUT` after
    /// `cl_timeout` of silence. The caller leaves the game with it.
    pub fn take_dropped(&mut self) -> Option<String> {
        self.dropped.take()
    }

    /// Configstring 1 once it differs from the last one taken, live only:
    /// the caller sets its dvars as `CL_SystemInfoChanged` (iw3mp.exe
    /// 0x473ab0) does, `sv_cheats` 0 putting every cheat dvar back first
    /// (cod4-client-perf.md 20). A demo sets none.
    pub fn take_systeminfo(&mut self) -> Option<String> {
        if !self.is_live() {
            return None;
        }
        let si = self.gamestate()?.configstring(1);
        if self.systeminfo_seen.as_deref() == Some(si) {
            return None;
        }
        let out = String::from_utf8_lossy(si).into_owned();
        self.systeminfo_seen = Some(si.to_vec());
        Some(out)
    }

    /// `CG_DrawDisconnect` (`iw3mp.exe` 0x42f930): the server has not run
    /// the oldest of the 128 kept cmds (`cmdNumber - 127`) though its time
    /// has come, `cg.snap->ps.commandTime < serverTime <= cg.time`:
    /// `Connection Interrupted` and the blinking `net_disconnect`. Never
    /// before 128 cmds were sent, as that slot still reads 0, nor in a
    /// demo or a capture.
    pub fn interrupted(&self) -> bool {
        if !self.is_live() || self.sent_cmds.len() < cod4_predict::CMD_BACKUP {
            return false;
        }
        let (Some(time), Some(oldest)) = (self.cg_time, self.sent_cmds.front()) else {
            return false;
        };
        let snaps = &self.client().snapshots;
        let Some(snap) = snaps
            .iter()
            .rposition(|s| s.server_time <= Msec::from_raw(time))
            .or((!snaps.is_empty()).then_some(0))
            .map(|i| &snaps[i])
        else {
            return false;
        };
        let command_time = snap.ps.word(vcod_cod4::ps_word!("commandTime")) as i32;
        command_time < oldest.server_time && oldest.server_time <= time
    }

    /// Gamestates read since the join: a new one starts the cgame's level
    /// state over.
    pub fn gamestate_count(&self) -> u32 {
        self.gamestates
    }

    #[cfg(test)]
    pub(crate) fn add_gamestate_for_test(&mut self) {
        self.gamestates += 1;
    }

    /// The map a new gamestate names when it is not the loaded one, once:
    /// the caller loads it (a map change, `loadingnewmap`, then the
    /// gamestate; cod4-map-cycle.md 6). Any new gamestate starts the
    /// level's own state over, the way `CL_ParseGamestate` restarts the
    /// cgame.
    pub fn take_map_change(&mut self) -> Option<String> {
        if self.gamestates == self.level_gamestates {
            return None;
        }
        self.level_gamestates = self.gamestates;
        self.walked = self.client().snapshots.len();
        self.ps_ring = None;
        self.level_defs = None;
        self.rings.clear();
        self.temps.clear();
        self.cg_time = None;
        self.own_delta = None;
        self.cmd_angles = None;
        self.pending.clear();
        self.sent_cmds.clear();
        self.cmd_last = i32::MIN;
        self.predictor.reset();
        self.restart_dynents();
        let map = self.map_name()?;
        if self.loaded_map.as_deref() == Some(map.as_str()) {
            log::info!("cod4: new gamestate for {map}, the map stays loaded");
            return None;
        }
        log::info!("cod4: map change {:?} -> {map}", self.loaded_map);
        self.loaded_map = Some(map.clone());
        Some(map)
    }

    /// The level's clip map and weapon scales: from here on our own body
    /// is predicted.
    pub fn set_world(&mut self, world: Arc<Cod4World>) {
        self.world = Some(world);
        self.predictor.reset();
        self.restart_dynents();
    }

    /// `DynEntCl_InitEntities` (iw3mp.exe 0x48d000, from `CG_Init` and
    /// `CG_MapRestart` 0x44b3eb): every dynamic entity back at its def's
    /// pose and health.
    pub fn restart_dynents(&mut self) {
        self.dynents = self.world.as_ref().map(|w| {
            let mut d = vcod_cod4::dynent::DynEnts::new(w.dynents.clone());
            d.pass_start_contents(|p| w.clip.point_contents(p));
            d
        });
    }

    /// The prediction world, when one was handed in.
    pub fn world(&self) -> Option<&Arc<Cod4World>> {
        self.world.as_ref()
    }

    /// A capture's cmd clock, the one its probe stamped its cmds on: the
    /// newest snapshot's time plus its age (a live session's is
    /// [`Self::cmd_time`]). Arrival jitter would step it with every snapshot, so its offset
    /// off session ms slews 1 ms a snapshot toward that, as
    /// `CL_AdjustTimeDelta` slews `cl.serverTimeDelta`, and starts over past
    /// 100 ms. Never runs back.
    fn slew_cmd_clock(&mut self, newest: i32, came: bool) {
        let target = f64::from(newest) - self.snap_at_ms;
        match &mut self.cmd_delta {
            Some(d) if (target - *d).abs() <= 100.0 => {
                if came {
                    *d += (target - *d).clamp(-1.0, 1.0);
                }
            }
            d => *d = Some(target),
        }
    }

    pub fn client(&self) -> &Client {
        source_client(&self.source)
    }

    pub fn gamestate(&self) -> Option<&Gamestate> {
        self.client().gamestate.as_ref()
    }

    pub fn is_live(&self) -> bool {
        matches!(self.source, Source::Live { .. })
    }

    /// `cl_packetdup` and `cl_maxpackets` for a live session's next
    /// packets.
    pub fn set_packet_rates(&mut self, dup: i32, max_packets: i32) {
        if let Source::Live { conn, .. } = &mut self.source {
            conn.set_packet_dup(dup);
            conn.set_max_packets(max_packets);
        }
    }

    /// A capture read to its end.
    pub fn finished(&self) -> bool {
        match &self.source {
            Source::Capture { lines, next, .. } => *next >= lines.len(),
            Source::Live { .. } => false,
            Source::Demo(d) => d.ended,
        }
    }

    /// The systeminfo's `sv_voice` and `sv_voiceQuality` (both are
    /// systeminfo dvars, so the client's copies are the server's;
    /// cod4-voice.md 4); off and 1 before a gamestate.
    pub fn voice_settings(&self) -> (bool, i32) {
        let Some(gs) = self.gamestate() else {
            return (false, vcod_cod4::net::voice::DEFAULT_QUALITY);
        };
        let si = gs.configstring(1);
        let int = |k: &str| info_value(si, k).map(|v| vcod_cod4::menuexp::atoi(&v));
        (
            int("sv_voice").unwrap_or(0) != 0,
            int("sv_voiceQuality").unwrap_or(vcod_cod4::net::voice::DEFAULT_QUALITY),
        )
    }

    /// `CL_IsPlayerTalking` at this step's clock: our own client while
    /// `+talk` is held (with `sv_voice` or `cl_voice` on), anyone else on
    /// a frame heard in the last 300 ms.
    pub fn is_talking(&self, client: u32) -> bool {
        if Some(client) == self.own_client() {
            let held = self.input.held(crate::play::input::Action::Talk);
            return crate::voice::own_talking(held, self.voice_settings().0, self.voice.enabled);
        }
        self.voice.talking(client, self.now_ms)
    }

    /// The talker list's clients (`UI_GetTalkerClientNum`): the talking
    /// ones but us, by number.
    pub fn talkers(&self) -> Vec<u32> {
        self.voice.talkers(self.own_client(), self.now_ms)
    }

    /// Our client number off the newest snapshot.
    pub fn own_client(&self) -> Option<u32> {
        Some(
            self.client()
                .snapshots
                .last()?
                .ps
                .word(vcod_cod4::ps_word!("clientNum")),
        )
    }

    /// The local player's `team_t` in the newest snapshot's client
    /// list (`clientinfo[clientNum].team`), 0 (free) without one.
    pub fn own_team(&self) -> i32 {
        let Some(snap) = self.client().snapshots.last() else {
            return 0;
        };
        let me = snap.ps.word(vcod_cod4::ps_word!("clientNum"));
        snap.clients
            .iter()
            .find(|c| c.0[0] == me)
            .map_or(0, |c| client_u32(c, "team") as i32)
    }

    /// The players the newest snapshot lists, (client number, name) by
    /// number, the named ones only (`UI_BuildPlayerList` over
    /// `CL_GetClientName`).
    pub fn players(&self) -> Vec<(u32, String)> {
        let Some(snap) = self.client().snapshots.last() else {
            return Vec::new();
        };
        let mut out: Vec<(u32, String)> = snap
            .clients
            .iter()
            .map(|c| (c.0[0], client_name(c)))
            .filter(|(_, n)| !n.is_empty())
            .collect();
        out.sort_by_key(|p| p.0);
        out
    }

    /// The serverinfo's `mapname`.
    pub fn map_name(&self) -> Option<String> {
        info_value(self.gamestate()?.configstring(CS_SERVERINFO), "mapname")
    }

    /// Serverinfo's `g_gametype`.
    pub fn gametype(&self) -> Option<String> {
        info_value(self.gamestate()?.configstring(CS_SERVERINFO), "g_gametype")
    }

    /// The server's `fs_game` (`mods/<name>`), off configstring 1; `None`
    /// when empty (cod4-handshake.md 9).
    pub fn fs_game(&self) -> Option<String> {
        info_value(self.gamestate()?.configstring(1), "fs_game").filter(|g| !g.is_empty())
    }

    /// The models and weapons the gamestate precached.
    pub fn entity_assets(&self) -> EntityAssets {
        let Some(gs) = self.gamestate() else {
            return EntityAssets::default();
        };
        EntityAssets {
            models: (1..MAX_MODELS)
                .map(|i| String::from_utf8_lossy(gs.configstring(CS_MODELS + i)).into_owned())
                .filter(|s| !s.is_empty())
                .collect(),
            weapons: weapon_list(gs).to_vec(),
        }
    }

    /// Reads what is due at session ms `now_ms` (a capture's lines up to
    /// it; a live server's queue, answered with cmds) and walks the new
    /// snapshots.
    pub fn step(&mut self, now_ms: f64) -> Result<()> {
        let upload = self.menus.take_upload_stats();
        self.save_profile(upload);
        let now = clock_now();
        match &mut self.source {
            Source::Capture {
                lines,
                next,
                session,
                start_ms,
                ..
            } => {
                while *next < lines.len() && (lines[*next].ms as f64) <= *start_ms as f64 + now_ms {
                    let line = &lines[*next];
                    *next += 1;
                    let snaps = session.client.snapshots.len();
                    // A line that does not decode is skipped, as a live
                    // illegible message is.
                    match session.feed(line) {
                        Ok(Some(Item::Client { msg, .. })) => {
                            let last = msg.ops.iter().rev().find_map(|o| match o {
                                ClOp::Move { cmds, .. } => cmds.last().map(|(c, _)| c.angles),
                                _ => None,
                            });
                            self.cmd_angles = last.or(self.cmd_angles);
                        }
                        Ok(_) => {}
                        Err(e) => log::warn!("cod4: capture line at {} ms: {e:#}", line.ms),
                    }
                    if session.client.snapshots.len() > snaps {
                        self.snap_at_ms = line.ms.saturating_sub(*start_ms) as f64;
                    }
                }
            }
            Source::Live { conn, last_msg, .. } => {
                // The step after a gamestate runs once its level is loaded
                // (`Cod4Client::start_map_load`).
                conn.cgame_up()?;
                let got = conn.poll(now)?;
                if !got.is_empty() {
                    *last_msg = now;
                }
                // `CL_ConnectionlessPacket`'s `print` (rcon's answers) goes
                // to the console with the other lines.
                for text in conn.take_prints() {
                    let text = text.trim_end_matches('\n');
                    if !text.is_empty() {
                        self.lines.push(text.to_string());
                    }
                }
                let menus = &self.menus;
                self.voice
                    .hear(conn.take_voice(), now_ms, |c| menus.is_muted(c));
                if let Some(batch) = self.voice.due(now_ms) {
                    conn.send_voice(&batch)?;
                }
                // `CL_DisconnectPacket`: an out-of-band `disconnect`
                // past 3 s of silence, its reason or
                // `EXE_SERVER_DISCONNECTED`.
                if let Some(why) = conn.take_disconnect()
                    && now.saturating_duration_since(*last_msg) >= OOB_DISCONNECT_QUIET
                    && self.dropped.is_none()
                {
                    self.dropped = Some(match why.is_empty() {
                        true => "EXE_SERVER_DISCONNECTED".into(),
                        false => why,
                    });
                }
                // `CL_CheckTimeout` (0x46c590): six frames in a row past
                // `cl_timeout` without a sequenced message.
                if now.saturating_duration_since(*last_msg) > self.timeout {
                    self.timeout_frames += 1;
                    if self.timeout_frames >= TIMEOUT_FRAMES && self.dropped.is_none() {
                        self.dropped = Some("EXE_ERR_SERVER_TIMEOUT".into());
                    }
                } else {
                    self.timeout_frames = 0;
                }
                if let Some(rec) = self.recording.as_mut() {
                    // A CoD4x side channel gamestate has no netchan message.
                    for r in got.iter().filter(|r| !r.msg.is_empty()) {
                        // `CL_ParseSnapshot` clears `demowaiting` on a full
                        // frame (0x4737d5) before `CL_PacketEvent` writes.
                        if r.parsed.ops.iter().any(|o| {
                            matches!(o, Op::Snapshot(i) if conn.client.snapshots[*i].base.is_none())
                        }) {
                            rec.waiting = false;
                        }
                        if let Err(e) = rec.write(r.sequence as i32, &r.msg, &conn.client.archive) {
                            log::warn!("cod4: demo write: {e}");
                        }
                    }
                    conn.no_delta = rec.waiting;
                }
                for r in &got {
                    for op in &r.parsed.ops {
                        match op {
                            Op::Gamestate => {
                                self.gamestates += 1;
                                // `cl` is cleared: nothing goes until the
                                // level is up.
                                self.packet_due = false;
                                self.pending.clear();
                            }
                            Op::Snapshot(_) => self.snap_at_ms = now_ms,
                            Op::ServerCommand { text, .. } => {
                                log::debug!(
                                    "cod4: server command {}",
                                    String::from_utf8_lossy(text)
                                )
                            }
                            _ => {}
                        }
                    }
                }
            }
            Source::Demo(d) => self.gamestates = d.gamestates,
        }
        self.now_ms = now_ms;
        let c = self.client();
        let (n, newest) = (
            c.total_snapshots(),
            c.snapshots.last().map(|s| s.server_time),
        );
        if let Some(t) = newest {
            self.slew_cmd_clock(t.raw(), n > self.cmd_snaps);
        }
        self.cmd_snaps = n;
        // `CL_SetCGameTime` (iw3mp.exe 0x45c440): once a frame, the
        // delta adjusted when a snapshot came.
        if let Source::Demo(d) = &mut self.source {
            if let Some(t) = d.frame(now_ms.floor() as i32) {
                self.cg_time = Some(t);
            }
        } else if let Some(new) = self.client().snapshots.last() {
            let snaps = &self.client().snapshots;
            let prev = snaps
                .len()
                .checked_sub(2)
                .map(|i| snaps[i].server_time.raw());
            let n = self.client().total_snapshots() as u32;
            let t = self
                .clock
                .update_cod2(now_ms.floor() as i32, new.server_time.raw(), prev, n);
            self.cg_time = Some(t);
        }
        let new = self.queue_commands();
        for text in &new {
            if let Some(why) = drop_reason(text)
                && self.is_live()
                && self.dropped.is_none()
            {
                self.dropped = Some(why);
            }
        }
        if new.iter().any(|t| is_map_restart(t)) {
            self.restart_dynents();
        }
        if self.is_live() {
            self.live_frame(&new)?;
        }
        self.walk();
        // A live connection and a demo keep the snapshots they still draw,
        // delta and walk: with every one kept the soak's client grew 100 to
        // 250 MB a three-minute level (cod4-map-sweep.md 5).
        let client = match &mut self.source {
            Source::Live { conn, .. } => Some(&mut conn.client),
            Source::Demo(d) => Some(&mut d.client),
            Source::Capture { .. } => None,
        };
        if let Some(c) = client
            && c.snapshots.len() >= 2 * self.keep_snapshots
        {
            let n = c.shed(self.keep_snapshots);
            self.walked = self.walked.saturating_sub(n);
        }
        Ok(())
    }

    /// Esc with no script menu up: the server's `g_scriptMainMenu`.
    pub fn open_main_menu(&mut self) {
        // The source's gamestate borrowed beside the menus, not cloned.
        let Some(gs) = &source_client(&self.source).gamestate else {
            return;
        };
        for a in self.menus.open_main(gs) {
            self.command(&a);
        }
    }

    /// A key in the open script menu, its answers sent; false when the
    /// menu does nothing with it (or none is up).
    pub fn menu_key(&mut self, key: crate::play::cod4_menus::MenuKey) -> bool {
        let Some(gs) = &source_client(&self.source).gamestate else {
            return false;
        };
        let mut out = Vec::new();
        let taken = self.menus.key(key, gs, &mut out);
        for a in out {
            self.command(&a);
        }
        taken
    }

    /// The mouse over the open script menu: raw counts `d` or a position
    /// `at` on a `size` window, or (both `None`) MOUSE1; its answers sent.
    pub fn menu_mouse(&mut self, d: Option<[f32; 2]>, at: Option<[f32; 2]>, size: (f32, f32)) {
        let Some(gs) = &source_client(&self.source).gamestate else {
            return;
        };
        let out = if d.is_none() && at.is_none() {
            self.menus.click(size, gs)
        } else {
            self.menus.mouse_move(d, at, size, gs)
        };
        for a in out {
            self.command(&a);
        }
    }

    /// The profile's stats to its file once settled, or at once, and to
    /// the connection, whose next `requeststats` (a map change) sends them.
    fn save_profile(&mut self, now: bool) {
        let Some(stats) = self.menus.stats() else {
            return;
        };
        if let Some(p) = self.profile.as_mut() {
            p.save(stats, now);
        }
        if let Source::Live { conn, .. } = &mut self.source {
            conn.set_stats(stats);
        }
    }

    /// Queues a reliable client command on a live server; a capture
    /// sends nothing.
    pub fn command(&mut self, text: &str) {
        if let Source::Live { conn, .. } = &mut self.source {
            conn.command(text);
        }
    }

    /// A new `name` dvar: the userinfo goes again (`CL_CheckUserinfo`,
    /// [`Conn::set_name`]); a capture or a demo sends nothing.
    pub fn set_name(&mut self, name: &str) {
        if let Source::Live { conn, .. } = &mut self.source {
            conn.set_name(name);
        }
    }

    /// The reliable commands queued on a live connection so far.
    #[cfg(test)]
    fn source_commands(&self) -> usize {
        match &self.source {
            Source::Live { conn, .. } => conn.reliable_sequence() as usize,
            _ => 0,
        }
    }

    /// An out-of-band packet to the live server (`rcon`); a capture sends
    /// nothing.
    pub fn send_oob(&mut self, text: &[u8]) {
        if let Source::Live { conn, .. } = &mut self.source
            && let Err(e) = conn.send_oob(text)
        {
            log::warn!("cod4: rcon: {e:#}");
        }
    }

    /// Leaves a live server (a capture has nothing to leave). A recording
    /// stops first (`CL_Disconnect` runs `stoprecord`, 0x4696ef).
    pub fn disconnect(&mut self) {
        self.save_profile(true);
        if self.recording.is_some() {
            crate::console::log::print(&self.stop_recording());
        }
        if let Source::Live { conn, .. } = &mut self.source
            && let Err(e) = conn.disconnect()
        {
            log::warn!("cod4: disconnect: {e:#}");
        }
    }

    /// The events fired since the last call, oldest first.
    pub fn take_fired(&mut self) -> Vec<Fired> {
        self.kick_seen = 0;
        std::mem::take(&mut self.fired)
    }

    /// Console lines (obituaries, rcon's answers) since the last call.
    pub fn take_lines(&mut self) -> Vec<String> {
        std::mem::take(&mut self.lines)
    }

    /// The server commands since the last call, oldest first, each once
    /// (the reliable stream repeats a command until it is acked).
    pub fn take_commands(&mut self) -> Vec<Vec<u8>> {
        std::mem::take(&mut self.commands)
    }

    fn queue_commands(&mut self) -> Vec<Vec<u8>> {
        let c = self.client();
        let newest = c.server_command_sequence;
        // A gamestate restarts the count past the commands it carried.
        let from = match &c.gamestate {
            Some(gs) if self.cmd_seq > newest => gs.server_command_sequence,
            _ => self.cmd_seq,
        };
        // The commands up to the gamestate's sequence came with it: only
        // those received are walked. Past it, more than the netchan's ring
        // of reliable commands means the ones between are lost:
        // `EXE_ERR_RELIABLE_CYCLED_OUT`, not a walk of them.
        let base = c
            .gamestate
            .as_ref()
            .map_or(from, |gs| gs.server_command_sequence.max(from));
        let ring = vcod_common::net::netchan::NetchanParams::COD2.reliable_commands;
        if i64::from(newest) - i64::from(base) > ring as i64 {
            self.cmd_seq = newest;
            self.dropped
                .get_or_insert_with(|| "EXE_ERR_RELIABLE_CYCLED_OUT".into());
            return Vec::new();
        }
        let early = (from < base).then(|| c.server_commands.range(from + 1..=base));
        let got: Vec<Vec<u8>> = early
            .into_iter()
            .flatten()
            .map(|(_, t)| t.clone())
            .chain(
                (i64::from(base) + 1..=i64::from(newest))
                    .map(|q| c.server_command(q as i32).to_vec()),
            )
            .collect();
        self.cmd_seq = newest;
        self.commands.extend(got.iter().cloned());
        got
    }
}

/// The weapon list's names, index 0 standing for weapon 1.
pub fn weapon_list(gs: &Gamestate) -> std::sync::Arc<[String]> {
    // A frame asks for it a dozen times: the last split is kept, keyed by
    // the configstring's bytes.
    thread_local! {
        static LAST: std::cell::RefCell<(Vec<u8>, std::sync::Arc<[String]>)> =
            std::cell::RefCell::new((Vec::new(), std::sync::Arc::from([])));
    }
    let cs = gs.configstring(CS_WEAPONS);
    LAST.with_borrow_mut(|(bytes, list)| {
        if bytes.as_slice() != cs {
            *list = String::from_utf8_lossy(cs)
                .split_whitespace()
                .map(str::to_string)
                .collect();
            *bytes = cs.to_vec();
        }
        list.clone()
    })
}

/// Now for a live session's cmd clock, send pacing and timeouts: the
/// client's frame clock (`cls.realtime`, which `cl_avidemo` steps), or the
/// clock a live test drives faster than real time on this thread
/// (`tests::sim`).
pub(crate) fn clock_now() -> Instant {
    #[cfg(test)]
    if let Some(t) = tests::sim::now() {
        return t;
    }
    crate::frame_clock::now()
}

#[cfg(test)]
pub(crate) mod tests;
