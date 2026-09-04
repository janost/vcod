//! Bullets and what they hit: means of death, the hit-location table, the
//! box partition a hit point resolves to, and the shot itself
//! (`docs/research/cod11-combat.md`, sections 2 to 4).

use crate::game::hitrig::HitRigs;
use crate::game::temp_entity::{Scope, TempEntity};
use crate::spectate::{ClientSim, PmType};
use glam::{Quat, Vec3};
use vcod_common::animtree::PlayerAnims;
use vcod_common::bonetrace::{bone_trace, PriorityMap};
use vcod_common::collision::{sound_material, CollisionWorld};
use vcod_common::net::events::dir_to_byte;
use vcod_common::pk3::Pk3Fs;
use vcod_common::playerpose::pose_player;
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
    /// (combat doc, section 3.5).
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

/// `bulletPriorityMap` and `riflePriorityMap`, the two 19-byte tables the
/// engine ranks bone candidates with (combat doc, 2.3). A `rifleBullet`
/// weapon takes the second, everything else the first.
pub const BULLET_PRIORITY: PriorityMap = [1, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 0];
pub const RIFLE_PRIORITY: PriorityMap = [1, 9, 9, 9, 8, 7, 6, 6, 6, 6, 5, 5, 4, 4, 4, 4, 3, 3, 0];

/// What the locational trace needs beyond the shot itself: the paks to load a
/// victim's models out of, the animtree its clip names come from, and the rig
/// cache. Without one a hit still lands, at hit location `none`.
pub struct BoneTraceCtx<'a> {
    pub fs: &'a vcod_common::pk3::Pk3Fs,
    pub anims: &'a PlayerAnims,
    pub rigs: &'a mut HitRigs,
    /// serverTime the shot is posed at.
    pub now_ms: i32,
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
/// or the same blend from `adsSpread` off a settled sight. `ads` is
/// `fWeaponPosFrac == 1.0`, the exact compare retail's two arms split on,
/// and not the usercmd's sight bit: the sight is up for the whole of a
/// transition and the cone is the hip one until it lands.
fn spread_deg(def: &WeaponDef, sim: &ClientSim, ads: bool) -> f32 {
    use vcod_common::pmove::Stance;
    let scale = sim.ps.aim_spread_scale / 255.0;
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
/// world and every live player's box, and then against the bones of whoever
/// the box test found (combat doc, sections 2 and 3). `sims` is every client
/// with a sim, the shooter among them; `ads` is whether the shot left a
/// settled sight; `weapon_name` is what the callback is told
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
    mut bones: Option<&mut BoneTraceCtx>,
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
    let world_fraction = trace.as_ref().map_or(1.0, |t| t.fraction);
    // The link box is the broad phase only: retail's locational trace then
    // has to score a bone, and a ray can cross the column and meet none
    // (combat doc, section 3). Candidates in the order the ray reaches them,
    // and the first one whose bones it does score is the victim.
    let mut candidates: Vec<(usize, &ClientSim, f32)> = sims
        .iter()
        .filter(|(slot, sim)| *slot != shooter && sim.pm_type == PmType::Normal && !sim.dead)
        .filter_map(|(slot, sim)| {
            let lo = sim.ps.origin + sim.ps.mins();
            let hi = sim.ps.origin + sim.ps.maxs();
            let t = ray_box(muzzle, end, lo, hi)?;
            (t < world_fraction).then_some((*slot, *sim, t))
        })
        .collect();
    candidates.sort_by(|a, b| a.2.total_cmp(&b.2));
    let priority = if def.sounds.rifle_bullet {
        &RIFLE_PRIORITY
    } else {
        &BULLET_PRIORITY
    };
    let mut victim: Option<(usize, &ClientSim, f32, &'static str)> = None;
    for (slot, sim, box_t) in candidates {
        // No paks, no rig: the shot still lands, at no location, which the
        // shipped multiplier table reads as full damage.
        let Some(ctx) = bones.as_mut() else {
            victim = Some((slot, sim, box_t, "none"));
            break;
        };
        let BoneTraceCtx {
            fs,
            anims,
            rigs,
            now_ms,
        } = &mut **ctx;
        let Some(skel) = rigs.rig(fs, &sim.assembly) else {
            victim = Some((slot, sim, box_t, "none"));
            break;
        };
        let inputs = sim.pose_inputs(anims, *now_ms);
        let pose = pose_player(&skel, &inputs, |name| rigs.clip(fs, name));
        // Into the victim's own frame: a player entity is yaw-only and its
        // `tag_origin` sits at the feet.
        let inv = Quat::from_rotation_z(-sim.ps.yaw);
        let (ls, le) = (inv * (muzzle - sim.ps.origin), inv * (end - sim.ps.origin));
        let Some(hit) = bone_trace(&skel, &pose, ls, le, priority) else {
            continue;
        };
        let loc = HITLOC_NAMES
            .get(hit.hit_location as usize)
            .copied()
            .unwrap_or("none");
        victim = Some((slot, sim, hit.fraction, loc));
        break;
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
    if let Some((slot, _sim, t, loc)) = victim {
        let point = muzzle + (end - muzzle) * t;
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

    /// Fires with no trace context, which is the no-paks path: the box hit
    /// stands and carries no location.
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
            None,
            rng,
        )
    }

    /// The stock American body, head and helmet, the assembly the character
    /// script dresses a client in.
    fn stock_assembly() -> crate::game::hitrig::Assembly {
        crate::game::hitrig::Assembly {
            body: "playerbody_american_airborne".into(),
            attachments: vec![
                ("basehead2".into(), None),
                ("USAirborneHelmet".into(), Some("tag_helmet".into())),
            ],
        }
    }

    /// The one measured hit location there is (combat doc, 8.4): a level shot
    /// from eye height at a standing player 100 units away read `head` on
    /// retail, so the bone trace has to read it too.
    #[test]
    fn a_level_eye_height_shot_reads_head_through_the_bone_trace() {
        let Some(fs) = vcod_common::testing::game_fs() else {
            return;
        };
        let anims = vcod_common::animtree::PlayerAnims::load(&fs).expect("the player anims");
        let world = vcod_common::collision::test_world(&[]);
        let table = HitLocTable::load(&fs);
        let a = new_for_test([0.0, 0.0, 0.0], 0.0);
        let mut b = new_for_test([100.0, 0.0, 0.0], 180.0);
        b.assembly = stock_assembly();
        // The animscript selects nothing off the ground, and a sim no pmove
        // has stepped is airborne; without this B stands in bind pose.
        b.ps.on_ground = true;
        let inputs = crate::spectate::AnimInputs {
            anims: &anims,
            weapon: "m1carbine_mp",
            weapon_class: "rifle",
        };
        b.update_anims(&inputs, &vcod_common::net::msg::NULL_USERCMD, 0, &[]);
        let mut rigs = HitRigs::default();
        let mut ctx = BoneTraceCtx {
            fs: &fs,
            anims: &anims,
            rigs: &mut rigs,
            now_ms: 0,
        };
        let mut rng = 1u64;
        let r = bullet_fire(
            0,
            &zero_spread_carbine(),
            "m1carbine_mp",
            false,
            &[(0, &a), (1, &b)],
            Some(&world),
            &table,
            Some(&mut ctx),
            &mut rng,
        );
        let hit = r.hit.expect("the shot reached B");
        assert_eq!(hit.hitloc, "head");
        assert_eq!(hit.damage, 67, "45 through the head multiplier");
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
            hit.hitloc, "none",
            "with no rig to trace the hit carries no location"
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
