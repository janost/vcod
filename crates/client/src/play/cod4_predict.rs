//! CoD4 1.7 client-side prediction: `CG_PredictPlayerState`.
//! Every frame the drawn pair's newer
//! playerstate (`cg.nextSnap`) is rebuilt for `vcod_cod4::pmove`
//! ([`PlayerState::from_wire`]) and every cmd past its `commandTime` up to
//! the frame's is stepped on top through `player_step`, the server's own
//! move, over the level's `ClipMap` and the snapshot's solid entities
//! ([`Solids`]). The frame's own cmd, stamped at the frame's time, goes on
//! last; it is not kept (a capture's client, and vcod live, send cmds
//! less often than a frame, so the next frame builds its own off the same
//! cmds). What a new snapshot corrects eases out
//! over `cg_errordecay`, and a step up or down eases the eye over
//! `cg_viewZSmoothingTime` (`CG_SmoothCameraZ`; docs/research/cod4-client-view.md 5).
//!
//! `oldVelocity` is not on the wire: the predictor carries what its own
//! last replay reached at the snapshot's `commandTime`, as CoD2's does
//! (`play::cod2_predict`). The mantle runs in the replayed cmds as on
//! the server, its clips installed with the world ([`Cod4World`]). The
//! weapon half runs with the level's weapon defs ([`Cod4World::level_defs`]),
//! and the predicted ring's new events fire the frame the replay raises
//! them ([`new_events`], `CG_CheckPlayerstateEvents`). Item pickups are
//! not predicted.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;

use anyhow::Result;
use glam::Vec3;
use vcod_cod4::assets::Assets;
use vcod_cod4::assets::model::XModelCollSurf;
use vcod_cod4::collision::ClipMap;
use vcod_cod4::hud::{CS_CODINFO, CS_CODINFO_VALUE};
use vcod_cod4::net::delta;
use vcod_cod4::pmove::weapon::{WeaponDef, WeaponDefs};
use vcod_cod4::pmove::{
    self, CONTENTS_BODY, Cmd, PlayerState, PmWorld, Pmove, Rules, Tr, WeaponMove,
};
use vcod_common::turretpose::angles_quat;

/// `cg_errordecay`'s default: `Dvar_RegisterFloat` (iw3mp.exe 0x43c3d6)
/// of the name at 0x6ca2cc with the float 100.0 at 0x6f9e58
/// (cod4-client-view.md 5).
pub const ERROR_DECAY_MS: f64 = 100.0;
/// `CG_PredictPlayerState`'s miss threshold.
const MISS_EPSILON: f32 = 0.1;
/// `cg_viewZSmoothingMin`, `Max` and `Time` (seconds) at their registered
/// defaults.
const STEP_SMOOTH_MIN: f32 = 1.0;
const STEP_SMOOTH_MAX: f32 = 16.0;
const STEP_SMOOTH_MS: i32 = 100;
/// `CMD_BACKUP`: the cmds a client keeps.
pub const CMD_BACKUP: usize = 128;

/// What prediction moves through: the level's clip map and every weapon's
/// move scales, by name.
pub struct Cod4World {
    pub clip: Arc<ClipMap>,
    pub weapons: HashMap<String, WeaponMove>,
    /// The weapons `weapnext` walks: `inventoryType` (WeaponDef +316,
    /// cod4-gsc.md 7) 0, a primary.
    pub primaries: HashSet<String>,
    /// Each weapon's def for the weapon half, by lower-case name, with its
    /// `ammoName`, `clipName` and `altWeapon`.
    pub defs: HashMap<String, (WeaponDef, [String; 3])>,
    /// The zones' shellshock records (`shock/<name>.shock` rawfiles) by
    /// lower-case name (`vcod_cod4::shock`).
    pub shocks: HashMap<String, vcod_cod4::shock::ShockParms>,
    /// Each weapon's recoil keys by name (`play::cod4_kick`).
    pub kicks: HashMap<String, crate::play::cod4_kick::KickDef>,
    /// The corpses' ragdoll def, `ragdoll.cfg`'s def 1 off the search path
    /// ([`Cod4World::read_ragdoll`]); `None` leaves corpses on their death
    /// anims.
    pub ragdoll: Option<Arc<vcod_cod4::ragdoll::RagdollDef>>,
    /// Each xmodel's collision surfaces by lower-case name, the zones'
    /// first: what a script model's rounds meet (`CG_LocationalTraceDObj`).
    pub model_coll: HashMap<String, Arc<Vec<XModelCollSurf>>>,
    /// Each xmodel with a physics preset by lower-case name: what a
    /// `physicsLaunch`ed part flies as (`play::cod4_physics`).
    pub parts: HashMap<String, Arc<vcod_cod4::physics::PartModel>>,
}

impl Cod4World {
    /// The clip map of `map` (a walked map zone) and the weapons of
    /// `zones`, the first naming one winning; the mantle clips out of
    /// `zones` go to [`vcod_cod4::pmove::mantle::install`].
    pub fn from_assets(map: &Assets, zones: &[&Assets]) -> Result<Cod4World> {
        let clip = Arc::new(ClipMap::from_assets(map)?);
        vcod_cod4::pmove::mantle::install(zones);
        let mut weapons = HashMap::new();
        let mut primaries = HashSet::new();
        let mut defs = HashMap::new();
        let mut shocks = HashMap::new();
        let mut kicks = HashMap::new();
        for a in zones {
            for r in &a.raw_files {
                let name = r.name.to_ascii_lowercase();
                if let Some(n) = name
                    .strip_prefix("shock/")
                    .and_then(|n| n.strip_suffix(".shock"))
                {
                    shocks.entry(n.to_string()).or_insert_with(|| {
                        vcod_cod4::shock::ShockParms::from_text(&String::from_utf8_lossy(&r.data))
                    });
                }
            }
            for d in &a.weapons {
                if let Some(m) = WeaponMove::from_raw(&d.raw) {
                    weapons.entry(d.name.clone()).or_insert(m);
                }
                if d.raw.get(316..320) == Some(&[0; 4]) {
                    primaries.insert(d.name.clone());
                }
                kicks
                    .entry(d.name.clone())
                    .or_insert_with(|| crate::play::cod4_kick::KickDef::from_raw(&d.raw));
                if let Some(def) = WeaponDef::of(d) {
                    let s = |k: &str| d.strings.get(k).cloned().unwrap_or_default();
                    let names = [s("ammoName"), s("clipName"), s("altWeapon")];
                    defs.entry(def.name.clone()).or_insert((def, names));
                }
            }
        }
        let mut model_coll = HashMap::new();
        for a in std::iter::once(map).chain(zones.iter().copied()) {
            for x in a
                .xmodels
                .iter()
                .filter(|x| x.coll_lod >= 0 && !x.coll.is_empty())
            {
                model_coll
                    .entry(x.name.to_ascii_lowercase())
                    .or_insert_with(|| Arc::new(x.coll.clone()));
            }
        }
        let all: Vec<&Assets> = std::iter::once(map).chain(zones.iter().copied()).collect();
        Ok(Cod4World {
            clip,
            weapons,
            primaries,
            defs,
            shocks,
            kicks,
            ragdoll: None,
            model_coll,
            parts: vcod_cod4::physics::part_models(&all),
        })
    }

    /// `exec ragdoll.cfg` (iw3mp.exe's start-up, `iw_00.iwd` on a stock
    /// install) off `fs`: its MP def into [`Self::ragdoll`]. Returns
    /// whether it built one.
    pub fn read_ragdoll(&mut self, fs: &vcod_common::pk3::Pk3Fs) -> bool {
        let def = fs
            .read("ragdoll.cfg")
            .map(|b| vcod_cod4::ragdoll::parse_cfg(&String::from_utf8_lossy(&b)))
            .map(|mut d| std::mem::take(&mut d[vcod_cod4::ragdoll::MP_DEF]))
            .filter(|d| !d.bones.is_empty());
        self.ragdoll = def.map(Arc::new);
        self.ragdoll.is_some()
    }

    /// Under a set `fs_game` the weapons as `BG_LoadWeaponDef` reads them
    /// off the search path (`weapons/mp/<name>`, a mod's iwd over
    /// `main/`'s) in place of the zones' structs, the way vcod-server's
    /// `weapon_files_on_disk` does (cod4-fastfile.md 13): every weapon the
    /// zones carry and every file there; a missing or broken file takes
    /// `defaultweapon_mp`'s under its own name, and the zone's struct
    /// stays only when that fails too. Returns the weapons replaced.
    pub fn overlay_weapon_files(&mut self, fs: &vcod_common::pk3::Pk3Fs) -> usize {
        let mut names: Vec<String> = self.defs.keys().cloned().collect();
        for path in fs.list_prefix("weapons/mp/") {
            let name = &path["weapons/mp/".len()..];
            if !name.is_empty() && !name.contains('/') {
                names.push(name.to_ascii_lowercase());
            }
        }
        names.sort();
        names.dedup();
        let text = |name: &str| match vcod_cod4::weapon_file::load(fs, name)? {
            Ok(t) => Some(t),
            Err(e) => {
                log::warn!("cod4: weapons/mp/{name}: {e:#}");
                None
            }
        };
        let mut n = 0;
        for name in names {
            let Some(t) = text(&name).or_else(|| text("defaultweapon_mp")) else {
                continue;
            };
            let w = t.weapon;
            let Some(def) = WeaponDef::of(&w).map(|d| WeaponDef {
                name: name.to_ascii_lowercase(),
                ..d
            }) else {
                continue;
            };
            let s = |k: &str| w.strings.get(k).cloned().unwrap_or_default();
            let refs = [s("ammoName"), s("clipName"), s("altWeapon")];
            if let Some(m) = WeaponMove::from_raw(&w.raw) {
                self.weapons.insert(name.clone(), m);
            }
            self.kicks.insert(
                name.clone(),
                crate::play::cod4_kick::KickDef::from_raw(&w.raw),
            );
            if w.raw.get(316..320) == Some(&[0; 4]) {
                self.primaries.insert(name.clone());
            } else {
                self.primaries.remove(&name);
            }
            self.defs.insert(name, (def, refs));
            n += 1;
        }
        n
    }

    /// The record a shellshock name loads: its own file, else
    /// `shock/default.shock`, as `BG_LoadShellShockDvars` falls back.
    pub fn shock(&self, name: &str) -> Option<&vcod_cod4::shock::ShockParms> {
        let name = name.to_ascii_lowercase();
        self.shocks
            .get(&name)
            .or_else(|| self.shocks.get("default"))
    }

    /// [`Rules::shock_slows`] for the shellshock configstrings `cs`
    /// (1955 on, one a `shellshockIndex` from 1).
    pub fn shock_slows(&self, cs: impl Fn(u16) -> Vec<u8>) -> u16 {
        (1..16u16)
            .filter(|&i| {
                let name = String::from_utf8_lossy(&cs(1954 + i)).into_owned();
                !name.is_empty() && self.shock(&name).is_some_and(|p| p.movement)
            })
            .fold(0, |bits, i| bits | 1 << i)
    }

    /// The level's weapon defs by number (0 the empty hand) off its weapon
    /// list (`CS_WEAPONS`, in registration order), each ammo and clip slot
    /// the first earlier weapon's of its name and the alt linked, as the
    /// server registers them (`BG_SetupAmmoIndexes`,
    /// `vcod_server::game::cod4::weapons`). A name no zone holds is an
    /// empty def.
    pub fn level_defs(&self, list: &[String]) -> Vec<WeaponDef> {
        let (mut ammo, mut clips) = (vec![String::new()], vec![String::new()]);
        let slot = |v: &mut Vec<String>, n: &str| {
            v.iter().position(|x| x == n).unwrap_or_else(|| {
                v.push(n.to_string());
                v.len() - 1
            })
        };
        let mut alts = vec![String::new()];
        let mut defs = vec![WeaponDef::default()];
        for n in list {
            let (mut d, [a, c, alt]) = self
                .defs
                .get(&n.to_ascii_lowercase())
                .cloned()
                .unwrap_or_default();
            d.ammo_index = slot(&mut ammo, &a);
            d.clip_index = slot(&mut clips, &c);
            alts.push(alt.to_ascii_lowercase());
            defs.push(d);
        }
        for i in 1..defs.len() {
            if alts[i].is_empty() {
                continue;
            }
            if let Some(k) = (1..defs.len()).find(|&k| defs[k].name == alts[i]) {
                defs[i].alt_weapon = k as u32;
            }
        }
        defs
    }
}

/// What a client has fired of its own playerstate's ring: the sequence
/// and the four events (`transPlayerState_t`'s).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Ring {
    pub sequence: i32,
    pub events: [u32; 4],
}

impl Ring {
    pub fn of(ps: &PlayerState) -> Ring {
        Ring {
            sequence: ps.event_sequence & 0xff,
            events: ps.events,
        }
    }
}

/// `CG_CheckPlayerstateEvents` (`iw3mp.exe` 0x446170, client-view doc
/// 5.4): the ring slots of `ps` to fire against `old`, oldest first. Of
/// the newest four sequences, every one from `old`'s on, and before it
/// each of the three under it whose event differs from `old`'s (a
/// prediction the snapshot corrected). Sequences are the wire's 8 bits:
/// an old one more than 64 past the new has wrapped and reads 256 lower.
pub fn new_events(ps: &Ring, old: &Ring) -> Vec<usize> {
    let wrapped = old.sequence > ps.sequence + 64;
    let seen = old.sequence - if wrapped { 256 } else { 0 };
    let ring = ps.sequence - 4..ps.sequence;
    let first_unseen = seen.clamp(ring.start, ring.end);
    let slot = |seq: i32| (seq & 3) as usize;
    let corrected = (ring.start.max(seen - 3)..first_unseen)
        .filter(move |&q| ps.events[slot(q)] != old.events[slot(q)]);
    corrected.chain(first_unseen..ring.end).map(slot).collect()
}

/// The lean dvars the body controllers read (`player_lean_shift_*`,
/// `player_lean_rotate_*`) off the server's mirror, 1.7's defaults where
/// it names none (cod4-player-anims.md 8).
pub fn lean_from_mirror(cs: impl Fn(u16) -> Vec<u8>) -> vcod_common::playerpose::LeanDvars {
    let mut d = vcod_common::playerpose::LeanDvars::default();
    for i in CS_CODINFO..CS_CODINFO_VALUE {
        let name = String::from_utf8_lossy(&cs(i)).to_ascii_lowercase();
        let Some(key) = name.strip_prefix("player_lean_") else {
            continue;
        };
        let value = String::from_utf8_lossy(&cs(i + CS_CODINFO_VALUE - CS_CODINFO)).into_owned();
        let Ok(v) = value.trim().parse::<f32>() else {
            continue;
        };
        let slot = match key {
            "shift_left" => &mut d.shift_left,
            "shift_right" => &mut d.shift_right,
            "shift_crouch_left" => &mut d.shift_crouch_left,
            "shift_crouch_right" => &mut d.shift_crouch_right,
            "rotate_left" => &mut d.rotate_left,
            "rotate_right" => &mut d.rotate_right,
            "rotate_crouch_left" => &mut d.rotate_crouch_left,
            "rotate_crouch_right" => &mut d.rotate_crouch_right,
            _ => continue,
        };
        *slot = v;
    }
    d
}

/// The movement dvars off the server's mirror (`CS_CODINFO` names, the
/// values 128 slots on): what a name the mirror carries says, the rest
/// [`Rules::default`].
pub fn rules_from_mirror(cs: impl Fn(u16) -> Vec<u8>) -> Rules {
    let mut r = Rules::default();
    for i in CS_CODINFO..CS_CODINFO_VALUE {
        let name = String::from_utf8_lossy(&cs(i)).into_owned();
        if name.is_empty() {
            continue;
        }
        let value = String::from_utf8_lossy(&cs(i + CS_CODINFO_VALUE - CS_CODINFO)).into_owned();
        let Ok(v) = value.trim().parse::<f32>() else {
            continue;
        };
        let slot = match name.to_ascii_lowercase().as_str() {
            "friction" => &mut r.friction,
            "stopspeed" => &mut r.stopspeed,
            "inertiamax" => &mut r.inertia_max,
            "inertiaangle" => &mut r.inertia_angle,
            "jump_height" => &mut r.jump_height,
            "jump_stepsize" => &mut r.jump_step_size,
            "jump_ladderpushvel" => &mut r.jump_ladder_push_vel,
            "jump_spreadadd" => &mut r.jump_spread_add,
            "bg_ladder_yawcap" => &mut r.ladder_yawcap,
            "bg_prone_yawcap" => &mut r.prone_yawcap,
            "player_view_pitch_up" => &mut r.view_pitch_up,
            "player_view_pitch_down" => &mut r.view_pitch_down,
            "bg_falldamageminheight" => &mut r.fall_damage_min_height,
            "bg_falldamagemaxheight" => &mut r.fall_damage_max_height,
            "player_movethreshhold" => &mut r.move_threshold,
            "player_footstepsthreshhold" => &mut r.footsteps_threshold,
            "player_strafespeedscale" => &mut r.strafe_speed_scale,
            "player_backspeedscale" => &mut r.back_speed_scale,
            "player_spectatespeedscale" => &mut r.spectate_speed_scale,
            "player_sprintspeedscale" => &mut r.sprint_speed_scale,
            "player_sprinttime" => &mut r.sprint_time,
            "player_sprintmintime" => &mut r.sprint_min_time,
            "player_sprintrechargepause" => &mut r.sprint_recharge_pause,
            "player_sprintstrafespeedscale" => &mut r.sprint_strafe_speed_scale,
            "player_sprintcamerabob" => &mut r.sprint_camera_bob,
            "player_dmgtimer_maxtime" => &mut r.dmgtimer_max_time,
            "player_meleechargefriction" => &mut r.melee_charge_friction,
            "player_dmgtimer_minscale" => &mut r.dmgtimer_min_scale,
            "perk_sprintmultiplier" => &mut r.perk_sprint_multiplier,
            "mantle_check_range" => &mut r.mantle_check_range,
            "mantle_check_radius" => &mut r.mantle_check_radius,
            "mantle_check_angle" => &mut r.mantle_check_angle,
            "mantle_view_yawcap" => &mut r.mantle_view_yawcap,
            "mantle_enable" => {
                r.mantle_enable = v != 0.0;
                continue;
            }
            "jump_slowdownenable" => {
                r.jump_slowdown_enable = v != 0.0;
                continue;
            }
            "player_sprintforwardminimum" => {
                r.sprint_forward_minimum = v as i32;
                continue;
            }
            _ => continue,
        };
        *slot = v;
    }
    r
}

/// `es.solid` of a brush model.
pub const SOLID_BMODEL: u32 = 0xff_ffff;

/// A snapshot entity the prediction clips, where the frame draws it.
#[derive(Clone, Debug)]
pub struct Solid {
    pub number: u32,
    pub origin: Vec3,
    pub angles: Vec3,
    pub kind: SolidKind,
}

#[derive(Clone, Debug)]
pub enum SolidKind {
    /// `CM_TempBoxModel` of a packed `solid`, its contents.
    Box {
        mins: Vec3,
        maxs: Vec3,
        contents: u32,
    },
    /// Inline brush model `*N`.
    Brush(usize),
    /// A script mover with no `solid`: its model's collision surfaces,
    /// which rounds meet and players never (contents 0x2080,
    /// cod4-combat.md 13.1), its `partBits` hiding parts.
    Model {
        surfs: Arc<Vec<XModelCollSurf>>,
        hidden: [u32; 4],
    },
}

impl Solid {
    /// `CG_ClipMoveToEntity`'s box of entity `number`'s `solid`
    /// (`CG_GetEntityBModelBounds`: a unit inside the packed box on every
    /// side; `CG_GetEntityBModelContents`: a player is `CONTENTS_BODY`, the
    /// rest solid), or its brush model. `None` for one that clips nothing
    /// (no `solid`, or a brush model flagged non-solid, `eFlags` 1).
    pub fn of(
        number: u32,
        player: bool,
        solid: u32,
        eflags: u32,
        index: u32,
        origin: Vec3,
        angles: Vec3,
    ) -> Option<Solid> {
        let kind = match solid {
            0 => return None,
            SOLID_BMODEL if eflags & 1 != 0 || index == 0 => return None,
            SOLID_BMODEL => SolidKind::Brush(index as usize),
            s => {
                let x = (s & 0xff) as f32;
                let zd = ((s >> 8) & 0xff) as f32 - 1.0;
                let zu = ((s >> 16) & 0xff) as f32 - 32.0;
                SolidKind::Box {
                    mins: Vec3::new(1.0 - x, 1.0 - x, 1.0 - zd),
                    maxs: Vec3::new(x - 1.0, x - 1.0, zu - 1.0),
                    contents: if player { CONTENTS_BODY } else { 1 },
                }
            }
        };
        Some(Solid {
            number,
            origin,
            angles,
            kind,
        })
    }
}

/// `CG_TraceCapsule`: the world, then each solid entity but `pass` that
/// cuts the trace shorter (`CG_ClipMoveToEntities`).
pub struct Solids<'a> {
    pub clip: &'a ClipMap,
    pub solids: &'a [Solid],
}

impl PmWorld for Solids<'_> {
    fn trace(&self, start: Vec3, end: Vec3, mins: Vec3, maxs: Vec3, pass: u32, mask: u32) -> Tr {
        let mut t = pmove::world_tr(&self.clip.trace(start, end, mins, maxs, mask));
        if t.fraction == 0.0 {
            return t;
        }
        for s in self.solids.iter().filter(|s| s.number != pass) {
            match s.kind {
                // Players never meet a script model's 0x2080.
                SolidKind::Model { .. } => continue,
                SolidKind::Box {
                    mins: bmin,
                    maxs: bmax,
                    contents,
                } => {
                    if contents & mask == 0 {
                        continue;
                    }
                    let mut c = vcod_common::collision::Trace::clear(end);
                    c.fraction = t.fraction;
                    vcod_common::collision::clip_entity_box(
                        &mut c,
                        start,
                        end,
                        mins,
                        maxs,
                        s.origin + bmin,
                        s.origin + bmax,
                        contents,
                        s.number,
                    );
                    if c.fraction < t.fraction || (c.startsolid && !t.startsolid) {
                        t = Tr {
                            fraction: c.fraction,
                            normal: c.normal,
                            surface_flags: 0,
                            contents,
                            entity_num: s.number,
                            startsolid: c.startsolid,
                            allsolid: c.allsolid,
                            walkable: c.normal.z >= vcod_cod4::collision::WALKABLE_NORMAL_Z,
                        };
                    }
                }
                SolidKind::Brush(model) => {
                    if model >= self.clip.submodels.len() {
                        continue;
                    }
                    // `CM_TransformedBoxTrace`: in the model's frame.
                    let q = angles_quat(s.angles.to_array());
                    let local = |p: Vec3| q.inverse() * (p - s.origin);
                    let c =
                        self.clip
                            .trace_model(model, local(start), local(end), mins, maxs, mask);
                    if c.fraction < t.fraction || (c.startsolid && !t.startsolid) {
                        let mut w = pmove::world_tr(&c);
                        w.normal = q * c.normal;
                        w.entity_num = s.number;
                        t = w;
                    }
                }
            }
            if t.fraction == 0.0 {
                break;
            }
        }
        t
    }
}

/// What a frame's prediction gives the view.
#[derive(Clone, Debug)]
pub struct Predicted {
    /// The replayed state at the frame's own cmd.
    pub ps: PlayerState,
    /// `ps.origin` with what is left of the correction and of the last
    /// step's ease (`CG_SmoothCameraZ`).
    pub origin: Vec3,
    /// The weapon half ran (the level's defs were handed in).
    pub armed: bool,
    /// Per event ring slot, the time of the cmd whose step raised it: the
    /// seed a fire event's spread draws from, as the server's shot of that
    /// cmd draws it (cod4-effects.md 12.2).
    pub event_times: [i32; 4],
    /// Per event ring slot, the eye and the gun's pitch and yaw the cmd
    /// that raised it fired from (`ShotAim`).
    pub event_aims: [Option<ShotAim>; 4],
}

/// Where a cmd's rounds leave from and along, as the server's think works
/// it out before the cmd's `Pmove` (`vcod_cod4::pmove::aim`,
/// cod4-combat.md 3.2): the bobbed and leaned eye and the gun pitch
/// and yaw.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ShotAim {
    pub eye: Vec3,
    pub gun: [f32; 2],
}

#[derive(Clone, Copy, Debug, Default)]
pub struct PredictStats {
    pub frames: u64,
    pub cmds_run: u64,
    /// Snapshots whose replay moved last frame's prediction at all, and
    /// past the 0.1 miss threshold.
    pub corrected: u64,
    pub missed: u64,
    /// The longest correction seen.
    pub max_correction: f32,
}

#[derive(Default)]
pub struct Cod4Predictor {
    pub stats: PredictStats,
    /// Last frame's newest `commandTime` and origin, uncorrected.
    last: Option<(i32, Vec3)>,
    /// What starts the view over when it changes (the teleport bit, the
    /// view's client, `stats[4]`), as of the last snapshot predicted from.
    cut: Option<(u32, u32, i32)>,
    error: Vec3,
    /// Local ms the error eases from (`cg.predictedErrorTime`).
    error_ms: f64,
    /// The last frame's local ms (`cg.oldTime`).
    last_ms: Option<f64>,
    /// `oldVelocity` after each replayed cmd, oldest first.
    old_velocity: VecDeque<(i32, [f32; 2])>,
    /// Last frame's view: kept when no cmd runs past the snapshot.
    view: Option<[f32; 3]>,
    /// `viewChangeTime`'s start: the newest kept cmd whose step counted.
    view_change_time: i32,
    /// The frame cmd's step last frame, already eased.
    provisional: f32,
    /// `cg.stepViewChange` and `cg.stepViewStart` (cmd time).
    step_change: f32,
    step_start: i32,
    /// The aim block's state as of `aim_time`, the newest kept cmd it ran
    /// on: a replayed cmd's aim is the one worked out when it first ran.
    aim: vcod_cod4::pmove::aim::AimState,
    aim_time: i32,
    aims: VecDeque<(i32, ShotAim)>,
    /// The view each kept cmd's step left, by its time: a snapshot's
    /// playerstate does not carry the server's view to the bit, and the
    /// aim block reads the view the last cmd left.
    views: VecDeque<(i32, [f32; 3])>,
    /// The newest snapshot's `damageEvent` and time, and the step between
    /// the last two snapshots' times ([`Self::note_damage`]).
    damage_event: Option<u32>,
    level_time: Option<i32>,
    frame_ms: i32,
}

impl Cod4Predictor {
    /// The aim block before `cmd`'s step on `ps` (`ClientThink_real`'s,
    /// cod4-combat.md 3.2), committed to the kept state when `keep` and
    /// the cmd is new. The gun spring takes no kick here: the server's
    /// kicks draw its own `rand()`.
    fn shot_aim(
        &mut self,
        ps: &PlayerState,
        cmd: &Cmd,
        defs: Option<&dyn WeaponDefs>,
        keep: bool,
        sent: i32,
    ) -> ShotAim {
        let t = cmd.server_time;
        if let Some(&(_, a)) = self.aims.iter().find(|(at, _)| *at == t) {
            return a;
        }
        let keys = |w: u32| {
            defs.and_then(|d| d.def(w))
                .map(|d| d.aim.clone())
                .unwrap_or_default()
        };
        let mut ps = ps.clone();
        if let Some(&(_, v)) = self.views.iter().find(|(at, _)| *at == ps.command_time) {
            ps.view_angles = v;
        }
        let ps = &ps;
        let msec = (t - ps.command_time).min(200);
        let view = keys(vcod_cod4::pmove::aim::view_weapon(ps));
        let held = keys(ps.weapon);
        let mut state = self.aim;
        let level_time = self.level_at(sent.max(t)).unwrap_or(ps.command_time);
        let gun = state.fire_angles(ps, &view, &held, level_time, msec);
        let a = ShotAim {
            eye: vcod_cod4::pmove::aim::view_origin(ps, ps.command_time),
            gun,
        };
        if keep && t > self.aim_time {
            self.aim = state;
            self.aim_time = t;
            self.aims.push_back((t, a));
            while self.aims.len() > 64 {
                self.aims.pop_front();
            }
        }
        a
    }

    /// The newest snapshot, before [`Self::predict`]: a step of its
    /// `damageEvent` is the server's `P_DamageFeedback` at `server_time`,
    /// whose damage kick the server's aim block eases into the next cmds'
    /// rounds (cod4-combat.md 3.2). The kick is worked out as the server
    /// did, off the snapshot's view, `damageCount` and direction bytes
    /// (in 256ths, as the server packed them), and the snapshot's time
    /// stands in for the `level.time` the next cmds run at.
    pub fn note_damage(&mut self, snap: &delta::PlayerState, server_time: i32) {
        if let Some(last) = self.level_time.replace(server_time)
            && server_time > last
        {
            self.frame_ms = server_time - last;
        }
        let event = snap.get("damageEvent").unwrap_or(0);
        let last = self.damage_event.replace(event);
        if last.is_none_or(|e| e == event) {
            return;
        }
        let byte = |n: &str| snap.get(n).unwrap_or(0);
        let from = match (byte("damagePitch"), byte("damageYaw")) {
            (255, 255) => None,
            (p, y) => {
                let a = |b: u32| b as f32 * (360.0 / 256.0);
                Some(vcod_common::x87::angle_vectors(Vec3::new(a(p), a(y), 0.0))[0])
            }
        };
        let view =
            std::array::from_fn(|k| snap.get_f32(&format!("viewangles[{k}]")).unwrap_or(0.0));
        let count = byte("damageCount") as i32;
        self.aim.kick = vcod_cod4::pmove::aim::DamageKick::feedback(count, from, view, server_time);
    }

    /// The `level.time` the server runs a packet whose newest cmd is `t`
    /// at, as best the client knows it: the newest snapshot's, or a later
    /// frame's that `t` has reached (the packet arrives after that frame
    /// ran). Every cmd of the packet runs at it.
    fn level_at(&self, t: i32) -> Option<i32> {
        let snap = self.level_time?;
        let frame = self.frame_ms.max(1);
        Some(snap + (t - snap).max(0) / frame * frame)
    }

    /// Predicts off `snap` (`cg.nextSnap`'s playerstate) with `cmds`, the
    /// cmds kept, oldest first, then `frame`, the frame's own. `cut` is the
    /// snapshot's teleport bit, view client and `stats[4]`: a change starts
    /// over and drops any correction in flight (`CG_TransitionPlayerState`).
    /// `defs`, the level's weapon defs, runs the weapon half
    /// (not for a spectator, as the server's think leaves it out).
    #[allow(clippy::too_many_arguments)]
    pub fn predict(
        &mut self,
        snap: &delta::PlayerState,
        cut: (u32, u32, i32),
        cmds: &[Cmd],
        frame: Option<&Cmd>,
        world: &dyn PmWorld,
        rules: Rules,
        weapon: &dyn Fn(u32) -> WeaponMove,
        defs: Option<&dyn WeaponDefs>,
        now_ms: f64,
    ) -> Predicted {
        let old_ms = self.last_ms.replace(now_ms);
        let mut ps = PlayerState::from_wire(snap);
        self.stats.frames += 1;
        if self.cut.is_some_and(|c| c != cut) {
            self.reset();
        }
        self.cut = Some(cut);
        let defs = defs.filter(|_| ps.pm_type != pmove::pm_type::SPECTATOR);
        let ct = ps.command_time;
        if let Some(&(_, v)) = self.old_velocity.iter().find(|(t, _)| *t == ct) {
            ps.old_velocity = v;
        }
        // `ClientThink`'s `oldcmd`: the cmd the server last ran.
        let mut old = cmds
            .iter()
            .rev()
            .find(|c| c.server_time <= ct)
            .copied()
            .unwrap_or_default();
        let last = self.last;
        let mut at_last = last.filter(|&(t, _)| t == ct).map(|_| ps.origin);
        self.old_velocity.retain(|(t, _)| *t >= ct);
        // `pmove_t.viewChange`: the kept cmds' steps, each cmd's once
        // (`viewChangeTime`).
        let mut view_change = 0.0;
        let mut ran = false;
        let mut event_times = [ct; 4];
        let mut event_aims = [None; 4];
        let mut raised = |ps: &PlayerState, from: i32, t: i32, a: ShotAim| {
            for q in from..ps.event_sequence {
                event_times[(q & 3) as usize] = t;
                event_aims[(q & 3) as usize] = Some(a);
            }
        };
        // The newest cmd sent: the server runs a packet's cmds at the
        // frame its newest falls in.
        let sent = cmds.last().map_or(ct, |c| c.server_time);
        for cmd in cmds.iter().filter(|c| c.server_time > ct) {
            let w = weapon(ps.weapon);
            let seq = ps.event_sequence;
            let aim = self.shot_aim(&ps, cmd, defs, true, sent);
            let steps = step(&mut ps, cmd, &mut old, world, rules, w, defs);
            let t = cmd.server_time;
            if !self.views.iter().any(|(at, _)| *at == t) {
                self.views.push_back((t, ps.view_angles));
                while self.views.len() > 64 {
                    self.views.pop_front();
                }
            }
            raised(&ps, seq, cmd.server_time, aim);
            for (t, dz) in steps {
                if self.view_change_time < t {
                    view_change += dz;
                    self.view_change_time = t;
                }
            }
            ran = true;
            self.stats.cmds_run += 1;
            let t = cmd.server_time;
            match self.old_velocity.iter_mut().find(|(at, _)| *at == t) {
                Some(slot) => slot.1 = ps.old_velocity,
                None => self.old_velocity.push_back((t, ps.old_velocity)),
            }
            if last.is_some_and(|(lt, _)| lt == t) {
                at_last = Some(ps.origin);
            }
        }
        // Last frame's prediction against this one's at the same cmd.
        if let (Some((_, before)), Some(after)) = (last, at_last) {
            let delta = before - after;
            let len = delta.length();
            self.stats.max_correction = self.stats.max_correction.max(len);
            if delta != Vec3::ZERO {
                self.stats.corrected += 1;
            }
            if len > MISS_EPSILON {
                self.stats.missed += 1;
                self.error = self.error * self.decay(now_ms) + delta;
                self.error_ms = old_ms.unwrap_or(now_ms);
            }
        }
        self.last = Some((ps.command_time, ps.origin));
        let mut provisional = 0.0;
        match frame.filter(|f| f.server_time > ps.command_time) {
            Some(f) => {
                let w = weapon(ps.weapon);
                let seq = ps.event_sequence;
                let aim = self.shot_aim(&ps, f, defs, false, f.server_time);
                let steps = step(&mut ps, f, &mut old, world, rules, w, defs);
                raised(&ps, seq, f.server_time, aim);
                provisional = steps.iter().map(|s| s.1).sum();
            }
            // No cmd past the snapshot: the view stays last frame's.
            None if !ran => {
                if let Some(v) = self.view {
                    ps.view_angles = v;
                }
            }
            None => {}
        }
        self.view = Some(ps.view_angles);
        // The frame cmd steps again each frame off the same kept cmds, and
        // a kept cmd later takes the step it took: only what changed is new.
        let view_change = view_change + provisional - self.provisional;
        self.provisional = provisional;
        let time = ps.command_time;
        let smooth = matches!(
            ps.pm_type,
            pmove::pm_type::NORMAL | pmove::pm_type::NOCLIP | pmove::pm_type::UFO
        );
        self.smooth_steps(view_change, time, smooth);
        let mut origin = ps.origin + self.error * self.decay(now_ms);
        origin.z -= self.step_left(time);
        Predicted {
            ps,
            origin,
            armed: defs.is_some(),
            event_times,
            event_aims,
        }
    }

    /// `CG_PredictPlayerState`'s tail: a `viewChange` of at least
    /// `cg_viewZSmoothingMin` restarts the ease at the frame's cmd time
    /// with what is left of the last one added, capped at
    /// `cg_viewZSmoothingMax`; otherwise an ease past its time is dropped;
    /// retail starts the ease at the stepping cmd's time, which
    /// is the frame's own.
    fn smooth_steps(&mut self, change: f32, time: i32, smooth: bool) {
        if !smooth || change.abs() < STEP_SMOOTH_MIN {
            if time - self.step_start > STEP_SMOOTH_MS {
                self.step_change = 0.0;
            }
            return;
        }
        let left = self.step_left(time);
        self.step_change = (change + left).clamp(-STEP_SMOOTH_MAX, STEP_SMOOTH_MAX);
        self.step_start = time;
    }

    /// `CG_SmoothCameraZ`: what is left of the step's ease at `time`.
    fn step_left(&self, time: i32) -> f32 {
        let since = time - self.step_start;
        if self.step_change == 0.0 || since < 0 {
            return 0.0;
        }
        let lerp = (since as f32 / STEP_SMOOTH_MS as f32).min(1.0);
        (1.0 - lerp) * self.step_change
    }

    /// Forgets the last frame and any correction in flight.
    pub fn reset(&mut self) {
        self.last = None;
        self.cut = None;
        self.error = Vec3::ZERO;
        self.old_velocity.clear();
        self.view = None;
        self.step_change = 0.0;
        self.provisional = 0.0;
        self.aim = Default::default();
        self.aim_time = 0;
        self.aims.clear();
        self.views.clear();
        self.damage_event = None;
        self.level_time = None;
        self.frame_ms = 0;
    }

    /// The correction still drawn at `now_ms`.
    #[cfg(test)]
    pub fn drawn_error(&self, now_ms: f64) -> f32 {
        (self.error * self.decay(now_ms)).length()
    }

    /// `(cg_errordecay - (cg.time - predictedErrorTime)) / cg_errordecay`,
    /// 0 outside (0, 1) as `CG_CalcViewValues` drops it.
    fn decay(&self, now_ms: f64) -> f32 {
        let f = (ERROR_DECAY_MS - (now_ms - self.error_ms)) / ERROR_DECAY_MS;
        if f > 0.0 && f < 1.0 { f as f32 } else { 0.0 }
    }
}

/// One cmd as the server's think runs it (`pmove::player_step_viewed`,
/// or `player_step_armed` with `defs`): the steps the eye took.
fn step(
    ps: &mut PlayerState,
    cmd: &Cmd,
    oldcmd: &mut Cmd,
    world: &dyn PmWorld,
    rules: Rules,
    weapon: WeaponMove,
    defs: Option<&dyn WeaponDefs>,
) -> Vec<(i32, f32)> {
    let mut pm = Pmove::new(ps, *cmd, *oldcmd, world, rules);
    pm.weapon = weapon;
    pm.defs = defs;
    pmove::pmove(&mut pm);
    *oldcmd = *cmd;
    pm.view_steps
}

/// The flat speed `BG_GetSpeed` reads off a predicted state: on a ladder
/// the climb's, else the horizontal velocity's.
pub fn xyspeed(ps: &PlayerState) -> f32 {
    if ps.pm_flags & pmove::pmf::LADDER != 0 {
        ps.velocity.z.abs()
    } else {
        ps.velocity.truncate().length()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `new_events` on hand-built rings: the slots past the old sequence,
    /// a corrected slot inside the last four, the 8-bit wrap, and nothing
    /// for an equal ring.
    #[test]
    fn new_events_picks_new_and_corrected_slots() {
        let ring = |sequence, events| Ring { sequence, events };
        let cases = [
            (ring(10, [1, 2, 3, 4]), ring(10, [1, 2, 3, 4]), vec![]),
            (ring(12, [1, 2, 3, 4]), ring(10, [1, 2, 3, 4]), vec![2, 3]),
            (
                ring(12, [1, 2, 3, 4]),
                ring(4, [1, 2, 3, 4]),
                vec![0, 1, 2, 3],
            ),
            (ring(10, [1, 9, 3, 4]), ring(10, [1, 2, 3, 4]), vec![1]),
            (ring(10, [9, 2, 3, 4]), ring(9, [1, 2, 3, 4]), vec![0, 1]),
            (
                ring(11, [9, 2, 3, 9]),
                ring(10, [1, 2, 3, 4]),
                vec![3, 0, 2],
            ),
            (
                ring(2, [5, 6, 3, 4]),
                ring(254, [1, 2, 3, 4]),
                vec![2, 3, 0, 1],
            ),
            (
                ring(2, [5, 6, 3, 4]),
                ring(255, [5, 2, 3, 4]),
                vec![3, 0, 1],
            ),
            (ring(40, [1, 2, 3, 4]), ring(100, [1, 2, 3, 4]), vec![]),
            (ring(3, [1, 2, 3, 4]), ring(0, [1, 2, 3, 4]), vec![0, 1, 2]),
        ];
        for (ps, old, want) in cases {
            assert_eq!(new_events(&ps, &old), want, "{ps:?} against {old:?}");
        }
    }

    /// A floor at z 0, solid below, as `vcod_cod4::pmove`'s own tests
    /// build one.
    struct Floor;

    impl PmWorld for Floor {
        fn trace(&self, start: Vec3, end: Vec3, mins: Vec3, _: Vec3, _: u32, _: u32) -> Tr {
            let (a, b) = (start.z + mins.z, end.z + mins.z);
            let mut t = Tr::default();
            if a < 0.0 {
                t.fraction = 0.0;
                t.startsolid = true;
                t.allsolid = b < 0.0;
            } else if b < 0.125 && b < a {
                t.fraction = ((a - 0.125) / (a - b)).max(0.0);
            } else {
                return t;
            }
            t.normal = Vec3::Z;
            t.walkable = true;
            t.entity_num = pmove::ENTITYNUM_WORLD as u32;
            t.contents = 1;
            t
        }
    }

    /// A correction past 0.1 is drawn whole on the frame that finds it
    /// less a frame's decay, and is gone `cg_errordecay` ms later.
    #[test]
    fn a_correction_eases_out_over_the_decay() {
        let mut p = Cod4Predictor::default();
        let mut snap = PlayerState::spawn(Vec3::new(0.0, 0.0, 0.125), 0.0, 1000);
        snap.ground_entity_num = pmove::ENTITYNUM_WORLD;
        // `to_wire` writes what pmove owns; the class script's scale is the
        // server's.
        let mut base = delta::PlayerState::default();
        let k = vcod_cod4::net::fields::PLAYER_STATE
            .iter()
            .find(|f| f.name == "moveSpeedScaleMultiplier")
            .unwrap()
            .word();
        base.words[k] = 1f32.to_bits();
        let wire = snap.to_wire(&base);
        let run = |t: i32| Cmd {
            server_time: t,
            forward_move: 127,
            ..Cmd::default()
        };
        let cmds: Vec<Cmd> = (1..=6).map(|k| run(1000 + 16 * k)).collect();
        // A rifle's scales: weapon 0 moves nothing.
        let w = |_| WeaponMove {
            move_speed_scale: 1.0,
            ads_move_speed_scale: 0.5,
            sprint_duration_scale: 1.0,
        };
        let cut = (0, 0, 0);
        let rules = Rules::default();
        let a = p.predict(&wire, cut, &cmds, None, &Floor, rules, &w, None, 0.0);
        assert!(a.ps.origin.x > 1.0, "ran {:?}", a.ps.origin);
        // The same snapshot again: no correction.
        p.predict(&wire, cut, &cmds, None, &Floor, rules, &w, None, 16.0);
        assert_eq!(p.stats.missed, 0);
        // A snapshot that puts the body 5 units aside at the same cmds.
        let mut moved = snap.clone();
        moved.origin.y += 5.0;
        let wire2 = moved.to_wire(&base);
        let b = p.predict(&wire2, cut, &cmds, None, &Floor, rules, &w, None, 32.0);
        assert_eq!(p.stats.missed, 1);
        assert!((b.origin.y - b.ps.origin.y + 5.0 * (1.0 - 16.0 / 100.0)).abs() < 1e-3);
        assert!(p.drawn_error(16.0 + 100.0) == 0.0);
        // A cut drops it.
        p.predict(
            &wire2,
            (2, 0, 0),
            &cmds,
            None,
            &Floor,
            rules,
            &w,
            None,
            48.0,
        );
        assert_eq!(p.drawn_error(48.0), 0.0);
    }

    /// A packed player `solid` clips as the box a unit inside it.
    #[test]
    fn a_body_blocks_the_replay() {
        // `SV_LinkEntity`'s packing of (-15,-15,0)..(15,15,70).
        let solid = 15 | (1 << 8) | ((70 + 32) << 16);
        let s = Solid::of(3, true, solid, 0, 0, Vec3::new(64.0, 0.0, 0.0), Vec3::ZERO).unwrap();
        let SolidKind::Box { mins, maxs, .. } = s.kind else {
            panic!("a box");
        };
        assert_eq!(
            (mins, maxs),
            (Vec3::new(-14.0, -14.0, 1.0), Vec3::new(14.0, 14.0, 69.0))
        );
    }
}

/// The weapon files under a mod (`COD4_DIR` with `mods/ModWarfare`; passes
/// vacuously without): every stock weapon is read off `main/`'s iwds in
/// place of the zone's struct, and reads as the zone compiled it.
#[cfg(test)]
#[test]
fn cod4_weapon_files_replace_the_zone_defs_under_a_mod() {
    let Some(install) = vcod_cod4::testing::cod4_install() else {
        return;
    };
    let g = Some("mods/ModWarfare");
    if install.clone().with_fs_game(g).mod_zone().is_none() {
        return;
    }
    let mut world = crate::cod4_fly::load_world(install.root(), "mp_crash").unwrap();
    let zone = world.defs.clone();
    let fs = crate::cod4_fly::open_fs(install.root(), g, Some("mp_crash")).unwrap();
    let n = world.overlay_weapon_files(&fs);
    assert!(n >= zone.len(), "{n} of {} weapons", zone.len());
    for name in ["m16_mp", "ak47_mp", "frag_grenade_mp"] {
        assert_eq!(world.defs.get(name), zone.get(name), "{name}");
    }
}
