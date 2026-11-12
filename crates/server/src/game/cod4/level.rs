//! A running CoD4 1.7 level: the script runtime over the zones, and each
//! client's playerstate as `ClientConnect`, `ClientSpawn`, `ClientThink`
//! and `ClientEndFrame` keep it (docs/research/cod4-gsc.md 7). Each cmd runs
//! `vcod_cod4::pmove` as it arrives, over the map's clip map, the other
//! bodies and the brush entities ([`super::clip::Cod4Clip`],
//! docs/research/cod4-pmove.md 15).
//!
//! The playerstate rules are 1.7's where the walk capture shows them
//! (`play-1.7-walk.txt`, cod4-netfields.md 8), and INFERRED
//! elsewhere unless marked.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

use glam::Vec3;
use vcod_cod4::collision::ClipMap;
use vcod_cod4::pmove::{self, Cmd, Rules};

use vcod_cod4::install::Install;
use vcod_cod4::net::clmsg::UserCmd;
use vcod_cod4::net::delta::{ClientState, ENTITYNUM_NONE, EntityState, PlayerState};
use vcod_cod4::net::fields::{CLIENT_STATE, PLAYER_STATE};
use vcod_cod4::net::stats::Stats;

use super::Cod4Zones;
use super::clip::{Body4, Brush4, Cod4Clip};
use crate::follow::SessionState;
use crate::game::host::{ClientEvent, SpawnMode};
use crate::game::script::{MenuRef, ScriptRuntime};
use vcod_gsc::Value;

/// How far back the killcam may look: 10 s of frames (200 at `sv_fps` 20).
const HISTORY_MS: i32 = 10_000;

/// `pm_type`s: the walk capture's spectator reads 4, a playing
/// client 0. VERIFIED live.
pub const PM_NORMAL: u32 = 0;
/// A player `linkTo` holds (a plant or defuse), alive and dead.
pub const PM_NORMAL_LINKED: u32 = 1;
pub const PM_DEAD_LINKED: u32 = 8;
pub const PM_SPECTATOR: u32 = 4;
pub const PM_INTERMISSION: u32 = 5;
/// `PM_LASTSTAND`: Last Stand's down state, a body still in play.
pub const PM_LASTSTAND: u32 = 6;
pub const PM_DEAD: u32 = 7;

/// A body in play: `pm_type` normal (linked or not) or last stand.
pub fn in_play(pm_type: u32) -> bool {
    matches!(pm_type, PM_NORMAL | PM_NORMAL_LINKED | PM_LASTSTAND)
}

/// `otherFlags`: following someone (2), a player in the world
/// (4, `POF_PLAYER`), may follow a team (8), may fly free (0x10). The walk
/// capture's 8 on a team with no class, 0xa following the teammate, 4 on
/// the spawn. VERIFIED live.
const POF_FOLLOW: u32 = 2;
const POF_PLAYER: u32 = 4;
/// `pingPlayer`'s `eFlags` bit (`cod4_lnxded` 0x80a884c).
const EF_PING: u32 = 0x40_0000;
const POF_SPECTATE_TEAM: u32 = 8;
const POF_SPECTATE_FREE: u32 = 0x10;

/// `TEAM_*` and the no-spectate bits `allowSpectateTeam` sets: bit `team`,
/// `freelook` bit 4 (`TEAM_NUM_TEAMS`), as vcod's shared builtin keeps them
/// (`GameHost::client_no_spectate`).
const TEAM_FREE: u32 = 0;
const TEAM_AXIS: u32 = 1;
const TEAM_ALLIES: u32 = 2;
const TEAM_SPECTATOR: u32 = 3;
const NO_SPECTATE_FREELOOK: u8 = 1 << 4;

/// A playerstate word by field name, the index looked up once.
fn ps_word(name: &str) -> usize {
    PLAYER_STATE
        .iter()
        .find(|f| f.name == name)
        .unwrap_or_else(|| panic!("no playerstate field {name}"))
        .word()
}

fn cs_word(name: &str) -> usize {
    CLIENT_STATE
        .iter()
        .find(|f| f.name == name)
        .unwrap_or_else(|| panic!("no client field {name}"))
        .word()
}

fn ent_word(etype: u32, name: &str) -> Option<usize> {
    EntityState::table(etype)
        .iter()
        .find(|f| f.name == name)
        .map(|f| f.word())
}

/// An entity's word by field name in its type's table, 0 for none.
pub(super) fn ent_u32(e: &EntityState, name: &str) -> u32 {
    ent_word(e.etype(), name).map_or(0, |w| e.0[w])
}

/// A spectator's refusals (`setSpectatePermissions`, client +0x3004):
/// bit `1 << team` refuses watching that team, [`NO_SPECTATE_FREELOOK`]
/// free flight.
#[derive(Clone, Copy)]
struct Refused(u8);

impl Refused {
    fn team(self, team: u32) -> bool {
        self.0 & (1 << team) != 0
    }

    fn free_flight(self) -> bool {
        self.0 & NO_SPECTATE_FREELOOK != 0
    }

    fn every_team(self) -> bool {
        [TEAM_FREE, TEAM_AXIS, TEAM_ALLIES]
            .into_iter()
            .all(|t| self.team(t))
    }
}

/// A watcher's `eFlags`: the watched playerstate's, but bit 0x100000,
/// which stays the watcher's own (0x80a4ba9..0x80a4bc0). VERIFIED live:
/// the walker following its mate keeps its own 0x100000 (cod4-gsc.md 7).
fn followed_eflags(own: &PlayerState, theirs: &PlayerState) -> u32 {
    const OWN: u32 = 0x10_0000;
    theirs.word("eFlags") & !OWN | own.word("eFlags") & OWN
}

/// Field-name access on a playerstate.
pub(super) trait PsExt {
    fn set(&mut self, name: &str, v: u32);
    fn setf(&mut self, name: &str, v: f32) {
        self.set(name, v.to_bits());
    }
    fn word(&self, name: &str) -> u32;
}

impl PsExt for PlayerState {
    fn set(&mut self, name: &str, v: u32) {
        self.words[ps_word(name)] = v;
    }
    fn word(&self, name: &str) -> u32 {
        self.words[ps_word(name)]
    }
}

/// One client in the level.
pub struct Player4 {
    pub ps: PlayerState,
    /// The newest cmd thought (`sess.cmd`) and the one before
    /// (`sess.oldcmd`).
    pub cmd: UserCmd,
    pub oldcmd: Cmd,
    /// `ps.oldVelocity`, which the wire does not carry.
    pub old_velocity: [f32; 2],
    /// `client->spectatorClient`: the slot followed, -1 for none.
    pub spectator_client: i32,
    /// The archived frame a killcam shows this client, by level time.
    pub killcam_at: Option<i32>,
    /// The anim script's `MOVETYPE` and `STRAFING` conditions
    /// (`clientInfo_t.clientConditions`), which the wire does not carry.
    pub anim_conditions: (u8, u8),
    /// `client->useHoldEntity` and `useHoldTime` (`super::use_key`).
    pub use_hold: Option<(vcod_gsc::EntId, i32)>,
    /// A script `linkTo` holds the player (`ent->tagInfo`), until its
    /// `unlink` or the next spawn.
    pub linked: bool,
    /// The body's event ring `BG_PlayerToEntityProcessEvents` fills.
    pub body_events: BodyEvents,
    /// `ent->active`: 1 on a gun, 2 a release a use press asked for
    /// (`super::turret`).
    pub turret_active: u8,
    /// The gun's `triggerDown`, which holds a single-shot gun to one round
    /// a press.
    pub trigger_down: bool,
    /// `client->inactivityTime` and `inactivityWarning`
    /// ([`Cod4Level::inactivity_timer`]).
    pub inactivity_time: i32,
    pub inactivity_warned: bool,
}

/// [`Cod4Level::inactivity_timer`]'s verdict on one cmd.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Inactivity {
    Keep,
    /// Ten seconds left: `c "GAME_INACTIVEDROPWARNING"`, once.
    Warn,
    /// Time is up: `GAME_DROPPEDFORINACTIVITY`, and the cmd is not thought.
    Drop,
}

/// The player entity's own event ring, refilled each end frame from the
/// playerstate's (`BG_PlayerToEntityProcessEvents`, `cod4_lnxded`
/// 0x8052b84..0x8052ca0, cod4-pmove.md 15.2): `taken` is how far the
/// playerstate's sequence has been copied (`ps.oldEventSequence`, +0xd8),
/// `seq` the entity's `eventSequence`, a byte.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct BodyEvents {
    taken: i32,
    seq: u32,
    events: [u32; 4],
    parms: [u32; 4],
}

/// The events the body never shows: 1.7's `serverOnlyEvents` and
/// `singleClientEvents` (`cod4_lnxded` 0x820d954 and 0x820d920, file
/// 0x1c5954 and 0x1c5920, each ended by -1), read off as bytes.
const SERVER_ONLY_EVENTS: [u32; 3] = [31, 20, 19];
const SINGLE_CLIENT_EVENTS: [u32; 12] = [6, 7, 8, 34, 13, 14, 32, 33, 34, 37, 42, 43];

impl BodyEvents {
    /// Copies what the playerstate's ring gained since the last call: at
    /// most its four newest, and nothing when its sequence went back (a
    /// fresh playerstate). Event and parm are read as bytes; the hidden
    /// events are skipped without a slot.
    fn take(&mut self, ps: &PlayerState) {
        let newest = ps.word("eventSequence") as i32;
        let from = self.taken.max(newest.saturating_sub(4)).min(newest);
        for n in from..newest {
            let ring = (n & 3) as usize;
            let event = ps.word(&format!("events[{ring}]")) & 0xff;
            let hidden = SERVER_ONLY_EVENTS
                .iter()
                .chain(&SINGLE_CLIENT_EVENTS)
                .any(|&e| e == event);
            if !hidden {
                self.push(event, ps.word(&format!("eventParms[{ring}]")) & 0xff);
            }
        }
        self.taken = newest;
    }

    fn push(&mut self, event: u32, parm: u32) {
        let at = (self.seq % 4) as usize;
        self.events[at] = event;
        self.parms[at] = parm;
        self.seq = (self.seq + 1) % 256;
    }

    /// The ring onto the body's entity.
    fn write(&self, e: &mut EntityState) {
        let mut set = |name: &str, v: u32| {
            if let Some(w) = ent_word(1, name) {
                e.0[w] = v;
            }
        };
        set("eventSequence", self.seq);
        for k in 0..4 {
            set(&format!("events[{k}]"), self.events[k]);
            set(&format!("eventParms[{k}]"), self.parms[k]);
        }
    }
}

pub struct Cod4Level {
    pub rt: ScriptRuntime,
    pub players: Vec<Option<Player4>>,
    /// Each client's playerstate as the last frame left it: what a
    /// follower copies (`SV_GetArchivedClientInfo` at archive time 0, the
    /// newest archived frame).
    archive: Vec<Option<PlayerState>>,
    /// Every frame's playerstates, newest first, for the killcam's look
    /// back (`SV_GetArchivedClientInfo` at an `archivetime`).
    history: std::collections::VecDeque<(i32, Vec<Option<PlayerState>>)>,
    level_time: i32,
    /// `level.previousTime`: the last frame's `level.time` (`G_RunFrame`,
    /// 0x80b6212).
    previous_time: i32,
    /// `sv_fps`, whose `1000 / sv_fps` the server steps ([`Self::set_sv_fps`]).
    sv_fps: i32,
    /// The map's clip map; `None` leaves every client where it stands.
    pub(super) clip: Option<Arc<ClipMap>>,
    /// The shots' state (`combat`).
    pub(super) combat: super::combat::Combat4,
    /// The movement dvars (`Rules::default`, what the mirror carries on
    /// every stock 1.7 level, cod4-pmove.md 12).
    pub rules: Rules,
    /// The brush entities pmove clips, as the last frame left them.
    pub(super) brushes: Vec<Brush4>,
    /// Each snapshot round's entities by level time, newest first, with
    /// the numbers of its temp entities each client may not see: the
    /// killcam's look back (`SV_ArchiveSnapshot`'s cached entities).
    #[allow(clippy::type_complexity)]
    entity_history: std::collections::VecDeque<(
        i32,
        Arc<BTreeMap<u32, EntityState>>,
        Arc<Vec<(u32, crate::game::temp_entity::Scope)>>,
    )>,
    /// The mantles' blockages ([`Self::spawn_mantle_block`]): the entity,
    /// its think's time and its box.
    mantle_blocks: Vec<(vcod_gsc::EntId, i32, Body4)>,
    /// Each gun's own event ring (`EV_FIRE_WEAPON_MG42`).
    pub(super) gun_events: std::collections::HashMap<vcod_gsc::EntId, super::turret::GunEvents>,
}

impl Cod4Level {
    /// The zones' level: `game::cod4::load`'s bootstrap at `now_ms`.
    #[allow(clippy::too_many_arguments)]
    pub fn load(
        zones: &Cod4Zones,
        install: &Install,
        map: &str,
        gametype: &str,
        cvars: crate::cvars::Cvars,
        fresh_mirror: bool,
        overrides: Option<PathBuf>,
        now_ms: i32,
    ) -> anyhow::Result<Cod4Level> {
        Self::load_with(
            zones,
            install,
            map,
            gametype,
            cvars,
            fresh_mirror,
            overrides,
            now_ms,
            Default::default(),
            None,
            super::GAME_SEED,
        )
    }

    /// [`Self::load`] with what the last level handed on
    /// ([`super::load_with`], [`Self::take_carry`]).
    #[allow(clippy::too_many_arguments)]
    pub fn load_with(
        zones: &Cod4Zones,
        install: &Install,
        map: &str,
        gametype: &str,
        cvars: crate::cvars::Cvars,
        fresh_mirror: bool,
        overrides: Option<PathBuf>,
        now_ms: i32,
        carry: crate::game::script::Carry,
        configstrings: Option<Vec<String>>,
        seed: u32,
    ) -> anyhow::Result<Cod4Level> {
        let rt = super::load_with(
            zones,
            install,
            map,
            gametype,
            cvars,
            fresh_mirror,
            overrides,
            now_ms,
            carry,
            configstrings,
            seed,
        )?;
        let n = crate::server::MAX_CLIENTS;
        // The precached shellshocks whose record slows the walk, for
        // pmove's `PM_CmdScaleForWeapon` (cod4-combat.md 10.8).
        let (first, last) = rt
            .host
            .allocators
            .layout()
            .bounds(vcod_common::net::protocol::CsRange::ShellShock);
        let names: Vec<String> = (first..=last)
            .map(|i| rt.host.configstrings[i].to_string())
            .collect();
        let source = zones.source.clone();
        let read = move |path: &str| {
            use vcod_common::asset_source::AssetSource;
            source
                .read(path)
                .map(|b| String::from_utf8_lossy(&b).into_owned())
        };
        let rules = Rules {
            shock_slows: vcod_cod4::shock::movement_bits(&names, &read),
            ..Rules::default()
        };
        let sv_fps = super::sv_fps(&rt.host.cvars);
        Ok(Cod4Level {
            rt,
            players: (0..n).map(|_| None).collect(),
            archive: vec![None; n],
            history: std::collections::VecDeque::new(),
            level_time: now_ms,
            // A frame back, so the first frame's `frametime` reads one.
            previous_time: now_ms - 1000 / sv_fps,
            sv_fps,
            clip: zones.clip.clone(),
            combat: super::combat::Combat4::new(zones.bodies.clone(), n),
            rules,
            brushes: Vec::new(),
            mantle_blocks: Vec::new(),
            gun_events: Default::default(),
            entity_history: std::collections::VecDeque::new(),
        })
    }

    pub fn level_time(&self) -> i32 {
        self.level_time
    }

    /// The brush entities pmove clips, as the last frame left them.
    pub fn brushes(&self) -> &[Brush4] {
        &self.brushes
    }

    /// `level.previousTime` (0x836f8b0), what a missile's move this frame
    /// started from.
    pub fn previous_time(&self) -> i32 {
        self.previous_time
    }

    /// `level.frametime` (0x836f8b4): `level.time` less
    /// `level.previousTime`, `1000 / sv_fps` past a level's first frame.
    pub fn frame_time(&self) -> i32 {
        self.level_time - self.previous_time
    }

    /// `sv_fps`.
    pub fn sv_fps(&self) -> i32 {
        self.sv_fps
    }

    /// `1000 / sv_fps`, as `SV_FrameInternal` steps it and the antilag
    /// reads it (cod4-server-frame.md 8).
    pub fn frame_ms(&self) -> i32 {
        1000 / self.sv_fps
    }

    /// `sv_fps` for the frames to come; the server sets it before each
    /// (the dvar is read every `SV_Frame`, not latched).
    pub fn set_sv_fps(&mut self, fps: i32) {
        self.sv_fps = fps.clamp(1, 1000);
        self.rt.set_frame_ms(self.frame_ms());
    }

    /// How many frames [`HISTORY_MS`] holds at this `sv_fps`.
    fn history_frames(&self) -> usize {
        (HISTORY_MS / self.frame_ms()) as usize
    }

    /// Slot `slot`'s `lastVoiceTime` (cod4-voice.md 3), `None` before
    /// its first relayed frame.
    pub fn last_voice_time(&self, slot: usize) -> Option<i32> {
        self.rt.host.cod4().clients.get(slot)?.last_voice_time
    }

    /// What the next level keeps (`G_GetSavePersist`, cod4-map-cycle.md
    /// 1): `game[]` and every `pers[]` under `savePersist`, and on a
    /// `fast_restart` (`restart`) the compiled scripts and the item
    /// registry, which the engine keeps.
    pub fn take_carry(&self, restart: bool) -> crate::game::script::Carry {
        let all = self.rt.take_carry();
        let keep = self.rt.host.save_persist;
        crate::game::script::Carry {
            game: all.game.filter(|_| keep),
            pers: if keep { all.pers } else { Vec::new() },
            items: all.items.filter(|_| restart),
            scripts: all.scripts.filter(|_| restart),
            gametype: all.gametype,
            ..Default::default()
        }
    }

    /// `ClientConnect` (`SV_DirectConnect` runs it): the slot's state
    /// cleared, the client's entity and `CodeCallback_PlayerConnect` on the
    /// next frame's packet pass, a spectator floating at `spot` until the
    /// script spawns it.
    pub fn connect(&mut self, slot: usize, name: &str, spot: ([f32; 3], [f32; 3])) {
        self.rt.host.client_names[slot] = name.to_string();
        let c4 = self.rt.host.cod4_mut();
        c4.clients[slot] = Default::default();
        c4.stats[slot] = Stats::fresh();
        self.rt.push_client_event(ClientEvent::Connect {
            slot,
            name: name.to_string(),
        });
        self.rt.host.client_roster[slot] = Some(crate::game::scoreboard::RosterSlot {
            connecting: true,
            ..Default::default()
        });
        let mut ps = PlayerState::default();
        ps.set("clientNum", slot as u32);
        ps.set("commandTime", self.level_time as u32);
        ps.setf("moveSpeedScaleMultiplier", 1.0);
        ps.set("throwBackGrenadeOwner", ENTITYNUM_NONE);
        ps.set("viewlocked_entNum", ENTITYNUM_NONE);
        ps.set("pm_type", PM_SPECTATOR);
        place(&mut ps, spot.0, spot.1, None);
        self.players[slot] = Some(Player4 {
            ps,
            cmd: UserCmd::default(),
            oldcmd: Cmd::default(),
            old_velocity: [0.0; 2],
            spectator_client: -1,
            killcam_at: None,
            anim_conditions: (0, 0),
            use_hold: None,
            linked: false,
            body_events: BodyEvents::default(),
            turret_active: 0,
            trigger_down: false,
            inactivity_time: 0,
            inactivity_warned: false,
        });
        self.archive[slot] = None;
    }

    /// `ClientInactivityTimer` (0x80a4d56), from `ClientThink_real` for a
    /// playing or dead client (0x80a5ccf; a spectator or one at the
    /// intermission is never timed). `g_inactivity` 0 keeps the deadline a
    /// minute ahead; a cmd with a move key or the 0x401 buttons
    /// (`forwardmove`/`rightmove` as one word at 0x80a4d91, the buttons at
    /// 0x80a4dcd) puts it `g_inactivity` seconds ahead. Past the deadline
    /// the client goes; ten seconds before, it is warned once. 1.7 has no
    /// `g_inactivitySpectator` (no such string in `cod4_lnxded`). The
    /// engine calls this for its network clients only: `pers.localClient`
    /// (a bot or loopback address, 0x813b1b0) exempts the rest. VERIFIED
    /// live: an idle axis probe at `g_inactivity 30` was warned 20 s and
    /// dropped 30 s after its spawn, a spectator beside it kept.
    pub fn inactivity_timer(&mut self, slot: usize, cmd: &UserCmd) -> Inactivity {
        let state = self.rt.client_session(slot).map(|s| s.state);
        if !matches!(state, Some(SessionState::Playing | SessionState::Dead)) {
            return Inactivity::Keep;
        }
        let secs = crate::game::builtins::cvar::atoi(self.rt.host.cvars.get("g_inactivity"));
        let now = self.level_time;
        let Some(p) = self.players[slot].as_mut() else {
            return Inactivity::Keep;
        };
        if secs == 0 {
            p.inactivity_time = now + 60_000;
            p.inactivity_warned = false;
            return Inactivity::Keep;
        }
        if cmd.forwardmove != 0 || cmd.rightmove != 0 || cmd.buttons & 0x401 != 0 {
            p.inactivity_time = now.wrapping_add(secs.wrapping_mul(1000));
            p.inactivity_warned = false;
            return Inactivity::Keep;
        }
        if now > p.inactivity_time {
            return Inactivity::Drop;
        }
        if now > p.inactivity_time - 10_000 && !p.inactivity_warned {
            p.inactivity_warned = true;
            return Inactivity::Warn;
        }
        Inactivity::Keep
    }

    /// The GUID the client's `getchallenge` named, after [`Self::connect`].
    pub fn set_guid(&mut self, slot: usize, guid: &str) {
        self.rt.host.cod4_mut().clients[slot].guid = guid.to_string();
    }

    /// The client's own stats block, once its seven packets are in.
    pub fn set_stats(&mut self, slot: usize, stats: &Stats) {
        self.rt.host.cod4_mut().stats[slot] = stats.clone();
    }

    /// The slot's stats block as the scripts left it.
    pub fn stats(&self, slot: usize) -> Option<&Stats> {
        self.rt.host.cod4().stats.get(slot)
    }

    /// `ClientBegin`: `pers.connected` CON_CONNECTED and the `begin` notify.
    pub fn begin(&mut self, slot: usize) {
        if let Some(r) = self.rt.host.client_roster[slot].as_mut() {
            r.connecting = false;
            r.active = true;
        }
        if let Some(p) = self.players[slot].as_mut() {
            p.ps.set("pm_type", PM_SPECTATOR);
        }
        self.rt.push_client_event(ClientEvent::Begin(slot));
    }

    /// `ClientUserinfoChanged` (`cod4_lnxded` 0x80a8068) on a client's new
    /// userinfo `name`: under `setClientNameMode("auto_change")` the
    /// name the scripts, the chat and the snapshots' client states read
    /// is the new one at once; under `"manual_change"` (stock `sd`) a
    /// client in the game keeps his until `updateClientNames`, which no
    /// stock script calls (cod4-gsc.md 12). True when it changed.
    pub fn userinfo_changed(&mut self, slot: usize, name: &str) -> bool {
        use crate::game::builtins::cvar::ClientNameMode;
        let in_game = self.players.get(slot).is_some_and(Option::is_some);
        let host = &mut self.rt.host;
        if host.client_name_mode == ClientNameMode::Manual && in_game {
            return false;
        }
        let Some(n) = host.client_names.get_mut(slot) else {
            return false;
        };
        let changed = n != name;
        *n = name.to_string();
        changed
    }

    /// The name the game holds for `slot` (`sess.cs.name`).
    pub fn client_name(&self, slot: usize) -> &str {
        self.rt
            .host
            .client_names
            .get(slot)
            .map_or("", String::as_str)
    }

    /// `ClientDisconnect`, inside the drop.
    pub fn disconnect(&mut self, slot: usize) {
        self.rt.client_disconnect(slot);
        self.rt.host.client_roster[slot] = None;
        self.players[slot] = None;
        self.archive[slot] = None;
    }

    /// `callvote` or `vote` (`Cmd_CallVote_f` 0x80adbf2, `Cmd_Vote_f`
    /// 0x80ada2a; `game::vote` under `VoteRules::COD4`): the lines to
    /// send, the voters' `eFlags` 0x100000 set.
    pub fn vote_command(&mut self, slot: usize, argv: &[String]) -> crate::game::vote::Sends {
        let now = self.level_time;
        let sends = self.rt.vote_command(slot, argv, now);
        self.sync_voted();
        sends
    }

    /// `ps.eFlags` 0x100000 (`EF_VOTED`) off who has voted: a call clears
    /// it on every client and sets it on the caller, a vote on the voter;
    /// a spawn keeps it.
    fn sync_voted(&mut self) {
        let voted = self.rt.host.client_voted.clone();
        for (slot, p) in self.players.iter_mut().enumerate() {
            let Some(p) = p.as_mut() else { continue };
            let on = voted.get(slot).copied().unwrap_or(false);
            let e = p.ps.word("eFlags") & !0x10_0000;
            p.ps.set("eFlags", e | if on { 0x10_0000 } else { 0 });
        }
    }

    /// `ps.pm_type` is `PM_INTERMISSION`: `ClientCommand` takes only
    /// `say`, `say_team` and `score`.
    pub fn at_intermission(&self, slot: usize) -> bool {
        self.players
            .get(slot)
            .and_then(Option::as_ref)
            .is_some_and(|p| p.ps.word("pm_type") == PM_INTERMISSION)
    }

    pub fn menu_response(&mut self, slot: usize, menu: &MenuRef, response: &str) {
        self.rt.menu_response(slot, menu, response);
    }

    /// `ClientThink` for one cmd: `sess.cmd` and the buttons the script
    /// reads, then [`Self::think`].
    pub fn client_think(&mut self, slot: usize, cmd: &UserCmd) {
        if self.players[slot].is_none() {
            return;
        }
        let state = self.rt.client_session(slot).map(|s| s.state);
        if state == Some(SessionState::Spectator) {
            self.spectator_buttons(slot, cmd);
        }
        // The script's weapon builtins write the loadout, retail's straight
        // into `ps` (`G_GivePlayerWeapon`, `BG_TakePlayerWeapon`): a kill
        // inside an earlier cmd of this frame (the stock streak's
        // `giveWeapon`, `setActionSlot`; the victim's `dropWeaponForDeath`)
        // goes in ahead of this one's move, or the move carries the old
        // weapons back over it.
        let loadout = matches!(state, Some(SessionState::Playing | SessionState::Dead))
            .then(|| self.rt.host.cod4().clients[slot].loadout.clone());
        let Some(p) = self.players[slot].as_mut() else {
            return;
        };
        if let Some(l) = &loadout {
            loadout_into(&mut p.ps, l);
        }
        // A follower's cmds are not kept: the spawn after a follow takes
        // the view off the last cmd before it (the walk capture's spawn
        // `delta_angles` 110.00061 is its 45 less the -65 the walker sent
        // before it followed its teammate, VERIFIED live).
        if p.ps.word("otherFlags") & POF_FOLLOW != 0 {
            return;
        }
        // `SV_UserMove`: a weapon byte or offhand the client does
        // not hold reads as the playerstate's own (a probe echoing a
        // weapon 0 from before its spawn keeps the rifle, VERIFIED live:
        // the walk capture's teammate).
        let mut cmd = *cmd;
        let held = |w: u8| {
            let i = usize::from(w);
            i < 128 && p.ps.word(&format!("weapons[{}]", i / 32)) >> (i % 32) & 1 != 0
        };
        if !held(cmd.weapon) {
            cmd.weapon = p.ps.word("weapon") as u8;
        }
        if !held(cmd.offhand) {
            cmd.offhand = p.ps.word("offHandIndex") as u8;
        }
        let cmd = &cmd;
        p.cmd = *cmd;
        let selected = cmd.selected_location;
        let mut buttons = 0u8;
        if cmd.buttons & 0x28 != 0 {
            buttons |= vcod_common::net::msg::BUTTON_USE;
        }
        if cmd.buttons & 1 != 0 {
            buttons |= vcod_common::net::msg::BUTTON_ATTACK;
        }
        self.rt.set_client_buttons(slot, buttons);
        let pressed = cmd.buttons;
        let old = self.rt.host.cod4().buttons[slot];
        let mut cmd = Cmd::from(cmd);
        cmd.buttons = self.latch_buttons(slot, cmd.buttons, selected);
        if let Some((seq, time)) = self.think(slot, cmd) {
            self.after_think(slot, seq, time);
            self.update_activate(slot, pressed, old);
        }
    }

    /// `SpectatorThink`'s buttons (`cod4_lnxded` 0x80a4e82): the cmd's
    /// buttons latched, then, with no script-forced target and free
    /// flight allowed, a follower's melee (0x4) pressed or let go stops
    /// the follow (`StopFollowing`); a fresh attack press cycles to the
    /// next client (`Cmd_FollowCycle_f(ent, 1)`), else a fresh sight
    /// press (0x800) to the one before. VERIFIED (the instructions,
    /// cod4-pmove.md 15).
    fn spectator_buttons(&mut self, slot: usize, cmd: &UserCmd) {
        let buttons = cmd.buttons;
        let c4 = self.rt.host.cod4_mut();
        let old = c4.buttons[slot];
        c4.buttons[slot] = buttons;
        c4.buttons_since[slot] |= buttons & !old;
        let forced = self
            .rt
            .client_session(slot)
            .map_or(-1, |s| s.spectator_client);
        if forced >= 0 {
            return;
        }
        let no_spectate = self.rt.host.client_no_spectate[slot];
        let free = no_spectate & NO_SPECTATE_FREELOOK == 0;
        let following = self.players[slot]
            .as_ref()
            .is_some_and(|p| p.spectator_client >= 0);
        if free && following && (buttons ^ old) & 4 != 0 {
            self.stop_following(slot, Some(cmd));
        }
        let pressed = buttons & !old;
        let dir = if pressed & 1 != 0 {
            1
        } else if pressed & 0x800 != 0 {
            -1
        } else {
            return;
        };
        let can = |team: u32| no_spectate & (1 << team) == 0;
        if let Some(t) = self.follow_cycle(slot, dir, &can) {
            self.players[slot].as_mut().unwrap().spectator_client = t as i32;
        }
    }

    /// `StopFollowing` (`cod4_lnxded` 0x80af24c): no target; a follower is
    /// set down 40 behind and 10 over the followed eye (an 8 capsule
    /// traced there under 0x810011, stopping short of a wall), looking
    /// 15 lower, as itself again: `eFlags` 0x300, the view lock, the
    /// sight and shellshock flags (0x10010), `weapFlags` 0x40, the follow
    /// bit, `fWeaponPosFrac` and the shellshock cleared. VERIFIED (the
    /// instructions and their -40, 10 and 15 at 0x8219944, 0x820aac8 and
    /// 0x8215e54). The view goes through `delta_angles` off `cmd`.
    fn stop_following(&mut self, slot: usize, cmd: Option<&UserCmd>) {
        let Some(p) = self.players[slot].as_mut() else {
            return;
        };
        p.spectator_client = -1;
        if p.ps.word("otherFlags") & POF_FOLLOW == 0 {
            return;
        }
        let ps = pmove::PlayerState::from_wire(&p.ps);
        let eye = super::combat::view_origin(&ps, self.level_time);
        let [forward, _, up] = vcod_common::x87::angle_vectors(Vec3::from(ps.view_angles));
        let end = eye - 40.0 * forward + 10.0 * up;
        let pos = match self.clip.as_deref() {
            Some(clip) => {
                let bodies = Vec::new();
                let world = Cod4Clip {
                    clip,
                    bodies: &bodies,
                    brushes: &self.brushes,
                };
                let r = Vec3::splat(8.0);
                let tr = pmove::PmWorld::trace(&world, eye, end, -r, r, ENTITYNUM_NONE, 0x81_0011);
                eye + (end - eye) * tr.fraction
            }
            None => end,
        };
        let mut angles = ps.view_angles;
        angles[0] += 15.0;
        let p = self.players[slot].as_mut().unwrap();
        p.ps.set("clientNum", slot as u32);
        let w = |p: &mut Player4, f: &str, clear: u32| {
            let v = p.ps.word(f) & !clear;
            p.ps.set(f, v);
        };
        w(p, "eFlags", 0x300);
        w(p, "pm_flags", 0x1_0010);
        w(p, "weapFlags", 0x40);
        w(p, "otherFlags", POF_FOLLOW);
        p.ps.set("viewlocked", 0);
        p.ps.set("viewlocked_entNum", ENTITYNUM_NONE);
        p.ps.setf("fWeaponPosFrac", 0.0);
        for f in ["shellshockIndex", "shellshockTime", "shellshockDuration"] {
            p.ps.set(f, 0);
        }
        place(&mut p.ps, pos.to_array(), angles, cmd);
    }

    /// The player animtree's names by wire index, for `getCorpseAnim`,
    /// once the anims are in.
    fn sync_anim_names(&mut self) {
        if !self.rt.host.cod4().anim_names.is_empty() {
            return;
        }
        let Some(set) = self.combat.anim_set() else {
            return;
        };
        let names: Vec<String> = (0..512)
            .map_while(|w| set.anims.name(w).map(str::to_string))
            .collect();
        self.rt.host.cod4_mut().anim_names = std::rc::Rc::new(names);
    }

    /// `ClientThink_real`'s button latch (`cod4_lnxded`
    /// 0x80a5cdc..0x80a5d81): the cmd's buttons become the client's, and
    /// what they newly press gathers in `buttonsSinceLastFrame` until the
    /// end frame. With the location selector up
    /// (`ps.locationSelectionInfo`, +0x5b4) a gathered fire press
    /// (0x10000) confirms the spot under the cursor, else a 0x20000 press
    /// cancels; either way only the stance and hold-breath bits (0x1300)
    /// stay latched and reach the move.
    fn latch_buttons(&mut self, slot: usize, buttons: u32, cursor: [u8; 2]) -> u32 {
        const CONFIRM: u32 = 0x1_0000;
        const CANCEL: u32 = 0x2_0000;
        const KEPT: u32 = 0x1300;
        let c4 = self.rt.host.cod4_mut();
        let before = std::mem::replace(&mut c4.buttons[slot], buttons);
        c4.buttons_since[slot] |= buttons & !before;
        let gathered = c4.buttons_since[slot];
        if !self.selecting_location(slot) {
            return buttons;
        }
        if gathered & CONFIRM != 0 {
            let spot = super::compass(&self.rt.host).map(|c| c.world_point(cursor));
            let spot = Value::Vector(spot.unwrap_or_default());
            self.rt
                .notify_client_value(slot, "confirm_location", Some(spot));
        } else if gathered & CANCEL != 0 {
            self.rt.notify_client_value(slot, "cancel_location", None);
        }
        let c4 = self.rt.host.cod4_mut();
        c4.buttons[slot] &= KEPT;
        c4.buttons_since[slot] &= KEPT;
        buttons & KEPT
    }

    /// Whether `slot` is in the game (playing or dead) with
    /// `beginLocationSelection`'s selector up.
    fn selecting_location(&mut self, slot: usize) -> bool {
        let in_game = matches!(
            self.rt.client_session(slot).map(|s| s.state),
            Some(SessionState::Playing | SessionState::Dead)
        );
        in_game
            && self.players[slot]
                .as_ref()
                .is_some_and(|p| p.ps.word("locationSelectionInfo") != 0)
    }

    /// `ClientThink_real`: the cmd's clock held within
    /// [-1000, +200] ms of `level.time`; nothing for a cmd no newer than
    /// `commandTime` or in the intermission; a spectator not following
    /// flies (`SpectatorThink`: `ps.speed` 400 where it may look freely,
    /// else 0, no `oldcmd`); a player walks at `g_speed`. Then the body is
    /// relinked where the move left it (`SV_LinkEntity`), a spectator
    /// unlinked. Without a clip map only the clock and the view move.
    fn think(&mut self, slot: usize, mut cmd: Cmd) -> Option<(i32, i32)> {
        let state = self
            .rt
            .client_session(slot)
            .map_or(SessionState::Spectator, |s| s.state);
        let level_time = self.level_time;
        let freelook = self.rt.host.client_no_spectate[slot] & NO_SPECTATE_FREELOOK == 0;
        let g_speed = crate::game::builtins::cvar::atoi(self.rt.host.cvars.get("g_speed"));
        let bodies = self.bodies(slot);
        self.combat.anims();
        self.sync_anim_names();
        let p = self.players[slot].as_mut()?;
        cmd.server_time = cmd.server_time.clamp(level_time - 1000, level_time + 200);
        let command_time = p.ps.word("commandTime") as i32;
        if cmd.server_time - command_time < 1 && p.ps.word("clientNum") == slot as u32 {
            return None;
        }
        let Some(clip) = self.clip.as_deref() else {
            p.ps.set("commandTime", cmd.server_time as u32);
            view_only(&mut p.ps, &cmd);
            return None;
        };
        let mut oldcmd = p.oldcmd;
        match state {
            SessionState::Intermission => return None,
            SessionState::Spectator => {
                if p.ps.word("otherFlags") & POF_FOLLOW != 0 {
                    return None;
                }
                p.ps.set("pm_type", PM_SPECTATOR);
                p.ps.set("speed", if freelook { 400 } else { 0 });
                oldcmd = Cmd::default();
            }
            SessionState::Playing | SessionState::Dead => {
                p.ps.set("speed", g_speed as u32);
                // `ClientThink_real`: a shellshock past its end
                // lets the walk go.
                let end =
                    p.ps.word("shellshockTime") as i32 + p.ps.word("shellshockDuration") as i32;
                if end - level_time <= 0 {
                    let f = p.ps.word("pm_flags") & !pmove::pmf::SHELLSHOCKED;
                    p.ps.set("pm_flags", f);
                }
            }
        }
        let mut ps = pmove::PlayerState::from_wire(&p.ps);
        ps.old_velocity = p.old_velocity;
        (ps.anim_movetype, ps.anim_strafing) = p.anim_conditions;
        let old_seq = ps.event_sequence;
        let cmd_time = cmd.server_time;
        let weapon = self
            .rt
            .host
            .cod4()
            .table
            .defs
            .get(ps.weapon as usize)
            .map_or(pmove::WeaponMove::NONE, |d| d.mv);
        let world = Cod4Clip {
            clip,
            bodies: &bodies,
            brushes: &self.brushes,
        };
        let armed = state != SessionState::Spectator;
        // `r.maxs` as the last cmd left them, what a blockage copies.
        let box_before = Vec3::new(15.0, 15.0, body_height(p.ps.word("pm_flags")));
        let mut mantle = None;
        if armed {
            // `ClientThink_real`'s aim block (0x80a5e95..0x80a6162) on the
            // playerstate the last cmd left: the angles this cmd's rounds
            // leave along (cod4-combat.md 3.2).
            let msec = (cmd.server_time - command_time).min(200);
            let keys = |w: u32| {
                self.combat
                    .defs
                    .get(w as usize)
                    .map(|d| d.aim.clone())
                    .unwrap_or_default()
            };
            let (view_keys, held_keys) = (keys(pmove::aim::view_weapon(&ps)), keys(ps.weapon));
            self.combat.gun_angles[slot] =
                self.combat.aim[slot].fire_angles(&ps, &view_keys, &held_keys, level_time, msec);
            // `BG_AnimUpdatePlayerStateConditions`: the viewmodel weapon's
            // anim type and class (the offhand while it is in hand).
            let vm = if ps.weap_flags & pmove::weapon::wf::USING_OFFHAND != 0 {
                ps.off_hand_index
            } else {
                ps.weapon
            };
            let def = self.combat.defs.get(vm as usize);
            let anim = self.combat.anim_set().map(|set| pmove::anim::AnimInput {
                anims: &set.anims,
                player_anim_type: def.map_or(0, |d| d.player_anim_type),
                weapon_class: def.map_or(0, |d| d.weap_class),
            });
            let c4 = self.rt.host.cod4_mut();
            let mut rand = || c4.rand();
            mantle = pmove::player_step_animated(
                &mut ps,
                &cmd,
                &mut oldcmd,
                &world,
                self.rules,
                weapon,
                &self.combat.defs,
                anim,
                Some(&mut rand),
            )
            .1;
        } else {
            pmove::player_step(&mut ps, &cmd, &mut oldcmd, &world, self.rules, weapon);
        }
        p.ps = ps.to_wire(&p.ps);
        p.old_velocity = ps.old_velocity;
        p.anim_conditions = (ps.anim_movetype, ps.anim_strafing);
        if state != SessionState::Spectator {
            p.oldcmd = oldcmd;
        }
        if let Some((end, duration)) = mantle {
            self.spawn_mantle_block(slot, end, box_before, duration);
        }
        // `clonePlayer` reads the body as this cmd left it (0x80acfeb).
        if let Some(c) = self.rt.host.cod4_mut().corpse_source[slot].as_mut() {
            c.origin = ps.origin.to_array();
            c.velocity = ps.velocity.to_array();
        }
        self.relink(slot, &ps);
        // `player_die` draws its death anim off the live playerstate, so a
        // flinch this cmd started is what the death's toggle flips
        // (cod4-combat.md 5).
        if state == SessionState::Playing {
            self.death_choices(slot);
        }
        armed.then_some((old_seq, cmd_time))
    }

    /// `G_AddPlayerMantleBlockage`, inlined in `ClientThink_real` after a
    /// pmove that started a mantle (`cod4_lnxded` 0x80a6537..0x80a6618):
    /// a `G_Spawn` owned by the client, contents and clipmask 0x10000,
    /// `svFlags` 0x21, `ET_INVISIBLE`, the client's box before this cmd at
    /// the mantle's end, linked, its think (which frees it) due
    /// `mantleDuration + g_mantleBlockTimeBuffer` ms on (the dvar 500, 0 to
    /// 60000). Every other player's pmove clips it (cod4-pmove.md 17.4).
    fn spawn_mantle_block(&mut self, slot: usize, end: Vec3, maxs: Vec3, duration: i32) {
        let buffer = self
            .rt
            .host
            .cvars
            .get("g_mantleBlockTimeBuffer")
            .trim()
            .parse::<i32>()
            .unwrap_or(500)
            .clamp(0, 60000);
        match self.rt.with_host(|h, cx| h.ents.spawn(cx)) {
            Ok(id) => {
                let block = Body4 {
                    entity: id.0,
                    origin: end,
                    mins: pmove::PLAYER_MINS,
                    maxs,
                    contents: 0x10000,
                    owner: Some(slot as u32),
                };
                let at = self.level_time + duration + buffer;
                self.mantle_blocks.push((id, at, block));
            }
            Err(e) => log::warn!("cod4: mantle block dropped: {e:?}"),
        }
    }

    /// The mantle blocks whose think is due, freed (`G_FreeEntity`).
    fn run_mantle_blocks(&mut self) {
        let now = self.level_time;
        let (due, kept): (Vec<_>, Vec<_>) = std::mem::take(&mut self.mantle_blocks)
            .into_iter()
            .partition(|&(_, at, _)| at <= now);
        self.mantle_blocks = kept;
        for (id, _, _) in due {
            self.rt.host.free_entity(id);
        }
    }

    /// After a player's cmd: the loadout follows what its weapon half did,
    /// then `ClientEvents` runs its events.
    fn after_think(&mut self, slot: usize, old_seq: i32, cmd_time: i32) {
        let Some(ps) = self.pm_state(slot) else {
            return;
        };
        let l = &mut self.rt.host.cod4_mut().clients[slot].loadout;
        l.weapon = ps.weapon as usize;
        l.offhand = ps.off_hand_index as usize;
        l.weapons = ps.weapons;
        l.weaponold = ps.weaponold;
        l.ammo[..ps.ammo.len()].copy_from_slice(&ps.ammo);
        l.ammoclip.copy_from_slice(&ps.ammoclip);
        self.rt.host.cod4_mut().pm_flags[slot] = ps.pm_flags;
        self.rt.host.cod4_mut().view_angles[slot] = ps.view_angles;
        self.rt.host.cod4_mut().velocity[slot] = ps.velocity.to_array();
        // `PWF_USING_OFFHAND` (2): the held grenade is the offhand.
        self.rt.host.cod4_mut().clients[slot].held_grenade =
            (ps.grenade_time_left != 0).then(|| {
                let w = if ps.weap_flags & 2 != 0 {
                    ps.off_hand_index
                } else {
                    ps.weapon
                };
                (w, ps.grenade_time_left)
            });
        self.client_events(slot, old_seq, cmd_time);
    }

    /// What the script and the area tree read of a move: the origin, the
    /// angles (0, the view's yaw, 0), the eye and aim, the ground, and the
    /// link of a body in play.
    fn relink(&mut self, slot: usize, ps: &pmove::PlayerState) {
        let origin = ps.origin.to_array();
        let playing = ps.pm_type < pmove::pm_type::SPECTATOR;
        self.rt.set_client_pm_type(slot, ps.pm_type);
        self.rt.set_client_on_ground(slot, ps.on_ground());
        let eye = [origin[0], origin[1], origin[2] + ps.view_height_current];
        self.rt
            .set_client_aim(slot, eye, [ps.view_angles[0], ps.view_angles[1]], false);
        self.rt.set_client_origin(slot, origin);
        self.rt
            .set_client_angles(slot, [0.0, ps.view_angles[1], 0.0]);
        let host = &mut self.rt.host;
        if playing {
            let h = body_height(ps.pm_flags);
            host.client_height[slot] = h;
            let contents = vcod_common::movetrace::CONTENTS_BODY as i32;
            host.link_client(slot, origin, host.client_box(slot), contents, true);
        } else {
            host.area.unlink(slot as u32);
        }
    }

    /// Every other playing client's body, as its last move linked it, then
    /// the mantles' blockages.
    fn bodies(&self, slot: usize) -> Vec<Body4> {
        self.players
            .iter()
            .enumerate()
            .filter(|&(i, _)| i != slot)
            .filter_map(|(i, p)| {
                let ps = &p.as_ref()?.ps;
                (ps.word("otherFlags") & POF_PLAYER != 0
                    && in_play(ps.word("pm_type"))
                    && ps.word("clientNum") == i as u32)
                    .then(|| Body4 {
                        entity: i as u32,
                        origin: Vec3::new(
                            f32::from_bits(ps.word("origin[0]")),
                            f32::from_bits(ps.word("origin[1]")),
                            f32::from_bits(ps.word("origin[2]")),
                        ),
                        mins: pmove::PLAYER_MINS,
                        maxs: Vec3::new(15.0, 15.0, body_height(ps.word("pm_flags"))),
                        contents: pmove::CONTENTS_BODY,
                        owner: None,
                    })
            })
            .chain(self.mantle_blocks.iter().map(|b| b.2))
            .collect()
    }

    /// `G_RunFrame`: the script's threads and entity pass, the spawns they
    /// made, every client's `ClientEndFrame`, `CheckTeamStatus`, and the
    /// playerstates archived for the next frame's followers.
    pub fn run_frame(&mut self, now_ms: i32) {
        self.previous_time = self.level_time;
        self.level_time = now_ms;
        let mut _p = crate::tick_profile::phase("frame/touch triggers");
        crate::tick_trace::step!("G_TouchTriggers");
        self.touch_triggers(now_ms);
        _p = crate::tick_profile::phase("frame/scripts");
        crate::tick_trace::step!("Scr_RunThreads");
        self.rt.run_frame(now_ms);
        _p = crate::tick_profile::phase("frame/script ops");
        self.apply_set_origins();
        self.apply_link_ops();
        self.per_frame_notifies();
        for id in std::mem::take(&mut self.rt.host.cod4_mut().free_next) {
            self.rt.host.ents.free(id);
        }
        // The script's `finishPlayerDamage`s and `suicide`s wrote their
        // victims' playerstates inside the call, ahead of any `spawn` after.
        self.apply_damage_ops();
        for s in self.rt.take_client_spawns() {
            self.client_spawn(s.slot, s.origin, [0.0, s.yaw_deg, 0.0], s.mode);
        }
        self.sync_defs();
        _p = crate::tick_profile::phase("frame/entities");
        // The corpses' moves (`G_RunCorpse`, cod4-combat.md 5.2), then the
        // body queue's thinks (`BodyEnd`, cod4-combat.md 5).
        self.run_corpses();
        self.rt
            .host
            .bodies
            .run_thinks(now_ms, &super::COD4_HOST_PROTO);
        self.run_items();
        self.touch_items();
        self.brushes = self.brush_entities();
        self.run_missiles();
        self.run_vehicles();
        // What the next frame's script traces cut.
        _p = crate::tick_profile::phase("frame/trace bodies");
        let bodies = self.bodies(usize::MAX);
        let posed = self.posed_bodies();
        let c4 = self.rt.host.cod4_mut();
        c4.trace_bodies = bodies;
        c4.posed_bodies = posed;
        c4.trace_brushes = self.brushes.clone();
        _p = crate::tick_profile::phase("frame/mantle and hud");
        self.run_mantle_blocks();
        // `G_UpdateHudElemsToClients`, ahead of the end frames:
        // both arrays into every client's playerstate; a follower's
        // `ClientEndFrame` copies the followed one's and puts its own
        // current half back (cod4-hud.md 5.1).
        for slot in 0..self.players.len() {
            self.hud_into(slot, true);
        }
        _p = crate::tick_profile::phase("frame/end frames");
        self.release_stale_turrets();
        for slot in 0..self.players.len() {
            if self.players[slot].is_some() {
                self.client_end_frame(slot);
            }
        }
        self.record_positions();
        // `CheckTeamStatus`'s `TeamplayInfoMessage`: `stats[3]`
        // -1 for every client not following; the wire's 6 bits read 63
        // (VERIFIED live, the spawn frame).
        for p in self.players.iter_mut().flatten() {
            if p.ps.word("otherFlags") & POF_FOLLOW == 0 {
                p.ps.stats[3] = 63;
            }
        }
        // `CheckVote` (0x80b423a) after `CheckTeamStatus`: its
        // line queues with the frame's commands.
        _p = crate::tick_profile::phase("frame/vote and archive");
        crate::tick_trace::step!("CheckVote");
        self.rt.check_vote(now_ms);
        self.sync_voted();
        for (a, p) in self.archive.iter_mut().zip(&self.players) {
            *a = p.as_ref().map(|p| p.ps.clone());
        }
        self.history.push_front((now_ms, self.archive.clone()));
        self.history.truncate(self.history_frames());
    }

    /// A player's `setOrigin` (`SetClientOrigin`, INFERRED):
    /// the playerstate's origin (the builtin's unit lift already on it),
    /// the teleport bit flipped, the body relinked there. The shared
    /// builtin queues it; nothing else of the sim ops is CoD4's.
    fn apply_set_origins(&mut self) {
        use crate::game::host::SimOp;
        for (slot, op) in self.rt.take_sim_ops() {
            let Some(p) = self.players[slot].as_mut() else {
                continue;
            };
            let origin = match op {
                SimOp::SetOrigin { origin } => origin,
                // `setPlayerAngles`' `SetClientViewAngle`: the view
                // and `delta_angles` off the last cmd, where it stands.
                SimOp::SetViewAngles { angles } => {
                    let o =
                        std::array::from_fn(|k| f32::from_bits(p.ps.word(&format!("origin[{k}]"))));
                    let cmd = p.cmd;
                    place(&mut p.ps, o, angles, Some(&cmd));
                    continue;
                }
                _ => continue,
            };
            for (k, v) in origin.into_iter().enumerate() {
                p.ps.setf(&format!("origin[{k}]"), v);
            }
            let e = p.ps.word("eFlags") ^ 2;
            p.ps.set("eFlags", e);
            let ps = pmove::PlayerState::from_wire(&p.ps);
            self.relink(slot, &ps);
        }
    }

    /// `G_ClientDoPerFrameNotifies` after the frame's threads:
    /// `weapon_change` with the weapon's name on each client whose
    /// `ps.weapon` moved since the last (`_hardpoints::hardpointItemWaiter`
    /// calls a hardpoint in on it, cod4-hud.md 15). INFERRED; the
    /// firing, night vision and sprint pairs are not raised.
    fn per_frame_notifies(&mut self) {
        for slot in 0..self.players.len() {
            if self.players[slot].is_none() || self.rt.client_entity(slot).is_none() {
                continue;
            }
            let c4 = self.rt.host.cod4_mut();
            let c = &mut c4.clients[slot];
            let w = c.loadout.weapon;
            if w == c.last_weapon {
                continue;
            }
            c.last_weapon = w;
            let name = c4
                .table
                .get(w)
                .map_or_else(|| "none".into(), |d| d.name.clone());
            self.rt.notify_client(slot, "weapon_change", &[&name]);
        }
    }

    /// `HudElem_UpdateClient` for `slot`: the current array, and the
    /// archival one with `archival`.
    fn hud_into(&mut self, slot: usize, archival: bool) {
        if self.players[slot].is_none() || self.rt.client_entity(slot).is_none() {
            return;
        }
        let team = self.rt.client_team(slot);
        let (a, c) = super::wire::hud_elems(&self.rt.host, slot, team);
        let p = self.players[slot].as_mut().unwrap();
        if archival {
            p.ps.hud_archival = a;
        }
        p.ps.hud_current = c;
    }

    /// `ClientSpawn` (`cod4_lnxded` 0x80a7c5d..0x80a7e75): a fresh
    /// playerstate ([`spawn_playerstate`]) at the spot, the view through
    /// `delta_angles` off the last cmd, the `g_inactivity` deadline
    /// (0x80a7e05), the aim state cleared with the memset (cod4-combat.md
    /// 3.2), and then the spawn's own think: the last cmd at `level.time`
    /// from a `commandTime` 100 ms back (0x80a7e5b), before any loadout,
    /// so it moves nothing; the walk capture's spawn frame reads that
    /// think's 66 and 34 ms of `holdBreathScale` (0.519216), the ground and
    /// the idle, and `commandTime` the frame's own (VERIFIED live,
    /// cod4-gsc.md 7).
    fn client_spawn(&mut self, slot: usize, origin: [f32; 3], angles: [f32; 3], mode: SpawnMode) {
        crate::tick_trace::step!("ClientSpawn slot {slot}");
        let now = self.level_time;
        let cvar = |name: &str| crate::game::builtins::cvar::atoi(self.rt.host.cvars.get(name));
        let (idle_secs, gravity) = (cvar("g_inactivity"), cvar("g_gravity"));
        let max_health = self.rt.host.client_vitals[slot].max_health;
        // A gunner still on a gun lets go first, so the release's
        // teleport lands before the spawn spot (0x80a7b8c..0x80a7ecf).
        if let Some(p) = self.players[slot].as_ref()
            && p.ps.word("otherFlags") & POF_PLAYER != 0
            && p.ps.word("eFlags") & vcod_cod4::pmove::ef::TURRET != 0
            && let Some(gun) = self.manned_gun(slot)
        {
            self.stop_using_turret(gun);
        }
        let Some(p) = self.players[slot].as_mut() else {
            return;
        };
        let mut ps = spawn_playerstate(&p.ps, slot, max_health, mode);
        place(&mut ps, origin, angles, Some(&p.cmd));
        ps.set("gravity", gravity as u32);
        ps.set("commandTime", now.wrapping_sub(100) as u32);
        p.ps = ps;
        p.linked = false;
        p.spectator_client = -1;
        p.inactivity_time = now.wrapping_add(idle_secs.wrapping_mul(1000));
        let mut think = Cmd::from(&p.cmd);
        think.server_time = now;
        self.combat.aim[slot] = Default::default();
        self.combat.gun_angles[slot] = [0.0; 2];
        self.combat.clear_blood(slot);
        if mode != SpawnMode::Intermission {
            self.think(slot, think);
        }
        if let Some(p) = self.players[slot].as_mut() {
            p.ps.set("commandTime", now as u32);
        }
    }

    /// The brush entities pmove clips: every entity linked with an inline
    /// model, solid (`es.solid` `SOLID_BMODEL` and no `EF_NONSOLID_BMODEL`).
    fn brush_entities(&mut self) -> Vec<Brush4> {
        let word = |e: &EntityState, n: &str| ent_word(e.etype(), n).map_or(0, |w| e.0[w]);
        let f = |e: &EntityState, n: &str| f32::from_bits(word(e, n));
        self.rt
            .packet_entities(&super::COD4_HOST_PROTO)
            .values()
            .filter_map(super::wire::ent4)
            .filter(|e| {
                word(e, "solid") == 0xff_ffff
                    && word(e, "lerp.eFlags") & 1 == 0
                    && word(e, "index") != 0
            })
            .map(|e| Brush4 {
                entity: e.number(),
                model: word(&e, "index") as usize,
                origin: Vec3::new(
                    f(&e, "lerp.pos.trBase[0]"),
                    f(&e, "lerp.pos.trBase[1]"),
                    f(&e, "lerp.pos.trBase[2]"),
                ),
                angles: Vec3::new(
                    f(&e, "lerp.apos.trBase[0]"),
                    f(&e, "lerp.apos.trBase[1]"),
                    f(&e, "lerp.apos.trBase[2]"),
                ),
            })
            .collect()
    }

    /// `ClientEndFrame` by session: a playing client is a player
    /// (`otherFlags` 4) with its loadout, view model, perks and the move
    /// speed and gravity its think writes; a spectator follows whom the
    /// team rule makes it (`SpectatorClientEndFrame`), else floats.
    fn client_end_frame(&mut self, slot: usize) {
        if let Some(p) = self.players[slot].as_mut() {
            p.killcam_at = None;
        }
        self.script_ps_writes(slot);
        // `ClientEndFrame`'s head for every session:
        // `moveSpeedScaleMultiplier` off the session's, so a
        // spectator in the team menu reads 1 (VERIFIED live, the first
        // snapshot of `play-1.7-svfps40.txt`; cod4-server-frame.md 8).
        let scale = self.rt.host.cod4().clients[slot].loadout.move_speed_scale;
        if let Some(p) = self.players[slot].as_mut() {
            p.ps.setf("moveSpeedScaleMultiplier", scale);
        }
        let session = self.rt.client_session(slot);
        let state = session
            .as_ref()
            .map_or(SessionState::Spectator, |s| s.state);
        match state {
            SessionState::Playing | SessionState::Dead => {
                self.player_end_frame(slot, state);
                // `BG_PlayerStateToEntityState`'s events.
                let p = self.players[slot].as_mut().unwrap();
                p.body_events.take(&p.ps);
            }
            SessionState::Spectator => self.spectator_end_frame(slot),
            SessionState::Intermission => {
                let p = self.players[slot].as_mut().unwrap();
                let of = p.ps.word("otherFlags") & !0x1c;
                p.ps.set("otherFlags", of);
                p.ps.set("pm_type", PM_INTERMISSION);
                p.ps.set("viewmodelIndex", 0);
            }
        }
    }

    /// What the script's player methods wrote for the playerstate:
    /// `setDepthOfField`'s six `dof*`, `beginLocationSelection`'s
    /// `locationSelectionInfo`, and `radarEnabled` off the team's
    /// `level.teamHasRadar` (`ClientEndFrame`); the frame's
    /// `buttonsSinceLastFrame` cleared.
    fn script_ps_writes(&mut self, slot: usize) {
        let team = self.rt.client_team(slot);
        let c4 = self.rt.host.cod4_mut();
        c4.buttons_since[slot] = 0;
        let radar = usize::try_from(team)
            .ok()
            .and_then(|t| c4.team_radar.get(t).copied())
            .unwrap_or(false);
        let c = &mut c4.clients[slot];
        let (dof, loc) = (c.dof.take(), c.location_selection.take());
        let Some(p) = self.players[slot].as_mut() else {
            return;
        };
        if let Some(d) = dof {
            for (name, v) in [
                "dofNearStart",
                "dofNearEnd",
                "dofFarStart",
                "dofFarEnd",
                "dofNearBlur",
                "dofFarBlur",
            ]
            .into_iter()
            .zip(d)
            {
                p.ps.setf(name, v);
            }
        }
        if let Some(l) = loc {
            p.ps.set("locationSelectionInfo", l);
        }
        p.ps.set("radarEnabled", u32::from(radar));
    }

    fn player_end_frame(&mut self, slot: usize, state: SessionState) {
        let host = &self.rt.host;
        let c4 = &host.cod4().clients[slot];
        let l = &c4.loadout;
        let health = host.client_vitals[slot].health;
        let max_health = host.client_vitals[slot].max_health;
        let viewmodel = host.client_viewmodel[slot] as u32;
        let speed = crate::game::builtins::cvar::atoi(host.cvars.get("g_speed")) as u32;
        let gravity = crate::game::builtins::cvar::atoi(host.cvars.get("g_gravity")) as u32;
        let perks = c4.perks;
        let weap_flags = c4.weap_flags;
        let frozen = c4.frozen;
        let last_stand = c4.last_stand;
        let (c4_ping_until, host_time) = (c4.compass_ping_until, host.level_time_ms);
        let shock = host.client_shellshock[slot];
        let l = l.clone();
        let shock_pmf = self.rt.host.cod4_mut().clients[slot].shock_pmf.take();
        let pinged = c4_ping_until > host_time;
        if state == SessionState::Playing {
            self.damage_feedback(slot);
        }
        let p = self.players[slot].as_mut().unwrap();
        let ps = &mut p.ps;
        let of = (ps.word("otherFlags") | POF_PLAYER) & !(POF_SPECTATE_TEAM | POF_SPECTATE_FREE);
        ps.set("otherFlags", of);
        ps.set("viewmodelIndex", viewmodel);
        // `ClientEndFrame`: linked or not by `tagInfo`; Last
        // Stand's down state is its own `pm_type`.
        ps.set(
            "pm_type",
            match (state == SessionState::Dead, p.linked) {
                (true, false) => PM_DEAD,
                (true, true) => PM_DEAD_LINKED,
                (false, _) if last_stand => PM_LASTSTAND,
                (false, false) => PM_NORMAL,
                (false, true) => PM_NORMAL_LINKED,
            },
        );
        ps.stats[0] = health;
        ps.stats[2] = max_health;
        // The think's own writes (`ClientThink_real`): the
        // spawn frame's 190 and 800 (VERIFIED live).
        ps.set("speed", speed);
        ps.set("gravity", gravity);
        ps.setf("moveSpeedScaleMultiplier", l.move_speed_scale);
        ps.set("perks", perks);
        // `disableWeapons`' bit is the script's; the rest are pmove's.
        let wf = ps.word("weapFlags");
        ps.set("weapFlags", (wf & !0x80) | (weap_flags & 0x80));
        // The frozen flag as `PMF_FROZEN`.
        let pmf = ps.word("pm_flags");
        let mut pmf = if frozen {
            pmf | vcod_cod4::pmove::pmf::FROZEN
        } else {
            pmf & !vcod_cod4::pmove::pmf::FROZEN
        };
        // `shellShock` / `stopShellShock` write the playerstate itself
        // (cod4-combat.md 10.8).
        match shock_pmf {
            Some(true) => pmf |= vcod_cod4::pmove::pmf::SHELLSHOCKED,
            Some(false) => pmf &= !vcod_cod4::pmove::pmf::SHELLSHOCKED,
            None => {}
        }
        ps.set("pm_flags", pmf);
        // `pingPlayer`'s flash, cleared once its time is up (0x80a6c9f).
        let ef = ps.word("eFlags");
        ps.set("eFlags", if pinged { ef | EF_PING } else { ef & !EF_PING });
        ps.set("shellshockIndex", shock[0] as u32);
        ps.set("shellshockTime", shock[1] as u32);
        ps.set("shellshockDuration", shock[2] as u32);
        loadout_into(ps, &l);
        let source = crate::game::builtins::cod4_combat::CorpseSource {
            origin: [0, 1, 2].map(|k| f32::from_bits(ps.word(&format!("origin[{k}]")))),
            velocity: [0, 1, 2].map(|k| f32::from_bits(ps.word(&format!("velocity[{k}]")))),
            yaw: f32::from_bits(ps.word("viewangles[1]")),
            e_flags: ps.word("eFlags"),
            legs_anim: ps.word("legsAnim"),
            torso_anim: ps.word("torsoAnim"),
        };
        // `ClientEndFrame` runs the hint pass dead or alive; a dead player's
        // only clears `cursorHint` (cod4-hud.md 20).
        if state == SessionState::Dead {
            self.update_cursor_hints(slot);
        }
        if state == SessionState::Playing {
            self.rt.host.cod4_mut().corpse_source[slot] = Some(source);
            self.update_cursor_hints(slot);
            if shock_pmf == Some(true) {
                self.shellshock_anim(slot);
            }
            self.animate_body(slot);
            self.death_choices(slot);
        }
        // `ClientEndFrame` after `BG_PlayerAnimation`, dead or alive
        // (0x80a6f57): a gunner's `turret_think_client`, which lets a
        // dead one go on his death frame (cod4-gsc.md 9.2).
        self.turret_end_frame(slot);
    }

    /// `GScr_ShellShock`'s `BG_AnimScriptEvent(ps, ANIM_ET_SHELLSHOCK, 0,
    /// 1)` on a live player (cod4_lnxded 0x80c4bab..0x80c4bcc): the
    /// `shellshock` block's clause for the body's conditions, its command
    /// by `rand() % n` off the level's stream (0x8050280), forced
    /// (cod4-combat.md 10.8). The stock script's standing and crouched
    /// clauses play `pb_stand_shellshock` on the torso for 5000 ms; prone
    /// has none and draws nothing.
    fn shellshock_anim(&mut self, slot: usize) {
        let Some(p) = self.players[slot].as_ref() else {
            return;
        };
        let mut ps = pmove::PlayerState::from_wire(&p.ps);
        (ps.anim_movetype, ps.anim_strafing) = p.anim_conditions;
        let buttons = p.cmd.buttons;
        let def = self.combat.defs.get(ps.weapon as usize);
        let Some(set) = self.combat.anim_set() else {
            return;
        };
        let input = pmove::anim::AnimInput {
            anims: &set.anims,
            player_anim_type: def.map_or(0, |d| d.player_anim_type),
            weapon_class: def.map_or(0, |d| d.weap_class),
        };
        let c4 = self.rt.host.cod4_mut();
        let pick = |n: usize| c4.rand() as usize % n;
        // A torso-only line reads -1 (the legs took nothing) yet plays.
        pmove::anim::script_event(&mut ps, buttons, &input, "shellshock", pick, (false, true));
        let p = self.players[slot].as_mut().unwrap();
        p.ps = ps.to_wire(&p.ps);
    }

    /// The `death` event's commands as `slot` stands now, for a kill this
    /// frame to draw from (`player_die`'s `BG_AnimScriptEvent`).
    fn death_choices(&mut self, slot: usize) {
        let Some(p) = self.players[slot].as_ref() else {
            return;
        };
        let mut ps = pmove::PlayerState::from_wire(&p.ps);
        (ps.anim_movetype, ps.anim_strafing) = p.anim_conditions;
        ps.pm_type = pmove::pm_type::DEAD;
        let buttons = p.cmd.buttons;
        let def = self.combat.defs.get(ps.weapon as usize);
        let choices = self.combat.anim_set().map_or_else(Vec::new, |set| {
            let input = pmove::anim::AnimInput {
                anims: &set.anims,
                player_anim_type: def.map_or(0, |d| d.player_anim_type),
                weapon_class: def.map_or(0, |d| d.weap_class),
            };
            pmove::anim::event_choices(&ps, buttons, &input, "death")
        });
        self.rt.host.cod4_mut().death_anims[slot] = choices;
    }

    /// `SpectatorClientEndFrame` (`cod4_lnxded` 0x80a49f0) past the
    /// killcam: the client followed, or with free flight refused the first
    /// one `Cmd_FollowCycle_f` finds, is watched when the archive holds it
    /// as a player on a team this client may watch; anyone else floats
    /// (cod4-gsc.md 7, the walker's follow and float; cod4-pmove.md 15.2).
    fn spectator_end_frame(&mut self, slot: usize) {
        if self.killcam_end_frame(slot) {
            return;
        }
        let refused = Refused(self.rt.host.client_no_spectate[slot]);
        match self.spectate_target(slot, refused) {
            Some((target, theirs)) => self.watch(slot, target, theirs, refused),
            None => self.float(slot, refused),
        }
    }

    /// Whom `slot` watches this frame and that client's archived
    /// playerstate. A target picked by the cycle is kept as followed even
    /// when the archive then refuses it (0x80a4c37 then 0x80a4b1c).
    fn spectate_target(&mut self, slot: usize, refused: Refused) -> Option<(usize, PlayerState)> {
        let followed = self.players[slot].as_ref()?.spectator_client;
        let target = match usize::try_from(followed) {
            Ok(t) => t,
            Err(_) if refused.free_flight() => {
                let t = self.follow_cycle(slot, 1, &|team| !refused.team(team))?;
                self.players[slot].as_mut()?.spectator_client = t as i32;
                t
            }
            Err(_) => return None,
        };
        let theirs = self.archive.get(target)?.clone()?;
        let is_player = theirs.word("otherFlags") & POF_PLAYER != 0;
        let team = self.rt.client_team(target) as u32;
        (is_player && !refused.team(team)).then_some((target, theirs))
    }

    /// The copy (0x80a4ba9..0x80a4c6e): `theirs` whole but this client's
    /// `eFlags` 0x100000, `otherFlags` without the player bit and with the
    /// follow and team bits, 0x10 where free flight is allowed; the
    /// current hudelems this client's own.
    fn watch(&mut self, slot: usize, target: usize, theirs: PlayerState, refused: Refused) {
        let Some(p) = self.players[slot].as_mut() else {
            return;
        };
        let free = if refused.free_flight() {
            0
        } else {
            POF_SPECTATE_FREE
        };
        let other = theirs.word("otherFlags") & !(POF_PLAYER | POF_SPECTATE_FREE)
            | POF_FOLLOW
            | POF_SPECTATE_TEAM
            | free;
        let e_flags = followed_eflags(&p.ps, &theirs);
        p.ps = theirs;
        p.ps.set("eFlags", e_flags);
        p.ps.set("otherFlags", other);
        p.spectator_client = target as i32;
        self.hud_into(slot, false);
    }

    /// Nobody to watch (0x80a4b59..0x80a4b98): a follower lets go
    /// (`StopFollowing`) and becomes a plain spectator; no view model,
    /// no player or free-flight bit, the team bit while any of the three
    /// teams may be watched.
    fn float(&mut self, slot: usize, refused: Refused) {
        let Some(p) = self.players[slot].as_ref() else {
            return;
        };
        if p.ps.word("otherFlags") & POF_FOLLOW != 0 {
            let cmd = p.cmd;
            self.stop_following(slot, Some(&cmd));
            let p = self.players[slot].as_mut().unwrap();
            p.ps.set("otherFlags", 0);
            p.ps.set("pm_type", PM_SPECTATOR);
        }
        let p = self.players[slot].as_mut().unwrap();
        p.spectator_client = -1;
        let team = if refused.every_team() {
            0
        } else {
            POF_SPECTATE_TEAM
        };
        let other = p.ps.word("otherFlags") & !(POF_PLAYER | POF_SPECTATE_FREE | POF_SPECTATE_TEAM);
        p.ps.set("otherFlags", other | team);
        p.ps.set("viewmodelIndex", 0);
    }

    /// The killcam: a spectator whose script set `spectatorclient` and an
    /// `archivetime` is sent the followed client's playerstate as it was
    /// that long ago, `otherFlags` the follow bit alone, `deltaTime` the
    /// look back and `killCamEntity` the script's `killcamentity` (1023
    /// for -1, cod4_lnxded 0x80a4a9b; the kill capture's 50000 reads
    /// the shooter's spawn frame of 46900 with `deltaTime` 3100, VERIFIED
    /// live). The snapshot's entities stay the live ones (cod4-combat.md
    /// 7).
    fn killcam_end_frame(&mut self, slot: usize) -> bool {
        let Some(session) = self.rt.client_session(slot) else {
            return false;
        };
        let (target, back) = (session.spectator_client, session.archive_ms);
        if target < 0 || back <= 0 {
            return false;
        }
        let at = self.level_time - back;
        // The oldest archived frame at or after the look back's time in
        // which the followed client is a player; a look back past it is
        // trimmed to it and written back (the kill capture's 4500 ms
        // `archivetime` reads `deltaTime` 3100: the shooter's spawn at
        // 46900, VERIFIED live).
        let found = self
            .history
            .iter()
            .rev()
            .filter(|(t, _)| *t >= at)
            .filter_map(|(t, f)| {
                let ps = f.get(target as usize)?.as_ref()?;
                (ps.word("otherFlags") & POF_PLAYER != 0).then(|| (*t, ps.clone()))
            })
            .next();
        let Some((t, theirs)) = found else {
            return false;
        };
        let back = self.level_time - t;
        self.rt.set_client_archive_ms(slot, back);
        // `self.killcamentity` (gclient +0x2f6c), -1 as none (0x80a4a9b).
        let kill_cam = self
            .rt
            .client_field(slot, "killcamentity")
            .and_then(|v| v.parse::<i32>().ok())
            .filter(|&n| n >= 0)
            .map_or(ENTITYNUM_NONE, |n| n as u32);
        let now = self.level_time;
        let p = self.players[slot].as_mut().unwrap();
        let e_flags = followed_eflags(&p.ps, &theirs);
        p.ps = theirs;
        shift_archived(&mut p.ps, back, now);
        p.ps.set("eFlags", e_flags);
        p.ps.set("otherFlags", POF_FOLLOW);
        p.ps.set("killCamEntity", kill_cam);
        p.killcam_at = Some(t);
        self.hud_into(slot, false);
        true
    }

    /// `setviewpos x y z [yaw [pitch]]` (`Cmd_SetViewpos_f`,
    /// `TeleportPlayer`): the eye to the point (the origin a view height under it,
    /// then a unit up), the view to the angles through `delta_angles` off
    /// the last cmd, the teleport bit flipped, the body relinked. The kill
    /// capture's shooter reads x 123 after `setviewpos 123 2110 300`
    /// (VERIFIED live).
    pub fn set_view_pos(&mut self, slot: usize, origin: [f32; 3], angles: [f32; 3]) {
        let Some(p) = self.players[slot].as_mut() else {
            return;
        };
        let h = f32::from_bits(p.ps.word("viewHeightCurrent"));
        let o = [origin[0], origin[1], origin[2] - h + 1.0];
        let e = p.ps.word("eFlags") ^ 2;
        p.ps.set("eFlags", e);
        let cmd = p.cmd;
        place(&mut p.ps, o, angles, Some(&cmd));
        let ps = pmove::PlayerState::from_wire(&p.ps);
        self.relink(slot, &ps);
    }

    /// `Cmd_Kill_f`: a playing client under `sv_cheats` (with
    /// health, `CheatsOk`) dies by its own hand: `player_die` as
    /// `MOD_SUICIDE` with 100000 damage, the `death` notify, the death anim
    /// and `CodeCallback_PlayerKilled`. Otherwise the refusal `CheatsOk`
    /// prints, both strings in 1.7's `cod4_lnxded` (VERIFIED). The reply
    /// to send, if any.
    /// Reseeds the scripts' `randomInt` / `randomFloat` state, so a test
    /// replays a stock script's draws (an airstrike's path) the same on
    /// every run; for tests.
    pub fn reseed_script_rand(&mut self, seed: u64) {
        self.rt.host.rng = seed | 1;
    }

    /// Starts `path::name(args...)` (string arguments) on slot `slot`'s
    /// player, the way a dev dvar's `/# #/` block calls the stock scripts
    /// (`_dev::updateDevSettings`'s `scr_giveradar`); for tests.
    pub fn start_player_script(
        &mut self,
        slot: usize,
        path: &str,
        name: &str,
        args: &[&str],
    ) -> anyhow::Result<()> {
        let id = self
            .rt
            .client_entity(slot)
            .ok_or_else(|| anyhow::anyhow!("slot {slot} has no player"))?;
        let args = self.rt.with_host(|_, cx| {
            args.iter()
                .map(|a| Value::String(cx.intern_exact(a)))
                .collect()
        });
        self.rt
            .start_with_args(path, name, Some(vcod_gsc::Target::Entity(id)), args)
    }

    /// The stock airstrike on `at` called by slot `slot`'s player, as a
    /// confirmed location runs it (`_hardpoints::useAirstrike`); for tests.
    pub fn start_airstrike(&mut self, slot: usize, at: [f32; 3]) -> anyhow::Result<()> {
        let id = self
            .rt
            .client_entity(slot)
            .ok_or_else(|| anyhow::anyhow!("slot {slot} has no player"))?;
        self.rt.start_with_args(
            "maps/mp/gametypes/_hardpoints",
            "useAirstrike",
            Some(vcod_gsc::Target::Entity(id)),
            vec![Value::Vector(at)],
        )
    }

    /// Test-facing: a living `slot` to 1100 health and `maxhealth` (the
    /// script's health writes clamp to it), more than any one frame's
    /// rounds take: the cmds of two packets can both run between two
    /// frames, and two head hits kill.
    pub fn heal_for_test(&mut self, slot: usize) {
        if let Some(v) = self.rt.host.client_vitals.get_mut(slot)
            && v.health > 0
        {
            v.max_health = 1100;
            v.health = 1100;
        }
    }

    /// Test-facing: `slot` takes `damage` without knockback along `dir`
    /// (`None`: from no direction), his health left alone, as a script's
    /// `finishPlayerDamage` followed by a heal.
    pub fn hurt_for_test(&mut self, slot: usize, damage: i32, dir: Option<[f32; 3]>) {
        self.rt
            .host
            .cod4_mut()
            .damage_ops
            .push(super::combat::Damage4 {
                slot,
                damage,
                dflags: super::combat::DFLAG_NO_KNOCKBACK,
                dir,
                fatal: false,
                attacker: None,
                death: None,
                dead_yaw: None,
            });
    }

    pub fn kill_command(&mut self, slot: usize) -> Option<String> {
        let playing = self
            .rt
            .client_session(slot)
            .is_some_and(|s| s.state == SessionState::Playing);
        if !playing {
            return None;
        }
        if crate::game::builtins::cvar::atoi(self.rt.host.cvars.get("sv_cheats")) == 0 {
            return Some("e \"GAME_CHEATSNOTENABLED\"".into());
        }
        if self.rt.client_vitals(slot).health <= 0 {
            return Some("e \"GAME_MUSTBEALIVECOMMAND\"".into());
        }
        let id = self.rt.client_entity(slot)?;
        self.rt.with_host(|h, cx| {
            crate::game::builtins::cod4_combat::death_grenades(
                h,
                cx,
                slot,
                Some(vcod_gsc::Target::Entity(id)),
                "MOD_SUICIDE",
            )
        });
        let host = &mut self.rt.host;
        host.die(slot);
        host.client_vitals[slot].health = 0;
        let choices = host.cod4().death_anims[slot].clone();
        let death = (!choices.is_empty()).then(|| {
            let k = host.cod4_mut().rand() as usize % choices.len();
            choices[k]
        });
        if let (Some(d), Some(c)) = (death, host.cod4_mut().corpse_source[slot].as_mut()) {
            c.legs_anim = d.legs_anim;
            c.torso_anim = d.torso_anim;
        }
        host.cod4_mut().damage_ops.push(super::combat::Damage4 {
            slot,
            damage: 0,
            dflags: super::combat::DFLAG_NO_KNOCKBACK,
            dir: None,
            fatal: true,
            attacker: Some(slot),
            death,
            dead_yaw: None,
        });
        self.apply_damage_ops();
        let args = self.rt.with_host(|_, cx| {
            let me = Value::Entity(id);
            vec![
                me,
                me,
                Value::Int(100_000),
                Value::String(cx.intern_exact("MOD_SUICIDE")),
                Value::String(cx.intern_exact("none")),
                Value::Undefined,
                Value::String(cx.intern_exact("none")),
            ]
        });
        self.rt
            .host
            .engine_notifies
            .push((id, "death", vec![Value::Entity(id)]));
        self.rt.host.cod4_mut().callback = crate::game::host::Cod2CallbackArgs {
            time_offset: 0,
            death_anim_duration: death.map_or(0, |d| d.duration),
        };
        if let Err(e) = self.rt.start_with_args(
            crate::game::script::CALLBACK_SETUP,
            "CodeCallback_PlayerKilled",
            Some(vcod_gsc::Target::Entity(id)),
            args,
        ) {
            log::error!("gsc: {e:#}");
        }
        self.rt.host.cod4_mut().callback = Default::default();
        None
    }

    /// `Cmd_FollowCycle_f(ent, dir)` (`cod4_lnxded` 0x80aebee): from the
    /// slot followed (0 with none) a step of `dir` at a time round the
    /// slots, back to the start, the first other client playing on a
    /// team `can` allows. VERIFIED (the instructions); "playing" stands
    /// for `SV_GetArchivedClientInfo` finding it. INFERRED.
    fn follow_cycle(&mut self, slot: usize, dir: i32, can: &dyn Fn(u32) -> bool) -> Option<usize> {
        let n = self.players.len() as i32;
        let start = self.players[slot].as_ref()?.spectator_client.max(0);
        let mut t = start;
        loop {
            t += dir;
            if t >= n {
                t = 0;
            }
            if t < 0 {
                t = n - 1;
            }
            let u = t as usize;
            let playing = u != slot
                && self.players[u].is_some()
                && self
                    .rt
                    .client_session(u)
                    .is_some_and(|s| s.state == SessionState::Playing);
            if playing && can(self.rt.client_team(u) as u32) {
                return Some(u);
            }
            if t == start {
                return None;
            }
        }
    }

    /// The scoreboard reply for a `score` command: `b <rows> <team 1>
    /// <team 2> <score limit>` then per client in `SortRanks`' order its
    /// number, score, ping (-1 connecting), deaths, status icon, kills and
    /// assists; the limit is `scr_<gametype>_scorelimit`, else the round
    /// limit. `pings` are the engine's by slot, `None` for a free one
    /// (cod4-hud.md 7).
    pub fn scoreboard(&mut self, pings: &[Option<i32>]) -> String {
        let gametype = self.rt.host.cvars.get("g_gametype").to_string();
        let int = |h: &crate::game::host::GameHost, n: &str| {
            crate::game::builtins::cvar::atoi(h.cvars.get(n))
        };
        let mut limit = int(&self.rt.host, &format!("scr_{gametype}_scorelimit"));
        if limit == 0 {
            limit = int(&self.rt.host, &format!("scr_{gametype}_roundlimit"));
        }
        let mut rows = Vec::new();
        for (slot, ping) in pings.iter().enumerate() {
            let Some(ping) = *ping else { continue };
            let Some(r) = self.rt.host.client_roster.get(slot).copied().flatten() else {
                continue;
            };
            let num = |v: Option<String>| v.and_then(|s| s.parse::<i64>().ok()).unwrap_or(0);
            let score = num(self.rt.client_field(slot, "score"));
            let deaths = num(self.rt.client_field(slot, "deaths"));
            let kills = num(self.rt.client_field(slot, "kills"));
            let assists = num(self.rt.client_field(slot, "assists"));
            let spectator = self
                .rt
                .client_field(slot, "sessionteam")
                .is_some_and(|t| t == "spectator");
            let icon = self
                .rt
                .client_field(slot, "statusicon")
                .filter(|n| !n.is_empty());
            let icon = crate::game::scoreboard::status_icon_index(
                &self.rt.host.configstrings,
                self.rt.host.allocators.layout(),
                icon.as_deref(),
            );
            rows.push((
                r.connecting,
                spectator,
                -score,
                deaths,
                slot,
                ping,
                kills,
                assists,
                icon,
            ));
        }
        rows.sort_by_key(|r| (r.0, r.1, r.2, r.3, r.4));
        let mut body = String::new();
        for &(connecting, _, score, deaths, slot, ping, kills, assists, icon) in &rows {
            let ping = if connecting { -1 } else { ping };
            body.push_str(&format!(
                " {slot} {} {ping} {deaths} {icon} {kills} {assists}",
                -score
            ));
        }
        let [axis, allies] = self.rt.host.team_scores;
        format!("b {} {axis} {allies} {limit}{body}", rows.len())
    }

    /// A client's playerstate as its next snapshot carries it.
    pub fn playerstate(&self, slot: usize) -> Option<&PlayerState> {
        self.players.get(slot)?.as_ref().map(|p| &p.ps)
    }

    /// One client state per client with a `Player4`: the team, the
    /// userinfo's name, the rank, the perks and the body the script set.
    pub fn client_state(&mut self, slot: usize, name: &str) -> ClientState {
        let mut cs = ClientState::default();
        cs.0[0] = slot as u32;
        let team = match self.rt.client_entity(slot) {
            Some(_) => match self.rt.client_team(slot) {
                t if t < 0 => TEAM_SPECTATOR,
                t => t as u32,
            },
            None => TEAM_SPECTATOR,
        };
        cs.0[cs_word("team")] = team;
        cs.0[cs_word("attachedVehEntNum")] = ENTITYNUM_NONE;
        let mut bytes = [0u8; 16];
        for (d, s) in bytes.iter_mut().zip(name.bytes().take(15)) {
            *d = s;
        }
        let at = cs_word("name[0]");
        for k in 0..4 {
            cs.0[at + k] = u32::from_le_bytes(bytes[k * 4..k * 4 + 4].try_into().unwrap());
        }
        let c4 = &self.rt.host.cod4().clients[slot];
        cs.0[cs_word("rank")] = u32::from(c4.rank);
        cs.0[cs_word("prestige")] = u32::from(c4.prestige);
        if let Some(p) = self.players[slot].as_ref()
            && p.ps.word("otherFlags") & POF_FOLLOW == 0
        {
            cs.0[cs_word("perks")] = p.ps.word("perks");
        }
        // The body: `setModel` and `attach` (`client_attachments`, a model
        // configstring index and a tag index per attachment).
        let model = self.rt.client_model_index(slot);
        cs.0[cs_word("modelindex")] = model as u32;
        for (k, (m, t)) in self
            .rt
            .client_attachments(slot)
            .into_iter()
            .take(6)
            .enumerate()
        {
            cs.0[cs_word(&format!("attachModelIndex[{k}]"))] = m as u32;
            cs.0[cs_word(&format!("attachTagIndex[{k}]"))] = t as u32;
        }
        cs
    }

    /// The level's entities in CoD4's numbering: the script's entities
    /// through [`super::wire::ent4`], then each playing client's body
    /// (`BG_PlayerStateToEntityState`). A hidden one is left out: `hide`
    /// sets every client's bit in `r.clientMask`, and the walk
    /// capture's four bomb sites `war` hides (105, 109, 120, 364, their
    /// baselines' `eFlags` 0x20) reach no client in any 1.7 capture
    /// (VERIFIED live, cod4-pmove.md 15).
    pub fn entities(&mut self) -> BTreeMap<u32, EntityState> {
        let all = self.rt.packet_entities(&super::COD4_HOST_PROTO);
        let ents = &self.rt.host.ents;
        let mut out: BTreeMap<u32, EntityState> = all
            .iter()
            .filter(|&(&n, _)| {
                !ents
                    .handle(n)
                    .and_then(|id| ents.get(id))
                    .is_some_and(|e| e.hidden)
            })
            .map(|(_, e)| e)
            .filter_map(super::wire::ent4)
            .map(|e| (e.number(), self.with_parts(e)))
            .collect();
        for (slot, p) in self.players.iter().enumerate() {
            let Some(p) = p else { continue };
            out.remove(&(slot as u32));
            if p.ps.word("otherFlags") & POF_PLAYER != 0 && in_play(p.ps.word("pm_type")) {
                let mut e = player_entity(slot, &p.ps);
                p.body_events.write(&mut e);
                out.insert(slot as u32, e);
            }
        }
        // The body queue's corpses, the dropped weapons and the temp
        // entities, each under its own number.
        let bodies: Vec<_> = self
            .rt
            .body_entities(&super::COD4_HOST_PROTO)
            .filter_map(|(_, e)| super::wire::ent4(&e))
            .collect();
        for e in bodies {
            out.insert(e.number(), e);
        }
        for e in self.item_entities() {
            out.insert(e.number(), e);
        }
        for e in self.missile_entities() {
            out.insert(e.number(), e);
        }
        for (e, _) in self.temp_entities4() {
            out.insert(e.number(), e);
        }
        // `spawnFx` and `playLoopedFx`'s effects while their entity lives.
        let host = &mut self.rt.host;
        let dead: Vec<_> = host
            .cod4()
            .fx
            .keys()
            .filter(|id| host.ents.get(**id).is_none())
            .copied()
            .collect();
        for id in dead {
            host.cod4_mut().fx.remove(&id);
        }
        for (id, fx) in &self.rt.host.cod4().fx {
            out.insert(id.0, fx.entity(id.0));
        }
        self.vehicle_entities(&mut out);
        self.turret_entities(&mut out);
        out
    }

    /// Keeps a snapshot round's entities for the killcam's look back.
    pub fn archive_entities(&mut self, world: &BTreeMap<u32, EntityState>) {
        let now = self.level_time;
        if self
            .entity_history
            .front()
            .is_some_and(|(t, _, _)| *t == now)
        {
            return;
        }
        let scopes: Vec<_> = self
            .rt
            .temp_entities()
            .map(|(n, te)| (n, te.scope))
            .collect();
        self.entity_history
            .push_front((now, Arc::new(world.clone()), Arc::new(scopes)));
        let keep = self.history_frames();
        self.entity_history.truncate(keep);
    }

    /// The entities a killcam follower is sent: the archived round at or
    /// before the frame its playerstate came from (cod4-combat.md 7),
    /// without the temp entities the followed client was not sent (the
    /// victim's own flesh event, 17 + 42 or 43). A temp entity past its
    /// look back is not in it. Each trajectory's time, where set, moves
    /// up by the playerstate's `deltaTime`, as the archived playerstate's
    /// clocks do: the killcam entity capture's mover started at 41550
    /// reads 45800 in a killcam 4250 back.
    pub fn killcam_entities(&self, slot: usize) -> Option<BTreeMap<u32, EntityState>> {
        let p = self.players.get(slot)?.as_ref()?;
        let t = p.killcam_at?;
        let back = p.ps.word("deltaTime");
        let followed = p.ps.word("clientNum") as usize;
        let (_, w, scopes) = self.entity_history.iter().find(|(at, _, _)| *at <= t)?;
        let mut world = (**w).clone();
        for (n, scope) in scopes.iter() {
            if !scope.admits(followed) {
                world.remove(n);
            }
        }
        for e in world.values_mut() {
            for name in ["lerp.pos.trTime", "lerp.apos.trTime"] {
                if let Some(w) = ent_word(e.etype(), name)
                    && e.0[w] != 0
                {
                    e.0[w] = e.0[w].wrapping_add(back);
                }
            }
        }
        Some(world)
    }

    /// `SV_CreateBaseline`: every entity as the load left it.
    pub fn baselines(&mut self) -> BTreeMap<u32, EntityState> {
        let mut out = self
            .rt
            .baseline_entities(&super::COD4_HOST_PROTO)
            .values()
            .filter_map(super::wire::ent4)
            .map(|e| (e.number(), self.with_parts(e)))
            .collect();
        self.turret_entities(&mut out);
        out
    }

    /// The entity's `partBits` as `hidePart` left them, for the handle
    /// that still holds its number.
    fn with_parts(&self, mut e: EntityState) -> EntityState {
        let host = &self.rt.host;
        if let Some((id, bits)) = host.cod4().part_bits.get(&e.number())
            && host.ents.get(*id).is_some()
        {
            for (k, b) in bits.iter().enumerate() {
                if let Some(w) = ent_word(e.etype(), &format!("partBits[{k}]")) {
                    e.0[w] = *b;
                }
            }
        }
        e
    }
}

/// The playerstate `ClientSpawn`'s memset (0x80a7cc9) leaves `slot`
/// before the spot: of the old one only `eFlags` 0x100002, with 0x2
/// flipped (0x80a7c63, 0x80a7d68), and the spawn count, stepped
/// (`stats[4]`, 0x80a7cfe); then the maximum health (`stats[2]`), the
/// slot, no view lock and no grenade thrower (1023), the view height 60,
/// the depth of field's blurs 6 and 1.8 (0x80a7d6f..0x80a7dbc), the
/// session's move type and `PMF_RESPAWNED`. The walk capture's spawn frame
/// reads them all (cod4-gsc.md 7). VERIFIED.
fn spawn_playerstate(
    old: &PlayerState,
    slot: usize,
    max_health: i32,
    mode: SpawnMode,
) -> PlayerState {
    let pm_type = match mode {
        SpawnMode::Player => PM_NORMAL,
        SpawnMode::Spectator => PM_SPECTATOR,
        SpawnMode::Intermission => PM_INTERMISSION,
    };
    let mut ps = PlayerState::default();
    for (word, v) in [
        ("eFlags", (old.word("eFlags") & 0x10_0002) ^ 0x2),
        ("clientNum", slot as u32),
        ("viewlocked_entNum", ENTITYNUM_NONE),
        ("throwBackGrenadeOwner", ENTITYNUM_NONE),
        ("viewHeightTarget", 60),
        ("pm_type", pm_type),
        ("pm_flags", pmove::pmf::RESPAWNED),
    ] {
        ps.set(word, v);
    }
    for (word, v) in [
        ("viewHeightCurrent", 60.0),
        ("dofNearBlur", 6.0),
        ("dofFarBlur", 1.8),
    ] {
        ps.setf(word, v);
    }
    ps.stats[2] = max_health;
    ps.stats[4] = old.stats[4] + 1;
    ps
}

/// `G_SetOrigin`, `SetClientViewAngle`: the origin, `delta_angles` the
/// spot's view less the last cmd's, a float wrapped to -180..180 as
/// `AngleNormalize180` does (cod4-pmove.md 15), and the view itself. The
/// wire's half-short rounding (`angle16_sent`) is what makes the
/// spectator's 65 read 65.00061 (VERIFIED live).
fn place(ps: &mut PlayerState, origin: [f32; 3], angles: [f32; 3], cmd: Option<&UserCmd>) {
    for i in 0..3 {
        ps.setf(&format!("origin[{i}]"), origin[i]);
        let cmd_angle = cmd.map_or(0, |c| i32::from(c.angles[i]));
        let d = angles[i] - (cmd_angle as f32 * 0.005_493_164_f32);
        let turns = d * 0.002_777_777_8_f32;
        let delta = (turns - (turns + 0.5).floor()) * 360.0;
        ps.setf(&format!("delta_angles[{i}]"), delta);
        ps.setf(&format!("viewangles[{i}]"), angles[i]);
    }
}

/// [`place`] for the other level modules.
pub(super) fn place_player(
    ps: &mut PlayerState,
    origin: [f32; 3],
    angles: [f32; 3],
    cmd: Option<&UserCmd>,
) {
    place(ps, origin, angles, cmd);
}

/// The view without a clip map: the cmd's plus `delta_angles`, as
/// `PM_UpdateViewAngles` sums them.
fn view_only(ps: &mut PlayerState, cmd: &Cmd) {
    for i in 0..3 {
        let delta = ps.word(&format!("delta_angles[{i}]"));
        let view =
            (f64::from(cmd.angles[i]) * 0.0054931640625 + f64::from(f32::from_bits(delta))) as f32;
        ps.setf(&format!("viewangles[{i}]"), view);
    }
}

/// `r.maxs[2]` as `PM_CheckDuck` leaves it: 30 prone, 50 crouched, 70
/// standing.
fn body_height(pm_flags: u32) -> f32 {
    if pm_flags & pmove::pmf::PRONE != 0 {
        30.0
    } else if pm_flags & pmove::pmf::DUCKED != 0 {
        50.0
    } else {
        70.0
    }
}

/// The loadout's playerstate fields.
fn loadout_into(ps: &mut PlayerState, l: &super::weapons::Loadout) {
    for i in 0..4 {
        ps.set(&format!("weapons[{i}]"), l.weapons[i]);
        ps.set(&format!("weaponold[{i}]"), l.weaponold[i]);
        ps.set(
            &format!("actionSlotType[{i}]"),
            u32::from(l.action_slot_type[i]),
        );
        ps.set(&format!("actionSlotParam[{i}]"), l.action_slot_param[i]);
    }
    ps.set("weapon", l.weapon as u32);
    ps.set("offHandIndex", l.offhand as u32);
    ps.set("offhandSecondary", l.offhand_secondary);
    let n = ps.ammo.len();
    ps.ammo.copy_from_slice(&l.ammo[..n]);
    let m = ps.ammoclip.len();
    ps.ammoclip.copy_from_slice(&l.ammoclip[..m]);
    ps.weapon_models = l.weapon_models;
}

/// The entity's `fTorsoPitch` and `fWaistPitch`: 0 unless the eye's target
/// is prone (`PM_GetEffectiveStance` 1, 0x8055ffe), else the playerstate's
/// wrapped to -180..180 (`(x - floor(x + 0.5)) * 360` of the turns, the
/// floor stored as a float) times the eye lerp's fraction toward prone
/// (`1 - f` on a lerp up). 0x8052cc1..0x8052d5f.
fn prone_pitches(ps: &PlayerState) -> (f32, f32) {
    let int = |n: &str| ps.word(n) as i32;
    if int("viewHeightTarget") != 11 {
        return (0.0, 0.0);
    }
    let lerp_time = int("viewHeightLerpTime");
    let frac = if lerp_time == 0 {
        1.0
    } else {
        let down = int("viewHeightLerpDown") != 0;
        let dur = vcod_cod4::pmove::view_height_lerp_duration(int("viewHeightLerpTarget"), down);
        let f = (f64::from(int("commandTime") - lerp_time) / f64::from(dur)).clamp(0.0, 1.0);
        if down { f } else { 1.0 - f }
    };
    let wrap = |name: &str| {
        let t = f64::from(f32::from_bits(ps.word(name))) * f64::from(1.0f32 / 360.0);
        let floor = (t + 0.5).floor() as f32;
        ((t - f64::from(floor)) * 360.0 * frac) as f32
    };
    (wrap("fTorsoPitch"), wrap("fWaistPitch"))
}

/// `BG_PlayerStateToEntityState` (1.7's 0x8052940, inlined): the snapped
/// origin and view angles (the capture's teammate at z 236 off its
/// playerstate's 236.125), the lean, a prone body's torso and waist
/// pitches, the weapon, the stance flags, the ground, and the box packed
/// as `SV_LinkEntity` does (`x | (1 - mins.z) << 8 | (maxs.z + 32) << 16`:
/// 0x66010f for the 30 x 70 body, VERIFIED live); `pos` reads `trType` 3
/// and `apos` 1 on the capture's still teammate (VERIFIED live). The
/// angles' and pitches' sources are cod4-player-anims.md 8.
pub fn player_entity(slot: usize, ps: &PlayerState) -> EntityState {
    let mut e = EntityState::default();
    e.0[0] = slot as u32;
    e.0[1] = 1;
    let mut set = |name: &str, v: u32| {
        if let Some(w) = ent_word(1, name) {
            e.0[w] = v;
        }
    };
    for i in 0..3 {
        let o = f32::from_bits(ps.word(&format!("origin[{i}]")));
        set(&format!("lerp.pos.trBase[{i}]"), o.trunc().to_bits());
    }
    for i in 0..3 {
        let a = f32::from_bits(ps.word(&format!("viewangles[{i}]")));
        set(&format!("lerp.apos.trBase[{i}]"), a.trunc().to_bits());
    }
    set("lerp.u.player.leanf", ps.word("leanf"));
    let (torso, waist) = prone_pitches(ps);
    set("fTorsoPitch", torso.to_bits());
    set("fWaistPitch", waist.to_bits());
    set("lerp.u.player.movementDir", ps.word("movementDir"));
    set("lerp.pos.trType", 3);
    set("lerp.apos.trType", 1);
    // 0x20000 for a dead body (`pm_type` past 6), 0x40000 for one down
    // its sights (`PMF_SIGHT` 0x10), each cleared otherwise
    // (0x8052a54..0x8052a82, 0x8052ca2, 0x8052cb1).
    let mut e_flags = ps.word("eFlags") & !0x6_0000;
    if ps.word("pm_type") > 6 {
        e_flags |= 0x2_0000;
    }
    if ps.word("pm_flags") & 0x10 != 0 {
        e_flags |= 0x4_0000;
    }
    set("lerp.eFlags", e_flags);
    set("weapon", ps.word("weapon"));
    set("legsAnim", ps.word("legsAnim"));
    set("torsoAnim", ps.word("torsoAnim"));
    set("groundEntityNum", ps.word("groundEntityNum"));
    // A gunner names his gun: `eFlags & 0x300` copies `viewlocked_entNum`
    // (ps+0x59c) into `otherEntityNum` (0x8052ab9; cod4-gsc.md 9.2).
    if ps.word("eFlags") & 0x300 != 0 {
        set("otherEntityNum", ps.word("viewlocked_entNum"));
    }
    set("clientNum", slot as u32);
    let h = body_height(ps.word("pm_flags")) as u32;
    set("solid", 15 | (1 << 8) | ((h + 32) << 16));
    e
}

/// The playerstate clocks an archived frame carries (`commandTime`,
/// `pm_time`, `foliageSoundTime`, `jumpTime`, `viewHeightLerpTime`,
/// `shellshockTime`: +0, +0x18, +0x54, +0x80, +0x11c, +0x630).
const ARCHIVED_CLOCKS: [&str; 6] = [
    "commandTime",
    "pm_time",
    "foliageSoundTime",
    "jumpTime",
    "viewHeightLerpTime",
    "shellshockTime",
];

/// A `hudelem_s`'s words: its type, and the clocks it runs on (`time`
/// +0x70, `scaleStartTime` +0x50, `moveStartTime` +0x68); the fade's start
/// (+0x2c) moves too but never past now.
const HE_TYPE: usize = 0;
const HE_CLOCKS: [usize; 3] = [0x70 / 4, 0x50 / 4, 0x68 / 4];
const HE_FADE_START: usize = 0x2c / 4;

/// `SV_GetArchivedClientInfo` (`cod4_lnxded` 0x81795e2) on a frame `back`
/// ms old: each clock that is set moves up by `back`, the archival
/// hudelems' up to the first unused one, and `deltaTime` grows by it
/// (0x81797bc..0x81798aa, cod4-combat.md 7). The killcam's first frame
/// reads `commandTime` 50000 off the shooter's 46900 that way.
fn shift_archived(ps: &mut PlayerState, back: i32, now: i32) {
    let later = |t: u32| match t {
        0 => 0,
        t => t.wrapping_add(back as u32),
    };
    for clock in ARCHIVED_CLOCKS {
        let t = later(ps.word(clock));
        ps.set(clock, t);
    }
    let used = ps.hud_archival.iter_mut().take_while(|h| h.0[HE_TYPE] != 0);
    for elem in used {
        for w in HE_CLOCKS {
            elem.0[w] = later(elem.0[w]);
        }
        if elem.0[HE_FADE_START] != 0 {
            let fade = later(elem.0[HE_FADE_START]) as i32;
            elem.0[HE_FADE_START] = fade.min(now) as u32;
        }
    }
    let delta = ps.word("deltaTime").wrapping_add(back as u32);
    ps.set("deltaTime", delta);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The killcam's shift: the six playerstate clocks and the archival
    /// hudelems' four move up by the look back only where set, a fade's
    /// start no later than now, the walk stopping at the first free
    /// element; `deltaTime` always.
    #[test]
    fn an_archived_playerstate_moves_its_clocks_up_by_the_look_back() {
        let mut ps = PlayerState::default();
        ps.set("commandTime", 46_900);
        ps.set("pm_time", 0);
        ps.set("jumpTime", 46_000);
        ps.set("shellshockTime", 45_000);
        ps.set("deltaTime", 100);
        ps.hud_archival[0].0[0] = 1;
        ps.hud_archival[0].0[11] = 46_950;
        ps.hud_archival[0].0[28] = 40_000;
        ps.hud_archival[1].0[0] = 9;
        ps.hud_archival[1].0[11] = 45_000;
        ps.hud_archival[1].0[20] = 44_000;
        ps.hud_archival[1].0[26] = 43_000;
        ps.hud_archival[3].0[0] = 1;
        ps.hud_archival[3].0[28] = 1_000;
        shift_archived(&mut ps, 3_100, 50_000);
        assert_eq!(ps.word("commandTime"), 50_000);
        assert_eq!(ps.word("pm_time"), 0);
        assert_eq!(ps.word("jumpTime"), 49_100);
        assert_eq!(ps.word("shellshockTime"), 48_100);
        assert_eq!(ps.word("foliageSoundTime"), 0);
        assert_eq!(ps.word("deltaTime"), 3_200);
        let h = &ps.hud_archival;
        assert_eq!((h[0].0[11], h[0].0[28], h[0].0[20]), (50_000, 43_100, 0));
        assert_eq!(
            (h[1].0[11], h[1].0[20], h[1].0[26], h[1].0[28]),
            (48_100, 47_100, 46_100, 0)
        );
        // Past the free element 2: untouched.
        assert_eq!(h[3].0[28], 1_000);
    }

    /// The body's ring takes the playerstate's last four events at most,
    /// none twice, the server-only and single-client ones left out, the
    /// parm a byte, its sequence wrapping at 256.
    #[test]
    fn the_body_ring_takes_the_last_four_events_but_the_private_ones() {
        let mut ps = PlayerState::default();
        let mut ring = BodyEvents::default();
        let fire = |ps: &mut PlayerState, seq: u32, event: u32, parm: u32| {
            ps.set(&format!("events[{}]", seq & 3), event);
            ps.set(&format!("eventParms[{}]", seq & 3), parm);
            ps.set("eventSequence", seq + 1);
        };
        fire(&mut ps, 0, 25, 0x1ff);
        fire(&mut ps, 1, 31, 7);
        fire(&mut ps, 2, 6, 7);
        ring.take(&ps);
        assert_eq!((ring.taken, ring.seq), (3, 1));
        assert_eq!((ring.events[0], ring.parms[0]), (25, 0xff));
        // Nothing new: nothing taken.
        ring.take(&ps);
        assert_eq!(ring.seq, 1);
        // Six behind: only the last four.
        for (k, ev) in [50, 51, 52, 53, 54, 55].into_iter().enumerate() {
            fire(&mut ps, 3 + k as u32, ev, k as u32);
        }
        ring.take(&ps);
        assert_eq!((ring.taken, ring.seq), (9, 5));
        assert_eq!(ring.events, [55, 52, 53, 54]);
        assert_eq!(ring.parms, [5, 2, 3, 4]);
        // A sequence behind the ring's (a fresh playerstate): nothing.
        ps.set("eventSequence", 2);
        ring.take(&ps);
        assert_eq!((ring.taken, ring.seq), (2, 5));
        // The ring's sequence wraps at 256.
        ring.seq = 255;
        fire(&mut ps, 2, 60, 1);
        ring.take(&ps);
        assert_eq!((ring.seq, ring.events[3]), (0, 60));
    }

    /// A dm level on mp_crash with two connected clients and their
    /// script entities, before anyone spawns; `None` without `COD4_DIR`.
    fn spectating_level() -> Option<Cod4Level> {
        let install = vcod_cod4::testing::cod4_install()?;
        let mut sv = crate::cod4::Cod4Server::new(
            crate::cod4::Cod4Config {
                map: "mp_crash".into(),
                gametype: "dm".into(),
                hostname: "vcod".into(),
                max_clients: 8,
            },
            7,
        );
        sv.set_cvar("sv_pure", "0");
        sv.load(Some(&install)).unwrap();
        let mut level = sv.game.level.take()?;
        let spot = ([0.0, 0.0, 100.0], [0.0, 90.0, 0.0]);
        for slot in 0..3 {
            level.connect(slot, &format!("c{slot}"), spot);
            level.begin(slot);
        }
        let mut t = level.level_time();
        for _ in 0..10 {
            t += 50;
            level.run_frame(t);
        }
        Some(level)
    }

    /// `slot`'s `sessionstate` and `sessionteam` as a script would set
    /// them, and a playing one's archived playerstate a player's.
    fn put(level: &mut Cod4Level, slot: usize, state: &str, team: &str) {
        level.rt.set_client_state_for_test(slot, state);
        level.rt.set_client_team_for_test(slot, team);
        let p = level.players[slot].as_mut().unwrap();
        if state == "playing" {
            p.ps.set("otherFlags", POF_PLAYER);
            p.ps.set("eFlags", 0x22);
            p.ps.set("clientNum", slot as u32);
        }
        level.archive[slot] = Some(p.ps.clone());
    }

    /// What a spectator's end frame leaves: `otherFlags`, the slot
    /// followed, the playerstate's `clientNum` and `eFlags`.
    fn watch(level: &mut Cod4Level, slot: usize, refused: u8) -> (u32, i32, u32, u32) {
        level.rt.host.client_no_spectate[slot] = refused;
        level.spectator_end_frame(slot);
        let p = level.players[slot].as_ref().unwrap();
        (
            p.ps.word("otherFlags"),
            p.spectator_client,
            p.ps.word("clientNum"),
            p.ps.word("eFlags"),
        )
    }

    /// `SpectatorClientEndFrame` with nobody to watch: free flight floats
    /// with `otherFlags` 8 while any team may be watched, none when every
    /// team and free flight are refused; refused free flight with nobody
    /// playing floats too.
    #[test]
    fn a_spectator_with_nobody_playing_floats() {
        let Some(mut level) = spectating_level() else {
            return;
        };
        for slot in 0..3 {
            put(&mut level, slot, "spectator", "spectator");
        }
        level.players[0]
            .as_mut()
            .unwrap()
            .ps
            .set("eFlags", 0x10_0000);
        assert_eq!(watch(&mut level, 0, 0), (8, -1, 0, 0x10_0000));
        assert_eq!(watch(&mut level, 0, 0x17), (0, -1, 0, 0x10_0000));
        assert_eq!(watch(&mut level, 0, 0x10), (8, -1, 0, 0x10_0000));
        assert_eq!(watch(&mut level, 0, 0x16), (8, -1, 0, 0x10_0000));
        let ps = &level.players[0].as_ref().unwrap().ps;
        assert_eq!(ps.word("viewmodelIndex"), 0);
        assert_eq!(ps.word("pm_type"), PM_SPECTATOR);
    }

    /// Refused free flight follows the first playing client of a team it
    /// may watch, its archived playerstate copied but this client's
    /// `eFlags` 0x100000; `otherFlags` 0xa, 0x1a once free flight is
    /// allowed; a team refused later lets go and floats.
    #[test]
    fn a_spectator_follows_the_first_player_it_may_watch() {
        let Some(mut level) = spectating_level() else {
            return;
        };
        put(&mut level, 0, "spectator", "spectator");
        put(&mut level, 1, "playing", "allies");
        put(&mut level, 2, "playing", "axis");
        level.players[0]
            .as_mut()
            .unwrap()
            .ps
            .set("eFlags", 0x10_0000);
        // Allies refused: the axis player.
        assert_eq!(watch(&mut level, 0, 0x14), (0xa, 2, 2, 0x10_0022));
        // Still following, now free flight is allowed.
        assert_eq!(watch(&mut level, 0, 0x04), (0x1a, 2, 2, 0x10_0022));
        // Axis refused too: let go, float.
        let (of, followed, num, _) = watch(&mut level, 0, 0x16);
        assert_eq!((of, followed, num), (8, -1, 0));
        assert_eq!(
            level.players[0].as_ref().unwrap().ps.word("pm_type"),
            PM_SPECTATOR
        );
        // Every team allowed, free flight refused: the allies player first.
        assert_eq!(watch(&mut level, 0, 0x10), (0xa, 1, 1, 0x10_0022));
    }

    /// `Cmd_FollowCycle_f` steps round the slots from the one followed,
    /// both ways, wrapping, past spectators and teams refused.
    #[test]
    fn follow_cycling_steps_round_the_players() {
        let Some(mut level) = spectating_level() else {
            return;
        };
        put(&mut level, 0, "spectator", "spectator");
        put(&mut level, 1, "playing", "allies");
        put(&mut level, 2, "playing", "axis");
        let all = |_: u32| true;
        let cycle = |level: &mut Cod4Level, from: i32, dir: i32, can: &dyn Fn(u32) -> bool| {
            level.players[0].as_mut().unwrap().spectator_client = from;
            level.follow_cycle(0, dir, can)
        };
        assert_eq!(cycle(&mut level, -1, 1, &all), Some(1));
        assert_eq!(cycle(&mut level, 1, 1, &all), Some(2));
        assert_eq!(cycle(&mut level, 2, 1, &all), Some(1));
        assert_eq!(cycle(&mut level, 1, -1, &all), Some(2));
        assert_eq!(cycle(&mut level, 2, -1, &all), Some(1));
        let no_axis = |t: u32| t != TEAM_AXIS;
        assert_eq!(cycle(&mut level, 1, 1, &no_axis), Some(1));
        let none = |_: u32| false;
        assert_eq!(cycle(&mut level, 1, 1, &none), None);
    }

    /// Others see a body's aim and lean: `apos` is the view `(int)`, which
    /// the -100 field sends a half short up (the kill capture's aiming
    /// shooter reads pitch 1.99951 and yaw -68.99414, cod4-player-anims.md
    /// 8), `leanf` rides
    /// along, and the torso and waist pitches only prone, eased by the
    /// eye's lerp.
    #[test]
    fn a_body_carries_its_aim_its_lean_and_its_prone_pitches() {
        let f = |v: f32| v.to_bits();
        let mut ps = PlayerState::default();
        ps.set("viewangles[0]", f(2.7));
        ps.set("viewangles[1]", f(-69.6));
        ps.set("leanf", f(-0.25));
        ps.set("fTorsoPitch", f(10.0));
        ps.set("fWaistPitch", f(350.0));
        ps.set("viewHeightTarget", 60);
        let word = |e: &EntityState, n: &str| f32::from_bits(e.0[ent_word(1, n).unwrap()]);
        let e = player_entity(3, &ps);
        assert_eq!(word(&e, "lerp.apos.trBase[0]"), 2.0);
        assert_eq!(word(&e, "lerp.apos.trBase[1]"), -69.0);
        let sent = e.sent();
        assert_eq!(word(&sent, "lerp.apos.trBase[0]"), 1.9995117);
        assert_eq!(word(&sent, "lerp.apos.trBase[1]"), -68.99414);
        assert_eq!(word(&e, "lerp.u.player.leanf"), -0.25);
        assert_eq!(word(&e, "fTorsoPitch"), 0.0);
        ps.set("viewHeightTarget", 11);
        let e = player_entity(3, &ps);
        assert_eq!(word(&e, "fTorsoPitch"), 10.0);
        assert!((word(&e, "fWaistPitch") + 10.0).abs() < 1e-3);
        // Halfway down into prone: half the pitch.
        ps.set("commandTime", 1200);
        ps.set("viewHeightLerpTime", 1000);
        ps.set("viewHeightLerpTarget", 11);
        ps.set("viewHeightLerpDown", 1);
        let e = player_entity(3, &ps);
        assert_eq!(word(&e, "fTorsoPitch"), 5.0);
    }
}
