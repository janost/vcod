//! CoD2's shots, damage, deaths and missiles on a [`Cod2Level`]:
//! `ClientEvents`' fire arms (`FireWeaponAntiLag`, `FireWeaponMelee`,
//! `G_UseOffHand`), `Bullet_Fire_Extended`'s locational trace through the
//! world and the players' posed hit boxes, `G_Damage`'s hit-location
//! multipliers into the script's damage callback, and the grenades and
//! rockets `fire_grenade` and `fire_rocket` put in the air with their
//! `G_RunMissile`, `G_BounceMissile`, `G_ExplodeMissile` and
//! `G_RadiusDamage`. docs/research/cod2-combat.md.
//!
//! The script half (`finishPlayerDamage`, `cloneplayer`) is CoD2's own in
//! `builtins::cod2`; `player_die`'s playerstate half runs here off the
//! `SimOp::Damaged` the builtin queues.

use std::collections::{HashMap, VecDeque};
use vcod_common::asset_source::{AssetSource, RawFile};

use super::cod2_archive::Rewind;
use std::rc::Rc;
use std::sync::Arc;

use glam::Vec3;
use vcod_cod2::collision::{
    CONTENTS_BODY, CONTENTS_GLASS, ClipMap, MASK_BLAST, MASK_SHOT, SURF_NOIMPACT,
};
use vcod_cod2::player::{BodyFrame, Character, PlayerAnimSet, PlayerBody, PlayerRig};
use vcod_cod2::pmove::weapon::WeaponParams;
use vcod_cod2::pmove::{PlayerState, Stance, pm_type};
use vcod_common::animclock::Times;
use vcod_common::bonetrace::{PriorityMap, bone_trace};
use vcod_common::net::events::dir_to_byte;
use vcod_common::pk3::Pk3Fs;
use vcod_common::x87::angle_vectors;
use vcod_gsc::{EntId, Value};

use super::cod2::Cod2Level;
use super::combat::Hit;
use super::host::{GameHost, SimOp};
use super::temp_entity::{Scope, TempEntity};

/// `bulletPriorityMap` (lnxded 0x8187c0c) and `riflePriorityMap`
/// (0x8187c1f), one byte per hit location. VERIFIED, `.data`.
pub const BULLET_PRIORITY: PriorityMap = [1, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 0];
pub const RIFLE_PRIORITY: PriorityMap = [1, 9, 9, 9, 8, 7, 6, 6, 6, 6, 5, 5, 4, 4, 4, 4, 3, 3, 0];

/// The events the server raises for the client's effects (CoD2's table).
mod ev {
    pub const GRENADE_BOUNCE: i32 = 187;
    pub const GRENADE_EXPLODE: i32 = 188;
    pub const ROCKET_EXPLODE: i32 = 189;
    pub const ROCKET_EXPLODE_NOMARKS: i32 = 190;
    pub const CUSTOM_EXPLODE: i32 = 191;
}

/// `ET_GENERAL` and `ET_MISSILE`.
const ET_GENERAL: i32 = 0;
const ET_MISSILE: i32 = 4;
/// `actorLocationalMins` and `actorLocationalMaxs` (lnxded 0x817db88,
/// 0x817db94): the box a locational trace culls a body by.
const ACTOR_LOCATIONAL_MINS: Vec3 = Vec3::new(-64.0, -64.0, -32.0);
const ACTOR_LOCATIONAL_MAXS: Vec3 = Vec3::new(64.0, 64.0, 72.0);
/// `Bullet_Fire`'s range for everything but a spread weapon, 8192
/// (`Bullet_Endpos` handed 0x46000000, lnxded 0x8120484).
const BULLET_RANGE: f32 = 8192.0;
/// `Bullet_Fire_Extended`'s recursion cap (lnxded 0x811fe90: `< 0xd`).
const MAX_RECURSION: i32 = 13;
/// `DFLAG_RADIUS`, `DFLAG_NO_KNOCKBACK`; the bullet's 0x2 and 0x20
/// (`armorPiercing`, `rifleBullet`).
pub const DFLAG_RADIUS: i32 = 1;
pub const DFLAG_NO_KNOCKBACK: i32 = 4;
const DFLAG_ARMOR_PIERCING: i32 = 2;
const DFLAG_PENETRATION: i32 = 0x20;
/// `EVENT_VALID_MSEC`: an exploded missile is freed this long after.
const EVENT_VALID_MS: i32 = 300;
/// `g_clonePlayerMaxVelocity`'s default is not read; vcod settles a corpse
/// where it fell.
/// A grenade's and a rocket's clip mask, 0x2802891 (`fire_grenade`
/// lnxded 0x810e68e, `fire_rocket` 0x810e938): the blast probes' mask with
/// bodies.
const MASK_MISSILE: u32 = MASK_BLAST | CONTENTS_BODY;

/// One trace's contact as `G_LocationalTrace` reports it.
#[derive(Clone, Copy, Debug)]
struct Contact {
    fraction: f32,
    normal: Vec3,
    surface_flags: u32,
    contents: u32,
    /// The client struck, or `None` for the world.
    client: Option<usize>,
    /// The entity with no client struck: a brush model or a script model.
    entity: Option<EntId>,
    hit_location: u8,
}

/// The entity a blast passes over (`G_RadiusDamage`'s `ignore`): a
/// client, or an entity with none.
#[derive(Clone, Copy, Debug, Default)]
pub struct Struck {
    pub client: Option<usize>,
    pub entity: Option<EntId>,
}

/// A grenade or rocket in the air, or its explode event waiting out its
/// 300 ms on the wire.
#[derive(Clone, Debug)]
pub struct Missile {
    pub id: EntId,
    pub owner: Option<usize>,
    pub weapon: u8,
    pub rocket: bool,
    pub tr_type: i32,
    pub tr_time: i32,
    pub base: Vec3,
    pub delta: Vec3,
    pub apos_type: i32,
    pub apos_base: Vec3,
    pub apos_delta: Vec3,
    pub apos_time: i32,
    /// `s.time`: the fire's `level.time` plus 50.
    pub time: i32,
    pub origin: Vec3,
    /// `nextthink`: the fuse.
    pub explode_at: i32,
    pub ground: i32,
    pub event_sequence: i32,
    pub events: [i32; 4],
    pub event_parms: [i32; 4],
    pub surf_type: i32,
    /// When the explode went out; the entity goes 300 ms later.
    pub exploded_at: Option<i32>,
    /// A rocket's ms from `trTime` to its next wobble (`ent+0x1a8`).
    pub destab: f32,
    /// Whether it wobbled yet (`flags` 0x10000).
    pub wobbled: bool,
    /// `flags` 0x20000, the owner's `setStableMissile` at the fire: no
    /// wobble.
    pub stable: bool,
}

impl Missile {
    /// `BG_EvaluateTrajectory` (lnxded 0x80df5d4): the seconds stored as a
    /// float, then `VectorMA` on the stack with one store per component.
    fn evaluate(&self, at: i32) -> Vec3 {
        let t = (at - self.tr_time) as f32 * 0.001;
        let ma = |b: f32, d: f32| (f64::from(d) * f64::from(t) + f64::from(b)) as f32;
        let linear = Vec3::new(
            ma(self.base.x, self.delta.x),
            ma(self.base.y, self.delta.y),
            ma(self.base.z, self.delta.z),
        );
        match self.tr_type {
            vcod_cod2::trajectory::TR_GRAVITY => {
                let mut p = linear;
                let fall = f64::from(t) * 400.0 * f64::from(t);
                p.z = (f64::from(p.z) - fall) as f32;
                p
            }
            vcod_cod2::trajectory::TR_LINEAR => linear,
            _ => self.base,
        }
    }

    fn velocity(&self, at: i32) -> Vec3 {
        match self.tr_type {
            vcod_cod2::trajectory::TR_GRAVITY => {
                let t = (at - self.tr_time) as f32 * 0.001;
                self.delta - Vec3::Z * (800.0 * t)
            }
            vcod_cod2::trajectory::TR_LINEAR => self.delta,
            _ => Vec3::ZERO,
        }
    }

    /// `G_AddEvent` on the missile's own ring.
    fn add_event(&mut self, event: i32, parm: i32) {
        let i = (self.event_sequence & 3) as usize;
        self.events[i] = event;
        self.event_parms[i] = parm & 0xff;
        self.event_sequence += 1;
    }
}

/// The combat state a CoD2 level keeps beside its script host.
#[derive(Default)]
pub struct Combat {
    /// `bg_weaponDefs` by the level's weapon index, 0 empty.
    pub weapons: Vec<Option<WeaponParams>>,
    /// The world's clip, read for its contents and surface flags.
    clip: Option<Arc<ClipMap>>,
    /// The static models' collision (`CM_LoadStaticModels`), which only
    /// the locational trace clips.
    statics: Option<vcod_common::collision::CollisionWorld>,
    /// `g_fHitLocDamageMult`, `info/mp_lochit_dmgtable` (1 by default).
    pub hitloc_mult: [f32; 19],
    anims: Option<PlayerAnimSet>,
    rigs: HashMap<Character, Option<Rc<PlayerRig>>>,
    pub missiles: Vec<Missile>,
    /// The last few hits `G_Damage` handed the damage callback, newest
    /// last: what a test or a log reads back.
    pub hits: VecDeque<Hit>,
    /// The game's two generators: `rand()` (the spread, the gun spring's
    /// kick, the torso events' picks, the melee's extra damage) and
    /// `holdrand` (the rocket's wobble, the grenade's tumble, the script's
    /// `random*`); shared with the host's `Cod2HostState::rand`, which
    /// `player_die`'s death anim and the script's builtins draw from.
    pub rand: std::rc::Rc<GameRand>,
    /// Each client's body as the server animates it.
    bodies: Vec<ServerBody>,
    /// `SV_ResetSkeletonCache`'s stamp (lnxded 0x8091edc): a new one each
    /// client packet and each frame, and a body's skeleton is built once
    /// under each.
    stamp: u32,
}

/// One client's body on the server: its `clientInfo_t` (lnxded
/// `0x87035bc + client * 0x4b8`, zeroed by `ClientConnect`, not by a spawn)
/// and the skeleton last built from it, under which stamp. cod2-combat.md
/// 9.3, "The hit pose".
#[derive(Clone, Default)]
struct ServerBody {
    body: PlayerBody,
    /// What its last end frame read (`ClientEndFrame` copies it into the
    /// `clientInfo_t`), which the controller hook eases toward.
    frame: Option<BodyFrame>,
    built: Option<(u32, Rc<PlayerRig>, Rc<vcod_common::skeleton::PoseBuffer>)>,
}

/// `info/mp_lochit_dmgtable`: `LOCDMGTABLE` then an info string of
/// location names and multipliers (`G_ParseHitLocDmgTable`, Mac
/// 0x1c3c7e). A location the table leaves out reads 1.
pub fn parse_hitloc_table(text: &str) -> Option<[f32; 19]> {
    let body = text.strip_prefix("LOCDMGTABLE")?;
    let mut out = [1.0; 19];
    let mut parts = body.split('\\').skip_while(|s| s.is_empty());
    while let (Some(k), Some(v)) = (parts.next(), parts.next()) {
        if let Some(i) = vcod_cod2::xmodel::HIT_LOCATIONS
            .iter()
            .position(|n| n.eq_ignore_ascii_case(k.trim()))
        {
            out[i] = v.trim().parse().unwrap_or(1.0);
        }
    }
    Some(out)
}

impl Combat {
    /// The level's combat state over the map's clip and the paks.
    pub fn new(clip: Option<Arc<ClipMap>>, fs: Option<&Pk3Fs>) -> Combat {
        Self::with_statics(clip, None, fs)
    }

    /// [`Self::new`] with the static models the bullets clip: the
    /// `misc_model` placements' xmodel collision, as world triangles.
    pub fn with_statics(
        clip: Option<Arc<ClipMap>>,
        statics: Option<vcod_common::collision::CollisionWorld>,
        fs: Option<&Pk3Fs>,
    ) -> Combat {
        let hitloc_mult = fs
            .and_then(|fs| fs.load::<RawFile>("info/mp_lochit_dmgtable").ok())
            .and_then(|b| parse_hitloc_table(&b.text()))
            .unwrap_or([1.0; 19]);
        let anims = fs.and_then(|fs| match PlayerAnimSet::load(fs) {
            Ok(a) => Some(a),
            Err(e) => {
                log::warn!("cod2: no player anims for the hit poses: {e:#}");
                None
            }
        });
        Combat {
            weapons: vec![None],
            clip,
            statics,
            hitloc_mult,
            anims,
            rigs: HashMap::new(),
            missiles: Vec::new(),
            hits: VecDeque::new(),
            rand: Default::default(),
            bodies: vec![ServerBody::default(); crate::server::MAX_CLIENTS],
            stamp: 0,
        }
    }

    /// The player anim tree and script, `None` without paks.
    pub fn player_anims(&self) -> Option<&vcod_common::animtree::PlayerAnims> {
        self.anims.as_ref().map(|a| &a.anims)
    }

    /// The clip a wire anim index plays (toggle ignored), loaded once.
    pub fn anim_clip(
        &mut self,
        fs: &dyn AssetSource,
        wire: i32,
    ) -> Option<Rc<vcod_common::xanim::XAnim>> {
        let set = self.anims.as_mut()?;
        let name = set.anims.name(wire)?.to_string();
        set.clip(fs, &name)
    }

    /// Whether the world lets a shot from `a` to `b`.
    pub fn clear_shot(&self, a: Vec3, b: Vec3) -> bool {
        self.clip.as_ref().is_none_or(|c| {
            c.trace(a, b, Vec3::ZERO, Vec3::ZERO, MASK_SHOT & !CONTENTS_BODY)
                .fraction
                >= 1.0
        })
    }

    /// The floor a point stands over, by a ray 2000 down under the player
    /// mask.
    pub fn floor_under(&self, p: Vec3) -> Option<Vec3> {
        let clip = self.clip.as_ref()?;
        let end = p - Vec3::Z * 2000.0;
        let t = clip.trace(p, end, Vec3::ZERO, Vec3::ZERO, 0x81_0011);
        (t.fraction < 1.0 && !t.startsolid).then(|| p + (end - p) * t.fraction)
    }

    /// A draw in [0, 1), stored as a float.
    fn randomf(&mut self) -> f32 {
        self.g_random() as f32
    }

    /// `G_random` (lnxded 0x80a3774): `rand() / 2^31`, left on the stack.
    pub fn g_random(&self) -> f64 {
        self.rand.g_random()
    }

    /// `G_PlayerEvent` (lnxded 0x80f7b38) over a cmd's new events, from
    /// `g_smoothClients`' entity copy after `Pmove`: each fire event (158,
    /// 159, 160, 175) kicks the gun spring (`BG_WeaponFireRecoil`), four
    /// draws.
    pub fn fire_recoil(
        &self,
        aim: &mut vcod_cod2::pmove::aim::AimState,
        ps: &PlayerState,
        ws: &vcod_cod2::pmove::weapon::WeaponState,
        events: impl Iterator<Item = i32>,
    ) {
        for e in events {
            if matches!(e, 158..=160 | 175) {
                aim.fire_recoil(ws, ps, &self.weapons, &mut || self.rand.g_random());
            }
        }
    }

    /// Pins the game's `rand()` to glibc's seeded `srand(seed)`, `skip`
    /// draws in, leaving `holdrand` as the level's draws left it: a test
    /// replaying a capture seeds both at the load ([`GameRand::reseed`],
    /// retail's `G_InitGame` seeds both off one wall-clock read,
    /// cod2-combat.md 9.6, 9.7), then pins glibc's where its replay starts.
    pub fn pin_rand(&mut self, seed: u32, skip: usize) {
        *self.rand.g.borrow_mut() = vcod_common::rng::GlibcRand::new(seed);
        for _ in 0..skip {
            self.rand.rand();
        }
    }

    /// Each weapon the level registered since the last call, read from its
    /// file with the table's ammo, clip and alt indices.
    pub fn sync_weapons(&mut self, host: &GameHost) {
        let Some(fs) = host.fs.as_deref() else {
            return;
        };
        while self.weapons.len() <= host.cod2().weapons.len() {
            let index = self.weapons.len();
            let name = &host.cod2().weapons[index - 1];
            let params = match vcod_cod2::weapon::load(fs, name) {
                Ok(p) => {
                    let table = host.weapons.get(index);
                    Some(WeaponParams::from_def(
                        &p.def,
                        table.map_or(0, |d| d.ammo_index),
                        table.map_or(0, |d| d.clip_index),
                        table.map_or(0, |d| d.alt_index as u8),
                    ))
                }
                Err(e) => {
                    log::warn!("cod2: weapon {name}: {e:#}");
                    None
                }
            };
            self.weapons.push(params);
        }
    }

    /// `ClientConnect`'s memset of the client's body.
    pub fn reset_body(&mut self, slot: usize) {
        self.bodies[slot] = ServerBody::default();
    }

    /// `SV_ResetSkeletonCache`: every body's next trace builds its
    /// skeleton anew.
    pub fn new_stamp(&mut self) {
        self.stamp = self.stamp.wrapping_add(1);
    }

    /// The rig of the character `c` names, loaded once.
    fn rig(&mut self, fs: &Pk3Fs, c: &Character) -> Option<Rc<PlayerRig>> {
        if let Some(r) = self.rigs.get(c) {
            return r.clone();
        }
        let rig = match PlayerRig::assemble(fs, c) {
            Ok(r) => Some(Rc::new(r)),
            Err(e) => {
                log::warn!("cod2: no hit rig for {}: {e:#}", c.body);
                None
            }
        };
        self.rigs.insert(c.clone(), rig.clone());
        rig
    }
}

/// The game module's two generators: glibc's `rand()`, as `cod2_lnxded`
/// links it, and q_shared's `holdrand`, which `G_InitGame` seeds each level
/// with one seed ([`GameRand::reseed`], cod2-combat.md 2.6.1, 9.6).
/// Unseeded they are glibc's `srand(1)` and `holdrand`'s static 0x89abcdef
/// (lnxded .data 0x817e338). Behind cells so a draw can sit beside a borrow
/// of the weapon table.
#[derive(Debug)]
pub struct GameRand {
    g: std::cell::RefCell<vcod_common::rng::GlibcRand>,
    drawn: std::cell::Cell<u64>,
    hold: std::cell::Cell<vcod_common::rng::HoldRand>,
}

impl Default for GameRand {
    fn default() -> Self {
        GameRand {
            g: std::cell::RefCell::new(vcod_common::rng::GlibcRand::new(1)),
            drawn: std::cell::Cell::new(0),
            hold: std::cell::Cell::new(vcod_common::rng::HoldRand(0x89ab_cdef)),
        }
    }
}

impl GameRand {
    /// `G_InitGame`'s `srand(seed)` then `Rand_Init(seed)` (lnxded
    /// 0x810911e, 0x8109129).
    pub fn reseed(&self, seed: u32) {
        *self.g.borrow_mut() = vcod_common::rng::GlibcRand::new(seed);
        self.hold.set(vcod_common::rng::HoldRand(seed));
    }

    /// `Rand_Init(seed)` alone: `holdrand`, glibc's stream left as it is.
    pub fn rand_init(&self, seed: u32) {
        self.hold.set(vcod_common::rng::HoldRand(seed));
    }

    /// `flrand(min, max)` off `holdrand`.
    pub fn flrand(&self, min: f32, max: f32) -> f32 {
        let mut h = self.hold.get();
        let v = h.flrand(min, max);
        self.hold.set(h);
        v
    }

    /// `irand(min, max)` off `holdrand`.
    pub fn irand(&self, min: i32, max: i32) -> i32 {
        let mut h = self.hold.get();
        let v = h.irand(min, max);
        self.hold.set(h);
        v
    }

    /// How many draws the stream has made, reseeds included: what a gate
    /// counts against a retail trace.
    pub fn drawn(&self) -> u64 {
        self.drawn.get()
    }

    /// `rand()`: 31 bits.
    pub fn rand(&self) -> u32 {
        self.drawn.set(self.drawn.get() + 1);
        self.g.borrow_mut().next_u31()
    }

    /// `G_random`: `rand() / 2^31` on the stack.
    pub fn g_random(&self) -> f64 {
        f64::from(self.rand()) / 2_147_483_648.0
    }
}

/// The idle clip a body at `stance` is posed in for the hit boxes when no
/// legs anim rides its playerstate. INFERRED stand-in.
fn idle_anim(stance: Stance) -> &'static str {
    match stance {
        Stance::Stand => "pb_stand_alert",
        Stance::Crouch => "pb_crouch_alert",
        Stance::Prone => "pb_prone_aim",
    }
}

/// `ByteToDir`'s inverse over a direction.
fn dir_byte(v: Vec3) -> i32 {
    dir_to_byte(v.to_array())
}

/// `Melee_Trace`'s `traceOffsets` (lnxded 0x815ed20): the centre, then the
/// four corners, as (right, up) shares of the width and height.
const MELEE_OFFSETS: [(f32, f32); 5] = [
    (0.0, 0.0),
    (1.0, 1.0),
    (1.0, -1.0),
    (-1.0, 1.0),
    (-1.0, -1.0),
];

/// `VectorMA`: `a + b * s` on the stack, one store per component.
fn vector_ma(a: Vec3, s: f32, b: Vec3) -> Vec3 {
    let d = f64::from;
    Vec3::new(
        (d(s) * d(b.x) + d(a.x)) as f32,
        (d(s) * d(b.y) + d(a.y)) as f32,
        (d(s) * d(b.z) + d(a.z)) as f32,
    )
}

/// `Vec3Lerp`: `(b - a) * f + a` on the stack, one store per component.
fn vec3_lerp(a: Vec3, b: Vec3, f: f32) -> Vec3 {
    let l = |a: f32, b: f32| ((f64::from(b) - f64::from(a)) * f64::from(f) + f64::from(a)) as f32;
    Vec3::new(l(a.x, b.x), l(a.y, b.y), l(a.z, b.z))
}

impl Cod2Level {
    /// The eye raised `viewHeightCurrent` over the origin and leaned
    /// (`AddLeanToPosition` with 16 and 20), without the walk bob
    /// [`Cod2Level::player_view_origin`] adds.
    pub fn view_origin(ps: &PlayerState) -> Vec3 {
        let eye = ps.origin + Vec3::Z * ps.view_height_current;
        Self::leaned(ps, eye)
    }

    fn leaned(ps: &PlayerState, eye: Vec3) -> Vec3 {
        vcod_common::pmove::aim::add_lean(
            eye,
            vcod_common::x87::radians(ps.view_angles[1]),
            ps.leanf,
            vcod_common::patch::Arith::Stored,
        )
    }

    /// `G_GetPlayerViewOrigin` (lnxded 0x80f8916): the eye with the walk
    /// bob (`vcod_cod2::pmove::aim::bobbed_eye`), leaned, held at least 8
    /// over the origin.
    pub fn player_view_origin(ps: &PlayerState, level_time: i32) -> Vec3 {
        let mut eye = Self::leaned(ps, vcod_cod2::pmove::aim::bobbed_eye(ps, level_time));
        let floor = ps.origin.z + 8.0;
        if eye.z < floor {
            eye.z = floor;
        }
        eye
    }

    /// The `[forward, right, up]` and muzzle `FireWeaponAntiLag` and
    /// `G_UseOffHand` build (`G_CalcMuzzlePoints`, lnxded 0x81207ec): the
    /// aim block's angles of the client's last cmd, the view's roll.
    fn weapon_parms(&self, slot: usize) -> Option<([Vec3; 3], Vec3)> {
        let p = self.players.get(slot)?.as_ref()?;
        let [pitch, yaw] = p.fire_angles;
        let axes = angle_vectors(Vec3::new(pitch, yaw, p.ps.view_angles[2]));
        Some((axes, Self::player_view_origin(&p.ps, self.level_time())))
    }

    /// `G_AntiLagRewindClientPos`' lookup (lnxded 0x811fc76) for `slot` at
    /// `time`: the archive's record, or with an age walked to 0 the live
    /// playerstate's origin while it carries `pm_flags` 0x800000
    /// (`GetFollowPlayerState`).
    pub(super) fn rewound_origin(&self, slot: usize, time: i32) -> Option<Rewind> {
        use super::cod2_archive::{ClientInfo, position_at_time, ps_origin};
        let (on, now) = (self.rt.archive_on(), self.level_time());
        position_at_time(now, time, crate::cod2::FRAME_MS, |age| {
            match self.archive.client_info(on, slot, age, now) {
                ClientInfo::Archived(ps, _) => Some(ps_origin(&ps)),
                ClientInfo::Live => self.players.get(slot)?.as_ref().and_then(|p| {
                    (p.ps.pm_flags & super::cod2::PMF_PLAYING != 0).then_some(p.ps.origin)
                }),
                ClientInfo::None => None,
            }
        })
    }

    /// The level's clients that take a shot: playing, not `skip`.
    fn targets(&self, skip: Option<usize>) -> Vec<usize> {
        (0..self.players.len())
            .filter(|&s| Some(s) != skip && self.playing(s))
            .collect()
    }

    /// The character `slot`'s entity carries: its model and attachments.
    fn character(&mut self, slot: usize) -> Option<Character> {
        let strip = |s: &str| s.strip_prefix("xmodel/").unwrap_or(s).to_string();
        let body = strip(&self.rt.client_field(slot, "model")?);
        if body.is_empty() {
            return None;
        }
        let id = self.rt.client_entity(slot)?;
        let (attachments, ignore_collision) = self.rt.with_host(|h, cx| {
            h.ents.get(id).map_or_else(
                || (Vec::new(), 0),
                |e| {
                    let list = e
                        .attachments
                        .iter()
                        .map(|(m, t)| {
                            let tag = cx.resolve(*t).to_string();
                            (strip(cx.resolve(*m)), (!tag.is_empty()).then_some(tag))
                        })
                        .collect();
                    (list, u32::from(e.attach_ignore_collision))
                },
            )
        });
        Some(Character {
            body,
            attachments,
            ignore_collision,
        })
    }

    /// The frame `slot`'s body poses from at `origin`: its playerstate's
    /// anims, stance, view and lean.
    fn body_frame(&self, slot: usize, origin: Vec3) -> Option<BodyFrame> {
        let ps = &self.players.get(slot)?.as_ref()?.ps;
        let set = self.combat.anims.as_ref()?;
        // The anims pmove picked; the stance's idle where none rode the
        // playerstate (no paks for the script).
        let legs = match ps.legs_anim {
            0 => set
                .anims
                .wire_of(idle_anim(ps.effective_stance()))
                .unwrap_or(0),
            l => l,
        };
        // The entity's words as `BG_PlayerStateToEntityState` (0x80dfcc2)
        // writes them: `eFlags` with 0x20000 dead and 0x40000 sighted,
        // `angles2[1]` the move's direction, the prone pitches.
        let mut eflags = ps.e_flags as i32;
        if ps.pm_type >= pm_type::DEAD {
            eflags |= vcod_cod2::player::eflags::DEAD;
        }
        if ps.pm_flags & 0x40 != 0 {
            eflags |= vcod_cod2::player::eflags::ADS;
        }
        let prone = ps.effective_stance() == Stance::Prone;
        let pitches = if prone { ps.prone_valid } else { [0.0; 3] };
        Some(BodyFrame {
            origin,
            // The swings and the controls read the playerstate's view
            // (`ClientEndFrame` copies ps+0xe8 to ci+0x3e8), not the
            // entity's snapped angles.
            angles: [ps.view_angles[0], ps.view_angles[1], 0.0],
            legs_anim: legs,
            torso_anim: ps.torso_anim,
            eflags,
            movement_dir: ps.movement_dir as f32,
            lean: ps.leanf,
            torso_height: pitches[0],
            torso_pitch: pitches[1],
            waist_pitch: pitches[2],
            client_num: slot as i32,
        })
    }

    /// `BG_PlayerAnimation` as `ClientEndFrame` runs it (lnxded 0x80dbdbc,
    /// from 0x80f7516): `slot`'s swings and lerp frames stepped a frame.
    pub(super) fn animate_body(&mut self, slot: usize, frametime_ms: i32) {
        let now = self.level_time();
        let Some(p) = self.players.get(slot).and_then(Option::as_ref) else {
            return;
        };
        // The lerp frames measure the rate off the entity's `pos.trBase`
        // (0x80da42a reads es+0x18), which the end frame's
        // `BG_PlayerStateToEntityState` (0x80dfcc2) truncated to whole
        // units (`fistp` under control word | 0xc00).
        let Some(f) = self.body_frame(slot, p.ps.origin.map(f32::trunc)) else {
            return;
        };
        let Some(set) = self.combat.anims.as_ref() else {
            return;
        };
        let at = Times {
            clock_ms: now,
            snapshot_ms: now,
        };
        self.combat.bodies[slot].frame = Some(f);
        self.combat.bodies[slot].body.animate(
            &set.anims,
            &f,
            at,
            frametime_ms,
            vcod_common::playerpose::BG_SWING_SPEED,
        );
    }

    /// `slot`'s body posed at `origin`: its rig, its pose and its frame.
    /// The skeleton is built once a stamp (`G_DObjCalcPose`, lnxded
    /// 0x811e3c0): the build runs the controller hook (`G_PlayerController`,
    /// 0x80f729a), which eases the controls one `level.frametime` on, and
    /// samples each clip at its node time.
    fn body_pose(
        &mut self,
        slot: usize,
        origin: Vec3,
    ) -> Option<(
        Rc<PlayerRig>,
        Rc<vcod_common::skeleton::PoseBuffer>,
        BodyFrame,
    )> {
        let f = self.body_frame(slot, origin)?;
        self.body_pose_at(slot, f)
    }

    /// [`Self::body_pose`] with the controls eased toward `f`.
    fn body_pose_at(
        &mut self,
        slot: usize,
        f: BodyFrame,
    ) -> Option<(
        Rc<PlayerRig>,
        Rc<vcod_common::skeleton::PoseBuffer>,
        BodyFrame,
    )> {
        let fs = self.rt.host.fs.clone()?;
        let character = self.character(slot)?;
        let rig = self.combat.rig(&fs, &character)?;
        let now = self.level_time();
        let stamp = self.combat.stamp;
        let frametime = self.frametime_ms();
        let Combat { anims, bodies, .. } = &mut self.combat;
        let set = anims.as_mut()?;
        let b = &mut bodies[slot];
        if let Some((s, r, pose)) = &b.built
            && *s == stamp
            && Rc::ptr_eq(r, &rig)
        {
            return Some((rig, pose.clone(), f));
        }
        b.body.ease_controls(&set.anims, &f, frametime);
        let pose = Rc::new(b.body.pose_in(&rig, set, &*fs, &f, now, true, None));
        b.built = Some((stamp, rig.clone(), pose.clone()));
        Some((rig, pose, f))
    }

    /// `G_DObjGetWorldTagMatrix`'s origin (lnxded 0x811e542) for `slot`'s
    /// `tag`: the tag's posed place ([`Self::body_pose`]) turned by
    /// `AnglesToAxis(r.currentAngles)`, (0, the view's yaw, 0), and moved
    /// to `origin` (`MatrixTransformVector43`, each component on the
    /// stack). `None` without the paks or the tag.
    fn world_tag(&mut self, slot: usize, origin: Vec3, tag: &str) -> Option<Vec3> {
        // The controller hook reads the `clientInfo_t` its last end frame
        // filled, not the playerstate the death since rewrote.
        let (rig, pose, _) = match self.combat.bodies[slot].frame {
            Some(f) => self.body_pose_at(slot, BodyFrame { origin, ..f })?,
            None => self.body_pose(slot, origin)?,
        };
        let bone = rig.skeleton.bone_index(tag)?;
        let local = pose.bone_world(&rig.skeleton, bone).0.as_dvec3();
        let yaw = self.players[slot].as_ref()?.ps.view_angles[1];
        let axis = vcod_common::x87::angles_to_axis(Vec3::new(0.0, yaw, 0.0));
        let [x, y, z] = axis.map(|a| a.as_dvec3());
        Some((x * local.x + y * local.y + z * local.z + origin.as_dvec3()).as_vec3())
    }

    /// `Drop_Weapon`'s start from a tag (lnxded 0x81064de) for every weapon
    /// a script dropped since the last call (`game::cod2_items::TagDrop`):
    /// a capsule of the item's box swept from the dropper's box's centre to
    /// the tag under 0x491 (the item has no clipmask yet), the item placed
    /// where it stops and its `pos.trTime` the level's. A body with no
    /// tag keeps the launch's start. VERIFIED, the calls and stores.
    pub fn place_drops(&mut self) {
        let drops = std::mem::take(&mut self.rt.host.cod2_mut().tag_drops);
        for d in drops {
            let Some(body) = self.rt.host.cod2().clients[d.slot].body else {
                continue;
            };
            let Some(at) = self.world_tag(d.slot, body.origin, &d.tag) else {
                continue;
            };
            // `0.5 * (mins + maxs)` stored, then the origin added.
            let half = ((body.mins.as_dvec3() + body.maxs.as_dvec3()) * 0.5).as_vec3();
            let centre = (half.as_dvec3() + body.origin.as_dvec3()).as_vec3();
            let tr = self.world.collision.item_trace(
                centre,
                at,
                Vec3::splat(-1.0),
                Vec3::splat(1.0),
                super::item::RUN_CLIPMASK,
            );
            // `Vec3Lerp(centre, tag, fraction)`.
            let c = centre.as_dvec3();
            let start = (c + (at.as_dvec3() - c) * f64::from(tr.fraction)).as_vec3();
            let now = self.level_time();
            self.rt.with_host(|host, cx| {
                let origin = cx.intern_folded("origin");
                let _ = vcod_gsc::Host::set_field(
                    host,
                    cx,
                    d.item,
                    origin,
                    Value::Vector(start.into()),
                );
                if let Some(pos) = host
                    .ents
                    .get_mut(d.item)
                    .and_then(|e| e.item.as_mut())
                    .and_then(|i| i.pos.as_mut())
                {
                    pos.base = start;
                    pos.tr_time = now;
                }
            });
        }
    }

    /// `slot`'s body traced from `start` to `end` under `priority`, posed
    /// at `origin` ([`Self::body_pose`]); the fraction is clipped to
    /// `limit` (cod11-combat.md 3.1's walk).
    fn body_trace(
        &mut self,
        slot: usize,
        origin: Vec3,
        start: Vec3,
        end: Vec3,
        priority: &PriorityMap,
        limit: f32,
    ) -> Option<vcod_common::bonetrace::BoneHit> {
        let (rig, pose, _) = self.body_pose(slot, origin)?;
        // The DObj sits at `r.currentAngles`, the unsnapped view's yaw.
        let yaw = self.players[slot].as_ref()?.ps.view_angles[1];
        let placed = BodyFrame {
            origin,
            angles: [0.0, yaw, 0.0],
            ..Default::default()
        }
        .transform();
        // Into the model's frame: the transform's inverse.
        let inv = placed.inverse();
        let (ls, le) = (inv.transform_point3(start), inv.transform_point3(end));
        let mut hit = bone_trace(&rig.skeleton, &pose, ls, le, priority, limit)?;
        hit.normal = placed.transform_vector3(hit.normal);
        Some(hit)
    }

    /// Where `names` sit in the world on a skeleton built at `now_ms` from
    /// `slot`'s body as it stands, its controls eased once, as a link's
    /// `G_DObjGetWorldTagPos` in the entity pass builds it. What
    /// `probe_combat.gsc`'s `probe_tags` lines log.
    #[cfg(test)]
    pub(super) fn tag_positions(&mut self, slot: usize, now_ms: i32, names: &[&str]) -> Vec<Vec3> {
        let fs = self.rt.host.fs.clone().unwrap();
        let character = self.character(slot).unwrap();
        let ps = &self.players[slot].as_ref().unwrap().ps;
        let (origin, yaw) = (ps.origin, ps.view_angles[1]);
        let f = self.body_frame(slot, origin).unwrap();
        let rig = self.combat.rig(&fs, &character).unwrap();
        let frametime = self.frametime_ms();
        let Combat { anims, bodies, .. } = &mut self.combat;
        let set = anims.as_mut().unwrap();
        let body = &mut bodies[slot].body;
        body.ease_controls(&set.anims, &f, frametime);
        let pose = body.pose_in(&rig, set, &*fs, &f, now_ms, true, None);
        let placed = BodyFrame {
            origin,
            angles: [0.0, yaw, 0.0],
            ..Default::default()
        }
        .transform();
        let worlds = pose.bone_worlds(&rig.skeleton);
        names
            .iter()
            .map(|n| {
                let i = rig
                    .skeleton
                    .bones()
                    .iter()
                    .position(|b| b.name == *n)
                    .unwrap();
                placed.transform_point3(worlds[i].0)
            })
            .collect()
    }

    /// `slot`'s bones as [`Self::body_pose`] poses them, in the model's
    /// frame, beside the bind pose's.
    #[cfg(test)]
    pub(super) fn body_bones(&mut self, slot: usize) -> Vec<(String, Vec3, Vec3)> {
        let origin = self.players[slot].as_ref().unwrap().ps.origin;
        let Some((rig, pose, _)) = self.body_pose(slot, origin) else {
            return Vec::new();
        };
        let bind = vcod_common::skeleton::PoseBuffer::new(&rig.skeleton);
        let (w, b) = (
            pose.bone_worlds(&rig.skeleton),
            bind.bone_worlds(&rig.skeleton),
        );
        rig.skeleton
            .bones()
            .iter()
            .enumerate()
            .map(|(i, bone)| (bone.name.clone(), w[i].0, b[i].0))
            .collect()
    }

    /// The entity with no client a segment meets before `limit`: a linked
    /// brush model (`SV_Trace`'s entity clip) or a script model's xmodel
    /// collision, through the host's `bulletTrace` pass; `skip`'s own
    /// entity is passed.
    fn entity_contact(
        &mut self,
        start: Vec3,
        end: Vec3,
        skip: Option<usize>,
        limit: f32,
    ) -> Option<Contact> {
        let ignore = skip.and_then(|s| self.rt.client_entity(s));
        // The brush models under CoD2's shot mask (the shared `shot_trace`
        // holds 1.1's), then the script models.
        let brush = self
            .world
            .collision
            .point_trace(start, end, MASK_SHOT & !CONTENTS_BODY, false);
        let n = self.world.collision.entity_num(&brush);
        let brush = (brush.fraction < 1.0 && n < vcod_common::net::protocol::ENTITYNUM_WORLD)
            .then(|| self.rt.host.ents.handle(n))
            .flatten()
            .map(|id| (brush.fraction, brush.normal, Some(id), brush.surface_flags));
        let (model, _) = self.rt.with_host(|h, cx| {
            super::builtins::combat::bullet_hit(h, cx, start, end, false, ignore)
        });
        let model = model.filter(|m| m.2.is_some());
        let hit = match (brush, model) {
            (Some(b), Some(m)) if m.0 < b.0 => m,
            (Some(b), _) => b,
            (None, Some(m)) => m,
            (None, None) => return None,
        };
        let (fraction, normal, Some(id), surface_flags) = hit else {
            return None;
        };
        (fraction < limit).then_some(Contact {
            fraction,
            normal,
            surface_flags,
            contents: 1,
            client: None,
            entity: Some(id),
            hit_location: 0,
        })
    }

    /// `G_LocationalTrace` from `start` to `end` under `mask`, passing
    /// `skip`'s body: the world's clip, then every other playing body's
    /// hit boxes, each walked only where its box meets the segment.
    /// `origins` overrides where a client stands (the antilag's rewind).
    fn locational_trace(
        &mut self,
        start: Vec3,
        end: Vec3,
        skip: Option<usize>,
        mask: u32,
        priority: &PriorityMap,
        origins: &HashMap<usize, Rewind>,
    ) -> Contact {
        let mut best = match &self.combat.clip {
            Some(clip) => {
                let t = clip.trace(start, end, Vec3::ZERO, Vec3::ZERO, mask & !CONTENTS_BODY);
                Contact {
                    fraction: t.fraction,
                    normal: t.normal,
                    surface_flags: t.surface_flags,
                    contents: t.contents,
                    client: None,
                    entity: None,
                    hit_location: 0,
                }
            }
            None => Contact {
                fraction: 1.0,
                normal: Vec3::ZERO,
                surface_flags: 0,
                contents: 0,
                client: None,
                entity: None,
                hit_location: 0,
            },
        };
        // `SV_Trace` with `locational` clips the static models after the
        // world, before the entities (cod2-d3dbsp-format.md, "The trace";
        // cod2-combat.md 9.3).
        if let Some(statics) = &self.combat.statics {
            let t = statics.point_trace(start, end, mask & !CONTENTS_BODY, true);
            if t.fraction < best.fraction {
                best = Contact {
                    fraction: t.fraction,
                    normal: t.normal,
                    surface_flags: t.surface_flags,
                    contents: vcod_cod2::collision::CONTENTS_SOLID,
                    client: None,
                    entity: None,
                    hit_location: 0,
                };
            }
        }
        if let Some(e) = self.entity_contact(start, end, skip, best.fraction) {
            best = e;
        }
        if mask & CONTENTS_BODY == 0 {
            return best;
        }
        for slot in self.targets(skip) {
            let ps = &self.players[slot].as_ref().expect("a target plays").ps;
            let origin = match origins.get(&slot) {
                Some(Rewind::At(o)) => *o,
                // Retail's NaN body: no round meets it (2.3.3).
                Some(Rewind::Nowhere) => continue,
                None => ps.origin,
            };
            // A trace under a priority map culls a body by the actor box
            // about `r.currentOrigin`, not its bounds, and builds the
            // skeleton past the cull (cod2-combat.md 9.5).
            let (lo, hi) = (
                origin + ACTOR_LOCATIONAL_MINS,
                origin + ACTOR_LOCATIONAL_MAXS,
            );
            if !segment_meets_box(start, end, lo, hi, best.fraction) {
                continue;
            }
            if let Some(h) = self.body_trace(slot, origin, start, end, priority, best.fraction)
                && h.fraction < best.fraction
            {
                best = Contact {
                    fraction: h.fraction,
                    normal: h.normal,
                    // Flesh (7), as `finishPlayerDamage` names it.
                    surface_flags: 7 << 20,
                    contents: CONTENTS_BODY,
                    client: Some(slot),
                    entity: None,
                    hit_location: h.hit_location,
                };
            }
        }
        best
    }

    /// A round's `G_LocationalTrace` from `start` to `end` under the
    /// pistol-bullet priority map, no body passed, the clients in
    /// `origins` standing there: the fraction, the client struck and its
    /// hit location. `cod2_hitpose_ab` and `cod2_antilag_ab` replay
    /// retail's rounds through it.
    #[cfg(test)]
    pub(super) fn round_trace(
        &mut self,
        start: Vec3,
        end: Vec3,
        origins: &HashMap<usize, Vec3>,
    ) -> (f32, Option<usize>, u8) {
        let origins = origins.iter().map(|(&s, &o)| (s, Rewind::At(o))).collect();
        let c = self.locational_trace(start, end, None, MASK_SHOT, &BULLET_PRIORITY, &origins);
        (c.fraction, c.client, c.hit_location)
    }

    /// `ClientEvents` (Mac 0x1a30ec) for the events `slot`'s cmd raised: a
    /// fire event shoots (`FireWeaponAntiLag` at the client's lag time
    /// under `g_antilag`, else `level.time`), the melee strikes, the
    /// offhand throws, a grenade cooked to its end kills its holder, and a
    /// hard landing hurts.
    pub fn client_events(&mut self, slot: usize, events: &[(i32, i32)]) {
        use vcod_cod2::pmove::weapon::ev as wev;
        for &(event, parm) in events {
            match event {
                wev::FIRE_WEAPON | 159 | wev::FIRE_WEAPON_LASTSHOT | 175 => {
                    let antilag = self.rt.host.cvars.get("g_antilag") != "0";
                    let lag = if antilag {
                        self.rt.host.cod2().clients[slot].lag_time
                    } else {
                        self.level_time()
                    };
                    self.fire_weapon(slot, lag);
                }
                wev::FIRE_MELEE => self.fire_melee(slot),
                wev::USE_OFFHAND => self.use_offhand(slot),
                wev::GRENADE_SUICIDE => self.grenade_suicide(slot),
                116..=138 => self.fall_damage(slot, parm),
                _ => {}
            }
        }
    }

    /// The landing events' `G_Damage` (`MOD_FALLING`, `ClientEvents`):
    /// the parm is the share of max health, 100 and up all of it.
    fn fall_damage(&mut self, slot: usize, parm: i32) {
        let max = self.rt.client_vitals(slot).max_health;
        let scale = if parm > 99 { 1.0 } else { parm as f32 * 0.01 };
        if scale == 0.0 {
            return;
        }
        let damage = (max as f32 * scale) as i32;
        self.rt.host.cod2_mut().callback.time_offset = 0;
        self.rt.deliver_fall(slot, damage);
        self.apply_damage_ops();
    }

    /// `FireWeaponAntiLag` (lnxded 0x8120870) at `lag_time`.
    pub fn fire_weapon(&mut self, slot: usize, lag_time: i32) {
        let Some(p) = self.players.get(slot).and_then(Option::as_ref) else {
            return;
        };
        if p.ps.e_flags & 0x300 != 0 {
            return;
        }
        let (weapon, ps) = (p.ws.weapon, p.ps.clone());
        let pos_frac = p.ws.pos_frac;
        let spread_scale = p.spread_scale;
        let Some(w) = self
            .combat
            .weapons
            .get(usize::from(weapon))
            .cloned()
            .flatten()
        else {
            return;
        };
        let Some((axes, muzzle)) = self.weapon_parms(slot) else {
            return;
        };
        let spread = vcod_cod2::pmove::weapon::shot_spread(
            ps.view_height_current,
            pos_frac,
            spread_scale * 255.0,
            &w,
        );
        match w.weap_type {
            0 => self.bullet_fire(
                slot,
                slot as u32,
                &w,
                weapon,
                axes,
                muzzle,
                spread,
                lag_time,
            ),
            1 => self.grenade_fire(slot, weapon, axes, muzzle),
            2 => self.rocket_fire(slot, &w, weapon, axes, muzzle, spread),
            _ => {}
        }
    }

    /// `Bullet_Fire` (lnxded 0x8120484): every playing client, the shooter
    /// too, rewound to the lag time (`G_AntiLagRewindClientPos`, when it is
    /// more than a frame back; only `setarchive` gives it a past to read),
    /// then one round, or `shotCount` pellets out to
    /// `minDamageRange` for a spread weapon. `w` is the round's weapon,
    /// `weapon` the one `G_Damage` reads off the shooter's entity: a
    /// gunner's is the weapon in his hands (cod2-turrets.md 11). `owner`
    /// is the entity a world impact names (`otherEntityNum`): the shooter,
    /// or a gunner's turret.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn bullet_fire(
        &mut self,
        slot: usize,
        owner: u32,
        w: &WeaponParams,
        weapon: u8,
        axes: [Vec3; 3],
        muzzle: Vec3,
        spread: f32,
        lag_time: i32,
    ) {
        let now = self.level_time();
        let mut origins = HashMap::new();
        if self.rt.host.cvars.get("g_antilag") != "0" && now - lag_time > crate::cod2::FRAME_MS {
            for s in self.targets(None) {
                if let Some(at) = self.rewound_origin(s, lag_time) {
                    origins.insert(s, at);
                }
            }
        }
        let saved = self.antilag_move(&origins);
        let time_offset = now - lag_time;
        let (pellets, range) = if w.weap_class == 3 {
            (w.shot_count.max(0), w.min_damage_range)
        } else {
            (1, BULLET_RANGE)
        };
        for _ in 0..pellets {
            let end = self.bullet_endpos(spread, axes, muzzle, range);
            self.bullet_extended(
                slot,
                owner,
                Some(slot),
                w,
                weapon,
                axes[0],
                muzzle,
                end,
                1.0,
                0,
                time_offset,
                &origins,
            );
        }
        self.antilag_restore(saved);
    }

    /// `G_AntiLagRewindClientPos`' move (lnxded 0x811fc76): each rewound
    /// client's `r.currentOrigin` (the blast body's origin, the script's
    /// `origin`) set to its answer and relinked, `ps.origin` left alone.
    /// What the shot's callbacks read until [`Self::antilag_restore`]
    /// (cod2-combat.md 2.3.2). Returns each one's saved pair.
    fn antilag_move(
        &mut self,
        origins: &HashMap<usize, Rewind>,
    ) -> Vec<(usize, Option<Vec3>, [f32; 3])> {
        let mut saved = Vec::new();
        for (&s, &r) in origins {
            let Rewind::At(at) = r else {
                continue;
            };
            let state = &mut self.rt.host.cod2_mut().clients[s];
            let body = state.body.map(|b| b.origin);
            if let Some(b) = state.body.as_mut() {
                b.origin = at;
            }
            let field = self.rt.client_origin(s);
            self.rt.set_client_origin(s, at.to_array());
            saved.push((s, body, field));
        }
        saved
    }

    /// `G_AntiLag_RestoreClientPos` (lnxded 0x811fdec): the saved origins
    /// written back over whatever the callbacks left, while `g_antilag`
    /// still reads true.
    fn antilag_restore(&mut self, saved: Vec<(usize, Option<Vec3>, [f32; 3])>) {
        if self.rt.host.cvars.get("g_antilag") == "0" {
            return;
        }
        for (s, body, field) in saved {
            if let (Some(b), Some(o)) = (self.rt.host.cod2_mut().clients[s].body.as_mut(), body) {
                b.origin = o;
            }
            self.rt.set_client_origin(s, field);
        }
    }

    /// `Bullet_Endpos` (lnxded 0x811fac8): `range` along the aim, pushed
    /// off it by up to `tan(spread) * range` at a random angle. Two draws:
    /// the angle `360 * G_random()`, then the fraction (0x811fa70); every
    /// product stored, each `VectorMA` component stored.
    fn bullet_endpos(&mut self, spread: f32, axes: [Vec3; 3], muzzle: Vec3, range: f32) -> Vec3 {
        let d = f64::from;
        let rad = std::f64::consts::PI / 180.0;
        let tan = (d(spread) * rad).tan() as f32;
        let k = (d(tan) * d(range)) as f32;
        let angle = (d(360.0f32) * self.combat.g_random()) as f32;
        let frac = self.combat.g_random() as f32;
        let a = (d(angle) * rad) as f32;
        let (s, c) = (d(a).sin() as f32, d(a).cos() as f32);
        let x = (d((d(frac) * d(c)) as f32) * d(k)) as f32;
        let y = (d((d(frac) * d(s)) as f32) * d(k)) as f32;
        let ma = |a: Vec3, s: f32, b: Vec3| {
            Vec3::new(
                (d(s) * d(b.x) + d(a.x)) as f32,
                (d(s) * d(b.y) + d(a.y)) as f32,
                (d(s) * d(b.z) + d(a.z)) as f32,
            )
        };
        let end = ma(muzzle, range, axes[0]);
        let end = ma(end, x, axes[1]);
        ma(end, y, axes[2])
    }

    /// `Bullet_Fire_Extended` (lnxded 0x811fe90): the trace, the world's
    /// impact event, glass passed a quarter unit on, and a struck client's
    /// `G_Damage` at the range's damage times `scale`; a rifle round goes
    /// on through a client not on the shooter's side at half.
    #[allow(clippy::too_many_arguments)]
    fn bullet_extended(
        &mut self,
        attacker: usize,
        owner: u32,
        skip: Option<usize>,
        w: &WeaponParams,
        weapon: u8,
        forward: Vec3,
        start: Vec3,
        end: Vec3,
        scale: f32,
        depth: i32,
        time_offset: i32,
        origins: &HashMap<usize, Rewind>,
    ) {
        if depth >= MAX_RECURSION {
            log::debug!("Bullet_Fire_Extended: Too many resursions, bullet aborted");
            return;
        }
        let mut dflags = 0;
        let (mod_, priority) = if w.rifle_bullet {
            dflags |= DFLAG_PENETRATION;
            ("MOD_RIFLE_BULLET", &RIFLE_PRIORITY)
        } else {
            ("MOD_PISTOL_BULLET", &BULLET_PRIORITY)
        };
        if w.armor_piercing {
            dflags |= DFLAG_ARMOR_PIERCING;
        }
        let tr = self.locational_trace(start, end, skip, MASK_SHOT, priority, origins);
        let hit = start + (end - start) * tr.fraction;
        // `G_CheckHitTriggerDamage` along the leg, the weapon's `damage` and
        // its means (Mac 0x1c867e).
        self.rt.trigger_sweep(&super::combat::TriggerSweep {
            attacker,
            start,
            end: hit,
            damage: w.damage,
            mod_,
        });
        let dir = (end - start).normalize_or_zero();
        let reflect = dir + tr.normal * (-2.0 * dir.dot(tr.normal));
        if tr.surface_flags & SURF_NOIMPACT == 0 && tr.client.is_none() && tr.fraction < 1.0 {
            self.temp_entity(TempEntity {
                event: if w.rifle_bullet {
                    vcod_common::net::event_ids::EV_BULLET_HIT_LARGE
                } else {
                    vcod_common::net::event_ids::EV_BULLET_HIT_SMALL
                },
                parm: dir_byte(tr.normal),
                surf_type: ((tr.surface_flags & 0x1f0_0000) >> 20) as i32,
                other: owner,
                attacker: 0,
                weapon: 0,
                client_num: 0,
                scale: dir_byte(reflect),
                origin: hit.to_array(),
                scope: Scope::Pvs,
                quake: None,
                origin2: None,
            });
        }
        if tr.contents & CONTENTS_GLASS != 0 {
            let d = -dir.dot(tr.normal);
            let off = if d >= 0.125 { 0.25 / d } else { 0.0 };
            let next = hit + dir * off;
            self.bullet_extended(
                attacker,
                owner,
                skip,
                w,
                weapon,
                forward,
                next,
                end,
                scale,
                depth + 1,
                time_offset,
                origins,
            );
            return;
        }
        // The range's damage times the leg's scale on the stack, truncated
        // (lnxded 0x81202ba).
        let damage = (f64::from(w.damage_at(super::cod2_blast::stored_length(start - hit)))
            * f64::from(scale)) as i32;
        if let Some(id) = tr.entity {
            self.entity_damage(id, attacker, None, damage, dflags, mod_, hit, forward);
            return;
        }
        let Some(victim) = tr.client.filter(|&v| v != attacker) else {
            return;
        };
        self.g_damage(
            victim,
            attacker,
            None,
            weapon,
            hit,
            Some(forward),
            damage,
            dflags,
            mod_,
            tr.hit_location,
            time_offset,
        );
        let ff = self.rt.host.cvars.get("scr_friendlyfire");
        let friendly = !matches!(ff, "" | "0") || !self.same_team(victim, attacker);
        if dflags & DFLAG_PENETRATION != 0 && friendly {
            self.bullet_extended(
                attacker,
                owner,
                Some(victim),
                w,
                weapon,
                forward,
                hit,
                end,
                scale * 0.5,
                depth + 1,
                time_offset,
                origins,
            );
        }
    }

    /// `OnSameTeam`: both on one team under a team gametype (a free-for-all
    /// reads every client's `sessionteam` as `none`).
    /// `Player_UpdateLookAtEntity` (Mac 0x1cd1d4), from `ClientEndFrame`:
    /// `pm_flags` 0x300000 cleared, then a 15000-unit locational trace
    /// along the view (mask 0x22802801, the rifle priority map for a
    /// `rifleBullet` weapon) that meets a playing body sets 0x100000 for a
    /// teammate inside `g_friendlyfireDist` and 0x200000 for anyone else
    /// inside the held weapon's `enemyCrosshairRange` (cod2-combat.md 9.2).
    /// The smoke test (`SV_FX_GetVisibility` at least 0.2) is not done.
    pub(crate) fn look_at_flags(&mut self, slot: usize) {
        const FRIENDLY: u32 = 0x10_0000;
        const ENEMY: u32 = 0x20_0000;
        let Some(p) = self.players.get_mut(slot).and_then(Option::as_mut) else {
            return;
        };
        p.ps.pm_flags &= !(FRIENDLY | ENEMY);
        let (ps, weapon) = (p.ps.clone(), p.ws.weapon);
        let w = self
            .combat
            .weapons
            .get(usize::from(weapon))
            .cloned()
            .flatten();
        let priority = match &w {
            Some(w) if weapon != 0 && w.rifle_bullet => &RIFLE_PRIORITY,
            _ => &BULLET_PRIORITY,
        };
        let eye = Self::player_view_origin(&ps, self.level_time());
        let forward = angle_vectors(Vec3::from_array(ps.view_angles))[0];
        let end = eye + forward * 15000.0;
        let tr =
            self.locational_trace(eye, end, Some(slot), 0x2280_2801, priority, &HashMap::new());
        let Some(other) = tr.client else {
            return;
        };
        let Some(o) = self.players.get(other).and_then(Option::as_ref) else {
            return;
        };
        let d = o.ps.origin - eye;
        let dist2 = d.dot(d);
        let flag = if self.same_team(slot, other) {
            let r = match self.rt.host.cvars.get("g_friendlyfireDist") {
                "" => 256.0,
                v => crate::game::builtins::cvar::atof(v),
            };
            (dist2 < r * r).then_some(FRIENDLY)
        } else {
            let r = w.map_or(0.0, |w| w.enemy_crosshair_range);
            (dist2 < r * r).then_some(ENEMY)
        };
        if let (Some(f), Some(p)) = (flag, self.players[slot].as_mut()) {
            p.ps.pm_flags |= f;
        }
    }

    fn same_team(&mut self, a: usize, b: usize) -> bool {
        let (ta, tb) = (self.rt.client_team(a), self.rt.client_team(b));
        ta == tb
            && matches!(
                ta,
                crate::game::script::TEAM_AXIS | crate::game::script::TEAM_ALLIES
            )
    }

    /// `G_Damage` on a client (Mac 0x1c4272): the inflictor's weapon's
    /// `loc*` multiplier for a bullet weapon, else the level's hit-location
    /// table, truncated; a result above 0 goes to the damage callback with
    /// `timeOffset`.
    #[allow(clippy::too_many_arguments)]
    pub fn g_damage(
        &mut self,
        victim: usize,
        attacker: usize,
        inflictor: Option<EntId>,
        weapon: u8,
        point: Vec3,
        dir: Option<Vec3>,
        damage: i32,
        dflags: i32,
        mod_: &'static str,
        hit_location: u8,
        time_offset: i32,
    ) {
        if !self.playing(victim) || !self.rt.client_vitals(victim).takedamage {
            return;
        }
        // `G_Damage` passes over a client in `noclip` or `ufo`.
        let cheat = self.rt.host.cod2().clients[victim];
        if cheat.noclip || cheat.ufo {
            return;
        }
        let loc = usize::from(hit_location).min(18);
        let mult = match self
            .combat
            .weapons
            .get(usize::from(weapon))
            .cloned()
            .flatten()
        {
            Some(w) if w.is_bullet() => w.loc_multipliers[loc],
            _ => self.combat.hitloc_mult[loc],
        };
        let damage = (mult * damage as f32) as i32;
        if damage <= 0 {
            return;
        }
        let name = self
            .rt
            .host
            .weapon_name(usize::from(weapon))
            .unwrap_or_else(|| "none".into());
        self.rt.host.cod2_mut().callback.time_offset = time_offset;
        let hit = Hit {
            victim,
            attacker,
            inflictor,
            damage,
            dflags,
            mod_,
            weapon: name,
            point: point.to_array(),
            dir: dir.unwrap_or(Vec3::ZERO).to_array(),
            hitloc: vcod_cod2::xmodel::HIT_LOCATIONS[loc],
        };
        if self.combat.hits.len() == 32 {
            self.combat.hits.pop_front();
        }
        self.combat.hits.push_back(hit.clone());
        self.rt.deliver_hits(vec![hit]);
        // A later kill outside this path (`suicide`) reads 0s.
        self.rt.host.cod2_mut().callback = Default::default();
        self.apply_damage_ops();
    }

    /// `FireWeaponMelee` (lnxded 0x8120a70): `Weapon_Melee` (0x811f844)
    /// from `G_GetPlayerViewOrigin` along the view's own axes
    /// (`G_GetPlayerViewDirection`, `ps.viewangles`), unless the client is
    /// on a turret. `Melee_Trace` (0x811f6b0) tries up to five traces of
    /// `player_meleeRange` under the shot mask, the centre then the
    /// corners `player_meleeWidth` right and `player_meleeHeight` up off
    /// it, and keeps the first that meets something without
    /// `SURF_NOIMPACT`; the hit triggers take the first trace's sweep. A
    /// contact raises `EV_MELEE_HIT` on a client, `EV_MELEE_MISS` on
    /// anything else, and a struck target that takes damage takes
    /// `meleeDamage` plus `rand() % 5` (cod2-combat.md 2.7).
    fn fire_melee(&mut self, slot: usize) {
        let Some(p) = self.players.get(slot).and_then(Option::as_ref) else {
            return;
        };
        // Retail tests `ent+0x162` beside the turret bits; vcod's gunner
        // never has it clear. INFERRED.
        if p.ps.e_flags & 0x300 != 0 {
            return;
        }
        let (weapon, ps) = (p.ws.weapon, p.ps.clone());
        let damage = self
            .combat
            .weapons
            .get(usize::from(weapon))
            .and_then(Option::as_ref)
            .map_or(0, |w| w.melee_damage);
        let eye = Self::player_view_origin(&ps, self.level_time());
        let axes = angle_vectors(Vec3::from_array(ps.view_angles));
        let dvar = |name: &str, default: f32| match self.rt.host.cvars.get(name) {
            "" => default,
            v => crate::game::builtins::cvar::atof(v),
        };
        let range = dvar("player_meleeRange", 64.0);
        let width = dvar("player_meleeWidth", 10.0);
        let height = dvar("player_meleeHeight", 10.0);
        let traces = if width > 0.0 || height > 0.0 { 5 } else { 1 };
        let mut contact = None;
        for (k, &(right, up)) in MELEE_OFFSETS.iter().take(traces).enumerate() {
            let end = vector_ma(eye, range, axes[0]);
            let end = vector_ma(end, width * right, axes[1]);
            let end = vector_ma(end, height * up, axes[2]);
            let tr = self.locational_trace(
                eye,
                end,
                Some(slot),
                MASK_SHOT,
                &BULLET_PRIORITY,
                &HashMap::new(),
            );
            let hit = vec3_lerp(eye, end, tr.fraction);
            if k == 0 {
                self.rt.trigger_sweep(&super::combat::TriggerSweep {
                    attacker: slot,
                    start: eye,
                    end: hit,
                    damage,
                    mod_: "MOD_MELEE",
                });
            }
            if tr.surface_flags & SURF_NOIMPACT == 0 && tr.fraction != 1.0 {
                contact = Some((tr, hit));
                break;
            }
        }
        let Some((tr, hit)) = contact else {
            return;
        };
        use vcod_common::net::event_ids::{EV_MELEE_HIT, EV_MELEE_MISS};
        let (event, other) = match (tr.client, tr.entity) {
            (Some(c), _) => (EV_MELEE_HIT, c as u32),
            (None, Some(id)) => (EV_MELEE_MISS, id.0),
            (None, None) => (EV_MELEE_MISS, vcod_common::net::protocol::ENTITYNUM_WORLD),
        };
        self.temp_entity(TempEntity {
            event,
            parm: dir_byte(tr.normal),
            surf_type: 0,
            other,
            attacker: 0,
            weapon: i32::from(weapon),
            client_num: 0,
            scale: 0,
            origin: hit.to_array(),
            scope: Scope::Pvs,
            quake: None,
            origin2: None,
        });
        if let Some(victim) = tr.client {
            let damage = (self.combat.rand.rand() % 5) as i32 + damage;
            self.g_damage(
                victim,
                slot,
                None,
                weapon,
                hit,
                Some(axes[0]),
                damage,
                0,
                "MOD_MELEE",
                tr.hit_location,
                0,
            );
        } else if let Some(id) = tr.entity
            && self.rt.host.entity_takes_damage(id)
        {
            let damage = (self.combat.rand.rand() % 5) as i32 + damage;
            self.entity_damage(id, slot, None, damage, 0, "MOD_MELEE", hit, axes[0]);
        }
    }

    /// `G_UseOffHand` (Mac 0x1c8192): the offhand grenade thrown.
    fn use_offhand(&mut self, slot: usize) {
        let Some(p) = self.players.get(slot).and_then(Option::as_ref) else {
            return;
        };
        let offhand = p.ws.offhand;
        let Some((axes, muzzle)) = self.weapon_parms(slot) else {
            return;
        };
        self.grenade_fire(slot, offhand, axes, muzzle);
    }

    /// `weapon_grenadelauncher_fire` (lnxded 0x8120552): `projectileSpeed`
    /// along the aim plus `projectileSpeedUp` up, `fire_grenade` on the
    /// fuse left in hand, and the thrower's velocity along the throw added.
    fn grenade_fire(&mut self, slot: usize, weapon: u8, axes: [Vec3; 3], muzzle: Vec3) {
        let Some(w) = self
            .combat
            .weapons
            .get(usize::from(weapon))
            .cloned()
            .flatten()
        else {
            return;
        };
        let mut dir = axes[0] * w.projectile_speed as f32;
        dir.z += w.projectile_speed_up as f32;
        let (fuse, vel) = match self.players[slot].as_mut() {
            Some(p) => {
                let left = std::mem::take(&mut p.ws.grenade_time_left);
                (left, p.ps.velocity)
            }
            None => (0, Vec3::ZERO),
        };
        let fuse = if fuse != 0 { fuse } else { w.fuse_time };
        let Some(i) = self.fire_grenade(Some(slot), weapon, muzzle, dir, fuse) else {
            return;
        };
        let n = dir.normalize_or_zero();
        let m = &mut self.combat.missiles[i];
        m.delta += n * vel.dot(n);
    }

    /// `fire_grenade` (lnxded 0x810e68e): a `TR_GRAVITY` missile from
    /// `origin` at the truncated `velocity`, tumbling (`apos` `TR_LINEAR` at
    /// 720 and 360 degrees a second, each plus `flrand(-45, 45)` off
    /// `holdrand`), its fuse `fuse_ms` out. Returns its index in the
    /// level's list.
    pub fn fire_grenade(
        &mut self,
        owner: Option<usize>,
        weapon: u8,
        origin: Vec3,
        velocity: Vec3,
        fuse_ms: i32,
    ) -> Option<usize> {
        let now = self.level_time();
        let id = self.spawn_missile_entity()?;
        let delta = Vec3::new(velocity.x.trunc(), velocity.y.trunc(), velocity.z.trunc());
        let angles = super::builtins::math::vectoangles(velocity.to_array());
        let pitch = (angles[0] - 120.0).rem_euclid(360.0);
        let tumble = |about: f32| {
            (f64::from(about) + f64::from(self.combat.rand.flrand(-45.0, 45.0))) as f32
        };
        let spin = Vec3::new(tumble(720.0), 0.0, tumble(360.0));
        self.combat.missiles.push(Missile {
            id,
            owner,
            weapon,
            rocket: false,
            tr_type: vcod_cod2::trajectory::TR_GRAVITY,
            tr_time: now,
            base: origin,
            delta,
            apos_type: vcod_cod2::trajectory::TR_LINEAR,
            apos_base: Vec3::new(pitch, angles[1], angles[2]),
            apos_delta: spin,
            apos_time: now,
            time: now + 50,
            origin,
            explode_at: now + fuse_ms,
            ground: 1023,
            event_sequence: 0,
            events: [0; 4],
            event_parms: [0; 4],
            surf_type: 0,
            exploded_at: None,
            destab: 0.0,
            wobbled: false,
            stable: false,
        });
        Some(self.combat.missiles.len() - 1)
    }

    /// `Weapon_RocketLauncher_Fire` (lnxded 0x812061a): the aim pushed off
    /// by the spread at 16 units, `fire_rocket` along it, and the shooter
    /// knocked back 64.
    fn rocket_fire(
        &mut self,
        slot: usize,
        w: &WeaponParams,
        weapon: u8,
        axes: [Vec3; 3],
        muzzle: Vec3,
        spread: f32,
    ) {
        let r = 16.0 * (f64::from(spread).to_radians().tan() as f32);
        let angle = (self.combat.randomf() * 360.0).to_radians();
        let frac = self.combat.randomf();
        let dir = (axes[0] * 16.0
            + axes[1] * (angle.cos() * frac * r)
            + axes[2] * (angle.sin() * frac * r))
            .normalize_or_zero();
        self.fire_rocket(slot, w, weapon, muzzle, dir);
        if let Some(p) = self.players[slot].as_mut() {
            p.ps.velocity -= axes[0] * 64.0;
        }
    }

    /// `fire_rocket` (lnxded 0x810e938): a `TR_LINEAR` missile from `start`
    /// along `dir` normalized, at `projectileSpeed` truncated to whole
    /// units, its clock 50 ms back (`trTime` `level.time - 50`), `eFlags`
    /// 0x400, `apos` stationary at the flight's angles (`G_SetAngle`), the
    /// first wobble `destabilizeDistance / projectileSpeed` seconds out and
    /// the explode 30 s out.
    fn fire_rocket(&mut self, slot: usize, w: &WeaponParams, weapon: u8, start: Vec3, dir: Vec3) {
        let dir = vcod_common::x87::normalize_stored(dir);
        let now = self.level_time();
        let Some(id) = self.spawn_missile_entity() else {
            return;
        };
        let speed = w.projectile_speed as f32;
        let delta = (dir * speed).trunc();
        let destab = (1000.0 * (f64::from(w.destabilize_distance) / f64::from(speed))) as f32;
        let angles = super::builtins::math::vectoangles(dir.to_array());
        let stable = self.rt.host.cod2().clients[slot].flags
            & crate::game::cod2_cmds::FL_STABLE_MISSILE
            != 0;
        self.combat.missiles.push(Missile {
            id,
            owner: Some(slot),
            weapon,
            rocket: true,
            tr_type: vcod_cod2::trajectory::TR_LINEAR,
            tr_time: now - 50,
            base: start,
            delta,
            apos_type: vcod_cod2::trajectory::TR_STATIONARY,
            apos_base: Vec3::from(angles),
            apos_delta: Vec3::ZERO,
            apos_time: 0,
            time: now + 50,
            origin: start,
            explode_at: now + 30_000,
            ground: 1023,
            event_sequence: 0,
            events: [0; 4],
            event_parms: [0; 4],
            surf_type: 0,
            exploded_at: None,
            destab,
            wobbled: false,
            stable,
        });
    }

    /// `G_RunMissile`'s wobble (lnxded 0x810de4e) for a rocket in clear
    /// air once `destab` ms have passed since `trTime`: its direction
    /// pushed by `tan(destabilizationAngleMax)` times three
    /// `flrand(-1, 1)` off `holdrand`, renormalized, flown on at
    /// `projectileSpeed` from where it is now. The next wobble is
    /// `destabilizationBaseTime` seconds out, each after that
    /// `destabilizationTimeReductionRatio` of the last.
    fn wobble_rocket(&mut self, i: usize) {
        use vcod_common::x87::normalize_stored;
        let now = self.level_time();
        let m = &self.combat.missiles[i];
        if m.destab as i32 + m.tr_time >= now {
            return;
        }
        let Some(w) = self
            .combat
            .weapons
            .get(usize::from(m.weapon))
            .and_then(Option::as_ref)
        else {
            return;
        };
        let (angle, base, ratio, speed) = (
            w.destabilization_angle_max,
            w.destabilization_base_time,
            w.destabilization_time_reduction_ratio,
            w.projectile_speed as f32,
        );
        // The angle through the double `M_PI / 180`, stored, then `tan`.
        let rad = (f64::from(angle) * (std::f64::consts::PI / 180.0)) as f32;
        let t = f64::from(rad).tan() as f32;
        let mut dir = normalize_stored(m.delta);
        for k in 0..3 {
            let push = self.combat.rand.flrand(-1.0, 1.0) * t;
            dir[k] += push;
        }
        let dir = normalize_stored(dir);
        let angles = super::builtins::math::vectoangles(dir.to_array());
        let m = &mut self.combat.missiles[i];
        m.delta = dir * speed;
        m.base = m.origin;
        m.apos_type = vcod_cod2::trajectory::TR_STATIONARY;
        m.apos_base = Vec3::from(angles);
        m.apos_delta = Vec3::ZERO;
        m.apos_time = 0;
        m.tr_time = now;
        m.destab = if m.wobbled {
            m.destab * ratio
        } else {
            1000.0 * base
        };
        m.wobbled = true;
    }

    /// A cooked grenade gone off in hand (`ClientEvents`' event 197):
    /// `player_die` with `MOD_SUICIDE` through the script's damage path is
    /// retail's; vcod blows the grenade up where its holder stands.
    fn grenade_suicide(&mut self, slot: usize) {
        let Some(p) = self.players.get(slot).and_then(Option::as_ref) else {
            return;
        };
        let (offhand, origin) = (p.ws.offhand, p.ps.origin);
        if let Some(i) = self.fire_grenade(Some(slot), offhand, origin, Vec3::ZERO, 0) {
            let now = self.level_time();
            self.combat.missiles[i].explode_at = now;
        }
    }

    /// An entity number for a missile: `G_Spawn` with classname `grenade`,
    /// which the object table's wire pass leaves to [`Self::missile_entities`].
    fn spawn_missile_entity(&mut self) -> Option<EntId> {
        self.rt.with_host(|h, cx| {
            let id = h.ents.spawn(cx).ok()?;
            if let crate::game::fields::Route::Engine { slot, .. } =
                crate::game::fields::route_entity("classname")
                && let Some(e) = h.ents.get_mut(id)
            {
                e.engine[slot] = Value::String(cx.intern_exact("grenade"));
            }
            Some(id)
        })
    }

    /// A temp entity at the level's clock.
    fn temp_entity(&mut self, te: TempEntity) {
        self.rt.with_host(|h, cx| h.add_temp_entity(cx, te));
    }

    /// `G_RunMissile` for every missile, then the fuses (`G_ExplodeMissile`
    /// off `G_RunThink`), and the explode events' frees.
    pub fn run_missiles(&mut self) {
        let now = self.level_time();
        let mut i = 0;
        while i < self.combat.missiles.len() {
            let m = &self.combat.missiles[i];
            if let Some(at) = m.exploded_at {
                if now - at > EVENT_VALID_MS {
                    let id = m.id;
                    self.combat.missiles.remove(i);
                    self.rt.with_host(|h, _| h.free_entity(id));
                    continue;
                }
                i += 1;
                continue;
            }
            if !self.run_missile(i) {
                // Freed (into the sky).
                let id = self.combat.missiles.remove(i).id;
                self.rt.with_host(|h, _| h.free_entity(id));
                continue;
            }
            if self.combat.missiles[i].exploded_at.is_none()
                && now >= self.combat.missiles[i].explode_at
            {
                self.explode_missile(i);
            }
            i += 1;
        }
    }

    /// One missile's move (Mac 0x1ca0e4): the trace from where it is to
    /// where its trajectory puts it, a bounce off the world
    /// (`G_BounceMissile`) with its event, a rocket's impact explode.
    /// Returns false when the missile is freed.
    fn run_missile(&mut self, i: usize) -> bool {
        let now = self.level_time();
        let m = self.combat.missiles[i].clone();
        if m.tr_type == vcod_cod2::trajectory::TR_STATIONARY {
            return true;
        }
        let to = m.evaluate(now);
        let delta = to - m.origin;
        if delta.length() < 0.001 {
            return true;
        }
        let origins = HashMap::new();
        let mut tr = self.locational_trace(
            m.origin,
            to,
            m.owner,
            MASK_MISSILE,
            &BULLET_PRIORITY,
            &origins,
        );
        let lerp = vec3_lerp;
        let mut hit = lerp(m.origin, to, tr.fraction);
        // A bouncing missile that flew clear or met a floor (normal z over
        // 0.7) probes 1.5 down; on the world it rides 1.5 above the ground
        // it meets there, and the bounce reads that trace instead
        // (cod2-combat.md 9.4).
        if !m.rocket && (tr.fraction >= 1.0 || tr.normal.z > 0.7) {
            let below = hit - Vec3::Z * 1.5;
            let down = self.locational_trace(
                hit,
                below,
                m.owner,
                MASK_MISSILE,
                &BULLET_PRIORITY,
                &origins,
            );
            if down.fraction < 1.0 && down.client.is_none() {
                let end = lerp(hit, below, down.fraction);
                self.combat.missiles[i].base.z += (end.z + 1.5) - hit.z;
                hit = end + Vec3::Z * 1.5;
                tr = down;
            }
        }
        self.combat.missiles[i].origin = hit;
        if tr.fraction >= 1.0 {
            // In the air: no ground, and a projectile weapon's missile
            // wobbles unless `setStableMissile` marked it (lnxded
            // 0x810e0ce's clear arm, 0x810e66c).
            if self.combat.missiles[i].delta != Vec3::ZERO {
                self.combat.missiles[i].ground = 1023;
                let projectile = self
                    .combat
                    .weapons
                    .get(usize::from(m.weapon))
                    .and_then(Option::as_ref)
                    .is_some_and(|w| w.weap_type == 2);
                if projectile && !m.stable {
                    self.wobble_rocket(i);
                }
            }
            return true;
        }
        if tr.surface_flags & SURF_NOIMPACT != 0 {
            return false;
        }
        let surf = ((tr.surface_flags & 0x1f0_0000) >> 20) as i32;
        self.combat.missiles[i].surf_type = surf;
        if m.rocket {
            self.rocket_impact(i, &tr, hit);
            return true;
        }
        let prev = now - 50;
        let at = prev + ((now - prev) as f32 * tr.fraction) as i32;
        if self.bounce_missile(i, &tr, at) {
            self.combat.missiles[i].add_event(ev::GRENADE_BOUNCE, surf);
        }
        true
    }

    /// A rocket's contact (lnxded 0x810d512): the struck body or entity
    /// takes the weapon's `damage`, the hit triggers at the point take it
    /// too, and the rocket goes off at the point snapped to whole units
    /// toward its `trBase` (`SnapVectorTowards`), `trTime` 0, with
    /// `EV_ROCKET_EXPLODE`, or its no-marks twin on a body. The blast
    /// passes the struck one (`G_RadiusDamage`'s `ignore`).
    fn rocket_impact(&mut self, i: usize, tr: &Contact, hit: Vec3) {
        let m = self.combat.missiles[i].clone();
        if let Some(v) = tr.client {
            self.impact_damage(i, v, hit);
        } else if let Some(id) = tr.entity {
            self.impact_entity(i, id, hit);
        }
        let damage = self
            .combat
            .weapons
            .get(usize::from(m.weapon))
            .and_then(Option::as_ref)
            .map_or(0, |w| w.damage);
        if let (Some(attacker), true) = (m.owner, damage != 0) {
            self.rt.trigger_sweep(&super::combat::TriggerSweep {
                attacker,
                start: hit,
                end: hit,
                damage,
                mod_: "MOD_PROJECTILE",
            });
        }
        let toward = |v: f32, to: f32| if to <= v { v.floor() } else { v.ceil() };
        let at = Vec3::new(
            toward(hit.x, m.base.x),
            toward(hit.y, m.base.y),
            toward(hit.z, m.base.z),
        );
        let struck = Struck {
            client: tr.client,
            entity: tr.entity,
        };
        self.explode_missile_at(i, at, tr.normal, struck);
        let m = &mut self.combat.missiles[i];
        m.tr_time = 0;
        // The contact raises the rocket's pair whatever `projExplosionType`
        // says, unlike `G_ExplodeMissile`.
        let k = ((m.event_sequence - 1) & 3) as usize;
        m.events[k] = if tr.client.is_some() {
            ev::ROCKET_EXPLODE_NOMARKS
        } else {
            ev::ROCKET_EXPLODE
        };
    }

    /// A rocket's direct hit: its `damage` on the body it struck, as
    /// `MOD_PROJECTILE`, along its velocity as it is (`(0, 0, 1)` when
    /// still), not normalized (lnxded 0x810d512).
    fn impact_damage(&mut self, i: usize, victim: usize, at: Vec3) {
        let m = self.combat.missiles[i].clone();
        let Some(w) = self
            .combat
            .weapons
            .get(usize::from(m.weapon))
            .cloned()
            .flatten()
        else {
            return;
        };
        if w.damage == 0 {
            return;
        }
        let mut dir = m.velocity(self.level_time());
        if dir == Vec3::ZERO {
            dir.z = 1.0;
        }
        self.g_damage(
            victim,
            m.owner.unwrap_or(1023),
            Some(m.id),
            m.weapon,
            at,
            Some(dir),
            w.damage,
            0,
            "MOD_PROJECTILE",
            0,
            0,
        );
    }

    /// `G_BounceMissile` (Mac 0x1c9db0): the velocity at the contact
    /// reflected, scaled by the weapon's `parallel`/`perpendicular` bounce
    /// for the surface by how square it struck; one slower than 20 on a
    /// floor (normal z over 0.7) comes to rest. Returns whether the bounce
    /// raises its event: not in water and the velocity changed by over 100.
    fn bounce_missile(&mut self, i: usize, tr: &Contact, at: i32) -> bool {
        let now = self.level_time();
        let surf = ((tr.surface_flags & 0x1f0_0000) >> 20) as usize;
        let weapon = self.combat.missiles[i].weapon;
        let w = self
            .combat
            .weapons
            .get(usize::from(weapon))
            .cloned()
            .flatten();
        let water = self
            .combat
            .clip
            .as_ref()
            .is_some_and(|c| c.point_contents(self.combat.missiles[i].origin) & 0x20 != 0);
        let m = &mut self.combat.missiles[i];
        let vel = m.velocity(at);
        let dot = vel.dot(tr.normal);
        let mut delta = vel + tr.normal * (-2.0 * dot);
        if tr.normal.z > 0.7 {
            m.ground = tr.client.map_or(1022, |c| c as i32);
        }
        let speed = vel.length();
        if speed > 0.0 && dot <= 0.0 {
            let (par, perp) = w.as_ref().map_or((0.5, 0.5), |w| {
                (
                    w.parallel_bounce[surf.min(22)],
                    w.perpendicular_bounce[surf.min(22)],
                )
            });
            let f = (dot / -speed) * (perp - par) + par;
            delta *= f;
        }
        if tr.normal.z > 0.7 && delta.length() < 20.0 {
            m.tr_type = vcod_cod2::trajectory::TR_STATIONARY;
            m.base = m.origin;
            m.delta = Vec3::ZERO;
            m.tr_time = 0;
            m.apos_delta = Vec3::ZERO;
            return false;
        }
        let mut lift = tr.normal * 0.1;
        lift.z = lift.z.min(0.0);
        m.origin += lift;
        m.base = m.origin;
        m.delta = delta;
        m.tr_time = now;
        !water && (delta - vel).length() > 100.0
    }

    /// `G_ExplodeMissile` (Mac 0x1c94f4) at the trajectory's point, its
    /// normal off a 16-unit trace down.
    fn explode_missile(&mut self, i: usize) {
        let now = self.level_time();
        let m = &self.combat.missiles[i];
        let p = m.evaluate(now);
        let at = Vec3::new(p.x.trunc(), p.y.trunc(), p.z.trunc());
        let normal = match &self.combat.clip {
            Some(c) => {
                let t = c.trace(at, at - Vec3::Z * 16.0, Vec3::ZERO, Vec3::ZERO, 0x811);
                if t.fraction < 1.0 { t.normal } else { Vec3::Z }
            }
            None => Vec3::Z,
        };
        let id = self.combat.missiles[i].id;
        self.explode_missile_at(
            i,
            at,
            normal,
            Struck {
                client: None,
                entity: Some(id),
            },
        );
    }

    /// The explode's tail: the entity turns `ET_GENERAL`, hidden, with
    /// `EV_GRENADE_EXPLODE` (`EV_ROCKET_EXPLODE` for a rocket,
    /// `EV_CUSTOM_EXPLODE` for `projExplosionType` none) on its ring, and
    /// `G_RadiusDamage` with the weapon's explosion keys, passing `ignore`.
    fn explode_missile_at(&mut self, i: usize, at: Vec3, normal: Vec3, ignore: Struck) {
        let now = self.level_time();
        let weapon = self.combat.missiles[i].weapon;
        let w = self
            .combat
            .weapons
            .get(usize::from(weapon))
            .cloned()
            .flatten();
        let event = match w.as_ref().map_or(0, |w| w.proj_explosion_type) {
            2 => ev::CUSTOM_EXPLODE,
            _ if self.combat.missiles[i].rocket => ev::ROCKET_EXPLODE,
            _ => ev::GRENADE_EXPLODE,
        };
        let m = &mut self.combat.missiles[i];
        m.origin = at;
        m.base = at;
        m.tr_type = vcod_cod2::trajectory::TR_STATIONARY;
        m.delta = Vec3::ZERO;
        m.add_event(event, dir_byte(normal));
        m.exploded_at = Some(now);
        let (id, owner, rocket) = (m.id, m.owner, m.rocket);
        let Some(w) = w else {
            return;
        };
        if w.explosion_inner_damage != 0 {
            let mod_ = if rocket {
                "MOD_PROJECTILE_SPLASH"
            } else {
                "MOD_GRENADE_SPLASH"
            };
            self.radius_damage(
                at,
                id,
                owner,
                weapon,
                w.explosion_inner_damage as f32,
                w.explosion_outer_damage as f32,
                w.explosion_radius as f32,
                mod_,
                ignore,
            );
        }
    }

    /// `G_RadiusDamage` (Mac 0x1c45b8) over the playing clients: each
    /// within `radius` of `at` takes the outer-to-inner lerp by distance
    /// times `CanDamage`'s share; one `CanDamage` cannot see takes a tenth
    /// when a capsule trace to its box's centre meets something within a
    /// fifth of the radius. The direction is the victim's origin less the
    /// point, raised 24. Under `setPlayerIgnoreRadiusDamage` no client is
    /// touched. A thrower who left takes no walk at all. `ignore` is
    /// passed over.
    #[allow(clippy::too_many_arguments)]
    pub fn radius_damage(
        &mut self,
        at: Vec3,
        inflictor: EntId,
        owner: Option<usize>,
        weapon: u8,
        inner: f32,
        outer: f32,
        radius: f32,
        mod_: &'static str,
        ignore: Struck,
    ) {
        let Some(attacker) = owner else {
            return;
        };
        if self.rt.host.ignore_radius_damage {
            return;
        }
        let radius = radius.max(1.0);
        for v in self.targets(ignore.client) {
            let origin = self.players[v].as_ref().expect("a target").ps.origin;
            let dist = super::cod2_blast::stored_length(origin - at);
            if dist >= radius {
                continue;
            }
            let damage = |mult| super::cod2_blast::blast_damage(inner, outer, radius, dist, mult);
            let share = self.can_damage(v, at);
            let dealt = if share > 0.0 {
                damage(share)
            } else {
                let ps = &self.players[v].as_ref().expect("a target").ps;
                let centre = origin + (ps.mins + ps.maxs) * 0.5;
                let reached = self.combat.clip.as_ref().is_some_and(|c| {
                    c.trace(at, centre, Vec3::ZERO, Vec3::ZERO, 0x811).fraction < 1.0
                });
                if !reached || centre.distance(at) >= 0.2 * radius {
                    continue;
                }
                damage(0.1)
            };
            let mut dir = origin - at;
            dir.z += 24.0;
            self.g_damage(
                v,
                attacker,
                Some(inflictor),
                weapon,
                at,
                Some(dir),
                dealt,
                DFLAG_RADIUS,
                mod_,
                0,
                0,
            );
        }
        // The entities with `takedamage` the walk meets (`game::cod2_blast`).
        let attacker_ent = self.rt.client_entity(attacker);
        self.rt.with_host(|h, cx| {
            super::cod2_blast::damage_entities(
                h,
                cx,
                at,
                inner,
                outer,
                radius,
                attacker_ent,
                Some(inflictor),
                ignore.entity,
                mod_,
                true,
            )
        });
    }

    /// `CanDamage` (Mac 0x1c3f18) for a client: five probes from the blast
    /// to the body's middle and its four corners, 15 either side across
    /// the line to the blast and half the eye's height up and down; 0 when
    /// none passes, `n / 3` for one to three, 1 for four or five. Each is
    /// `G_LocationalTracePassed` (lnxded 0x810a622) under 0x802891 passing
    /// the victim: the world, the static models and the entities block it.
    fn can_damage(&mut self, victim: usize, at: Vec3) -> f32 {
        if self.combat.clip.is_none() {
            return 1.0;
        }
        let ps = &self.players[victim].as_ref().expect("a target").ps;
        let eye = Self::view_origin(ps);
        let centre = (eye + ps.origin) * 0.5;
        let half = (eye.z - ps.origin.z) * 0.5;
        let mut d = at - ps.origin;
        d.z = 0.0;
        let d = d.normalize_or_zero();
        let side = Vec3::new(-d.y, d.x, 0.0);
        let probes = [
            centre,
            centre + side * 15.0 + Vec3::Z * half,
            centre + side * 15.0 - Vec3::Z * half,
            centre - side * 15.0 + Vec3::Z * half,
            centre - side * 15.0 - Vec3::Z * half,
        ];
        let none = HashMap::new();
        let passed = probes
            .iter()
            .filter(|&&p| {
                self.locational_trace(at, p, Some(victim), MASK_BLAST, &BULLET_PRIORITY, &none)
                    .fraction
                    >= 1.0
            })
            .count();
        match passed {
            0 => 0.0,
            n @ 1..=3 => n as f32 / 3.0,
            _ => 1.0,
        }
    }

    /// The missiles' entities for the snapshot, in CoD2's numbers over
    /// its table: `ET_MISSILE` with its two trajectories and the bounce
    /// flag in the air, `ET_GENERAL` with `EF_NODRAW` once it went off
    /// (`G_ExplodeMissile` 0x810d970), each with its ring.
    pub fn missile_entities(&self) -> Vec<(u32, vcod_common::net::msg::EntityState)> {
        use vcod_common::net::msg::EntityState;
        let p = vcod_cod2::net::PROTOCOL_118.as_cod1_shim();
        self.combat
            .missiles
            .iter()
            .map(|m| {
                let mut e = EntityState::null(p);
                e.number = m.id.0;
                let mut seti = |name: &str, v: i32| {
                    if let Some(i) = EntityState::field_index(p, name) {
                        e.fields[i] = v;
                    }
                };
                let exploded = m.exploded_at.is_some();
                seti("eType", if exploded { ET_GENERAL } else { ET_MISSILE });
                // `EF_NODRAW` once exploded; the 0x1000000 bounce flag,
                // above the field's 24 bits, which the delta's change and
                // zero bits still carry (cod2-protocol.md 5.2). A rocket
                // flies with 0x400 and keeps it past the impact, which
                // toggles `EF_TELEPORT_BIT` (lnxded 0x810d512).
                let flags = match (m.rocket, exploded) {
                    (false, false) => 0x0100_0000,
                    (false, true) => 0x20,
                    (true, false) => 0x400,
                    (true, true) => 0x422,
                };
                seti("eFlags", flags);
                seti("time", m.time);
                seti("weapon", i32::from(m.weapon));
                seti("pos.trType", m.tr_type);
                seti("pos.trTime", m.tr_time);
                seti("apos.trType", if exploded { 0 } else { m.apos_type });
                seti("apos.trTime", m.apos_time);
                for k in 0..3 {
                    seti(&format!("pos.trBase[{k}]"), m.base[k].to_bits() as i32);
                    seti(&format!("pos.trDelta[{k}]"), m.delta[k].to_bits() as i32);
                    seti(
                        &format!("apos.trBase[{k}]"),
                        m.apos_base[k].to_bits() as i32,
                    );
                    seti(
                        &format!("apos.trDelta[{k}]"),
                        m.apos_delta[k].to_bits() as i32,
                    );
                }
                seti("groundEntityNum", m.ground);
                seti("surfType", m.surf_type);
                seti("clientNum", m.owner.map_or(1023, |o| o as i32));
                seti("eventSequence", m.event_sequence & 0xff);
                // The ring is kept in CoD2's numbers.
                for k in 0..4 {
                    seti(&format!("events[{k}]"), m.events[k]);
                    seti(&format!("eventParms[{k}]"), m.event_parms[k]);
                }
                (m.id.0, e)
            })
            .collect()
    }

    /// What `finishPlayerDamage` queued for the playerstates: the
    /// knockback, the damage timer and the flinch, and `player_die`'s
    /// playerstate half on a fatal one.
    pub fn apply_damage_ops(&mut self) {
        let ops = self.rt.take_sim_ops();
        let mut rest = Vec::new();
        for (slot, op) in ops {
            match op {
                SimOp::Damaged {
                    damage,
                    dir,
                    knockback,
                    fatal,
                    dead_yaw,
                    attacker,
                    ..
                } => {
                    let now = self.level_time();
                    if let Some(a) = attacker.filter(|&a| a != slot)
                        && let Some(p) = self.players.get_mut(slot).and_then(Option::as_mut)
                    {
                        p.last_attacker = Some((a, now));
                    }
                    self.damaged(slot, damage, Vec3::from(dir), knockback, fatal, dead_yaw)
                }
                other => rest.push((slot, other)),
            }
        }
        self.rt.host.client_sim_ops.extend(rest);
    }

    /// `finishPlayerDamage`'s playerstate stores (Mac 0x1c2e32): the
    /// knockback, 0.3 of the damage standing, 0.15 crouched, 0.02 prone,
    /// at most 60, times `g_knockback` over 250 along the direction with a
    /// `pm_time` of twice it in 50..200; the damage timer
    /// (`player_dmgtimer_timePerPoint` 100 a point, at most
    /// `player_dmgtimer_maxTime` 750) and the flinch's yaw. A fatal one runs
    /// `player_die`'s (Mac 0x1c4a7c): a grenade still in hand drops live,
    /// `pm_type` goes dead, the box's top to 30.
    fn damaged(
        &mut self,
        slot: usize,
        damage: i32,
        dir: Vec3,
        knockback: bool,
        fatal: bool,
        dead_yaw: i32,
    ) {
        let g_knockback = match self.rt.host.cvars.get("g_knockback") {
            "" => 1000.0,
            v => crate::game::builtins::cvar::atof(v),
        };
        let now = self.level_time();
        let Some(p) = self.players.get_mut(slot).and_then(Option::as_mut) else {
            return;
        };
        let ps = &mut p.ps;
        if knockback && ps.e_flags & 0x300 == 0 {
            let scale = if ps.pm_flags & vcod_cod2::pmove::pmf::PRONE != 0 {
                0.02
            } else if ps.pm_flags & vcod_cod2::pmove::pmf::DUCKED != 0 {
                0.15
            } else {
                0.3
            };
            let knock = ((scale * damage as f32) as i32).min(60);
            if knock > 0 {
                ps.velocity += dir * (knock as f32 * g_knockback / 250.0);
                if ps.pm_time == 0 {
                    ps.pm_time = (knock * 2).clamp(50, 200);
                    ps.pm_flags |= vcod_cod2::pmove::pmf::TIME_KNOCKBACK;
                }
            }
        }
        // A god takes the knockback alone (`finishPlayerDamage`, Mac 0x1c2e32).
        if self.rt.host.cod2().clients[slot].flags & crate::game::cod2_cmds::FL_GODMODE != 0 {
            return;
        }
        let ps = &mut p.ps;
        ps.damage_timer = ((ps.damage_timer as f32 + damage as f32 * 100.0) as i32).min(750);
        ps.damage_duration = ps.damage_timer;
        // `P_DamageFeedback`'s input (`gclient+0x27e0..`).
        let from_world = dir == Vec3::ZERO;
        let from = if from_world { ps.origin } else { dir };
        // Two truncations and an int subtraction on lnxded (0x80fb170;
        // the Mac build subtracts in float): `(int)vectoyaw(dir) -
        // (int)viewYaw` (cod2-combat.md 9.2).
        let flinch = if dir == Vec3::ZERO {
            0
        } else {
            super::builtins::math::vectoyaw(dir.to_array()) as i32
        };
        let mut view = ps.view_angles[1];
        if view < 0.0 {
            view += 360.0;
        }
        ps.flinch_yaw = flinch - view as i32;
        p.damage_blood += damage;
        p.damage_from_world = from_world;
        p.damage_from = from;
        if !fatal {
            return;
        }
        // `player_die`: a grenade in hand drops from 40 up on three draws
        // of 160 (two `crandom`, one `random`).
        let fuse = std::mem::take(&mut p.ws.grenade_time_left);
        let offhand = p.ws.offhand;
        let origin = p.ps.origin;
        let was_dead = p.ps.pm_type >= pm_type::DEAD;
        p.ps.pm_type = if p.ps.pm_type == pm_type::NORMAL_LINKED {
            pm_type::DEAD_LINKED
        } else {
            pm_type::DEAD
        };
        p.ps.maxs.z = 30.0;
        // The view takes the entity's angles, which each
        // `ClientThink_real` leaves at (0, view yaw, 0) (Mac 0x1a3d00):
        // the dead look level (cod2-killcam.md 5). A `setPlayerAngles`
        // since the last cmd, which sets them whole, is not kept apart.
        if !was_dead {
            p.ps.view_angles = [0.0, p.ps.view_angles[1], 0.0];
        }
        p.dead_yaw = dead_yaw;
        p.died_at = now;
        // Then `BG_AnimScriptEvent(ps, ANIM_ET_DEATH, 0, 1)` on the line
        // `finishPlayerDamage` drew: legs the death anim, torso none.
        let state = self.rt.host.cod2().clients[slot];
        let live = &state.death_anims[..usize::from(state.death_anim_count)];
        // `player_die` returns for a body already dead (`pm_type` past 1).
        if let (false, Some(table), Some(k)) = (
            was_dead,
            self.anims.as_deref(),
            live.iter().position(|a| a.0 == state.death_anim),
        ) {
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
            super::cod2::death_event(&mut p.ps, &input, |_| k);
        }
        if fuse != 0 {
            let v = Vec3::new(
                (self.combat.randomf() * 2.0 - 1.0) * 160.0,
                (self.combat.randomf() * 2.0 - 1.0) * 160.0,
                self.combat.randomf() * 160.0,
            );
            self.fire_grenade(Some(slot), offhand, origin + Vec3::Z * 40.0, v, fuse);
        }
    }
}

/// Whether the segment `start..start + (end - start) * limit` meets the
/// box `lo..hi`.
fn segment_meets_box(start: Vec3, end: Vec3, lo: Vec3, hi: Vec3, limit: f32) -> bool {
    let d = end - start;
    let (mut t0, mut t1) = (0.0f32, limit);
    for a in 0..3 {
        if d[a].abs() < 1e-6 {
            if start[a] < lo[a] || start[a] > hi[a] {
                return false;
            }
            continue;
        }
        let (mut x, mut y) = ((lo[a] - start[a]) / d[a], (hi[a] - start[a]) / d[a]);
        if x > y {
            std::mem::swap(&mut x, &mut y);
        }
        t0 = t0.max(x);
        t1 = t1.min(y);
        if t0 > t1 {
            return false;
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_hitloc_table_parses_its_info_string() {
        let t =
            parse_hitloc_table("LOCDMGTABLE\\head\\2.5\\torso_upper\\1\\left_foot\\0.5").unwrap();
        assert_eq!(t[2], 2.5);
        assert_eq!(t[4], 1.0);
        assert_eq!(t[17], 0.5);
        assert_eq!(t[0], 1.0);
        assert!(parse_hitloc_table("nope").is_none());
    }

    #[test]
    fn a_segment_meets_a_box_only_inside_its_limit() {
        let (lo, hi) = (Vec3::new(-15.0, -15.0, 0.0), Vec3::new(15.0, 15.0, 70.0));
        let (s, e) = (Vec3::new(-100.0, 0.0, 40.0), Vec3::new(100.0, 0.0, 40.0));
        assert!(segment_meets_box(s, e, lo, hi, 1.0));
        assert!(!segment_meets_box(s, e, lo, hi, 0.3));
        assert!(!segment_meets_box(
            s,
            e + Vec3::Z * 200.0,
            lo,
            hi + Vec3::Z * 0.0,
            0.2
        ));
    }
}
