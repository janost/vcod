//! CoD4 1.7's mounted guns on a [`Cod4Level`]: the stock maps'
//! `misc_turret` SAWs (docs/research/cod4-gsc.md 9.2). The mount
//! (`turret_use`, reached through the use key, `super::use_key`), the
//! gunner's frame in `ClientEndFrame` (`turret_think_client`: aim, fire,
//! release) and the release (`G_ClientStopUsingTurret`). The record each
//! gun keeps is the shared [`TurretRecord`] `G_SpawnTurret` built; an
//! unmanned gun's walk home is the shared entity pass's
//! (`GameHost::run_turret_think`). Names and order are
//! INFERRED; the values the capture shows are VERIFIED
//! live there.

use std::collections::BTreeMap;

use glam::Vec3;
use vcod_cod4::net::delta::{ENTITYNUM_NONE, EntityState};
use vcod_cod4::pmove::{self, ef, pmf};
use vcod_gsc::{EntId, Value};

use super::level::{Cod4Level, PsExt};
use crate::game::turret::{TurretRecord, TurretStance};
use vcod_common::pmove::Stance;

/// One server frame off the fire and loop timers (`turret_track`'s
/// `fireTime -= 50`).
const FRAME_MS: i32 = 50;
/// `ET_MG42`.
pub const ET_MG42: u32 = 11;
/// `EV_FIRE_WEAPON_MG42` in 1.7's numbering (`net::events`; the capture's
/// gun rings 38 a round).
const EV_FIRE_WEAPON_MG42: u32 = 38;
/// `EV_STANCE_FORCE_STAND`, `_CROUCH`, `_PRONE` (1.7's 6, 7, 8).
const EV_STANCE_FORCE_STAND: u32 = 6;
const EV_STANCE_FORCE_CROUCH: u32 = 7;
const EV_STANCE_FORCE_PRONE: u32 = 8;
/// `PLAYERVIEWLOCK_FULL` and `_WEAPONJITTER`.
const VIEWLOCK_FULL: u32 = 1;
const VIEWLOCK_JITTER: u32 = 2;
/// `WEAPON_FIRETYPE_SINGLESHOT`.
const FIRETYPE_SINGLESHOT: i32 = 1;
/// 1 / 360 as the double 1.7's angle helpers multiply by.
const INV_360: f64 = 0.002777777845039964;

/// A gun as the use key and the bots read it.
#[derive(Clone, Copy, Debug)]
pub struct Gun4 {
    pub origin: Vec3,
    pub angles: [f32; 3],
}

/// `AngleDelta(a, b)` left on the x87 stack: the turns' fraction off
/// `floor(x + 0.5)`, never stored.
fn angle_delta_kept(a: f32, b: f32) -> f64 {
    let t = (f64::from(a) - f64::from(b)) * INV_360;
    (t - (t + 0.5).floor()) * 360.0
}

/// `AngleDelta` stored, as the aim's clamp reads it.
fn angle_delta(a: f32, b: f32) -> f32 {
    let t = ((f64::from(a) - f64::from(b)) as f32 as f64 * INV_360) as f32;
    let fl = (f64::from(t) + 0.5).floor() as f32;
    ((f64::from(t) - f64::from(fl)) * 360.0) as f32
}

/// `turret_use`'s `AngleNormalize360` of the clamp's base: the turns'
/// fraction off `floor`, with no second subtraction, so a base a hair
/// under 0 reads 360 (the capture's yaw base 360 on a gun at yaw 0).
fn normalize360_once(a: f64) -> f32 {
    let t = a * INV_360;
    ((t - t.floor()) * 360.0) as f32
}

/// One axis of a gun's reach, degrees off its own angle (pitch is
/// `pitch_range`, yaw `yaw_range`, cod4-gsc.md 9.2).
#[derive(Clone, Copy)]
struct Arc {
    low: f32,
    high: f32,
}

impl Arc {
    /// The gun's pitch and yaw arcs.
    fn pair(rec: &TurretRecord) -> [Arc; 2] {
        [rec.pitch_range, rec.yaw_range].map(|[low, high]| Arc { low, high })
    }

    /// The barrel's offset for a view at `look` on a gun facing `rest`: the
    /// view's stored delta pinned to the arc, the high edge tested first
    /// (the capture's yaw 45 for a view 70 past, cod4-gsc.md 9.2).
    fn pin(self, look: f32, rest: f32) -> f32 {
        match angle_delta(look, rest) {
            off if off >= self.high => self.high.max(self.low),
            off if off <= self.low => self.low,
            off => off,
        }
    }

    /// `viewAngleClampBase` and `viewAngleClampRange` for this axis: half
    /// the arc's span either side of its middle, the base turned once into
    /// 0..360 (the capture's yaw 360 and 45, pitch 7.4999995 and 22.5).
    fn view_clamp(self, rest: f32) -> (f32, f32) {
        let half = angle_delta_kept(self.high, self.low) * 0.5;
        let middle = f64::from(rest) + f64::from(self.high) - half;
        (normalize360_once(middle), half as f32)
    }
}

/// The barrel's angles for `view` on a gun at `gun`: pitch and yaw pinned
/// to the arcs, never a roll (`turret_clientaim`).
fn barrel_for(rec: &TurretRecord, view: [f32; 3], gun: [f32; 3]) -> [f32; 3] {
    let [pitch, yaw] = Arc::pair(rec);
    [pitch.pin(view[0], gun[0]), yaw.pin(view[1], gun[1]), 0.0]
}

/// A playerstate vector field `name[0..3]` as floats.
fn ps_vec(ps: &vcod_cod4::net::delta::PlayerState, name: &str) -> [f32; 3] {
    [0, 1, 2].map(|k| f32::from_bits(ps.word(&format!("{name}[{k}]"))))
}

/// `viewlocked` and the entity it is locked on.
fn lock_view(ps: &mut vcod_cod4::net::delta::PlayerState, mode: u32, on: u32) {
    ps.set("viewlocked", mode);
    ps.set("viewlocked_entNum", on);
}

/// The stance a body stands in by its `pm_flags`, prone before crouched.
fn stance_of(pm_flags: u32) -> Stance {
    if pm_flags & pmf::PRONE != 0 {
        Stance::Prone
    } else if pm_flags & pmf::DUCKED != 0 {
        Stance::Crouch
    } else {
        Stance::Stand
    }
}

/// The gunner's `eFlags` stance bits for the gun's stance: 0x100 prone,
/// 0x200 crouched (the capture's crouch SAW), both standing.
fn gun_stance_bits(stance: TurretStance) -> u32 {
    match stance {
        TurretStance::Prone => 0x100,
        TurretStance::Duck => 0x200,
        TurretStance::Stand => ef::TURRET,
    }
}

/// What a release hands back for the stance mounted from: its `pm_flags`
/// bit, the eye's target height and the `EV_STANCE_FORCE_*` event (the
/// release capture's `EV_STANCE_FORCE_STAND`, cod4-gsc.md 9.2).
fn stance_restore(stance: Stance) -> (u32, i32, u32) {
    match stance {
        Stance::Prone => (pmf::PRONE, 11, EV_STANCE_FORCE_PRONE),
        Stance::Crouch => (pmf::DUCKED, 40, EV_STANCE_FORCE_CROUCH),
        Stance::Stand => (0, 60, EV_STANCE_FORCE_STAND),
    }
}

/// One frame of the gun's fire timer: whether a round leaves. The timer
/// runs down a frame and holds at 0; a held trigger then fires, a
/// single-shot gun only on a fresh press (cod4-gsc.md 9.2: the SAW's
/// `fireTime` 50 rings a round every frame the trigger is held).
fn round_due(timer: &mut i32, latched: &mut bool, held: bool, single_shot: bool) -> bool {
    let left = *timer - FRAME_MS;
    *timer = left.max(0);
    if left > 0 {
        return false;
    }
    if !held {
        *latched = false;
        return false;
    }
    let repeat = single_shot && *latched;
    *latched = true;
    !repeat
}

impl Cod4Level {
    /// The gun's place as its entity holds it.
    pub(super) fn gun(&mut self, id: EntId) -> Option<Gun4> {
        let (o, a) = self.rt.link_anchor(id)?;
        Some(Gun4 {
            origin: Vec3::from(o),
            angles: a,
        })
    }

    /// The turret weapon's number in the level's list and its defs.
    fn gun_weapon(&self, id: EntId) -> Option<(u32, vcod_cod4::pmove::weapon::WeaponDef)> {
        let rec = self.rt.host.turrets.get(&id)?;
        let w = self.rt.host.cod4().table.index(&rec.weapon)?;
        Some((w as u32, self.combat.defs.get(w)?.clone()))
    }

    /// The gun's weapon file (its hint slots, its spread).
    fn gun_file(&self, id: EntId) -> Option<std::rc::Rc<super::WeaponFile>> {
        let rec = self.rt.host.turrets.get(&id)?;
        self.rt.host.cod4().weapon_files.get(&rec.weapon).cloned()
    }

    /// The hint-string slot of `name`, -1 for none (the hint index,
    /// `dropHintStringIndex`, taken at registration).
    fn hint_slot(&self, name: Option<&str>) -> i32 {
        let Some(name) = name else { return -1 };
        let base = self.rt.host.allocators.layout().hint_strings;
        (0..32)
            .find(|&i| self.rt.host.configstrings[base + i] == name)
            .map_or(-1, |i| i as i32)
    }

    /// `G_IsTurretUsable`: nobody on it, the player behind it inside its
    /// yaw arcs (`turret_behind`, the shared rule), no grenade in hand and
    /// on the ground.
    pub(super) fn turret_usable(&mut self, slot: usize, id: EntId) -> bool {
        let Some(g) = self.gun(id) else {
            return false;
        };
        let (Some(ps), Some(rec)) = (self.pm_state(slot), self.rt.host.turrets.get(&id)) else {
            return false;
        };
        rec.owner.is_none()
            && crate::game::turret::usable(
                rec,
                g.origin.to_array(),
                g.angles[1],
                ps.origin.to_array(),
                ps.grenade_time_left,
                ps.ground_entity_num != ENTITYNUM_NONE as i32,
            )
    }

    /// The cursor hint a usable gun shows (`Player_UpdateCursorHints`'s
    /// `ET_MG42` arm): its weapon plus 4 and its use hint's slot (the
    /// capture's SAW, weapon 8: 12 and 0, VERIFIED live).
    pub(super) fn turret_hint(&mut self, slot: usize, id: EntId) -> Option<(u32, i32)> {
        if !self.turret_usable(slot, id) {
            return None;
        }
        let (w, _) = self.gun_weapon(id)?;
        let file = self.gun_file(id);
        Some((
            w + 4,
            self.hint_slot(file.and_then(|f| f.use_hint.clone()).as_deref()),
        ))
    }

    /// `turret_use`: the player takes the gun. The gun is owned and busy,
    /// the view locked on it, where he stood and how kept for the release,
    /// his `eFlags` carry the gun's stance, the barrel turns to his view
    /// inside the arcs and the view clamp sits on the arcs' middle. The
    /// view itself is left alone. cod4-gsc.md 9.2 (the mount capture:
    /// `eFlags` 0x200, `viewlocked` 1 on 393, the clamp's 360/45 and
    /// 7.4999995/22.5).
    pub(super) fn turret_use(&mut self, slot: usize, id: EntId) {
        let Some(gun) = self.gun(id) else {
            return;
        };
        let (Some(p), Some(rec)) = (
            self.players.get_mut(slot).and_then(Option::as_mut),
            self.rt.host.turrets.get_mut(&id),
        ) else {
            return;
        };
        let ps = &mut p.ps;
        // The gun's side: where and how he stood, the barrel on his view.
        rec.angles2 = barrel_for(rec, ps_vec(ps, "viewangles"), gun.angles);
        rec.mount_stance = Some(stance_of(ps.word("pm_flags")));
        rec.mount_origin = ps_vec(ps, "origin");
        rec.fresh_mount = true;
        rec.busy = 1;
        rec.owner = Some(slot);
        // His side: the clamp on the arcs, the gun's stance, the lock.
        for (axis, arc) in Arc::pair(rec).into_iter().enumerate() {
            let (base, range) = arc.view_clamp(gun.angles[axis]);
            ps.setf(&format!("viewAngleClampBase[{axis}]"), base);
            ps.setf(&format!("viewAngleClampRange[{axis}]"), range);
        }
        let flags = ps.word("eFlags") & !ef::TURRET | gun_stance_bits(rec.def.stance);
        ps.set("eFlags", flags);
        lock_view(ps, VIEWLOCK_FULL, id.0);
        p.turret_active = 1;
    }

    /// `ClientEndFrame`'s gunner arm, after the body's anims: a player in
    /// the world on a gun (`otherFlags` 4, `eFlags` 0x300) runs
    /// `turret_think_client`, dead or alive (`cod4_lnxded` 0x80a6f57).
    pub(super) fn turret_end_frame(&mut self, slot: usize) {
        let Some(p) = self.players[slot].as_ref() else {
            return;
        };
        if p.ps.word("otherFlags") & 4 == 0 || p.ps.word("eFlags") & ef::TURRET == 0 {
            return;
        }
        let Some(id) = self.manned_gun(slot) else {
            return;
        };
        self.turret_think_client(slot, id);
    }

    /// Every gun as the bots read it: where it stands, its arcs and its
    /// gunner.
    pub fn gun_list(&mut self) -> Vec<crate::game::cod2_turret::GunInfo> {
        let ids: Vec<EntId> = self.rt.host.turrets.keys().copied().collect();
        ids.into_iter()
            .filter_map(|id| {
                let g = self.gun(id)?;
                let rec = self.rt.host.turrets.get(&id)?;
                Some(crate::game::cod2_turret::GunInfo {
                    id,
                    origin: g.origin,
                    yaw: g.angles[1],
                    yaw_range: rec.yaw_range,
                    pitch_range: rec.pitch_range,
                    owner: rec.owner,
                })
            })
            .collect()
    }

    /// The gun `slot` mans, if any.
    pub fn manned_gun(&self, slot: usize) -> Option<EntId> {
        (self.rt.host.turrets.iter())
            .find(|(_, r)| r.owner == Some(slot))
            .map(|(id, _)| *id)
    }

    /// Guns whose gunner left the server are let go. A death lets go in
    /// the gunner's own end frame and a spawn before it places him
    /// (cod4-gsc.md 9.2), so a gunner still here is never moved from here.
    pub(super) fn release_stale_turrets(&mut self) {
        let stale: Vec<EntId> = (self.rt.host.turrets.iter())
            .filter_map(|(id, r)| {
                let gone = self.players.get(r.owner?).is_none_or(Option::is_none);
                gone.then_some(*id)
            })
            .collect();
        for id in stale {
            self.stop_using_turret(id);
        }
    }

    /// `turret_think_client` (0x80b79e4): a gunner still on the gun
    /// (`active` 1, 0x80b7a1e), playing (`sessionState` 0, 0x80b7a40) and
    /// not in Last Stand (`pm_type` 6, 0x80b7a4a) gets `turret_track`,
    /// anyone else the release, a dead gunner on his death frame.
    fn turret_think_client(&mut self, slot: usize, id: EntId) {
        let active = self.players[slot].as_ref().map_or(0, |p| p.turret_active);
        let playing = self.rt.client_session(slot).map(|s| s.state)
            == Some(crate::follow::SessionState::Playing);
        let last_stand = self.players[slot]
            .as_ref()
            .is_some_and(|p| p.ps.word("pm_type") == super::level::PM_LASTSTAND);
        if active != 1 || !playing || last_stand {
            self.stop_using_turret(id);
        } else {
            self.turret_track(slot, id);
        }
    }

    /// `turret_track`, one manned frame (cod4-gsc.md 9.2). The body is set
    /// on the gun off the barrel's last frame ([`Self::place_gunner`]),
    /// then the barrel follows the view; the first frame after a mount
    /// flips the gun's teleport bit. The view stays locked, `viewlocked` 2
    /// on a frame that fires. A round sets the timer to `fireTime` and the
    /// loop sound's delay to three of them (`turret_shoot_internal`), lights
    /// the gun's firing bit and goes out through [`Self::fire_lead`]; a
    /// frozen gunner never fires. The capture: `EV_FIRE_WEAPON_MG42` every
    /// held frame, `eFlags` 0x40, `viewlocked` 2 (VERIFIED live).
    fn turret_track(&mut self, slot: usize, id: EntId) {
        let (Some(gun), Some((_, def))) = (self.gun(id), self.gun_weapon(id)) else {
            return;
        };
        let last = self.rt.host.turrets.get(&id).map(|r| r.angles2);
        if let Some(last) = last {
            self.place_gunner(slot, id, gun, last);
        }
        let attack = self.rt.host.cod4().buttons[slot] & 1 != 0;
        let (Some(p), Some(rec)) = (
            self.players[slot].as_mut(),
            self.rt.host.turrets.get_mut(&id),
        ) else {
            return;
        };
        lock_view(&mut p.ps, VIEWLOCK_FULL, id.0);
        let view = ps_vec(&p.ps, "viewangles");
        rec.angles2 = barrel_for(rec, view, gun.angles);
        if std::mem::take(&mut rec.fresh_mount) {
            rec.teleport_bit = !rec.teleport_bit;
        }
        rec.firing = false;

        let held = attack && p.ps.word("pm_flags") & pmf::FROZEN == 0;
        let single_shot = def.fire_type == FIRETYPE_SINGLESHOT;
        if !round_due(&mut rec.cooldown_ms, &mut p.trigger_down, held, single_shot) {
            return;
        }
        rec.cooldown_ms = def.fire_time;
        rec.loop_left_ms = 3 * def.fire_time;
        rec.firing = true;
        p.ps.set("viewlocked", VIEWLOCK_JITTER);
        let (barrel, tags) = (rec.angles2, rec.tags);
        let spread = self.gun_file(id).map_or(0.0, |f| f.player_spread.max(0.0));
        self.fire_lead(slot, id, &def, gun, barrel, tags, spread, view);
    }

    /// `G_PlayerTurretPositionAndBlend`: the legs' `turretanim` group
    /// blended by the barrel's yaw over the gun's `animHorRotateInc` and by
    /// the body's height over its rows, hung off `tag_weapon`
    /// (`turretpose::place_gunner`, the rule CoD1 and CoD2 share), then a
    /// point trace from the eye's height to 60 under the spot sets the
    /// body's height. The gun's `tag_weapon` is posed with `barrel`, the
    /// angles its last frame left, so the body trails the barrel a frame
    /// (cod4-gsc.md 9.2). Skipped with no tags, no anims or a legs anim
    /// that is not a turret group, as retail's anim flag test skips it.
    fn place_gunner(&mut self, slot: usize, id: EntId, g: Gun4, barrel: [f32; 3]) {
        /// `end[2] -= 60` under the placed spot.
        const PLACE_DROP: f32 = 60.0;
        /// `G_TraceCapsule`'s mask there (`MASK_PLAYERSOLID`).
        const PLACE_MASK: u32 = 0x81_0011;
        let Some(rec) = self.rt.host.turrets.get(&id) else {
            return;
        };
        let (Some(tags), inc) = (rec.tags, rec.def.anim_hor_rotate_inc) else {
            return;
        };
        let Some(p) = self.players[slot].as_ref() else {
            return;
        };
        let word = |k: &str| p.ps.word(k);
        let legs = word("legsAnim") as i32;
        let origin =
            Vec3::from_array([0, 1, 2].map(|k| f32::from_bits(word(&format!("origin[{k}]")))));
        let eye = f32::from_bits(word("viewHeightCurrent"));
        let Some(src) = self.combat.src.clone() else {
            return;
        };
        let Some(set) = self.combat.anims() else {
            return;
        };
        let f = vcod_cod2::player::BodyFrame {
            origin,
            legs_anim: legs,
            eflags: vcod_cod2::player::eflags::MOUNTED,
            ..Default::default()
        };
        let gun = vcod_cod2::player::GunFrame {
            model: String::new(),
            weapon: String::new(),
            origin: g.origin,
            angles: g.angles,
            barrel,
        };
        let placed = vcod_cod2::player::gunner_placement(
            vcod_common::turretpose::GunnerRule::Iw3,
            set,
            &*src,
            &f,
            &gun,
            (tags.aim, tags.weapon),
            inc,
        );
        let Some(placed) = placed else {
            return;
        };
        let mut at = placed.origin;
        if let Some(clip) = self.clip.as_deref() {
            let start = Vec3::new(at.x, at.y, at.z + eye);
            let end = Vec3::new(at.x, at.y, at.z - PLACE_DROP);
            let tr = clip.trace(start, end, Vec3::ZERO, Vec3::ZERO, PLACE_MASK);
            if tr.fraction < 1.0 {
                at.z = start.z + (end.z - start.z) * tr.fraction;
            }
        }
        if let Some(p) = self.players[slot].as_mut() {
            for k in 0..3 {
                p.ps.setf(&format!("origin[{k}]"), at[k]);
            }
        }
        self.rt.set_client_origin(slot, at.to_array());
    }

    /// `Fire_Lead`: `Turret_FillWeaponParms`' muzzle (the gunner's eye at
    /// the gun's `tag_player`, the round leaving that far along the view as
    /// `tag_flash` is from it), `Bullet_Fire` with the gun's `playerSpread`
    /// at `level.time` (no antilag), then `EV_FIRE_WEAPON_MG42` on the gun
    /// with the gunner's number.
    #[allow(clippy::too_many_arguments)]
    fn fire_lead(
        &mut self,
        slot: usize,
        id: EntId,
        d: &vcod_cod4::pmove::weapon::WeaponDef,
        g: Gun4,
        angles2: [f32; 3],
        tags: Option<crate::game::turret::TurretTags>,
        spread: f32,
        view: [f32; 3],
    ) {
        let [forward, right, up] = vcod_common::x87::angle_vectors(Vec3::from(view));
        if let Some(tags) = tags {
            let start = Vec3::from(crate::game::turret::muzzle(
                &tags,
                g.origin.to_array(),
                g.angles,
                angles2,
                forward.to_array(),
            ));
            let now = self.level_time();
            let range = super::combat::BULLET_RANGE;
            let aim = (spread * 0.017_453_292).tan() * range;
            let (x, y) = super::combat::random_dir(now);
            let end = start + forward * range + right * (x * aim) + up * (y * aim);
            self.combat
                .shots
                .push((slot, now, start, start + forward * range, end));
            let origins = self.rewound(now);
            self.bullet_fire_round(slot, d, start, end, now, &origins);
        }
        let ev = self.gun_events.entry(id).or_default();
        ev.add(EV_FIRE_WEAPON_MG42, slot as u32);
    }

    /// `G_ClientStopUsingTurret` (0x80b7866): the gun is freed, idle and silent; its
    /// gunner gets the stance he mounted from back (with its
    /// `EV_STANCE_FORCE_*`), loses the gun's stance bits and the view lock,
    /// and is teleported a unit over the spot he mounted from, his view at
    /// the body's angles (`SetClientViewAngle`: (0, the view's yaw, 0) as
    /// each think leaves them). The release capture: `EV_STANCE_FORCE_STAND`,
    /// the spot a unit up, `eFlags` 0x206 to 0x4, `viewlocked` 0 and 1023
    /// (VERIFIED live, cod4-gsc.md 9.2).
    pub fn stop_using_turret(&mut self, id: EntId) {
        let Some(rec) = self.rt.host.turrets.get_mut(&id) else {
            return;
        };
        let Some(slot) = rec.owner.take() else {
            return;
        };
        rec.busy = 0;
        rec.fresh_mount = false;
        rec.loop_left_ms = 0;
        let spot = rec.mount_origin;
        let stance = rec.mount_stance.take();
        if let Some(ent) = self.rt.host.ents.get_mut(id) {
            ent.loop_sound = 0;
        }
        let Some(p) = self.players.get_mut(slot).and_then(Option::as_mut) else {
            return;
        };
        p.turret_active = 0;
        p.trigger_down = false;

        let mut body = pmove::PlayerState::from_wire(&p.ps);
        if let Some((bit, height, event)) = stance.map(stance_restore) {
            body.pm_flags = (body.pm_flags & !(pmf::PRONE | pmf::DUCKED)) | bit;
            body.view_height_target = height;
            body.add_event(event, 0);
        }
        p.ps = body.to_wire(&p.ps);

        let flags = (p.ps.word("eFlags") ^ 2) & !ef::TURRET;
        p.ps.set("eFlags", flags);
        let to = [spot[0], spot[1], spot[2] + 1.0];
        let yaw = ps_vec(&p.ps, "viewangles")[1];
        let cmd = p.cmd;
        super::level::place_player(&mut p.ps, to, [0.0, yaw, 0.0], Some(&cmd));
        lock_view(&mut p.ps, 0, ENTITYNUM_NONE);
        self.rt.set_client_origin(slot, to);
    }

    /// The guns as 1.7 sends them: `ET_MG42` on the general table, the
    /// model in `index`, the turret weapon, the barrel's three angles in
    /// `lerp.u.turret.gunAngles` (`u.anonymous.data[0..3]`), the gunner's
    /// number in `otherEntityNum`, `apos.trType` 3, the teleport and firing
    /// bits, and the gun's event ring (the capture's unmanned SAW at its
    /// rest pitch -51, weapon 8, model 22, VERIFIED live).
    pub(super) fn turret_entities(&mut self, out: &mut BTreeMap<u32, EntityState>) {
        let ids: Vec<EntId> = self.rt.host.turrets.keys().copied().collect();
        for id in ids {
            let Some(e) = out.get_mut(&id.0) else {
                continue;
            };
            let weapon = self.gun_weapon(id).map_or(0, |(w, _)| w);
            let rec = &self.rt.host.turrets[&id];
            let index = super::level::ent_u32(e, "index");
            let pos = [0, 1, 2].map(|k| super::level::ent_u32(e, &format!("lerp.pos.trBase[{k}]")));
            let apos =
                [0, 1, 2].map(|k| super::level::ent_u32(e, &format!("lerp.apos.trBase[{k}]")));
            let apos_type = super::level::ent_u32(e, "lerp.apos.trType");
            let mut t = EntityState::default();
            t.0[0] = id.0;
            t.0[1] = ET_MG42;
            let mut set = |name: &str, v: u32| {
                if let Some(f) = EntityState::table(ET_MG42).iter().find(|f| f.name == name) {
                    t.0[f.word()] = v;
                }
            };
            for k in 0..3 {
                set(&format!("lerp.pos.trBase[{k}]"), pos[k]);
                set(&format!("lerp.apos.trBase[{k}]"), apos[k]);
                set(
                    &format!("lerp.u.anonymous.data[{k}]"),
                    rec.angles2[k].to_bits(),
                );
            }
            set("lerp.apos.trType", apos_type);
            set("index", index);
            set("weapon", weapon);
            if let Some(owner) = rec.owner {
                set("otherEntityNum", owner as u32);
            }
            let mut flags = 0;
            if rec.teleport_bit {
                flags |= 2;
            }
            if rec.firing {
                flags |= ef::FIRING;
            }
            set("lerp.eFlags", flags);
            if let Some(ev) = self.gun_events.get(&id) {
                ev.write(&mut set);
            }
            *e = t;
        }
    }

    /// The player's use of a gun under the hint ([`Self::use_entity`]'s
    /// `ET_MG42` arm): usable, `"trigger"` on the gun with the player, then
    /// `turret_use`.
    pub(super) fn use_turret(&mut self, slot: usize, id: EntId) {
        if !self.turret_usable(slot, id) {
            return;
        }
        if let Some(client) = self.rt.client_entity(slot) {
            self.rt
                .host
                .engine_notifies
                .push((id, "trigger", vec![Value::Entity(client)]));
        }
        self.turret_use(slot, id);
    }

    /// Slot `slot` uses gun entity `gun` as the use key would, for a
    /// client test that needs another player on a gun; whether he is on it.
    pub fn use_turret_for_test(&mut self, slot: usize, gun: u32) -> bool {
        let Some(id) = self.rt.host.ents.handle(gun) else {
            return false;
        };
        self.use_turret(slot, id);
        self.rt.host.turrets.get(&id).and_then(|r| r.owner) == Some(slot)
    }

    /// `Player_SetTurretDropHint`: on a gun with a drop hint the hint is
    /// its weapon plus 4 with the drop string, and no entity (the capture's
    /// 12, 1 and 1023, VERIFIED live).
    pub(super) fn turret_drop_hint(&mut self, slot: usize) {
        let Some(id) = self.manned_gun(slot) else {
            return;
        };
        let Some(file) = self.gun_file(id) else {
            return;
        };
        let Some(drop) = file.drop_hint.clone() else {
            return;
        };
        let Some((w, _)) = self.gun_weapon(id) else {
            return;
        };
        let s = self.hint_slot(Some(&drop));
        if let Some(p) = self.players[slot].as_mut() {
            p.ps.set("cursorHintEntIndex", ENTITYNUM_NONE);
            p.ps.set("cursorHint", w + 4);
            p.ps.set("cursorHintString", s as u32);
        }
    }
}

/// A gun's own event ring (`G_AddEvent` on the turret entity).
#[derive(Clone, Copy, Debug, Default)]
pub struct GunEvents {
    seq: u32,
    events: [u32; 4],
    parms: [u32; 4],
}

impl GunEvents {
    fn add(&mut self, event: u32, parm: u32) {
        let k = (self.seq & 3) as usize;
        self.events[k] = event;
        self.parms[k] = parm & 0xff;
        self.seq = (self.seq + 1) & 0xff;
    }

    fn write(&self, set: &mut impl FnMut(&str, u32)) {
        set("eventSequence", self.seq);
        for k in 0..4 {
            set(&format!("events[{k}]"), self.events[k]);
            set(&format!("eventParms[{k}]"), self.parms[k]);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The capture's crouch SAW at yaw 0 with arcs 45 either side and
    /// -15..30 of pitch: the view clamp's yaw range 45 at base 360 and its
    /// pitch range 22.5 at base 7.4999995 (`play-1.7-saw.txt`, VERIFIED
    /// live).
    #[test]
    fn the_mount_centres_the_view_clamp_on_the_arcs_as_1_7_rounds_it() {
        let yaw = Arc {
            low: -45.0,
            high: 45.0,
        };
        assert_eq!(yaw.view_clamp(0.0), (360.0, 45.0));
        let pitch = Arc {
            low: -15.0,
            high: 30.0,
        };
        assert_eq!(pitch.view_clamp(0.0), (f32::from_bits(0x40ef_ffff), 22.5));
        // A view past either edge holds on it.
        assert_eq!(yaw.pin(70.0, 0.0), 45.0);
        assert_eq!(pitch.pin(-40.0, 0.0), -15.0);
        assert_eq!(yaw.pin(-10.0, 0.0), -10.0);
    }
}
