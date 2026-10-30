//! CoD2's use key: the use list (`Player_GetUseList`), the cursor hint it
//! picks (`Player_UpdateCursorHints`, from `ClientEndFrame`) and the press
//! that uses the hint's entity (`Player_UpdateActivate`, last in
//! `ClientThink_real`). One path for the weapon items, the mounted guns and
//! the `trigger_use` / `trigger_use_touch` triggers; docs/research/
//! cod2-gsc-diff.md 5.3 has the evidence. Mac names, lnxded addresses where
//! the symbol table has them.

use glam::Vec3;
use vcod_cod2::net::UserCmd2;
use vcod_cod2::pmove::{ef, pm_type, pmf};
use vcod_gsc::{EntId, Host, Value};

use super::cod2::Cod2Level;
use crate::game::trigger::TriggerKind;

/// `+activate` (0x8) and `+usereload` (0x20): `useButtonPressed`'s bits.
pub const USE_BUTTONS: i32 = 0x28;
/// `+usereload` alone: a tap with nothing to use is a reload.
const USE_RELOAD: i32 = 0x20;
/// `Player_GetUseList`'s `CM_AreaEntities` box about the eye and its
/// contents (lnxded rodata 0x815ee4c..0x815ee54).
const AREA_BOX: Vec3 = Vec3::new(192.0, 192.0, 96.0);
const CONTENTS_USE: i32 = 0x20_0000;
/// The reach from the eye to a bounds centre (0x815ee60), the view-angle
/// weight (0x815ee58, 0x815ee5c) and the penalty an ungrabbable item or a
/// blocked sight takes (0x815ee64).
const REACH: f32 = 128.0;
const ANGLE_WEIGHT: f32 = 256.0;
const PENALTY: f32 = 10000.0;
/// The sight trace's mask (`G_TraceCapsuleComplete(.., 0x11)`).
const SIGHT_MASK: u32 = 0x11;
/// `weaponType` 1: a grenade item never hints.
const WEAP_TYPE_GRENADE: i32 = 1;

/// What a use-list entry is.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum UseKind {
    /// A dropped item (`eType` 3): its weapon row, if a weapon, and
    /// whether `BG_CanItemBeGrabbed` lets the player take it.
    Item { weapon: Option<u8>, grabbable: bool },
    /// A mounted gun (`eType` 9).
    Turret,
    /// A `trigger_use` or (`touch`) `trigger_use_touch`.
    Trigger {
        touch: bool,
        team: u8,
        claimer: Option<u16>,
        hint: i32,
        hint_string: i32,
    },
}

/// One entry of the use list.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct UseEntry {
    pub id: EntId,
    pub kind: UseKind,
    pub score: f32,
    /// Where the sight trace aims: the bounds centre, a gun's `tag_aim`.
    pub sight: Vec3,
}

/// The cursor hint fields: `ps.cursorHint`, `cursorHintString` and
/// `cursorHintEntIndex` (ps+0x598, 0x59c, 0x5a0).
pub type Hint = (i32, i32, Option<EntId>);
const NO_HINT: Hint = (0, -1, None);

/// An entry's score off its bounds centre, `None` past the reach:
/// `((dot + 1) * -0.5 + 1) * 256`, less 256 for a `trigger_use`, plus the
/// distance. The penalty is added by the caller between the two.
fn score(eye: Vec3, forward: Vec3, centre: Vec3) -> Option<(f32, f32)> {
    let to = centre - eye;
    let dist = to.length();
    if dist > REACH {
        return None;
    }
    let dir = if dist > 0.0 { to / dist } else { Vec3::ZERO };
    Some((((dir.dot(forward) + 1.0) * -0.5 + 1.0) * ANGLE_WEIGHT, dist))
}

/// `compare_use` (Mac 0x1cc8e2): the score difference truncated to an
/// int, under glibc's merge sort.
fn sort(list: &mut [UseEntry]) {
    crate::game::item::msort(list, |a, b| (a.score - b.score) as i32 <= 0);
}

/// The hint walk of `Player_UpdateCursorHints` off a gun (Mac 0x1ccebe):
/// the first entry that hints wins. A weapon item hints `weapon + 4` with
/// no string unless it is a grenade or one the player holds (`held`); a
/// gun hints when `turret` names its hint and string (usable); a trigger
/// of another team or claimed by another client is passed over, else its
/// cursor hint and string win, and a hint of 0 ends the walk with none.
pub fn first_hint(
    list: &[UseEntry],
    team: i32,
    slot: usize,
    mut item: impl FnMut(u8) -> bool,
    mut turret: impl FnMut(EntId) -> Option<(i32, i32)>,
) -> Hint {
    let set = |id, hint: i32, string| {
        if hint == 0 {
            NO_HINT
        } else {
            (hint, string, Some(id))
        }
    };
    for e in list {
        match e.kind {
            UseKind::Item {
                weapon: Some(w), ..
            } if item(w) => return set(e.id, i32::from(w) + 4, -1),
            UseKind::Item { .. } => {}
            UseKind::Turret => {
                if let Some((hint, string)) = turret(e.id) {
                    return set(e.id, hint, string);
                }
            }
            UseKind::Trigger {
                team: t,
                claimer,
                hint,
                hint_string,
                ..
            } => {
                let team_ok = t == 0 || i32::from(t) == team;
                let claim_ok = claimer.is_none_or(|c| usize::from(c) == slot);
                if team_ok && claim_ok {
                    return set(e.id, hint, hint_string);
                }
            }
        }
    }
    NO_HINT
}

impl Cod2Level {
    /// `Player_GetUseList` (lnxded 0x8121494): the entities with contents
    /// 0x200000 in the box about the eye, in the area walk's order; a
    /// `trigger_use_touch` the player's capsule meets scores -256, anything
    /// else in reach by [`score`], an ungrabbable item 10000 behind; sorted,
    /// the ungrabbable cut, each left but a touch trigger traced to from
    /// the eye (a gun at its `tag_aim`) and a blocked one 10000 behind and
    /// cut; sorted again.
    pub fn use_list(&mut self, slot: usize) -> Vec<UseEntry> {
        let Some(p) = self.players.get(slot).and_then(Option::as_ref) else {
            return Vec::new();
        };
        let eye = Cod2Level::view_origin(&p.ps);
        let forward = Vec3::from(crate::game::spawn::angle_forward(p.ps.view_angles));
        let exact = (
            (p.ps.origin + p.ps.mins).to_array(),
            (p.ps.origin + p.ps.maxs).to_array(),
        );
        let mut list = self.rt.with_host(|host, cx| {
            use crate::game::pickup::{can_grab, item_kind};
            let classname = cx.intern_folded("classname");
            let inv = crate::game::item::inventory(host, slot);
            let weapons = host.weapons.clone();
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
                let centre = (Vec3::from(link.absmin) + Vec3::from(link.absmax)) * 0.5;
                let mut sight = centre;
                let kind = if let Some(item) = host.ents.get(id).and_then(|e| e.item) {
                    if item.taken {
                        continue;
                    }
                    let kind = item_kind(host.defs, item.index as usize);
                    let grabbable = kind.is_some_and(|k| {
                        can_grab(&inv, k, item.owner.map(usize::from), slot, false, &weapons)
                    });
                    let weapon = match kind {
                        Some(crate::game::pickup::ItemKind::Weapon(w)) => Some(w),
                        _ => None,
                    };
                    UseKind::Item { weapon, grabbable }
                } else if let Some(rec) = host.turrets.get(&id) {
                    if let Some(tags) = rec.tags {
                        let origin = Vec3::from(crate::game::item::origin_of(host, cx, id));
                        let angles = crate::game::item::angles_of(host, cx, id);
                        sight = origin + vcod_common::turretpose::angles_quat(angles) * tags.aim;
                    }
                    UseKind::Turret
                } else if let Some(t) = host.triggers.get(id).filter(|t| t.kind == TriggerKind::Use)
                {
                    let t = *t;
                    let touch = matches!(host.get_field(cx, id, classname),
                        Value::String(s) if cx.resolve(s) == "trigger_use_touch");
                    let kind = UseKind::Trigger {
                        touch,
                        team: t.team,
                        claimer: t.claimer,
                        hint: t.cursor_hint,
                        hint_string: t.hint_string,
                    };
                    if touch {
                        if crate::game::trigger::contacts(host, cx, id, &t, exact, true) {
                            list.push(UseEntry {
                                id,
                                kind,
                                score: -256.0,
                                sight,
                            });
                        }
                        continue;
                    }
                    kind
                } else {
                    // INFERRED: no other entity a CoD2 level links with
                    // contents 0x200000 comes between (retail ends the
                    // hint walk on an `eType` 0 one).
                    continue;
                };
                let Some((mut s, dist)) = score(eye, forward, centre) else {
                    continue;
                };
                if matches!(kind, UseKind::Trigger { .. }) {
                    s -= ANGLE_WEIGHT;
                }
                if matches!(
                    kind,
                    UseKind::Item {
                        grabbable: false,
                        ..
                    }
                ) {
                    s += PENALTY;
                }
                list.push(UseEntry {
                    id,
                    kind,
                    score: dist + s,
                    sight,
                });
            }
            list
        });
        let ungrabbable = list
            .iter()
            .filter(|e| {
                matches!(
                    e.kind,
                    UseKind::Item {
                        grabbable: false,
                        ..
                    }
                )
            })
            .count();
        sort(&mut list);
        list.truncate(list.len() - ungrabbable);
        let mut blocked = 0;
        for e in list.iter_mut() {
            if matches!(e.kind, UseKind::Trigger { touch: true, .. }) {
                continue;
            }
            // INFERRED: the world and the brush models stop the trace,
            // not the static models.
            let tr = self
                .world
                .collision
                .point_trace(eye, e.sight, SIGHT_MASK, false);
            if tr.fraction < 1.0 {
                e.score += PENALTY;
                blocked += 1;
            }
        }
        sort(&mut list);
        list.truncate(list.len() - blocked);
        list
    }

    /// `Player_UpdateCursorHints` (Mac 0x1ccebe, lnxded 0x8121bc6), from
    /// `ClientEndFrame`: reset, then for a living player off weapon states
    /// 17..22 either the use list's [`first_hint`] (off a gun, not
    /// mantling) or a gunner's drop hint (`Player_SetTurretDropHint`).
    pub fn update_cursor_hints(&mut self, slot: usize) {
        let Some(p) = self.players[slot].as_ref() else {
            return;
        };
        let alive = self.rt.client_vitals(slot).health > 0;
        let (busy, flags, pm, state) = (p.gunner.busy, p.ps.e_flags, p.ps.pm_flags, p.ws.state);
        let spectating = p.ps.pm_type >= pm_type::SPECTATOR;
        let held = p.ws.held;
        let mut hint = NO_HINT;
        if alive && !(17..=22).contains(&state) {
            if busy == 0 && pm & pmf::MANTLE == 0 && !spectating {
                let list = self.use_list(slot);
                let team = self.rt.client_team(slot);
                let grenade: Vec<bool> = (self.combat.weapons.iter())
                    .map(|d| d.as_ref().is_some_and(|d| d.weap_type == WEAP_TYPE_GRENADE))
                    .collect();
                let item = |w: u8| {
                    held >> w & 1 == 0 && !grenade.get(usize::from(w)).copied().unwrap_or(false)
                };
                // `G_IsTurretUsable` is asked only of the guns the walk
                // reaches, and the hint strings register then.
                let turret = |id| {
                    if !self.turret_usable(slot, id) {
                        return None;
                    }
                    self.gun_keys(id)
                        .map(|k| (i32::from(k.weapon) + 4, k.use_hint))
                };
                hint = first_hint(&list, team, slot, item, turret);
            } else if busy != 0
                && flags & ef::TURRET != 0
                && let Some(id) = self.manned_gun(slot)
                && let Some(k) = self.gun_keys(id)
                && k.drop_hint >= 0
            {
                hint = (i32::from(k.weapon) + 4, k.drop_hint, None);
            }
        }
        let g = &mut self.players[slot].as_mut().unwrap().gunner;
        (g.hint, g.hint_string, g.hint_ent) = hint;
    }

    /// `Player_UpdateActivate` (Mac 0x1cc628, lnxded 0x8121314), last in
    /// `ClientThink_real`. `pm_flags` 0x8 (the reload request) is cleared;
    /// off weapon states 17..22: with a target pending, `+usereload`
    /// released asks for a reload instead; a press of 0x28 clears the
    /// target and either asks a gunner's release (busy byte 2) or takes
    /// the hint's entity with the press time; with a target and 0x28 held
    /// once `g_useholdtime` has run, the target is used. A `+usereload`
    /// press with no target and nothing activated asks for a reload.
    pub fn update_activate(&mut self, slot: usize, cmd: &UserCmd2, oldcmd: &UserCmd2) {
        let level_time = self.level_time();
        let hold_ms = match self.rt.host.cvars.get("g_useholdtime") {
            "" => 0,
            v => crate::game::builtins::cvar::atof(v) as i32,
        };
        let Some(p) = self.players.get_mut(slot).and_then(Option::as_mut) else {
            return;
        };
        p.ps.pm_flags &= !vcod_cod2::pmove::weapon::wpf::RELOAD_REQUEST;
        if (17..=22).contains(&p.ws.state) {
            return;
        }
        let (held, old) = (cmd.buttons, oldcmd.buttons);
        let g = &mut p.gunner;
        if g.use_target.is_some() && old & USE_RELOAD != 0 && held & USE_RELOAD == 0 {
            p.ps.pm_flags |= vcod_cod2::pmove::weapon::wpf::RELOAD_REQUEST;
            return;
        }
        let mut activated = false;
        if held & !old & USE_BUTTONS != 0 {
            g.use_target = None;
            activated = true;
            if g.busy != 0 {
                g.busy = if p.ps.e_flags & ef::TURRET != 0 { 2 } else { 0 };
            } else if p.ps.pm_flags & pmf::MANTLE == 0 {
                g.use_target = g.hint_ent;
                g.use_time = level_time;
                activated = g.use_target.is_some();
            }
        }
        if g.use_target.is_none() && !activated {
            if held & !old & USE_RELOAD != 0 {
                p.ps.pm_flags |= vcod_cod2::pmove::weapon::wpf::RELOAD_REQUEST;
            }
            return;
        }
        // `g_useholdspawndelay` (default 1 ms since the spawn) is not kept.
        if held & USE_BUTTONS != 0
            && g.use_target.is_some()
            && level_time - g.use_time >= hold_ms
            && let Some(target) = g.use_target.take()
        {
            self.use_entity(slot, target);
        }
    }

    /// `Player_UpdateActivate`'s use of the target (lnxded 0x8121054,
    /// `Player_UseEntity`): an item is notified `touch` with the player and
    /// touched with `touched` 0 (the swap); a gun only when
    /// `G_IsTurretUsable`, and anything else, is notified `trigger` with the
    /// player and its use callback runs (`turret_use` for a gun).
    fn use_entity(&mut self, slot: usize, id: EntId) {
        let Some(client) = self.rt.client_entity(slot) else {
            return;
        };
        let is_item = self.rt.host.ents.get(id).and_then(|e| e.item).is_some();
        if is_item {
            self.rt.with_host(|host, cx| {
                host.engine_notifies
                    .push((id, "touch", vec![Value::Entity(client)]));
                super::item::touch(host, cx, id, slot, false);
            });
            return;
        }
        let turret = self.rt.host.turrets.contains_key(&id);
        if turret && !self.turret_usable(slot, id) {
            return;
        }
        if self.rt.host.ents.get(id).is_none() {
            return;
        }
        self.rt
            .host
            .engine_notifies
            .push((id, "trigger", vec![Value::Entity(client)]));
        if turret {
            self.mount_turret(slot, id);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(n: u32, kind: UseKind, score: f32) -> UseEntry {
        UseEntry {
            id: EntId(n, 0),
            kind,
            score,
            sight: Vec3::ZERO,
        }
    }

    fn trigger(team: u8, claimer: Option<u16>, hint: i32) -> UseKind {
        UseKind::Trigger {
            touch: false,
            team,
            claimer,
            hint,
            hint_string: 3,
        }
    }

    /// The score: straight ahead at 64 scores 64, a `trigger_use` beside
    /// it 256 less; behind, 256 more; past 128 nothing.
    #[test]
    fn the_use_list_scores_direction_and_distance() {
        let f = Vec3::X;
        let (s, d) = score(Vec3::ZERO, f, Vec3::new(64.0, 0.0, 0.0)).unwrap();
        assert_eq!((s, d), (0.0, 64.0));
        let (s, _) = score(Vec3::ZERO, f, Vec3::new(-64.0, 0.0, 0.0)).unwrap();
        assert_eq!(s, 256.0);
        assert!(score(Vec3::ZERO, f, Vec3::new(129.0, 0.0, 0.0)).is_none());
        // Truncated comparison: under a unit apart keeps the walk order.
        let mut l = vec![
            entry(1, UseKind::Turret, 10.9),
            entry(2, UseKind::Turret, 10.0),
            entry(3, UseKind::Turret, -300.0),
        ];
        sort(&mut l);
        let ids: Vec<u32> = l.iter().map(|e| e.id.0).collect();
        assert_eq!(ids, [3, 1, 2]);
    }

    /// The hint walk's order: a held weapon, an unusable gun and another
    /// team's trigger are passed over; the first that hints wins, whatever
    /// its kind; a trigger with hint 0 ends the walk with no hint.
    #[test]
    fn the_cursor_hint_takes_the_first_entry_that_hints() {
        let held = |w: u8| w != 5;
        let usable = |id: EntId| (id.0 == 30).then_some((9, 1));
        let list = [
            entry(10, trigger(1, None, 2), -300.0),
            entry(11, trigger(0, Some(4), 2), -290.0),
            entry(
                20,
                UseKind::Item {
                    weapon: Some(5),
                    grabbable: true,
                },
                -10.0,
            ),
            entry(31, UseKind::Turret, 0.0),
            entry(30, UseKind::Turret, 5.0),
            entry(
                21,
                UseKind::Item {
                    weapon: Some(6),
                    grabbable: true,
                },
                6.0,
            ),
        ];
        // Allies (2), slot 0: the axis trigger and slot 4's claim are
        // skipped, the held weapon 5 and gun 31 too: gun 30 wins.
        assert_eq!(
            first_hint(&list, 2, 0, held, usable),
            (9, 1, Some(EntId(30, 0)))
        );
        // Axis: its own trigger comes first.
        assert_eq!(
            first_hint(&list, 1, 0, held, usable),
            (2, 3, Some(EntId(10, 0)))
        );
        // Slot 4 owns the claim on the any-team trigger.
        assert_eq!(
            first_hint(&list, 2, 4, held, usable),
            (2, 3, Some(EntId(11, 0)))
        );
        // With nothing held, weapon 5 hints `5 + 4` with no string.
        assert_eq!(
            first_hint(&list[..4], 2, 0, |_| true, |_| None),
            (9, -1, Some(EntId(20, 0)))
        );
        assert_eq!(
            first_hint(&list, 2, 0, held, |_| None),
            (10, -1, Some(EntId(21, 0)))
        );
        // A hint-0 trigger ends the walk: nothing behind it hints.
        let ended = [entry(12, trigger(0, None, 0), -300.0), list[4]];
        assert_eq!(first_hint(&ended, 2, 0, held, usable), NO_HINT);
    }

    /// The recipe of cod2-gsc-diff.md 5.3's retail check: a player 40
    /// behind a gun, facing along it, and an mp40 30 ahead of him (or
    /// behind) at eye height. Ahead, the item scores first and hints
    /// `weapon + 4` with no string, and the use press takes it, leaving the
    /// gun's hint; behind, the gun hints and the press mounts it.
    #[test]
    fn an_item_and_a_gun_share_one_use_list() {
        for side in [1.0f32, -1.0] {
            let Some(mut f) = crate::game::cod2_fight::fight() else {
                return;
            };
            let mut guns: Vec<EntId> = f.level.rt.host.turrets.keys().copied().collect();
            guns.sort_by_key(|id| id.0);
            let gun = guns[0];
            let (at, angles) = f.level.rt.link_anchor(gun).unwrap();
            let yaw = angles[1];
            let ahead = Vec3::from(crate::game::spawn::angle_forward([0.0, yaw, 0.0]));
            let spot = Vec3::from(at) - ahead * 40.0;
            let floor = f.level.combat.floor_under(spot + Vec3::Z * 16.0).unwrap();
            let p = f.level.players[0].as_mut().unwrap();
            p.ps.origin = floor + Vec3::Z * 0.125;
            p.ps.velocity = Vec3::ZERO;
            f.aim[0] = [0.0, yaw];
            f.frames(4);
            let origin = f.level.players[0].as_ref().unwrap().ps.origin;
            let pos = origin + Vec3::Z * 60.0 + ahead * 30.0 * side;
            // vcod's CoD2 `spawn` makes no weapon item, so it is launched
            // still, under `TR_GRAVITY`: a frame drops it a unit.
            let mp40 = f.level.rt.host.weapon_index("mp40_mp").unwrap();
            let item = f.level.rt.with_host(|host, cx| {
                crate::game::item::launch(host, cx, mp40, pos, Vec3::ZERO, None).unwrap()
            });
            f.frame();
            let mp40 = mp40 as i32;
            let gun_weapon = f.level.gun_keys(gun).unwrap().weapon as i32;
            let hint = |f: &crate::game::cod2_fight::Fight| {
                let g = f.level.players[0].as_ref().unwrap().gunner;
                (g.hint, g.hint_string, g.hint_ent)
            };
            let gun_hint = (
                gun_weapon + 4,
                f.level.gun_keys(gun).unwrap().use_hint,
                Some(gun),
            );
            if side > 0.0 {
                assert_eq!(hint(&f), (mp40 + 4, -1, Some(item)));
            } else {
                assert_eq!(hint(&f), gun_hint);
            }
            f.buttons[0] = 0x8;
            f.frame();
            f.buttons[0] = 0;
            f.frames(2);
            let w = f.level.rt.host.client_weapons[0];
            let mounted = f.level.manned_gun(0) == Some(gun);
            if side > 0.0 {
                assert!(w.holds(mp40 as usize), "the press took the item");
                assert!(!mounted);
                assert_eq!(hint(&f), gun_hint);
            } else {
                assert!(!w.holds(mp40 as usize));
                assert!(mounted, "the press mounted the gun");
            }
            crate::game::cod2_fight::assert_no_script_errors(&mut f);
        }
    }

    /// A `+usereload` tap with nothing to use asks for a reload
    /// (`pm_flags` 0x8), which the next cmd's `PM_Weapon` takes; `+activate`
    /// asks nothing.
    #[test]
    fn a_usereload_tap_with_nothing_to_use_reloads() {
        let Some(mut f) = crate::game::cod2_fight::fight() else {
            return;
        };
        use vcod_cod2::pmove::weapon::wpf::RELOAD_REQUEST;
        let flags = |f: &crate::game::cod2_fight::Fight| {
            f.level.players[0].as_ref().unwrap().ps.pm_flags & RELOAD_REQUEST
        };
        assert_eq!(f.level.players[0].as_ref().unwrap().gunner.hint_ent, None);
        f.buttons[0] = 0x8;
        f.frame();
        assert_eq!(flags(&f), 0);
        f.buttons[0] = 0;
        f.frame();
        f.buttons[0] = USE_RELOAD;
        f.frame();
        assert_ne!(flags(&f), 0);
        f.buttons[0] = 0;
        f.frame();
        assert_eq!(flags(&f), 0, "the next cmd takes it");
    }
}
