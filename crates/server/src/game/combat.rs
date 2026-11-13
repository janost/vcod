//! Bullets, swings and what they hit: means of death, the hit-location
//! table, the box partition a hit point resolves to, and the two attacks
//! themselves (`docs/research/cod11-combat.md`, sections 2 to 4).

use crate::game::hitrig::HitRigs;
use crate::game::temp_entity::{Scope, TempEntity};
use crate::spectate::ClientSim;
use glam::{Quat, Vec3};
use vcod_common::animtree::PlayerAnims;
use vcod_common::bonetrace::{PriorityMap, bone_trace_in, into_frame_stored};
use vcod_common::collision::{CONTENTS_GLASS, CollisionWorld, sound_material};
use vcod_common::net::events::dir_to_byte;
use vcod_common::net::protocol::ENTITYNUM_WORLD;
use vcod_common::patch::Arith;
use vcod_common::pk3::Pk3Fs;
use vcod_common::playerpose::pose_player;
use vcod_common::pmove::weapon::{SpreadStance, hip_spread_min};
use vcod_common::pmove::{MoveRules, PlayerState};
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
    /// `GameHost::dobj_posed`, for a script trace: a bone the frame already
    /// posed keeps that pose, and the trace stamps every other one at
    /// `now_ms` (movers doc 15, "A bone asked for before the anim update").
    pub posed: Option<&'a mut [DobjStamps]>,
    /// The server build's float arithmetic: 1.5b's and UO's store every
    /// step of the trace (combat doc 3.1, "The arithmetic on 1.5 and UO").
    pub arith: Arith,
}

impl<'a> BoneTraceCtx<'a> {
    /// The context when the host has paks and an animtree; `posed` is
    /// `GameHost::dobj_posed`, which every engine and script trace stamps.
    pub fn of(
        fs: Option<&'a vcod_common::pk3::Pk3Fs>,
        anims: Option<&'a PlayerAnims>,
        rigs: &'a mut HitRigs,
        now_ms: i32,
        posed: &'a mut [DobjStamps],
        arith: Arith,
    ) -> Option<Self> {
        Some(Self {
            fs: fs?,
            anims: anims?,
            rigs,
            now_ms,
            posed: Some(posed),
            arith,
        })
    }
}

/// One client's skeleton under the current stamp (`GameHost::dobj_posed`):
/// when each bone was first posed and by which build. Each build that finds
/// a bone missing runs the DObj's controller hook, which on 1.5 and UO eases
/// the controls one more step from where the last build left them, in this
/// stamp or an earlier one; a stamp nothing builds in eases nothing
/// (docs/research/coduo-vehicles.md 13, "The glide back"; cod11-movers.md
/// 15).
#[derive(Clone, Default, Debug)]
pub struct DobjStamps {
    /// Each bone's posing time and build.
    bones: Vec<Option<(i32, usize)>>,
    /// The controls each build posed with.
    builds: Vec<Option<vcod_common::playerpose::Controllers>>,
    /// The controls the last build eased to (the record's `+0x408` block),
    /// kept across stamps: the next build eases on from them.
    record: Option<vcod_common::playerpose::Controllers>,
    /// `r.currentAngles`, which places the DObj, while a `setPlayerAngles`
    /// holds them (pitch and roll too) until the client's next cmd writes
    /// `(0, view yaw, 0)` back (`SetClientViewAngle` 0x4209f,
    /// `ClientThink_real` 0x405e2; cod11-combat.md 3.7).
    angles: Option<[f32; 3]>,
}

impl DobjStamps {
    /// A new stamp: every bone goes unposed, the record stays.
    pub fn clear(&mut self) {
        self.bones.clear();
        self.builds.clear();
    }

    /// A new client in the slot: no record either.
    pub fn reset(&mut self) {
        *self = Self::default();
    }

    /// Stamps each bone of `want` the stamp has not posed with `now_ms`
    /// under a new build (one hook run), and returns every `want` bone's
    /// stamp in order.
    pub fn stamp(
        &mut self,
        n_bones: usize,
        want: &[usize],
        now_ms: i32,
        pose: &BodyPose,
    ) -> Vec<(i32, usize)> {
        self.bones.resize(n_bones, None);
        if want.iter().any(|&b| self.bones[b].is_none()) {
            let build = self.builds.len();
            // `pose.eased` is the record as the last end frame took it; 1.1
            // (`None`) sets its controls outright.
            let last = match self.builds.last() {
                Some(last) => *last,
                None => pose.eased.and(self.record.or(pose.eased)),
            };
            let controls = last.map(|mut c| {
                if let Some(target) = pose.target_controllers() {
                    c.ease_toward(&target, pose.ease_ms);
                }
                c
            });
            if controls.is_some() {
                self.record = controls;
            }
            self.builds.push(controls);
            for &b in want {
                self.bones[b].get_or_insert((now_ms, build));
            }
        }
        want.iter()
            .map(|&b| self.bones[b].expect("stamped"))
            .collect()
    }

    /// `pose`'s inputs at `now_ms` with the controls of `build`.
    pub fn inputs<'a>(
        &self,
        pose: &'a BodyPose,
        anims: &'a PlayerAnims,
        now_ms: i32,
        build: usize,
    ) -> vcod_common::playerpose::PoseInputs<'a> {
        let mut inputs = pose.pose_inputs(anims, now_ms);
        if let Some(c) = self.builds.get(build).copied().flatten() {
            inputs.controllers = Some(c);
        }
        inputs
    }

    /// `setPlayerAngles` wrote `r.currentAngles`; `None` once a cmd wrote
    /// them back.
    pub fn set_angles(&mut self, angles: Option<[f32; 3]>) {
        self.angles = angles;
    }

    /// The rotation a trace poses `body` in: `r.currentAngles`.
    pub fn frame(&self, body: &HitBody) -> Quat {
        self.angles
            .map_or_else(|| body.frame(), vcod_common::turretpose::angles_quat)
    }

    /// [`Self::frame`]'s axes as `AnglesToAxis` rounds them
    /// (`x87::angles_to_axis`).
    pub fn axis_stored(&self, body: &HitBody) -> [Vec3; 3] {
        self.angles.map_or_else(
            || body.axis_stored(),
            |a| vcod_common::x87::angles_to_axis(a.into()),
        )
    }

    /// The controls the last build eased to, which the end frame's record
    /// takes (`ClientSim::keep_eased`).
    pub fn record(&self) -> Option<vcod_common::playerpose::Controllers> {
        self.record
    }
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
    /// An entity with no client struck (a UO vehicle), for its `G_Damage`.
    EntityHit(EntityHit),
    /// A leg's `G_CheckHitTriggerDamage`, run before anything the leg
    /// struck takes its damage (combat doc 14.8).
    TriggerSweep(TriggerSweep),
}

/// One segment `G_CheckHitTriggerDamage` walks: the leg's start to where
/// its trace stopped, the leg's damage before any falloff, its means of
/// death.
#[derive(Clone, Copy, Debug)]
pub struct TriggerSweep {
    pub attacker: usize,
    pub start: Vec3,
    pub end: Vec3,
    pub damage: i32,
    pub mod_: &'static str,
}

/// A round's `G_Damage` on an entity with no client: the falloff's damage,
/// the shot's forward as the direction (combat doc 2.4, step 4).
#[derive(Clone, Debug)]
pub struct EntityHit {
    pub target: EntId,
    pub attacker: usize,
    /// The round's own inflictor when it is not the attacker: a vehicle's
    /// gun's vehicle.
    pub inflictor: Option<EntId>,
    pub damage: i32,
    pub dflags: i32,
    pub mod_: &'static str,
    pub point: [f32; 3],
    pub dir: [f32; 3],
}

/// What an attack did, in retail's order: `Bullet_Fire_Extended` runs each
/// leg's `G_Damage` before it traces the next leg (combat doc 2.4, step 5),
/// and `Weapon_Melee` spawns its event ahead of its `G_Damage` (2.5), so a
/// caller applies these in sequence and the callback's temp entities number
/// between them.
#[derive(Default)]
pub struct ShotResult {
    pub effects: Vec<Effect>,
    /// The shootable grenades a round damaged, for `G_GrenadeDie`.
    pub struck: Vec<EntId>,
}

/// An entity with no client a round stops on, UO's shootable grenade: its
/// box, and the client whose traces still pass it (its `r.ownerNum`).
#[derive(Clone, Copy, Debug)]
pub struct ShotTarget {
    pub id: EntId,
    pub lo: Vec3,
    pub hi: Vec3,
    pub owner: Option<usize>,
}

impl ShotResult {
    pub fn impacts(&self) -> impl Iterator<Item = &TempEntity> {
        self.effects.iter().filter_map(|e| match e {
            Effect::Impact(te) => Some(te),
            Effect::Hit(_) | Effect::EntityHit(_) | Effect::TriggerSweep(_) => None,
        })
    }

    pub fn hits(&self) -> impl Iterator<Item = &Hit> {
        self.effects.iter().filter_map(|e| match e {
            Effect::Hit(h) => Some(h),
            Effect::Impact(_) | Effect::EntityHit(_) | Effect::TriggerSweep(_) => None,
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
/// How far a bullet travels on 1.1 and 1.5: `muzzle + forward * 8192`
/// (combat doc, 2.2).
pub const BULLET_RANGE: f32 = 8192.0;
/// UO's `BG_Bullet_Endpos` (game.mp.uo 0x3011c) runs 10000 out, for every
/// `Bullet_Fire` and the artillery strike (combat doc, 2.2).
pub const UO_BULLET_RANGE: f32 = 10_000.0;

/// How far a protocol's server traces a bullet.
pub fn bullet_range(p: &vcod_common::net::protocol::Protocol) -> f32 {
    if p.is_uo() {
        UO_BULLET_RANGE
    } else {
        BULLET_RANGE
    }
}

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
/// the last end frame's `aimSpreadScale` (`ClientSim::spread_frac`), or the
/// same blend from `adsSpread` off a settled sight.
/// `ads` is `fWeaponPosFrac == 1.0`, the exact compare retail's two arms
/// split on, and not the usercmd's sight bit: the sight is up for the whole
/// of a transition and the cone is the hip one until it lands.
fn spread_deg(def: &WeaponDef, sim: &ClientSim, ads: bool, stance: &SpreadStance) -> f32 {
    let scale = sim.spread_frac;
    let min = if ads {
        def.ads_spread
    } else {
        hip_spread_min(def, stance)
    };
    min + (def.hip_spread_max - min) * scale
}

/// [`spread_deg`] from the hip for a shot `sim` fires on its next cmd,
/// standing as it stands now.
pub(crate) fn hip_spread_now(def: &WeaponDef, sim: &ClientSim) -> f32 {
    spread_deg(def, sim, false, &SpreadStance::of(&sim.ps, 0, 0))
}

/// `gunrandom` (combat doc, 2.2): a point in the unit disc with the radius
/// drawn uniformly, so the cone is denser at its centre.
pub(crate) fn gun_random(rng: &mut u64) -> (f32, f32) {
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
/// lean, then each component truncated again (combat doc 2.1). 1.5 and UO
/// start from the unsnapped origin, add the walk bob, never truncate and
/// keep the point 8 over the feet ([`muzzle_point_in`]).
pub fn muzzle_point(ps: &PlayerState) -> Vec3 {
    muzzle_point_in(ps, None)
}

/// [`muzzle_point`] with the world UO's deployed LMG traces its muzzle
/// against. On 1.5 and UO it is `CalcMuzzlePoint` (1.5 game 0x7e91f, UO
/// 0x8b8ea): `ps.origin` plus the eye (UO: 28 in a vehicle seat), then,
/// unless on a turret, UO's bipod trace, the walk bob, the lean and a
/// floor 8 over the feet (combat doc 2.1, "1.5 and United Offensive").
pub fn muzzle_point_in(ps: &PlayerState, world: Option<&CollisionWorld>) -> Vec3 {
    if !ps.rules.v15() {
        // `G_AddLean`, the offset `PlayerState::view` adds, on the snapped
        // origin.
        let eye = ps.origin.trunc() + Vec3::Z * ps.view_height();
        return vcod_common::pmove::aim::add_lean(eye, ps.yaw, ps.lean, Arith::Kept).trunc();
    }
    let o = ps.origin;
    let uo = ps.rules == MoveRules::Uo;
    let eye = if uo && ps.seat.is_some() {
        UO_SEAT_MUZZLE_HEIGHT
    } else {
        ps.view_height()
    };
    let mut p = Vec3::new(o.x, o.y, o.z + eye);
    if ps.mounted.is_some() {
        return p;
    }
    if uo && ps.deployed() {
        p = bipod_muzzle(ps, world, p);
    }
    p = vcod_common::pmove::aim::add_muzzle_bob(ps, p);
    p = vcod_common::pmove::aim::add_lean(p, ps.yaw, ps.lean, Arith::Stored);
    p.z = p.z.max(o.z + MUZZLE_FLOOR);
    p
}

/// UO's muzzle height over a seated rider's origin (game rodata 0xb5f88).
const UO_SEAT_MUZZLE_HEIGHT: f32 = 28.0;
/// How far over the feet 1.5's and UO's muzzle stays (1.5 rodata 0x8f1c4,
/// UO 0xb5f90).
const MUZZLE_FLOOR: f32 = 8.0;
/// UO's deployed-LMG muzzle trace reach (rodata 0xb5f8c).
const BIPOD_MUZZLE_REACH: f32 = 19.0;

/// UO's `CalcMuzzlePoint` arm for an LMG on its bipod (`weaponClass` 3 with
/// pm_flags 0x20, 0x8b9b6..0x8ba6a): an 8-unit box (.data 0xbe93c) traced
/// from the muzzle 19 forward along `proneDirection` under mask 0x11, the
/// muzzle moved to where it stops unless it is all solid. Only the world
/// is traced here; retail's `trap_Trace` meets entities too.
fn bipod_muzzle(ps: &PlayerState, world: Option<&CollisionWorld>, muzzle: Vec3) -> Vec3 {
    let Some(world) = world else {
        return muzzle;
    };
    let yaw = ps.prone_direction.to_radians();
    let end = muzzle + Vec3::new(yaw.cos(), yaw.sin(), 0.0) * BIPOD_MUZZLE_REACH;
    let half = Vec3::splat(8.0);
    let tr = world.move_trace(muzzle, end, -half, half, 0x11);
    if tr.allsolid { muzzle } else { tr.endpos }
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
    uo: Option<UoRound>,
    ads: bool,
    stance: &SpreadStance,
    aim: [f32; 2],
    sims: &[(usize, &ClientSim)],
    targets: &[ShotTarget],
    world: Option<&CollisionWorld>,
    models: &[PlacedModel],
    hitlocs: &HitLocTable,
    bones: Option<&mut BoneTraceCtx>,
    rng: &mut u64,
) -> ShotResult {
    let Some((_, me)) = sims.iter().find(|(slot, _)| *slot == shooter) else {
        return ShotResult::default();
    };
    // Only a UO server's rounds carry a `UoRound`.
    let range = if uo.is_some() {
        UO_BULLET_RANGE
    } else {
        BULLET_RANGE
    };
    let (muzzle, forward, end) = aim_end(me, def, ads, stance, aim, range, world, rng);
    let round = Round {
        damage: def.damage,
        rifle_bullet: def.sounds.rifle_bullet,
        weapon_name,
        uo,
        falloff: uo.map(|_| Falloff::of(def)),
    };
    let pass = Pass::client(shooter, sims);
    fire_round(
        shooter,
        pass,
        muzzle,
        end,
        forward,
        round,
        sims,
        targets,
        world,
        models,
        hitlocs,
        bones,
        Some(rng),
    )
}

/// `CalcMuzzlePoints` and `BG_Bullet_Endpos`: the muzzle, the aim's forward
/// and where a round with the weapon's spread ends, `range` out.
/// `world` is what a UO LMG on its bipod traces its muzzle against; no
/// rocket or strike is fired from one.
#[allow(clippy::too_many_arguments)]
pub fn aim_end(
    me: &ClientSim,
    def: &WeaponDef,
    ads: bool,
    stance: &SpreadStance,
    aim: [f32; 2],
    range: f32,
    world: Option<&CollisionWorld>,
    rng: &mut u64,
) -> (Vec3, Vec3, Vec3) {
    let muzzle = muzzle_point_in(&me.ps, world);
    let frame = aim_frame(aim);
    let spread = spread_deg(def, me, ads, stance);
    let end = spread_point(&frame, muzzle, spread, range, gun_random(rng), arith_of(me));
    (muzzle, frame[0], end)
}

/// `Weapon_RocketLauncher_Fire` (1.1d 0x68c40, 1.5b 0x6e6bc, UO 0x7b3da):
/// the muzzle, the aim's forward and the rocket's direction, the spread
/// point 16 out normalized (combat doc 13.6). `draw` is `gunrandom`'s pair.
pub fn rocket_aim(
    me: &ClientSim,
    def: &WeaponDef,
    ads: bool,
    stance: &SpreadStance,
    aim: [f32; 2],
    draw: (f32, f32),
) -> (Vec3, Vec3, Vec3) {
    let muzzle = muzzle_point_in(&me.ps, None);
    let frame = aim_frame(aim);
    let spread = spread_deg(def, me, ads, stance);
    let dir = rocket_dir(&frame, spread, draw, arith_of(me));
    (muzzle, frame[0], dir)
}

/// The rocket's spread point 16 out from the origin, normalized as the
/// protocol's `VectorNormalize` rounds.
pub(crate) fn rocket_dir(frame: &[Vec3; 3], spread: f32, draw: (f32, f32), arith: Arith) -> Vec3 {
    let d = spread_point(frame, Vec3::ZERO, spread, ROCKET_REACH, draw, arith);
    match arith {
        Arith::Kept => vcod_common::x87::normalize_in_place(d),
        Arith::Stored | Arith::Msvc => vcod_common::x87::normalize_stored(d),
    }
}

/// How far out `Weapon_RocketLauncher_Fire` builds its spread point
/// (1.1d .rodata 0x79c68, UO 0x7b3ec).
const ROCKET_REACH: f32 = 16.0;

/// A 1.1 shooter's sums stay on the x87 stack; 1.5b's and UO's are stored.
fn arith_of(me: &ClientSim) -> Arith {
    if me.ps.rules.v15() {
        Arith::Stored
    } else {
        Arith::Kept
    }
}

/// `AngleVectors` of the aim block's pitch and yaw with no roll: forward,
/// right and up. Each angle rounded to float radians, its `fsincos` stored
/// (UO 0x3b068, 0x3f464); with the roll 0 every component is one product
/// or one sine, so the three servers agree. `right.z` is `-sin(roll) *
/// cos(pitch)`, a negative zero.
pub fn aim_frame(aim: [f32; 2]) -> [Vec3; 3] {
    let sin_cos = |deg: f32| {
        let rad = f64::from((f64::from(deg) * (std::f64::consts::PI / 180.0)) as f32);
        (rad.sin() as f32, rad.cos() as f32)
    };
    let (sp, cp) = sin_cos(aim[0]);
    let (sy, cy) = sin_cos(aim[1]);
    [
        Vec3::new(cp * cy, cp * sy, -sp),
        Vec3::new(sy, -cy, -0.0),
        Vec3::new(sp * cy, sp * sy, cp),
    ]
}

/// `base + forward * reach + right * x * k + up * y * k`, `k` the spread's
/// `tan` times `reach` and `(x, y)` the `gunrandom` draw: `BG_Bullet_Endpos`
/// (1.1d inline in `Bullet_Fire` 0x690dc, UO 0x3011c) with `base` the
/// muzzle, the rocket's direction before its normalize with `base` 0, a UO
/// vehicle gun's with the flash tag's frame. 1.1d keeps everything past the
/// tangent's float on the x87 stack and rounds each component once; 1.5b
/// and UO store `k`, the two offsets and each component after every add
/// (combat doc 2.2).
pub(crate) fn spread_point(
    frame: &[Vec3; 3],
    base: Vec3,
    spread: f32,
    reach: f32,
    (x, y): (f32, f32),
    arith: Arith,
) -> Vec3 {
    let [f, r, u] = frame.map(|v| v.as_dvec3());
    let base = base.as_dvec3();
    let t = f64::from((f64::from(spread) * std::f64::consts::PI / 180.0).tan() as f32);
    let reach = f64::from(reach);
    match arith {
        Arith::Kept => {
            let k = t * reach;
            let (x, y) = (f64::from(x) * k, f64::from(y) * k);
            (f * reach + base + r * x + u * y).as_vec3()
        }
        Arith::Stored | Arith::Msvc => {
            let st = |v: f64| f64::from(v as f32);
            let k = st(t * reach);
            let (x, y) = (st(f64::from(x) * k), st(f64::from(y) * k));
            let e = (f * reach + base).as_vec3().as_dvec3();
            let e = (r * x + e).as_vec3().as_dvec3();
            (u * y + e).as_vec3()
        }
    }
}

/// `muzzle + forward * range`, the product unrounded: `BG_Bullet_Endpos`
/// adds it on the x87 stack (game.mp.uo 0x30189), which only a range that
/// is no power of two can tell apart.
fn bullet_end(muzzle: Vec3, forward: Vec3, range: f32) -> Vec3 {
    (muzzle.as_dvec3() + forward.as_dvec3() * f64::from(range)).as_vec3()
}

/// A turret's round (turrets doc 6.3): `Bullet_Fire` with no spread from the
/// muzzle the gun worked out, along `dir`. The callback is told the gunner's
/// carried weapon and names the gunner as the inflictor (turrets doc 13).
/// `pass` is the trace's pass entity: the gunner on a turret, the vehicle
/// for a UO vehicle's gun, whose riders it then passes too; `falloff` a UO
/// weapon's `Damage_Falloff`.
#[allow(clippy::too_many_arguments)]
pub fn bullet_fire_from(
    shooter: usize,
    pass: Pass,
    muzzle: Vec3,
    dir: Vec3,
    range: f32,
    damage: i32,
    rifle_bullet: bool,
    falloff: Option<Falloff>,
    weapon_name: &str,
    sims: &[(usize, &ClientSim)],
    targets: &[ShotTarget],
    world: Option<&CollisionWorld>,
    models: &[PlacedModel],
    hitlocs: &HitLocTable,
    bones: Option<&mut BoneTraceCtx>,
) -> ShotResult {
    let end = bullet_end(muzzle, dir, range);
    bullet_fire_to(
        shooter,
        pass,
        muzzle,
        end,
        dir,
        damage,
        rifle_bullet,
        falloff,
        weapon_name,
        sims,
        targets,
        world,
        models,
        hitlocs,
        bones,
    )
}

/// [`bullet_fire_from`] to an end point already worked out: a UO vehicle
/// gun's `BG_Bullet_Endpos` with its spread (`vehicle::fire::prepare`).
#[allow(clippy::too_many_arguments)]
pub fn bullet_fire_to(
    shooter: usize,
    pass: Pass,
    muzzle: Vec3,
    end: Vec3,
    forward: Vec3,
    damage: i32,
    rifle_bullet: bool,
    falloff: Option<Falloff>,
    weapon_name: &str,
    sims: &[(usize, &ClientSim)],
    targets: &[ShotTarget],
    world: Option<&CollisionWorld>,
    models: &[PlacedModel],
    hitlocs: &HitLocTable,
    bones: Option<&mut BoneTraceCtx>,
) -> ShotResult {
    let round = Round {
        damage,
        rifle_bullet,
        weapon_name,
        uo: None,
        falloff,
    };
    fire_round(
        shooter, pass, muzzle, end, forward, round, sims, targets, world, models, hitlocs, bones,
        None,
    )
}

/// `SV_ClipMoveToEntity`'s pass rule (`coduo_lnxded` 0x809a72a): with a
/// pass entity, a trace skips it, whatever it owns (`r.ownerNum`), and
/// whatever its own owner owns. A seated UO rider's owner is his vehicle
/// and a driven vehicle's its driver, so a vehicle's round passes its
/// riders, and a rider's passes the others in his seat's vehicle but not
/// the vehicle itself (docs/research/coduo-vehicles.md 13).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Pass {
    pub ent: u32,
    /// The pass entity's `r.ownerNum`, `None` for `ENTITYNUM_NONE`.
    pub owner: Option<u32>,
}

impl Pass {
    /// Client `slot` as the pass entity, owned by the vehicle it sits in.
    pub fn client(slot: usize, sims: &[(usize, &ClientSim)]) -> Pass {
        let owner = sims
            .iter()
            .find(|(s, _)| *s == slot)
            .and_then(|(_, sim)| sim.seat.map(|seat| seat.vehicle));
        Pass {
            ent: slot as u32,
            owner,
        }
    }

    /// Whether the trace skips entity `ent`, owned by `owner`.
    pub fn skips(&self, ent: u32, owner: Option<u32>) -> bool {
        ent == self.ent || owner.is_some_and(|o| o == self.ent || Some(o) == self.owner)
    }
}

/// United Offensive's `Damage_Falloff` (game.mp.uo 0x7a7f8) off the weapon
/// file's `minDamagePercent`, `damageInnerRadius` and `damageOuterRadius`
/// (weaponDef +0x210..0x218): whole to the inner radius, `minDamagePercent`
/// of it past the outer, a straight line between
/// (docs/research/coduo-diff.md 21).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Falloff {
    pub min_percent: i32,
    pub inner: i32,
    pub outer: i32,
}

impl Falloff {
    pub fn of(def: &WeaponDef) -> Falloff {
        Falloff {
            min_percent: def.min_damage_percent,
            inner: def.damage_inner_radius,
            outer: def.damage_outer_radius,
        }
    }

    /// The damage a round carries `dist` units from where its leg started,
    /// truncated as `Bullet_Fire_Extended` stores it (an `fistp` under a
    /// 0xc00 control word, 0x7b10b). Each x87 step is stored to an f32
    /// where the binary stores one.
    pub fn damage(&self, dist: f32, damage: i32) -> i32 {
        let d = f64::from(damage as f32);
        let min = f64::from(self.min_percent as f32);
        let pct = f64::from(0.01f32);
        let v = if dist <= self.inner as f32 {
            damage as f32
        } else if dist > self.outer as f32 {
            (min * pct * d) as f32
        } else {
            let span = f64::from(self.outer - self.inner);
            let t = (1.0 - (f64::from(dist) - f64::from(self.inner)) / span) as f32;
            (((100.0 - min) * f64::from(t) + min) * pct * d) as f32
        };
        v.max(0.0) as i32
    }
}

/// What a bullet carries beyond its path.
struct Round<'a> {
    damage: i32,
    rifle_bullet: bool,
    weapon_name: &'a str,
    uo: Option<UoRound>,
    /// A UO weapon's distance falloff, run on every leg.
    falloff: Option<Falloff>,
}

/// A UO player's round (`Bullet_Fire_Extended`, game.mp.uo 0x7a920): the
/// world impact carries the weapon and the round's reflection as a byte dir
/// in `es.scale`, where 1.1's sends both 0, and a round that meets a player
/// or nothing may draw `EV_BULLET_TRACER` from the muzzle.
#[derive(Clone, Copy, Debug)]
pub struct UoRound {
    pub weapon: i32,
    /// `g_tracerChanceLMG` for an `lmg`/`hmg`/`umg` gun, else
    /// `g_tracerChance`: what each leg's tracer roll is under.
    pub tracer_chance: f32,
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
/// after 13 legs. A UO round's damage first takes its weapon's falloff over
/// the leg, and its halving halves that (game.mp.uo 0x7a920); a UO model
/// it meets (a vehicle) takes `G_Damage` and stops it. A UO round rolls its
/// tracer off `rng` on each leg that may draw one (coduo-diff.md 21).
#[allow(clippy::too_many_arguments)]
fn fire_round(
    shooter: usize,
    pass: Pass,
    muzzle: Vec3,
    end: Vec3,
    forward: Vec3,
    round: Round,
    sims: &[(usize, &ClientSim)],
    targets: &[ShotTarget],
    world: Option<&CollisionWorld>,
    models: &[PlacedModel],
    hitlocs: &HitLocTable,
    mut bones: Option<&mut BoneTraceCtx>,
    mut rng: Option<&mut u64>,
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
    let (mut start, mut pass, mut damage) = (muzzle, pass, round.damage);
    // `EV_BULLET_TRACER` from the leg's start, drawn when `g_tracerChance`
    // beats the leg's `rand()` (0x7adb7, 0x7aeb1).
    let mut tracer = |from: Vec3, to: Vec3| {
        let u = round.uo?;
        let drawn = crate::game::host::rand_unit(rng.as_deref_mut()?) < u.tracer_chance;
        drawn.then(|| {
            Effect::Impact(TempEntity {
                event: vcod_common::net::event_ids::EV_BULLET_TRACER,
                parm: u.weapon,
                surf_type: 0,
                other: 0,
                attacker: 0,
                weapon: 0,
                origin: from.into(),
                client_num: 0,
                scale: 0,
                scope: Scope::Pvs,
                quake: None,
                origin2: Some(to.into()),
            })
        })
    };
    // A UO round draws its impact only on a leg whose pass entity is the
    // shooter (game.mp.uo 0x7aa77): not past a player it went through.
    let draws_impact = |pass: &Pass| round.uo.is_none() || pass.ent == shooter as u32;
    // The leg's damage at `at`: the falloff over the leg, or the whole.
    let leg_damage = |start: Vec3, at: Vec3, damage: i32| match round.falloff {
        Some(f) => f.damage((at - start).length(), damage),
        None => damage,
    };
    let impact = |t_endpos: Vec3, normal: Vec3, flags: u32, start: Vec3| {
        let dir = (end - start).normalize_or_zero();
        let reflected = dir - normal * (2.0 * dir.dot(normal));
        Effect::Impact(TempEntity {
            event,
            parm: dir_to_byte(normal.into()),
            surf_type: sound_material(flags),
            other: shooter as u32,
            attacker: 0,
            weapon: round.uo.map_or(0, |u| u.weapon),
            origin: t_endpos.into(),
            client_num: 0,
            scale: round.uo.map_or(0, |_| dir_to_byte(reflected.into())),
            scope: Scope::Broadcast,
            quake: None,
            origin2: None,
        })
    };
    for _ in 0..=MAX_BULLET_DEPTH {
        // A surface with no impact draws the tracer instead, on a leg from
        // the shooter only (0x7ac3f-0x7ac57, then 0x7ad40); so does a miss
        // (0x7ae38). A player hit draws one on any leg.
        let from_shooter = pass.ent == shooter as u32;
        let traced = trace_attack(
            start,
            end,
            pass,
            sims,
            targets,
            world,
            models,
            priority,
            bones.as_deref_mut(),
        );
        out.effects.push(Effect::TriggerSweep(TriggerSweep {
            attacker: shooter,
            start,
            end: traced.endpos(start, end),
            damage,
            mod_,
        }));
        match traced {
            Traced::Player {
                slot,
                hitloc,
                point,
                ..
            } => {
                out.effects.extend(tracer(start, point));
                let dealt = leg_damage(start, point, damage);
                if dealt <= 0 {
                    break;
                }
                // `Bullet_Fire_Extended` raises no impact on a client (2.4).
                out.effects.push(Effect::Hit(Hit {
                    victim: slot,
                    attacker: shooter,
                    inflictor: None,
                    damage: located_damage(dealt, hitlocs.multiplier(hitloc)),
                    dflags,
                    mod_,
                    weapon: round.weapon_name.to_string(),
                    point: point.into(),
                    dir: forward.into(),
                    hitloc,
                }));
                damage = match round.falloff {
                    // `(int)(0.5f * damage)`, truncated (0x7b266).
                    Some(_) => (0.5 * dealt as f32) as i32,
                    None => damage / 2,
                };
                if dflags & DFLAG_PASSTHRU == 0 || damage <= 0 {
                    break;
                }
                start = point;
                pass = Pass::client(slot, sims);
            }
            Traced::Model {
                id,
                fraction,
                normal,
                surface_flags,
                takes_damage,
            } => {
                let point = start + (end - start) * fraction;
                if surface_flags & SURF_NO_IMPACT != 0 && from_shooter {
                    out.effects.extend(tracer(start, point));
                }
                if surface_flags & SURF_NO_IMPACT == 0 && draws_impact(&pass) {
                    out.effects
                        .push(impact(point, normal, surface_flags, start));
                }
                // The entity's own contents (a vehicle's 0xa00000, a script
                // model's 0x2080) are the trace's, never glass: the round
                // ends here, and `G_Damage` runs only on `takedamage` (2.4).
                let dealt = leg_damage(start, point, damage);
                if takes_damage && dealt > 0 {
                    out.effects.push(Effect::EntityHit(EntityHit {
                        target: id,
                        attacker: shooter,
                        inflictor: None,
                        damage: dealt,
                        dflags,
                        mod_,
                        point: point.into(),
                        dir: forward.into(),
                    }));
                }
                break;
            }
            Traced::World(t) => {
                if t.surface_flags & SURF_NO_IMPACT != 0 && from_shooter {
                    out.effects.extend(tracer(start, t.endpos));
                }
                if t.surface_flags & SURF_NO_IMPACT == 0 && draws_impact(&pass) {
                    out.effects
                        .push(impact(t.endpos, t.normal, t.surface_flags, start));
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
            Traced::Entity {
                id,
                fraction,
                normal,
            } => {
                // A box with no surface: the impact carries surface 0, and
                // `G_Damage` takes the shootable grenade's 1 health.
                let point = start + (end - start) * fraction;
                let dir = (end - start).normalize_or_zero();
                let reflected = dir - normal * (2.0 * dir.dot(normal));
                out.effects.push(Effect::Impact(TempEntity {
                    event,
                    parm: dir_to_byte(normal.into()),
                    surf_type: 0,
                    other: shooter as u32,
                    attacker: 0,
                    weapon: round.uo.map_or(0, |u| u.weapon),
                    origin: point.into(),
                    client_num: 0,
                    scale: round.uo.map_or(0, |_| dir_to_byte(reflected.into())),
                    scope: Scope::Broadcast,
                    quake: None,
                    origin2: None,
                }));
                if damage > 0 {
                    out.struck.push(id);
                }
                break;
            }
            Traced::Nothing => {
                if from_shooter {
                    out.effects.extend(tracer(start, end));
                }
                break;
            }
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
        quake: None,
        origin2: None,
    };
    let client = TempEntity {
        event: client,
        parm: 0,
        scale: 0,
        client_num: victim as i32,
        scope: Scope::Only(victim),
        quake: None,
        origin2: None,
        ..base
    };
    [base, client]
}

/// What an attack's trace found: the nearest live player whose bones the ray
/// scored, a placed model it met first, the world surface it stopped on, or
/// none of them.
enum Traced {
    Player {
        slot: usize,
        hitloc: &'static str,
        /// Where along `muzzle..end`, as the server build rounds it
        /// ([`trace_point`]).
        point: Vec3,
    },
    /// An entity's xmodel collision (a script model, a UO vehicle's hull):
    /// `G_Damage`'s target when it takes damage, else a wall.
    Model {
        id: EntId,
        fraction: f32,
        normal: Vec3,
        surface_flags: u32,
        takes_damage: bool,
    },
    World(vcod_common::collision::Trace),
    /// A [`ShotTarget`]'s box, with the face's normal.
    Entity {
        id: EntId,
        fraction: f32,
        normal: Vec3,
    },
    Nothing,
}

impl Traced {
    /// Where the trace from `start` to `end` stopped.
    fn endpos(&self, start: Vec3, end: Vec3) -> Vec3 {
        match self {
            Traced::Player { point, .. } => *point,
            Traced::Model { fraction, .. } | Traced::Entity { fraction, .. } => {
                start + (end - start) * *fraction
            }
            Traced::World(t) => t.endpos,
            Traced::Nothing => end,
        }
    }
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

impl HitBody {
    /// `r.currentAngles` as a rotation, what a trace poses the body in: the
    /// yaw the last cmd wrote. A UO rider's end frame turns it to the seat
    /// tag (game.mp.uo 0x9048f), but his next cmd writes `(0, view yaw, 0)`
    /// back (0x41c74..0x41ca7) before another client's round meets him.
    pub fn frame(&self) -> Quat {
        Quat::from_rotation_z(self.yaw)
    }

    /// 1.5b's and UO's `AnglesToAxis` of `(0, yaw, 0)` (UO 0x806a46e over
    /// `AngleVectors` 0x80674ad): the yaw's float radians, its `fsincos`
    /// stored, forward, left and up.
    pub fn axis_stored(&self) -> [Vec3; 3] {
        let (s, c) = (
            f64::from(self.yaw).sin() as f32,
            f64::from(self.yaw).cos() as f32,
        );
        [Vec3::new(c, s, 0.0), Vec3::new(-s, c, 0.0), Vec3::Z]
    }
}

/// What `ClientEndFrame`'s `BG_PlayerAnimation` hands the DObj: its models,
/// both anim channels and the controllers' inputs.
#[derive(Clone, Default, Debug)]
pub struct BodyPose {
    pub assembly: crate::game::hitrig::Assembly,
    /// Wire `legsAnim` / `torsoAnim` and each channel's clip clock.
    pub legs: i32,
    pub torso: i32,
    pub legs_clock: vcod_common::animclock::ChannelClock,
    pub torso_clock: vcod_common::animclock::ChannelClock,
    /// What the end frame copied into the record, and the swings it left
    /// (`docs/research/cod11-combat.md` 16.3, 16.4).
    pub input: vcod_common::playerpose::BodyInput,
    pub angles: vcod_common::playerpose::BodyAngles,
    pub slope: vcod_common::playerpose::BodySlope,
    /// A gunner's legs blend: the turret group and the rows its placement
    /// gave it.
    pub turret_leaves: Option<(usize, Vec<vcod_common::turretpose::TurretRow>)>,
    /// `ClientSim::gunner_left`.
    pub turret_left: Option<(usize, Vec<vcod_common::turretpose::TurretRow>)>,
    /// A UO rider's seat: the frame the body is posed in and the
    /// controllers' rider branch (docs/research/coduo-vehicles.md 11.4).
    pub seat: Option<vcod_common::playerpose::BodySeat>,
    /// The patch, which picks the controllers' split and whether they ease.
    pub rules: vcod_common::pmove::MoveRules,
    /// 1.5's and UO's controls as eased so far (the record's `+0x408`
    /// block); `None` under 1.1, which sets them outright.
    pub eased: Option<vcod_common::playerpose::Controllers>,
    /// The seat blend's weights as the end frames ramped them.
    pub seat_ramp: vcod_common::playerpose::SeatRamp,
    /// `bg.frametime` at the end frame, each ease's step.
    pub ease_ms: i32,
    /// The entity fields the controllers' hook reads off `s` as the
    /// client's last `BG_PlayerStateToEntityState` wrote them, which runs
    /// after each cmd's move and in `SetClientOrigin` as well as in the end
    /// frame (docs/research/coduo-vehicles.md 13, "The weapon-swap frame").
    pub es: HookEs,
    /// The record's seat angles (`+0x3f4`): the last seat tag an end frame
    /// placed the rider by, kept past an exit, zero from the connect.
    pub ci_seat_angles: [f32; 3],
}

/// What [`BodyPose::es`] holds: whether the entity rides (`eFlags`
/// 0x100000) with its vehicle's kind and seat (`s+0x88`), and its `weapon`
/// (`s+0xcc`).
#[derive(Clone, Copy, Default, Debug, PartialEq)]
pub struct HookEs {
    /// `(kind, seat)`, `None` on foot.
    pub seat: Option<(i32, i32)>,
    pub weapon: i32,
}

impl BodyPose {
    pub fn pose_inputs<'a>(
        &'a self,
        anims: &'a PlayerAnims,
        now_ms: i32,
    ) -> vcod_common::playerpose::PoseInputs<'a> {
        let mounted = self.input.eflags & vcod_common::net::flags::EF_MOUNTED != 0;
        vcod_common::playerpose::PoseInputs {
            anims,
            legs: self.legs,
            torso: self.torso,
            legs_layers: self.legs_clock.layers(now_ms),
            torso_layers: self.torso_clock.layers(now_ms),
            nodes: self.torso_clock.node_weights(now_ms),
            // Down positive, as `descend_aim` reads it; only a gunner with no
            // placement blend descends an aim group.
            group_pitch: vcod_common::pmove::aim::angle_normalize_180(self.input.view[0]),
            turret_leaves: self
                .turret_leaves
                .as_ref()
                .map(|(n, l)| (*n, l.as_slice()))
                .filter(|_| mounted),
            turret_left: self
                .turret_left
                .as_ref()
                .map(|(n, l)| (*n, l.as_slice()))
                .filter(|_| mounted),
            controllers: self.eased.or_else(|| self.target_controllers()),
            seat_leaves: match self.seat_ramp.at(now_ms) {
                ramped if !ramped.is_empty() => ramped,
                _ => self
                    .seat
                    .as_ref()
                    .and_then(|seat| vcod_common::playerpose::seat_inputs(anims, self.legs, seat))
                    .into_iter()
                    .collect(),
            },
            leaf_times: self.seat_ramp.leaf_times(now_ms),
            node_time: self.rules.v15(),
            binds: None,
        }
    }

    /// The controls the hook aims at now: a rider's off his seat, anyone
    /// else's off the swings; `None` for a mounted one under 1.1. A dead body
    /// runs them too, its `tag_origin` turned by the legs' world yaw on top
    /// of the body's frame (combat doc 16.4, "The dying body"). Whether he
    /// rides, where and armed or not come off the entity as his last cmd
    /// left it, the angles off the record as the end frame wrote it
    /// (coduo-vehicles.md 13, "The weapon-swap frame").
    pub fn target_controllers(&self) -> Option<vcod_common::playerpose::Controllers> {
        use vcod_common::playerpose::{BodySeat, Controllers};
        match self.es.seat {
            Some((kind, pos)) => {
                let steer = self.seat.map_or(0.0, |s| s.steer);
                let seat =
                    BodySeat::new(self.ci_seat_angles, kind, pos, steer).armed(self.es.weapon);
                Some(Controllers::rider(&self.input, &seat, &self.slope))
            }
            None => Controllers::target(self.rules, &self.angles, &self.input, &self.slope),
        }
    }
}

/// The trace a bullet and a swing both make (combat doc, sections 2.2, 2.5
/// and 3): the world, every placed model and shootable box, then every live
/// player's body, each entity under `pass`'s rule.
#[allow(clippy::too_many_arguments)]
fn trace_attack(
    muzzle: Vec3,
    end: Vec3,
    pass: Pass,
    sims: &[(usize, &ClientSim)],
    targets: &[ShotTarget],
    world: Option<&CollisionWorld>,
    models: &[PlacedModel],
    priority: &PriorityMap,
    bones: Option<&mut BoneTraceCtx>,
) -> Traced {
    let trace = world.map(|w| w.shot_trace(muzzle, end));
    let mut limit = trace.as_ref().map_or(1.0, |t| t.fraction);
    let mut model_hit = None;
    for m in models {
        if pass.skips(m.id.0, m.owner) {
            continue;
        }
        if let Some((f, n, flags)) = m.clip(muzzle, end, vcod_common::collision::MASK_SHOT, limit) {
            limit = f;
            model_hit = Some((m.id, f, n, flags, m.takes_damage));
        }
    }
    // `SV_ClipMoveToEntities` skips an entity the pass entity owns.
    let target = targets
        .iter()
        .filter(|t| !pass.skips(t.id.0, t.owner.map(|o| o as u32)))
        .filter_map(|t| Some((t, ray_box(muzzle, end, t.lo, t.hi)?)))
        .filter(|(_, f)| *f < limit)
        .min_by(|a, b| a.1.total_cmp(&b.1));
    if let Some((_, f)) = target {
        limit = f;
        model_hit = None;
    }
    let bodies: Vec<HitBody> = sims
        .iter()
        .filter(|(s, sim)| !pass.skips(*s as u32, sim.seat.map(|seat| seat.vehicle)))
        .filter_map(|(s, sim)| sim.hit_body(*s))
        .collect();
    let arith = bones.as_ref().map_or(Arith::Kept, |b| b.arith);
    if let Some((slot, fraction, hitloc)) =
        trace_bodies(muzzle, end, usize::MAX, &bodies, limit, priority, bones)
    {
        return Traced::Player {
            slot,
            hitloc,
            point: trace_point(muzzle, end, fraction, arith),
        };
    }
    if let Some((t, fraction)) = target {
        return Traced::Entity {
            id: t.id,
            fraction,
            normal: box_normal(muzzle + (end - muzzle) * fraction, t.lo, t.hi),
        };
    }
    if let Some((id, fraction, normal, surface_flags, takes_damage)) = model_hit {
        return Traced::Model {
            id,
            fraction,
            normal,
            surface_flags,
            takes_damage,
        };
    }
    match trace {
        Some(t) if t.fraction < 1.0 => Traced::World(t),
        _ => Traced::Nothing,
    }
}

/// A locational trace's end point on a body or model at `fraction` of
/// `start..end`. `SV_ClipMoveToEntity` (1.1d 0x8091352, 1.5b 0x8099b18,
/// UO 0x809ab10) rounds each `start + (end - start) * fraction` once off
/// the x87 stack: `trap_LocationalTrace` is a point trace, which skips
/// the box path's stored move (cod11-combat.md 3.1).
pub fn trace_point(start: Vec3, end: Vec3, fraction: f32, arith: Arith) -> Vec3 {
    match arith {
        Arith::Stored | Arith::Kept => {
            let s = start.as_dvec3();
            (s + (end.as_dvec3() - s) * f64::from(fraction)).as_vec3()
        }
        Arith::Msvc => start + (end - start) * fraction,
    }
}

/// The outward normal of the face of `lo..hi` that `p`, on its surface,
/// lies on.
fn box_normal(p: Vec3, lo: Vec3, hi: Vec3) -> Vec3 {
    let mut best = (f32::MAX, Vec3::Z);
    for axis in 0..3 {
        for (d, sign) in [(p[axis] - lo[axis], -1.0), (hi[axis] - p[axis], 1.0)] {
            if d.abs() < best.0 {
                let mut n = Vec3::ZERO;
                n[axis] = sign;
                best = (d.abs(), n);
            }
        }
    }
    best.1
}

/// `SURF_SKY`, which a strike's trace refunds on.
const SURF_SKY: u32 = 0x4;

/// `Weapon_ArtilleryStrike_Fire`'s locational trace (game.mp.uo 0x7b758,
/// mask 0x2802031 with `bulletPriorityMap`): where the aim from `muzzle` to
/// `end` stops, on the world, a live body or a shootable grenade, and
/// whether it struck a sky face. `None` when it met nothing.
#[allow(clippy::too_many_arguments)]
pub fn strike_point(
    muzzle: Vec3,
    end: Vec3,
    shooter: usize,
    sims: &[(usize, &ClientSim)],
    targets: &[ShotTarget],
    world: Option<&CollisionWorld>,
    models: &[PlacedModel],
    bones: Option<&mut BoneTraceCtx>,
) -> Option<(Vec3, bool)> {
    match trace_attack(
        muzzle,
        end,
        Pass::client(shooter, sims),
        sims,
        targets,
        world,
        models,
        &BULLET_PRIORITY,
        bones,
    ) {
        Traced::Player { point, .. } => Some((point, false)),
        Traced::Entity { fraction, .. } | Traced::Model { fraction, .. } => {
            Some((muzzle + (end - muzzle) * fraction, false))
        }
        Traced::World(t) => Some((t.endpos, t.surface_flags & SURF_SKY != 0)),
        Traced::Nothing => None,
    }
}

/// `SV_ClipMoveToEntity`'s broad box around a player's `r.currentOrigin`
/// (`.data 0x80f5110` in `coduo_lnxded`, 0x80e30d0 in `cod_lnxded`):
/// fixed, whatever the stance. docs/research/coduo-vehicles.md 13.
const BODY_BROAD_MINS: Vec3 = Vec3::new(-64.0, -64.0, -32.0);
const BODY_BROAD_MAXS: Vec3 = Vec3::new(64.0, 64.0, 72.0);

/// The body a segment scores nearer than `limit`, skipping `pass`: slot,
/// fraction along `start..end` and hit location. The broad box only picks
/// the candidates: retail's locational trace then has to score a bone, and
/// a ray can cross the box and meet none (combat doc, section 3). The
/// nearest bone any candidate scores is the answer. With no rig to pose,
/// the link box is the hit.
pub fn trace_bodies(
    start: Vec3,
    end: Vec3,
    pass: usize,
    bodies: &[HitBody],
    limit: f32,
    priority: &PriorityMap,
    bones: Option<&mut BoneTraceCtx>,
) -> Option<(usize, f32, &'static str)> {
    trace_bodies_faced(start, end, pass, bodies, limit, priority, bones)
        .map(|h| (h.slot, h.fraction, h.hitloc))
}

/// A body [`trace_bodies_faced`] met.
#[derive(Clone, Copy, Debug)]
pub struct BodyHit {
    pub slot: usize,
    pub fraction: f32,
    pub hitloc: &'static str,
    /// The struck bone face's world normal (combat doc 3.1); off a link
    /// box, that box's face.
    pub normal: Vec3,
}

/// [`trace_bodies`] with the struck face's normal, which a missile bounces
/// off (combat doc 12.4).
pub fn trace_bodies_faced(
    start: Vec3,
    end: Vec3,
    pass: usize,
    bodies: &[HitBody],
    limit: f32,
    priority: &PriorityMap,
    mut bones: Option<&mut BoneTraceCtx>,
) -> Option<BodyHit> {
    let link_box = |b: &HitBody, t: f32| BodyHit {
        slot: b.slot,
        fraction: t,
        hitloc: "none",
        normal: box_normal(
            start + (end - start) * t,
            b.origin + b.mins,
            b.origin + b.maxs,
        ),
    };
    let Some(ctx) = bones.as_mut() else {
        return bodies
            .iter()
            .filter(|b| b.slot != pass)
            .filter_map(|b| {
                let t = ray_box(start, end, b.origin + b.mins, b.origin + b.maxs)?;
                (t < limit).then(|| link_box(b, t))
            })
            .min_by(|a, b| a.fraction.total_cmp(&b.fraction));
    };
    let mut candidates: Vec<(&HitBody, f32)> = bodies
        .iter()
        .filter(|b| b.slot != pass)
        .filter_map(|b| {
            let t = ray_box(
                start,
                end,
                b.origin + BODY_BROAD_MINS,
                b.origin + BODY_BROAD_MAXS,
            )?;
            (t < limit).then_some((b, t))
        })
        .collect();
    candidates.sort_by(|a, b| a.1.total_cmp(&b.1));
    let mut best: Option<BodyHit> = None;
    let mut limit = limit;
    for (body, box_t) in candidates {
        // Sorted by the box's entry, which no bone inside it comes before.
        if box_t >= limit {
            break;
        }
        let BoneTraceCtx {
            fs,
            anims,
            rigs,
            now_ms,
            posed,
            arith,
        } = &mut **ctx;
        let arith = *arith;
        let Some(skel) = rigs.rig(*fs, &body.pose.assembly) else {
            let Some(t) = ray_box(start, end, body.origin + body.mins, body.origin + body.maxs)
                .filter(|&t| t < limit)
            else {
                continue;
            };
            limit = t;
            best = Some(link_box(body, t));
            continue;
        };
        let stamps = posed.as_deref_mut().and_then(|p| p.get_mut(body.slot));
        let frame = stamps
            .as_deref()
            .map_or_else(|| body.frame(), |s| s.frame(body));
        let axis_stored = stamps
            .as_deref()
            .map_or_else(|| body.axis_stored(), |s| s.axis_stored(body));
        let pose = stamped_pose(&skel, body, anims, rigs, fs, *now_ms, stamps);
        // Into the body's own frame: its `tag_origin` sits at the feet.
        let (ls, le) = if arith == Arith::Stored {
            let axis = axis_stored;
            (
                into_frame_stored(start, &axis, body.origin),
                into_frame_stored(end, &axis, body.origin),
            )
        } else {
            let inv = frame.inverse();
            (inv * (start - body.origin), inv * (end - body.origin))
        };
        // A bone past `limit` lies behind what the trace already met: the
        // box can open in front of a hull or wall the bones stand behind
        // (docs/research/coduo-vehicles.md 13, "The silhouette").
        let Some(hit) = bone_trace_in(&skel, &pose, ls, le, priority, limit, arith)
            .filter(|h| h.fraction < limit)
        else {
            continue;
        };
        let hitloc = HITLOC_NAMES
            .get(hit.hit_location as usize)
            .copied()
            .unwrap_or("none");
        limit = hit.fraction;
        let normal = if arith == Arith::Stored {
            // UO 0x8069154: each component of the turned normal rounded once.
            let [f, l, u] = axis_stored.map(|a| a.as_dvec3());
            let n = hit.normal.as_dvec3();
            (f * n.x + l * n.y + u * n.z).as_vec3()
        } else {
            frame * hit.normal
        };
        best = Some(BodyHit {
            slot: body.slot,
            fraction: hit.fraction,
            hitloc,
            normal,
        });
    }
    best
}

/// `body` posed at `now_ms`, or with `stamps` each bone at the time and
/// with the controls the frame first posed it, those not yet posed stamped
/// `now_ms` under one new build.
fn stamped_pose(
    skel: &vcod_common::skeleton::Skeleton,
    body: &HitBody,
    anims: &PlayerAnims,
    rigs: &mut HitRigs,
    fs: &vcod_common::pk3::Pk3Fs,
    now_ms: i32,
    stamps: Option<&mut DobjStamps>,
) -> vcod_common::skeleton::PoseBuffer {
    let Some(stamps) = stamps else {
        let inputs = body.pose.pose_inputs(anims, now_ms);
        return pose_player(skel, &inputs, |name| rigs.clip(fs, name));
    };
    let all: Vec<usize> = (0..skel.bones().len()).collect();
    let marks = stamps.stamp(all.len(), &all, now_ms, &body.pose);
    let mut kinds: Vec<(i32, usize)> = marks.clone();
    kinds.sort_unstable();
    kinds.dedup();
    let mut pose: Option<vcod_common::skeleton::PoseBuffer> = None;
    for k in kinds {
        let inputs = stamps.inputs(&body.pose, anims, k.0, k.1);
        let then = pose_player(skel, &inputs, |name| rigs.clip(fs, name));
        match pose.as_mut() {
            None => pose = Some(then),
            Some(p) => {
                for (b, _) in marks.iter().enumerate().filter(|(_, m)| **m == k) {
                    p.copy_bone(&then, b);
                }
            }
        }
    }
    pose.expect("a skeleton has bones")
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
    targets: &[ShotTarget],
    world: Option<&CollisionWorld>,
    models: &[PlacedModel],
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
    let pass = Pass::client(attacker, sims);
    let traced = trace_attack(
        muzzle,
        end,
        pass,
        sims,
        targets,
        world,
        models,
        &BULLET_PRIORITY,
        bones,
    );
    // `G_CheckHitTriggerDamage` with `meleeDamage` ahead of the swing's
    // own damage roll (1.1d 0x687a0, combat doc 14.8).
    let sweep = Effect::TriggerSweep(TriggerSweep {
        attacker,
        start: muzzle,
        end: traced.endpos(muzzle, end),
        damage: def.melee_damage,
        mod_: "MOD_MELEE",
    });
    let mut swing = || -> ShotResult {
        let weapon = weapon_index as i32;
        if let Traced::Player {
            slot,
            hitloc,
            point,
            ..
        } = traced
        {
            let damage = def.melee_damage + (vcod_common::rng::xorshift(rng) % 5) as i32;
            let damage = located_damage(damage, hitlocs.multiplier(hitloc));
            return ShotResult {
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
                        quake: None,
                        origin2: None,
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
                struck: Vec::new(),
            };
        }
        // A miss names what the trace met; a sky face or nothing at all
        // spawns no event (0x687a9, 0x687c2; UO 0x8a6ff).
        let miss = |normal: Vec3, surface_flags: u32, other: u32, at: Vec3| {
            Effect::Impact(TempEntity {
                event: EV_MELEE_MISS,
                parm: dir_to_byte(normal.into()),
                surf_type: sound_material(surface_flags),
                other,
                attacker: 0,
                weapon,
                origin: at.into(),
                client_num: 0,
                scale: 0,
                scope: Scope::Broadcast,
                quake: None,
                origin2: None,
            })
        };
        match traced {
            Traced::Player { .. } => unreachable!("returned above"),
            // `G_Damage` on anything with `takedamage`: a UO script model
            // `setTakeDamage` opened (a vehicle's hull takes it too, and
            // `MOD_MELEE` is one no vehicle is hurt by). A 1.1 script model
            // has no `takedamage`.
            Traced::Model {
                id,
                fraction,
                normal,
                surface_flags,
                takes_damage,
            } => {
                let at = muzzle + (end - muzzle) * fraction;
                let mut effects = vec![miss(normal, surface_flags, id.0, at)];
                if takes_damage {
                    effects.push(Effect::EntityHit(EntityHit {
                        target: id,
                        attacker,
                        inflictor: None,
                        damage: def.melee_damage + (vcod_common::rng::xorshift(rng) % 5) as i32,
                        dflags: 0,
                        mod_: "MOD_MELEE",
                        point: at.into(),
                        dir: forward.into(),
                    }));
                }
                ShotResult {
                    effects,
                    struck: Vec::new(),
                }
            }
            // A shootable grenade's box: its 1 health goes as a round's does.
            Traced::Entity {
                id,
                fraction,
                normal,
            } => ShotResult {
                effects: vec![miss(normal, 0, id.0, muzzle + (end - muzzle) * fraction)],
                struck: vec![id],
            },
            Traced::World(t) if t.surface_flags & crate::game::missile::SURF_NOIMPACT == 0 => {
                ShotResult {
                    effects: vec![miss(t.normal, t.surface_flags, ENTITYNUM_WORLD, t.endpos)],
                    struck: Vec::new(),
                }
            }
            Traced::World(_) | Traced::Nothing => ShotResult::default(),
        }
    };
    let mut out = swing();
    out.effects.insert(0, sweep);
    out
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

/// An entity's xmodel collision as a locational trace meets it: a linked
/// `script_model`'s surfaces at its origin and angles (combat doc, 2.7), or
/// a UO vehicle's posed per bone (docs/research/coduo-vehicles.md 13).
pub struct PlacedModel {
    pub id: EntId,
    pub origin: Vec3,
    pub axis: glam::Mat3,
    pub tris: std::sync::Arc<[vcod_common::collision::ModelTri]>,
    /// A posed vehicle's collision surfaces over `tris`, each culled by its
    /// bounds in its bone's frame; empty clips `tris` whole.
    pub surfs: std::sync::Arc<[vcod_common::collision::ModelSurf]>,
    /// `r.ownerNum`, `None` for `ENTITYNUM_NONE`: a driven vehicle's
    /// driver, which a trace's pass rule reads.
    pub owner: Option<u32>,
    /// `takedamage`: a vehicle's, and a UO script model's after
    /// `setTakeDamage(1)`; never a 1.1 script model's (combat doc, 2.7).
    pub takes_damage: bool,
    /// `r.contents` carries `CONTENTS_BODY` (0x2000000): only UO's
    /// `setContents` can give a script model that, and a missile's bounce
    /// off it is soft (combat doc 12.4).
    pub body: bool,
    /// The xmodel descriptor's own bounds, unrotated about `origin`: the
    /// entity clip drops the model when the segment misses them before it
    /// clips a triangle (combat doc 2.7). `None` skips the test.
    pub bounds: Option<(Vec3, Vec3)>,
}

impl PlacedModel {
    /// The segment's first hit on this model closer than `best`: fraction,
    /// world normal and the surface's flags.
    pub fn clip(&self, start: Vec3, end: Vec3, mask: u32, best: f32) -> Option<(f32, Vec3, u32)> {
        if let Some((mins, maxs)) = self.bounds {
            let (lo, hi) = (self.origin + mins, self.origin + maxs);
            if vcod_common::collision::segment_misses_box(start, end, lo, hi, best) {
                return None;
            }
        }
        let local = |p: Vec3| self.axis.transpose() * (p - self.origin);
        let (s, e) = (local(start), local(end));
        match self.surfs.is_empty() {
            true => vcod_common::collision::clip_model_tris(s, e, &self.tris, mask, best),
            false => {
                vcod_common::collision::clip_model_surfs(s, e, &self.tris, &self.surfs, mask, best)
            }
        }
        .map(|(f, n, flags)| (f, self.axis * n, flags))
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

/// A blast candidate with no client: a turret, a `func_static` spawned
/// with spawnflags 2 or 4 (combat doc 14.6, movers doc 21) or a
/// `trigger_damage` (14.8). Retail's door spawn stores `takedamage` too,
/// but vcod runs none.
#[derive(Clone)]
pub struct EntityVictim {
    pub id: EntId,
    /// `r.currentOrigin`.
    pub origin: Vec3,
    pub mins: Vec3,
    pub maxs: Vec3,
    /// A brush model's clip (`r.bmodel`): the distance is to its box rather
    /// than its origin, and `CanDamage`'s traces pass through it, the
    /// entity being their pass entity.
    pub brush: Option<vcod_common::collision::Pusher>,
    /// A UO vehicle (`ent+0x164`, its record) with its last link's
    /// `r.absmin`/`r.absmax`: the distance is to that box drawn in by
    /// three quarters of `mins`/`maxs`, and `CanDamage` probes about its
    /// middle.
    pub vehicle_link: Option<(Vec3, Vec3)>,
}

impl EntityVictim {
    /// What `G_RadiusDamage` hands `G_Damage` for this victim (game.mp.uo
    /// 0x5098b): the direction from the blast to `r.currentOrigin` lifted
    /// 24 units, the blast's own origin as the point.
    pub fn hit(&self, blast: &Blast, inflictor: EntId) -> crate::game::host::EntityHit {
        let d = self.origin - blast.at + Vec3::Z * 24.0;
        crate::game::host::EntityHit {
            inflictor,
            mod_: blast.mod_.to_string(),
            dir: d.into(),
            point: blast.at.into(),
        }
    }

    /// The middle of `r.absmin`/`r.absmax`, whose one-unit widening cancels.
    fn mid(&self) -> Vec3 {
        match self.vehicle_link {
            Some((lo, hi)) => (lo + hi) * 0.5,
            None => self.origin + (self.mins + self.maxs) * 0.5,
        }
    }

    /// `G_RadiusDamage`'s distance (14.1): origin to origin, or for a brush
    /// model the gap to `r.absmin`/`r.absmax` on each axis, 0 inside. A UO
    /// vehicle's box is `r.absmin - 0.75 r.mins` to `r.absmax - 0.75
    /// r.maxs` (`G_RadiusDamage`, game.mp.uo 0x5098b; coduo-vehicles.md 13).
    fn distance(&self, at: Vec3) -> f32 {
        if self.brush.is_none() {
            return (self.origin - at).length();
        }
        let (lo, hi) = match self.vehicle_link {
            Some((lo, hi)) => (lo - self.mins * 0.75, hi - self.maxs * 0.75),
            None => (
                self.origin + self.mins - Vec3::ONE,
                self.origin + self.maxs + Vec3::ONE,
            ),
        };
        (lo - at).max(at - hi).max(Vec3::ZERO).length()
    }
}

/// `CanDamage`'s arm for an entity with no client (combat doc, 14.3): the
/// box midpoint and four points 15 units off it on x and y, all or nothing.
/// The pass entity is the victim: a turret's contents share no bit with the
/// mask and it is no body, and a brush model's own clip is passed through.
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
        world
            .point_trace_except(p, at, mask, true, v.brush)
            .fraction
            >= 1.0
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
        if let Some((lo, hi)) = v.vehicle_link {
            let (slo, shi) = self.search_box();
            return lo.cmple(Vec3::from(shi)).all() && hi.cmpge(Vec3::from(slo)).all();
        }
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
    /// measured origin to origin (a brush model to its box), every live body
    /// stops a probe.
    pub fn entity_damage(
        &self,
        v: &EntityVictim,
        world: Option<&CollisionWorld>,
        models: &[PlacedModel],
        bodies: &[HitBody],
        bones: Option<&mut BoneTraceCtx>,
    ) -> Option<i32> {
        let points = self.points(v.distance(self.at))?;
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
    use crate::game::trigger::{PLAYER_MAXS, PLAYER_MINS};
    use std::collections::HashMap;

    fn ps_on(rules: MoveRules, origin: Vec3) -> PlayerState {
        let mut ps = PlayerState::spawn(origin, 45.0);
        ps.rules = rules;
        ps
    }

    /// The prone frag strike on mp_carentan, read under gdb on 1.5b
    /// (cod11-combat.md 3.1, "The prone strike on mp_carentan"): a body
    /// `setPlayerAngles` turned to (0, 225, 0) traces in `AnglesToAxis`'s
    /// rounding, forward x `0xbf3504f4`, and the hit point rounds `start +
    /// (end - start) * fraction` once, z `0xc12aabc1`.
    #[test]
    fn a_strike_on_a_turned_body_lands_on_retails_bits() {
        let b = |h: u32| f32::from_bits(h);
        let mut stamps = DobjStamps::default();
        stamps.set_angles(Some([0.0, 225.0, 0.0]));
        let body = HitBody {
            slot: 0,
            origin: Vec3::ZERO,
            yaw: b(0x407b53d1),
            mins: Vec3::ZERO,
            maxs: Vec3::ZERO,
            pose: BodyPose::default(),
        };
        let axis = stamps.axis_stored(&body);
        assert_eq!(axis, body.axis_stored());
        assert_eq!(axis[0].x.to_bits(), 0xbf3504f4);
        let start = Vec3::new(b(0x44b2bde3), b(0x4526e093), b(0x409d9b68));
        let end = Vec3::new(b(0x44b69c49), b(0x4528cfc7), b(0xc15acbe7));
        let p = trace_point(start, end, b(0x3f569a06), Arith::Stored);
        assert_eq!(
            p.to_array().map(f32::to_bits),
            [0x44b5fc20, 0x45287fb2, 0xc12aabc1]
        );
    }

    /// 1.1d's end point keeps the difference and the product on the x87
    /// stack too (0x8091352): ours' replay of the turret `-spawn` capture's
    /// wound, whose z an `f32` product rounds a step low.
    #[test]
    fn a_1_1_hit_point_rounds_once() {
        let b = |h: [u32; 3]| Vec3::from_array(h.map(f32::from_bits));
        let (start, end) = (
            b([0x44d54d3a, 0x44e3e2d7, 0x41e5738f]),
            b([0xc55cbd0b, 0xc58bc88a, 0xc30b61ca]),
        );
        let p = trace_point(start, end, f32::from_bits(0x3d0e1d95), Arith::Kept);
        assert_eq!(p.z.to_bits(), 0x41b6cd59);
    }

    /// UO's `BG_Bullet_Endpos` (0x3011c) runs a round 10000 out, the product
    /// unrounded: a turret round's muzzle, forward and the end the engine
    /// traced, read under gdb on retail UO 1.51b (turrets doc 13.8).
    #[test]
    fn a_uo_round_ends_on_retails_bits() {
        let b = |h: [u32; 3]| Vec3::from_array(h.map(f32::from_bits));
        let end = bullet_end(
            b([0x44d544ae, 0x44e3ea34, 0x41e57399]),
            b([0xbf29781e, 0xbf3fcdb6, 0xbca8103c]),
            UO_BULLET_RANGE,
        );
        assert_eq!(
            end.to_array().map(f32::to_bits),
            [0xc5998df5, 0xc5b12810, 0xc330795e]
        );
    }

    /// UO bazooka rounds read under gdb on retail UO 1.51b (combat doc
    /// 13.6): the aim block's angles, the frame `AngleVectors` gave, the
    /// spread and `gunrandom`'s pair, the spread point 16 out after each
    /// store, its normalize, and the `trDelta` `fire_rocket` truncates off
    /// it at 2800. The first is `probe_uofx`'s hip shot along (0, 0), the
    /// other two `probe_skyshot`'s 80 degrees up.
    #[test]
    fn a_uo_rocket_leaves_on_retails_bits() {
        let bits = |v: Vec3| v.to_array().map(f32::to_bits);
        // aim, frame, spread, draw, pre-normalize, dir, delta
        type Row = (
            [u32; 2],
            [[u32; 3]; 3],
            u32,
            [u32; 2],
            [u32; 3],
            [u32; 3],
            [u32; 3],
        );
        let rows: [Row; 3] = [
            (
                [0, 0],
                [
                    [0x3f800000, 0, 0x80000000],
                    [0, 0xbf800000, 0x80000000],
                    [0, 0, 0x3f800000],
                ],
                0x40e00000,
                [0xbea62115, 0xbf0d3c83],
                [0x41800000, 0x3f232f51, 0xbf8abbb1],
                [0x3f7f368f, 0x3d22aee9, 0xbd8a4e86],
                [0x452e7000, 0x42de0000, 0xc33d0000],
            ),
            (
                [0x438c13af, 0x4239034e],
                [
                    [0x3df9a6d8, 0x3e0268d6, 0x3f7bfd95],
                    [0x3f38ef66, 0xbf310455, 0x80000000],
                    [0xbf2e3e98, 0xbf3609e9, 0x3e348598],
                ],
                0x40800000,
                [0x3edcb079, 0xbec36cc8],
                [0x4025b98e, 0x400080a1, 0x417ac938],
                [0x3e259737, 0x3e006601, 0x3f7a9542],
                [0x43e20000, 0x43af8000, 0x452b4000],
            ),
            (
                [0x438c1400, 0x422ff91e],
                [
                    [0x3e01e702, 0x3dfad4e8, 0x3f7bfd16],
                    [0x3f31cf9a, 0xbf382bfe, 0x80000000],
                    [0xbf354935, 0xbf2f0655, 0x3e3490ab],
                ],
                0x40800000,
                [0xbdeb9f54, 0xbed0aff9],
                [0x4010d921, 0x40174be9, 0x417ab3b7],
                [0x3e10c8e2, 0x3e173af0, 0x3f7a9798],
                [0x43c58000, 0x43ce8000, 0x452b4000],
            ),
        ];
        for (aim, frame, spread, draw, pre, dir, delta) in rows {
            let ours = aim_frame(aim.map(f32::from_bits));
            assert_eq!(ours.map(bits), frame, "the frame at {aim:x?}");
            let spread = f32::from_bits(spread);
            let draw = (f32::from_bits(draw[0]), f32::from_bits(draw[1]));
            let d = spread_point(&ours, Vec3::ZERO, spread, ROCKET_REACH, draw, Arith::Stored);
            assert_eq!(bits(d), pre);
            // `fire_rocket` normalizes it once more.
            let d = crate::game::missile::rocket_launch_dir(
                rocket_dir(&ours, spread, draw, Arith::Stored),
                true,
            );
            assert_eq!(bits(d), dir);
            assert_eq!(bits((d * 2800.0).trunc()), delta);
        }
    }

    /// UO kar98k rounds from the hip, read under gdb on retail UO 1.51b
    /// (`probe_uofx`): `BG_Bullet_Endpos`'s stored `k` and end point.
    #[test]
    fn a_uo_rounds_spread_lands_on_retails_bits() {
        let v = |h: [u32; 3]| Vec3::from_array(h.map(f32::from_bits));
        let frame = aim_frame([0.0, 0.0]);
        let muzzle = v([0x44a70000, 0xc3ac0000, 0x43442000]);
        let spread = f32::from_bits(0x40000000);
        for (draw, end) in [
            (
                [0x3e7b9475, 0xbe369fc7],
                [0x46312000, 0xc3d6e5b4, 0x4305d890],
            ),
            (
                [0xbe9a816c, 0x3da3ec80],
                [0x46312000, 0xc36e9ebc, 0x4360136f],
            ),
            (
                [0x3f45c833, 0x3f16b6ac],
                [0x46312000, 0xc41972c0, 0x43c8db2b],
            ),
        ] {
            let draw = (f32::from_bits(draw[0]), f32::from_bits(draw[1]));
            let e = spread_point(&frame, muzzle, spread, UO_BULLET_RANGE, draw, Arith::Stored);
            assert_eq!(e.to_array().map(f32::to_bits), end);
        }
    }

    /// A 1.1d panzerfaust round read under gdb on the retail server
    /// (`probe_panzer`, combat doc 13.6): the spread, `gunrandom`'s pair,
    /// the tangent's float, the frame, the point 16 out kept on the x87
    /// stack, the twice-normalized direction and `trDelta` at 2800.
    #[test]
    fn a_11_rocket_leaves_on_retails_bits() {
        let v = |h: [u32; 3]| Vec3::from_array(h.map(f32::from_bits));
        let bits = |v: Vec3| v.to_array().map(f32::to_bits);
        let spread = f32::from_bits(0x40600000);
        let draw = (f32::from_bits(0xbcf26ff0), f32::from_bits(0xbe91d118));
        let t = (f64::from(spread) * std::f64::consts::PI / 180.0).tan() as f32;
        assert_eq!(t.to_bits(), 0x3d7a85a8);
        let frame = [
            v([0x3f7fff8d, 0xbb6e84eb, 0x3a357443]),
            v([0xbb6e84ef, 0xbf7fff91, 0x80000000]),
            v([0xba3573f4, 0x36291050, 0x3f7ffffc]),
        ];
        let pre = spread_point(&frame, Vec3::ZERO, spread, ROCKET_REACH, draw, Arith::Kept);
        assert_eq!(bits(pre), [0x41800063, 0xbcefcbd2, 0xbe8906ae]);
        let dir = crate::game::missile::rocket_launch_dir(
            rocket_dir(&frame, spread, draw, Arith::Kept),
            false,
        );
        assert_eq!(bits(dir), [0x3f7ff6ba, 0xbaefc269, 0xbc89014e]);
        assert_eq!(
            bits((dir * 2800.0).trunc()),
            [0x452ef000, 0xc0a00000, 0xc2380000]
        );
    }

    /// 1.1d kar98k rounds read under gdb on the retail server
    /// (`--save-combat`, combat doc 2.2): `FireWeapon`'s inline end point,
    /// kept on the x87 stack and rounded once per component, for three
    /// standing hip rounds, a crouched and a prone one, each at a spread
    /// scale of 0.
    #[test]
    fn an_11_rounds_spread_lands_on_retails_bits() {
        let v = |h: [u32; 3]| Vec3::from_array(h.map(f32::from_bits));
        let bits = |v: Vec3| v.to_array().map(f32::to_bits);
        let frame = [
            v([0xb33bbd2e, 0xbf800000, 0x80000000]),
            v([0xbf800000, 0x333bbd2e, 0x80000000]),
            v([0x80000000, 0, 0x3f800000]),
        ];
        // spread, draw, tan, muzzle z, end
        type Row = (u32, [u32; 2], u32, u32, [u32; 3]);
        let rows: [Row; 5] = [
            (
                0x40000000,
                [0xbdd526b5, 0x3f1dd6fc],
                0x3d0f0915,
                0x435c0000,
                [0x440d717d, 0xc5dd4800, 0x43c630b0],
            ),
            (
                0x40000000,
                [0xbf2882b3, 0x3e255b42],
                0x3d0f0915,
                0x435c0000,
                [0x44351378, 0xc5dd4800, 0x438518f6],
            ),
            (
                0x40000000,
                [0xbe8a354e, 0xbe2833f6],
                0x3d0f0915,
                0x435c0000,
                [0x44194e26, 0xc5dd4800, 0x432d0281],
            ),
            (
                0x3fcccccd,
                [0xbc5a4688, 0xbdc73b48],
                0x3ce4d2c3,
                0x43480000,
                [0x4406c315, 0xc5dd4800, 0x4331bd66],
            ),
            (
                0x3fa978d5,
                [0xbf042bd4, 0xbcdcb58c],
                0x3cbd55f3,
                0x432f0000,
                [0x441e7029, 0xc5dd4800, 0x4329e61f],
            ),
        ];
        for (spread, draw, tan, z, end) in rows {
            let spread = f32::from_bits(spread);
            let t = (f64::from(spread) * std::f64::consts::PI / 180.0).tan() as f32;
            assert_eq!(t.to_bits(), tan);
            let muzzle = v([0x44060000, 0x448ae000, z]);
            let draw = (f32::from_bits(draw[0]), f32::from_bits(draw[1]));
            let e = spread_point(&frame, muzzle, spread, BULLET_RANGE, draw, Arith::Kept);
            assert_eq!(bits(e), end);
        }
    }

    /// Each build eases the controls a step on from the last build's, in
    /// its stamp or an earlier one, and a bone the first build posed keeps
    /// the first's (coduo-vehicles.md 13, "The glide back"; movers doc 15).
    #[test]
    fn each_build_eases_the_controls_once_more() {
        use vcod_common::playerpose::Controllers;
        let mut pose = BodyPose {
            rules: MoveRules::Uo,
            ease_ms: 50,
            eased: Some(Controllers::default()),
            ..Default::default()
        };
        pose.input.view = [0.0, 90.0, 0.0];
        pose.angles.torso_yaw = 40.0;
        let target = pose.target_controllers().expect("UO aims its controls");
        let steps = |n: usize| {
            let mut c = Controllers::default();
            for _ in 0..n {
                c.ease_toward(&target, 50);
            }
            c
        };
        assert_ne!(steps(1), steps(2), "the target is more than a step away");

        let mut stamps = DobjStamps::default();
        assert_eq!(stamps.record(), None);
        assert_eq!(stamps.stamp(4, &[0, 1], 50, &pose), [(50, 0), (50, 0)]);
        assert_eq!(stamps.record(), Some(steps(1)));
        assert_eq!(stamps.stamp(4, &[1], 50, &pose), [(50, 0)], "no new build");
        assert_eq!(stamps.record(), Some(steps(1)));
        assert_eq!(stamps.stamp(4, &[1, 2], 50, &pose), [(50, 0), (50, 1)]);
        assert_eq!(stamps.record(), Some(steps(2)));
        // A new stamp eases on from the record, not from the end frame's.
        stamps.clear();
        assert_eq!(stamps.stamp(4, &[3], 100, &pose), [(100, 0)]);
        assert_eq!(stamps.record(), Some(steps(3)));
        // A stamp nothing builds in eases nothing.
        stamps.clear();
        stamps.clear();
        assert_eq!(stamps.record(), Some(steps(3)));
        stamps.reset();
        assert_eq!(stamps.record(), None);
    }

    /// 1.1d truncates the origin and the point; 1.5b and UO take the
    /// unsnapped origin plus the eye (the 1.5b throw capture's 36.1252 over
    /// feet at -23.8748).
    #[test]
    fn the_muzzle_truncates_on_11_only() {
        let o = Vec3::new(1368.034, 2608.136, -23.8748);
        assert_eq!(
            muzzle_point(&ps_on(MoveRules::V1_1, o)),
            Vec3::new(1368.0, 2608.0, 37.0)
        );
        for rules in [MoveRules::V1_5, MoveRules::Uo] {
            assert_eq!(muzzle_point(&ps_on(rules, o)), o + Vec3::Z * 60.0);
        }
    }

    /// 1.5's walk bob rides the muzzle: at bob cycle 0 a standing run of
    /// 200 lifts it `(sin 4pi + 0.2 sin(pi/2 + 8pi)) * 0.75 * 1.4`. On a
    /// turret it does not, and a dip past the feet stops 8 over them.
    #[test]
    fn the_15_muzzle_bobs_and_stays_over_the_feet() {
        let mut ps = ps_on(MoveRules::V1_5, Vec3::ZERO);
        ps.velocity = Vec3::new(200.0, 0.0, 0.0);
        let up = muzzle_point(&ps).z - 60.0;
        assert!((up - 0.21).abs() < 1e-4, "{up}");
        ps.mounted = Some(vcod_common::pmove::Stance::Stand);
        assert_eq!(muzzle_point(&ps), Vec3::Z * 60.0);
        let mut ps = ps_on(MoveRules::V1_5, Vec3::ZERO);
        ps.bob.max = 100.0;
        ps.velocity = Vec3::new(10_000.0, 0.0, 0.0);
        // sin(2c) and sin(pi/2 + 4c) both near -1: 60 - 0.9 * 70 is under 8.
        ps.bob_cycle = 96;
        assert_eq!(muzzle_point(&ps).z, MUZZLE_FLOOR);
    }

    /// UO's muzzle sits 28 over a seated rider's origin, whatever his eye.
    #[test]
    fn a_uo_rider_fires_from_28_up() {
        let mut ps = ps_on(MoveRules::Uo, Vec3::new(0.0, 0.0, 100.0));
        ps.seat = Some(vcod_common::pmove::VehicleSeat {
            kind: 1,
            pos: 3,
            angles: [0.0; 3],
        });
        assert_eq!(muzzle_point(&ps).z, 128.0);
    }

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
            Effect::EntityHit(_) => "entity",
            Effect::TriggerSweep(_) => "sweep",
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
        let Some(fs) = vcod_common::testing::game_fs_11() else {
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
            sim.commit_pose(
                crate::spectate::EndFrame {
                    frametime_ms: 50,
                    command_time: 0,
                    now_ms: 0,
                    client_num: 0,
                },
                vcod_common::playerpose::BG_SWING_SPEED,
                None,
            );
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
            None,
            false,
            &SpreadStance::of(ps, 0, 0),
            [-ps.pitch.to_degrees(), ps.yaw.to_degrees()],
            sims,
            &[],
            Some(world),
            &[],
            &table,
            None,
            rng,
        )
    }

    /// A strike called on a live body lands on the body, not the wall
    /// behind it (`Weapon_ArtilleryStrike_Fire`'s mask 0x2802031 carries
    /// `CONTENTS_BODY`); the caller's own body is the pass entity.
    #[test]
    fn a_strike_on_a_body_stops_at_the_body() {
        let world = vcod_common::collision::test_world(&[(
            Vec3::new(500.0, -64.0, 0.0),
            Vec3::new(516.0, 64.0, 128.0),
        )]);
        let spotter = new_for_test([0.0, 0.0, 0.0], 0.0);
        let target = new_for_test([200.0, 0.0, 0.0], 180.0);
        let muzzle = Vec3::new(0.0, 0.0, 40.0);
        let end = Vec3::new(1000.0, 0.0, 40.0);
        let (at, sky) = strike_point(
            muzzle,
            end,
            0,
            &[(0, &spotter)],
            &[],
            Some(&world),
            &[],
            None,
        )
        .unwrap();
        assert!(!sky);
        assert!((at.x - 500.0).abs() < 1.0, "the wall: {at}");
        let sims = [(0, &spotter), (1, &target)];
        let (at, _) = strike_point(muzzle, end, 0, &sims, &[], Some(&world), &[], None).unwrap();
        assert!((at.x - 185.0).abs() < 1.0, "the body's box: {at}");
    }

    /// A round stops on a shootable grenade's box short of the wall, raises
    /// its impact there with surface 0 and reports it struck; the grenade's
    /// thrower, while its traces still pass it, shoots straight through.
    #[test]
    fn a_round_stops_on_a_satchel_its_shooter_does_not_own() {
        let world = vcod_common::collision::test_world(&[(
            Vec3::new(500.0, -64.0, 0.0),
            Vec3::new(516.0, 64.0, 128.0),
        )]);
        let shooter = new_for_test([0.0, 0.0, 0.0], 0.0);
        let muzzle = crate::game::combat::muzzle_point(&shooter.ps);
        let at = Vec3::new(200.0, 0.0, muzzle.z - 3.0);
        let id = EntId(150, 0);
        let table = HitLocTable::default();
        let shoot = |owner: Option<usize>| {
            let target = ShotTarget {
                id,
                lo: at + Vec3::new(-6.0, -6.0, 0.0),
                hi: at + Vec3::new(6.0, 6.0, 6.0),
                owner,
            };
            bullet_fire_from(
                0,
                Pass {
                    ent: 0,
                    owner: None,
                },
                muzzle,
                Vec3::X,
                BULLET_RANGE,
                45,
                true,
                None,
                "m1carbine_mp",
                &[(0, &shooter)],
                &[target],
                Some(&world),
                &[],
                &table,
                None,
            )
        };
        let r = shoot(Some(1));
        assert_eq!(r.struck, vec![id]);
        let te = r.impacts().next().unwrap();
        assert!((te.origin[0] - 194.0).abs() < 0.01, "{:?}", te.origin);
        assert_eq!(te.surf_type, 0);
        let r = shoot(Some(0));
        assert!(r.struck.is_empty());
        assert!(r.impacts().next().unwrap().origin[0] > 499.0);
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
            ignore_collision: 0,
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
            (0, 0),
            &[],
            &mut 1u64,
        );
        settle(&mut b);
        let mut rigs = HitRigs::default();
        let mut ctx = BoneTraceCtx {
            fs: &fs,
            anims: &anims,
            rigs: &mut rigs,
            arith: vcod_common::patch::Arith::Kept,
            now_ms: 0,
            posed: None,
        };
        let mut rng = 1u64;
        let r = bullet_fire(
            0,
            &zero_spread_carbine(),
            "m1carbine_mp",
            None,
            false,
            &SpreadStance::of(&a.ps, 0, 0),
            a.aim_angles(),
            &[(0, &a), (1, &b)],
            &[],
            Some(&world),
            &[],
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
        b.ps.settle_stance(vcod_common::pmove::Stance::Crouch);
        let idle = vcod_common::net::msg::NULL_USERCMD;
        b.update_anims(&inputs, &idle, (0, 0), &[], &mut 1u64);
        settle(&mut b);
        b.ps.settle_stance(vcod_common::pmove::Stance::Stand);
        b.update_anims(&inputs, &idle, (50, 50), &[], &mut 1u64);
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
                arith: vcod_common::patch::Arith::Kept,
                now_ms: 50,
                posed: None,
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

    /// A hull or wall the trace met between the link box's face and the
    /// bones stops the round: a bone past `limit` scores nothing. Retail's
    /// jeep gunner took no round on the feet the hull's bed hides; ours did
    /// (docs/research/coduo-vehicles.md 13, "The silhouette").
    #[test]
    fn a_bone_behind_what_the_trace_met_scores_nothing() {
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
        let idle = vcod_common::net::msg::NULL_USERCMD;
        b.update_anims(&inputs, &idle, (0, 0), &[], &mut 1u64);
        settle(&mut b);
        let body = b.hit_body(1).expect("a live body");
        let mut rigs = HitRigs::default();
        let mut trace = |limit: f32| {
            let mut ctx = BoneTraceCtx {
                fs: &fs,
                anims: &anims,
                rigs: &mut rigs,
                arith: vcod_common::patch::Arith::Kept,
                now_ms: 50,
                posed: None,
            };
            trace_bodies(
                Vec3::new(-100.0, 0.0, 30.0),
                Vec3::new(100.0, 0.0, 30.0),
                0,
                std::slice::from_ref(&body),
                limit,
                &BULLET_PRIORITY,
                Some(&mut ctx),
            )
            .map(|(_, f, _)| f)
        };
        let bone = trace(1.0).expect("the body is there");
        let face = (100.0 + body.mins.x) / 200.0;
        assert!(bone > face, "the bones lie inside the box: {bone} {face}");
        assert_eq!(trace((bone + face) * 0.5), None);
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

    /// A UO hull between the gun and a player stops the round on itself:
    /// the vehicle takes the hit (the shot's forward, the leg's point) and
    /// the player behind it nothing; a vehicle's own round passes it and
    /// its riders, which it owns.
    #[test]
    fn a_hull_stops_a_round_and_takes_it() {
        use vcod_common::collision::ModelTri;
        let world = vcod_common::collision::test_world(&[]);
        let table = HitLocTable::default();
        let a = new_for_test([0.0, 0.0, 0.0], 0.0);
        let b = new_for_test([200.0, 0.0, 0.0], 180.0);
        // A 200-unit wall at x 100 facing the gun.
        let (p, q, r, t) = (
            Vec3::new(0.0, -100.0, -100.0),
            Vec3::new(0.0, 100.0, -100.0),
            Vec3::new(0.0, 100.0, 100.0),
            Vec3::new(0.0, -100.0, 100.0),
        );
        let face = |tri| ModelTri {
            tri,
            contents: 1,
            surface_flags: 13 << 20,
        };
        let hull = PlacedModel {
            id: EntId(300, 0),
            origin: Vec3::new(100.0, 0.0, 40.0),
            axis: glam::Mat3::IDENTITY,
            tris: std::sync::Arc::from(vec![face([p, r, q]), face([p, t, r])]),
            surfs: Default::default(),
            owner: None,
            takes_damage: true,
            body: false,
            bounds: None,
        };
        let sims = [(0, &a), (1, &b)];
        let fire = |pass: Pass| {
            bullet_fire_from(
                0,
                pass,
                Vec3::new(10.0, 0.0, 40.0),
                Vec3::X,
                BULLET_RANGE,
                50,
                false,
                None,
                "thompson_mp",
                &sims,
                &[],
                Some(&world),
                std::slice::from_ref(&hull),
                &table,
                None,
            )
        };
        let r = fire(Pass {
            ent: 0,
            owner: None,
        });
        assert_eq!(r.hits().count(), 0, "the player behind the hull");
        let hit = r
            .effects
            .iter()
            .find_map(|e| match e {
                Effect::EntityHit(h) => Some(h.clone()),
                _ => None,
            })
            .expect("the hull takes the round");
        assert_eq!(
            (hit.target, hit.damage, hit.mod_),
            (EntId(300, 0), 50, "MOD_PISTOL_BULLET")
        );
        assert_eq!(hit.dir, [1.0, 0.0, 0.0]);
        assert!((hit.point[0] - 100.0).abs() < 0.2, "{:?}", hit.point);
        let impact = only(r.impacts(), "the hull's impact");
        assert_eq!(impact.surf_type, 13);
        let r = fire(Pass {
            ent: 300,
            owner: None,
        });
        assert_eq!(
            r.hits().count(),
            1,
            "the vehicle's own round passes its hull"
        );
    }

    /// A 1.1 script model (no `takedamage`) stops a round on its mesh with
    /// the surface's impact and no damage, and the player behind is
    /// unhurt; a swing that meets it names it (combat doc, 2.4, 2.5, 2.7).
    #[test]
    fn a_script_model_stops_a_round_and_takes_nothing() {
        use vcod_common::collision::ModelTri;
        let world = vcod_common::collision::test_world(&[]);
        let table = HitLocTable::default();
        let a = new_for_test([0.0, 0.0, 0.0], 0.0);
        let b = new_for_test([200.0, 0.0, 0.0], 180.0);
        let (p, q, r, t) = (
            Vec3::new(0.0, -100.0, -100.0),
            Vec3::new(0.0, 100.0, -100.0),
            Vec3::new(0.0, 100.0, 100.0),
            Vec3::new(0.0, -100.0, 100.0),
        );
        let face = |tri| ModelTri {
            tri,
            contents: 1,
            surface_flags: 4 << 20,
        };
        let crate_ = |x: f32| PlacedModel {
            id: EntId(70, 0),
            origin: Vec3::new(x, 0.0, 40.0),
            axis: glam::Mat3::IDENTITY,
            tris: std::sync::Arc::from(vec![face([p, r, q]), face([p, t, r])]),
            surfs: Default::default(),
            owner: None,
            takes_damage: false,
            body: false,
            bounds: None,
        };
        let sims = [(0, &a), (1, &b)];
        let model = crate_(100.0);
        let r = bullet_fire_from(
            0,
            Pass::client(0, &sims),
            Vec3::new(10.0, 0.0, 40.0),
            Vec3::X,
            BULLET_RANGE,
            50,
            true,
            None,
            "kar98k_mp",
            &sims,
            &[],
            Some(&world),
            std::slice::from_ref(&model),
            &table,
            None,
        );
        assert_eq!(r.hits().count(), 0, "the player behind the model");
        assert!(
            !r.effects.iter().any(|e| matches!(e, Effect::EntityHit(_))),
            "no takedamage, no G_Damage"
        );
        let impact = only(r.impacts(), "the model's impact");
        assert_eq!(impact.surf_type, 4);
        assert!(
            (impact.origin[0] - 100.0).abs() < 0.2,
            "{:?}",
            impact.origin
        );

        // A swing whose reach (64 from the eye) meets the model 40 out.
        let model = crate_(40.0);
        let mut rng = 1;
        let r = melee_fire(
            0,
            &melee_carbine(),
            "kar98k_mp",
            1,
            [0.0, 0.0],
            &sims[..1],
            &[],
            Some(&world),
            std::slice::from_ref(&model),
            &table,
            None,
            &mut rng,
        );
        let miss = only(r.impacts(), "the swing's event");
        assert_eq!(
            (miss.event, miss.other, miss.surf_type),
            (EV_MELEE_MISS, 70, 4)
        );
    }

    /// `Damage_Falloff` over the thompson's 1000..5000 at 35%: whole inside,
    /// a straight line between, 35% past, truncated.
    #[test]
    fn a_uo_round_falls_off_with_its_legs_length() {
        let f = Falloff {
            min_percent: 35,
            inner: 1000,
            outer: 5000,
        };
        assert_eq!(f.damage(999.0, 50), 50);
        assert_eq!(f.damage(1000.0, 50), 50);
        // 0.01 * ((100 - 35) * 0.5 + 35) * 50 = 33.75.
        assert_eq!(f.damage(3000.0, 50), 33);
        assert_eq!(f.damage(6000.0, 50), 17);
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
            Pass {
                ent: 0,
                owner: None,
            },
            Vec3::new(40.0, 0.0, 40.0),
            Vec3::X,
            BULLET_RANGE,
            60,
            true,
            None,
            "m1carbine_mp",
            &[(0, &a), (1, &b)],
            &[],
            Some(&world),
            &[],
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
    /// A UO round rolls its tracer on every leg that may draw one
    /// (coduo-diff.md 21): through B into C both player hits draw, each from
    /// its own leg's start, and the miss past B draws none, its pass entity
    /// being B. A miss from the shooter draws one to the round's end.
    #[test]
    fn a_uo_round_rolls_its_tracer_on_each_leg() {
        let a = new_for_test([0.0, 0.0, 0.0], 0.0);
        let b = new_for_test([100.0, 0.0, 0.0], 180.0);
        let c = new_for_test([200.0, 0.0, 0.0], 180.0);
        let open = vcod_common::collision::test_world(&[]);
        let table = HitLocTable::default();
        let round = UoRound {
            weapon: 7,
            tracer_chance: 1.0,
        };
        let mut def = zero_spread_carbine();
        def.damage_inner_radius = 1000;
        def.damage_outer_radius = 5000;
        let fire_uo = |sims: &[(usize, &ClientSim)], rng: &mut u64| {
            let ps = &sims[0].1.ps;
            bullet_fire(
                0,
                &def,
                "m1carbine_mp",
                Some(round),
                false,
                &SpreadStance::of(ps, 0, 0),
                [-ps.pitch.to_degrees(), ps.yaw.to_degrees()],
                sims,
                &[],
                Some(&open),
                &[],
                &table,
                None,
                rng,
            )
        };
        let tracers = |r: &ShotResult| -> Vec<(f32, f32)> {
            r.impacts()
                .filter(|te| te.event == vcod_common::net::event_ids::EV_BULLET_TRACER)
                .map(|te| (te.origin[0], te.origin2.unwrap()[0]))
                .collect()
        };
        let mut rng = 1u64;
        let r = fire_uo(&[(0, &a), (1, &b), (2, &c)], &mut rng);
        let got = tracers(&r);
        assert_eq!(got.len(), 2, "{got:?}");
        assert!((got[0].1 - 85.0).abs() < 0.01, "A to B: {got:?}");
        assert!((got[1].0 - 85.0).abs() < 0.01, "B to C: {got:?}");
        assert!((got[1].1 - 185.0).abs() < 0.01, "B to C: {got:?}");

        let r = fire_uo(&[(0, &a), (1, &b)], &mut rng);
        assert_eq!(tracers(&r).len(), 1, "B's hit, not the miss past him");

        let r = fire_uo(&[(0, &a)], &mut rng);
        let got = tracers(&r);
        assert_eq!(got.len(), 1, "the miss from the shooter");
        assert!(got[0].1 > 1000.0, "{got:?}");
    }

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
        assert_eq!(
            order,
            ["sweep", "hit", "sweep", "hit", "sweep", "impact"],
            "each leg in turn, its trigger walk first"
        );
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
        assert_eq!(
            order,
            ["sweep", "impact", "sweep", "hit", "sweep", "impact"],
            "pane, B, wall"
        );
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
                &[],
                Some(&world),
                &[],
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

        // Retail 1.1d and UO swing at nothing with no event at all
        // (`client-probes/probe_skyshot`'s air part).
        let r = swing(&[(0, &a), (1, &far)], &mut rng);
        assert!(r.hits().next().is_none(), "100 units is out of reach");
        assert!(
            r.impacts().next().is_none(),
            "a swing at the air spawns nothing"
        );
    }

    /// `Weapon_Melee` runs `G_Damage` on whatever it meets with `takedamage`
    /// (2.5): retail UO's `setTakeDamage` barrel in
    /// `client-probes/probe_takedamage` takes `meleeDamage + rand()%5` as
    /// `MOD_MELEE` beside the miss event naming it; one without
    /// `takedamage` takes only the event. A shootable grenade's box is
    /// struck as a round strikes it.
    #[test]
    fn a_swing_damages_what_takes_damage() {
        use vcod_common::collision::ModelTri;
        let world = vcod_common::collision::test_world(&[]);
        let table = HitLocTable::default();
        let a = new_for_test([0.0, 0.0, 0.0], 0.0);
        let def = melee_carbine();
        let muzzle = muzzle_point(&a.ps);
        let (p, q, r, t) = (
            Vec3::new(0.0, -50.0, -50.0),
            Vec3::new(0.0, 50.0, -50.0),
            Vec3::new(0.0, 50.0, 50.0),
            Vec3::new(0.0, -50.0, 50.0),
        );
        let face = |tri| ModelTri {
            tri,
            contents: 0x2080,
            surface_flags: 0,
        };
        let model = |takes_damage| PlacedModel {
            id: EntId(170, 0),
            origin: Vec3::new(30.0, 0.0, muzzle.z),
            axis: glam::Mat3::IDENTITY,
            tris: std::sync::Arc::from(vec![face([p, r, q]), face([p, t, r])]),
            surfs: Default::default(),
            owner: None,
            takes_damage,
            body: false,
            bounds: None,
        };
        let swing = |models: &[PlacedModel], targets: &[ShotTarget]| {
            melee_fire(
                0,
                &def,
                "m1carbine_mp",
                12,
                a.aim_angles(),
                &[(0, &a)],
                targets,
                Some(&world),
                models,
                &table,
                None,
                &mut 1u64,
            )
        };
        let r = swing(&[model(true)], &[]);
        let te = only(r.impacts(), "the miss event");
        assert_eq!((te.event, te.other), (EV_MELEE_MISS, 170));
        let hit = r
            .effects
            .iter()
            .find_map(|e| match e {
                Effect::EntityHit(h) => Some(h.clone()),
                _ => None,
            })
            .expect("G_Damage on the barrel");
        assert_eq!((hit.target, hit.mod_), (EntId(170, 0), "MOD_MELEE"));
        assert!((50..55).contains(&hit.damage), "{}", hit.damage);

        let r = swing(&[model(false)], &[]);
        assert!(
            !r.effects.iter().any(|e| matches!(e, Effect::EntityHit(_))),
            "no takedamage, no damage"
        );

        let satchel = ShotTarget {
            id: EntId(150, 0),
            lo: Vec3::new(30.0, -6.0, muzzle.z - 3.0),
            hi: Vec3::new(42.0, 6.0, muzzle.z + 3.0),
            owner: None,
        };
        let r = swing(&[], &[satchel]);
        assert_eq!(r.struck, vec![EntId(150, 0)]);
        let te = only(r.impacts(), "the miss event");
        assert_eq!((te.event, te.other), (EV_MELEE_MISS, 150));
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
            &[],
            Some(&world),
            &[],
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
            mins: PLAYER_MINS.into(),
            maxs: PLAYER_MAXS.into(),
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
        // built, at the snapped origin: at x 64.9 the midpoint is 69.46 from
        // the blast at the link's x 64 and 70.29 at the feet, so only the
        // link puts it inside the 70 units `radius * 0.2` reaches.
        let edge = blast_victim(3, 64.9);
        let snapped = blast(&edge, &wall);
        assert_eq!(snapped.len(), 1);
        // A tenth of 98.68, the falloff at the feet's own 64.9.
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
            tris: std::sync::Arc::from(vec![face([a, b, c]), face([a, c, d])]),
            surfs: Default::default(),
            owner: None,
            takes_damage: false,
            body: false,
            bounds: None,
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
            mins: PLAYER_MINS.into(),
            maxs: PLAYER_MAXS.into(),
            pose: Default::default(),
        }
    }

    /// A rider's trace frame is the yaw his last cmd wrote (game.mp.uo
    /// 0x41c74), not the seat tag his end frame turned it to: the
    /// controllers read the tag, the frame does not.
    #[test]
    fn a_rider_is_posed_in_his_cmds_yaw() {
        let mut body = box_body(0, 0.0);
        body.yaw = 1.0;
        body.pose.seat = Some(vcod_common::playerpose::BodySeat::new(
            [-30.0, 90.0, 0.0],
            1,
            1,
            0.0,
        ));
        assert!((body.frame() * Vec3::X - Vec3::new(1f32.cos(), 1f32.sin(), 0.0)).length() < 1e-5);
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
        let Some(fs) = vcod_common::testing::game_fs_11() else {
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
                    (0, 0),
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
                mins: PLAYER_MINS.into(),
                maxs: PLAYER_MAXS.into(),
                eye: back_feet + Vec3::Z * 60.0,
            };
            let mut ctx = BoneTraceCtx {
                fs: &fs,
                anims: &anims,
                rigs: &mut rigs,
                arith: vcod_common::patch::Arith::Kept,
                now_ms: 0,
                posed: None,
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
