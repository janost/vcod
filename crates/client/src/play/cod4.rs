//! A CoD4 1.7 session the client watches or plays, renderer-free: a
//! committed datagram capture replayed on its own clock, or a live join of
//! a server (`vcod_cod4::net::conn`) that answers its script menus
//! (`play::cod4_menus`) and sends a cmd a frame off the keys
//! (`play::cod4_input`). Each step reads what is due,
//! walks the new snapshots for their events (the playerstate's ring, the
//! entities' rings, the temp entities as they enter) and the obituaries;
//! [`Cod4Session::scene`] is the view and the entities to draw, lerped
//! between the two newest snapshots. `cod4_client` puts it on screen; the
//! replay gate below runs the same steps headless.
//!
//! Our own body, or a free spectator's view, is predicted
//! (`play::cod4_predict`) once [`Cod4Session::set_world`] hands in the
//! level's clip map; a followed view, and any view before that, is the
//! snapshots' playerstate, lerped. Players and corpses carry what posing
//! them needs ([`BodyView`]); `play::cod4_players` poses them.

use anyhow::{Context, Result, bail};
use glam::Vec3;
use std::collections::{HashMap, HashSet};
use std::net::SocketAddr;
use std::time::{Duration, Instant};
use vcod_cod4::net::capture::{self, Item, Line};
use vcod_cod4::net::clmsg::{ClOp, UserCmd};
use vcod_cod4::net::conn::{Conn, Cp, Join, angle2short};
use vcod_cod4::net::delta::{ClientState, EntityState, PlayerState};
use vcod_cod4::net::download::DownloadProgress;
use vcod_cod4::net::events::event_name;
use vcod_cod4::net::fields::CLIENT_STATE;
use vcod_cod4::net::parse::{Client, Gamestate, Op, Snapshot};

use crate::cod4_fly::EntityAssets;
use crate::play::clock::ServerClock;
use crate::play::cod4_input::Cod4Input;
use crate::play::cod4_kick;
use crate::play::cod4_menus::{Join as MenuJoin, ScriptMenus};
use crate::play::cod4_predict::{self, Cod4Predictor, Cod4World, Ring, Solid};
use std::collections::VecDeque;
use std::sync::Arc;
use vcod_cod2::player::{BodyFrame, Character, eflags as cod2_eflags};
use vcod_cod2::trajectory::{self as tr2, Trajectory2};
use vcod_cod4::pmove::weapon::{WeaponDef, WeaponDefs};
use vcod_cod4::pmove::{self as pm, Cmd, WeaponMove};

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
pub const ET_EVENTS: u32 = 17;
/// A brush model's `solid`.
const SOLID_BMODEL: u32 = 0xff_ffff;
pub const EV_OBITUARY: u32 = 66;
/// `EF_TELEPORT_BIT`: toggled, the entity or view starts over instead of
/// lerping (`CG_SetNextSnap`, cod4-client-view.md 2).
const EF_TELEPORT: u32 = 2;
/// `eFlags` 0x40 on a mounted gun: its trigger held, a round out
/// (cod4-gsc.md 9.2).
const EF_FIRING: u32 = 0x40;
/// Live cmd packets go out at most this often, carrying every cmd built
/// since the last (`cl_maxpackets` 30).
const SEND_MS: u64 = 33;
/// The cmds one packet carries at most.
const MAX_PACKET_CMDS: usize = 32;

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

fn client_u32(c: &ClientState, name: &str) -> u32 {
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

/// An info string's value (`\key\value\...`).
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

/// One event fired this step.
#[derive(Clone, Debug, PartialEq)]
pub struct Fired {
    pub id: u32,
    pub name: &'static str,
    /// The entity it rode, `None` for the playerstate's ring.
    pub entity: Option<u32>,
    pub server_time: i32,
    /// The ring entry's `eventParms[k]`, or an event entity's `eventParm`.
    pub parm: u32,
    /// The entity's `lerp.pos.trBase`, or the playerstate's origin.
    pub origin: Vec3,
    /// The entity's state in the snapshot that carried the event; `None`
    /// for the playerstate's ring.
    pub state: Option<EntityState>,
    /// The entity's weapon, or the playerstate's.
    pub weapon: u32,
    /// The perks of the client the entity names ([`client_perks`]), or
    /// of the playerstate's client.
    pub perks: u32,
    /// A bullet shot's seed and spread (`cod4_fx::client_bullets`), set
    /// where the session knows them.
    pub aim: Option<crate::cod4_fx::Aim>,
}

/// `CG_EntityEvent`'s `EV_STANCE_FORCE_*` off our playerstate's ring
/// (`iw3mp.exe` 0x436155, 0x436195, 0x4361d5): `CL_SetStance` (0x462870)
/// makes the stance keys' state the forced one unless `+prone` or
/// `+movedown` is held, so a sprint out of a crouch stays standing
/// (cod4-pmove.md 8).
fn force_stance(input: &mut Cod4Input, fired: &[Fired]) {
    use crate::play::cod4_input::Stance;
    for f in fired.iter().filter(|f| f.entity.is_none()) {
        input.set_stance(match f.name {
            "EV_STANCE_FORCE_STAND" => Stance::Stand,
            "EV_STANCE_FORCE_CROUCH" => Stance::Crouch,
            "EV_STANCE_FORCE_PRONE" => Stance::Prone,
            _ => continue,
        });
    }
}

/// What an entity draws as.
#[derive(Clone, Debug, PartialEq)]
pub enum Visual {
    /// An xmodel by name (its box when no zone holds it).
    Model(String),
    /// Inline brush model `*N`.
    Submodel(usize),
    /// Nothing the client could name: its type's box.
    Box,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Drawn {
    pub number: u32,
    pub etype: u32,
    pub visual: Visual,
    pub origin: Vec3,
    /// Pitch, yaw, roll in degrees.
    pub angles: Vec3,
    /// A player's or corpse's pose inputs; `None` for anything else, or a
    /// body whose client state names no model.
    pub body: Option<BodyView>,
    /// A helicopter's controllers and damage stage; `None` for anything
    /// else.
    pub vehicle: Option<VehicleView>,
    /// A mounted gun's `lerp.u.turret.gunAngles` (pitch, yaw, the flash's
    /// pitch), lerped between the two snapshots; `None` for anything else.
    pub turret: Option<[f32; 3]>,
    /// The newer snapshot's `lerp.eFlags` 0x40: a gun firing
    /// (`CG_mg42_PreControllers`, cod4-client-view.md 13).
    pub firing: bool,
    /// The newer snapshot's `partBits`: the bones `hidePart` hid, whose
    /// surfaces the model draws without (cod4-client-view.md 15).
    pub part_bits: [u32; 4],
}

/// What a helicopter is posed and smoked from (the pose controllers,
/// The vehicle effects step, cod4-client-view.md 11): its `lerp.u.vehicle` angles
/// between the two snapshots, in degrees, and its
/// `un1.helicopterStage`.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct VehicleView {
    pub body_pitch: f32,
    pub body_roll: f32,
    pub gun_pitch: f32,
    pub gun_yaw: f32,
    pub stage: u32,
}

/// What a player or corpse is posed from: its entity's anims, flags and
/// angles in CoD2's numbering (the shared pose code's), the models its
/// client state names and the weapon it holds.
#[derive(Clone, Debug, PartialEq)]
pub struct BodyView {
    pub frame: BodyFrame,
    pub character: Character,
    /// The weapon list's name (`m16_gl_mp`), `None` empty handed.
    pub weapon: Option<String>,
    /// Its `weaponModel`, the camo: the `worldModel[]` slot the held gun
    /// draws (`iw3mp.exe` 0x407401..0x40746a, cod4-client-view.md 4.7).
    pub weapon_model: usize,
    /// An `ET_PLAYER_CORPSE`: its models are the ones copied when it
    /// arrived, not its client's live ones (`play::cod4_players`).
    pub corpse: bool,
    /// Its `pos.trType` is one of `startRagdoll`'s (9 to 11): the client
    /// simulates its bones (`play::cod4_players`, cod4-combat.md 5.1).
    pub ragdoll: bool,
    /// A gunner's gun (`eFlags` 0x300): `otherEntityNum` when it names a
    /// general entity (`CG_PlayerTurretPositionAndBlend`'s test), and the
    /// gun's weapon when the snapshot carries it.
    pub gun: Option<(u32, Option<String>)>,
}

/// The frame to draw: the eye, its angles and the entities.
#[derive(Clone, Debug, Default)]
pub struct Scene {
    pub eye: Vec3,
    /// The view weapon's lighting origin: the eye before its floor, the
    /// origin raised by `viewHeightCurrent` and leaned 16 and 20
    /// (`CG_AddViewWeapon`, iw3mp.exe 0x456dd7, cod4-world.md 3.3).
    pub light_eye: Vec3,
    /// Pitch, yaw, roll.
    pub angles: Vec3,
    pub entities: Vec<Drawn>,
    /// The time of the newer snapshot drawn (`cg.nextSnap`), whose
    /// states the entities carry.
    pub server_time: i32,
    /// The time drawn: between the two newest snapshots' (`cg.time`).
    pub render_time: i32,
    /// The view is another client's (a followed player, the killcam), not
    /// our own body's.
    pub following: bool,
    /// The view starts over between the two snapshots drawn (a spawn, a
    /// teleport, another client's view): drawn at the newer, unlerped.
    #[cfg_attr(not(test), allow(dead_code))]
    pub view_reset: bool,
    /// What the view weapon reads off the drawn playerstate.
    pub view: crate::cod4_viewmodel::ViewPs,
    /// `CS_MODELS + viewmodelIndex`, the hands the playerstate names.
    pub hands: Option<String>,
    /// The newer snapshot's `dof*` fields (`crate::cod4_dof`).
    pub dof: crate::cod4_dof::PsDof,
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
        last_send: Option<Instant>,
        last_snap_at: Instant,
        /// When the last sequenced message came (`clc.lastPacketTime`).
        last_msg: Instant,
    },
    /// A `.dm_1` played back (cod4-demos.md 5).
    Demo(Box<DemoPlay>),
}

/// `demo` / `timedemo`: `CL_PlayDemo_f`'s `clc` fields (`iw3mp.exe`
/// 0x469120) and `CL_SetCGameTime`'s demo arm (0x45c440).
struct DemoPlay {
    reader: vcod_cod4::net::demo::DemoReader,
    client: Client,
    /// The archive rows the demo carries (`cl.clientArchive`).
    archive: vcod_cod4::net::demo::ArchiveRing,
    /// Every archive row in the file, by time: the recorder's view at
    /// each of his frames, read ahead of the messages (cod4-demos.md 5).
    views: Vec<vcod_cod4::net::demo::Archive>,
    /// `clc.firstDemoFrameSkipped` (+0x401a0).
    first_frame_skipped: bool,
    /// `timedemo` (+0x40198) and its count.
    timedemo: Option<crate::play::demos::TimeDemo>,
    /// `clc.timeDemoBaseTime` (0x934e98): the first snapshot's time.
    base_time: i32,
    /// `cl_freezeDemo`: the clock holds.
    freeze: bool,
    /// `cl.serverTimeDelta` once the first snapshot set it; `None` primed.
    delta: Option<i32>,
    /// `cl.oldServerTime` and `cl.serverTime`.
    old_time: i32,
    time: i32,
    /// The file has no record left.
    ended: bool,
    /// Gamestates read (a map change inside the demo).
    gamestates: u32,
    /// The newest server command applied to the table (`d`).
    applied: i32,
    /// Records read, messages parsed and parse errors.
    messages: usize,
    errors: usize,
}

impl DemoPlay {
    /// `CL_ReadDemoMessage` (0x4690c0): one record; false once there is
    /// none. A message that fails to parse is counted and skipped.
    fn read(&mut self) -> bool {
        use vcod_cod4::net::demo::{DemoEnd, Record};
        if self.ended {
            return false;
        }
        match self.reader.next_record() {
            Ok(Record::Archive(i, a)) => {
                self.archive.rows[i] = a;
                self.archive.index = (i + 1) % vcod_cod4::net::demo::ARCHIVE_ROWS;
                true
            }
            Ok(Record::Message(seq, msg)) => {
                self.messages += 1;
                if msg.len() < 4 {
                    return true;
                }
                match self.client.parse(seq, &msg[4..]) {
                    Ok(p) => {
                        let n = p.ops.iter().filter(|o| matches!(o, Op::Gamestate)).count();
                        self.gamestates += n as u32;
                        vcod_cod4::net::conn::apply_configstrings(
                            &mut self.client,
                            &mut self.applied,
                            &p,
                        );
                    }
                    Err(e) => {
                        self.errors += 1;
                        log::warn!("cod4: demo message {seq}: {e:#}");
                    }
                }
                true
            }
            Err(end) => {
                match end {
                    DemoEnd::Completed => {}
                    DemoEnd::Truncated => crate::console::log::print("Demo file truncated."),
                    DemoEnd::Corrupt => crate::console::log::print("Demo file corrupt."),
                    DemoEnd::TooLong(_) => {
                        crate::console::log::print("CL_ReadDemoMessage: demoMsglen > MAX_MSGLEN")
                    }
                }
                self.ended = true;
                false
            }
        }
    }

    /// The recorder's view at client time `time`, lerped between his
    /// rows either side of it, which his snapshots leave out
    /// (cod4-demos.md 5).
    fn archived_view(&self, time: i32) -> Option<Vec3> {
        let after = self.views.partition_point(|r| r.server_time <= time);
        let before = self.views.get(after.checked_sub(1)?)?;
        let Some(next) = self.views.get(after) else {
            return Some(Vec3::from(before.view_angles));
        };
        let f = (time - before.server_time) as f32 / (next.server_time - before.server_time) as f32;
        Some(lerp_angles(
            Vec3::from(before.view_angles),
            Vec3::from(next.view_angles),
            f,
        ))
    }

    /// One frame of `CL_SetCGameTime` at local ms `realtime`: primed, the
    /// first call is skipped, then a record a frame until a snapshot
    /// without `SNAPFLAG_NOT_ACTIVE` (2) seeds the clock (`CL_FirstSnapshot`,
    /// 0x45c250); active, `serverTime = realtime + delta`, never back
    /// (held under `cl_freezeDemo`; `CL_AdjustTimeDelta` is never reached),
    /// or under `timedemo` 50 ms a frame off the first snapshot
    /// (0x45c350), then records while the newest snapshot is not past it.
    fn frame(&mut self, realtime: i32) -> Option<i32> {
        if self.delta.is_none() {
            if !self.first_frame_skipped {
                self.first_frame_skipped = true;
                return None;
            }
            let had = self.client.snapshots.len();
            self.read();
            let snap = self.client.snapshots.last()?;
            if self.client.snapshots.len() == had || snap.snap_flags & 2 != 0 {
                return None;
            }
            let t = snap.server_time;
            self.delta = Some(t - realtime);
            (self.old_time, self.time, self.base_time) = (t, t, t);
        }
        let delta = self.delta?;
        if !self.freeze {
            self.time = (realtime + delta).max(self.old_time);
        }
        if let Some(td) = self.timedemo.as_mut() {
            self.time = self.base_time + td.step(realtime) * 50;
        }
        self.old_time = self.time;
        while self
            .client
            .snapshots
            .last()
            .is_none_or(|s| self.time >= s.server_time)
        {
            if !self.read() {
                break;
            }
        }
        Some(self.time)
    }
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
    /// (`cod4_voice`).
    pub voice: crate::cod4_voice::VoiceLink,
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
    /// `cl.clientArchive`: our predicted state at each packet's time.
    archive: vcod_cod4::net::demo::ArchiveRing,
    /// The last predicted playerstate, which the archive copies.
    last_predicted: Option<pm::PlayerState>,
    /// The recoil and the landing dip (`play::cod4_kick`), and how many of
    /// [`Self::fired`] they have taken.
    pub kick: cod4_kick::Cod4Kick,
    kick_seen: usize,
    /// The snapshots a live session keeps when it sheds
    /// ([`KEEP_SNAPSHOTS`]; a test sheds sooner).
    keep_snapshots: usize,
}

/// `cg.selectedLocation` and `locSelInputState`: where the location
/// selector's cursor is over the full map (0..1 east and south), and a
/// confirm or cancel the next cmd carries.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Selector {
    pub at: [f32; 2],
    pub confirm: bool,
    pub cancel: bool,
    /// `+attack` was down at the last cmd: a press confirms.
    attack_was: bool,
}

impl Default for Selector {
    fn default() -> Self {
        Selector {
            at: [0.5; 2],
            confirm: false,
            cancel: false,
            attack_was: false,
        }
    }
}

/// The cmd bits of a selecting body (`CL_CreateCmd`'s selector arm): always
/// 0x100000, 0x10000 to confirm, 0x20000 to cancel.
pub const BUTTON_SELECTING: u32 = 0x10_0000;
pub const BUTTON_CONFIRM_LOCATION: u32 = 0x1_0000;
pub const BUTTON_CANCEL_LOCATION: u32 = 0x2_0000;
/// `cg_mapLocationSelectionCursorSpeed`'s default 0.6 (the float at
/// `iw3mp.exe` 0x6b7a9c its registration at 0x43bc9c loads).
pub const SELECTOR_SPEED: f32 = 0.6;

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

    /// Joins `addr` and waits for its gamestate (`cp` answered), up to
    /// 30 s; `join` answers the team and class menus on their own.
    #[cfg(test)]
    pub fn connect(addr: SocketAddr, name: &str, cp: Cp, join: MenuJoin) -> Result<Cod4Session> {
        Self::connect_with(addr, name, cp, join, None)
    }

    /// [`Self::connect`], then, with `downloads`, the files the server
    /// references that the install lacks: fetched into it (over HTTP when
    /// the server redirects and `cl_wwwDownload` is on) and the gamestate
    /// `donedl` brings waited for, or with downloads off the join refused
    /// as retail's `CL_InitDownloads` refuses it (cod4-handshake.md 11). A
    /// disconnected HTTP fetch (`sv_wwwDlDisconnected`) joins again once it
    /// lands.
    pub fn connect_with(
        addr: SocketAddr,
        name: &str,
        cp: Cp,
        join: MenuJoin,
        downloads: Option<Fetch<'_>>,
    ) -> Result<Cod4Session> {
        Self::connect_password(addr, name, "", cp, join, downloads)
    }

    /// [`Self::connect_with`] with the userinfo's `password`, as the
    /// browser's join sends it.
    pub fn connect_password(
        addr: SocketAddr,
        name: &str,
        password: &str,
        cp: Cp,
        join: MenuJoin,
        mut downloads: Option<Fetch<'_>>,
    ) -> Result<Cod4Session> {
        let mut fetched = std::collections::HashSet::new();
        let mut profile = super::cod4_profile::ProfileStats::load();
        let cancel = downloads.as_ref().and_then(|d| d.cancel.clone());
        let notes = downloads.as_ref().and_then(|d| d.notes.clone());
        // A reconnect a disconnected fetch, each one landing a file.
        for _ in 0..16 {
            let conn = Self::join_conn(
                addr,
                name,
                password,
                cp.clone(),
                downloads.as_ref().is_some_and(|d| d.www),
                cancel.clone(),
                notes.clone(),
            )?;
            let Some(conn) = (match downloads.as_mut() {
                Some(d) => fetch_missing(conn, d, &mut fetched)?,
                None => Some(conn),
            }) else {
                log::info!("cod4: reconnecting to {addr}");
                continue;
            };
            let mut s = Self::new(Source::Live {
                conn: Box::new(conn),
                last_send: None,
                last_snap_at: clock_now(),
                last_msg: clock_now(),
            });
            s.menus = ScriptMenus::new(join);
            if let Some((p, stats)) = profile.take() {
                s.menus.set_stats(stats);
                s.profile = Some(p);
            }
            s.loaded_map = s.map_name();
            return Ok(s);
        }
        bail!("{addr}: still missing files after 16 reconnects")
    }

    /// `connect` to `addr` and its first gamestate (`cp` answered).
    fn join_conn(
        addr: SocketAddr,
        name: &str,
        password: &str,
        cp: Cp,
        www: bool,
        cancel: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
        notes: Option<vcod_cod4::net::conn::Notes>,
    ) -> Result<Conn> {
        // The stats packets carry the active profile's stats file.
        let stats = super::cod4_profile::ProfileStats::load().map(|(_, s)| s);
        // A qport of its own, as `Netchan_Init`'s random one, so two clients
        // on one host are two clients; under 0x8000, as retail's stats
        // lookup misses a larger one (it read none on a live server).
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.subsec_nanos());
        let mut conn = Conn::join(
            addr,
            Join {
                name: name.into(),
                password: password.into(),
                cp,
                qport: (nanos ^ std::process::id()) as u16 & 0x7fff,
                www,
                cancel,
                notes,
                voice: true,
                stats,
                ..Join::default()
            },
        )?;
        let deadline = clock_now() + Duration::from_secs(30);
        while conn.client.gamestate.is_none() {
            if clock_now() > deadline {
                bail!("{addr}: no gamestate");
            }
            conn.send(&[])?;
            conn.poll(clock_now() + Duration::from_millis(50))?;
        }
        Ok(conn)
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
            archive: Default::default(),
            last_predicted: None,
            kick: Default::default(),
            kick_seen: 0,
            keep_snapshots: KEEP_SNAPSHOTS,
        }
    }

    /// `ps.locationSelectionInfo` of the newest snapshot: non-zero while
    /// the server's `beginLocationSelection` holds us on the full map.
    pub fn selecting_location(&self) -> u32 {
        self.client()
            .snapshots
            .last()
            .and_then(|s| s.ps.get("locationSelectionInfo"))
            .unwrap_or(0)
    }

    /// The mouse's turn, `[pitch, yaw]` degrees, moving the selector's
    /// cursor instead of the view: a degree right moves it
    /// `cg_mapLocationSelectionCursorSpeed` / 100 of the map east, down
    /// as far south (vcod's scale; 1.7 moves it by the mouse's counts).
    pub fn move_selector(&mut self, [pitch, yaw]: [f32; 2]) {
        let k = SELECTOR_SPEED / 100.0;
        let at = &mut self.selector.at;
        at[0] = (at[0] - yaw * k).clamp(0.0, 1.0);
        at[1] = (at[1] + pitch * k).clamp(0.0, 1.0);
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
            .rposition(|s| s.server_time <= time)
            .or((!snaps.is_empty()).then_some(0))
            .map(|i| &snaps[i])
        else {
            return false;
        };
        let command_time = snap.ps.get("commandTime").unwrap_or(0) as i32;
        command_time < oldest.server_time && oldest.server_time <= time
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
        self.predictor.reset();
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
    }

    /// The prediction world, when one was handed in.
    pub fn world(&self) -> Option<&Arc<Cod4World>> {
        self.world.as_ref()
    }

    /// The cmd clock, what a cmd built now is stamped: the newest
    /// snapshot's time plus its age, as vcod's live cmds and the probe's
    /// are. Arrival jitter would step it with every snapshot, so its offset
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

    /// A capture read to its end.
    pub fn finished(&self) -> bool {
        match &self.source {
            Source::Capture { lines, next, .. } => *next >= lines.len(),
            Source::Live { .. } => false,
            Source::Demo(d) => d.ended,
        }
    }

    /// The serverinfo's `mapname`.
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
            return crate::cod4_voice::own_talking(
                held,
                self.voice_settings().0,
                self.voice.enabled,
            );
        }
        self.voice.talking(client, self.now_ms)
    }

    /// The talker list's clients (`UI_GetTalkerClientNum`): the talking
    /// ones but us, by number.
    pub fn talkers(&self) -> Vec<u32> {
        self.voice
            .talkers(self.own_client().unwrap_or(u32::MAX), self.now_ms)
    }

    /// Our client number off the newest snapshot.
    pub fn own_client(&self) -> Option<u32> {
        self.client().snapshots.last()?.ps.get("clientNum")
    }

    /// The local player's `team_t` in the newest snapshot's client
    /// list (`clientinfo[clientNum].team`), 0 (free) without one.
    pub fn own_team(&self) -> i32 {
        let Some(snap) = self.client().snapshots.last() else {
            return 0;
        };
        let me = snap.ps.get("clientNum").unwrap_or(u32::MAX);
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
        self.save_profile(false);
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
                    let snaps = session.client.snapshots.len();
                    if let Some(Item::Client { msg, .. }) = session.feed(line)? {
                        let last = msg.ops.iter().rev().find_map(|o| match o {
                            ClOp::Move { cmds, .. } => cmds.last().map(|(c, _)| c.angles),
                            _ => None,
                        });
                        self.cmd_angles = last.or(self.cmd_angles);
                    }
                    if session.client.snapshots.len() > snaps {
                        self.snap_at_ms = line.ms.saturating_sub(*start_ms) as f64;
                    }
                    *next += 1;
                }
            }
            Source::Live {
                conn,
                last_snap_at,
                last_msg,
                ..
            } => {
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
                    for r in &got {
                        // `CL_ParseSnapshot` clears `demowaiting` on a full
                        // frame (0x4737d5) before `CL_PacketEvent` writes.
                        if r.parsed.ops.iter().any(|o| {
                            matches!(o, Op::Snapshot(i) if conn.client.snapshots[*i].delta_num == 0)
                        }) {
                            rec.waiting = false;
                        }
                        if let Err(e) = rec.write(r.sequence as i32, &r.msg, &self.archive) {
                            log::warn!("cod4: demo write: {e}");
                        }
                    }
                    conn.no_delta = rec.waiting;
                }
                for r in &got {
                    for op in &r.parsed.ops {
                        match op {
                            Op::Gamestate => self.gamestates += 1,
                            Op::Snapshot(_) => {
                                *last_snap_at = now;
                                self.snap_at_ms = now_ms;
                            }
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
            self.slew_cmd_clock(t, n > self.cmd_snaps);
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
            let prev = snaps.len().checked_sub(2).map(|i| snaps[i].server_time);
            let n = self.client().total_snapshots() as u32;
            let t = self
                .clock
                .update_cod2(now_ms.floor() as i32, new.server_time, prev, n);
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
        if self.is_live() {
            self.live_frame(&new)?;
        }
        self.walk();
        // A live connection keeps the snapshots it still draws, deltas and
        // walks: with every one kept the soak's client grew 100 to 250 MB a
        // three-minute level (cod4-map-sweep.md, "The soak").
        if let Source::Live { conn, .. } = &mut self.source
            && conn.client.snapshots.len() >= 2 * self.keep_snapshots
        {
            let n = conn.client.shed(self.keep_snapshots);
            self.walked = self.walked.saturating_sub(n);
        }
        Ok(())
    }

    /// A live frame: the new server commands' menus and weapon picks, the
    /// view the server turned, this frame's cmd, and the packet when one
    /// is due.
    fn live_frame(&mut self, new: &[Vec<u8>]) -> Result<()> {
        let mut answers = Vec::new();
        // The source alone, beside the menus and the input.
        if let Some(gs) = &source_client(&self.source).gamestate {
            for text in new {
                answers.extend(self.menus.command(text, gs));
                let t = String::from_utf8_lossy(text);
                let mut w = t.split_whitespace();
                let arg = |w: Option<&str>| w.and_then(|x| x.parse::<u8>().ok());
                match w.next() {
                    // `switchToWeapon`, `setSpawnWeapon`: `cg_weaponSelect`.
                    Some("a") => {
                        if let Some(n) = arg(w.next()) {
                            self.input.select(n);
                        }
                    }
                    // The equipped offhand.
                    Some("C") => {
                        if let Some(n) = arg(w.next()) {
                            self.input.set_offhand(n);
                        }
                    }
                    _ => {}
                }
            }
        }
        for a in answers {
            log::info!("cod4: {a}");
            self.command(&a);
        }
        self.take_own_view();
        if let Some(d) = self.ran_dry.take() {
            self.out_of_ammo_change(&d);
        }
        let now = clock_now();
        let send_due = |t: &Option<Instant>| {
            t.is_none_or(|t| now.saturating_duration_since(t) >= Duration::from_millis(SEND_MS))
        };
        // The packet's archive row, built only when one goes out.
        let row = match &self.source {
            Source::Live { last_send, .. } if send_due(last_send) => self.archive_row(),
            _ => None,
        };
        let Source::Live {
            conn,
            last_send,
            last_snap_at,
            ..
        } = &mut self.source
        else {
            return Ok(());
        };
        if conn.client.gamestate.is_some() {
            let floor = self.pending.last().unwrap_or(conn.last_cmd()).server_time;
            // `CL_SetUserCmdAimValues`: the recoil rides the cmd's angles.
            let [kp, ky, _] = self.kick.view_kick();
            let c = own_cmd(
                &conn.client,
                now.saturating_duration_since(*last_snap_at),
                [self.view[0] + kp, self.view[1] + ky],
                &self.input,
                self.talking,
            );
            // One cmd a frame, never two on one millisecond.
            if c.server_time >= floor {
                let mut c = UserCmd {
                    server_time: c.server_time.max(floor + 1),
                    buttons: c.buttons | self.input.take_extra(),
                    ..c
                };
                let world = self.world.as_ref().map(|w| &*w.clip);
                (c.melee_charge_yaw, c.melee_charge_dist) = melee_charge(
                    &conn.client,
                    self.last_predicted.as_ref(),
                    [self.view[0] + kp, self.view[1] + ky],
                    c.buttons,
                    world,
                );
                selector_cmd(&mut self.selector, &mut c, ps_u_last(&conn.client));
                self.pending.push(c);
                // Unpredicted (a linked body), the drawn view is still
                // this cmd's over the newest `delta_angles`.
                self.cmd_angles = Some(c.angles);
                if self.sent_cmds.len() == cod4_predict::CMD_BACKUP {
                    self.sent_cmds.pop_front();
                }
                self.sent_cmds.push_back(Cmd::from(&c));
            }
        }
        if send_due(last_send) {
            *last_send = Some(now);
            if let Some(r) = row {
                self.archive.add(r);
            }
            let from = self.pending.len().saturating_sub(MAX_PACKET_CMDS);
            conn.send(&self.pending[from..])?;
            self.pending.clear();
        }
        Ok(())
    }

    /// The newest snapshot's own playerstate: a spawn or a turn the
    /// server made moves [`Self::view`] to its view, and the input takes
    /// its weapons. A followed one is left alone.
    fn take_own_view(&mut self) {
        let Some(gs) = self.gamestate() else {
            return;
        };
        let own = gs.client_num as u32;
        let Some(ps) = self.client().snapshots.last().map(|s| s.ps.clone()) else {
            return;
        };
        let u = |n: &str| ps.get(n).unwrap_or(0);
        if u("clientNum") != own || u("otherFlags") & 2 != 0 {
            self.own_delta = None;
            return;
        }
        let delta = [
            ps_f32(&ps, "delta_angles[0]"),
            ps_f32(&ps, "delta_angles[1]"),
        ];
        if self.own_delta != Some(delta) {
            // The cmd's angles stay as retail's `cl.viewangles` do, so the
            // view moves by the delta's change: the pitch clamp's push or
            // a spawn's turn. The snapshot's viewangles are stale unless
            // the view is locked (cod4-netfields.md 7); only the first
            // own playerstate takes them.
            self.view = match self.own_delta {
                Some(old) => std::array::from_fn(|i| {
                    vcod_common::pmove::angle_normalize180(self.view[i] + delta[i] - old[i])
                }),
                None => [ps_f32(&ps, "viewangles[0]"), ps_f32(&ps, "viewangles[1]")],
            };
            self.own_delta = Some(delta);
            self.view_jump = true;
        }
        let held = std::array::from_fn(|i| u(&format!("weapons[{i}]")));
        self.input.sync(
            own,
            ps.stats[4],
            u("weapon") as u8,
            u("offHandIndex") as u8,
            held,
        );
    }

    /// Our own playing body in the newest snapshot (`pm_type` normal, not
    /// following): its origin and weapon, as a line.
    pub fn spawn(&self) -> Option<String> {
        let gs = self.gamestate()?;
        let ps = &self.client().snapshots.last()?.ps;
        let u = |n: &str| ps.get(n).unwrap_or(0);
        if u("clientNum") != gs.client_num as u32 || u("otherFlags") & 2 != 0 || u("pm_type") != 0 {
            return None;
        }
        let w = u("weapon");
        Some(format!(
            "client {} at ({:.0} {:.0} {:.0}) weapon {w} {}",
            gs.client_num,
            ps_f32(ps, "origin[0]"),
            ps_f32(ps, "origin[1]"),
            ps_f32(ps, "origin[2]"),
            weapon_name(&weapon_list(gs), w).unwrap_or("?")
        ))
    }

    /// The mouse's turn this frame (pitch, yaw), degrees, added to
    /// [`Self::view`] as `CL_MouseMove` adds it to `cl.viewangles`: past
    /// the pitch limit it piles up until the server's `delta_angles` push
    /// takes it back, while the drawn view stays clamped.
    pub fn turn(&mut self, by: [f32; 2]) {
        use vcod_common::pmove::angle_normalize180;
        self.view = std::array::from_fn(|i| angle_normalize180(self.view[i] + by[i]));
    }

    /// The server turned the view since the last call: [`Self::view`] is
    /// the camera's now.
    pub fn take_view_jump(&mut self) -> bool {
        std::mem::take(&mut self.view_jump)
    }

    /// The held weapons `weapnext` walks: those the level's weapon defs
    /// call primaries (`inventoryType` 0), in index order.
    pub fn held_primaries(&self) -> Vec<u8> {
        let (Some(gs), Some(w), Some(snap)) = (
            self.gamestate(),
            &self.world,
            self.client().snapshots.last(),
        ) else {
            return Vec::new();
        };
        let held: [u32; 4] =
            std::array::from_fn(|i| snap.ps.get(&format!("weapons[{i}]")).unwrap_or(0));
        let list = weapon_list(gs);
        (1..128)
            .filter(|&n| crate::play::cod4_input::holds(held, n))
            .filter(|&n| weapon_name(&list, u32::from(n)).is_some_and(|x| w.primaries.contains(x)))
            .collect()
    }

    /// The weapon an `altMode`
    /// action slot shows, the selection itself when it is an alt mode,
    /// else its alt, else the alt of the first held primary after it that
    /// has one (`Cod4Input::action_slot`'s order); 0 for none. Also
    /// whether the selection is an alt mode (the slot's active state).
    pub fn alt_toggle_weapon(&self) -> (u8, bool) {
        let Some((_, defs)) = &self.level_defs else {
            return (0, false);
        };
        let def = |w: u8| defs.get(usize::from(w));
        let selected = self.input.weapons().0;
        if def(selected).is_some_and(|d| d.inventory_type == INVENTORY_ALT_MODE) {
            return (selected, true);
        }
        let alt = |w: u8| def(w).map_or(0, |d| d.alt_weapon as u8);
        if alt(selected) != 0 {
            return (alt(selected), false);
        }
        let primaries = self.held_primaries();
        let n = primaries.len();
        let at = primaries.iter().position(|&p| p == selected);
        let w = (1..=n)
            .map(|k| primaries[(at.unwrap_or(n.saturating_sub(1)) + k) % n])
            .map(alt)
            .find(|&a| a != 0)
            .unwrap_or(0);
        (w, false)
    }

    /// `+actionslot <n>` (1..=4) over the newest playerstate's slot and
    /// the level's weapon defs (each def's alt): the weapon it selects
    /// goes in the next cmds (`Cod4Input::action_slot`). True when the
    /// selection moved.
    pub fn action_slot(&mut self, n: usize) -> bool {
        let Some(snap) = self.client().snapshots.last() else {
            return false;
        };
        let ps = &snap.ps;
        let u = |name: &str| ps.get(name).unwrap_or(0);
        let k = n.saturating_sub(1).min(3);
        let s = crate::play::cod4_input::SlotState {
            kind: u(&format!("actionSlotType[{k}]")),
            param: u(&format!("actionSlotParam[{k}]")),
            weapon_state: u("weaponstate") as i32,
            pm_flags: u("pm_flags"),
            weap_flags: u("weapFlags"),
            e_flags: u("eFlags"),
            other_flags: u("otherFlags"),
            held: std::array::from_fn(|i| u(&format!("weapons[{i}]"))),
            weapon: u("weapon") as u8,
        };
        let now = snap.server_time;
        let primaries = self.held_primaries();
        let defs = self.level_defs.as_ref().map(|(_, d)| d.clone());
        let alt_of = |w: u8| -> u8 {
            defs.as_ref()
                .and_then(|d| d.get(usize::from(w)))
                .map_or(0, |d| d.alt_weapon as u8)
        };
        self.input.action_slot(&s, &alt_of, &primaries, now)
    }

    /// `CG_OutOfAmmoChange` (`iw3mp.exe` 0x457560) after our own EV_NOAMMO
    /// (called from 0x436376): unless dead or the weapon cancels the auto
    /// holster (+1288), an alt mode with its base gun held selects the
    /// gun when it has ammo (a grenade launcher's last round goes back to
    /// the rifle), else the primary cycle (0x4576d0) selects the next
    /// held primary by index that has ammo; an alt mode whose gun is gone
    /// selects nothing (the alt mode check, 0x457510). The arm
    /// for an empty hand (`weaponLatestPrimaryIdx`) is left out.
    fn out_of_ammo_change(&mut self, d: &Dry) {
        let Some((_, defs)) = &self.level_defs else {
            return;
        };
        let primaries = self.held_primaries();
        if let Some(w) = out_of_ammo_pick(defs, d, self.input.weapons().0, &primaries) {
            self.input.select(w);
        }
    }

    /// Esc with no script menu up: the server's `g_scriptMainMenu`.
    pub fn open_main_menu(&mut self) {
        if let Some(gs) = self.gamestate().cloned() {
            for a in self.menus.open_main(&gs) {
                self.command(&a);
            }
        }
    }

    /// A key in the open script menu, its answers sent; false when the
    /// menu does nothing with it (or none is up).
    pub fn menu_key(&mut self, key: crate::play::cod4_menus::MenuKey) -> bool {
        let Some(gs) = self.gamestate().cloned() else {
            return false;
        };
        let mut out = Vec::new();
        let taken = self.menus.key(key, &gs, &mut out);
        for a in out {
            self.command(&a);
        }
        taken
    }

    /// The mouse over the open script menu: raw counts `d` or a position
    /// `at` on a `size` window, or (both `None`) MOUSE1; its answers sent.
    pub fn menu_mouse(&mut self, d: Option<[f32; 2]>, at: Option<[f32; 2]>, size: (f32, f32)) {
        let Some(gs) = self.gamestate().cloned() else {
            return;
        };
        let out = if d.is_none() && at.is_none() {
            self.menus.click(size, &gs)
        } else {
            self.menus.mouse_move(d, at, size, &gs)
        };
        for a in out {
            self.command(&a);
        }
    }

    /// What `CL_SetUserCmdOrigin` (0x45ad30) last handed the client: the
    /// predicted body, else the newest snapshot's, at `cl.serverTime`.
    fn archive_row(&self) -> Option<vcod_cod4::net::demo::Archive> {
        let time = self.cg_time?;
        let built;
        let ps = match &self.last_predicted {
            Some(p) => p,
            None => {
                // Unpredicted, the view is the last cmd's over the newest
                // `delta_angles`, as the scene draws it.
                let snap = &self.client().snapshots.last()?.ps;
                let mut ps = pm::PlayerState::from_wire(snap);
                if let Some(a) = self.cmd_angles {
                    for (k, (v, a)) in ps.view_angles.iter_mut().zip(a).enumerate() {
                        let d = angle2short(ps_f32(snap, &format!("delta_angles[{k}]")));
                        *v = f32::from(a.wrapping_add(d) as i16) * (360.0 / 65536.0);
                    }
                }
                built = ps;
                &built
            }
        };
        Some(vcod_cod4::net::demo::Archive {
            server_time: time,
            origin: ps.origin.to_array(),
            velocity: ps.velocity.to_array(),
            bob_cycle: ps.bob_cycle,
            movement_dir: ps.movement_dir,
            view_angles: ps.view_angles,
        })
    }

    /// `demo <name>` / `timedemo <name>` (`CL_PlayDemo_f`, `iw3mp.exe`
    /// 0x469120): the file's records read up to its gamestate; the caller
    /// loads the map it names, then steps the session on the demo's clock
    /// with nothing predicted or sent (cod4-demos.md 5).
    pub fn play_demo(data: Vec<u8>, timedemo: bool) -> Result<Cod4Session> {
        let mut views = Vec::new();
        let mut scan = vcod_cod4::net::demo::DemoReader::new(data.clone());
        while let Ok(r) = scan.next_record() {
            if let vcod_cod4::net::demo::Record::Archive(_, a) = r {
                views.push(a);
            }
        }
        views.retain(|a| a.server_time != 0);
        views.sort_by_key(|a| a.server_time);
        views.dedup_by_key(|a| a.server_time);
        let mut d = DemoPlay {
            views,
            reader: vcod_cod4::net::demo::DemoReader::new(data),
            client: Client::new(),
            archive: Default::default(),
            first_frame_skipped: false,
            timedemo: timedemo.then(Default::default),
            base_time: 0,
            freeze: false,
            delta: None,
            old_time: 0,
            time: 0,
            ended: false,
            gamestates: 0,
            applied: 0,
            messages: 0,
            errors: 0,
        };
        // `CL_PlayDemo_f`'s loop: records while the state is 5 to 7, so
        // up to the gamestate's (`CL_ParseGamestate` moves it on).
        while d.client.gamestate.is_none() {
            if !d.read() {
                bail!("the demo ends before its gamestate");
            }
        }
        let n = d.gamestates;
        let mut s = Self::new(Source::Demo(Box::new(d)));
        s.gamestates = n;
        s.level_gamestates = n;
        s.loaded_map = s.map_name();
        Ok(s)
    }

    /// Whether this session plays a demo.
    pub fn is_demo(&self) -> bool {
        matches!(self.source, Source::Demo(_))
    }

    /// `cl_freezeDemo` for the next frames.
    pub fn set_freeze_demo(&mut self, freeze: bool) {
        if let Source::Demo(d) = &mut self.source {
            d.freeze = freeze;
        }
    }

    /// The demo ran out: the caller runs `CL_DemoCompleted` (0x468df0):
    /// [`Self::timedemo_report`], the disconnect, `nextdemo`.
    pub fn demo_over(&self) -> bool {
        matches!(&self.source, Source::Demo(d) if d.ended)
    }

    /// The records' messages read and how many failed to parse.
    #[cfg(test)]
    pub fn demo_counts(&self) -> Option<(usize, usize)> {
        match &self.source {
            Source::Demo(d) => Some((d.messages, d.errors)),
            _ => None,
        }
    }

    /// `CL_DemoCompleted`'s `%i frames, %3.1f seconds: %3.1f fps` under
    /// `timedemo`, when any time passed (0x468e2b).
    pub fn timedemo_report(&self, now_ms: i32) -> Option<String> {
        match &self.source {
            Source::Demo(d) => d.timedemo.as_ref()?.report(now_ms),
            _ => None,
        }
    }

    /// `record [name]` (`CL_Record_f`, 0x468960) into `dir` (the game's
    /// `demos/`): `<name>.dm_1`, or the first free `demoNNNN`. The console
    /// line it prints.
    pub fn start_recording(&mut self, dir: &std::path::Path, name: Option<&str>) -> String {
        use vcod_cod4::net::demo::{EXTENSION, Recording, gamestate_message};
        if self.recording.is_some() {
            return "Already recording.".into();
        }
        let Source::Live { conn, .. } = &mut self.source else {
            return "You must be in level to record.".into();
        };
        // `cls.state` 9 (`CA_ACTIVE`): a snapshot is in.
        let Some(gs) = conn
            .client
            .gamestate
            .as_ref()
            .filter(|_| !conn.client.snapshots.is_empty())
        else {
            return "You must be in level to record.".into();
        };
        let file = |n: &str| dir.join(format!("{n}.dm_{EXTENSION}"));
        let name = match name {
            // `clc.demoName` keeps 63 characters.
            Some(n) => n.chars().take(63).collect(),
            None => vcod_common::net::demo::auto_name(|n| file(n).exists()),
        };
        let qpath = format!("demos/{name}.dm_{EXTENSION}");
        let opened = std::fs::create_dir_all(dir).and_then(|_| std::fs::File::create(file(&name)));
        let Ok(f) = opened else {
            return "ERROR: couldn't open.".into();
        };
        let msg = gamestate_message(
            &conn.client.huff,
            conn.reliable_sequence(),
            conn.client.server_command_sequence,
            gs,
        );
        match Recording::start(
            Box::new(std::io::BufWriter::new(f)),
            &name,
            conn.message_sequence(),
            &msg,
            &self.archive,
        ) {
            Ok(r) => {
                self.recording = Some(r);
                conn.no_delta = true;
                format!("recording to {qpath}.")
            }
            Err(_) => "ERROR: couldn't open.".into(),
        }
    }

    /// `stoprecord` (`CL_StopRecord_f`, 0x4688c0); the console line.
    pub fn stop_recording(&mut self) -> String {
        let Some(r) = self.recording.take() else {
            return "Not recording demo.".into();
        };
        if let Source::Live { conn, .. } = &mut self.source {
            conn.no_delta = false;
        }
        match r.finish() {
            Ok(_) => "Stopped demo.".into(),
            Err(e) => format!("ERROR: {e}"),
        }
    }

    /// The demo's name and its bytes so far, for the `RECORDING` line.
    pub fn recording_status(&self) -> Option<(&str, u64)> {
        self.recording
            .as_ref()
            .map(|r| (r.name.as_str(), r.written()))
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
            Source::Live { conn, .. } => conn.commands().len(),
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
        let got: Vec<Vec<u8>> = (from + 1..=newest)
            .map(|q| c.server_command(q).to_vec())
            .collect();
        self.cmd_seq = newest;
        self.commands.extend(got.iter().cloned());
        got
    }

    fn walk(&mut self) {
        let n = self.client().snapshots.len();
        while self.walked < n {
            let i = self.walked;
            self.walked += 1;
            let world = self.world.is_some() && !self.is_demo();
            let (fired, lines, ps_ring, rings, temps) = {
                let c = self.client();
                let s = &c.snapshots[i];
                let predicted = c
                    .gamestate
                    .as_ref()
                    .is_some_and(|g| predicts(world, g, &s.ps));
                walk_snapshot(c, s, self.ps_ring, predicted, &self.rings, &self.temps)
            };
            let fired = self.aim_others(i, fired);
            force_stance(&mut self.input, &fired);
            self.fired.extend(fired);
            self.lines.extend(lines);
            self.ps_ring = ps_ring;
            self.rings = rings;
            self.temps = temps;
        }
    }

    /// The level's weapon def `n` (0 the empty hand).
    fn level_def(&self, n: u32) -> Option<&vcod_cod4::pmove::weapon::WeaponDef> {
        self.level_defs
            .as_ref()?
            .1
            .get(n as usize)
            .filter(|_| n != 0)
    }

    /// Another player's fire events in snapshot `i` take their spread
    /// ([`crate::cod4_fx::Aim::other`]): his stance and sight off his legs
    /// anim, his speed off his `trBase` in the snapshot before, seeded by
    /// our own `commandTime`, the predicted one where we predict. A gun's
    /// (`ET_MG42`) take its weapon's `playerSpread` on the same seed
    /// ([`crate::cod4_fx::Aim::turret`]).
    fn aim_others(&self, i: usize, mut fired: Vec<Fired>) -> Vec<Fired> {
        use vcod_cod4::pmove::anim::LegsPose;
        let c = self.client();
        let (Some(now), Some(prev)) = (
            c.snapshots.get(i),
            i.checked_sub(1).and_then(|k| c.snapshots.get(k)),
        ) else {
            return fired;
        };
        let seed = self
            .last_predicted
            .as_ref()
            .map_or(now.ps.get("commandTime").unwrap_or(0) as i32, |p| {
                p.command_time
            });
        let dt = now.server_time.wrapping_sub(prev.server_time);
        for f in fired
            .iter_mut()
            .filter(|f| f.name.starts_with("EV_FIRE_WEAPON"))
        {
            let (Some(n), Some(e)) = (f.entity, f.state.as_ref()) else {
                continue;
            };
            let Some(d) = self.level_def(f.weapon) else {
                continue;
            };
            if e.etype() == ET_MG42 {
                f.aim = Some(crate::cod4_fx::Aim::turret(d, seed));
                continue;
            }
            if e.etype() != ET_PLAYER {
                continue;
            }
            let base = |e: &EntityState| vec3(|k| ent_f32(e, &format!("lerp.pos.trBase[{k}]")));
            let mut v = prev
                .entities
                .iter()
                .find(|p| p.number() == n)
                .map_or(Vec3::ZERO, |p| base(e) - base(p));
            if dt > 0 {
                v *= 1000.0 / dt as f32;
            }
            let legs = match &self.legs_movetypes {
                Some(t) => {
                    let wire = ent_u32(e, "legsAnim") & !vcod_cod4::pmove::anim::ANIM_TOGGLEBIT;
                    LegsPose::of(t.get(wire as usize).copied().unwrap_or(0))
                }
                None => {
                    let flags = ent_u32(e, "lerp.eFlags");
                    LegsPose {
                        prone: flags & 8 != 0,
                        crouched: flags & 4 != 0,
                        sighted: false,
                    }
                }
            };
            f.aim = Some(crate::cod4_fx::Aim::other(d, legs, v.length(), seed));
        }
        fired
    }

    /// The view and entities at this frame's `cl.serverTime`
    /// ([`Self::step`]'s clock), lerped between the snapshots either side
    /// of it, as `CG_ProcessSnapshots` and `CG_InterpolatePlayerState` do
    /// with no prediction (cod4-client-view.md 2); our own view predicted
    /// over them (section 5), which steps the predictor, so once a frame.
    pub fn scene(&mut self) -> Option<Scene> {
        // The source alone, so the predictor can be stepped beside it.
        let c = source_client(&self.source);
        let gs = c.gamestate.as_ref()?;
        let time = self.cg_time?;
        // `cg.snap`: the newest at or before the time (the first when the
        // time is before them all); `cg.nextSnap` the first after it, or
        // `cg.snap` again when none has come.
        let at = c
            .snapshots
            .iter()
            .rposition(|s| s.server_time <= time)
            .unwrap_or(0);
        let old = &c.snapshots[at];
        let new = c.snapshots[at + 1..]
            .iter()
            .find(|s| s.server_time > old.server_time)
            .unwrap_or(old);
        let time = time.max(old.server_time);
        // `CG_SetFrameInterpolation`.
        let frac = match new.server_time - old.server_time {
            0 => 0.0,
            span => ((time - old.server_time) as f32 / span as f32).max(0.0),
        };
        // A respawn, a teleport or another client's view starts over:
        // `CG_SetNextSnap` copies the new playerstate over the old.
        let ps_u = |ps: &PlayerState, f: &str| ps.get(f).unwrap_or(0);
        let reset = (ps_u(&old.ps, "eFlags") ^ ps_u(&new.ps, "eFlags")) & EF_TELEPORT != 0
            || ps_u(&old.ps, "clientNum") != ps_u(&new.ps, "clientNum")
            || old.ps.stats[4] != new.ps.stats[4];
        let old_ps = if reset { &new.ps } else { &old.ps };
        let f = |ps: &PlayerState, name: &str| ps_f32(ps, name);
        let lerp1 = |name: &str| f(old_ps, name) + (f(&new.ps, name) - f(old_ps, name)) * frac;
        let mut eye =
            vec3(|k| lerp1(&format!("origin[{k}]"))) + Vec3::Z * lerp1("viewHeightCurrent");
        // Our own view is the newest cmd's over the next snapshot's
        // `delta_angles` (`PM_UpdateViewAngles`, unlerped); one we follow
        // (the killcam) lerps its own.
        let own = new.ps.get("clientNum") == Some(gs.client_num as u32);
        let mut angles = match self.cmd_angles.filter(|_| own) {
            Some(a) => vec3(|k| {
                let d = angle2short(f(&new.ps, &format!("delta_angles[{k}]")));
                f32::from(a[k].wrapping_add(d) as i16) * (360.0 / 65536.0)
            }),
            None => {
                let view = |ps: &PlayerState| vec3(|k| f(ps, &format!("viewangles[{k}]")));
                lerp_angles(view(old_ps), view(&new.ps), frac)
            }
        };
        // A demo's own view: the recorder's rows lerped at the drawn time,
        // all read ahead; his eye the playerstate's lerp. Rows read with
        // their messages lag the snapshots, so the view and eye stood
        // still between them and stepped at the snapshot rate.
        if own
            && let Source::Demo(d) = &self.source
            && let Some(view) = d.archived_view(time)
        {
            angles = view;
        }
        let weapons = weapon_list(gs);
        let render_time = time;
        let mut entities = Vec::new();
        for e in &new.entities {
            let Some(visual) = visual(gs, new, &weapons, e) else {
                continue;
            };
            let place = |e: &EntityState| {
                (
                    vec3(|k| ent_f32(e, &format!("lerp.pos.trBase[{k}]"))),
                    vec3(|k| ent_f32(e, &format!("lerp.apos.trBase[{k}]"))),
                )
            };
            let (o1, a1) = place(e);
            // One new to the snapshots, or whose teleport bit toggled,
            // starts from its new state (`CG_ResetEntity`).
            let prev = old
                .entities
                .iter()
                .find(|p| p.number() == e.number() && p.etype() == e.etype())
                .filter(|p| {
                    (ent_u32(p, "lerp.eFlags") ^ ent_u32(e, "lerp.eFlags")) & EF_TELEPORT == 0
                });
            let (origin, angles) = match prev {
                Some(p) if interpolates(p, e) => {
                    let (o0, a0) = place(p);
                    (o0.lerp(o1, frac), lerp_angles(a0, a1, frac))
                }
                // `BG_EvaluateTrajectory` on the older state at the drawn
                // time (`CG_CalcEntityLerpPositions`).
                Some(p) => (
                    trajectory(p, "pos").evaluate(render_time),
                    trajectory(p, "apos").evaluate(render_time),
                ),
                None if interpolates(e, e) => (o1, a1),
                None => (
                    trajectory(e, "pos").evaluate(render_time),
                    trajectory(e, "apos").evaluate(render_time),
                ),
            };
            let mut body = body_view(gs, new, &weapons, e, origin, angles);
            // `CG_InterpolateEntityPosition`: a player's lean and move
            // direction step between the two snapshots with its angles
            // (cod4-player-anims.md 8).
            if let (Some(b), Some(p)) = (body.as_mut(), prev)
                && e.etype() == ET_PLAYER
                && interpolates(p, e)
            {
                let lean = ent_f32(p, "lerp.u.player.leanf");
                b.frame.lean = lerp_wrapped(lean, b.frame.lean, frac);
                let dir = movement_dir(p);
                b.frame.movement_dir = lerp_wrapped(dir, b.frame.movement_dir, frac);
            }
            let vehicle = (e.etype() == ET_HELICOPTER).then(|| {
                let a = |s: &EntityState, n: &str| ent_f32(s, &format!("lerp.u.vehicle.{n}"));
                let lerp = |n: &str| {
                    let to = a(e, n);
                    prev.map_or(to, |p| lerp_angles(Vec3::X * a(p, n), Vec3::X * to, frac).x)
                };
                VehicleView {
                    body_pitch: lerp("bodyPitch"),
                    body_roll: lerp("bodyRoll"),
                    gun_pitch: lerp("gunPitch"),
                    gun_yaw: lerp("gunYaw"),
                    stage: ent_u32(e, "un1.helicopterStage"),
                }
            });
            // `turret_controller`'s angles as `CG_mg42` lerps them.
            let turret = (e.etype() == ET_MG42).then(|| {
                let gun = |s: &EntityState| {
                    Vec3::from_array(std::array::from_fn(|k| {
                        ent_f32(s, &format!("lerp.u.anonymous.data[{k}]"))
                    }))
                };
                prev.map_or(gun(e), |p| lerp_angles(gun(p), gun(e), frac))
                    .to_array()
            });
            entities.push(Drawn {
                number: e.number(),
                etype: e.etype(),
                visual,
                origin,
                angles,
                body,
                vehicle,
                turret,
                firing: ent_u32(e, "lerp.eFlags") & EF_FIRING != 0,
                part_bits: std::array::from_fn(|k| ent_u32(e, &format!("partBits[{k}]"))),
            });
        }
        // `CG_PredictPlayerState`: our own view, not one we follow
        // (`otherFlags` 2) nor a linked body, is the snapshot's playerstate
        // with our cmds replayed on it (cod4-client-view.md 5).
        if let Some(w) = &self.world {
            let list = weapons.join(" ");
            if self.level_defs.as_ref().is_none_or(|(l, _)| *l != list) {
                self.level_defs = Some((list, w.level_defs(&weapons)));
            }
        }
        // `cg.demoPlayback` predicts nothing (cod4-demos.md 5).
        let demo = matches!(self.source, Source::Demo(_));
        let predicts = predicts(self.world.is_some() && !demo, gs, &new.ps);
        // The snapshot's solid entities but our own, which the prediction
        // and the client-side rounds trace (cod4-client-view.md 5).
        let solids: Vec<Solid> = entities
            .iter()
            .filter(|d| d.etype != ET_ITEM && d.number != gs.client_num as u32)
            .filter_map(|d| {
                let e = new.entities.iter().find(|e| e.number() == d.number)?;
                let solid = ent_u32(e, "solid");
                // A script mover with no `solid` meets rounds by its
                // model's collision (`CG_LocationalTraceDObj`).
                if d.etype == ET_SCRIPTMOVER && solid == 0 {
                    let name = model_cs(gs, ent_u32(e, "index"))?.to_ascii_lowercase();
                    let surfs = self.world.as_ref()?.model_coll.get(&name)?.clone();
                    return Some(Solid {
                        number: d.number,
                        origin: d.origin,
                        angles: d.angles,
                        kind: cod4_predict::SolidKind::Model {
                            surfs,
                            hidden: std::array::from_fn(|k| ent_u32(e, &format!("partBits[{k}]"))),
                        },
                    });
                }
                Solid::of(
                    d.number,
                    d.etype == ET_PLAYER,
                    solid,
                    ent_u32(e, "lerp.eFlags"),
                    ent_u32(e, "index"),
                    d.origin,
                    d.angles,
                )
            })
            .collect();
        let predicted = match &self.world {
            Some(w) if predicts => {
                let now = {
                    let t = (self.now_ms + self.cmd_delta.unwrap_or(0.0)).floor() as i32;
                    self.cmd_last = self.cmd_last.max(t);
                    self.cmd_last
                };
                // The cmds built by now, and the frame's own, stamped now:
                // live, the last sent's keys and view; a capture's, those
                // its client sent for this moment (the first cmd at or past
                // it), where a frame-rate client would have built one, its
                // view eased from the cmd before (replay-only smoothing,
                // cod4-client-view.md 6).
                let (cmds, next): (Vec<Cmd>, Option<Cmd>) = match &self.source {
                    Source::Capture { cmds, .. } => {
                        let k = cmds.partition_point(|c| c.server_time <= now);
                        let from = k.saturating_sub(cod4_predict::CMD_BACKUP);
                        let next = cmds.get(k).map(|n| match k.checked_sub(1) {
                            Some(i) => eased_cmd(&cmds[i], n, now),
                            None => *n,
                        });
                        (cmds[from..k].to_vec(), next)
                    }
                    Source::Live { .. } | Source::Demo(_) => {
                        (self.sent_cmds.iter().copied().collect(), None)
                    }
                };
                let frame = next.or(cmds.last().copied()).map(|c| Cmd {
                    server_time: now,
                    ..c
                });
                let world = cod4_predict::Solids {
                    clip: &w.clip,
                    solids: &solids,
                };
                let rules = vcod_cod4::pmove::Rules {
                    // The server's slowed walk under a concussion
                    // (cod4-combat.md 10.8).
                    shock_slows: w.shock_slows(|i| gs.configstring(i).to_vec()),
                    ..cod4_predict::rules_from_mirror(|i| gs.configstring(i).to_vec())
                };
                let weapon = |n: u32| {
                    weapon_name(&weapons, n)
                        .and_then(|name| w.weapons.get(name).copied())
                        .unwrap_or(WeaponMove::NONE)
                };
                let cut = (
                    ps_u(&new.ps, "eFlags") & EF_TELEPORT,
                    ps_u(&new.ps, "clientNum"),
                    new.ps.stats[4],
                );
                self.predictor.note_damage(&new.ps, new.server_time);
                Some(self.predictor.predict(
                    &new.ps,
                    cut,
                    &cmds,
                    frame.as_ref(),
                    &world,
                    rules,
                    &weapon,
                    self.level_defs.as_ref().map(|(_, d)| d as &dyn WeaponDefs),
                    self.now_ms,
                ))
            }
            _ => {
                self.predictor.reset();
                None
            }
        };
        self.shot_solids = solids;
        // `CG_TransitionPlayerState` on the predicted state: its ring's new
        // events fire this frame, and the snapshots' copies of them never
        // (`walk_snapshot` leaves a predicted ring alone).
        // `cg.lastFrame.aimSpreadScale`: the last frame's, which an own
        // shot's spread reads (cod4-effects.md 12.2).
        let last_scale = self
            .last_predicted
            .as_ref()
            .map_or(0.0, |p| p.aim_spread_scale);
        if let Some(p) = &predicted {
            self.last_predicted = Some(p.ps.clone());
        }
        if let Some(p) = &predicted {
            let ring = cod4_predict::Ring::of(&p.ps);
            let from = self.fired.len();
            if let Some(old) = &self.ps_ring {
                for k in cod4_predict::new_events(&ring, old) {
                    let id = p.ps.events[k];
                    let Some(name) = event_name(id).filter(|_| id != 0) else {
                        continue;
                    };
                    if name == "EV_NOAMMO" {
                        self.ran_dry = Some(Dry::of(&p.ps));
                    }
                    self.fired.push(Fired {
                        id,
                        name,
                        entity: None,
                        server_time: p.ps.command_time,
                        parm: p.ps.event_parms[k],
                        origin: p.ps.origin,
                        state: None,
                        weapon: p.ps.weapon,
                        perks: client_perks(new, p.ps.client_num),
                        aim: name
                            .starts_with("EV_FIRE_WEAPON")
                            .then(|| self.level_def(p.ps.weapon))
                            .flatten()
                            .map(|d| crate::cod4_fx::Aim {
                                from: p.event_aims[k],
                                ..crate::cod4_fx::Aim::own(&p.ps, d, last_scale, p.event_times[k])
                            }),
                    });
                }
            }
            self.ps_ring = Some(ring);
            let own = self.fired.split_off(from);
            force_stance(&mut self.input, &own);
            self.fired.extend(own);
        }
        let u = |name: &str| ps_u(&new.ps, name);
        // `CG_KickAngles` on this frame, then the own shots' recoil and
        // landings fired since the last (cod4-client-view.md 14).
        {
            let ps = predicted.as_ref().filter(|p| p.armed).map(|p| &p.ps);
            let frac = ps.map_or(f(&new.ps, "fWeaponPosFrac"), |p| p.weapon_pos_frac);
            let restrict = ps.map_or(u("weaponRestrictKickTime") as i32, |p| {
                p.weapon_restrict_kick_time
            });
            let weapon = if ps.map_or(u("weapFlags"), |p| p.weap_flags) & 2 != 0 {
                u("offHandIndex")
            } else {
                ps.map_or(u("weapon"), |p| p.weapon)
            };
            let def = weapon_name(&weapons, weapon)
                .and_then(|n| self.world.as_ref()?.kicks.get(n).cloned());
            let other = u("otherFlags");
            self.kick.check_respawn(u("clientNum"), new.ps.stats[4]);
            self.kick.step(render_time, def.as_ref(), frac, other);
            let heights = {
                let r = cod4_predict::rules_from_mirror(|i| gs.configstring(i).to_vec());
                (r.fall_damage_min_height, r.fall_damage_max_height)
            };
            for fd in self.fired.iter().skip(self.kick_seen) {
                if fd.entity.is_some() {
                    continue;
                }
                if let Some(d) = def.as_ref().filter(|_| cod4_kick::is_fire(fd.name)) {
                    self.kick.fire(d, other, frac, restrict);
                }
                self.kick.land(fd.name, fd.parm, heights, render_time);
            }
            self.kick_seen = self.fired.len();
        }
        let mut velocity = Vec3::new(lerp1("velocity[0]"), lerp1("velocity[1]"), 0.0);
        let mut feet = vec3(|k| lerp1(&format!("origin[{k}]")));
        if let Some(p) = &predicted {
            let ps = &p.ps;
            feet = p.origin;
            eye = p.origin + Vec3::Z * ps.view_height_current;
            angles = Vec3::from_array(ps.view_angles);
            velocity = ps.velocity;
        }
        // `CG_OffsetFirstPersonView`'s landing dip, then its lean
        // (`AddLeanToPosition` with 16
        // and 20, nothing rolls the view), then the floor 8 over the feet
        // (cod4-client-view.md 7).
        let land = self.kick.landing(render_time);
        // Not on a gun (`eFlags` 0x300, 0x450063).
        if u("eFlags") & 0x300 == 0 {
            eye.z += cod4_kick::eye_dip(land);
        }
        let leanf = predicted.as_ref().map_or(lerp1("leanf"), |p| p.ps.leanf);
        if leanf != 0.0 {
            eye = vcod_common::pmove::aim::add_lean(
                eye,
                vcod_common::x87::radians(angles.y),
                leanf,
                vcod_common::patch::Arith::Msvc,
            );
        }
        let light_eye = eye;
        if leanf != 0.0 {
            eye.z = eye.z.max(feet.z + 8.0);
        }
        let pick = |pred: fn(&pm::PlayerState) -> u32, name: &str| {
            predicted.as_ref().map_or(u(name), |p| pred(&p.ps))
        };
        // The weapon half's fields, where it ran.
        let armed = predicted.as_ref().filter(|p| p.armed);
        let pick_armed =
            |pred: fn(&pm::PlayerState) -> u32, name: &str| armed.map_or(u(name), |p| pred(&p.ps));
        let view = crate::cod4_viewmodel::ViewPs {
            client_num: u("clientNum"),
            pm_type: u("pm_type"),
            pm_flags: pick(|p| p.pm_flags, "pm_flags"),
            eflags: pick(|p| p.e_flags, "eFlags"),
            weap_flags: pick_armed(|p| p.weap_flags, "weapFlags"),
            weapon: pick_armed(|p| p.weapon, "weapon"),
            off_hand: u("offHandIndex"),
            weap_anim: pick_armed(|p| p.weap_anim, "weapAnim"),
            weaponstate: pick_armed(|p| p.weapon_state as u32, "weaponstate"),
            weapon_time: pick_armed(|p| p.weapon_time as u32, "weaponTime") as i32,
            pos_frac: armed.map_or(lerp1("fWeaponPosFrac"), |p| p.ps.weapon_pos_frac),
            bob_cycle: pick(|p| p.bob_cycle as u32, "bobCycle") & 0xff,
            xyspeed: predicted
                .as_ref()
                .map_or(velocity.length(), |p| cod4_predict::xyspeed(&p.ps)),
            speed: u("speed") as i32 as f32,
            view_height_target: pick(|p| p.view_height_target as u32, "viewHeightTarget") as i32,
            view_height_current: predicted
                .as_ref()
                .map_or(f(&new.ps, "viewHeightCurrent"), |p| {
                    p.ps.view_height_current
                }),
            leanf: predicted.as_ref().map_or(lerp1("leanf"), |p| p.ps.leanf),
            viewmodel_index: u("viewmodelIndex"),
            angles,
            ammoclip: armed.map_or(new.ps.ammoclip, |p| p.ps.ammoclip),
            weapon_models: new.ps.weapon_models,
            aim_spread_scale: armed.map_or(f(&new.ps, "aimSpreadScale"), |p| p.ps.aim_spread_scale),
            perks: u("perks"),
            grenade_time_left: pick_armed(|p| p.grenade_time_left as u32, "grenadeTimeLeft") as i32,
            spread_override: u("spreadOverride") as i32,
            spread_override_state: u("spreadOverrideState") as i32,
            origin: feet,
            other_flags: u("otherFlags"),
            gun_kick: self.kick.gun_kick(),
            land,
            damage: None,
            viewlocked: u("viewlocked"),
            viewlocked_ent: u("viewlocked_entNum"),
            gun_dir: None,
        };
        let hands = model_cs(gs, view.viewmodel_index);
        Some(Scene {
            eye,
            light_eye,
            angles,
            entities,
            server_time: new.server_time,
            render_time,
            following: !own,
            view_reset: reset,
            view,
            hands,
            dof: crate::cod4_dof::PsDof {
                scripted: [
                    "dofNearStart",
                    "dofNearEnd",
                    "dofFarStart",
                    "dofFarEnd",
                    "dofNearBlur",
                    "dofFarBlur",
                ]
                .map(|n| f(&new.ps, n)),
                viewmodel: ["dofViewmodelStart", "dofViewmodelEnd"].map(|n| f(&new.ps, n)),
            },
        })
    }
}

/// `CG_PredictPlayerState` predicts `ps` (cod4-client-view.md 5): our own
/// view, not one we follow (`otherFlags` 2) nor a linked body, once a
/// prediction world was handed in.
fn predicts(world: bool, gs: &Gamestate, ps: &PlayerState) -> bool {
    let u = |f: &str| ps.get(f).unwrap_or(0);
    let linked = matches!(
        u("pm_type") as i32,
        pm::pm_type::NORMAL_LINKED | pm::pm_type::DEAD_LINKED
    );
    world && u("clientNum") == gs.client_num as u32 && u("otherFlags") & 2 == 0 && !linked
}

/// The longest gap between a capture's cmds [`eased_cmd`] eases across,
/// ms, and the fastest turn, degrees a ms: a faster one is a snap (a
/// scripted view, or the client making up a `delta_angles` change),
/// which the view takes at once.
const EASE_MAX_MS: i32 = 250;
const EASE_MAX_TURN: f32 = 0.36;

/// `next` with its view eased from `prev`'s by how far `now` is between
/// their times, each angle the short way round in `SHORT` units: a
/// capture's client sent a cmd every 16 to 100 ms, and its view would
/// otherwise turn in those steps. Replay-only smoothing.
fn eased_cmd(prev: &Cmd, next: &Cmd, now: i32) -> Cmd {
    let span = next.server_time - prev.server_time;
    let d: [i16; 3] = std::array::from_fn(|k| next.angles[k].wrapping_sub(prev.angles[k]) as i16);
    let fastest = d.iter().map(|d| f32::from(*d).abs()).fold(0.0, f32::max) * (360.0 / 65536.0);
    if span <= 0 || span > EASE_MAX_MS || fastest > EASE_MAX_TURN * span as f32 {
        return *next;
    }
    let f = ((now - prev.server_time) as f32 / span as f32).clamp(0.0, 1.0);
    let angles = std::array::from_fn(|k| {
        prev.angles[k].wrapping_add((f32::from(d[k]) * f).round() as i16 as u16)
    });
    Cmd { angles, ..*next }
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

/// How [`Cod4Session::connect_with`] fetches what the install lacks.
pub struct Fetch<'a> {
    /// The install root the files land in, at their referenced paths.
    pub root: &'a std::path::Path,
    /// `cl_allowDownload`.
    pub allow: bool,
    /// `cl_wwwDownload`: sent in the userinfo, and a redirect fetched over
    /// HTTP.
    pub www: bool,
    /// Told the file in flight while a download runs, and `None` once
    /// the downloads end, for the connect screen.
    pub progress: Option<&'a mut dyn FnMut(Option<DownloadProgress>)>,
    /// Esc on the connect screen: set, the join's next read fails
    /// (`conn::Cancelled`).
    pub cancel: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
    /// The connect screen's `print` lines and stages
    /// (`vcod_cod4::net::conn::JoinNote`).
    pub notes: Option<vcod_cod4::net::conn::Notes>,
}

impl Fetch<'_> {
    fn report(&mut self, p: Option<DownloadProgress>) {
        if let Some(f) = self.progress.as_mut() {
            f(p);
        }
    }
}

/// `CL_InitDownloads` (iw3mp.exe 0x46ac60) on each gamestate of a live
/// join, as `CL_ParseGamestate` runs it (0x47423e): the referenced files
/// `fetch.root` lacks (`net::download::missing_in`), each fetched over
/// `svc_download` or the HTTP redirect with no usercmds sent meanwhile,
/// then `donedl` and the compare again on the gamestate it brings. A
/// refusal ends the join with its reason; with `allow` off a missing file
/// ends it as retail's `EXE_...`-less text does (0x6d0910). Retail fetches
/// a file that still differs after its download again on every
/// gamestate; vcod ends the join on the second ask (`fetched` holds what
/// this connect landed, across the joins of a disconnected fetch). `None` when a disconnected HTTP fetch landed and
/// the caller joins again.
fn fetch_missing(
    mut conn: Conn,
    fetch: &mut Fetch,
    fetched: &mut std::collections::HashSet<String>,
) -> Result<Option<Conn>> {
    loop {
        let gs = conn.client.gamestate.as_ref().context("gamestate")?;
        let si = String::from_utf8_lossy(gs.configstring(1)).into_owned();
        let map = info_value(gs.configstring(CS_SERVERINFO), "mapname");
        let wanted = vcod_cod4::net::download::missing_in(fetch.root, &si, map.as_deref());
        if wanted.is_empty() {
            return Ok(Some(conn));
        }
        let names: Vec<&str> = wanted.iter().map(|w| w.remote.as_str()).collect();
        log::info!("cod4: need files: {}", names.join(" "));
        if !fetch.allow {
            bail!(
                "You are missing some files referenced by the server: {}\nGo to the Multiplayer \
                 options menu to allow downloads",
                names.join(" ")
            );
        }
        if let Some(again) = names.iter().find(|n| fetched.contains(**n)) {
            return Err(vcod_cod4::net::conn::Refused(format!(
                "{again} is different from the server after its download"
            ))
            .into());
        }
        match download_round(conn, fetch, fetched, wanted)? {
            Some(c) => conn = c,
            None => return Ok(None),
        }
    }
}

/// `CL_NextDownload` through the list, then `donedl` and the gamestate it
/// brings (or nothing asked for); each file that lands goes in `fetched`.
/// `None` when a disconnected HTTP fetch landed: join again.
fn download_round(
    mut conn: Conn,
    fetch: &mut Fetch,
    fetched: &mut std::collections::HashSet<String>,
    wanted: Vec<vcod_cod4::net::download::Wanted>,
) -> Result<Option<Conn>> {
    use vcod_cod4::net::download::{DownloadEvent, Downloads};
    let mut dls = Downloads::new(fetch.root, wanted).with_www(fetch.www);
    let mut events = dls.next_download();
    let mut waiting = false;
    let mut last_heard = clock_now();
    let mut last_report = clock_now();
    let mut shown: Option<DownloadProgress> = None;
    loop {
        events.extend(dls.poll());
        for e in events.drain(..) {
            match e {
                DownloadEvent::Command(c) if c == "donedl" => {
                    conn.done_downloading();
                    waiting = true;
                }
                DownloadEvent::Command(c) if c == "wwwdl fail" => {
                    conn.www_failed();
                }
                DownloadEvent::Command(c) => {
                    conn.command(c);
                }
                DownloadEvent::Finished(name, p, size) => {
                    log::info!("cod4: downloaded {}", p.display());
                    // The file whole, though a fast fetch can land before
                    // a look ever saw it (or saw its size).
                    shown = None;
                    fetch.report(Some(DownloadProgress {
                        file: name.clone(),
                        bytes: size,
                        total: size,
                    }));
                    fetched.insert(name);
                }
                DownloadEvent::Skipped(why) => log::warn!("cod4: {why}"),
                DownloadEvent::Failed(why) => {
                    fetch.report(None);
                    return Err(vcod_cod4::net::conn::Refused(why).into());
                }
                DownloadEvent::Reconnect => {
                    fetch.report(None);
                    let _ = conn.disconnect();
                    return Ok(None);
                }
            }
        }
        if dls.is_done() && !waiting {
            fetch.report(None);
            return Ok(Some(conn));
        }
        // A disconnected fetch hears nothing from the server it left.
        if clock_now().saturating_duration_since(last_heard) > Duration::from_secs(30)
            && !dls.is_disconnected()
        {
            bail!("{}: the download stalled", conn.server());
        }
        let now = dls.progress();
        if now != shown {
            shown = now.clone();
            fetch.report(now);
        }
        if clock_now().saturating_duration_since(last_report) > Duration::from_secs(1)
            && let Some(p) = &shown
        {
            last_report = clock_now();
            log::info!(
                "cod4: downloading {}: {} of {} bytes",
                p.file,
                p.bytes,
                p.total
            );
        }
        conn.send(&[])?;
        for got in conn.poll(clock_now() + Duration::from_millis(50))? {
            last_heard = clock_now();
            for op in &got.parsed.ops {
                match op {
                    Op::Download(m) => events.extend(dls.on_message(m)),
                    Op::Gamestate if waiting => {
                        fetch.report(None);
                        return Ok(Some(conn));
                    }
                    Op::Gamestate => events.extend(dls.on_gamestate()),
                    _ => {}
                }
            }
        }
    }
}

/// `vcod --game cod4 --net-probe ADDR [--team T [--class C]]`: joins a
/// server headless for `secs` through the same session steps as the
/// window (join, `cp`, the menus answered, a cmd a frame, events, the
/// scene), a line a second, and the menus opened and the spawn as they
/// come.
/// With `install`, the files the server references that it lacks are
/// downloaded into it first.
pub fn probe(
    addr: SocketAddr,
    secs: u64,
    name: &str,
    cp: Cp,
    join: MenuJoin,
    install: Option<&std::path::Path>,
) -> Result<()> {
    let fetch = install.map(|root| Fetch {
        root,
        allow: true,
        www: true,
        progress: None,
        cancel: None,
        notes: None,
    });
    let mut s = Cod4Session::connect_with(addr, name, cp, join, fetch)?;
    println!(
        "cod4: joined {addr}, map {}",
        s.map_name().unwrap_or_default()
    );
    let t0 = Instant::now();
    let mut next = 0;
    let (mut menus, mut spawned) = (0, false);
    while t0.elapsed() < Duration::from_secs(secs) {
        let ms = t0.elapsed().as_secs_f64() * 1000.0;
        s.step(ms)?;
        for m in &s.menus.opened[menus..] {
            println!("{ms:.0} menu {m}");
        }
        menus = s.menus.opened.len();
        if let Some(sp) = s.spawn().filter(|_| !spawned) {
            spawned = true;
            println!("{ms:.0} spawned {sp}");
        }
        for f in s.take_fired() {
            println!("{ms:.0} event {} entity {:?}", f.name, f.entity);
        }
        for l in s.take_lines() {
            println!("{ms:.0} {l}");
        }
        if t0.elapsed().as_secs() >= next {
            next = t0.elapsed().as_secs() + 1;
            if let Some(sc) = s.scene() {
                let kinds: Vec<u32> = sc.entities.iter().map(|d| d.etype).collect();
                let players: Vec<String> = sc
                    .entities
                    .iter()
                    .filter(|d| d.etype == ET_PLAYER)
                    .map(|d| {
                        format!(
                            "{} ({:.0} {:.0} {:.0})",
                            d.number, d.origin.x, d.origin.y, d.origin.z
                        )
                    })
                    .collect();
                println!(
                    "t={next} snapshots {} time {} eye ({:.0} {:.0} {:.0}) drawn {kinds:?} players {}",
                    s.client().snapshots.len(),
                    sc.server_time,
                    sc.eye.x,
                    sc.eye.y,
                    sc.eye.z,
                    players.join(", ")
                );
            }
        }
        std::thread::sleep(Duration::from_millis(16));
    }
    s.disconnect();
    Ok(())
}

/// Weapon `n`'s name.
pub fn weapon_name(weapons: &[String], n: u32) -> Option<&str> {
    weapons
        .get((n as usize).checked_sub(1)?)
        .map(String::as_str)
}

fn model_cs(gs: &Gamestate, index: u32) -> Option<String> {
    let i = u16::try_from(index)
        .ok()
        .filter(|&i| i > 0 && i < MAX_MODELS)?;
    let s = String::from_utf8_lossy(gs.configstring(CS_MODELS + i)).into_owned();
    (!s.is_empty()).then_some(s)
}

/// A client's body model: its `clientState.modelindex`.
fn body_model(gs: &Gamestate, snap: &Snapshot, client: u32) -> Option<String> {
    let c = snap.clients.iter().find(|c| c.0[0] == client)?;
    model_cs(gs, client_u32(c, "modelindex"))
}

/// What `e` draws as; `None` for what is never drawn (event entities, an
/// entity with no model, a missile's or item's that has none).
fn visual(gs: &Gamestate, snap: &Snapshot, weapons: &[String], e: &EntityState) -> Option<Visual> {
    let index = ent_u32(e, "index");
    let weapon = |n: u32| weapon_name(weapons, n).map(str::to_string);
    Some(match e.etype() {
        ET_PLAYER | ET_PLAYER_CORPSE => {
            let client = ent_u32(e, "clientNum");
            body_model(gs, snap, client).map_or(Visual::Box, Visual::Model)
        }
        ET_SCRIPTMOVER if ent_u32(e, "solid") == SOLID_BMODEL => {
            (index > 0).then_some(Visual::Submodel(index as usize))?
        }
        // `CG_ScriptMover` draws a plane its `index` model
        // (`CG_ProcessEntity`).
        ET_SCRIPTMOVER | ET_GENERAL | ET_PLANE | ET_MG42 => Visual::Model(model_cs(gs, index)?),
        // A helicopter's model rides `un2`, its `index` 0
        // (`play-1.7-hardpoint.txt`, cod4-gsc.md 3.2); a vcod-server from
        // before that capture sent it in `index`.
        ET_HELICOPTER => {
            let model = match ent_u32(e, "un2") {
                0 => index,
                m => m,
            };
            Visual::Model(model_cs(gs, model)?)
        }
        // An item's `index` is its weapon (the kill capture's dropped M16,
        // cod4-netfields.md 10); the client draws the weapon's world model.
        ET_ITEM => Visual::Model(format!("weapon:{}", weapon(index)?)),
        ET_MISSILE => Visual::Model(format!("projectile:{}", weapon(ent_u32(e, "weapon"))?)),
        _ => return None,
    })
}

/// Whether an entity lerps between its two snapshots: its older state is
/// `TR_INTERPOLATE` (the newer not `TR_PHYSICS`), or it is a client on
/// `TR_LINEAR_STOP`; anything else runs its trajectory
/// (cod4-player-anims.md 6; INFERRED, not yet confirmed on 1.7).
fn interpolates(old: &EntityState, new: &EntityState) -> bool {
    let t = ent_u32(old, "lerp.pos.trType");
    (t == TR_INTERPOLATE && ent_u32(new, "lerp.pos.trType") != TR_PHYSICS)
        || (t == TR_LINEAR_STOP && old.number() < 64)
}

/// CoD4's `trType`s (`trType_t`): CoD2's up to `TR_GRAVITY`, then
/// `TR_ACCELERATE` 6, `TR_DECELERATE` 7, `TR_PHYSICS` 8 and the ragdoll
/// types 9 to 11.
const TR_INTERPOLATE: u32 = 1;
const TR_LINEAR_STOP: u32 = 3;
const TR_PHYSICS: u32 = 8;

/// `lerp.<prefix>` as a CoD2 trajectory, CoD4's type renumbered onto
/// CoD2's (`BG_EvaluateTrajectory` treats `TR_PHYSICS` and
/// `TR_RAGDOLL_INTERPOLATE` as stationary, `TR_FIRST_RAGDOLL` as linear and
/// `TR_RAGDOLL_GRAVITY` as gravity).
fn trajectory(e: &EntityState, prefix: &str) -> Trajectory2 {
    let f = |name: &str| ent_u32(e, &format!("lerp.{prefix}.{name}"));
    let v = |name: &str| vec3(|k| ent_f32(e, &format!("lerp.{prefix}.{name}[{k}]")));
    let tr_type = match f("trType") {
        t @ 0..=5 => t as i32,
        6 => tr2::TR_ACCELERATE,
        7 => tr2::TR_DECELERATE,
        9 => tr2::TR_LINEAR,
        10 => tr2::TR_GRAVITY,
        _ => tr2::TR_STATIONARY,
    };
    Trajectory2 {
        tr_type,
        tr_time: f("trTime") as i32,
        tr_duration: f("trDuration") as i32,
        base: v("trBase"),
        delta: v("trDelta"),
    }
}

/// CoD4's `eFlags` in CoD2's numbering: the same bits but the mantle,
/// 0x8000 on CoD4 and 0x4000 on CoD2 (cod4-pmove.md 17.1,
/// cod4-player-anims.md 3; INFERRED, not yet confirmed on 1.7).
pub fn cod2_flags(e: u32) -> i32 {
    const MANTLE: u32 = 0x8000;
    let kept = (e & !(MANTLE | 0x4000)) as i32;
    if e & MANTLE != 0 {
        kept | cod2_eflags::MANTLE
    } else {
        kept
    }
}

/// A player's or corpse's [`BodyView`]: the client state's body and its
/// six attachments on their tags (an empty tag grafts by the
/// attachment's root bone: cod4-player-anims.md 5, INFERRED, not yet
/// confirmed on 1.7), the entity's anims, flags and angles.
fn body_view(
    gs: &Gamestate,
    snap: &Snapshot,
    weapons: &[String],
    e: &EntityState,
    origin: Vec3,
    angles: Vec3,
) -> Option<BodyView> {
    if !matches!(e.etype(), ET_PLAYER | ET_PLAYER_CORPSE) {
        return None;
    }
    let client = ent_u32(e, "clientNum");
    let c = snap.clients.iter().find(|c| c.0[0] == client)?;
    let body = model_cs(gs, client_u32(c, "modelindex"))?;
    let attachments = (0..6)
        .filter_map(|i| {
            let model = model_cs(gs, client_u32(c, &format!("attachModelIndex[{i}]")))?;
            let tag = client_u32(c, &format!("attachTagIndex[{i}]"));
            let tag = u16::try_from(tag)
                .ok()
                .map(|t| String::from_utf8_lossy(gs.configstring(CS_TAGS + t)).into_owned())
                .filter(|t| !t.is_empty());
            Some((model, tag))
        })
        .collect();
    let frame = BodyFrame {
        origin,
        angles: angles.to_array(),
        legs_anim: ent_u32(e, "legsAnim") as i32,
        torso_anim: ent_u32(e, "torsoAnim") as i32,
        eflags: cod2_flags(ent_u32(e, "lerp.eFlags")),
        movement_dir: movement_dir(e),
        lean: ent_f32(e, "lerp.u.player.leanf"),
        torso_height: 0.0,
        torso_pitch: ent_f32(e, "fTorsoPitch"),
        waist_pitch: ent_f32(e, "fWaistPitch"),
        client_num: client as i32,
    };
    Some(BodyView {
        frame,
        character: Character {
            body,
            attachments,
            ignore_collision: 0,
        },
        // `BG_UpdatePlayerDObj` builds a gunner's body without his own
        // weapon (cod4-player-anims.md 5).
        weapon: weapon_name(weapons, ent_u32(e, "weapon"))
            .filter(|_| ent_u32(e, "lerp.eFlags") & 0x300 == 0)
            .map(str::to_string),
        weapon_model: ent_u32(e, "weaponModel") as usize,
        corpse: e.etype() == ET_PLAYER_CORPSE,
        ragdoll: (9..=11).contains(&ent_u32(e, "lerp.pos.trType")),
        gun: Some(ent_u32(e, "otherEntityNum"))
            .filter(|&g| ent_u32(e, "lerp.eFlags") & 0x300 != 0 && (64..1023).contains(&g))
            .map(|g| {
                let gun = snap.entities.iter().find(|x| x.number() == g);
                let weapon = gun.and_then(|x| weapon_name(weapons, ent_u32(x, "weapon")));
                (g, weapon.map(str::to_string))
            }),
    })
}

/// A player's move direction: a signed byte of degrees
/// (`BG_PlayerStateToEntityState`).
fn movement_dir(e: &EntityState) -> f32 {
    f32::from(ent_u32(e, "lerp.u.player.movementDir") as u8 as i8)
}

/// `a` to `b` by `t` the way the cgame steps an angle: the difference
/// wrapped into -180..180 through `floor`.
fn lerp_wrapped(a: f32, b: f32, t: f32) -> f32 {
    let d = (b - a) * (1.0 / 360.0);
    (d - (d + 0.5).floor()) * 360.0 * t + a
}

/// Angles stepped the short way round.
fn lerp_angles(a: Vec3, b: Vec3, t: f32) -> Vec3 {
    let d = |x: f32, y: f32| {
        let mut d = (y - x) % 360.0;
        if d > 180.0 {
            d -= 360.0;
        } else if d < -180.0 {
            d += 360.0;
        }
        x + d * t
    };
    Vec3::new(d(a.x, b.x), d(a.y, b.y), d(a.z, b.z))
}

type Walked = (
    Vec<Fired>,
    Vec<String>,
    Option<Ring>,
    HashMap<u32, u32>,
    HashSet<u32>,
);

/// One snapshot's events against the state the last one left: the
/// playerstate ring past `ps_ring` unless the frame predicts it
/// (`predicted`: the predicted ring fires instead, [`Cod4Session::scene`]),
/// each entity's ring past its last `eventSequence` (an entity entering
/// starts its count), and each event entity that was not in the last
/// snapshot.
/// The client a source reads, borrowed apart from the session's other
/// fields.
fn source_client(source: &Source) -> &Client {
    match source {
        Source::Capture { session, .. } => &session.client,
        Source::Live { conn, .. } => &conn.client,
        Source::Demo(d) => &d.client,
    }
}

fn walk_snapshot(
    c: &Client,
    s: &Snapshot,
    ps_ring: Option<Ring>,
    predicted: bool,
    rings: &HashMap<u32, u32>,
    temps: &HashSet<u32>,
) -> Walked {
    let mut fired = Vec::new();
    let mut lines = Vec::new();
    let t = s.server_time;
    // `ev(k)` is ring slot `k`'s event and parm.
    let ring =
        |seq_from: u32, seq_to: u32, ev: &dyn Fn(u32) -> (u32, u32), ent: Option<&EntityState>| {
            let mut out = Vec::new();
            // At most the ring's four.
            let from = seq_from.max(seq_to.saturating_sub(4));
            for q in from..seq_to {
                let (id, parm) = ev(q & 3);
                if let Some(name) = event_name(id).filter(|_| id != 0) {
                    out.push(Fired {
                        id,
                        name,
                        entity: ent.map(EntityState::number),
                        server_time: t,
                        parm,
                        origin: ent.map_or_else(
                            || vec3(|k| ps_f32(&s.ps, &format!("origin[{k}]"))),
                            origin_of,
                        ),
                        state: ent.copied(),
                        weapon: ent
                            .map_or(s.ps.get("weapon").unwrap_or(0), |e| ent_u32(e, "weapon")),
                        perks: client_perks(
                            s,
                            ent.map_or(s.ps.get("clientNum").unwrap_or(0), |e| {
                                ent_u32(e, "clientNum")
                            }),
                        ),
                        aim: None,
                    });
                }
            }
            out
        };
    let seq = s.ps.get("eventSequence").unwrap_or(0) & 0xff;
    let get = |n: String| s.ps.get(&n).unwrap_or(0);
    let mut own_ring = ps_ring;
    // A predicted view with no ring yet (no scene since the gamestate)
    // starts from the snapshot's, so the first predicted frame fires what
    // its cmds raised past it instead of taking them as the baseline.
    if !predicted || ps_ring.is_none() {
        if let Some(prev) = ps_ring.map(|r| r.sequence as u32) {
            let to = if seq < prev { seq + 256 } else { seq };
            let ev = |k: u32| (get(format!("events[{k}]")), get(format!("eventParms[{k}]")));
            fired.extend(ring(prev, to, &ev, None));
        }
        own_ring = Some(Ring {
            sequence: seq as i32,
            events: std::array::from_fn(|k| get(format!("events[{k}]"))),
        });
    }
    let mut new_rings = HashMap::new();
    let mut new_temps = HashSet::new();
    let names: HashMap<u32, String> = s
        .clients
        .iter()
        .map(|cl| (cl.0[0], client_name(cl)))
        .collect();
    let weapons = c.gamestate.as_ref().map(weapon_list).unwrap_or_default();
    for e in &s.entities {
        let n = e.number();
        if e.etype() >= ET_EVENTS {
            new_temps.insert(n);
            if temps.contains(&n) {
                continue;
            }
            let id = e.etype() - ET_EVENTS;
            if let Some(name) = event_name(id) {
                fired.push(Fired {
                    id,
                    name,
                    entity: Some(n),
                    server_time: t,
                    parm: ent_u32(e, "eventParm"),
                    origin: origin_of(e),
                    state: Some(*e),
                    weapon: ent_u32(e, "weapon"),
                    perks: client_perks(s, ent_u32(e, "clientNum")),
                    aim: None,
                });
            }
            if id == EV_OBITUARY {
                let who = |k: u32| {
                    names
                        .get(&k)
                        .cloned()
                        .unwrap_or_else(|| format!("entity {k}"))
                };
                let victim = ent_u32(e, "otherEntityNum");
                let attacker = ent_u32(e, "attackerEntityNum");
                let weapon = weapon_name(&weapons, ent_u32(e, "eventParm")).unwrap_or_default();
                lines.push(if attacker == victim || attacker >= 64 {
                    format!("{} died ({weapon})", who(victim))
                } else {
                    format!("{} killed {} ({weapon})", who(attacker), who(victim))
                });
            }
            continue;
        }
        let seq = ent_u32(e, "eventSequence") & 0xff;
        new_rings.insert(n, seq);
        // A general entity or a missile entering replays its ring from
        // 0, unless it is a missile launched over 200 ms ago (`eFlags`
        // 0x10000); the frag's explode rides such an entity
        // (cod4-effects.md 8; the rule INFERRED, not yet confirmed on 1.7).
        let entering = match e.etype() {
            ET_GENERAL | ET_MISSILE => {
                let launched = ent_u32(e, "lerp.u.anonymous.data[0]") as i32;
                let old = ent_u32(e, "lerp.eFlags") & 0x10000 != 0 && t - launched > 200;
                (!old).then_some(0)
            }
            _ => None,
        };
        if let Some(prev) = rings.get(&n).copied().or(entering) {
            let to = if seq < prev { seq + 256 } else { seq };
            let ev = |k: u32| {
                (
                    ent_u32(e, &format!("events[{k}]")),
                    ent_u32(e, &format!("eventParms[{k}]")),
                )
            };
            fired.extend(ring(prev, to, &ev, Some(e)));
        }
    }
    (fired, lines, own_ring, new_rings, new_temps)
}

/// An entity's `lerp.pos.trBase`.
pub fn origin_of(e: &EntityState) -> Vec3 {
    vec3(|k| ent_f32(e, &format!("lerp.pos.trBase[{k}]")))
}

/// Where `e`'s trajectory puts it at `time` (`BG_EvaluateTrajectory`).
pub fn position_at(e: &EntityState, time: i32) -> Vec3 {
    trajectory(e, "pos").evaluate(time)
}

/// `e`'s yaw at `time` off its angle trajectory.
pub fn yaw_at(e: &EntityState, time: i32) -> f32 {
    angles_at(e, time).y
}

/// `e`'s angles at `time` off its angle trajectory.
pub fn angles_at(e: &EntityState, time: i32) -> Vec3 {
    trajectory(e, "apos").evaluate(time)
}

/// This frame's cmd on the newest snapshot's time plus its age (0 before
/// one), `view` over the playerstate's `delta_angles`, the keys, the
/// selected weapon and offhand (`play::cod4_input`).
/// `CL_CGameNeedsServerCommand`'s `w` (`iw3mp.exe` 0x45b1d9): the server
/// dropped us, `w "<reason>"` (`CL_DisconnectError`), or with no reason
/// `EXE_SERVER_DISCONNECTED`; a `PB` third argument is PunkBuster's kick,
/// its reason the same. `None` for any other command.
fn drop_reason(text: &[u8]) -> Option<String> {
    let text = String::from_utf8_lossy(text);
    let args = vcod_cod4::hud::tokenize(&text);
    // The switch reads the first token's first character.
    if !args.first()?.starts_with('w') {
        return None;
    }
    Some(match args.get(1) {
        Some(why) => format!("EXE_SERVERDISCONNECTREASON\u{15}{why}"),
        None => "EXE_SERVER_DISCONNECTED".into(),
    })
}

/// A drop's reason ([`Cod4Session::take_dropped`]) in the install's own
/// words: an `EXE_*` key translated, `EXE_SERVERDISCONNECTREASON` with
/// its `&&1` the translated reason after the `\x15` (`CL_DisconnectError`,
/// `iw3mp.exe` 0x46b4e0); a key the strings lack stays as it is.
pub fn localize_drop(why: &str, install: &std::path::Path) -> String {
    let strings = vcod_cod4::install::Install::at(install)
        .and_then(|i| i.code_strings("english").ok())
        .unwrap_or_default();
    let tr = |k: &str| strings.get(k).cloned().unwrap_or_else(|| k.to_string());
    match why.split_once('\u{15}') {
        Some((outer, arg)) => tr(outer).replace("&&1", &tr(arg)),
        None => tr(why),
    }
}

/// [`Cod4Session::out_of_ammo_change`]'s pick over the level's `defs`,
/// the selection `selected` and the held primaries by index.
fn out_of_ammo_pick(defs: &[WeaponDef], d: &Dry, selected: u8, primaries: &[u8]) -> Option<u8> {
    if d.pm_type >= pm::pm_type::DEAD {
        return None;
    }
    let def = defs.get(d.weapon as usize)?;
    if def.cancel_auto_holster_when_empty {
        return None;
    }
    if def.inventory_type == INVENTORY_ALT_MODE {
        // The primary cycle on an alt mode selects its gun as well.
        let alt = def.alt_weapon as u8;
        return Some(match crate::play::cod4_input::holds(d.held, alt) {
            true => alt,
            false => 0,
        });
    }
    let has_ammo = |w: u8| {
        defs.get(usize::from(w)).is_some_and(|x| {
            d.ammoclip.get(x.clip_index).copied().unwrap_or(0)
                + d.ammo.get(x.ammo_index).copied().unwrap_or(0)
                > 0
        })
    };
    primaries
        .iter()
        .copied()
        .filter(|&w| w != selected && has_ammo(w))
        .min_by_key(|&w| (w < selected, w))
}

/// `locationSelectionInfo` of a client's newest snapshot.
fn ps_u_last(c: &Client) -> u32 {
    c.snapshots
        .last()
        .and_then(|s| s.ps.get("locationSelectionInfo"))
        .unwrap_or(0)
}

/// `CL_CreateCmd`'s location selector arm, INFERRED: while
/// `loc` is set the cmd carries only its stance, 0x100000, and a confirm
/// (the `+attack` press, with the cursor in `selectedLocation` as
/// `round(at * 255) + 0x80`) or a cancel (Esc); without it the cursor
/// goes back to the map's middle (`CG_DrawActiveFrame`).
fn selector_cmd(sel: &mut Selector, c: &mut UserCmd, loc: u32) {
    let attack = c.buttons & vcod_cod4::pmove::button::ATTACK != 0;
    if loc == 0 {
        *sel = Selector {
            attack_was: attack,
            ..Selector::default()
        };
        return;
    }
    if attack && !sel.attack_was {
        sel.confirm = true;
    }
    sel.attack_was = attack;
    use vcod_cod4::pmove::button::{CROUCH, PRONE};
    c.buttons = c.buttons & (CROUCH | PRONE) | BUTTON_SELECTING;
    if std::mem::take(&mut sel.confirm) {
        c.buttons |= BUTTON_CONFIRM_LOCATION;
        c.selected_location = sel
            .at
            .map(|v| ((v * 255.0 + 0.5).floor() as i32 + 0x80) as u8);
    } else if std::mem::take(&mut sel.cancel) {
        c.buttons |= BUTTON_CANCEL_LOCATION;
    }
}

/// `j_head` and `j_helmet` over the origin by stance (standing, crouched,
/// prone): the bind pose's, as the pick reads them without the posed
/// bodies (cod4-combat.md 10.4).
const AIM_HEIGHTS: [(f32, f32); 3] = [(64.0, 70.0), (44.0, 50.0), (12.0, 18.0)];

/// The cmd's melee charge (`CL_CreateCmd`'s aim assist step, called at
/// `iw3mp.exe` 0x463821, cod4-pmove.md): `charge::client_pick` over the newest snapshot's
/// players, the enemies (anyone without teams) seen from the predicted
/// eye at `cg_fov` 65 on 4:3, as the yaw's short and the distance.
fn melee_charge(
    c: &Client,
    predicted: Option<&pm::PlayerState>,
    view: [f32; 2],
    buttons: u32,
    clip: Option<&vcod_cod4::collision::ClipMap>,
) -> (u16, u8) {
    use vcod_cod4::pmove::charge::{AimBody, AimEye, client_pick};
    let (Some(ps), Some(s)) = (predicted, c.snapshots.last()) else {
        return (0, 0);
    };
    if buttons & pm::button::MELEE == 0 {
        return (0, 0);
    }
    // The pick skips a dead, spectating or linked body.
    if ps.other_flags & 2 != 0
        || !matches!(ps.pm_type, pm::pm_type::NORMAL | pm::pm_type::NORMAL_LINKED)
    {
        return (0, 0);
    }
    let team = |n: u32| {
        s.clients
            .iter()
            .find(|c| c.0[0] == n)
            .map_or(0, client_team)
    };
    let own = team(ps.client_num);
    let t = 32.5f32.to_radians().tan();
    let eye = AimEye {
        origin: ps.origin + Vec3::Z * ps.view_height_current,
        angles: Vec3::new(view[0], view[1], 0.0),
        tan_half_fov: [t, t * 0.75],
        fov_scale_inv: 1.0,
    };
    let bodies: Vec<AimBody> = s
        .entities
        .iter()
        .filter(|e| e.etype() == ET_PLAYER && e.number() != ps.client_num)
        .filter(|e| own == 0 || team(e.number()) != own)
        .filter_map(|e| {
            let origin = origin_of(e);
            let flags = ent_u32(e, "lerp.eFlags");
            let (head, top) = AIM_HEIGHTS[if flags & pm::ef::PRONE != 0 {
                2
            } else if flags & pm::ef::CROUCH != 0 {
                1
            } else {
                0
            }];
            // The visibility test: the eye to `j_head` under
            // 0x803003, the world only.
            let head = origin + Vec3::Z * head;
            if let Some(clip) = clip {
                let tr = clip.trace(eye.origin, head, Vec3::ZERO, Vec3::ZERO, 0x80_3003);
                if tr.fraction < 1.0 {
                    return None;
                }
            }
            Some(AimBody {
                origin,
                yaw: ent_f32(e, "lerp.apos.trBase[1]"),
                aim: origin,
                top,
            })
        })
        .collect();
    let (yaw, dist) = client_pick(buttons, ps.pm_flags, ps.origin, &eye, &bodies);
    (angle2short(yaw), dist)
}

/// Now for a live session's cmd clock, send pacing and timeouts: the wall
/// clock, or the clock a live test drives faster than real time on this
/// thread (`tests::sim`).
pub(crate) fn clock_now() -> Instant {
    #[cfg(test)]
    if let Some(t) = tests::sim::now() {
        return t;
    }
    Instant::now()
}

fn own_cmd(
    c: &Client,
    snap_age: Duration,
    view: [f32; 2],
    input: &Cod4Input,
    talking: bool,
) -> UserCmd {
    let none;
    let ps = match c.snapshots.last() {
        Some(s) => &s.ps,
        None => {
            none = PlayerState::default();
            &none
        }
    };
    let server_time = c
        .snapshots
        .last()
        .map_or(0, |s| s.server_time + snap_age.as_millis() as i32);
    let delta = |i: usize| angle2short(ps_f32(ps, &format!("delta_angles[{i}]")));
    let [forwardmove, rightmove] = input.moves(talking);
    let (weapon, offhand) = input.weapons();
    let pm_type = ps.get("pm_type").unwrap_or(0) as i32;
    UserCmd {
        server_time,
        buttons: input.buttons(pm_type, talking),
        weapon,
        offhand,
        angles: [
            angle2short(view[0]).wrapping_sub(delta(0)),
            angle2short(view[1]).wrapping_sub(delta(1)),
            0u16.wrapping_sub(delta(2)),
        ],
        forwardmove,
        rightmove,
        ..UserCmd::default()
    }
}

/// A refused challenge's `EXE_*` reason (`conn::Refused`) in the
/// install's own words, from `localized_code_post_gfx_mp`, its arguments
/// in ([`localize_message`]); any other error as it came.
pub fn localize_refusal(e: anyhow::Error, install: &std::path::Path) -> anyhow::Error {
    let Some(vcod_cod4::net::conn::Refused(why)) = e.downcast_ref() else {
        return e;
    };
    let Some(strings) =
        vcod_cod4::install::Install::at(install).and_then(|i| i.code_strings("english").ok())
    else {
        return e;
    };
    match localize_message(why, |k| strings.get(k).cloned()) {
        Some(t) => anyhow::anyhow!("{t}"),
        None => e,
    }
}

/// `SEH_LocalizeTextMessage`
/// (cod4-front-end.md 7): tokens split by 0x14 (the next one localized,
/// as the first is) and 0x15 (the next one as it stands); each token
/// after the first fills the text's next `&&n` in turn, or goes on its
/// end when none is left. `None` when a localized token has no string.
pub fn localize_message(msg: &str, get: impl Fn(&str) -> Option<String>) -> Option<String> {
    let mut out = String::new();
    let mut next = 1;
    let mut localize = true;
    let mut token = String::new();
    let mut tokens = Vec::new();
    for c in msg.chars() {
        match c {
            '\u{14}' | '\u{15}' => {
                tokens.push((std::mem::take(&mut token), localize));
                localize = c == '\u{14}';
            }
            c => token.push(c),
        }
    }
    tokens.push((token, localize));
    for (i, (t, loc)) in tokens.into_iter().enumerate() {
        if t.is_empty() {
            continue;
        }
        let t = if loc { get(&t)? } else { t };
        let slot = format!("&&{next}");
        match out.find(&slot) {
            Some(at) if i > 0 => {
                out.replace_range(at..at + slot.len(), &t);
                next += 1;
            }
            _ => out.push_str(&t),
        }
    }
    Some(out)
}

/// A refusal's argument fills its `&&1` (`EXE_SERVER_IS_DIFFERENT_VER`
/// with its 0x15 version, cod4-handshake.md), and a plain key reads as
/// its string.
/// `demo`'s missing file (`timedemo 1` looks for `demos/1.dm_1`) reads
/// in the install's words with the path in, not as its key
/// (cod4-demos.md 1).
#[cfg(test)]
#[test]
fn a_missing_demo_reads_localized() {
    let Some(install) = vcod_cod4::testing::cod4_install() else {
        return;
    };
    let t = localize_drop("EXE_ERR_NOT_FOUND\u{15}demos/1.dm_1", install.root());
    assert!(t.contains("demos/1.dm_1") && !t.contains("EXE_"), "{t}");
}

#[cfg(test)]
#[test]
fn a_refusal_fills_its_arguments_in() {
    let get = |k: &str| match k {
        "EXE_SERVER_IS_DIFFERENT_VER" => Some("Server is a different version:\n&&1".to_string()),
        "EXE_SERVERISFULL" => Some("Server is full.".to_string()),
        _ => None,
    };
    assert_eq!(
        localize_message("EXE_SERVER_IS_DIFFERENT_VER\u{15}1.7", get).as_deref(),
        Some("Server is a different version:\n1.7")
    );
    assert_eq!(
        localize_message("EXE_SERVERISFULL", get).as_deref(),
        Some("Server is full.")
    );
    assert_eq!(localize_message("EXE_NOPE", get), None);
}

#[cfg(test)]
pub(crate) mod tests {
    //! The replay gate: the committed captures through the session's frame
    //! path (step, events, scene) at 60 frames a second of the capture's
    //! clock, as `cod4_client` runs them.

    use super::*;
    use std::collections::BTreeMap;

    mod hud_menus;

    /// The clock a live test's client thread and its in-process server
    /// share, so the test runs as fast as both compute instead of in real
    /// time. Only the client moves it, with every sleep ([`clk_sleep`]),
    /// and each sleep waits until [`serve`] has read every packet sent
    /// before and run every server frame due by then; the session's own
    /// clock reads it ([`clock_now`]), so the two run in lockstep however
    /// slow a frame is or however long the client computes between
    /// sleeps (an asset load). The join waits and times out on it too, a
    /// frame a wait (`conn::set_thread_clock`), so a client wait that
    /// does not sleep on it never ends: [`Clock::turn`] fails one that
    /// leaves the clock still for [`STALL`].
    pub(crate) mod sim {
        use std::cell::RefCell;
        use std::sync::{Arc, Condvar, Mutex};
        use std::time::{Duration, Instant};

        /// The wall time the client may leave the clock still.
        pub(crate) const STALL: Duration = Duration::from_secs(600);

        #[derive(Clone)]
        pub(crate) struct Clock(Arc<(Mutex<State>, Condvar)>);

        struct State {
            /// The clock.
            now: Instant,
            /// What the server has run up to.
            served: Instant,
        }

        thread_local! {
            static CLOCK: RefCell<Option<Clock>> = const { RefCell::new(None) };
        }

        impl Clock {
            pub(crate) fn new() -> Clock {
                let t = Instant::now();
                let st = State { now: t, served: t };
                Clock(Arc::new((Mutex::new(st), Condvar::new())))
            }

            pub(crate) fn now(&self) -> Instant {
                self.0.0.lock().unwrap().now
            }

            /// Moves the clock on by `d` and waits for the server to catch
            /// up to it.
            fn sleep(&self, d: Duration) {
                let (m, cv) = &*self.0;
                let mut st = m.lock().unwrap();
                st.now += d;
                cv.notify_all();
                let want = st.now;
                let real = Instant::now();
                while st.served < want {
                    assert!(
                        real.elapsed() < Duration::from_secs(60),
                        "the server stopped serving the clock"
                    );
                    st = cv.wait_timeout(st, Duration::from_millis(100)).unwrap().0;
                }
            }

            /// The server's turn: waits for the client to move the clock,
            /// and returns the time to run up to, or `None` once
            /// `finished` says the client is done.
            pub(crate) fn turn(&self, finished: impl Fn() -> bool) -> Option<Instant> {
                let (m, cv) = &*self.0;
                let mut st = m.lock().unwrap();
                let real = Instant::now();
                while st.served >= st.now {
                    if finished() {
                        return None;
                    }
                    assert!(
                        real.elapsed() < STALL,
                        "the client left the clock still for {STALL:?}: a wait that does not sleep on it"
                    );
                    st = cv.wait_timeout(st, Duration::from_millis(50)).unwrap().0;
                }
                Some(st.now)
            }

            /// The server has run up to `t`.
            pub(crate) fn served(&self, t: Instant) {
                let (m, cv) = &*self.0;
                let mut st = m.lock().unwrap();
                st.served = st.served.max(t);
                cv.notify_all();
            }
        }

        /// A client thread on `clock`.
        pub(crate) fn spawn<T: Send + 'static>(
            clock: &Clock,
            f: impl FnOnce() -> T + Send + 'static,
        ) -> std::thread::JoinHandle<T> {
            let c = clock.clone();
            std::thread::spawn(move || {
                CLOCK.with(|x| *x.borrow_mut() = Some(c));
                // The join's waits and deadlines too, a frame a wait as a
                // client reads its packets once a frame.
                vcod_cod4::net::conn::set_thread_clock(super::clk_now, |_| {
                    super::clk_sleep(Duration::from_millis(16))
                });
                f()
            })
        }

        /// This thread's clock, if it runs on one.
        pub(crate) fn now() -> Option<Instant> {
            CLOCK.with(|c| c.borrow().as_ref().map(Clock::now))
        }

        pub(crate) fn sleep(d: Duration) -> bool {
            CLOCK.with(|c| c.borrow().as_ref().map(|c| c.sleep(d)).is_some())
        }
    }

    /// Now on this thread's [`sim`] clock, else the wall clock.
    pub(crate) fn clk_now() -> Instant {
        sim::now().unwrap_or_else(Instant::now)
    }

    /// Sleeps on this thread's [`sim`] clock, else the wall clock.
    pub(crate) fn clk_sleep(d: Duration) {
        if !sim::sleep(d) {
            std::thread::sleep(d);
        }
    }

    /// [`Instant::elapsed`] on [`clk_now`].
    pub(crate) fn since(t: Instant) -> Duration {
        clk_now().saturating_duration_since(t)
    }

    /// An in-process server's loop under `client`, on `clock`: each turn
    /// reads the packets the client sent (stamped with the time it sent
    /// them), runs every frame (`1000 / sv_fps`) due by the clock and sends what they
    /// queued. `tick` runs ahead of each frame, at its time, between the
    /// packets' cmds and the frame (it may move the next frame's time);
    /// once it returns false the server goes quiet for good: nothing read,
    /// run or sent.
    pub(crate) fn serve<T>(
        sv: &mut vcod_server::cod4::Cod4Server,
        sock: &std::net::UdpSocket,
        clock: &sim::Clock,
        client: &std::thread::JoinHandle<T>,
        mut tick: impl FnMut(&mut vcod_server::cod4::Cod4Server, Instant, &mut Instant) -> bool,
    ) {
        let mut buf = [0u8; 2048];
        let mut quiet = false;
        let mut next = clock.now();
        let mut heard = next;
        while let Some(until) = clock.turn(|| client.is_finished()) {
            // The bots' graph lands on the frame that asks for it, not
            // when a build thread is done.
            if let Some(b) = sv.bots_mut() {
                b.wait_for_graph();
            }
            while !quiet && let Ok((n, from)) = sock.recv_from(&mut buf) {
                sv.handle_packet(from, &buf[..n], heard);
            }
            while !quiet && next <= until {
                let at = next;
                if !tick(sv, at, &mut next) {
                    quiet = true;
                    break;
                }
                sv.tick_frames(at, 1);
                next += Duration::from_millis(sv.frame_ms() as u64);
                for (to, p) in sv.take_outgoing() {
                    let _ = sock.send_to(&p, to);
                }
            }
            // The connectionless answers the packets queued.
            for (to, p) in sv.take_outgoing().into_iter().filter(|_| !quiet) {
                let _ = sock.send_to(&p, to);
            }
            heard = until;
            clock.served(until);
        }
    }

    /// `client` against `sv` on one fresh [`sim`] clock, through
    /// [`serve`]: its frames, the server's frames and every packet between
    /// them land on the same times run to run whatever the wall does.
    /// `tick` is [`serve`]'s.
    pub(crate) fn run_held<T: Send + 'static>(
        sv: &mut vcod_server::cod4::Cod4Server,
        client: impl FnOnce(std::net::SocketAddr) -> T + Send + 'static,
        tick: impl FnMut(&mut vcod_server::cod4::Cod4Server, Instant, &mut Instant) -> bool,
    ) -> T {
        let sock = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
        sock.set_nonblocking(true).unwrap();
        let addr = sock.local_addr().unwrap();
        let clock = sim::Clock::new();
        let handle = sim::spawn(&clock, move || client(addr));
        serve(sv, &sock, &clock, &handle, tick);
        handle.join().unwrap()
    }

    const FIXTURES: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../cod4/tests/fixtures/net");

    /// The M16's grenade launcher (2) out of grenades goes back to the
    /// rifle (1); an empty rifle cycles to the next primary with rounds,
    /// wrapping past the highest index; a launcher whose rifle is gone
    /// selects nothing; the dead change nothing.
    #[test]
    fn cod4_out_of_ammo_picks_as_cg_out_of_ammo_change() {
        let def = |clip_index, ammo_index, inventory_type, alt_weapon| WeaponDef {
            clip_index,
            ammo_index,
            inventory_type,
            alt_weapon,
            ..WeaponDef::default()
        };
        // 1 the rifle, 2 its launcher, 3 a pistol.
        let defs = vec![
            WeaponDef::default(),
            def(1, 1, 0, 2),
            def(2, 2, INVENTORY_ALT_MODE, 1),
            def(3, 3, 0, 0),
        ];
        let mut d = Dry {
            pm_type: 0,
            weapon: 2,
            held: [0b1110, 0, 0, 0],
            ammo: [0; 64],
            ammoclip: [0; 128],
        };
        d.ammoclip[1] = 30;
        d.ammoclip[3] = 12;
        assert_eq!(out_of_ammo_pick(&defs, &d, 2, &[1, 3]), Some(1));
        d.weapon = 3;
        d.ammoclip[3] = 0;
        assert_eq!(out_of_ammo_pick(&defs, &d, 3, &[1, 3]), Some(1), "wraps");
        d.ammoclip[1] = 0;
        assert_eq!(
            out_of_ammo_pick(&defs, &d, 3, &[1, 3]),
            None,
            "nothing has rounds"
        );
        d.weapon = 2;
        d.held = [0b0100, 0, 0, 0];
        assert_eq!(out_of_ammo_pick(&defs, &d, 2, &[]), Some(0));
        d.pm_type = pm::pm_type::DEAD;
        assert_eq!(out_of_ammo_pick(&defs, &d, 2, &[]), None);
    }

    #[derive(Default, Debug)]
    struct Counts {
        snapshots: usize,
        frames: usize,
        /// Entities drawn, summed over frames, by `eType`.
        drawn: BTreeMap<u32, usize>,
        /// The most drawn in one frame.
        most: usize,
        submodels: HashSet<usize>,
        models: HashSet<String>,
        fired: BTreeMap<&'static str, usize>,
        lines: Vec<String>,
    }

    fn replay(name: &str) -> (Cod4Session, Counts) {
        let text = std::fs::read_to_string(format!("{FIXTURES}/{name}")).unwrap();
        let mut s = Cod4Session::open_capture(&text).unwrap();
        assert_eq!(s.map_name().as_deref(), Some("mp_crash"));
        let mut n = Counts::default();
        let mut ms = 0.0;
        while !s.finished() {
            ms += 1000.0 / 60.0;
            s.step(ms).unwrap();
            for f in s.take_fired() {
                *n.fired.entry(f.name).or_default() += 1;
            }
            n.lines.extend(s.take_lines());
            let Some(scene) = s.scene() else { continue };
            n.frames += 1;
            n.most = n.most.max(scene.entities.len());
            for d in &scene.entities {
                *n.drawn.entry(d.etype).or_default() += 1;
                match &d.visual {
                    Visual::Submodel(m) => {
                        n.submodels.insert(*m);
                    }
                    Visual::Model(m) => {
                        n.models.insert(m.clone());
                    }
                    Visual::Box => {}
                }
                assert!(d.origin.is_finite() && d.angles.is_finite());
            }
            assert!(scene.eye.is_finite() && scene.angles.is_finite());
        }
        n.snapshots = s.client().snapshots.len();
        (s, n)
    }

    /// Events counted straight off the snapshots, without the session:
    /// each event entity once per entry, each ring entry once.
    fn capture_temp_events(c: &Client) -> usize {
        let mut last: HashSet<u32> = HashSet::new();
        let mut n = 0;
        for s in &c.snapshots {
            let now: HashSet<u32> = s
                .entities
                .iter()
                .filter(|e| e.etype() >= ET_EVENTS)
                .map(|e| e.number())
                .collect();
            n += now.difference(&last).count();
            last = now;
        }
        n
    }

    #[test]
    fn spectator_capture_replays_through_the_frame_path() {
        let (s, n) = replay("play-1.7-spec.txt");
        let c = s.client();
        assert_eq!(n.snapshots, c.snapshots.len());
        assert_eq!(n.snapshots, 900);
        // The temp entities the capture holds, each fired once.
        let temps: usize = n
            .fired
            .iter()
            .filter(|(k, _)| **k == "EV_EARTHQUAKE")
            .map(|(_, v)| v)
            .sum();
        assert_eq!(temps, capture_temp_events(c));
        // The walker's body: its footsteps, shots and jump as the
        // spectator sees them (cod4-netfields.md 8).
        let fired: Vec<(&str, usize)> = n.fired.iter().map(|(k, v)| (*k, *v)).collect();
        assert_eq!(
            fired,
            [
                ("EV_EARTHQUAKE", 1),
                ("EV_FIRE_WEAPON", 18),
                ("EV_FOOTSTEP_RUN", 10),
                ("EV_GRENADE_BOUNCE", 4),
                ("EV_GRENADE_EXPLODE", 1),
                ("EV_JUMP", 1),
                ("EV_LANDING_ASPHALT", 1),
            ]
        );
        assert_eq!(n.most, 14);
        // Two players, the movers, the frag in flight.
        assert!(n.drawn.contains_key(&ET_PLAYER), "{:?}", n.drawn);
        assert!(n.drawn.contains_key(&ET_SCRIPTMOVER), "{:?}", n.drawn);
        assert!(n.drawn.contains_key(&ET_MISSILE), "{:?}", n.drawn);
        assert!(
            n.models.iter().any(|m| m.starts_with("body_mp_")),
            "{:?}",
            n.models
        );
        assert!(
            n.models.contains("projectile:frag_grenade_mp"),
            "{:?}",
            n.models
        );
        // mp_crash's movers are its destructible cars, xmodels (`solid` 0),
        // not brush models.
        assert!(n.submodels.is_empty());
        assert!(
            n.models
                .iter()
                .any(|m| m.starts_with("vehicle_80s_sedan1_")),
            "{:?}",
            n.models
        );
    }

    /// Retail's destructible sedans reach the drawn scene with the
    /// `partBits` their setup's `hidePart`s left (the ten later-state
    /// tags, cod4-client-view.md 15), which the draw hides surfaces by.
    #[test]
    fn retail_sedans_draw_with_their_hidden_parts() {
        let text = std::fs::read_to_string(format!("{FIXTURES}/play-1.7-spec.txt")).unwrap();
        let mut s = Cod4Session::open_capture(&text).unwrap();
        let mut ms = 0.0;
        let mut cars = Vec::new();
        while cars.is_empty() && !s.finished() {
            ms += 1000.0 / 60.0;
            s.step(ms).unwrap();
            if let Some(scene) = s.scene() {
                cars = scene
                    .entities
                    .iter()
                    .filter(|d| {
                        matches!(&d.visual, Visual::Model(m) if m.starts_with("vehicle_80s_sedan1_"))
                    })
                    .map(|d| d.part_bits)
                    .collect();
            }
        }
        assert!(!cars.is_empty(), "no sedan drawn");
        for bits in cars {
            assert_eq!(bits, [0x242a, 0x9248_0000, 0, 0], "{bits:08x?}");
        }
    }

    /// A watched body shows its aim and its lean (cod4-player-anims.md 8):
    /// the spectator capture's walker, posed with its entity's pitch and
    /// `leanf` overridden, drops its head aiming down and swings it back
    /// aiming up, and moves it to the side it leans to.
    #[test]
    fn a_watched_body_aims_and_leans() {
        use crate::play::cod4_players::Cod4Players;
        let Some(install) = vcod_cod4::testing::cod4_install() else {
            return;
        };
        let text = std::fs::read_to_string(format!("{FIXTURES}/play-1.7-spec.txt")).unwrap();
        let mut s = Cod4Session::open_capture(&text).unwrap();
        let (_, weapons, src) =
            crate::cod4_fly::load_bodies(install.root(), "mp_crash", &s.entity_assets()).unwrap();
        let mut ms = 0.0;
        let mut walker = None;
        while walker.is_none() && !s.finished() {
            ms += 1000.0 / 60.0;
            s.step(ms).unwrap();
            walker = s.scene().and_then(|scene| {
                let d = scene.entities.iter().find(|d| d.number == 2)?;
                Some((d.body.clone()?, scene.render_time, scene.server_time))
            });
        }
        let (view, t, snap) = walker.unwrap();
        // The head's model-space spot after a second of frames at `pitch`
        // and `lean` (the controls ease at 0.36 degrees a ms).
        let head = |pitch: f32, lean: f32| {
            let mut v = view.clone();
            v.frame.angles[0] = pitch;
            v.frame.lean = lean;
            let mut players = Cod4Players::default();
            let mut out = Vec::new();
            for k in 0..60 {
                out = players.posed(&src, &weapons, &[(2, &v)], &[], t + k * 16, snap);
            }
            let (_, rig, p) = &out[0];
            let body = &rig.models[0].1;
            let b = body.bones.iter().position(|b| b.name == "j_head").unwrap();
            // Model space: x forward, y left.
            let at = p.models[0].1[b].transform_point3(body.bones[b].pos);
            (at.z, at.x, -at.y)
        };
        let level = head(0.0, 0.0);
        let down = head(45.0, 0.0);
        let up = head(-45.0, 0.0);
        let right = head(0.0, 0.5);
        let left = head(0.0, -0.5);
        println!("head level {level:?} down {down:?} up {up:?} right {right:?} left {left:?}");
        // The torso pitches toward twice the view, half of it on the
        // spine with no `back_up`: the head swings 9 units each way (3.6
        // at CoD2's 0.6 of the view).
        assert!(
            down.0 < level.0 - 6.0 && down.1 > level.1 + 8.0,
            "{down:?} {level:?}"
        );
        assert!(
            up.0 > level.0 - 0.5 && up.1 < level.1 - 8.0,
            "{up:?} {level:?}"
        );
        assert!(right.2 > level.2 + 3.0, "{right:?} {level:?}");
        assert!(left.2 < level.2 - 3.0, "{left:?} {level:?}");
    }

    /// One body under two weapons is two rigs over one copy of its body
    /// and head: each rig baked its own before (cod4-client-perf.md 23).
    #[test]
    fn a_body_under_two_weapons_shares_its_meshes() {
        use crate::play::cod4_players::Cod4Players;
        let Some(install) = vcod_cod4::testing::cod4_install() else {
            return;
        };
        let text = std::fs::read_to_string(format!("{FIXTURES}/play-1.7-spec.txt")).unwrap();
        let mut s = Cod4Session::open_capture(&text).unwrap();
        let mut ents = s.entity_assets();
        ents.weapons.extend(["ak47_mp".into(), "m16_mp".into()]);
        let (_, weapons, src) =
            crate::cod4_fly::load_bodies(install.root(), "mp_crash", &ents).unwrap();
        let mut ms = 0.0;
        let mut walker = None;
        while walker.is_none() && !s.finished() {
            ms += 1000.0 / 60.0;
            s.step(ms).unwrap();
            walker = s.scene().and_then(|scene| {
                let d = scene.entities.iter().find(|d| d.number == 2)?;
                Some((d.body.clone()?, scene.render_time, scene.server_time))
            });
        }
        let (view, t, snap) = walker.unwrap();
        let posed = |players: &mut Cod4Players, weapon: &str, at: i32| {
            let mut v = view.clone();
            v.weapon = Some(weapon.into());
            let mut out = players.posed(&src, &weapons, &[(2, &v)], &[], at, snap);
            let (_, rig, p) = out.remove(0);
            (rig, p.transform, p.models)
        };
        let mut players = Cod4Players::default();
        let (ak, ..) = posed(&mut players, "ak47_mp", t);
        let (m16, transform, drawn) = posed(&mut players, "m16_mp", t + 16);
        assert!(!std::rc::Rc::ptr_eq(&ak, &m16));
        // What is drawn is what a rig loaded on its own draws.
        let mut alone = Cod4Players::default();
        posed(&mut alone, "m16_mp", t);
        let (_, transform_alone, drawn_alone) = posed(&mut alone, "m16_mp", t + 16);
        assert_eq!((transform, &drawn), (transform_alone, &drawn_alone));
        assert_eq!(ak.models.len(), m16.models.len());
        for (i, ((a, ma), (b, mb))) in ak.models.iter().zip(&m16.models).enumerate() {
            // The gun is the first attachment; the body and the rest are
            // one model each.
            assert_eq!(std::rc::Rc::ptr_eq(ma, mb), i != 1, "{a} / {b}");
        }
    }

    /// A gunner drawn by another client stands where the server put him
    /// (`CG_PlayerTurretPositionAndBlend`, cod4-gsc.md 9.2): the spectator
    /// capture's walker given mp_backlot's crouch SAW 393 at (-720 68 290)
    /// and its gunner's legs anim (0x35) and wire origin, drawn at retail's
    /// placement for the barrel at 0 and -45 (`play-1.7-saw.txt`).
    #[test]
    fn a_watched_gunner_stands_on_his_gun() {
        use crate::play::cod4_players::{Cod4Players, GunInfo};
        let Some(install) = vcod_cod4::testing::cod4_install() else {
            return;
        };
        let text = std::fs::read_to_string(format!("{FIXTURES}/play-1.7-spec.txt")).unwrap();
        let mut s = Cod4Session::open_capture(&text).unwrap();
        let mut ms = 0.0;
        let mut walker = None;
        while walker.is_none() && !s.finished() {
            ms += 1000.0 / 60.0;
            s.step(ms).unwrap();
            walker = s.scene().and_then(|scene| {
                let d = scene.entities.iter().find(|d| d.number == 2)?;
                Some((d.body.clone()?, scene.render_time, scene.server_time))
            });
        }
        let (view, t, snap) = walker.unwrap();
        let mut ents = s.entity_assets();
        ents.models.push("weapon_saw_mg_setup".into());
        ents.weapons.push("saw_bipod_crouch_mp".into());
        let (models, weapons, src) =
            crate::cod4_fly::load_bodies(install.root(), "mp_backlot", &ents).unwrap();
        let gun_model = models
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case("weapon_saw_mg_setup"))
            .map(|(_, m)| m)
            .unwrap_or_else(|| {
                panic!(
                    "{:?}",
                    models
                        .keys()
                        .filter(|k| k.contains("saw"))
                        .collect::<Vec<_>>()
                )
            });
        let bones = &gun_model.bones;
        let tag = |n: &str| bones.iter().find(|b| b.name == n).unwrap().pos;
        let drawn = |barrel: f32, origin: [f32; 3]| {
            let mut v = view.clone();
            v.frame.legs_anim = 0x35;
            v.frame.eflags |= 0x200;
            v.frame.origin = Vec3::from(origin);
            let gun = GunInfo {
                frame: vcod_cod2::player::GunFrame {
                    model: "weapon_saw_mg_setup".into(),
                    weapon: "saw_bipod_crouch_mp".into(),
                    origin: Vec3::new(-720.0, 68.0, 290.0),
                    angles: [0.0; 3],
                    barrel: [0.0, barrel, 0.0],
                },
                tags: (tag("tag_aim"), tag("tag_weapon")),
                rotate_inc: weapons["saw_bipod_crouch_mp"].rotate_inc,
            };
            let mut players = Cod4Players::default();
            let out = players.posed(&src, &weapons, &[(2, &v)], &[Some(gun)], t, snap);
            out[0].2.transform.w_axis.truncate()
        };
        // The wire's truncated origins of retail's placed gunner: the
        // height off the truncated z blends the rows a little apart from
        // the server's, as the cgame's does off the same origin.
        for (barrel, wire, placed) in [
            (0.0, [-754.0, 75.0, 258.0], [-754.039, 75.398]),
            (-45.0, [-738.0, 99.0, 258.0], [-738.634, 99.883]),
        ] {
            let at = drawn(barrel, wire);
            assert!(
                (at.x - placed[0]).abs() < 0.15 && (at.y - placed[1]).abs() < 0.15,
                "barrel {barrel}: drawn at {at:?}, retail placed {placed:?}"
            );
        }
    }

    /// The spectator capture's bodies posed through `play::cod4_players`
    /// at 60 frames a second, with the zones' clips and models
    /// (`COD4_DIR`; vacuous without it): every anim the walker's entity
    /// names resolves to a clip, every frame it is drawn it is posed, and
    /// the pose moves it (cod4-player-anims.md 6).
    #[test]
    fn spectator_capture_poses_its_bodies() {
        use crate::play::cod4_players::Cod4Players;
        use vcod_common::asset_source::AssetSource;
        let Some(install) = vcod_cod4::testing::cod4_install() else {
            return;
        };
        let text = std::fs::read_to_string(format!("{FIXTURES}/play-1.7-spec.txt")).unwrap();
        let mut s = Cod4Session::open_capture(&text).unwrap();
        let (_, weapons, src) =
            crate::cod4_fly::load_bodies(install.root(), "mp_crash", &s.entity_assets()).unwrap();
        let mut players = Cod4Players::default();
        assert!(players.anim_count(&src) > 300);
        let (mut frames, mut posed, mut drawn) = (0, 0, 0);
        let mut names: BTreeMap<String, bool> = BTreeMap::new();
        // The walker's neck, model space, standing and crouched.
        let mut necks: Vec<(i32, f32)> = Vec::new();
        let mut moved = 0;
        let mut last: Option<Vec<glam::Mat4>> = None;
        let mut wrists: Vec<glam::Vec3> = Vec::new();
        let mut ms = 0.0;
        while !s.finished() {
            ms += 1000.0 / 60.0;
            s.step(ms).unwrap();
            let Some(scene) = s.scene() else { continue };
            frames += 1;
            let views: Vec<(u32, &BodyView)> = scene
                .entities
                .iter()
                .filter_map(|d| Some((d.number, d.body.as_ref()?)))
                .collect();
            drawn += views.len();
            for (_, v) in &views {
                for wire in [v.frame.legs_anim, v.frame.torso_anim] {
                    if wire & 511 == 0 {
                        continue;
                    }
                    let name = players.anim_name(&src, wire).unwrap_or_default();
                    let ok = src.decoded_anim(&name).is_some_and(|c| c.is_ok());
                    names.insert(name, ok);
                }
            }
            let out = players.posed(
                &src,
                &weapons,
                &views,
                &[],
                scene.render_time,
                scene.server_time,
            );
            posed += out.len();
            for (i, rig, p) in &out {
                assert!(
                    p.models
                        .iter()
                        .all(|(_, m)| m.iter().all(|m| m.is_finite()))
                );
                // The held weapon (model 1) in the right hand: the wrist
                // behind the gun's `tag_weapon`, along its barrel.
                if views[*i].1.weapon.is_some() {
                    let body = &rig.models[0].1;
                    let b = body
                        .bones
                        .iter()
                        .position(|b| b.name == "j_wrist_ri")
                        .unwrap();
                    let wrist = p.models[0].1[b].transform_point3(body.bones[b].pos);
                    let at = p.models[1].1[0].inverse().transform_point3(wrist);
                    assert!(
                        (-25.0..-8.0).contains(&at.x),
                        "right wrist at {at} off the gun"
                    );
                    wrists.push(at);
                }
                if views[*i].0 != 2 {
                    continue;
                }
                let body = &rig.models[0].1;
                let neck = body.bones.iter().position(|b| b.name == "neck").unwrap();
                let at = p.models[0].1[neck].transform_point3(body.bones[neck].pos);
                necks.push((views[*i].1.frame.eflags, at.z));
                if last.as_ref().is_some_and(|l| *l != p.models[0].1) {
                    moved += 1;
                }
                last = Some(p.models[0].1.clone());
            }
        }
        assert!(frames > 2000, "{frames}");
        assert_eq!(posed, drawn, "every body drawn is posed");
        let missing: Vec<&String> = names
            .iter()
            .filter(|(_, ok)| !**ok)
            .map(|(n, _)| n)
            .collect();
        assert!(missing.is_empty(), "no clip for {missing:?}");
        assert!(
            names.contains_key("pb_combatrun_forward_loop") && names.contains_key("pt_stand_shoot"),
            "{names:?}"
        );
        assert!(
            moved > frames / 2,
            "the walker moved in {moved} of {frames} frames"
        );
        let mean = |crouched: bool| {
            let v: Vec<f32> = necks
                .iter()
                .filter(|(f, _)| (f & cod2_eflags::CROUCH != 0) == crouched)
                .map(|(_, z)| *z)
                .collect();
            v.iter().sum::<f32>() / v.len().max(1) as f32
        };
        let (stand, crouch) = (mean(false), mean(true));
        let wrist = wrists.iter().sum::<glam::Vec3>() / wrists.len().max(1) as f32;
        println!(
            "right wrist off the gun's tag_weapon: mean {wrist} over {}",
            wrists.len()
        );
        assert!(wrists.len() > 2000, "{}", wrists.len());
        assert!(
            wrist.distance(glam::Vec3::new(-14.1, -1.0, -4.8)) < 1.0,
            "right wrist at {wrist} on average"
        );
        println!(
            "clips {}  posed {posed} of {frames} frames  neck stand {stand:.1} crouch {crouch:.1}",
            names.len()
        );
        assert!((50.0..70.0).contains(&stand), "standing neck at {stand}");
        assert!(
            crouch < stand - 10.0,
            "crouched neck at {crouch}, standing {stand}"
        );
    }

    #[test]
    fn kill_capture_draws_the_corpse_and_prints_the_obituary() {
        let (s, n) = replay("play-1.7-kill-target.txt");
        assert_eq!(n.snapshots, s.client().snapshots.len());
        assert_eq!(n.snapshots, 336);
        assert!(n.drawn.contains_key(&ET_PLAYER_CORPSE), "{:?}", n.drawn);
        assert!(n.drawn.contains_key(&ET_ITEM), "{:?}", n.drawn);
        assert!(n.models.contains("weapon:m16_gl_mp"), "{:?}", n.models);
        // Twice: the killcam replays the shooter's frames, the obituary's
        // temp entity among them (cod4-netfields.md 10).
        assert_eq!(n.fired.get("EV_OBITUARY"), Some(&2), "{:?}", n.fired);
        assert_eq!(
            n.fired.get("EV_BULLET_HIT_CLIENT_LARGE"),
            Some(&2),
            "{:?}",
            n.fired
        );
        assert_eq!(n.lines.len(), 2);
        assert_eq!(n.lines[0], n.lines[1]);
        assert_eq!(n.fired.get("EV_FIRE_WEAPON"), Some(&32), "{:?}", n.fired);
        assert_eq!(n.lines[0], "shooter killed target (m16_gl_mp)");
    }

    /// What the events of a capture play (`cod4_audio`, `cod4_fx`).
    #[derive(Default, Debug)]
    struct AvCounts {
        /// Cues by alias, and the aliases no zone holds.
        sounds: BTreeMap<String, usize>,
        sounds_missing: std::collections::BTreeSet<String>,
        /// Effects by name, and the names no zone holds.
        effects: BTreeMap<String, usize>,
        effects_missing: std::collections::BTreeSet<String>,
        quakes: Vec<crate::play::quake::Quake>,
    }

    /// `name` through the session's frame path with the zones' sounds and
    /// effects (`COD4_DIR`; `None` without it): each fired event's cues and
    /// effects, counted as `cod4_client` plays them.
    fn replay_av(name: &str) -> Option<AvCounts> {
        let install = vcod_cod4::testing::cod4_install()?;
        let ((mut sounds, mut effects), _) =
            crate::cod4_fly::load_av(install.root(), "mp_crash").unwrap();
        let text = std::fs::read_to_string(format!("{FIXTURES}/{name}")).unwrap();
        let mut s = Cod4Session::open_capture(&text).unwrap();
        let mut n = AvCounts::default();
        let mut ms = 0.0;
        while !s.finished() {
            ms += 1000.0 / 60.0;
            s.step(ms).unwrap();
            let fired = s.take_fired();
            let Some(gs) = s.gamestate() else { continue };
            let weapons = weapon_list(gs);
            let cs = |k: u16| String::from_utf8_lossy(gs.configstring(k)).into_owned();
            for f in &fired {
                for c in sounds.cues(f, &weapons, &cs) {
                    *n.sounds.entry(c.alias).or_default() += 1;
                }
                for p in effects.plays(f, &weapons, &cs, &HashMap::new(), None) {
                    *n.effects.entry(p.key).or_default() += 1;
                }
                n.quakes.extend(crate::cod4_fx::earthquake(f));
            }
        }
        n.sounds_missing = sounds.missing.iter().cloned().collect();
        n.effects_missing = effects.missing.iter().cloned().collect();
        assert_eq!(sounds.started, n.sounds.values().sum::<usize>());
        assert_eq!(effects.spawned, n.effects.values().sum::<usize>());
        println!("{name}: {n:#?}");
        println!("element types left out: {:?}", effects.gaps);
        Some(n)
    }

    /// The spectator capture's walker heard and seen (cod4-sound.md 4,
    /// cod4-effects.md 6): its rock and asphalt steps with the gear rattle,
    /// the M16's shots and flashes, the frag's bounces and explode, the
    /// landing, and the frag's earthquake shaking the view. Every alias and
    /// effect resolves.
    #[test]
    fn spectator_capture_plays_its_sounds_and_effects() {
        let Some(n) = replay_av("play-1.7-spec.txt") else {
            return;
        };
        let count = |k: &str| n.sounds.get(k).copied().unwrap_or(0);
        assert_eq!(count("step_run_rock"), 3, "{:?}", n.sounds);
        assert_eq!(count("step_run_asphalt"), 8, "{:?}", n.sounds);
        assert_eq!(count("gear_rattle_run"), 11, "{:?}", n.sounds);
        assert_eq!(count("land_asphalt"), 1, "{:?}", n.sounds);
        assert_eq!(count("weap_m16_fire_npc"), 18, "{:?}", n.sounds);
        assert!(n.sounds_missing.is_empty(), "{:?}", n.sounds_missing);
        assert!(n.effects_missing.is_empty(), "{:?}", n.effects_missing);
        // The frag's explode on the entity that enters with it.
        assert_eq!(count("grenade_explode_concrete"), 1, "{:?}", n.sounds);
        assert_eq!(
            n.effects.get("explosions/grenadeexp_concrete_1.efx"),
            Some(&1),
            "{:?}",
            n.effects
        );
        assert_eq!(n.effects.values().sum::<usize>(), 23, "{:?}", n.effects);
        assert_eq!(n.quakes.len(), 1);
        let q = n.quakes[0];
        assert_eq!(
            (q.scale, q.radius, q.duration_ms),
            (0.3, 400.0, 500.0),
            "{q:?}"
        );
    }

    /// The walker's shots flash on its rifle's `tag_flash` and eject from
    /// its `tag_brass`, off the posed body (cod4-effects.md 10).
    #[test]
    fn spectator_capture_flashes_on_the_weapon_tags() {
        use crate::fx::sim::SpawnAt;
        use crate::play::cod4_players::Cod4Players;
        let Some(install) = vcod_cod4::testing::cod4_install() else {
            return;
        };
        let text = std::fs::read_to_string(format!("{FIXTURES}/play-1.7-spec.txt")).unwrap();
        let mut s = Cod4Session::open_capture(&text).unwrap();
        let (_, models, src) =
            crate::cod4_fly::load_bodies(install.root(), "mp_crash", &s.entity_assets()).unwrap();
        let ((_, mut effects), _) = crate::cod4_fly::load_av(install.root(), "mp_crash").unwrap();
        let mut players = Cod4Players::default();
        let (mut flashes, mut ejects) = (Vec::new(), 0);
        let mut ms = 0.0;
        while !s.finished() {
            ms += 1000.0 / 60.0;
            s.step(ms).unwrap();
            let fired = s.take_fired();
            let Some(scene) = s.scene() else { continue };
            let gs = s.gamestate().unwrap();
            let weapons = weapon_list(gs);
            let cs = |k: u16| String::from_utf8_lossy(gs.configstring(k)).into_owned();
            for f in fired.iter().filter(|f| f.name == "EV_FIRE_WEAPON") {
                let body = scene.entities.iter().find(|d| Some(d.number) == f.entity);
                for p in effects.plays(f, &weapons, &cs, &players.tags, None) {
                    let k = p.key;
                    match p.at {
                        SpawnAt::Oriented { pos, axis } if k.starts_with("muzzleflashes/") => {
                            flashes.push((pos, axis[0], body.map(|b| (b.origin, b.angles.y))))
                        }
                        SpawnAt::Oriented { .. } if k.starts_with("shellejects/") => ejects += 1,
                        _ => {}
                    }
                }
            }
            let views: Vec<(u32, &BodyView)> = scene
                .entities
                .iter()
                .filter_map(|d| Some((d.number, d.body.as_ref()?)))
                .collect();
            players.posed(
                &src,
                &models,
                &views,
                &[],
                scene.render_time,
                scene.server_time,
            );
        }
        println!(
            "{} flashes on the tag, {ejects} ejects: {flashes:?}",
            flashes.len()
        );
        // All 18 of the walker's shots, its rifle posed in each.
        assert_eq!(flashes.len(), 18);
        assert_eq!(ejects, 18);
        for (pos, forward, body) in flashes {
            let (origin, yaw) = body.unwrap();
            let off = pos - origin;
            // Out at the muzzle, ahead of the body along its yaw, below
            // the old 56-unit guess at the eye.
            let ahead = glam::Vec2::new(yaw.to_radians().cos(), yaw.to_radians().sin());
            assert!(off.truncate().dot(ahead) > 15.0, "{off:?} yaw {yaw}");
            assert!((10.0..60.0).contains(&off.z), "{off:?}");
            assert!(
                forward.truncate().normalize().dot(ahead) > 0.7,
                "{forward:?} {yaw}"
            );
        }
    }

    /// The walker's flashes and ejects ride its rifle's tags while they
    /// play (cod4-effects.md 10): each play names the walker's
    /// `tag_flash` / `tag_brass` bolt, and through the fx system the
    /// flash's elements that move with the effect stay on the tag as the
    /// body moves on, frame after frame.
    #[test]
    fn spectator_capture_flashes_follow_the_tag() {
        use crate::cod4_fx::slot;
        use crate::fx::sim::{FxSystem, TagBolt};
        use crate::play::cod4_players::Cod4Players;
        let Some(install) = vcod_cod4::testing::cod4_install() else {
            return;
        };
        let text = std::fs::read_to_string(format!("{FIXTURES}/play-1.7-spec.txt")).unwrap();
        let mut s = Cod4Session::open_capture(&text).unwrap();
        let (_, models, src) =
            crate::cod4_fly::load_bodies(install.root(), "mp_crash", &s.entity_assets()).unwrap();
        let ((_, mut effects), _) = crate::cod4_fly::load_av(install.root(), "mp_crash").unwrap();
        let fs = vcod_common::pk3::Pk3Fs::empty();
        let mut fx = FxSystem::new();
        for (k, v) in effects.take_effects().unwrap() {
            fx.insert(&k, v);
        }
        let mut players = Cod4Players::default();
        let (mut bolted, mut walker) = (Vec::new(), None);
        // Over the frames a flash rides the tag, the farthest the nearest
        // sprite sat from the tag's pose this frame, and the frames.
        let (mut worst, mut riding) = (0f32, 0);
        let mut ms = 0.0;
        let mut now = 0.0f64;
        while !s.finished() {
            ms += 1000.0 / 60.0;
            now += 1.0 / 60.0;
            s.step(ms).unwrap();
            let fired = s.take_fired();
            let Some(scene) = s.scene() else { continue };
            let gs = s.gamestate().unwrap();
            let weapons = weapon_list(gs);
            let cs = |k: u16| String::from_utf8_lossy(gs.configstring(k)).into_owned();
            for f in fired.iter().filter(|f| f.name == "EV_FIRE_WEAPON") {
                walker = f.entity;
                for p in effects.plays(f, &weapons, &cs, &players.tags, None) {
                    bolted.push(p.bolt);
                    effects.start(&mut fx, &fs, &p, now, None);
                }
            }
            let views: Vec<(u32, &BodyView)> = scene
                .entities
                .iter()
                .filter_map(|d| Some((d.number, d.body.as_ref()?)))
                .collect();
            players.posed(
                &src,
                &models,
                &views,
                &[],
                scene.render_time,
                scene.server_time,
            );
            effects.set_tags(&mut fx, &players.tags, None);
            effects.run_loops(&mut fx, &fs, now);
            fx.step(1.0 / 60.0, now as f32, None);
            fx.spawn_chained(&fs, now as f32);
            let ridden = fx.tag_bolts();
            if let Some(w) = walker
                && let Some(tag) = players.tags.get(&w).and_then(|t| t.flash)
                && ridden.contains(&TagBolt {
                    entity: w,
                    slot: slot::FLASH,
                })
            {
                // The nearest drawn sprite: the flash's own, on the tag.
                let near = fx
                    .build_quads(scene.eye, glam::Vec3::Y, glam::Vec3::Z, now as f32)
                    .iter()
                    .map(|q| {
                        let c = q
                            .verts
                            .iter()
                            .map(|v| glam::Vec3::from(*v))
                            .sum::<glam::Vec3>();
                        (c / 4.0 - tag.0).length()
                    })
                    .fold(f32::MAX, f32::min);
                worst = worst.max(near);
                riding += 1;
            }
        }
        println!("bolts {bolted:?}, {riding} frames riding, worst {worst}");
        let flash = Some(TagBolt {
            entity: walker.unwrap(),
            slot: slot::FLASH,
        });
        let brass = Some(TagBolt {
            entity: walker.unwrap(),
            slot: slot::BRASS,
        });
        assert_eq!(bolted.iter().filter(|b| **b == flash).count(), 18);
        assert_eq!(bolted.iter().filter(|b| **b == brass).count(), 18);
        assert!(riding >= 18, "{riding}");
        assert!(worst < 12.0, "{worst}");
    }

    /// Per shot the hit's distance and surface, the impacts by name, the
    /// whizbys.
    type Bullets = (Vec<(f32, u32)>, BTreeMap<String, usize>, usize);

    /// The first round of [`crate::cod4_fx::client_bullets`].
    fn first_bullet(
        f: &Fired,
        weapons: &[String],
        defs: &HashMap<String, crate::cod4_audio::WeaponAv>,
        tags: &HashMap<u32, crate::play::cod4_players::WeaponTags>,
        own: Option<&crate::cod4_fx::OwnView>,
        clip: &vcod_cod4::collision::ClipMap,
        solids: &[Solid],
    ) -> Option<crate::cod4_fx::BulletHit> {
        let world = crate::cod4_fx::ShotWorld { clip, solids };
        crate::cod4_fx::client_bullets(f, weapons, defs, tags, own, world)
            .into_iter()
            .next()
    }

    /// Each capture's client-side bullets (cod4-effects.md 12): the other
    /// players' shots traced from their posed `tag_flash` along their
    /// view, the world impacts' effects and sounds, and the `whizby`s the
    /// drawn eye hears. Returns per shot the hit's distance and surface,
    /// the impact names and the whizbys.
    fn replay_bullets(name: &str) -> Option<Bullets> {
        use crate::play::cod4_players::Cod4Players;
        let install = vcod_cod4::testing::cod4_install()?;
        let world = crash_world()?;
        let text = std::fs::read_to_string(format!("{FIXTURES}/{name}")).unwrap();
        let mut s = Cod4Session::open_capture(&text).unwrap();
        let (_, models, src) =
            crate::cod4_fly::load_bodies(install.root(), "mp_crash", &s.entity_assets()).unwrap();
        let ((mut sounds, mut effects), _) =
            crate::cod4_fly::load_av(install.root(), "mp_crash").unwrap();
        let mut players = Cod4Players::default();
        let (mut shots, mut names, mut whizby) = (Vec::new(), BTreeMap::new(), 0);
        let mut ms = 0.0;
        let mut eye = None;
        while !s.finished() {
            ms += 1000.0 / 60.0;
            s.step(ms).unwrap();
            let fired = s.take_fired();
            let Some(scene) = s.scene() else { continue };
            let weapons = weapon_list(s.gamestate().unwrap());
            for f in &fired {
                let defs = sounds.weapons.clone();
                let Some(hit) = first_bullet(
                    f,
                    &weapons,
                    &defs,
                    &players.tags,
                    None,
                    &world.clip,
                    &s.shot_solids,
                ) else {
                    continue;
                };
                assert!(hit.hit, "{hit:?}");
                shots.push(((hit.end - hit.start).length(), hit.surf));
                for p in effects.bullet_plays(f, &weapons, &hit) {
                    *names.entry(p.key).or_default() += 1;
                }
                for c in sounds.bullet_cues(f, &weapons, &hit, eye) {
                    if c.alias == "whizby" {
                        whizby += 1;
                    } else {
                        *names.entry(c.alias).or_default() += 1;
                    }
                }
            }
            let views: Vec<(u32, &BodyView)> = scene
                .entities
                .iter()
                .filter_map(|d| Some((d.number, d.body.as_ref()?)))
                .collect();
            players.posed(
                &src,
                &models,
                &views,
                &[],
                scene.render_time,
                scene.server_time,
            );
            eye = Some(scene.eye);
        }
        assert!(sounds.missing.is_empty(), "{:?}", sounds.missing);
        assert!(effects.missing.is_empty(), "{:?}", effects.missing);
        println!("{name}: shots {shots:?}, {names:?}, {whizby} whizbys");
        Some((shots, names, whizby))
    }

    /// The spectator capture's 18 M16 shots each strike the world, the
    /// table's impact and the surface's `bullet_large_*` playing at each.
    /// The kill target hears none of the shooter's rounds go by: they
    /// strike the world under 400 units out, most under 90, and 1.7 plays a
    /// whizby only to an eye 64 clear of both ends (cod4-effects.md 12).
    #[test]
    fn client_side_bullets_strike_the_world() {
        let Some((shots, names, _)) = replay_bullets("play-1.7-spec.txt") else {
            return;
        };
        assert_eq!(shots.len(), 18);
        let effects: usize = names
            .iter()
            .filter(|(k, _)| k.ends_with(".efx"))
            .map(|(_, n)| n)
            .sum();
        let large: usize = names
            .iter()
            .filter(|(k, _)| k.starts_with("bullet_large_"))
            .map(|(_, n)| n)
            .sum();
        // One round passes a plaster wall (the penetration walk): its far
        // side plays the exit effect and sound, and the wood it meets next
        // its own, two more of each (cod4-effects.md 12.1).
        assert_eq!((effects, large), (20, 20), "{names:?}");
        for k in [
            "impacts/large_plaster_1_exit.efx",
            "bullet_large_exit_plaster",
            "impacts/large_woodhit.efx",
            "bullet_large_wood",
        ] {
            assert_eq!(names.get(k), Some(&1), "{k} in {names:?}");
        }
        assert!(
            shots.iter().all(|&(d, _)| d > 16.0 && d < 4000.0),
            "{shots:?}"
        );
        let (target, _, whizby) = replay_bullets("play-1.7-kill-target.txt").unwrap();
        assert!(!target.is_empty());
        assert_eq!(whizby, 0);
    }

    /// Retail's footstep and landing parms against the ground our clip map
    /// gives under the stepping body (cod4-sound.md 4): every step of
    /// each capture names the surface type a trace down from its origin
    /// reports.
    #[test]
    fn footstep_surfaces_match_the_clip_map() {
        use vcod_cod4::collision::MASK_PLAYERSOLID;
        let Some(world) = crash_world() else { return };
        let mut by: BTreeMap<(u32, u32), usize> = BTreeMap::new();
        for name in [
            "play-1.7-walk.txt",
            "play-1.7-spec.txt",
            "play-1.7-kill-target.txt",
        ] {
            let text = std::fs::read_to_string(format!("{FIXTURES}/{name}")).unwrap();
            let mut s = Cod4Session::open_capture(&text).unwrap();
            let mut ms = 0.0;
            while !s.finished() {
                ms += 1000.0 / 60.0;
                s.step(ms).unwrap();
                for f in s.take_fired() {
                    if !f.name.starts_with("EV_FOOTSTEP") {
                        continue;
                    }
                    let o = f.origin;
                    let t = world.clip.trace(
                        o + glam::Vec3::Z,
                        o - glam::Vec3::Z * 16.0,
                        glam::Vec3::new(-15.0, -15.0, 0.0),
                        glam::Vec3::new(15.0, 15.0, 0.0),
                        MASK_PLAYERSOLID,
                    );
                    let ours = if t.surface_flags & 0x2000 != 0 {
                        0
                    } else {
                        (t.surface_flags & 0x1f0_0000) >> 20
                    };
                    *by.entry((f.parm, ours)).or_default() += 1;
                }
            }
        }
        println!("(retail parm, our ground) {by:?}");
        // One asphalt step reads the caulk under a curb's edge: the event
        // carries the snapshot's origin, not the cmd's.
        let (right, wrong): (Vec<_>, Vec<_>) = by.iter().partition(|((a, b), _)| a == b);
        let n = |v: &[(&(u32, u32), &usize)]| v.iter().map(|(_, n)| **n).sum::<usize>();
        assert!(n(&right) >= 19 && n(&wrong) <= 1, "{by:?}");
        assert!(
            right.iter().any(|((a, _), _)| *a == 17) && right.iter().any(|((a, _), _)| *a == 22)
        );
    }

    /// The kill shooter's own shots as a live client plays them
    /// (cod4-effects.md 12): each traced from the drawn eye along the
    /// view, its impact effect and sound off the table, the decal its
    /// effect places landing on the map's mark world in a material the
    /// renderer holds, and about half drawing a `gfx_tracer` beam from
    /// the view weapon's muzzle.
    #[test]
    fn own_shots_strike_mark_and_trace() {
        use crate::cod4_fx::OwnView;
        use crate::fx::sim::FxSystem;
        let Some(install) = vcod_cod4::testing::cod4_install() else {
            return;
        };
        let text =
            std::fs::read_to_string(format!("{FIXTURES}/play-1.7-kill-shooter.txt")).unwrap();
        let mut s = Cod4Session::open_capture(&text).unwrap();
        let mut map =
            crate::cod4_fly::load(install.root(), None, "mp_crash", &s.entity_assets()).unwrap();
        let (mut sounds, mut effects) = map.av.take().unwrap();
        let world = map.world.clone().unwrap();
        let fs = vcod_common::pk3::Pk3Fs::empty();
        let mut fx = FxSystem::new();
        for (k, v) in effects.take_effects().unwrap() {
            fx.insert(&k, v);
        }
        let marks_world = crate::marks::MarkWorld::build(&map.bsp, |_| true);
        let (mut shots, mut impacts, mut cues, mut marks, mut placed) = (0, 0, 0, Vec::new(), 0);
        let mut pierced: BTreeMap<(bool, bool), usize> = BTreeMap::new();
        let mut bodies = 0;
        let mut now = 0.0f32;
        let mut ms = 0.0;
        while !s.finished() {
            ms += 1000.0 / 60.0;
            now += 1.0 / 60.0;
            s.step(ms).unwrap();
            let fired = s.take_fired();
            let Some(scene) = s.scene() else { continue };
            let weapons = weapon_list(s.gamestate().unwrap());
            let own = OwnView {
                client: Some(scene.view.client_num),
                eye: scene.eye,
                angles: scene.angles,
                flash: None,
            };
            for f in fired.iter().filter(|f| f.entity.is_none()) {
                let defs = sounds.weapons.clone();
                let Some(hit) = first_bullet(
                    f,
                    &weapons,
                    &defs,
                    &HashMap::new(),
                    Some(&own),
                    &world.clip,
                    &s.shot_solids,
                ) else {
                    continue;
                };
                assert!(hit.own && hit.hit, "{hit:?}");
                shots += 1;
                // A round into the target's box plays nothing here.
                bodies += usize::from(hit.body.is_some());
                // A penetrating round's further contacts play too; one
                // into a caulk floor (surface 0) only its sound.
                for p in hit.pierced.iter().flatten() {
                    *pierced.entry((p.exit, p.surf == 0)).or_default() += 1;
                }
                effects.tracers(&mut fx, std::slice::from_ref(&hit), now, 16);
                for p in effects.bullet_plays(f, &weapons, &hit) {
                    impacts += 1;
                    effects.start(&mut fx, &fs, &p, f64::from(now), None);
                }
                cues += sounds.bullet_cues(f, &weapons, &hit, Some(scene.eye)).len();
            }
            fx.step(1.0 / 60.0, now, None);
            fx.spawn_chained(&fs, now);
            for m in fx.take_marks(now) {
                placed += usize::from(
                    !crate::marks::place(&marks_world, &m, (now * 1000.0) as i32).is_empty(),
                );
                marks.push(m.shader);
            }
        }
        println!(
            "{shots} own shots ({bodies} into a body), {impacts} impacts, {cues} cues, {} tracers, marks {marks:?}, {placed} placed, pierced {pierced:?}",
            effects.tracers
        );
        assert!(shots >= 10, "{shots}");
        let further: usize = pierced.values().sum();
        let bare = pierced
            .iter()
            .filter(|((_, b), _)| *b)
            .map(|(_, n)| n)
            .sum::<usize>();
        assert_eq!(
            (impacts, cues),
            (shots - bodies + further - bare, shots - bodies + further)
        );
        assert_eq!(marks.len(), impacts, "{marks:?}");
        assert_eq!(placed, impacts);
        assert!(
            marks.iter().all(|m| map.materials.get(m).is_some()),
            "{marks:?}"
        );
        // `cg_firstPersonTracerChance` 0.5.
        assert!(
            effects.tracers * 4 > shots && effects.tracers * 4 < shots * 3,
            "{}",
            effects.tracers
        );
        assert!(
            map.materials
                .get(crate::fx::cod2_tracer::COD4.material)
                .is_some()
        );
    }

    /// The kill captures' shots, hits and the killcam's, from both ends.
    #[test]
    fn kill_captures_play_their_hits() {
        let Some(shooter) = replay_av("play-1.7-kill-shooter.txt") else {
            return;
        };
        let target = replay_av("play-1.7-kill-target.txt").unwrap();
        for n in [&shooter, &target] {
            assert!(n.sounds_missing.is_empty(), "{:?}", n.sounds_missing);
            assert!(n.effects_missing.is_empty(), "{:?}", n.effects_missing);
        }
        // The shooter hears its own shots as the player.
        assert!(
            shooter.sounds.keys().any(|k| k.ends_with("_plr")),
            "{:?}",
            shooter.sounds
        );
        assert!(
            target
                .sounds
                .keys()
                .any(|k| k.starts_with("bullet_large_flesh")),
            "{:?}",
            target.sounds
        );
    }

    /// The shooter's own view is its cmds' (snapshots leave it out): while
    /// it aims down its sights at the target, the drawn view points at the
    /// target's body.
    #[test]
    fn shooter_view_is_its_cmds_aim() {
        let text =
            std::fs::read_to_string(format!("{FIXTURES}/play-1.7-kill-shooter.txt")).unwrap();
        let mut s = Cod4Session::open_capture(&text).unwrap();
        let (mut on, mut off) = (0, 0);
        let mut ms = 0.0;
        while !s.finished() {
            ms += 1000.0 / 60.0;
            s.step(ms).unwrap();
            let Some(scene) = s.scene() else { continue };
            let own = s.gamestate().unwrap().client_num as u32;
            let Some(target) = scene
                .entities
                .iter()
                .find(|d| d.etype == ET_PLAYER && d.number != own)
            else {
                continue;
            };
            let d = target.origin + Vec3::Z * 40.0 - scene.eye;
            let yaw = d.y.atan2(d.x).to_degrees();
            let off_by = ((yaw - scene.angles.y + 540.0) % 360.0 - 180.0).abs();
            if off_by < 5.0 {
                on += 1;
            } else {
                off += 1;
            }
        }
        assert!(on > 3 * off, "on target {on} frames, off {off}");
    }

    /// The view of a capture played at `hz`, drawn off the snapshots: per
    /// frame the drawn time, the eye's step from the last frame and whether
    /// the view started over.
    fn view_steps(name: &str, hz: f64) -> Vec<(i32, f32, bool)> {
        let (steps, _) = view_steps_in(name, hz, None);
        steps.iter().map(|&(t, d, r)| (t, d.length(), r)).collect()
    }

    /// mp_crash's prediction world, loaded once.
    fn crash_world() -> Option<Arc<Cod4World>> {
        static WORLD: std::sync::OnceLock<Option<Arc<Cod4World>>> = std::sync::OnceLock::new();
        WORLD
            .get_or_init(|| {
                let install = vcod_cod4::testing::cod4_install()?;
                let w = crate::cod4_fly::load_world(install.root(), "mp_crash").unwrap();
                Some(Arc::new(w))
            })
            .clone()
    }

    /// [`view_steps`] as the eye's moves, predicting through `world` when
    /// given; the predictor's counts at the end.
    fn view_steps_in(
        name: &str,
        hz: f64,
        world: Option<Arc<Cod4World>>,
    ) -> (Vec<(i32, Vec3, bool)>, cod4_predict::PredictStats) {
        let text = std::fs::read_to_string(format!("{FIXTURES}/{name}")).unwrap();
        let mut s = Cod4Session::open_capture(&text).unwrap();
        if let Some(w) = world {
            s.set_world(w);
        }
        let (mut ms, mut last, mut out) = (0.0, None::<(Vec3, bool)>, Vec::new());
        while !s.finished() {
            ms += 1000.0 / hz;
            s.step(ms).unwrap();
            let Some(sc) = s.scene() else { continue };
            if let Some((eye, reset)) = last {
                // A reset counts on the frame the drawn pair turns to it.
                let new_reset = sc.view_reset && !reset;
                out.push((sc.render_time, sc.eye - eye, new_reset));
            }
            last = Some((sc.eye, sc.view_reset));
        }
        (out, s.predictor.stats)
    }

    /// The walk capture drawn at 60 and 144 frames a second: once the
    /// clock has settled the drawn time steps by the frame's length (the
    /// adjust moves it a millisecond or two a snapshot), the eye never
    /// steps further than a run's pace, and it jumps only where the view
    /// starts over (the two spawns), as retail cuts (cod4-client-view.md).
    #[test]
    fn walk_capture_view_runs_smooth() {
        for hz in [60.0, 144.0] {
            let frame = (1000.0 / hz) as i32;
            let steps = view_steps("play-1.7-walk.txt", hz);
            let settled = &steps[(2.0 * hz) as usize..];
            let times: Vec<i32> = settled.windows(2).map(|w| w[1].0 - w[0].0).collect();
            let (lo, hi) = (times.iter().min().unwrap(), times.iter().max().unwrap());
            let most = steps
                .iter()
                .filter(|s| !s.2)
                .map(|s| s.1)
                .fold(0.0, f32::max);
            let jumps = steps.iter().filter(|s| s.1 > 20.0).count();
            let resets = steps.iter().filter(|s| s.2 && s.1 > 20.0).count();
            println!(
                "{hz} Hz: drawn time steps {lo}..{hi}, max eye step {most:.2}, jumps {jumps}, at resets {resets}"
            );
            assert!(
                *lo >= frame - 3 && *hi <= frame + 4,
                "{hz} Hz: drawn time steps {lo}..{hi}"
            );
            assert!(most < 8.0, "{hz} Hz: eye stepped {most}");
            assert_eq!((jumps, resets), (2, 2), "{hz} Hz");
        }
        // The kill pair: the spawns, the killcam's start and end and the
        // respawn, each a reset.
        for (name, n) in [
            ("play-1.7-kill-shooter.txt", 2),
            ("play-1.7-kill-target.txt", 5),
        ] {
            let steps = view_steps(name, 60.0);
            let jumps = steps.iter().filter(|s| s.1 > 20.0).count();
            let resets = steps.iter().filter(|s| s.2 && s.1 > 20.0).count();
            assert_eq!((jumps, resets), (n, n), "{name}");
            // Predicted, the same cuts and nothing else.
            if let Some(w) = crash_world() {
                let (steps, st) = view_steps_in(name, 60.0, Some(w));
                let jumps = steps.iter().filter(|s| s.1.length() > 20.0).count();
                let resets = steps.iter().filter(|s| s.2 && s.1.length() > 20.0).count();
                println!("{name} predicted: jumps {jumps}, at resets {resets}; {st:?}");
                assert_eq!((jumps, resets), (n, n), "{name} predicted");
            }
        }
    }

    /// A capture's own view turns every frame, not in its cmds' 100 ms
    /// steps: the walk capture predicted at 60 Hz turns on about twice the
    /// frames it did unsmoothed (92), its largest step a frame outside the
    /// cuts a fifth of the unsmoothed 1.29 degrees (cod4-client-view.md 6).
    #[test]
    fn capture_view_turns_every_frame() {
        let Some(world) = crash_world() else { return };
        let text = std::fs::read_to_string(format!("{FIXTURES}/play-1.7-walk.txt")).unwrap();
        let mut s = Cod4Session::open_capture(&text).unwrap();
        s.set_world(world);
        let (mut ms, mut last) = (0.0, None::<f32>);
        let (mut most, mut turning) = (0.0f32, 0);
        while !s.finished() {
            ms += 1000.0 / 60.0;
            s.step(ms).unwrap();
            let Some(sc) = s.scene() else { continue };
            if let Some(y) = last.filter(|_| !sc.following && !sc.view_reset) {
                let d = ((sc.angles.y - y + 540.0) % 360.0 - 180.0).abs();
                most = most.max(d);
                turning += usize::from(d > 0.01);
            }
            last = Some(sc.angles.y);
        }
        println!("max yaw step {most:.2}, {turning} turning frames");
        assert!(most < 0.4, "{most}");
        assert!(turning > 150, "{turning}");
    }

    #[test]
    fn an_eased_cmd_turns_the_short_way() {
        let cmd = |t: i32, yaw: u16| Cmd {
            server_time: t,
            angles: [0, yaw, 0],
            ..Cmd::default()
        };
        // 350 degrees to 10 crosses 0, not 180.
        let (a, b) = (cmd(0, 63716), cmd(100, 1820));
        assert_eq!(eased_cmd(&a, &b, 50).angles[1], 0);
        assert_eq!(eased_cmd(&a, &b, 100).angles[1], 1820);
        assert_eq!(eased_cmd(&a, &b, 0).angles[1], 63716);
        assert_eq!(eased_cmd(&b, &b, 50).angles, b.angles);
        // A snap (65 degrees in 19 ms) and a long gap are taken at once.
        let snap = cmd(119, 1820 + 11833);
        assert_eq!(eased_cmd(&b, &snap, 110).angles, snap.angles);
        let late = cmd(400, 3640);
        assert_eq!(eased_cmd(&b, &late, 250).angles, late.angles);
    }

    /// The walk capture predicted: its own cmds replayed over mp_crash's
    /// clip map from each snapshot, the frame's own cmd on top
    /// (cod4-client-view.md 5). The eye moves every frame by the frame's
    /// share of the run (no 100 ms beats), level within a run's pace at
    /// 190 units a second, up and down within a jump's takeoff; every
    /// snapshot's playerstate lands within the last bits of where the
    /// replay put it.
    #[test]
    fn walk_capture_predicts_its_own_body() {
        let Some(world) = crash_world() else { return };
        for hz in [60.0, 144.0] {
            let dt = 1.0 / hz as f32;
            let flat = |v: &[(i32, Vec3, bool)]| {
                v.iter()
                    .filter(|s| !s.2)
                    .map(|s| s.1.truncate().length())
                    .fold(0.0, f32::max)
            };
            let whole = |v: &[(i32, Vec3, bool)]| {
                v.iter()
                    .filter(|s| !s.2)
                    .map(|s| s.1.length())
                    .fold(0.0, f32::max)
            };
            let (before, _) = view_steps_in("play-1.7-walk.txt", hz, None);
            let (steps, st) = view_steps_in("play-1.7-walk.txt", hz, Some(world.clone()));
            let jumps = steps.iter().filter(|s| s.1.length() > 20.0).count();
            let resets = steps.iter().filter(|s| s.2 && s.1.length() > 20.0).count();
            println!(
                "{hz} Hz: max eye step level {:.2} -> {:.2}, whole {:.2} -> {:.2}; \
                 jumps {jumps}, at resets {resets}; {} snapshots corrected (max {:.4}), {} past 0.1, {} cmds run",
                flat(&before),
                flat(&steps),
                whole(&before),
                whole(&steps),
                st.corrected,
                st.max_correction,
                st.missed,
                st.cmds_run,
            );
            // The frame's cmd is stamped in whole ms, a ms of slew on top.
            let dt = dt + 0.002;
            assert!(
                flat(&steps) <= 190.0 * dt,
                "{hz} Hz: level step {}",
                flat(&steps)
            );
            // A jump's takeoff: sqrt(2 * 39 * 800) units a second up.
            assert!(
                whole(&steps) <= 260.0 * dt,
                "{hz} Hz: step {}",
                whole(&steps)
            );
            assert_eq!((jumps, resets), (2, 2), "{hz} Hz");
            assert_eq!(st.missed, 0, "{hz} Hz");
            assert!(st.max_correction < 0.05, "{hz} Hz: {}", st.max_correction);
        }
    }

    /// The mantle capture predicted: both mantles and the leans replay
    /// in the client's own cmds within the last bits of the server's
    /// (cod4-pmove.md 17, cod4-client-view.md 7).
    #[test]
    fn mantle_capture_predicts_without_a_correction() {
        let Some(world) = crash_world() else { return };
        let (_, st) = view_steps_in("play-1.7-mantle.txt", 60.0, Some(world));
        println!("{st:?}");
        assert_eq!(st.missed, 0);
        assert!(st.max_correction < 0.05, "{}", st.max_correction);
    }

    /// The own events the prediction fires on top of the snapshots', by
    /// capture. Empty since the weapon half models `Mantle_IsWeaponInactive`
    /// (cod4-pmove.md 17.4): the mantle's putaway is predicted as retail
    /// sends it.
    const OWN_EVENT_GAPS: &[(&str, &str, usize)] = &[];

    /// Our own ring's events as the window fires them off the prediction,
    /// against the snapshots' ring read straight: each capture's own
    /// events (shots, reloads, switches, footsteps, the jumps) fire once,
    /// none a second time when the snapshot carrying it arrives.
    #[test]
    fn predicted_own_events_fire_once() {
        let Some(world) = crash_world() else {
            return;
        };
        for name in [
            "play-1.7-kill-shooter.txt",
            "play-1.7-walk.txt",
            "play-1.7-mantle.txt",
        ] {
            let text = std::fs::read_to_string(format!("{FIXTURES}/{name}")).unwrap();
            let mut s = Cod4Session::open_capture(&text).unwrap();
            s.set_world(world.clone());
            let mut ours: BTreeMap<&str, usize> = BTreeMap::new();
            let mut ms = 0.0;
            while !s.finished() {
                ms += 1000.0 / 60.0;
                s.step(ms).unwrap();
                s.scene();
                for f in s.take_fired().iter().filter(|f| f.entity.is_none()) {
                    *ours.entry(f.name).or_default() += 1;
                }
            }
            // The snapshots' ring, each entry once, up to the last
            // snapshot the scene predicted from.
            let c = s.client();
            let mut theirs: BTreeMap<&str, usize> = BTreeMap::new();
            let mut seen: Option<u32> = None;
            for x in &c.snapshots {
                let seq = x.ps.get("eventSequence").unwrap_or(0) & 0xff;
                if let Some(prev) = seen {
                    let to = if seq < prev { seq + 256 } else { seq };
                    for q in prev.max(to.saturating_sub(4))..to {
                        let id = x.ps.get(&format!("events[{}]", q & 3)).unwrap_or(0);
                        if let Some(n) = event_name(id).filter(|_| id != 0) {
                            *theirs.entry(n).or_default() += 1;
                        }
                    }
                }
                seen = Some(seq);
            }
            println!("{name}: predicted {ours:?}\n  snapshots {theirs:?}");
            for &(gap, event, n) in OWN_EVENT_GAPS {
                if gap == name {
                    *theirs.entry(event).or_default() += n;
                }
            }
            assert_eq!(ours, theirs, "{name}");
        }
    }

    /// What the view weapon drew over a capture (cod4-client-view.md 4).
    #[derive(Default, Debug)]
    struct ViewCounts {
        frames: usize,
        /// Frames drawn with a view weapon, by weapon.
        drawn: BTreeMap<String, usize>,
        /// The own view's shots, and the fire nodes started.
        shots: usize,
        fire_starts: usize,
        /// Each own shot's view flash: its place off the eye along the
        /// view (forward, right, up) and its forward against the view's.
        flashes: Vec<(Vec3, f32)>,
        ejects: usize,
    }

    /// `name` through the frame path with the view weapon as `cod4_client`
    /// draws it at 60 frames a second: the scene (its prediction firing our
    /// own events), this frame's pose, then the shots' effects off its
    /// view tags.
    fn replay_view(name: &str) -> Option<ViewCounts> {
        use crate::cod4_viewmodel::{Cod4View, tag_to_world};
        use crate::fx::sim::SpawnAt;
        let install = vcod_cod4::testing::cod4_install()?;
        let text = std::fs::read_to_string(format!("{FIXTURES}/{name}")).unwrap();
        let mut s = Cod4Session::open_capture(&text).unwrap();
        // Predicted, as the window plays it.
        s.set_world(crash_world()?);
        let (models, weapons, _) =
            crate::cod4_fly::load_bodies(install.root(), "mp_crash", &s.entity_assets()).unwrap();
        let ((_, mut effects), _) = crate::cod4_fly::load_av(install.root(), "mp_crash").unwrap();
        let mut view = Cod4View::default();
        let mut n = ViewCounts::default();
        let (mut starts, mut ms) = (0, 0.0);
        while !s.finished() {
            ms += 1000.0 / 60.0;
            s.step(ms).unwrap();
            let Some(scene) = s.scene() else {
                s.take_fired();
                continue;
            };
            let fired = s.take_fired();
            let gs = s.gamestate().unwrap();
            let list = weapon_list(gs);
            let cs = |k: u16| String::from_utf8_lossy(gs.configstring(k)).into_owned();
            let (yaw, pitch) = (scene.angles.y.to_radians(), -scene.angles.x.to_radians());
            let basis = crate::camera::basis(yaw, pitch);
            n.frames += 1;
            let out = view.frame(
                &scene.view,
                &list,
                scene.hands.as_deref(),
                &weapons,
                &models,
                scene.render_time,
            );
            let mut tags = None;
            if let (Some(frame), Some((w, anim))) = (&out.frame, view.anim()) {
                assert!(frame.draw.bone_sets.iter().flatten().all(|m| m.is_finite()));
                *n.drawn.entry(w.to_string()).or_default() += 1;
                if anim.starts > starts && [3, 5, 28, 29].contains(&anim.node) {
                    n.fire_starts += 1;
                }
                starts = anim.starts;
                let world =
                    |t: Option<(Vec3, glam::Quat)>| t.map(|t| tag_to_world(t, scene.eye, basis));
                tags = Some((
                    scene.view.client_num,
                    crate::play::cod4_players::WeaponTags {
                        flash: world(out.tags.flash),
                        brass: world(out.tags.brass),
                    },
                ));
            } else {
                starts = 0;
            }
            for f in fired
                .iter()
                .filter(|f| f.entity.is_none() && f.name.starts_with("EV_FIRE_WEAPON"))
            {
                n.shots += 1;
                for p in effects.plays(f, &list, &cs, &HashMap::new(), tags.as_ref()) {
                    let k = p.key;
                    match p.at {
                        SpawnAt::Oriented { pos, axis } if k.starts_with("muzzleflashes/") => {
                            let d = pos - scene.eye;
                            let (fwd, right, up) = basis;
                            n.flashes.push((
                                Vec3::new(d.dot(fwd), d.dot(right), d.dot(up)),
                                axis[0].dot(fwd),
                            ))
                        }
                        SpawnAt::Oriented { .. } if k.starts_with("shellejects/") => n.ejects += 1,
                        _ => {}
                    }
                }
            }
        }
        Some(n)
    }

    /// The shooter draws its M16 in first person, plays the fire clip on
    /// every own shot and flashes on the view model's `tag_flash`, at the
    /// muzzle ahead of the eye, pointing down the view.
    #[test]
    fn shooter_draws_its_view_weapon_and_flashes_on_its_tag() {
        let Some(n) = replay_view("play-1.7-kill-shooter.txt") else {
            return;
        };
        println!("{n:?}");
        assert!(
            n.drawn.keys().all(|w| w.starts_with("m16")),
            "{:?}",
            n.drawn
        );
        assert!(n.drawn.values().sum::<usize>() > n.frames / 2, "{n:?}");
        assert!(n.shots > 0);
        assert_eq!(n.fire_starts, n.shots, "{n:?}");
        assert_eq!(n.flashes.len(), n.shots);
        assert_eq!(n.ejects, n.shots);
        for (at, along) in &n.flashes {
            assert!((10.0..40.0).contains(&at.x), "{at}");
            assert!(at.y.abs() < 10.0 && at.z.abs() < 10.0, "{at}");
            assert!(*along > 0.9, "{along}");
        }
    }

    /// The walker draws its rifle in first person once it spawns (a frag
    /// for the throws), the idle teammate's while its view follows him,
    /// and each of its shots flashes and ejects on the view tags.
    #[test]
    fn walker_draws_its_view_weapon() {
        let Some(n) = replay_view("play-1.7-walk.txt") else {
            return;
        };
        println!("{n:?}");
        assert!(n.drawn.contains_key("m16_gl_mp"), "{n:?}");
        assert!(n.drawn.values().sum::<usize>() > n.frames / 2, "{n:?}");
        assert!(n.fire_starts >= n.shots, "{n:?}");
        assert_eq!((n.flashes.len(), n.ejects), (n.shots, n.shots));
    }

    /// vcod-server's `map mp_backlot` under a joined, spawned client: the
    /// session reads `loadingnewmap`, answers `requeststats`, takes the new
    /// gamestate and its `cp`, names the new map once for the caller to
    /// load, and the menus' answers spawn it there (cod4-map-cycle.md 6).
    #[test]
    fn cod4_live_map_change_names_the_new_map_and_spawns_there() {
        use std::sync::Arc;
        use std::sync::atomic::{AtomicBool, Ordering};
        use vcod_server::cod4::{Cod4Config, Cod4Game, Cod4Server};
        use vcod_server::engine::GameModule;
        let Some(install) = vcod_cod4::testing::cod4_install() else {
            return;
        };
        let mut sv = Cod4Server::new(
            Cod4Config {
                map: "mp_crash".into(),
                gametype: "war".into(),
                hostname: "vcod".into(),
                max_clients: 4,
            },
            7,
        );
        sv.load(Some(&install)).unwrap();
        let sock = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
        sock.set_nonblocking(true).unwrap();
        let addr = sock.local_addr().unwrap();
        let spawned_once = Arc::new(AtomicBool::new(false));
        let flag = spawned_once.clone();
        let clock = sim::Clock::new();
        let client = sim::spawn(&clock, move || {
            let join = MenuJoin {
                team: Some("allies".into()),
                class: Some("assault_mp,0".into()),
            };
            let mut s = Cod4Session::connect(addr, "joiner", Cp::Bare, join).unwrap();
            let t0 = clk_now();
            let (mut changes, mut spawns) = (Vec::new(), Vec::new());
            while since(t0) < Duration::from_secs(60) && spawns.len() < 2 {
                s.step(since(t0).as_secs_f64() * 1000.0).unwrap();
                s.take_commands();
                changes.extend(s.take_map_change());
                let up = s.spawn().is_some();
                if up && spawns.len() == changes.len() {
                    spawns.push(s.map_name());
                    flag.store(true, Ordering::Relaxed);
                }
                clk_sleep(Duration::from_millis(16));
            }
            s.disconnect();
            (changes, spawns)
        });
        let mut asked = false;
        serve(&mut sv, &sock, &clock, &client, |sv, now, next| {
            if !asked && spawned_once.load(Ordering::Relaxed) {
                asked = true;
                sv.core.console.push_back("map mp_backlot".into());
                Cod4Game::drain_console(sv, now);
                *next = now;
            }
            true
        });
        let (changes, spawns) = client.join().unwrap();
        assert_eq!(changes, ["mp_backlot"]);
        assert_eq!(
            spawns,
            [Some("mp_crash".to_string()), Some("mp_backlot".to_string())]
        );
    }

    /// `record` against vcod-server's `war` with four shooting bots,
    /// watched as a spectator, then `demo` of the file in process
    /// (cod4-demos.md 6): the demo opens on the live table, holds every
    /// snapshot the session saw from its first full frame on, state for
    /// state, fires the same events off them, and ends on the trailer.
    #[test]
    fn cod4_live_recorded_demo_plays_back_what_the_session_saw() {
        use vcod_server::cod4::{Cod4Config, Cod4Server};
        let Some(install) = vcod_cod4::testing::cod4_install() else {
            return;
        };
        let mut sv = Cod4Server::new(
            Cod4Config {
                map: "mp_crash".into(),
                gametype: "war".into(),
                hostname: "vcod".into(),
                max_clients: 8,
            },
            7,
        );
        sv.set_cvar("scr_game_playerwaittime", "1");
        sv.set_cvar("scr_game_matchstarttime", "1");
        sv.load(Some(&install)).unwrap();
        sv.add_bots(4, true);
        let sock = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
        sock.set_nonblocking(true).unwrap();
        let addr = sock.local_addr().unwrap();
        let dir = Scratch::new("demo");
        let demos = dir.0.clone();
        let clock = sim::Clock::new();
        let client = sim::spawn(&clock, move || {
            let join = MenuJoin {
                team: Some("spectator".into()),
                class: None,
            };
            let mut s = Cod4Session::connect(addr, "recorder", Cp::Bare, join).unwrap();
            let t0 = clk_now();
            let mut started = None;
            let mut fired = Vec::new();
            let mut lines = Vec::new();
            while since(t0) < Duration::from_secs(16) {
                s.step(since(t0).as_secs_f64() * 1000.0).unwrap();
                s.take_commands();
                // Past the match start's `d`s (the `*_1mc_boost` slots at
                // 3 s): one landing while the record waits for its full
                // frame never reaches the file (cod4-demos.md 4).
                if started.is_none() && since(t0) > Duration::from_secs(4) {
                    let line = s.start_recording(&demos, Some("live"));
                    if line.starts_with("recording to") {
                        assert_eq!(line, "recording to demos/live.dm_1.");
                        assert_eq!(s.start_recording(&demos, None), "Already recording.");
                        started = Some(s.client().snapshots.len());
                        s.take_fired();
                    }
                }
                if started.is_some() {
                    fired.extend(s.take_fired());
                }
                lines.extend(s.recording_status().map(|(n, b)| (n.to_string(), b)));
                clk_sleep(Duration::from_millis(16));
            }
            assert_eq!(s.stop_recording(), "Stopped demo.");
            assert_eq!(s.stop_recording(), "Not recording demo.");
            let snaps = s.client().snapshots[started.unwrap()..].to_vec();
            let table: Vec<Vec<u8>> = s
                .gamestate()
                .unwrap()
                .all_configstrings()
                .into_iter()
                .map(<[u8]>::to_vec)
                .collect();
            s.disconnect();
            (snaps, fired, table, lines)
        });
        serve(&mut sv, &sock, &clock, &client, |_, _, _| true);
        let (live, live_fired, live_table, status) = client.join().unwrap();
        let (name, bytes) = status.last().cloned().unwrap();
        assert_eq!(name, "live");
        let data = std::fs::read(dir.0.join("live.dm_1")).unwrap();
        // The trailer: a type 0 record of -1 and -1.
        assert_eq!(
            &data[data.len() - 9..],
            &[0, 255, 255, 255, 255, 255, 255, 255, 255]
        );
        assert!(data.len() as u64 >= bytes);

        // `timedemo`: 50 ms of the demo a frame, one snapshot each
        // (vcod-server's 20 a second).
        let mut t = Cod4Session::play_demo(data.clone(), true).unwrap();
        let mut steps = 0;
        while !t.demo_over() && steps < 10_000 {
            t.step(f64::from(steps) * 3.0).unwrap();
            steps += 1;
        }
        let report = t.timedemo_report(steps * 3).unwrap();
        let frames: usize = report.split(' ').next().unwrap().parse().unwrap();
        let snaps = t.client().snapshots.len();
        assert!(frames.abs_diff(snaps) <= 2, "{report}: {snaps} snapshots");
        assert!(
            report.contains(" frames, ") && report.ends_with(" fps"),
            "{report}"
        );

        let mut d = Cod4Session::play_demo(data, false).unwrap();
        assert!(d.is_demo() && !d.is_live());
        assert_eq!(d.map_name().as_deref(), Some("mp_crash"));
        let mut fired = Vec::new();
        let mut now = 0.0;
        while !d.demo_over() && now < 120_000.0 {
            d.step(now).unwrap();
            d.take_commands();
            fired.extend(d.take_fired());
            now += 16.0;
        }
        assert!(d.demo_over(), "the demo never ended");
        let (messages, errors) = d.demo_counts().unwrap();
        assert_eq!(errors, 0);
        let demo = &d.client().snapshots;
        assert!(demo.len() > 100, "{} snapshots", demo.len());
        assert_eq!(
            messages,
            demo.len() + 1,
            "the gamestate, then a snapshot a message"
        );
        // The first is the full frame `demowaiting` asked for.
        assert_eq!(demo[0].delta_num, 0);
        let first = live
            .iter()
            .position(|s| s.server_time == demo[0].server_time)
            .expect("the demo's first frame is one the session saw");
        assert_eq!(live.len() - first, demo.len());
        for (l, m) in live[first..].iter().zip(demo) {
            assert_eq!(l.server_time, m.server_time);
            assert_eq!(l.ps, m.ps, "{}", l.server_time);
            assert_eq!(l.entities, m.entities, "{}", l.server_time);
            assert_eq!(l.clients, m.clients, "{}", l.server_time);
        }
        let table: Vec<Vec<u8>> = d
            .gamestate()
            .unwrap()
            .all_configstrings()
            .into_iter()
            .map(<[u8]>::to_vec)
            .collect();
        assert_eq!(table, live_table);
        // Events off the snapshots past the first, which both walked from
        // the same state.
        let key = |f: &Fired| (f.name, f.entity, f.server_time, f.parm);
        let after = |v: &[Fired]| -> Vec<_> {
            v.iter()
                .filter(|f| f.server_time > demo[0].server_time)
                .map(key)
                .collect()
        };
        let (a, b) = (after(&live_fired), after(&fired));
        eprintln!(
            "demo: {bytes} bytes, {messages} messages, {} snapshots, {} events",
            demo.len(),
            a.len()
        );
        assert!(!a.is_empty(), "no event in the session");
        assert_eq!(a, b);
    }

    /// `rcon` from a client joined to vcod-server (cod4-handshake.md 14.2):
    /// the console line through the shell and `CL_Rcon_f`'s rules, out of
    /// band on the connection's socket, the server's `print`s back as
    /// console lines. Before `rcon login` nothing goes; a wrong password
    /// is refused; the right one runs `status`.
    #[test]
    fn cod4_live_rcon_status_from_a_joined_client() {
        use crate::console::rcon::{Cod4Rcon, Cod4RconOut};
        use crate::console::shell::{Effect, Shell};
        use vcod_server::cod4::{Cod4Config, Cod4Server};
        let Some(install) = vcod_cod4::testing::cod4_install() else {
            return;
        };
        let mut sv = Cod4Server::new(
            Cod4Config {
                map: "mp_crash".into(),
                gametype: "war".into(),
                hostname: "vcod".into(),
                max_clients: 4,
            },
            7,
        );
        sv.set_cvar("rcon_password", "test");
        sv.load(Some(&install)).unwrap();
        let sock = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
        sock.set_nonblocking(true).unwrap();
        let addr = sock.local_addr().unwrap();
        let clock = sim::Clock::new();
        let client = sim::spawn(&clock, move || {
            let join = MenuJoin {
                team: Some("spectator".into()),
                class: None,
            };
            let mut s = Cod4Session::connect(addr, "admin", Cp::Bare, join).unwrap();
            let mut shell = Shell::new();
            let mut rcon = Cod4Rcon::default();
            let t0 = clk_now();
            let mut answers = Vec::new();
            for line in [
                "rcon status",
                "rcon login wrong",
                "rcon status",
                "rcon login test",
                "rcon status",
            ] {
                let effects = shell.execute(line);
                let [Effect::Rcon(args)] = effects.as_slice() else {
                    panic!("{line}");
                };
                let mut got = match rcon.command(args, s.is_live()) {
                    Cod4RconOut::Lines(l) => l,
                    Cod4RconOut::Server(text) => {
                        s.send_oob(&text);
                        Vec::new()
                    }
                    Cod4RconOut::Host(..) => panic!("joined, yet sent to the host"),
                };
                // Past the server's 500 ms between two rcons.
                let end = since(t0) + Duration::from_millis(700);
                while since(t0) < end {
                    s.step(since(t0).as_secs_f64() * 1000.0).unwrap();
                    s.take_commands();
                    s.take_fired();
                    got.extend(s.take_lines());
                    clk_sleep(Duration::from_millis(16));
                }
                answers.push(got);
            }
            s.disconnect();
            answers
        });
        serve(&mut sv, &sock, &clock, &client, |_, _, _| true);
        let a = client.join().unwrap();
        assert_eq!(
            a[0],
            ["You need to log in with 'rcon login <password>' before using rcon."]
        );
        assert!(a[1].is_empty(), "{:?}", a[1]);
        assert_eq!(a[2], ["Invalid password."]);
        assert!(a[3].is_empty(), "{:?}", a[3]);
        let status = a[4].join("\n");
        assert!(
            status.starts_with("map: mp_crash\nnum score ping"),
            "{status}"
        );
        assert!(status.contains(" admin^7"), "{status}");
    }

    /// `name` typed in a joined client's console (cod4-gsc.md 12): the
    /// userinfo goes again, and under `war`'s `auto_change` the client
    /// state the scoreboard and the kill feed read carries the new name
    /// within a second; the same name again sends nothing.
    #[test]
    fn cod4_live_name_change_reaches_the_client_states() {
        use crate::console::shell::{Effect, Shell};
        use vcod_server::cod4::{Cod4Config, Cod4Server};
        let Some(install) = vcod_cod4::testing::cod4_install() else {
            return;
        };
        let mut sv = Cod4Server::new(
            Cod4Config {
                map: "mp_crash".into(),
                gametype: "war".into(),
                hostname: "vcod".into(),
                max_clients: 4,
            },
            7,
        );
        sv.load(Some(&install)).unwrap();
        let sock = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
        sock.set_nonblocking(true).unwrap();
        let addr = sock.local_addr().unwrap();
        let clock = sim::Clock::new();
        let client = sim::spawn(&clock, move || {
            let join = MenuJoin {
                team: Some("allies".into()),
                class: Some("assault_mp,0".into()),
            };
            let mut s = Cod4Session::connect(addr, "before", Cp::Bare, join).unwrap();
            let mut shell = Shell::new();
            let own = |s: &Cod4Session| {
                let n = s.gamestate()?.client_num as u32;
                let snap = s.client().snapshots.last()?;
                snap.clients.iter().find(|c| c.0[0] == n).map(client_name)
            };
            let t0 = clk_now();
            let mut seen = Vec::new();
            let mut sent = 0;
            while since(t0) < Duration::from_secs(6) {
                s.step(since(t0).as_secs_f64() * 1000.0).unwrap();
                s.take_commands();
                s.take_fired();
                if since(t0) > Duration::from_secs(2) && sent == 0 {
                    let effects = shell.execute("name after");
                    assert!(effects.contains(&Effect::Userinfo), "{effects:?}");
                    s.set_name(&shell.userinfo().name);
                    sent = s.source_commands();
                    // The same name again queues nothing.
                    s.set_name("after");
                    assert_eq!(s.source_commands(), sent);
                }
                if let Some(n) = own(&s)
                    && seen.last() != Some(&n)
                {
                    seen.push(n);
                }
                clk_sleep(Duration::from_millis(16));
            }
            s.disconnect();
            seen
        });
        serve(&mut sv, &sock, &clock, &client, |_, _, _| true);
        let seen = client.join().unwrap();
        assert_eq!(seen, ["before", "after"]);
    }

    /// A player records himself running back and forth on mp_crash and
    /// turning, predicted as the app plays, and the demo plays back at
    /// 144 frames a second: his eye moves and his view turns nearly every
    /// frame, where they stood still between snapshots (cod4-demos.md 5).
    #[test]
    fn cod4_live_recorded_run_plays_back_smoothly() {
        use crate::play::input::Action;
        use vcod_server::cod4::{Cod4Config, Cod4Server};
        let Some(install) = vcod_cod4::testing::cod4_install() else {
            return;
        };
        let Some(world) = crash_world() else {
            return;
        };
        let mut sv = Cod4Server::new(
            Cod4Config {
                map: "mp_crash".into(),
                gametype: "war".into(),
                hostname: "vcod".into(),
                max_clients: 4,
            },
            7,
        );
        sv.set_cvar("scr_game_playerwaittime", "1");
        sv.set_cvar("scr_game_matchstarttime", "1");
        sv.load(Some(&install)).unwrap();
        let sock = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
        sock.set_nonblocking(true).unwrap();
        let addr = sock.local_addr().unwrap();
        let dir = Scratch::new("demorun");
        let demos = dir.0.clone();
        let clock = sim::Clock::new();
        let client = sim::spawn(&clock, move || {
            let join = MenuJoin {
                team: Some("allies".into()),
                class: Some("assault_mp,0".into()),
            };
            let mut s = Cod4Session::connect(addr, "runner", Cp::Bare, join).unwrap();
            s.set_world(world);
            let t0 = clk_now();
            let mut started = None;
            while since(t0) < Duration::from_secs(20) {
                let now = since(t0).as_secs_f64() * 1000.0;
                s.step(now).unwrap();
                s.take_commands();
                // The app draws a scene each frame, which predicts.
                s.scene();
                s.take_fired();
                if s.spawn().is_some() && started.is_none() {
                    started = Some(now);
                }
                if let Some(at) = started {
                    // A steady turn, 30 degrees a second.
                    s.view[1] = (s.view[1] + 0.21).rem_euclid(360.0);
                    // Back and forth, so a wall stops no run for long.
                    let back = ((now - at) / 1500.0) as i32 % 2 == 1;
                    s.input.key(Action::Forward, !back && now - at > 500.0);
                    s.input.key(Action::Back, back);
                    if now - at > 1000.0 && s.recording_status().is_none() {
                        s.start_recording(&demos, Some("run"));
                    }
                    if now - at > 7000.0 {
                        break;
                    }
                }
                clk_sleep(Duration::from_millis(7));
            }
            s.stop_recording();
            s.disconnect();
        });
        serve(&mut sv, &sock, &clock, &client, |_, _, _| true);
        client.join().unwrap();
        let data = std::fs::read(dir.0.join("run.dm_1")).unwrap();
        let mut d = Cod4Session::play_demo(data, false).unwrap();
        let mut eyes = Vec::new();
        let mut now = 0.0;
        while !d.demo_over() && now < 60_000.0 {
            d.step(now).unwrap();
            d.take_commands();
            d.take_fired();
            if let Some(sc) = d.scene() {
                eyes.push((sc.eye, sc.angles[1]));
            }
            now += 1000.0 / 144.0;
        }
        // Per frame: how far the eye went and the view turned.
        let steps: Vec<(f32, f32)> = eyes
            .windows(2)
            .map(|w| {
                let turn = (w[1].1 - w[0].1 + 540.0).rem_euclid(360.0) - 180.0;
                ((w[1].0 - w[0].0).truncate().length(), turn.abs())
            })
            .collect();
        // Frames past the recording's first second, where both run.
        let steps = &steps[steps.len().min(144)..];
        let still = |k: fn(&(f32, f32)) -> f32| {
            let mut v: Vec<f32> = steps.iter().map(k).collect();
            v.sort_by(f32::total_cmp);
            let median = v[v.len() / 2];
            let still = v.iter().filter(|&&x| x < median * 0.25).count();
            (median, still)
        };
        let (run, run_still) = still(|s| s.0);
        let (turn, turn_still) = still(|s| s.1);
        eprintln!(
            "demo run: {} frames, eye median {run:.3} still {run_still}, turn median {turn:.3} still {turn_still}",
            steps.len()
        );
        assert!(steps.len() > 500, "{} frames", steps.len());
        assert!(turn > 0.1, "the view never turned: {turn}");
        // A frame or two a second stand still at a wall or where the run
        // turns back; a stepped view stood still six frames in seven.
        assert!(turn_still * 20 < steps.len(), "{turn_still} still turns");
        assert!(run_still * 5 < steps.len(), "{run_still} still eyes");
    }

    /// A scratch directory under the temp dir, removed on drop.
    struct Scratch(std::path::PathBuf);

    impl Scratch {
        fn new(tag: &str) -> Scratch {
            let p = std::env::temp_dir().join(format!(
                "vcod-cod4-www-{tag}-{}-{:?}",
                std::process::id(),
                std::thread::current().id()
            ));
            let _ = std::fs::remove_dir_all(&p);
            std::fs::create_dir_all(&p).unwrap();
            Scratch(p)
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// A loopback HTTP server over `root`: each GET's path logged, the
    /// file sent (a byte longer for one under `pad`), or 404 for one under
    /// `deny` or not there, each answer `slow` in coming. It runs on the wall, so [`live_www`] holds
    /// its sim clock on [`HttpServer::busy`] while an exchange is open.
    fn http_server(
        root: std::path::PathBuf,
        deny: &'static [&'static str],
        pad: &'static [&'static str],
        slow: Duration,
    ) -> HttpServer {
        use std::io::{BufRead, Write};
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        l.set_nonblocking(true).unwrap();
        let port = l.local_addr().unwrap().port();
        let log = Arc::new(std::sync::Mutex::new(Vec::new()));
        let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let busy = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let (lg, st, bz) = (log.clone(), stop.clone(), busy.clone());
        std::thread::spawn(move || {
            while !st.load(std::sync::atomic::Ordering::Relaxed) {
                let Ok((sock, _)) = l.accept() else {
                    std::thread::sleep(Duration::from_millis(1));
                    continue;
                };
                bz.store(true, std::sync::atomic::Ordering::SeqCst);
                sock.set_nonblocking(false).unwrap();
                sock.set_read_timeout(Some(Duration::from_secs(60)))
                    .unwrap();
                let mut r = std::io::BufReader::new(sock);
                let mut first = String::new();
                r.read_line(&mut first).unwrap();
                let mut line = String::new();
                while r.read_line(&mut line).unwrap() > 2 {
                    line.clear();
                }
                let path = first.split_whitespace().nth(1).unwrap_or("/").to_string();
                lg.lock().unwrap().push(path.clone());
                std::thread::sleep(slow);
                let mut body = (!deny.contains(&path.as_str()))
                    .then(|| std::fs::read(root.join(path.trim_start_matches('/'))).ok())
                    .flatten();
                if let Some(b) = body.as_mut().filter(|_| pad.contains(&path.as_str())) {
                    b.push(0);
                }
                let mut w = r.into_inner();
                let _ = match body {
                    Some(b) => write!(w, "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n", b.len())
                        .and_then(|()| w.write_all(&b)),
                    None => write!(w, "HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\r\n"),
                };
                // Open until the client has read it all and hung up.
                let _ = w.shutdown(std::net::Shutdown::Write);
                let _ = std::io::Read::read(&mut w, &mut [0u8; 1]);
                bz.store(false, std::sync::atomic::Ordering::SeqCst);
            }
        });
        HttpServer {
            port,
            asked: log,
            stop,
            busy,
        }
    }

    /// [`http_server`]'s handles.
    struct HttpServer {
        port: u16,
        /// The paths asked for.
        asked: Arc<std::sync::Mutex<Vec<String>>>,
        stop: Arc<std::sync::atomic::AtomicBool>,
        /// An exchange is open: accepted, not yet closed by the client.
        busy: Arc<std::sync::atomic::AtomicBool>,
    }

    /// What [`live_www`] saw.
    struct Www {
        /// The paths the HTTP server was asked for.
        asked: Vec<String>,
        /// Every progress report.
        progress: Vec<Option<DownloadProgress>>,
        /// The files as they landed in the client's install, and as served.
        landed: Vec<(Option<Vec<u8>>, Vec<u8>)>,
        spawned: Option<String>,
        /// The client's clock from the join's start to the spawn's look.
        took: Duration,
        /// Why the join ended before the world, if it did.
        refused: Option<String>,
    }

    /// vcod-server on mp_crash under `fs_game mods/test` (a generated iwd
    /// holding the stock `weapons/mp/ak47_mp`, which the load reads off it
    /// so it is referenced, and a stock fastfile as its `mod.ff`), `sv_wwwDownload`
    /// on with `sets` and a loopback HTTP server under `sv_wwwBaseURL` that
    /// refuses `deny` and pads `pad`; a client with an empty install joins
    /// through `connect_with` and spawns.
    fn live_www(
        sets: &[(&str, &str)],
        deny: &'static [&'static str],
        pad: &'static [&'static str],
        slow: Duration,
    ) -> Option<Www> {
        use vcod_server::cod4::{Cod4Config, Cod4Server};
        let install = vcod_cod4::testing::cod4_install()?;
        let base = Scratch::new("base");
        let home = Scratch::new("home");
        for d in ["main", "zone"] {
            std::os::unix::fs::symlink(install.root().join(d), base.0.join(d)).unwrap();
        }
        let game = base.0.join("mods/test");
        std::fs::create_dir_all(&game).unwrap();
        let stock = vcod_common::pk3::Pk3Fs::open_iwd_layers(&install.root().join("main"), &[])
            .unwrap()
            .read("weapons/mp/ak47_mp")
            .unwrap();
        let filler: Vec<u8> = (0..20000u32)
            .map(|i| (i.wrapping_mul(2654435761) >> 24) as u8)
            .collect();
        let iwd = vcod_common::testing::pk3_with(&[
            ("weapons/mp/ak47_mp", &stock),
            ("vcod/f.bin", &filler),
        ]);
        std::fs::write(game.join("z_test.iwd"), &iwd).unwrap();
        let ff = std::fs::read(install.zone_dir("english").join("code_post_gfx_mp.ff")).unwrap();
        std::fs::write(game.join("mod.ff"), &ff).unwrap();

        let HttpServer {
            port,
            asked,
            stop,
            busy,
        } = http_server(base.0.clone(), deny, pad, slow);
        let mut sv = Cod4Server::new(
            Cod4Config {
                map: "mp_crash".into(),
                gametype: "war".into(),
                hostname: "vcod".into(),
                max_clients: 4,
            },
            7,
        );
        sv.game.cvars_mut().set("fs_game", "mods/test");
        sv.load(Some(&vcod_cod4::install::Install::at(&base.0).unwrap()))
            .unwrap();
        sv.game.cvars_mut().set("sv_wwwDownload", "1");
        sv.game
            .cvars_mut()
            .set("sv_wwwBaseURL", &format!("http://127.0.0.1:{port}"));
        for (k, v) in sets {
            sv.game.cvars_mut().set(k, v);
        }
        let sock = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
        sock.set_nonblocking(true).unwrap();
        let addr = sock.local_addr().unwrap();
        let root = home.0.clone();
        let clock = sim::Clock::new();
        let client = sim::spawn(&clock, move || {
            let join = MenuJoin {
                team: Some("allies".into()),
                class: Some("assault_mp,0".into()),
            };
            let start = clk_now();
            let mut progress = Vec::new();
            let mut report = |p| progress.push(p);
            let joined = Cod4Session::connect_with(
                addr,
                "joiner",
                Cp::Bare,
                join,
                Some(Fetch {
                    root: &root,
                    allow: true,
                    www: true,
                    progress: Some(&mut report),
                    cancel: None,
                    notes: None,
                }),
            );
            let mut s = match joined {
                Ok(s) => s,
                Err(e) => return (progress, None, since(start), Some(format!("{e:#}"))),
            };
            let t0 = clk_now();
            let mut spawned = None;
            while since(t0) < Duration::from_secs(60) && spawned.is_none() {
                s.step(since(t0).as_secs_f64() * 1000.0).unwrap();
                s.take_commands();
                spawned = s.spawn();
                clk_sleep(Duration::from_millis(16));
            }
            let took = since(start);
            s.disconnect();
            (progress, spawned, took, None)
        });
        // The fetch runs on the wall while the client's waits cost it next
        // to nothing: no server frame runs while one is in flight, or a
        // slow machine would see war's time limit pass before the spawn.
        serve(&mut sv, &sock, &clock, &client, |_, _, _| {
            let wall = Instant::now();
            while busy.load(std::sync::atomic::Ordering::SeqCst) {
                assert!(
                    wall.elapsed() < Duration::from_secs(50),
                    "an HTTP fetch hung"
                );
                std::thread::sleep(Duration::from_millis(1));
            }
            true
        });
        let (progress, spawned, took, refused) = client.join().unwrap();
        stop.store(true, std::sync::atomic::Ordering::Relaxed);
        let landed = [("z_test.iwd", iwd), ("mod.ff", ff)]
            .into_iter()
            .map(|(n, b)| (std::fs::read(home.0.join("mods/test").join(n)).ok(), b))
            .collect();
        let asked = asked.lock().unwrap().clone();
        Some(Www {
            asked,
            progress,
            landed,
            spawned,
            took,
            refused,
        })
    }

    /// A `sv_wwwDownload` redirect is fetched over HTTP: `wwwdl ack`, the
    /// GET, `wwwdl done` and the next file, then `donedl` and the join
    /// (cod4-handshake.md 11). The connect screen's progress ends at each
    /// file's size.
    #[test]
    fn cod4_live_www_redirect_fetches_over_http_and_joins() {
        let Some(w) = live_www(&[], &[], &[], Duration::ZERO) else {
            return;
        };
        assert_eq!(w.asked, ["/mods/test/z_test.iwd", "/mods/test/mod.ff"]);
        for (got, want) in &w.landed {
            assert!(got.as_ref() == Some(want), "a file did not land whole");
        }
        for (name, (_, want)) in ["mods/test/z_test.iwd", "mods/test/mod.ff"]
            .iter()
            .zip(&w.landed)
        {
            let full = DownloadProgress {
                file: name.to_string(),
                bytes: want.len() as u64,
                total: want.len() as u64,
            };
            assert!(w.progress.contains(&Some(full)), "{:?}", w.progress);
        }
        assert_eq!(w.progress.last(), Some(&None));
        assert!(w.spawned.is_some());
    }

    /// A fetch slow on the wall costs the join no time on the client's
    /// clock: the server runs no frame while it is open, where a few wall
    /// seconds once ran out war's time limit before the spawn.
    #[test]
    fn cod4_live_www_slow_fetch_costs_no_client_time() {
        let Some(w) = live_www(&[], &[], &[], Duration::from_secs(2)) else {
            return;
        };
        assert!(w.spawned.is_some());
        assert!(w.took < Duration::from_secs(20), "{:?}", w.took);
    }

    /// An HTTP failure answers `wwwdl fail`; the server sends the
    /// gamestate again and the file comes over UDP (0x816fc72).
    #[test]
    fn cod4_live_www_failure_falls_back_to_udp() {
        let Some(w) = live_www(&[], &["/mods/test/z_test.iwd"], &[], Duration::ZERO) else {
            return;
        };
        assert_eq!(w.asked, ["/mods/test/z_test.iwd", "/mods/test/mod.ff"]);
        for (got, want) in &w.landed {
            assert!(got.as_ref() == Some(want), "a file did not land whole");
        }
        assert!(w.spawned.is_some());
    }

    /// `sv_wwwDlDisconnected`: `wwwdl ack` and `wwwdl bbl8r`, the server
    /// drops us, the file lands and the client joins again, once a file.
    #[test]
    fn cod4_live_disconnected_www_reconnects_after_each_file() {
        let Some(w) = live_www(&[("sv_wwwDlDisconnected", "1")], &[], &[], Duration::ZERO) else {
            return;
        };
        assert_eq!(w.asked, ["/mods/test/z_test.iwd", "/mods/test/mod.ff"]);
        for (got, want) in &w.landed {
            assert!(got.as_ref() == Some(want), "a file did not land whole");
        }
        assert!(w.spawned.is_some());
    }

    /// A file that still differs after its download is kept, as 1.7
    /// keeps it, and asked for again on `donedl`'s gamestate; where
    /// retail would fetch it on every gamestate, the join ends there.
    #[test]
    fn cod4_live_www_file_still_different_ends_the_join() {
        let Some(w) = live_www(&[], &[], &["/mods/test/mod.ff"], Duration::ZERO) else {
            return;
        };
        assert_eq!(w.asked, ["/mods/test/z_test.iwd", "/mods/test/mod.ff"]);
        let (got, want) = &w.landed[1];
        assert_eq!(got.as_ref().map(Vec::len), Some(want.len() + 1));
        assert!(w.spawned.is_none());
        let why = w.refused.unwrap();
        assert!(why.contains("mods/test/mod.ff is different"), "{why}");
    }

    /// The level time of the newest snapshot once `ui_score_bar "0"` has
    /// come, among the commands since the last call.
    fn score_bar_off(s: &mut Cod4Session) -> Option<i32> {
        let off = s
            .take_commands()
            .iter()
            .any(|c| String::from_utf8_lossy(c).contains("ui_score_bar \"0\""));
        off.then(|| s.client().snapshots.last().map(|f| f.server_time))?
    }

    /// What `sv_fps` shows a client: the `serverTime` steps between
    /// snapshots past the first (which follows the gamestate), and the
    /// level time from the spawn's snapshot (`otherFlags` 4) to `off`.
    fn frame_timing(s: &Cod4Session, off: i32) -> (std::collections::BTreeSet<i32>, i32) {
        let snaps = &s.client().snapshots;
        let steps = snaps
            .windows(2)
            .skip(1)
            .map(|w| w[1].server_time - w[0].server_time)
            .collect();
        let spawn = snaps
            .iter()
            .find(|f| f.ps.get("otherFlags").unwrap_or(0) & 4 != 0)
            .expect("a spawn")
            .server_time;
        (steps, off - spawn)
    }

    /// vcod-server at `sv_fps` 40 against retail's capture at 40
    /// (cod4-server-frame.md 8): each frame steps 25 and a snapshot goes
    /// out every frame, and `showClientScoreBar(5.0)`'s `wait` counts 100
    /// frames, 2500 ms of level time.
    #[test]
    fn cod4_live_sv_fps_40_frames_and_waits_as_retail() {
        let text = std::fs::read_to_string(format!("{FIXTURES}/play-1.7-svfps40.txt")).unwrap();
        let mut s = Cod4Session::open_capture(&text).unwrap();
        let (mut ms, mut off) = (0.0, None);
        while !s.finished() && off.is_none() {
            ms += 1000.0 / 60.0;
            s.step(ms).unwrap();
            off = score_bar_off(&mut s);
        }
        let retail = frame_timing(&s, off.expect("retail's ui_score_bar 0"));
        assert_eq!(retail, (std::collections::BTreeSet::from([25]), 2500));
        let Some(ours) = live_join_on(
            "mp_crash",
            None,
            &[("sv_fps", "40")],
            |s, spawned, t0, _| {
                assert!(spawned.is_some());
                let mut off = None;
                while since(t0) < Duration::from_secs(20) && off.is_none() {
                    s.step(since(t0).as_secs_f64() * 1000.0).unwrap();
                    off = score_bar_off(s);
                    clk_sleep(Duration::from_millis(16));
                }
                frame_timing(s, off.expect("ui_score_bar 0"))
            },
        ) else {
            return;
        };
        assert_eq!(ours, retail);
    }

    /// What the first snapshot after the gamestate carries: the commands
    /// the gamestate already held, the playerstate's words but its
    /// `commandTime`, and the entities by number.
    fn first_snapshot(s: &Cod4Session) -> (i32, Vec<(&'static str, u32)>, Vec<u32>) {
        let c = s.client();
        let f = c.snapshots.first().expect("a snapshot");
        let ps = vcod_cod4::net::fields::PLAYER_STATE
            .iter()
            .filter(|d| d.name != "commandTime")
            .map(|d| (d.name, f.ps.words[d.word()]))
            .filter(|&(_, v)| v != 0)
            .collect();
        let gs = c.gamestate.as_ref().expect("a gamestate");
        (
            gs.server_command_sequence,
            ps,
            f.entities.iter().map(|e| e.number()).collect(),
        )
    }

    /// The first snapshot after the gamestate as retail's
    /// (`play-1.7-svfps40.txt`, cod4-server-frame.md 8): the connect's two
    /// `v` commands ride the gamestate, and the team menu's spectator reads
    /// every playerstate word retail sends, `moveSpeedScaleMultiplier` 1
    /// among them.
    #[test]
    fn cod4_live_first_snapshot_as_retail() {
        let text = std::fs::read_to_string(format!("{FIXTURES}/play-1.7-svfps40.txt")).unwrap();
        let mut s = Cod4Session::open_capture(&text).unwrap();
        let mut ms = 0.0;
        while !s.finished() && s.client().snapshots.is_empty() {
            ms += 1000.0 / 60.0;
            s.step(ms).unwrap();
        }
        let retail = first_snapshot(&s);
        assert_eq!(retail.0, 2);
        let Some(ours) = live_join_on("mp_crash", None, &[("sv_fps", "40")], |s, _, _, _| {
            let (seq, ps, ents) = first_snapshot(s);
            (seq, ps, ents)
        }) else {
            return;
        };
        assert_eq!(ours, retail);
    }

    /// A 60 Hz frame grid for a live test's loop: [`FrameClock::wait`]
    /// sleeps to the next 16 ms mark, so a frame's own work does not
    /// stretch the frame as a fixed sleep after it would.
    struct FrameClock(Instant);

    impl FrameClock {
        fn new() -> FrameClock {
            FrameClock(clk_now())
        }

        fn wait(&mut self) {
            self.0 += Duration::from_millis(16);
            let now = clk_now();
            match self.0.checked_duration_since(now) {
                Some(d) => clk_sleep(d),
                // Behind: the next frame at once, the grid from here.
                None => self.0 = now,
            }
        }
    }

    /// vcod-server's CoD4 `war` on mp_crash in process (`COD4_DIR`;
    /// `None` without), on a loopback socket: the client joins with
    /// `--team allies --class assault_mp,0` and steps until it spawns (or
    /// 60 s pass), then `play` runs on the session with the spawn's
    /// weapons and the clock its steps read; the client disconnects after.
    fn live_join<T: Send + 'static>(
        play: impl FnOnce(&mut Cod4Session, Option<String>, Instant) -> T + Send + 'static,
    ) -> Option<T> {
        live_join_ctl(move |s, spawned, t0, _| play(s, spawned, t0))
    }

    /// What a [`live_join_ctl`] client asks of the server's loop.
    enum Ctl {
        /// A server command to slot 0.
        Command(String),
        /// The server stops: no packet read, frame run or sent.
        Silence,
        /// A live frag of slot 0's lies at the point with the fuse left.
        DropFrag([f32; 3], i32),
        /// An axis bot joins, alone and held still, and is kept standing at
        /// the point (its feet), facing west, at full health.
        PinBot([f32; 3]),
        /// [`Ctl::PinBot`]'s bot at his own health, let go once dead.
        PinVictim([f32; 3]),
        /// [`Ctl::PinBot`]'s bot, fighting.
        PinShooter([f32; 3]),
        /// An axis bot joins, is put with his eye at the point and uses
        /// the gun entity, as the use key would, until he is on it.
        MountBot([f32; 3], u32),
        /// Slot 0's rounds the server fired so far: game time, start, the
        /// unspread end along the view, end.
        Shots(std::sync::mpsc::Sender<Vec<(i32, Vec3, Vec3, Vec3)>>),
        /// Slot 0's `commandTime` and view angles on the server, sent back.
        Probe(std::sync::mpsc::Sender<(i32, [f32; 3])>),
        /// Slot 0's stats block on the server, sent back.
        Stats(std::sync::mpsc::Sender<vcod_cod4::net::stats::Stats>),
        /// A bot joins; its name, sent back.
        AddBot(std::sync::mpsc::Sender<String>),
        /// A line for the server console.
        Console(String),
        /// Slot 0 takes the damage along the direction (none: from no
        /// direction), without knockback and his health left alone.
        Hurt(i32, Option<[f32; 3]>),
        /// The script models a round meets: number, origin, open to damage.
        Models(std::sync::mpsc::Sender<ModelList>),
        /// Where each line first meets the world or a script model on the
        /// server, and the model's number.
        ModelTrace(Vec<(Vec3, Vec3)>, std::sync::mpsc::Sender<ModelHits>),
    }

    type ModelList = Vec<(u32, Vec3, bool)>;
    type ModelHits = Vec<(Vec3, Option<u32>)>;

    /// [`live_join`] with a line to the server's loop.
    fn live_join_ctl<T: Send + 'static>(
        play: impl FnOnce(&mut Cod4Session, Option<String>, Instant, std::sync::mpsc::Sender<Ctl>) -> T
        + Send
        + 'static,
    ) -> Option<T> {
        live_join_on("mp_crash", crash_world(), &[], play)
    }

    /// [`live_join_ctl`] on `map` with `sets`, predicting through `world`
    /// when given.
    fn live_join_on<T: Send + 'static>(
        map: &str,
        world: Option<Arc<Cod4World>>,
        sets: &[(&str, &str)],
        play: impl FnOnce(&mut Cod4Session, Option<String>, Instant, std::sync::mpsc::Sender<Ctl>) -> T
        + Send
        + 'static,
    ) -> Option<T> {
        live_join_as(map, world, sets, "assault_mp,0", play)
    }

    /// [`live_join_on`] picking `class` from the class menu.
    fn live_join_as<T: Send + 'static>(
        map: &str,
        world: Option<Arc<Cod4World>>,
        sets: &[(&str, &str)],
        class: &'static str,
        play: impl FnOnce(&mut Cod4Session, Option<String>, Instant, std::sync::mpsc::Sender<Ctl>) -> T
        + Send
        + 'static,
    ) -> Option<T> {
        use vcod_server::cod4::{Cod4Config, Cod4Server};
        let install = vcod_cod4::testing::cod4_install()?;
        let mut sv = Cod4Server::new(
            Cod4Config {
                map: map.into(),
                gametype: "war".into(),
                hostname: "vcod".into(),
                max_clients: 4,
            },
            7,
        );
        for (k, v) in sets {
            sv.set_cvar(k, v);
        }
        sv.load(Some(&install)).unwrap();
        let sock = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
        sock.set_nonblocking(true).unwrap();
        let addr = sock.local_addr().unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        let clock = sim::Clock::new();
        let client = sim::spawn(&clock, move || {
            let join = MenuJoin {
                team: Some("allies".into()),
                class: Some(class.into()),
            };
            let mut s = Cod4Session::connect(addr, "joiner", Cp::Bare, join).unwrap();
            if let Some(w) = world {
                s.set_world(w);
            }
            let t0 = clk_now();
            let mut spawned = None;
            while since(t0) < Duration::from_secs(60) && spawned.is_none() {
                s.step(since(t0).as_secs_f64() * 1000.0).unwrap();
                s.take_commands();
                spawned = s.spawn();
                clk_sleep(Duration::from_millis(16));
            }
            let out = play(&mut s, spawned, t0, tx);
            s.disconnect();
            out
        });
        let mut silent = false;
        let mut pinned: Option<(usize, [f32; 3], bool)> = None;
        // A victim's pin: mortal, and whether he has stood up yet.
        let (mut mortal, mut up) = (false, false);
        let mut mounting: Option<(usize, [f32; 3], u32)> = None;
        serve(&mut sv, &sock, &clock, &client, |sv, now, _| {
            let mut pin = None;
            match rx.try_recv() {
                Ok(Ctl::Command(text)) => sv.send_server_command(0, &text),
                Ok(Ctl::Silence) => silent = true,
                Ok(Ctl::DropFrag(at, fuse)) => {
                    let l = sv.game.level.as_mut().unwrap();
                    assert!(l.drop_frag_for_test(0, Vec3::from(at), fuse));
                }
                Ok(Ctl::PinBot(at)) => pin = Some((at, false)),
                Ok(Ctl::PinVictim(at)) => {
                    pin = Some((at, false));
                    mortal = true;
                }
                Ok(Ctl::PinShooter(at)) => pin = Some((at, true)),
                Ok(Ctl::MountBot(eye, gun)) => {
                    mounting = Some((sv.add_bots(2, false)[1], eye, gun));
                }
                Ok(Ctl::Shots(tx)) => {
                    let l = sv.game.level.as_ref().unwrap();
                    let shots = l.combat_shots().iter().filter(|r| r.0 == 0);
                    let _ = tx.send(shots.map(|r| (r.1, r.2, r.3, r.4)).collect());
                }
                Ok(Ctl::Probe(back)) => {
                    let l = sv.game.level.as_ref().unwrap();
                    if let Some(ps) = l.pm_state(0) {
                        let _ = back.send((ps.command_time, ps.view_angles));
                    }
                }
                Ok(Ctl::Stats(back)) => {
                    let l = sv.game.level.as_ref().unwrap();
                    let _ = back.send(l.stats(0).unwrap().clone());
                }
                Ok(Ctl::AddBot(back)) => {
                    let bot = sv.add_bots(1, false)[0];
                    let l = sv.game.level.as_ref().unwrap();
                    let _ = back.send(l.client_name(bot).to_string());
                }
                Ok(Ctl::Models(back)) => {
                    let l = sv.game.level.as_mut().unwrap();
                    let _ = back.send(l.shot_models_for_test());
                }
                Ok(Ctl::ModelTrace(lines, back)) => {
                    let l = sv.game.level.as_mut().unwrap();
                    let hits = lines
                        .into_iter()
                        .map(|(a, b)| l.shot_model_trace_for_test(a, b))
                        .collect();
                    let _ = back.send(hits);
                }
                Ok(Ctl::Hurt(damage, dir)) => {
                    let l = sv.game.level.as_mut().unwrap();
                    l.hurt_for_test(0, damage, dir);
                }
                Ok(Ctl::Console(line)) => {
                    use vcod_server::engine::GameModule;
                    sv.core.console.push_back(line);
                    vcod_server::cod4::Cod4Game::drain_console(sv, now);
                }
                Err(_) => {}
            }
            if let Some((at, shoot)) = pin {
                // The second bot of a team game is axis. A bot that only
                // stands there is the one axis bot, held still: a walking
                // one stepped a frame off its pin before every shot, and an
                // allied one crossed the line of fire.
                let add = |sv: &mut vcod_server::cod4::Cod4Server| match shoot {
                    true => sv.add_bots(2, true)[1],
                    false => sv.add_held_bot_for_test(true).unwrap(),
                };
                let bot = pinned.map_or_else(|| add(sv), |p| p.0);
                pinned = Some((bot, at, shoot));
            }
            if let Some((bot, at, shoot)) = pinned
                && let Some(l) = sv.game.level.as_mut()
                && l.playerstate(bot).is_some()
            {
                // Healed every frame past what any frame's rounds take.
                if !shoot && !mortal {
                    l.heal_for_test(bot);
                }
                let Some(ps) = l.playerstate(bot) else {
                    return !silent;
                };
                // A victim's body stays where he fell, and so does his
                // next life.
                let alive = ps.get("pm_type") == Some(0);
                if mortal && up && !alive {
                    pinned = None;
                    return !silent;
                }
                up |= alive;
                let o =
                    [0, 1].map(|k| f32::from_bits(ps.get(&format!("origin[{k}]")).unwrap_or(0)));
                if (o[0] - at[0]).hypot(o[1] - at[1]) > 2.0 {
                    l.set_view_pos(bot, [at[0], at[1], at[2] + 60.0], [0.0, 180.0, 0.0]);
                }
            }
            if let Some((bot, eye, gun)) = mounting
                && let Some(l) = sv.game.level.as_mut()
                && let Some(ps) = l.playerstate(bot)
                && ps.get("pm_type") == Some(0)
            {
                let o =
                    [0, 1].map(|k| f32::from_bits(ps.get(&format!("origin[{k}]")).unwrap_or(0)));
                if (o[0] - eye[0]).hypot(o[1] - eye[1]) > 2.0 {
                    l.set_view_pos(bot, eye, [0.0, 0.0, 0.0]);
                } else if l.use_turret_for_test(bot, gun) {
                    mounting = None;
                }
            }
            !silent
        });
        Some(client.join().unwrap())
    }

    /// `rcon kick "[BOT] Major Lag"`: the others' game message reads the
    /// whole name and the localized reason, as 1.7's quoted
    /// `e "\x15<name>^7 \x14EXE_PLAYERKICKED"` does (cod4-handshake.md 2).
    #[test]
    fn cod4_live_kick_message_keeps_the_whole_name() {
        let Some(r) = live_join_ctl(|s, spawned, t0, ctl| {
            spawned.as_ref()?;
            let (tx, rx) = std::sync::mpsc::channel();
            ctl.send(Ctl::AddBot(tx)).unwrap();
            let mut name = None;
            let mut seen = Vec::new();
            let end = since(t0) + Duration::from_secs(10);
            while since(t0) < end {
                s.step(since(t0).as_secs_f64() * 1000.0).unwrap();
                seen.extend(s.take_commands());
                if name.is_none()
                    && let Ok(n) = rx.try_recv()
                {
                    ctl.send(Ctl::Console(format!("kick \"{n}\""))).unwrap();
                    name = Some(n);
                }
                if seen.iter().any(|c| c.starts_with(b"K ")) {
                    break;
                }
                clk_sleep(Duration::from_millis(16));
            }
            Some((name?, seen))
        })
        .flatten() else {
            return;
        };
        let (name, seen) = r;
        assert!(name.contains(' '), "{name}");
        let Some(vcod_cod4::hud::Command::GameMessage(msg)) = seen
            .iter()
            .filter_map(|c| vcod_cod4::hud::Command::parse(c))
            .find(|c| matches!(c, vcod_cod4::hud::Command::GameMessage(_)))
        else {
            panic!("no game message in {seen:?}");
        };
        let install = vcod_cod4::testing::cod4_install().unwrap();
        let strings = install.code_strings("english").unwrap();
        let kicked = &strings["EXE_PLAYERKICKED"];
        // The HUD's own path (`Cod4Hud::message`).
        let mut loc = vcod_common::localize::Localized::default();
        for (k, v) in &strings {
            loc.insert(k, v);
        }
        let shown = loc.message(&msg);
        assert_eq!(shown, format!("{name}^7 {kicked}"), "{msg:?}");
    }

    /// The stock scripts' `setStat`s on a ranked (`sv_pure 1`) join reach
    /// the client as `N <index> <value>`: its profile's block ends equal
    /// to the server's, which the rank scripts moved off a fresh one's.
    #[test]
    fn cod4_live_set_stat_reaches_the_clients_profile() {
        use vcod_cod4::net::stats::Stats;
        let Some((client, server)) = live_join_ctl(|s, spawned, t0, ctl| {
            spawned.as_ref()?;
            let end = since(t0) + Duration::from_millis(1500);
            while since(t0) < end {
                s.step(since(t0).as_secs_f64() * 1000.0).unwrap();
                clk_sleep(Duration::from_millis(16));
            }
            let (tx, rx) = std::sync::mpsc::channel();
            ctl.send(Ctl::Stats(tx)).unwrap();
            let server = loop {
                s.step(since(t0).as_secs_f64() * 1000.0).unwrap();
                clk_sleep(Duration::from_millis(16));
                if let Ok(b) = rx.try_recv() {
                    break b;
                }
            };
            let client: Vec<i32> = (0..3498).map(|i| s.menus.stat(i)).collect();
            Some((client, server))
        })
        .flatten() else {
            return;
        };
        let fresh = Stats::fresh();
        let at = |b: &Stats, i: i32| b.get(i as u16).unwrap_or(0) as i32;
        let moved: Vec<i32> = (4..3498)
            .filter(|&i| at(&server, i) != at(&fresh, i))
            .collect();
        assert!(
            !moved.is_empty(),
            "the scripts set no stat off a fresh profile's"
        );
        let differ: Vec<(i32, i32, i32)> = (4..3498)
            .filter(|&i| client[i as usize] != at(&server, i))
            .map(|i| (i, client[i as usize], at(&server, i)))
            .collect();
        assert!(
            differ.is_empty(),
            "client, server: {differ:?} (moved {moved:?})"
        );
    }

    /// A map change's `requeststats` sends the block as the `N`s left it,
    /// not the join's: 300 XP the client took before `map mp_crash` reach
    /// the new level's `_rank.gsc`, which ranks the player 3 off them.
    #[test]
    fn cod4_live_map_change_sends_the_stats_the_server_set() {
        let Some(server) = live_join_ctl(|s, spawned, t0, ctl| {
            spawned.as_ref()?;
            let step = |s: &mut Cod4Session, ms: u64| {
                let end = since(t0) + Duration::from_millis(ms);
                while since(t0) < end {
                    s.step(since(t0).as_secs_f64() * 1000.0).unwrap();
                    s.take_commands();
                    s.take_map_change();
                    clk_sleep(Duration::from_millis(16));
                }
            };
            ctl.send(Ctl::Command("N 2301 300".into())).unwrap();
            step(s, 500);
            ctl.send(Ctl::Console("map mp_crash".into())).unwrap();
            let mut last = None;
            for _ in 0..40 {
                step(s, 500);
                let (tx, rx) = std::sync::mpsc::channel();
                ctl.send(Ctl::Stats(tx)).unwrap();
                let got = loop {
                    step(s, 16);
                    if let Ok(b) = rx.try_recv() {
                        break b;
                    }
                };
                let rank = (got.get(2301), got.get(2350));
                last = Some(got);
                if rank == (Some(300), Some(3)) {
                    break;
                }
            }
            last
        })
        .flatten() else {
            return;
        };
        assert_eq!((server.get(2301), server.get(2350)), (Some(300), Some(3)));
    }

    /// A joined client crouched with `gocrouch`, then sprinting: the
    /// server's forced stand (`EV_STANCE_FORCE_STAND`) becomes the
    /// stance keys' state, so the body stays up after the sprint instead
    /// of crouching again a cmd later (cod4-pmove.md 8).
    #[test]
    fn cod4_live_sprint_out_of_a_crouch_stays_standing() {
        use crate::console::shell::Effect;
        use crate::play::cod4_input::Stance;
        use crate::play::input::Action;
        use vcod_cod4::pmove::pmf;
        let sets = [
            ("scr_game_playerwaittime", "1"),
            ("scr_game_matchstarttime", "1"),
        ];
        let Some(r) = live_join_on("mp_crash", crash_world(), &sets, |s, spawned, t0, _| {
            spawned.as_ref()?;
            let step = |s: &mut Cod4Session, ms: u64| {
                let end = since(t0) + Duration::from_millis(ms);
                while since(t0) < end {
                    s.step(since(t0).as_secs_f64() * 1000.0).unwrap();
                    s.scene();
                    s.take_commands();
                    s.take_fired();
                    clk_sleep(Duration::from_millis(16));
                }
            };
            let flags = |s: &Cod4Session| {
                let f = s.client().snapshots.last();
                f.and_then(|f| f.ps.get("pm_flags")).unwrap_or(0)
            };
            step(s, 3000);
            s.input.effect(&Effect::Impulse(Action::Crouch), &[]);
            step(s, 800);
            let crouched = (s.input.stance, flags(s) & pmf::DUCKED != 0);
            s.input.key(Action::Forward, true);
            s.input.key(Action::Sprint, true);
            let mut sprinted = false;
            for _ in 0..10 {
                step(s, 100);
                sprinted |= flags(s) & pmf::SPRINTING != 0;
            }
            s.input.key(Action::Sprint, false);
            s.input.key(Action::Forward, false);
            step(s, 1000);
            Some((crouched, sprinted, s.input.stance, flags(s)))
        }) else {
            return;
        };
        let (crouched, sprinted, stance, after) = r.expect("never spawned");
        assert_eq!(crouched, (Stance::Crouch, true));
        assert!(sprinted, "never sprinted");
        assert_eq!(stance, Stance::Stand);
        assert_eq!(after & (pmf::DUCKED | pmf::PRONE), 0, "pm_flags {after:#x}");
    }

    /// The client's cheat dvars follow the server's `sv_cheats` through
    /// the systeminfo, as `CL_SystemInfoChanged` (iw3mp.exe 0x473ab0)
    /// sets it: set at the server's start, `fx_draw` and `r_glow` take a
    /// 0; an rcon-style `sv_cheats 0` on the server's console reaches the
    /// client as `d 1`, locks them and puts them back; `sv_cheats 1`
    /// unlocks them again (cod4-client-perf.md 20).
    #[test]
    fn cod4_live_cheat_dvars_follow_the_servers_sv_cheats() {
        use crate::console::shell::{Effect, Shell};
        let sets = [
            ("sv_cheats", "1"),
            ("scr_game_playerwaittime", "1"),
            ("scr_game_matchstarttime", "1"),
        ];
        let Some(r) = live_join_on("mp_crash", None, &sets, |s, _, t0, ctl| {
            let mut shell = Shell::new();
            shell.register_cod4_options();
            // steps until the systeminfo's sv_cheats reads `want`
            let follow = |s: &mut Cod4Session, shell: &mut Shell, want: &str| {
                let end = since(t0) + Duration::from_secs(5);
                while since(t0) < end {
                    s.step(since(t0).as_secs_f64() * 1000.0).unwrap();
                    s.take_commands();
                    if let Some(si) = s.take_systeminfo() {
                        shell.apply_systeminfo(&si);
                    }
                    if shell.cvar("sv_cheats") == Some(want) {
                        return true;
                    }
                    clk_sleep(Duration::from_millis(16));
                }
                false
            };
            let mut out = Vec::new();
            out.push(follow(s, &mut shell, "1"));
            shell.execute("fx_draw 0");
            shell.execute("r_glow 0");
            out.push(shell.cvar("fx_draw") == Some("0") && shell.cvar("r_glow") == Some("0"));
            ctl.send(Ctl::Console("sv_cheats 0".into())).unwrap();
            out.push(follow(s, &mut shell, "0"));
            out.push(shell.cvar("fx_draw") == Some("1") && shell.cvar("r_glow") == Some("1"));
            out.push(
                shell.execute("fx_draw 0") == [Effect::Print("fx_draw is cheat protected.".into())],
            );
            ctl.send(Ctl::Console("sv_cheats 1".into())).unwrap();
            out.push(follow(s, &mut shell, "1"));
            shell.execute("fx_draw 0");
            out.push(shell.cvar("fx_draw") == Some("0"));
            out
        }) else {
            return;
        };
        assert_eq!(
            r, [true; 7],
            "start, set at start, rcon 0, reset, locked, rcon 1, set again"
        );
    }

    /// mp_backlot's crouch SAW (entity 393, yaw 0, arcs 45 and -15..30)
    /// through a joined client, as the retail capture plays it
    /// (cod4-gsc.md 9.2): behind it the hint names it, a use press mans it
    /// (`eFlags` 0x200, the view locked to it), the held trigger fires a
    /// round a frame off the gun (`EV_FIRE_WEAPON_MG42`, its firing bit,
    /// `viewlocked` 2), a look past its arc leaves the barrel on the arc,
    /// and a second press lets go (`EV_STANCE_FORCE_STAND`, the lock off).
    #[test]
    fn cod4_live_saw_mounts_fires_clamps_and_lets_go() {
        use crate::play::input::Action;
        const GUN: u32 = 393;
        let sets = [
            ("sv_cheats", "1"),
            ("scr_game_playerwaittime", "1"),
            ("scr_game_matchstarttime", "1"),
        ];
        let Some(r) = live_join_on("mp_backlot", None, &sets, |s, spawned, t0, _| {
            spawned.as_ref()?;
            let step = |s: &mut Cod4Session, ms: u64| {
                let end = since(t0) + Duration::from_millis(ms);
                while since(t0) < end {
                    s.step(since(t0).as_secs_f64() * 1000.0).unwrap();
                    s.take_commands();
                    clk_sleep(Duration::from_millis(16));
                }
            };
            let ps = |s: &Cod4Session, n: &str| {
                s.client()
                    .snapshots
                    .last()
                    .and_then(|f| f.ps.get(n))
                    .unwrap_or(0)
            };
            let gun = |s: &Cod4Session| {
                let f = s.client().snapshots.last()?;
                f.entities.iter().find(|e| e.number() == GUN).cloned()
            };
            step(s, 2500);
            s.command("setviewpos -770 68 320 0");
            s.view = [0.0, 0.0];
            step(s, 1500);
            let hint = (ps(s, "cursorHint"), ps(s, "cursorHintEntIndex"));
            let weapon = gun(s).map(|e| ent_u32(&e, "weapon"));
            s.input.key(Action::Use, true);
            step(s, 200);
            s.input.key(Action::Use, false);
            step(s, 300);
            let mounted = (
                ps(s, "eFlags") & 0x300,
                ps(s, "viewlocked"),
                ps(s, "viewlocked_entNum"),
            );
            // The gunner's crosshair is the gun's own reticle.
            let reticle = vcod_cod4::testing::cod4_install().and_then(|install| {
                let mut hud = crate::cod4_hud::Cod4Hud::default();
                hud.load(&install.root().join("main"), "mp_backlot");
                let v = s.scene()?.view;
                let shell = crate::console::shell::Shell::new();
                let q = hud.crosshair(s, &v, [0.75, 0.5], (1280.0, 960.0), &shell);
                let name = weapon_list(s.gamestate()?)
                    .get(weapon? as usize - 1)?
                    .clone();
                Some((q, hud.weapon_hud(&name)?.reticle.center_size))
            });
            let origin =
                |s: &Cod4Session| [0, 1, 2].map(|k| f32::from_bits(ps(s, &format!("origin[{k}]"))));
            let barrel = |s: &Cod4Session| {
                gun(s).map_or(f32::NAN, |e| {
                    f32::from_bits(ent_u32(&e, "lerp.u.anonymous.data[1]"))
                })
            };
            let mut placed = vec![(barrel(s), origin(s))];
            let seq0 = gun(s).map_or(0, |e| ent_u32(&e, "eventSequence"));
            s.input.key(Action::Attack, true);
            let mut jitter = false;
            let mut firing = false;
            for _ in 0..20 {
                step(s, 50);
                jitter |= ps(s, "viewlocked") == 2;
                firing |= gun(s).is_some_and(|e| ent_u32(&e, "lerp.eFlags") & 0x40 != 0);
            }
            s.input.key(Action::Attack, false);
            let fired = gun(s).map(|e| {
                let seq = ent_u32(&e, "eventSequence");
                let last = ent_u32(&e, &format!("events[{}]", (seq.wrapping_sub(1)) & 3));
                (seq.wrapping_sub(seq0) & 0xff, last)
            });
            s.view = [0.0, 80.0];
            step(s, 800);
            let yaw = gun(s).map(|e| f32::from_bits(ent_u32(&e, "lerp.u.anonymous.data[1]")));
            placed.push((barrel(s), origin(s)));
            s.view = [0.0, -80.0];
            step(s, 800);
            placed.push((barrel(s), origin(s)));
            s.input.key(Action::Use, true);
            step(s, 200);
            s.input.key(Action::Use, false);
            step(s, 500);
            let released = (
                ps(s, "eFlags") & 0x300,
                ps(s, "viewlocked"),
                ps(s, "viewlocked_entNum"),
            );
            Some((
                hint, weapon, mounted, jitter, firing, fired, yaw, released, placed, reticle,
            ))
        }) else {
            return;
        };
        let (hint, weapon, mounted, jitter, firing, fired, yaw, released, placed, reticle) =
            r.expect("never spawned");
        // `CG_DrawTurretCrossHair`: one centred quad, the gun's
        // `reticleCenterSize` on the 2 px a unit of 1280 by 960.
        let (quads, size) = reticle.expect("no crosshair read");
        assert_eq!(quads.len(), 1);
        let xs = quads[0].verts.map(|v| v[0]);
        let (lo, hi) = (
            xs.iter().copied().fold(f32::MAX, f32::min),
            xs.iter().copied().fold(f32::MIN, f32::max),
        );
        assert!(
            (lo + hi - 1280.0).abs() < 0.01 && (hi - lo - 2.0 * size as f32).abs() < 0.01,
            "{:?} {size}",
            quads[0].verts
        );
        let w = weapon.expect("no gun 393 in the snapshot");
        assert_eq!(hint, (w + 4, GUN), "hint");
        assert_eq!(mounted, (0x200, 1, GUN), "mount");
        assert!(
            jitter && firing,
            "no round: jitter {jitter} firing {firing}"
        );
        let (rounds, last) = fired.unwrap();
        assert!(rounds >= 10, "{rounds} rounds in a second");
        assert_eq!(last, 38, "the gun's last event");
        assert_eq!(yaw, Some(45.0), "the barrel past its arc");
        assert_eq!(released, (0, 0, 1023), "release");
        // `G_PlayerTurretPositionAndBlend`: retail's gunner at barrel yaw 0
        // and 45 (`play-1.7-saw.txt`, cod4-gsc.md 9.2).
        let retail = [
            (0.0, [-754.039, 75.398, 258.125]),
            (45.0, [-752.46, 51.005, 258.125]),
            (-45.0, [-738.634, 99.883, 258.125]),
        ];
        let mut seen = Vec::new();
        for (yaw, at) in placed {
            let Some((_, want)) = retail.iter().find(|(y, _)| (y - yaw).abs() < 0.02) else {
                continue;
            };
            let near = at.iter().zip(want).all(|(x, y)| (x - y).abs() < 0.02);
            assert!(near, "barrel {yaw}: placed {at:?}, retail {want:?}");
            seen.push(yaw.round());
        }
        assert!(seen.contains(&45.0) && seen.contains(&-45.0), "{seen:?}");
    }

    /// Another player on a gun, through a joined client: the server still
    /// sends his held weapon, and his drawn body leaves it out
    /// (`BG_UpdatePlayerDObj`, cod4-player-anims.md 5), so the gun is the
    /// only one in his hands. Off the gun the same body holds it.
    #[test]
    fn cod4_live_a_gunner_holds_no_weapon_of_his_own() {
        const GUN: u32 = 393;
        let sets = [
            ("scr_game_playerwaittime", "1"),
            ("scr_game_matchstarttime", "1"),
        ];
        let Some(r) = live_join_on("mp_backlot", None, &sets, |s, spawned, t0, ctl| {
            spawned.as_ref()?;
            // (sent weapon, drawn weapon) of each player body before and
            // on the gun.
            let mut off = None;
            let mut on = None;
            ctl.send(Ctl::MountBot([-770.0, 68.0, 320.0], GUN)).ok()?;
            let end = since(t0) + Duration::from_secs(20);
            while since(t0) < end && on.is_none() {
                s.step(since(t0).as_secs_f64() * 1000.0).unwrap();
                s.take_commands();
                let snap = s.client().snapshots.last().cloned();
                if let (Some(scene), Some(snap)) = (s.scene(), snap) {
                    for d in &scene.entities {
                        let Some(b) = d.body.as_ref().filter(|b| !b.corpse) else {
                            continue;
                        };
                        let Some(e) = snap.entities.iter().find(|e| e.number() == d.number) else {
                            continue;
                        };
                        let seen = (ent_u32(e, "weapon"), b.weapon.clone());
                        if b.gun.as_ref().is_some_and(|g| g.0 == GUN) {
                            on = Some(seen);
                        } else if ent_u32(e, "lerp.eFlags") & 0x300 == 0 && seen.0 != 0 {
                            off = Some(seen);
                        }
                    }
                }
                clk_sleep(Duration::from_millis(16));
            }
            Some((off, on))
        }) else {
            return;
        };
        let (off, on) = r.expect("never spawned");
        let (sent, drawn) = off.expect("no armed body off the gun");
        assert!(drawn.is_some(), "off the gun: weapon {sent} not drawn");
        let (sent, drawn) = on.expect("the bot never manned gun 393");
        assert_ne!(sent, 0, "the gunner's entity sends no weapon");
        assert_eq!(drawn, None, "the gunner draws his own weapon {sent}");
    }

    /// A live frag at our feet through a joined client (cod4-gsc.md 9.3):
    /// the hint names it with its fuse's time left, the frag key takes it
    /// (`EV_SWITCH_OFFHAND`, then `EV_AMMO_PICKUP`, the owner named, the
    /// hand's `grenadeTimeLeft` the fuse's), and the throw puts a new
    /// grenade in the world while the frag count stays.
    #[test]
    fn cod4_live_frag_at_the_feet_is_thrown_back() {
        use crate::play::input::Action;
        let sets = [
            ("scr_game_playerwaittime", "1"),
            ("scr_game_matchstarttime", "1"),
        ];
        let Some(r) = live_join_on("mp_crash", crash_world(), &sets, |s, spawned, t0, tx| {
            spawned.as_ref()?;
            let step = |s: &mut Cod4Session, ms: u64| {
                let end = since(t0) + Duration::from_millis(ms);
                while since(t0) < end {
                    s.step(since(t0).as_secs_f64() * 1000.0).unwrap();
                    s.take_commands();
                    clk_sleep(Duration::from_millis(16));
                }
            };
            let ps = |s: &Cod4Session, n: &str| {
                s.client()
                    .snapshots
                    .last()
                    .and_then(|f| f.ps.get(n))
                    .unwrap_or(0)
            };
            // Past the prematch freeze.
            step(s, 3000);
            let f = |n: &str| ps_f32(&s.client().snapshots.last().unwrap().ps, n);
            let at = [f("origin[0]") + 24.0, f("origin[1]"), f("origin[2]") + 2.0];
            let frags = |s: &Cod4Session| {
                s.client()
                    .snapshots
                    .last()
                    .unwrap()
                    .ps
                    .ammoclip
                    .iter()
                    .sum::<i32>()
            };
            let before = frags(s);
            s.view = [60.0, 0.0];
            tx.send(Ctl::DropFrag(at, 3000)).unwrap();
            step(s, 300);
            let hint = (ps(s, "cursorHint"), ps(s, "throwBackGrenadeTimeLeft"));
            let mut seen = Vec::new();
            let mut owner = 1023;
            let mut hand = 0;
            s.input.key(Action::Frag, true);
            for _ in 0..12 {
                step(s, 50);
                let p = &s.client().snapshots.last().unwrap().ps;
                for k in 0..4 {
                    seen.push(p.get(&format!("events[{k}]")).unwrap_or(0));
                }
                if owner == 1023 {
                    owner = ps(s, "throwBackGrenadeOwner");
                    hand = ps(s, "grenadeTimeLeft");
                }
            }
            s.input.key(Action::Frag, false);
            let mut thrown = false;
            for _ in 0..40 {
                step(s, 50);
                thrown |= s
                    .client()
                    .snapshots
                    .last()
                    .unwrap()
                    .entities
                    .iter()
                    .any(|e| e.etype() == ET_MISSILE);
            }
            Some((hint, seen, owner, hand, thrown, before, frags(s)))
        }) else {
            return;
        };
        let (hint, seen, owner, hand, thrown, before, after) = r.expect("never spawned");
        assert!(
            hint.0 > 4 && hint.1 > 2000 && hint.1 <= 3000,
            "hint {hint:?}"
        );
        assert!(seen.contains(&34) && seen.contains(&10), "events {seen:?}");
        assert_eq!(owner, 0, "the thrower named");
        assert!(hand > 0 && hand <= 3000, "the hand's fuse {hand}");
        assert!(thrown, "nothing thrown back");
        assert_eq!(before, after, "a throw back spends no frag");
    }

    /// A live frag at our feet through a joined client (cod4-hud.md 20):
    /// the server's hint pass puts it under `cursorHint`, and the HUD's
    /// `cursorhints` item draws `PLATFORM_THROWBACKGRENADE` with the frag
    /// key in place of its `&&1` and the frag's `hudIcon` to its right,
    /// over the screen's centre, the grenade indicator's throw back icon
    /// over the crosshair; once the blast has killed us no hint draws.
    #[test]
    fn cod4_live_frag_at_our_feet_draws_the_throw_back_hint() {
        const WIDE: (f32, f32) = (2560.0, 1440.0);
        let sets = [
            ("scr_game_playerwaittime", "1"),
            ("scr_game_matchstarttime", "1"),
        ];
        let Some(r) = live_join_on("mp_crash", crash_world(), &sets, |s, spawned, t0, tx| {
            spawned.as_ref()?;
            let install = vcod_cod4::testing::cod4_install()?;
            let mut hud = crate::cod4_hud::Cod4Hud::default();
            hud.load(&install.root().join("main"), "mp_crash");
            let mut shell = crate::console::shell::Shell::new();
            shell.execute("bind G +frag");
            let size = (1280.0, 960.0);
            let frame = |hud: &mut crate::cod4_hud::Cod4Hud, s: &mut Cod4Session| {
                s.step(since(t0).as_secs_f64() * 1000.0).unwrap();
                let fired = s.take_fired();
                let now = s.scene().map_or(0, |x| x.render_time);
                hud.update(s, &fired, now);
                clk_sleep(Duration::from_millis(16));
                now
            };
            let end = since(t0) + Duration::from_millis(3000);
            while since(t0) < end {
                frame(&mut hud, s);
            }
            let f = |n: &str| ps_f32(&s.client().snapshots.last().unwrap().ps, n);
            let at = [f("origin[0]") + 24.0, f("origin[1]"), f("origin[2]") + 2.0];
            s.view = [60.0, 0.0];
            tx.send(Ctl::DropFrag(at, 2000)).unwrap();
            let (mut drawn, mut quads, mut gone) = (None, 0, false);
            let mut wide = Vec::new();
            let mut indicators = Vec::new();
            let end = since(t0) + Duration::from_millis(3500);
            while since(t0) < end {
                let now = frame(&mut hud, s);
                let d = hud.cursor_hint_now(s, &shell, size, now);
                if let Some(sc) = s.scene() {
                    let g = hud.grenade_indicators(s, &sc.view, &|| f32::MAX, size, now);
                    if indicators.is_empty() {
                        indicators = g.into_iter().map(|q| (q.verts, q.texture)).collect();
                    }
                }
                gone = drawn.is_some() && d.is_none();
                if drawn.is_none() && d.is_some() {
                    quads = hud.build(s, size, now, None, &shell).len();
                    wide = hud.build(s, WIDE, now, None, &shell);
                    drawn = d;
                }
            }
            let strings = hud.strings()?;
            let frag_icon = hud.weapon_icon("frag_grenade_mp");
            let key = shell.key_text("+frag", &strings);
            let throwback = hud.texture_of("hud_grenadethrowback");
            let icon_tex = frag_icon.as_deref().and_then(|m| hud.texture_of(m));
            Some((
                drawn,
                quads,
                gone,
                strings,
                frag_icon,
                key,
                indicators,
                throwback,
                (wide, icon_tex),
            ))
        }) else {
            return;
        };
        let (drawn, quads, gone, strings, frag_icon, key, indicators, throwback, (wide, icon_tex)) =
            r.expect("never spawned");
        // At 1440p the hint's font is not the 640 grid's: the text still
        // ends where its icon starts, as retail measures it with the font
        // it draws (cod4-hud.md 20). The last glyph's image and shadow
        // reach about 3 px past its advance; the grid's width ran 5.5.
        let icon_tex = icon_tex.expect("the frag icon's image");
        let icon = wide
            .iter()
            .filter(|q| q.texture == icon_tex)
            // The hint's, near the middle (the weapon HUD shows the frag too).
            .min_by_key(|q| ((q.verts[0][0] + q.verts[1][0]) - WIDE.0).abs() as i32)
            .expect("the hint's icon at 1080p");
        let (top, bottom) = (icon.verts[0][1], icon.verts[2][1]);
        let text_end = wide
            .iter()
            .filter(|q| q.texture != icon_tex)
            .filter(|q| {
                let mid = (q.verts[0][1] + q.verts[2][1]) * 0.5;
                mid > top && mid < bottom && q.verts[1][0] < icon.verts[1][0]
            })
            .map(|q| q.verts[1][0])
            .fold(f32::MIN, f32::max);
        assert!(text_end > f32::MIN, "no hint text beside the icon");
        assert!(
            text_end <= icon.verts[0][0] + 4.0,
            "text ends at {text_end}, icon starts at {}",
            icon.verts[0][0]
        );
        // The grenade indicator (cod4-hud.md 21): the pointer, then the
        // throw back icon, the frag ahead of a view at yaw 0, so above the
        // centre of the 1280x960 window by `IconOffset` 50 grid units.
        let [(_, _), (icon, tex)] = &indicators[..] else {
            panic!("indicators {indicators:?}");
        };
        assert_eq!(Some(tex.clone()), throwback);
        let mid = |k: usize| icon.iter().map(|v| v[k]).sum::<f32>() / 4.0;
        assert!((mid(0) - 640.0).abs() < 2.0, "{icon:?}");
        assert!((mid(1) - (480.0 - 100.0)).abs() < 2.0, "{icon:?}");
        let (d, alpha) = drawn.expect("no hint drawn");
        assert_eq!(alpha, 1.0);
        let key = key.expect("+frag bound");
        let want = strings
            .get("PLATFORM_THROWBACKGRENADE")
            .expect("localized")
            .replacen("&&1", &key, 1);
        let [(text, x, y)] = &d.texts[..] else {
            panic!("{d:?}");
        };
        assert_eq!(text, &want);
        // Centred on the item's middle with the icon after the text, a
        // 40-unit square on the item's row.
        let (material, rect) = d.icon.clone().expect("an icon");
        assert_eq!(Some(material), frag_icon);
        assert!(
            (x + rect[2] * 0.5 + (rect[0] - x) * 0.5).abs() < 0.01,
            "{d:?}"
        );
        assert!(*x < 0.0 && rect[0] > *x && *y > 60.0, "{d:?}");
        assert_eq!((rect[1], rect[2], rect[3]), (40.0, 40.0, 40.0));
        assert!(quads > text.len() / 2, "quads {quads}");
        assert!(gone, "the hint outlived the frag");
    }

    /// One frame of a live kick test: the step, the scene (which steps
    /// the kicks), the fired events dropped; the scene.
    fn kick_frame(s: &mut Cod4Session, t0: Instant) -> Option<Scene> {
        s.step(since(t0).as_secs_f64() * 1000.0).unwrap();
        s.take_commands();
        let scene = s.scene();
        s.take_fired();
        clk_sleep(Duration::from_millis(16));
        scene
    }

    /// A trigger pull through a joined client on vcod-server
    /// (cod4-client-view.md 14): the M16's burst kicks the view
    /// (`CG_KickAngles`) and the kick rides the cmds' angles, so the
    /// server's view of each cmd is the client's drawn one, kick and all,
    /// and both come back to the mouse's view once the kick has centred.
    #[test]
    fn cod4_live_recoil_rides_the_cmd_and_the_server_takes_it() {
        use crate::play::input::Action;
        let sets = [
            ("scr_game_playerwaittime", "1"),
            ("scr_game_matchstarttime", "1"),
        ];
        let Some(r) = live_join_on("mp_crash", crash_world(), &sets, |s, spawned, t0, tx| {
            spawned.as_ref()?;
            let end = since(t0) + Duration::from_millis(3000);
            while since(t0) < end {
                kick_frame(s, t0);
            }
            s.view = [0.0, 0.0];
            // How far a view (pitch, yaw) is off the mouse's 0, 0.
            let off = |a: [f32; 3]| {
                use vcod_common::pmove::angle_normalize180;
                angle_normalize180(a[0])
                    .abs()
                    .max(angle_normalize180(a[1]).abs())
            };
            let (ptx, prx) = std::sync::mpsc::channel();
            let (mut kick, mut drawn, mut server) = (0.0f32, 0.0f32, 0.0f32);
            let (mut checked, mut worst) = (0, 0.0f32);
            let start = since(t0);
            while since(t0) < start + Duration::from_millis(2500) {
                let held = since(t0) - start < Duration::from_millis(60);
                s.input.key(Action::Attack, held);
                let scene = kick_frame(s, t0)?;
                kick = kick.max(off(s.kick.view_kick()));
                drawn = drawn.max(off(scene.angles.to_array()));
                tx.send(Ctl::Probe(ptx.clone())).unwrap();
                let ps = &s.client().snapshots.last()?.ps;
                let delta: [u16; 2] =
                    std::array::from_fn(|k| angle2short(ps_f32(ps, &format!("delta_angles[{k}]"))));
                while let Ok((t, va)) = prx.try_recv() {
                    server = server.max(off(va));
                    let Some(c) = s.sent_cmds.iter().find(|c| c.server_time == t) else {
                        continue;
                    };
                    for k in 0..2 {
                        let mine = f32::from(c.angles[k].wrapping_add(delta[k]) as i16)
                            * (360.0 / 65536.0);
                        let miss = vcod_common::pmove::angle_normalize180(mine - va[k]);
                        worst = worst.max(miss.abs());
                    }
                    checked += 1;
                }
            }
            let rest = off(kick_frame(s, t0)?.angles.to_array());
            Some((
                kick,
                drawn,
                server,
                checked,
                worst,
                s.kick.view_kick(),
                rest,
            ))
        }) else {
            return;
        };
        let (kick, drawn, server, checked, worst, left, rest) = r.expect("never spawned");
        assert!(kick > 0.1, "kick {kick}");
        assert!(drawn > 0.1 && server > 0.1, "drawn {drawn} server {server}");
        assert!(
            checked > 20 && worst < 0.01,
            "{checked} cmds, worst {worst}"
        );
        assert_eq!(left, [0.0; 3]);
        assert!(rest < 0.01, "the view {rest} off the mouse's");
    }

    /// A standing jump through a joined client: the landing's
    /// `EV_LANDING_*` dips the eye by its parm over 150 ms and brings it
    /// back by 450, and the view weapon's `ViewPs` carries the dip.
    #[test]
    fn cod4_live_landing_dips_the_eye_and_the_gun() {
        use crate::play::input::Action;
        let sets = [
            ("scr_game_playerwaittime", "1"),
            ("scr_game_matchstarttime", "1"),
        ];
        let Some(r) = live_join_on("mp_crash", crash_world(), &sets, |s, spawned, t0, _| {
            spawned.as_ref()?;
            let end = since(t0) + Duration::from_millis(3000);
            let mut rest = 0.0;
            while since(t0) < end {
                if let Some(sc) = kick_frame(s, t0) {
                    rest = sc.eye.z;
                }
            }
            let (mut change, mut low, mut gun) = (0.0f32, rest, (0.0, 0));
            let start = since(t0);
            let end = start + Duration::from_millis(2000);
            let mut back = 0.0;
            while since(t0) < end {
                s.input
                    .key(Action::Jump, since(t0) - start < Duration::from_millis(50));
                let sc = kick_frame(s, t0)?;
                let (c, t) = sc.view.land;
                if c < 0.0 && (0..450).contains(&t) {
                    change = c;
                    low = low.min(sc.eye.z);
                    gun = sc.view.land;
                }
                back = sc.eye.z;
            }
            Some((rest, change, low, gun, back))
        }) else {
            return;
        };
        let (rest, change, low, gun, back) = r.expect("never spawned");
        assert!(change <= -1.0, "landChange {change}");
        assert!(
            low < rest + change * 0.5,
            "eye {low} from {rest}, change {change}"
        );
        assert!(
            crate::play::cod4_kick::gun_dip(gun) < 0.0 || gun.1 >= 150,
            "{gun:?}"
        );
        assert!((back - rest).abs() < 0.01, "eye {back} back from {rest}");
    }

    /// A live frag 240 units off our side through a joined client
    /// (cod4-hud.md 19): its blast leaves us under `pulseStart`, so the
    /// `low health` menu's overlay pulses in over the whole screen, and a
    /// `hit_direction` icon takes the blast's yaw, drawn on the frag's side
    /// of the centre for its 2 s and gone after. The stock
    /// `shellshockOnDamage` shocks us with `frag_grenade_mp` for 0.5 s, a
    /// blurred screen blend, under which 1.7 draws no icon (0x42c6f3).
    #[test]
    fn cod4_live_blast_pulses_the_overlay_and_points_an_icon_at_it() {
        let sets = [
            ("scr_game_playerwaittime", "1"),
            ("scr_game_matchstarttime", "1"),
        ];
        let Some(r) = live_join_on("mp_crash", crash_world(), &sets, |s, spawned, t0, tx| {
            spawned.as_ref()?;
            let install = vcod_cod4::testing::cod4_install()?;
            let mut hud = crate::cod4_hud::Cod4Hud::default();
            hud.load(&install.root().join("main"), "mp_crash");
            let shell = crate::console::shell::Shell::new();
            let size = (1280.0, 960.0);
            let frame = |hud: &mut crate::cod4_hud::Cod4Hud, s: &mut Cod4Session| {
                s.step(since(t0).as_secs_f64() * 1000.0).unwrap();
                let fired = s.take_fired();
                let now = s.scene().map_or(0, |x| x.render_time);
                hud.update(s, &fired, now);
                clk_sleep(Duration::from_millis(16));
                now
            };
            // Past the stock spawn protection: a grenade within 250 of
            // the spawn point hurts nobody for 3.5 s from the spawn
            // (`Callback_PlayerDamage`, `lastSpawnTime + 3500`), and the
            // spawn's snapshot is no earlier than `lastSpawnTime`.
            let spawned_at = s.client().snapshots.last()?.server_time;
            let level = |s: &Cod4Session| s.client().snapshots.last().map_or(0, |x| x.server_time);
            let end = since(t0) + Duration::from_millis(3000);
            while since(t0) < end || level(s) < spawned_at + 3600 {
                frame(&mut hud, s);
            }
            // The frag 240 units off along the first yaw with open,
            // level floor that far (the spawn's own surroundings decide no
            // more whether the blast reaches us), and the view a quarter
            // turn right of it, so the blast comes from the left.
            let f = |n: &str| ps_f32(&s.client().snapshots.last().unwrap().ps, n);
            let feet = Vec3::new(f("origin[0]"), f("origin[1]"), f("origin[2]"));
            let clip = &s.world()?.clip;
            let z = Vec3::ZERO;
            let side = (0..24).map(|k| k as f32 * 15.0).find(|y| {
                let d = Vec3::new(y.to_radians().cos(), y.to_radians().sin(), 0.0);
                let at = feet + d * 240.0 + Vec3::Z * 8.0;
                let open = [8.0, 60.0].iter().all(|h| {
                    let from = feet + Vec3::Z * *h;
                    let mut t = clip.trace(from, at, z, z, 0x280_6891);
                    clip.trace_static_models(from, at, 0x280_6891, &mut t);
                    t.fraction == 1.0 && !t.startsolid
                });
                let down = clip.trace(at, at - Vec3::Z * 16.0, z, z, 0x280_6891);
                open && down.fraction < 1.0 && down.normal.z > 0.9
            })?;
            let d = Vec3::new(side.to_radians().cos(), side.to_radians().sin(), 0.0);
            let at = (feet + d * 240.0 + Vec3::Z * 8.0).to_array();
            s.view = [0.0, side - 90.0];
            tx.send(Ctl::DropFrag(at, 50)).unwrap();
            let (mut alpha, mut overlay, mut health) = (0.0f32, false, 100);
            let (mut icons, mut gone) = (Vec::new(), false);
            // A frame with a live slot under the shock's blend, and whether
            // any such frame drew an icon.
            let mut shocked = (false, false);
            // The view's damage kick (cod4-client-view.md 14): its peak
            // and its last value.
            let (mut kick_peak, mut kick_last) = ([0.0f32; 3], [0.0f32; 3]);
            let end = since(t0) + Duration::from_millis(3500);
            while since(t0) < end {
                let now = frame(&mut hud, s);
                let dk = hud.damage_kick();
                kick_last = dk.view(now - dk.time, 0.0, false);
                if kick_last[2].abs() > kick_peak[2].abs() {
                    kick_peak = kick_last;
                }
                let Some(scene) = s.scene() else { continue };
                health = health.min(s.client().snapshots.last().unwrap().ps.stats[0]);
                let a = hud.low_health_alpha(now);
                if a > alpha {
                    alpha = a;
                    // A quad over the whole window at the overlay's alpha.
                    overlay = hud.build(s, size, now, None, &shell).iter().any(|q| {
                        q.rgba[3] == a
                            && q.verts[0][0] <= 0.0
                            && q.verts[2][0] >= size.0
                            && q.verts[2][1] >= size.1
                    });
                }
                let q = hud.damage_icons(s, &scene.view, size, now, scene.angles.y);
                let (p, start, ms) = crate::play::cod4_shock::of_session(s);
                let flashed = crate::play::cod4_shock::flashbanged(p.as_ref(), start, ms, now);
                let live = hud
                    .damage_slots()
                    .iter()
                    .any(|i| i.ms != 0 && now - i.start > 0 && now - i.start < i.ms);
                if flashed && live {
                    shocked.0 = true;
                    shocked.1 |= !q.is_empty();
                }
                if !q.is_empty() && icons.is_empty() {
                    icons = hud
                        .damage_slots()
                        .iter()
                        .filter(|i| i.ms != 0)
                        .map(|i| (i.yaw, q.len(), q[0].verts))
                        .collect();
                }
                gone = !icons.is_empty() && q.is_empty();
            }
            Some((
                health, alpha, overlay, icons, gone, shocked, kick_peak, kick_last, side,
            ))
        }) else {
            return;
        };
        let (health, alpha, overlay, icons, gone, shocked, kick_peak, kick_last, side) =
            r.expect("never spawned");
        assert_eq!(
            shocked,
            (true, false),
            "(hit under the shock, icon drawn then)"
        );
        // A blast from the left, `damageCount` * 0.2 (at least 5): the
        // view rolls right, barely pitches, and is level again by 500 ms.
        assert!(kick_peak[2] >= 4.0, "kick {kick_peak:?}");
        assert!(kick_peak[0].abs() < 1.5, "kick {kick_peak:?}");
        assert_eq!(kick_last, [0.0; 3]);
        assert!(health > 0 && health < 55, "health {health}");
        assert_eq!(alpha, 1.0, "the first pulse's full alpha");
        assert!(overlay, "no overlay quad");
        let [(yaw, n, verts)] = icons[..] else {
            panic!("icons {icons:?}");
        };
        assert_eq!(n, 1);
        // The frag lay at yaw `side` from us, so the blast came along the
        // opposite way (`damageYaw` is the push's way), 10 degrees of
        // jitter either way.
        let off = (yaw - (side + 180.0)).rem_euclid(360.0);
        assert!(
            off.min(360.0 - off) <= 10.0,
            "icon yaw {yaw}, frag at {side}"
        );
        // Facing a quarter turn right of it, the icon turns from below
        // the centre to its left,
        // the frag's side, `cg_hudDamageIconOffset` 128 grid units out.
        let xs = verts.map(|v| v[0]);
        assert!(
            xs.iter().all(|&x| x < 640.0 - 128.0 * 2.0 + 30.0),
            "{verts:?}"
        );
        assert!(gone, "the icon outlived its 2 s");
    }

    /// The stock first-spawn splash on vcod-server's `war`
    /// (`_hud_message.gsc`'s `notifyTitle`, `glowColor` (0.3, 0.6, 0.3),
    /// `glowAlpha` 1) draws with its glow: four additive quads a glyph in
    /// the glow's colour times 0.1 (cod4-hud.md 22). Once the compass
    /// shows, its tickertape slides with the view: facing south its S runs 0.25 to
    /// 0.75, and an eighth of a turn left moves it an eighth down.
    #[test]
    fn cod4_live_splash_glows_and_the_ticker_turns() {
        // The prematch holds the compass back; a short one lets it show.
        let sets = [
            ("scr_game_playerwaittime", "1"),
            ("scr_game_matchstarttime", "1"),
        ];
        let Some(r) = live_join_on("mp_crash", crash_world(), &sets, |s, spawned, t0, _| {
            spawned.as_ref()?;
            let install = vcod_cod4::testing::cod4_install()?;
            let mut hud = crate::cod4_hud::Cod4Hud::default();
            hud.load(&install.root().join("main"), "mp_crash");
            let shell = crate::console::shell::Shell::new();
            let size = (1280.0, 960.0);
            // The glowing text's colour and when its first letter is out
            // (`fx_birth_time` plus a letter's time): the newest snapshot
            // runs ahead of the drawn time, so the elem may be seen a
            // frame or more before its typewriter shows a letter.
            let (mut glow, mut quads) = (None::<([u8; 4], i32)>, Vec::new());
            let end = since(t0) + Duration::from_millis(6000);
            while since(t0) < end && quads.is_empty() {
                s.step(since(t0).as_secs_f64() * 1000.0).unwrap();
                let fired = s.take_fired();
                let now = s.scene().map_or(0, |x| x.render_time);
                hud.update(s, &fired, now);
                if let Some((_, shown)) = glow
                    && now >= shown
                {
                    quads = hud.build(s, size, now, None, &shell);
                }
                clk_sleep(Duration::from_millis(16));
                let snap = s.client().snapshots.last().unwrap();
                glow = glow.or_else(|| {
                    vcod_cod4::hud::hud_elems(&snap.ps)
                        .iter()
                        .find(|e| e.glow_color[3] != 0 && e.kind == vcod_cod4::hud::kind::TEXT)
                        .map(|e| (e.glow_color, e.fx_birth_time + e.fx_letter_time))
                });
            }
            let glow = glow.map(|g| g.0);
            // The compass shows once the spawn's splash has gone.
            let eye = glam::Vec3::ZERO;
            let mut now = 0;
            let end = since(t0) + Duration::from_millis(30000);
            while since(t0) < end {
                s.step(since(t0).as_secs_f64() * 1000.0).unwrap();
                let fired = s.take_fired();
                now = s.scene().map_or(0, |x| x.render_time);
                hud.update(s, &fired, now);
                clk_sleep(Duration::from_millis(16));
                let q = hud.build(s, size, now, Some((eye, 0.0)), &shell);
                if q.iter().any(|q| q.texture.contains("tickertape")) {
                    break;
                }
            }
            let tape = |yaw: f32| {
                hud.build(s, size, now, Some((eye, yaw)), &shell)
                    .into_iter()
                    .filter(|q| {
                        q.texture.contains("tickertape") && !q.texture.contains("background")
                    })
                    .map(|q| q.uvs[0][0])
                    .collect::<Vec<_>>()
            };
            let north = s.gamestate().map_or(0.0, |g| {
                String::from_utf8_lossy(g.configstring(vcod_cod4::hud::CS_NORTH_YAW))
                    .trim()
                    .parse::<f32>()
                    .unwrap_or(0.0)
            });
            Some((glow, quads, tape(north - 180.0), tape(north - 135.0)))
        }) else {
            return;
        };
        let (glow, quads, at0, at90) = r.expect("never spawned");
        let glow = glow
            .expect("no glowing hudelem")
            .map(|c| f32::from(c) / 255.0);
        let add: Vec<_> = quads
            .iter()
            .filter(|q| q.texture.starts_with(crate::hud::ADDITIVE))
            .collect();
        assert!(
            !add.is_empty() && add.len() % 4 == 0,
            "{} glow quads",
            add.len()
        );
        for q in &add {
            for (got, g) in q.rgba.iter().zip(glow).take(3) {
                assert!((got - g * 0.1).abs() < 1e-6, "{:?}", q.rgba);
            }
        }
        // One span each; the left edge's S is the centre less 0.25, the
        // centre `-(yaw - north) / 360`: 0.5 facing south, 0.375 an eighth
        // left of it.
        let (&[a], &[b]) = (&at0[..], &at90[..]) else {
            panic!("tape {at0:?} {at90:?}");
        };
        assert!(
            (a - 0.25).abs() < 1e-4 && (b - 0.125).abs() < 1e-4,
            "{a} {b}"
        );
    }

    /// A spectator's newest playerstate as the spectate tests read it:
    /// `otherFlags`, `clientNum`, `pm_type` and the origin.
    fn spec_ps(s: &Cod4Session) -> Option<(u32, u32, u32, [f32; 3])> {
        let ps = &s.client().snapshots.last()?.ps;
        let u = |n: &str| ps.get(n).unwrap_or(0);
        let o = [0, 1, 2].map(|i| ps_f32(ps, &format!("origin[{i}]")));
        Some((u("otherFlags"), u("clientNum"), u("pm_type"), o))
    }

    /// vcod-server's `gametype` on mp_crash with `sets` and one bot, which
    /// we join through the team menu's Spectator (then `assault_mp,0`,
    /// should a team pick follow); `each` runs every frame with our
    /// session, the bot's slot and the ms since the join until it returns
    /// `Some` or 30 s pass. `None` without an install.
    fn live_spectate<T: Send + 'static>(
        gametype: &str,
        sets: &[(&str, &str)],
        mut each: impl FnMut(&mut Cod4Session, usize, f64) -> Option<T> + Send + 'static,
    ) -> Option<Option<T>> {
        use vcod_server::cod4::{Cod4Config, Cod4Server};
        let install = vcod_cod4::testing::cod4_install()?;
        let world = crash_world();
        let mut sv = Cod4Server::new(
            Cod4Config {
                map: "mp_crash".into(),
                gametype: gametype.into(),
                hostname: "vcod".into(),
                max_clients: 4,
            },
            7,
        );
        sv.set_cvar("scr_game_playerwaittime", "1");
        sv.set_cvar("scr_game_matchstarttime", "1");
        for (k, v) in sets {
            sv.set_cvar(k, v);
        }
        sv.load(Some(&install)).unwrap();
        let bot = sv.add_bots(1, false)[0];
        let sock = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
        sock.set_nonblocking(true).unwrap();
        let addr = sock.local_addr().unwrap();
        let clock = sim::Clock::new();
        let client = sim::spawn(&clock, move || {
            let join = MenuJoin {
                team: Some("spectator".into()),
                class: Some("assault_mp,0".into()),
            };
            let mut s = Cod4Session::connect(addr, "watcher", Cp::Bare, join).unwrap();
            if let Some(w) = world {
                s.set_world(w);
            }
            let t0 = clk_now();
            let mut out = None;
            while since(t0) < Duration::from_secs(30) && out.is_none() {
                let now = since(t0).as_secs_f64() * 1000.0;
                s.step(now).unwrap();
                s.take_commands();
                out = each(&mut s, bot, now);
                clk_sleep(Duration::from_millis(16));
            }
            s.disconnect();
            out
        });
        serve(&mut sv, &sock, &clock, &client, |_, _, _| true);
        Some(client.join().unwrap())
    }

    /// At the stock `scr_game_spectatetype` 1 on `war` the team menu's
    /// spectator watches no one (`setSpectatePermissions` refuses every
    /// team to the spectator team) and stands still holding forward
    /// (`ps.speed` 0, cod4-pmove.md 15); attack follows no one. Then the
    /// team menu again takes us onto allies and a spawn.
    #[test]
    fn cod4_live_spectator_at_1_holds_still_then_joins_a_team() {
        use crate::play::input::Action;
        let mut first = None;
        let mut asked = false;
        let Some(r) = live_spectate("war", &[], move |s, _, now| {
            let (of, num, pm, o) = spec_ps(s)?;
            if s.spawn().is_some() {
                return Some(Ok(first));
            }
            if pm != 4 || num != s.gamestate()?.client_num as u32 {
                return None;
            }
            let at = *first.get_or_insert((now, o));
            s.input.key(Action::Forward, true);
            s.input
                .key(Action::Attack, (now - at.0) as i32 / 300 % 2 == 1);
            if of & 2 != 0 {
                return Some(Err(format!("followed {num}")));
            }
            if now - at.0 > 2500.0 && !asked {
                let moved = (0..3).map(|i| (o[i] - at.1[i]).abs()).fold(0.0, f32::max);
                if moved > 0.01 {
                    return Some(Err(format!("moved {moved}")));
                }
                s.input.release_all();
                s.open_main_menu();
                let gs = s.gamestate()?.clone();
                let i = s
                    .menus
                    .open()?
                    .responses
                    .iter()
                    .position(|r| r == "allies")?;
                let c = s.menus.choose(i, &gs)?;
                s.command(&c);
                asked = true;
            }
            None
        }) else {
            return;
        };
        let r = r.expect("no spawn after the team pick");
        assert!(r.unwrap().is_some(), "never a spectator");
    }

    /// `dm` at `scr_game_spectatetype` 1 lets a spectator watch any team
    /// but not fly, so the end frame follows the first player playing
    /// (`Cmd_FollowCycle_f(ent, 1)` from `SpectatorClientEndFrame`): the
    /// bot, its playerstate ours with the follow bit.
    #[test]
    fn cod4_live_spectator_at_1_follows_the_bot_on_dm() {
        let Some(r) = live_spectate("dm", &[], |s, bot, _| {
            let (of, num, _, _) = spec_ps(s)?;
            (of & 2 != 0 && num == bot as u32).then_some(())
        }) else {
            return;
        };
        assert!(r.is_some(), "never followed the bot");
    }

    /// At `scr_game_spectatetype` 2 a spectator flies on its keys; attack
    /// follows the bot (`SpectatorThink`'s `Cmd_FollowCycle_f`), melee
    /// lets go (`StopFollowing`), and it flies on from there.
    #[test]
    fn cod4_live_spectator_at_2_flies_follows_and_lets_go() {
        use crate::play::input::Action;
        #[derive(Debug, PartialEq)]
        enum Step {
            Fly,
            Follow,
            Release,
            Again(f64),
        }
        let mut step = Step::Fly;
        let mut from = None;
        let mut log = Vec::new();
        let Some(r) = live_spectate(
            "war",
            &[("scr_game_spectatetype", "2")],
            move |s, bot, now| {
                let (of, num, pm, o) = spec_ps(s)?;
                let me = s.gamestate()?.client_num as u32;
                let i = &mut s.input;
                match step {
                    Step::Fly if pm == 4 && num == me => {
                        let o0 = *from.get_or_insert(o);
                        i.key(Action::Forward, true);
                        let d = ((o[0] - o0[0]).powi(2) + (o[1] - o0[1]).powi(2)).sqrt();
                        if d > 100.0 {
                            log.push(format!("flew {d:.0} at {now:.0}"));
                            i.key(Action::Forward, false);
                            i.key(Action::Attack, true);
                            step = Step::Follow;
                        }
                    }
                    Step::Follow if of & 2 != 0 && num == bot as u32 => {
                        log.push(format!("followed {num} at {now:.0}"));
                        i.key(Action::Attack, false);
                        i.key(Action::Melee, true);
                        step = Step::Release;
                    }
                    Step::Release if of & 2 == 0 && num == me => {
                        log.push(format!("let go at {now:.0}"));
                        i.key(Action::Melee, false);
                        from = Some(o);
                        step = Step::Again(now);
                    }
                    Step::Again(at) => {
                        // Let go facing a wall the bot stood at, forward
                        // goes nowhere: back away after a second.
                        let back = now - at > 1000.0;
                        i.key(Action::Forward, !back);
                        i.key(Action::Back, back);
                        let o0 = from.unwrap();
                        let d = ((o[0] - o0[0]).powi(2) + (o[1] - o0[1]).powi(2)).sqrt();
                        if d > 100.0 {
                            return Some(log.clone());
                        }
                    }
                    _ => {}
                }
                None
            },
        ) else {
            return;
        };
        let log = r.expect("the fly, follow, let go and fly again did not all happen");
        eprintln!("{log:?}");
    }

    /// What one frame of [`live_war_pair`] hands its test.
    struct PairFrame<'a> {
        /// Us, on allies, the HUD's session.
        s: &'a mut Cod4Session,
        /// The enemy, on axis.
        e: &'a mut Cod4Session,
        hud: &'a mut crate::cod4_hud::Cod4Hud,
        quads: &'a [crate::hud::HudQuad],
        /// Our session's events this frame, as the HUD took them.
        fired: &'a [Fired],
        /// Our drawn playerstate, as the crosshair reads it, the eye and
        /// its angles.
        view: &'a crate::cod4_viewmodel::ViewPs,
        eye: Vec3,
        angles: Vec3,
        shell: &'a crate::console::shell::Shell,
        /// Both have spawned, and our newest snapshot's time.
        both: bool,
        level: i32,
        /// The `playLocalSound` / `stopLocalSound` aliases the HUD took
        /// this frame.
        local_sounds: &'a [(String, bool)],
        /// A script on a slot's player, run between the frame's packets
        /// and its tick.
        tx: &'a std::sync::mpsc::Sender<(usize, &'static str, &'static str, &'static str)>,
    }

    /// Two clients on vcod-server's war (us on allies, an enemy on axis)
    /// with the prematch cut to a second, our HUD loaded and built every
    /// frame, `each` run on every frame until it returns `Some` or 120 s
    /// pass. `None` without an install.
    fn live_war_pair<T: Send + 'static>(
        each: impl FnMut(PairFrame) -> Option<T> + Send + 'static,
    ) -> Option<Option<T>> {
        live_war_pair_as(["assault_mp,0", "assault_mp,0"], each)
    }

    /// [`live_war_pair`] with our class and the enemy's.
    fn live_war_pair_as<T: Send + 'static>(
        classes: [&'static str; 2],
        each: impl FnMut(PairFrame) -> Option<T> + Send + 'static,
    ) -> Option<Option<T>> {
        live_war_pair_in(classes, None, each)
    }

    /// [`live_war_pair_as`] with the loose scripts of `gsc` read over the
    /// zones' (`Cod4Server::set_script_overrides`).
    fn live_war_pair_in<T: Send + 'static>(
        classes: [&'static str; 2],
        gsc: Option<&'static str>,
        each: impl FnMut(PairFrame) -> Option<T> + Send + 'static,
    ) -> Option<Option<T>> {
        live_war_pair_on(["allies", "axis"], classes, gsc, each)
    }

    /// [`live_war_pair_in`] with the two on `teams` (ours first).
    fn live_war_pair_on<T: Send + 'static>(
        teams: [&'static str; 2],
        classes: [&'static str; 2],
        gsc: Option<&'static str>,
        mut each: impl FnMut(PairFrame) -> Option<T> + Send + 'static,
    ) -> Option<Option<T>> {
        use vcod_server::cod4::{Cod4Config, Cod4Server};
        let install = vcod_cod4::testing::cod4_install()?;
        let world = crash_world();
        let mut sv = Cod4Server::new(
            Cod4Config {
                map: "mp_crash".into(),
                gametype: "war".into(),
                hostname: "vcod".into(),
                max_clients: 4,
            },
            7,
        );
        sv.set_cvar("scr_game_playerwaittime", "1");
        sv.set_cvar("scr_game_matchstarttime", "1");
        if let Some(dir) = gsc {
            sv.set_script_overrides(dir.into());
        }
        sv.load(Some(&install)).unwrap();
        let sock = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
        sock.set_nonblocking(true).unwrap();
        let addr = sock.local_addr().unwrap();
        let (tx, rx) =
            std::sync::mpsc::channel::<(usize, &'static str, &'static str, &'static str)>();
        let main = install.root().join("main");
        let clock = sim::Clock::new();
        let client = sim::spawn(&clock, move || {
            let join = |team: &str, class: &str| MenuJoin {
                team: Some(team.into()),
                class: Some(class.into()),
            };
            let mut s =
                Cod4Session::connect(addr, "joiner", Cp::Bare, join(teams[0], classes[0])).unwrap();
            let mut e =
                Cod4Session::connect(addr, "enemy", Cp::Bare, join(teams[1], classes[1])).unwrap();
            if let Some(w) = world {
                s.set_world(w);
            }
            let mut hud = crate::cod4_hud::Cod4Hud::default();
            hud.load(&main, "mp_crash");
            // 1.7's `default_mp_controls.cfg` binds, as a client with no
            // config of its own starts.
            let mut shell = crate::console::shell::Shell::new();
            shell.use_cod4_binds();
            let t0 = clk_now();
            let mut out = None;
            while since(t0) < Duration::from_secs(120) && out.is_none() {
                let now = since(t0).as_secs_f64() * 1000.0;
                for c in [&mut s, &mut e] {
                    c.step(now).unwrap();
                }
                e.take_commands();
                e.take_fired();
                let fired = s.take_fired();
                let Some(scene) = s.scene() else {
                    clk_sleep(Duration::from_millis(16));
                    continue;
                };
                hud.update(&mut s, &fired, scene.server_time);
                let quads = hud.build(
                    &s,
                    (1280.0, 960.0),
                    scene.render_time,
                    Some((scene.eye, scene.angles.y)),
                    &shell,
                );
                let both = s.spawn().is_some() && e.spawn().is_some();
                let level = s.client().snapshots.last().map_or(0, |x| x.server_time);
                let local_sounds = hud.take_local_sounds();
                out = each(PairFrame {
                    s: &mut s,
                    e: &mut e,
                    hud: &mut hud,
                    quads: &quads,
                    fired: &fired,
                    view: &scene.view,
                    eye: scene.eye,
                    angles: scene.angles,
                    shell: &shell,
                    both,
                    level,
                    local_sounds: &local_sounds,
                    tx: &tx,
                });
                clk_sleep(Duration::from_millis(16));
            }
            s.disconnect();
            e.disconnect();
            out
        });
        serve(&mut sv, &sock, &clock, &client, |sv, _, _| {
            // A script call between the packets' cmds and the frame,
            // where a frame's own threads run; with no path, `srand`
            // reseeds the scripts' draws with the argument.
            if let Ok((slot, path, name, arg)) = rx.try_recv() {
                let l = sv.game.level.as_mut().unwrap();
                match (path, name) {
                    ("", "srand") => l.reseed_script_rand(arg.parse().unwrap()),
                    _ => l.start_player_script(slot, path, name, &[arg]).unwrap(),
                }
            }
            true
        });
        Some(client.join().unwrap())
    }

    /// [`live_war_pair`]: the stock `giveHardpointItem("airstrike_mp")`,
    /// its action slot pressed, and the stock `useAirstrike`'s
    /// `beginLocationSelection` sets `ps.locationSelectionInfo`: the HUD
    /// draws `fullscreenmap` (the minimap, its border, the selector's
    /// material under the cursor), the mouse moves the cursor and not the
    /// view, and the `+attack` press's 0x10000 confirms it, which ends the
    /// selection (cod4-hud.md 15.8).
    /// [`live_war_pair`]: the assault class's M16 grenade launcher, its
    /// action slot pressed and its trigger pulled, puts a `gl_m16_mp`
    /// grenade in our snapshots on a gravity arc along the view, which
    /// bursts (`EV_GRENADE_EXPLODE`) past its 375-unit arming distance. A
    /// grenade that wobbled like a rocket hung 120 ahead of the eye and
    /// never burst (cod4-combat.md 10.3).
    #[test]
    fn cod4_live_grenade_launcher_fires_along_the_view() {
        use crate::play::input::Action;
        #[derive(Default, Debug)]
        struct Seen {
            pressed_at: Option<i32>,
            fired_at: Option<i32>,
            released: bool,
            forward: Vec3,
            eye: Vec3,
            /// The first state: `trType`, `trBase`, `trDelta`.
            launch: Option<(u32, Vec3, Vec3)>,
            /// Where the burst's event rode.
            burst: Option<Vec3>,
            /// The yaw the shot goes along.
            yaw: f32,
            /// The snapshots read so far.
            read: usize,
            /// Each grenade state's server time, `trType`, `trBase` and
            /// events, for a failure's message.
            states: Vec<(i32, u32, Vec3, [u32; 4])>,
        }
        let mut seen = Seen::default();
        let Some(out) = live_war_pair(move |f| {
            let snap = f.s.client().snapshots.last().cloned()?;
            let u = |n: &str| snap.ps.get(n).unwrap_or(0);
            if !f.both || f.level < 9000 {
                return None;
            }
            if seen.pressed_at.is_none() {
                // Level along a yaw with 600 units open under the grenade's
                // mask (`G_InitGrenadeEntity`'s 0x2806891, missile clip
                // too), the static models and the solid entities, so it
                // arms before it meets anything: one that met something
                // inside its 375 went dud, whichever way the spawn faced.
                let clip = &crash_world()?.clip;
                // The server's eye: the scene's may still be the spawn's
                // camera move.
                let eye = vec3(|k| ps_f32(&snap.ps, &format!("origin[{k}]"))) + Vec3::Z * 60.0;
                let yaw = (0..24).map(|k| k as f32 * 15.0).find(|y| {
                    let d = Vec3::new(y.to_radians().cos(), y.to_radians().sin(), 0.0);
                    let (e, end) = (Vec3::splat(4.0), eye + d * 600.0);
                    let mut t = clip.trace(eye, end, -e, e, 0x280_6891);
                    clip.trace_static_models(eye, end, 0x280_6891, &mut t);
                    let solids = crate::play::cod4_predict::Solids {
                        clip,
                        solids: &f.s.shot_solids,
                    };
                    let me = f.s.client().gamestate.as_ref().map_or(0, |g| g.client_num);
                    let ents = vcod_cod4::pmove::PmWorld::trace(
                        &solids, eye, end, -e, e, me as u32, 0x280_6891,
                    );
                    t.fraction == 1.0 && !t.startsolid && ents.fraction == 1.0
                })?;
                seen.yaw = yaw;
                f.s.view = [0.0, yaw];
                let slot = (0..4).find(|k| u(&format!("actionSlotType[{k}]")) == 2)?;
                if f.s.action_slot(slot + 1) {
                    seen.pressed_at = Some(f.level);
                }
                return None;
            }
            let Some(fired) = seen.fired_at else {
                let weapon = u("weapon") as usize;
                let on_alt =
                    f.s.level_defs
                        .as_ref()
                        .is_some_and(|(_, d)| d.get(weapon).is_some_and(|d| d.name == "gl_m16_mp"));
                // The view on the open yaw first.
                let turned = (f.angles.y - seen.yaw)
                    .rem_euclid(360.0)
                    .min((seen.yaw - f.angles.y).rem_euclid(360.0))
                    < 0.5;
                if on_alt && u("weaponstate") == 0 && turned {
                    f.s.input.key(Action::Attack, true);
                    seen.fired_at = Some(f.level);
                    seen.eye = f.eye;
                    seen.forward = vcod_common::x87::angle_vectors(f.angles)[0];
                }
                assert!(
                    f.level - seen.pressed_at.unwrap() < 5000,
                    "never on the launcher"
                );
                return None;
            };
            // Held until the grenade shows: a loaded machine may run few
            // frames in 200 ms.
            if !seen.released && seen.launch.is_some() {
                f.s.input.key(Action::Attack, false);
                seen.released = true;
            }
            // Every snapshot since the last frame, not the newest alone: the
            // burst rides its entity 300 ms, and one frame may read more.
            let snaps = &f.s.client().snapshots;
            let from = seen.read.max(snaps.len().saturating_sub(64));
            seen.read = snaps.len();
            for snap in &snaps[from..] {
                for e in snap.entities.iter().filter(|e| e.etype() == 4) {
                    let v = |n: &str| vec3(|k| ent_f32(e, &format!("lerp.pos.{n}[{k}]")));
                    let events = std::array::from_fn(|k| ent_u32(e, &format!("events[{k}]")));
                    let tr = ent_u32(e, "lerp.pos.trType");
                    seen.states
                        .push((snap.server_time, tr, v("trBase"), events));
                    if seen.launch.is_none() {
                        seen.launch = Some((tr, v("trBase"), v("trDelta")));
                    }
                    if events.contains(&45) {
                        seen.burst = Some(v("trBase"));
                        return Some(std::mem::take(&mut seen));
                    }
                }
            }
            if f.level - fired > 8000 {
                return Some(std::mem::take(&mut seen));
            }
            None
        }) else {
            return;
        };
        let out = out.expect("no launcher shot in 120 s");
        let (tr, base, delta) = out.launch.expect("no launcher grenade in the snapshots");
        assert_eq!(tr, 5, "TR_GRAVITY: {out:?}");
        assert!(base.distance(out.eye) < 2.0, "{out:?}");
        assert!((delta.dot(out.forward) - 2400.0).abs() < 20.0, "{out:?}");
        let burst = out
            .burst
            .unwrap_or_else(|| panic!("the grenade never burst: {out:?}"));
        assert!((burst - base).dot(out.forward) > 375.0, "{out:?}");
    }

    #[test]
    fn cod4_live_airstrike_picks_its_spot_on_the_full_map() {
        use crate::play::input::Action;
        #[derive(Default)]
        struct Seen {
            given: bool,
            pressed: bool,
            frames: u32,
            drawn: Option<(bool, bool, bool)>,
            moved: Option<[f32; 2]>,
            fired: bool,
            ended: bool,
            /// The menu's texts as drawn.
            texts: Vec<String>,
        }
        let mut seen = Seen::default();
        let Some(out) = live_war_pair(move |f| {
            if f.both && !seen.given && f.level > 9000 {
                seen.given = true;
                f.tx.send((
                    0,
                    "maps/mp/gametypes/_hardpoints",
                    "giveHardpointItem",
                    "airstrike_mp",
                ))
                .ok();
            }
            if seen.given && !seen.pressed && f.s.action_slot(4) {
                seen.pressed = true;
            }
            let info = f.s.selecting_location();
            if info == 0 {
                if seen.fired {
                    seen.ended = true;
                    return Some(std::mem::take(&mut seen));
                }
                return None;
            }
            seen.frames += 1;
            let tex = |m: &str| f.hud.texture_of(m);
            let cs = |i: u16| f.hud.configstring(f.s, i);
            let map = vcod_cod4::hud::tokenize(&cs(vcod_cod4::hud::CS_MINIMAP))
                .first()
                .and_then(|m| tex(m));
            let selector = tex(&cs(827 + (info & 3) as u16 - 1));
            let border = tex("map_border");
            let has = |t: &Option<String>| {
                t.as_ref()
                    .is_some_and(|t| f.quads.iter().any(|q| &q.texture == t))
            };
            seen.drawn = Some((has(&map), has(&selector), has(&border)));
            seen.texts = f.hud.fullscreen_texts(f.shell);
            if seen.frames == 5 {
                f.s.move_selector([10.0, -20.0]);
                seen.moved = Some(f.s.selector.at);
            }
            if seen.frames == 10 {
                f.s.input.key(Action::Attack, true);
                seen.fired = true;
            }
            if seen.frames == 14 {
                f.s.input.key(Action::Attack, false);
            }
            None
        }) else {
            return;
        };
        let out = out.expect("the airstrike's selection never came and went in 120 s");
        assert!(out.pressed, "action slot 4 selected nothing");
        assert_eq!(
            out.drawn,
            Some((true, true, true)),
            "map, selector, border drawn"
        );
        let at = out.moved.unwrap();
        assert!(
            (at[0] - 0.62).abs() < 1e-4 && (at[1] - 0.56).abs() < 1e-4,
            "{at:?}"
        );
        assert!(out.ended, "the confirm never ended the selection");
        // `@PLATFORM_PRESS_TO_SET_AIRSTRIKE` localized, its key named.
        assert!(
            !out.texts.is_empty() && out.texts.iter().all(|t| !t.contains('@')),
            "{:?}",
            out.texts
        );
    }

    /// What [`streak_run`] saw: the HUD's texts, and when each step came.
    #[derive(Default)]
    struct Streak {
        kills: u32,
        last: Option<Duration>,
        texts: std::collections::BTreeSet<String>,
        /// The `dpad` drew the hardpoint's icon.
        icon: bool,
        /// Action slot 4 selected it.
        pressed: bool,
        /// The location selector came up; frames since.
        selecting: u32,
        /// The selector came down after the confirm.
        confirmed: bool,
        /// Our compass drew the vehicle as ours.
        called: bool,
    }

    /// The killcam on vcod-server, live: the enemy kills us through the
    /// stock damage callback (`gsc/killcam`), the stock killcam follows
    /// him. `CG_DrawDisconnect` never fires on it, as the server moves the
    /// archived `commandTime` up to the frame's (cod4-combat.md 7), and
    /// the use key skips it (`waitSkipKillcamButton` polls
    /// `useButtonPressed`).
    #[test]
    fn cod4_live_killcam_draws_no_interrupt_and_the_use_key_skips_it() {
        use crate::play::input::Action;
        const GSC: &str = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../cod4/tests/fixtures/gsc/killcam"
        );
        #[derive(Debug, Default)]
        struct Kc {
            sent: bool,
            start: Option<i32>,
            frames: u32,
            interrupted: u32,
            pressed: Option<i32>,
            end: Option<i32>,
        }
        let mut kc = Kc::default();
        let out = live_war_pair_in(["assault_mp,0", "assault_mp,0"], Some(GSC), move |f| {
            if f.both && f.level > 9000 && !kc.sent {
                f.tx.send((0, "maps/mp/mp_crash_fx", "killedBy", "enemy"))
                    .ok();
                kc.sent = true;
            }
            let enemy =
                f.e.client()
                    .snapshots
                    .last()
                    .and_then(|x| x.ps.get("clientNum"));
            let ps = &f.s.client().snapshots.last()?.ps;
            let watching = kc.sent
                && ps.get("otherFlags").unwrap_or(0) & 2 != 0
                && ps.get("clientNum") == enemy;
            if watching {
                let start = *kc.start.get_or_insert(f.level);
                kc.frames += 1;
                kc.interrupted += u32::from(f.s.interrupted());
                if kc.pressed.is_none() && f.level - start >= 500 {
                    f.s.input.key(Action::Use, true);
                    kc.pressed = Some(f.level);
                }
            } else if kc.start.is_some() {
                f.s.input.key(Action::Use, false);
                kc.end = Some(f.level);
                return Some(std::mem::take(&mut kc));
            }
            None
        });
        let Some(out) = out else {
            return;
        };
        let kc = out.expect("the killcam started and ended");
        eprintln!("{kc:?}");
        assert!(kc.frames > 10, "{kc:?}");
        assert_eq!(kc.interrupted, 0, "{kc:?}");
        let (pressed, end) = (kc.pressed.expect("pressed"), kc.end.unwrap());
        assert!(end - pressed <= 500, "the use key skipped it: {kc:?}");
    }

    /// The stock primary progress bar, live: `updateBar( 0, 0.5 )` sets
    /// the fill a unit wide and `scaleOverTime`s it to full in 2 s (the HQ
    /// capture's bar). Our fill grows with it, never full from the start.
    #[test]
    fn cod4_live_progress_bar_fills_over_its_scale_time() {
        const GSC: &str = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../cod4/tests/fixtures/gsc/killcam"
        );
        let mut sent: Option<i32> = None;
        let mut widths: Vec<(i32, f32)> = Vec::new();
        let out = live_war_pair_in(["assault_mp,0", "assault_mp,0"], Some(GSC), move |f| {
            if f.both && f.level > 9000 && sent.is_none() {
                f.tx.send((0, "maps/mp/mp_crash_fx", "progressBar", ""))
                    .ok();
                sent = Some(f.level);
            }
            let start = sent?;
            let fill = f.hud.texture_of("progress_bar_fill").expect("the fill");
            // The fill is white; the backing under it shares its image, black.
            if let Some(q) = f
                .quads
                .iter()
                .find(|q| q.texture == fill && q.rgba[0] > 0.5)
            {
                widths.push((f.level - start, q.verts[1][0] - q.verts[0][0]));
            }
            (f.level - start > 3000).then(|| std::mem::take(&mut widths))
        });
        let Some(out) = out else {
            return;
        };
        let w = out.expect("the bar drew");
        let full = w.last().expect("a bar").1;
        let at = |ms: i32| w.iter().find(|(t, _)| *t >= ms).map(|x| x.1);
        let early = w.first().unwrap().1;
        assert!(early < 0.25 * full, "{w:?}");
        let mid = at(1200).expect("a frame at 1.2 s");
        assert!(mid > 0.25 * full && mid < 0.9 * full, "{w:?}");
    }

    /// [`live_war_pair_in`] under `streak_kill`'s probe gsc (no killcam,
    /// `scr_game_forceuav` 1): we kill the enemy `kills` times in a row
    /// through the stock damage callback, so the stock
    /// `Callback_PlayerKilled` counts the streak and
    /// `giveHardpointItemForStreak` hands out `weapon`. Then the `dpad`'s
    /// icon, its key, the airstrike's selector and its confirm, until our
    /// compass draws the `kind` vehicle as ours (cod4-hud.md 15).
    fn streak_run(kills: u32, weapon: &'static str, kind: &'static str) -> Option<Streak> {
        use crate::play::input::Action;
        const GSC: &str = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../cod4/tests/fixtures/gsc/streak_kill"
        );
        let t0 = clk_now();
        let mut seen = Streak::default();
        let out = live_war_pair_in(["assault_mp,0", "assault_mp,0"], Some(GSC), move |f| {
            let icon = f
                .hud
                .dpad_icon(weapon)
                .and_then(|m| f.hud.texture_of(&m))
                .unwrap_or_else(|| panic!("{weapon}'s dpad icon"));
            let ours = f
                .hud
                .texture_of(&format!("compass_objpoint_{kind}_friendly"))
                .unwrap_or_else(|| panic!("{kind}'s compass icon"));
            let t = since(t0);
            if f.both
                && f.level > 9000
                && seen.kills < kills
                && f.e.spawn().is_some()
                && seen
                    .last
                    .is_none_or(|l| t - l > Duration::from_millis(1500))
            {
                f.tx.send((1, "maps/mp/mp_crash_fx", "killedBy", "joiner"))
                    .ok();
                seen.kills += 1;
                seen.last = Some(t);
            }
            if let Some(snap) = f.s.client().snapshots.last() {
                for e in vcod_cod4::hud::hud_elems(&snap.ps) {
                    if let Some(t) = f.hud.elem_text(f.s, &e, f.level, f.shell)
                        && !t.is_empty()
                    {
                        seen.texts.insert(t);
                    }
                }
            }
            seen.icon |= seen.kills == kills && f.quads.iter().any(|q| q.texture == icon);
            if seen.icon && !seen.pressed && f.s.action_slot(4) {
                seen.pressed = true;
            }
            if f.s.selecting_location() != 0 {
                seen.selecting += 1;
                // The confirm: `+attack`'s 0x10000 while the map is up.
                f.s.input.key(Action::Attack, seen.selecting >= 10);
            } else if seen.selecting > 0 && !seen.confirmed {
                seen.confirmed = true;
                f.s.input.key(Action::Attack, false);
            }
            seen.called |= f.quads.iter().any(|q| q.texture == ours);
            // The streak's splash waits its turn behind the promotion's.
            let splash = seen.texts.contains(&format!("{kills} Kill Streak!"));
            (seen.called && splash).then(|| std::mem::take(&mut seen))
        })?;
        Some(out.unwrap_or_else(|| panic!("no {kind} of ours on the compass in 40 s")))
    }

    /// One earn of [`airstrike_twice`].
    #[derive(Debug, Default)]
    struct Earn {
        /// The `dpad` drew the airstrike, action slot 4 selected it, the
        /// selector came up and went down on the confirm.
        icon: bool,
        pressed: bool,
        confirmed: bool,
        /// The enemy, set down where the strike aims, died under it.
        enemy_killed: bool,
        /// The loop sounds our snapshots carried during the strike, and
        /// those still on 20 s after the confirm.
        loops: std::collections::BTreeSet<String>,
        loops_after: Vec<String>,
    }

    /// [`live_war_pair_in`] under `streak_kill`'s probe gsc: twice, our
    /// own M16 kills the enemy three times in a row (each time set down
    /// before us, the kill landing inside one of our cmds as a real one
    /// does), the `dpad` draws the airstrike, its key opens the selector,
    /// the confirm sends the planes, the enemy stands where they aim, and
    /// 20 s on the strike's loop sounds are gone. Between the two the
    /// enemy kills us, which ends our streak. The HUD's texts and the
    /// local sounds come back beside.
    fn airstrike_twice() -> Option<(Vec<Earn>, std::collections::BTreeSet<String>, Vec<String>)> {
        use crate::play::input::Action;
        const GSC: &str = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../cod4/tests/fixtures/gsc/streak_kill"
        );
        #[derive(Clone, Copy, Debug, PartialEq)]
        enum Phase {
            Kill,
            Earned(Duration),
            Selecting(u32),
            Strike(Duration),
            Reset(Duration),
        }
        let t0 = clk_now();
        let mut earns: Vec<Earn> = vec![Earn::default()];
        let mut texts = std::collections::BTreeSet::new();
        let mut sounds = Vec::new();
        let mut phase = Phase::Kill;
        let mut kills = 0;
        let mut placed: Option<Duration> = None;
        let mut ready_at = Duration::ZERO;
        let mut under = false;
        // A seed whose strike lands on him both times.
        const SRAND: &str = "1";
        let out = live_war_pair_in(["assault_mp,0", "assault_mp,0"], Some(GSC), move |f| {
            let t = since(t0);
            let icon = f
                .hud
                .dpad_icon("airstrike_mp")
                .and_then(|m| f.hud.texture_of(&m))
                .expect("the airstrike's dpad icon");
            sounds.extend(f.local_sounds.iter().filter(|s| s.1).map(|s| s.0.clone()));
            if let Some(snap) = f.s.client().snapshots.last() {
                for e in vcod_cod4::hud::hud_elems(&snap.ps) {
                    if let Some(t) = f.hud.elem_text(f.s, &e, f.level, f.shell)
                        && !t.is_empty()
                    {
                        texts.insert(t);
                    }
                }
            }
            let loops: Vec<String> = f.s.client().snapshots.last().map_or_else(Vec::new, |x| {
                let cs = |i: u16| f.hud.configstring(f.s, i);
                crate::cod4_audio::loop_sounds(&x.entities, &cs, &[], &origin_of)
                    .into_values()
                    .filter_map(|(_, a, _)| a)
                    .filter(|a| a.starts_with("veh_mig29"))
                    .collect()
            });
            let enemy_up = f.e.spawn().is_some();
            if phase == Phase::Kill
                && let Some(at) = placed
                && t - at > Duration::from_millis(300)
                && !enemy_up
            {
                f.s.input.key(Action::Attack, false);
                placed = None;
                kills += 1;
                ready_at = t + Duration::from_millis(1500);
                if kills == 3 {
                    phase = Phase::Earned(t);
                }
            }
            let earn = earns.last_mut().unwrap();
            match phase {
                Phase::Kill => {
                    if f.level < 9000 || !f.both || t < ready_at {
                        return None;
                    }
                    let Some(at) = placed else {
                        f.tx.send((1, "maps/mp/mp_crash_fx", "standBefore", "joiner"))
                            .ok();
                        placed = Some(t);
                        return None;
                    };
                    let since = t - at;
                    if since < Duration::from_millis(300) {
                        return None;
                    }
                    if since > Duration::from_secs(6) {
                        // A wall in the way: set him down again.
                        placed = None;
                        return None;
                    }
                    // Aim at his chest and fire the burst over and over.
                    let ps = &f.e.client().snapshots.last()?.ps;
                    let target = vec3(|k| ps_f32(ps, &format!("origin[{k}]"))) + Vec3::Z * 40.0;
                    let d = target - f.eye;
                    let yaw = d.y.atan2(d.x).to_degrees();
                    let pitch = -d.z.atan2(d.truncate().length()).to_degrees();
                    let wrap = |a: f32| (a + 540.0).rem_euclid(360.0) - 180.0;
                    f.s.turn([wrap(pitch - f.angles.x), wrap(yaw - f.angles.y)]);
                    f.s.input
                        .key(Action::Attack, (since.as_millis() / 120).is_multiple_of(2));
                }
                Phase::Earned(at) => {
                    earn.icon |= f.quads.iter().any(|q| q.texture == icon);
                    // The third kill's respawn first (about 2 s), so he
                    // stands where the strike aims before its bomb.
                    let respawned = t - at > Duration::from_secs(3) && enemy_up;
                    if earn.icon && respawned && !earn.pressed && f.s.action_slot(4) {
                        earn.pressed = true;
                        // The strike's path and spray are the stock
                        // script's draws: the same ones on every run.
                        f.tx.send((0, "", "srand", SRAND)).ok();
                    }
                    if f.s.selecting_location() != 0 {
                        phase = Phase::Selecting(0);
                    } else if t - at > Duration::from_secs(8) {
                        // Never came: report it.
                        return Some((
                            std::mem::take(&mut earns),
                            std::mem::take(&mut texts),
                            std::mem::take(&mut sounds),
                        ));
                    }
                }
                Phase::Selecting(n) => {
                    if f.s.selecting_location() != 0 {
                        f.s.input.key(Action::Attack, n >= 10);
                        phase = Phase::Selecting(n + 1);
                    } else {
                        f.s.input.key(Action::Attack, false);
                        earn.confirmed = true;
                        f.tx.send((1, "maps/mp/mp_crash_fx", "standAtStrike", ""))
                            .ok();
                        phase = Phase::Strike(t);
                    }
                }
                Phase::Strike(at) => {
                    earn.loops.extend(loops.iter().cloned());
                    // Up under the strike once `standAtStrike` set him
                    // down, then dead.
                    if enemy_up && t - at > Duration::from_secs(3) {
                        under = true;
                    }
                    earn.enemy_killed |= under && !enemy_up;
                    if t - at > Duration::from_secs(20) {
                        earn.loops_after = loops;
                        if earns.len() == 2 {
                            return Some((
                                std::mem::take(&mut earns),
                                std::mem::take(&mut texts),
                                std::mem::take(&mut sounds),
                            ));
                        }
                        // The strike may have spared us: the enemy ends
                        // our streak.
                        f.tx.send((0, "maps/mp/mp_crash_fx", "killedBy", "enemy"))
                            .ok();
                        earns.push(Earn::default());
                        under = false;
                        phase = Phase::Reset(t);
                    }
                }
                Phase::Reset(at) => {
                    if t - at > Duration::from_secs(1) {
                        phase = Phase::Kill;
                        kills = 0;
                        ready_at = t;
                    }
                }
            }
            None
        })?;
        Some(out.expect("the two airstrikes did not play out in 120 s"))
    }

    /// [`airstrike_twice`]: each earn draws on the `dpad`, its key
    /// opens the selector, the strike kills the enemy standing under it
    /// and its jets' loops end; the splash reads "3 Kill Streak!" and
    /// names the slot's key, and the promotion's splash plays
    /// `mp_level_up`.
    #[test]
    fn cod4_live_three_kills_earn_the_airstrike_and_call_it_in() {
        let Some((earns, texts, sounds)) = airstrike_twice() else {
            return;
        };
        println!("{earns:#?}\n{texts:?}\n{sounds:?}");
        assert_eq!(earns.len(), 2, "{earns:#?}");
        for (k, e) in earns.iter().enumerate() {
            assert!(e.icon, "earn {k}: the dpad never drew the airstrike");
            assert!(e.pressed, "earn {k}: action slot 4 selected nothing");
            assert!(e.confirmed, "earn {k}: no selector");
            assert!(e.enemy_killed, "earn {k}: the strike killed nobody");
            assert!(!e.loops.is_empty(), "earn {k}: no jet loops");
            assert!(e.loops_after.is_empty(), "earn {k}: {:?}", e.loops_after);
        }
        for t in [
            "3 Kill Streak!",
            "Press 6 for AIRSTRIKE.",
            "You've been promoted!",
        ] {
            assert!(texts.contains(t), "{t}: {texts:?}");
        }
        assert!(
            texts.iter().all(|t| !t.contains("&&") && !t.contains("[{")),
            "{texts:?}"
        );
        assert!(sounds.iter().any(|s| s == "mp_level_up"), "{sounds:?}");
    }

    /// [`streak_run`]: the fifth earns the helicopter over the unused
    /// airstrike, the `dpad` draws it and its key calls it in.
    #[test]
    fn cod4_live_five_kills_earn_the_helicopter_and_call_it_in() {
        let Some(out) = streak_run(5, "helicopter_mp", "helicopter") else {
            return;
        };
        println!("{:?}", out.texts);
        assert_eq!(out.kills, 5);
        assert!(out.icon, "the dpad never drew the helicopter");
        assert!(out.pressed, "action slot 4 selected nothing");
        assert_eq!(out.selecting, 0, "a selector for the helicopter");
        assert!(out.texts.contains("5 Kill Streak!"), "{:?}", out.texts);
    }

    /// What [`cod4_live_airstrike_then_helicopter_in_one_life`] saw.
    #[derive(Debug, Default)]
    struct OneLife {
        kills: u32,
        last: Option<Duration>,
        strike_pressed: bool,
        selecting: u32,
        confirmed: bool,
        /// Our level time when slot 4 took the helicopter.
        heli_pressed: Option<i32>,
        radio_up: bool,
        /// Our compass drew the helicopter as ours.
        called: bool,
        /// The weapon in hand once the radio went down.
        back: Option<String>,
        died: bool,
    }

    /// [`streak_run`]'s kills in one life: the third earns the airstrike,
    /// which we call in from the selector and stand clear of; the fifth
    /// earns the helicopter, whose key raises the radio, calls it in and
    /// puts the M16 back. The stock `hardpointItemWaiter` once died with
    /// the airstrike's `stop_location_selection`, an `endon` its returned
    /// `selectAirstrikeLocation` had left on the thread, so the radio stuck
    /// in hand until the next life (cod4-gsc.md 4.2).
    #[test]
    fn cod4_live_airstrike_then_helicopter_in_one_life() {
        use crate::play::input::Action;
        const GSC: &str = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../cod4/tests/fixtures/gsc/streak_kill"
        );
        let t0 = clk_now();
        let mut seen = OneLife::default();
        let out = live_war_pair_in(["assault_mp,0", "assault_mp,0"], Some(GSC), move |f| {
            let icon = |w: &str| {
                let tex = f.hud.dpad_icon(w).and_then(|m| f.hud.texture_of(&m));
                tex.is_some_and(|t| f.quads.iter().any(|q| q.texture == t))
            };
            let ours = f
                .hud
                .texture_of("compass_objpoint_helicopter_friendly")
                .expect("the helicopter's compass icon");
            let t = since(t0);
            let target = if seen.confirmed { 5 } else { 3 };
            if f.both
                && f.level > 9000
                && seen.kills < target
                && f.e.spawn().is_some()
                && seen
                    .last
                    .is_none_or(|l| t - l > Duration::from_millis(1500))
            {
                f.tx.send((1, "maps/mp/mp_crash_fx", "killedBy", "joiner"))
                    .ok();
                seen.kills += 1;
                seen.last = Some(t);
            }
            seen.died |= seen.kills > 0 && f.s.spawn().is_none();
            if seen.kills == 3 && !seen.strike_pressed && icon("airstrike_mp") && f.s.action_slot(4)
            {
                seen.strike_pressed = true;
            }
            if f.s.selecting_location() != 0 {
                seen.selecting += 1;
                f.s.input.key(Action::Attack, seen.selecting >= 10);
            } else if seen.selecting > 0 && !seen.confirmed {
                seen.confirmed = true;
                f.s.input.key(Action::Attack, false);
                f.tx.send((0, "maps/mp/mp_crash_fx", "standClear", "")).ok();
            }
            if seen.kills == 5
                && seen.heli_pressed.is_none()
                && icon("helicopter_mp")
                && f.s.action_slot(4)
            {
                seen.heli_pressed = Some(f.level);
            }
            let pressed = seen.heli_pressed?;
            let weapon =
                f.s.client()
                    .snapshots
                    .last()
                    .map_or(0, |x| x.ps.get("weapon").unwrap_or(0) as usize);
            let name =
                f.s.level_defs
                    .as_ref()
                    .and_then(|(_, d)| d.get(weapon))
                    .map(|d| d.name.clone());
            seen.radio_up |= name.as_deref() == Some("helicopter_mp");
            seen.called |= f.quads.iter().any(|q| q.texture == ours);
            if seen.radio_up
                && seen.back.is_none()
                // Empty hands between the take and the switch read "".
                && let Some(n) = name.filter(|n| !n.is_empty() && n != "helicopter_mp")
            {
                seen.back = Some(n);
            }
            let done = seen.called && seen.back.is_some();
            (done || f.level - pressed > 10_000).then(|| std::mem::take(&mut seen))
        });
        let Some(out) = out else {
            return;
        };
        let s = out.expect("the helicopter's key never came in 120 s");
        println!("{s:?}");
        assert!(!s.died, "we died: a second life hides it: {s:?}");
        assert!(s.confirmed, "the airstrike was never called: {s:?}");
        assert!(s.radio_up, "the radio never came up: {s:?}");
        assert!(s.called, "the helicopter never came: {s:?}");
        assert_eq!(s.back.as_deref(), Some("m16_gl_mp"), "{s:?}");
    }

    /// [`live_war_pair_in`] under `offscreen_waypoint`'s probe gsc: a
    /// client waypoint 2000 units behind us with an offscreen material
    /// (`setWaypoint(true, "waypoint_targetneutral")`) draws in the 2D
    /// pass, pushed to the screen's top edge padding with
    /// `hud_offscreenobjectivepointer` beside it, and never as a world
    /// sprite (cod4-hud.md 16.3).
    #[test]
    fn cod4_live_offscreen_waypoint_sits_on_the_edge_with_its_pointer() {
        const GSC: &str = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../cod4/tests/fixtures/gsc/offscreen_waypoint"
        );
        let out = live_war_pair_in(["assault_mp,0", "assault_mp,0"], Some(GSC), |f| {
            let icon = f
                .hud
                .texture_of("waypoint_targetneutral")
                .expect("the icon's image");
            let pointer = f
                .hud
                .texture_of(crate::cod4_waypoints::POINTER)
                .expect("the pointer's image");
            let (fwd, right, up) =
                crate::camera::basis(f.angles.y.to_radians(), -f.angles.x.to_radians());
            let size = [1280.0f32, 960.0];
            // `cg_fov` 65 on a 4:3 window.
            let tan_x = 32.5f32.to_radians().tan();
            let eye = crate::cod4_waypoints::Eye {
                origin: f.eye,
                forward: fwd,
                left: -right,
                up,
                tan: [tan_x, tan_x * 0.75],
                size,
            };
            let q = f.hud.offscreen_waypoints(f.s, &eye, f.level);
            let found = q.iter().any(|q| q.texture == icon);
            found.then(|| {
                (
                    q.iter()
                        .map(|q| (q.texture.clone(), q.verts))
                        .collect::<Vec<_>>(),
                    icon,
                    pointer,
                )
            })
        });
        let Some(out) = out else {
            return;
        };
        let (quads, icon, pointer) = out.expect("the waypoint never drew in 120 s");
        println!("{quads:?}");
        assert_eq!(quads.len(), 2, "the pointer and the icon");
        assert_eq!(quads[0].0, pointer);
        assert_eq!(quads[1].0, icon);
        let centre = |v: [[f32; 2]; 4]| [(v[0][0] + v[2][0]) * 0.5, (v[0][1] + v[2][1]) * 0.5];
        let [x, y] = centre(quads[1].1);
        // Behind and above the eye: the projection (0x439320) sends it the
        // way its height off the eye points, to the top edge's padding
        // (6 + 30 of the 480 grid, 2 px a unit), the arrow over it; 0.9
        // of 36 square at 2000 units (`waypointDistScale*`).
        assert!(
            (x - 640.0).abs() < 40.0 && (y - 72.0).abs() < 1.0,
            "icon at ({x}, {y})"
        );
        assert!(centre(quads[0].1)[1] < y, "the pointer points up and out");
        let w = quads[1].1[1][0] - quads[1].1[0][0];
        assert!((w - 64.8).abs() < 0.1, "icon {w} wide");
    }

    /// [`live_war_pair_as`] as Spec Ops (MP5, Steady Aim in `ps.perks`'s
    /// bit 1) and as Assault (M16, no Steady Aim): the crosshair draws its
    /// centre and four sides off the live playerstate, the sides as far
    /// out as `CG_CalcReticleSpread` puts them at the drawn fov, Spec
    /// Ops's spread scaled by `perk_weapSpreadMultiplier` 0.65
    /// (cod4-hud.md 17).
    #[test]
    fn cod4_live_crosshair_spreads_by_the_weapon_and_steady_aim() {
        // The crosshair quads, the weapon's def and the playerstate, once
        // we stand spawned with a weapon up.
        type Seen = (
            Vec<crate::hud::HudQuad>,
            vcod_cod4::hud::WeaponHud,
            crate::cod4_viewmodel::ViewPs,
        );
        let run = |class: &'static str| {
            live_war_pair_as([class, "assault_mp,0"], |f| {
                let v = f.view;
                if !f.both || f.level < 3000 || v.weaponstate != 0 || v.weapon == 0 {
                    return None;
                }
                let gs = f.s.gamestate()?;
                let name = weapon_list(gs).get(v.weapon as usize - 1)?.clone();
                let def = f.hud.weapon_hud(&name)?.clone();
                let q =
                    f.hud
                        .crosshair(f.s, v, [TAN_Y * 4.0 / 3.0, TAN_Y], (1280.0, 960.0), f.shell);
                Some((q, def, *v))
            })
        };
        const TAN_Y: f32 = 0.5;
        for (class, steady) in [("specops_mp,0", true), ("assault_mp,0", false)] {
            let Some(out) = run(class) else {
                return;
            };
            let (quads, def, ps): Seen = out.expect("never spawned with a weapon up");
            assert_eq!(ps.perks & 2 != 0, steady, "{class}'s perks {:#x}", ps.perks);
            let r = &def.reticle;
            let drawn: Vec<_> = quads
                .iter()
                .map(|q| (&q.texture, q.verts[0], q.rgba))
                .collect();
            println!("{class}: {drawn:?} {r:?} ass {}", ps.aim_spread_scale);
            let centre = usize::from(r.center.is_some());
            assert!(r.side.is_some(), "{class}: no reticleSide");
            assert_eq!(
                quads.len(),
                centre + 4,
                "{class}: the centre and four sides"
            );
            // The right side (the second, turned 90) sits `spread` right
            // of the centre, on the 1280 by 960 window's 2 px a unit.
            let right = &quads[centre + 1];
            let left_edge = right.verts.iter().map(|v| v[0]).fold(f32::MAX, f32::min);
            let k = if steady { 0.65 } else { 1.0 };
            let [stand_min, _, _, stand_max, _, _] = r.hip_spread;
            let deg = ((stand_max - stand_min) * ps.aim_spread_scale / 255.0 + stand_min) * k;
            let px = (deg.to_radians().tan() * 240.0 / TAN_Y).max(r.min_ofs as f32);
            let spread = px - r.side_pos * r.side_size as f32;
            assert!(
                (left_edge - (640.0 + 2.0 * spread)).abs() < 0.01,
                "{class}: right side at {left_edge}, expected {}",
                640.0 + 2.0 * spread
            );
        }
    }

    /// [`live_war_pair`]: the M16's sight raised, its crosshair drawn while
    /// the lerp runs. With `cg_crosshairDynamic` 1 the centre sits where
    /// the view weapon's turned gun points (`CG_CalcCrosshairPosition`,
    /// cod4-hud.md 17), off the centre the stock 0 keeps it on.
    #[test]
    fn cod4_live_dynamic_crosshair_follows_the_raised_gun() {
        use crate::play::input::Action;
        let root = vcod_cod4::testing::cod4_install().map(|i| i.root().to_path_buf());
        let mut view = crate::cod4_viewmodel::Cod4View::default();
        let mut bodies = None;
        let mut dynamic = crate::console::shell::Shell::new();
        dynamic.execute("set cg_crosshairDynamic 1");
        let mut raised = false;
        let Some(out) = live_war_pair(move |f| {
            let v = *f.view;
            if !f.both || f.level < 3000 || v.weapon == 0 {
                return None;
            }
            // The load takes frames: the sight goes up after it.
            let Some(bodies) = &bodies else {
                let ents = f.s.entity_assets();
                bodies =
                    Some(crate::cod4_fly::load_bodies(root.as_ref()?, "mp_crash", &ents).unwrap());
                return None;
            };
            if !raised {
                raised = true;
                f.s.input.key(Action::Ads, true);
            }
            let scene = f.s.scene()?;
            let list = weapon_list(f.s.gamestate()?);
            let out = view.frame(
                &v,
                &list,
                scene.hands.as_deref(),
                &bodies.1,
                &bodies.0,
                scene.render_time,
            );
            eprintln!(
                "pos {} dir {:?} t {} pmf {:#x} ws {}",
                v.pos_frac, out.gun_dir, scene.render_time, v.pm_flags, v.weaponstate
            );
            let dir = out.gun_dir?;
            let v = crate::cod4_viewmodel::ViewPs {
                gun_dir: Some(dir),
                ..v
            };
            let tan = [0.75, 0.5];
            let size = (1280.0, 960.0);
            let still = f.hud.crosshair(f.s, &v, tan, size, f.shell);
            let moved = f.hud.crosshair(f.s, &v, tan, size, &dynamic);
            (!still.is_empty() && dir[1].abs() + dir[2].abs() > 1e-4)
                .then(|| (still[0].verts, moved[0].verts, dir))
        }) else {
            return;
        };
        let (still, moved, dir) = out.expect("no frame with the sight coming up");
        let centre = |q: [[f32; 2]; 4]| [0, 1].map(|k| q.iter().map(|v| v[k]).sum::<f32>() / 4.0);
        let (a, b) = (centre(still), centre(moved));
        // 2 px a grid unit on 1280 by 960.
        let want = [
            dir[1] / (dir[0] * 0.75) * -640.0,
            dir[2] / (dir[0] * 0.5) * -480.0,
        ];
        // The first piece (the centre, else the top side) moves by the
        // gun's projection, sitting on the centre's column without it.
        assert!((a[0] - 640.0).abs() < 0.01, "{a:?}");
        assert!(
            (b[0] - a[0] - want[0]).abs() < 0.01 && (b[1] - a[1] - want[1]).abs() < 0.01,
            "{b:?} for {dir:?}, still {a:?}"
        );
        assert!(want[0].abs() > 1.0, "the gun turned {dir:?}");
    }

    /// The camo and the rocket through vcod-server, live: the probe
    /// script's `camo` gives `ak47_mp` on `gunModel[3]` and then `rpg_mp`
    /// (whose slot 3 is empty: slot 0), switching to each. The snapshot's
    /// `weaponmodels[]` reaches the view weapon, which draws the AK-47's
    /// Blue Tiger materials and the RPG's rocket (cod4-client-view.md 4.7).
    #[test]
    fn cod4_live_a_given_camo_and_the_rocket_reach_the_view_weapon() {
        const GSC: &str = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../cod4/tests/fixtures/gsc/killcam"
        );
        let root = vcod_cod4::testing::cod4_install().map(|i| i.root().to_path_buf());
        let mut view = crate::cod4_viewmodel::Cod4View::default();
        let mut bodies = None;
        let mut asked = 0;
        // The AK-47's `weaponmodels` slot and gun materials once drawn.
        let mut ak: Option<(u8, Vec<String>)> = None;
        let out = live_war_pair_in(["assault_mp,0", "assault_mp,0"], Some(GSC), move |f| {
            let v = *f.view;
            if !f.both || f.level < 3000 || v.weapon == 0 {
                return None;
            }
            let Some(bodies) = &bodies else {
                let ents = f.s.entity_assets();
                bodies =
                    Some(crate::cod4_fly::load_bodies(root.as_ref()?, "mp_crash", &ents).unwrap());
                return None;
            };
            let give = ["ak47_mp", "rpg_mp"];
            if asked < give.len() && (asked == 0 || ak.is_some()) {
                f.tx.send((0, "maps/mp/mp_crash_fx", "camo", give[asked]))
                    .ok();
                asked += 1;
            }
            let scene = f.s.scene()?;
            let list = weapon_list(f.s.gamestate()?);
            let name = list.get(v.weapon as usize - 1)?.clone();
            let out = view.frame(
                &v,
                &list,
                scene.hands.as_deref(),
                &bodies.1,
                &bodies.0,
                scene.render_time,
            );
            let models = out.frame?.models?;
            match name.as_str() {
                "ak47_mp" if ak.is_none() => {
                    let slot = v.weapon_models[v.weapon as usize];
                    ak = Some((slot, models[1].lods[0].materials.clone()));
                    None
                }
                "rpg_mp" => Some((ak.clone()?, models.len())),
                _ => None,
            }
        });
        let Some(out) = out else {
            return;
        };
        let ((slot, mats), rpg_parts) = out.expect("never drew the AK-47 and then the RPG");
        assert_eq!(slot, 3);
        assert!(
            mats.iter().any(|m| m == "mc/mtl_weapon_ak47_bwmrpt"),
            "{mats:?}"
        );
        assert_eq!(rpg_parts, 3, "hands, launcher and rocket");
    }

    /// Another player's gun draws his camo, live on vcod-server: the probe
    /// script's `camo` gives the enemy `ak47_mp` on slot 3 and `near`
    /// stands him beside us. His entity in our snapshot carries
    /// `weaponModel` 3, and his posed rig holds `worldModel[3]`, whose
    /// materials are the Blue Tiger's (cod4-client-view.md 4.7).
    #[test]
    fn cod4_live_another_players_gun_draws_his_camo() {
        use crate::play::cod4_players::Cod4Players;
        const GSC: &str = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../cod4/tests/fixtures/gsc/killcam"
        );
        let root = vcod_cod4::testing::cod4_install().map(|i| i.root().to_path_buf());
        let mut bodies = None;
        let mut asked = 0;
        let out = live_war_pair_in(["assault_mp,0", "assault_mp,0"], Some(GSC), move |f| {
            if !f.both || f.level < 3000 {
                return None;
            }
            if bodies.is_none() {
                let ents = f.s.entity_assets();
                bodies =
                    Some(crate::cod4_fly::load_bodies(root.as_ref()?, "mp_crash", &ents).unwrap());
                return None;
            }
            let script = [("camo", "ak47_mp"), ("near", "joiner")];
            if let Some((name, arg)) = script.get(asked) {
                f.tx.send((1, "maps/mp/mp_crash_fx", *name, *arg)).ok();
                asked += 1;
                return None;
            }
            let enemy = f.e.gamestate()?.client_num;
            let scene = f.s.scene()?;
            let d = scene
                .entities
                .iter()
                .find(|d| d.body.as_ref().is_some_and(|b| b.frame.client_num == enemy))?;
            let v = d.body.clone()?;
            (v.weapon.as_deref() == Some("ak47_mp")).then(|| {
                let at = (d.number, scene.render_time, scene.server_time);
                (bodies.take().unwrap(), v, at)
            })
        });
        let Some(out) = out else {
            return;
        };
        let ((_, weapons, src), v, (n, render, server)) =
            out.expect("never saw the enemy hold the AK-47");
        assert_eq!(v.weapon_model, 3);
        // His body posed as the session draws it.
        let mut players = Cod4Players::default();
        let posed = players.posed(&src, &weapons, &[(n, &v)], &[], render, server);
        let (_, rig, _) = posed.first().expect("his rig builds");
        let camo = weapons["ak47_mp"].worlds[3].clone();
        assert_eq!(camo.as_deref(), Some("weapon_ak47_mp_bwmrpt"));
        let (_, gun) = rig
            .models
            .iter()
            .find(|(m, _)| Some(m) == camo.as_ref())
            .expect("the rig holds the camo's world model");
        let mats = &gun.lods[0].materials;
        assert!(
            mats.iter().any(|m| m == "mc/mtl_weapon_ak47_bwmrpt"),
            "{mats:?}"
        );
        assert!(!mats.iter().any(|m| m == "mc/mtl_weapon_ak47"), "{mats:?}");
    }

    /// [`live_war_pair_as`] with the enemy a demolitions class: the stock
    /// `_weapons` stows his RPG on `tag_stowed_back`, a tag first indexed
    /// by his client info at his spawn. Every snapshot that names an
    /// attachment's tag arrives with the tag's configstring already in
    /// the table, as retail's `G_UpdateClientInfo` indexes it inside the
    /// frame, so no body grafts an attachment by its root bone for a
    /// snapshot (cod4-player-anims.md 5).
    #[test]
    fn cod4_live_a_new_attachment_tag_arrives_with_its_snapshot() {
        let mut stowed = false;
        let Some(out) = live_war_pair_as(["assault_mp,0", "demolitions_mp,0"], move |f| {
            let gs = f.s.gamestate()?;
            let snap = f.s.client().snapshots.last()?;
            for c in &snap.clients {
                for i in 0..6 {
                    let Some(model) =
                        model_cs(gs, client_u32(c, &format!("attachModelIndex[{i}]")))
                    else {
                        continue;
                    };
                    let tag = client_u32(c, &format!("attachTagIndex[{i}]"));
                    let named =
                        u16::try_from(tag).is_ok_and(|t| !gs.configstring(CS_TAGS + t).is_empty());
                    if model.contains("_stow") {
                        stowed = true;
                    }
                    if tag != 0 && !named {
                        return Some(Err(format!("{model}: tag {tag} not yet in the table")));
                    }
                }
            }
            (stowed && f.both && f.level > 3000).then_some(Ok(()))
        }) else {
            return;
        };
        out.expect("never saw a stowed weapon").unwrap();
    }

    /// [`live_war_pair`]: the stock
    /// `_hardpoints::giveHardpointItem("radar_mp")` puts the UAV on action
    /// slot 4 and the `dpad` menu draws its icon; the slot's key selects
    /// it, the stock `hardpointItemWaiter` calls the UAV in
    /// (`setTeamRadar`, `ps.radarEnabled`), the sweep's line crosses the
    /// compass and the compass draws the enemy's ping (cod4-hud.md 15).
    #[test]
    fn cod4_live_uav_hardpoint_shows_on_the_dpad_and_pings_the_enemy() {
        #[derive(Default)]
        struct Seen {
            given: bool,
            icon: Option<Duration>,
            pressed: Option<Duration>,
            radar: Option<Duration>,
            line_s: Vec<f32>,
            pinged: Option<Duration>,
        }
        let t0 = clk_now();
        let mut seen = Seen::default();
        let Some(out) = live_war_pair(move |f| {
            let radar_icon = f
                .hud
                .dpad_icon("radar_mp")
                .and_then(|m| f.hud.texture_of(&m))
                .expect("radar_mp's dpad icon");
            let ping = f
                .hud
                .texture_of("compassping_enemy")
                .expect("the enemy ping");
            let line = f
                .hud
                .texture_of("compass_radarline")
                .expect("the sweep's line");
            if f.both && !seen.given && f.level > 9000 {
                seen.given = true;
                f.tx.send((
                    0,
                    "maps/mp/gametypes/_hardpoints",
                    "giveHardpointItem",
                    "radar_mp",
                ))
                .ok();
            }
            let t = since(t0);
            if seen.icon.is_none() && f.quads.iter().any(|q| q.texture == radar_icon) {
                seen.icon = Some(t);
            }
            if seen.icon.is_some() && seen.pressed.is_none() && f.s.action_slot(4) {
                seen.pressed = Some(t);
            }
            let on =
                f.s.client()
                    .snapshots
                    .last()
                    .and_then(|x| x.ps.get("radarEnabled"))
                    == Some(1);
            if on && seen.radar.is_none() {
                seen.radar = Some(t);
            }
            let lines: Vec<_> = f.quads.iter().filter(|q| q.texture == line).collect();
            assert!(on || lines.is_empty(), "a sweep line with no UAV up");
            if let Some(q) = lines.first() {
                seen.line_s.push((q.uvs[0][0] + q.uvs[2][0]) * 0.5);
            }
            if seen.radar.is_some() && f.quads.iter().any(|q| q.texture == ping) {
                seen.pinged = Some(t);
            }
            // The ping, and a second of the sweep.
            (seen.pinged.is_some() && seen.line_s.len() > 60).then(|| std::mem::take(&mut seen))
        }) else {
            return;
        };
        let out = out.expect("no ping and a second of the sweep in 120 s");
        println!(
            "given {} icon {:?} pressed {:?} radar {:?} ping {:?} line {}",
            out.given,
            out.icon,
            out.pressed,
            out.radar,
            out.pinged,
            out.line_s.len()
        );
        assert!(out.given, "the two never spawned");
        assert!(out.icon.is_some(), "the dpad never drew the UAV");
        assert!(out.pressed.is_some(), "action slot 4 selected nothing");
        assert!(out.radar.is_some(), "radarEnabled never came");
        assert!(out.pinged.is_some(), "the compass never pinged the enemy");
        // The line's texture slides one way across the compass as the
        // sweep goes east (`compassRadarUpdateTime` 4 s a pass).
        let moved: Vec<f32> = out.line_s.windows(2).map(|w| w[1] - w[0]).collect();
        assert!(
            moved.iter().filter(|d| **d < 0.0).count() > moved.len() * 8 / 10,
            "{:?}",
            out.line_s
        );
    }

    /// Two clients on vcod-server's war, both on allies: the teammate's
    /// stock `pingPlayer` (the quick messages' call) sets his `eFlags`
    /// 0x400000 for 3 s and our compass flashes `objective_friendly_chat`
    /// over his arrow; then he dies, his body leaves our snapshots and our
    /// compass keeps his arrow where he fell for 1.5 s
    /// (`iw3mp.exe` 0x429880, cod4-hud.md 15.9).
    #[test]
    fn cod4_live_compass_flashes_a_pinged_friend_and_keeps_him_when_he_falls() {
        const GSC: &str = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../cod4/tests/fixtures/gsc/compass_friend"
        );
        const PATH: &str = "maps/mp/mp_crash_fx";
        #[derive(Debug, Default)]
        struct Seen {
            pinged: Option<i32>,
            flag: Option<i32>,
            chat: Option<i32>,
            flag_off: Option<i32>,
            killed: Option<i32>,
            gone: Option<i32>,
            back: Option<i32>,
            last_arrow: Option<i32>,
        }
        let mut seen = Seen::default();
        let out = live_war_pair_on(
            ["allies", "allies"],
            ["assault_mp,0", "assault_mp,0"],
            Some(GSC),
            move |f| {
                let arrow = f.hud.texture_of("compassping_friendly_mp")?;
                let chat = f.hud.texture_of("objective_friendly_chat")?;
                let t = f.level;
                if !f.both && seen.killed.is_none() || t < 3000 {
                    return None;
                }
                let snap = f.s.client().snapshots.last()?;
                let body = snap
                    .entities
                    .iter()
                    .find(|e| e.etype() == ET_PLAYER && ent_u32(e, "clientNum") == 1);
                let flagged = body.is_some_and(|e| ent_u32(e, "lerp.eFlags") & 0x40_0000 != 0);
                let has = |tex: &str| f.quads.iter().any(|q| q.texture == tex);
                if seen.pinged.is_none() {
                    seen.pinged = Some(t);
                    f.tx.send((1, PATH, "ping", "")).ok();
                }
                if flagged && seen.flag.is_none() {
                    seen.flag = Some(t);
                }
                if seen.flag.is_some() && !flagged && seen.flag_off.is_none() {
                    seen.flag_off = Some(t);
                }
                if has(&chat) && seen.chat.is_none() {
                    seen.chat = Some(t);
                }
                // Once the flash is over, he dies.
                if seen.flag_off.is_some() && seen.killed.is_none() {
                    seen.killed = Some(t);
                    f.tx.send((1, PATH, "die", "")).ok();
                }
                if seen.killed.is_some() && body.is_none() && seen.gone.is_none() {
                    seen.gone = Some(t);
                }
                if seen.gone.is_some() && body.is_some() && seen.back.is_none() {
                    seen.back = Some(t);
                }
                if seen.gone.is_some() && seen.back.is_none() && has(&arrow) {
                    seen.last_arrow = Some(t);
                }
                let done = seen.gone.is_some_and(|g| t > g + 2500);
                done.then(|| std::mem::take(&mut seen))
            },
        );
        let Some(out) = out else {
            return;
        };
        let s = out.expect("no ping, death and 2.5 s after it in 120 s");
        println!("{s:?}");
        let (flag, off) = (s.flag.expect("no 0x400000"), s.flag_off.unwrap());
        assert!(
            (2900..=3200).contains(&(off - flag)),
            "the flag held {} ms",
            off - flag
        );
        let chat = s.chat.expect("the compass never flashed the chat icon");
        assert!(chat >= flag && chat < off, "{s:?}");
        let (gone, last) = (
            s.gone.unwrap(),
            s.last_arrow.expect("no arrow once he fell"),
        );
        assert!(
            (1300..=1600).contains(&(last - gone)),
            "the arrow stayed {} ms",
            last - gone
        );
    }

    /// [`live_war_pair`]: the enemy kills us through the stock damage
    /// callback (`gsc/killcam`), and war's `giveTeamScore` sends `G 10`.
    /// The bar menus' `team( score )` reads it at once, with no `score`
    /// reply: `cg.teamScores` takes `G` and `H` as well as `b`. Read off
    /// `b` alone, the top `scorebar` said "Tied 0 - 0" through a match
    /// until Tab asked for the board (cod4-hud.md 23).
    #[test]
    fn cod4_live_a_kill_moves_the_team_scores_without_the_board() {
        const GSC: &str = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../cod4/tests/fixtures/gsc/killcam"
        );
        let mut sent = None;
        let Some(out) = live_war_pair_in(["assault_mp,0", "assault_mp,0"], Some(GSC), move |f| {
            if !f.both || f.level < 9000 {
                return None;
            }
            let Some(at) = sent else {
                f.tx.send((0, "maps/mp/mp_crash_fx", "killedBy", "enemy"))
                    .ok();
                sent = Some(f.level);
                return None;
            };
            let now = f.level;
            let scores = f.hud.menu_game(f.s, now).team_scores;
            let bars = f.hud.zone_menu_texts(f.s, "scorebars", f.shell, now);
            let asked = f.hud.counts.scores;
            (scores[1] == 10 || now > at + 3000).then_some((scores, bars, asked))
        }) else {
            return;
        };
        let (scores, bars, asked) = out.expect("the kill never happened");
        assert_eq!(asked, 0, "a `score` reply came in: the gate needs none");
        assert_eq!(scores, [0, 10, 0, 0], "axis's kill never reached the bars");
        assert!(bars.iter().any(|t| t.trim() == "10"), "{bars:?}");
    }

    /// [`live_war_pair`] with both on allies: the teammate dies
    /// (`suicide`), and the stock `_deathicons::addDeathIcon` raises a
    /// `headicon_dead` waypoint at the clone's `origin` plus 54, which we
    /// draw over his corpse once turned to it. A corpse whose `origin` read `(0, 0, 0)` put
    /// it in the middle of the map on every death (cod4-hud.md 16.4).
    #[test]
    fn cod4_live_a_teammates_death_icon_sits_over_his_corpse() {
        const GSC: &str = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../cod4/tests/fixtures/gsc/compass_friend"
        );
        let mut died: Option<(i32, Vec3)> = None;
        let Some(out) = live_war_pair_on(
            ["allies", "allies"],
            ["assault_mp,0", "assault_mp,0"],
            Some(GSC),
            move |f| {
                let t = f.level;
                if !f.both || t < 4000 {
                    return None;
                }
                let Some((k, at)) = died else {
                    let ps = &f.e.client().snapshots.last()?.ps;
                    died = Some((t, vec3(|k| ps_f32(ps, &format!("origin[{k}]")))));
                    f.tx.send((1, "maps/mp/mp_crash_fx", "die", "")).ok();
                    return None;
                };
                let snap = f.s.client().snapshots.last()?;
                let icon = vcod_cod4::hud::hud_elems(&snap.ps).into_iter().find(|e| {
                    e.kind == vcod_cod4::hud::kind::WAYPOINT
                        && f.hud
                            .configstring(f.s, vcod_cod4::hud::CS_MATERIALS + e.material as u16)
                            == "headicon_dead"
                });
                if t > k + 3000 {
                    return Some(Err(format!("no death icon drawn by {t}")));
                }
                let e = icon?;
                let (yaw, pitch) = (f.angles.y.to_radians(), -f.angles.x.to_radians());
                let size = (1280.0, 960.0);
                let up = crate::camera::up_hint(yaw, pitch, 0.0);
                let fwd = crate::camera::basis(yaw, pitch).0;
                let right = fwd.cross(up).normalize();
                let view = crate::cod2_world_hud::WorldView {
                    view_proj: crate::camera::view_proj_from(
                        f.eye,
                        yaw,
                        pitch,
                        0.0,
                        crate::camera::DEFAULT_FOV_DEG,
                        size.0 / size.1,
                    ),
                    right,
                    up: right.cross(fwd),
                    size,
                };
                let drawn = f
                    .hud
                    .world_quads(f.s, &view, t)
                    .iter()
                    .any(|q| q.texture.ends_with("headicon_dead.iwi"));
                let icon = Vec3::new(e.x, e.y, e.z);
                if !drawn {
                    // Turned to it: where we look after the spawn is the
                    // spawn point's, not the test's.
                    let to = icon - f.eye;
                    f.s.view = [
                        -to.z.atan2(to.truncate().length()).to_degrees(),
                        to.y.atan2(to.x).to_degrees(),
                    ];
                    return None;
                }
                Some(Ok((icon, at, drawn)))
            },
        ) else {
            return;
        };
        let (icon, at, drawn) = out.expect("the teammate never died").unwrap();
        // The corpse comes down from where he stood onto the floor under him.
        assert!(
            (icon.truncate() - at.truncate()).length() < 2.0,
            "the icon {icon} is not over the corpse at {at}"
        );
        assert!(
            icon.z <= at.z + 54.5 && icon.z > at.z + 54.0 - 64.0,
            "the icon {icon} is not 54 over the corpse at {at}"
        );
        assert!(drawn, "the icon at {icon} is not drawn as a world sprite");
    }

    /// [`live_war_pair`]: the enemy fires one M16 round; our session's
    /// events play through a headless mixer as the client plays them
    /// (`cod4_client::play_event_cues`), and the shot's
    /// `compassping_enemyfiring` holds full alpha for the fire sound's
    /// length before its 2 s fade, where a 0 ms hold fades at once
    /// (`CG_CompassAddWeaponPingInfo`, cod4-hud.md 15.9).
    #[test]
    fn cod4_live_enemy_shot_ping_holds_for_its_fire_sound() {
        use crate::audio::{AudioOpts, AudioSystem};
        use crate::play::input::Action;
        let Some(install) = vcod_cod4::testing::cod4_install() else {
            return;
        };
        let root = install.root().to_path_buf();
        let mut av = None;
        let (mut pressed, mut fired_at, mut full_until) = (None, None, None);
        let mut faded = false;
        let out = live_war_pair(move |f| {
            let frozen = |s: &Cod4Session| {
                s.client()
                    .snapshots
                    .last()
                    .and_then(|x| x.ps.get("pm_flags"))
                    .is_none_or(|p| p & 0x800 != 0)
            };
            let (audio, fs, sounds) = av.get_or_insert_with(|| {
                let ((mut sounds, _), files) = crate::cod4_fly::load_av(&root, "mp_crash").unwrap();
                let mut fs = crate::cod4_fly::open_fs(&root, None, None).unwrap();
                for (p, w) in files {
                    fs.overlay(&p, w);
                }
                let mut a = AudioSystem::new(
                    &fs,
                    AudioOpts {
                        enabled: true,
                        volume: 1.0,
                        rate: 44100,
                    },
                );
                a.on_gamestate("mp_crash");
                a.set_cod4_aliases(sounds.take_table().unwrap());
                (a, fs, sounds)
            });
            let gs = f.s.gamestate()?;
            let weapons = weapon_list(gs);
            let cs = |k: u16| String::from_utf8_lossy(gs.configstring(k)).into_owned();
            crate::cod4_client::play_event_cues(
                audio,
                fs,
                sounds,
                f.hud,
                f.fired,
                &weapons,
                &cs,
                &crate::cod4_audio::Placement {
                    own: None,
                    flash: &|_| None,
                },
            );
            audio.step(&HashMap::new(), &HashMap::new(), None);
            if !f.both || frozen(f.e) || frozen(f.s) {
                return None;
            }
            let t = f.level;
            // One tap of the trigger.
            match pressed {
                None => {
                    f.e.input.key(Action::Attack, true);
                    pressed = Some(t);
                }
                Some(p) if t > p + 100 => f.e.input.key(Action::Attack, false),
                _ => {}
            }
            if fired_at.is_none()
                && f.fired
                    .iter()
                    .any(|x| x.entity == Some(1) && x.name == "EV_FIRE_WEAPON")
            {
                fired_at = Some(t);
            }
            let flash = f.hud.texture_of("compassping_enemyfiring")?;
            let alpha = f
                .quads
                .iter()
                .find(|q| q.texture == flash)
                .map(|q| q.rgba[3]);
            if let (Some(at), Some(a)) = (fired_at, alpha) {
                if a >= 0.999 && !faded {
                    full_until = Some(t - at);
                } else if a < 0.999 {
                    faded = true;
                }
            }
            (faded && fired_at.is_some_and(|at| t > at + 500)).then_some(full_until)
        });
        let Some(out) = out else {
            return;
        };
        let held = out
            .expect("no shot and fade in 120 s")
            .expect("no full-alpha flash");
        println!("the flash held {held} ms");
        assert!((150..5000).contains(&held), "held {held} ms");
    }

    /// [`live_war_pair`]: the enemy fires his M16 (unsilenced) with no UAV
    /// up and our compass flashes `compassping_enemyfiring` where he
    /// stands, never the sweep's ping; then he calls in the stock
    /// helicopter (`triggerHardpoint("helicopter_mp")`) and our compass
    /// draws the enemy's `compass_objpoint_helicopter_busy`
    /// (`CG_CompassAddWeaponPingInfo` and the vehicles' draw,
    /// cod4-hud.md 15.7).
    #[test]
    fn cod4_live_compass_flashes_the_enemys_shots_and_draws_his_helicopter() {
        use crate::play::input::Action;
        let t0 = clk_now();
        let (mut firing, mut swept, mut held, mut called) = (None, 0, false, false);
        let Some(out) = live_war_pair(move |f| {
            let tex = |m: &str| f.hud.texture_of(m).unwrap_or_else(|| panic!("{m}"));
            let (flash, ping, heli) = (
                tex("compassping_enemyfiring"),
                tex("compassping_enemy"),
                tex("compass_objpoint_helicopter_busy"),
            );
            let frozen = |s: &Cod4Session| {
                s.client()
                    .snapshots
                    .last()
                    .and_then(|x| x.ps.get("pm_flags"))
                    .is_none_or(|p| p & 0x800 != 0)
            };
            if !f.both || frozen(f.e) || frozen(f.s) {
                return None;
            }
            // Bursts of a third of a second, a second apart.
            let want = firing.is_none() && since(t0).as_millis() % 1000 < 330;
            if want != held {
                f.e.input.key(Action::Attack, want);
                held = want;
            }
            swept += f.quads.iter().filter(|q| q.texture == ping).count();
            if firing.is_none() && f.quads.iter().any(|q| q.texture == flash) {
                firing = Some(since(t0));
            }
            if firing.is_some() && !called {
                called = true;
                f.tx.send((
                    1,
                    "maps/mp/gametypes/_hardpoints",
                    "triggerHardpoint",
                    "helicopter_mp",
                ))
                .ok();
            }
            // The helicopter's icon is 40 square: twice a ping's width.
            let wide = |q: &crate::hud::HudQuad| {
                let (a, b) = (q.verts[0], q.verts[1]);
                ((b[0] - a[0]).powi(2) + (b[1] - a[1]).powi(2)).sqrt()
            };
            let pings: Vec<f32> = f
                .quads
                .iter()
                .filter(|q| q.texture == flash)
                .map(wide)
                .collect();
            f.quads
                .iter()
                .find(|q| q.texture == heli)
                .map(|q| (firing, swept, wide(q), pings.first().copied()))
        }) else {
            return;
        };
        let (firing, swept, heli, flash) = out.expect("no helicopter on the compass in 120 s");
        println!("firing {firing:?} sweep pings {swept} helicopter {heli} flash {flash:?}");
        assert!(firing.is_some(), "no shot flashed on the compass");
        // The enemies' draw shows the radar ping too at
        // `1 - cg.time / 4000` with no sweep yet: only in a level's
        // first 4 s.
        assert!(swept < 10, "{swept} sweep pings with no UAV up");
        if let Some(flash) = flash {
            assert!((heli / flash - 40.0 / 18.75).abs() < 0.01, "{heli} {flash}");
        }
    }

    /// Two clients on vcod-server's war: us on allies with the stock
    /// `offline_class2_mp` (Bomb Squad, UAV Jammer, Dead Silence:
    /// `mp/offline_classTable.csv`), an enemy on axis with Assault, both
    /// walking. Our steps play the quiet `qstep_*_plr_*` set and the
    /// enemy's own the plain one, each off the client state's perks; the
    /// stock `_weapons::claymoreDetectionTrigger("axis")` laid where we
    /// stand makes the stock `showHeadIcon` raise our `waypoint_bombsquad`
    /// waypoint, which draws in the world ahead once we back off
    /// (cod4-hud.md 16, cod4-sound.md 4).
    #[test]
    fn cod4_live_dead_silence_steps_quiet_and_bomb_squad_shows_the_trigger() {
        use crate::play::input::Action;
        use vcod_server::cod4::{Cod4Config, Cod4Server};
        let Some(install) = vcod_cod4::testing::cod4_install() else {
            return;
        };
        let world = crash_world();
        let mut sv = Cod4Server::new(
            Cod4Config {
                map: "mp_crash".into(),
                gametype: "war".into(),
                hostname: "vcod".into(),
                max_clients: 4,
            },
            7,
        );
        sv.set_cvar("scr_game_playerwaittime", "1");
        sv.set_cvar("scr_game_matchstarttime", "1");
        sv.load(Some(&install)).unwrap();
        let sock = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
        sock.set_nonblocking(true).unwrap();
        let addr = sock.local_addr().unwrap();
        let (tx, rx) = std::sync::mpsc::channel::<usize>();
        let (ready_tx, ready_rx) = std::sync::mpsc::channel::<()>();
        let main = install.root().join("main");
        let root = install.root().to_path_buf();
        let clock = sim::Clock::new();
        let client = sim::spawn(&clock, move || {
            // The sounds and the HUD before the level's clock runs: a
            // loaded machine takes long enough over them to spawn us past
            // the grace period, where the class change waits a respawn.
            let ((mut sounds, _), _) = crate::cod4_fly::load_av(&root, "mp_crash").unwrap();
            let mut hud = crate::cod4_hud::Cod4Hud::default();
            hud.load(&main, "mp_crash");
            ready_tx.send(()).unwrap();
            let join = |team: &str, class: &str| MenuJoin {
                team: Some(team.into()),
                class: Some(class.into()),
            };
            let us = join("allies", "assault_mp,0");
            let mut s = Cod4Session::connect(addr, "joiner", Cp::Bare, us).unwrap();
            let them = join("axis", "assault_mp,0");
            let mut e = Cod4Session::connect(addr, "enemy", Cp::Bare, them).unwrap();
            if let Some(w) = world {
                s.set_world(w.clone());
                e.set_world(w);
            }
            let icon = hud
                .texture_of("waypoint_bombsquad")
                .expect("waypoint_bombsquad's image");
            let shell = crate::console::shell::Shell::new();
            // Assault's two perks, the third its `specialty_null`.
            let spawn_icons: Vec<String> = ["specialty_bulletdamage", "specialty_longersprint"]
                .iter()
                .map(|p| hud.texture_of(p).expect("the perk icon"))
                .collect();
            let mut shown = [false; 2];
            let frozen = |s: &Cod4Session| {
                s.client()
                    .snapshots
                    .last()
                    .and_then(|x| x.ps.get("pm_flags"))
                    .is_some_and(|f| f & 0x800 != 0)
            };
            let t0 = clk_now();
            let (mut ours, mut theirs) = (HashSet::new(), HashSet::new());
            let (mut perks, mut chose, mut laid, mut walked, mut drawn) =
                (0, false, false, None, None);
            let mut up_at = None;
            let backed =
                |w: Option<Instant>| w.is_some_and(|w| since(w) > Duration::from_millis(1500));
            while since(t0) < Duration::from_secs(120)
                && !(drawn.is_some() && backed(walked) && !ours.is_empty() && !theirs.is_empty())
            {
                let now = since(t0).as_secs_f64() * 1000.0;
                for c in [&mut s, &mut e] {
                    c.step(now).unwrap();
                    c.take_commands();
                }
                let up = s.spawn().is_some() && e.spawn().is_some() && !frozen(&s) && !frozen(&e);
                let level = s.client().snapshots.last().map_or(0, |x| x.server_time);
                if up {
                    up_at.get_or_insert(level);
                }
                // The class menu's response for the offline class, inside
                // the grace period: `menuClass` gives the loadout at once.
                // Not before the spawn's Assault icons drew (or a second
                // of the server's clock went by without them): a loaded
                // machine can spawn us past the prematch, unfrozen at once.
                let icons_seen = shown == [true; 2] || up_at.is_some_and(|t| level > t + 1000);
                if up && !chose && icons_seen {
                    chose = true;
                    let gs = s.gamestate().unwrap();
                    let menu = crate::play::cod4_menus::menu_slot(gs, "changeclass").unwrap();
                    let sid = gs.systeminfo("sv_serverid").unwrap();
                    s.command(&format!("mr {sid} {menu} offline_class2_mp,0"));
                }
                if up && !laid && perks & 0x100 != 0 {
                    laid = true;
                    tx.send(0).ok();
                    walked = Some(clk_now());
                    s.input.key(Action::Back, true);
                    e.input.key(Action::Back, true);
                }
                // A second back from the trigger, then stand.
                if walked.is_some_and(|w| since(w) > Duration::from_millis(1200)) {
                    s.input.key(Action::Back, false);
                    e.input.key(Action::Back, false);
                }
                // The enemy's predicted ring fires in its scene.
                e.scene();
                for (c, heard) in [(&mut s, &mut ours), (&mut e, &mut theirs)] {
                    let fired = c.take_fired();
                    let Some(gs) = c.gamestate() else { continue };
                    let weapons = weapon_list(gs);
                    let cs = |k: u16| String::from_utf8_lossy(gs.configstring(k)).into_owned();
                    for f in fired.iter().filter(|f| f.entity.is_none()) {
                        heard.extend(sounds.cues(f, &weapons, &cs).into_iter().map(|c| c.alias));
                    }
                }
                if let Some(snap) = s.client().snapshots.last() {
                    let own = s.gamestate().map_or(0, |g| g.client_num as u32);
                    perks = client_perks(snap, own);
                }
                let Some(scene) = s.scene() else {
                    clk_sleep(Duration::from_millis(16));
                    continue;
                };
                hud.update(&mut s, &[], scene.server_time);
                // The spawn's perk icons (`_globallogic::spawnPlayer`'s
                // `showPerk`, hudelems at the bottom right for 3 s).
                let flat = hud.build(&s, (1280.0, 960.0), scene.render_time, None, &shell);
                for (k, t) in spawn_icons.iter().enumerate() {
                    shown[k] |= flat.iter().any(|q| q.texture == *t);
                }
                let (yaw, pitch) = (scene.angles.y.to_radians(), -scene.angles.x.to_radians());
                let size = (1280.0, 960.0);
                let view_proj = crate::camera::view_proj_from(
                    scene.eye,
                    yaw,
                    pitch,
                    0.0,
                    crate::camera::DEFAULT_FOV_DEG,
                    size.0 / size.1,
                );
                let up = crate::camera::up_hint(yaw, pitch, 0.0);
                let fwd = crate::camera::basis(yaw, pitch).0;
                let right = fwd.cross(up).normalize();
                let view = crate::cod2_world_hud::WorldView {
                    view_proj,
                    right,
                    up: right.cross(fwd),
                    size,
                };
                let quads = hud.world_quads(&s, &view, scene.render_time);
                // Backed off: the icon over the trigger is ahead, on screen.
                if backed(walked) {
                    drawn = quads
                        .into_iter()
                        .find(|q| q.texture == icon)
                        .map(|q| q.verts);
                }
                clk_sleep(Duration::from_millis(16));
            }
            s.disconnect();
            e.disconnect();
            let missing: Vec<String> = sounds.missing.iter().cloned().collect();
            (perks, laid, ours, theirs, drawn, missing, shown)
        });
        ready_rx.recv().unwrap();
        serve(&mut sv, &sock, &clock, &client, |sv, _, _| {
            if let Ok(slot) = rx.try_recv() {
                let l = sv.game.level.as_mut().unwrap();
                l.start_player_script(
                    slot,
                    "maps/mp/gametypes/_weapons",
                    "claymoreDetectionTrigger",
                    &["axis"],
                )
                .unwrap();
            }
            true
        });
        let (perks, laid, ours, theirs, drawn, missing, shown) = client.join().unwrap();
        assert_eq!(shown, [true; 2], "the spawn's perk icons");
        println!("perks {perks:#x} ours {ours:?} theirs {theirs:?} drawn {drawn:?}");
        assert!(laid, "the two never spawned");
        // `specialty_quieter`, `specialty_gpsjammer`, `specialty_detectexplosive`.
        assert_eq!(perks & 0x901, 0x901, "{perks:#x}");
        let steps = |h: &HashSet<String>, quiet: bool| {
            h.iter()
                .filter(|a| a.contains("step_"))
                .all(|a| a.starts_with('q') == quiet)
                && h.iter().any(|a| a.contains("step_") && a.contains("_plr_"))
        };
        assert!(steps(&ours, true), "{ours:?}");
        assert!(steps(&theirs, false), "{theirs:?}");
        assert!(!missing.iter().any(|m| m.starts_with('q')), "{missing:?}");
        let v = drawn.expect("the bomb squad waypoint never drew");
        let centre = [(v[0][0] + v[2][0]) / 2.0, (v[0][1] + v[2][1]) / 2.0];
        assert!(
            (0.0..1280.0).contains(&centre[0]) && (0.0..960.0).contains(&centre[1]),
            "{v:?}"
        );
    }

    /// A watched player's lean and move direction step between snapshots
    /// as its angles do, the short way round, rather than jumping at
    /// each snapshot (a choppy lean seen in play).
    #[test]
    fn lean_and_move_direction_lerp_between_snapshots() {
        let near = |a: f32, b: f32| (a - b).abs() < 1e-4;
        assert!(near(lerp_wrapped(0.25, 0.75, 0.5), 0.5));
        assert!(near(lerp_wrapped(-0.5, 0.0, 0.25), -0.375));
        assert!(near(lerp_wrapped(170.0, -170.0, 0.5), 180.0));
        assert!(near(lerp_wrapped(-45.0, 45.0, 1.0), 45.0));
    }

    /// The server gone quiet (Ctrl+C with no `w` sent): the cmds the
    /// server has not run pile up past the 128 kept and `Connection
    /// Interrupted` draws, then `cl_timeout` (cut to 6 s here) of silence
    /// drops the session with `EXE_ERR_SERVER_TIMEOUT`. The interrupt
    /// counts cmds (one a frame) and the timeout wall time, so the first
    /// 128 frames run 2 ms apart, as a fast client's would, then a 16 ms
    /// grid: a loaded machine's slow frames cannot push the 128th past
    /// the timeout.
    #[test]
    fn cod4_live_silence_interrupts_then_times_out() {
        const TIMEOUT: Duration = Duration::from_secs(6);
        let Some((interrupted, dropped)) = live_join_ctl(|s, spawned, t0, tx| {
            spawned.as_ref()?;
            s.timeout = TIMEOUT;
            tx.send(Ctl::Silence).ok()?;
            let quiet = clk_now();
            let mut interrupted = None;
            let mut grid = FrameClock::new();
            let mut frame = 0u32;
            while since(quiet) < TIMEOUT * 5 {
                s.step(since(t0).as_secs_f64() * 1000.0).unwrap();
                s.take_commands();
                s.scene();
                s.take_fired();
                frame += 1;
                if interrupted.is_none() && s.interrupted() {
                    interrupted = Some((frame, since(quiet)));
                }
                if let Some(why) = s.take_dropped() {
                    return Some((interrupted, (why, frame, since(quiet))));
                }
                if frame < 128 {
                    clk_sleep(Duration::from_millis(2));
                    grid = FrameClock::new();
                } else {
                    grid.wait();
                }
            }
            Some((interrupted, (String::new(), frame, since(quiet))))
        })
        .flatten() else {
            return;
        };
        println!("interrupted {interrupted:?}, dropped {dropped:?}");
        let (at, when) = interrupted.expect("Connection Interrupted drew");
        assert_eq!(dropped.0, "EXE_ERR_SERVER_TIMEOUT");
        assert!(dropped.1 > at && when < dropped.2, "{at} {dropped:?}");
        assert!(dropped.2 >= TIMEOUT, "{:?}", dropped.2);
    }

    /// A `w` from the server (its `quit`, a kick) drops the session with
    /// its reason, as `CL_DisconnectError` words it.
    #[test]
    fn cod4_live_w_drops_with_its_reason() {
        let Some(why) = live_join_ctl(|s, spawned, t0, tx| {
            spawned.as_ref()?;
            tx.send(Ctl::Command("w \"EXE_SERVERQUIT\"".into())).ok()?;
            let sent = clk_now();
            while since(sent) < Duration::from_secs(3) {
                s.step(since(t0).as_secs_f64() * 1000.0).unwrap();
                s.take_commands();
                if let Some(why) = s.take_dropped() {
                    return Some(why);
                }
                clk_sleep(Duration::from_millis(16));
            }
            Some(String::new())
        })
        .flatten() else {
            return;
        };
        assert_eq!(why, "EXE_SERVERDISCONNECTREASON\u{15}EXE_SERVERQUIT");
        let Some(install) = vcod_cod4::testing::cod4_install() else {
            return;
        };
        let text = localize_drop(&why, install.root());
        assert!(!text.contains("EXE_") && !text.contains("&&1"), "{text}");
    }

    /// Holding the mouse past `player_view_pitch_down` (85) or `_up` keeps
    /// the drawn view still at the limit: the mouse turns the cmd's view
    /// ([`Cod4Session::turn`], `cl.viewangles`), the camera draws the
    /// predicted view, and the server's `delta_angles` pushes take the
    /// excess back. In play the view jumped between 85 and the
    /// camera's own 89 at each push. Turning back moves the view at once
    /// and only one way.
    /// A live session sheds its old snapshots (the soak's leak: every
    /// snapshot was kept, 100 MB a match) and plays on: the clock reads
    /// each new snapshot, the scene draws and our own shots still fire
    /// their events after the sheds.
    #[test]
    fn cod4_live_session_sheds_old_snapshots_and_plays_on() {
        use crate::play::input::Action;
        let Some(out) = live_join(|s, spawned, t0| {
            spawned.as_ref()?;
            // The prematch freezes the weapon (`PMF_FROZEN` 0x800).
            let frozen = |s: &Cod4Session| {
                s.client()
                    .snapshots
                    .last()
                    .and_then(|x| x.ps.get("pm_flags"))
                    .is_some_and(|f| f & 0x800 != 0)
            };
            let wait = clk_now();
            while frozen(s) && since(wait) < Duration::from_secs(30) {
                s.step(since(t0).as_secs_f64() * 1000.0).unwrap();
                s.take_commands();
                s.take_fired();
                clk_sleep(Duration::from_millis(16));
            }
            s.keep_snapshots = 40;
            let (mut fired_late, mut scenes, mut max_len) = (0, 0, 0);
            let (mut last_time, mut stuck, mut held) = (0, 0, false);
            let start = s.client().total_snapshots();
            let t1 = clk_now();
            while since(t1) < Duration::from_secs(30) && s.client().total_snapshots() < start + 240
            {
                let want = since(t1).as_millis() % 1000 < 300;
                if want != held {
                    s.input.key(Action::Attack, want);
                    held = want;
                }
                s.step(since(t0).as_secs_f64() * 1000.0).unwrap();
                s.take_commands();
                let shed = s.client().shed_count > 0;
                fired_late += s
                    .take_fired()
                    .iter()
                    .filter(|f| shed && f.name.starts_with("EV_FIRE_WEAPON"))
                    .count();
                max_len = max_len.max(s.client().snapshots.len());
                let scene = s.scene();
                if shed && let Some(scene) = scene {
                    scenes += 1;
                    // The drawn time never runs back or sticks across a
                    // shed: the clock still reads each new snapshot.
                    assert!(scene.server_time >= last_time, "time ran back");
                    stuck = if scene.server_time == last_time {
                        stuck + 1
                    } else {
                        0
                    };
                    assert!(stuck < 60, "the drawn time stuck");
                    last_time = scene.server_time;
                }
                clk_sleep(Duration::from_millis(16));
            }
            let c = s.client();
            Some((
                c.total_snapshots() - start,
                c.shed_count,
                max_len,
                fired_late,
                scenes,
            ))
        }) else {
            return;
        };
        let (got, shed, max_len, fired_late, scenes) = out.expect("spawned");
        assert!(got >= 200, "{got} snapshots");
        assert!(shed + 80 >= got, "{shed} of {got} shed");
        assert!(max_len < 80, "{max_len} snapshots held");
        assert!(fired_late > 0, "no shot fired after a shed");
        assert!(scenes > 100, "{scenes} scenes");
    }

    #[test]
    fn cod4_live_pitch_held_past_the_limit_stays_at_the_limit() {
        let Some(out) = live_join(|s, spawned, t0| {
            spawned.as_ref()?;
            // (mouse degrees a frame, frames) phases, about 2, 0.5 and
            // 2.5 s at 60 Hz: counted in frames, as a loaded machine
            // stretches each; the drawn pitch of each.
            let phases = [(3.0f32, 125u32), (-0.5, 31), (-3.0, 156)];
            let mut drawn = vec![Vec::new(); phases.len()];
            let mut held = Vec::new();
            let mut jumps = 0;
            for (k, &(mouse, frames)) in phases.iter().enumerate() {
                if k == 1 {
                    // Still until the server has run every held cmd, so
                    // its last push has taken all the excess back: the
                    // excess a round trip leaves grows with the lag, and a
                    // starved server's would outlast the slow turn back.
                    let last = s.sent_cmds.back().map_or(0, |c| c.server_time);
                    let run = |s: &Cod4Session| {
                        s.client()
                            .snapshots
                            .last()
                            .is_some_and(|x| x.ps.get("commandTime").unwrap_or(0) as i32 >= last)
                    };
                    let wait = clk_now();
                    while !run(s) && since(wait) < Duration::from_secs(30) {
                        s.turn([0.0, 0.0]);
                        s.step(since(t0).as_secs_f64() * 1000.0).unwrap();
                        s.take_commands();
                        jumps += usize::from(s.take_view_jump());
                        if let Some(scene) = s.scene() {
                            held.push(scene.angles.x);
                        }
                        clk_sleep(Duration::from_millis(16));
                    }
                }
                for _ in 0..frames {
                    s.turn([mouse, 0.0]);
                    s.step(since(t0).as_secs_f64() * 1000.0).unwrap();
                    s.take_commands();
                    jumps += usize::from(s.take_view_jump());
                    if let Some(scene) = s.scene() {
                        drawn[k].push(scene.angles.x);
                    }
                    clk_sleep(Duration::from_millis(16));
                }
            }
            Some((drawn, held, jumps))
        })
        .flatten() else {
            return;
        };
        let (drawn, held, jumps) = out;
        println!(
            "pitch: {:?} frames, {jumps} jumps",
            drawn.iter().map(Vec::len).collect::<Vec<_>>()
        );
        assert!(jumps > 0, "the server never pushed the pitch");
        // Held down: once at the limit the view never leaves it.
        let at = drawn[0]
            .iter()
            .position(|&p| p >= 84.99)
            .expect("never reached 85");
        assert!(drawn[0][at..].len() > 60, "{}", drawn[0].len());
        assert!(
            drawn[0][at..].iter().all(|&p| (84.99..=85.01).contains(&p)),
            "{:?}",
            drawn[0]
        );
        assert!(
            held.iter().all(|&p| (84.99..=85.01).contains(&p)),
            "{held:?}"
        );
        // Turned back slowly: the view comes off the limit and never
        // steps back toward it.
        let back = &drawn[1];
        assert!(back.last().is_some_and(|&p| p < 84.0), "{back:?}");
        assert!(back.windows(2).all(|w| w[1] <= w[0] + 0.01), "{back:?}");
        // Held up: the same at -85.
        let up = &drawn[2];
        let at = up
            .iter()
            .position(|&p| p <= -84.99)
            .expect("never reached -85");
        assert!(up[at..].len() > 60, "{}", up.len());
        assert!(
            up[at..].iter().all(|&p| (-85.01..=-84.99).contains(&p)),
            "{up:?}"
        );
    }

    /// The live join answers the team menu and the class menu
    /// `changeclass_marines` opens, and spawns playing with the class's
    /// weapon, its cmds carrying that weapon as its own pick.
    #[test]
    fn cod4_live_join_picks_a_team_and_a_class_and_spawns() {
        let Some((spawned, opened, ps, sent)) = live_join(|s, spawned, t0| {
            // A second of play: the cmds go on carrying the pick.
            let t1 = clk_now();
            while spawned.is_some() && since(t1) < Duration::from_secs(1) {
                s.step(since(t0).as_secs_f64() * 1000.0).unwrap();
                clk_sleep(Duration::from_millis(16));
            }
            let ps = s.client().snapshots.last().map(|x| x.ps.clone());
            let sent = s.sent_cmds.back().copied();
            (spawned, s.menus.opened.clone(), ps, sent)
        }) else {
            return;
        };
        assert_eq!(opened[..2], ["team_marinesopfor", "changeclass_marines"]);
        let spawned = spawned.expect("no spawn");
        assert!(spawned.contains("m16_gl_mp"), "{spawned}");
        let ps = ps.unwrap();
        let weapon = ps.get("weapon").unwrap();
        assert_eq!(sent.map(|c| u32::from(c.weapon)), Some(weapon));
    }

    /// A live session on vcod-server fires its own rounds into mp_crash's
    /// ground (cod4-effects.md 12): `+attack` held, each fire event the server
    /// sends back is traced client-side from the drawn eye, strikes the
    /// world and resolves an impact effect and an impact sound; the
    /// tracer rolls run on the same hits. Waits out the prematch freeze
    /// first.
    #[test]
    fn cod4_live_own_shots_strike_the_world() {
        use crate::cod4_fx::OwnView;
        use crate::play::input::Action;
        let Some(out) = live_join(|s, spawned, t0| {
            spawned.as_ref()?;
            let install = vcod_cod4::testing::cod4_install()?;
            let ((mut sounds, mut effects), _) =
                crate::cod4_fly::load_av(install.root(), "mp_crash").ok()?;
            let world = s.world()?.clone();
            let mut fx = crate::fx::sim::FxSystem::new();
            let (mut fires, mut hits, mut impacts, mut cues) = (0, Vec::new(), 0, 0);
            let mut further = Vec::new();
            // The prematch freezes the player and his weapon
            // (`PMF_FROZEN` 0x800) for its first seconds.
            let frozen = |s: &Cod4Session| {
                s.client()
                    .snapshots
                    .last()
                    .and_then(|x| x.ps.get("pm_flags"))
                    .is_some_and(|f| f & 0x800 != 0)
            };
            let wait = clk_now();
            while frozen(s) && since(wait) < Duration::from_secs(30) {
                s.step(since(t0).as_secs_f64() * 1000.0).unwrap();
                s.take_commands();
                s.take_fired();
                clk_sleep(Duration::from_millis(16));
            }
            // Down at the ground ahead: the spawn faces the map's caulk
            // edge, whose surface (0) the impact table leaves empty.
            s.view[0] = 45.0;
            let t1 = clk_now();
            let mut held = false;
            while since(t1) < Duration::from_millis(2500) {
                // Two bursts: held 0.3 s to 1 s and 1.4 s to 2 s.
                let ms = since(t1).as_millis();
                let want = (300..1000).contains(&ms) || (1400..2000).contains(&ms);
                if want != held {
                    s.input.key(Action::Attack, want);
                    held = want;
                }
                s.step(since(t0).as_secs_f64() * 1000.0).unwrap();
                s.take_commands();
                let fired = s.take_fired();
                let Some(scene) = s.scene() else { continue };
                let weapons = weapon_list(s.gamestate().unwrap());
                let own = OwnView {
                    client: Some(scene.view.client_num),
                    eye: scene.eye,
                    angles: scene.angles,
                    flash: None,
                };
                for f in fired
                    .iter()
                    .filter(|f| f.name.starts_with("EV_FIRE_WEAPON"))
                {
                    fires += 1;
                    let defs = sounds.weapons.clone();
                    let Some(hit) = first_bullet(
                        f,
                        &weapons,
                        &defs,
                        &HashMap::new(),
                        Some(&own),
                        &world.clip,
                        &s.shot_solids,
                    ) else {
                        continue;
                    };
                    effects.tracers(&mut fx, std::slice::from_ref(&hit), 0.0, 16);
                    impacts += effects.bullet_plays(f, &weapons, &hit).len();
                    cues += sounds.bullet_cues(f, &weapons, &hit, None).len();
                    hits.push(((hit.end - hit.start).length(), hit.surf, hit.own));
                    further.extend(hit.pierced.iter().flatten().map(|p| p.surf));
                }
                clk_sleep(Duration::from_millis(16));
            }
            s.input.key(Action::Attack, false);
            Some((
                fires,
                hits,
                impacts,
                cues,
                effects.tracers,
                effects.missing.clone(),
                further,
            ))
        })
        .flatten() else {
            return;
        };
        let (fires, hits, impacts, cues, tracers, missing, further) = out;
        println!(
            "live: {fires} fire events, hits {hits:?}, {impacts} impacts, {cues} cues, {tracers} tracers, further {further:?}"
        );
        assert!(fires >= 3, "{fires}");
        assert_eq!(hits.len(), fires);
        assert!(
            hits.iter().all(|h| h.2 && h.0 < 400.0 && h.1 != 0),
            "{hits:?}"
        );
        // A round passes the rock's one-sided terrain (cod4-collision.md 3)
        // and meets the caulk floor under it: a sound, no effect.
        let bare = further.iter().filter(|&&s| s == 0).count();
        assert_eq!(
            (impacts, cues),
            (fires + further.len() - bare, fires + further.len()),
            "{missing:?}"
        );
    }

    /// Our own M16 rounds on vcod-server through the spawn's wall
    /// (the penetration walk, cod4-effects.md 12.1): turned east, each
    /// round's first contact is the 8-unit concrete wall ahead, its far
    /// side plays the large exit effect and sound, and the round goes on
    /// to the next surface, which plays its own impact.
    #[test]
    fn cod4_live_own_rounds_pierce_the_wall_and_play_its_far_side() {
        use crate::cod4_fx::OwnView;
        use crate::play::input::Action;
        let Some(out) = live_join(|s, spawned, t0| {
            spawned.as_ref()?;
            let install = vcod_cod4::testing::cod4_install()?;
            let ((mut sounds, mut effects), _) =
                crate::cod4_fly::load_av(install.root(), "mp_crash").ok()?;
            let world = s.world()?.clone();
            let frozen = |s: &Cod4Session| {
                s.client()
                    .snapshots
                    .last()
                    .and_then(|x| x.ps.get("pm_flags"))
                    .is_some_and(|f| f & 0x800 != 0)
            };
            let wait = clk_now();
            while frozen(s) && since(wait) < Duration::from_secs(30) {
                s.step(since(t0).as_secs_f64() * 1000.0).unwrap();
                s.take_commands();
                s.take_fired();
                clk_sleep(Duration::from_millis(16));
            }
            s.view = [0.0, 0.0];
            let (mut rounds, mut exits, mut names) = (0, 0, HashSet::new());
            let t1 = clk_now();
            while since(t1) < Duration::from_millis(1500) {
                let ms = since(t1).as_millis();
                s.input.key(Action::Attack, (300..900).contains(&ms));
                s.step(since(t0).as_secs_f64() * 1000.0).unwrap();
                s.take_commands();
                let fired = s.take_fired();
                let Some(scene) = s.scene() else { continue };
                let weapons = weapon_list(s.gamestate().unwrap());
                let own = OwnView {
                    client: Some(scene.view.client_num),
                    eye: scene.eye,
                    angles: scene.angles,
                    flash: None,
                };
                let defs = sounds.weapons.clone();
                for f in fired
                    .iter()
                    .filter(|f| f.name.starts_with("EV_FIRE_WEAPON"))
                {
                    let Some(hit) = first_bullet(
                        f,
                        &weapons,
                        &defs,
                        &HashMap::new(),
                        Some(&own),
                        &world.clip,
                        &s.shot_solids,
                    ) else {
                        continue;
                    };
                    rounds += 1;
                    exits += hit.pierced.iter().flatten().filter(|p| p.exit).count();
                    names.extend(
                        effects
                            .bullet_plays(f, &weapons, &hit)
                            .into_iter()
                            .map(|p| p.key),
                    );
                    names.extend(
                        sounds
                            .bullet_cues(f, &weapons, &hit, None)
                            .into_iter()
                            .map(|c| c.alias),
                    );
                }
                clk_sleep(Duration::from_millis(16));
            }
            s.input.key(Action::Attack, false);
            Some((
                rounds,
                exits,
                names,
                effects.missing.clone(),
                sounds.missing.clone(),
            ))
        })
        .flatten() else {
            return;
        };
        let (rounds, exits, names, fx_missing, snd_missing) = out;
        println!("pierce: {rounds} rounds, {exits} exits, {names:?}");
        assert!(rounds >= 3, "{rounds}");
        assert_eq!(exits, rounds, "{names:?}");
        assert!(names.iter().any(|n| n.ends_with("_exit.efx")), "{names:?}");
        assert!(
            names.iter().any(|n| n.starts_with("bullet_large_exit")),
            "{names:?}"
        );
        assert!(
            fx_missing.is_empty() && snd_missing.is_empty(),
            "{fx_missing:?} {snd_missing:?}"
        );
    }

    /// Our own M16 rounds on vcod-server at an axis bot kept standing
    /// 200 units ahead (cod4-effects.md 12.2): each round leaves along the
    /// direction the server's `Bullet_Fire` drew for it (the spread off
    /// the same `commandTime` seed), stops on the bot's box and plays no
    /// impact of its own; the server's `EV_BULLET_HIT` flesh events land
    /// on the bot where our rounds stopped, and they play the flesh hit.
    #[test]
    fn cod4_live_rounds_stop_on_the_bot_where_the_server_bleeds() {
        rounds_on_the_bot(false, false);
    }

    /// [`cod4_live_rounds_stop_on_the_bot_where_the_server_bleeds`] up the
    /// sight: the rounds leave along the gun's angles, the gun spring's
    /// kick in them (cod4-combat.md 3.2).
    #[test]
    fn cod4_live_sighted_rounds_stop_on_the_bot_where_the_server_bleeds() {
        rounds_on_the_bot(true, false);
    }

    /// [`cod4_live_rounds_stop_on_the_bot_where_the_server_bleeds`] with
    /// us hurt 280 ms before each burst, in turn from ahead, from the right
    /// and from no direction: the rounds leave along the damage kick the
    /// server's aim block eases in, which the predictor works out off the
    /// snapshot's `damageEvent` (cod4-combat.md 3.2).
    #[test]
    fn cod4_live_hurt_rounds_take_the_servers_damage_kick() {
        rounds_on_the_bot(false, true);
    }

    /// The melee key with an axis bot standing 100 units ahead, past
    /// `player_meleeRange` and inside `aim_automelee_range`: our cmd asks
    /// for a 100-unit lunge along the yaw to him, vcod-server's pmove runs
    /// it (`PMF_MELEE_CHARGE`) and throws us at him, the charge swing kills
    /// him and we run on past where he stood, as the retail capture's
    /// attacker does (`play-1.7-lunge.txt`), and our prediction lands where
    /// the server put us (cod4-pmove.md 19).
    #[test]
    fn cod4_live_the_melee_key_lunges_at_a_bot_in_reach() {
        use crate::play::input::Action;
        let sets = [
            ("sv_cheats", "1"),
            ("scr_game_playerwaittime", "1"),
            ("scr_game_matchstarttime", "1"),
        ];
        let Some(out) = live_join_on(
            "mp_crash",
            crash_world(),
            &sets,
            move |s, spawned, t0, tx| {
                spawned.as_ref().expect("spawned");
                let world = s.world().expect("world").clone();
                let step = |s: &mut Cod4Session, ms: u64| {
                    let end = clk_now() + Duration::from_millis(ms);
                    while clk_now() < end {
                        s.step(since(t0).as_secs_f64() * 1000.0).unwrap();
                        s.take_commands();
                        // The frame's prediction, which the pick reads.
                        s.scene();
                        clk_sleep(Duration::from_millis(16));
                    }
                };
                let flags = |s: &Cod4Session| {
                    s.client()
                        .snapshots
                        .last()
                        .and_then(|x| x.ps.get("pm_flags"))
                        .unwrap_or(0)
                };
                let wait = clk_now();
                while flags(s) & 0x800 != 0 && since(wait) < Duration::from_secs(30) {
                    step(s, 16);
                }
                let feet = |s: &Cod4Session| {
                    let ps = &s.client().snapshots.last().unwrap().ps;
                    vec3(|k| ps_f32(ps, &format!("origin[{k}]")))
                };
                let start = feet(s);
                let eye = start + Vec3::Z * 60.0;
                // A yaw along which a standing body's box crosses 150 units
                // of level floor (the bot stands at 100).
                let (lo, hi) = (Vec3::new(-16.0, -16.0, 0.0), Vec3::new(16.0, 16.0, 70.0));
                let yaw = (0..24).map(|k| k as f32 * 15.0).find(|y| {
                    let d = Vec3::new(y.to_radians().cos(), y.to_radians().sin(), 0.0);
                    let lift = Vec3::Z * 1.0;
                    let t = world.clip.trace(
                        start + lift,
                        start + lift + d * 150.0,
                        lo,
                        hi,
                        0x281_0011,
                    );
                    let level = [50.0, 100.0, 150.0].iter().all(|&r| {
                        let p = start + d * r;
                        let down = world.clip.trace(
                            p + Vec3::Z * 2.0,
                            p - Vec3::Z * 2.0,
                            Vec3::ZERO,
                            Vec3::ZERO,
                            1,
                        );
                        down.fraction < 1.0 && !down.startsolid
                    });
                    let sight =
                        world
                            .clip
                            .trace(eye, eye + d * 200.0, Vec3::ZERO, Vec3::ZERO, 0x280_6831);
                    t.fraction == 1.0 && !t.startsolid && level && sight.fraction == 1.0
                });
                let yaw = yaw.expect("an open yaw");
                let d = Vec3::new(yaw.to_radians().cos(), yaw.to_radians().sin(), 0.0);
                let at = start + d * 100.0;
                // Mortal, as retail's target is: the swing kills him and the
                // lunge runs on through where he stood.
                tx.send(Ctl::PinVictim(at.to_array())).unwrap();
                s.view = [0.0, yaw];
                step(s, 6000);
                let before = feet(s);
                s.input.key(Action::Melee, true);
                let (mut charged, mut path) = (false, Vec::new());
                let t1 = clk_now();
                while since(t1) < Duration::from_millis(1200) {
                    step(s, 16);
                    if since(t1) > Duration::from_millis(100) {
                        s.input.key(Action::Melee, false);
                    }
                    charged |= flags(s) & vcod_cod4::pmove::pmf::MELEE_CHARGE != 0;
                    path.push(feet(s));
                }
                let predicted = s.last_predicted.as_ref().expect("predicted").origin;
                (before, at, charged, path, predicted, feet(s))
            },
        ) else {
            return;
        };
        let (before, bot, charged, path, predicted, server) = out;
        println!("from {before} bot {bot} path {path:?}");
        assert!(charged, "the server never ran the charge");
        // At least the lunge's 100 units toward him (the forced forward
        // key walks on while the charge lasts).
        let toward = (server - before)
            .truncate()
            .dot((bot - before).truncate().normalize());
        assert!(toward > 100.0, "moved {toward} toward him");
        // The lunge's first snapshot moves at its speed, not the walk's.
        let first = path
            .windows(2)
            .map(|w| (w[1] - w[0]).truncate().length())
            .fold(0.0, f32::max);
        assert!(first > 15.0, "fastest snapshot step {first}");
        assert!(predicted.distance(server) < 0.5, "{predicted} {server}");
    }

    /// Our own M16 rounds on vcod-server at an mp_crash destructible car
    /// (cod4-combat.md 13.1): a script mover with no `solid`, which the
    /// client traces by its model's collision (`CG_LocationalTraceDObj`)
    /// as the server's `Bullet_Trace` does, so each round stops on the car
    /// where the server's trace of the same line stops.
    #[test]
    fn cod4_live_own_rounds_stop_on_a_car_where_the_servers_do() {
        use crate::cod4_fx::OwnView;
        use crate::play::input::Action;
        let sets = [
            ("sv_cheats", "1"),
            ("scr_game_playerwaittime", "1"),
            ("scr_game_matchstarttime", "1"),
        ];
        let Some(out) = live_join_on(
            "mp_crash",
            crash_world(),
            &sets,
            move |s, spawned, t0, tx| {
                spawned.as_ref()?;
                let install = vcod_cod4::testing::cod4_install()?;
                let ((sounds, _), _) = crate::cod4_fly::load_av(install.root(), "mp_crash").ok()?;
                let world = s.world()?.clone();
                let step = |s: &mut Cod4Session, ms: u64| {
                    let end = clk_now() + Duration::from_millis(ms);
                    let mut fired = Vec::new();
                    while clk_now() < end {
                        s.step(since(t0).as_secs_f64() * 1000.0).unwrap();
                        s.take_commands();
                        fired.extend(s.take_fired());
                        clk_sleep(Duration::from_millis(16));
                    }
                    fired
                };
                let frozen = |s: &Cod4Session| {
                    s.client()
                        .snapshots
                        .last()
                        .and_then(|x| x.ps.get("pm_flags"))
                        .is_some_and(|f| f & 0x800 != 0)
                };
                let wait = clk_now();
                while frozen(s) && since(wait) < Duration::from_secs(30) {
                    step(s, 16);
                }
                let (mtx, mrx) = std::sync::mpsc::channel();
                tx.send(Ctl::Models(mtx)).ok()?;
                let models: ModelList = loop {
                    step(s, 16);
                    if let Ok(v) = mrx.try_recv() {
                        break v;
                    }
                };
                let ps = &s.client().snapshots.last()?.ps;
                let feet = vec3(|k| ps_f32(ps, &format!("origin[{k}]")));
                let mut cars: Vec<_> = models.iter().filter(|m| m.2).collect();
                cars.sort_by(|a, b| a.1.distance(feet).total_cmp(&b.1.distance(feet)));
                // A spot 160 units off a car, on the floor, whose eye sees
                // the car's doors over open air.
                let (car, spot, aim) = cars.iter().find_map(|c| {
                    (0..16).find_map(|k| {
                        let y = (k as f32 * 22.5).to_radians();
                        let d = Vec3::new(y.cos(), y.sin(), 0.0);
                        let spot = c.1 + d * 160.0;
                        let (top, bottom) = (spot + Vec3::Z * 64.0, spot - Vec3::Z * 128.0);
                        let down =
                            world
                                .clip
                                .trace(top, bottom, Vec3::ZERO, Vec3::ZERO, 0x281_0011);
                        if down.startsolid || down.fraction >= 1.0 {
                            return None;
                        }
                        let floor = top.lerp(bottom, down.fraction) + Vec3::Z * 0.125;
                        if (floor.z - c.1.z).abs() > 32.0 {
                            return None;
                        }
                        let (lo, hi) = (Vec3::new(-16.0, -16.0, 0.0), Vec3::new(16.0, 16.0, 70.0));
                        let room = world.clip.trace(floor, floor, lo, hi, 0x281_0011);
                        if room.startsolid {
                            return None;
                        }
                        let aim = c.1 + Vec3::Z * 22.0;
                        let eye = floor + Vec3::Z * 60.0;
                        let t = world
                            .clip
                            .trace(eye, aim, Vec3::ZERO, Vec3::ZERO, 0x280_6831);
                        (t.fraction == 1.0).then_some((c.0, floor, aim))
                    })
                })?;
                let to = aim - (spot + Vec3::Z * 60.0);
                let yaw = to.y.atan2(to.x).to_degrees();
                // `setviewpos` places the eye.
                s.command(&format!(
                    "setviewpos {} {} {} {yaw}",
                    spot.x,
                    spot.y,
                    spot.z + 60.0
                ));
                step(s, 1500);
                let ps = &s.client().snapshots.last()?.ps;
                let eye = vec3(|k| ps_f32(ps, &format!("origin[{k}]"))) + Vec3::Z * 60.0;
                // On the spot, not knocked off it.
                if eye.truncate().distance(spot.truncate()) > 1.0 {
                    return None;
                }
                let to = aim - eye;
                s.view = [
                    -to.z.atan2(to.truncate().length()).to_degrees(),
                    to.y.atan2(to.x).to_degrees(),
                ];
                step(s, 500);
                let weapons = weapon_list(s.gamestate()?);
                let defs = sounds.weapons.clone();
                let mut rounds = Vec::new();
                let t1 = clk_now();
                while since(t1) < Duration::from_millis(1600) {
                    // Two short bursts: the car must not burn out.
                    let ms = since(t1).as_millis() % 800;
                    s.input.key(Action::Attack, ms < 100);
                    s.step(since(t0).as_secs_f64() * 1000.0).unwrap();
                    s.take_commands();
                    let scene = s.scene();
                    let fired = s.take_fired();
                    let Some(scene) = scene else {
                        clk_sleep(Duration::from_millis(16));
                        continue;
                    };
                    let own = OwnView {
                        client: Some(scene.view.client_num),
                        eye: scene.eye,
                        angles: scene.angles,
                        flash: None,
                    };
                    for f in fired
                        .iter()
                        .filter(|f| f.name.starts_with("EV_FIRE_WEAPON"))
                    {
                        if let Some(h) = first_bullet(
                            f,
                            &weapons,
                            &defs,
                            &HashMap::new(),
                            Some(&own),
                            &world.clip,
                            &s.shot_solids,
                        ) {
                            rounds.push((h.start, h.end));
                        }
                    }
                    clk_sleep(Duration::from_millis(16));
                }
                s.input.key(Action::Attack, false);
                let lines = rounds
                    .iter()
                    .map(|&(a, b)| (a, a + (b - a).normalize_or_zero() * 8192.0))
                    .collect();
                let (htx, hrx) = std::sync::mpsc::channel();
                tx.send(Ctl::ModelTrace(lines, htx)).ok()?;
                let server: ModelHits = loop {
                    step(s, 16);
                    if let Ok(v) = hrx.try_recv() {
                        break v;
                    }
                };
                Some((car, rounds, server))
            },
        )
        .flatten() else {
            return;
        };
        let (car, rounds, server) = out;
        println!("car {car} rounds {rounds:?}\nserver {server:?}");
        assert!(rounds.len() >= 4, "{rounds:?}");
        // The wire carries the car's origin in whole units, as retail's
        // (car 153 reads 996 -1408 64 in `play-1.7-spec.txt`, where the
        // server's stands at 995.9 -1407.9), so the client's car is a
        // tenth of a unit off the server's on each axis.
        for ((_, end), (sv_end, sv_car)) in rounds.iter().zip(&server) {
            assert_eq!(*sv_car, Some(car), "the server's round missed the car");
            assert!(
                end.distance(*sv_end) < 0.5,
                "ours {end} the server's {sv_end}"
            );
        }
    }

    fn rounds_on_the_bot(sighted: bool, hurt: bool) {
        use crate::cod4_fx::OwnView;
        use crate::play::input::Action;
        let sets = [
            ("sv_cheats", "1"),
            ("scr_game_playerwaittime", "1"),
            ("scr_game_matchstarttime", "1"),
        ];
        let Some(out) = live_join_on(
            "mp_crash",
            crash_world(),
            &sets,
            move |s, spawned, t0, tx| {
                spawned.as_ref()?;
                let install = vcod_cod4::testing::cod4_install()?;
                let ((sounds, mut effects), _) =
                    crate::cod4_fly::load_av(install.root(), "mp_crash").ok()?;
                let world = s.world()?.clone();
                // A frame as the client's: the scene too, whose prediction
                // runs the aim block on every cmd from the spawn on, as
                // the server's think does; one first drawn at the burst
                // started the idle drift's clock there, the server's
                // seconds ahead, and a sighted round left up to 0.05
                // degrees off by the join's length.
                let step = |s: &mut Cod4Session, ms: u64| {
                    let end = clk_now() + Duration::from_millis(ms);
                    let mut fired = Vec::new();
                    while clk_now() < end {
                        s.step(since(t0).as_secs_f64() * 1000.0).unwrap();
                        s.take_commands();
                        s.scene();
                        fired.extend(s.take_fired());
                        clk_sleep(Duration::from_millis(16));
                    }
                    fired
                };
                let frozen = |s: &Cod4Session| {
                    s.client()
                        .snapshots
                        .last()
                        .and_then(|x| x.ps.get("pm_flags"))
                        .is_some_and(|f| f & 0x800 != 0)
                };
                let wait = clk_now();
                while frozen(s) && since(wait) < Duration::from_secs(30) {
                    step(s, 16);
                }
                // A yaw along which the eye sees 400 units of open floor.
                let ps = &s.client().snapshots.last()?.ps;
                let feet = vec3(|k| ps_f32(ps, &format!("origin[{k}]")));
                let eye = feet + Vec3::Z * 60.0;
                let yaw = (0..24).map(|k| k as f32 * 15.0).find(|y| {
                    let d = Vec3::new(y.to_radians().cos(), y.to_radians().sin(), 0.0);
                    let t =
                        world
                            .clip
                            .trace(eye, eye + d * 400.0, Vec3::ZERO, Vec3::ZERO, 0x280_6831);
                    let down = world.clip.trace(
                        feet + d * 200.0 + Vec3::Z * 8.0,
                        feet + d * 200.0 - Vec3::Z * 8.0,
                        Vec3::ZERO,
                        Vec3::ZERO,
                        1,
                    );
                    t.fraction == 1.0 && down.fraction < 1.0
                })?;
                let d = Vec3::new(yaw.to_radians().cos(), yaw.to_radians().sin(), 0.0);
                let at = feet + d * 200.0;
                tx.send(Ctl::PinBot(at.to_array())).ok()?;
                // At his chest, 15 under the eye: level, the hip spread put
                // a burst's rounds over his head by its seeds, the cmds'
                // times, and the server's bones met none of them.
                s.view = [15f32.atan2(200.0).to_degrees(), yaw];
                s.input.key(Action::Ads, sighted);
                step(s, 6000);
                let weapons = weapon_list(s.gamestate()?);
                let defs = sounds.weapons.clone();
                let (mut rounds, mut bleeds, mut own_plays) = (Vec::new(), Vec::new(), 0);
                let mut flesh = Vec::new();
                let mut body = None;
                let mut hurts = 0;
                let t1 = clk_now();
                while since(t1) < Duration::from_millis(4000) {
                    // Three bursts a second and more apart, the spread settled
                    // between them.
                    let ms = since(t1).as_millis() % 1300;
                    let burst = since(t1).as_millis() / 1300;
                    if hurt && hurts <= burst && ms >= 20 {
                        let view = Vec3::new(0.0, yaw, 0.0);
                        let [f, r, _] = vcod_common::x87::angle_vectors(view);
                        let dir = [Some(f.to_array()), Some(r.to_array()), None];
                        tx.send(Ctl::Hurt(40, dir[hurts as usize % 3])).ok()?;
                        hurts += 1;
                    }
                    s.input.key(Action::Ads, sighted);
                    // Hurt, the burst waits 280 ms: the snapshot with
                    // the hit reaches the predictor first, and the kick
                    // (out by 500 ms) still turns the rounds. A round
                    // fired before that snapshot leaves without it, on
                    // retail too.
                    let burst_at = if hurt { 300..450 } else { 0..150 };
                    s.input.key(Action::Attack, burst_at.contains(&ms));
                    s.step(since(t0).as_secs_f64() * 1000.0).unwrap();
                    s.take_commands();
                    // The scene first, as the client's frame takes it: its
                    // prediction fires our own shots this frame, which leave
                    // along this frame's view (the next frame's has moved on
                    // by the recoil).
                    let scene = s.scene();
                    let fired = s.take_fired();
                    let Some(scene) = scene else {
                        clk_sleep(Duration::from_millis(16));
                        continue;
                    };
                    let own = OwnView {
                        client: Some(scene.view.client_num),
                        eye: scene.eye,
                        angles: scene.angles,
                        flash: None,
                    };
                    for f in &fired {
                        if f.name == "EV_BULLET_HIT" {
                            let surf = f.state.as_ref().map_or(0, |e| ent_u32(e, "surfType"));
                            if surf == 7 {
                                bleeds.push(f.origin);
                                flesh.extend(
                                    effects
                                        .plays(
                                            f,
                                            &weapons,
                                            &|_| String::new(),
                                            &HashMap::new(),
                                            None,
                                        )
                                        .into_iter()
                                        .map(|p| p.key),
                                );
                            }
                            continue;
                        }
                        if !f.name.starts_with("EV_FIRE_WEAPON") {
                            continue;
                        }
                        let world = crate::cod4_fx::ShotWorld {
                            clip: &world.clip,
                            solids: &s.shot_solids,
                        };
                        for hit in crate::cod4_fx::client_bullets(
                            f,
                            &weapons,
                            &defs,
                            &HashMap::new(),
                            Some(&own),
                            world,
                        ) {
                            body = body.or(hit.body);
                            // Past the body the round goes on, and what it
                            // meets beyond plays; the body itself never.
                            if hit.body.is_some() {
                                let plays = effects.bullet_plays(f, &weapons, &hit).len();
                                own_plays +=
                                    plays.saturating_sub(hit.pierced.iter().flatten().count());
                            }
                            let (seed, spread) = f.aim.map_or((-1, 0.0), |a| (a.seed, a.spread));
                            rounds.push((seed, hit.start, hit.end, hit.body, spread));
                        }
                    }
                    clk_sleep(Duration::from_millis(16));
                }
                s.input.key(Action::Attack, false);
                step(s, 300);
                let (stx, srx) = std::sync::mpsc::channel();
                tx.send(Ctl::Shots(stx)).ok()?;
                let mut shots = None;
                while shots.is_none() {
                    step(s, 16);
                    shots = srx.try_recv().ok();
                }
                Some((rounds, bleeds, flesh, own_plays, body, at, shots?))
            },
        )
        .flatten() else {
            return;
        };
        let (rounds, bleeds, flesh, own_plays, body, at, shots) = out;
        println!(
            "rounds {rounds:?}\nbleeds {bleeds:?}\nflesh {flesh:?}\nbody {body:?} at {at}\nserver {shots:?}"
        );
        assert!(rounds.len() >= 6, "{rounds:?}");
        // Most rounds stop on the bot's box and play nothing of their own
        // (hurt, the kick throws the rounds about).
        let on_body = rounds.iter().filter(|r| r.3.is_some()).count();
        assert!(body.is_some(), "{rounds:?}");
        assert!(hurt || on_body * 2 >= rounds.len(), "{rounds:?}");
        assert_eq!(own_plays, 0, "a round into a bot played an impact on him");
        // Each round is seeded by the time of the cmd that fired, the
        // server's round of that cmd: the same disc point along the same
        // view. The spread's scale is the client's last frame's and the
        // server's last end frame's, which part during a burst by where
        // the server's frames fall between the cmds' packets (round two of
        // a burst read 3 against the client's 4.6 degrees in one phase and
        // agreed in another), so a round leaves along the server's disc
        // point scaled to the client's own spread, and exactly along the
        // server's where the two scales agree.
        let dir = |a: Vec3, b: Vec3| (b - a).normalize();
        let tan = |deg: f32| (deg * 0.017_453_292).tan();
        // `Vec3::angle_between`'s f32 `acos` reads in steps near 0 (0.0396,
        // 0.0442, 0.0484, 0.0523 degrees), so one step past a true 0.049
        // read over the bound; the angle off the cross and dot in f64.
        let degrees = |a: Vec3, b: Vec3| {
            let (a, b) = (a.as_dvec3(), b.as_dvec3());
            a.cross(b).length().atan2(a.dot(b)).to_degrees()
        };
        let mut exact = 0;
        for (t, start, end, _, spread) in &rounds {
            let sv = shots.iter().find(|s| s.0 == *t);
            let sv = sv.unwrap_or_else(|| panic!("no server round at {t}: {shots:?}"));
            let (aimed, sv_end) = (sv.2, sv.3);
            // The disc point off the seed (`Bullet_RandomDir`), at the
            // client's spread, along the server's offset's direction.
            let (x, y) = vcod_cod4::bullet::random_dir(*t);
            let range = aimed.distance(sv.1);
            let off = (sv_end - aimed).normalize_or_zero() * x.hypot(y) * range * tan(*spread);
            let deg = degrees(dir(*start, *end), dir(sv.1, aimed + off));
            // Hurt, a round's kick is the server's to a frame: which frame
            // runs a cmd depends on when its packet goes out (every 33 ms
            // of the client's clock), up to 1.3 degrees in the kick's
            // return (cod4-combat.md 3.2); 3.9 without the predictor's.
            let bound = if hurt { 1.5 } else { 0.05 };
            assert!(
                deg < bound,
                "round at {t}: {deg} degrees off the server's disc point"
            );
            exact += usize::from(degrees(dir(*start, *end), dir(sv.1, sv_end)) < 0.05);
        }
        assert!(exact >= 2, "{exact} of {} exact", rounds.len());
        // The first burst's first round plays too, though the loop's first
        // frame is the session's first scene since the spawn.
        assert_eq!(rounds[0].0, shots[0].0, "{rounds:?}");
        // The server bled the axis bot where our rounds stopped on him.
        assert!(!bleeds.is_empty());
        for b in &bleeds {
            let near = rounds
                .iter()
                .filter(|r| r.3.is_some())
                .map(|r| r.2.distance(*b))
                .fold(f32::MAX, f32::min);
            assert!(near < 24.0, "bleed {b} {near} from our nearest round");
        }
        assert!(flesh.iter().any(|k| k.contains("flesh")), "{flesh:?}");
    }

    /// Our demolitions shotgun on vcod-server (cod4-effects.md 12.2): each
    /// shot's pellets leave along the server's pellets (the seed the cmd's
    /// time plus the pellet, the disc point scaled to our own spread), and
    /// the shot rolls its tracer once (`DrawBulletImpacts`, iw3mp.exe
    /// 0x4567f2): every pellet draws one or none does.
    #[test]
    fn cod4_live_own_shotgun_pellets_share_one_tracer_roll() {
        use crate::cod4_fx::OwnView;
        use crate::play::input::Action;
        let sets = [
            ("scr_game_playerwaittime", "1"),
            ("scr_game_matchstarttime", "1"),
        ];
        let Some(out) = live_join_as(
            "mp_crash",
            crash_world(),
            &sets,
            "demolitions_mp,0",
            |s, spawned, t0, tx| {
                spawned.as_ref()?;
                let install = vcod_cod4::testing::cod4_install()?;
                let ((sounds, mut effects), _) =
                    crate::cod4_fly::load_av(install.root(), "mp_crash").ok()?;
                let world = s.world()?.clone();
                let mut fx = crate::fx::sim::FxSystem::new();
                let step = |s: &mut Cod4Session, ms: u64| {
                    let end = clk_now() + Duration::from_millis(ms);
                    while clk_now() < end {
                        s.step(since(t0).as_secs_f64() * 1000.0).unwrap();
                        s.take_commands();
                        s.scene();
                        s.take_fired();
                        clk_sleep(Duration::from_millis(16));
                    }
                };
                step(s, 3000);
                let weapons = weapon_list(s.gamestate()?);
                let defs = sounds.weapons.clone();
                // (seed, spread, pellets' start and end, tracers drawn).
                let mut shots = Vec::new();
                for press in 0..8 {
                    // The pump's rechamber done between two presses.
                    let t1 = clk_now();
                    while since(t1) < Duration::from_millis(1600) {
                        s.input
                            .key(Action::Attack, since(t1) < Duration::from_millis(100));
                        s.step(since(t0).as_secs_f64() * 1000.0).unwrap();
                        s.take_commands();
                        let scene = s.scene();
                        let fired = s.take_fired();
                        let Some(scene) = scene else {
                            clk_sleep(Duration::from_millis(16));
                            continue;
                        };
                        let own = OwnView {
                            client: Some(scene.view.client_num),
                            eye: scene.eye,
                            angles: scene.angles,
                            flash: None,
                        };
                        for f in fired
                            .iter()
                            .filter(|f| f.entity.is_none() && f.name.starts_with("EV_FIRE_WEAPON"))
                        {
                            let shot_world = crate::cod4_fx::ShotWorld {
                                clip: &world.clip,
                                solids: &s.shot_solids,
                            };
                            let hits = crate::cod4_fx::client_bullets(
                                f,
                                &weapons,
                                &defs,
                                &HashMap::new(),
                                Some(&own),
                                shot_world,
                            );
                            let drawn = effects.tracers(&mut fx, &hits, press as f32, 16);
                            let aim = f.aim?;
                            let rounds: Vec<_> = hits.iter().map(|h| (h.start, h.end)).collect();
                            shots.push((aim.seed, aim.spread, rounds, drawn));
                        }
                        clk_sleep(Duration::from_millis(16));
                    }
                }
                s.input.key(Action::Attack, false);
                step(s, 300);
                let (stx, srx) = std::sync::mpsc::channel();
                tx.send(Ctl::Shots(stx)).ok()?;
                let mut server = None;
                while server.is_none() {
                    step(s, 16);
                    server = srx.try_recv().ok();
                }
                Some((shots, server?))
            },
        )
        .flatten() else {
            return;
        };
        let (shots, server) = out;
        println!("client {shots:?}\nserver {server:?}");
        assert!(shots.len() >= 6, "{} shots", shots.len());
        let tan = |deg: f32| (deg * 0.017_453_292).tan();
        let dir = |a: Vec3, b: Vec3| (b - a).normalize();
        let mut rolled = [0; 2];
        for (seed, spread, rounds, drawn) in &shots {
            assert!(rounds.len() > 1, "not a shotgun: {} rounds", rounds.len());
            assert!(
                *drawn == 0 || *drawn == rounds.len(),
                "{drawn} tracers for {} pellets",
                rounds.len()
            );
            rolled[usize::from(*drawn > 0)] += 1;
            let pellets: Vec<_> = server.iter().filter(|r| r.0 == *seed).collect();
            assert_eq!(pellets.len(), rounds.len(), "server pellets at {seed}");
            for (k, ((start, end), sv)) in rounds.iter().zip(&pellets).enumerate() {
                let (x, y) = vcod_cod4::bullet::random_dir(seed + k as i32);
                let (aimed, sv_end) = (sv.2, sv.3);
                let range = aimed.distance(sv.1);
                let off = (sv_end - aimed).normalize_or_zero() * x.hypot(y) * range * tan(*spread);
                let deg = dir(*start, *end)
                    .angle_between(dir(sv.1, aimed + off))
                    .to_degrees();
                assert!(deg < 0.05, "pellet {k} at {seed}: {deg} degrees off");
            }
        }
        assert!(rolled[0] > 0 && rolled[1] > 0, "rolls {rolled:?}");
    }

    /// An axis bot pinned 200 units ahead on vcod-server fires at us
    /// (cod4-effects.md 12.2): each of his fire events takes the spread
    /// of the stance and sight his entity's legs anim reads, as 1.7's
    /// `CG_GuessSpreadForWeapon` and 0x456200 read it, through the anim
    /// set's movetype bits; every legs anim he shoots in names a movetype
    /// but a `fireweapon` clause's (a throw's, which names none and reads
    /// standing) on a standing or crouched body, a standing body's never a
    /// crouched or prone one, a crouched one only on a crouched body (a
    /// crouched run reads standing).
    #[test]
    fn cod4_live_a_bots_rounds_spread_by_his_legs_anim() {
        use crate::play::cod4_players::Cod4Players;
        use vcod_cod4::pmove::anim::{ANIM_TOGGLEBIT, LegsPose};
        let sets = [
            ("scr_game_playerwaittime", "1"),
            ("scr_game_matchstarttime", "1"),
        ];
        let Some(out) = live_join_on("mp_crash", crash_world(), &sets, |s, spawned, t0, tx| {
            spawned.as_ref()?;
            let install = vcod_cod4::testing::cod4_install()?;
            let (_, _, src) =
                crate::cod4_fly::load_bodies(install.root(), "mp_crash", &s.entity_assets())
                    .ok()?;
            let mut players = Cod4Players::default();
            let table = players.legs_movetypes(&src)?;
            let throws = players.fire_event_legs(&src)?;
            s.legs_movetypes = Some(table.clone());
            let world = s.world()?.clone();
            // Past the prematch's freeze.
            let wait = clk_now();
            while since(wait) < Duration::from_secs(30)
                && s.client()
                    .snapshots
                    .last()
                    .and_then(|x| x.ps.get("pm_flags"))
                    .is_some_and(|f| f & 0x800 != 0)
            {
                s.step(since(t0).as_secs_f64() * 1000.0).unwrap();
                s.take_commands();
                s.take_fired();
                clk_sleep(Duration::from_millis(16));
            }
            let ps = &s.client().snapshots.last()?.ps;
            let feet = vec3(|k| ps_f32(ps, &format!("origin[{k}]")));
            let eye = feet + Vec3::Z * 60.0;
            let yaw = (0..24).map(|k| k as f32 * 15.0).find(|y| {
                let d = Vec3::new(y.to_radians().cos(), y.to_radians().sin(), 0.0);
                let t = world
                    .clip
                    .trace(eye, eye + d * 400.0, Vec3::ZERO, Vec3::ZERO, 0x280_6831);
                let down = world.clip.trace(
                    feet + d * 200.0 + Vec3::Z * 8.0,
                    feet + d * 200.0 - Vec3::Z * 8.0,
                    Vec3::ZERO,
                    Vec3::ZERO,
                    1,
                );
                t.fraction == 1.0 && down.fraction < 1.0
            })?;
            let d = Vec3::new(yaw.to_radians().cos(), yaw.to_radians().sin(), 0.0);
            tx.send(Ctl::PinShooter((feet + d * 200.0).to_array()))
                .ok()?;
            s.view = [0.0, yaw];
            let mut shots = Vec::new();
            let t1 = clk_now();
            while since(t1) < Duration::from_secs(20) && shots.len() < 12 {
                s.step(since(t0).as_secs_f64() * 1000.0).unwrap();
                s.take_commands();
                // The window's frame: its scene loads the level's defs.
                s.scene();
                for f in s.take_fired() {
                    // Another's shot: our own carry no entity.
                    let (Some(_), Some(e), Some(aim)) = (f.entity, f.state.as_ref(), f.aim) else {
                        continue;
                    };
                    if !f.name.starts_with("EV_FIRE_WEAPON") {
                        continue;
                    }
                    let wire = ent_u32(e, "legsAnim") & !ANIM_TOGGLEBIT;
                    let def = s.level_def(f.weapon)?.clone();
                    shots.push((wire, ent_u32(e, "lerp.eFlags"), aim, def));
                }
                clk_sleep(Duration::from_millis(16));
            }
            let names: Vec<_> = shots
                .iter()
                .map(|x| players.anim_name(&src, x.0 as i32).unwrap_or_default())
                .collect();
            Some((shots, table, throws, names))
        }) else {
            return;
        };
        let Some((shots, table, throws, names)) = out else {
            return;
        };
        println!(
            "{:?}",
            shots
                .iter()
                .zip(&names)
                .map(|((w, f, a, _), n)| (w, n, f, table[*w as usize], a.spread))
                .collect::<Vec<_>>()
        );
        assert!(shots.len() >= 3, "the bot fired {} rounds", shots.len());
        for ((wire, flags, aim, def), name) in shots.iter().zip(&names) {
            let mask = table[*wire as usize];
            // A `fireweapon` clause's legs anim (`pb_stand_grenade_throw`
            // on `movetype idle`, `pb_crouch_grenade_throw` on `idlecr`,
            // which the next rounds still fire in) is in no state's block:
            // movetype 0, read as standing (cod4-effects.md 12.2). Its
            // timer holds it through a crouch or a stand: the script has no
            // event between the two (cod4-player-anims.md 7). Lying down
            // has one (`crouch_to_prone`'s `both pb_crouch2prone`).
            if mask == 0 {
                assert!(
                    throws.contains(wire),
                    "legs anim {wire} {name} names no movetype and no fire event's"
                );
                assert!(flags & 8 == 0, "{wire} {name} on a prone body");
            }
            let legs = LegsPose::of(mask);
            if flags & 0xc == 0 {
                assert!(!legs.crouched && !legs.prone, "{wire} standing: {legs:?}");
            }
            if legs.crouched {
                assert!(flags & 4 != 0, "{wire} crouched on a body standing");
            }
            // The speed scale is the snapshots', not ours: the spread
            // lies between its stance's near end and its far end.
            let near = crate::cod4_fx::Aim::other(def, legs, 0.0, aim.seed).spread;
            let far = crate::cod4_fx::Aim::other(def, legs, 255.0, aim.seed).spread;
            assert!(
                aim.spread >= near.min(far) - 1e-4 && aim.spread <= near.max(far) + 1e-4,
                "{wire} {legs:?}: spread {} outside {near}..{far}",
                aim.spread
            );
        }
    }

    /// `class`'s primary on vcod-server, from the hip and then sighted,
    /// two presses each (held `hold` frames), in the window's frame order
    /// (step, scene, events): every predicted own `EV_FIRE_WEAPON`'s seed
    /// (the time of the cmd that fired it) with whether it was sighted, and
    /// the times of the rounds the server's `Bullet_Fire` fired for us.
    #[allow(clippy::type_complexity)]
    fn own_rounds(class: &'static str, hold: usize) -> Option<(Vec<(i32, bool)>, Vec<i32>)> {
        use crate::play::input::Action;
        let sets = [
            ("scr_game_playerwaittime", "1"),
            ("scr_game_matchstarttime", "1"),
        ];
        live_join_as(
            "mp_crash",
            crash_world(),
            &sets,
            class,
            move |s, spawned, t0, tx| {
                spawned.as_ref()?;
                let mut seeds = Vec::new();
                let frame = |s: &mut Cod4Session, ads: bool, seeds: &mut Vec<(i32, bool)>| {
                    s.step(since(t0).as_secs_f64() * 1000.0).unwrap();
                    s.take_commands();
                    s.scene();
                    for f in s.take_fired() {
                        if f.entity.is_none() && f.name.starts_with("EV_FIRE_WEAPON") {
                            seeds.push((f.aim.map_or(-1, |a| a.seed), ads));
                        }
                    }
                    clk_sleep(Duration::from_millis(16));
                };
                let snap = |s: &Cod4Session, n: &str| {
                    let ps = &s.client().snapshots.last().unwrap().ps;
                    (ps.get(n).unwrap_or(0), ps_f32(ps, n))
                };
                // Not frozen, and the weapon ready on the server's clock.
                let ready = |s: &Cod4Session| {
                    snap(s, "pm_flags").0 & 0x800 == 0
                        && snap(s, "weaponstate").0 == 0
                        && snap(s, "weaponTime").0 == 0
                };
                for ads in [false, true] {
                    s.input.key(Action::Ads, ads);
                    for _ in 0..2 {
                        let wait = clk_now();
                        while since(wait) < Duration::from_secs(30)
                            && !(ready(s) && (!ads || snap(s, "fWeaponPosFrac").1 >= 1.0))
                        {
                            frame(s, ads, &mut seeds);
                        }
                        s.input.key(Action::Attack, true);
                        for _ in 0..hold {
                            frame(s, ads, &mut seeds);
                        }
                        s.input.key(Action::Attack, false);
                        for _ in 0..30 {
                            frame(s, ads, &mut seeds);
                        }
                    }
                }
                s.input.key(Action::Ads, false);
                // The server's rounds once the last snapshot carrying ours is in.
                for _ in 0..30 {
                    frame(s, false, &mut seeds);
                }
                let (stx, srx) = std::sync::mpsc::channel();
                tx.send(Ctl::Shots(stx)).ok()?;
                let mut shots = None;
                while shots.is_none() {
                    frame(s, false, &mut seeds);
                    shots = srx.try_recv().ok();
                }
                let mut times: Vec<i32> = shots?.iter().map(|r| r.0).collect();
                times.dedup();
                Some((seeds, times))
            },
        )
        .flatten()
    }

    /// Every round the server fires for our presses plays once on the
    /// client, the first of each burst included (cod4-effects.md 12.2):
    /// the M16's three-round bursts, the MP5 held, the M40A3's single
    /// shots, from the hip and sighted.
    #[test]
    fn cod4_live_every_server_round_fires_once() {
        for (class, hold) in [
            ("assault_mp,0", 8),
            ("specops_mp,0", 20),
            ("sniper_mp,0", 4),
        ] {
            let Some((seeds, times)) = own_rounds(class, hold) else {
                return;
            };
            println!("{class}: client {seeds:?}\nserver {times:?}");
            for ads in [false, true] {
                let n = seeds.iter().filter(|s| s.1 == ads).count();
                assert!(n >= 2, "{class} ads {ads}: {seeds:?}");
            }
            let ours: Vec<i32> = seeds.iter().map(|s| s.0).collect();
            assert_eq!(ours, times, "{class}");
        }
    }

    /// Our own shots on vcod-server fire off the prediction
    /// (`CG_CheckPlayerstateEvents` on the predicted state, cod4-effects.md
    /// 12): in the window's frame order (step, scene, events) each burst's
    /// first `EV_FIRE_WEAPON` fires on the frame `+attack` goes down, with
    /// its flash and its client-side impact, and every round fires once:
    /// as many own fire events as the server's snapshots carry in our ring,
    /// none of those again when they arrive a round trip later.
    #[test]
    fn cod4_live_own_shots_fire_the_frame_they_press() {
        use crate::cod4_fx::OwnView;
        use crate::play::input::Action;
        let Some(out) = live_join(|s, spawned, t0| {
            spawned.as_ref()?;
            let install = vcod_cod4::testing::cod4_install()?;
            let ((sounds, mut effects), _) =
                crate::cod4_fly::load_av(install.root(), "mp_crash").ok()?;
            let world = s.world()?.clone();
            let frozen = |s: &Cod4Session| {
                s.client()
                    .snapshots
                    .last()
                    .and_then(|x| x.ps.get("pm_flags"))
                    .is_some_and(|f| f & 0x800 != 0)
            };
            // The weapon ready on the server's clock (`weaponstate` 0, no
            // `weaponTime` left): a loaded machine spawns us late, so the
            // spawn's raise can still run where a press would land.
            let ready = |s: &Cod4Session| {
                s.client().snapshots.last().is_some_and(|x| {
                    let u = |n: &str| x.ps.get(n).unwrap_or(0);
                    u("weaponstate") == 0 && u("weaponTime") == 0
                })
            };
            let wait = clk_now();
            while (frozen(s) || !ready(s)) && since(wait) < Duration::from_secs(60) {
                s.step(since(t0).as_secs_f64() * 1000.0).unwrap();
                s.take_commands();
                s.scene();
                s.take_fired();
                clk_sleep(Duration::from_millis(16));
            }
            s.view[0] = 45.0;
            // The server's own count: fire events in our snapshot ring, by
            // the frame they arrived.
            let mut seen = s
                .client()
                .snapshots
                .last()
                .map(|x| x.ps.get("eventSequence").unwrap_or(0) & 0xff);
            let mut walked = s.client().snapshots.len();
            let (mut frame, mut held) = (0usize, false);
            let mut presses = Vec::new();
            let (mut ours, mut theirs) = (Vec::new(), Vec::new());
            let (mut flashes, mut impacts) = (0, 0);
            let t1 = clk_now();
            // Two bursts of 30 frames (about 300..800 and 1300..1800 ms at
            // 60 Hz), counted in frames as a loaded machine stretches
            // them; the second waits for the weapon to settle after the
            // first. 72 frames past the second, and on until the server's
            // ring has every predicted round: a starved server sends the
            // last bursts' late.
            let mut bursts = [(18usize, 48usize), (78, 108)];
            while frame < bursts[1].1 + 72
                || (theirs.len() < ours.len() && since(t1) < Duration::from_secs(30))
            {
                if !held && bursts[1].0 == frame && presses.len() == 1 && !ready(s) {
                    bursts[1].0 += 1;
                    bursts[1].1 += 1;
                }
                let want = bursts.iter().any(|&(a, b)| (a..b).contains(&frame));
                if want != held {
                    s.input.key(Action::Attack, want);
                    held = want;
                    if want {
                        presses.push((frame, since(t1)));
                    }
                }
                s.step(since(t0).as_secs_f64() * 1000.0).unwrap();
                s.take_commands();
                let scene = s.scene();
                let fired = s.take_fired();
                let snaps = &s.client().snapshots;
                for x in &snaps[walked..] {
                    let seq = x.ps.get("eventSequence").unwrap_or(0) & 0xff;
                    if let Some(prev) = seen {
                        let to = if seq < prev { seq + 256 } else { seq };
                        for q in prev.max(to.saturating_sub(4))..to {
                            let id = x.ps.get(&format!("events[{}]", q & 3)).unwrap_or(0);
                            if event_name(id).is_some_and(|n| n.starts_with("EV_FIRE_WEAPON")) {
                                theirs.push((frame, since(t1)));
                            }
                        }
                    }
                    seen = Some(seq);
                }
                walked = snaps.len();
                let weapons = weapon_list(s.gamestate().unwrap());
                let cs = |k: u16| {
                    String::from_utf8_lossy(s.gamestate().unwrap().configstring(k)).into_owned()
                };
                for f in fired
                    .iter()
                    .filter(|f| f.name.starts_with("EV_FIRE_WEAPON"))
                {
                    assert!(f.entity.is_none(), "{f:?}");
                    ours.push((frame, since(t1)));
                    flashes += effects
                        .plays(f, &weapons, &cs, &HashMap::new(), None)
                        .iter()
                        .filter(|p| p.key.starts_with("muzzleflashes/"))
                        .count();
                    let Some(scene) = &scene else { continue };
                    let own = OwnView {
                        client: Some(scene.view.client_num),
                        eye: scene.eye,
                        angles: scene.angles,
                        flash: None,
                    };
                    let defs = sounds.weapons.clone();
                    if let Some(hit) = first_bullet(
                        f,
                        &weapons,
                        &defs,
                        &HashMap::new(),
                        Some(&own),
                        &world.clip,
                        &s.shot_solids,
                    ) {
                        impacts += effects.bullet_plays(f, &weapons, &hit).len();
                    }
                }
                frame += 1;
                clk_sleep(Duration::from_millis(16));
            }
            s.input.key(Action::Attack, false);
            Some((presses, ours, theirs, flashes, impacts))
        })
        .flatten() else {
            return;
        };
        let (presses, ours, theirs, flashes, impacts) = out;
        let first = |v: &[(usize, Duration)], p: &(usize, Duration)| {
            v.iter().find(|x| x.0 >= p.0).copied().unwrap()
        };
        for p in &presses {
            let (o, t) = (first(&ours, p), first(&theirs, p));
            println!(
                "press at frame {}: predicted fire {} frames / {} ms later, the server's {} frames / {} ms",
                p.0,
                o.0 - p.0,
                (o.1 - p.1).as_millis(),
                t.0 - p.0,
                (t.1 - p.1).as_millis()
            );
            assert!(o.0 - p.0 <= 1, "{p:?} {o:?}");
        }
        println!(
            "{} predicted fire events, {} in the server's ring, {flashes} flashes, {impacts} impacts",
            ours.len(),
            theirs.len()
        );
        assert_eq!(presses.len(), 2);
        assert!(ours.len() >= 4, "{ours:?}");
        assert_eq!(ours.len(), theirs.len(), "{ours:?} {theirs:?}");
        assert_eq!((flashes, impacts), (ours.len(), ours.len()));
    }

    /// `+scores` in a live session: the board draws over the HUD while
    /// held (asking for `score`), and goes once let go and faded.
    #[test]
    fn cod4_live_scores_toggle_the_board() {
        let Some(out) = live_join(|s, spawned, t0| {
            spawned.as_ref()?;
            let install = vcod_cod4::testing::cod4_install()?;
            let mut hud = crate::cod4_hud::Cod4Hud::default();
            hud.load(&install.root().join("main"), "mp_crash");
            let shell = crate::console::shell::Shell::new();
            let size = (1280.0, 960.0);
            // A frame on the session's clock; the HUD's quads on it with
            // the board up and down are counted on one frame, as a loaded
            // machine's frames change the HUD under the toggle.
            let frame = |hud: &mut crate::cod4_hud::Cod4Hud, s: &mut Cod4Session| {
                s.step(since(t0).as_secs_f64() * 1000.0).unwrap();
                let fired = s.take_fired();
                let now = s.scene().map_or(0, |x| x.render_time);
                hud.update(s, &fired, now);
                now
            };
            let count = |hud: &crate::cod4_hud::Cod4Hud, s: &Cod4Session, now: i32| {
                hud.build(s, size, now, None, &shell).len()
            };
            let now = frame(&mut hud, s);
            let before = count(&hud, s, now);
            let asked = hud.set_scores(true, now);
            let held = count(&hud, s, now);
            let now = frame(&mut hud, s);
            hud.set_scores(false, now);
            // The 100 ms fade on the session's clock, which a starved
            // server holds at the newest snapshot: frames until it passes.
            let wait = clk_now();
            let mut at = frame(&mut hud, s);
            while at < now + 250 && since(wait) < Duration::from_secs(20) {
                clk_sleep(Duration::from_millis(16));
                at = frame(&mut hud, s);
            }
            let after = count(&hud, s, at);
            // The same frame with the board held again.
            hud.set_scores(true, at);
            let again = count(&hud, s, at);
            Some((asked, before, held, after, again))
        }) else {
            return;
        };
        let (asked, before, held, after, again) = out.expect("no spawn");
        assert!(asked, "the press asks for `score`");
        assert!(held > before + 4, "{before} quads, {held} with the board");
        assert!(after < again, "{after} quads once let go, {again} held");
    }

    /// vcod-server's `war` with twenty bots and us on allies, the board
    /// held: past one screen of rows it opens at the top with the column
    /// headers on the first banner's line and the scroll bar's down
    /// arrow, a wheel notch moves it `cg_scoreboardScrollStep` lines down
    /// (headers on their own line, the up arrow) and a notch up brings it
    /// back (`CG_DrawScoreboard_ScoresList`, `Scoreboard_HandleInput`,
    /// cod4-hud.md 7.1). `cg_scoreboardPingGraph 1` typed at the console
    /// adds each row's bars.
    #[test]
    fn cod4_live_a_long_board_scrolls_by_the_wheel() {
        use crate::cod4_scoreboard::Piece;
        use vcod_server::cod4::{Cod4Config, Cod4Server};
        let Some(install) = vcod_cod4::testing::cod4_install() else {
            return;
        };
        let main = install.root().join("main");
        let mut sv = Cod4Server::new(
            Cod4Config {
                map: "mp_crash".into(),
                gametype: "war".into(),
                hostname: "vcod".into(),
                max_clients: 24,
            },
            7,
        );
        sv.set_cvar("scr_game_playerwaittime", "1");
        sv.set_cvar("scr_game_matchstarttime", "1");
        sv.load(Some(&install)).unwrap();
        sv.add_bots(20, false);
        let sock = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
        sock.set_nonblocking(true).unwrap();
        let addr = sock.local_addr().unwrap();
        let clock = sim::Clock::new();
        let client = sim::spawn(&clock, move || {
            let join = MenuJoin {
                team: Some("allies".into()),
                class: Some("assault_mp,0".into()),
            };
            let mut s = Cod4Session::connect(addr, "joiner", Cp::Bare, join).unwrap();
            let mut hud = crate::cod4_hud::Cod4Hud::default();
            hud.load(&main, "mp_crash");
            let mut shell = crate::console::shell::Shell::new();
            shell.register_cod4_options();
            let size = (1280.0, 960.0);
            let t0 = clk_now();
            let mut held = false;
            let mut out = None;
            // (pieces at the top, scrolled, back up, with the ping graph)
            while since(t0) < Duration::from_secs(60) && out.is_none() {
                s.step(since(t0).as_secs_f64() * 1000.0).unwrap();
                let fired = s.take_fired();
                clk_sleep(Duration::from_millis(16));
                let Some(now) = s.scene().map(|x| x.render_time) else {
                    continue;
                };
                hud.update(&mut s, &fired, now);
                if s.spawn().is_none() {
                    continue;
                }
                if !held || hud.score_request(now) {
                    held = true;
                    if hud.set_scores(true, now) {
                        s.command("score");
                    }
                }
                if hud.scores.as_ref().is_none_or(|sc| sc.rows.len() < 21) {
                    continue;
                }
                let board = |hud: &crate::cod4_hud::Cod4Hud,
                             s: &Cod4Session,
                             sh: &crate::console::shell::Shell| {
                    hud.build(s, size, now, None, sh);
                    hud.board_pieces(s, sh)
                };
                let top = board(&hud, &s, &shell);
                assert!(hud.scoreboard_key("MWHEELDOWN", 3));
                let down = board(&hud, &s, &shell);
                assert!(hud.scoreboard_key("MWHEELUP", 3));
                let up = board(&hud, &s, &shell);
                // Typed bare, as on 1.7, where the cgame registers it: an
                // unregistered name went to the server as a command.
                let typed = shell.execute("cg_scoreboardPingGraph 1");
                assert!(
                    !typed
                        .iter()
                        .any(|e| matches!(e, crate::console::shell::Effect::Unknown { .. })),
                    "{typed:?}"
                );
                let graph = board(&hud, &s, &shell);
                out = Some((top, down, up, graph));
            }
            s.disconnect();
            out
        });
        serve(&mut sv, &sock, &clock, &client, |_, _, _| true);
        let (top, down, up, graph) = client.join().unwrap().expect("no long board");
        let pic = |b: &[Piece], m: &str| {
            b.iter()
                .any(|p| matches!(p, Piece::Pic { material, .. } if material == m))
        };
        let text_y = |b: &[Piece], t: &str| {
            b.iter().find_map(|p| match p {
                Piece::Text { text, y, .. } if text == t => Some(*y),
                _ => None,
            })
        };
        // At the top the headers share the first banner's baseline.
        let banner = text_y(&top, "Marines").expect("our banner");
        assert_eq!(text_y(&top, "Score"), Some(banner), "{top:?}");
        assert!(pic(&top, "hudscoreboardscroll_downarrow"), "{top:?}");
        assert!(!pic(&top, "hudscoreboardscroll_uparrow"));
        // Three lines down: our banner scrolled off, the headers alone.
        assert!(text_y(&down, "Marines").is_none(), "{down:?}");
        assert!(text_y(&down, "Score").is_some());
        assert!(pic(&down, "hudscoreboardscroll_uparrow"), "{down:?}");
        assert_eq!(up, top, "the notch up is the top again");
        // Each drawn row gets the graph's backing and at least one bar.
        let whites = |b: &[Piece]| {
            b.iter()
                .filter(|p| matches!(p, Piece::Pic { material, .. } if material == "white"))
                .count()
        };
        assert!(
            whites(&graph) >= whites(&top) * 3,
            "{} {}",
            whites(&top),
            whites(&graph)
        );
    }

    /// vcod-server's `war` with eight shooting bots, watched as a
    /// spectator and posed through `play::cod4_players` each frame: a
    /// corpse keeps the models it was copied with while its client
    /// respawns and changes its stowed weapons (`CG_ResetEntity`'s
    /// `CG_CopyCorpseInfo`), and the server's `BodyEnd` takes its fresh
    /// bit out, so nothing replays its death anim (cod4-player-anims.md
    /// 9). `None` without an install.
    #[test]
    fn cod4_live_corpses_keep_their_models_and_go_stale() {
        use crate::play::cod4_players::Cod4Players;
        use vcod_server::cod4::{Cod4Config, Cod4Server};
        let Some(install) = vcod_cod4::testing::cod4_install() else {
            return;
        };
        let root = install.root().to_path_buf();
        let world = crash_world();
        let mut sv = Cod4Server::new(
            Cod4Config {
                map: "mp_crash".into(),
                gametype: "war".into(),
                hostname: "vcod".into(),
                max_clients: 12,
            },
            7,
        );
        sv.set_cvar("scr_game_playerwaittime", "1");
        sv.set_cvar("scr_game_matchstarttime", "1");
        sv.load(Some(&install)).unwrap();
        sv.add_bots(8, true);
        let sock = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
        sock.set_nonblocking(true).unwrap();
        let addr = sock.local_addr().unwrap();
        let clock = sim::Clock::new();
        let client = sim::spawn(&clock, move || {
            let join = MenuJoin {
                team: Some("spectator".into()),
                class: Some("assault_mp,0".into()),
            };
            let mut s = Cod4Session::connect(addr, "watcher", Cp::Bare, join).unwrap();
            if let Some(w) = world {
                s.set_world(w);
            }
            let mut players = Cod4Players::default();
            let mut loaded = None;
            // Per corpse (number, client, teleport bit): the models it
            // was first posed with, and whether its fresh bit went out.
            let mut corpses: BTreeMap<(u32, i32, i32), (Character, bool)> = BTreeMap::new();
            let mut changed = Vec::new();
            let t0 = clk_now();
            while since(t0) < Duration::from_secs(45) {
                let now = since(t0).as_secs_f64() * 1000.0;
                s.step(now).unwrap();
                s.take_commands();
                clk_sleep(Duration::from_millis(16));
                let Some(scene) = s.scene() else { continue };
                if loaded.is_none() {
                    loaded = Some(
                        crate::cod4_fly::load_bodies(&root, "mp_crash", &s.entity_assets())
                            .unwrap(),
                    );
                }
                let (_, weapons, src) = loaded.as_ref().unwrap();
                let views: Vec<(u32, &BodyView)> = scene
                    .entities
                    .iter()
                    .filter_map(|d| Some((d.number, d.body.as_ref()?)))
                    .collect();
                players.posed(
                    src,
                    weapons,
                    &views,
                    &[],
                    scene.render_time,
                    scene.server_time,
                );
                for (n, v) in views.iter().filter(|(_, v)| v.corpse) {
                    let Some(c) = players.character(*n) else {
                        continue;
                    };
                    let key = (*n, v.frame.client_num, v.frame.eflags & 2);
                    let stale = v.frame.eflags & cod2_eflags::FRESH_CORPSE == 0;
                    let e = corpses.entry(key).or_insert((c.clone(), false));
                    e.1 |= stale;
                    if e.0 != *c {
                        changed.push(format!("{key:?}: {:?} -> {c:?}", e.0));
                        e.0 = c.clone();
                    }
                }
            }
            s.disconnect();
            (corpses, changed)
        });
        serve(&mut sv, &sock, &clock, &client, |_, _, _| true);
        let (corpses, changed) = client.join().unwrap();
        println!("{} corpses", corpses.len());
        assert!(corpses.len() >= 3, "{corpses:?}");
        assert!(changed.is_empty(), "corpse models changed: {changed:#?}");
        assert!(
            corpses.values().filter(|(_, stale)| *stale).count() >= corpses.len() / 2,
            "fresh bits never went out: {corpses:?}"
        );
    }

    /// Up to `n` spots of open, level floor on the map, searched on a
    /// 64-unit grid round each `mp_tdm_spawn` (in the map's order, nearest
    /// first): floor flat 48 units every way, clear of the world 48 units
    /// round and 72 up, 160 from another spot. Each is the floor point.
    fn open_floor_spots(
        clip: &vcod_cod4::collision::ClipMap,
        map: &vcod_cod4::map::Map,
        n: usize,
    ) -> Vec<Vec3> {
        use vcod_cod4::ragdoll::CLIP_MASK;
        let z = Vec3::ZERO;
        let floor = |p: Vec3| {
            let t = clip.trace(p + Vec3::Z * 16.0, p - Vec3::Z * 64.0, z, z, CLIP_MASK);
            (t.fraction < 1.0 && !t.startsolid && t.normal.z > 0.99)
                .then(|| p + Vec3::Z * 16.0 - Vec3::Z * 80.0 * t.fraction)
        };
        let mut grid: Vec<Vec3> = (-4..=4)
            .flat_map(|i| (-4..=4).map(move |j| Vec3::new(i as f32, j as f32, 0.0) * 64.0))
            .collect();
        grid.sort_by(|a, b| a.length().total_cmp(&b.length()));
        let mut out: Vec<Vec3> = Vec::new();
        for e in &map.entities {
            let get = |key: &str| e.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str());
            if get("classname") != Some("mp_tdm_spawn") {
                continue;
            }
            let Some(o) = get("origin").and_then(vcod_common::bsp::parse_vec3) else {
                continue;
            };
            for g in &grid {
                let Some(f) = floor(Vec3::from(o) + *g) else {
                    continue;
                };
                let level = [Vec3::X, -Vec3::X, Vec3::Y, -Vec3::Y]
                    .iter()
                    .all(|d| floor(f + *d * 48.0).is_some_and(|g| (g.z - f.z).abs() < 1.0));
                let room = Vec3::new(48.0, 48.0, 72.0);
                let clear = !clip
                    .trace(
                        f + Vec3::Z * 2.0,
                        f + Vec3::Z * 2.0,
                        -room.with_z(0.0),
                        room,
                        CLIP_MASK,
                    )
                    .startsolid;
                if level && clear && out.iter().all(|s| s.distance(f) > 160.0) {
                    out.push(f);
                    break;
                }
            }
            if out.len() == n {
                break;
            }
        }
        out
    }

    /// vcod-server's `war` with eight bots held still, watched as a
    /// spectator and posed through `play::cod4_players` with the level's
    /// ragdoll: every 3 s one bot is put on a spot of open, level floor
    /// ([`open_floor_spots`]) and dies there by `kill`, so where a corpse
    /// falls is the test's, not a fight's. Each corpse whose entity takes
    /// `startRagdoll`'s type runs its ragdoll, which goes to sleep before
    /// `ragdoll_max_life` (or, for under half of them, freezes there),
    /// every point clear of the world, each that slept resting on what is
    /// under it, every def bone's segment clear (cod4-player-anims.md 10).
    /// Returns at once without an install.
    #[test]
    fn cod4_live_corpses_ragdoll_onto_the_floor() {
        use crate::play::cod4_players::{Cod4Players, RagdollWorld};
        use vcod_cod4::ragdoll::{CLIP_MASK, Dvars, State};
        use vcod_server::cod4::{Cod4Config, Cod4Server};
        let Some(install) = vcod_cod4::testing::cod4_install() else {
            return;
        };
        let root = install.root().to_path_buf();
        let world = crash_world().expect("mp_crash's world");
        let def = world.ragdoll.clone().expect("ragdoll.cfg's def");
        let (map, _) = vcod_cod4::map::Map::load(&install, "english", "mp_crash").unwrap();
        let spots = open_floor_spots(&world.clip, &map, 8);
        assert!(spots.len() >= 6, "{spots:?}");
        let mut sv = Cod4Server::new(
            Cod4Config {
                map: "mp_crash".into(),
                gametype: "war".into(),
                hostname: "vcod".into(),
                max_clients: 12,
            },
            11,
        );
        sv.set_cvar("scr_game_playerwaittime", "1");
        sv.set_cvar("scr_game_matchstarttime", "1");
        sv.set_cvar("sv_cheats", "1");
        sv.load(Some(&install)).unwrap();
        let bots: Vec<usize> = (0..spots.len())
            .map(|k| sv.add_held_bot_for_test(k % 2 == 1).unwrap())
            .collect();
        let sock = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
        sock.set_nonblocking(true).unwrap();
        let addr = sock.local_addr().unwrap();
        let clip = world.clip.clone();
        let clock = sim::Clock::new();
        let client = sim::spawn(&clock, move || {
            let join = MenuJoin {
                team: Some("spectator".into()),
                class: Some("assault_mp,0".into()),
            };
            let mut s = Cod4Session::connect(addr, "watcher", Cp::Bare, join).unwrap();
            s.set_world(world.clone());
            let mut players = Cod4Players::default();
            players.ragdoll = Some(RagdollWorld {
                def,
                clip: world.clip.clone(),
                dvars: Dvars::default(),
            });
            let mut loaded = None;
            // Per corpse (number, client, teleport bit): the last of its
            // ragdoll seen (its state, ms run, points), and whether its
            // entity ever read a ragdoll type.
            type Shape = (Vec<(Vec3, f32)>, Vec<(Vec3, Vec3, bool)>);
            type Seen = (Option<(State, f32, Shape)>, bool, bool);
            let mut corpses: BTreeMap<(u32, i32, i32), Seen> = BTreeMap::new();
            let t0 = clk_now();
            while since(t0) < Duration::from_secs(40) {
                let now = since(t0).as_secs_f64() * 1000.0;
                s.step(now).unwrap();
                s.take_commands();
                clk_sleep(Duration::from_millis(16));
                let Some(scene) = s.scene() else { continue };
                if loaded.is_none() {
                    let assets = s.entity_assets();
                    loaded =
                        Some(crate::cod4_fly::load_bodies(&root, "mp_crash", &assets).unwrap());
                }
                let (_, weapons, src) = loaded.as_ref().unwrap();
                let views: Vec<(u32, &BodyView)> = scene
                    .entities
                    .iter()
                    .filter_map(|d| Some((d.number, d.body.as_ref()?)))
                    .collect();
                players.posed(
                    src,
                    weapons,
                    &views,
                    &[],
                    scene.render_time,
                    scene.server_time,
                );
                for (n, v) in views.iter().filter(|(_, v)| v.corpse) {
                    let key = (*n, v.frame.client_num, v.frame.eflags & 2);
                    let e = corpses.entry(key).or_insert((None, false, false));
                    e.1 |= v.ragdoll;
                    e.2 |= players.ragdoll_refused(*n);
                    if let Some(r) = players.ragdoll_of(*n) {
                        let shape = (r.swept_spheres().collect(), r.segments().collect());
                        e.0 = Some((r.state, r.msec(), shape));
                    }
                }
            }
            s.disconnect();
            corpses
        });
        // Bot `k` goes onto spot `k` at 6 s + 3 s k and dies there two
        // frames later, once his feet are down.
        let mut placed: Vec<Option<i32>> = vec![None; bots.len()];
        let mut killed = 0;
        serve(&mut sv, &sock, &clock, &client, |sv, _, _| {
            let Some(l) = sv.game.level.as_mut() else {
                return true;
            };
            let now = l.level_time();
            for (k, &bot) in bots.iter().enumerate() {
                let in_play = l
                    .playerstate(bot)
                    .is_some_and(|p| p.get("pm_type") == Some(0));
                match placed[k] {
                    None if in_play && now >= 6000 + 3000 * k as i32 => {
                        let at = spots[k] + Vec3::Z * 60.0;
                        l.set_view_pos(bot, at.to_array(), [0.0, 90.0 * k as f32, 0.0]);
                        placed[k] = Some(now);
                    }
                    Some(t) if in_play && now - t >= 100 => {
                        l.kill_command(bot);
                        placed[k] = Some(i32::MAX);
                        killed += 1;
                    }
                    _ => {}
                }
            }
            true
        });
        assert_eq!(killed, bots.len(), "every bot died on his spot");
        let corpses = client.join().unwrap();
        let flipped = corpses.values().filter(|c| c.1).count();
        let refused = corpses.values().filter(|c| c.2).count();
        let asleep: Vec<_> = corpses
            .iter()
            .filter_map(|(k, c)| Some((k, c.0.as_ref()?)))
            .filter(|(_, (state, ..))| *state == State::Idle)
            .collect();
        println!(
            "{} corpses, {flipped} took a ragdoll type, {refused} refused, {} asleep",
            corpses.len(),
            asleep.len()
        );
        assert!(asleep.len() >= 2, "{corpses:#?}");
        // Retail freezes a body still moving at `ragdoll_max_life` too:
        // counted, and held to under half.
        let timed_out = asleep
            .iter()
            .filter(|(_, (_, ms, _))| *ms >= 4500.0)
            .count();
        println!("{timed_out} frozen at their max life");
        assert!(
            timed_out * 2 <= asleep.len(),
            "{timed_out} of {} at their max life",
            asleep.len()
        );
        let mut problems = Vec::new();
        let mut started_through = 0;
        let z = Vec3::ZERO;
        for (key, (_, ms, (points, bones))) in &asleep {
            for &(p, r) in points.iter() {
                let e = Vec3::splat((r - 0.5).max(0.0));
                if clip.trace(p, p, -e, e, CLIP_MASK).startsolid {
                    problems.push(format!("{key:?}: point {p} (radius {r}) in the world"));
                }
            }
            // A bone the death anim's pose already put through the world
            // is the anim's (the root motion vcod-server leaves out,
            // cod4-combat.md 5), not the ragdoll's.
            started_through += bones.iter().filter(|b| b.2).count();
            for &(a, b, _) in bones.iter().filter(|b| !b.2) {
                let t = clip.trace(a, b, z, z, CLIP_MASK);
                if t.startsolid || t.fraction < 1.0 {
                    problems.push(format!("{key:?}: bone {a} {b} through the world"));
                }
            }
            // One that went to sleep rests on what is under it: some
            // point whose sphere a 2-unit drop would put into the world.
            // One frozen at `ragdoll_max_life` holds wherever it was,
            // mid-fall too (`Ragdoll_CheckIdle`, cod4-player-anims.md
            // 10): a body that slid off a ledge late froze 9 units up.
            let rests = *ms >= 4500.0
                || points.iter().any(|&(p, r)| {
                    let e = Vec3::splat(r);
                    clip.trace(p, p - Vec3::Z * 2.0, -e, e, CLIP_MASK).fraction < 1.0
                });
            if !rests {
                problems.push(format!("{key:?}: rests on nothing"));
            }
        }
        println!("{started_through} bones started through the world");
        assert!(problems.is_empty(), "{problems:#?}");
    }

    /// One step of a ragdoll: the ms run, its state and each def bone's
    /// ends and rotation.
    type RagStep = (
        f32,
        vcod_cod4::ragdoll::State,
        Vec<(Vec3, Vec3, glam::Quat)>,
    );
    /// Per corpse (number, client, teleport bit), its ragdoll each step.
    type RagFrames = BTreeMap<(u32, i32, i32), Vec<RagStep>>;

    /// [`cod4_live_corpses_ragdoll_onto_the_floor`]'s scene (six held bots
    /// killed on open floor, 3 s apart, posed by a spectator through
    /// `Cod4Players`) with the client's frame `frame_ms` long; every
    /// ragdoll recorded on each frame its step moved it.
    fn live_ragdoll_frames(frame_ms: u64) -> Option<RagFrames> {
        use crate::play::cod4_players::{Cod4Players, RagdollWorld};
        use vcod_cod4::ragdoll::Dvars;
        use vcod_server::cod4::{Cod4Config, Cod4Server};
        let install = vcod_cod4::testing::cod4_install()?;
        let root = install.root().to_path_buf();
        let world = crash_world().expect("mp_crash's world");
        let def = world.ragdoll.clone().expect("ragdoll.cfg's def");
        let (map, _) = vcod_cod4::map::Map::load(&install, "english", "mp_crash").unwrap();
        let spots = open_floor_spots(&world.clip, &map, 6);
        let mut sv = Cod4Server::new(
            Cod4Config {
                map: "mp_crash".into(),
                gametype: "war".into(),
                hostname: "vcod".into(),
                max_clients: 12,
            },
            11,
        );
        sv.set_cvar("scr_game_playerwaittime", "1");
        sv.set_cvar("scr_game_matchstarttime", "1");
        sv.set_cvar("sv_cheats", "1");
        sv.load(Some(&install)).unwrap();
        let bots: Vec<usize> = (0..spots.len())
            .map(|k| sv.add_held_bot_for_test(k % 2 == 1).unwrap())
            .collect();
        let sock = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
        sock.set_nonblocking(true).unwrap();
        let addr = sock.local_addr().unwrap();
        let clock = sim::Clock::new();
        let client = sim::spawn(&clock, move || {
            let join = MenuJoin {
                team: Some("spectator".into()),
                class: Some("assault_mp,0".into()),
            };
            let mut s = Cod4Session::connect(addr, "watcher", Cp::Bare, join).unwrap();
            s.set_world(world.clone());
            let mut players = Cod4Players::default();
            players.ragdoll = Some(RagdollWorld {
                def,
                clip: world.clip.clone(),
                dvars: Dvars::default(),
            });
            let mut loaded = None;
            let mut out: RagFrames = BTreeMap::new();
            let t0 = clk_now();
            while since(t0) < Duration::from_secs(32) {
                let now = since(t0).as_secs_f64() * 1000.0;
                s.step(now).unwrap();
                s.take_commands();
                clk_sleep(Duration::from_millis(frame_ms));
                let Some(scene) = s.scene() else { continue };
                if loaded.is_none() {
                    let assets = s.entity_assets();
                    loaded =
                        Some(crate::cod4_fly::load_bodies(&root, "mp_crash", &assets).unwrap());
                }
                let (_, weapons, src) = loaded.as_ref().unwrap();
                let views: Vec<(u32, &BodyView)> = scene
                    .entities
                    .iter()
                    .filter_map(|d| Some((d.number, d.body.as_ref()?)))
                    .collect();
                players.posed(
                    src,
                    weapons,
                    &views,
                    &[],
                    scene.render_time,
                    scene.server_time,
                );
                for (n, v) in views.iter().filter(|(_, v)| v.corpse) {
                    let key = (*n, v.frame.client_num, v.frame.eflags & 2);
                    if let Some(r) = players.ragdoll_of(*n) {
                        let e = out.entry(key).or_default();
                        if e.last().is_none_or(|l| l.0 != r.msec()) {
                            e.push((r.msec(), r.state, r.frames().collect()));
                        }
                    }
                }
            }
            s.disconnect();
            out
        });
        let mut placed: Vec<Option<i32>> = vec![None; bots.len()];
        serve(&mut sv, &sock, &clock, &client, |sv, _, _| {
            let Some(l) = sv.game.level.as_mut() else {
                return true;
            };
            let now = l.level_time();
            for (k, &bot) in bots.iter().enumerate() {
                let in_play = l
                    .playerstate(bot)
                    .is_some_and(|p| p.get("pm_type") == Some(0));
                match placed[k] {
                    None if in_play && now >= 6000 + 3000 * k as i32 => {
                        let at = spots[k] + Vec3::Z * 60.0;
                        l.set_view_pos(bot, at.to_array(), [0.0, 90.0 * k as f32, 0.0]);
                        placed[k] = Some(now);
                    }
                    Some(t) if in_play && now - t >= 100 => {
                        l.kill_command(bot);
                        placed[k] = Some(i32::MAX);
                    }
                    _ => {}
                }
            }
            true
        });
        Some(client.join().unwrap())
    }

    /// The angle of `q`, degrees.
    fn turn_deg(q: glam::Quat) -> f32 {
        2f32.to_degrees() * q.xyz().length().atan2(q.w.abs())
    }

    /// What a corpse's ragdoll did, from its steps: the neck bone's
    /// widest twist about itself in the upper spine's frame, against the
    /// shortest turn of its axis from its first step, degrees; and over
    /// its last 900 ms (`phys_autoDisableTime`, the time it lay still
    /// before it slept) the farthest a bone's end moved and the most the
    /// pelvis and spine bones turned, degrees.
    fn ragdoll_spin(steps: &[RagStep]) -> (f32, f32, f32) {
        // ragdoll.cfg's def 1: bones 0..2 the pelvis and spine, 3 the neck.
        let (spine, neck) = (2, 3);
        let in_spine = |f: &[(Vec3, Vec3, glam::Quat)]| {
            let r = f[spine].2.inverse();
            (
                (r * (f[neck].1 - f[neck].0)).normalize_or_zero(),
                r * f[neck].2,
            )
        };
        let (d0, rel0) = in_spine(&steps[0].2);
        let twist = steps
            .iter()
            .map(|s| {
                let (d, rel) = in_spine(&s.2);
                turn_deg((glam::Quat::from_rotation_arc(d0, d) * rel0).inverse() * rel)
            })
            .fold(0.0, f32::max);
        let last = steps.last().unwrap();
        let k = steps
            .iter()
            .position(|s| s.0 >= last.0 - 900.0)
            .unwrap_or(0);
        let ends = steps[k].2.iter().zip(&last.2);
        let moved = ends
            .clone()
            .map(|(x, y)| x.0.distance(y.0).max(x.1.distance(y.1)))
            .fold(0.0, f32::max);
        let turned = ends
            .take(3)
            .map(|(x, y)| turn_deg(x.2.inverse() * y.2))
            .fold(0.0, f32::max);
        (twist, moved, turned)
    }

    /// The corpses of [`live_ragdoll_frames`] at 250 and at 10 frames a
    /// second: none twists its neck past `ragdoll.cfg`'s 30 degrees (its
    /// limit 2's x, which 1.7 holds in an angular motor), and each that
    /// went to sleep before `ragdoll_max_life` lay still over the 900 ms
    /// before it did, no bone end moving a unit and the pelvis and spine
    /// turning under 5 degrees (cod4-player-anims.md 10.2). Prints each
    /// corpse's numbers. Returns at once without an install.
    #[test]
    fn cod4_live_ragdolls_hold_still_once_down() {
        for frame_ms in [4, 100] {
            let Some(frames) = live_ragdoll_frames(frame_ms) else {
                return;
            };
            assert!(
                frames.len() >= 4,
                "{frame_ms} ms: {} ragdolls",
                frames.len()
            );
            let mut slept = 0;
            let mut problems = Vec::new();
            for (key, steps) in &frames {
                let (twist, moved, turned) = ragdoll_spin(steps);
                let &(ms, state, _) = steps.last().unwrap();
                println!(
                    "{frame_ms} ms {key:?}: {state:?} at {ms:.0} ms; neck twist {twist:.1} deg; last 900 ms moved {moved:.2}, pelvis and spine turned {turned:.1} deg"
                );
                if twist > 30.0 {
                    problems.push(format!("{frame_ms} ms {key:?}: neck twist {twist}"));
                }
                if state == vcod_cod4::ragdoll::State::Idle && ms < 4500.0 {
                    slept += 1;
                    if moved > 1.0 || turned > 5.0 {
                        problems.push(format!(
                            "{frame_ms} ms {key:?}: moved {moved}, turned {turned}"
                        ));
                    }
                }
            }
            assert!(slept * 2 >= frames.len(), "{frame_ms} ms: {slept} slept");
            assert!(problems.is_empty(), "{problems:#?}");
        }
    }

    /// vcod-server's `war` with eight shooting bots, watched as a
    /// spectator posing every body with the level's ragdoll: each
    /// grenade, rocket or script blast the server sends turns into the
    /// ragdoll push of its weapon's `explosionRadius` (or the script's
    /// radii) through [`cod4_players::explosion_of`], and pushes the
    /// running and sleeping ragdolls around it (cod4-player-anims.md
    /// 10.1). Prints how many blasts reached a body. Returns at once
    /// without an install.
    #[test]
    fn cod4_live_blasts_push_the_ragdolls() {
        use crate::play::cod4_players::{Cod4Players, RagdollWorld, explosion_of};
        use vcod_cod4::ragdoll::Dvars;
        use vcod_server::cod4::{Cod4Config, Cod4Server};
        let Some(install) = vcod_cod4::testing::cod4_install() else {
            return;
        };
        let root = install.root().to_path_buf();
        let world = crash_world().expect("mp_crash's world");
        let def = world.ragdoll.clone().expect("ragdoll.cfg's def");
        let ((sounds, _), _) = crate::cod4_fly::load_av(&root, "mp_crash").unwrap();
        let defs = sounds.weapons.clone();
        let mut sv = Cod4Server::new(
            Cod4Config {
                map: "mp_crash".into(),
                gametype: "war".into(),
                hostname: "vcod".into(),
                max_clients: 12,
            },
            13,
        );
        sv.set_cvar("scr_game_playerwaittime", "1");
        sv.set_cvar("scr_game_matchstarttime", "1");
        sv.load(Some(&install)).unwrap();
        sv.add_bots(8, true);
        let (events, built, reached, missing) = run_held(
            &mut sv,
            move |addr| {
                let join = MenuJoin {
                    team: Some("spectator".into()),
                    class: Some("assault_mp,0".into()),
                };
                let mut s = Cod4Session::connect(addr, "watcher", Cp::Bare, join).unwrap();
                s.set_world(world.clone());
                let mut players = Cod4Players::default();
                players.ragdoll = Some(RagdollWorld {
                    def,
                    clip: world.clip.clone(),
                    dvars: Dvars::default(),
                });
                let mut loaded = None;
                // (explode events, pushes built, ragdolls reached, the
                // explodes whose weapon the client has no def of)
                let (mut events, mut built, mut reached) = (0, 0, 0);
                let mut missing = Vec::new();
                let t0 = clk_now();
                while since(t0) < Duration::from_secs(60) {
                    let now = since(t0).as_secs_f64() * 1000.0;
                    s.step(now).unwrap();
                    s.take_commands();
                    clk_sleep(Duration::from_millis(16));
                    let fired = s.take_fired();
                    let Some(gs) = s.gamestate() else { continue };
                    let weapons = weapon_list(gs);
                    for f in &fired {
                        let explode =
                            f.name.contains("_EXPLODE") && f.name != "EV_FLASHBANG_EXPLODE";
                        events += usize::from(explode);
                        if let Some(e) = explosion_of(f, &weapons, &defs) {
                            built += 1;
                            reached += players.explode(&e);
                        } else if explode
                            && !weapon_name(&weapons, f.weapon)
                                .is_some_and(|w| defs.contains_key(w))
                        {
                            missing.push((f.name, f.weapon));
                        }
                    }
                    let Some(scene) = s.scene() else { continue };
                    if loaded.is_none() {
                        loaded = Some(
                            crate::cod4_fly::load_bodies(&root, "mp_crash", &s.entity_assets())
                                .unwrap(),
                        );
                    }
                    let (_, weapons, src) = loaded.as_ref().unwrap();
                    let views: Vec<(u32, &BodyView)> = scene
                        .entities
                        .iter()
                        .filter_map(|d| Some((d.number, d.body.as_ref()?)))
                        .collect();
                    players.posed(
                        src,
                        weapons,
                        &views,
                        &[],
                        scene.render_time,
                        scene.server_time,
                    );
                }
                s.disconnect();
                (events, built, reached, missing)
            },
            |_, _, _| true,
        );
        println!("{events} explodes, {built} pushes, {reached} ragdolls reached");
        assert!(events > 0, "no blast in a minute of bots");
        assert!(
            missing.is_empty(),
            "explodes of unknown weapons: {missing:?}"
        );
        assert!(built > 0, "no explode turned into a push");
    }

    /// The retail kill capture's corpse (64) as its shooter saw it: its
    /// entity turns ragdoll at 49550 (cod4-combat.md 5.1), when the death
    /// anim has put its hand into the wall it fell against, so the tunnel
    /// test refuses the ragdoll and the corpse keeps its death anim's pose
    /// (cod4-player-anims.md 10). Fixed 60 Hz frames, so it repeats.
    #[test]
    fn the_kill_captures_corpse_fails_the_tunnel_test() {
        use crate::play::cod4_players::{Cod4Players, RagdollWorld};
        use vcod_cod4::ragdoll::Dvars;
        let Some(install) = vcod_cod4::testing::cod4_install() else {
            return;
        };
        let world = crash_world().expect("mp_crash's world");
        let text =
            std::fs::read_to_string(format!("{FIXTURES}/play-1.7-kill-shooter.txt")).unwrap();
        let mut s = Cod4Session::open_capture(&text).unwrap();
        let (_, weapons, src) =
            crate::cod4_fly::load_bodies(install.root(), "mp_crash", &s.entity_assets()).unwrap();
        let mut players = Cod4Players::default();
        players.ragdoll = Some(RagdollWorld {
            def: world.ragdoll.clone().expect("ragdoll.cfg's def"),
            clip: world.clip.clone(),
            dvars: Dvars::default(),
        });
        let (mut ms, mut flipped_at, mut refused_at) = (0.0, None, None);
        while !s.finished() {
            ms += 1000.0 / 60.0;
            s.step(ms).unwrap();
            let Some(scene) = s.scene() else { continue };
            let views: Vec<(u32, &BodyView)> = scene
                .entities
                .iter()
                .filter_map(|d| Some((d.number, d.body.as_ref()?)))
                .collect();
            players.posed(
                &src,
                &weapons,
                &views,
                &[],
                scene.render_time,
                scene.server_time,
            );
            if views.iter().any(|(n, v)| *n == 64 && v.ragdoll) {
                flipped_at.get_or_insert(scene.server_time);
            }
            if players.ragdoll_refused(64) {
                refused_at.get_or_insert(scene.server_time);
            }
            assert!(players.ragdoll_of(64).is_none(), "a ragdoll ran");
        }
        assert_eq!(flipped_at, Some(49_550));
        assert_eq!(refused_at, Some(49_550));
    }

    /// Three clients on vcod-server's war with `sv_voice 1`: we (allies)
    /// hold `+talk` with a tone for a microphone, through the session's
    /// batch (`CL_VoiceTransmit`) out of band to the server, which relays
    /// each frame to our teammate (allies) and not the enemy (axis); with
    /// `voice_global 1` the enemy hears too. The teammate's talker list
    /// names us, and the server stamps our `lastVoiceTime`. With the
    /// system's libspeex the frames are Speex: 20 bytes at quality 3,
    /// which decode back to the tone; without it the test sends fixed
    /// frames (cod4-voice.md 5).
    #[test]
    fn cod4_live_voice_reaches_the_team_and_voice_global_the_enemy() {
        use crate::cod4_voice::{Speex, Tone, VoiceChat, VoiceFrame, VoiceInput, may_send};
        use crate::play::input::Action;
        use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};
        use vcod_server::cod4::{Cod4Config, Cod4Server};
        let Some(install) = vcod_cod4::testing::cod4_install() else {
            return;
        };
        struct Steady(Tone);
        impl VoiceInput for Steady {
            fn take(&mut self) -> Vec<i16> {
                // 16 ms of 8000 Hz a frame, as the clock steps.
                self.0.samples(128)
            }
        }
        let mut sv = Cod4Server::new(
            Cod4Config {
                map: "mp_crash".into(),
                gametype: "war".into(),
                hostname: "vcod".into(),
                max_clients: 4,
            },
            7,
        );
        sv.set_cvar("scr_game_playerwaittime", "1");
        sv.set_cvar("scr_game_matchstarttime", "1");
        sv.set_cvar("sv_voice", "1");
        sv.load(Some(&install)).unwrap();
        let sock = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
        sock.set_nonblocking(true).unwrap();
        let addr = sock.local_addr().unwrap();
        let global = Arc::new(AtomicBool::new(false));
        let stamped = Arc::new(AtomicI32::new(-1));
        let (g, st) = (global.clone(), stamped.clone());
        let clock = sim::Clock::new();
        #[derive(Debug, Default)]
        struct Seen {
            mate: Vec<(u8, Vec<u8>)>,
            enemy_before: usize,
            enemy_after: Vec<(u8, Vec<u8>)>,
            mate_listed: bool,
            sent: u64,
        }
        let client = sim::spawn(&clock, move || {
            let join = |team: &str| MenuJoin {
                team: Some(team.into()),
                class: Some("assault_mp,0".into()),
            };
            let mut t = Cod4Session::connect(addr, "talker", Cp::Bare, join("allies")).unwrap();
            let mut m = Cod4Session::connect(addr, "mate", Cp::Bare, join("allies")).unwrap();
            let mut e = Cod4Session::connect(addr, "enemy", Cp::Bare, join("axis")).unwrap();
            let speex = Speex::open();
            let real = speex.is_some();
            let mut chat = VoiceChat::with(speex, Some(Box::new(Steady(Tone::new()))));
            let mut seen = Seen::default();
            let t0 = clk_now();
            let mut talk_from = None;
            while since(t0) < Duration::from_secs(60) {
                let now = since(t0).as_secs_f64() * 1000.0;
                for c in [&mut t, &mut m, &mut e] {
                    c.step(now).unwrap();
                    c.take_commands();
                    c.take_fired();
                }
                let all = [&t, &m, &e].iter().all(|c| c.spawn().is_some());
                if all && talk_from.is_none() {
                    talk_from = Some(now);
                    t.input.key(Action::Talk, true);
                }
                let (sv_voice, quality) = t.voice_settings();
                let held = t.input.held(Action::Talk);
                let f = VoiceFrame {
                    sending: may_send(
                        held,
                        t.client().snapshots.last().is_some(),
                        sv_voice,
                        true,
                        25000,
                    ),
                    quality,
                };
                if real {
                    chat.frame(&mut t.voice, &f, None);
                } else if f.sending {
                    t.voice.queue(vec![0x5a; 20]);
                }
                let mate_heard = m.voice.take_heard();
                if !mate_heard.is_empty() && m.talkers() == vec![0] {
                    seen.mate_listed = true;
                }
                seen.mate.extend(mate_heard);
                let enemy_heard = e.voice.take_heard();
                if g.load(Ordering::Relaxed) {
                    seen.enemy_after.extend(enemy_heard);
                } else {
                    seen.enemy_before += enemy_heard.len();
                }
                match talk_from.map(|t0| now - t0) {
                    Some(ms) if ms > 4000.0 => break,
                    Some(ms) if ms > 2000.0 => g.store(true, Ordering::Relaxed),
                    _ => {}
                }
                clk_sleep(Duration::from_millis(16));
            }
            seen.sent = chat.sent;
            for c in [&mut t, &mut m, &mut e] {
                c.disconnect();
            }
            (seen, real)
        });
        let mut set = false;
        serve(&mut sv, &sock, &clock, &client, |sv, _, _| {
            if global.load(Ordering::Relaxed) && !set {
                set = true;
                sv.set_cvar("voice_global", "1");
            }
            if let Some(t) = sv.game.level.as_ref().and_then(|l| l.last_voice_time(0)) {
                st.store(t, Ordering::Relaxed);
            }
            true
        });
        let (seen, real) = client.join().unwrap();
        assert!(
            seen.mate.len() > 50,
            "{} frames reached the mate",
            seen.mate.len()
        );
        assert!(seen.mate.iter().all(|(t, _)| *t == 0));
        assert!(seen.mate_listed, "the mate's talker list never named us");
        assert_eq!(
            seen.enemy_before, 0,
            "the enemy heard us without voice_global"
        );
        assert!(
            !seen.enemy_after.is_empty(),
            "voice_global relayed nothing to the enemy"
        );
        assert!(stamped.load(Ordering::Relaxed) > 0, "no lastVoiceTime");
        if real {
            assert!(seen.sent as usize >= seen.mate.len());
            assert!(seen.mate.iter().all(|(_, f)| f.len() == 20));
            let mut dec = Speex::open().unwrap();
            let pcm: Vec<i16> = seen.mate[20..40]
                .iter()
                .flat_map(|(_, f)| dec.decode(f))
                .collect();
            assert_eq!(pcm.len(), 20 * 160);
            let rms =
                (pcm.iter().map(|&s| f64::from(s).powi(2)).sum::<f64>() / pcm.len() as f64).sqrt();
            assert!(rms > 1000.0, "the tone decoded to rms {rms}");
        }
    }
}
