//! CoD2's mounted guns on a [`Cod2Level`] (docs/research/cod2-turrets.md):
//! the mount (`turret_use`, reached through the use key's one path in
//! `game::cod2_use`), the gunner's frame in
//! `ClientEndFrame` (aim, body placement, fire, loop sound) and the
//! release (`G_ClientStopUsingTurret`). The record each gun keeps is the
//! shared [`TurretRecord`] `G_SpawnTurret` built; an unmanned gun's walk
//! home stays the shared entity pass's (`GameHost::run_turret_think`).

use std::collections::HashMap;
use std::rc::Rc;

use glam::Vec3;
use vcod_cod2::pmove::{PlayerState, ef, ev, pmf};
use vcod_common::pmove::aim::angle_subtract;
use vcod_common::xanim::XAnim;
use vcod_gsc::EntId;

use super::cod2::Cod2Level;
use super::turret::{TurretRecord, TurretStance};

/// `ENTITYNUM_NONE`.
const NONE: u32 = 1023;
/// One server frame of the gun's cooldown and loop timers (lnxded
/// 0x810b850, 0x810b926: `-0x32`). VERIFIED.
const FRAME_MS: i32 = 50;
/// The placement's trace: from the eye height down to 60 under the placed
/// origin (rodata 0x815ac4c), mask 0x2810011 (lnxded 0x810aa50). VERIFIED.
const PLACE_DROP: f32 = 60.0;

/// A gun as the bots read it ([`Cod2Level::gun_list`]).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GunInfo {
    pub id: EntId,
    pub origin: Vec3,
    pub yaw: f32,
    /// [min, max] off the gun's angles: pitch [-top, bottom], yaw [-right,
    /// left] (`TurretRecord`).
    pub yaw_range: [f32; 2],
    pub pitch_range: [f32; 2],
    pub owner: Option<usize>,
}

/// What a client's turret use keeps beside its playerstate.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Gunner {
    /// The player entity's busy byte (`ent+0x162`): 1 on a gun, 2 a
    /// release asked for by a use press.
    pub busy: u8,
    /// `gclient+0x2830`: the entity a use press picked off the hint, which
    /// the held key uses.
    pub use_target: Option<EntId>,
    /// `gclient+0x2834`: the level time of the press that set the target.
    pub use_time: i32,
    /// `ps.viewlocked` (ps+0x590) and `ps.viewlocked_entNum` (ps+0x594).
    pub viewlocked: i32,
    pub viewlocked_ent: u32,
    /// `ps.cursorHint`, `ps.cursorHintString` and `cursorHintEntIndex`
    /// (ps+0x598, 0x59c, 0x5a0), as the last end frame's hint pass left them.
    pub hint: i32,
    pub hint_string: i32,
    pub hint_ent: Option<EntId>,
}

impl Default for Gunner {
    fn default() -> Self {
        Gunner {
            busy: 0,
            use_target: None,
            use_time: 0,
            viewlocked: 0,
            viewlocked_ent: NONE,
            hint: 0,
            hint_string: -1,
            hint_ent: None,
        }
    }
}

/// A gun's weapon-file keys the CoD2 frame reads that the shared
/// [`TurretRecord`] does not keep.
#[derive(Clone, Copy, Debug)]
pub(super) struct GunKeys {
    /// The turret weapon's level index (`s.weapon`).
    pub(super) weapon: u8,
    /// `fireTime` (def+0x204), stored truncated.
    fire_time: i32,
    /// `playerSpread` (def+0x540, rec+0x38): the round's spread, degrees.
    spread: f32,
    /// `useHintString` and `dropHintString`'s hint-string indices
    /// (def+0x574, def+0x578), -1 for none.
    pub(super) use_hint: i32,
    pub(super) drop_hint: i32,
}

/// The level-wide turret state: each gun's keys and the gunner anims'
/// clips, loaded once.
#[derive(Default)]
pub struct Turrets {
    keys: HashMap<EntId, GunKeys>,
    clips: HashMap<String, Option<Rc<XAnim>>>,
}

/// `G_GetHintStringIndex` (lnxded 0x8112432): the slot of the 32
/// hint-string configstrings holding `name`, or the first empty one,
/// written; -1 when they are full. VERIFIED.
fn hint_string_index(cs: &mut crate::configstrings::ConfigStrings, name: &str) -> i32 {
    let base = vcod_cod2::net::protocol::PROTOCOL_118.cs.hint_strings;
    for i in 0..32 {
        let slot = &cs[base + i];
        if slot == name {
            return i as i32;
        }
        if slot.is_empty() {
            cs.set(base + i, name.to_string());
            return i as i32;
        }
    }
    -1
}

impl Cod2Level {
    /// The CoD2 keys of the gun `id`, read from its weapon file the first
    /// time it is asked for. `G_RegisterWeapon` (lnxded 0x8107216)
    /// registers the use hint, then the drop hint.
    pub(super) fn gun_keys(&mut self, id: EntId) -> Option<GunKeys> {
        if let Some(k) = self.turrets.keys.get(&id) {
            return Some(*k);
        }
        let name = self.rt.host.turrets.get(&id)?.weapon.clone();
        let fs = self.rt.host.fs.clone()?;
        let def = vcod_cod2::weapon::load(&*fs, &name).ok()?.def;
        let weapon = crate::game::builtins::cod2::register_weapon(&mut self.rt.host, &name)?;
        let cs = &mut self.rt.host.configstrings;
        let mut hint = |key: &str| match def.str(key) {
            "" => -1,
            s => hint_string_index(cs, s),
        };
        let use_hint = hint("useHintString");
        let drop_hint = hint("dropHintString");
        let keys = GunKeys {
            weapon: weapon as u8,
            fire_time: def.int("fireTime"),
            spread: def.float("playerSpread").max(0.0),
            use_hint,
            drop_hint,
        };
        self.turrets.keys.insert(id, keys);
        Some(keys)
    }

    /// The gun's origin and angles as its entity holds them.
    fn gun_pose(&mut self, id: EntId) -> Option<([f32; 3], [f32; 3])> {
        self.rt.link_anchor(id)
    }

    /// Every gun with its pose, arcs and gunner, for the bots
    /// (`bots::cod2::gun`).
    pub fn gun_list(&mut self) -> Vec<GunInfo> {
        let ids: Vec<EntId> = self.rt.host.turrets.keys().copied().collect();
        ids.into_iter()
            .filter_map(|id| {
                let (origin, angles) = self.gun_pose(id)?;
                let rec = self.rt.host.turrets.get(&id)?;
                Some(GunInfo {
                    id,
                    origin: Vec3::from(origin),
                    yaw: angles[1],
                    yaw_range: rec.yaw_range,
                    pitch_range: rec.pitch_range,
                    owner: rec.owner,
                })
            })
            .collect()
    }

    /// The gun `slot` mans, if any.
    pub fn manned_gun(&self, slot: usize) -> Option<EntId> {
        self.rt
            .host
            .turrets
            .iter()
            .find(|(_, r)| r.owner == Some(slot))
            .map(|(id, _)| *id)
    }

    /// `G_IsTurretUsable` (lnxded 0x810c358) with 0x810c1be's arc test:
    /// the shared rule (cod11-turrets.md 4.2) reads the same on CoD2.
    pub(super) fn turret_usable(&mut self, slot: usize, id: EntId) -> bool {
        let Some((origin, angles)) = self.gun_pose(id) else {
            return false;
        };
        let (Some(p), Some(rec)) = (
            self.players.get(slot).and_then(Option::as_ref),
            self.rt.host.turrets.get(&id),
        ) else {
            return false;
        };
        rec.owner.is_none()
            && super::turret::usable(
                rec,
                origin,
                angles[1],
                p.ps.origin.to_array(),
                p.ws.grenade_time_left,
                p.ps.ground_entity_num != NONE as i32,
            )
    }

    /// `turret_use` (lnxded 0x810c3de): both busy bytes 1, the gun owned,
    /// the view locked to it, the mount spot and stance kept, the gun's
    /// stance in `eFlags` 0x300, the barrel put where the player looks
    /// inside the arcs, and the view clamp centred on the arcs. No view is
    /// set. VERIFIED, the stores.
    pub(super) fn mount_turret(&mut self, slot: usize, id: EntId) {
        let Some((_, angles)) = self.gun_pose(id) else {
            return;
        };
        // The gun's hint strings, which `G_RegisterWeapon` registered.
        self.gun_keys(id);
        let (Some(p), Some(rec)) = (
            self.players.get_mut(slot).and_then(Option::as_mut),
            self.rt.host.turrets.get_mut(&id),
        ) else {
            return;
        };
        p.gunner.busy = 1;
        rec.busy = 1;
        rec.owner = Some(slot);
        p.gunner.viewlocked = 1;
        p.gunner.viewlocked_ent = id.0;
        rec.fresh_mount = true;
        rec.mount_origin = p.ps.origin.to_array();
        use vcod_common::pmove::Stance as S;
        rec.mount_stance = Some(if p.ps.pm_flags & pmf::PRONE != 0 {
            S::Prone
        } else if p.ps.pm_flags & pmf::DUCKED != 0 {
            S::Crouch
        } else {
            S::Stand
        });
        let ps = &mut p.ps;
        ps.e_flags = match rec.def.stance {
            TurretStance::Prone => (ps.e_flags | 0x100) & !0x200,
            TurretStance::Duck => (ps.e_flags | 0x200) & !0x100,
            TurretStance::Stand => ps.e_flags | 0x300,
        };
        aim_barrel(rec, ps, angles);
        let ranges = [rec.pitch_range, rec.yaw_range];
        for i in 0..2 {
            let [lo, hi] = ranges[i];
            let half = (0.5f64 * f64::from(angle_subtract(hi, lo))) as f32;
            ps.view_angle_clamp_range[i] = half;
            ps.view_angle_clamp_base[i] = normalize360(angles[i] + hi - half);
        }
    }

    /// `ClientEndFrame`'s turret work for every client, in slot order: the
    /// cursor hint (`Player_UpdateCursorHints`, lnxded 0x8121bc6), then for a playing
    /// gunner `turret_think_client` (0x810bb0a). Runs after the frame's
    /// `pm_flags` 0x800000 is set.
    pub fn turret_end_frame(&mut self) {
        // A gunner gone or respawned off the gun lets it go (`ClientSpawn`
        // and the entity's free call `G_ClientStopUsingTurret`, lnxded
        // 0x80f910e). INFERRED, which of the two vcod's respawn stands for.
        let stale: Vec<EntId> = (self.rt.host.turrets.iter())
            .filter_map(|(id, r)| {
                let p = self.players.get(r.owner?).and_then(Option::as_ref);
                p.is_none_or(|p| p.ps.e_flags & ef::TURRET == 0)
                    .then_some(*id)
            })
            .collect();
        for id in stale {
            self.stop_using_turret(id);
        }
        for slot in 0..self.players.len() {
            if self.players[slot].is_none() {
                continue;
            }
            self.update_cursor_hints(slot);
            let p = self.players[slot].as_ref().unwrap();
            let gunner =
                p.ps.pm_flags & super::cod2::PMF_PLAYING != 0 && p.ps.e_flags & ef::TURRET != 0;
            if gunner && let Some(id) = self.manned_gun(slot) {
                self.turret_think_client(slot, id);
            }
        }
    }

    /// `turret_think_client` (lnxded 0x810bb0a): a gunner still on the gun
    /// (busy byte 1) and playing gets the mounted frame and the loop
    /// sound, anyone else the release.
    fn turret_think_client(&mut self, slot: usize, id: EntId) {
        let busy = self.players[slot].as_ref().map_or(0, |p| p.gunner.busy);
        let playing = self.rt.client_field(slot, "sessionstate").as_deref() == Some("playing");
        if busy == 1 && playing {
            self.manned_frame(slot, id);
            self.gun_loop_sound(id);
        } else {
            self.stop_using_turret(id);
        }
    }

    /// The mounted frame (lnxded 0x810b850): the aim (0x810b6ce), the body
    /// put on the gun (0x810aa50), `viewlocked` 1, the firing bit cleared,
    /// the cooldown down a frame, and at 0 with the attack held on the
    /// frame's last cmd a round (0x810b7e0): the loop timer at three
    /// `fireTime`s, the bullet, `EV_FIRE_WEAPON_MG42` on the gun with the
    /// gunner's number and `viewlocked` 2. VERIFIED, the stores and their
    /// order.
    fn manned_frame(&mut self, slot: usize, id: EntId) {
        let Some((origin, angles)) = self.gun_pose(id) else {
            return;
        };
        let Some(keys) = self.gun_keys(id) else {
            return;
        };
        // The placement reads `tag_weapon` off the gun's pose as the frame
        // built it before the aim: the barrel a frame behind (section 7).
        let posed = {
            let (Some(p), Some(rec)) = (
                self.players[slot].as_mut(),
                self.rt.host.turrets.get_mut(&id),
            ) else {
                return;
            };
            p.gunner.viewlocked = 1;
            p.gunner.viewlocked_ent = id.0;
            let posed = rec.angles2;
            aim_barrel(rec, &p.ps, angles);
            if std::mem::take(&mut rec.fresh_mount) {
                rec.teleport_bit = !rec.teleport_bit;
            }
            posed
        };
        self.place_gunner(slot, id, origin, angles, posed);
        let p = self.players[slot].as_mut().unwrap();
        let rec = self.rt.host.turrets.get_mut(&id).unwrap();
        p.gunner.viewlocked = 1;
        rec.firing = false;
        rec.cooldown_ms -= FRAME_MS;
        if rec.cooldown_ms >= 1 {
            return;
        }
        rec.cooldown_ms = 0;
        if p.oldcmd.buttons & vcod_cod2::pmove::button::ATTACK == 0 {
            return;
        }
        rec.cooldown_ms = keys.fire_time;
        rec.loop_left_ms = 3 * keys.fire_time;
        p.gunner.viewlocked = 2;
        rec.firing = true;
        let view = p.ps.view_angles;
        // `G_Damage` reads the weapon off the inflictor, the gunner, whose
        // `s.weapon` is the weapon in his hands (lnxded 0x8101b40): its
        // `loc*` multipliers and its name reach the damage callback. The
        // turret's file has no `loc*` keys, all 0 (section 11). VERIFIED.
        let held = p.ws.weapon;
        let tags = rec.tags;
        let angles2 = rec.angles2;
        let [f, r, u] = vcod_common::x87::angle_vectors(Vec3::from_array(view));
        let Some(tags) = tags else {
            return;
        };
        // `Turret_FillWeaponParms` (0x810a880): the eye is the gun's
        // `tag_player` (`G_GetPlayerViewOrigin`'s gunner arm, 0x80f8916),
        // the muzzle that far along the view as `tag_flash` is from it.
        let muzzle = Vec3::from(super::turret::muzzle(
            &tags,
            origin,
            angles,
            angles2,
            f.to_array(),
        ));
        if let Some(w) = self
            .combat
            .weapons
            .get(usize::from(keys.weapon))
            .cloned()
            .flatten()
        {
            let now = self.level_time();
            self.bullet_fire(slot, id.0, &w, held, [f, r, u], muzzle, keys.spread, now);
        }
        if let Some(ent) = self.rt.host.ents.get_mut(id) {
            ent.events.add(
                vcod_common::net::event_ids::EV_FIRE_WEAPON_MG42,
                slot as i32,
            );
        }
    }

    /// 0x810aa50: the gunner's anim group blended by the barrel's yaw and
    /// the body's height (`vcod_common::turretpose::place_gunner`), hung
    /// off `tag_weapon`, then a trace from the eye's height to 60 under it
    /// sets the body's height. Skipped with no tags, no anims or a group
    /// that will not load, as retail skips a missing `tag_weapon`.
    fn place_gunner(
        &mut self,
        slot: usize,
        id: EntId,
        origin: [f32; 3],
        angles: [f32; 3],
        angles2: [f32; 3],
    ) {
        let Some(fs) = self.rt.host.fs.clone() else {
            return;
        };
        let Some(rec) = self.rt.host.turrets.get(&id) else {
            return;
        };
        let (Some(tags), Some(p)) = (rec.tags, self.players[slot].as_ref()) else {
            return;
        };
        let Some(anims) = self.combat.player_anims() else {
            return;
        };
        // The legs anim pmove's script picked (`mounted mg42`, its `firing`
        // clause), the client info's copy 0x810aa50 reads; `place_gunner`
        // refuses one that is not a `turretanim` group.
        let legs = p.ps.legs_anim;
        let clips = &mut self.turrets.clips;
        let placed = vcod_common::turretpose::place_gunner(
            anims,
            |n| {
                clips
                    .entry(n.to_string())
                    .or_insert_with(|| vcod_common::xanim::load(&*fs, n).ok().map(Rc::new))
                    .clone()
            },
            legs,
            super::turret::tag_weapon_local(&tags, angles2),
            (
                Vec3::from(origin),
                vcod_common::turretpose::angles_quat(angles),
            ),
            p.ps.origin,
            rec.def.anim_hor_rotate_inc,
        );
        let Some(g) = placed else {
            return;
        };
        let mut at = g.origin;
        let start = Vec3::new(at.x, at.y, at.z + p.ps.view_height_current);
        let end = Vec3::new(at.x, at.y, at.z - PLACE_DROP);
        let tr = self.world.collision.point_trace(
            start,
            end,
            vcod_common::collision::MASK_PLAYERSOLID,
            false,
        );
        if tr.fraction < 1.0 {
            at.z = tr.endpos.z;
        }
        let p = self.players[slot].as_mut().unwrap();
        p.ps.origin = at;
        self.rt.set_client_origin(slot, at.to_array());
        self.rt.set_client_yaw(slot, g.yaw());
    }

    /// The gun's loop sound (lnxded 0x810b926): its alias while the loop
    /// timer runs, a frame off the timer each call, and the stop alias
    /// with the loop cut on the frame it runs out. VERIFIED.
    fn gun_loop_sound(&mut self, id: EntId) {
        let host = &mut self.rt.host;
        let Some(rec) = host.turrets.get_mut(&id) else {
            return;
        };
        let (loop_index, stop_index) =
            super::turret::sound_indices(rec, &host.configstrings, host.allocators.layout());
        let mut loop_sound = 0;
        let mut stop = false;
        if rec.loop_left_ms > 0 {
            loop_sound = loop_index;
            rec.loop_left_ms -= FRAME_MS;
            if rec.loop_left_ms < 1 && stop_index != 0 {
                loop_sound = 0;
                stop = true;
            }
        }
        if let Some(ent) = host.ents.get_mut(id) {
            ent.loop_sound = loop_sound;
            if stop {
                ent.events
                    .add(vcod_common::net::event_ids::EV_SOUND_ALIAS, stop_index);
            }
        }
    }

    /// `G_ClientStopUsingTurret` (lnxded 0x810b9a4): the loop cut, the
    /// stance the gunner mounted from as `EV_STANCE_FORCE_*` on him,
    /// `TeleportPlayer` back to the mount spot facing his entity's angles,
    /// the gun's stance and the view lock cleared, both busy bytes 0 and
    /// the gun free. VERIFIED, the stores.
    pub fn stop_using_turret(&mut self, id: EntId) {
        let Some(rec) = self.rt.host.turrets.get_mut(&id) else {
            return;
        };
        let Some(slot) = rec.owner.take() else {
            return;
        };
        rec.busy = 0;
        rec.loop_left_ms = 0;
        // The gun's `EF_FIRING` is left for its next think to clear
        // (cod2-turrets.md 14.1).
        rec.fresh_mount = false;
        let (origin, stance) = (rec.mount_origin, rec.mount_stance.take());
        if let Some(ent) = self.rt.host.ents.get_mut(id) {
            ent.loop_sound = 0;
        }
        let Some(p) = self.players.get_mut(slot).and_then(Option::as_mut) else {
            return;
        };
        use vcod_common::pmove::Stance as S;
        if let Some(s) = stance {
            let e = match s {
                S::Prone => ev::STANCE_FORCE_PRONE,
                S::Crouch => ev::STANCE_FORCE_CROUCH,
                S::Stand => ev::STANCE_FORCE_STAND,
            };
            p.ps.add_event(e, 0);
        }
        // `TeleportPlayer` (0x810a708): a unit up, the teleport bit, the
        // view at the entity's angles, which each think leaves at (0, the
        // view's yaw, 0).
        p.ps.origin = Vec3::from(origin) + Vec3::Z;
        p.ps.e_flags ^= vcod_cod2::net::flags::EF_TELEPORT_BIT as u32;
        let view = [0.0, p.ps.view_angles[1], 0.0];
        super::cod2::set_view_angles(&mut p.ps, &p.oldcmd, view);
        p.ps.e_flags &= !ef::TURRET;
        p.gunner.viewlocked = 0;
        p.gunner.viewlocked_ent = NONE;
        p.gunner.busy = 0;
        let at = p.ps.origin.to_array();
        self.rt.set_client_origin(slot, at);
    }
}

/// The aim's barrel (lnxded 0x810b6ce, and `turret_use`'s copy): the view
/// off the gun's angles, each axis clamped to its arc, no roll. VERIFIED.
fn aim_barrel(rec: &mut TurretRecord, ps: &PlayerState, angles: [f32; 3]) {
    let ranges = [rec.pitch_range, rec.yaw_range];
    for i in 0..2 {
        let [lo, hi] = ranges[i];
        // 0x810ccf2: two `fsel`s, the high bound first.
        let a = angle_subtract(ps.view_angles[i], angles[i]);
        let a = if a - hi >= 0.0 { hi } else { a };
        rec.angles2[i] = if lo - a >= 0.0 { lo } else { a };
    }
    rec.angles2[2] = 0.0;
}

/// `AngleNormalize360Accurate`.
fn normalize360(mut a: f32) -> f32 {
    while a < 0.0 {
        a += 360.0;
    }
    while a >= 360.0 {
        a -= 360.0;
    }
    a
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_hint_strings_take_the_first_free_slots_in_order() {
        let mut cs = crate::configstrings::ConfigStrings::new(vec![
            String::new();
            vcod_cod2::net::ops::MAX_CONFIGSTRINGS
        ]);
        assert_eq!(hint_string_index(&mut cs, "PLATFORM_HOLD_TO_USE"), 0);
        assert_eq!(hint_string_index(&mut cs, "PLATFORM_HOLD_TO_DROP"), 1);
        assert_eq!(hint_string_index(&mut cs, "PLATFORM_HOLD_TO_USE"), 0);
    }
}
