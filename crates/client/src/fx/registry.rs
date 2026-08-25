//! Event to visual-effect resolution: the MP `EV_*` ids joined with
//! `fx/iw_impacts.csv`. docs/research/cod11-events-and-fx.md sections 1 and 5a.

use crate::fx::sim::SpawnAt;
use glam::Vec3;
use std::collections::HashMap;
use std::sync::OnceLock;
use vcod_common::net::events::{byte_to_dir, GameEvent};
use vcod_common::pk3::Pk3Fs;

// EV_* ids from `cgame_mp_x86.dll` `eventnames[]` (VA 0x30077040), identical
// in every MP module and in 1.5. The SP module's table diverges from 173 up.

pub const EV_NONE: i32 = 0;

/// ids 1-138 are six groups of 23, one entry per surface type (doc section 1).
pub(crate) const EV_FOOTSTEP_RUN_BASE: i32 = 1;
pub(crate) const EV_FOOTSTEP_WALK_BASE: i32 = 24;
pub(crate) const EV_FOOTSTEP_PRONE_BASE: i32 = 47;
pub(crate) const EV_JUMP_BASE: i32 = 70;
pub(crate) const EV_LANDING_BASE: i32 = 93;
pub(crate) const EV_LANDING_PAIN_BASE: i32 = 116;
pub(crate) const SURFACE_GROUP_SIZE: i32 = 23;

pub const EV_FOLIAGE_SOUND: i32 = 139;
pub const EV_STANCE_FORCE_STAND: i32 = 140;
pub const EV_STANCE_FORCE_CROUCH: i32 = 141;
pub const EV_STANCE_FORCE_PRONE: i32 = 142;
pub const EV_STEP_VIEW: i32 = 143;
pub const EV_WATER_TOUCH: i32 = 144;
pub const EV_WATER_LEAVE: i32 = 145;
pub const EV_ITEM_PICKUP: i32 = 146;
pub const EV_ITEM_PICKUP_QUIET: i32 = 147;
pub const EV_AMMO_PICKUP: i32 = 148;
pub const EV_NOAMMO: i32 = 149;
pub const EV_EMPTYCLIP: i32 = 150;
pub const EV_RELOAD: i32 = 151;
pub const EV_RELOAD_FROM_EMPTY: i32 = 152;
pub const EV_RELOAD_START: i32 = 153;
pub const EV_RELOAD_END: i32 = 154;
pub const EV_RAISE_WEAPON: i32 = 155;
pub const EV_PUTAWAY_WEAPON: i32 = 156;
pub const EV_WEAPON_ALT: i32 = 157;
pub const EV_PULLBACK_WEAPON: i32 = 158;
pub const EV_FIRE_WEAPON: i32 = 159;
pub const EV_FIRE_WEAPONB: i32 = 160;
pub const EV_FIRE_WEAPON_LASTSHOT: i32 = 161;
pub const EV_RECHAMBER_WEAPON: i32 = 162;
pub const EV_EJECT_BRASS: i32 = 163;
pub const EV_MELEE_SWIPE: i32 = 164;
pub const EV_FIRE_MELEE: i32 = 165;
pub const EV_MELEE_HIT: i32 = 166;
pub const EV_MELEE_MISS: i32 = 167;
pub const EV_FIRE_WEAPON_MG42: i32 = 168;
pub const EV_FIRE_QUADBARREL_1: i32 = 169;
pub const EV_FIRE_QUADBARREL_2: i32 = 170;
pub const EV_BULLET_TRACER: i32 = 171;
pub const EV_SOUND_ALIAS: i32 = 172;
pub const EV_BULLET_HIT_SMALL: i32 = 173;
pub const EV_BULLET_HIT_LARGE: i32 = 174;
/// Delivered to the victim only; never reaches a spectator (doc section 2).
pub const EV_BULLET_HIT_CLIENT_SMALL: i32 = 175;
pub const EV_BULLET_HIT_CLIENT_LARGE: i32 = 176;
pub const EV_GRENADE_BOUNCE: i32 = 177;
pub const EV_GRENADE_EXPLODE: i32 = 178;
pub const EV_ROCKET_EXPLODE: i32 = 179;
pub const EV_ROCKET_EXPLODE_NOMARKS: i32 = 180;
pub const EV_MOLOTOV_EXPLODE: i32 = 181;
pub const EV_MOLOTOV_EXPLODE_NOMARKS: i32 = 182;
pub const EV_CUSTOM_EXPLODE: i32 = 183;
pub const EV_CUSTOM_EXPLODE_NOMARKS: i32 = 184;
pub const EV_RAILTRAIL: i32 = 185;
pub const EV_BULLET: i32 = 186;
pub const EV_PAIN: i32 = 187;
pub const EV_CROUCH_PAIN: i32 = 188;
pub const EV_DEATH: i32 = 189;
pub const EV_DEBUG_LINE: i32 = 190;
pub const EV_PLAY_FX: i32 = 191;
pub const EV_PLAY_FX_DIR: i32 = 192;
pub const EV_PLAY_FX_ON_TAG: i32 = 193;
pub const EV_FLAMEBARREL_BOUNCE: i32 = 194;
pub const EV_EARTHQUAKE: i32 = 195;
pub const EV_DROPWEAPON: i32 = 196;
pub const EV_ITEM_RESPAWN: i32 = 197;
pub const EV_ITEM_POP: i32 = 198;
pub const EV_PLAYER_TELEPORT_IN: i32 = 199;
pub const EV_PLAYER_TELEPORT_OUT: i32 = 200;
pub const EV_OBITUARY: i32 = 201;

/// `surfType` index order (doc section 4); also the csv's surface column.
const SURFACE_NAMES: [&str; 23] = [
    "default", "bark", "brick", "carpet", "cloth", "concrete", "dirt", "flesh", "foliage", "glass",
    "grass", "gravel", "ice", "metal", "mud", "paper", "plaster", "rock", "sand", "snow", "water",
    "wood", "asphalt",
];

/// The csv's nine impact-type literals (docs/research/efx-grammar.md, 6a).
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
}

impl ImpactKind {
    fn parse(s: &str) -> Option<ImpactKind> {
        Some(match s {
            "bullet_small_normal" => ImpactKind::BulletSmallNormal,
            "bullet_small_reflect" => ImpactKind::BulletSmallReflect,
            "bullet_large_normal" => ImpactKind::BulletLargeNormal,
            "bullet_large_reflect" => ImpactKind::BulletLargeReflect,
            "grenade_bounce" => ImpactKind::GrenadeBounce,
            "grenade_explode" => ImpactKind::GrenadeExplode,
            "rocket_explode" => ImpactKind::RocketExplode,
            "molotov_explode_normal" => ImpactKind::MolotovExplodeNormal,
            "molotov_explode_reflect" => ImpactKind::MolotovExplodeReflect,
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
}

/// Format: docs/research/efx-grammar.md section 6a. Duplicate rows agree in
/// stock data, so last write wins. Unknown literals are skipped so a mod's
/// extra rows do not break the rest.
fn parse_impacts_csv(text: &str) -> ImpactTable {
    let mut map = HashMap::new();
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
        let Some(surf) = SURFACE_NAMES.iter().position(|s| *s == surf_s.trim()) else {
            continue;
        };
        if !path.is_empty() {
            map.insert((kind, surf as u8), path.to_string());
        }
    }
    ImpactTable { map }
}

const IMPACTS_CSV_PATH: &str = "fx/iw_impacts.csv";

fn load(fs: &Pk3Fs) -> ImpactTable {
    match fs.read(IMPACTS_CSV_PATH) {
        Some(bytes) => parse_impacts_csv(&String::from_utf8_lossy(&bytes)),
        None => {
            log::warn!("fx {IMPACTS_CSV_PATH}: not found; bullet impacts will have no visual");
            ImpactTable::default()
        }
    }
}

static TABLE: OnceLock<ImpactTable> = OnceLock::new();

/// Call once before any event is drained; `resolve()` reads an empty table
/// otherwise.
pub fn init(fs: &Pk3Fs) {
    if TABLE.set(load(fs)).is_err() {
        log::warn!(
            "fx {IMPACTS_CSV_PATH}: init() called after the table was already set (e.g. by an \
             earlier resolve()); this call's parse is discarded and the earlier table stays live"
        );
    }
}

fn table() -> &'static ImpactTable {
    TABLE.get_or_init(ImpactTable::default)
}

/// `Known` is a recognized event with nothing to draw. `Unknown` is an id this
/// build does not recognize; the F3 overlay counts those.
#[derive(Debug)]
pub enum Resolved {
    Spawn {
        path: String,
        at: SpawnAt,
    },
    /// Retail draws tracers as a hardcoded quad, not an `.efx`
    /// (docs/research/efx-grammar.md R8); see [`crate::fx::sim::FxSystem::spawn_tracer`].
    Tracer {
        muzzle: Vec3,
        impact: Vec3,
    },
    Known,
    Unknown,
}

pub struct ResolveCtx<'a> {
    /// Entity number to (world muzzle position, forward) this frame. Missing
    /// means the shooter's model has not loaded yet.
    pub muzzles: &'a HashMap<u32, (Vec3, Vec3)>,
    /// 1-based CS7 weapon index to that weapon file's `worldFlashEffect`.
    /// Missing falls back to [`flash_effect_path`].
    pub weapon_flash: &'a HashMap<i32, String>,
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

/// Resolve one drained [`GameEvent`] against the cached table and this
/// frame's muzzles.
pub fn resolve(ev: &GameEvent, ctx: &ResolveCtx) -> Vec<Resolved> {
    resolve_with(ev, table(), ctx)
}

fn resolve_with(ev: &GameEvent, table: &ImpactTable, ctx: &ResolveCtx) -> Vec<Resolved> {
    match ev.event {
        EV_NONE => vec![Resolved::Known],

        // eventParm is the byte-dir surface normal; `ev.dir` (origin2) is zero
        // on these events (doc section 2). Retail draws the tracer from here,
        // in `CG_BulletHitWall`, not from the fire event: `EV_BULLET_TRACER`
        // has no MP case (doc section 6). `other_entity_num` is the shooter.
        EV_BULLET_HIT_SMALL | EV_BULLET_HIT_LARGE => {
            let mut out = Vec::new();
            let kind = if ev.event == EV_BULLET_HIT_SMALL {
                ImpactKind::BulletSmallNormal
            } else {
                ImpactKind::BulletLargeNormal
            };
            let surf = ev.surf_type.clamp(0, SURFACE_NAMES.len() as i32 - 1) as u8;
            let impact = Vec3::from(ev.pos);
            if let Some(path) = table.get(kind, surf) {
                out.push(Resolved::Spawn {
                    path: path.to_string(),
                    at: SpawnAt::Surface {
                        pos: impact,
                        normal: Vec3::from(byte_to_dir(ev.parm)),
                    },
                });
            }
            // Every surface type, flesh included (doc section 6). Not
            // `fx/tagged/tracers.efx`, which is the AA gun's tracer. Chance
            // roll, distance gate and geometry live in `FxSystem::spawn_tracer`.
            if let Some(&(muzzle_pos, _)) = ctx.muzzles.get(&ev.other_entity_num) {
                out.push(Resolved::Tracer {
                    muzzle: muzzle_pos,
                    impact,
                });
            }
            known_or(out)
        }

        // Fire events fire on the shooter itself: `entity_num`, not
        // `other_entity_num`. No muzzle entry yet drops the flash rather than
        // misplacing it.
        EV_FIRE_WEAPON
        | EV_FIRE_WEAPONB
        | EV_FIRE_WEAPON_LASTSHOT
        | EV_FIRE_WEAPON_MG42
        | EV_FIRE_QUADBARREL_1
        | EV_FIRE_QUADBARREL_2 => match ctx.muzzles.get(&ev.entity_num) {
            Some(&(pos, dir)) => {
                let path = ctx
                    .weapon_flash
                    .get(&ev.weapon)
                    .cloned()
                    .unwrap_or_else(|| flash_effect_path(ev.event).to_string());
                vec![Resolved::Spawn {
                    path,
                    at: SpawnAt::Directed { pos, dir },
                }]
            }
            None => vec![Resolved::Known],
        },

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
                });
            }
            known_or(out)
        }

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
        | EV_EJECT_BRASS
        | EV_MELEE_SWIPE
        | EV_FIRE_MELEE
        | EV_MELEE_HIT
        | EV_MELEE_MISS
        | EV_BULLET_TRACER
        | EV_SOUND_ALIAS
        | EV_BULLET_HIT_CLIENT_SMALL
        | EV_BULLET_HIT_CLIENT_LARGE
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
        | EV_PLAY_FX
        | EV_PLAY_FX_DIR
        | EV_PLAY_FX_ON_TAG
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
    use vcod_common::net::events::BYTE_DIRS;

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
            other_entity_num: 40,
            attacker_entity_num: -1,
        }
    }

    fn empty_ctx() -> ResolveCtx<'static> {
        static EMPTY_MUZZLES: OnceLock<HashMap<u32, (Vec3, Vec3)>> = OnceLock::new();
        static EMPTY_FLASH: OnceLock<HashMap<i32, String>> = OnceLock::new();
        ResolveCtx {
            muzzles: EMPTY_MUZZLES.get_or_init(HashMap::new),
            weapon_flash: EMPTY_FLASH.get_or_init(HashMap::new),
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
            } => {
                assert_eq!(path, "fx/impacts/test.efx");
                assert_eq!(*pos, Vec3::new(10.0, 20.0, 30.0));
                assert_eq!(*normal, Vec3::from(BYTE_DIRS[0]));
            }
            other => panic!("{other:?}"),
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
            muzzles: &muzzles,
            weapon_flash: &weapon_flash,
        };
        let mut e = ev(EV_BULLET_HIT_SMALL, 0, 5, [10.0, 0.0, 0.0]); // concrete, blank cell
        e.other_entity_num = 7;
        let rs = resolve_with(&e, &ImpactTable::default(), &ctx);
        assert_eq!(rs.len(), 1, "{rs:?}"); // impact itself was a blank cell
        match &rs[0] {
            Resolved::Tracer { muzzle, impact } => {
                assert_eq!(*muzzle, Vec3::ZERO);
                assert_eq!(*impact, Vec3::new(10.0, 0.0, 0.0));
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn flesh_hits_get_a_tracer() {
        let mut muzzles = HashMap::new();
        muzzles.insert(7u32, (Vec3::ZERO, Vec3::X));
        let weapon_flash = HashMap::new();
        let ctx = ResolveCtx {
            muzzles: &muzzles,
            weapon_flash: &weapon_flash,
        };
        let mut e = ev(EV_BULLET_HIT_SMALL, 0, 7, [10.0, 0.0, 0.0]);
        e.other_entity_num = 7;
        let rs = resolve_with(&e, &ImpactTable::default(), &ctx);
        assert!(
            rs.iter().any(|r| matches!(r, Resolved::Tracer { .. })),
            "{rs:?}"
        );
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
            EV_BULLET_HIT_CLIENT_SMALL,
            EV_BULLET_HIT_CLIENT_LARGE,
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
            other_entity_num: 0,
            attacker_entity_num: -1,
        };
        let rs = resolve(
            &ev,
            &ResolveCtx {
                muzzles: &muzzles,
                weapon_flash: &weapon_flash,
            },
        );
        assert_eq!(rs.len(), 1, "{rs:?}");
        match &rs[0] {
            Resolved::Spawn {
                path,
                at: SpawnAt::Directed { pos, dir },
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
            other_entity_num: u32::MAX,
            attacker_entity_num: -1,
        };
        let rs = resolve(
            &ev,
            &ResolveCtx {
                muzzles: &muzzles,
                weapon_flash: &weapon_flash,
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
            muzzles: &muzzles,
            weapon_flash: &weapon_flash,
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
            muzzles: &muzzles,
            weapon_flash: &weapon_flash,
        };
        let mut e = ev(EV_BULLET_HIT_SMALL, 0, 1, [10.0, 20.0, 30.0]);
        e.other_entity_num = 7;

        let rs = resolve_with(&e, &table, &ctx);
        assert_eq!(rs.len(), 2, "{rs:?}");
        match &rs[0] {
            Resolved::Spawn {
                path,
                at: SpawnAt::Surface { .. },
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
            } => {
                assert_eq!(path, "fx/explosions/grenade2.efx");
                assert_eq!(*pos, Vec3::new(5.0, 5.0, 0.0));
                assert_eq!(*normal, Vec3::from(BYTE_DIRS[0]));
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn rocket_explode_and_nomarks_both_resolve_the_same_row() {
        let mut table = ImpactTable::default();
        table.map.insert(
            (ImpactKind::RocketExplode, 6), // dirt
            "fx/impacts/newimps/v_blast1dirt.efx".to_string(),
        );
        for id in [EV_ROCKET_EXPLODE, EV_ROCKET_EXPLODE_NOMARKS] {
            let e = ev(id, 0, 6, [0.0, 0.0, 0.0]);
            let rs = resolve_with(&e, &table, &empty_ctx());
            match &rs[..] {
                [Resolved::Spawn { path, .. }] => {
                    assert_eq!(path, "fx/impacts/newimps/v_blast1dirt.efx", "event {id}")
                }
                other => panic!("event {id}: {other:?}"),
            }
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
}
