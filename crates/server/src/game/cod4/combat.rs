//! CoD4 1.7's shots, damage and deaths on a [`Cod4Level`]
//! (docs/research/cod4-combat.md): `ClientEvents`' fire arm
//! (`FireWeapon` at the client's lag time under `g_antilag`), `Bullet_Fire`'s
//! spread and trace through the clip map and the players' posed hit boxes
//! (`XBoneInfo` bounds by `partClassification`, posed off the player
//! animtree through `vcod_cod2::player`'s body), `G_DamageClient`'s weapon
//! location multipliers into `CodeCallback_PlayerDamage`, and the
//! playerstate half of `finishPlayerDamage` and `player_die`
//! ([`Damage4`], queued by `builtins::cod4_combat` and applied when the
//! callback returns), `P_DamageFeedback` at the end frame, and the weapons
//! `dropItem` puts in the world
//! ([`Item4`]).
//!
//! `cod4_combat_ab` compares the result with the kill capture.

use std::collections::{HashMap, VecDeque};
use std::rc::Rc;

use glam::Vec3;
use vcod_cod2::player::{BodyFrame, Character, PlayerAnimSet, PlayerBody, PlayerRig};
use vcod_cod4::anim::BodySource;
use vcod_cod4::net::delta::{EntityState, PlayerState};
use vcod_cod4::pmove::weapon::{WEAPTYPE_GRENADE, WeaponDef, ev as wev};
use vcod_common::animclock::Times;
use vcod_common::bonetrace::{PriorityMap, bone_trace};
use vcod_gsc::{EntId, Value};

use super::level::{Cod4Level, PM_DEAD, PsExt};
use crate::follow::SessionState;

/// `bulletPriorityMap` and `riflePriorityMap`, one byte per hit
/// location: 1.7's `.data` at 0x826ce60 and 0x826ce73, picked by
/// `rifleBullet` in `Bullet_Trace` (0x8089f3f, 0x8089f4c); the melee
/// traces pass the first (0x80a240f, 0x80a2598). VERIFIED (the bytes and
/// the loads).
pub const BULLET_PRIORITY: PriorityMap = [1, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 0];
pub const RIFLE_PRIORITY: PriorityMap = [1, 9, 9, 9, 8, 7, 6, 6, 6, 6, 5, 5, 4, 4, 4, 4, 3, 3, 0];

/// `hitLocation_t` by number, as `G_GetHitLocationString` names them.
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

/// `weapType` 2 (a projectile) and `weapClass` 5 (a grenade): the grenade
/// launcher's pair (`gl_m16_mp`, VERIFIED off `common_mp.ff`).
pub const WEAPTYPE_PROJECTILE: i32 = 2;
pub const WEAPCLASS_GRENADE: i32 = 5;
/// `player_meleeRange`, `..Width`, `..Height`: 1.7's 64, 10, 10
/// (`tools/re/cod4_dvars.py`, VERIFIED).
const MELEE_RANGE: f32 = 64.0;
const MELEE_WIDTH: f32 = 10.0;
const MELEE_HEIGHT: f32 = 10.0;
/// Where the melee's rays end, in `player_meleeWidth` right and
/// `..Height` up: the centre, then the corners, upper right first. 1.7's
/// rodata at 0x82173e0, read by the rays (0x80a24c0) and the edges
/// (0x80a26ab). VERIFIED (the floats and the loads).
const MELEE_OFFSETS: [[f32; 2]; 5] = [
    [0.0, 0.0],
    [1.0, 1.0],
    [-1.0, 1.0],
    [1.0, -1.0],
    [-1.0, -1.0],
];
/// A surface flag the melee's traces swing on through (tested at
/// 0x80a247b and 0x80a25fc).
const SURF_MELEE_PASS: u32 = 0x10;
/// The melee's mask (0x2806891, pushed at 0x80a2417 and 0x80a25a0).
const MASK_MELEE: u32 = 0x280_6891;

/// `EV_AMMO_PICKUP` (cod4-netfields.md 9).
const EV_AMMO_PICKUP: u32 = 9;

/// `Bullet_Trace`'s mask (0x2806831).
pub const MASK_SHOT: u32 = 0x280_6831;
/// `iDFlags` 4.
pub const DFLAG_NO_KNOCKBACK: i32 = 4;
/// `Bullet_Fire`'s range for everything but a spread weapon.
pub(super) const BULLET_RANGE: f32 = vcod_cod4::bullet::BULLET_RANGE;
/// The archive of positions the antilag reads: one second of frames.
const HISTORY_MS: i32 = 1000;

/// The temp entity events this module raises, 1.7's numbers (VERIFIED off
/// the name table, cod4-netfields.md 9).
pub mod ev {
    pub const BULLET_HIT: i32 = 41;
    pub const BULLET_HIT_CLIENT_SMALL: i32 = 42;
    pub const MELEE_HIT: i32 = 35;
    pub const MELEE_MISS: i32 = 36;
    pub const MELEE_BLOOD: u32 = 37;
    pub const OBITUARY: i32 = 66;
}

/// One `finishPlayerDamage`'s playerstate half, applied by the level on its
/// victim ([`Cod4Level::apply_damage_ops`]) as soon as the gsc call that
/// queued it returns: retail writes it inside the call (cod4-combat.md 4).
#[derive(Clone, Debug)]
pub struct Damage4 {
    pub slot: usize,
    pub damage: i32,
    pub dflags: i32,
    /// The normalized direction; `None` when the script passed none.
    pub dir: Option<[f32; 3]>,
    pub fatal: bool,
    /// The attacker's client slot, for the dead view.
    pub attacker: Option<usize>,
    /// A kill's death anim (`BG_AnimScriptEvent(ps, ANIM_ET_DEATH, 0, 1)`).
    pub death: Option<vcod_cod4::pmove::anim::EventAnim>,
    /// A kill's `STAT_DEAD_YAW` toward its killer (`dead_yaw` in
    /// `builtins::cod4_combat`); `None` keeps the body's own view yaw.
    pub dead_yaw: Option<i32>,
}

/// A live grenade `player_die` drops (the death grenade drop, `cod4_lnxded`
/// 0x80b1af7 and 0x80b1e17): the held one with its time left, or
/// Martyrdom's `perk_grenadeDeath` on its fuse, from 40 over the body at
/// a `G_crandom` velocity times 160, spinning. The level fires it
/// ([`Cod4Level::apply_damage_ops`]).
#[derive(Clone, Copy, Debug)]
pub struct DeathGrenade4 {
    pub slot: usize,
    pub weapon: u32,
    pub spot: Vec3,
    pub velocity: Vec3,
    pub time: i32,
}

/// An item's `clientNum` once nobody owns it.
pub const ITEM_NO_OWNER: u32 = 64;

/// A weapon in the world (`LaunchItem`): its entity, the weapon number, the
/// owner's slot and its gravity arc from `start_ms`, at rest once
/// `resting`.
#[derive(Clone, Debug)]
pub struct Item4 {
    pub id: EntId,
    pub weapon: u32,
    pub owner: u32,
    /// When the drop's think hands it to anyone: `owner` becomes 64
    /// (`cod4_lnxded` 0x8090050), 1000 ms past the drop (cod4-combat.md 6).
    pub clear_owner_at: i32,
    pub base: Vec3,
    pub delta: Vec3,
    pub yaw: f32,
    pub start_ms: i32,
    pub resting: bool,
    /// `itemWeaponSetAmmo`'s clip and stock: what a taker gets.
    pub clip: i32,
    pub stock: i32,
}

/// A playing body's hit boxes as one frame posed them: its entity, the
/// rig, the pose and the model's place (origin and view yaw).
#[derive(Clone)]
pub struct PosedBody4 {
    pub entity: u32,
    pub origin: Vec3,
    pub rig: Rc<PlayerRig>,
    pub pose: Rc<vcod_common::skeleton::PoseBuffer>,
    pub placed: glam::Mat4,
}

impl PosedBody4 {
    /// `G_LocationalTrace`'s body test: `start..end` against the posed
    /// hit boxes under `priority`, nearer than `limit`; the hit and the
    /// struck face's normal in the world.
    pub fn trace(
        &self,
        start: Vec3,
        end: Vec3,
        priority: &PriorityMap,
        limit: f32,
    ) -> Option<(vcod_common::bonetrace::BoneHit, Vec3)> {
        let lo = self.origin + Vec3::new(-31.0, -31.0, -16.0);
        let hi = self.origin + Vec3::new(31.0, 31.0, 86.0);
        if !segment_meets_box(start, end, lo, hi, limit) {
            return None;
        }
        let inv = self.placed.inverse();
        let (ls, le) = (inv.transform_point3(start), inv.transform_point3(end));
        let h = bone_trace(&self.rig.skeleton, &self.pose, ls, le, priority, limit)?;
        let n = self.placed.transform_vector3(h.normal).normalize_or_zero();
        Some((h, n))
    }
}

/// What the level keeps for the shots: the body poses, the hit poses' rigs,
/// the weapon defs pmove reads and the antilag's positions.
#[derive(Default)]
pub struct Combat4 {
    pub src: Option<Rc<BodySource>>,
    anims: Option<Option<PlayerAnimSet>>,
    rigs: HashMap<Character, Option<Rc<PlayerRig>>>,
    bodies: Vec<PlayerBody>,
    /// The level's weapon defs by number, as pmove reads them.
    pub defs: Vec<WeaponDef>,
    /// Each frame's live origins (`SV_GetClientPositionAtTime`'s archive),
    /// newest first.
    history: VecDeque<(i32, Vec<Option<Vec3>>)>,
    /// `client->damage_blood` and `damage_from`, by slot, until the end
    /// frame's feedback takes them.
    blood: Vec<(i32, Option<[f32; 3]>)>,
    /// The direction of the blow behind each slot's last `damageEvent`
    /// step, for the bots: a real one can pack as the bytes' 255 none
    /// (cod4-combat.md 4).
    hit_dir: Vec<Option<[f32; 3]>>,
    /// `client->currentAimSpreadScale`: the end frame's spread over 255.
    aim_spread: Vec<f32>,
    /// The aim block's state by slot (`vcod_cod4::pmove::aim`), zeroed at
    /// each spawn.
    pub(super) aim: Vec<vcod_cod4::pmove::aim::AimState>,
    /// The gun pitch and yaw by slot: what the last cmd's aim block left,
    /// the angles a round leaves along (cod4-combat.md 3.2).
    pub(super) gun_angles: Vec<[f32; 2]>,
    /// Every bullet or melee hit scored, `(attacker, victim, hit
    /// location)`, for the tests.
    pub hits: Vec<(usize, usize, u8)>,
    /// Every bullet round fired, a gunner's turret rounds too, `(shooter,
    /// game time, start, the aim's end, where the spread put it)`, for the
    /// tests.
    pub shots: Vec<(usize, i32, Vec3, Vec3, Vec3)>,
    /// Every player a blast's splash reached, `(victim, damage)` as the
    /// engine hands it to the scripts, for the tests.
    pub splashes: Vec<(usize, i32)>,
    /// `info/bullet_penetration_mp`, read at the level's first shot
    /// (`None` inside when the zones carry none).
    pub(super) penetration: Option<Option<Rc<super::bullet::PenTable>>>,
}

impl Combat4 {
    /// `ClientSpawn`'s memset of the whole client (`cod4_lnxded` 0x80a7cc9,
    /// 0x3184 bytes) drops the blood and the blow not yet fed back, so a
    /// killing blow steps no `damageEvent` on the next life.
    pub(super) fn clear_blood(&mut self, slot: usize) {
        self.blood[slot] = (0, None);
        self.hit_dir[slot] = None;
    }

    pub fn new(src: Option<Rc<BodySource>>, slots: usize) -> Combat4 {
        Combat4 {
            src,
            bodies: vec![PlayerBody::default(); slots],
            blood: vec![(0, None); slots],
            hit_dir: vec![None; slots],
            aim_spread: vec![0.0; slots],
            aim: vec![Default::default(); slots],
            gun_angles: vec![[0.0; 2]; slots],
            ..Default::default()
        }
    }

    /// The player anims once loaded (`anims`).
    pub fn anim_set(&self) -> Option<&PlayerAnimSet> {
        self.anims.as_ref()?.as_ref()
    }

    pub(super) fn anims(&mut self) -> Option<&mut PlayerAnimSet> {
        let src = self.src.clone()?;
        self.anims
            .get_or_insert_with(|| {
                PlayerAnimSet::load(&*src)
                    .inspect_err(|e| log::warn!("cod4 hit poses: {e:#}"))
                    .ok()
            })
            .as_mut()
    }

    fn rig(&mut self, c: &Character) -> Option<Rc<PlayerRig>> {
        let src = self.src.clone()?;
        self.rigs
            .entry(c.clone())
            .or_insert_with(|| {
                PlayerRig::assemble(&*src, c)
                    .inspect_err(|e| log::warn!("cod4 hit pose {}: {e:#}", c.body))
                    .ok()
                    .map(Rc::new)
            })
            .clone()
    }
}

pub(super) use vcod_cod4::bullet::random_dir;
use vcod_cod4::bullet::{bullet_end, shots_and_range, spread_amount, spread_for_weapon};

/// `G_GetPlayerViewOrigin` (`cod4_lnxded` 0x80a7778) for a body off a
/// turret: `BG_GetPlayerViewOrigin` at `time` (`level.time` on retail),
/// the eye with its walk bob and lean (cod4-combat.md 3.2).
pub fn view_origin(ps: &vcod_cod4::pmove::PlayerState, time: i32) -> Vec3 {
    vcod_cod4::pmove::aim::view_origin(ps, time)
}

/// A trace's contact.
#[derive(Clone, Copy, Debug)]
pub(super) struct Contact {
    pub(super) fraction: f32,
    pub(super) client: Option<usize>,
    pub(super) hit_location: u8,
    pub(super) surface_flags: u32,
    pub(super) startsolid: bool,
}

/// The stance's idle anim where the playerstate carries none (the anim
/// script is not ported, cod4-player-anims.md 3).
fn idle_anim(pm_flags: u32) -> &'static str {
    use vcod_cod4::pmove::pmf;
    if pm_flags & pmf::PRONE != 0 {
        "pb_prone_aim"
    } else if pm_flags & pmf::DUCKED != 0 {
        "pb_crouch_alert"
    } else {
        "pb_stand_alert"
    }
}

/// Does `start..end` meet the box `lo..hi` before `limit`?
pub(super) fn segment_meets_box(start: Vec3, end: Vec3, lo: Vec3, hi: Vec3, limit: f32) -> bool {
    let d = end - start;
    let (mut t0, mut t1) = (0.0f32, limit);
    for a in 0..3 {
        if d[a].abs() < 1e-9 {
            if start[a] < lo[a] || start[a] > hi[a] {
                return false;
            }
            continue;
        }
        let (mut u, mut v) = ((lo[a] - start[a]) / d[a], (hi[a] - start[a]) / d[a]);
        if u > v {
            std::mem::swap(&mut u, &mut v);
        }
        t0 = t0.max(u);
        t1 = t1.min(v);
        if t0 > t1 {
            return false;
        }
    }
    true
}

impl Cod4Level {
    /// The level's weapon table as pmove reads it, rebuilt when a weapon
    /// registered since the last call.
    pub(super) fn sync_defs(&mut self) {
        let table = &self.rt.host.cod4().table;
        if self.combat.defs.len() == table.defs.len() {
            return;
        }
        let files = &self.rt.host.cod4().weapon_files;
        self.combat.defs = table
            .defs
            .iter()
            .enumerate()
            .map(|(i, d)| {
                if i == 0 {
                    return WeaponDef::default();
                }
                let mut w = files
                    .get(&d.name)
                    .and_then(|f| f.pm.clone())
                    .unwrap_or_default();
                w.ammo_index = d.ammo_slot;
                w.clip_index = d.clip_slot;
                w.alt_weapon = d.alt as u32;
                w
            })
            .collect();
    }

    /// `ClientEvents` for the events `slot`'s cmd raised
    /// (`old_seq..ps.eventSequence`): a fire event shoots at the cmd's
    /// time under `g_antilag`, else at `level.time`.
    pub(super) fn client_events(&mut self, slot: usize, old_seq: i32, cmd_time: i32) {
        let Some(p) = self.players[slot].as_ref() else {
            return;
        };
        let ps = &p.ps;
        let seq = ps.word("eventSequence") as i32;
        let events: Vec<(u32, u32)> = (old_seq.max(seq - 4)..seq)
            .map(|k| {
                (
                    ps.word(&format!("events[{}]", k & 3)),
                    ps.word(&format!("eventParms[{}]", k & 3)),
                )
            })
            .collect();
        let antilag = self.rt.host.cvars.get("g_antilag") != "0";
        let t = if antilag { cmd_time } else { self.level_time() };
        for (e, parm) in events {
            if matches!(e, wev::FIRE_WEAPON | wev::FIRE_WEAPON_LASTSHOT) {
                // `BG_WeaponFireRecoil` (0x80609fc) off the fire event's
                // callback (0x80a47b4): the view's two kicks (the client's
                // own, only drawn) and the gun's two into the spring the
                // next cmd's aim block reads.
                let Some(p) = self.players[slot].as_ref() else {
                    return;
                };
                let ps = vcod_cod4::pmove::PlayerState::from_wire(&p.ps);
                let keys = self
                    .combat
                    .defs
                    .get(vcod_cod4::pmove::aim::view_weapon(&ps) as usize)
                    .map(|d| d.aim.clone())
                    .unwrap_or_default();
                let c4 = self.rt.host.cod4_mut();
                self.combat.aim[slot].fire_recoil(&ps, &keys, || {
                    f64::from(c4.rand()) * f64::from(f32::from_bits(0x3000_0000))
                });
            }
            match e {
                wev::FIRE_WEAPON | wev::FIRE_WEAPON_LASTSHOT => self.fire_weapon(slot, t),
                wev::FIRE_MELEE => self.fire_melee(slot, t),
                wev::USE_OFFHAND => self.use_offhand(slot),
                wev::PREP_OFFHAND => self.grenade_pullback(slot, parm),
                wev::GRENADE_SUICIDE => self.grenade_suicide(slot, parm),
                wev::SWITCH_OFFHAND => self.attempt_live_grenade_pickup(slot, parm),
                _ => {}
            }
        }
    }

    /// `FireWeapon` for a bullet weapon: the spread between the
    /// weapon's two ends by the end frame's aim spread, then `Bullet_Fire`:
    /// the bodies rewound to `game_time`, one round (a spread weapon its
    /// `shotCount`) from the eye along the view, its end off the disc
    /// `Bullet_RandomDir` draws from `game_time`.
    fn fire_weapon(&mut self, slot: usize, game_time: i32) {
        let Some(p) = self.players[slot].as_ref() else {
            return;
        };
        let ps = vcod_cod4::pmove::PlayerState::from_wire(&p.ps);
        let Some(d) = self.combat.defs.get(ps.weapon as usize).cloned() else {
            return;
        };
        if ps.weapon == 0 {
            return;
        }
        match (d.weap_type, d.weap_class) {
            (0, _) => {}
            (WEAPTYPE_GRENADE, _) => return self.throw_grenade(slot, ps.weapon, &d),
            (WEAPTYPE_PROJECTILE, WEAPCLASS_GRENADE) => {
                return self.launch_grenade(slot, ps.weapon, &d);
            }
            (WEAPTYPE_PROJECTILE, _) => {}
            _ => return,
        }
        let scale = self.combat.aim_spread[slot];
        let spread = spread_amount(
            &d,
            spread_for_weapon(&ps, &d),
            scale,
            ps.weapon_pos_frac == 1.0,
        );
        if d.weap_type == WEAPTYPE_PROJECTILE {
            return self.fire_rocket(slot, ps.weapon, &d, spread);
        }
        let axis = self.muzzle_axis(slot, &ps);
        let start = view_origin(&ps, self.level_time());
        let origins = self.rewound(game_time);
        let (shots, range) = shots_and_range(&d);
        for shot in 0..shots {
            let end = bullet_end(game_time + shot, spread, start, &axis, range);
            let aimed = start + axis[0] * range;
            self.combat.shots.push((slot, game_time, start, aimed, end));
            let dir = (end - start).normalize_or_zero();
            let priority = if d.rifle_bullet {
                &RIFLE_PRIORITY
            } else {
                &BULLET_PRIORITY
            };
            let c = self.locational_trace(start, end, slot, priority, &origins);
            // A helicopter on the way: its collision under the shot mask
            // (`SV_ClipMoveToEntity`'s model trace).
            if let Some(h) = self.heli_trace(start, end, c.fraction, None, MASK_SHOT) {
                let hit = start + (end - start) * h.fraction;
                let damage = bullet_damage(&d, hit.distance(start));
                let mod_ = if d.rifle_bullet {
                    "MOD_RIFLE_BULLET"
                } else {
                    "MOD_PISTOL_BULLET"
                };
                let dflags = if d.armor_piercing { 2 } else { 0 };
                self.damage_heli(h.id, Some(slot), &d, damage, dflags, mod_, dir, hit);
                continue;
            }
            self.bullet_fire_round(slot, &d, start, end, game_time, &origins);
        }
    }

    /// `CalcMuzzlePoints`' axis (inlined at 0x80a2b81 and 0x80a30a7):
    /// The gun pitch and yaw, what the
    /// last cmd's aim block left, over the view's roll (cod4-combat.md
    /// 3.2).
    pub(super) fn muzzle_axis(&self, slot: usize, ps: &vcod_cod4::pmove::PlayerState) -> [Vec3; 3] {
        let [pitch, yaw] = self.combat.gun_angles[slot];
        vcod_common::x87::angle_vectors(Vec3::new(pitch, yaw, ps.view_angles[2]))
    }

    /// A bullet's `G_DamageClient` at the shot's time.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn damage_client(
        &mut self,
        attacker: usize,
        victim: usize,
        d: &WeaponDef,
        damage: i32,
        dflags: i32,
        dir: Vec3,
        point: Vec3,
        hitloc: u8,
        game_time: i32,
    ) {
        let mod_ = if d.rifle_bullet {
            "MOD_RIFLE_BULLET"
        } else {
            "MOD_PISTOL_BULLET"
        };
        let dflags = dflags | if d.armor_piercing { 2 } else { 0 };
        self.combat.hits.push((attacker, victim, hitloc));
        let offset = self.level_time() - game_time;
        self.rt.host.cod4_mut().callback.time_offset = offset;
        self.damage_from(
            attacker, victim, None, d, damage, dflags, mod_, dir, point, hitloc,
        );
        self.rt.host.cod4_mut().callback = Default::default();
    }

    /// `G_DamageClient`: a playing victim that takes damage, the
    /// weapon's location multiplier (not on a melee), at least 1, into
    /// `CodeCallback_PlayerDamage`, then the playerstate half the script's
    /// `finishPlayerDamage` queued.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn damage_from(
        &mut self,
        attacker: usize,
        victim: usize,
        inflictor: Option<EntId>,
        d: &WeaponDef,
        damage: i32,
        dflags: i32,
        mod_: &'static str,
        dir: Vec3,
        point: Vec3,
        hitloc: u8,
    ) {
        let session = self.rt.client_session(victim).map(|s| s.state);
        if damage <= 0
            || session != Some(SessionState::Playing)
            || !self.rt.client_vitals(victim).takedamage
        {
            return;
        }
        // `G_GetWeaponHitLocationMultiplier`: a bullet weapon's
        // own table; anything else (a grenade, a turret) reads
        // `info/mp_lochit_dmgtable`, 1 everywhere but the gun's 0 (VERIFIED,
        // the `common_mp` rawfile).
        let damage = if mod_ == "MOD_MELEE" {
            damage
        } else {
            let mult = if d.weap_type == 0 && d.weap_class != 7 {
                d.location_damage
                    .get(hitloc as usize)
                    .copied()
                    .unwrap_or(1.0)
            } else if hitloc == 18 {
                0.0
            } else {
                1.0
            };
            ((f64::from(mult) * f64::from(damage)) as i32).max(1)
        };
        let hit = crate::game::combat::Hit {
            victim,
            attacker,
            inflictor,
            damage,
            dflags,
            mod_,
            weapon: d.name.clone(),
            point: point.to_array(),
            dir: dir.normalize_or_zero().to_array(),
            hitloc: HITLOC_NAMES[(hitloc as usize).min(18)],
        };
        self.rt.deliver_hits(vec![hit]);
        self.apply_damage_ops();
    }

    /// `FireWeaponMelee` on `EV_FIRE_MELEE` (cod4-combat.md 10.4): nothing
    /// on a turret. The swing's contact ([`Self::melee_contact`]) on a
    /// body bleeds the attacker (`EV_MELEE_BLOOD`), raises `EV_MELEE_HIT`
    /// and deals `meleeDamage` plus `G_rand() % 5` as `MOD_MELEE` along
    /// the view at the shot's time; on anything else `EV_MELEE_MISS`. The
    /// event's parm is the knife bit.
    pub(super) fn fire_melee(&mut self, slot: usize, game_time: i32) {
        let Some(ps) = self.pm_state(slot) else {
            return;
        };
        if ps.e_flags & vcod_cod4::pmove::ef::TURRET != 0 {
            return;
        }
        let Some(d) = self.combat.defs.get(ps.weapon as usize).cloned() else {
            return;
        };
        let eye = view_origin(&ps, self.level_time());
        let view = vcod_common::x87::angle_vectors(Vec3::from(ps.view_angles));
        let bodies = self.rewound(game_time);
        let Some((contact, at)) = self.melee_contact(slot, eye, view, &bodies) else {
            return;
        };
        let victim = contact.client;
        if victim.is_some()
            && let Some(p) = self.players[slot].as_mut()
        {
            let mut own = vcod_cod4::pmove::PlayerState::from_wire(&p.ps);
            own.add_event(ev::MELEE_BLOOD, 0);
            p.ps = own.to_wire(&p.ps);
        }
        let event = crate::game::temp_entity::TempEntity {
            event: if victim.is_some() {
                ev::MELEE_HIT
            } else {
                ev::MELEE_MISS
            },
            parm: i32::from(d.knife_model),
            surf_type: 0,
            other: victim.map_or(vcod_cod4::pmove::ENTITYNUM_WORLD as u32, |v| v as u32),
            attacker: 0,
            weapon: ps.weapon as i32,
            client_num: 0,
            scale: 0,
            origin: at.to_array(),
            scope: crate::game::temp_entity::Scope::Broadcast,
            quake: None,
            origin2: None,
        };
        self.rt.with_host(|h, cx| h.add_temp_entity(cx, event));
        let Some(victim) = victim else {
            return;
        };
        self.combat.hits.push((slot, victim, contact.hit_location));
        let damage = d.melee_damage + self.rt.host.cod4_mut().rand() % 5;
        self.rt.host.cod4_mut().callback.time_offset = self.level_time() - game_time;
        self.damage_from(
            slot,
            victim,
            None,
            &d,
            damage,
            0,
            "MOD_MELEE",
            view[0],
            at,
            contact.hit_location,
        );
        self.rt.host.cod4_mut().callback = Default::default();
    }

    /// `Melee_Trace` from `eye` along `view` (forward, right, up), past
    /// `slot`'s own body, the others where `bodies` puts them: the five
    /// rays out to the [`MELEE_OFFSETS`] points at `player_meleeRange`,
    /// then the two edges joining the upper and the lower pair of corners,
    /// which must not start in solid. The first that ends on something
    /// other than a [`SURF_MELEE_PASS`] surface wins: its contact and the
    /// point it was met at.
    fn melee_contact(
        &mut self,
        slot: usize,
        eye: Vec3,
        [forward, right, up]: [Vec3; 3],
        bodies: &HashMap<usize, Vec3>,
    ) -> Option<(Contact, Vec3)> {
        let reach = eye + forward * MELEE_RANGE;
        let corners =
            MELEE_OFFSETS.map(|[x, y]| reach + right * (MELEE_WIDTH * x) + up * (MELEE_HEIGHT * y));
        let rays = corners.iter().map(|&end| (eye, end, false));
        let edges = [
            (corners[1], corners[2], true),
            (corners[3], corners[4], true),
        ];
        for (from, to, open_start) in rays.chain(edges) {
            let c =
                self.locational_trace_mask(from, to, slot, &BULLET_PRIORITY, bodies, MASK_MELEE);
            let met = c.fraction != 1.0 && c.surface_flags & SURF_MELEE_PASS == 0;
            if met && !(open_start && c.startsolid) {
                return Some((c, from.lerp(to, c.fraction)));
            }
        }
        None
    }

    /// The pullback notify on `EV_PREP_OFFHAND`: `grenade_pullback`
    /// with the weapon's name on the thrower.
    pub(super) fn grenade_pullback(&mut self, slot: usize, weapon: u32) {
        let Some(id) = self.rt.client_entity(slot) else {
            return;
        };
        let name = self
            .combat
            .defs
            .get(weapon as usize)
            .map(|d| d.name.clone())
            .unwrap_or_default();
        let v = self
            .rt
            .with_host(|_, cx| Value::String(cx.intern_exact(&name)));
        self.rt
            .host
            .engine_notifies
            .push((id, "grenade_pullback", vec![v]));
    }

    /// `EV_GRENADE_SUICIDE`: a grenade cooked to its end kills its holder
    /// (`player_die` as `MOD_SUICIDE`; held back from a throw back, the
    /// thrower's `MOD_GRENADE_SPLASH`).
    pub(super) fn grenade_suicide(&mut self, slot: usize, weapon: u32) {
        let Some(d) = self.combat.defs.get(weapon as usize).cloned() else {
            return;
        };
        let health = self.rt.client_vitals(slot).health;
        let Some(ps) = self.pm_state(slot) else {
            return;
        };
        let origin = ps.origin;
        // A throw back held too long kills as its thrower's `MOD_GRENADE`
        // (3), else as `MOD_SUICIDE` (12): `cod4_lnxded` 0x80a593c tests
        // `throwBackGrenadeOwner` against 1023, VERIFIED.
        let owner = ps.throw_back_grenade_owner;
        let (attacker, mod_) = match usize::try_from(owner) {
            Ok(o)
                if owner != vcod_cod4::net::delta::ENTITYNUM_NONE as i32
                    && o < self.players.len() =>
            {
                (o, "MOD_GRENADE")
            }
            _ => (slot, "MOD_SUICIDE"),
        };
        self.damage_from(
            attacker,
            slot,
            None,
            &d,
            health.max(1) + 100_000,
            0,
            mod_,
            Vec3::ZERO,
            origin,
            0,
        );
    }

    /// The clients' origins at `time` (`G_AntiLagRewindClientPos`): only for a
    /// shot 50 to 400 ms old, through CoD2's
    /// `SV_GetClientPositionAtTime` walk over this level's archive.
    pub(super) fn rewound(&mut self, time: i32) -> HashMap<usize, Vec3> {
        let now = self.level_time();
        let age = now - time;
        let mut out = HashMap::new();
        // `1000 / sv_fps`, not 50 (0x80a2a54, cod4-server-frame.md 8).
        let frame = self.frame_ms();
        if !(age <= 400 && age > frame) {
            return out;
        }
        let playing: Vec<usize> = (0..self.players.len())
            .filter(|&s| self.playing(s))
            .collect();
        for slot in playing {
            // The archive by frames back, `age * sv_fps / 1000` (0x81795fa).
            let fps = i64::from(self.sv_fps());
            let at = crate::game::cod2_archive::position_at_time(now, time, frame, |age| {
                let back = usize::try_from(i64::from(*age) * fps / 1000).ok()?;
                let (t, row) = self.combat.history.get(back)?;
                (*t == now - back as i32 * frame)
                    .then(|| *row.get(slot)?)
                    .flatten()
            });
            if let Some(crate::game::cod2_archive::Rewind::At(o)) = at {
                out.insert(slot, o);
            }
        }
        out
    }

    /// A body in play: playing, `pm_type` normal or last stand.
    pub(super) fn playing(&mut self, slot: usize) -> bool {
        self.players[slot].as_ref().is_some_and(|p| {
            super::level::in_play(p.ps.word("pm_type")) && p.ps.word("otherFlags") & 4 != 0
        }) && self
            .rt
            .client_session(slot)
            .is_some_and(|s| s.state == SessionState::Playing)
    }

    /// `G_LocationalTrace` from `start` to `end`, passing `skip`'s body:
    /// the clip map under the shot mask, then every other playing body's
    /// hit boxes where its box meets the segment before the world.
    pub(super) fn locational_trace(
        &mut self,
        start: Vec3,
        end: Vec3,
        skip: usize,
        priority: &PriorityMap,
        origins: &HashMap<usize, Vec3>,
    ) -> Contact {
        self.locational_trace_mask(start, end, skip, priority, origins, MASK_SHOT)
    }

    /// [`Self::locational_trace`] under `mask`.
    fn locational_trace_mask(
        &mut self,
        start: Vec3,
        end: Vec3,
        skip: usize,
        priority: &PriorityMap,
        origins: &HashMap<usize, Vec3>,
        mask: u32,
    ) -> Contact {
        let mut best = match &self.clip {
            Some(c) => {
                let mut t = c.trace(
                    start,
                    end,
                    Vec3::ZERO,
                    Vec3::ZERO,
                    mask & !vcod_cod4::pmove::CONTENTS_BODY,
                );
                // The static models clip point traces (cod4-collision.md 4).
                c.trace_static_models(start, end, mask, &mut t);
                Contact {
                    fraction: t.fraction,
                    client: None,
                    hit_location: 0,
                    surface_flags: t.surface_flags,
                    startsolid: t.startsolid,
                }
            }
            None => Contact {
                fraction: 1.0,
                client: None,
                hit_location: 0,
                surface_flags: 0,
                startsolid: false,
            },
        };
        for slot in 0..self.players.len() {
            if slot == skip || !self.playing(slot) {
                continue;
            }
            let ps =
                vcod_cod4::pmove::PlayerState::from_wire(&self.players[slot].as_ref().unwrap().ps);
            let origin = origins.get(&slot).copied().unwrap_or(ps.origin);
            // The hit boxes reach past the link box (a head over 70):
            // the broad test takes the box grown 16 every way.
            let lo = origin + Vec3::new(-31.0, -31.0, -16.0);
            let hi = origin + Vec3::new(31.0, 31.0, 86.0);
            if !segment_meets_box(start, end, lo, hi, best.fraction) {
                continue;
            }
            if let Some(h) = self.body_trace(slot, origin, start, end, priority, best.fraction)
                && h.fraction < best.fraction
            {
                best = Contact {
                    fraction: h.fraction,
                    client: Some(slot),
                    hit_location: h.hit_location,
                    surface_flags: 0,
                    startsolid: h.start_solid,
                };
            }
        }
        best
    }

    /// The first playing body but `skip`'s whose hit boxes `start..end`
    /// meets before `limit`: its slot, the hit, and the struck face's
    /// normal in the world.
    pub(super) fn body_hit(
        &mut self,
        start: Vec3,
        end: Vec3,
        skip: Option<usize>,
        priority: &PriorityMap,
        origins: &HashMap<usize, Vec3>,
        limit: f32,
    ) -> Option<(usize, vcod_common::bonetrace::BoneHit, Vec3)> {
        let mut best: Option<(usize, vcod_common::bonetrace::BoneHit, Vec3)> = None;
        let mut limit = limit;
        for slot in 0..self.players.len() {
            if Some(slot) == skip || !self.playing(slot) {
                continue;
            }
            let ps =
                vcod_cod4::pmove::PlayerState::from_wire(&self.players[slot].as_ref().unwrap().ps);
            let origin = origins.get(&slot).copied().unwrap_or(ps.origin);
            let lo = origin + Vec3::new(-31.0, -31.0, -16.0);
            let hi = origin + Vec3::new(31.0, 31.0, 86.0);
            if !segment_meets_box(start, end, lo, hi, limit) {
                continue;
            }
            if let Some(h) = self.body_trace(slot, origin, start, end, priority, limit)
                && h.fraction < limit
            {
                let yaw = ps.view_angles[1].to_radians();
                let normal = glam::Quat::from_rotation_z(yaw) * h.normal;
                limit = h.fraction;
                best = Some((slot, h, normal));
            }
        }
        best
    }

    /// The character `slot`'s entity carries: its body and attachments.
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

    /// The frame `slot`'s body poses from at `origin`: the playerstate's
    /// anims (the stance's idle where it carries none), the view and the
    /// stance bits.
    fn body_frame(&mut self, slot: usize, origin: Vec3) -> Option<BodyFrame> {
        let ps = vcod_cod4::pmove::PlayerState::from_wire(&self.players.get(slot)?.as_ref()?.ps);
        let w = &self.players[slot].as_ref()?.ps;
        let (legs, torso) = (w.word("legsAnim") as i32, w.word("torsoAnim") as i32);
        let set = self.combat.anims()?;
        let legs = match legs {
            0 => set.anims.wire_of(idle_anim(ps.pm_flags)).unwrap_or(0),
            l => l,
        };
        let mut eflags = ps.e_flags as i32;
        if ps.pm_flags & vcod_cod4::pmove::pmf::SIGHT != 0 {
            eflags |= vcod_cod2::player::eflags::ADS;
        }
        Some(BodyFrame {
            origin,
            angles: [ps.view_angles[0], ps.view_angles[1], 0.0],
            legs_anim: legs,
            torso_anim: torso,
            eflags,
            movement_dir: ps.movement_dir as i8 as f32,
            lean: ps.leanf,
            torso_height: 0.0,
            torso_pitch: ps.torso_pitch,
            waist_pitch: ps.waist_pitch,
            client_num: slot as i32,
        })
    }

    /// `BG_PlayerAnimation` as the end frame runs it: `slot`'s lerp frames
    /// and swings a frame on.
    pub(super) fn animate_body(&mut self, slot: usize) {
        let Some(p) = self.players[slot].as_ref() else {
            return;
        };
        let origin = Vec3::new(
            f32::from_bits(p.ps.word("origin[0]")).trunc(),
            f32::from_bits(p.ps.word("origin[1]")).trunc(),
            f32::from_bits(p.ps.word("origin[2]")).trunc(),
        );
        let now = self.level_time();
        // `level.frametime` (0x80a4838).
        let dt = self.frame_time();
        let Some(f) = self.body_frame(slot, origin) else {
            return;
        };
        let Combat4 { anims, bodies, .. } = &mut self.combat;
        let Some(Some(set)) = anims.as_mut() else {
            return;
        };
        let at = Times {
            clock_ms: now,
            snapshot_ms: now,
        };
        bodies[slot].animate(
            &set.anims,
            &f,
            at,
            dt,
            vcod_common::playerpose::BG_SWING_SPEED,
        );
    }

    /// `slot`'s body posed at `origin` and traced from `start` to `end` in
    /// the model's frame (the origin and the view's yaw).
    fn body_trace(
        &mut self,
        slot: usize,
        origin: Vec3,
        start: Vec3,
        end: Vec3,
        priority: &PriorityMap,
        limit: f32,
    ) -> Option<vcod_common::bonetrace::BoneHit> {
        self.body_trace_eased(slot, origin, start, end, priority, limit, true)
    }

    /// [`Self::body_trace`]; `ease` steps the body's controls a frame
    /// first, as every trace but a round's penetration legs does.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn body_trace_eased(
        &mut self,
        slot: usize,
        origin: Vec3,
        start: Vec3,
        end: Vec3,
        priority: &PriorityMap,
        limit: f32,
        ease: bool,
    ) -> Option<vcod_common::bonetrace::BoneHit> {
        let character = self.character(slot)?;
        let f = self.body_frame(slot, origin)?;
        let rig = self.combat.rig(&character)?;
        let now = self.level_time();
        let src = self.combat.src.clone()?;
        let dt = self.frame_time();
        let Combat4 { anims, bodies, .. } = &mut self.combat;
        let set = anims.as_mut()?.as_mut()?;
        let b = &mut bodies[slot];
        if ease {
            b.ease_controls(&set.anims, &f, dt);
        }
        let pose = b.pose_in(&rig, set, &*src, &f, now, true, None);
        let placed = BodyFrame {
            origin,
            angles: [0.0, f.angles[1], 0.0],
            ..Default::default()
        }
        .transform();
        let inv = placed.inverse();
        let (ls, le) = (inv.transform_point3(start), inv.transform_point3(end));
        bone_trace(&rig.skeleton, &pose, ls, le, priority, limit)
    }

    /// Every playing body posed where it stands at this frame's time, for
    /// the script's locational traces (`bulletTrace`) to read through the
    /// next frame's threads.
    pub(super) fn posed_bodies(&mut self) -> Vec<PosedBody4> {
        let now = self.level_time();
        let mut out = Vec::new();
        for slot in 0..self.players.len() {
            if !self.playing(slot) {
                continue;
            }
            let Some(entity) = self.rt.client_entity(slot) else {
                continue;
            };
            let Some(ps) = self.pm_state(slot) else {
                continue;
            };
            let Some(character) = self.character(slot) else {
                continue;
            };
            let Some(f) = self.body_frame(slot, ps.origin) else {
                continue;
            };
            let Some(rig) = self.combat.rig(&character) else {
                continue;
            };
            let Some(src) = self.combat.src.clone() else {
                continue;
            };
            let Combat4 { anims, bodies, .. } = &mut self.combat;
            let Some(Some(set)) = anims.as_mut() else {
                continue;
            };
            let pose = bodies[slot].pose_in(&rig, set, &*src, &f, now, true, None);
            let placed = BodyFrame {
                origin: ps.origin,
                angles: [0.0, f.angles[1], 0.0],
                ..Default::default()
            }
            .transform();
            out.push(PosedBody4 {
                entity: entity.0,
                origin: ps.origin,
                rig,
                pose: Rc::new(pose),
                placed,
            });
        }
        out
    }

    /// The world centre of the first hit box of `slot`'s posed body whose
    /// location is `hitloc` (2 the head), as a shot this frame meets it.
    pub fn hit_box_center(&mut self, slot: usize, hitloc: u8) -> Option<Vec3> {
        let ps = self.pm_state(slot)?;
        let character = self.character(slot)?;
        let f = self.body_frame(slot, ps.origin)?;
        let rig = self.combat.rig(&character)?;
        let now = self.level_time();
        let src = self.combat.src.clone()?;
        let dt = self.frame_time();
        let Combat4 { anims, bodies, .. } = &mut self.combat;
        let set = anims.as_mut()?.as_mut()?;
        let b = &mut bodies[slot];
        b.ease_controls(&set.anims, &f, dt);
        let pose = b.pose_in(&rig, set, &*src, &f, now, true, None);
        let skel = &rig.skeleton;
        let (i, bone) = skel
            .bones()
            .iter()
            .enumerate()
            .find(|(_, b)| b.has_hit_box() && b.hit_location == hitloc)?;
        let (pos, rot) = pose.bone_world(skel, i);
        let local = pos + rot * ((bone.hit_mins + bone.hit_maxs) * 0.5);
        let placed = BodyFrame {
            origin: ps.origin,
            angles: [0.0, f.angles[1], 0.0],
            ..Default::default()
        }
        .transform();
        Some(placed.transform_point3(local))
    }

    /// The archive the antilag reads: this frame's live origins.
    pub(super) fn record_positions(&mut self) {
        let now = self.level_time();
        let playing: Vec<bool> = (0..self.players.len()).map(|s| self.playing(s)).collect();
        let row = (0..self.players.len())
            .map(|s| {
                playing[s].then(|| {
                    let ps = &self.players[s].as_ref().unwrap().ps;
                    Vec3::new(
                        f32::from_bits(ps.word("origin[0]")),
                        f32::from_bits(ps.word("origin[1]")),
                        f32::from_bits(ps.word("origin[2]")),
                    )
                })
            })
            .collect();
        self.combat.history.push_front((now, row));
        let keep = (HISTORY_MS / self.frame_ms()) as usize;
        self.combat.history.truncate(keep);
    }

    /// The playerstate half of each `finishPlayerDamage` the script ran
    /// ([`Damage4`], cod4-combat.md 4), in order: the push
    /// ([`knock_back`]), the blood and its direction for the end frame's
    /// feedback, `damageTimer` (and `damageDuration`) 100 ms a point up to
    /// 750, the script's health into the stats, and on a kill
    /// `player_die`'s dead `pm_type`, death anim, empty hands and health 0.
    /// Then the live grenades `player_die` dropped.
    pub(super) fn apply_damage_ops(&mut self) {
        let ops = std::mem::take(&mut self.rt.host.cod4_mut().damage_ops);
        let g_knockback = g_knockback(&self.rt.host.cvars);
        for op in ops {
            let health = self.rt.client_vitals(op.slot).health;
            let Some(p) = self.players[op.slot].as_mut() else {
                continue;
            };
            let mut ps = vcod_cod4::pmove::PlayerState::from_wire(&p.ps);
            if op.dflags & DFLAG_NO_KNOCKBACK == 0 {
                let dir = op.dir.map_or(Vec3::ZERO, Vec3::from);
                knock_back(&mut ps, op.damage, dir, g_knockback);
            }
            let blood = &mut self.combat.blood[op.slot];
            *blood = (blood.0 + op.damage, op.dir);
            let timer = ps.damage_timer + (op.damage as f32 * DAMAGE_TIMER_PER_POINT) as i32;
            let flinch = flinch_yaw(op.dir, ps.view_angles[1]);
            ps.damage_timer = timer.min(MAX_DAMAGE_TIMER);
            ps.damage_duration = ps.damage_timer;
            if op.fatal {
                ps.pm_type = vcod_cod4::pmove::pm_type::DEAD;
                if let Some(anim) = op.death {
                    (ps.legs_anim, ps.legs_timer) = (anim.legs_anim, anim.legs_timer);
                    (ps.torso_anim, ps.torso_timer) = (anim.torso_anim, anim.torso_timer);
                }
            }
            p.ps = ps.to_wire(&p.ps);
            p.ps.set("damageDuration", ps.damage_duration as u32);
            p.ps.set("flinchYawAnim", flinch);
            p.ps.stats[0] = if op.fatal { 0 } else { health.max(0) };
            if op.fatal {
                p.ps.set("weapon", 0);
                p.ps.stats[1] = op.dead_yaw.unwrap_or(ps.view_angles[1] as i32);
            }
        }
        for g in std::mem::take(&mut self.rt.host.cod4_mut().death_grenades) {
            self.sync_defs();
            let Some(d) = self.combat.defs.get(g.weapon as usize).cloned() else {
                continue;
            };
            self.fire_grenade(g.slot, g.spot, g.velocity, g.weapon, &d, true, g.time);
        }
    }

    /// `P_DamageFeedback` (0x80a5156) at `slot`'s end frame, for a body not
    /// dead (cod4-combat.md 4): `damageCount` clears once the last kick is
    /// 500 ms old; blood since the last frame becomes a count, its share
    /// of max health in per cent (at most 127), which widens the aim
    /// spread, steps `damageEvent`, names where it came from in 256ths of
    /// a turn (255 both ways from nowhere) and starts the damage kick the
    /// next cmds' aim blocks ease in (3.2). Then `currentAimSpreadScale`
    /// for the next frame's shots.
    pub(super) fn damage_feedback(&mut self, slot: usize) {
        let now = self.level_time();
        let max_health = self.rt.client_vitals(slot).max_health;
        let kick_age = now - self.combat.aim[slot].kick.time;
        let (blood, from) = self.combat.blood[slot];
        let Some(p) = self.players[slot].as_mut() else {
            return;
        };
        let ps = &mut p.ps;
        let hurt =
            (blood > 0 && max_health > 0).then(|| (100 * blood / max_health).min(MAX_DAMAGE_COUNT));
        match hurt {
            _ if ps.word("pm_type") >= PM_DEAD => {}
            Some(count) => {
                let [pitch, yaw] = damage_direction(from);
                ps.set("damageEvent", ps.word("damageEvent").wrapping_add(1));
                ps.set("damageCount", count as u32);
                ps.set("damagePitch", pitch);
                ps.set("damageYaw", yaw);
                let widened =
                    f64::from(f32::from_bits(ps.word("aimSpreadScale"))) + f64::from(count);
                ps.setf("aimSpreadScale", widened.min(255.0) as f32);
                let view =
                    std::array::from_fn(|k| f32::from_bits(ps.word(&format!("viewangles[{k}]"))));
                self.combat.blood[slot] = (0, None);
                self.combat.hit_dir[slot] = from;
                self.combat.aim[slot].kick = vcod_cod4::pmove::aim::DamageKick::feedback(
                    count,
                    from.map(Vec3::from),
                    view,
                    now,
                );
            }
            None if kick_age > DAMAGE_COUNT_MS => ps.set("damageCount", 0),
            None => {}
        }
        self.combat.aim_spread[slot] = f32::from_bits(ps.word("aimSpreadScale")) / 255.0;
    }

    /// The weapons `dropItem` launched: each runs its gravity arc from its
    /// drop and comes to rest where a trace down meets the world (the
    /// bounce `G_RunItem` gives a fall is left out, cod4-combat.md 6).
    pub(super) fn run_items(&mut self) {
        let now = self.level_time();
        let gravity = 800.0f32;
        let clip = self.clip.clone();
        let host = &mut self.rt.host;
        let mut items = std::mem::take(&mut host.cod4_mut().items);
        items.retain(|i| host.ents.get(i.id).is_some());
        for it in items.iter_mut() {
            if it.owner != ITEM_NO_OWNER && now >= it.clear_owner_at {
                it.owner = ITEM_NO_OWNER;
            }
            if it.resting {
                continue;
            }
            let t = (now - it.start_ms) as f32 * 0.001;
            let at = it.base + it.delta * t - Vec3::Z * (0.5 * gravity * t * t);
            let Some(c) = clip.as_deref() else { continue };
            let from = it.base + it.delta * (t - 0.05).max(0.0)
                - Vec3::Z * (0.5 * gravity * (t - 0.05).max(0.0).powi(2));
            let tr = c.trace(from, at, Vec3::splat(-1.0), Vec3::splat(1.0), 0x491);
            if tr.fraction < 1.0 {
                it.base = from + (at - from) * tr.fraction;
                it.delta = Vec3::ZERO;
                it.start_ms = now;
                it.resting = true;
            }
        }
        host.cod4_mut().items = items;
    }

    /// `Touch_Item` for the weapons at rest: a living player whose box
    /// meets the item's and who holds the same weapon takes its stock and
    /// clip into the reserve (`WeaponPickup_Touch`, `Add_Ammo`, capped at
    /// `maxAmmo`); one that took any gets `EV_AMMO_PICKUP` and the item
    /// goes. Picking up a weapon not held is the use key's swap, not run
    /// (cod4-combat.md 6).
    pub(super) fn touch_items(&mut self) {
        let items = self.rt.host.cod4().items.clone();
        let mut taken = Vec::new();
        for it in items.iter().filter(|i| i.resting) {
            let Some(d) = self.combat.defs.get(it.weapon as usize).cloned() else {
                continue;
            };
            for slot in 0..self.players.len() {
                if !self.playing(slot) || self.rt.client_vitals(slot).health < 1 {
                    continue;
                }
                let Some(mut ps) = self.pm_state(slot) else {
                    continue;
                };
                let lo = ps.origin + Vec3::new(-15.0, -15.0, 0.0);
                let hi = ps.origin + Vec3::new(15.0, 15.0, 70.0);
                let meets = (0..3).all(|a| it.base[a] + 1.0 >= lo[a] && it.base[a] - 1.0 <= hi[a]);
                if !meets || !ps.has_weapon(it.weapon) || d.ammo_index >= ps.ammo.len() {
                    continue;
                }
                let have = ps.ammo[d.ammo_index];
                let want = (it.stock.max(0) + it.clip.max(0)).max(0);
                let after = (have + want).min(d.max_ammo.max(have));
                if after == have {
                    continue;
                }
                ps.ammo[d.ammo_index] = after;
                ps.add_event(EV_AMMO_PICKUP, it.weapon);
                let p = self.players[slot].as_mut().unwrap();
                p.ps = ps.to_wire(&p.ps);
                let l = &mut self.rt.host.cod4_mut().clients[slot].loadout;
                l.ammo[d.ammo_index] = after;
                taken.push(it.id);
                break;
            }
        }
        if taken.is_empty() {
            return;
        }
        self.rt
            .host
            .cod4_mut()
            .items
            .retain(|i| !taken.contains(&i.id));
        for id in taken {
            self.rt.host.ents.free(id);
        }
    }

    /// Each live item as 1.7 sends it: `ET_ITEM` (3), `index` its weapon,
    /// `clientNum` its dropper, `pos` its arc (`TR_GRAVITY`, 5) or rest
    /// (`TR_STATIONARY`, 0) truncated.
    pub(super) fn item_entities(&self) -> Vec<EntityState> {
        let host = &self.rt.host;
        host.cod4()
            .items
            .iter()
            .filter(|i| host.ents.get(i.id).is_some())
            .map(|i| {
                let mut e = EntityState::default();
                e.0[0] = i.id.0;
                e.0[1] = 3;
                let table = EntityState::table(3);
                let mut set = |name: &str, v: u32| {
                    if let Some(f) = table.iter().find(|f| f.name == name) {
                        e.0[f.word()] = v;
                    }
                };
                set("index", i.weapon);
                set("clientNum", i.owner);
                set("lerp.pos.trType", if i.resting { 0 } else { 5 });
                set("lerp.pos.trTime", i.start_ms as u32);
                for k in 0..3 {
                    set(
                        &format!("lerp.pos.trBase[{k}]"),
                        i.base[k].trunc().to_bits(),
                    );
                    set(&format!("lerp.pos.trDelta[{k}]"), i.delta[k].to_bits());
                }
                set("lerp.apos.trBase[1]", i.yaw.to_bits());
                e
            })
            .collect()
    }

    /// The temp entities in 1.7's numbering (the host raises them with
    /// 1.7's event numbers already, `builtins::cod4_combat`): `eType` 17
    /// plus the event, each field into its word.
    pub(super) fn temp_entities4(&self) -> Vec<(EntityState, crate::game::temp_entity::Scope)> {
        self.rt
            .temp_entities()
            .map(|(n, te)| {
                // The shared `earthquake` raises CoD1's number; 1.7's is 61.
                let event = if te.quake.is_some() {
                    61
                } else {
                    te.event as u32
                };
                let etype = 17 + event;
                let mut e = EntityState::default();
                e.0[0] = n;
                e.0[1] = etype;
                let table = EntityState::table(etype);
                let mut set = |name: &str, v: u32| {
                    if let Some(f) = table.iter().find(|f| f.name == name) {
                        e.0[f.word()] = v;
                    }
                };
                set("eventParm", te.parm as u32);
                set("surfType", te.surf_type as u32);
                set("otherEntityNum", te.other);
                set("attackerEntityNum", te.attacker as u32);
                set("weapon", te.weapon as u32);
                set("clientNum", te.client_num as u32);
                // `un1`, the event tables' name for the byte at +212.
                set("un1", te.scale as u32);
                if let Some(q) = te.quake {
                    set("lerp.u.anonymous.data[0]", q.scale.to_bits());
                    set("lerp.u.anonymous.data[1]", q.radius.to_bits());
                    set("lerp.u.anonymous.data[2]", q.duration_ms as u32);
                }
                // `EV_PHYS_EXPLOSION_SPHERE` (57) and `_CYLINDER` (58): `physicsExplosion*`'s inner radius and magnitude
                // (`builtins::cod4_vehicle`), else `playFx`'s angles
                // (`builtins::cod4_script::play_fx`).
                if (event == 57 || event == 58)
                    && let Some([inner, magnitude, _]) = te.origin2
                {
                    set("lerp.u.anonymous.data[0]", inner.to_bits());
                    set("lerp.u.anonymous.data[1]", magnitude.to_bits());
                } else {
                    for (k, v) in te.origin2.iter().flatten().enumerate() {
                        set(&format!("lerp.apos.trBase[{k}]"), v.to_bits());
                    }
                }
                for (k, v) in te.origin.iter().enumerate() {
                    set(
                        &format!("lerp.pos.trBase[{k}]"),
                        (v.trunc() + 0.0).to_bits(),
                    );
                }
                (e, te.scope)
            })
            .collect()
    }

    /// Who may see each temp entity: the numbers `viewer`'s snapshot leaves
    /// out.
    pub fn hidden_from(&self, viewer: usize) -> Vec<u32> {
        self.rt
            .temp_entities()
            .filter(|(_, te)| !te.scope.admits(viewer))
            .map(|(n, _)| n)
            .collect()
    }

    /// The dropped weapons at rest, where they lie.
    pub fn items_at_rest(&self) -> Vec<Vec3> {
        self.rt
            .host
            .cod4()
            .items
            .iter()
            .filter(|i| i.resting)
            .map(|i| i.base)
            .collect()
    }

    /// [`Combat4::hits`].
    pub fn combat_hits(&self) -> Vec<(usize, usize, u8)> {
        self.combat.hits.clone()
    }

    /// [`Combat4::splashes`].
    pub fn combat_splashes(&self) -> &[(usize, i32)] {
        &self.combat.splashes
    }

    /// [`Combat4::shots`].
    pub fn combat_shots(&self) -> &[(usize, i32, Vec3, Vec3, Vec3)] {
        &self.combat.shots
    }

    /// `slot`'s playerstate as a pmove one.
    pub fn pm_state(&self, slot: usize) -> Option<vcod_cod4::pmove::PlayerState> {
        Some(vcod_cod4::pmove::PlayerState::from_wire(
            &self.players.get(slot)?.as_ref()?.ps,
        ))
    }

    /// The wire playerstate.
    pub fn wire_state(&self, slot: usize) -> Option<&PlayerState> {
        self.players.get(slot)?.as_ref().map(|p| &p.ps)
    }

    /// The unit direction of the blow behind `slot`'s last `damageEvent`
    /// step, from the attacker toward the body, or `None` for one with
    /// none. Not from `damagePitch`/`damageYaw`: a real direction can
    /// truncate to their 255 none (cod4-combat.md 4).
    pub fn hit_dir(&self, slot: usize) -> Option<Vec3> {
        Vec3::from(self.combat.hit_dir.get(slot).copied()??).try_normalize()
    }
}

/// `finishPlayerDamage`'s numbers (cod4-combat.md 4): `g_knockback`'s
/// default, the body's mass, the push's cap, `pm_time`'s bounds, and the
/// damage timer's ms a point and cap.
const DEFAULT_KNOCKBACK: f32 = 1000.0;
const PLAYER_MASS: f32 = 250.0;
const MAX_KNOCKBACK: i32 = 60;
const KNOCKBACK_TIME_MS: (i32, i32) = (50, 200);
/// `finishPlayerDamage`'s `flinchYawAnim` (`cod4_lnxded` 0x80ac7b5..0x80ac85a,
/// 0x80acdbd..0x80ace25): the blow's `vectoyaw` less the view yaw's whole
/// degrees (the view put in 0..360 first), put in 0..360: 0 from 315 or
/// under 45, 2 to 135, 1 to 225, 3 to 315 (rodata 0x8218dc0..0x8218dc8,
/// 0x820dec0); 0 for a blow with no direction (0x80acad4).
fn flinch_yaw(dir: Option<[f32; 3]>, view_yaw: f32) -> u32 {
    let Some(dir) = dir else {
        return 0;
    };
    let yaw = crate::game::builtins::math::vectoyaw(dir) as f32;
    let view = if view_yaw < 0.0 {
        (f64::from(view_yaw) + 360.0) as f32
    } else {
        view_yaw
    };
    let mut d = f64::from(yaw) - f64::from(view as i32);
    if d < 0.0 {
        d += 360.0;
    }
    match d {
        d if !(45.0..315.0).contains(&d) => 0,
        d if d < 135.0 => 2,
        d if d < 225.0 => 1,
        _ => 3,
    }
}

const DAMAGE_TIMER_PER_POINT: f32 = 100.0;
const MAX_DAMAGE_TIMER: i32 = 750;
/// `P_DamageFeedback`'s count cap and how long a count stands.
const MAX_DAMAGE_COUNT: i32 = 127;
const DAMAGE_COUNT_MS: i32 = 500;

/// The push a hit of `damage` from along `dir` gives a body with these
/// `pm_flags` and `eFlags`, added to `velocity`: a share of the damage by
/// stance (0.3 standing, 0.15 crouched, 0.02 prone), truncated and capped,
/// as speed scaled by `g_knockback` over the body's mass, each component
/// summed on the x87 stack and stored once; none on a turret
/// (`cod4_lnxded` 0x80aca21..0x80aca9b). The push, or `None` for none.
pub(crate) fn knockback(
    velocity: &mut [f32; 3],
    pm_flags: u32,
    e_flags: u32,
    damage: i32,
    dir: [f32; 3],
    g_knockback: f32,
) -> Option<i32> {
    use vcod_cod4::pmove::pmf;
    let share = match pm_flags {
        f if f & pmf::PRONE != 0 => 0.02f32,
        f if f & pmf::DUCKED != 0 => 0.15,
        _ => 0.3,
    };
    let push = ((damage as f32 * share) as i32).min(MAX_KNOCKBACK);
    if push == 0 || e_flags & vcod_cod4::pmove::ef::TURRET != 0 {
        return None;
    }
    let speed = f64::from(push) * f64::from(g_knockback) / f64::from(PLAYER_MASS);
    for k in 0..3 {
        velocity[k] = (f64::from(velocity[k]) + f64::from(dir[k]) * speed) as f32;
    }
    Some(push)
}

/// `g_knockback` as the host's cvars hold it.
pub(crate) fn g_knockback(cvars: &crate::cvars::Cvars) -> f32 {
    match cvars.get("g_knockback") {
        "" => DEFAULT_KNOCKBACK,
        v => crate::game::builtins::cvar::atof(v),
    }
}

/// [`knockback`] on `ps`. A push starts the knockback timer, twice the
/// push in ms within its bounds, unless a `pm_time` already runs
/// (cod4-combat.md 4).
fn knock_back(ps: &mut vcod_cod4::pmove::PlayerState, damage: i32, dir: Vec3, g_knockback: f32) {
    let mut v = ps.velocity.to_array();
    let Some(push) = knockback(
        &mut v,
        ps.pm_flags,
        ps.e_flags,
        damage,
        dir.to_array(),
        g_knockback,
    ) else {
        return;
    };
    ps.velocity = Vec3::from(v);
    if ps.pm_time == 0 {
        let (lo, hi) = KNOCKBACK_TIME_MS;
        ps.pm_time = (2 * push).clamp(lo, hi);
        ps.pm_flags |= vcod_cod4::pmove::pmf::TIME_KNOCKBACK;
    }
}

/// Where a hit came from as `damagePitch` and `damageYaw`: its angles in
/// 256ths of a turn, truncated, or 255 both ways for a hit from nowhere
/// (cod4-combat.md 4).
fn damage_direction(from: Option<[f32; 3]>) -> [u32; 2] {
    let Some(d) = from else {
        return [255; 2];
    };
    let angles = crate::game::builtins::math::vectoangles(d);
    [angles[0], angles[1]].map(|a| (f64::from(a) / 360.0 * 256.0) as i32 as u32)
}

#[cfg(test)]
impl Cod4Level {
    /// `slot`'s melee swing at the level's time; for tests.
    pub fn fire_melee_for_test(&mut self, slot: usize) {
        let now = self.level_time();
        self.fire_melee(slot, now);
    }

    /// Applies the queued [`Damage4`]s now; for tests.
    pub fn apply_damage_ops_for_test(&mut self) {
        self.apply_damage_ops();
    }

    /// `slot`'s end-frame damage feedback now; for tests.
    pub fn damage_feedback_for_test(&mut self, slot: usize) {
        self.damage_feedback(slot);
    }
}

/// `Bullet_GetDamage`: full damage inside `maxDamageRange`,
/// `minDamage` past `minDamageRange`, lerped between.
pub(super) fn bullet_damage(d: &WeaponDef, dist: f32) -> i32 {
    let range = d.min_damage_range - d.max_damage_range;
    if d.damage == d.min_damage || range == 0.0 || dist < d.max_damage_range {
        return d.damage;
    }
    if dist >= d.min_damage_range {
        return d.min_damage;
    }
    let lerp = (dist - d.max_damage_range) / range;
    (f64::from(lerp) * f64::from(d.min_damage) + (1.0 - f64::from(lerp)) * f64::from(d.damage))
        as f32 as i32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_disc_draw_stays_in_the_unit_disc() {
        for t in [1, 47950, 48000, 123_456] {
            let (x, y) = random_dir(t);
            assert!(x * x + y * y <= 1.0, "{t}: {x} {y}");
        }
    }

    #[test]
    fn a_segment_meets_a_box_only_inside_its_limit() {
        let (lo, hi) = (Vec3::splat(-1.0), Vec3::splat(1.0));
        let (a, b) = (Vec3::new(-10.0, 0.0, 0.0), Vec3::new(10.0, 0.0, 0.0));
        assert!(segment_meets_box(a, b, lo, hi, 1.0));
        assert!(!segment_meets_box(a, b, lo, hi, 0.4));
    }
}
