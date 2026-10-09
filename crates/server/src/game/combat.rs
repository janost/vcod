//! Bullets, swings and what they hit: means of death, the hit-location
//! table, the box partition a hit point resolves to, and the two attacks
//! themselves (`docs/research/cod11-combat.md`, sections 2 to 4).

use crate::game::hitrig::HitRigs;
use crate::game::temp_entity::{Scope, TempEntity};
use crate::spectate::ClientSim;
use glam::{Quat, Vec3};
use vcod_common::animtree::PlayerAnims;
use vcod_common::bonetrace::{PriorityMap, bone_trace};
use vcod_common::collision::{CONTENTS_GLASS, CollisionWorld, sound_material};
use vcod_common::net::events::dir_to_byte;
use vcod_common::net::protocol::{ENTITYNUM_NONE, ENTITYNUM_WORLD};
use vcod_common::pk3::Pk3Fs;
use vcod_common::playerpose::pose_player;
use vcod_common::pmove::PlayerState;
use vcod_common::pmove::weapon::{SpreadStance, hip_spread_min};
use vcod_common::weapon::WeaponDef;
use vcod_gsc::EntId;

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
    /// The entity the damage came out of, when it is not the attacker
    /// himself: a blast names the missile that went off. `None` on a bullet
    /// and a swing, where the two are the same.
    pub inflictor: Option<EntId>,
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

/// One thing an attack does to the frame.
pub enum Effect {
    /// A wall impact or a swing's hit or miss event. A bullet's flesh
    /// impacts are `finishPlayerDamage`'s ([`flesh_impacts`]).
    Impact(TempEntity),
    /// A player struck, for `CodeCallback_PlayerDamage`.
    Hit(Hit),
}

/// What an attack did, in retail's order: `Bullet_Fire_Extended` runs each
/// leg's `G_Damage` before it traces the next leg (combat doc 2.4, step 5),
/// and `Weapon_Melee` spawns its event ahead of its `G_Damage` (2.5), so a
/// caller applies these in sequence and the callback's temp entities number
/// between them.
#[derive(Default)]
pub struct ShotResult {
    pub effects: Vec<Effect>,
}

impl ShotResult {
    pub fn impacts(&self) -> impl Iterator<Item = &TempEntity> {
        self.effects.iter().filter_map(|e| match e {
            Effect::Impact(te) => Some(te),
            Effect::Hit(_) => None,
        })
    }

    pub fn hits(&self) -> impl Iterator<Item = &Hit> {
        self.effects.iter().filter_map(|e| match e {
            Effect::Hit(h) => Some(h),
            Effect::Impact(_) => None,
        })
    }
}

use vcod_common::net::event_ids::EV_BULLET_HIT_CLIENT_LARGE;
/// `EV_BULLET_HIT_CLIENT_SMALL` / `EV_BULLET_HIT_CLIENT_LARGE`, the victim's
/// copy of a flesh hit (`docs/research/cod11-events-and-fx.md` section 2).
use vcod_common::net::event_ids::EV_BULLET_HIT_CLIENT_SMALL;
use vcod_common::net::event_ids::EV_BULLET_HIT_LARGE;
/// `EV_BULLET_HIT_SMALL` / `EV_BULLET_HIT_LARGE`
/// (`docs/research/cod11-events-and-fx.md` section 1).
use vcod_common::net::event_ids::EV_BULLET_HIT_SMALL;
/// The flesh `surfType` `finishPlayerDamage` hardcodes (combat doc, 4.5).
const SURF_FLESH: i32 = 7;
/// The surface flag that suppresses the impact effect (combat doc, 2.3).
const SURF_NO_IMPACT: u32 = 0x4;
/// How far a bullet travels: `muzzle + forward * 8192` (combat doc, 2.2).
const BULLET_RANGE: f32 = 8192.0;
/// The deepest `Bullet_Fire_Extended` recursion that still traces; the
/// first call is depth 0 (combat doc, 2.3).
const MAX_BULLET_DEPTH: i32 = 12;
/// A glass leg's next start is `0.25 / d` past the pane along the ray, where
/// `d` is the cosine off the pane's normal, and none under 0.125
/// (`.rodata 0x79c64` and `0x79c60`, combat doc 2.4, step 3).
const GLASS_NUDGE: f32 = 0.25;
const GLASS_MIN_COS: f32 = 0.125;

/// The aim block's wire-convention degrees as the sim's radians: yaw as is,
/// pitch negated (positive down on the wire, up in the sim).
pub fn aim_radians(aim: [f32; 2]) -> (f32, f32) {
    (aim[1].to_radians(), -aim[0].to_radians())
}

/// The cone's half-angle in degrees for this shot (combat doc, 2.1): the
/// hip minimum `stance` gives blended toward `hipSpreadMax` by
/// `aimSpreadScale`, or the same blend from `adsSpread` off a settled sight.
/// `ads` is `fWeaponPosFrac == 1.0`, the exact compare retail's two arms
/// split on, and not the usercmd's sight bit: the sight is up for the whole
/// of a transition and the cone is the hip one until it lands.
fn spread_deg(def: &WeaponDef, sim: &ClientSim, ads: bool, stance: &SpreadStance) -> f32 {
    let scale = sim.ps.aim_spread_scale / 255.0;
    let min = if ads {
        def.ads_spread
    } else {
        hip_spread_min(def, stance)
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
pub(crate) fn ray_box(start: Vec3, end: Vec3, lo: Vec3, hi: Vec3) -> Option<f32> {
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

/// The means of death and the damage flags a bullet carries, off the weapon
/// file's `rifleBullet` (combat doc, 2.4). The stock bolt-actions spell it
/// `0`, so a kar98k, an enfield or a mosin logs `MOD_PISTOL_BULLET` however
/// little that reads like a rifle.
fn bullet_mod(rifle_bullet: bool) -> (&'static str, i32) {
    if rifle_bullet {
        ("MOD_RIFLE_BULLET", DFLAG_PASSTHRU)
    } else {
        ("MOD_PISTOL_BULLET", 0)
    }
}

/// Where a shot, a swing or a throw starts: the origin truncated toward zero
/// (`ClientEvents` reads the snapped `s.pos.trBase`), then the eye height and
/// lean, then each component truncated again (combat doc 2.1).
pub fn muzzle_point(ps: &PlayerState) -> Vec3 {
    // `PlayerState::view`'s offset, added to the snapped origin.
    let right = Vec3::new(ps.yaw.sin(), -ps.yaw.cos(), 0.0);
    (ps.origin.trunc() + Vec3::Z * ps.view_height() + right * ps.lean).trunc()
}

/// A player's shot: from the eye along the view with spread, against the
/// world and every live player's box, and then against the bones of whoever
/// the box test found (combat doc, sections 2 and 3). `sims` is every client
/// with a sim, the shooter among them; `ads` is whether the shot left a
/// settled sight; `stance` is what the hip minimum is read off; `aim` is the pitch and yaw the cmd's aim block left
/// (`ClientSim::aim_angles`, combat doc 15), which down a sight is the
/// swayed gun rather than the view; `weapon_name` is what the callback is
/// told (`BG_GetInfoForWeapon(weapon)->name`). Damage is `weaponDef.damage`
/// through the hit-location table with no distance term (2.4), halved at
/// each player a rifle round passes through (2.4, step 5).
#[allow(clippy::too_many_arguments)]
pub fn bullet_fire(
    shooter: usize,
    def: &WeaponDef,
    weapon_name: &str,
    ads: bool,
    stance: &SpreadStance,
    aim: [f32; 2],
    sims: &[(usize, &ClientSim)],
    world: Option<&CollisionWorld>,
    hitlocs: &HitLocTable,
    bones: Option<&mut BoneTraceCtx>,
    rng: &mut u64,
) -> ShotResult {
    let Some((_, me)) = sims.iter().find(|(slot, _)| *slot == shooter) else {
        return ShotResult::default();
    };
    let muzzle = muzzle_point(&me.ps);
    let (yaw, pitch) = aim_radians(aim);
    let forward = Vec3::new(
        pitch.cos() * yaw.cos(),
        pitch.cos() * yaw.sin(),
        pitch.sin(),
    );
    let right = Vec3::new(yaw.sin(), -yaw.cos(), 0.0);
    let up = right.cross(forward);
    let (x, y) = gun_random(rng);
    let r = spread_deg(def, me, ads, stance).to_radians().tan() * BULLET_RANGE;
    let end = muzzle + forward * BULLET_RANGE + right * (x * r) + up * (y * r);

    let round = Round {
        damage: def.damage,
        rifle_bullet: def.sounds.rifle_bullet,
        weapon_name,
    };
    fire_round(
        shooter, muzzle, end, forward, round, sims, world, hitlocs, bones,
    )
}

/// A turret's round (turrets doc 6.3): `Bullet_Fire` with no spread from the
/// muzzle the gun worked out, along `dir`. The callback is told the gunner's
/// carried weapon and names the gunner as the inflictor (turrets doc 13).
#[allow(clippy::too_many_arguments)]
pub fn bullet_fire_from(
    shooter: usize,
    muzzle: Vec3,
    dir: Vec3,
    damage: i32,
    rifle_bullet: bool,
    weapon_name: &str,
    sims: &[(usize, &ClientSim)],
    world: Option<&CollisionWorld>,
    hitlocs: &HitLocTable,
    bones: Option<&mut BoneTraceCtx>,
) -> ShotResult {
    let round = Round {
        damage,
        rifle_bullet,
        weapon_name,
    };
    let end = muzzle + dir * BULLET_RANGE;
    fire_round(
        shooter, muzzle, end, dir, round, sims, world, hitlocs, bones,
    )
}

/// What a bullet carries beyond its path.
struct Round<'a> {
    damage: i32,
    rifle_bullet: bool,
    weapon_name: &'a str,
}

/// `(int)(damage * multiplier)`: `G_Damage` multiplies on the x87 stack and
/// truncates with an explicit `fldcw` (combat doc 4.2), so the product is
/// taken wider than a float before the cut.
pub(crate) fn located_damage(damage: i32, multiplier: f32) -> i32 {
    (damage as f64 * multiplier as f64) as i32
}

/// One bullet from `muzzle` to `end`: `Bullet_Fire_Extended`'s recursion as
/// a loop (combat doc, 2.3 and 2.4). A player hit carries `damage` through
/// the location multiplier; a rifle round then goes on from the hit point
/// with that player as the pass entity and `damage / 2`. Any round goes on
/// through glass at the same damage, after the pane's own impact. It ends on
/// any other surface, on nothing, on a halving that leaves no damage, or
/// after 13 legs.
#[allow(clippy::too_many_arguments)]
fn fire_round(
    shooter: usize,
    muzzle: Vec3,
    end: Vec3,
    forward: Vec3,
    round: Round,
    sims: &[(usize, &ClientSim)],
    world: Option<&CollisionWorld>,
    hitlocs: &HitLocTable,
    mut bones: Option<&mut BoneTraceCtx>,
) -> ShotResult {
    let mut out = ShotResult::default();
    let priority = if round.rifle_bullet {
        &RIFLE_PRIORITY
    } else {
        &BULLET_PRIORITY
    };
    let event = if round.rifle_bullet {
        EV_BULLET_HIT_LARGE
    } else {
        EV_BULLET_HIT_SMALL
    };
    let (mod_, dflags) = bullet_mod(round.rifle_bullet);
    let (mut start, mut pass, mut damage) = (muzzle, shooter, round.damage);
    for _ in 0..=MAX_BULLET_DEPTH {
        match trace_attack(
            start,
            end,
            pass,
            sims,
            world,
            priority,
            bones.as_deref_mut(),
        ) {
            Traced::Player {
                slot,
                fraction,
                hitloc,
            } => {
                let point = start + (end - start) * fraction;
                // `Bullet_Fire_Extended` raises no impact on a client (2.4).
                out.effects.push(Effect::Hit(Hit {
                    victim: slot,
                    attacker: shooter,
                    inflictor: None,
                    damage: located_damage(damage, hitlocs.multiplier(hitloc)),
                    dflags,
                    mod_,
                    weapon: round.weapon_name.to_string(),
                    point: point.into(),
                    dir: forward.into(),
                    hitloc,
                }));
                damage /= 2;
                if dflags & DFLAG_PASSTHRU == 0 || damage <= 0 {
                    break;
                }
                (start, pass) = (point, slot);
            }
            Traced::World(t) => {
                if t.surface_flags & SURF_NO_IMPACT == 0 {
                    out.effects.push(Effect::Impact(TempEntity {
                        event,
                        parm: dir_to_byte(t.normal.into()),
                        surf_type: sound_material(t.surface_flags),
                        other: shooter as u32,
                        attacker: 0,
                        weapon: 0,
                        origin: t.endpos.into(),
                        client_num: 0,
                        scale: 0,
                        scope: Scope::Broadcast,
                    }));
                }
                if world.is_none_or(|w| w.hit_contents(&t) & CONTENTS_GLASS == 0) {
                    break;
                }
                // Glass (2.4, step 3): on at full damage past the pane, with
                // no nudge at a grazing angle.
                let dir = (end - start).normalize();
                let d = -t.normal.dot(dir);
                let nudge = if d >= GLASS_MIN_COS {
                    GLASS_NUDGE / d
                } else {
                    0.0
                };
                start = t.endpos + dir * nudge;
            }
            Traced::Nothing => break,
        }
    }
    out
}

/// The pair `finishPlayerDamage` raises at `point` for a hit with a
/// `weaponType` bullet weapon (combat doc 4.5): the plain flesh impact for
/// every snapshot but the victim's (`svFlags` 0x2000) and the client one for
/// the victim's alone (0x800). `dir` is the callback's `vDir` normalized, or
/// zero; `attacker` is `otherEntityNum`, the world when no entity was passed.
pub fn flesh_impacts(
    point: [f32; 3],
    dir: [f32; 3],
    rifle_bullet: bool,
    attacker: u32,
    victim: usize,
) -> [TempEntity; 2] {
    let (plain, client) = if rifle_bullet {
        (EV_BULLET_HIT_LARGE, EV_BULLET_HIT_CLIENT_LARGE)
    } else {
        (EV_BULLET_HIT_SMALL, EV_BULLET_HIT_CLIENT_SMALL)
    };
    let byte = dir_to_byte(dir);
    let base = TempEntity {
        event: plain,
        parm: byte,
        surf_type: SURF_FLESH,
        other: attacker,
        attacker: 0,
        weapon: 0,
        client_num: 0,
        scale: byte,
        origin: point,
        scope: Scope::AllBut(victim),
    };
    let client = TempEntity {
        event: client,
        parm: 0,
        scale: 0,
        client_num: victim as i32,
        scope: Scope::Only(victim),
        ..base
    };
    [base, client]
}

/// What an attack's trace found: the nearest live player whose bones the ray
/// scored, the world surface it stopped on, or neither.
enum Traced {
    Player {
        slot: usize,
        /// Along `muzzle..end`.
        fraction: f32,
        hitloc: &'static str,
    },
    World(vcod_common::collision::Trace),
    Nothing,
}

/// A live player's body as a locational trace meets it (combat doc, section
/// 3): the link box is the broad phase and the posed bones are the hit. Only a
/// playing client has one: its contents are BODY, the bit the bullet and the
/// blast masks both carry, where a corpse's CORPSE and a dead or spectating
/// client's 0 meet neither.
/// The link half (origin, yaw, box) is what the body's last cmd left, the pose
/// half what its last end frame did (section 16.1).
#[derive(Clone, Debug)]
pub struct HitBody {
    pub slot: usize,
    /// `r.currentOrigin`, at the feet.
    pub origin: Vec3,
    /// Radians. A player entity is yaw-only.
    pub yaw: f32,
    pub mins: Vec3,
    pub maxs: Vec3,
    pub pose: BodyPose,
}

/// What `ClientEndFrame`'s `BG_PlayerAnimation` hands the DObj: its models,
/// both anim channels and the controllers' inputs.
#[derive(Clone, Default, Debug)]
pub struct BodyPose {
    pub assembly: crate::game::hitrig::Assembly,
    /// Wire `legsAnim` / `torsoAnim` and the serverTime each last started.
    pub legs: i32,
    pub torso: i32,
    pub legs_start_ms: i32,
    pub torso_start_ms: i32,
    /// What the end frame copied into the record, and the swings it left
    /// (`docs/research/cod11-combat.md` 16.3, 16.4).
    pub input: vcod_common::playerpose::BodyInput,
    pub angles: vcod_common::playerpose::BodyAngles,
    pub slope: vcod_common::playerpose::BodySlope,
    /// A gunner's legs blend, the turret placement's leaves.
    pub turret_leaves: Option<Vec<(usize, f32)>>,
}

impl BodyPose {
    pub fn pose_inputs<'a>(
        &'a self,
        anims: &'a PlayerAnims,
        now_ms: i32,
    ) -> vcod_common::playerpose::PoseInputs<'a> {
        let mounted = self.input.eflags & vcod_common::net::flags::EF_MOUNTED != 0;
        let dead = self.input.eflags & vcod_common::net::flags::EF_DEAD != 0;
        vcod_common::playerpose::PoseInputs {
            anims,
            legs: self.legs,
            torso: self.torso,
            legs_start_ms: self.legs_start_ms,
            torso_start_ms: self.torso_start_ms,
            now_ms,
            // Down positive, as `descend_aim` reads it; only a gunner with no
            // placement blend descends an aim group.
            group_pitch: vcod_common::pmove::aim::angle_normalize_180(self.input.view[0]),
            turret_leaves: self.turret_leaves.as_deref().filter(|_| mounted),
            // A dead body's `tag_origin` takes the legs' world yaw, not its
            // yaw off the view, which this pose's frame would turn twice.
            controllers: (!dead)
                .then(|| {
                    vcod_common::playerpose::Controllers::new(
                        &self.angles,
                        &self.input,
                        &self.slope,
                    )
                })
                .flatten(),
        }
    }
}

/// The trace a bullet and a swing both make (combat doc, sections 2.2, 2.5
/// and 3): the world, then every live player's body.
fn trace_attack(
    muzzle: Vec3,
    end: Vec3,
    attacker: usize,
    sims: &[(usize, &ClientSim)],
    world: Option<&CollisionWorld>,
    priority: &PriorityMap,
    bones: Option<&mut BoneTraceCtx>,
) -> Traced {
    let trace = world.map(|w| w.shot_trace(muzzle, end));
    let world_fraction = trace.as_ref().map_or(1.0, |t| t.fraction);
    let bodies: Vec<HitBody> = sims
        .iter()
        .filter_map(|(s, sim)| sim.hit_body(*s))
        .collect();
    if let Some((slot, fraction, hitloc)) = trace_bodies(
        muzzle,
        end,
        attacker,
        &bodies,
        world_fraction,
        priority,
        bones,
    ) {
        return Traced::Player {
            slot,
            fraction,
            hitloc,
        };
    }
    match trace {
        Some(t) if t.fraction < 1.0 => Traced::World(t),
        _ => Traced::Nothing,
    }
}

/// The body a segment scores nearer than `limit`, skipping `pass`: slot,
/// fraction along `start..end` and hit location. The link box is the broad
/// phase only: retail's locational trace then has to score a bone, and a ray
/// can cross the column and meet none (combat doc, section 3). Candidates in
/// the order the ray reaches them, and the first one whose bones it does
/// score is the answer.
pub fn trace_bodies(
    start: Vec3,
    end: Vec3,
    pass: usize,
    bodies: &[HitBody],
    limit: f32,
    priority: &PriorityMap,
    mut bones: Option<&mut BoneTraceCtx>,
) -> Option<(usize, f32, &'static str)> {
    let mut candidates: Vec<(&HitBody, f32)> = bodies
        .iter()
        .filter(|b| b.slot != pass)
        .filter_map(|b| {
            let t = ray_box(start, end, b.origin + b.mins, b.origin + b.maxs)?;
            (t < limit).then_some((b, t))
        })
        .collect();
    candidates.sort_by(|a, b| a.1.total_cmp(&b.1));
    for (body, box_t) in candidates {
        // No paks, no rig: the trace still lands, at no location, which the
        // shipped multiplier table reads as full damage.
        let Some(ctx) = bones.as_mut() else {
            return Some((body.slot, box_t, "none"));
        };
        let BoneTraceCtx {
            fs,
            anims,
            rigs,
            now_ms,
        } = &mut **ctx;
        let Some(skel) = rigs.rig(fs, &body.pose.assembly) else {
            return Some((body.slot, box_t, "none"));
        };
        let inputs = body.pose.pose_inputs(anims, *now_ms);
        let pose = pose_player(&skel, &inputs, |name| rigs.clip(fs, name));
        // Into the body's own frame: its `tag_origin` sits at the feet.
        let inv = Quat::from_rotation_z(-body.yaw);
        let (ls, le) = (inv * (start - body.origin), inv * (end - body.origin));
        let Some(hit) = bone_trace(&skel, &pose, ls, le, priority) else {
            continue;
        };
        let hitloc = HITLOC_NAMES
            .get(hit.hit_location as usize)
            .copied()
            .unwrap_or("none");
        return Some((body.slot, hit.fraction, hitloc));
    }
    None
}

/// The two a swing spawns a temp entity for
/// (`docs/research/cod11-events-and-fx.md` section 1).
pub use vcod_common::net::event_ids::EV_MELEE_HIT;
pub use vcod_common::net::event_ids::EV_MELEE_MISS;
/// `Weapon_Melee`'s reach, `.rodata 0x79c00` (combat doc, 2.5).
pub const MELEE_RANGE: f32 = 64.0;

/// `Weapon_Melee` (combat doc, 2.5): a 64-unit trace along the view with the
/// bullet mask and `bulletPriorityMap` whatever the weapon, a hit or miss
/// temp entity carrying the swinger's weapon -- the hit's parm is
/// `DirToByte(-forward)`, the miss's the surface normal -- and on a
/// player a `MOD_MELEE` hit for `meleeDamage + rand()%5`. The hit-location
/// multiplier is `G_Damage`'s, the same one a bullet goes through (4.2):
/// retail's melee capture read 81 and 79 off a 50-damage swing at `head`.
#[allow(clippy::too_many_arguments)]
pub fn melee_fire(
    attacker: usize,
    def: &WeaponDef,
    weapon_name: &str,
    weapon_index: u8,
    aim: [f32; 2],
    sims: &[(usize, &ClientSim)],
    world: Option<&CollisionWorld>,
    hitlocs: &HitLocTable,
    bones: Option<&mut BoneTraceCtx>,
    rng: &mut u64,
) -> ShotResult {
    let Some((_, me)) = sims.iter().find(|(slot, _)| *slot == attacker) else {
        return ShotResult::default();
    };
    let muzzle = muzzle_point(&me.ps);
    let (yaw, pitch) = aim_radians(aim);
    let forward = Vec3::new(
        pitch.cos() * yaw.cos(),
        pitch.cos() * yaw.sin(),
        pitch.sin(),
    );
    let end = muzzle + forward * MELEE_RANGE;
    let traced = trace_attack(muzzle, end, attacker, sims, world, &BULLET_PRIORITY, bones);
    let weapon = weapon_index as i32;
    match traced {
        Traced::Player {
            slot,
            fraction,
            hitloc,
        } => {
            let point = muzzle + (end - muzzle) * fraction;
            let damage = def.melee_damage + (vcod_common::rng::xorshift(rng) % 5) as i32;
            let damage = located_damage(damage, hitlocs.multiplier(hitloc));
            ShotResult {
                effects: vec![
                    Effect::Impact(TempEntity {
                        event: EV_MELEE_HIT,
                        // The bone trace answers no normal. Retail's is the
                        // bone's own, which the melee capture reads as the swing
                        // reversed and tilted: parm 115 against a forward of +y.
                        parm: dir_to_byte((-forward).into()),
                        surf_type: SURF_FLESH,
                        other: slot as u32,
                        attacker: 0,
                        weapon,
                        origin: point.into(),
                        client_num: 0,
                        scale: 0,
                        scope: Scope::Broadcast,
                    }),
                    Effect::Hit(Hit {
                        victim: slot,
                        attacker,
                        inflictor: None,
                        damage,
                        dflags: 0,
                        mod_: "MOD_MELEE",
                        weapon: weapon_name.to_string(),
                        point: point.into(),
                        dir: forward.into(),
                        hitloc,
                    }),
                ],
            }
        }
        // A swing that meets nothing still raises the miss: `Weapon_Melee`
        // spawns one of the two events on every path.
        Traced::World(t) => ShotResult {
            effects: vec![Effect::Impact(TempEntity {
                event: EV_MELEE_MISS,
                parm: dir_to_byte(t.normal.into()),
                surf_type: sound_material(t.surface_flags),
                other: ENTITYNUM_WORLD,
                attacker: 0,
                weapon,
                origin: t.endpos.into(),
                client_num: 0,
                scale: 0,
                scope: Scope::Broadcast,
            })],
        },
        Traced::Nothing => ShotResult {
            effects: vec![Effect::Impact(TempEntity {
                event: EV_MELEE_MISS,
                parm: 0,
                surf_type: 0,
                other: ENTITYNUM_NONE,
                attacker: 0,
                weapon,
                origin: end.into(),
                client_num: 0,
                scale: 0,
                scope: Scope::Broadcast,
            })],
        },
    }
}

/// The rise `G_RadiusDamage` adds to the direction it hands the callback
/// (combat doc, 14.1).
const RADIUS_DIR_RISE: f32 = 24.0;
/// How close to the blast the second-chance arm reaches, as a share of the
/// radius, and what share of the falloff it charges there (14.1).
const SECOND_CHANCE_RANGE: f32 = 0.2;
const SECOND_CHANCE_SHARE: f32 = 0.1;
/// The half-width of `CanDamage`'s probe rectangle (14.3).
const CAN_DAMAGE_HALF_WIDTH: f32 = 15.0;
/// The second chance's `trap_Trace` mask, SOLID and GLASS (14.1). A plain
/// trace, so no static model stops it, and a script model's 0x2080 shares no
/// bit with the mask.
const SECOND_CHANCE_MASK: u32 = 0x11;

/// A linked `script_model`'s collision as a locational trace meets it: the
/// xmodel's surfaces at the entity's origin and angles (combat doc, 2.7).
pub struct PlacedModel {
    pub id: EntId,
    pub origin: Vec3,
    pub axis: glam::Mat3,
    pub tris: std::rc::Rc<[vcod_common::collision::ModelTri]>,
}

impl PlacedModel {
    /// The segment's first hit on this model closer than `best`: fraction
    /// and world normal.
    pub fn clip(&self, start: Vec3, end: Vec3, mask: u32, best: f32) -> Option<(f32, Vec3)> {
        let local = |p: Vec3| self.axis.transpose() * (p - self.origin);
        vcod_common::collision::clip_model_tris(local(start), local(end), &self.tris, mask, best)
            .map(|(f, n, _)| (f, self.axis * n))
    }
}

/// A client as a blast candidate. The other kind is [`EntityVictim`].
pub struct BlastVictim {
    pub slot: usize,
    /// `r.currentOrigin`, at the feet: what the distance is measured to.
    /// Unsnapped, since a blast runs outside every client's cmd (14.3).
    pub origin: Vec3,
    /// `r.currentOrigin` at the client's last link, which `r.absmin` and
    /// `r.absmax`, and so the second chance's box midpoint, are built off:
    /// the snapped origin after a cmd (14.1).
    pub link_origin: Vec3,
    pub mins: Vec3,
    pub maxs: Vec3,
    /// The eye, lean included, that `CanDamage` builds its probes around.
    pub eye: Vec3,
}

/// `CanDamage`'s client arm (combat doc, 14.3): five traces at the body
/// centre and at the corners of a 30-unit-wide, body-tall rectangle held
/// broadside to the blast. None clear is 0, four or five is 1, anything
/// between is `count / 3`. Each is a locational trace from the probe to the
/// blast with the victim as its pass entity, so `models` and every other
/// body in `bodies` stop it as the world does (14.4).
pub fn can_damage(
    at: Vec3,
    v: &BlastVictim,
    world: &CollisionWorld,
    models: &[PlacedModel],
    bodies: &[HitBody],
    mut bones: Option<&mut BoneTraceCtx>,
) -> f32 {
    let mid = (v.eye + v.origin) * 0.5;
    let mut to_blast = at - v.origin;
    to_blast.z = 0.0;
    let d = to_blast.normalize_or_zero();
    let across = Vec3::new(-d.y, d.x, 0.0) * CAN_DAMAGE_HALF_WIDTH;
    let up = Vec3::Z * (v.eye.z - v.origin.z) * 0.5;
    let probes = [
        mid,
        mid + across + up,
        mid + across - up,
        mid - across + up,
        mid - across - up,
    ];
    let mask = vcod_common::collision::MASK_BLAST;
    let clear = probes
        .iter()
        .filter(|&&p| {
            world.point_trace(p, at, mask, true).fraction >= 1.0
                && models.iter().all(|m| m.clip(p, at, mask, 1.0).is_none())
                && trace_bodies(
                    p,
                    at,
                    v.slot,
                    bodies,
                    1.0,
                    &BULLET_PRIORITY,
                    bones.as_deref_mut(),
                )
                .is_none()
        })
        .count();
    match clear {
        0 => 0.0,
        4 | 5 => 1.0,
        n => n as f32 / 3.0,
    }
}

/// A blast candidate with no client: a turret, the one entity with
/// `takedamage` this server spawns. Retail's door, `func_static` and
/// `trigger_damage` spawns store it too, but no stock MP map places one
/// (combat doc, 14.6), so the brush-model arms of `G_RadiusDamage` have no
/// caller here.
#[derive(Clone)]
pub struct EntityVictim {
    pub id: EntId,
    /// `r.currentOrigin`, the distance's other end: not a brush model.
    pub origin: Vec3,
    pub mins: Vec3,
    pub maxs: Vec3,
}

impl EntityVictim {
    /// The middle of `r.absmin`/`r.absmax`, whose one-unit widening cancels.
    fn mid(&self) -> Vec3 {
        self.origin + (self.mins + self.maxs) * 0.5
    }
}

/// `CanDamage`'s arm for an entity with no client (combat doc, 14.3): the
/// box midpoint and four points 15 units off it on x and y, all or nothing.
/// Nothing is skipped as the pass entity: a turret's contents share no bit
/// with the mask, and it is no body.
fn can_damage_entity(
    at: Vec3,
    v: &EntityVictim,
    world: &CollisionWorld,
    models: &[PlacedModel],
    bodies: &[HitBody],
    mut bones: Option<&mut BoneTraceCtx>,
) -> f32 {
    let mid = v.mid();
    let h = CAN_DAMAGE_HALF_WIDTH;
    let probes = [
        mid,
        mid + Vec3::new(h, h, 0.0),
        mid + Vec3::new(h, -h, 0.0),
        mid + Vec3::new(-h, h, 0.0),
        mid + Vec3::new(-h, -h, 0.0),
    ];
    let mask = vcod_common::collision::MASK_BLAST;
    let clear = probes.iter().any(|&p| {
        world.point_trace(p, at, mask, true).fraction >= 1.0
            && models.iter().all(|m| m.clip(p, at, mask, 1.0).is_none())
            && trace_bodies(
                p,
                at,
                usize::MAX,
                bodies,
                1.0,
                &BULLET_PRIORITY,
                bones.as_deref_mut(),
            )
            .is_none()
    });
    if clear { 1.0 } else { 0.0 }
}

/// One `G_RadiusDamage` call (combat doc, 14.1). Retail collects its
/// candidates once, then runs each victim's damage callback inside the walk,
/// so whatever a callback does (a kill above all, whose corpse stops
/// shielding) is what the next candidate is measured against (14.5). The
/// caller owns that loop: it asks [`Blast::reaches`] once per candidate at
/// the start, then [`Blast::hit`] per candidate with the bodies and the
/// victim as they stand now, and delivers each hit before the next.
pub struct Blast {
    pub at: Vec3,
    /// Raised to 1 the way retail raises it.
    pub radius: f32,
    pub inner: f32,
    pub outer: f32,
    /// `None` for a blast the world set off.
    pub attacker: Option<usize>,
    pub inflictor: Option<EntId>,
    pub weapon: String,
    pub mod_: &'static str,
}

impl Blast {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        at: Vec3,
        radius: f32,
        inner: f32,
        outer: f32,
        attacker: Option<usize>,
        inflictor: Option<EntId>,
        weapon: &str,
        mod_: &'static str,
    ) -> Self {
        Blast {
            at,
            radius: radius.max(1.0),
            inner,
            outer,
            attacker,
            inflictor,
            weapon: weapon.to_string(),
            mod_,
        }
    }

    /// Whether `trap_EntitiesInBox` puts `v` on the candidate list: its
    /// linked box, `r.absmin` and `r.absmax` with their one-unit widening
    /// (14.1), against `at` plus and minus `radius * sqrt(2)`.
    pub fn reaches(&self, v: &BlastVictim) -> bool {
        self.box_reaches(v.link_origin, v.mins, v.maxs)
    }

    /// [`Blast::reaches`] for an entity with no client.
    pub fn reaches_entity(&self, v: &EntityVictim) -> bool {
        self.box_reaches(v.origin, v.mins, v.maxs)
    }

    /// `trap_EntitiesInBox`' box: the blast plus and minus `radius * sqrt 2`
    /// on each axis (14.1).
    pub fn search_box(&self) -> ([f32; 3], [f32; 3]) {
        let half = Vec3::splat(self.radius * std::f32::consts::SQRT_2);
        ((self.at - half).into(), (self.at + half).into())
    }

    fn box_reaches(&self, origin: Vec3, mins: Vec3, maxs: Vec3) -> bool {
        let (lo, hi) = self.search_box();
        let (lo, hi) = (Vec3::from(lo), Vec3::from(hi));
        let min = origin + mins - Vec3::ONE;
        let max = origin + maxs + Vec3::ONE;
        min.cmple(hi).all() && max.cmpge(lo).all()
    }

    /// The falloff before line of sight at `dist`, `None` at or past the
    /// radius. At double precision: retail keeps the whole expression on the
    /// x87 stack, and an f32 round trip loses a point of damage at the round
    /// ratios a script picks.
    fn points(&self, dist: f32) -> Option<f64> {
        (dist < self.radius).then(|| {
            self.outer as f64
                + (1.0 - dist as f64 / self.radius as f64) * (self.inner as f64 - self.outer as f64)
        })
    }

    /// The damage `CanDamage`'s `fraction` leaves, or with none the second
    /// chance's tenth when the trace to the box midpoint `mid` was blocked
    /// and `mid` is inside `radius * 0.2` (14.1).
    fn charge(
        &self,
        points: f64,
        fraction: f32,
        mid: Vec3,
        world: Option<&CollisionWorld>,
    ) -> Option<i32> {
        if fraction > 0.0 {
            return Some((fraction as f64 * points) as i32);
        }
        let blocked = world.is_some_and(|w| {
            w.point_trace(self.at, mid, SECOND_CHANCE_MASK, false)
                .fraction
                < 1.0
        });
        if !blocked || (mid - self.at).length() >= self.radius * SECOND_CHANCE_RANGE {
            return None;
        }
        Some((points * SECOND_CHANCE_SHARE as f64) as i32)
    }

    /// What the blast charges an entity with no client, before `G_Damage`
    /// raises a charge of 0 to 1 (4.2). As [`Blast::hit`] otherwise:
    /// measured origin to origin, every live body stops a probe.
    pub fn entity_damage(
        &self,
        v: &EntityVictim,
        world: Option<&CollisionWorld>,
        models: &[PlacedModel],
        bodies: &[HitBody],
        bones: Option<&mut BoneTraceCtx>,
    ) -> Option<i32> {
        let points = self.points((v.origin - self.at).length())?;
        let fraction = world.map_or(1.0, |w| {
            can_damage_entity(self.at, v, w, models, bodies, bones)
        });
        self.charge(points, fraction, v.mid(), world)
    }

    /// The falloff from `inner` at the blast to `outer` at the radius,
    /// scaled by `CanDamage`'s fraction and truncated the way `G_Damage`
    /// truncates. A victim with no line of sight at all still takes the
    /// second chance's tenth when the trace to its box midpoint, taken at
    /// `link_origin`, was blocked and that midpoint is inside
    /// `radius * 0.2`. Distance is origin to origin, which is what retail
    /// measures for anything that is not a brush model. `models` and
    /// `bodies` stop `CanDamage`'s traces and not the second chance's.
    /// Without a `world` nothing is traced and every victim inside the
    /// radius takes the falloff whole, which is what a unit test wants and
    /// what a host with no map has.
    pub fn hit(
        &self,
        v: &BlastVictim,
        world: Option<&CollisionWorld>,
        models: &[PlacedModel],
        bodies: &[HitBody],
        bones: Option<&mut BoneTraceCtx>,
    ) -> Option<Hit> {
        let at = self.at;
        let points = self.points((v.origin - at).length())?;
        let fraction = world.map_or(1.0, |w| can_damage(at, v, w, models, bodies, bones));
        let mid = v.link_origin + (v.mins + v.maxs) * 0.5;
        let damage = self.charge(points, fraction, mid, world)?;
        Some(Hit {
            victim: v.slot,
            attacker: self.attacker.unwrap_or(ENTITYNUM_WORLD as usize),
            inflictor: self.inflictor,
            damage,
            dflags: DFLAG_RADIUS,
            mod_: self.mod_,
            weapon: self.weapon.clone(),
            point: at.into(),
            // Unnormalized, and raised, exactly as retail hands it over: its
            // length is what carries the distance.
            dir: (v.origin - at + Vec3::Z * RADIUS_DIR_RISE).into(),
            hitloc: "none",
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    /// Every victim of one blast with nothing changing between them.
    #[allow(clippy::too_many_arguments)]
    fn radius_damage(
        at: Vec3,
        radius: f32,
        inner: f32,
        outer: f32,
        attacker: Option<usize>,
        inflictor: Option<EntId>,
        weapon: &str,
        mod_: &'static str,
        victims: &[BlastVictim],
        world: Option<&CollisionWorld>,
        models: &[PlacedModel],
        bodies: &[HitBody],
        mut bones: Option<&mut BoneTraceCtx>,
    ) -> Vec<Hit> {
        let blast = Blast::new(at, radius, inner, outer, attacker, inflictor, weapon, mod_);
        victims
            .iter()
            .filter_map(|v| blast.hit(v, world, models, bodies, bones.as_deref_mut()))
            .collect()
    }

    fn effect_kind(e: &Effect) -> &'static str {
        match e {
            Effect::Impact(_) => "impact",
            Effect::Hit(_) => "hit",
        }
    }

    fn only<T>(v: impl IntoIterator<Item = T>, what: &str) -> T {
        let mut v: Vec<T> = v.into_iter().collect();
        assert_eq!(v.len(), 1, "{what}");
        v.remove(0)
    }

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

    /// `rifleBullet` picks the means of death, and the stock files put the
    /// bolt-actions and every smg on `MOD_PISTOL_BULLET`: a kar98k torso hit
    /// logging one is retail behaviour, not a misread weapon.
    #[test]
    fn the_stock_weapons_means_of_death_is_the_files_rifle_bullet() {
        let Some(fs) = vcod_common::testing::game_fs() else {
            return;
        };
        let want = [
            ("kar98k_mp", "MOD_PISTOL_BULLET"),
            ("enfield_mp", "MOD_PISTOL_BULLET"),
            ("mosin_nagant_mp", "MOD_PISTOL_BULLET"),
            ("mp44_mp", "MOD_PISTOL_BULLET"),
            ("thompson_mp", "MOD_PISTOL_BULLET"),
            ("colt_mp", "MOD_PISTOL_BULLET"),
            ("m1carbine_mp", "MOD_RIFLE_BULLET"),
            ("m1garand_mp", "MOD_RIFLE_BULLET"),
            ("springfield_mp", "MOD_RIFLE_BULLET"),
            ("kar98k_sniper_mp", "MOD_RIFLE_BULLET"),
            ("bar_mp", "MOD_RIFLE_BULLET"),
        ];
        for (name, mod_) in want {
            let def = vcod_common::weapon::load(&fs, name).unwrap();
            assert_eq!(bullet_mod(def.sounds.rifle_bullet).0, mod_, "{name}");
        }
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

    /// End frames enough for a body standing at its yaw to have swung its
    /// legs and torso onto it, as the retail captures' bodies had.
    fn settle(sim: &mut ClientSim) {
        for _ in 0..20 {
            sim.commit_pose(50, vcod_common::playerpose::BG_SWING_SPEED, None);
        }
    }

    fn zero_spread_carbine() -> WeaponDef {
        let mut m = HashMap::new();
        m.insert("damage".to_string(), "45".to_string());
        m.insert("rifleBullet".to_string(), "1".to_string());
        m.insert("weaponClass".to_string(), "rifle".to_string());
        WeaponDef::from_map(&m)
    }

    /// Fires with no trace context, which is the no-paks path: the box hit
    /// stands and carries no location. The aim is the shooter's raw view, so
    /// a test can point the shot by setting `ps.pitch` and `ps.yaw`.
    fn fire(
        def: &WeaponDef,
        sims: &[(usize, &ClientSim)],
        world: &CollisionWorld,
        rng: &mut u64,
    ) -> ShotResult {
        let table = HitLocTable::default();
        let ps = &sims[0].1.ps;
        bullet_fire(
            0,
            def,
            "m1carbine_mp",
            false,
            &SpreadStance::of(ps, 0, 0),
            [-ps.pitch.to_degrees(), ps.yaw.to_degrees()],
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
        b.update_anims(
            &inputs,
            &vcod_common::net::msg::NULL_USERCMD,
            0,
            &[],
            &mut 1u64,
        );
        settle(&mut b);
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
            &SpreadStance::of(&a.ps, 0, 0),
            a.aim_angles(),
            &[(0, &a), (1, &b)],
            Some(&world),
            &table,
            Some(&mut ctx),
            &mut rng,
        );
        let hit = only(r.hits(), "the shot reached B");
        assert_eq!(hit.hitloc, "head");
        assert_eq!(hit.damage, 67, "45 through the head multiplier");
    }

    /// A body is posed off its last end frame, not off the cmds since (combat
    /// doc, 16.1): one that ended the frame crouched and has stood up since
    /// has the standing link box and still the crouched bones, so a level
    /// round at standing head height crosses the box and scores nothing.
    #[test]
    fn a_body_stood_up_since_its_end_frame_is_still_posed_crouched() {
        let Some(fs) = vcod_common::testing::game_fs() else {
            return;
        };
        let anims = vcod_common::animtree::PlayerAnims::load(&fs).expect("the player anims");
        let inputs = crate::spectate::AnimInputs {
            anims: &anims,
            weapon: "m1carbine_mp",
            weapon_class: "rifle",
        };
        let mut b = new_for_test([0.0, 0.0, 0.0], 180.0);
        b.assembly = stock_assembly();
        b.ps.on_ground = true;
        b.ps.stance = vcod_common::pmove::Stance::Crouch;
        let idle = vcod_common::net::msg::NULL_USERCMD;
        b.update_anims(&inputs, &idle, 0, &[], &mut 1u64);
        settle(&mut b);
        b.ps.stance = vcod_common::pmove::Stance::Stand;
        b.update_anims(&inputs, &idle, 50, &[], &mut 1u64);
        let body = b.hit_body(1).expect("a live body");
        assert_eq!(
            body.maxs.z,
            vcod_common::pmove::Stance::Stand.height(),
            "the link box is the cmd's"
        );
        let mut rigs = HitRigs::default();
        let mut trace_at = |z: f32, body: &HitBody| {
            let mut ctx = BoneTraceCtx {
                fs: &fs,
                anims: &anims,
                rigs: &mut rigs,
                now_ms: 50,
            };
            trace_bodies(
                Vec3::new(-100.0, 0.0, z),
                Vec3::new(100.0, 0.0, z),
                0,
                std::slice::from_ref(body),
                1.0,
                &RIFLE_PRIORITY,
                Some(&mut ctx),
            )
            .map(|(_, _, loc)| loc)
        };
        assert_eq!(trace_at(64.0, &body), None, "the crouched bones end lower");
        assert!(
            trace_at(30.0, &body).is_some(),
            "the crouched body is there"
        );
        settle(&mut b);
        let stood = b.hit_body(1).expect("a live body");
        assert_eq!(
            trace_at(64.0, &stood),
            Some("head"),
            "the next end frame stands it up"
        );
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
        let hit = only(r.hits(), "the shot reached B");
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
        assert!(
            r.impacts().next().is_none(),
            "the flesh pair is the callback's"
        );

        // Looking down: the floor, broadcast, with the floor's material.
        a.ps.pitch = -1.2;
        let r = fire(&def, &[(0, &a), (1, &b)], &world, &mut rng);
        assert!(r.hits().next().is_none());
        let te = only(r.impacts(), "a wall impact");
        assert_eq!(te.event, 174);
        assert_ne!(te.surf_type, 7);
        assert_eq!(te.scope, Scope::Broadcast);
        assert!(te.origin[2].abs() < 0.2, "on the floor, {:?}", te.origin);
    }

    /// A turret round (turrets doc 6.3): no spread, from the muzzle the gun
    /// computed along the gunner's view, carrying the gun's damage and the
    /// rifle means of death.
    #[test]
    fn a_turret_round_leaves_its_muzzle_along_the_view_and_names_the_gun() {
        let world = vcod_common::collision::test_world(&[]);
        let table = HitLocTable::default();
        let a = new_for_test([0.0, 0.0, 0.0], 0.0);
        let b = new_for_test([100.0, 0.0, 0.0], 180.0);
        let r = bullet_fire_from(
            0,
            Vec3::new(40.0, 0.0, 40.0),
            Vec3::X,
            60,
            true,
            "m1carbine_mp",
            &[(0, &a), (1, &b)],
            Some(&world),
            &table,
            None,
        );
        let hit = only(r.hits(), "the round reached B");
        assert_eq!((hit.victim, hit.attacker), (1, 0));
        assert_eq!(hit.inflictor, None);
        assert_eq!(hit.damage, 60);
        assert_eq!((hit.mod_, hit.dflags), ("MOD_RIFLE_BULLET", DFLAG_PASSTHRU));
        assert_eq!(hit.weapon, "m1carbine_mp");
        assert!((hit.point[0] - 85.0).abs() < 0.01, "{:?}", hit.point);
        assert!((hit.point[2] - 40.0).abs() < 0.01, "{:?}", hit.point);
        assert!(
            r.impacts().next().is_none(),
            "the flesh pair is the callback's"
        );
    }

    /// `G_Damage` multiplies on the x87 stack and truncates (combat doc 4.2):
    /// the turret capture's 53 is 60 * 0.9f there, where a float product
    /// rounds to 54 first (turrets doc 12.6).
    #[test]
    fn the_hit_location_product_truncates_at_extended_precision() {
        assert_eq!(located_damage(60, 0.9), 53);
        assert_eq!(located_damage(45, 1.5), 67);
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
        assert!(r.hits().next().is_none(), "the wall is nearer than B");
        assert!((only(r.impacts(), "the wall impact").origin[0] - 40.0).abs() < 0.2);

        let open = vcod_common::collision::test_world(&[]);
        b.dead = true;
        let r = fire(&def, &[(0, &a), (1, &b)], &open, &mut rng);
        assert!(r.hits().next().is_none(), "a dead player is not hit again");
        assert!(
            r.impacts().next().is_none(),
            "and the open floor is out of range of a level shot"
        );

        // A pistol round names the pistol means of death and passes nothing.
        b.dead = false;
        let mut m = HashMap::new();
        m.insert("damage".to_string(), "30".to_string());
        let colt = WeaponDef::from_map(&m);
        let r = fire(&colt, &[(0, &a), (1, &b)], &open, &mut rng);
        let hit = only(r.hits(), "the colt round");
        assert_eq!(
            (hit.mod_, hit.dflags, hit.damage),
            ("MOD_PISTOL_BULLET", 0, 30)
        );
    }

    /// Combat doc 2.4, step 5: a rifle round goes on from the hit point past
    /// the player it hit, at half its damage, into the next player and then
    /// the wall behind; a pistol round stops on the first player, and so
    /// does a rifle round whose halving leaves nothing.
    #[test]
    fn a_rifle_round_passes_through_a_player_at_half_damage_and_a_pistol_round_stops() {
        let a = new_for_test([0.0, 0.0, 0.0], 0.0);
        let b = new_for_test([100.0, 0.0, 0.0], 180.0);
        let c = new_for_test([200.0, 0.0, 0.0], 180.0);
        let sims = [(0, &a), (1, &b), (2, &c)];
        let wall = vcod_common::collision::test_world(&[(
            Vec3::new(300.0, -64.0, 0.0),
            Vec3::new(308.0, 64.0, 128.0),
        )]);
        let mut rng = 1u64;

        let r = fire(&zero_spread_carbine(), &sims, &wall, &mut rng);
        let hits: Vec<&Hit> = r.hits().collect();
        let got: Vec<(usize, i32)> = hits.iter().map(|h| (h.victim, h.damage)).collect();
        assert_eq!(got, [(1, 45), (2, 22)], "B at full damage, C at 45 / 2");
        assert!(
            (hits[1].point[0] - 185.0).abs() < 0.01,
            "{:?}",
            hits[1].point
        );
        assert_eq!(hits[1].dir, hits[0].dir, "both carry the shot's forward");
        let order: Vec<&str> = r.effects.iter().map(effect_kind).collect();
        assert_eq!(order, ["hit", "hit", "impact"], "each leg in turn");
        let te = only(r.impacts(), "the wall behind C");
        assert_eq!(te.event, 174);
        assert!((te.origin[0] - 300.0).abs() < 0.2, "{:?}", te.origin);

        let mut m = HashMap::new();
        m.insert("damage".to_string(), "30".to_string());
        let r = fire(&WeaponDef::from_map(&m), &sims, &wall, &mut rng);
        assert_eq!(only(r.hits(), "the colt round").victim, 1);
        assert!(r.impacts().next().is_none(), "the pistol round ends in B");

        let mut weak = zero_spread_carbine();
        weak.damage = 1;
        let r = fire(&weak, &sims, &wall, &mut rng);
        assert_eq!(only(r.hits(), "1 / 2 carries nothing on").victim, 1);
        assert!(r.impacts().next().is_none());
    }

    /// Combat doc 2.4, step 3: any round raises the pane's impact and goes on
    /// through glass at full damage; one meeting it at under 0.125 of the
    /// normal is not nudged, meets the pane again at every leg and ends
    /// there when the depth runs out.
    #[test]
    fn a_round_goes_on_through_glass_at_full_damage() {
        let world = vcod_common::collision::synthetic_world(
            &[
                ("textures/test/solid", 0x1, 0),
                ("textures/test/glass", 0x8000010, 0x900000),
            ],
            &[
                (0, [-1024.0, -1024.0, -16.0], [1024.0, 1024.0, 0.0]),
                (1, [40.0, -1024.0, 0.0], [42.0, 1024.0, 128.0]),
                (0, [300.0, -64.0, 0.0], [308.0, 64.0, 128.0]),
            ],
        );
        let mut a = new_for_test([0.0, 0.0, 0.0], 0.0);
        let b = new_for_test([100.0, 0.0, 0.0], 180.0);
        let mut rng = 1u64;

        let r = fire(
            &zero_spread_carbine(),
            &[(0, &a), (1, &b)],
            &world,
            &mut rng,
        );
        let got: Vec<(usize, i32)> = r.hits().map(|h| (h.victim, h.damage)).collect();
        assert_eq!(got, [(1, 45)], "B behind the pane at full damage");
        let order: Vec<&str> = r.effects.iter().map(effect_kind).collect();
        assert_eq!(order, ["impact", "hit", "impact"], "pane, B, wall");
        let at: Vec<(f32, i32)> = r.impacts().map(|t| (t.origin[0], t.surf_type)).collect();
        assert_eq!(r.impacts().count(), 2, "{at:?}");
        assert!((at[0].0 - 40.0).abs() < 0.2 && at[0].1 == 9, "{at:?}");
        assert!((at[1].0 - 300.0).abs() < 0.2, "the wall behind B, {at:?}");

        let mut m = HashMap::new();
        m.insert("damage".to_string(), "30".to_string());
        let r = fire(
            &WeaponDef::from_map(&m),
            &[(0, &a), (1, &b)],
            &world,
            &mut rng,
        );
        assert_eq!(only(r.hits(), "the colt round past the pane").damage, 30);
        assert_eq!(only(r.impacts(), "the pane").surf_type, 9);

        // 84 degrees off the pane's normal: cos 0.105.
        a.ps.yaw = 84f32.to_radians();
        let r = fire(
            &zero_spread_carbine(),
            &[(0, &a), (1, &b)],
            &world,
            &mut rng,
        );
        assert!(r.hits().next().is_none());
        assert_eq!(r.impacts().count(), 13, "one per leg, all on the pane");
        assert!(
            r.impacts()
                .all(|t| t.surf_type == 9 && (t.origin[0] - 40.0).abs() < 0.2)
        );
    }

    /// Combat doc 4.5: the plain copy carries the direction twice and goes
    /// to everyone but the victim, the client copy carries the victim's
    /// number and no direction and goes to the victim alone; `rifleBullet`
    /// picks the large pair.
    #[test]
    fn a_flesh_hit_is_a_plain_copy_for_others_and_a_client_copy_for_the_victim() {
        let [plain, client] = flesh_impacts([85.5, 0.0, 40.0], [1.0, 0.0, 0.0], false, 3, 1);
        let x = dir_to_byte([1.0, 0.0, 0.0]);
        assert_eq!(
            (
                plain.event,
                plain.parm,
                plain.scale,
                plain.surf_type,
                plain.other
            ),
            (173, x, x, 7, 3)
        );
        assert_eq!((plain.client_num, plain.scope), (0, Scope::AllBut(1)));
        assert_eq!(
            (
                client.event,
                client.parm,
                client.scale,
                client.surf_type,
                client.other
            ),
            (175, 0, 0, 7, 3)
        );
        assert_eq!((client.client_num, client.scope), (1, Scope::Only(1)));
        assert_eq!(plain.origin, client.origin);
        let [plain, client] = flesh_impacts([0.0; 3], [0.0; 3], true, 1022, 0);
        assert_eq!((plain.event, client.event), (174, 176));
    }

    fn melee_carbine() -> WeaponDef {
        let mut m = HashMap::new();
        m.insert("meleeDamage".to_string(), "50".to_string());
        WeaponDef::from_map(&m)
    }

    /// `Weapon_Melee` reaches 64 units and no further (combat doc, 2.5): the
    /// hit event names the victim, the miss names the world, and both go to
    /// everyone. No rig here, so the location is `none` and the damage is the
    /// raw `meleeDamage + rand()%5`.
    #[test]
    fn a_swing_reaches_64_units_and_names_its_victim() {
        let world = vcod_common::collision::test_world(&[]);
        let table = HitLocTable::default();
        let a = new_for_test([0.0, 0.0, 0.0], 0.0);
        let near = new_for_test([30.0, 0.0, 0.0], 180.0);
        let far = new_for_test([100.0, 0.0, 0.0], 180.0);
        let def = melee_carbine();
        let swing = |sims: &[(usize, &ClientSim)], rng: &mut u64| {
            melee_fire(
                0,
                &def,
                "m1carbine_mp",
                12,
                sims[0].1.aim_angles(),
                sims,
                Some(&world),
                &table,
                None,
                rng,
            )
        };
        let mut rng = 1u64;
        let r = swing(&[(0, &a), (1, &near)], &mut rng);
        let hit = only(r.hits(), "the swing reached B");
        assert_eq!(hit.victim, 1);
        assert_eq!(hit.mod_, "MOD_MELEE");
        assert_eq!(hit.dflags, 0);
        assert!((50..55).contains(&hit.damage), "{}", hit.damage);
        let te = only(r.impacts(), "the hit event");
        assert_eq!(te.event, EV_MELEE_HIT);
        assert_eq!(te.other, 1, "the hit names the victim");
        assert_eq!(te.surf_type, SURF_FLESH);
        assert_eq!(te.weapon, 12, "the swinger's weapon rides the event");
        assert_eq!(te.scope, Scope::Broadcast, "the victim is sent it too");

        let r = swing(&[(0, &a), (1, &far)], &mut rng);
        assert!(r.hits().next().is_none(), "100 units is out of reach");
        let te = only(r.impacts(), "the miss event");
        assert_eq!(te.event, EV_MELEE_MISS);
        assert_eq!(te.other, ENTITYNUM_NONE);
    }

    /// A melee hit goes through the same `located_damage` as a bullet
    /// (combat doc 4.2, turrets doc 12.6): a 0.9 location multiplier
    /// truncates at extended precision, not at `f32`.
    #[test]
    fn a_melee_hit_at_a_09_location_truncates_like_a_bullet() {
        let world = vcod_common::collision::test_world(&[]);
        let mut table = HitLocTable::default();
        table.mult[0] = 0.9; // "none": the melee trace here carries no rig.
        let a = new_for_test([0.0, 0.0, 0.0], 0.0);
        let near = new_for_test([30.0, 0.0, 0.0], 180.0);
        let def = melee_carbine();
        let mut rng = 1u64;
        let mut expected_rng = rng;
        let base = def.melee_damage + (vcod_common::rng::xorshift(&mut expected_rng) % 5) as i32;
        let r = melee_fire(
            0,
            &def,
            "m1carbine_mp",
            12,
            a.aim_angles(),
            &[(0, &a), (1, &near)],
            Some(&world),
            &table,
            None,
            &mut rng,
        );
        let hit = only(r.hits(), "the swing reached B");
        assert_eq!(hit.damage, located_damage(base, 0.9));
    }

    /// A standing client as a blast candidate: the box and the eye
    /// `CanDamage` builds its probes from.
    fn blast_victim(slot: usize, x: f32) -> BlastVictim {
        BlastVictim {
            slot,
            origin: Vec3::new(x, 0.0, 0.0),
            link_origin: Vec3::new(x.trunc(), 0.0, 0.0),
            mins: Vec3::new(-15.0, -15.0, 0.0),
            maxs: Vec3::new(15.0, 15.0, 72.0),
            eye: Vec3::new(x, 0.0, 60.0),
        }
    }

    /// `G_RadiusDamage`'s falloff (combat doc, 14.1) on the frag's own
    /// numbers, truncated the way retail truncates: 120 at the blast, 5 at
    /// the radius, nothing past it. The 137-unit entry is the retail pair
    /// capture, where a blast at (1329, 3297, -22) left a target standing at
    /// (1192, 3296, -23.9) with health 26 and `damageCount` 74.
    #[test]
    fn radius_damage_falls_from_inner_to_outer() {
        let v = [
            blast_victim(1, 0.0),
            blast_victim(2, 137.0),
            blast_victim(3, 175.0),
            blast_victim(4, 349.0),
            blast_victim(5, 351.0),
        ];
        let hits = radius_damage(
            Vec3::ZERO,
            350.0,
            120.0,
            5.0,
            Some(0),
            None,
            "fraggrenade_mp",
            "MOD_GRENADE_SPLASH",
            &v,
            None,
            &[],
            &[],
            None,
        );
        let by: std::collections::BTreeMap<usize, i32> =
            hits.iter().map(|h| (h.victim, h.damage)).collect();
        assert_eq!(by[&1], 120);
        assert_eq!(by[&2], 74, "74.98 truncated, the capture's own number");
        assert_eq!(by[&3], 62, "62.5 truncated");
        assert_eq!(by[&4], 5, "5.33 truncated");
        assert!(!by.contains_key(&5), "past the radius");
        assert!(
            hits.iter()
                .all(|h| h.dflags == DFLAG_RADIUS && h.mod_ == "MOD_GRENADE_SPLASH")
        );
        assert!(hits.iter().all(|h| h.hitloc == "none" && h.attacker == 0));
        // The direction is the blast to the victim with 24 added to z, and
        // not a unit vector: its length is what carries the distance.
        let at137 = hits.iter().find(|h| h.victim == 2).unwrap();
        assert_eq!(at137.dir, [137.0, 0.0, 24.0]);
        assert_eq!(at137.point, [0.0; 3]);
    }

    /// `CanDamage` (combat doc, 14.3) counts clear traces: five in the open,
    /// two through a waist-high wall, none through a full one. The fraction
    /// scales the falloff, and a victim with no line of sight at all takes
    /// only the second chance's tenth, and only inside `radius * 0.2`.
    #[test]
    fn line_of_sight_scales_the_blast_and_the_second_chance_arm_is_a_tenth() {
        let at = Vec3::new(0.0, 0.0, 8.0);
        let near = blast_victim(1, 50.0);
        let far = blast_victim(2, 100.0);
        let open = vcod_common::collision::test_world(&[]);
        assert_eq!(can_damage(at, &far, &open, &[], &[], None), 1.0);

        // Close to the victim and waist-high: the body centre and the two
        // low probes are behind it, the two shoulder ones clear it.
        let waist = vcod_common::collision::test_world(&[(
            Vec3::new(80.0, -64.0, 0.0),
            Vec3::new(88.0, 64.0, 40.0),
        )]);
        assert!((can_damage(at, &far, &waist, &[], &[], None) - 2.0 / 3.0).abs() < 1e-6);

        let wall = vcod_common::collision::test_world(&[(
            Vec3::new(20.0, -64.0, 0.0),
            Vec3::new(28.0, 64.0, 128.0),
        )]);
        assert_eq!(can_damage(at, &far, &wall, &[], &[], None), 0.0);
        assert_eq!(can_damage(at, &near, &wall, &[], &[], None), 0.0);

        let blast = |v: &BlastVictim, w: &vcod_common::collision::CollisionWorld| {
            radius_damage(
                at,
                350.0,
                120.0,
                5.0,
                Some(0),
                None,
                "fraggrenade_mp",
                "MOD_GRENADE_SPLASH",
                std::slice::from_ref(v),
                Some(w),
                &[],
                &[],
                None,
            )
        };
        // Two thirds of the falloff at 100 units: 87.14 * 2/3.
        let partial = blast(&far, &waist);
        assert_eq!(partial[0].damage, 58);
        // Behind the wall and inside `radius * 0.2`: a tenth of the falloff.
        let second = blast(&near, &wall);
        assert_eq!(second.len(), 1);
        assert_eq!(second[0].damage, 10);
        // Behind the same wall but past `radius * 0.2`: nothing at all.
        assert!(blast(&far, &wall).is_empty());

        // The second chance's midpoint comes off the box the last link
        // built, at the snapped origin: at x 64.5 the midpoint is 69.86 from
        // the blast at the link's x 64 and 70.31 at the feet, so only the
        // link puts it inside the 70 units `radius * 0.2` reaches.
        let edge = blast_victim(3, 64.5);
        let snapped = blast(&edge, &wall);
        assert_eq!(snapped.len(), 1);
        // A tenth of 98.81, the falloff at the feet's own 64.5.
        assert_eq!(snapped[0].damage, 9);
        let unsnapped = BlastVictim {
            link_origin: edge.origin,
            ..edge
        };
        assert!(blast(&unsnapped, &wall).is_empty());
    }

    /// A script model shields a blast the way the world does, since
    /// `CanDamage` traces it on retail (combat doc, 14.4), but it does not
    /// block the second chance's plain trace: a victim hidden behind one
    /// close in takes nothing, where a world wall there gives a tenth.
    #[test]
    fn a_script_model_shields_a_blast_and_leaves_no_second_chance() {
        use vcod_common::collision::ModelTri;
        let at = Vec3::new(0.0, 0.0, 8.0);
        let near = blast_victim(1, 50.0);
        // One quad at x 24, 128 across and 128 tall, facing the victim: the
        // clip is one-sided and `CanDamage` traces from the victim's probes
        // toward the blast.
        let (a, b, c, d) = (
            Vec3::new(0.0, -64.0, 0.0),
            Vec3::new(0.0, 64.0, 0.0),
            Vec3::new(0.0, 64.0, 128.0),
            Vec3::new(0.0, -64.0, 128.0),
        );
        let face = |tri| ModelTri {
            tri,
            contents: 1,
            surface_flags: 0,
        };
        let wall = PlacedModel {
            id: EntId(200, 0),
            origin: Vec3::new(24.0, 0.0, 0.0),
            axis: glam::Mat3::IDENTITY,
            tris: std::rc::Rc::from(vec![face([a, b, c]), face([a, c, d])]),
        };
        let open = vcod_common::collision::test_world(&[]);
        assert_eq!(can_damage(at, &near, &open, &[], &[], None), 1.0);
        let models = std::slice::from_ref(&wall);
        assert_eq!(can_damage(at, &near, &open, models, &[], None), 0.0);
        let hits = radius_damage(
            at,
            350.0,
            120.0,
            5.0,
            None,
            None,
            "none",
            "MOD_EXPLOSIVE",
            std::slice::from_ref(&near),
            Some(&open),
            models,
            &[],
            None,
        );
        assert!(
            hits.is_empty(),
            "{:?}",
            hits.iter().map(|h| h.damage).collect::<Vec<_>>()
        );
    }

    /// A standing box body for the no-paks path, where the link box is the
    /// whole hit.
    fn box_body(slot: usize, x: f32) -> HitBody {
        HitBody {
            slot,
            origin: Vec3::new(x, 0.0, 0.0),
            yaw: 0.0,
            mins: Vec3::new(-15.0, -15.0, 0.0),
            maxs: Vec3::new(15.0, 15.0, 72.0),
            pose: Default::default(),
        }
    }

    /// `CanDamage` traces with the victim as its pass entity, so another
    /// player's body in the line stops a probe and the victim's own does not
    /// (combat doc, 14.4). With no rig to pose, the link box is the hit.
    #[test]
    fn another_body_in_the_line_stops_the_probes_and_the_victims_own_does_not() {
        let at = Vec3::new(0.0, 0.0, 8.0);
        let far = blast_victim(2, 100.0);
        let open = vcod_common::collision::test_world(&[]);
        let own = [box_body(2, 100.0)];
        assert_eq!(can_damage(at, &far, &open, &[], &own, None), 1.0);
        let front = [box_body(1, 50.0), box_body(2, 100.0)];
        assert_eq!(can_damage(at, &far, &open, &[], &front, None), 0.0);
    }

    /// `client-probes/probe_blastbody` on the retail server (combat doc,
    /// 14.4): a 20-damage `radiusDamage` at 130 units, with an allied
    /// carbine player standing 70 units down the same line at each yaw its
    /// `angles` read back, and with its corpse there instead. The back
    /// player's origin drifts between rows under the blasts' knockback, which
    /// is what takes the last two yaw-0 rows from 13 to 6.
    #[test]
    fn a_body_in_the_line_shields_a_blast_the_way_the_retail_probe_measured() {
        let Some(fs) = vcod_common::testing::game_fs() else {
            return;
        };
        let anims = vcod_common::animtree::PlayerAnims::load(&fs).expect("the player anims");
        let world = vcod_common::collision::test_world(&[]);
        let mut rigs = HitRigs::default();
        // Carentan's floor at -32 onto the test world's at 0.
        let shift = Vec3::new(226.0, -2424.0, 32.0);
        let at = Vec3::new(-176.8, 2473.1, 7.0) + shift;
        let front_feet = Vec3::new(-226.0, 2424.0, -31.87) + shift;
        // (front yaw, back x, back y, damage); `None` is the corpse.
        let rows: [(Option<f32>, f32, f32, i32); 18] = [
            (Some(0.0), -269.0, 2381.0, 13),
            (Some(0.0), -271.0, 2379.0, 13),
            (Some(0.0), -271.77, 2378.23, 13),
            (Some(0.0), -272.53, 2377.47, 13),
            (Some(0.0), -270.94, 2379.06, 13),
            (Some(0.0), -271.71, 2378.29, 13),
            (Some(0.0), -272.71, 2377.30, 13),
            (Some(0.0), -273.80, 2376.20, 13),
            (Some(0.0), -274.66, 2375.34, 13),
            (Some(0.0), -275.36, 2374.64, 6),
            (Some(0.0), -275.80, 2374.21, 6),
            (Some(135.0), -273.30, 2376.70, 0),
            (Some(180.0), -273.30, 2376.70, 6),
            (Some(225.0), -273.30, 2376.70, 0),
            (Some(270.0), -273.30, 2376.70, 0),
            (Some(315.0), -273.30, 2376.70, 0),
            (None, -273.30, 2376.70, 20),
            (None, -276.51, 2373.49, 20),
        ];
        for (yaw, bx, by, want) in rows {
            let back_feet = Vec3::new(bx, by, -31.99) + shift;
            let mut back = new_for_test(back_feet.into(), 225.0);
            back.assembly = stock_assembly();
            settle(&mut back);
            let mut bodies = vec![back.hit_body(1).expect("a live body")];
            if let Some(yaw) = yaw {
                let mut front = new_for_test(front_feet.into(), yaw);
                front.assembly = stock_assembly();
                front.ps.on_ground = true;
                let inputs = crate::spectate::AnimInputs {
                    anims: &anims,
                    weapon: "m1carbine_mp",
                    weapon_class: "rifle",
                };
                front.update_anims(
                    &inputs,
                    &vcod_common::net::msg::NULL_USERCMD,
                    0,
                    &[],
                    &mut 1u64,
                );
                settle(&mut front);
                bodies.push(front.hit_body(0).expect("a live body"));
            }
            let victim = BlastVictim {
                slot: 1,
                origin: back_feet,
                link_origin: back_feet.trunc(),
                mins: Vec3::new(-15.0, -15.0, 0.0),
                maxs: Vec3::new(15.0, 15.0, 72.0),
                eye: back_feet + Vec3::Z * 60.0,
            };
            let mut ctx = BoneTraceCtx {
                fs: &fs,
                anims: &anims,
                rigs: &mut rigs,
                now_ms: 0,
            };
            let hits = radius_damage(
                at,
                500.0,
                20.0,
                20.0,
                None,
                None,
                "none",
                "MOD_EXPLOSIVE",
                std::slice::from_ref(&victim),
                Some(&world),
                &[],
                &bodies,
                Some(&mut ctx),
            );
            let got = hits.first().map_or(0, |h| h.damage);
            assert_eq!(got, want, "front yaw {yaw:?}, back at ({bx}, {by})");
        }
    }
}
