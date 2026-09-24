//! Items on the host: the component an item entity carries, the 32-slot
//! drop ring, `Drop_Weapon`'s launch and the inventory the pickup
//! arithmetic (`crate::game::pickup`) runs on. Addresses are in
//! docs/research/cod11-items.md.

use crate::game::entity::{ThinkFn, ENTITYNUM_WORLD};
use crate::game::host::{GameHost, WeaponOp};
use crate::game::pickup::{Dropped, Inventory};
use vcod_common::pmove::weapon::NUM_AMMO;
use vcod_gsc::{Cx, EntId, ErrorKind, Host, Value};

/// What the `gentity_t` of a `bg_itemlist` entity carries beyond its fields.
/// The reserve is the entity's own `count` field (`ent+0x250`), so script
/// reads the same number.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ItemState {
    /// `s.index`, the `bg_itemlist` row.
    pub index: u8,
    /// `ent+0x2cc`: 0 unset, -1 empty.
    pub clip: i32,
    /// `s.clientNum` while the dropper's lockout holds.
    pub owner: Option<u8>,
    /// `flags & 0x10`: freed after a pickup rather than kept hidden.
    pub dropped: bool,
    /// `svFlags & 1` after a pickup: off every snapshot, out of both passes.
    pub taken: bool,
    /// `s.groundEntityNum`: the world once the item has come to rest on it,
    /// 0 for a swap's drop, which never lands (docs/research/cod11-items.md
    /// 12.6).
    pub ground: i32,
}

pub const DROP_RING: usize = 32;
pub const OWNER_LOCKOUT_MS: i32 = 1000;
pub const FREE_AFTER_PICKUP_MS: i32 = 100;

/// `level+0x1d5c`, the dropped-item ring `GetFreeCueSpot` (0x4da44) fills.
pub struct DropRing {
    slots: [Option<EntId>; DROP_RING],
}

impl Default for DropRing {
    fn default() -> Self {
        DropRing {
            slots: [None; DROP_RING],
        }
    }
}

impl DropRing {
    /// Puts `new` in the first slot whose item is gone; with all 32 live,
    /// in slot 0, returning the item it evicts. Slot 0 every time: the
    /// distance score counts only intermission clients (section 8).
    pub fn claim(&mut self, live: impl Fn(EntId) -> bool, new: EntId) -> Option<EntId> {
        if let Some(s) = self.slots.iter_mut().find(|s| s.is_none_or(|id| !live(id))) {
            *s = Some(new);
            return None;
        }
        self.slots[0].replace(new)
    }
}

/// Makes `id` an item of row `index`: placed, owned by nobody, not taken.
pub fn attach(host: &mut GameHost, id: EntId, index: usize) {
    if let Some(e) = host.ents.get_mut(id) {
        e.item = Some(ItemState {
            index: index as u8,
            clip: 0,
            owner: None,
            dropped: false,
            taken: false,
            ground: ENTITYNUM_WORLD as i32,
        });
    }
}

/// The player as the pickup arithmetic sees it, off the host's mirrors.
pub fn inventory(host: &GameHost, slot: usize) -> Inventory {
    let v = host.client_vitals[slot];
    let a = host.client_ammo[slot];
    Inventory {
        weapons: host.client_weapons[slot],
        ammo: a.ammo,
        clip: a.clip,
        health: v.health,
        max_health: v.max_health,
        alive: v.health > 0 && !v.dead,
    }
}

/// What a pickup or a drop changed, back onto the host: the weapons and
/// health directly, the ammo as ops for the sim.
pub fn write_back(host: &mut GameHost, slot: usize, before: &Inventory, after: &Inventory) {
    host.client_weapons[slot] = after.weapons;
    for i in 0..NUM_AMMO {
        if after.ammo[i] != before.ammo[i] {
            host.weapon_op(
                slot,
                WeaponOp::SetAmmo {
                    ammo_index: i,
                    rounds: after.ammo[i],
                },
            );
        }
        if after.clip[i] != before.clip[i] {
            host.weapon_op(
                slot,
                WeaponOp::SetClip {
                    clip_index: i,
                    rounds: after.clip[i],
                },
            );
        }
    }
    host.client_vitals[slot].health = after.health;
}

/// Where a launched weapon comes to rest.
pub enum DropAt {
    /// Under the dropper, found by the floor trace that stands in for
    /// `G_RunItem`'s fall.
    Feet,
    /// A swap's drop: exactly where the item it was swapped for lay.
    Exactly { origin: [f32; 3], angles: [f32; 3] },
}

/// `LaunchItem` (0x4db98) for a weapon `Drop_Weapon` took off `slot`.
pub fn launch_weapon(
    host: &mut GameHost,
    cx: &mut Cx,
    slot: usize,
    d: Dropped,
    at: DropAt,
) -> Result<EntId, ErrorKind> {
    let name = crate::items::item_name(d.weapon as usize)
        .unwrap_or_default()
        .to_string();
    let origin = match at {
        DropAt::Feet => {
            let player = host
                .ents
                .handle(slot as u32)
                .ok_or(ErrorKind::BadType("no such player"))?;
            let field = cx.intern_folded("origin");
            match host.get_field(cx, player, field) {
                Value::Vector(v) => v,
                _ => [0.0; 3],
            }
        }
        DropAt::Exactly { origin, .. } => origin,
    };
    let id = host.ents.spawn(cx)?;
    let classname = crate::game::spawn::radiant_name(&name).unwrap_or("mpweapon_dropped");
    let mut fields = vec![
        ("classname", Value::String(cx.intern_exact(classname))),
        ("origin", Value::Vector(origin)),
        ("count", Value::Int(d.count)),
    ];
    if let DropAt::Exactly { angles, .. } = at {
        fields.push(("angles", Value::Vector(angles)));
    }
    for (field, value) in fields {
        let atom = cx.intern_folded(field);
        host.set_field(cx, id, atom, value)?;
    }
    host.register_item(&name);
    if matches!(at, DropAt::Feet) {
        crate::game::spawn::drop_item_to_floor(host, cx, id, true);
    }
    if let Some(e) = host.ents.get_mut(id) {
        e.item = Some(ItemState {
            index: d.weapon,
            clip: d.clip,
            owner: Some(slot as u8),
            dropped: true,
            taken: false,
            ground: match at {
                DropAt::Feet => ENTITYNUM_WORLD as i32,
                DropAt::Exactly { .. } => 0,
            },
        });
    }
    let now = host.level_time_ms;
    host.ents
        .schedule(id, ThinkFn::ClearOwner, now + OWNER_LOCKOUT_MS);
    let ents = &host.ents;
    if let Some(old) = host.drop_ring.claim(|i| ents.get(i).is_some(), id) {
        host.ents.schedule(old, ThinkFn::Free, now + 1);
    }
    Ok(id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game::testing::fixture;

    /// `GetFreeCueSpot` with every slot held evicts slot 0, and a slot whose
    /// item is gone is reused before anything is evicted.
    #[test]
    fn the_thirty_third_drop_evicts_slot_zero() {
        let mut vm = vcod_gsc::Vm::new();
        let mut ents = crate::game::entity::ObjectTable::new();
        let mut ring = DropRing::default();
        let ids: Vec<EntId> = (0..33)
            .map(|_| vm.with_cx(|cx| ents.spawn(cx).unwrap()))
            .collect();
        for id in &ids[..32] {
            assert_eq!(ring.claim(|i| ents.get(i).is_some(), *id), None);
        }
        assert_eq!(ring.claim(|i| ents.get(i).is_some(), ids[32]), Some(ids[0]));
        ents.free(ids[5]);
        let next = vm.with_cx(|cx| ents.spawn(cx).unwrap());
        assert_eq!(ring.claim(|i| ents.get(i).is_some(), next), None);
    }

    /// A full ring whose slot-0 drop the script deleted: the next drop takes
    /// that slot, and the entity that reused the deleted number, a different
    /// generation, is never the one evicted. The drop after that finds the
    /// ring full again and evicts slot 0 once more, the newest drop.
    #[test]
    fn a_deleted_drop_frees_its_ring_slot_and_a_reused_number_is_never_evicted() {
        let mut vm = vcod_gsc::Vm::new();
        let mut ents = crate::game::entity::ObjectTable::new();
        let mut ring = DropRing::default();
        let first = vm.with_cx(|cx| ents.spawn(cx).unwrap());
        assert_eq!(ring.claim(|i| ents.get(i).is_some(), first), None);
        for _ in 0..31 {
            let id = vm.with_cx(|cx| ents.spawn(cx).unwrap());
            assert_eq!(ring.claim(|i| ents.get(i).is_some(), id), None);
        }
        ents.free(first);
        let reuse = vm.with_cx(|cx| ents.spawn(cx).unwrap());
        assert_eq!(reuse.0, first.0);
        let a = vm.with_cx(|cx| ents.spawn(cx).unwrap());
        assert_eq!(ring.claim(|i| ents.get(i).is_some(), a), None);
        let b = vm.with_cx(|cx| ents.spawn(cx).unwrap());
        assert_eq!(ring.claim(|i| ents.get(i).is_some(), b), Some(a));
    }

    /// A placed fg42 carries its item index, on the wire as `index` with
    /// `clientNum` 254; once taken it leaves every snapshot.
    #[test]
    fn a_placed_item_is_on_the_wire_until_it_is_taken() {
        let (mut vm, mut host) = fixture();
        let fg = crate::configstrings::weapon_index("fg42_mp").unwrap();
        let id = vm.with_cx(|cx| {
            let id = host.ents.spawn(cx).unwrap();
            let cn = cx.intern_folded("classname");
            let v = Value::String(cx.intern_exact("mpweapon_fg42"));
            host.set_field(cx, id, cn, v).unwrap();
            id
        });
        attach(&mut host, id, fg);
        let p = &vcod_common::net::protocol::PROTOCOL_V1;
        let ents = vm.with_cx(|cx| crate::game::wire::packet_entities(&mut host, cx, p));
        let e = &ents[&id.0];
        assert_eq!(e.field_i32(p, "eType"), 3);
        assert_eq!(e.field_i32(p, "index"), fg as i32);
        assert_eq!(e.field_i32(p, "clientNum"), 254);
        host.ents.get_mut(id).unwrap().item.as_mut().unwrap().taken = true;
        let ents = vm.with_cx(|cx| crate::game::wire::packet_entities(&mut host, cx, p));
        assert!(!ents.contains_key(&id.0));
    }

    /// A drop carries its dropper in `clientNum` until the lockout clears.
    /// `groundEntityNum` is how it arrived: a `dropItem` drop has landed on
    /// the world, a swap's drop never flew and reads 0, as the retail swap
    /// does (docs/research/cod11-items.md 12.6).
    #[test]
    fn a_drop_names_its_dropper_and_its_ground_is_how_it_arrived() {
        let (mut vm, mut host) = fixture();
        let fg = crate::configstrings::weapon_index("fg42_mp").unwrap() as u8;
        let d = Dropped {
            weapon: fg,
            count: 70,
            clip: 20,
        };
        let (id, swap) = vm.with_cx(|cx| {
            host.ents.spawn_client(cx, 3, None).unwrap();
            let id = launch_weapon(&mut host, cx, 3, d, DropAt::Feet).unwrap();
            let at = DropAt::Exactly {
                origin: [826.0, 2274.0, -22.8],
                angles: [0.0, 270.0, 90.0],
            };
            (id, launch_weapon(&mut host, cx, 3, d, at).unwrap())
        });
        let p = &vcod_common::net::protocol::PROTOCOL_V1;
        let ents = vm.with_cx(|cx| crate::game::wire::packet_entities(&mut host, cx, p));
        assert_eq!(ents[&id.0].field_i32(p, "clientNum"), 3);
        assert_eq!(ents[&id.0].field_i32(p, "groundEntityNum"), 1022);
        assert_eq!(ents[&swap.0].field_i32(p, "groundEntityNum"), 0);
        host.run_entity_thinks(OWNER_LOCKOUT_MS);
        let ents = vm.with_cx(|cx| crate::game::wire::packet_entities(&mut host, cx, p));
        assert_eq!(ents[&id.0].field_i32(p, "clientNum"), 254);
    }
}
