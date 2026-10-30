//! Event to visual-effect resolution: the MP `EV_*` ids joined with
//! `fx/iw_impacts.csv`. docs/research/cod11-events-and-fx.md sections 1 and 5a.

use crate::fx::sim::SpawnAt;
use glam::Vec3;
use std::collections::HashMap;
use std::sync::{LazyLock, PoisonError, RwLock};
use vcod_common::asset_source::{Asset, AssetKind, AssetSource};
use vcod_common::net::events::{GameEvent, byte_to_dir};
use vcod_common::net::flags::{EF_CROUCH, EF_IN_VEHICLE_V22, EF_PRONE, ET_TURRET, ET_VEHICLE_V22};
use vcod_common::net::protocol::{CsLayout, CsRange};
use vcod_common::pmove::{VIEW_CROUCH, VIEW_PRONE, VIEW_STAND};
use vcod_common::weapon::WeaponDef;

pub use vcod_common::net::event_ids::*;

/// `surfType` index order (doc section 4); also the csv's surface column.
const SURFACE_NAMES: [&str; 23] = [
    "default", "bark", "brick", "carpet", "cloth", "concrete", "dirt", "flesh", "foliage", "glass",
    "grass", "gravel", "ice", "metal", "mud", "paper", "plaster", "rock", "sand", "snow", "water",
    "wood", "asphalt",
];

/// United Offensive's 24th csv column, after the 23 surfaces: what an
/// explode on a vehicle plays (`CG_EntityEvent`, uo cgame 0x300233c0).
const SURF_VEHICLE: u8 = 23;

/// The csv's impact-type literals (docs/research/efx-grammar.md, 6a), 1.1's
/// nine and United Offensive's (`uo_cgame_mp_x86.dll` 0x3001dfb0's list of
/// 22, docs/research/coduo-diff.md section 8).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
enum ImpactKind {
    BulletSmallNormal,
    BulletSmallReflect,
    BulletLargeNormal,
    BulletLargeReflect,
    GrenadeBounce,
    GrenadeExplode,
    RocketExplode,
    MolotovExplodeNormal,
    MolotovExplodeReflect,
    /// `(ammoType row, reflect)`: UO's bullet hits, by the shooter's
    /// weapon's `ammoType` (smg 0, pistol 1, rifle 2, lmg 3, hmg 4, umg 5).
    Bullet(u8, bool),
    /// UO's projectile explodes by `projExplosionType` (grenade 0, smoke 1,
    /// rocket 2, molotov 3, artillery 4, mortar 5, tank 6, b17 7).
    Explode(u8),
}

/// UO's bullet rows in `ammoType` order.
const UO_BULLET_ROWS: [&str; 6] = ["smg", "pistol", "rifle", "lmg", "hmg", "umg"];

/// UO's explode rows in `projExplosionType` order; molotov's is its
/// `_normal` row.
const UO_EXPLODE_ROWS: [&str; 8] = [
    "grenade_explode",
    "smoke_grenade_explode",
    "rocket_explode",
    "molotov_explode_normal",
    "artillery_explode",
    "mortar_explode",
    "tank_explode",
    "b17_explode",
];

impl ImpactKind {
    fn parse(s: &str) -> Option<ImpactKind> {
        if let Some(i) = UO_EXPLODE_ROWS.iter().position(|r| *r == s) {
            // 1.1's own literals keep their own kinds; both games' files
            // name grenade, rocket and molotov the same.
            return Some(match i {
                0 => ImpactKind::GrenadeExplode,
                2 => ImpactKind::RocketExplode,
                3 => ImpactKind::MolotovExplodeNormal,
                i => ImpactKind::Explode(i as u8),
            });
        }
        if let Some(rest) = s.strip_prefix("bullet_")
            && let Some((class, side)) = rest.split_once('_')
            && let Some(row) = UO_BULLET_ROWS.iter().position(|r| *r == class)
        {
            return match side {
                "normal" => Some(ImpactKind::Bullet(row as u8, false)),
                "reflect" => Some(ImpactKind::Bullet(row as u8, true)),
                _ => None,
            };
        }
        Some(match s {
            "bullet_small_normal" => ImpactKind::BulletSmallNormal,
            "bullet_small_reflect" => ImpactKind::BulletSmallReflect,
            "bullet_large_normal" => ImpactKind::BulletLargeNormal,
            "bullet_large_reflect" => ImpactKind::BulletLargeReflect,
            "grenade_bounce" => ImpactKind::GrenadeBounce,
            "molotov_explode_reflect" => ImpactKind::MolotovExplodeReflect,
            _ => return None,
        })
    }

    /// A UO explode row by `projExplosionType`; 8 (`none`) and anything
    /// past it has none.
    fn explode(kind: i32) -> Option<ImpactKind> {
        Some(match kind {
            0 => ImpactKind::GrenadeExplode,
            2 => ImpactKind::RocketExplode,
            3 => ImpactKind::MolotovExplodeNormal,
            1 | 4..=7 => ImpactKind::Explode(kind as u8),
            _ => return None,
        })
    }
}

/// `(impact type, surfType)` to `.efx` path. Absent means no effect.
#[derive(Default)]
struct ImpactTable {
    map: HashMap<(ImpactKind, u8), String>,
}

impl ImpactTable {
    fn get(&self, kind: ImpactKind, surf: u8) -> Option<&str> {
        self.map.get(&(kind, surf)).map(String::as_str)
    }

    /// Format: docs/research/efx-grammar.md section 6a. Duplicate rows agree
    /// in stock data, so last write wins, as a later file's do. Unknown
    /// literals are skipped so a mod's extra rows do not break the rest.
    fn add_csv(&mut self, text: &str) {
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') || line.starts_with("\"#") {
                continue;
            }
            let mut cols = line.splitn(3, ',');
            let (Some(kind_s), Some(surf_s)) = (cols.next(), cols.next()) else {
                continue;
            };
            let path = cols.next().unwrap_or("").trim();
            let Some(kind) = ImpactKind::parse(kind_s.trim()) else {
                continue;
            };
            let surf = match surf_s.trim() {
                "vehicle" => SURF_VEHICLE,
                s => match SURFACE_NAMES.iter().position(|n| *n == s) {
                    Some(i) => i as u8,
                    None => continue,
                },
            };
            if path.is_empty() {
                self.map.remove(&(kind, surf));
            } else {
                self.map.insert((kind, surf), path.to_string());
            }
        }
    }
}

#[cfg(test)]
fn parse_impacts_csv(text: &str) -> ImpactTable {
    let mut table = ImpactTable::default();
    table.add_csv(text);
    table
}

/// Every `fx/*.csv`, in name order, a later file overriding an earlier
/// one's rows (`CG_RegisterImpactEffects`, uo cgame 0x3001dfb0, which lists
/// the directory and sorts it). 1.1's paks ship only `fx/iw_impacts.csv`;
/// UO's add `fx/gmi_impacts.csv` ahead of their own `iw_impacts.csv`.
/// Built through [`Asset::find`], uncached: [`init`] keeps the table.
impl Asset for ImpactTable {
    const KIND: AssetKind = AssetKind::ImpactEffects;

    fn find(src: &dyn AssetSource, _: &str) -> anyhow::Result<Self> {
        let files: Vec<String> = src
            .list("fx", ".csv")
            .into_iter()
            .filter(|n| {
                n.strip_prefix("fx/")
                    .is_some_and(|rest| !rest.contains('/'))
            })
            .collect();
        let mut table = ImpactTable::default();
        for name in &files {
            if let Some(bytes) = src.read(name) {
                table.add_csv(&String::from_utf8_lossy(&bytes));
            }
        }
        if files.is_empty() {
            log::warn!("fx: no fx/*.csv; bullet impacts will have no visual");
        }
        Ok(table)
    }
}

fn load(fs: &dyn AssetSource) -> ImpactTable {
    ImpactTable::find(fs, "").unwrap_or_default()
}

static TABLE: LazyLock<RwLock<ImpactTable>> = LazyLock::new(RwLock::default);

/// Loads the table off `fs`: at startup, and again whenever a download
/// reopens the search path. `resolve()` reads an empty table before the first.
pub fn init(fs: &dyn AssetSource) {
    *TABLE.write().unwrap_or_else(PoisonError::into_inner) = load(fs);
}

/// `Known` is a recognized event with nothing to draw. `Unknown` is an id this
/// build does not recognize; the F3 overlay counts those.
#[derive(Debug)]
pub enum Resolved {
    Spawn {
        path: String,
        at: SpawnAt,
        /// `EV_ROCKET_EXPLODE_NOMARKS`: the effect places no impact mark
        /// (cgame 0x3001e59f sets the flag `CG_ImpactMark` tests at 0x3002357f).
        nomarks: bool,
        /// The `ctx.muzzles` key of the tag it plays on, which its
        /// `relative` elements ride (`CG_MuzzleFlash`, syscall 0xe5).
        bolt: Option<u32>,
    },
    /// `EV_PLAY_FX_ON_TAG`: `path` on `entity`'s `tag`, which the caller
    /// poses ([`crate::fx::sim::FxSystem::spawn_on_tag`]). `entity` is
    /// `u32::MAX` on the playerState ring, as `GameEvent` has it; `slot`
    /// is the event's parm, the bolt's key.
    OnTag {
        path: String,
        entity: u32,
        tag: String,
        slot: i32,
    },
    /// Retail draws tracers as a hardcoded quad, not an `.efx`
    /// (docs/research/efx-grammar.md R8); see [`crate::fx::sim::FxSystem::spawn_tracer`].
    Tracer {
        muzzle: Vec3,
        impact: Vec3,
        /// `surfType == 7`, which retail draws as a one-frame segment.
        flesh: bool,
        /// A United Offensive lmg, hmg or umg round (`ammoType` 3..5), the
        /// `*lmg` tracer cvars' (uo cgame 0x30048d60).
        heavy: bool,
        /// Whether the tracer chance still has to be rolled; a server's
        /// `EV_BULLET_TRACER` comes rolled.
        roll: bool,
    },
    Known,
    Unknown,
}

pub struct ResolveCtx<'a> {
    /// Entity number to (world muzzle position, forward) this frame. Missing
    /// means the shooter's model has not loaded yet.
    pub muzzles: &'a HashMap<u32, (Vec3, Vec3)>,
    /// Per player body with no `tag_flash` this frame:
    /// [`muzzle_fallback`], where its tracers start ([`shot_origin`]).
    pub shot_fallbacks: &'a HashMap<u32, Vec3>,
    /// 1-based CS7 weapon index to that weapon file's `worldFlashEffect`.
    /// Missing falls back to [`flash_effect_path`].
    pub weapon_flash: &'a HashMap<i32, String>,
    /// Configstring 7's weapons while a viewmodel is drawn, for the
    /// `viewFlashEffect` of playerState-ring fire events
    /// (`entity_num == u32::MAX`); `None` while none is drawn.
    pub view_flash: Option<&'a [Option<WeaponDef>]>,
    /// The shooter whose bullet hits draw no tracer; see [`view_body`].
    pub view_body: Option<u32>,
    /// `cg_blood`: off swaps a flesh hit's effect for
    /// [`FLESH_NOBLOOD`] (`CG_BulletHitWall`, cgame 0x30039679).
    pub blood: bool,
    /// Entity number to (world `tag_brass`, its forward) this frame;
    /// `u32::MAX` is the viewmodel's, as in `muzzles`.
    pub brass: &'a HashMap<u32, (Vec3, Vec3)>,
    /// Configstring 7's weapons, by the events' 1-based `weapon`.
    pub weapons: &'a [Option<WeaponDef>],
    /// `cg_brass`: off, `CG_EjectBrass` returns at once (cgame 0x30038a30).
    pub eject_brass: bool,
    /// The gamestate's table, for the effect paths `EV_PLAY_FX` names.
    pub configstrings: &'a [String],
    /// Where its blocks sit (`Protocol::cs`).
    pub cs: &'a CsLayout,
}

/// Every `.efx` a CoD1 level can play off its events: the impact table's
/// rows, the flash fallbacks, `FLESH_NOBLOOD`, the effect configstrings
/// configstring 7's weapons' flash, eject and explosion effects, and on
/// United Offensive the vehicles' wheel dust. The
/// gamestate preloads them (`FxSystem::preload`).
pub fn level_effects(
    configstrings: &[String],
    cs: &CsLayout,
    weapons: &[Option<WeaponDef>],
    uo: bool,
) -> Vec<String> {
    let mut out: Vec<String> = TABLE
        .read()
        .unwrap_or_else(PoisonError::into_inner)
        .map
        .values()
        .cloned()
        .collect();
    out.push(FLESH_NOBLOOD.into());
    out.push(flash_effect_path(EV_FIRE_WEAPON_MG42).into());
    out.push(flash_effect_path(0).into());
    out.extend(
        (1..128)
            .filter_map(|id| effect_path(configstrings, cs, id))
            .map(str::to_string),
    );
    for w in weapons.iter().flatten() {
        out.extend(
            [
                &w.world_flash_effect,
                &w.view_flash_effect,
                &w.shell_eject_effect,
                &w.last_shot_eject_effect,
                &w.proj_explosion_effect,
            ]
            .into_iter()
            .flatten()
            .filter(|p| !p.is_empty())
            .cloned(),
        );
    }
    if uo {
        out.extend(
            crate::fx::vehicle_trails::EFFECTS
                .iter()
                .map(|p| p.to_string()),
        );
    }
    out.sort();
    out.dedup();
    out
}

/// The `.efx` an effect id names: configstring `780 + id`, ids 1..63
/// (`CG_PlayFx`, cgame 0x3001db20, which prints an error and plays nothing
/// for any other id), United Offensive's 80-slot block past 917
/// (`CsLayout`). `None` for an id out of range or an empty slot.
pub fn effect_path<'a>(configstrings: &'a [String], cs: &CsLayout, id: i32) -> Option<&'a str> {
    let path = configstrings.get(cs.slot(CsRange::Effect, id)?)?;
    (!path.is_empty()).then_some(path.as_str())
}

/// `CG_PlayFxOnTag`'s read of configstring `844 + slot` (cgame
/// 0x3001db80): the effect id in the two leading digits, the tag after
/// them. Retail turns the two characters into an id without checking them
/// (0x3001dbb5..0x3001dbbf); `None` here for a short or non-digit string.
pub fn fx_tag<'a>(
    configstrings: &'a [String],
    layout: &CsLayout,
    slot: i32,
) -> Option<(i32, &'a str)> {
    let cs = configstrings.get(layout.fx_tags() + usize::try_from(slot).ok()?)?;
    let digits = cs.get(..2)?;
    let id = digits
        .bytes()
        .all(|b| b.is_ascii_digit())
        .then(|| digits.parse().ok())??;
    Some((id, &cs[2..]))
}

/// `CG_EjectBrass` (cgame 0x30038a30, docs/research/cod11-events-and-fx.md
/// 5d): the weapon's `lastShotEjectEffect` for `EV_FIRE_WEAPON_LASTSHOT`
/// when it has one, else its `shellEjectEffect`, on the shooter's
/// `tag_brass`. Nothing under `cg_brass 0`, for weapon 0, a weapon with
/// neither effect, or a shooter whose tag is not posed this frame.
fn eject_brass(ev: &GameEvent, ctx: &ResolveCtx) -> Option<Resolved> {
    if !ctx.eject_brass {
        return None;
    }
    let def = ctx
        .weapons
        .get(usize::try_from(ev.weapon).ok().filter(|&w| w != 0)?)?
        .as_ref()?;
    let path = def
        .last_shot_eject_effect
        .as_ref()
        .filter(|_| ev.event == EV_FIRE_WEAPON_LASTSHOT)
        .or(def.shell_eject_effect.as_ref())?;
    let &(pos, dir) = ctx.brass.get(&ev.entity_num)?;
    Some(Resolved::Spawn {
        path: path.clone(),
        at: SpawnAt::Directed { pos, dir },
        nomarks: false,
        bolt: None,
    })
}

/// `CG_FireWeapon` ejects a case on every shot of a weapon without
/// `boltAction` (cgame 0x30038ce6); a bolt-action's comes on
/// `EV_EJECT_BRASS` instead.
fn fire_ejects_brass(ev: &GameEvent, ctx: &ResolveCtx) -> bool {
    usize::try_from(ev.weapon)
        .ok()
        .and_then(|w| ctx.weapons.get(w)?.as_ref())
        .is_some_and(|def| !def.bolt_action)
}

/// The flesh hit `cg_blood 0` draws (docs/research/cod11-events-and-fx.md,
/// the surface type table).
pub const FLESH_NOBLOOD: &str = "fx/impacts/flesh_hit_noblood.efx";

/// `ps.pm_flags` bits that attach the view to a player body: 0x40000 our own
/// live one, 0x10000 a followed one (docs/research/cod11-events-and-fx.md 7).
const PMF_VIEW_BODY: i32 = 0x50000;

/// The body the view rides, whose shots `CG_Tracer` draws no tracer for
/// (docs/research/cod11-events-and-fx.md 6): `ps.clientNum` while `pm_flags`
/// carries [`PMF_VIEW_BODY`], else none.
pub fn view_body(pm_flags: i32, ps_client_num: i32) -> Option<u32> {
    if pm_flags & PMF_VIEW_BODY == 0 {
        return None;
    }
    u32::try_from(ps_client_num).ok()
}

/// `CG_CalcMuzzlePoint`'s point for a player body whose model has no
/// `tag_flash` (a gunner, whose weapon is not drawn): the snapshot's
/// `pos.trBase` raised by `bg_viewheight_prone`, `_crouched` or
/// `_standing` on the `eFlags` stance (cgame 0x30039440, 1.5 0x3003ab90,
/// UO 0x30048b60; docs/research/cod11-events-and-fx.md 6).
pub fn muzzle_fallback(tr_base: Vec3, eflags: i32) -> Vec3 {
    let eye = if eflags & EF_PRONE != 0 {
        VIEW_PRONE
    } else if eflags & EF_CROUCH != 0 {
        VIEW_CROUCH
    } else {
        VIEW_STAND
    };
    tr_base + Vec3::Z * eye
}

/// Where `CG_Tracer` and `CG_BulletWhizby` start a round of `shooter`'s:
/// its muzzle tag, else [`muzzle_fallback`]; `None` while it has no body.
pub fn shot_origin(
    muzzles: &HashMap<u32, (Vec3, Vec3)>,
    fallbacks: &HashMap<u32, Vec3>,
    shooter: u32,
) -> Option<Vec3> {
    muzzles
        .get(&shooter)
        .map(|&(pos, _)| pos)
        .or_else(|| fallbacks.get(&shooter).copied())
}

/// Class-based guess when `ctx.weapon_flash` has no entry. Every MG42/PTRS41
/// weapon file in pak0 uses `mg42hv.efx`; most others `standardflashworld.efx`.
fn flash_effect_path(event: i32) -> &'static str {
    match event {
        EV_FIRE_WEAPON_MG42 | EV_FIRE_QUADBARREL_1 | EV_FIRE_QUADBARREL_2 => {
            "fx/muzzleflashes/mg42hv.efx"
        }
        _ => "fx/muzzleflashes/standardflashworld.efx",
    }
}

/// An empty spawn list is still a recognized event.
fn known_or(spawns: Vec<Resolved>) -> Vec<Resolved> {
    if spawns.is_empty() {
        vec![Resolved::Known]
    } else {
        spawns
    }
}

/// ids 1-138. No visual; vcod draws no footstep dust.
fn is_footstep_group(event: i32) -> bool {
    const BASES: [i32; 6] = [
        EV_FOOTSTEP_RUN_BASE,
        EV_FOOTSTEP_WALK_BASE,
        EV_FOOTSTEP_PRONE_BASE,
        EV_JUMP_BASE,
        EV_LANDING_BASE,
        EV_LANDING_PAIN_BASE,
    ];
    BASES
        .iter()
        .any(|&base| (base..base + SURFACE_GROUP_SIZE).contains(&event))
}

/// The tag a vehicle's fire event flashes on (`CG_FireWeapon`, uo cgame
/// 0x30047eb3): `tag_altfire` for `EV_FIRE_WEAPONB`, `tag_secondary_flash`
/// for the gunner's `EV_FIRE_WEAPONC`, else `tag_flash`.
fn vehicle_flash_tag(event: i32) -> (&'static str, i32) {
    match event {
        EV_FIRE_WEAPONB => ("tag_altfire", BOLT_ALTFIRE),
        EV_UO_FIRE_WEAPONC => ("tag_secondary_flash", BOLT_SECONDARY_FLASH),
        _ => ("tag_flash", BOLT_FLASH),
    }
}

/// A vehicle's muzzle flash, the fired weapon's `worldFlashEffect` on the
/// tag [`vehicle_flash_tag`] names (0x30045c10).
fn vehicle_flash(ev: &GameEvent, ctx: &ResolveCtx) -> Option<Resolved> {
    let (tag, slot) = vehicle_flash_tag(ev.event);
    let weapon = fire_weapon(ev);
    let path = weapon_def(ctx, weapon)
        .and_then(|d| d.world_flash_effect.clone())
        .filter(|p| !p.is_empty())
        .or_else(|| ctx.weapon_flash.get(&weapon).cloned())?;
    Some(Resolved::OnTag {
        path,
        entity: ev.entity_num,
        tag: tag.to_string(),
        slot,
    })
}

/// A United Offensive round the client traces itself (docs/research/
/// coduo-diff.md 19, "Client-traced rounds"): a vehicle's or a turret's
/// fire event with parm bit 7. `CG_FireWeapon` hands the bit to the flash
/// (uo cgame 0x30047ef6), which traces from the flash's tag once the flash
/// plays (0x30045c80), so a round whose flash does not play is not traced.
/// 1.1's turrets send parm 0, so the bit gates the protocol too.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ClientRound {
    pub entity: u32,
    pub weapon: i32,
    pub tag: &'static str,
    /// The tag `CG_BulletHitEvent` draws a hit's tracer from: on a vehicle
    /// the gunner's `tag_secondary_flash`, else `tag_altfire`; on a turret
    /// its `tag_flash` (0x30048ee0..0x30048f5a).
    pub tracer_tag: &'static str,
}

pub fn client_round(ev: &GameEvent, ctx: &ResolveCtx) -> Option<ClientRound> {
    if ev.parm & 0x80 == 0 {
        return None;
    }
    let fires = matches!(
        ev.event,
        EV_FIRE_WEAPON
            | EV_FIRE_WEAPONB
            | EV_FIRE_WEAPON_LASTSHOT
            | EV_FIRE_WEAPON_MG42
            | EV_FIRE_QUADBARREL_1
            | EV_FIRE_QUADBARREL_2
    );
    if ev.e_type == ET_TURRET && fires {
        let flash = weapon_def(ctx, fire_weapon(ev))
            .and_then(|d| d.world_flash_effect.as_ref())
            .or_else(|| ctx.weapon_flash.get(&fire_weapon(ev)));
        flash?;
        return Some(ClientRound {
            entity: ev.entity_num,
            weapon: fire_weapon(ev),
            tag: "tag_flash",
            tracer_tag: "tag_flash",
        });
    }
    if !(fires && ev.e_type == ET_VEHICLE_V22 || ev.event == EV_UO_FIRE_WEAPONC) {
        return None;
    }
    vehicle_flash(ev, ctx)?;
    let (tag, _) = vehicle_flash_tag(ev.event);
    Some(ClientRound {
        entity: ev.entity_num,
        weapon: fire_weapon(ev),
        tag,
        tracer_tag: match tag {
            "tag_secondary_flash" => tag,
            _ => "tag_altfire",
        },
    })
}

/// The weapon a fire event fires: United Offensive's `CG_FireWeapon` (uo
/// cgame 0x30047d20) takes the parm's low seven bits when they are set, else
/// `es.weapon`. Applied to vehicles and the gunner's event, the only
/// carriers whose parm was read for it here.
pub fn fire_weapon(ev: &GameEvent) -> i32 {
    let parm = ev.parm & 0x7f;
    if parm >= 1 && (ev.e_type == ET_VEHICLE_V22 || ev.event == EV_UO_FIRE_WEAPONC) {
        parm
    } else {
        ev.weapon
    }
}

/// An lmg, hmg or umg round, whose tracer takes the `*lmg` cvars (uo cgame
/// 0x30045600, 0x30048d9a: `ammoType` 3, 4 or 5).
pub fn heavy_round(def: &WeaponDef) -> bool {
    matches!(def.ammo_type.as_str(), "lmg" | "hmg" | "umg")
}

fn weapon_def<'a>(ctx: &'a ResolveCtx, index: i32) -> Option<&'a WeaponDef> {
    ctx.weapons
        .get(usize::try_from(index).ok().filter(|&w| w != 0)?)?
        .as_ref()
}

/// The bolt slots of the tags vcod plays on by name rather than through a
/// `playFxOnTag` configstring; [`bolt_tag`] names them. Negative, clear of
/// the configstring slots.
const BOLT_FLASH: i32 = -1;
const BOLT_ALTFIRE: i32 = -2;
const BOLT_SECONDARY_FLASH: i32 = -3;
const BOLT_BODY: i32 = -4;
/// The tag a smoke grenade's cloud rides (uo cgame 0x3002337a).
const BODY_TAG: &str = "g_body";

/// The tag a bolt slot names: a `playFxOnTag` configstring for a slot from
/// 0 up ([`fx_tag`]), one of vcod's fixed tags below.
pub fn bolt_tag<'a>(configstrings: &'a [String], layout: &CsLayout, slot: i32) -> Option<&'a str> {
    match slot {
        BOLT_FLASH => Some("tag_flash"),
        BOLT_ALTFIRE => Some("tag_altfire"),
        BOLT_SECONDARY_FLASH => Some("tag_secondary_flash"),
        BOLT_BODY => Some(BODY_TAG),
        _ => fx_tag(configstrings, layout, slot).map(|(_, tag)| tag),
    }
}

/// The muzzle flash of a fire event, on the shooter itself: `entity_num`,
/// not `other_entity_num`. No muzzle entry yet drops the flash rather than
/// misplacing it.
fn fire_flash(ev: &GameEvent, ctx: &ResolveCtx) -> Vec<Resolved> {
    match ctx.muzzles.get(&ev.entity_num) {
        Some(&(pos, dir)) => {
            let path = match ctx.view_flash.filter(|_| ev.entity_num == u32::MAX) {
                // First person draws the view flash or nothing
                // (`CG_MuzzleFlash`, doc section 5b).
                Some(weapons) => usize::try_from(ev.weapon)
                    .ok()
                    .and_then(|i| weapons.get(i)?.as_ref()?.view_flash_effect.clone()),
                // A loaded weapon file with no `worldFlashEffect` flashes
                // nothing (a grenade, binoculars); the class guess stands in
                // only for a weapon not loaded.
                None => match weapon_def(ctx, ev.weapon) {
                    Some(def) => def.world_flash_effect.clone(),
                    None => Some(
                        ctx.weapon_flash
                            .get(&ev.weapon)
                            .cloned()
                            .unwrap_or_else(|| flash_effect_path(ev.event).to_string()),
                    ),
                },
            };
            match path {
                Some(path) => vec![Resolved::Spawn {
                    path,
                    at: SpawnAt::Directed { pos, dir },
                    nomarks: false,
                    bolt: Some(ev.entity_num),
                }],
                None => Vec::new(),
            }
        }
        None => Vec::new(),
    }
}

/// [`resolve`] against `fs`'s impact table rather than the one [`init`]
/// loaded, which tests running beside each other would race on.
#[cfg(test)]
pub(crate) fn resolve_in(fs: &dyn AssetSource, ev: &GameEvent, ctx: &ResolveCtx) -> Vec<Resolved> {
    resolve_with(ev, &load(fs), ctx)
}

/// Resolve one drained [`GameEvent`] against the cached table and this
/// frame's muzzles.
pub fn resolve(ev: &GameEvent, ctx: &ResolveCtx) -> Vec<Resolved> {
    let table = TABLE.read().unwrap_or_else(PoisonError::into_inner);
    resolve_with(ev, &table, ctx)
}

fn resolve_with(ev: &GameEvent, table: &ImpactTable, ctx: &ResolveCtx) -> Vec<Resolved> {
    match ev.event {
        EV_NONE => vec![Resolved::Known],

        // eventParm is the byte-dir surface normal; `ev.dir` (origin2) is zero
        // on these events (doc section 2). Retail draws the tracer from here,
        // in `CG_BulletHitWall` and `CG_BulletHitFlesh`, not from the fire
        // event: `EV_BULLET_TRACER` has no MP case (doc section 6).
        // `other_entity_num` is the shooter. The client-hit pair is the
        // victim's copy of a flesh hit and carries no effect of its own.
        EV_BULLET_HIT_SMALL
        | EV_BULLET_HIT_LARGE
        | EV_BULLET_HIT_CLIENT_SMALL
        | EV_BULLET_HIT_CLIENT_LARGE => {
            let mut out = Vec::new();
            let kind = match ev.event {
                EV_BULLET_HIT_SMALL => Some(ImpactKind::BulletSmallNormal),
                EV_BULLET_HIT_LARGE => Some(ImpactKind::BulletLargeNormal),
                _ => None,
            };
            let surf = ev.surf_type.clamp(0, SURFACE_NAMES.len() as i32 - 1) as u8;
            let impact = Vec3::from(ev.pos);
            let flesh = SURFACE_NAMES.get(surf as usize) == Some(&"flesh");
            let path = kind.and_then(|kind| match flesh && !ctx.blood {
                true => Some(FLESH_NOBLOOD),
                false => table.get(kind, surf),
            });
            if let Some(path) = path {
                out.push(Resolved::Spawn {
                    path: path.to_string(),
                    at: SpawnAt::Surface {
                        pos: impact,
                        normal: Vec3::from(byte_to_dir(ev.parm)),
                    },
                    nomarks: false,
                    bolt: None,
                });
            }
            // Every surface type, flesh included (doc section 6). Not
            // `fx/tagged/tracers.efx`, which is the AA gun's tracer. Chance
            // roll, distance gate and geometry live in `FxSystem::spawn_tracer`.
            let muzzle = shot_origin(ctx.muzzles, ctx.shot_fallbacks, ev.other_entity_num)
                .filter(|_| ctx.view_body != Some(ev.other_entity_num));
            if let Some(muzzle_pos) = muzzle {
                out.push(Resolved::Tracer {
                    muzzle: muzzle_pos,
                    impact,
                    flesh,
                    heavy: false,
                    roll: true,
                });
            }
            known_or(out)
        }

        // A flash, and a spent case unless the weapon works a bolt.
        EV_FIRE_WEAPON
        | EV_FIRE_WEAPONB
        | EV_FIRE_WEAPON_LASTSHOT
        | EV_FIRE_WEAPON_MG42
        | EV_FIRE_QUADBARREL_1
        | EV_FIRE_QUADBARREL_2 => {
            if ev.e_type == ET_VEHICLE_V22 {
                return known_or(vehicle_flash(ev, ctx).into_iter().collect());
            }
            let mut out = fire_flash(ev, ctx);
            if fire_ejects_brass(ev, ctx) {
                out.extend(eject_brass(ev, ctx));
            }
            known_or(out)
        }

        EV_EJECT_BRASS => known_or(eject_brass(ev, ctx).into_iter().collect()),

        // eventParm is the surface normal here too (doc section 2). `_NOMARKS`
        // only suppresses the decal, so both rocket ids read the same csv row.
        // `grenade_bounce` is blank in stock data and resolves to `Known`.
        // The weapon's own `projExplosionEffect` is not spawned; `weapon.rs`
        // does not parse that key.
        EV_GRENADE_BOUNCE | EV_GRENADE_EXPLODE | EV_ROCKET_EXPLODE | EV_ROCKET_EXPLODE_NOMARKS => {
            let kind = match ev.event {
                EV_GRENADE_BOUNCE => ImpactKind::GrenadeBounce,
                EV_GRENADE_EXPLODE => ImpactKind::GrenadeExplode,
                _ => ImpactKind::RocketExplode,
            };
            let surf = ev.surf_type.clamp(0, SURFACE_NAMES.len() as i32 - 1) as u8;
            let mut out = Vec::new();
            if let Some(path) = table.get(kind, surf) {
                out.push(Resolved::Spawn {
                    path: path.to_string(),
                    at: SpawnAt::Surface {
                        pos: Vec3::from(ev.pos),
                        normal: Vec3::from(byte_to_dir(ev.parm)),
                    },
                    nomarks: ev.event == EV_ROCKET_EXPLODE_NOMARKS,
                    bolt: None,
                });
            }
            known_or(out)
        }

        // `playFx` (docs/research/cod11-events-and-fx.md 2): the effect at the
        // temp entity, on syscall 0xe3's up axis or, for `EV_PLAY_FX_DIR`,
        // on the forward `es.scale` carries as a byte dir (0x3001e6ce).
        EV_PLAY_FX | EV_PLAY_FX_DIR => {
            let Some(path) = effect_path(ctx.configstrings, ctx.cs, ev.parm) else {
                return vec![Resolved::Known];
            };
            let pos = Vec3::from(ev.pos);
            let at = match ev.event {
                EV_PLAY_FX_DIR => SpawnAt::Directed {
                    pos,
                    dir: Vec3::from(byte_to_dir(ev.scale)),
                },
                _ => SpawnAt::Point { pos },
            };
            vec![Resolved::Spawn {
                path: path.to_string(),
                at,
                nomarks: false,
                bolt: None,
            }]
        }

        // `playFxOnTag` (docs/research/cod11-events-and-fx.md 2): the tag
        // pair at configstring `844 + parm`, played where the caller poses
        // the tag on the event's own entity.
        EV_PLAY_FX_ON_TAG => {
            let Some((path, tag)) = fx_tag(ctx.configstrings, ctx.cs, ev.parm)
                .and_then(|(id, tag)| Some((effect_path(ctx.configstrings, ctx.cs, id)?, tag)))
            else {
                return vec![Resolved::Known];
            };
            vec![Resolved::OnTag {
                path: path.to_string(),
                entity: ev.entity_num,
                tag: tag.to_string(),
                slot: ev.parm,
            }]
        }

        // United Offensive's server-rolled tracer (case 0x30023172, `EV_BULLET_TRACER`
        // in 1.1's numbering): `trBase` to `origin2` through 0x30048260, the
        // weapon in the parm. No 1.1 server raises it, and 1.1's cgame has no
        // case for it (doc section 6).
        EV_BULLET_TRACER => {
            let end = Vec3::from(ev.dir);
            if end == Vec3::ZERO {
                return vec![Resolved::Known];
            }
            vec![Resolved::Tracer {
                muzzle: Vec3::from(ev.pos),
                impact: end,
                flesh: false,
                heavy: weapon_def(ctx, ev.parm).is_some_and(heavy_round),
                roll: false,
            }]
        }

        // United Offensive's bullet hit (pre-event 0x30048e60): the normal
        // and the reflected effect of the shooter's weapon's `ammoType` row,
        // the reflection on the byte dir `es.scale` carries; the tracer as
        // 1.1's, none for a grenade's or a projectile's.
        EV_UO_BULLET_HIT => {
            let def = weapon_def(ctx, ev.weapon);
            let row = def.and_then(|d| UO_BULLET_ROWS.iter().position(|r| *r == d.ammo_type));
            let surf = ev.surf_type.clamp(0, SURFACE_NAMES.len() as i32 - 1) as u8;
            let impact = Vec3::from(ev.pos);
            let flesh = SURFACE_NAMES.get(surf as usize) == Some(&"flesh");
            let mut out = Vec::new();
            if let Some(row) = row {
                let (normal, reflect) = match flesh && !ctx.blood {
                    true => (Some(FLESH_NOBLOOD), None),
                    false => (
                        table.get(ImpactKind::Bullet(row as u8, false), surf),
                        table.get(ImpactKind::Bullet(row as u8, true), surf),
                    ),
                };
                for (path, dir) in [(normal, ev.parm), (reflect, ev.scale)] {
                    if let Some(path) = path {
                        out.push(Resolved::Spawn {
                            path: path.to_string(),
                            at: SpawnAt::Surface {
                                pos: impact,
                                normal: Vec3::from(byte_to_dir(dir)),
                            },
                            nomarks: false,
                            bolt: None,
                        });
                    }
                }
            }
            let tracer =
                !def.is_some_and(|d| matches!(d.weapon_type.as_str(), "grenade" | "projectile"));
            let muzzle = shot_origin(ctx.muzzles, ctx.shot_fallbacks, ev.other_entity_num)
                .filter(|_| tracer && ctx.view_body != Some(ev.other_entity_num));
            if let Some(muzzle_pos) = muzzle {
                out.push(Resolved::Tracer {
                    muzzle: muzzle_pos,
                    impact,
                    flesh,
                    heavy: def.is_some_and(heavy_round),
                    roll: true,
                });
            }
            known_or(out)
        }

        // United Offensive's explodes (`CG_EntityEvent` 0x30023355): the
        // impact table's row for the explosion type in `es.scale` (smoke's
        // on the grenade's `g_body` tag, an explode on a vehicle from the
        // row's vehicle column when it has one), then the weapon's own
        // `projExplosionEffect`. Type 8, `none`, skips the row.
        EV_UO_PROJECTILE_EXPLODE | EV_UO_PROJECTILE_EXPLODE_NOMARKS => {
            let nomarks = ev.event == EV_UO_PROJECTILE_EXPLODE_NOMARKS;
            let surf = ev.surf_type.clamp(0, SURFACE_NAMES.len() as i32 - 1) as u8;
            let at = SpawnAt::Surface {
                pos: Vec3::from(ev.pos),
                normal: Vec3::from(byte_to_dir(ev.parm)),
            };
            let mut out = Vec::new();
            if ev.scale == 1 {
                if let Some(path) = table.get(ImpactKind::Explode(1), surf) {
                    out.push(Resolved::OnTag {
                        path: path.to_string(),
                        entity: ev.entity_num,
                        tag: BODY_TAG.to_string(),
                        slot: BOLT_BODY,
                    });
                }
            } else if let Some(kind) = ImpactKind::explode(ev.scale) {
                let path = table
                    .get(kind, SURF_VEHICLE)
                    .filter(|_| ev.eflags & EF_IN_VEHICLE_V22 != 0)
                    .or_else(|| table.get(kind, surf));
                if let Some(path) = path {
                    out.push(Resolved::Spawn {
                        path: path.to_string(),
                        at,
                        nomarks,
                        bolt: None,
                    });
                }
            }
            if let Some(path) =
                weapon_def(ctx, ev.weapon).and_then(|d| d.proj_explosion_effect.clone())
            {
                out.push(Resolved::Spawn {
                    path,
                    at,
                    nomarks,
                    bolt: None,
                });
            }
            known_or(out)
        }

        // The gunner's fire (`CG_FireWeapon` 0x30047d20): on a vehicle the
        // flash plays on the seat's tag.
        EV_UO_FIRE_WEAPONC => known_or(vehicle_flash(ev, ctx).into_iter().collect()),

        // Sounds only (`crate::audio::cues`), or nothing at all: the attached
        // fx pair and the attachments reach the jump table's default or its
        // no-op (uo cgame 0x30023564).
        EV_UO_FATIGUE_LAST_SOUND
        | EV_UO_FATIGUE_SOUND
        | EV_UO_DEPLOY_WEAPON
        | EV_UO_BREAKDOWN_WEAPON
        | EV_UO_GRENADE_SPOON
        | EV_UO_PROJECTILE_LAUNCH
        | EV_UO_PROJECTILE_INCOMING
        | EV_UO_PLAY_FX_ON_PLAYER
        | EV_UO_STOP_ATTACHED_FX
        | EV_UO_REMOVE_WEAPON_ATTACHMENTS
        | EV_UO_OVERHEATING
        | EV_UO_GRENADE_SUICIDE => vec![Resolved::Known],

        // Recognized, no visual.
        EV_FOLIAGE_SOUND
        | EV_STANCE_FORCE_STAND
        | EV_STANCE_FORCE_CROUCH
        | EV_STANCE_FORCE_PRONE
        | EV_STEP_VIEW
        | EV_WATER_TOUCH
        | EV_WATER_LEAVE
        | EV_ITEM_PICKUP
        | EV_ITEM_PICKUP_QUIET
        | EV_AMMO_PICKUP
        | EV_NOAMMO
        | EV_EMPTYCLIP
        | EV_RELOAD
        | EV_RELOAD_FROM_EMPTY
        | EV_RELOAD_START
        | EV_RELOAD_END
        | EV_RAISE_WEAPON
        | EV_PUTAWAY_WEAPON
        | EV_WEAPON_ALT
        | EV_PULLBACK_WEAPON
        | EV_RECHAMBER_WEAPON
        | EV_MELEE_SWIPE
        | EV_FIRE_MELEE
        | EV_MELEE_HIT
        | EV_MELEE_MISS
        | EV_SOUND_ALIAS
        | EV_MOLOTOV_EXPLODE
        | EV_MOLOTOV_EXPLODE_NOMARKS
        | EV_CUSTOM_EXPLODE
        | EV_CUSTOM_EXPLODE_NOMARKS
        | EV_RAILTRAIL
        | EV_BULLET
        | EV_PAIN
        | EV_CROUCH_PAIN
        | EV_DEATH
        | EV_DEBUG_LINE
        | EV_FLAMEBARREL_BOUNCE
        | EV_EARTHQUAKE
        | EV_DROPWEAPON
        | EV_ITEM_RESPAWN
        | EV_ITEM_POP
        | EV_PLAYER_TELEPORT_IN
        | EV_PLAYER_TELEPORT_OUT
        | EV_OBITUARY => vec![Resolved::Known],

        e if is_footstep_group(e) => vec![Resolved::Known],

        _ => vec![Resolved::Unknown],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::OnceLock;
    use vcod_common::net::events::BYTE_DIRS;
    use vcod_common::net::protocol::{CS_EFFECTS, CS_FX_TAGS};

    static EMPTY_BRASS: LazyLock<HashMap<u32, (Vec3, Vec3)>> = LazyLock::new(HashMap::new);

    fn ev(event: i32, parm: i32, surf_type: i32, pos: [f32; 3]) -> GameEvent {
        GameEvent {
            event,
            parm,
            entity_num: 40,
            client_num: -1,
            weapon: 2,
            surf_type,
            pos,
            // origin2 is zero on bullet hits; the normal comes from `parm`.
            dir: [0.0, 0.0, 0.0],
            scale: 0,
            other_entity_num: 40,
            attacker_entity_num: -1,
            e_type: 0,
            eflags: 0,
        }
    }

    fn empty_ctx() -> ResolveCtx<'static> {
        static EMPTY_MUZZLES: OnceLock<HashMap<u32, (Vec3, Vec3)>> = OnceLock::new();
        static EMPTY_FLASH: OnceLock<HashMap<i32, String>> = OnceLock::new();
        static EMPTY_FALLBACKS: OnceLock<HashMap<u32, Vec3>> = OnceLock::new();
        ResolveCtx {
            brass: &EMPTY_BRASS,
            weapons: &[],
            eject_brass: true,
            configstrings: &[],
            cs: &vcod_common::net::protocol::CS_LAYOUT_V1,
            blood: true,
            muzzles: EMPTY_MUZZLES.get_or_init(HashMap::new),
            shot_fallbacks: EMPTY_FALLBACKS.get_or_init(HashMap::new),
            weapon_flash: EMPTY_FLASH.get_or_init(HashMap::new),
            view_flash: None,
            view_body: None,
        }
    }

    #[test]
    fn bullet_hit_resolves_to_surface_impact() {
        let mut table = ImpactTable::default();
        table.map.insert(
            (ImpactKind::BulletSmallNormal, 1),
            "fx/impacts/test.efx".to_string(),
        );
        let e = ev(EV_BULLET_HIT_SMALL, 0, 1, [10.0, 20.0, 30.0]);
        let rs = resolve_with(&e, &table, &empty_ctx());
        assert_eq!(rs.len(), 1, "{rs:?}"); // no shooter muzzle known: no tracer
        match &rs[0] {
            Resolved::Spawn {
                path,
                at: SpawnAt::Surface { pos, normal },
                ..
            } => {
                assert_eq!(path, "fx/impacts/test.efx");
                assert_eq!(*pos, Vec3::new(10.0, 20.0, 30.0));
                assert_eq!(*normal, Vec3::from(BYTE_DIRS[0]));
            }
            other => panic!("{other:?}"),
        }
    }

    /// `playFx`'s two events read the effect off configstring `780 + parm`;
    /// the plain one plays on the up axis, the `_DIR` one along the byte dir
    /// in `es.scale`. An id outside 1..63 or an empty slot plays nothing.
    #[test]
    fn play_fx_resolves_the_effect_configstring() {
        let mut cs = vec![String::new(); 2048];
        cs[CS_EFFECTS + 3] = "fx/explosions/mp_bomb.efx".to_string();
        let ctx = ResolveCtx {
            configstrings: &cs,
            cs: &vcod_common::net::protocol::CS_LAYOUT_V1,
            ..empty_ctx()
        };
        let table = ImpactTable::default();
        let plain = ev(EV_PLAY_FX, 3, 0, [1.0, 2.0, 3.0]);
        match &resolve_with(&plain, &table, &ctx)[..] {
            [
                Resolved::Spawn {
                    path,
                    at: SpawnAt::Point { pos },
                    bolt: None,
                    ..
                },
            ] => {
                assert_eq!(path, "fx/explosions/mp_bomb.efx");
                assert_eq!(*pos, Vec3::new(1.0, 2.0, 3.0));
            }
            other => panic!("{other:?}"),
        }
        let mut dir = ev(EV_PLAY_FX_DIR, 3, 0, [0.0; 3]);
        dir.scale = 5;
        match &resolve_with(&dir, &table, &ctx)[..] {
            [
                Resolved::Spawn {
                    at: SpawnAt::Directed { dir, .. },
                    ..
                },
            ] => {
                assert_eq!(*dir, Vec3::from(BYTE_DIRS[5]));
            }
            other => panic!("{other:?}"),
        }
        for parm in [0, 4, 64] {
            let e = ev(EV_PLAY_FX, parm, 0, [0.0; 3]);
            assert!(matches!(
                &resolve_with(&e, &table, &ctx)[..],
                [Resolved::Known]
            ));
        }
    }

    /// `EV_PLAY_FX_ON_TAG` reads its pair at `844 + parm`: the effect id
    /// in two digits, then the tag, on the event's own entity. A pair
    /// naming an empty effect slot or a malformed string plays nothing.
    #[test]
    fn play_fx_on_tag_reads_the_tag_pair() {
        let mut cs = vec![String::new(); 2048];
        cs[CS_EFFECTS + 3] = "fx/explosions/mp_bomb.efx".to_string();
        cs[CS_FX_TAGS + 1] = "03tag_helmet".to_string();
        cs[CS_FX_TAGS + 2] = "07tag_helmet".to_string();
        cs[CS_FX_TAGS + 3] = "x".to_string();
        let ctx = ResolveCtx {
            configstrings: &cs,
            cs: &vcod_common::net::protocol::CS_LAYOUT_V1,
            ..empty_ctx()
        };
        let table = ImpactTable::default();
        let e = ev(EV_PLAY_FX_ON_TAG, 1, 0, [0.0; 3]);
        match &resolve_with(&e, &table, &ctx)[..] {
            [
                Resolved::OnTag {
                    path,
                    entity,
                    tag,
                    slot,
                },
            ] => {
                assert_eq!(path, "fx/explosions/mp_bomb.efx");
                assert_eq!((*entity, tag.as_str(), *slot), (40, "tag_helmet", 1));
            }
            other => panic!("{other:?}"),
        }
        for parm in [2, 3, 4] {
            let e = ev(EV_PLAY_FX_ON_TAG, parm, 0, [0.0; 3]);
            assert!(matches!(
                &resolve_with(&e, &table, &ctx)[..],
                [Resolved::Known]
            ));
        }
    }

    #[test]
    fn bullet_hit_large_uses_the_large_column() {
        let mut table = ImpactTable::default();
        table.map.insert(
            (ImpactKind::BulletLargeNormal, 13),
            "fx/impacts/metalhit_large.efx".to_string(),
        );
        let e = ev(EV_BULLET_HIT_LARGE, 5, 13, [0.0, 0.0, 0.0]);
        match &resolve_with(&e, &table, &empty_ctx())[0] {
            Resolved::Spawn { path, .. } => assert_eq!(path, "fx/impacts/metalhit_large.efx"),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn blank_csv_cell_resolves_to_known_not_spawn() {
        let e = ev(EV_BULLET_HIT_SMALL, 0, 4, [0.0, 0.0, 0.0]);
        let rs = resolve_with(&e, &ImpactTable::default(), &empty_ctx());
        assert!(matches!(rs.as_slice(), [Resolved::Known]), "{rs:?}");
    }

    #[test]
    fn cg_blood_off_swaps_the_flesh_hit() {
        let mut table = ImpactTable::default();
        table.map.insert(
            (ImpactKind::BulletLargeNormal, 7),
            "fx/impacts/flesh_hit.efx".to_string(),
        );
        let e = ev(EV_BULLET_HIT_LARGE, 0, 7, [0.0, 0.0, 0.0]);
        let path = |blood| match &resolve_with(
            &e,
            &table,
            &ResolveCtx {
                brass: &EMPTY_BRASS,
                weapons: &[],
                eject_brass: true,
                configstrings: &[],
                cs: &vcod_common::net::protocol::CS_LAYOUT_V1,
                blood,
                ..empty_ctx()
            },
        )[0]
        {
            Resolved::Spawn { path, .. } => path.clone(),
            other => panic!("{other:?}"),
        };
        assert_eq!(path(true), "fx/impacts/flesh_hit.efx");
        assert_eq!(path(false), FLESH_NOBLOOD);
    }

    #[test]
    fn out_of_range_surf_type_clamps_instead_of_panicking() {
        let mut table = ImpactTable::default();
        table.map.insert(
            (ImpactKind::BulletSmallNormal, 22),
            "fx/impacts/small_concrete.efx".to_string(),
        );
        let e = ev(EV_BULLET_HIT_SMALL, 0, 255, [0.0, 0.0, 0.0]);
        match &resolve_with(&e, &table, &empty_ctx())[0] {
            Resolved::Spawn { path, .. } => assert_eq!(path, "fx/impacts/small_concrete.efx"),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn bullet_hit_spawns_a_tracer_from_the_shooters_muzzle() {
        let mut muzzles = HashMap::new();
        muzzles.insert(7u32, (Vec3::new(0.0, 0.0, 0.0), Vec3::X));
        let weapon_flash = HashMap::new();
        let ctx = ResolveCtx {
            brass: &EMPTY_BRASS,
            weapons: &[],
            eject_brass: true,
            configstrings: &[],
            cs: &vcod_common::net::protocol::CS_LAYOUT_V1,
            blood: true,
            muzzles: &muzzles,
            shot_fallbacks: &HashMap::new(),
            weapon_flash: &weapon_flash,
            view_flash: None,
            view_body: None,
        };
        let mut e = ev(EV_BULLET_HIT_SMALL, 0, 5, [10.0, 0.0, 0.0]); // concrete, blank cell
        e.other_entity_num = 7;
        let rs = resolve_with(&e, &ImpactTable::default(), &ctx);
        assert_eq!(rs.len(), 1, "{rs:?}"); // impact itself was a blank cell
        match &rs[0] {
            Resolved::Tracer {
                muzzle,
                impact,
                flesh,
                ..
            } => {
                assert_eq!(*muzzle, Vec3::ZERO);
                assert_eq!(*impact, Vec3::new(10.0, 0.0, 0.0));
                assert!(!flesh);
            }
            other => panic!("{other:?}"),
        }
    }

    /// A gunner's body has no `tag_flash` (his weapon is not drawn), so
    /// `CG_CalcMuzzlePoint` puts his round's start at `trBase` plus the
    /// stance's `bg_viewheight`: his world hits draw their tracer from
    /// there, and a fire event on him still flashes nothing.
    #[test]
    fn a_body_without_a_flash_tag_traces_from_its_stance_point() {
        let base = Vec3::new(100.0, 200.0, -23.875);
        assert_eq!(muzzle_fallback(base, 0), base + Vec3::Z * 60.0);
        assert_eq!(muzzle_fallback(base, EF_CROUCH), base + Vec3::Z * 40.0);
        assert_eq!(muzzle_fallback(base, EF_PRONE), base + Vec3::Z * 11.0);
        assert_eq!(
            muzzle_fallback(base, EF_PRONE | EF_CROUCH),
            base + Vec3::Z * 11.0
        );

        let fallbacks = HashMap::from([(7u32, muzzle_fallback(base, 0))]);
        let ctx = ResolveCtx {
            shot_fallbacks: &fallbacks,
            ..empty_ctx()
        };
        let mut e = ev(EV_BULLET_HIT_SMALL, 0, 5, [400.0, 200.0, 0.0]);
        e.other_entity_num = 7;
        match &resolve_with(&e, &ImpactTable::default(), &ctx)[..] {
            [Resolved::Tracer { muzzle, .. }] => assert_eq!(*muzzle, base + Vec3::Z * 60.0),
            r => panic!("{r:?}"),
        }
        let mut fire = ev(EV_FIRE_WEAPON, 0, 0, [0.0; 3]);
        fire.entity_num = 7;
        assert!(matches!(
            resolve_with(&fire, &ImpactTable::default(), &ctx)[..],
            [Resolved::Known]
        ));
        // A drawn tag wins over the stance point.
        let muzzles = HashMap::from([(7u32, (Vec3::ONE, Vec3::X))]);
        assert_eq!(shot_origin(&muzzles, &fallbacks, 7), Some(Vec3::ONE));
        assert_eq!(shot_origin(&muzzles, &fallbacks, 8), None);
    }

    #[test]
    fn flesh_hits_get_a_tracer() {
        let mut muzzles = HashMap::new();
        muzzles.insert(7u32, (Vec3::ZERO, Vec3::X));
        let weapon_flash = HashMap::new();
        let ctx = ResolveCtx {
            brass: &EMPTY_BRASS,
            weapons: &[],
            eject_brass: true,
            configstrings: &[],
            cs: &vcod_common::net::protocol::CS_LAYOUT_V1,
            blood: true,
            muzzles: &muzzles,
            shot_fallbacks: &HashMap::new(),
            weapon_flash: &weapon_flash,
            view_flash: None,
            view_body: None,
        };
        let mut e = ev(EV_BULLET_HIT_SMALL, 0, 7, [10.0, 0.0, 0.0]);
        e.other_entity_num = 7;
        let rs = resolve_with(&e, &ImpactTable::default(), &ctx);
        assert!(
            rs.iter()
                .any(|r| matches!(r, Resolved::Tracer { flesh: true, .. })),
            "{rs:?}"
        );
    }

    /// `CG_Tracer` draws nothing for the body the view rides, ours or a
    /// followed one, and still resolves the impact.
    #[test]
    fn the_viewed_bodys_hits_draw_no_tracer() {
        let mut muzzles = HashMap::new();
        muzzles.insert(3u32, (Vec3::ZERO, Vec3::X));
        muzzles.insert(7u32, (Vec3::ZERO, Vec3::X));
        let weapon_flash = HashMap::new();
        let mut table = ImpactTable::default();
        table.map.insert(
            (ImpactKind::BulletSmallNormal, 5),
            "fx/impacts/small_concrete.efx".to_string(),
        );
        let resolve_for = |pm_flags, ps_client, shooter| {
            let ctx = ResolveCtx {
                brass: &EMPTY_BRASS,
                weapons: &[],
                eject_brass: true,
                configstrings: &[],
                cs: &vcod_common::net::protocol::CS_LAYOUT_V1,
                blood: true,
                muzzles: &muzzles,
                shot_fallbacks: &HashMap::new(),
                weapon_flash: &weapon_flash,
                view_flash: None,
                view_body: view_body(pm_flags, ps_client),
            };
            let mut e = ev(EV_BULLET_HIT_SMALL, 0, 5, [10.0, 0.0, 0.0]);
            e.other_entity_num = shooter;
            resolve_with(&e, &table, &ctx)
        };
        let tracer = |rs: &[Resolved]| rs.iter().any(|r| matches!(r, Resolved::Tracer { .. }));
        for (pm_flags, what) in [(0x40000, "playing"), (0x30000, "following")] {
            let rs = resolve_for(pm_flags, 3, 3);
            assert!(!tracer(&rs), "{what}: {rs:?}");
            assert!(matches!(rs.as_slice(), [Resolved::Spawn { .. }]), "{rs:?}");
            assert!(
                tracer(&resolve_for(pm_flags, 3, 7)),
                "{what}: another shooter"
            );
        }
        // A free spectator's `ps.clientNum` is its own, and the view rides no body.
        assert!(tracer(&resolve_for(0x20000, 3, 3)));
    }

    /// `CG_BulletHitFlesh` hands `CG_Tracer` the same shooter, impact and
    /// `surfType` the wall path does and spawns no effect.
    #[test]
    fn client_hit_draws_the_flesh_tracer_and_no_effect() {
        let mut muzzles = HashMap::new();
        muzzles.insert(7u32, (Vec3::new(0.0, 0.0, 60.0), Vec3::X));
        let weapon_flash = HashMap::new();
        let ctx = ResolveCtx {
            brass: &EMPTY_BRASS,
            weapons: &[],
            eject_brass: true,
            configstrings: &[],
            cs: &vcod_common::net::protocol::CS_LAYOUT_V1,
            blood: true,
            muzzles: &muzzles,
            shot_fallbacks: &HashMap::new(),
            weapon_flash: &weapon_flash,
            view_flash: None,
            view_body: None,
        };
        let mut table = ImpactTable::default();
        for kind in [ImpactKind::BulletSmallNormal, ImpactKind::BulletLargeNormal] {
            table
                .map
                .insert((kind, 7), "fx/impacts/flesh_hit.efx".to_string());
        }
        for id in [EV_BULLET_HIT_CLIENT_SMALL, EV_BULLET_HIT_CLIENT_LARGE] {
            let mut e = ev(id, 0, 7, [300.0, 0.0, 50.0]);
            e.other_entity_num = 7;
            match resolve_with(&e, &table, &ctx).as_slice() {
                [
                    Resolved::Tracer {
                        muzzle,
                        impact,
                        flesh: true,
                        ..
                    },
                ] => {
                    assert_eq!(*muzzle, Vec3::new(0.0, 0.0, 60.0));
                    assert_eq!(*impact, Vec3::new(300.0, 0.0, 50.0));
                }
                other => panic!("event {id}: {other:?}"),
            }
            // A shooter with no muzzle this frame: `CG_GetMuzzlePoint` fails.
            e.other_entity_num = 8;
            let rs = resolve_with(&e, &table, &ctx);
            assert!(matches!(rs.as_slice(), [Resolved::Known]), "{rs:?}");
        }
    }

    #[test]
    fn footstep_and_landing_groups_are_known_not_unknown() {
        for id in [
            EV_FOOTSTEP_RUN_BASE,
            EV_FOOTSTEP_RUN_BASE + 22,
            EV_JUMP_BASE,
            EV_LANDING_PAIN_BASE + 22,
        ] {
            let e = ev(id, 0, 0, [0.0, 0.0, 0.0]);
            let rs = resolve_with(&e, &ImpactTable::default(), &empty_ctx());
            assert!(
                matches!(rs.as_slice(), [Resolved::Known]),
                "event {id}: {rs:?}"
            );
        }
    }

    #[test]
    fn named_non_visual_events_are_known_not_unknown() {
        // Against an empty table the explode ids miss the csv and collapse
        // to Known too.
        for id in [
            EV_GRENADE_BOUNCE,
            EV_GRENADE_EXPLODE,
            EV_ROCKET_EXPLODE,
            EV_OBITUARY,
        ] {
            let e = ev(id, 0, 0, [0.0, 0.0, 0.0]);
            let rs = resolve_with(&e, &ImpactTable::default(), &empty_ctx());
            assert!(
                matches!(rs.as_slice(), [Resolved::Known]),
                "event {id}: {rs:?}"
            );
        }
    }

    #[test]
    fn fire_event_with_no_muzzle_entry_resolves_known() {
        for id in [EV_FIRE_WEAPON, EV_FIRE_WEAPON_MG42] {
            let e = ev(id, 0, 0, [0.0, 0.0, 0.0]);
            let rs = resolve_with(&e, &ImpactTable::default(), &empty_ctx());
            assert!(
                matches!(rs.as_slice(), [Resolved::Known]),
                "event {id}: {rs:?}"
            );
        }
    }

    #[test]
    fn unrecognized_event_id_is_unknown() {
        let e = ev(9999, 0, 0, [0.0, 0.0, 0.0]);
        let rs = resolve_with(&e, &ImpactTable::default(), &empty_ctx());
        assert!(matches!(rs.as_slice(), [Resolved::Unknown]), "{rs:?}");
    }

    #[test]
    fn csv_comment_and_blank_lines_are_skipped() {
        let text = concat!(
            "# This file maps impact types to effects,,\n",
            "\"# quoted comment, with a comma\",,\n",
            "\n",
            "bullet_small_normal,brick,fx/impacts/small_brick.efx\n",
        );
        let table = parse_impacts_csv(text);
        assert_eq!(
            table.get(ImpactKind::BulletSmallNormal, 2),
            Some("fx/impacts/small_brick.efx")
        );
    }

    #[test]
    fn duplicate_rows_and_missing_effect_column_are_handled() {
        let text = concat!(
            "bullet_small_normal,bark,fx/impacts/default_hit.efx\n",
            "bullet_small_normal,bark,fx/impacts/default_hit.efx\n",
            "bullet_small_reflect,bark\n", // column omitted entirely
            "grenade_bounce,gravel,\n",    // trailing comma, nothing after
        );
        let table = parse_impacts_csv(text);
        assert_eq!(
            table.get(ImpactKind::BulletSmallNormal, 1),
            Some("fx/impacts/default_hit.efx")
        );
        assert_eq!(table.get(ImpactKind::BulletSmallReflect, 1), None);
        assert_eq!(table.get(ImpactKind::GrenadeBounce, 11), None);
    }

    #[test]
    fn fire_event_spawns_a_flash_at_the_muzzle() {
        let mut muzzles = HashMap::new();
        muzzles.insert(7u32, (Vec3::new(1.0, 2.0, 3.0), Vec3::X));
        let weapon_flash = HashMap::new(); // no per-weapon path: class fallback
        let ev = GameEvent {
            event: EV_FIRE_WEAPON,
            parm: 0,
            entity_num: 7,
            client_num: 3,
            weapon: 2,
            surf_type: 0,
            pos: [0.0; 3],
            dir: [0.0; 3],
            scale: 0,
            other_entity_num: 0,
            attacker_entity_num: -1,
            e_type: 0,
            eflags: 0,
        };
        let rs = resolve(
            &ev,
            &ResolveCtx {
                brass: &EMPTY_BRASS,
                weapons: &[],
                eject_brass: true,
                configstrings: &[],
                cs: &vcod_common::net::protocol::CS_LAYOUT_V1,
                blood: true,
                muzzles: &muzzles,
                shot_fallbacks: &HashMap::new(),
                weapon_flash: &weapon_flash,
                view_flash: None,
                view_body: None,
            },
        );
        assert_eq!(rs.len(), 1, "{rs:?}");
        match &rs[0] {
            Resolved::Spawn {
                path,
                at: SpawnAt::Directed { pos, dir },
                ..
            } => {
                assert_eq!(path, "fx/muzzleflashes/standardflashworld.efx");
                assert_eq!(*pos, Vec3::new(1.0, 2.0, 3.0));
                assert_eq!(*dir, Vec3::X);
            }
            other => panic!("{other:?}"),
        }
    }

    /// playerState-ring fire events carry `entity_num == u32::MAX`; `main.rs`
    /// inserts a synthetic muzzle under that key.
    #[test]
    fn view_marker_fire_event_resolves_to_a_directed_flash_at_the_synthetic_muzzle() {
        let mut muzzles = HashMap::new();
        muzzles.insert(u32::MAX, (Vec3::new(5.0, 6.0, 7.0), Vec3::Y));
        let weapon_flash = HashMap::new();
        let ev = GameEvent {
            event: EV_FIRE_WEAPON,
            parm: 0,
            entity_num: u32::MAX,
            client_num: 3,
            weapon: 2,
            surf_type: 0,
            pos: [0.0; 3],
            dir: [0.0; 3],
            scale: 0,
            other_entity_num: u32::MAX,
            attacker_entity_num: -1,
            e_type: 0,
            eflags: 0,
        };
        let rs = resolve(
            &ev,
            &ResolveCtx {
                brass: &EMPTY_BRASS,
                weapons: &[],
                eject_brass: true,
                configstrings: &[],
                cs: &vcod_common::net::protocol::CS_LAYOUT_V1,
                blood: true,
                muzzles: &muzzles,
                shot_fallbacks: &HashMap::new(),
                weapon_flash: &weapon_flash,
                view_flash: None,
                view_body: None,
            },
        );
        assert_eq!(rs.len(), 1, "{rs:?}");
        match &rs[0] {
            Resolved::Spawn {
                at: SpawnAt::Directed { pos, dir },
                ..
            } => {
                assert_eq!(*pos, Vec3::new(5.0, 6.0, 7.0));
                assert_eq!(*dir, Vec3::Y);
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn fire_event_prefers_the_weapons_own_flash_over_the_class_fallback() {
        let mut muzzles = HashMap::new();
        muzzles.insert(7u32, (Vec3::ZERO, Vec3::X));
        let mut weapon_flash = HashMap::new();
        weapon_flash.insert(5, "fx/muzzleflashes/thompson.efx".to_string()); // weapon index 5 = thompson_mp
        let ctx = ResolveCtx {
            brass: &EMPTY_BRASS,
            weapons: &[],
            eject_brass: true,
            configstrings: &[],
            cs: &vcod_common::net::protocol::CS_LAYOUT_V1,
            blood: true,
            muzzles: &muzzles,
            shot_fallbacks: &HashMap::new(),
            weapon_flash: &weapon_flash,
            view_flash: None,
            view_body: None,
        };

        let mut thompson = ev(EV_FIRE_WEAPON, 0, 0, [0.0; 3]);
        thompson.entity_num = 7;
        thompson.weapon = 5;
        match &resolve_with(&thompson, &ImpactTable::default(), &ctx)[0] {
            Resolved::Spawn { path, .. } => assert_eq!(path, "fx/muzzleflashes/thompson.efx"),
            other => panic!("{other:?}"),
        }

        let mut kar98k = ev(EV_FIRE_WEAPON, 0, 0, [0.0; 3]);
        kar98k.entity_num = 7;
        kar98k.weapon = 2;
        match &resolve_with(&kar98k, &ImpactTable::default(), &ctx)[0] {
            Resolved::Spawn { path, .. } => {
                assert_eq!(path, "fx/muzzleflashes/standardflashworld.efx")
            }
            other => panic!("{other:?}"),
        }
    }

    /// First person flashes the fired weapon's view effect, or nothing when
    /// its file has none (`CG_MuzzleFlash`, docs/research/cod11-events-and-fx.md
    /// 5b); anyone else's shot, the world one.
    #[test]
    fn view_fire_takes_the_view_flash() {
        let mut muzzles = HashMap::new();
        muzzles.insert(u32::MAX, (Vec3::ZERO, Vec3::X));
        muzzles.insert(7u32, (Vec3::ZERO, Vec3::X));
        let mut weapon_flash = HashMap::new();
        weapon_flash.insert(5, "fx/muzzleflashes/thompson.efx".to_string());
        let mut weapons = vec![None; 6];
        weapons[5] = Some(WeaponDef {
            view_flash_effect: Some("fx/muzzleflashes/thompsonview.efx".to_string()),
            ..WeaponDef::default()
        });
        weapons[2] = Some(WeaponDef::default());
        let ctx = ResolveCtx {
            brass: &EMPTY_BRASS,
            weapons: &[],
            eject_brass: true,
            configstrings: &[],
            cs: &vcod_common::net::protocol::CS_LAYOUT_V1,
            blood: true,
            muzzles: &muzzles,
            shot_fallbacks: &HashMap::new(),
            weapon_flash: &weapon_flash,
            view_flash: Some(&weapons),
            view_body: None,
        };
        let path = |entity_num, weapon| {
            let mut e = ev(EV_FIRE_WEAPON, 0, 0, [0.0; 3]);
            e.entity_num = entity_num;
            e.weapon = weapon;
            match resolve_with(&e, &ImpactTable::default(), &ctx).remove(0) {
                Resolved::Spawn { path, .. } => Some(path),
                Resolved::Known => None,
                other => panic!("{other:?}"),
            }
        };
        assert_eq!(
            path(u32::MAX, 5).as_deref(),
            Some("fx/muzzleflashes/thompsonview.efx")
        );
        assert_eq!(path(u32::MAX, 2), None, "no viewFlashEffect, no flash");
        assert_eq!(path(u32::MAX, 9), None, "not in configstring 7");
        assert_eq!(path(7, 5).as_deref(), Some("fx/muzzleflashes/thompson.efx"));
    }

    /// Impact first, then tracer: the order `CG_BulletHitWall` runs them.
    #[test]
    fn bullet_hit_yields_both_impact_and_tracer_when_both_are_known() {
        let mut table = ImpactTable::default();
        table.map.insert(
            (ImpactKind::BulletSmallNormal, 1),
            "fx/impacts/test.efx".to_string(),
        );
        let mut muzzles = HashMap::new();
        muzzles.insert(7u32, (Vec3::new(0.0, 0.0, 0.0), Vec3::X));
        let weapon_flash = HashMap::new();
        let ctx = ResolveCtx {
            brass: &EMPTY_BRASS,
            weapons: &[],
            eject_brass: true,
            configstrings: &[],
            cs: &vcod_common::net::protocol::CS_LAYOUT_V1,
            blood: true,
            muzzles: &muzzles,
            shot_fallbacks: &HashMap::new(),
            weapon_flash: &weapon_flash,
            view_flash: None,
            view_body: None,
        };
        let mut e = ev(EV_BULLET_HIT_SMALL, 0, 1, [10.0, 20.0, 30.0]);
        e.other_entity_num = 7;

        let rs = resolve_with(&e, &table, &ctx);
        assert_eq!(rs.len(), 2, "{rs:?}");
        match &rs[0] {
            Resolved::Spawn {
                path,
                at: SpawnAt::Surface { .. },
                ..
            } => assert_eq!(path, "fx/impacts/test.efx"),
            other => panic!("expected the impact spawn first: {other:?}"),
        }
        match &rs[1] {
            Resolved::Tracer { .. } => {}
            other => panic!("expected the tracer second: {other:?}"),
        }
    }

    #[test]
    fn grenade_explode_resolves_to_a_surface_effect() {
        let mut table = ImpactTable::default();
        table.map.insert(
            (ImpactKind::GrenadeExplode, 0),
            "fx/explosions/grenade2.efx".to_string(),
        );
        let e = ev(EV_GRENADE_EXPLODE, 0, 0, [5.0, 5.0, 0.0]);
        let rs = resolve_with(&e, &table, &empty_ctx());
        assert!(matches!(&rs[..], [Resolved::Spawn { .. }]), "{rs:?}");
        match &rs[0] {
            Resolved::Spawn {
                path,
                at: SpawnAt::Surface { pos, normal },
                ..
            } => {
                assert_eq!(path, "fx/explosions/grenade2.efx");
                assert_eq!(*pos, Vec3::new(5.0, 5.0, 0.0));
                assert_eq!(*normal, Vec3::from(BYTE_DIRS[0]));
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn rocket_explode_and_nomarks_both_resolve_the_same_row_and_only_nomarks_drops_marks() {
        let mut table = ImpactTable::default();
        table.map.insert(
            (ImpactKind::RocketExplode, 6), // dirt
            "fx/impacts/newimps/v_blast1dirt.efx".to_string(),
        );
        for id in [EV_ROCKET_EXPLODE, EV_ROCKET_EXPLODE_NOMARKS] {
            let e = ev(id, 0, 6, [0.0, 0.0, 0.0]);
            let rs = resolve_with(&e, &table, &empty_ctx());
            match &rs[..] {
                [Resolved::Spawn { path, nomarks, .. }] => {
                    assert_eq!(path, "fx/impacts/newimps/v_blast1dirt.efx", "event {id}");
                    assert_eq!(*nomarks, id == EV_ROCKET_EXPLODE_NOMARKS);
                }
                other => panic!("event {id}: {other:?}"),
            }
        }
    }

    /// Retail's panzerfaust round (`net/snapshots-panzer-wall.bin`,
    /// cod11-combat.md 13.6) goes off as 179 on the rocket's own entity
    /// with the rock it struck as `surfType`, and draws `rocket_explode`'s
    /// rock row with its mark.
    #[test]
    fn the_panzer_capture_s_explode_draws_the_rocket_row() {
        use vcod_common::net::events::EventTracker;
        use vcod_common::net::protocol::PROTOCOL_V1;
        let p = &PROTOCOL_V1;
        let mut tracker = EventTracker::new();
        let explodes: Vec<GameEvent> = vcod_common::testing::fixture_frames(
            p,
            "net/gamestate-panzer-wall.bin",
            "net/snapshots-panzer-wall.bin",
        )
        .iter()
        .flat_map(|f| tracker.drain(&f.snapshot, p))
        .filter(|e| e.event == EV_ROCKET_EXPLODE)
        .collect();
        let [e] = &explodes[..] else {
            panic!("{} explodes, not one", explodes.len())
        };
        assert_eq!((e.e_type, e.surf_type), (0, 17), "eType 0 on rock");
        let mut table = ImpactTable::default();
        table.add_csv("rocket_explode,rock,fx/impacts/newimps/v_blast1.efx\n");
        match &resolve_with(e, &table, &empty_ctx())[..] {
            [Resolved::Spawn { path, nomarks, .. }] => {
                assert_eq!(path, "fx/impacts/newimps/v_blast1.efx");
                assert!(!nomarks);
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn grenade_bounce_with_no_csv_row_resolves_known() {
        let e = ev(EV_GRENADE_BOUNCE, 0, 0, [0.0, 0.0, 0.0]);
        let rs = resolve_with(&e, &ImpactTable::default(), &empty_ctx());
        assert!(matches!(rs.as_slice(), [Resolved::Known]), "{rs:?}");
    }

    /// Skips when the game is not installed (`testing::game_fs`).
    #[test]
    fn real_iw_impacts_csv_matches_the_research_doc() {
        let Some(fs) = vcod_common::testing::game_fs() else {
            return;
        };
        let table = load(&fs);
        assert_eq!(
            table.get(ImpactKind::BulletSmallNormal, 2), // brick
            Some("fx/impacts/small_brick.efx")
        );
        assert_eq!(
            table.get(ImpactKind::BulletLargeNormal, 20), // water
            Some("fx/impacts/waterhit_large.efx")
        );
        assert_eq!(
            table.get(ImpactKind::GrenadeExplode, 19), // snow
            Some("fx/impacts/snow_mortarlite.efx")
        );
        // blank for every surface in stock data
        assert_eq!(table.get(ImpactKind::BulletSmallReflect, 2), None);
        // the duplicate bark rows agree
        assert_eq!(
            table.get(ImpactKind::BulletSmallNormal, 1), // bark
            Some("fx/impacts/default_hit.efx")
        );
    }

    /// `CG_EjectBrass` (cgame 0x30038a30): a case on each shot of a gun
    /// without `boltAction`, on `EV_EJECT_BRASS` for one with it, the last
    /// shot's own effect when the file sets one, nothing under `cg_brass 0`.
    #[test]
    fn brass_follows_the_weapon_file_and_cg_brass() {
        let mut brass = HashMap::new();
        brass.insert(7u32, (Vec3::new(1.0, 2.0, 3.0), Vec3::Y));
        let mut weapons = vec![None; 4];
        weapons[1] = Some(WeaponDef {
            shell_eject_effect: Some("fx/shellejects/pistol.efx".to_string()),
            last_shot_eject_effect: Some("fx/shellejects/last.efx".to_string()),
            ..WeaponDef::default()
        });
        weapons[2] = Some(WeaponDef {
            shell_eject_effect: Some("fx/shellejects/rifle.efx".to_string()),
            bolt_action: true,
            ..WeaponDef::default()
        });
        weapons[3] = Some(WeaponDef::default());
        let paths = |event, weapon, on| {
            let ctx = ResolveCtx {
                brass: &brass,
                weapons: &weapons,
                eject_brass: on,
                ..empty_ctx()
            };
            let mut e = ev(event, 0, 0, [0.0; 3]);
            e.entity_num = 7;
            e.weapon = weapon;
            resolve_with(&e, &ImpactTable::default(), &ctx)
                .into_iter()
                .filter_map(|r| match r {
                    Resolved::Spawn { path, at, .. } if path.starts_with("fx/shellejects/") => {
                        match at {
                            SpawnAt::Directed { pos, dir } => {
                                assert_eq!((pos, dir), (Vec3::new(1.0, 2.0, 3.0), Vec3::Y));
                            }
                            other => panic!("{other:?}"),
                        }
                        Some(path)
                    }
                    _ => None,
                })
                .collect::<Vec<_>>()
        };
        assert_eq!(
            paths(EV_FIRE_WEAPON, 1, true),
            ["fx/shellejects/pistol.efx"]
        );
        assert_eq!(
            paths(EV_FIRE_WEAPON_LASTSHOT, 1, true),
            ["fx/shellejects/last.efx"]
        );
        assert_eq!(
            paths(EV_EJECT_BRASS, 1, true),
            ["fx/shellejects/pistol.efx"]
        );
        assert!(
            paths(EV_FIRE_WEAPON, 2, true).is_empty(),
            "a bolt waits for 163"
        );
        assert_eq!(paths(EV_EJECT_BRASS, 2, true), ["fx/shellejects/rifle.efx"]);
        assert!(
            paths(EV_FIRE_WEAPON, 3, true).is_empty(),
            "no shellEjectEffect"
        );
        assert!(paths(EV_EJECT_BRASS, 0, true).is_empty(), "weapon 0");
        assert!(paths(EV_FIRE_WEAPON, 1, false).is_empty(), "cg_brass 0");
        assert!(paths(EV_EJECT_BRASS, 2, false).is_empty(), "cg_brass 0");
    }

    /// UO's csv grammar: the per-class bullet rows, the explode rows and
    /// the vehicle column, with a later file's blank cell clearing a row.
    #[test]
    fn uo_csv_rows_parse_and_later_files_override() {
        let mut table = parse_impacts_csv(
            "bullet_rifle_normal,dirt,fx/a.efx\n\
             bullet_rifle_reflect,dirt,fx/b.efx\n\
             artillery_explode,vehicle,fx/c.efx\n\
             smoke_grenade_explode,dirt,fx/d.efx\n\
             grenade_explode,dirt,fx/e.efx\n",
        );
        assert_eq!(table.get(ImpactKind::Bullet(2, false), 6), Some("fx/a.efx"));
        assert_eq!(table.get(ImpactKind::Bullet(2, true), 6), Some("fx/b.efx"));
        assert_eq!(
            table.get(ImpactKind::Explode(4), SURF_VEHICLE),
            Some("fx/c.efx")
        );
        assert_eq!(table.get(ImpactKind::Explode(1), 6), Some("fx/d.efx"));
        assert_eq!(table.get(ImpactKind::GrenadeExplode, 6), Some("fx/e.efx"));
        table.add_csv("grenade_explode,dirt,\n");
        assert_eq!(table.get(ImpactKind::GrenadeExplode, 6), None);
    }

    fn uo_weapons() -> Vec<Option<WeaponDef>> {
        let mut weapons = vec![None; 4];
        weapons[1] = Some(WeaponDef {
            ammo_type: "rifle".into(),
            weapon_type: "bullet".into(),
            ..WeaponDef::default()
        });
        weapons[2] = Some(WeaponDef {
            proj_explosion_effect: Some("fx/shell.efx".into()),
            ..WeaponDef::default()
        });
        weapons
    }

    #[test]
    fn uo_bullet_hit_plays_the_ammo_type_row_both_ways() {
        let mut table = ImpactTable::default();
        table.add_csv("bullet_rifle_normal,dirt,fx/n.efx\nbullet_rifle_reflect,dirt,fx/r.efx\n");
        let weapons = uo_weapons();
        let ctx = ResolveCtx {
            weapons: &weapons,
            ..empty_ctx()
        };
        let mut e = ev(EV_UO_BULLET_HIT, 5, 6, [1.0, 2.0, 3.0]);
        e.weapon = 1;
        e.scale = 32;
        let rs = resolve_with(&e, &table, &ctx);
        let spawns: Vec<_> = rs
            .iter()
            .map(|r| match r {
                Resolved::Spawn {
                    path,
                    at: SpawnAt::Surface { normal, .. },
                    ..
                } => (path.as_str(), *normal),
                other => panic!("{other:?}"),
            })
            .collect();
        assert_eq!(
            spawns,
            [
                ("fx/n.efx", Vec3::from(BYTE_DIRS[5])),
                ("fx/r.efx", Vec3::from(BYTE_DIRS[32]))
            ]
        );
        // A weapon with no bullet row draws nothing.
        e.weapon = 2;
        assert!(matches!(
            resolve_with(&e, &table, &ctx)[..],
            [Resolved::Known]
        ));
    }

    #[test]
    fn uo_explode_reads_the_type_row_then_the_weapon_effect() {
        let mut table = ImpactTable::default();
        table.add_csv(
            "artillery_explode,dirt,fx/art.efx\n\
             artillery_explode,vehicle,fx/art_veh.efx\n\
             smoke_grenade_explode,dirt,fx/smoke.efx\n",
        );
        let weapons = uo_weapons();
        let ctx = ResolveCtx {
            weapons: &weapons,
            ..empty_ctx()
        };
        let paths = |e: &GameEvent| -> Vec<String> {
            resolve_with(e, &table, &ctx)
                .into_iter()
                .filter_map(|r| match r {
                    Resolved::Spawn { path, nomarks, .. } => {
                        assert_eq!(nomarks, e.event == EV_UO_PROJECTILE_EXPLODE_NOMARKS);
                        Some(path)
                    }
                    Resolved::OnTag { path, tag, .. } => Some(format!("{path}@{tag}")),
                    _ => None,
                })
                .collect()
        };
        let mut e = ev(EV_UO_PROJECTILE_EXPLODE, 0, 6, [0.0; 3]);
        e.scale = 4;
        e.weapon = 2;
        assert_eq!(paths(&e), ["fx/art.efx", "fx/shell.efx"]);
        e.eflags = EF_IN_VEHICLE_V22;
        e.event = EV_UO_PROJECTILE_EXPLODE_NOMARKS;
        assert_eq!(paths(&e), ["fx/art_veh.efx", "fx/shell.efx"]);
        // Smoke rides the grenade's tag; `none` plays only the weapon's.
        e.scale = 1;
        assert_eq!(paths(&e), ["fx/smoke.efx@g_body", "fx/shell.efx"]);
        e.scale = 8;
        assert_eq!(paths(&e), ["fx/shell.efx"]);
    }

    #[test]
    fn a_vehicle_flashes_on_the_fired_seats_tag() {
        // A vehicle's weapon is no body's, so its file names the flash.
        let mut weapons = vec![None; 4];
        weapons[3] = Some(WeaponDef {
            world_flash_effect: Some("fx/flash.efx".into()),
            ..WeaponDef::default()
        });
        let ctx = ResolveCtx {
            weapons: &weapons,
            ..empty_ctx()
        };
        let table = ImpactTable::default();
        for (event, tag) in [
            (EV_FIRE_WEAPON, "tag_flash"),
            (EV_FIRE_WEAPONB, "tag_altfire"),
            (EV_UO_FIRE_WEAPONC, "tag_secondary_flash"),
        ] {
            let mut e = ev(event, 0x80 | 3, 0, [0.0; 3]);
            e.e_type = ET_VEHICLE_V22;
            match &resolve_with(&e, &table, &ctx)[..] {
                [
                    Resolved::OnTag {
                        path,
                        entity: 40,
                        tag: t,
                        slot,
                    },
                ] => {
                    assert_eq!((path.as_str(), t.as_str()), ("fx/flash.efx", tag));
                    assert_eq!(
                        bolt_tag(&[], &vcod_common::net::protocol::CS_LAYOUT_V1, *slot),
                        Some(tag)
                    );
                }
                other => panic!("{event}: {other:?}"),
            }
        }
    }

    /// The UO capture's kar98k hits (`net/snapshots-uo-quake.bin`): the
    /// rifle row, normal off the parm, reflection off `es.scale`.
    #[test]
    fn the_uo_capture_bullet_hits_play_the_rifle_rows() {
        let p = &vcod_common::net::protocol::PROTOCOL_V22;
        let mut table = ImpactTable::default();
        table.add_csv(
            "bullet_rifle_normal,concrete,fx/n.efx\nbullet_rifle_reflect,concrete,fx/r.efx\n",
        );
        let mut weapons = vec![None; 21];
        weapons[20] = Some(WeaponDef {
            ammo_type: "rifle".into(),
            weapon_type: "bullet".into(),
            ..WeaponDef::default()
        });
        let ctx = ResolveCtx {
            weapons: &weapons,
            ..empty_ctx()
        };
        let mut tracker = vcod_common::net::events::EventTracker::new();
        let mut hits = 0;
        for f in vcod_common::testing::fixture_frames(
            p,
            "net/gamestate-uo-quake.bin",
            "net/snapshots-uo-quake.bin",
        ) {
            for ev in tracker.drain(&f.snapshot, p) {
                if ev.event != EV_UO_BULLET_HIT {
                    continue;
                }
                hits += 1;
                assert_eq!((ev.weapon, ev.surf_type), (20, 5));
                let paths: Vec<_> = resolve_with(&ev, &table, &ctx)
                    .into_iter()
                    .filter_map(|r| match r {
                        Resolved::Spawn {
                            path,
                            at: SpawnAt::Surface { normal, .. },
                            ..
                        } => Some((path, normal)),
                        _ => None,
                    })
                    .collect();
                assert_eq!(
                    paths,
                    [
                        ("fx/n.efx".to_string(), Vec3::from(byte_to_dir(ev.parm))),
                        ("fx/r.efx".to_string(), Vec3::from(byte_to_dir(ev.scale)))
                    ]
                );
            }
        }
        assert!(hits >= 4, "{hits} hits");
    }
}

/// The retail UO captures the event gates replay
/// (`crates/common/tests/fixtures/net/*-uo-*.bin`), with the weapons they
/// fire as their `pakuo` weapon files define them.
#[cfg(test)]
pub(crate) mod uo_captures {
    use std::collections::HashMap;
    use vcod_common::net::events::{EventTracker, GameEvent};
    use vcod_common::net::protocol::PROTOCOL_V22;
    use vcod_common::weapon::WeaponDef;

    /// Weapon file keys and values.
    type Keys = &'static [(&'static str, &'static str)];

    /// Configstring 7 index, name, and the weapon file keys the client
    /// reads, copied from `weapons/mp/<name>` in `pakuo*.pk3`.
    const WEAPONS: &[(usize, &str, Keys)] = &[
        (
            2,
            "50cal_tank_mp",
            &[
                ("weaponType", "bullet"),
                ("ammoType", "hmg"),
                ("fireTime", "0.11"),
                ("fireSound", "weap_50cal_box_fire"),
                ("worldFlashEffect", "fx/weapon/muzzleflash/mf_50cal.efx"),
            ],
        ),
        (
            6,
            "binoculars_artillery_mp",
            &[("weaponType", "spotter"), ("fireTime", "5")],
        ),
        (
            26,
            "mg34_mp",
            &[
                ("weaponType", "bullet"),
                ("ammoType", "lmg"),
                ("fireTime", "0.07"),
                ("loopFireSound", "weap_mg34_loop"),
                ("stopFireSound", "weap_mg34_cooldown"),
                ("deploySound", "weap_mg34_deploy"),
                ("breakdownSound", "weap_mg34_undeploy"),
                ("worldFlashEffect", "fx/weapon/muzzleflash/mf_mg34.efx"),
            ],
        ),
        (
            27,
            "mg34_tank_mp",
            &[
                ("weaponType", "bullet"),
                ("ammoType", "hmg"),
                ("fireTime", "0.1"),
                ("loopFireSound", "weap_mg34_loop"),
                ("stopFireSound", "weap_mg34_cooldown"),
                ("worldFlashEffect", "fx/weapon/muzzleflash/mf_mg34.efx"),
            ],
        ),
        (
            30,
            "mg42_bipod_stand_mp",
            &[
                ("weaponType", "bullet"),
                ("ammoType", "hmg"),
                ("fireTime", "0.05"),
                ("minSpread", "0"),
                ("maxSpread", "0"),
                ("loopFireSound", "weap_mg42_loop"),
                ("stopFireSound", "weap_mg42_cooldown"),
                ("worldFlashEffect", "fx/weapon/muzzleflash/mf_mg42.efx"),
            ],
        ),
        (
            32,
            "mg42_turret_mp",
            &[
                ("weaponType", "bullet"),
                ("ammoType", "hmg"),
                ("fireTime", "0.1"),
                ("loopFireSound", "weap_mg42_loop"),
                ("stopFireSound", "weap_mg42_cooldown"),
                ("worldFlashEffect", "fx/weapon/muzzleflash/mf_mg42.efx"),
            ],
        ),
        (
            42,
            "panzeriv_turret_mp",
            &[
                ("weaponType", "projectile"),
                ("fireTime", "3"),
                ("fireSound", "panzerIV_fire"),
                ("projectileSoundBlend1", "artillery_whiz_low"),
                ("projectileSoundBlend2", "artillery_whiz_high"),
                ("worldFlashEffect", "fx/vehicle/muzzleflash_panzerIV.efx"),
            ],
        ),
        (
            52,
            "smokegrenade_mp",
            &[
                ("weaponType", "grenade"),
                ("fireTime", "0.9"),
                ("fireSound", "weap_fraggrenade_fire"),
                ("pullbackSound", "weap_fraggrenade_pin"),
                ("projectileModel", "xmodel/w_us_grn_m18_game"),
                ("projExplosionSound", "smokegrenade_explode"),
            ],
        ),
    ];

    /// The weapons by configstring 7 index, each checked against the
    /// capture's own configstring 7.
    pub fn weapons(tag: &str) -> Vec<Option<WeaponDef>> {
        let h = vcod_common::net::huffman::Huffman::new();
        let data = vcod_common::testing::fixture(&format!("net/gamestate-{tag}.bin"));
        let gs = vcod_common::net::gamestate::parse(
            &mut vcod_common::net::msg::MsgReader::new(&data[4..], &h),
            &PROTOCOL_V22,
        )
        .unwrap();
        let names: Vec<&str> = gs.configstrings[7].split(' ').collect();
        let mut out = vec![None; names.len() + 1];
        for (index, name, keys) in WEAPONS {
            assert_eq!(
                names[index - 1],
                *name,
                "{tag}: configstring 7 slot {index}"
            );
            let map: HashMap<String, String> = keys
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect();
            out[*index] = Some(WeaponDef::from_map(&map));
        }
        out
    }

    /// Every event of a capture as the client drains it, in 1.1's
    /// numbering (`EV_UO_*` for UO's own).
    pub fn events(tag: &str) -> Vec<GameEvent> {
        let p = &PROTOCOL_V22;
        let mut tracker = EventTracker::new();
        vcod_common::testing::fixture_frames(
            p,
            &format!("net/gamestate-{tag}.bin"),
            &format!("net/snapshots-{tag}.bin"),
        )
        .iter()
        .flat_map(|f| tracker.drain(&f.snapshot, p))
        .collect()
    }
}

/// What the client draws for the retail UO captures' events
/// (docs/research/coduo-diff.md 19, "Captures").
#[cfg(test)]
mod uo_capture_gates {
    use super::*;
    use uo_captures::{events, weapons};
    use vcod_common::net::flags::ET_MISSILE;
    use vcod_common::pk3::Pk3Fs;

    fn ctx(weapons: &[Option<WeaponDef>]) -> ResolveCtx<'_> {
        static NONE: LazyLock<HashMap<u32, (Vec3, Vec3)>> = LazyLock::new(HashMap::new);
        static FLASH: LazyLock<HashMap<i32, String>> = LazyLock::new(HashMap::new);
        static NO_FALLBACKS: LazyLock<HashMap<u32, Vec3>> = LazyLock::new(HashMap::new);
        ResolveCtx {
            muzzles: &NONE,
            shot_fallbacks: &NO_FALLBACKS,
            weapon_flash: &FLASH,
            view_flash: None,
            view_body: None,
            blood: true,
            brass: &NONE,
            weapons,
            eject_brass: true,
            configstrings: &[],
            cs: &vcod_common::net::protocol::CS_LAYOUT_V22,
        }
    }

    /// A row of `kind` for every surface, `fx/<kind>/<surface>.efx`.
    fn rows(kinds: &[&str]) -> ImpactTable {
        let mut table = ImpactTable::default();
        for kind in kinds {
            for surf in SURFACE_NAMES.iter().chain(&["vehicle"]) {
                table.add_csv(&format!("{kind},{surf},fx/{kind}/{surf}.efx\n"));
            }
        }
        table
    }

    fn paths(rs: Vec<Resolved>) -> Vec<String> {
        rs.into_iter()
            .filter_map(|r| match r {
                Resolved::Spawn { path, .. } => Some(path),
                Resolved::OnTag {
                    path, entity, tag, ..
                } => Some(format!("{path}@{entity}:{tag}")),
                _ => None,
            })
            .collect()
    }

    /// The tank's main gun on `tag_flash`, its coaxial MG on `tag_altfire`,
    /// the gunner's MG on `tag_secondary_flash`, each the fired weapon's
    /// own `worldFlashEffect`; the jeep gunner's .50 on its seat's tag.
    #[test]
    fn the_uo_vehicle_captures_flash_each_gun_on_its_tag() {
        let flash = |w: &str| format!("fx/weapon/muzzleflash/{w}.efx");
        for (tag, expect) in [
            (
                "uo-tank",
                [
                    (
                        EV_FIRE_WEAPON,
                        "fx/vehicle/muzzleflash_panzerIV.efx".into(),
                        "tag_flash",
                    ),
                    (EV_FIRE_WEAPONB, flash("mf_mg34"), "tag_altfire"),
                    (EV_UO_FIRE_WEAPONC, flash("mf_mg42"), "tag_secondary_flash"),
                ]
                .to_vec(),
            ),
            (
                "uo-overheat",
                [
                    (
                        EV_FIRE_WEAPON,
                        "fx/vehicle/muzzleflash_panzerIV.efx".into(),
                        "tag_flash",
                    ),
                    (EV_FIRE_WEAPONB, flash("mf_mg34"), "tag_altfire"),
                    (EV_UO_FIRE_WEAPONC, flash("mf_mg42"), "tag_secondary_flash"),
                ]
                .to_vec(),
            ),
            (
                "uo-jeep",
                [(EV_UO_FIRE_WEAPONC, flash("mf_50cal"), "tag_secondary_flash")].to_vec(),
            ),
        ] {
            let weapons = weapons(tag);
            let ctx = ctx(&weapons);
            let mut seen = HashMap::new();
            for ev in events(tag) {
                if ev.e_type != ET_VEHICLE_V22 || !crate::audio::cues::fires_weapon(ev.event) {
                    continue;
                }
                let (_, path, t) = expect
                    .iter()
                    .find(|(e, ..)| *e == ev.event)
                    .unwrap_or_else(|| panic!("{tag}: unexpected fire event {}", ev.event));
                assert_eq!(
                    paths(resolve_with(&ev, &ImpactTable::default(), &ctx)),
                    [format!("{path}@{}:{t}", ev.entity_num)],
                    "{tag}"
                );
                *seen.entry(ev.event).or_insert(0) += 1;
            }
            assert_eq!(seen.len(), expect.len(), "{tag}: {seen:?}");
        }
    }

    /// The vehicle captures' MG rounds with parm bit 7, the ones the
    /// server raised no hit or tracer for (`CMD_VEH_FireTurret`,
    /// `game.mp.uo.i386.so` 0x8f4a7), are traced on the client from their
    /// gun's tag, the coaxial's hit tracer off `tag_altfire` and the
    /// gunner's off `tag_secondary_flash`. The jeep's one round without the
    /// bit came with the server's own bullet hit; the shell is never traced.
    #[test]
    fn the_uo_vehicle_captures_trace_the_mg_rounds_the_server_left() {
        for (tag, guns) in [
            (
                "uo-tank",
                [(27, "tag_altfire"), (32, "tag_secondary_flash")].to_vec(),
            ),
            (
                "uo-overheat",
                [(27, "tag_altfire"), (32, "tag_secondary_flash")].to_vec(),
            ),
            ("uo-jeep", [(2, "tag_secondary_flash")].to_vec()),
        ] {
            let weapons = weapons(tag);
            let ctx = ctx(&weapons);
            let mut traced = HashMap::new();
            let mut served = 0;
            for ev in events(tag) {
                if ev.e_type != ET_VEHICLE_V22 || !crate::audio::cues::fires_weapon(ev.event) {
                    continue;
                }
                let round = client_round(&ev, &ctx);
                let w = fire_weapon(&ev);
                if w == 42 || ev.parm & 0x80 == 0 {
                    assert_eq!(round, None, "{tag}: weapon {w} parm {:#x}", ev.parm);
                    served += (w != 42) as usize;
                    continue;
                }
                let (_, t) = guns
                    .iter()
                    .find(|(g, _)| *g == w)
                    .unwrap_or_else(|| panic!("{tag}: weapon {w}"));
                let r = round.unwrap_or_else(|| panic!("{tag}: weapon {w} untraced"));
                assert_eq!((r.entity, r.tag, r.tracer_tag), (ev.entity_num, *t, *t));
                *traced.entry(w).or_insert(0) += 1;
            }
            assert_eq!(traced.len(), guns.len(), "{tag}: {traced:?}");
            assert_eq!(served, (tag == "uo-jeep") as usize, "{tag}");
        }
    }

    /// The turret capture (`*-uo-turret.bin`, mp_carentan's mounted MG42
    /// held on a target): its rounds into the world carry bit 7 and are
    /// traced from the gun's `tag_flash`, its rounds into the target do not
    /// and come with the server's rolled `EV_BULLET_TRACER` from the muzzle
    /// to the hit, which draws a heavy tracer without a second roll.
    #[test]
    fn the_uo_turret_capture_traces_its_misses_and_draws_the_served_tracers() {
        let weapons = weapons("uo-turret");
        let ctx = ctx(&weapons);
        let (mut traced, mut served, mut tracers) = (0, 0, 0);
        for ev in events("uo-turret") {
            match ev.event {
                EV_FIRE_WEAPON_MG42 if ev.e_type == ET_TURRET => {
                    assert_eq!(ev.weapon, 30);
                    let round = client_round(&ev, &ctx);
                    if ev.parm & 0x80 != 0 {
                        let r = round.expect("a bit-7 round");
                        assert_eq!(
                            (r.tag, r.tracer_tag, r.weapon),
                            ("tag_flash", "tag_flash", 30)
                        );
                        traced += 1;
                    } else {
                        assert_eq!(round, None);
                        served += 1;
                    }
                }
                EV_BULLET_TRACER => {
                    assert_eq!(ev.parm, 30);
                    match resolve_with(&ev, &ImpactTable::default(), &ctx).as_slice() {
                        [
                            Resolved::Tracer {
                                muzzle,
                                impact,
                                flesh: false,
                                heavy: true,
                                roll: false,
                            },
                        ] => {
                            assert_eq!(*muzzle, Vec3::from(ev.pos));
                            assert_eq!(*impact, Vec3::from(ev.dir));
                        }
                        r => panic!("{r:?}"),
                    }
                    tracers += 1;
                }
                _ => {}
            }
        }
        assert!(traced >= 10 && served >= 100, "{traced} {served}");
        assert!(tracers >= 50 && tracers < served, "{tracers} of {served}");
    }

    /// The artillery strike: every shell explodes as type 4 on the
    /// `artillery_explode` row of the surface it hit, carrying the
    /// binoculars as its weapon; the launches and the incoming warnings
    /// draw nothing.
    #[test]
    fn the_uo_artillery_capture_explodes_on_the_artillery_rows() {
        let weapons = weapons("uo-arty");
        let ctx = ctx(&weapons);
        let table = rows(&["artillery_explode", "tank_explode"]);
        let mut counts = HashMap::new();
        for ev in events("uo-arty") {
            match ev.event {
                EV_UO_PROJECTILE_EXPLODE => {
                    assert_eq!((ev.scale, ev.weapon, ev.e_type), (4, 6, 0));
                    let surf = SURFACE_NAMES[ev.surf_type as usize];
                    assert_eq!(
                        paths(resolve_with(&ev, &table, &ctx)),
                        [format!("fx/artillery_explode/{surf}.efx")]
                    );
                }
                EV_UO_PROJECTILE_LAUNCH | EV_UO_PROJECTILE_INCOMING => {
                    assert_eq!(ev.parm, 4);
                    assert!(matches!(
                        resolve_with(&ev, &table, &ctx)[..],
                        [Resolved::Known]
                    ));
                }
                _ => continue,
            }
            *counts.entry(ev.event).or_insert(0) += 1;
        }
        assert!(counts[&EV_UO_PROJECTILE_EXPLODE] >= 12, "{counts:?}");
        assert!(counts[&EV_UO_PROJECTILE_LAUNCH] >= 12, "{counts:?}");
        assert!(counts[&EV_UO_PROJECTILE_INCOMING] >= 12, "{counts:?}");
    }

    /// The smoke grenade's cloud rides the grenade's own `g_body`, the
    /// grenade still a missile of the smoke grenade's model when it pops.
    #[test]
    fn the_uo_smoke_capture_bolts_the_cloud_to_the_grenade() {
        let weapons = weapons("uo-smoke");
        let ctx = ctx(&weapons);
        let table = rows(&["smoke_grenade_explode", "grenade_bounce"]);
        let evs = events("uo-smoke");
        let pops: Vec<_> = evs
            .iter()
            .filter(|e| e.event == EV_UO_PROJECTILE_EXPLODE)
            .collect();
        assert_eq!(pops.len(), 1);
        let pop = pops[0];
        assert_eq!((pop.scale, pop.weapon, pop.e_type), (1, 52, ET_MISSILE));
        assert_eq!(
            paths(resolve_with(pop, &table, &ctx)),
            [format!(
                "fx/smoke_grenade_explode/{}.efx@{}:g_body",
                SURFACE_NAMES[pop.surf_type as usize], pop.entity_num
            )]
        );
        // The missile draws this model, whose parts carry `g_body`
        // (`xmodelparts/w_us_grn_m18_game`, pakuo).
        assert_eq!(
            weapons[52].as_ref().unwrap().projectile_model.as_deref(),
            Some("w_us_grn_m18_game")
        );
        let bounces: Vec<_> = evs
            .iter()
            .filter(|e| e.event == EV_GRENADE_BOUNCE)
            .collect();
        assert_eq!(bounces.len(), 2);
        for b in bounces {
            assert_eq!((b.weapon, b.entity_num), (52, pop.entity_num));
            assert_eq!(
                paths(resolve_with(b, &table, &ctx)),
                [format!(
                    "fx/grenade_bounce/{}.efx",
                    SURFACE_NAMES[b.surf_type as usize]
                )]
            );
        }
    }

    /// Every event of the captures is one the client knows.
    #[test]
    fn the_uo_captures_carry_no_unknown_event() {
        for tag in [
            "uo-arty",
            "uo-smoke",
            "uo-mg",
            "uo-overheat",
            "uo-tank",
            "uo-jeep",
            "uo-wreck",
        ] {
            let weapons = weapons(tag);
            let ctx = ctx(&weapons);
            for ev in events(tag) {
                let rs = resolve_with(&ev, &ImpactTable::default(), &ctx);
                assert!(
                    !matches!(rs[..], [Resolved::Unknown]),
                    "{tag}: event {:#x} parm {}",
                    ev.event,
                    ev.parm
                );
            }
        }
    }

    /// A tank's death on retail (`*-uo-wreck.bin`, `probe_vehwreck` on
    /// mp_cassino; coduo-vehicles.md 15): `_tankeffects_gmi::tank_death`'s
    /// first blast on `tag_origin` and earthquake at the kill, its burning
    /// `playLoopedFx` each second until `allow_explode` ten seconds on,
    /// then the second blast on `tag_origin`. The tank keeps its live model
    /// throughout and leaves the snapshot half a second after.
    #[test]
    fn the_uo_wreck_capture_blows_the_tank_up_twice_and_burns_between() {
        use crate::fx::looped::LoopedFx;
        use crate::play::quake;
        use vcod_common::net::event_ids::EV_EARTHQUAKE;
        use vcod_common::net::events::EventTracker;
        use vcod_common::net::protocol::PROTOCOL_V22;
        let p = &PROTOCOL_V22;
        let gs = vcod_common::testing::fixture_gamestate(p, "net/gamestate-uo-wreck.bin");
        let mut cs = gs.configstrings.clone();
        let weapons = weapons("uo-wreck");
        let mut tracker = EventTracker::new();
        let mut looped = LoopedFx::default();
        let (mut on_tag, mut quakes, mut burns) = (Vec::new(), Vec::new(), Vec::new());
        let mut last_seen = 0;
        for f in vcod_common::testing::fixture_frames(
            p,
            "net/gamestate-uo-wreck.bin",
            "net/snapshots-uo-wreck.bin",
        ) {
            let s = &f.snapshot;
            for c in &f.commands {
                if let Some((n, v)) = c.strip_prefix("d ").and_then(|r| r.split_once(' ')) {
                    cs[n.parse::<usize>().unwrap()] = v.to_string();
                }
            }
            let mut ctx = ctx(&weapons);
            ctx.configstrings = &cs;
            for ev in tracker.drain(s, p) {
                let rs = resolve_with(&ev, &ImpactTable::default(), &ctx);
                assert!(
                    !matches!(rs[..], [Resolved::Unknown]),
                    "event {:#x}",
                    ev.event
                );
                on_tag.extend(paths(rs).into_iter().map(|path| (s.server_time, path)));
                if ev.event == EV_EARTHQUAKE {
                    let q = quake::from_event(p, &ev, s.entities.get(&ev.entity_num), Vec3::ZERO)
                        .unwrap();
                    quakes.push((s.server_time, q.scale, q.duration_ms, q.radius));
                }
            }
            for play in looped.frame(s, p, s.server_time, Vec3::ZERO, &cs) {
                burns.push((s.server_time, play.path));
            }
            if let Some(e) = s.entities.get(&186) {
                assert_eq!(e.field_i32(p, "eType"), ET_VEHICLE_V22);
                assert_eq!(e.field_i32(p, "xmodel"), 38, "no death model");
                last_seen = s.server_time;
            }
        }
        let dir = "fx/explosions/vehicles/t34_mp_n";
        assert_eq!(
            on_tag,
            [
                (33300, format!("{dir}/1st_exp_v2.efx@186:tag_origin")),
                (43300, format!("{dir}/2nd_exp.efx@186:tag_origin")),
            ]
        );
        assert_eq!(quakes, [(33300, 0.25, 3000.0, 1050.0)]);
        // `CG_FX`'s timer steps from 0 in whole periods, so after the
        // first play the rest land on the second.
        let want: Vec<_> = [33300]
            .into_iter()
            .chain((34..=43).map(|s| s * 1000))
            .map(|t| (t, format!("{dir}/burn_smoke.efx")))
            .collect();
        assert_eq!(burns, want);
        assert_eq!(last_seen, 43750);
        // With a UO install: each effect the death plays spawns.
        if let Some(fs) = vcod_common::testing::uo_fs() {
            for name in ["1st_exp_v2", "2nd_exp", "burn_smoke"] {
                let mut sim = crate::fx::sim::FxSystem::new();
                let at = SpawnAt::Point { pos: Vec3::ZERO };
                sim.spawn(&fs, &format!("{dir}/{name}.efx"), at, 0.0);
                assert!(sim.counts().0 > 0, "{name}");
            }
        }
    }

    /// A body throwing the smoke grenade or firing the artillery
    /// binoculars flashes nothing: neither weapon file names a
    /// `worldFlashEffect`, and `CG_MuzzleFlash` spawns only a set one.
    #[test]
    fn the_uo_captures_flash_no_grenade_or_binoculars() {
        for (tag, weapon) in [("uo-smoke", 52), ("uo-arty", 6)] {
            let weapons = weapons(tag);
            let muzzles = HashMap::from([(1, (Vec3::ZERO, Vec3::X))]);
            let ctx = ResolveCtx {
                muzzles: &muzzles,
                shot_fallbacks: &HashMap::new(),
                ..ctx(&weapons)
            };
            let fires: Vec<_> = events(tag)
                .into_iter()
                .filter(|e| crate::audio::cues::fires_weapon(e.event) && e.entity_num == 1)
                .collect();
            assert!(!fires.is_empty(), "{tag}");
            for ev in fires {
                assert_eq!(ev.weapon, weapon, "{tag}");
                assert!(
                    matches!(
                        resolve_with(&ev, &ImpactTable::default(), &ctx)[..],
                        [Resolved::Known]
                    ),
                    "{tag}"
                );
            }
        }
    }

    /// The tank's shell explodes as type 6 on the `tank_explode` row of
    /// the ground it hit, not the vehicle column. `uo-tank`'s second shell
    /// is first seen already exploded and still plays (`CG_ResetEntity`).
    #[test]
    fn the_uo_tank_capture_shell_explodes_on_the_tank_row() {
        let shells = |tag| {
            events(tag)
                .into_iter()
                .filter(|e| e.event == EV_UO_PROJECTILE_EXPLODE)
                .count()
        };
        assert_eq!(shells("uo-tank"), 2);
        for tag in ["uo-tank", "uo-overheat"] {
            let weapons = weapons(tag);
            let ctx = ctx(&weapons);
            let table = rows(&["tank_explode"]);
            let shells: Vec<_> = events(tag)
                .into_iter()
                .filter(|e| e.event == EV_UO_PROJECTILE_EXPLODE)
                .collect();
            assert!(!shells.is_empty(), "{tag}");
            for ev in shells {
                assert_eq!(
                    (ev.scale, ev.weapon, ev.eflags & EF_IN_VEHICLE_V22),
                    (6, 42, 0)
                );
                assert_eq!(
                    paths(resolve_with(&ev, &table, &ctx)),
                    [format!(
                        "fx/tank_explode/{}.efx",
                        SURFACE_NAMES[ev.surf_type as usize]
                    )]
                );
            }
        }
    }

    /// `uo-shellveh`'s three shells strike the Elefant: each explode is 186
    /// with the vehicle bit on metal, and draws the `tank_explode` row's
    /// vehicle column, not the metal one (coduo-vehicles.md 13, "A shell
    /// on a hull").
    #[test]
    fn the_uo_shellveh_capture_explodes_on_the_vehicle_column() {
        let weapons = weapons("uo-shellveh");
        let ctx = ctx(&weapons);
        let table = rows(&["tank_explode"]);
        let shells: Vec<_> = events("uo-shellveh")
            .into_iter()
            .filter(|e| {
                e.event == EV_UO_PROJECTILE_EXPLODE || e.event == EV_UO_PROJECTILE_EXPLODE_NOMARKS
            })
            .collect();
        assert_eq!(shells.len(), 3);
        for ev in shells {
            assert_eq!(
                (
                    ev.event,
                    ev.scale,
                    ev.surf_type,
                    ev.eflags & EF_IN_VEHICLE_V22
                ),
                (EV_UO_PROJECTILE_EXPLODE_NOMARKS, 6, 13, EF_IN_VEHICLE_V22)
            );
            assert_eq!(
                paths(resolve_with(&ev, &table, &ctx)),
                ["fx/tank_explode/vehicle.efx"]
            );
        }
    }

    /// `uo-tank`'s shells drawn off a pak whose impact table spells its
    /// rows as UO's `fx/gmi_impacts.csv` does, with backslashes: each
    /// explode's row reads its effect and the effect spawns particles.
    /// Before the pak lookup read a backslash as a slash, every UO
    /// explode row (tank, artillery, mortar, grenade, rocket, smoke)
    /// missed its file and drew nothing.
    #[test]
    fn the_uo_tank_capture_shells_draw_off_backslashed_rows() {
        let root = tempfile::tempdir().unwrap();
        let mut w =
            zip::ZipWriter::new(std::fs::File::create(root.path().join("pakuo00.pk3")).unwrap());
        let opts = zip::write::SimpleFileOptions::default();
        let mut csv = String::new();
        for surf in SURFACE_NAMES {
            csv += &format!("tank_explode,{surf},fx\\weapon\\explosions\\tank_{surf}.efx\n");
            w.start_file(format!("fx/weapon/explosions/tank_{surf}.efx"), opts)
                .unwrap();
            std::io::Write::write_all(
                &mut w,
                b"Particle\n{\n\tcount 4\n\tlife 1000\n\tsize\n\t{\n\t\tstart 10\n\t}\n\tshaders\n\t[\n\t\tsmoke/a\n\t]\n}\n",
            )
            .unwrap();
        }
        w.start_file("fx/gmi_impacts.csv", opts).unwrap();
        std::io::Write::write_all(&mut w, csv.as_bytes()).unwrap();
        w.finish().unwrap();
        let fs = Pk3Fs::open(root.path()).unwrap();
        let table = load(&fs);
        let weapons = weapons("uo-tank");
        let ctx = ctx(&weapons);
        let shells: Vec<_> = events("uo-tank")
            .into_iter()
            .filter(|e| e.event == EV_UO_PROJECTILE_EXPLODE)
            .collect();
        assert_eq!(shells.len(), 2);
        for ev in shells {
            let mut fx = crate::fx::sim::FxSystem::new();
            let mut spawned = 0;
            for r in resolve_with(&ev, &table, &ctx) {
                if let Resolved::Spawn {
                    path,
                    at,
                    nomarks,
                    bolt,
                } = r
                {
                    spawned += 1;
                    fx.spawn_marked(&fs, &path, at, bolt, 1.0, !nomarks);
                }
            }
            assert_eq!(spawned, 1, "the tank_explode row");
            fx.step(0.016, 1.016, None);
            assert!(fx.counts().0 > 0, "the shell at {:?} drew nothing", ev.pos);
        }
    }

    /// With `COD_DIR` at a UO install: every explode row of the stock
    /// impact tables names an effect the paks hold and that parses.
    #[test]
    fn every_uo_explode_row_reads_its_effect() {
        let Some(fs) = vcod_common::testing::uo_fs() else {
            return;
        };
        let table = load(&fs);
        let mut explodes = 0;
        for ((kind, _), path) in &table.map {
            if !matches!(
                kind,
                ImpactKind::Explode(_)
                    | ImpactKind::GrenadeExplode
                    | ImpactKind::RocketExplode
                    | ImpactKind::MolotovExplodeNormal
            ) {
                continue;
            }
            explodes += 1;
            let bytes = fs
                .read(&crate::fx::efx::chained_path(path))
                .unwrap_or_else(|| panic!("{kind:?}: {path} not in the paks"));
            crate::fx::efx::parse(&String::from_utf8_lossy(&bytes))
                .unwrap_or_else(|e| panic!("{path}: {e}"));
        }
        assert!(explodes > 100, "{explodes} explode rows");
    }
}
