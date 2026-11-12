//! The effects a CoD2 connection plays: the visual, table-driven events
//! `play::cod2_events::EventTracker` found, played as `CG_EntityEvent`
//! plays them, `ET_FX` entities replayed on their timers, and the weapons'
//! muzzle flashes and brass on their tags (docs/research/cod2-efx.md 6).
//! The sounds of the same events are `play::cod2_sound`'s.

use crate::fx::cod2_tracer;
use crate::fx::sim::{FxSystem, Rng, SpawnAt, TagBolt, TagPose};
use crate::play::cod2_events::{Event2, SURFACES};
use crate::play::cod2_sound::{PMF_FIRST_PERSON, muzzle_point};
use glam::Vec3;
use std::collections::{HashMap, HashSet};
use vcod_cod2::net::events::{self as ev, ET_EVENTS, ET_FX, ET_PLAYER, ET_TURRET};
use vcod_cod2::net::flags::EF_TURRET_STANCE;
use vcod_cod2::net::{ConfigStrings, EntityView, Frame, SnapshotView};
use vcod_common::asset_source::{Asset, AssetKind, AssetSource};

/// One effect to play.
#[derive(Clone, Debug)]
pub struct FxPlay {
    pub play: Play,
    /// The event or entity that played it, for logs and tests.
    pub why: String,
}

/// What an [`FxPlay`] spawns.
#[derive(Clone, Debug)]
pub enum Play {
    /// `fx/...`, as the effect configstring or table names it.
    Effect { path: String, at: SpawnAt },
    /// A bullet tracer (`CG_SpawnTracer`, `fx::cod2_tracer`), its chance
    /// already rolled; `back_ms` its back-date.
    Tracer {
        muzzle: Vec3,
        hit: Vec3,
        back_ms: i32,
    },
    /// An effect on a drawn body's tag (`FX_PlayEntityEffect`, Mac
    /// 0x60d14): spawned on the tag's pose, its `relative` elements riding
    /// the tag after.
    OnTag {
        path: String,
        bolt: TagBolt,
        pose: TagPose,
    },
    /// An effect on the view weapon's tag (`weapon` its CS 7 index), which
    /// the client spawns once the frame has placed the gun
    /// (`cod2_client::Cod2Client::view`).
    View {
        path: String,
        tag: ViewTag,
        weapon: i32,
    },
}

/// The weapon tag an effect plays on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ViewTag {
    /// `tag_flash` (`_s_barrelTags[0]`).
    Flash,
    /// `tag_brass`.
    Brass,
}

impl ViewTag {
    /// The [`TagBolt`] slot a body's or the view weapon's tag rides as:
    /// below the configstring slots `playFxOnTag` names.
    pub fn slot(self) -> i32 {
        match self {
            ViewTag::Flash => -1,
            ViewTag::Brass => -2,
        }
    }
}

/// The bolt entity of the view weapon's DObj, `0x400 + weapon`
/// (`CG_RegisterWeapon`, Mac 0x1d85cc): past every entity number.
pub fn view_bolt(weapon: i32) -> u32 {
    0x400 + weapon.max(0) as u32
}

/// A drawn body's `tag_flash` and `tag_brass` in the world, the poses its
/// world flash and brass spawn on (`FX_GetBoneOrientation`, Mac 0x59230).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct BodyTags {
    pub flash: Option<TagPose>,
    pub brass: Option<TagPose>,
}

/// What the frame's draw and dvars hand the effects.
#[derive(Clone, Copy)]
pub struct FxScene<'a> {
    /// Each drawn body's and gun's world `tag_flash`, where its tracers
    /// start.
    pub muzzles: &'a HashMap<u32, Vec3>,
    /// Each drawn body's tags ([`BodyTags`]).
    pub tags: &'a HashMap<u32, BodyTags>,
    /// `cg_brass`: off, `CG_EjectWeaponBrass` (Mac 0x1d80ba) returns at once.
    pub brass: bool,
}

impl FxPlay {
    fn effect(path: impl Into<String>, at: SpawnAt, why: impl Into<String>) -> FxPlay {
        FxPlay {
            play: Play::Effect {
                path: path.into(),
                at,
            },
            why: why.into(),
        }
    }

    /// The effect's path; `None` for a tracer.
    #[cfg(test)]
    pub fn path(&self) -> Option<&str> {
        match &self.play {
            Play::Effect { path, .. } | Play::OnTag { path, .. } | Play::View { path, .. } => {
                Some(path)
            }
            Play::Tracer { .. } => None,
        }
    }

    pub fn spawn(&self, fx: &mut FxSystem, fs: &dyn AssetSource, now: f32) {
        match &self.play {
            Play::Effect { path, at } => {
                log::debug!("cod2 fx {path} ({})", self.why);
                fx.spawn(fs, path, *at, now);
            }
            &Play::Tracer {
                muzzle,
                hit,
                back_ms,
            } => fx.spawn_cod2_tracer(muzzle, hit, now, back_ms),
            Play::OnTag { path, bolt, pose } => {
                log::debug!("cod2 fx {path} on {bolt:?} ({})", self.why);
                fx.spawn_on_tag(fs, path, *bolt, *pose, now);
            }
            // The client's to place (`Cod2Client::view`).
            Play::View { .. } => {}
        }
    }
}

/// `g_TypeName`'s eleven impact kinds (Mac 0x36e7e0), the rows of the
/// impact table.
const IMPACT_KINDS: [&str; 11] = [
    "bullet_small_normal",
    "bullet_small_reflect",
    "bullet_large_normal",
    "bullet_large_reflect",
    "shotgun_normal",
    "shotgun_reflect",
    "bullet_ap_normal",
    "bullet_ap_reflect",
    "grenade_bounce",
    "grenade_explode",
    "rocket_explode",
];
const SMALL: usize = 0;
const LARGE: usize = 2;
const SHOTGUN: usize = 4;
const GRENADE_BOUNCE: usize = 8;
const GRENADE_EXPLODE: usize = 9;
const ROCKET_EXPLODE: usize = 10;

/// `CG_RegisterImpactEffects` (Mac 0x1e7dde): the effect per impact kind
/// and surface, `None` for a blank cell.
#[derive(Clone, Debug, Default)]
pub struct ImpactTable {
    rows: Vec<[Option<String>; 23]>,
}

/// Every `.csv` directly in `fx/`, then in `fx/maps/<map>/` (`map` a map
/// name, `""` for none), each directory's files by name, a later row
/// overriding an earlier (`CG_RegisterImpactEffectsForDir`, Mac 0x1e7a9e).
/// Built through [`Asset::find`], uncached: [`Cod2Fx`] keeps the table.
impl Asset for ImpactTable {
    const KIND: AssetKind = AssetKind::Cod2ImpactEffects;

    fn find(src: &dyn AssetSource, map: &str) -> anyhow::Result<Self> {
        let mut t = ImpactTable {
            rows: vec![Default::default(); IMPACT_KINDS.len()],
        };
        let csvs = src.list("fx", ".csv");
        let mut dirs = vec!["fx/".to_string()];
        if !map.is_empty() {
            dirs.push(format!("fx/maps/{map}/"));
        }
        for dir in dirs {
            let mut files: Vec<&String> = csvs
                .iter()
                .filter(|n| n.strip_prefix(&dir).is_some_and(|rest| !rest.contains('/')))
                .collect();
            files.sort();
            for f in files {
                if let Some(b) = src.read(f) {
                    t.read_csv(&String::from_utf8_lossy(&b));
                }
            }
        }
        Ok(t)
    }
}

impl ImpactTable {
    /// [`Asset::find`]'s table for `map`.
    pub fn load(fs: &dyn AssetSource, map: Option<&str>) -> ImpactTable {
        ImpactTable::find(fs, map.unwrap_or("")).unwrap_or_default()
    }

    /// One file's rows: `kind,surface,effect`; `#` lines and unknown names
    /// are skipped.
    pub fn read_csv(&mut self, text: &str) {
        for line in text.lines() {
            let mut cols = line.split(',').map(|c| c.trim().trim_matches('"'));
            let (Some(kind), Some(surf)) = (cols.next(), cols.next()) else {
                continue;
            };
            let effect = cols.next().unwrap_or("");
            let (Some(k), Some(s)) = (
                IMPACT_KINDS
                    .iter()
                    .position(|n| n.eq_ignore_ascii_case(kind)),
                SURFACES.iter().position(|n| n.eq_ignore_ascii_case(surf)),
            ) else {
                continue;
            };
            self.rows[k][s] = (!effect.is_empty()).then(|| effect.to_string());
        }
    }

    pub fn get(&self, kind: usize, surface: i32) -> Option<&str> {
        let s = usize::try_from(surface)
            .ok()
            .filter(|&s| s < SURFACES.len())?;
        self.rows.get(kind)?[s].as_deref()
    }
}

/// `ByteToDir`: the same 162 normals as CoD1's.
fn byte_to_dir(b: i32) -> Vec3 {
    Vec3::from_array(vcod_common::net::events::byte_to_dir(b))
}

/// What a weapon file gives the effects: its flashes, shell ejects and
/// explosion, `CG_RegisterWeapon`'s (Mac 0x1d85cc) `cg_weapons` slots
/// +0xc0, +0xc4, +0x15c, +0x160 and +0x164.
#[derive(Clone, Debug, Default)]
struct WeaponFx {
    view_flash: Option<String>,
    world_flash: Option<String>,
    shell_eject: Option<String>,
    /// `lastShotEjectEffect`, else the shell eject.
    last_shot_eject: Option<String>,
    explosion: Option<String>,
    /// `boltAction`: the brass leaves at `EV_EJECT_BRASS`, not the shot.
    bolt_action: bool,
    /// `weaponType` binoculars.
    binoculars: bool,
}

impl WeaponFx {
    fn of(d: &vcod_cod2::weapon::WeaponDef) -> WeaponFx {
        let s = |k: &str| Some(d.str(k).to_string()).filter(|s| !s.is_empty());
        let shell_eject = s("shellEjectEffect");
        WeaponFx {
            view_flash: s("viewFlashEffect"),
            world_flash: s("worldFlashEffect"),
            last_shot_eject: s("lastShotEjectEffect").or_else(|| shell_eject.clone()),
            shell_eject,
            explosion: s("projExplosionEffect"),
            bolt_action: d.flag("boltAction"),
            binoculars: d.weap_type() == vcod_cod2::weapon::WEAPTYPE_BINOCULARS,
        }
    }
}

/// A CoD2 connection's effect state: the `ET_FX` timers, the impact table
/// and the weapon files by CS 7 index. A new gamestate starts it over.
#[derive(Default)]
pub struct Cod2Fx {
    /// `cent + 0x210`: the last play of each `ET_FX` entity.
    loops: HashMap<u32, i32>,
    impacts: Option<ImpactTable>,
    weapons: HashMap<i32, WeaponFx>,
    /// Bad effect ids already logged.
    warned: HashSet<i32>,
    /// The tracers' `rand()`.
    rng: Option<Rng>,
    /// The last frame's render time, for `cg.frametime`.
    last_ms: Option<i32>,
}

impl Cod2Fx {
    /// This frame's plays: `events` (the snapshot that became current,
    /// `Cod2Session::take_events`) and the `ET_FX` entities due at the
    /// render time, culled from `view`. Without `fs` the table-driven
    /// events play nothing.
    pub fn frame(
        &mut self,
        frame: &Frame,
        events: &[Event2],
        view: Vec3,
        fs: Option<&dyn AssetSource>,
        scene: FxScene,
    ) -> Vec<FxPlay> {
        let cs = frame.configstrings;
        if let Some(fs) = fs
            && self.impacts.is_none()
        {
            self.impacts = Some(ImpactTable::load(fs, cs.map_name()));
        }
        let frametime = self
            .last_ms
            .map_or(0, |l| frame.time_ms.wrapping_sub(l).max(0));
        self.last_ms = Some(frame.time_ms);
        let mut out = Vec::new();
        for e in events {
            self.event(e, cs, fs, &mut out);
            self.weapon_fx(e, frame.snap, cs, fs, scene, &mut out);
            self.tracer(e, frame.snap, scene.muzzles, frametime, &mut out);
        }
        for e in &frame.snap.entities {
            if e.etype == ET_FX {
                self.looped(e, frame, view, &mut out);
            }
        }
        out
    }

    /// Every effect a level registers at its gamestate: `CG_RegisterGraphics`
    /// (Mac 0x14374a) takes configstring effects 1 to 63 up to the first
    /// empty one, `fx/props/american_smoke_grenade.efx`, the impact table
    /// (`CG_RegisterImpactEffects`) and `fx/impacts/flesh_hit_noblood.efx`;
    /// `CG_RegisterWeapon` (0x1d85cc) each CS 7 weapon's flash, shell eject
    /// and explosion effects (cod2-client-perf.md, "The soak"). Loads the
    /// impact table and the weapon files on the way.
    pub fn level_effects(&mut self, cs: &ConfigStrings, fs: &dyn AssetSource) -> Vec<String> {
        let mut out: Vec<String> = (1..64)
            .map_while(|id| cs.effect(id))
            .map(str::to_string)
            .collect();
        out.push("fx/props/american_smoke_grenade.efx".into());
        out.push("fx/impacts/flesh_hit_noblood.efx".into());
        let impacts = self
            .impacts
            .get_or_insert_with(|| ImpactTable::load(fs, cs.map_name()));
        out.extend(impacts.rows.iter().flatten().flatten().cloned());
        for index in 1.. {
            let Some(name) = cs.weapon_name(index) else {
                break;
            };
            let Ok(p) = vcod_cod2::weapon::load(fs, name) else {
                continue;
            };
            let w = self
                .weapons
                .entry(index)
                .or_insert_with(|| WeaponFx::of(&p.def));
            out.extend(
                [
                    &w.view_flash,
                    &w.world_flash,
                    &w.shell_eject,
                    &w.last_shot_eject,
                    &w.explosion,
                ]
                .into_iter()
                .flatten()
                .cloned(),
            );
        }
        out.sort();
        out.dedup();
        out
    }

    fn weapon(&mut self, cs: &ConfigStrings, fs: Option<&dyn AssetSource>, index: i32) -> WeaponFx {
        if let Some(w) = self.weapons.get(&index) {
            return w.clone();
        }
        let Some(fs) = fs else {
            return WeaponFx::default();
        };
        let w = cs
            .weapon_name(index)
            .and_then(|n| vcod_cod2::weapon::load(fs, n).ok())
            .map_or_else(WeaponFx::default, |p| WeaponFx::of(&p.def));
        self.weapons.insert(index, w.clone());
        w
    }

    /// `BG_GetViewmodelWeaponIndex` (Mac 0x7b226) on `ps`, with the first
    /// binoculars weapon of configstring 7.
    fn view_weapon(
        &mut self,
        ps: &vcod_cod2::net::PlayerView,
        cs: &ConfigStrings,
        fs: Option<&dyn AssetSource>,
    ) -> i32 {
        let binoculars = (1..)
            .take_while(|&i| cs.weapon_name(i).is_some())
            .find(|&i| self.weapon(cs, fs, i).binoculars)
            .unwrap_or(0);
        crate::cod2_viewmodel::view_weapon_index(ps, binoculars)
    }

    /// A shot's muzzle flash and brass, and `EV_EJECT_BRASS`'s brass
    /// (cod2-efx.md 6.8). `CG_FireWeapon` (Mac 0x1d823c), on any weapon
    /// but 0, flags the shooter's flash for `CG_AddPlayerWeapon` (Mac
    /// 0x1d9c8e) to play when it next adds the gun: the playerstate's
    /// events on the view weapon's `tag_flash` with its `viewFlashEffect`,
    /// another player's on his `tag_flash` with the `worldFlashEffect`,
    /// none on a gunner (`eFlags` 0x300). A turret's own shot plays its
    /// flash at once on its `tag_flash`: the view flash on the gun the
    /// playerstate is locked on, else the world flash. It then ejects
    /// brass once per call (the quad barrel events call it twice) unless
    /// the weapon is `boltAction`.
    fn weapon_fx(
        &mut self,
        e: &Event2,
        snap: &SnapshotView,
        cs: &ConfigStrings,
        fs: Option<&dyn AssetSource>,
        scene: FxScene,
        out: &mut Vec<FxPlay>,
    ) {
        let calls = match e.event {
            ev::EV_FIRE_WEAPON
            | ev::EV_FIRE_WEAPONB
            | ev::EV_FIRE_WEAPON_LASTSHOT
            | ev::EV_FIRE_WEAPON_MG42 => 1,
            ev::EV_FIRE_QUADBARREL_1 | ev::EV_FIRE_QUADBARREL_2 => 2,
            ev::EV_EJECT_BRASS => {
                self.brass(e, snap, cs, fs, scene, out);
                return;
            }
            _ => return,
        };
        if e.weapon == 0 {
            return;
        }
        let why = ev::event_name(e.event).unwrap_or("?");
        if e.etype == ET_TURRET {
            let w = self.weapon(cs, fs, e.weapon);
            let manned = snap.ps.eflags & EF_TURRET_STANCE != 0
                && snap.ps.raw.i32("viewlocked_entNum") == e.entity as i32;
            let path = if manned { w.view_flash } else { w.world_flash };
            if let Some(path) = path
                && let Some(pose) = scene.tags.get(&e.entity).and_then(|t| t.flash)
            {
                out.push(FxPlay {
                    play: Play::OnTag {
                        path,
                        bolt: TagBolt {
                            entity: e.entity,
                            slot: ViewTag::Flash.slot(),
                        },
                        pose,
                    },
                    why: why.into(),
                });
            }
        } else if e.etype == ET_PLAYER && e.eflags & EF_TURRET_STANCE == 0 {
            if e.from_ps {
                let weapon = self.view_weapon(&snap.ps, cs, fs);
                if let Some(path) = self.weapon(cs, fs, weapon).view_flash {
                    out.push(FxPlay {
                        play: Play::View {
                            path,
                            tag: ViewTag::Flash,
                            weapon,
                        },
                        why: why.into(),
                    });
                }
            } else if !first_person(snap, e.entity)
                && let Some(path) = self.weapon(cs, fs, e.weapon).world_flash
                && let Some(pose) = scene.tags.get(&e.entity).and_then(|t| t.flash)
            {
                // The first-person body's own entity flags a flash no
                // draw plays: its gun is the view weapon.
                out.push(FxPlay {
                    play: Play::OnTag {
                        path,
                        bolt: TagBolt {
                            entity: e.entity,
                            slot: ViewTag::Flash.slot(),
                        },
                        pose,
                    },
                    why: why.into(),
                });
            }
        }
        if !self.weapon(cs, fs, e.weapon).bolt_action {
            for _ in 0..calls {
                self.brass(e, snap, cs, fs, scene, out);
            }
        }
    }

    /// `CG_EjectWeaponBrass` (Mac 0x1d80ba): under `cg_brass`, for an
    /// entity that is no temp event and holds a weapon, the weapon's
    /// last-shot eject on `EV_FIRE_WEAPON_LASTSHOT`, else its shell eject,
    /// on the view weapon's `tag_brass` for the first-person body, else on
    /// the shooter's.
    fn brass(
        &mut self,
        e: &Event2,
        snap: &SnapshotView,
        cs: &ConfigStrings,
        fs: Option<&dyn AssetSource>,
        scene: FxScene,
        out: &mut Vec<FxPlay>,
    ) {
        if !scene.brass || e.etype >= ET_EVENTS || e.weapon == 0 {
            return;
        }
        let w = self.weapon(cs, fs, e.weapon);
        let path = match e.event {
            ev::EV_FIRE_WEAPON_LASTSHOT => w.last_shot_eject,
            _ => w.shell_eject,
        };
        let Some(path) = path else {
            return;
        };
        let why = ev::event_name(e.event).unwrap_or("?").to_string();
        let play = if first_person(snap, e.entity) {
            Play::View {
                path,
                tag: ViewTag::Brass,
                weapon: e.weapon,
            }
        } else {
            let Some(pose) = scene.tags.get(&e.entity).and_then(|t| t.brass) else {
                return;
            };
            Play::OnTag {
                path,
                bolt: TagBolt {
                    entity: e.entity,
                    slot: ViewTag::Brass.slot(),
                },
                pose,
            }
        };
        out.push(FxPlay { play, why });
    }

    /// `CG_EntityEvent`'s effect arms (Mac 0x1e4650).
    fn event(
        &mut self,
        e: &Event2,
        cs: &ConfigStrings,
        fs: Option<&dyn AssetSource>,
        out: &mut Vec<FxPlay>,
    ) {
        let surf = e.surf;
        let why = ev::event_name(e.event).unwrap_or("?").to_string();
        let mut play = |path: &str, at: SpawnAt| out.push(FxPlay::effect(path, at, why.clone()));
        let pos = e.pos;
        let normal = byte_to_dir(e.parm);
        let impacts = self.impacts.as_ref();
        let row = |kind: usize| impacts.and_then(|t| t.get(kind, surf).map(String::from));
        match e.event {
            // The normal effect along the surface normal, the reflect
            // effect along `es.scale`'s direction (cod2-efx.md 6.2).
            ev::EV_SHOTGUN_HIT | ev::EV_BULLET_HIT_SMALL | ev::EV_BULLET_HIT_LARGE => {
                let kind = match e.event {
                    ev::EV_BULLET_HIT_SMALL => SMALL,
                    ev::EV_BULLET_HIT_LARGE => LARGE,
                    _ => SHOTGUN,
                };
                let reflect = byte_to_dir(e.scale);
                if let Some(p) = row(kind) {
                    play(&p, SpawnAt::Directed { pos, dir: normal });
                }
                if let Some(p) = row(kind + 1) {
                    play(&p, SpawnAt::Directed { pos, dir: reflect });
                }
            }
            ev::EV_GRENADE_BOUNCE => {
                if let Some(p) = row(GRENADE_BOUNCE) {
                    play(&p, SpawnAt::Directed { pos, dir: normal });
                }
            }
            ev::EV_GRENADE_EXPLODE
            | ev::EV_ROCKET_EXPLODE
            | ev::EV_ROCKET_EXPLODE_NOMARKS
            | ev::EV_CUSTOM_EXPLODE => {
                let kind = match e.event {
                    ev::EV_GRENADE_EXPLODE => Some(GRENADE_EXPLODE),
                    ev::EV_CUSTOM_EXPLODE => None,
                    _ => Some(ROCKET_EXPLODE),
                };
                if let Some(p) = kind.and_then(row) {
                    play(&p, SpawnAt::Directed { pos, dir: normal });
                }
                // The weapon's own explosion on top.
                let w = self.weapon(cs, fs, e.weapon);
                if let Some(p) = w.explosion {
                    out.push(FxPlay::effect(
                        p,
                        SpawnAt::Directed { pos, dir: normal },
                        ev::event_name(e.event).unwrap_or("?"),
                    ));
                }
            }
            // The default surface's grenade explosion, straight up.
            ev::EV_GRENADE_SUICIDE => {
                if let Some(p) = impacts.and_then(|t| t.get(GRENADE_EXPLODE, 0)) {
                    play(p, SpawnAt::Directed { pos, dir: Vec3::Z });
                }
            }
            // `es.eventParm` is the effect id, the entity's angles its
            // forward and up.
            ev::EV_PLAY_FX => match cs.effect(e.parm) {
                Some(p) if (1..64).contains(&e.parm) => {
                    let (fwd, _, up) = angle_vectors(e.angles);
                    play(
                        p,
                        SpawnAt::Oriented {
                            pos,
                            axis: [fwd, up.cross(fwd), up],
                        },
                    );
                }
                _ => self.bad_id(e.parm),
            },
            _ => {}
        }
    }

    /// The tracer `CG_BulletHitEvent` (Mac 0x1db1f4) and
    /// `CG_BulletHitClientEvent` (0x1d9b78) spawn after the hit: from the
    /// shooter's muzzle (`otherEntityNum`, `CG_CalcMuzzlePoint` on
    /// `tag_flash`) to the hit, never for the first-person body's own
    /// rounds, else with `cg_tracerchance` (cod2-efx.md 6.7).
    fn tracer(
        &mut self,
        e: &Event2,
        snap: &SnapshotView,
        muzzles: &HashMap<u32, Vec3>,
        frametime: i32,
        out: &mut Vec<FxPlay>,
    ) {
        if !matches!(
            e.event,
            ev::EV_SHOTGUN_HIT
                | ev::EV_BULLET_HIT_SMALL
                | ev::EV_BULLET_HIT_LARGE
                | ev::EV_BULLET_HIT_CLIENT_SMALL
                | ev::EV_BULLET_HIT_CLIENT_LARGE
        ) || cod2_tracer::CHANCE <= 0.0
        {
            return;
        }
        let shooter = e.other_entity;
        // A body or a gun drawn this frame starts at its tag (a turret
        // round's world impact names the gun, so its tracer leaves the
        // barrel and the gunner sees his own; cod2-turrets.md 12), any
        // other in the snapshot at the tagless fallback; none without one.
        let muzzle = u32::try_from(shooter)
            .ok()
            .and_then(|n| muzzles.get(&n).copied())
            .or_else(|| muzzle_point(snap, shooter, Vec3::ZERO));
        let Some(muzzle) = muzzle else {
            return;
        };
        if snap.ps.pm_flags & PMF_FIRST_PERSON != 0 && shooter == snap.ps.client_num {
            return;
        }
        let rng = self.rng.get_or_insert_with(|| Rng::new(0x7261_6365));
        if rng.next_f32() >= cod2_tracer::CHANCE {
            return;
        }
        // `(rand() % cg.frametime) / 2`.
        let back_ms = match frametime {
            0 => 0,
            ft => (rng.next_u64() % ft as u64) as i32 / 2,
        };
        out.push(FxPlay {
            play: Play::Tracer {
                muzzle,
                hit: e.pos,
                back_ms,
            },
            why: ev::event_name(e.event).unwrap_or("?").into(),
        });
    }

    fn bad_id(&mut self, id: i32) {
        if self.warned.insert(id) {
            log::warn!("cod2: CG_PlayFx called with invalid effect id {id}");
        }
    }

    /// `CG_ProcessEntity`'s `ET_FX` arm (Mac 0x164bd4): the timer, the
    /// cull, then the play along `origin2` with `eventParm`'s up.
    fn looped(&mut self, e: &EntityView, frame: &Frame, view: Vec3, out: &mut Vec<FxPlay>) {
        let raw = &e.raw;
        let time = frame.time_ms;
        let period = raw.f32("angles2[1]") as i32;
        let last = self.loops.entry(e.number).or_insert(0);
        if time < *last {
            *last = time;
        } else {
            if time - *last < period {
                return;
            }
            if period > 0 {
                *last += (time - *last) / period * period;
            } else {
                *last = time;
            }
        }
        let origin = frame.entity_origin(e);
        let cull = raw.f32("angles2[0]");
        if cull != 0.0 && origin.distance_squared(view) >= cull * cull {
            return;
        }
        let id = raw.i32("scale");
        let Some(path) = cs_effect(frame.configstrings, id) else {
            self.bad_id(id);
            return;
        };
        let fwd = Vec3::new(
            raw.f32("origin2[0]"),
            raw.f32("origin2[1]"),
            raw.f32("origin2[2]"),
        );
        let up = raw.i32("eventParm");
        let at = if fwd.length_squared() == 0.0 {
            SpawnAt::Point { pos: origin }
        } else if up == 0 {
            SpawnAt::Directed {
                pos: origin,
                dir: fwd,
            }
        } else {
            // The up made square to the forward.
            let u = byte_to_dir(up);
            let u = (u - fwd * u.dot(fwd)).normalize_or_zero();
            SpawnAt::Oriented {
                pos: origin,
                axis: [fwd, u.cross(fwd), u],
            }
        };
        out.push(FxPlay::effect(path, at, format!("ET_FX {}", e.number)));
    }
}

/// `entity` is the playerstate's first-person body (`pm_flags` 0xc00000),
/// whose gun is the view weapon.
fn first_person(snap: &SnapshotView, entity: u32) -> bool {
    snap.ps.pm_flags & PMF_FIRST_PERSON != 0 && entity == snap.ps.client_num as u32
}

fn cs_effect(cs: &ConfigStrings, id: i32) -> Option<&str> {
    (1..64).contains(&id).then(|| cs.effect(id)).flatten()
}

/// `AngleVectors`: forward, right, up of (pitch, yaw, roll) degrees.
fn angle_vectors(a: Vec3) -> (Vec3, Vec3, Vec3) {
    let (sp, cp) = a.x.to_radians().sin_cos();
    let (sy, cy) = a.y.to_radians().sin_cos();
    let (sr, cr) = a.z.to_radians().sin_cos();
    let fwd = Vec3::new(cp * cy, cp * sy, -sp);
    let right = Vec3::new(-sr * sp * cy + cr * sy, -sr * sp * sy - cr * cy, -sr * cp);
    let up = Vec3::new(cr * sp * cy + sr * sy, cr * sp * sy - sr * cy, cr * cp);
    (fwd, right, up)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::play::cod2_events::EventTracker;
    use std::collections::BTreeMap;
    use vcod_cod2::net::capture::{Capture, Dir};
    use vcod_cod2::net::oob::{self, Reply};
    use vcod_cod2::net::{Client, NetView};
    use vcod_common::pk3::Pk3Fs;

    const FIXTURE: &[u8] = include_bytes!("../../../cod2/tests/fixtures/mp_toujane-spectate.bin");

    /// The retail spectator session (crates/cod2/tests/replay.rs) through
    /// the client's view, the session's [`EventTracker`] and [`Cod2Fx`]:
    /// every effect played, and every event the tracker raised.
    fn replay() -> (Vec<FxPlay>, Vec<Event2>) {
        let cap = Capture::decode(FIXTURE).unwrap();
        let (mut challenge, mut client) = (None, None);
        let mut view = NetView::default();
        let mut tracker = EventTracker::default();
        let mut fx = Cod2Fx::default();
        let mut plays = Vec::new();
        let mut events = Vec::new();
        for (dir, p) in &cap.packets {
            if let Some(pl) = oob::payload(p) {
                match (dir, oob::parse_reply(pl)) {
                    (Dir::FromServer, Reply::Challenge(c)) => challenge = Some(c),
                    (Dir::ToServer, _) => {
                        let text = String::from_utf8_lossy(pl);
                        if let Some(q) = text.strip_prefix("connect \"").and_then(|ui| {
                            oob::info_value_for_key(ui.trim_end_matches('"'), "qport")
                        }) {
                            client = Some(Client::new(q.parse().unwrap(), challenge.unwrap()));
                        }
                    }
                    _ => {}
                }
                continue;
            }
            let (Dir::FromServer, Some(c)) = (dir, client.as_mut()) else {
                continue;
            };
            let Some(parsed) = c.receive(p).unwrap() else {
                continue;
            };
            view.update(&c.parser, &parsed);
            if parsed.snapshot.is_none() {
                continue;
            }
            let Some(snap) = &view.snap else {
                continue;
            };
            // The cgame's current snapshot is the older of the two.
            let cur = view.prev.as_ref().unwrap_or(snap);
            let new = tracker.transition(cur);
            let f = view.frame(snap.server_time).unwrap();
            let eye = Vec3::from(f.eye());
            let none = HashMap::new();
            let scene = FxScene {
                muzzles: &none,
                tags: &HashMap::new(),
                brass: true,
            };
            plays.extend(fx.frame(&f, &new, eye, None, scene));
            events.extend(new);
        }
        (plays, events)
    }

    #[test]
    fn the_spectate_capture_plays_its_looped_effects() {
        let (plays, events) = replay();
        // The one event the capture carries (`play::cod2_sound`'s replay
        // plays its step) is no effect's.
        let names: Vec<_> = events.iter().map(|e| (e.entity, e.event)).collect();
        assert_eq!(names, [(0, ev::EV_FOOTSTEP_RUN + 6)]);
        let mut by: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
        for p in &plays {
            by.entry(p.why.as_str())
                .or_default()
                .push(p.path().unwrap());
        }
        for (why, paths) in &by {
            println!("{why}: {paths:?}");
        }
        // mp_toujane's 19 `playLoopedFx` entities each play on first sight.
        let loops: Vec<_> = by.keys().filter(|w| w.starts_with("ET_FX ")).collect();
        assert_eq!(loops.len(), 19, "{loops:?}");
        // And again on their periods across the 23 snapshots.
        assert!(loops.iter().all(|w| by[*w].len() >= 2));
        assert!(plays.iter().all(|p| p.path().unwrap().starts_with("fx/")));
    }

    /// A small round on concrete plays the table's normal effect along the
    /// surface normal and its reflect effect along `es.scale`'s direction.
    #[test]
    fn a_bullet_hit_plays_its_table_row() {
        let mut fx = Cod2Fx::default();
        let mut t = ImpactTable {
            rows: vec![Default::default(); IMPACT_KINDS.len()],
        };
        t.read_csv(
            "# a comment,,\nbullet_small_normal,concrete,fx/impacts/small_concrete.efx\n\
             bullet_small_reflect,concrete,\nbullet_large_normal,concrete,fx/impacts/large.efx\n",
        );
        fx.impacts = Some(t);
        let e = Event2 {
            entity: 70,
            event: ev::EV_BULLET_HIT_SMALL,
            parm: 5,
            etype: ev::ET_EVENTS + ev::EV_BULLET_HIT_SMALL,
            eflags: 0,
            weapon: 0,
            surf: 5,
            other_entity: 0,
            time: 0,
            scale: 0,
            angles2: [0.0; 2],
            pos: Vec3::new(1.0, 2.0, 3.0),
            angles: Vec3::ZERO,
            from_ps: false,
        };
        let cs = ConfigStrings::default();
        let mut out = Vec::new();
        fx.event(&e, &cs, None, &mut out);
        // The reflect cell is blank: one play.
        assert_eq!(out.len(), 1);
        match &out[0].play {
            Play::Effect {
                path,
                at: SpawnAt::Directed { pos, dir },
            } => {
                assert_eq!(path, "fx/impacts/small_concrete.efx");
                let (pos, dir) = (*pos, *dir);
                assert_eq!(pos, Vec3::new(1.0, 2.0, 3.0));
                assert_eq!(dir, byte_to_dir(5));
            }
            other => panic!("{other:?}"),
        }
        // 183 is the large row.
        let large = Event2 {
            event: ev::EV_BULLET_HIT_LARGE,
            ..e
        };
        out.clear();
        fx.event(&large, &cs, None, &mut out);
        assert_eq!(out[0].path(), Some("fx/impacts/large.efx"));
    }

    /// Another player's round spawns a tracer from his `tag_flash` to the
    /// hit at `cg_tracerchance`'s 0.4, back-dated under half a frame; the
    /// first-person body's own rounds never spawn one, and a shooter with
    /// no body spawns none.
    #[test]
    fn bullet_hits_spawn_tracers_at_retails_rate() {
        use vcod_cod2::net::{EntityView, PlayerView};
        let mut ps = PlayerView::new(&Default::default(), 0);
        ps.pm_flags = PMF_FIRST_PERSON;
        ps.client_num = 0;
        let shooter = vcod_cod2::net::EntityState2::null(3);
        let snap = SnapshotView {
            message_num: 1,
            server_time: 1000,
            snap_flags: 0,
            ps,
            entities: vec![EntityView::new(&shooter)],
            clients: Vec::new(),
        };
        let tag = Vec3::new(10.0, 20.0, 50.0);
        let muzzles = HashMap::from([(3u32, tag)]);
        let hit = |other: i32, event: i32| Event2 {
            entity: 80,
            event,
            parm: 0,
            etype: ev::ET_EVENTS + event,
            eflags: 0,
            weapon: 0,
            surf: 5,
            other_entity: other,
            time: 0,
            scale: 0,
            angles2: [0.0; 2],
            pos: Vec3::new(2000.0, 20.0, 50.0),
            angles: Vec3::ZERO,
            from_ps: false,
        };
        let mut fx = Cod2Fx::default();
        let n = 20_000;
        let mut drawn = 0;
        for i in 0..n {
            let event = [
                ev::EV_BULLET_HIT_SMALL,
                ev::EV_BULLET_HIT_LARGE,
                ev::EV_SHOTGUN_HIT,
                ev::EV_BULLET_HIT_CLIENT_SMALL,
                ev::EV_BULLET_HIT_CLIENT_LARGE,
            ][i % 5];
            let mut out = Vec::new();
            fx.tracer(&hit(3, event), &snap, &muzzles, 16, &mut out);
            for p in out {
                let Play::Tracer {
                    muzzle,
                    hit,
                    back_ms,
                } = p.play
                else {
                    panic!("{p:?}");
                };
                assert_eq!((muzzle, hit), (tag, Vec3::new(2000.0, 20.0, 50.0)));
                assert!((0..8).contains(&back_ms), "{back_ms}");
                drawn += 1;
            }
        }
        let rate = drawn as f32 / n as f32;
        assert!((rate - cod2_tracer::CHANCE).abs() < 0.015, "{rate}");
        // The view's own rounds, a shooter in no snapshot, and an event
        // that is no bullet hit spawn nothing.
        for e in [
            hit(0, ev::EV_BULLET_HIT_SMALL),
            hit(9, ev::EV_BULLET_HIT_SMALL),
            hit(3, ev::EV_GRENADE_BOUNCE),
        ] {
            let mut out = Vec::new();
            for _ in 0..50 {
                fx.tracer(&e, &snap, &HashMap::new(), 16, &mut out);
            }
            assert!(out.is_empty(), "{:?}", e.other_entity);
        }
        // Without a drawn body the stance height stands in: 60 standing.
        let mut out = Vec::new();
        while out.is_empty() {
            fx.tracer(
                &hit(3, ev::EV_BULLET_HIT_SMALL),
                &snap,
                &HashMap::new(),
                0,
                &mut out,
            );
        }
        assert!(matches!(
            out[0].play,
            Play::Tracer { muzzle, back_ms: 0, .. } if muzzle == Vec3::new(0.0, 0.0, 60.0)
        ));
        // Spawned, it flies at 4500 u/s with the stock material.
        let mut sys = FxSystem::new();
        out[0].spawn(&mut sys, &Pk3Fs::empty(), 0.0);
        let q = sys.build_quads(Vec3::new(0.0, -500.0, 0.0), Vec3::X, Vec3::Z, 0.1);
        assert_eq!(q.len(), 1);
        assert_eq!(&*q[0].shader, cod2_tracer::MATERIAL);
        let xs: Vec<f32> = q[0].verts.iter().map(|v| v[0]).collect();
        assert!(
            (xs[2] - 450.0).abs() < 0.5 && (xs[0] - 610.0).abs() < 0.5,
            "{xs:?}"
        );
    }

    /// A drawn player's shot flashes his `worldFlashEffect` on his
    /// `tag_flash` and ejects his shell eject (the last shot's on the last
    /// round) on his `tag_brass`; a bolt action ejects at
    /// `EV_EJECT_BRASS` instead; the playerstate's shot goes to the view
    /// weapon's tags, and the first-person body's own entity flashes
    /// nothing; no flash on a turret, no brass under `cg_brass 0` or off a
    /// temp entity, nothing for weapon 0 or an undrawn body.
    #[test]
    fn shots_flash_and_eject_brass_on_their_tags() {
        use vcod_cod2::net::PlayerView;
        let mut ps = PlayerView::new(&Default::default(), 0);
        ps.pm_flags = PMF_FIRST_PERSON;
        ps.client_num = 0;
        ps.weapon = 1;
        let snap = SnapshotView {
            message_num: 1,
            server_time: 1000,
            snap_flags: 0,
            ps,
            entities: Vec::new(),
            clients: Vec::new(),
        };
        let pose = |x: f32| TagPose {
            origin: Vec3::new(x, 0.0, 50.0),
            axis: [Vec3::X, Vec3::Y, Vec3::Z],
        };
        let tags = HashMap::from([(
            3u32,
            BodyTags {
                flash: Some(pose(30.0)),
                brass: Some(pose(10.0)),
            },
        )]);
        let muzzles = HashMap::new();
        let scene = FxScene {
            muzzles: &muzzles,
            tags: &tags,
            brass: true,
        };
        let mut fx = Cod2Fx::default();
        let file = |bolt_action| WeaponFx {
            view_flash: Some("fx/view.efx".into()),
            world_flash: Some("fx/world.efx".into()),
            shell_eject: Some("fx/shell.efx".into()),
            last_shot_eject: Some("fx/last.efx".into()),
            bolt_action,
            ..Default::default()
        };
        fx.weapons.insert(1, file(false));
        fx.weapons.insert(2, file(true));
        let cs = ConfigStrings::default();
        let ev = |entity: u32, event: i32, weapon: i32| Event2 {
            entity,
            event,
            parm: 0,
            etype: ET_PLAYER,
            eflags: 0,
            weapon,
            surf: 0,
            other_entity: 0,
            time: 0,
            scale: 0,
            angles2: [0.0; 2],
            pos: Vec3::ZERO,
            angles: Vec3::ZERO,
            from_ps: false,
        };
        let mut run = |e: Event2, scene: FxScene| {
            let mut out = Vec::new();
            fx.weapon_fx(&e, &snap, &cs, None, scene, &mut out);
            out.into_iter()
                .map(|p| match p.play {
                    Play::OnTag { path, bolt, pose } => {
                        format!(
                            "{path} on {} {} at {}",
                            bolt.entity, bolt.slot, pose.origin.x
                        )
                    }
                    Play::View { path, tag, weapon } => format!("{path} on view {tag:?} {weapon}"),
                    other => panic!("{other:?}"),
                })
                .collect::<Vec<_>>()
        };
        let (world, shell) = ("fx/world.efx on 3 -1 at 30", "fx/shell.efx on 3 -2 at 10");
        assert_eq!(run(ev(3, ev::EV_FIRE_WEAPON, 1), scene), [world, shell]);
        assert_eq!(
            run(ev(3, ev::EV_FIRE_WEAPON_LASTSHOT, 1), scene),
            [world, "fx/last.efx on 3 -2 at 10"]
        );
        assert_eq!(
            run(ev(3, ev::EV_FIRE_QUADBARREL_1, 1), scene),
            [world, shell, shell]
        );
        assert_eq!(run(ev(3, ev::EV_FIRE_WEAPON, 2), scene), [world]);
        assert_eq!(run(ev(3, ev::EV_EJECT_BRASS, 2), scene), [shell]);
        let own = Event2 {
            from_ps: true,
            ..ev(0, ev::EV_FIRE_WEAPON, 1)
        };
        assert_eq!(
            run(own, scene),
            [
                "fx/view.efx on view Flash 1",
                "fx/shell.efx on view Brass 1"
            ]
        );
        assert_eq!(
            run(ev(0, ev::EV_FIRE_WEAPON, 1), scene),
            ["fx/shell.efx on view Brass 1"]
        );
        let gunner = Event2 {
            eflags: EF_TURRET_STANCE,
            ..ev(3, ev::EV_FIRE_WEAPON, 1)
        };
        assert_eq!(run(gunner, scene), [shell]);
        let no_brass = FxScene {
            brass: false,
            ..scene
        };
        assert_eq!(run(ev(3, ev::EV_FIRE_WEAPON, 1), no_brass), [world]);
        let temp = Event2 {
            etype: ET_EVENTS + ev::EV_EJECT_BRASS,
            ..ev(3, ev::EV_EJECT_BRASS, 1)
        };
        assert!(run(temp, scene).is_empty());
        assert!(run(ev(3, ev::EV_FIRE_WEAPON, 0), scene).is_empty());
        assert!(run(ev(5, ev::EV_FIRE_WEAPON, 1), scene).is_empty());
        // A turret's shot flashes its own tag, the view's flash for the
        // gun the view is locked on.
        let turret = Event2 {
            etype: ev::ET_TURRET,
            ..ev(3, ev::EV_FIRE_WEAPON_MG42, 1)
        };
        assert_eq!(run(turret, scene), [world, shell]);
        let mut raw = vcod_cod2::net::PlayerState2::default();
        let field =
            |n| vcod_cod2::net::delta::field_index(&vcod_cod2::net::fields::PLAYER_STATE_FIELDS, n);
        raw.f[field("viewlocked_entNum")] = 3;
        raw.f[field("eFlags")] = EF_TURRET_STANCE as u32;
        let gunner = SnapshotView {
            ps: PlayerView::new(&raw, 0),
            ..snap.clone()
        };
        let mut out = Vec::new();
        fx.weapon_fx(&turret, &gunner, &cs, None, scene, &mut out);
        assert_eq!(out[0].path(), Some("fx/view.efx"));
    }

    /// The stock table names an effect the fx system loads in every cell
    /// it fills.
    #[test]
    fn the_stock_impact_table_loads() {
        let Some(fs) = vcod_common::testing::cod2_fs() else {
            return;
        };
        let t = ImpactTable::load(&fs, Some("mp_toujane"));
        assert_eq!(t.get(SMALL, 5), Some("fx/impacts/small_concrete.efx"));
        let mut filled = 0;
        for (k, row) in t.rows.iter().enumerate() {
            for (s, cell) in row.iter().enumerate() {
                if let Some(p) = cell {
                    filled += 1;
                    let loaded = crate::fx::cod2::load(&fs, p);
                    assert!(
                        loaded.is_ok(),
                        "{} {}: {p}: {:?}",
                        IMPACT_KINDS[k],
                        SURFACES[s],
                        loaded.err()
                    );
                }
            }
        }
        println!("{filled} impact cells");
        // 139 filled rows in iw_impacts.csv, one bullet_ap_normal surface twice.
        assert_eq!(filled, 138);
    }
}
