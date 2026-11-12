//! CoD4's use key (`Player_GetUseList`): the triggers the stock game
//! objects stand on (`trigger_use_touch`: the `sd` and `sab` bomb zones
//! and defuse triggers), plain `trigger_use` and dropped weapons,
//! mounted guns and live frags (`super::turret`, the throw back),
//! and the touch pass the proximity ones need (`dom`'s flags, `koth`'s
//! radio, the carried bombs' pickup triggers), and a player's `linkTo`.
//! docs/research/cod4-gsc.md 9 has the evidence; names and order are
//! INFERRED.

use glam::Vec3;
use vcod_cod4::net::delta::ENTITYNUM_NONE;
use vcod_gsc::{EntId, Host, Value};

use super::level::{Cod4Level, PsExt};
use crate::game::trigger::TriggerKind;

/// `+activate` (0x8) and `+usereload` (0x20).
const USE_BUTTONS: u32 = 0x28;
/// `Player_GetUseList`'s `CM_AreaEntities` box about the eye (192 or the
/// throwback and MG radii, 96 up and down) and its contents.
const AREA_BOX: Vec3 = Vec3::new(192.0, 192.0, 96.0);
const CONTENTS_USE: i32 = 0x20_0000;
/// `playerMins`, `playerMaxs`.
const PLAYER_MINS: Vec3 = Vec3::new(-15.0, -15.0, 0.0);
const PLAYER_MAXS: Vec3 = Vec3::new(15.0, 15.0, 70.0);
/// The reach from the eye to an entry's middle, the view-angle weight,
/// the penalty an ungrabbable item or a blocked sight takes (`cod4_lnxded`
/// rodata 0x820e278, 0x8214ac8, 0x8220950) and the sight trace's mask
/// (`G_TraceCapsuleComplete(.., 17)`).
const REACH: f32 = 128.0;
const ANGLE_WEIGHT: f32 = 256.0;
const PENALTY: f32 = 10000.0;
const SIGHT_MASK: u32 = 0x11;
/// The use list's weight off a missile and a turret (rodata 0x8211e70's
/// 512 and half the angle weight, `cod4_lnxded` 0x80d3655..0x80d36c4).
const MISSILE_OFFSET: f32 = 512.0;
const TURRET_OFFSET: f32 = 128.0;
/// `player_throwbackInnerRadius`, `player_throwbackOuterRadius` and
/// `player_MGUseRadius`'s defaults (`cod4_lnxded` 0x80b5343, 0x80b537c,
/// 0x80b53b5: 90, 160, 128), and `bg_maxGrenadeIndicatorSpeed`'s (the
/// mirror's 20).
const THROWBACK_INNER: f32 = 90.0;
const THROWBACK_OUTER: f32 = 160.0;
const MG_USE_RADIUS: f32 = 128.0;
const MAX_GRENADE_INDICATOR_SPEED: f32 = 20.0;
/// `inventoryType` 0 and 3, `offhandClass` 2 (smoke) and 3 (flash).
const INV_PRIMARY: i32 = 0;
const INV_ALTMODE: i32 = 3;
const OFFHAND_SMOKE: i32 = 2;
const OFFHAND_FLASH: i32 = 3;

/// What a use-list entry is.
#[derive(Clone, Copy, Debug)]
enum UseKind {
    /// A `trigger_use` or `trigger_use_touch`: its team
    /// (`setTeamForTrigger`) and claimer (`clientClaimTrigger`).
    Trigger { team: u8, claimer: Option<u16> },
    /// A dropped weapon, its weapon number.
    Item(u32),
    /// A missile, its weapon number and its think's time (the fuse).
    Missile(u32, i32),
    /// A mounted gun.
    Turret,
}

/// The hint the use list's walk settles on: the entity, `cursorHint`,
/// `cursorHintString` and a live grenade's fuse time left.
#[derive(Clone, Copy, Debug)]
struct Hint {
    ent: EntId,
    hint: u32,
    string: i32,
    fuse_left: Option<i32>,
}

#[derive(Clone, Copy, Debug)]
struct UseEntry {
    id: EntId,
    kind: UseKind,
    score: f32,
    /// Where the sight trace aims; none for a touch trigger.
    sight: Option<Vec3>,
}

/// `compare_use` (0x80d2c10): the score difference truncated to an int,
/// under glibc's merge sort, as CoD2's.
fn sort(list: &mut [UseEntry]) {
    crate::game::item::msort(list, |a, b| (a.score - b.score) as i32 <= 0);
}

/// A float dvar off the host's cvars, `default` when unset or unparsed.
fn dvar(level: &Cod4Level, name: &str, default: f32) -> f32 {
    level
        .rt
        .host
        .cvars
        .get(name)
        .trim()
        .parse()
        .unwrap_or(default)
}

fn l_has(level: &Cod4Level, slot: usize, w: usize) -> bool {
    level.rt.host.cod4().clients[slot].loadout.has(w)
}

/// Weapon states 15..20 (the offhand's) refuse the press.
const OFFHAND_STATES: std::ops::RangeInclusive<i32> = 15..=20;

impl Cod4Level {
    /// `G_RunFrame`'s first pass: `G_TouchTriggers` for every
    /// client in use (the pass itself skips a `pm_type` past linked).
    pub(super) fn touch_triggers(&mut self, now_ms: i32) {
        for slot in 0..self.players.len() {
            if self.players[slot].is_some() && self.rt.client_entity(slot).is_some() {
                self.rt.touch_triggers(slot, now_ms);
            }
        }
    }

    /// `Player_GetUseList` (inlined in `Player_UpdateCursorHints`,
    /// `cod4_lnxded` 0x80d2c30): the entities with contents 0x200000 in the
    /// box about the eye, a `trigger_use_touch` the player's box meets at
    /// -256, a `trigger_use` or a dropped weapon whose middle lies within
    /// 128 of the eye at `(1 - (dot + 1) / 2) * 256` plus the distance, a
    /// `trigger_use` 256 under that, an item the player may not take 10000
    /// over it and cut; sorted, each left but a touch trigger traced to
    /// from the eye (mask 0x11) and a blocked one 10000 over and cut, sorted
    /// again; a missile within the throw back's reach and a gun within
    /// `player_MGUseRadius` are listed too. docs/research/cod4-gsc.md 9.1.
    fn use_list(&mut self, slot: usize) -> Vec<UseEntry> {
        let Some(ps) = self.pm_state(slot) else {
            return Vec::new();
        };
        let eye = super::combat::view_origin(&ps, self.level_time());
        let forward = Vec3::from(crate::game::spawn::angle_forward(ps.view_angles));
        let exact = (
            (ps.origin + PLAYER_MINS).to_array(),
            (ps.origin + PLAYER_MAXS).to_array(),
        );
        let outer = dvar(self, "player_throwbackOuterRadius", THROWBACK_OUTER);
        let inner = dvar(self, "player_throwbackInnerRadius", THROWBACK_INNER);
        let mg = dvar(self, "player_MGUseRadius", MG_USE_RADIUS);
        let max_speed = dvar(
            self,
            "bg_maxGrenadeIndicatorSpeed",
            MAX_GRENADE_INDICATOR_SPEED,
        );
        let prev_hint = ps.cursor_hint_ent_index as u32;
        let score_within = |centre: Vec3, reach: f32| -> Option<f32> {
            let to = centre - eye;
            let dist = to.length();
            if dist > reach {
                return None;
            }
            let dir = if dist > 0.0 { to / dist } else { Vec3::ZERO };
            Some((1.0 - (dir.dot(forward) + 1.0) * 0.5) * ANGLE_WEIGHT + dist)
        };
        let score = |centre: Vec3| -> Option<f32> {
            let to = centre - eye;
            let dist = to.length();
            if dist > REACH {
                return None;
            }
            let dir = if dist > 0.0 { to / dist } else { Vec3::ZERO };
            Some((1.0 - (dir.dot(forward) + 1.0) * 0.5) * ANGLE_WEIGHT + dist)
        };
        let mut turrets: Vec<(EntId, f32, Vec3)> = Vec::new();
        let mut list = self.rt.with_host(|host, cx| {
            let classname = cx.intern_folded("classname");
            let lo = (eye - AREA_BOX).to_array();
            let hi = (eye + AREA_BOX).to_array();
            let mut list = Vec::new();
            for n in host.area.entities_in_box(lo, hi, CONTENTS_USE) {
                if n as usize == slot {
                    continue;
                }
                let (Some(id), Some(link)) = (host.ents.handle(n), host.area.last_link(n)) else {
                    continue;
                };
                if host.turrets.contains_key(&id) {
                    let centre = (Vec3::from(link.absmin) + Vec3::from(link.absmax)) * 0.5;
                    if let Some(sc) = score_within(centre, mg) {
                        turrets.push((id, sc - TURRET_OFFSET, centre));
                    }
                    continue;
                }
                let Some(t) = host.triggers.get(id).filter(|t| t.kind == TriggerKind::Use) else {
                    continue;
                };
                let t = *t;
                let touch = matches!(host.get_field(cx, id, classname),
                    Value::String(s) if cx.resolve(s) == "trigger_use_touch");
                let kind = UseKind::Trigger {
                    team: t.team,
                    claimer: t.claimer,
                };
                if touch {
                    if crate::game::trigger::contacts(host, cx, id, &t, exact, false) {
                        list.push(UseEntry {
                            id,
                            kind,
                            score: -ANGLE_WEIGHT,
                            sight: None,
                        });
                    }
                    continue;
                }
                let centre = (Vec3::from(link.absmin) + Vec3::from(link.absmax)) * 0.5;
                if let Some(sc) = score(centre) {
                    list.push(UseEntry {
                        id,
                        kind,
                        score: sc - ANGLE_WEIGHT,
                        sight: Some(centre),
                    });
                }
            }
            list
        });
        // The dropped weapons are not in vcod's area tree; 1.7 lists them
        // in the same walk.
        let items: Vec<(EntId, u32, Vec3)> = (self.rt.host.cod4().items.iter())
            .filter(|i| i.resting && (i.base - eye).abs().cmple(AREA_BOX).all())
            .map(|i| (i.id, i.weapon, i.base))
            .collect();
        // A missile near the player's feet, or the one already under the
        // hint, moving slower than the grenade indicator's speed, within
        // the throw back's reach of the eye.
        let feet = ps.origin;
        let missiles: Vec<(EntId, u32, i32, Vec3)> = (self.rt.host.cod4().missiles.iter())
            .filter(|m| m.free_at.is_none() && (m.origin - eye).abs().cmple(AREA_BOX).all())
            .filter(|m| {
                let flat = (m.origin - feet).truncate().length_squared();
                (prev_hint == m.id.0 || inner * inner >= flat)
                    && max_speed * max_speed >= m.pos.delta.length_squared()
            })
            .map(|m| (m.id, m.weapon, m.next_think, m.origin))
            .collect();
        for (id, weapon, think, at) in missiles {
            let Some(sc) = score_within(at, outer) else {
                continue;
            };
            list.push(UseEntry {
                id,
                kind: UseKind::Missile(weapon, think),
                score: sc - MISSILE_OFFSET,
                sight: Some(at),
            });
        }
        for (id, sc, centre) in turrets {
            // The sight trace aims at the gun's `tag_aim` where it has one.
            let aim = self.turret_aim_point(id).unwrap_or(centre);
            list.push(UseEntry {
                id,
                kind: UseKind::Turret,
                score: sc,
                sight: Some(aim),
            });
        }
        let mut cut = 0;
        for (id, weapon, at) in items {
            let Some(sc) = score(at) else { continue };
            let grabbable = self.item_grabbable(slot, &ps, id, weapon);
            cut += usize::from(!grabbable);
            list.push(UseEntry {
                id,
                kind: UseKind::Item(weapon),
                score: sc + if grabbable { 0.0 } else { PENALTY },
                sight: Some(at),
            });
        }
        sort(&mut list);
        list.truncate(list.len() - cut);
        let mut blocked = 0;
        if let Some(clip) = self.clip.as_deref() {
            for e in list.iter_mut() {
                let Some(to) = e.sight else { continue };
                if clip
                    .trace(eye, to, Vec3::ZERO, Vec3::ZERO, SIGHT_MASK)
                    .fraction
                    < 1.0
                {
                    e.score += PENALTY;
                    blocked += 1;
                }
            }
        }
        sort(&mut list);
        list.truncate(list.len() - blocked);
        list
    }

    /// `BG_CanItemBeGrabbed` on a use (`touched` 0): not with
    /// weapons disabled (`weapFlags` 0x80), not the player's own drop
    /// (the item's `clientNum`), and a weapon (or its alt) the player does
    /// not hold, a flash or smoke grenade only where the secondary offhand
    /// takes it (`BG_PlayerCanPickUpWeaponType`).
    fn item_grabbable(
        &self,
        slot: usize,
        ps: &vcod_cod4::pmove::PlayerState,
        id: EntId,
        weapon: u32,
    ) -> bool {
        let c4 = self.rt.host.cod4();
        if ps.weap_flags & 0x80 != 0
            || c4
                .items
                .iter()
                .any(|i| i.id == id && i.owner == slot as u32)
        {
            return false;
        }
        let can = |w: usize| {
            c4.table.get(w).is_some_and(|d| {
                let kind_ok = match d.offhand_class {
                    OFFHAND_FLASH => ps.offhand_secondary == 1,
                    OFFHAND_SMOKE => ps.offhand_secondary == 0,
                    _ => true,
                };
                kind_ok && !ps.has_weapon(w as u32)
            })
        };
        let w = weapon as usize;
        can(w) || c4.table.get(w).is_some_and(|d| d.alt != 0 && can(d.alt))
    }

    /// The gun's `tag_aim` in the world (`G_DObjGetWorldTagPos`).
    fn turret_aim_point(&mut self, id: EntId) -> Option<Vec3> {
        let tags = self.rt.host.turrets.get(&id)?.tags?;
        let g = self.gun(id)?;
        let axis = vcod_common::x87::angles_to_axis(Vec3::from(g.angles));
        Some(g.origin + crate::game::spawn::transform(tags.aim, &axis))
    }

    /// `Player_UpdateCursorHints`, run in `ClientEndFrame` (its `eType`
    /// switch at `cod4_lnxded` 0x80d33dc). Each frame clears `cursorHint`
    /// and, unless a throw back is under way, its time left. The dead get
    /// nothing more; a gunner gets his gun's drop hint
    /// (`Player_SetTurretDropHint`); a mantle, a sprint, an offhand state,
    /// the prematch freeze or an empty use list leave the hint cleared, and
    /// Last Stand clears its entity too. Otherwise the use list is walked
    /// in order and the first entry that offers a hint wins
    /// ([`Self::entry_hint`]). Its entity is what the next use press
    /// (`Player_ActivateCmd`) or frag press (the live grenade pickup)
    /// takes. cod4-gsc.md 9.1-9.3; the pins are `turret_tests`.
    pub(super) fn update_cursor_hints(&mut self, slot: usize) {
        let Some(p) = self.players[slot].as_mut() else {
            return;
        };
        p.ps.set("cursorHint", 0);
        if p.ps.word("throwBackGrenadeOwner") == ENTITYNUM_NONE {
            p.ps.set("throwBackGrenadeTimeLeft", 0);
        }
        let alive = self.rt.client_vitals(slot).health > 0;
        let Some(ps) = self.pm_state(slot).filter(|_| alive) else {
            return;
        };
        if self.players[slot]
            .as_ref()
            .is_some_and(|p| p.turret_active != 0)
        {
            if ps.e_flags & vcod_cod4::pmove::ef::TURRET != 0 {
                self.turret_drop_hint(slot);
            }
            return;
        }
        let busy = vcod_cod4::pmove::pmf::MANTLE | vcod_cod4::pmove::pmf::SPRINTING;
        if ps.pm_flags & busy != 0 || (15..=19).contains(&ps.weapon_state) {
            return;
        }
        if ps.pm_type == super::level::PM_LASTSTAND as i32 {
            self.show_hint(slot, None);
            return;
        }
        if self.rt.host.cod4().clients[slot].frozen {
            return;
        }
        let mut picked = None;
        for entry in self.use_list(slot) {
            picked = self.entry_hint(slot, &ps, entry);
            if picked.is_some() {
                break;
            }
        }
        if let Some(hint) = picked {
            self.show_hint(slot, Some(hint));
        }
    }

    /// What one use-list entry offers, `None` to walk on. A trigger closed
    /// to the player's team or claimed by another is passed over, any
    /// other ends the walk with its own hint and string (no string without
    /// a hint; a hint of 0 shows nothing). A dropped weapon offers its
    /// number plus 4 when [`Self::item_hints`] lets it, a live grenade
    /// always offers its weapon plus 4 and its fuse's time left, a gun its
    /// hint when usable ([`Self::turret_hint`]). The SAW capture's 12, 0,
    /// 393 and the throw back's 44 are VERIFIED live (cod4-gsc.md 9.2,
    /// 9.3).
    fn entry_hint(
        &mut self,
        slot: usize,
        ps: &vcod_cod4::pmove::PlayerState,
        entry: UseEntry,
    ) -> Option<Hint> {
        let shown = |hint: u32, string: i32| Hint {
            ent: entry.id,
            hint,
            string,
            fuse_left: None,
        };
        match entry.kind {
            UseKind::Trigger { team, claimer } => {
                let open = team == 0 || i32::from(team) == self.rt.client_team(slot);
                let mine = claimer.is_none_or(|c| usize::from(c) == slot);
                if !(open && mine) {
                    return None;
                }
                let t = self.rt.host.triggers.get(entry.id);
                let hint = t.map_or(0, |t| t.cursor_hint) as u32;
                let string = t.filter(|_| hint != 0).map_or(-1, |t| t.hint_string);
                Some(shown(hint, string))
            }
            UseKind::Item(weapon) => self.item_hints(ps, weapon).then(|| shown(weapon + 4, -1)),
            UseKind::Missile(weapon, think) => Some(Hint {
                fuse_left: Some(think - self.level_time()),
                ..shown(weapon % 128 + 4, -1)
            }),
            UseKind::Turret => self
                .turret_hint(slot, entry.id)
                .map(|(hint, string)| shown(hint, string)),
        }
    }

    /// Writes the walk's pick into the playerstate: a throw back's time
    /// left, then the hint, its string and its entity, or no entity for no
    /// pick or a pick of hint 0.
    fn show_hint(&mut self, slot: usize, hint: Option<Hint>) {
        let Some(p) = self.players[slot].as_mut() else {
            return;
        };
        if let Some(left) = hint.and_then(|h| h.fuse_left) {
            p.ps.set("throwBackGrenadeTimeLeft", left as u32);
        }
        match hint.filter(|h| h.hint != 0) {
            Some(h) => {
                p.ps.set("cursorHintEntIndex", h.ent.0);
                p.ps.set("cursorHint", h.hint);
                p.ps.set("cursorHintString", h.string as u32);
            }
            None => p.ps.set("cursorHintEntIndex", ENTITYNUM_NONE),
        }
    }

    /// Whether a dropped weapon `w` shows its hint (the item arm's checks at
    /// `cod4_lnxded` 0x80d3523..0x80d355c, cod4-gsc.md 9.1): never one the
    /// player holds; otherwise only a primary or alt-mode item is refused,
    /// and only to a player holding two primaries with neither kind in
    /// hand. A weapon with no def reads as a primary.
    fn item_hints(&self, ps: &vcod_cod4::pmove::PlayerState, w: u32) -> bool {
        if ps.has_weapon(w) {
            return false;
        }
        let table = &self.rt.host.cod4().table;
        let kind = |w: u32| {
            table
                .get(w as usize)
                .map_or(INV_PRIMARY, |d| d.inventory_type)
        };
        let gun_like = |w: u32| matches!(kind(w), INV_PRIMARY | INV_ALTMODE);
        if gun_like(ps.weapon) || !gun_like(w) {
            return true;
        }
        let primaries = (1..128u32)
            .filter(|&h| ps.has_weapon(h) && kind(h) == INV_PRIMARY)
            .count();
        primaries < 2
    }

    /// `Player_UpdateActivate`, last in `ClientThink_real` for a
    /// player in the world. A press of 0x28 (`latched_buttons`,
    /// `Player_ActivateCmd`) drops the held target; on a gun it asks for
    /// the release (`active` 2, which the end frame's
    /// `turret_think_client` carries out); off a mantle, a sprint and the
    /// offhand states it takes the end frame's cursor hint's entity with
    /// the press time. While 0x28 is held and `g_useholdtime` has run since,
    /// the target is used ([`Self::use_entity`]) and dropped.
    pub(super) fn update_activate(&mut self, slot: usize, buttons: u32, old: u32) {
        let now = self.level_time();
        let hold_ms = crate::game::builtins::cvar::atoi(self.rt.host.cvars.get("g_useholdtime"));
        let Some(ps) = self.pm_state(slot) else {
            return;
        };
        if self.rt.client_vitals(slot).dead {
            return;
        }
        if buttons & !old & USE_BUTTONS != 0 {
            let refused = ps.pm_flags
                & (vcod_cod4::pmove::pmf::MANTLE | vcod_cod4::pmove::pmf::SPRINTING)
                != 0
                || OFFHAND_STATES.contains(&ps.weapon_state);
            let Some(p) = self.players[slot].as_mut() else {
                return;
            };
            p.use_hold = None;
            if p.turret_active != 0 {
                p.turret_active = if ps.e_flags & vcod_cod4::pmove::ef::TURRET != 0 {
                    2
                } else {
                    0
                };
            } else if !refused
                && p.ps.word("cursorHint") != 0
                && p.ps.word("cursorHintEntIndex") != ENTITYNUM_NONE
            {
                let n = p.ps.word("cursorHintEntIndex");
                p.use_hold = self.rt.host.ents.handle(n).map(|id| (id, now));
            }
        }
        let Some(p) = self.players[slot].as_mut() else {
            return;
        };
        if buttons & USE_BUTTONS != 0
            && let Some((id, at)) = p.use_hold
            && now - at >= hold_ms
        {
            p.use_hold = None;
            self.use_entity(slot, id);
        }
    }

    /// `Player_UseEntity`: an item is notified `touch` with the
    /// player and taken ([`Self::swap_weapon`]), then notified `trigger`
    /// with the player and the weapon it dropped, the pair
    /// `_weapons.gsc`'s `watchPickup` waits for (VERIFIED, the stock
    /// script); a gun is mounted if still usable
    /// ([`Self::use_turret`]); anything else is notified `trigger` with the
    /// player.
    fn use_entity(&mut self, slot: usize, id: EntId) {
        let Some(client) = self.rt.client_entity(slot) else {
            return;
        };
        if self.rt.host.ents.get(id).is_none() {
            return;
        }
        if self.rt.host.turrets.contains_key(&id) {
            self.use_turret(slot, id);
            return;
        }
        let player = Value::Entity(client);
        if !self.rt.host.cod4().items.iter().any(|i| i.id == id) {
            self.rt
                .host
                .engine_notifies
                .push((id, "trigger", vec![player]));
            return;
        }
        self.rt
            .host
            .engine_notifies
            .push((id, "touch", vec![player]));
        if let Some(dropped) = self.swap_weapon(slot, id) {
            let mut args = vec![player];
            args.extend(dropped.map(Value::Entity));
            self.rt.host.engine_notifies.push((id, "trigger", args));
        }
    }

    /// The live grenade pickup, inlined in the client event handler's
    /// `EV_SWITCH_OFFHAND` arm (`cod4_lnxded` 0x80a59c6..0x80a5aa5): with
    /// an entity under the hint (`cursorHintEntIndex` not 0), the event's
    /// weapon a frag (`offhandClass` 1 at def+0x144), the entity a missile
    /// (`eType` 4) of a frag, and `throwBackGrenadeTimeLeft` set, the
    /// thrower becomes `throwBackGrenadeOwner` (its parent, 1022 for none)
    /// and the fuse's time left the hand's `grenadeTimeLeft`; then its touch
    /// (`Touch_Item_Auto` with `touched` 0). VERIFIED (the instructions).
    /// The touch takes the live frag (the grab test always grabs
    /// one), puts `EV_AMMO_PICKUP` on the player, notifies `trigger` on the
    /// grenade with the player and frees it (`death`), INFERRED.
    /// The offhand half then throws it back with that fuse: pmove spends no
    /// ammo on a throw back and the grenade timer init arms the new grenade off
    /// `grenadeTimeLeft` (cod4-combat.md 10, cod4-gsc.md 9.3).
    pub(super) fn attempt_live_grenade_pickup(&mut self, slot: usize, weapon: u32) {
        const OFFHAND_FRAG: i32 = 1;
        const EV_AMMO_PICKUP: u32 = 10;
        let Some(p) = self.players[slot].as_ref() else {
            return;
        };
        let hint = p.ps.word("cursorHintEntIndex");
        let left = p.ps.word("throwBackGrenadeTimeLeft") as i32;
        let frag = |w: u32| {
            self.combat
                .defs
                .get(w as usize)
                .is_some_and(|d| d.offhand_class == OFFHAND_FRAG)
        };
        if hint == 0 || !frag(weapon) {
            return;
        }
        let Some(m) = (self.rt.host.cod4().missiles.iter())
            .find(|m| m.id.0 == hint && m.free_at.is_none())
            .map(|m| (m.id, m.weapon, m.owner))
        else {
            return;
        };
        let (id, w, owner) = m;
        if !frag(w % 128) || left == 0 {
            return;
        }
        let owner = owner.map_or(1022, |s| s as u32);
        let Some(client) = self.rt.client_entity(slot) else {
            return;
        };
        let alive = self.rt.client_vitals(slot).health >= 1;
        let p = self.players[slot].as_mut().unwrap();
        p.ps.set("throwBackGrenadeOwner", owner);
        p.ps.set("grenadeTimeLeft", left as u32);
        // `Touch_Item`: a living player who may grab (weapons on).
        if !alive || p.ps.word("weapFlags") & 0x80 != 0 {
            return;
        }
        let mut pm = vcod_cod4::pmove::PlayerState::from_wire(&p.ps);
        pm.add_event(EV_AMMO_PICKUP, w % 128);
        p.ps = pm.to_wire(&p.ps);
        self.rt
            .host
            .engine_notifies
            .push((id, "trigger", vec![Value::Entity(client)]));
        // `G_FreeEntity` drops it from every hint that names it.
        for p in self.players.iter_mut().flatten() {
            if p.ps.word("cursorHintEntIndex") == id.0 {
                p.ps.set("cursorHintEntIndex", ENTITYNUM_NONE);
            }
        }
        self.free_missile(id);
    }

    /// `Touch_Item` with `touched` 0 on a dropped weapon: with two
    /// primaries held and the item a primary, the weapon in hand (a
    /// primary) goes into the world with its clip and reserve as
    /// `dropItem` launches one; the item's weapon is given with the item's
    /// clip and reserve on top of what its ammo slot holds, switched to
    /// (`a <weapon>`), and the item freed. INFERRED: vcod's CoD2 swap's
    /// rule (`game::item::touch`); 1.7's `Touch_Item` is not read
    /// (cod4-gsc.md 9.1). `None` for nothing taken, else the item dropped
    /// in the swap, if one was. The item is freed at the next entity pass.
    fn swap_weapon(&mut self, slot: usize, id: EntId) -> Option<Option<EntId>> {
        let it = self
            .rt
            .host
            .cod4()
            .items
            .iter()
            .find(|i| i.id == id)
            .cloned()?;
        let w = it.weapon as usize;
        let (held, primaries, primary, held_primary) = {
            let c4 = self.rt.host.cod4();
            let l = &c4.clients[slot].loadout;
            let inv = |w: usize| c4.table.get(w).map_or(-1, |d| d.inventory_type);
            let primaries = l.held().filter(|&h| inv(h) == INV_PRIMARY).count();
            (
                l.weapon,
                primaries,
                inv(w) == INV_PRIMARY,
                inv(l.weapon) == INV_PRIMARY,
            )
        };
        if l_has(self, slot, w) {
            return None;
        }
        // `Weapon;<guid>;<num>;<name>;<weapon>` (0x8215e35, 0x8092183): the
        // log line of a weapon the player does not hold, taken. INFERRED
        // (the branch past the held-weapon bit test at 0x80920b9).
        if let Some(d) = self.rt.host.cod4().table.get(w) {
            let weapon = d.name.clone();
            let (who, guid) = crate::game::cod4::log_client(&self.rt.host, slot);
            crate::game::cod4::log_printf(
                &mut self.rt.host,
                format!("Weapon;{guid};{slot};{who};{weapon}\n"),
            );
        }
        let mut dropped = None;
        if primary && primaries >= 2 && held_primary && held != 0 {
            let (clip, stock) = {
                let c4 = self.rt.host.cod4();
                let l = &c4.clients[slot].loadout;
                c4.table
                    .get(held)
                    .map_or((0, 0), |d| (l.ammoclip[d.clip_slot], l.ammo[d.ammo_slot]))
            };
            let launched = self.rt.with_host(|host, cx| {
                crate::game::builtins::cod4_combat::launch_weapon(host, cx, slot, held)
                    .ok()
                    .flatten()
            });
            if let Some(d) = launched
                && let Some(i) = self.rt.host.cod4_mut().items.iter_mut().find(|i| i.id == d)
            {
                i.clip = clip;
                i.stock = stock;
            }
            dropped = launched;
            self.rt.host.cod4_mut().clients[slot].loadout.take(held);
        }
        let c4 = self.rt.host.cod4_mut();
        let table = std::mem::take(&mut c4.table);
        let l = &mut c4.clients[slot].loadout;
        if l.give(&table, w, 0).is_some()
            && let Some(d) = table.get(w)
        {
            l.ammoclip[d.clip_slot] = it.clip.clamp(0, d.clip_size);
            let max = l.ammo_max(&table, w);
            l.ammo[d.ammo_slot] = (l.ammo[d.ammo_slot] + it.stock.max(0)).min(max);
        }
        c4.table = table;
        c4.items.retain(|i| i.id != id);
        c4.free_next.push(id);
        self.rt.host.client_commands.push((slot, format!("a {w}")));
        Some(dropped)
    }

    /// A player's `linkTo` and `unlink` onto its playerstate
    /// (`G_EntLinkTo`, `G_EntUnlink`): the link held for the end
    /// frames ([`super::level::Player4::linked`]), `pm_type` 1 (8 dead)
    /// while linked, which pmove holds where it stands, and back to 0 (7).
    /// The stock scripts link a player only to a use trigger that stands
    /// still (`useHoldThink`), so the body is not carried along.
    pub(super) fn apply_link_ops(&mut self) {
        use super::level::{PM_DEAD, PM_DEAD_LINKED, PM_NORMAL, PM_NORMAL_LINKED};
        use crate::game::host::LinkOp;
        for (slot, op) in self.rt.take_link_ops() {
            let Some(p) = self.players.get_mut(slot).and_then(Option::as_mut) else {
                continue;
            };
            let pm = p.ps.word("pm_type");
            match op {
                LinkOp::Link { .. } => {
                    p.linked = true;
                    match pm {
                        PM_NORMAL => p.ps.set("pm_type", PM_NORMAL_LINKED),
                        PM_DEAD => p.ps.set("pm_type", PM_DEAD_LINKED),
                        _ => {}
                    }
                    for k in 0..3 {
                        p.ps.setf(&format!("velocity[{k}]"), 0.0);
                    }
                }
                LinkOp::Unlink => {
                    p.linked = false;
                    match pm {
                        PM_NORMAL_LINKED => p.ps.set("pm_type", PM_NORMAL),
                        PM_DEAD_LINKED => p.ps.set("pm_type", PM_DEAD),
                        _ => {}
                    }
                }
            }
        }
    }
}
