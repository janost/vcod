//! CoD2's game module over the shared script host: the level's collision
//! world, each client's CoD2 `playerState` and its `ClientThink`, the
//! engine notifies only CoD2 raises, and the callback arguments CoD2 adds.
//!
//! [`Cod2Level`] is the game half `server.rs` will drive for a CoD2 level:
//! it owns the [`ScriptRuntime`] (`load_cod2_in`) and one
//! [`vcod_cod2::pmove`] state per client, and takes the engine's calls in
//! retail's order (connect, begin, menu responses, one `client_think` per
//! cmd, one `run_frame` per `SV_Frame`). The netchan, the gamestate and the
//! snapshot writers stay the engine's. The builtins themselves stay on the
//! shared [`GameHost`], CoD2's own in `builtins::cod2`.

use std::rc::Rc;

use glam::Vec3;
use vcod_cod2::net::UserCmd2;
use vcod_cod2::pmove::{PlayerState, Rules, pm_type};
use vcod_common::movetrace::CONTENTS_BODY;
use vcod_common::pk3::Pk3Fs;

use crate::game::host::{ClientEvent, GameHost, SimOp, SpawnMode};
use crate::game::scoreboard::RosterSlot;
use crate::game::script::{MenuRef, PakScripts, ScriptRuntime};

/// What CoD2's builtins read of a client that the shared host has no field
/// for, by slot (`Cod2HostState::clients`).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Cod2ClientState {
    /// `ps.offHandIndex` (ps+0xd0): the offhand weapon the client's cmds
    /// select, `getCurrentOffhand`'s answer.
    pub offhand: u8,
    /// `ps.viewangles` after the client's last think, `getPlayerAngles`'
    /// answer (`PlayerCmd_getAngles` 0x80fa9ec reads ps+0xe8).
    pub view_angles: [f32; 3],
    /// `gclient+0x289c`, the time the client last saw the world at, which
    /// `SV_ClientThink` (0x8090dac) moves to each cmd's `serverTime`
    /// through 0x80f5614 ([`lag_time`]).
    pub lag_time: i32,
    /// The `death` clause's anims for the client's movetype, as wire index
    /// and `BG_AnimScriptEvent` duration (the clip's length plus 50), which
    /// `finishPlayerDamage` draws from on a kill; `death_anim_count` of
    /// them are live.
    pub death_anims: [(i32, i32); 8],
    pub death_anim_count: u8,
    /// The death anim the kill drew, the dead body's `legsAnim` and
    /// `torsoAnim` (0 for none).
    pub death_anim: i32,
    /// `ps.stats[5]`: the spawns since connect, which `ClientSpawn`
    /// (0x80f910e) counts across its memset.
    pub spawn_count: i32,
    /// `ent->flags` (gentity+0x174): god, demigod, notarget
    /// (`game::cod2_cmds::FL_*`).
    pub flags: u32,
    /// `gclient+0x27ac` and `+0x27b0`, the `noclip` and `ufo` toggles.
    pub noclip: bool,
    pub ufo: bool,
    /// The body a blast measures while the client plays, as its last think
    /// or end frame left it (`radiusDamage`, `game::cod2_blast`).
    pub body: Option<crate::game::cod2_blast::BlastBody>,
    /// `ps.stats[3]` and `stats[4]`: the client under the crosshair the
    /// last frame's scan found and his health (`game::cod2_identify`).
    pub identify: Option<(u8, i32)>,
    /// `gclient+0x2808`, `lastVoiceTime`: the level time `G_BroadcastVoice`
    /// last relayed a frame of the client's (cod2-voice.md 3).
    pub last_voice_time: Option<i32>,
}

/// 0x80f5614, the cmd half of `SV_ClientThink`: a cmd more than 1000 ms
/// behind `level_time` counts as 1000 behind, and the stored time moves
/// only forward, or to a time at or past `level_time`. VERIFIED.
pub fn lag_time(stored: i32, cmd_time: i32, level_time: i32) -> i32 {
    let t = if level_time - cmd_time > 1000 {
        level_time - 1000
    } else {
        cmd_time
    };
    if stored <= t || level_time <= t {
        t
    } else {
        stored
    }
}

/// A bullet's `timeOffset`: `G_Damage`'s tenth argument from the bullet
/// path is `level.time` minus the shot's time (0x811fe90's caller passes
/// `DAT_0864c56c - param_9`), and `FireWeaponAntiLag` (0x8120870) is
/// handed the client's [`lag_time`] under `g_antilag` 1, its default
/// (`Dvar_RegisterBool("g_antilag", 1, ..)`, 0x8111842..), else
/// `level.time`. Every other `G_Damage` caller (melee, blasts, falls,
/// triggers, `finishPlayerDamage`) passes 0. VERIFIED, the call sites.
pub fn time_offset(level_time: i32, lag_time: i32, antilag: bool) -> i32 {
    if antilag { level_time - lag_time } else { 0 }
}

/// The notify CoD2's `ClientEvents` (0x80f52da) raises on the player for a
/// playerstate event, events 168..172, VERIFIED (cod2-gsc-diff.md 4.2).
pub fn binocular_notify(event: i32) -> Option<&'static str> {
    use vcod_cod2::net::events::*;
    Some(match event {
        EV_BINOCULAR_ENTER => "binocular_enter",
        EV_BINOCULAR_EXIT => "binocular_exit",
        EV_BINOCULAR_FIRE => "binocular_fire",
        EV_BINOCULAR_RELEASE => "binocular_release",
        EV_BINOCULAR_DROP => "binocular_drop",
        _ => return None,
    })
}

/// A CoD2 map's server world: the engine's collision through
/// `vcod_cod2::collision::collision_world`, its PVS, and the spawn spots
/// the bots' graph floods from. No hazard, rubble or cover list: those are
/// CoD1 gametypes' and bots'.
pub fn level_world(
    map: &vcod_common::mapworld::MapWorld,
    clip: std::sync::Arc<vcod_cod2::collision::ClipMap>,
) -> crate::world::World {
    let collision = vcod_cod2::collision::collision_world(map, clip, &[]);
    collision.set_arith(vcod_common::patch::Arith::Stored);
    let spawn = vcod_common::bsp::find_spawn(&map.entities).unwrap_or(([0.0, 0.0, 64.0], 0.0));
    crate::world::World {
        collision,
        vis: map.visibility(),
        spawn,
        spawn_points: crate::nav::spawn_points(&map.entities),
        hazards: Vec::new(),
        rubble: Vec::new(),
        nav_cover: Vec::new(),
    }
}

/// `codescripts/struct::InitStructs`, then one `CreateStruct` per
/// `script_struct` block in lump order with the block's keys written onto
/// the struct it returns (cod2-gsc-diff.md 2.4). A key keeps its Radiant
/// type, any other is a string (INFERRED: the copy loop at 0x811a828 is
/// read for its calls, not its typing).
pub fn create_structs(vm: &mut vcod_gsc::Vm, host: &mut GameHost) {
    let blocks = std::mem::take(host.script_structs_mut());
    if blocks.is_empty() && !host.profile.init_structs_always {
        return;
    }
    let init = vm.func_ref("codescripts/struct", "InitStructs");
    let create = vm.func_ref("codescripts/struct", "CreateStruct");
    if !vm.has_function(init) || !vm.has_function(create) {
        log::warn!("gsc: codescripts/struct is not loaded; script_struct dropped");
        return;
    }
    if let Err(e) = vm.call_now(host, init, None, vec![]) {
        log::warn!("gsc: InitStructs: {e:?}");
        return;
    }
    for block in blocks {
        let s = match vm.call_now(host, create, None, vec![]) {
            Ok(vcod_gsc::Value::Struct(s)) => s,
            other => {
                log::warn!("gsc: CreateStruct returned {other:?}");
                return;
            }
        };
        vm.with_cx(|cx| {
            for (k, v) in &block {
                let folded = k.to_ascii_lowercase();
                let ty = host
                    .radiant_key(&folded)
                    .unwrap_or(crate::game::fields::FieldType::CString);
                if let Some(value) = crate::game::spawn::convert(cx, ty, v) {
                    let atom = cx.intern_folded(&folded);
                    cx.set_field(s, atom, value);
                }
            }
        });
    }
}

/// One client's CoD2 movement state: the playerstate pmove steps, the
/// weapon half beside it, and the cmd before the next.
pub struct Cod2Player {
    pub ps: PlayerState,
    pub oldcmd: UserCmd2,
    pub ws: vcod_cod2::pmove::weapon::WeaponState,
    /// `gclient+0x27d8`: the pitch and yaw the last cmd's shots leave
    /// along, the view before its move (`ClientThink_real`).
    pub fire_angles: [f32; 2],
    /// `gclient+0x2840..+0x289c`: the aim block's state across cmds.
    pub aim: vcod_cod2::pmove::aim::AimState,
    /// `gclient+0x2810`: `aimSpreadScale / 255` as the last `ClientEndFrame`
    /// left it, which the shot's spread reads.
    pub spread_scale: f32,
    /// `stats[1]`, the yaw `player_die` faces the dead toward.
    pub dead_yaw: i32,
    pub died_at: i32,
    /// `P_DamageFeedback`'s input, gathered by each `finishPlayerDamage`
    /// since the last end frame: `gclient+0x27e0` the damage, `+0x27e4`
    /// the direction it came along (or the body's origin), `+0x27f0` set
    /// when there was none.
    pub damage_blood: i32,
    pub damage_from: Vec3,
    pub damage_from_world: bool,
    /// `damageEvent`, `damageYaw`, `damagePitch`, `damageCount` (ps+0x11c
    /// .. ps+0x128) as `P_DamageFeedback` left them.
    pub damage_event: i32,
    pub damage_yaw: i32,
    pub damage_pitch: i32,
    pub damage_count: i32,
    /// The last other client whose `finishPlayerDamage` hurt this one,
    /// and the level time it did: vcod's own, for the bots
    /// (`bots::cod2`), which read the hit's direction off the feedback.
    pub last_attacker: Option<(usize, i32)>,
    /// `gclient+0x27a8`, `spectatorClient`: the client a spectator follows
    /// or cycled to, -1 for none.
    pub spectator_client: i32,
    /// The follow the last end frame landed (`pm_flags` 0x400000).
    pub follow: Option<crate::game::cod2_cmds::FollowView>,
    /// The archived record that follow copies, its times shifted
    /// (`SV_GetArchivedClientInfo`); `None` for a live copy.
    pub follow_copy: Option<Box<vcod_cod2::net::PlayerState2>>,
    /// The use key's and the turret's per-client state.
    pub gunner: crate::game::cod2_turret::Gunner,
    /// The body entity's own event ring, fed from the playerstate's.
    pub body_events: BodyEvents,
    /// `gclient+0x27bc`, the last cmd's buttons, and `+0x27c8`, the
    /// presses latched since the last `ClientEndFrame`: every arm of
    /// `ClientThink_real` writes both, the spectator's and the
    /// intermission's too, and `useButtonPressed` reads their OR
    /// (cod2-killcam.md 1.1).
    pub buttons: i32,
    pub latched_buttons: i32,
    /// `r.contents` (ent+0x11c): `G_SetClientContents`' 0x2000000 for a
    /// playing client, 0 otherwise, 0x4000000 for the frame after a
    /// `StuckInClient` push. Other players' moves clip a 0x2000000 body.
    pub contents: u32,
}

/// Events a player's body never carries: `ClientEndFrame`'s copy of the
/// playerstate ring into the entity (0x80f56f8, and
/// `BG_PlayerStateToEntityState` 0x80dfcc2) skips the -1 terminated list
/// at rodata 0x815486c: `EV_STANCE_FORCE_STAND`, `_CROUCH`, `_PRONE`,
/// `EV_STEP_VIEW`, `EV_BULLET_HIT_CLIENT_SMALL`, `_LARGE`
/// (cod2-protocol.md 16).
pub const PS_ONLY_EVENTS: [i32; 6] = [140, 141, 142, 143, 185, 186];

/// A player entity's `eventSequence`, `events` and `eventParms`, and the
/// playerstate word (ps+0xc8) that marks how far its ring was copied.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct BodyEvents {
    pub sequence: i32,
    pub events: [i32; 4],
    pub parms: [i32; 4],
    pub copied: i32,
}

impl BodyEvents {
    /// `ClientEndFrame`'s copy (0x80f56f8): a mark ahead of the ring (a
    /// spawn's memset) comes back to it, then every event past the mark
    /// not in [`PS_ONLY_EVENTS`] goes onto the entity's ring.
    pub fn copy_from(&mut self, ps: &PlayerState) {
        let seq = ps.event_sequence;
        if self.copied != seq && self.copied.wrapping_sub(seq) >= 0 {
            self.copied = seq;
        }
        while self.copied != seq {
            let k = (self.copied & 3) as usize;
            let event = ps.events[k] & 0xff;
            if !PS_ONLY_EVENTS.contains(&event) {
                let slot = (self.sequence & 3) as usize;
                self.events[slot] = event;
                self.parms[slot] = ps.event_parms[k] & 0xff;
                self.sequence = self.sequence.wrapping_add(1);
            }
            self.copied = self.copied.wrapping_add(1);
        }
    }
}

impl Cod2Player {
    fn new(ps: PlayerState, oldcmd: UserCmd2) -> Cod2Player {
        Cod2Player {
            ps,
            oldcmd,
            ws: Default::default(),
            fire_angles: [0.0; 2],
            aim: Default::default(),
            spread_scale: 0.0,
            dead_yaw: 0,
            died_at: 0,
            damage_blood: 0,
            damage_from: Vec3::ZERO,
            damage_from_world: false,
            damage_event: 0,
            damage_yaw: 0,
            damage_pitch: 0,
            damage_count: 0,
            last_attacker: None,
            spectator_client: -1,
            follow: None,
            follow_copy: None,
            gunner: Default::default(),
            body_events: BodyEvents::default(),
            buttons: 0,
            latched_buttons: 0,
            contents: 0,
        }
    }

    /// `ClientSpawn`'s memset of the gclient (Mac 0x1930a6) as it reaches
    /// the aim block's state (+0x2840..) and the damage feedback: the playerstate's four damage fields and the
    /// damage gathered since the last end frame start at 0, so a blow
    /// taken while dead never reaches the new life (cod2-combat.md 9.2).
    fn clear_damage(&mut self) {
        self.aim = Default::default();
        self.damage_blood = 0;
        self.damage_from = Vec3::ZERO;
        self.damage_from_world = false;
        self.damage_event = 0;
        self.damage_yaw = 0;
        self.damage_pitch = 0;
        self.damage_count = 0;
        self.last_attacker = None;
    }

    /// The unit direction the last blow came along, from the attacker
    /// toward the body, or `None` for one with none. Read from what
    /// `P_DamageFeedback` packs, not from `damageYaw`/`damagePitch`: a real
    /// direction can pack as their 255 none (cod2-combat.md 9.2).
    pub fn damage_dir(&self) -> Option<Vec3> {
        (!self.damage_from_world && self.damage_from != Vec3::ZERO).then_some(self.damage_from)
    }
}

/// What a CoD2 level starts with past its map: a gametype script off disk
/// (`--gametype-script`, overlaid at `maps/mp/gametypes/<stem>`), the
/// last level's carry, and on a `fast_restart` the configstring table the
/// restart keeps (cod2-map-cycle.md 1).
#[derive(Default)]
pub struct Cod2Init {
    pub overlay: Option<(String, String)>,
    pub carry: crate::game::script::Carry,
    pub configstrings: Option<Vec<String>>,
}

/// A CoD2 level's game module (see the module header).
pub struct Cod2Level {
    pub rt: ScriptRuntime,
    pub world: Rc<crate::world::World>,
    pub players: Vec<Option<Cod2Player>>,
    pub rules: Rules,
    pub combat: crate::game::cod2_combat::Combat,
    pub turrets: crate::game::cod2_turret::Turrets,
    /// The anim script pmove picks the legs and torso anims off; `None`
    /// without paks, which leaves them 0.
    pub anims: Option<Rc<vcod_cod2::pmove::AnimTable>>,
    /// The frames a killcam replays, kept while `setarchive` is on.
    pub archive: crate::game::cod2_archive::Cod2Archive,
    /// Each mantle's block entity, the level time its think frees it
    /// (lnxded 0x80f5c34) and the box other players' moves clip.
    mantle_blocks: Vec<(vcod_gsc::EntId, i32, vcod_common::movetrace::ClipBox)>,
    /// `addTestClient`'s slots given a `Cod2Player`, each with its bot
    /// port and name, for the engine to give a client.
    test_clients: Vec<(usize, u16, String)>,
    level_time: i32,
    /// `level.frametime`: the last frame's time minus the one before.
    frametime_ms: i32,
}

impl Cod2Level {
    /// `G_InitGame` for `map` under `gametype` off the mounted paks: the
    /// world, the collision, and the script bootstrap.
    pub fn load(
        fs: Rc<Pk3Fs>,
        map: &str,
        gametype: &str,
        cvars: crate::cvars::Cvars,
        now_ms: i32,
    ) -> anyhow::Result<Cod2Level> {
        let loaded = vcod_cod2::map::LoadedMap::find(&*fs, map)?;
        Self::load_loaded(
            fs,
            &loaded,
            map,
            gametype,
            cvars,
            now_ms,
            Cod2Init::default(),
        )
    }

    /// [`Self::load`] with `overlay`, a script (canonical path, text) read
    /// in place of the paks' copy: a gametype off disk.
    pub fn load_with_script(
        fs: Rc<Pk3Fs>,
        map: &str,
        gametype: &str,
        overlay: (String, String),
        cvars: crate::cvars::Cvars,
        now_ms: i32,
    ) -> anyhow::Result<Cod2Level> {
        let loaded = vcod_cod2::map::LoadedMap::find(&*fs, map)?;
        let init = Cod2Init {
            overlay: Some(overlay),
            ..Default::default()
        };
        Self::load_loaded(fs, &loaded, map, gametype, cvars, now_ms, init)
    }

    /// [`Self::load`] over a map the caller already parsed, with what a
    /// level boundary hands on ([`Cod2Init`]).
    #[allow(clippy::too_many_arguments)]
    pub fn load_map_with(
        fs: Rc<Pk3Fs>,
        file: &vcod_cod2::map::MapFile,
        map_world: &vcod_common::mapworld::MapWorld,
        map: &str,
        gametype: &str,
        cvars: crate::cvars::Cvars,
        now_ms: i32,
        init: Cod2Init,
    ) -> anyhow::Result<Cod2Level> {
        let clip = std::sync::Arc::new(file.clip()?);
        Self::load_level(fs, map_world, clip, map, gametype, cvars, now_ms, init)
    }

    /// [`Self::load_map_with`] over a [`vcod_cod2::map::LoadedMap`], whose
    /// world and ClipMap were built side by side.
    #[allow(clippy::too_many_arguments)]
    pub fn load_loaded(
        fs: Rc<Pk3Fs>,
        loaded: &vcod_cod2::map::LoadedMap,
        map: &str,
        gametype: &str,
        cvars: crate::cvars::Cvars,
        now_ms: i32,
        init: Cod2Init,
    ) -> anyhow::Result<Cod2Level> {
        Self::load_level(
            fs,
            &loaded.world,
            loaded.clip.clone(),
            map,
            gametype,
            cvars,
            now_ms,
            init,
        )
    }

    /// The level over the map's world and its ClipMap, which the engine's
    /// collision and the combat's traces share.
    #[allow(clippy::too_many_arguments)]
    fn load_level(
        fs: Rc<Pk3Fs>,
        map_world: &vcod_common::mapworld::MapWorld,
        clip: std::sync::Arc<vcod_cod2::collision::ClipMap>,
        map: &str,
        gametype: &str,
        mut cvars: crate::cvars::Cvars,
        now_ms: i32,
        init: Cod2Init,
    ) -> anyhow::Result<Cod2Level> {
        // The v20 xmodels (a turret's bones, the hit rigs) load through
        // vcod-common's format registry.
        vcod_cod2::model::register();
        let mut laps = vcod_common::laps::Laps::new(format!("cod2 level {map} parts"));
        // The static models' collision builds beside the world and the
        // script, which don't need it.
        let pak: &Pk3Fs = &fs;
        let (rt, statics, world, rules) = std::thread::scope(|sc| {
            // The static models' collision for the bullets (`CM_LoadStaticModels`
            // off the `misc_model` blocks); nothing else clips it.
            let statics = sc.spawn(|| {
                let tris = vcod_common::props::collision_tris(pak, &map_world.entities);
                (!tris.is_empty()).then(|| {
                    vcod_common::collision::CollisionWorld::build(
                        &vcod_common::mapworld::MapWorld::default(),
                        &tris,
                    )
                })
            });
            let world = Rc::new(level_world(map_world, clip.clone()));
            laps.lap("collision world");
            let rules = Rules {
                mantle_anims: vcod_cod2::pmove::mantle::anims(&fs),
                ..Rules::default()
            };
            cvars.set("g_gametype", gametype);
            cvars.set("mapname", map);
            let configstrings = init
                .configstrings
                .unwrap_or_else(|| vec![String::new(); vcod_cod2::net::ops::MAX_CONFIGSTRINGS]);
            let rt = ScriptRuntime::load_cod2_carry(
                Box::new(PakScripts::new(fs.clone(), init.overlay)),
                fs.clone(),
                map_world,
                Some(world.clone()),
                map,
                gametype,
                configstrings,
                cvars,
                now_ms,
                init.carry,
            )?;
            laps.lap("script");
            let statics = vcod_common::laps::joined(statics);
            laps.lap("static models");
            anyhow::Ok((rt, statics, world, rules))
        })?;
        let mut combat = crate::game::cod2_combat::Combat::with_statics(
            Some(clip),
            statics,
            rt.host.fs.as_deref(),
        );
        combat.rand = rt.host.cod2().rand.clone();
        let anims = rt.host.fs.as_deref().and_then(|fs| {
            vcod_cod2::pmove::AnimTable::load(fs)
                .map_err(|e| log::warn!("cod2: no player anim script: {e:#}"))
                .ok()
                .map(Rc::new)
        });
        laps.lap("combat and anims");
        laps.finish();
        Ok(Cod2Level {
            rt,
            world,
            players: (0..crate::server::MAX_CLIENTS).map(|_| None).collect(),
            rules,
            combat,
            turrets: Default::default(),
            anims,
            archive: Default::default(),
            mantle_blocks: Vec::new(),
            test_clients: Vec::new(),
            level_time: now_ms,
            frametime_ms: 50,
        })
    }

    /// `ClientConnect`: the entity and `CodeCallback_PlayerConnect` on the
    /// next frame's packet pass, and a playerstate floating at the map's
    /// spawn as a spectator until the script spawns it.
    pub fn connect(&mut self, slot: usize, name: &str) {
        let (origin, yaw) = self.world.spawn;
        self.connect_at(slot, name, Vec3::from(origin), [0.0, yaw, 0.0]);
    }

    /// [`Self::connect`] with the spectator placed at `origin`, looking
    /// along `angles` until the script spawns it.
    pub fn connect_at(&mut self, slot: usize, name: &str, origin: Vec3, angles: [f32; 3]) {
        self.rt.push_client_event(ClientEvent::Connect {
            slot,
            name: name.to_string(),
        });
        let mut ps = PlayerState::spawn(origin, angles[1], self.level_time);
        ps.view_angles = angles;
        ps.delta_angles = angles.map(angle_to_short);
        ps.client_num = slot as u32;
        set_spectator(&mut ps);
        self.players[slot] = Some(Cod2Player::new(
            ps,
            UserCmd2 {
                server_time: self.level_time,
                ..Default::default()
            },
        ));
        self.rt.host.cod2_mut().clients[slot] = Cod2ClientState::default();
        // `pers.connected` CON_CONNECTING (gclient+0x26c4 = 1) until
        // `ClientBegin`; the scoreboard and the chat recipients read it.
        self.rt.host.client_roster[slot] = Some(RosterSlot {
            connecting: true,
            ..Default::default()
        });
        self.combat.reset_body(slot);
    }

    /// The slots `addTestClient` connected (the script side ran inside
    /// the call), each given its spectator `Cod2Player` here, before the
    /// frame's spawns read it.
    fn install_test_clients(&mut self) {
        let added = std::mem::take(&mut self.rt.host.test_clients.added);
        for (slot, port, name) in added {
            let mut ps = PlayerState::spawn(Vec3::ZERO, 0.0, self.level_time);
            ps.client_num = slot as u32;
            set_spectator(&mut ps);
            self.players[slot] = Some(Cod2Player::new(
                ps,
                UserCmd2 {
                    server_time: self.level_time,
                    ..Default::default()
                },
            ));
            self.rt.host.client_names[slot] = name.clone();
            self.combat.reset_body(slot);
            self.test_clients.push((slot, port, name));
        }
    }

    /// The test clients connected since the last call: slot, bot port and
    /// name, for the engine's client list.
    pub fn take_test_clients(&mut self) -> Vec<(usize, u16, String)> {
        self.install_test_clients();
        std::mem::take(&mut self.test_clients)
    }

    /// A test client's cmd for the bots' pass (lnxded 0x809676c): none
    /// while it watches a killcam (`G_GetClientArchiveTime`), else from
    /// `G_random` draws in this order: attack (0x1) and use (0x28) each
    /// at 0.5, forward and right each 127 at 0.33 or else -127 at 0.5,
    /// then each view angle moved `360 * G_crandom()` at 0.33. VERIFIED,
    /// the calls and the constants at 0x814b8f8. The cmd's time is 0,
    /// which `ClientThink_real` lifts to a second behind `level.time`
    /// (the store at 0x80f5614 VERIFIED; the cmd's clamp INFERRED, Q3's).
    pub fn test_client_cmd(&mut self, slot: usize) -> Option<UserCmd2> {
        let weapon = self.players.get(slot)?.as_ref()?.ws.weapon;
        if self.rt.client_archive_ms(slot) != 0 {
            return None;
        }
        let r = |l: &Self| l.combat.g_random();
        let mut cmd = UserCmd2 {
            server_time: self.level_time - 1000,
            weapon,
            ..Default::default()
        };
        if r(self) < 0.5 {
            cmd.buttons |= 1;
        }
        if r(self) < 0.5 {
            cmd.buttons |= 0x28;
        }
        let axis = || {
            if r(self) < f64::from(0.33f32) {
                127
            } else if r(self) < 0.5 {
                -127
            } else {
                0
            }
        };
        cmd.forward_move = axis();
        cmd.right_move = axis();
        for a in &mut cmd.angles {
            if r(self) < f64::from(0.33f32) {
                *a = (360.0 * (2.0 * r(self) - 1.0)) as i32;
            }
        }
        Some(cmd)
    }

    /// `ClientBegin`: `pers.connected` CON_CONNECTED (2).
    pub fn begin(&mut self, slot: usize) {
        if let Some(r) = self.rt.host.client_roster[slot].as_mut() {
            r.connecting = false;
            r.active = true;
        }
        self.rt.push_client_event(ClientEvent::Begin(slot));
    }

    /// Marks `slot` as one of vcod's own bots (`--bots`, `addbot`), which
    /// `numVotingClients` leaves out (`game::vote`); retail has no such
    /// clients, and its `addTestClient` ones still count.
    pub fn mark_bot(&mut self, slot: usize) {
        if let Some(r) = self.rt.host.client_roster[slot].as_mut() {
            r.bot = true;
        }
    }

    /// `cl->ping` as `SV_CalcPings` left it, which the scoreboard sends.
    pub fn set_ping(&mut self, slot: usize, ping: i32) {
        if let Some(r) = self.rt.host.client_roster[slot].as_mut() {
            r.ping = ping;
        }
    }

    /// `Cmd_Kill_f`: nothing unless `sessionstate` is playing, then the
    /// god bits off and `player_die(self, self, self, 100000,
    /// MOD_SUICIDE)`, the death the `suicide` builtin gives.
    pub fn kill(&mut self, slot: usize) {
        if self.playing(slot) {
            // God and demigod go first, so the suicide lands.
            self.rt.host.cod2_mut().clients[slot].flags &= !3;
            // `player_die` draws the death anim, whose duration the killed
            // callback hands `cloneplayer` (cod2-combat.md 5).
            let duration = crate::game::builtins::cod2::draw_death_anim(&mut self.rt.host, slot);
            self.rt.host.cod2_mut().callback = crate::game::host::Cod2CallbackArgs {
                time_offset: 0,
                death_anim_duration: duration,
            };
            self.rt.kill_client(slot);
            self.rt.host.cod2_mut().callback = Default::default();
        }
    }

    /// The buttons `useButtonPressed` (0x28) and `attackButtonPressed`
    /// (0x1) read, the last cmd's OR the latched presses (Mac 0x1bff00,
    /// 0x1bff98), onto the host in CoD1's bits.
    fn mirror_buttons(&mut self, slot: usize) {
        let Some(p) = self.players.get(slot).and_then(Option::as_ref) else {
            return;
        };
        let b = p.buttons | p.latched_buttons;
        let mut buttons = 0u8;
        if b & 0x28 != 0 {
            buttons |= vcod_common::net::msg::BUTTON_USE;
        }
        if b & 1 != 0 {
            buttons |= vcod_common::net::msg::BUTTON_ATTACK;
        }
        self.rt.set_client_buttons(slot, buttons);
    }

    /// `Cmd_Score_f`: the `b` scoreboard for the asking client
    /// (`DeathmatchScoreboardMessage`, cod2-protocol.md 3.4).
    pub fn scoreboard(&mut self) -> String {
        self.rt.scoreboard()
    }

    /// `ClientDisconnect`: the `menuresponse` ("-1", "disconnect") notify,
    /// the disconnect callback and the entity's free.
    pub fn disconnect(&mut self, slot: usize) {
        // Inside the drop, as `SV_DropClient`'s `ClientDisconnect` runs it:
        // the script's lines go out ahead of the drop's `J`
        // (cod2-protocol.md 15.5).
        self.rt.client_disconnect(slot);
        self.rt.host.client_roster[slot] = None;
        self.rt.host.cod2_mut().clients[slot].body = None;
        self.players[slot] = None;
    }

    /// The `mr` client command.
    pub fn menu_response(&mut self, slot: usize, menu: &MenuRef, response: &str) {
        self.rt.menu_response(slot, menu, response);
    }

    /// `SV_ClientThink` then `ClientThink_real` for one cmd: the lag time,
    /// the sight flag and pmove over the level's collision, `PM_Weapon`
    /// beside it, what the script reads of the move written back onto the
    /// host, then `ClientEvents`: the shots and throws the cmd fired and
    /// the notifies. A slot with no client is ignored.
    pub fn client_think(&mut self, slot: usize, cmd: &UserCmd2) {
        crate::tick_trace::step!("ClientThink slot {slot} cmd {}", cmd.server_time);
        let level_time = self.level_time;
        // Before any arm, as each of `ClientThink_real`'s stores them (Mac
        // 0x1a3d00): a killcam's follower skips it on use.
        if let Some(p) = self.players.get_mut(slot).and_then(Option::as_mut) {
            p.latched_buttons |= cmd.buttons & !p.buttons;
            p.buttons = cmd.buttons;
        }
        self.mirror_buttons(slot);
        // `SpectatorThink` (Mac 0x1a2a08): the follow buttons, and no move
        // while the view rides another client.
        let spectator = self
            .players
            .get(slot)
            .and_then(Option::as_ref)
            .map(|p| (p.ps.pm_type == pm_type::SPECTATOR, p.oldcmd.buttons));
        if let Some((true, old)) = spectator
            && !self.spectator_buttons(slot, cmd.buttons, old)
        {
            if let Some(p) = self.players[slot].as_mut() {
                p.oldcmd = *cmd;
                p.ps.command_time = cmd.server_time;
            }
            return;
        }
        self.combat.sync_weapons(&self.rt.host);
        let blocks: Vec<_> = self.mantle_blocks.iter().map(|b| b.2).collect();
        let bodies = self.move_bodies();
        let Some(p) = self.players.get_mut(slot).and_then(Option::as_mut) else {
            return;
        };
        let state = &mut self.rt.host.cod2_mut().clients[slot];
        state.lag_time = lag_time(state.lag_time, cmd.server_time, level_time);
        let seq = p.ps.event_sequence;
        sync_weapons_in(&self.rt.host, slot, &mut p.ws);
        let defs = &self.combat.weapons;
        let msec = (cmd.server_time - p.ps.command_time).clamp(0, 200);
        let oldcmd = p.oldcmd;
        // The aim block runs before `Pmove`, on the cmd's ms (lnxded
        // 0x80f5d2e; cod2-combat.md 9.6).
        p.fire_angles = p.aim.fire_angles(&vcod_cod2::pmove::aim::AimInput {
            ps: &p.ps,
            ws: &p.ws,
            defs,
            level_time,
            msec,
        });
        let weapon_cmd = vcod_cod2::pmove::weapon::WeaponCmd {
            cmd,
            oldcmd: &oldcmd,
            msec,
            ads_exit_delay: 0,
            prone_change: false,
            anim: None,
        };
        // Only `PmoveSingle`'s default arm runs the weapon: a living or
        // dead player, never a spectator, a flyer or the intermission.
        let armed = matches!(
            p.ps.pm_type,
            pm_type::NORMAL | pm_type::NORMAL_LINKED | pm_type::DEAD | pm_type::DEAD_LINKED
        );
        let msec = if armed { msec } else { 0 };
        vcod_cod2::pmove::weapon::update_firing_flag(&mut p.ps, &p.ws, cmd, defs);
        if msec > 0 {
            use vcod_cod2::pmove::weapon as w;
            w::update_sight_flag(&mut p.ps, &p.ws, cmd, &oldcmd, defs);
            w::adjust_aim_spread(&mut p.ps, &p.ws, &weapon_cmd, defs);
        }
        // `pm.tracemask` meets the mantle blocks' 0x10000; a block lets its
        // own climber through (cod2-movement.md 9).
        let world = vcod_common::movetrace::MoveWorld {
            pass: slot as u32,
            bodies: &bodies,
            ..vcod_common::movetrace::MoveWorld::bare(&self.world.collision)
        }
        .with_clip_boxes(&blocks);
        // The events' clause picks draw off the game's `rand()`, inside
        // `Pmove` (cod2-combat.md 9.6).
        let rand = &self.combat.rand;
        let pick = |n: usize| rand.rand() as usize % n;
        // `BG_AnimUpdatePlayerStateConditions` reads the viewmodel weapon
        // (Mac 0x7b226) before each step.
        let anim = self
            .anims
            .as_deref()
            .map(|table| vcod_cod2::pmove::AnimInput {
                pick: Some(&pick),
                ..vcod_cod2::pmove::weapon::anim_input(table, &p.ps, &p.ws, defs)
            });
        let prone = p.ps.view_height_target == vcod_cod2::pmove::VIEW_PRONE;
        let move_scale = vcod_cod2::pmove::weapon::move_scale(defs, p.ws.weapon);
        let mantling = p.ps.pm_flags & vcod_cod2::pmove::pmf::MANTLE != 0;
        vcod_cod2::pmove::player_step_anim(
            &mut p.ps,
            cmd,
            &mut p.oldcmd,
            world,
            self.rules,
            move_scale,
            anim,
        );
        let started = !mantling && p.ps.pm_flags & vcod_cod2::pmove::pmf::MANTLE != 0;
        let mantle = started
            .then_some(self.rules.mantle_anims)
            .flatten()
            .map(|a| vcod_cod2::pmove::mantle::end_and_duration(a, &p.ps));
        if msec > 0 {
            // A torso event's pick among a clause's commands:
            // `BG_AnimScriptEvent` draws `rand() % n` for every clause with a
            // command, one command or more (lnxded 0x80d96ee).
            let weapon_cmd = vcod_cod2::pmove::weapon::WeaponCmd {
                prone_change: prone != (p.ps.view_height_target == vcod_cod2::pmove::VIEW_PRONE),
                anim: self
                    .anims
                    .as_deref()
                    .map(|table| vcod_cod2::pmove::weapon::WeaponAnims { table, pick: &pick }),
                ..weapon_cmd
            };
            vcod_cod2::pmove::weapon::pm_weapon(&mut p.ps, &mut p.ws, &weapon_cmd, defs);
            p.ps.weapon_state = p.ws.state;
        }
        sync_weapons_out(&mut self.rt.host, slot, &p.ws);
        let fired = (seq..p.ps.event_sequence).map(|i| p.ps.events[(i & 3) as usize]);
        self.combat.fire_recoil(&mut p.aim, &p.ps, &p.ws, fired);
        if let Some((end, duration)) = mantle {
            let (mins, maxs) = (p.ps.mins, p.ps.maxs);
            self.spawn_mantle_block(slot, end, (mins, maxs), duration, level_time);
        }
        let Some(p) = self.players.get_mut(slot).and_then(Option::as_mut) else {
            return;
        };
        let ps = &p.ps;
        let (pm, on_ground, playing) =
            (ps.pm_type, ps.on_ground(), ps.pm_type < pm_type::SPECTATOR);
        let state = &mut self.rt.host.cod2_mut().clients[slot];
        state.view_angles = ps.view_angles;
        state.body = playing.then(|| crate::game::cod2_blast::BlastBody::of(ps));
        let origin = ps.origin.to_array();
        let eye = [origin[0], origin[1], origin[2] + ps.view_height_current];
        let events: Vec<(i32, i32)> = (seq..ps.event_sequence)
            .map(|i| {
                (
                    ps.events[(i & 3) as usize],
                    ps.event_parms[(i & 3) as usize],
                )
            })
            .collect();
        let aim = [ps.view_angles[0], ps.view_angles[1]];
        self.rt.set_client_pm_type(slot, pm);
        self.rt.set_client_on_ground(slot, on_ground);
        self.rt.set_client_aim(slot, eye, aim, false);
        self.rt.set_client_origin(slot, origin);
        // `ClientThink_real` (lnxded 0x80f5d2e) after `Pmove`: `r.currentAngles`
        // is (0, the view's yaw, 0), what a corpse and a drop take
        // (cod2-protocol.md 15.7).
        self.rt.set_client_angles(slot, [0.0, aim[1], 0.0]);
        self.set_death_anims(slot);
        let body = self.body_entity(slot);
        self.rt.set_client_entity_state(slot, body);
        for &(e, _) in &events {
            if let Some(n) = binocular_notify(e) {
                self.rt.notify_client(slot, n, &[]);
            }
        }
        self.client_events(slot, &events);
        if playing {
            self.rt.touch_triggers(slot, level_time);
        }
        self.update_activate(slot, cmd, &oldcmd);
    }

    /// The `death` event's candidates for `slot` as its last step left it
    /// (`BG_AnimScriptEvent(ps, ANIM_ET_DEATH, 0, 1)` run dry over the
    /// playerstate's conditions), each as wire index and duration:
    /// `finishPlayerDamage` draws one before the playerstate takes it.
    fn set_death_anims(&mut self, slot: usize) {
        let mut anims = Vec::new();
        if let (Some(table), Some(p)) = (self.anims.as_deref(), self.players[slot].as_ref()) {
            let held = self
                .combat
                .weapons
                .get(usize::from(p.ws.weapon))
                .and_then(Option::as_ref);
            let input = vcod_cod2::pmove::AnimInput {
                table,
                player_anim_type: held.map_or(0, |w| w.player_anim_type),
                weapon_class: held.map_or(0, |w| w.weap_class),
                pick: None,
            };
            let mut n = 1;
            let mut k = 0;
            while k < n.min(8) {
                let mut ps = p.ps.clone();
                let pick = |count| {
                    n = count;
                    k
                };
                match death_event(&mut ps, &input, pick) {
                    Some(d) => anims.push((ps.legs_anim & !ANIM_TOGGLEBIT, d)),
                    None => break,
                }
                k += 1;
            }
        }
        let state = &mut self.rt.host.cod2_mut().clients[slot];
        state.death_anim_count = anims.len() as u8;
        for (to, from) in state.death_anims.iter_mut().zip(anims) {
            *to = from;
        }
    }

    /// One `G_RunFrame` at `now_ms`: the script threads and the entity
    /// pass, then the spawns the threads asked for land on the
    /// playerstates (`ClientSpawn`), so the next cmd moves the new body.
    pub fn run_frame(&mut self, now_ms: i32) {
        // A cmd's drops pose their dropper under its packet's stamp.
        let mut _p = crate::tick_profile::phase("frame/drops");
        self.place_drops();
        // `SV_RunFrame` resets the skeleton cache (lnxded 0x8096752).
        self.combat.new_stamp();
        self.frametime_ms = now_ms - self.level_time;
        self.level_time = now_ms;
        crate::tick_trace::step!("Scr_RunThreads");
        _p = crate::tick_profile::phase("frame/scripts");
        self.rt.run_frame(now_ms);
        _p = crate::tick_profile::phase("frame/drops");
        self.place_drops();
        _p = crate::tick_profile::phase("frame/test clients");
        self.install_test_clients();
        crate::tick_trace::step!("G_RunFrame entities");
        _p = crate::tick_profile::phase("frame/mantle and weapons");
        self.run_mantle_blocks();
        crate::game::builtins::cod2::write_weapon_configstrings(&mut self.rt.host);
        self.combat.sync_weapons(&self.rt.host);
        _p = crate::tick_profile::phase("frame/missiles");
        self.run_missiles();
        _p = crate::tick_profile::phase("frame/corpses");
        self.run_corpses();
        _p = crate::tick_profile::phase("frame/spawns");
        for s in self.rt.take_client_spawns() {
            crate::tick_trace::step!("ClientSpawn slot {}", s.slot);
            let Some(p) = self.players.get_mut(s.slot).and_then(Option::as_mut) else {
                continue;
            };
            let mut ps = PlayerState::spawn(Vec3::from(s.origin), s.yaw_deg, now_ms);
            ps.client_num = s.slot as u32;
            ps.pm_type = match s.mode {
                SpawnMode::Player => pm_type::NORMAL,
                SpawnMode::Spectator => pm_type::SPECTATOR,
                SpawnMode::Intermission => pm_type::INTERMISSION,
            };
            if ps.pm_type == pm_type::SPECTATOR {
                set_spectator(&mut ps);
            }
            // `MoveClientToIntermission` (0x80f6818): the team scores into
            // configstrings 5 (axis) and 6 (allies), VERIFIED; retail's tdm
            // intermission sent `d 5 0` and `d 6 0` (cod2-map-cycle.md 2).
            if ps.pm_type == pm_type::INTERMISSION {
                let [axis, allies] = self.rt.host.team_scores;
                self.rt.host.configstrings.set(5, axis.to_string());
                self.rt.host.configstrings.set(6, allies.to_string());
            }
            // The cmd angles are the client's; the spawn's yaw goes into
            // `delta_angles` so the view starts on it (`SetClientViewAngle`).
            set_view_angles(&mut ps, &p.oldcmd, [0.0, s.yaw_deg, 0.0]);
            // `ClientSpawn`'s memset takes the event ring with the rest
            // (`eventSequence` reads 0, cod2-killcam.md 5).
            ps.e_flags = spawn_e_flags(p.ps.e_flags);
            // `ClientSpawn` (Mac 0x1930a6) sets `pm_flags` 0x1000,
            // which holds the trigger off until it is let go (VERIFIED,
            // cod2-turrets.md 14's respawn snapshot reads 0x801000).
            ps.pm_flags |= vcod_cod2::pmove::pmf::RESPAWNED;
            p.ps = ps;
            p.body_events.copied = 0;
            p.clear_damage();
            // The spawn's playerstate is the client's own again.
            p.follow = None;
            p.follow_copy = None;
            self.rt.host.cod2_mut().clients[s.slot].spawn_count += 1;
            // A fresh weapon state: the held weapons and rounds are the
            // host's, which the spawn's script gave.
            p.ws = Default::default();
            // `ClientSpawn` ends in `ClientThink_real` on the client's last
            // cmd restamped to `level.time`, `commandTime` 100 ms before it
            // (Mac 0x1930a6, VERIFIED stores and calls): the body's first
            // anims, picked with no weapon up yet.
            if p.ps.pm_type == pm_type::NORMAL {
                let cmd = UserCmd2 {
                    server_time: now_ms,
                    ..p.oldcmd
                };
                let mut old = p.oldcmd;
                p.ps.command_time = now_ms - 100;
                // Its aim block too: 100 ms of idle clock and stance factor
                // on the empty hands, and the spawn's view for the sway.
                p.fire_angles = p.aim.fire_angles(&vcod_cod2::pmove::aim::AimInput {
                    ps: &p.ps,
                    ws: &p.ws,
                    defs: &self.combat.weapons,
                    level_time: now_ms,
                    msec: 100,
                });
                let rand = &self.combat.rand;
                let pick = |n: usize| rand.rand() as usize % n;
                let anim = self
                    .anims
                    .as_deref()
                    .map(|table| vcod_cod2::pmove::AnimInput {
                        table,
                        player_anim_type: 0,
                        weapon_class: 0,
                        pick: Some(&pick),
                    });
                let world = vcod_common::movetrace::MoveWorld::bare(&self.world.collision);
                let move_scale =
                    vcod_cod2::pmove::weapon::move_scale(&self.combat.weapons, p.ws.weapon);
                let oldcmd = old;
                vcod_cod2::pmove::player_step_anim(
                    &mut p.ps, &cmd, &mut old, world, self.rules, move_scale, anim,
                );
                // `PM_Weapon`'s breath step (lnxded 0x80f066e, 0x80ed084) once
                // the step let the trigger go: empty hands track the scale up
                // from 0 over the 100 ms in `Pmove`'s 66 and 34, 0.519216
                // live (cod2-protocol.md 15.7).
                if p.ps.pm_flags & vcod_cod2::pmove::pmf::RESPAWNED == 0 {
                    use vcod_cod2::pmove::weapon;
                    for msec in [66, 34] {
                        let c = weapon::WeaponCmd {
                            cmd: &cmd,
                            oldcmd: &oldcmd,
                            msec,
                            ads_exit_delay: 0,
                            prone_change: false,
                            anim: None,
                        };
                        weapon::update_hold_breath(
                            &mut p.ps,
                            &mut p.ws,
                            &c,
                            &self.combat.weapons,
                            &weapon::Breath::DEFAULT,
                        );
                    }
                }
            }
            let origin = p.ps.origin.to_array();
            self.rt.host.cod2_mut().clients[s.slot].death_anim = 0;
            self.rt.set_client_origin(s.slot, origin);
        }
        _p = crate::tick_profile::phase("frame/script ops");
        self.apply_sim_ops();
        self.apply_link_ops();
        // The ammo arrays the weapon builtins write are the host's own
        // (`GameHost::client_ammo`), which the playerstate is built from.
        self.rt.take_weapon_ops();
        _p = crate::tick_profile::phase("frame/end frames");
        self.end_frame();
        crate::tick_trace::step!("identify_clients");
        _p = crate::tick_profile::phase("frame/identify");
        self.identify_clients();
        // `CheckVote` (Mac 0x1aeb2a) last, past the identify pass
        // (0x811c382), as in 1.1: its line queues with the frame's commands,
        // ahead of the configstring 15 it clears.
        crate::tick_trace::step!("CheckVote");
        _p = crate::tick_profile::phase("frame/vote");
        self.rt.check_vote(now_ms);
    }

    /// `ClientThink_real`'s mantle block (lnxded 0x80f5c34), after a pmove
    /// that started a mantle: an `ET_INVISIBLE` entity owned by the client,
    /// contents 0x10000 with the body's box at the mantle's end, its think
    /// freeing it `mantleDuration + g_mantleBlockTimeBuffer` ms on
    /// (`Dvar_RegisterInt("g_mantleBlockTimeBuffer", 500, 0, 60000)`).
    /// VERIFIED, the stores. Every other player's pmove clips its box
    /// (cod2-movement.md 9).
    fn spawn_mantle_block(
        &mut self,
        slot: usize,
        end: glam::Vec3,
        (mins, maxs): (glam::Vec3, glam::Vec3),
        duration: i32,
        now: i32,
    ) {
        let buffer = self
            .rt
            .host
            .cvars
            .get("g_mantleBlockTimeBuffer")
            .trim()
            .parse::<i32>()
            .unwrap_or(500)
            .clamp(0, 60000);
        let id = self.rt.with_host(|h, cx| h.ents.spawn(cx));
        match id {
            Ok(id) => {
                let block = vcod_common::movetrace::ClipBox {
                    entity: id.0,
                    owner: slot as u32,
                    mins: end + mins,
                    maxs: end + maxs,
                    contents: 0x10000,
                };
                self.mantle_blocks
                    .push((id, duration + now + buffer, block));
            }
            Err(e) => log::warn!("mantle block dropped: {e:?}"),
        }
    }

    /// The mantle blocks whose think is due, freed (`G_FreeEntity`).
    fn run_mantle_blocks(&mut self) {
        let now = self.level_time;
        let (due, kept) = std::mem::take(&mut self.mantle_blocks)
            .into_iter()
            .partition(|&(_, at, _)| at <= now);
        self.mantle_blocks = kept;
        for (id, _, _) in due {
            self.rt.host.free_entity(id);
        }
    }

    /// A player's `linkTo` and `unlink` onto its playerstate: `pm_type` 1
    /// while linked, which pmove holds where it stands (`PmoveSingle`'s
    /// linked arm), and back to 0. The stock scripts link a player only to
    /// a trigger that stands still (the S&D plant and defuse), so the body
    /// is not carried along with its parent.
    fn apply_link_ops(&mut self) {
        use crate::game::host::LinkOp;
        for (slot, op) in self.rt.take_link_ops() {
            let Some(p) = self.players.get_mut(slot).and_then(Option::as_mut) else {
                continue;
            };
            match (op, p.ps.pm_type) {
                (LinkOp::Link { .. }, pm_type::NORMAL) => {
                    p.ps.pm_type = pm_type::NORMAL_LINKED;
                    p.ps.velocity = Vec3::ZERO;
                }
                (LinkOp::Unlink, pm_type::NORMAL_LINKED) => p.ps.pm_type = pm_type::NORMAL,
                _ => {}
            }
        }
    }

    /// A player's `setOrigin` and `setPlayerAngles` onto its playerstate.
    /// The other ops a CoD2 level has no path for yet are dropped.
    fn apply_sim_ops(&mut self) {
        for (slot, op) in self.rt.take_sim_ops() {
            let Some(p) = self.players.get_mut(slot).and_then(Option::as_mut) else {
                continue;
            };
            match op {
                SimOp::SetOrigin { origin } => {
                    p.ps.origin = Vec3::from(origin);
                    p.ps.velocity = Vec3::ZERO;
                    p.ps.e_flags ^= EF_TELEPORT_BIT;
                }
                SimOp::SetViewAngles { angles } => set_view_angles(&mut p.ps, &p.oldcmd, angles),
                SimOp::Damaged { .. } => {
                    self.rt.host.client_sim_ops.push((slot, op));
                    self.apply_damage_ops();
                }
                // A pickup's event on the player (`Touch_Item`, Mac 0x1b1bea:
                // 0x90 with a weapon given, 0x91 with only rounds).
                SimOp::Event { event, parm } => {
                    use vcod_common::net::event_ids as v1;
                    let event = match event {
                        v1::EV_ITEM_PICKUP => vcod_cod2::net::events::EV_ITEM_PICKUP,
                        v1::EV_AMMO_PICKUP => vcod_cod2::net::events::EV_AMMO_PICKUP,
                        _ => continue,
                    };
                    p.ps.add_event(event, parm);
                }
                _ => {}
            }
        }
    }

    /// The other players a cmd's move clips: every client whose
    /// `r.contents` is set, at its origin and its stance's box, as its last
    /// link left it. Retail clips a client as a capsule
    /// (`SV_ClipMoveToEntities`); vcod's movetrace capsule is 1.1's.
    fn move_bodies(&self) -> Vec<vcod_common::movetrace::Body> {
        self.players
            .iter()
            .enumerate()
            .filter_map(|(slot, p)| {
                let p = p.as_ref().filter(|p| p.contents != 0)?;
                let (mins, maxs) = p.ps.stance_box();
                Some(vcod_common::movetrace::Body {
                    entity: slot as u32,
                    origin: p.ps.origin,
                    mins,
                    maxs,
                    contents: p.contents,
                })
            })
            .collect()
    }

    /// `StuckInClient`'s CoD2 form (lnxded 0x80f6e9e) for `slot`: both
    /// players of the first overlapping pair take the push as their x/y
    /// velocity, `pm_time` 300 and `pm_flags` 0x200; `slot` turns to
    /// contents 0x4000000.
    fn stuck_in_client(&mut self, slot: usize, eject: f32) {
        use crate::game::stuck::{Cod2StuckView, stuck_in_client_cod2};
        let views: Vec<Option<Cod2StuckView>> = (0..self.players.len())
            .map(|i| {
                let p = self.players[i].as_ref()?;
                let playing = p.ps.pm_flags & PMF_PLAYING != 0
                    && self.rt.client_field(i, "sessionstate").as_deref() == Some("playing");
                let (mins, maxs) = p.ps.stance_box();
                Some(Cod2StuckView {
                    playing,
                    health: self.rt.client_vitals(i).health,
                    contents: p.contents,
                    origin: p.ps.origin,
                    mins,
                    maxs,
                    vel_xy: p.ps.velocity.truncate(),
                    speed: p.ps.speed as f32,
                })
            })
            .collect();
        let rand = &self.combat.rand;
        let Some(push) = stuck_in_client_cod2(slot, &views, eject, || rand.rand()) else {
            return;
        };
        for (i, v) in [(push.other, push.other_vel), (slot, push.self_vel)] {
            if let Some(p) = self.players[i].as_mut() {
                p.ps.velocity.x = v.x;
                p.ps.velocity.y = v.y;
                p.ps.pm_time = 300;
                p.ps.pm_flags |= vcod_cod2::pmove::pmf::TIME_LAND;
            }
        }
        if let Some(p) = self.players[slot].as_mut() {
            p.contents = vcod_common::movetrace::CONTENTS_CORPSE;
        }
    }

    /// `ClientEndFrame`'s playerstate half (0x80f7516): a playing client
    /// gets `pm_flags` 0x800000 with 0x3000000 cleared, `g_gravity` and
    /// `g_speed`; a spectator keeps the fields a retail spectator is sent
    /// (cod2-protocol.md 9). VERIFIED, the stores.
    fn end_frame(&mut self) {
        // `G_RunFrame`'s objective pass (lnxded 0x8109ce0): each connected
        // client's playerstate copy of the level's 16 objectives, a record
        // off or another team's kept but for its state 0. VERIFIED.
        for slot in 0..self.players.len() {
            if self.players[slot].is_some() {
                let team = self.rt.client_team(slot);
                self.rt.objectives_for(slot, team);
            }
        }
        let host = &self.rt.host;
        let cvar = |name: &str, default: i32| match host.cvars.get(name) {
            "" => default,
            v => crate::game::builtins::cvar::atof(v) as i32,
        };
        let (speed, gravity) = (cvar("g_speed", 190), cvar("g_gravity", 800));
        let eject = cvar("g_playerCollisionEjectSpeed", 25) as f32;
        let now = self.level_time;
        let mut spectators = Vec::new();
        for slot in 0..self.players.len() {
            if self.players[slot].is_none() {
                continue;
            }
            crate::tick_trace::step!("ClientEndFrame slot {slot}");
            // 0x80f72f8 at `ClientEndFrame`'s head: the attachments' tags
            // take their configstrings inside the frame, so the `d` leads
            // the first snapshot whose client state names one
            // (cod2-player-anims.md 1).
            self.rt.client_attachments(slot);
            let session = self.rt.client_field(slot, "sessionstate");
            // `ClientEndFrame` (Mac 0x1a47a8) takes its arm off the
            // session: a playerstate still holding last frame's follow
            // copy (`clientNum` not the client's) whose session left
            // "spectator" is `ClientSpawn`ed at the copy; a session the
            // script made "spectator" runs `SpectatorClientEndFrame`
            // whatever the client was (cod2-killcam.md 3, 5).
            if session.as_deref() != Some("spectator") {
                self.spawn_at_follow_copy(slot, session.as_deref() == Some("dead"));
            }
            let Some(p) = self.players[slot].as_mut() else {
                continue;
            };
            if session.as_deref() == Some("spectator") && p.ps.pm_type != pm_type::SPECTATOR {
                set_spectator(&mut p.ps);
            }
            // `ClientEndFrame` (Mac 0x1a47a8): a client the script made
            // dead gets `pm_type` 6, a playing one 0, linked or not.
            if p.ps.pm_type < pm_type::SPECTATOR || p.ps.pm_type >= pm_type::DEAD {
                let linked = matches!(p.ps.pm_type, pm_type::NORMAL_LINKED | pm_type::DEAD_LINKED);
                // `noclip` and `ufo` win over the session (`pm_type` 2, 3).
                let state = self.rt.host.cod2().clients[slot];
                match session.as_deref() {
                    _ if state.noclip => p.ps.pm_type = pm_type::NOCLIP,
                    _ if state.ufo => p.ps.pm_type = pm_type::UFO,
                    Some("dead") => {
                        p.ps.pm_type = if linked {
                            pm_type::DEAD_LINKED
                        } else {
                            pm_type::DEAD
                        };
                    }
                    Some("playing")
                        if p.ps.pm_type >= pm_type::DEAD
                            || matches!(p.ps.pm_type, pm_type::NOCLIP | pm_type::UFO) =>
                    {
                        p.ps.pm_type = if linked {
                            pm_type::NORMAL_LINKED
                        } else {
                            pm_type::NORMAL
                        };
                    }
                    _ => {}
                }
            }
            // The playing and dead arm: a dead client keeps `pm_flags`
            // 0x800000 too, so the archive and a follow still answer for
            // it (cod2-killcam.md 5).
            if !matches!(p.ps.pm_type, pm_type::SPECTATOR | pm_type::INTERMISSION) {
                p.ps.pm_flags = (p.ps.pm_flags | PMF_PLAYING) & !0x300_0000;
                p.ps.speed = speed;
                p.ps.gravity = gravity;
                p.spread_scale = p.ps.aim_spread_scale / 255.0;
                let max_health = self.rt.client_vitals(slot).max_health;
                damage_feedback(p, max_health, now);
                // `G_SetClientContents` (lnxded 0x80f5682): no body under
                // noclip or ufo or while dead.
                let state = self.rt.host.cod2().clients[slot];
                let solid = !state.noclip && !state.ufo && session.as_deref() != Some("dead");
                p.contents = if solid { CONTENTS_BODY } else { 0 };
            } else {
                p.contents = 0;
            }
            if p.ps.pm_type == pm_type::SPECTATOR {
                set_spectator(&mut p.ps);
                spectators.push(slot);
            }
            self.rt.host.cod2_mut().clients[slot].body = (p.ps.pm_type < pm_type::SPECTATOR)
                .then(|| crate::game::cod2_blast::BlastBody::of(&p.ps));
            if p.ps.pm_type < pm_type::SPECTATOR || p.ps.pm_type >= pm_type::DEAD {
                let _p = crate::tick_profile::phase("end frames/animate");
                self.animate_body(slot, self.frametime_ms);
            }
            // After the entity copy, a client with health that overlaps
            // another is pushed off it and drops to 0x4000000 for the
            // frame (lnxded 0x80f78dc; cod2-turrets.md 14.2).
            if self.rt.client_vitals(slot).health > 0 {
                self.stuck_in_client(slot, eject);
            }
        }
        let p = crate::tick_profile::phase("end frames/spectators and turrets");
        for slot in spectators {
            self.spectator_end_frame(slot);
        }
        self.turret_end_frame();
        drop(p);
        // Every arm of `ClientEndFrame` ends zeroing `gclient+0x27c8`
        // (Mac 0x1a47a8).
        for slot in 0..self.players.len() {
            if let Some(p) = self.players[slot].as_mut() {
                p.latched_buttons = 0;
                self.mirror_buttons(slot);
            }
        }
        // The dead run it too, which clears a flag left from life.
        for slot in 0..self.players.len() {
            let in_world = self.players[slot].as_ref().is_some_and(|p| {
                !matches!(p.ps.pm_type, pm_type::SPECTATOR | pm_type::INTERMISSION)
            });
            if in_world {
                let _p = crate::tick_profile::phase("end frames/look at");
                self.look_at_flags(slot);
            }
        }
        let _p = crate::tick_profile::phase("end frames/entity states");
        for slot in 0..self.players.len() {
            if let Some(p) = self.players[slot].as_mut() {
                p.body_events.copy_from(&p.ps);
                let body = self.body_entity(slot);
                self.rt.set_client_entity_state(slot, body);
            }
        }
    }

    /// `ClientEndFrame`'s last arm (Mac 0x1a47a8): a client whose
    /// playerstate holds the follow copy the last end frame left, out of
    /// "spectator" now, is `ClientSpawn`ed at the copy's origin, angles
    /// `(0, yaw, 0)`. The spawn's memset takes the weapons, the rounds and
    /// the event ring; `pm_flags` reads 0x801000 and `eFlags` the copy's
    /// teleport bit flipped (cod2-killcam.md 5). Nothing for a client
    /// that followed no one.
    fn spawn_at_follow_copy(&mut self, slot: usize, dead: bool) {
        let Some(p) = self.players[slot].as_mut() else {
            return;
        };
        let Some(f) = p.follow.take() else {
            return;
        };
        let (origin, yaw, e_flags) = match p.follow_copy.take() {
            Some(w) => (
                Vec3::from(w.origin()),
                w.f32("viewangles[1]"),
                w.i32("eFlags") as u32,
            ),
            None => match self.players.get(f.target).and_then(Option::as_ref) {
                Some(t) => (t.ps.origin, t.ps.view_angles[1], t.ps.e_flags),
                None => return,
            },
        };
        let now = self.level_time;
        let p = self.players[slot].as_mut().unwrap();
        p.spectator_client = -1;
        let mut ps = PlayerState::spawn(origin, yaw, now);
        ps.client_num = slot as u32;
        ps.pm_type = if dead { pm_type::DEAD } else { pm_type::NORMAL };
        ps.pm_flags = PMF_PLAYING | vcod_cod2::net::flags::PMF_SPAWN_FIRE_LATCH as u32;
        set_view_angles(&mut ps, &p.oldcmd, [0.0, yaw, 0.0]);
        ps.e_flags = spawn_e_flags(e_flags);
        p.ps = ps;
        p.body_events.copied = 0;
        p.ws = Default::default();
        p.dead_yaw = 0;
        // The spawn's own `ClientThink_real`, 100 ms on the last cmd: a
        // dead one's eye starts down toward 8 (42 on retail's frame).
        let cmd = UserCmd2 {
            server_time: now,
            ..p.oldcmd
        };
        let mut old = p.oldcmd;
        p.ps.command_time = now - 100;
        let rand = &self.combat.rand;
        let pick = |n: usize| rand.rand() as usize % n;
        let anim = self
            .anims
            .as_deref()
            .map(|table| vcod_cod2::pmove::AnimInput {
                table,
                player_anim_type: 0,
                weapon_class: 0,
                pick: Some(&pick),
            });
        let world = vcod_common::movetrace::MoveWorld::bare(&self.world.collision);
        vcod_cod2::pmove::player_step_anim(&mut p.ps, &cmd, &mut old, world, self.rules, 1.0, anim);
        self.rt.host.cod2_mut().clients[slot].spawn_count += 1;
        self.rt.engine_client_spawn(slot, origin.to_array(), yaw);
    }

    /// Whether `slot` is a client playing in the world (not a spectator or
    /// at the intermission).
    pub fn playing(&self, slot: usize) -> bool {
        self.players
            .get(slot)
            .and_then(Option::as_ref)
            .is_some_and(|p| p.ps.pm_type < pm_type::SPECTATOR)
    }

    /// The level clock the last frame ran at.
    pub fn level_time(&self) -> i32 {
        self.level_time
    }

    /// `level.frametime`.
    pub fn frametime_ms(&self) -> i32 {
        self.frametime_ms
    }

    /// `SV_PacketEvent`'s skeleton cache reset, before a client packet's
    /// cmds (lnxded 0x80960e2): the next trace through any body builds its
    /// skeleton anew.
    pub fn packet(&mut self) {
        self.combat.new_stamp();
    }

    /// `G_GetSavePersist`: whether the level's `map_restart` or
    /// `exitLevel` asked to keep `game[]` and `pers[]`.
    pub fn save_persist(&self) -> bool {
        self.rt.host.save_persist
    }

    /// What the next level keeps of this one: `game[]` and every `pers[]`
    /// under `savePersist`, and on a restart of the same map the compiled
    /// closures (`restart`).
    pub fn take_carry(&self, restart: bool) -> crate::game::script::Carry {
        let all = self.rt.take_carry();
        let keep = self.save_persist();
        crate::game::script::Carry {
            game: all.game.filter(|_| keep),
            pers: if keep { all.pers } else { Vec::new() },
            scripts: all.scripts.filter(|_| restart),
            gametype: all.gametype,
            ..Default::default()
        }
    }
}

/// `P_DamageFeedback` (Mac 0x1a3366), from `ClientEndFrame` once
/// `aimSpreadScale` has been read for the next shots: the frame's damage
/// as a percentage of the max health (127 at most) into `damageCount`,
/// added to `aimSpreadScale` (255 at most), the direction it came along
/// as the two angle bytes (255 both with none), and `damageEvent` counted
/// up (cod2-combat.md 9.2).
fn damage_feedback(p: &mut Cod2Player, max_health: i32, level_time: i32) {
    if p.ps.pm_type > pm_type::INTERMISSION {
        return;
    }
    p.damage_count = 0;
    if p.damage_blood < 1 || max_health < 1 {
        return;
    }
    let count = (p.damage_blood * 100 / max_health).min(127);
    p.ps.aim_spread_scale = (p.ps.aim_spread_scale + count as f32).min(255.0);
    // The view's damage kick (`gclient+0x2840..`, lnxded 0x80f481c):
    // `0.2 * count` degrees in 5..90, pitched and rolled by the blow's
    // direction against the view, which the aim block eases out.
    let d = f64::from;
    let kick = ((d(0.2f32) * d(count as f32)) as f32).clamp(5.0, 90.0);
    if p.damage_from_world {
        (p.damage_pitch, p.damage_yaw) = (255, 255);
        p.aim.kick_roll = 0.0;
        p.aim.kick_pitch = -kick;
    } else {
        let a = crate::game::builtins::math::vectoangles(p.damage_from.to_array());
        let axis = vcod_common::x87::angles_to_axis(Vec3::from_array(p.ps.view_angles));
        let dot = |v: Vec3| vcod_common::x87::dot(p.damage_from, v);
        p.aim.kick_roll = (dot(axis[1]) * -d(kick)) as f32;
        p.aim.kick_pitch = (d(kick) * dot(axis[0])) as f32;
        let byte = |deg: f32| (f64::from(deg) / 360.0 * 256.0) as i32;
        (p.damage_pitch, p.damage_yaw) = (byte(a[0]), byte(a[1]));
    }
    p.aim.kick_time = level_time - 20;
    p.damage_event += 1;
    p.damage_count = count;
    p.damage_blood = 0;
}

/// The script's tables into the weapon state before a cmd: the held
/// weapons, the weapon in hand, the offhand, the rounds. They are the
/// truth between cmds, which the weapon builtins write.
fn sync_weapons_in(host: &GameHost, slot: usize, ws: &mut vcod_cod2::pmove::weapon::WeaponState) {
    let w = host.client_weapons.get(slot).copied().unwrap_or_default();
    ws.held = u128::from(w.held);
    ws.weapon = w.current;
    ws.offhand = host.cod2().clients[slot].offhand;
    if let Some(a) = host.client_ammo.get(slot) {
        for i in 0..a.ammo.len() {
            ws.ammo[i] = i32::from(a.ammo[i]);
            ws.clip[i] = i32::from(a.clip[i]);
        }
    }
}

/// The weapon state's cmd back into the script's tables.
fn sync_weapons_out(host: &mut GameHost, slot: usize, ws: &vcod_cod2::pmove::weapon::WeaponState) {
    if let Some(w) = host.client_weapons.get_mut(slot) {
        w.current = ws.weapon;
        w.held &= ws.held as u64;
    }
    host.cod2_mut().clients[slot].offhand = ws.offhand;
    if let Some(a) = host.client_ammo.get_mut(slot) {
        for i in 0..a.ammo.len() {
            a.ammo[i] = ws.ammo[i].clamp(i16::MIN.into(), i16::MAX.into()) as i16;
            a.clip[i] = ws.clip[i].clamp(i16::MIN.into(), i16::MAX.into()) as i16;
        }
    }
}

/// `legsAnim`'s restart toggle.
const ANIM_TOGGLEBIT: i32 = vcod_cod2::pmove::anim::ANIM_TOGGLEBIT;

/// `player_die`'s `BG_AnimScriptEvent(ps, ANIM_ET_DEATH, 0, 1)` (Mac
/// 0x1c4a7c): the `death` block's command `pick` chooses, forced, onto
/// the playerstate; returns its duration.
pub fn death_event(
    ps: &mut PlayerState,
    input: &vcod_cod2::pmove::AnimInput,
    pick: impl FnOnce(usize) -> usize,
) -> Option<i32> {
    vcod_cod2::pmove::anim::script_event(ps, 0, input, "death", pick, (false, true))
}

/// `pm_flags` 0x800000, which `ClientEndFrame` sets on every playing
/// client and `BG_PlayerStateToEntityState` reads as a visible body
/// (cod2-protocol.md 14.1).
pub const PMF_PLAYING: u32 = vcod_cod2::net::flags::PMF_CMD_STANCE as u32;
/// `pm_flags` and `eFlags` a retail spectator carries (cod2-protocol.md 9).
pub const PMF_SPECTATOR: u32 = vcod_cod2::net::flags::PMF_SPECTATOR_SEEN as u32;
const EF_TELEPORT_BIT: u32 = vcod_cod2::net::flags::EF_TELEPORT_BIT as u32;

/// `ClientSpawn`'s `eFlags` (Mac 0x1930a6): the vote and teleport bits
/// kept over the memset, the teleport bit flipped so no client lerps the
/// body from where it was. A client connects with 0 (`ClientConnect`'s
/// memset), so its first spawn reads 2 (cod2-killcam.md 5).
fn spawn_e_flags(old: u32) -> u32 {
    (old & (super::cod2_cmds::EF_VOTED | EF_TELEPORT_BIT)) ^ EF_TELEPORT_BIT
}

/// The spectator's `speed`, `gravity` and `pm_flags` as a retail
/// spectator's playerstate reads (cod2-protocol.md 9); its teleport bit
/// is whatever its spawns left.
fn set_spectator(ps: &mut PlayerState) {
    ps.pm_type = pm_type::SPECTATOR;
    ps.speed = 400;
    ps.gravity = 0;
    ps.pm_flags = PMF_SPECTATOR;
    // No ground trace runs for a spectator: the live spectator's
    // `groundEntityNum` reads 0 (cod2-protocol.md 15.6).
    ps.ground_entity_num = 0;
}

/// `SetClientViewAngle`: the view at `angles`, the cmd's own angles kept
/// out through `delta_angles`.
pub(crate) fn set_view_angles(ps: &mut PlayerState, cmd: &UserCmd2, angles: [f32; 3]) {
    for (i, a) in angles.into_iter().enumerate() {
        ps.delta_angles[i] = (angle_to_short(a) - cmd.angles[i]) & 0xffff;
    }
    ps.view_angles = angles;
}

/// `ANGLE2SHORT`.
fn angle_to_short(deg: f32) -> i32 {
    ((deg * 65536.0 / 360.0) as i32) & 0xffff
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The body's ring takes the playerstate's events less the ones kept
    /// for the client's own ears; the retail client logs `Event
    /// EV_STEP_VIEW just for client N was sent to other clients` for each
    /// one that reaches another client (cod2-protocol.md 16).
    #[test]
    fn a_body_never_carries_the_playerstate_only_events() {
        use vcod_cod2::net::events as ev;
        let mut ps = PlayerState::default();
        let mut body = BodyEvents::default();
        ps.add_event(ev::EV_STEP_VIEW, 3);
        ps.add_event(ev::EV_FOOTSTEP_RUN, 0);
        ps.add_event(ev::EV_BULLET_HIT_CLIENT_SMALL, 7);
        ps.add_event(ev::EV_FIRE_WEAPON, 9);
        body.copy_from(&ps);
        assert_eq!(body.sequence, 2);
        assert_eq!(body.copied, 4);
        assert_eq!(
            &body.events[..2],
            &[ev::EV_FOOTSTEP_RUN, ev::EV_FIRE_WEAPON]
        );
        assert_eq!(&body.parms[..2], &[0, 9]);
        // Nothing new: the ring stands.
        body.copy_from(&ps);
        assert_eq!(body.sequence, 2);
        // A spawn's memset puts the ring and the mark back to 0; the
        // entity's own count runs on.
        let mut ps = PlayerState::default();
        body.copied = 0;
        ps.add_event(ev::EV_FIRE_WEAPON, 1);
        body.copy_from(&ps);
        assert_eq!((body.sequence, body.copied), (3, 1));
        assert_eq!(body.events[2], ev::EV_FIRE_WEAPON);
    }

    #[test]
    fn the_lag_time_moves_forward_and_trails_by_a_second_at_most() {
        assert_eq!(lag_time(0, 950, 1000), 950);
        assert_eq!(lag_time(950, 900, 1000), 950);
        assert_eq!(lag_time(950, 1000, 1000), 1000);
        assert_eq!(lag_time(950, 100, 5000), 4000);
        assert_eq!(time_offset(1000, 950, true), 50);
        assert_eq!(time_offset(1000, 950, false), 0);
    }

    /// `script_struct` blocks become `level.struct` entries in lump order,
    /// each key typed by the Radiant list.
    #[test]
    fn script_structs_land_in_level_struct() {
        use vcod_gsc::{ArrayKey, Target, Value, Vm};
        let src = "InitStructs() { level.struct = []; }\n\
                   CreateStruct() { struct = spawnstruct(); \
                   level.struct[level.struct.size] = struct; return struct; }\n";
        let mut vm = Vm::new();
        let ast = vcod_gsc::parse::parse_file(src).unwrap();
        let fns =
            vcod_gsc::compile::compile_file(&ast, "codescripts/struct", vm.interner_mut()).unwrap();
        vm.install(fns).unwrap();
        let mut host = GameHost::new(&crate::game::profile::COD2, vec![String::new(); 2048]);
        host.radiant_keys = Some(crate::game::fields::parse_radiant_keys(
            "vector origin\nint count\n",
        ));
        let block = |o: &str, n: &str| {
            [("origin", o), ("count", "3"), ("targetname", n)]
                .into_iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect()
        };
        host.cod2_mut().script_structs = vec![block("1 2 3", "a"), block("4 5 6", "b")];
        create_structs(&mut vm, &mut host);
        vm.with_cx(|cx| {
            let Target::Struct(level) = cx.level() else {
                panic!("level is a struct");
            };
            let structs = cx.intern_folded("struct");
            let Value::Array(arr) = cx.get_field(level, structs) else {
                panic!("level.struct is an array");
            };
            assert_eq!(cx.array_len(arr), 2);
            let Value::Struct(second) = cx.get_index(arr, ArrayKey::Int(1)) else {
                panic!("an entry is a struct");
            };
            let (origin, count, name) = (
                cx.intern_folded("origin"),
                cx.intern_folded("count"),
                cx.intern_folded("targetname"),
            );
            assert_eq!(cx.get_field(second, origin), Value::Vector([4.0, 5.0, 6.0]));
            assert_eq!(cx.get_field(second, count), Value::Int(3));
            let Value::String(n) = cx.get_field(second, name) else {
                panic!("targetname is a string");
            };
            assert_eq!(cx.resolve(n), "b");
        });
    }

    /// Another player's walk stops at a mantle block's box; the block's
    /// own climber walks through it (cod2-movement.md 9). Needs `COD2_DIR`.
    #[test]
    fn a_mantle_block_stops_every_move_but_its_climbers() {
        let Some(fs) = vcod_common::testing::cod2_fs() else {
            return;
        };
        let gsc = std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../cod2/tests/fixtures/turret/probe_turret.gsc"
        ))
        .unwrap();
        let mut cvars = crate::cvars::Cvars::new();
        for (k, v) in [
            ("probe_x", "-299"),
            ("probe_y", "1080"),
            ("probe_z", "43"),
            ("probe_yaw", "90"),
            ("probe_gun", "mg42_bipod_prone_mp"),
            ("probe_back", "56"),
        ] {
            cvars.set(k, v);
        }
        let script = ("maps/mp/gametypes/probe_turret".to_string(), gsc);
        let mut level = Cod2Level::load_with_script(
            Rc::new(fs),
            "mp_toujane",
            "probe_turret",
            script,
            cvars,
            0,
        )
        .unwrap();
        level.connect(0, "walker");
        let mut now = 0;
        let mut frame = |level: &mut Cod2Level, walk: bool| {
            for step in 1..=4 {
                let cmd = UserCmd2 {
                    server_time: now + step * 12,
                    forward_move: if walk { 127 } else { 0 },
                    ..Default::default()
                };
                level.client_think(0, &cmd);
            }
            now += 50;
            level.run_frame(now);
        };
        frame(&mut level, false);
        level.begin(0);
        for _ in 0..20 {
            frame(&mut level, false);
        }
        let start = level.players[0].as_ref().unwrap().ps.clone();
        let mut strafe = |level: &mut Cod2Level| {
            let at = level.level_time;
            let p = level.players[0].as_mut().unwrap();
            p.ps = start.clone();
            p.ps.command_time = at;
            for _ in 0..20 {
                frame(level, true);
            }
            let ps = &level.players[0].as_ref().unwrap().ps;
            (ps.origin - start.origin).truncate()
        };
        let free = strafe(&mut level);
        assert!(free.length() > 100.0, "the walk is clear: {free}");
        let dir = free.normalize().extend(0.0);
        let (mins, maxs) = (start.mins, start.maxs);
        let end = start.origin + dir * 60.0;
        let at = level.level_time;
        level.spawn_mantle_block(1, end, (mins, maxs), 60_000, at);
        let blocked = strafe(&mut level);
        assert!(
            blocked.length() < 31.0 && blocked.length() > 25.0,
            "stopped at the block: {blocked}"
        );
        level.mantle_blocks[0].2.owner = 0;
        let own = strafe(&mut level);
        assert!((own - free).length() < 1.0, "{own} vs {free}");
    }

    /// A blow latched while dead (`P_DamageFeedback` skips `pm_type` 6)
    /// is dropped by the spawn, so the new life's first end frame sends
    /// no damage feedback and the four fields read 0.
    #[test]
    fn a_spawn_drops_the_damage_latched_while_dead() {
        let mut p = Cod2Player::new(PlayerState::default(), UserCmd2::default());
        p.ps.pm_type = pm_type::DEAD;
        (p.damage_event, p.damage_yaw, p.damage_pitch, p.damage_count) = (7, 40, 3, 0);
        p.damage_blood = 60;
        p.damage_from = Vec3::new(1.0, 0.0, 0.0);
        damage_feedback(&mut p, 100, 0);
        assert_eq!(p.damage_blood, 60, "latched while dead");
        p.ps.pm_type = pm_type::NORMAL;
        p.clear_damage();
        damage_feedback(&mut p, 100, 0);
        assert_eq!(
            (p.damage_event, p.damage_yaw, p.damage_pitch, p.damage_count),
            (0, 0, 0, 0)
        );
    }

    /// A blow along yaw -1.2 from 0.7 degrees below packs as 255 both,
    /// the bytes' none, and still has its direction.
    #[test]
    fn a_direction_that_packs_as_none_keeps_its_direction() {
        let mut p = Cod2Player::new(PlayerState::default(), UserCmd2::default());
        let dir = Vec3::new(0.9997134, -0.02088902, 0.01170162);
        p.damage_blood = 40;
        p.damage_from = dir;
        damage_feedback(&mut p, 100, 0);
        assert_eq!((p.damage_pitch, p.damage_yaw), (255, 255));
        assert_eq!(p.damage_dir(), Some(dir));
        p.damage_from_world = true;
        assert_eq!(p.damage_dir(), None);
    }

    #[test]
    fn the_binocular_events_name_their_notifies() {
        assert_eq!(binocular_notify(168), Some("binocular_enter"));
        assert_eq!(binocular_notify(172), Some("binocular_drop"));
        assert_eq!(binocular_notify(173), None);
    }
}
