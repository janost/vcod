//! Bullets and what they hit: means of death, the hit-location table, the
//! box partition a hit point resolves to, and the shot itself
//! (`docs/research/cod11-combat.md`, sections 2 to 4).

use crate::game::temp_entity::{Scope, TempEntity};
use crate::spectate::{ClientSim, PmType};
use glam::Vec3;
use vcod_common::collision::{sound_material, CollisionWorld};
use vcod_common::net::events::dir_to_byte;
use vcod_common::pk3::Pk3Fs;
use vcod_common::weapon::WeaponDef;

/// The 25 names, in the order of the pointer table at `.so` file offset
/// `0x7cda0` (`docs/research/cod11-hud-protocol.md` section 2). The index is
/// the enum value the obituary encodes.
pub const MOD_NAMES: [&str; 25] = [
    "MOD_UNKNOWN",
    "MOD_PISTOL_BULLET",
    "MOD_RIFLE_BULLET",
    "MOD_GRENADE",
    "MOD_GRENADE_SPLASH",
    "MOD_PROJECTILE",
    "MOD_PROJECTILE_SPLASH",
    "MOD_MELEE",
    "MOD_HEAD_SHOT",
    "MOD_MORTAR",
    "MOD_MORTAR_SPLASH",
    "MOD_KICKED",
    "MOD_GRABBER",
    "MOD_DYNAMITE",
    "MOD_DYNAMITE_SPLASH",
    "MOD_AIRSTRIKE",
    "MOD_WATER",
    "MOD_SLIME",
    "MOD_LAVA",
    "MOD_CRUSH",
    "MOD_TELEFRAG",
    "MOD_FALLING",
    "MOD_SUICIDE",
    "MOD_TRIGGER_HURT",
    "MOD_EXPLOSIVE",
];

/// The seven means of death the obituary sends as `0x80 | mod` instead of a
/// weapon index; every other one sends the weapon's configstring 7 index
/// (hud protocol doc section 2, "Which deaths get the `0x80` flag").
pub const MOD_FLAGGED: [i32; 7] = [7, 8, 16, 17, 19, 21, 22];

/// The enum value of a `MOD_*` name, or `None` for a name the table has no
/// row for. Script string values intern exactly, not folded
/// (`docs/research/cod11-gsc-language.md`), and every stock call site spells
/// the name the way the table does.
pub fn mod_index(name: &str) -> Option<i32> {
    MOD_NAMES.iter().position(|n| *n == name).map(|i| i as i32)
}

/// `iDFlags`, as `_callbacksetup.gsc` defines them for the damage callback.
pub const DFLAG_RADIUS: i32 = 1;
pub const DFLAG_NO_ARMOR: i32 = 2;
pub const DFLAG_NO_KNOCKBACK: i32 = 4;
pub const DFLAG_NO_TEAM_PROTECTION: i32 = 8;
pub const DFLAG_NO_PROTECTION: i32 = 16;
pub const DFLAG_PASSTHRU: i32 = 32;

/// The 19 hit locations in retail's index order, the pointer table at `.so`
/// `0x7DD20` (combat doc, section 3).
pub const HITLOC_NAMES: [&str; 19] = [
    "none",
    "helmet",
    "head",
    "neck",
    "torso_upper",
    "torso_lower",
    "right_arm_upper",
    "left_arm_upper",
    "right_arm_lower",
    "left_arm_lower",
    "right_hand",
    "left_hand",
    "right_leg_upper",
    "left_leg_upper",
    "right_leg_lower",
    "left_leg_lower",
    "right_foot",
    "left_foot",
    "gun",
];

/// The pak file `G_ParseHitLocDmgTable` reads, and its header.
const HITLOC_TABLE_PATH: &str = "info/mp_lochit_dmgtable";
const HITLOC_TABLE_HEADER: &str = "LOCDMGTABLE";

/// `g_fHitLocDamageMult`: one damage multiplier per hit location, indexed
/// like `HITLOC_NAMES`.
#[derive(Clone, Debug, PartialEq)]
pub struct HitLocTable {
    mult: [f32; 19],
}

impl Default for HitLocTable {
    /// The state the parser overwrites: every location at 1, `gun` at 0
    /// (combat doc, section 3.1).
    fn default() -> Self {
        let mut mult = [1.0; 19];
        mult[18] = 0.0;
        HitLocTable { mult }
    }
}

impl HitLocTable {
    /// The shipped table out of the paks. Retail `Com_Error`s on a missing or
    /// malformed file and never starts; a server here keeps serving on the
    /// all-ones default and says so.
    pub fn load(fs: &Pk3Fs) -> HitLocTable {
        let Some(bytes) = fs.read(HITLOC_TABLE_PATH) else {
            log::error!(
                "{HITLOC_TABLE_PATH} not in the mounted paks; every hit location does full damage"
            );
            return HitLocTable::default();
        };
        match HitLocTable::parse(&String::from_utf8_lossy(&bytes)) {
            Ok(t) => t,
            Err(e) => {
                log::error!("{HITLOC_TABLE_PATH}: {e}; every hit location does full damage");
                HitLocTable::default()
            }
        }
    }

    /// `LOCDMGTABLE\name\mult\name\mult...`, an Info string behind a header.
    /// A name the table has no row for is an error, as retail's parse spec
    /// makes it; a location the file leaves out keeps the default.
    pub fn parse(text: &str) -> Result<HitLocTable, String> {
        let body = text
            .trim()
            .strip_prefix(HITLOC_TABLE_HEADER)
            .ok_or_else(|| "does not appear to be a hitloc damage table".to_string())?;
        let mut table = HitLocTable::default();
        let mut fields = body.split('\\').skip(1);
        while let Some(name) = fields.next() {
            let value = fields
                .next()
                .ok_or_else(|| format!("{name} has no value"))?;
            let i = HITLOC_NAMES
                .iter()
                .position(|n| *n == name)
                .ok_or_else(|| format!("{name} is not a hit location"))?;
            table.mult[i] = value
                .trim()
                .parse()
                .map_err(|_| format!("{name}: {value:?} is not a number"))?;
        }
        Ok(table)
    }

    /// The multiplier for a location name; an unknown name is `none`, which
    /// is what `G_GetHitLocationIndexFromString` returns for one.
    pub fn multiplier(&self, name: &str) -> f32 {
        let i = HITLOC_NAMES.iter().position(|n| *n == name).unwrap_or(0);
        self.mult[i]
    }
}

/// Where a point on a player's box lands, as one of `HITLOC_NAMES`.
///
/// Retail's partition lives inside `trap_LocationalTrace` in the engine
/// binary, per bone, and nothing read pins it (combat doc, section 3). This
/// is a partition of the link box by height fraction and side, INFERRED,
/// with the one measurement there is: a level shot from eye height (60 of a
/// 70-unit box) read `head` on retail (section 8.4's `MOD_HEAD_SHOT`). The
/// fractions are the doc's "As implemented" note under section 3.
pub fn hitloc(
    point: [f32; 3],
    victim_origin: [f32; 3],
    victim_yaw_rad: f32,
    mins: [f32; 3],
    maxs: [f32; 3],
) -> &'static str {
    let height = maxs[2] - mins[2];
    if height <= 0.0 {
        return "none";
    }
    let frac = ((point[2] - victim_origin[2] - mins[2]) / height).clamp(0.0, 1.0);
    // Lateral offset in the body's frame, positive to its right (the same
    // `right` vector `PlayerState::view` uses).
    let right = Vec3::new(victim_yaw_rad.sin(), -victim_yaw_rad.cos(), 0.0);
    let lateral = (Vec3::from(point) - Vec3::from(victim_origin)).dot(right);
    let half_width = (maxs[0] - mins[0]) * 0.5;
    let side = if lateral < 0.0 { "left" } else { "right" };
    let sided = |part: &str| -> &'static str {
        let name = format!("{side}_{part}");
        HITLOC_NAMES
            .iter()
            .copied()
            .find(|n| *n == name)
            .unwrap_or("none")
    };
    if frac >= 0.94 {
        "helmet"
    } else if frac >= 0.85 {
        "head"
    } else if frac >= 0.80 {
        "neck"
    } else if frac >= 0.40 {
        if lateral.abs() > 0.6 * half_width {
            if frac >= 0.65 {
                sided("arm_upper")
            } else if frac >= 0.55 {
                sided("arm_lower")
            } else {
                sided("hand")
            }
        } else if frac >= 0.55 {
            "torso_upper"
        } else {
            "torso_lower"
        }
    } else if frac >= 0.20 {
        sided("leg_upper")
    } else if frac >= 0.05 {
        sided("leg_lower")
    } else {
        sided("foot")
    }
}

/// One player hit, ready for `CodeCallback_PlayerDamage`.
#[derive(Clone, Debug)]
pub struct Hit {
    pub victim: usize,
    pub attacker: usize,
    pub damage: i32,
    pub dflags: i32,
    pub mod_: &'static str,
    pub weapon: String,
    pub point: [f32; 3],
    /// The shot's forward, which is the direction `G_Damage` is handed
    /// (combat doc, section 2.4, step 4).
    pub dir: [f32; 3],
    pub hitloc: &'static str,
}

pub struct ShotResult {
    /// The impact event: a wall hit for everyone, or a flesh hit for everyone
    /// but the victim. `None` when the bullet hit nothing, or a surface that
    /// asks for no impact.
    pub impact: Option<TempEntity>,
    pub hit: Option<Hit>,
}

/// `EV_BULLET_HIT_SMALL` / `EV_BULLET_HIT_LARGE`
/// (`docs/research/cod11-events-and-fx.md` section 1).
const EV_BULLET_HIT_SMALL: i32 = 173;
const EV_BULLET_HIT_LARGE: i32 = 174;
/// The flesh `surfType` `finishPlayerDamage` hardcodes (combat doc, 4.5).
const SURF_FLESH: i32 = 7;
/// The surface flag that suppresses the impact effect (combat doc, 2.3).
const SURF_NO_IMPACT: u32 = 0x4;
/// How far a bullet travels: `muzzle + forward * 8192` (combat doc, 2.2).
const BULLET_RANGE: f32 = 8192.0;

/// The cone's half-angle in degrees for this shot (combat doc, 2.1): the
/// stance's hip minimum blended toward `hipSpreadMax` by `aimSpreadScale`,
/// or the same blend from `adsSpread` when aiming down the sight.
fn spread_deg(def: &WeaponDef, sim: &ClientSim, ads: bool) -> f32 {
    use vcod_common::pmove::Stance;
    let scale = sim.aim_spread_scale / 255.0;
    let min = if ads {
        def.ads_spread
    } else {
        match sim.ps.stance {
            Stance::Prone => def.hip_spread_prone_min,
            Stance::Crouch => def.hip_spread_ducked_min,
            Stance::Stand => def.hip_spread_stand_min,
        }
    };
    min + (def.hip_spread_max - min) * scale
}

/// `gunrandom` (combat doc, 2.2): a point in the unit disc with the radius
/// drawn uniformly, so the cone is denser at its centre.
fn gun_random(rng: &mut u64) -> (f32, f32) {
    let angle = crate::game::host::rand_unit(rng) * std::f32::consts::TAU;
    let radius = crate::game::host::rand_unit(rng);
    (radius * angle.cos(), radius * angle.sin())
}

/// Ray-vs-box: the entry fraction along `start..end`, if the segment crosses
/// the box at all.
fn ray_box(start: Vec3, end: Vec3, lo: Vec3, hi: Vec3) -> Option<f32> {
    let d = end - start;
    let (mut t0, mut t1) = (0.0f32, 1.0f32);
    for axis in 0..3 {
        if d[axis].abs() < 1e-6 {
            if start[axis] < lo[axis] || start[axis] > hi[axis] {
                return None;
            }
            continue;
        }
        let inv = 1.0 / d[axis];
        let (mut a, mut b) = (
            (lo[axis] - start[axis]) * inv,
            (hi[axis] - start[axis]) * inv,
        );
        if a > b {
            std::mem::swap(&mut a, &mut b);
        }
        t0 = t0.max(a);
        t1 = t1.min(b);
        if t0 > t1 {
            return None;
        }
    }
    Some(t0)
}

/// A player's shot: from the eye along the view with spread, against the
/// world and every live player's box; the nearer wins (combat doc, section
/// 2). `sims` is every client with a sim, the shooter among them; `ads` is
/// the trigger cmd's sight bit; `weapon_name` is what the callback is told
/// (`BG_GetInfoForWeapon(weapon)->name`). Damage is `weaponDef.damage`
/// through the hit-location table with no distance term (2.4). A rifle
/// round's pass through the first player at half damage (2.4, step 5) is
/// not modelled: the shot stops at its first hit.
#[allow(clippy::too_many_arguments)]
pub fn bullet_fire(
    shooter: usize,
    def: &WeaponDef,
    weapon_name: &str,
    ads: bool,
    sims: &[(usize, &ClientSim)],
    world: Option<&CollisionWorld>,
    hitlocs: &HitLocTable,
    rng: &mut u64,
) -> ShotResult {
    let none = ShotResult {
        impact: None,
        hit: None,
    };
    let Some((_, me)) = sims.iter().find(|(slot, _)| *slot == shooter) else {
        return none;
    };
    // The muzzle: eye plus lean, each component rounded (2.1).
    let muzzle = me.ps.view().eye.round();
    let (yaw, pitch) = (me.ps.yaw, me.ps.pitch);
    let forward = Vec3::new(
        pitch.cos() * yaw.cos(),
        pitch.cos() * yaw.sin(),
        pitch.sin(),
    );
    let right = Vec3::new(yaw.sin(), -yaw.cos(), 0.0);
    let up = right.cross(forward);
    let (x, y) = gun_random(rng);
    let r = spread_deg(def, me, ads).to_radians().tan() * BULLET_RANGE;
    let end = muzzle + forward * BULLET_RANGE + right * (x * r) + up * (y * r);

    let trace = world.map(|w| w.shot_trace(muzzle, end));
    let mut nearest = trace.as_ref().map_or(1.0, |t| t.fraction);
    let mut victim: Option<(usize, &ClientSim, f32)> = None;
    for (slot, sim) in sims {
        if *slot == shooter || sim.pm_type != PmType::Normal || sim.dead {
            continue;
        }
        let lo = sim.ps.origin + sim.ps.mins();
        let hi = sim.ps.origin + sim.ps.maxs();
        if let Some(t) = ray_box(muzzle, end, lo, hi) {
            if t < nearest {
                nearest = t;
                victim = Some((*slot, sim, t));
            }
        }
    }
    let event = if def.sounds.rifle_bullet {
        EV_BULLET_HIT_LARGE
    } else {
        EV_BULLET_HIT_SMALL
    };
    let (mod_, dflags) = if def.sounds.rifle_bullet {
        ("MOD_RIFLE_BULLET", DFLAG_PASSTHRU)
    } else {
        ("MOD_PISTOL_BULLET", 0)
    };
    if let Some((slot, sim, t)) = victim {
        let point = muzzle + (end - muzzle) * t;
        let loc = hitloc(
            point.into(),
            sim.ps.origin.into(),
            sim.ps.yaw,
            sim.ps.mins().into(),
            sim.ps.maxs().into(),
        );
        // Truncated toward zero, `G_Damage`'s explicit `fldcw` (4.2).
        let damage = (def.damage as f32 * hitlocs.multiplier(loc)) as i32;
        return ShotResult {
            impact: Some(TempEntity {
                event,
                parm: dir_to_byte(forward.into()),
                surf_type: SURF_FLESH,
                other: shooter as u32,
                // `G_TempEntity` zeroes the state; only the obituary fills it.
                attacker: 0,
                origin: point.into(),
                scope: Scope::AllBut(slot),
            }),
            hit: Some(Hit {
                victim: slot,
                attacker: shooter,
                damage,
                dflags,
                mod_,
                weapon: weapon_name.to_string(),
                point: point.into(),
                dir: forward.into(),
                hitloc: loc,
            }),
        };
    }
    let Some(t) = trace.filter(|t| t.fraction < 1.0) else {
        return none;
    };
    if t.surface_flags & SURF_NO_IMPACT != 0 {
        return none;
    }
    ShotResult {
        impact: Some(TempEntity {
            event,
            parm: dir_to_byte(t.normal.into()),
            surf_type: sound_material(t.surface_flags),
            other: shooter as u32,
            attacker: 0,
            origin: t.endpos.into(),
            scope: Scope::Broadcast,
        }),
        hit: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn the_flagged_mods_are_the_seven_the_builtin_tags() {
        assert_eq!(mod_index("MOD_MELEE"), Some(7));
        assert_eq!(mod_index("MOD_HEAD_SHOT"), Some(8));
        assert_eq!(mod_index("MOD_SUICIDE"), Some(22));
        assert_eq!(mod_index("MOD_EXPLOSIVE"), Some(24));
        assert_eq!(mod_index("MOD_NOT_A_THING"), None);
        for m in MOD_FLAGGED {
            assert!(MOD_NAMES.get(m as usize).is_some());
        }
        // The two the client draws a weapon icon for, not a MOD icon.
        assert!(!MOD_FLAGGED.contains(&mod_index("MOD_RIFLE_BULLET").unwrap()));
        assert!(!MOD_FLAGGED.contains(&mod_index("MOD_TRIGGER_HURT").unwrap()));
    }

    /// The box partition, by height fraction and side. The one measured
    /// point is eye height reading `head`; the rest is the doc's INFERRED
    /// partition, pinned so a change to it is deliberate.
    #[test]
    fn hitloc_partitions_the_box_by_height_and_side() {
        let (mins, maxs) = ([-15.0, -15.0, 0.0], [15.0, 15.0, 70.0]);
        let o = [0.0, 0.0, 0.0];
        assert_eq!(hitloc([0.0, 0.0, 68.0], o, 0.0, mins, maxs), "helmet");
        assert_eq!(hitloc([0.0, 0.0, 64.0], o, 0.0, mins, maxs), "head");
        assert_eq!(hitloc([0.0, 0.0, 60.0], o, 0.0, mins, maxs), "head");
        assert_eq!(hitloc([0.0, 0.0, 57.0], o, 0.0, mins, maxs), "neck");
        assert_eq!(hitloc([0.0, 0.0, 50.0], o, 0.0, mins, maxs), "torso_upper");
        assert_eq!(hitloc([0.0, 0.0, 36.0], o, 0.0, mins, maxs), "torso_lower");
        assert_eq!(
            hitloc([0.0, 8.0, 10.0], o, 0.0, mins, maxs),
            "left_leg_lower"
        );
        assert_eq!(
            hitloc([0.0, -8.0, 10.0], o, 0.0, mins, maxs),
            "right_leg_lower"
        );
        assert_eq!(
            hitloc([0.0, 5.0, 20.0], o, 0.0, mins, maxs),
            "left_leg_upper"
        );
        assert_eq!(hitloc([0.0, 0.0, 2.0], o, 0.0, mins, maxs), "right_foot");
        // Wide of the torso is an arm, and the side follows the body's yaw:
        // facing +x, the body's right is -y.
        assert_eq!(
            hitloc([0.0, 12.0, 50.0], o, 0.0, mins, maxs),
            "left_arm_upper"
        );
        assert_eq!(
            hitloc([0.0, -12.0, 40.0], o, 0.0, mins, maxs),
            "right_arm_lower"
        );
        assert_eq!(hitloc([0.0, -12.0, 30.0], o, 0.0, mins, maxs), "right_hand");
        let quarter = std::f32::consts::FRAC_PI_2;
        assert_eq!(
            hitloc([12.0, 0.0, 50.0], o, quarter, mins, maxs),
            "right_arm_upper"
        );
        // The box travels with the body.
        let far = [1000.0, 2000.0, -30.0];
        assert_eq!(hitloc([1000.0, 2000.0, 34.0], far, 0.0, mins, maxs), "head");
    }

    #[test]
    fn the_table_parses_the_shipped_format_and_refuses_a_stranger() {
        let t = HitLocTable::parse("LOCDMGTABLE\\head\\1.5\\left_foot\\0.4").unwrap();
        assert_eq!(t.multiplier("head"), 1.5);
        assert_eq!(t.multiplier("left_foot"), 0.4);
        assert_eq!(
            t.multiplier("torso_upper"),
            1.0,
            "an unlisted location keeps the default"
        );
        assert_eq!(t.multiplier("gun"), 0.0);
        assert_eq!(t.multiplier("nowhere"), 1.0, "an unknown name is `none`");
        assert!(HitLocTable::parse("\\head\\1.5").is_err());
        assert!(HitLocTable::parse("LOCDMGTABLE\\elbow\\1.5").is_err());
        assert_eq!(
            (45.0 * t.multiplier("head")) as i32,
            67,
            "section 8.4's head hit"
        );
    }

    /// The paks' own table: the head multiplier that turns a 45-damage
    /// carbine into the 67 the retail capture measured (combat doc, 8.4).
    #[test]
    fn the_shipped_table_gives_the_head_one_and_a_half() {
        let Some(fs) = vcod_common::testing::game_fs() else {
            return;
        };
        let t = HitLocTable::load(&fs);
        assert_eq!(t.multiplier("head"), 1.5);
        assert_eq!(t.multiplier("helmet"), 1.5);
        assert_eq!(t.multiplier("torso_upper"), 0.9);
        assert_eq!(t.multiplier("left_foot"), 0.4);
        assert_eq!(t.multiplier("gun"), 0.0);
        assert_eq!((45.0 * t.multiplier("head")) as i32, 67);
    }

    fn new_for_test(origin: [f32; 3], yaw_deg: f32) -> ClientSim {
        let mut sim = ClientSim::spectator(origin, yaw_deg, [0; 3]);
        sim.become_player(origin, yaw_deg, [0; 3]);
        sim
    }

    fn zero_spread_carbine() -> WeaponDef {
        let mut m = HashMap::new();
        m.insert("damage".to_string(), "45".to_string());
        m.insert("rifleBullet".to_string(), "1".to_string());
        m.insert("weaponClass".to_string(), "rifle".to_string());
        WeaponDef::from_map(&m)
    }

    fn fire(
        def: &WeaponDef,
        sims: &[(usize, &ClientSim)],
        world: &CollisionWorld,
        rng: &mut u64,
    ) -> ShotResult {
        let table = HitLocTable::default();
        bullet_fire(
            0,
            def,
            "m1carbine_mp",
            false,
            sims,
            Some(world),
            &table,
            rng,
        )
    }

    #[test]
    fn a_shot_at_a_player_in_the_open_hits_it_and_a_shot_at_the_floor_hits_the_world() {
        // Two sims 100 units apart on the test floor; shooter yaw 0 faces +x.
        let world = vcod_common::collision::test_world(&[]);
        let table = HitLocTable::default();
        let mut a = new_for_test([0.0, 0.0, 0.0], 0.0);
        let b = new_for_test([100.0, 0.0, 0.0], 180.0);
        let def = zero_spread_carbine();
        let mut rng = 1u64;
        let r = fire(&def, &[(0, &a), (1, &b)], &world, &mut rng);
        let hit = r.hit.expect("the shot reached B");
        assert_eq!(hit.victim, 1);
        assert_eq!(hit.attacker, 0);
        assert_eq!(
            hit.hitloc, "head",
            "a level shot from the eye lands at eye height"
        );
        assert_eq!(hit.damage, (45.0 * table.multiplier(hit.hitloc)) as i32);
        assert_eq!(hit.mod_, "MOD_RIFLE_BULLET");
        assert_eq!(hit.dflags, DFLAG_PASSTHRU);
        assert_eq!(hit.weapon, "m1carbine_mp");
        assert!(
            (hit.point[0] - 85.0).abs() < 0.01,
            "the box's near face, {:?}",
            hit.point
        );
        assert!((hit.dir[0] - 1.0).abs() < 1e-5);
        let te = r.impact.expect("a flesh impact");
        assert_eq!((te.event, te.surf_type), (174, 7));
        assert_eq!(te.scope, Scope::AllBut(1));
        assert_eq!(te.other, 0);

        // Looking down: the floor, broadcast, with the floor's material.
        a.ps.pitch = -1.2;
        let r = fire(&def, &[(0, &a), (1, &b)], &world, &mut rng);
        assert!(r.hit.is_none());
        let te = r.impact.expect("a wall impact");
        assert_eq!(te.event, 174);
        assert_ne!(te.surf_type, 7);
        assert_eq!(te.scope, Scope::Broadcast);
        assert!(te.origin[2].abs() < 0.2, "on the floor, {:?}", te.origin);
    }

    /// The trace picks the nearer of the world and a player: a wall between
    /// the two stops the bullet, and a dead player is not in the way.
    #[test]
    fn a_wall_shields_and_a_corpse_does_not_block() {
        let a = new_for_test([0.0, 0.0, 0.0], 0.0);
        let mut b = new_for_test([100.0, 0.0, 0.0], 180.0);
        let def = zero_spread_carbine();
        let mut rng = 1u64;
        let wall = vcod_common::collision::test_world(&[(
            Vec3::new(40.0, -64.0, 0.0),
            Vec3::new(48.0, 64.0, 128.0),
        )]);
        let r = fire(&def, &[(0, &a), (1, &b)], &wall, &mut rng);
        assert!(r.hit.is_none(), "the wall is nearer than B");
        assert!(r.impact.is_some_and(|te| (te.origin[0] - 40.0).abs() < 0.2));

        let open = vcod_common::collision::test_world(&[]);
        b.dead = true;
        let r = fire(&def, &[(0, &a), (1, &b)], &open, &mut rng);
        assert!(r.hit.is_none(), "a dead player is not hit again");
        assert!(
            r.impact.is_none(),
            "and the open floor is out of range of a level shot"
        );

        // A pistol round names the pistol means of death and passes nothing.
        b.dead = false;
        let mut m = HashMap::new();
        m.insert("damage".to_string(), "30".to_string());
        let colt = WeaponDef::from_map(&m);
        let r = fire(&colt, &[(0, &a), (1, &b)], &open, &mut rng);
        let hit = r.hit.unwrap();
        assert_eq!(
            (hit.mod_, hit.dflags, hit.damage),
            ("MOD_PISTOL_BULLET", 0, 30)
        );
        assert_eq!(r.impact.unwrap().event, 173);
    }
}
