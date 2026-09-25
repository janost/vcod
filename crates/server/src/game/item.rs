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
    let (origin, angles) = match at {
        // The dropper's yaw seeds the alignment the landing does. The launch
        // is from the box's mid-height (items.md section 8): a grounded
        // origin sits within a quarter unit of the floor, so the landing
        // trace would start inside it.
        DropAt::Feet => {
            let player = host
                .ents
                .handle(slot as u32)
                .ok_or(ErrorKind::BadType("no such player"))?;
            let mut read = |name: &str| {
                let field = cx.intern_folded(name);
                match host.get_field(cx, player, field) {
                    Value::Vector(v) => v,
                    _ => [0.0; 3],
                }
            };
            let mut origin = read("origin");
            let yaw = read("angles")[1];
            origin[2] += host.client_height.get(slot).copied().unwrap_or(0.0) * 0.5;
            (origin, [0.0, yaw, 0.0])
        }
        DropAt::Exactly { origin, angles } => (origin, angles),
    };
    let id = host.ents.spawn(cx)?;
    let classname = crate::game::spawn::radiant_name(&name).unwrap_or("mpweapon_dropped");
    for (field, value) in [
        ("classname", Value::String(cx.intern_exact(classname))),
        ("origin", Value::Vector(origin)),
        ("angles", Value::Vector(angles)),
        ("count", Value::Int(d.count)),
    ] {
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

pub fn origin_of(host: &mut GameHost, cx: &mut Cx, id: EntId) -> [f32; 3] {
    let field = cx.intern_folded("origin");
    match host.get_field(cx, id, field) {
        Value::Vector(v) => v,
        _ => [0.0; 3],
    }
}

/// Any entity's `angles`, zero when unset.
pub fn angles_of(host: &mut GameHost, cx: &mut Cx, id: EntId) -> [f32; 3] {
    let field = cx.intern_folded("angles");
    match host.get_field(cx, id, field) {
        Value::Vector(v) => v,
        _ => [0.0; 3],
    }
}

fn live_items(host: &GameHost) -> Vec<(EntId, ItemState)> {
    host.ents
        .iter_inuse()
        .filter_map(|(id, e)| e.item.filter(|i| !i.taken).map(|i| (id, i)))
        .collect()
}

/// The items `G_TouchTriggers` would hand `Touch_Item` for a player at
/// `player`, ascending entity number.
pub fn touching(host: &mut GameHost, cx: &mut Cx, player: [f32; 3]) -> Vec<EntId> {
    live_items(host)
        .into_iter()
        .map(|(id, _)| id)
        .filter(|&id| crate::game::pickup::touches(player, origin_of(host, cx, id)))
        .collect()
}

/// What the use key and the cursor hint found: an item to take or a turret
/// to man.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Activate {
    Item(EntId),
    Turret(EntId),
}

/// A turret's bounds centre above its origin: `G_SpawnTurret`'s box is
/// (-32, -32, 0) to (32, 32, 56) (`docs/research/cod11-turrets.md` 3, 4.1).
const TURRET_CENTRE_Z: f32 = 28.0;

/// `G_GetActivateEnt`'s choice (section 2.1): the best-scoring grabbable
/// item or usable turret in reach whose centre the muzzle can see past the
/// world. Retail scores an ungrabbable item 10000 behind and cuts it off the
/// list; an unusable turret is scored the same way here, but retail traces
/// first and only then steps its use/hint loop past a turret
/// `G_IsTurretUsable` refuses, so a refused turret there still spends a
/// trace ours never takes (turrets doc 13).
pub fn activate_ent(
    host: &mut GameHost,
    cx: &mut Cx,
    slot: usize,
    eye: [f32; 3],
    view: [f32; 3],
) -> Option<Activate> {
    use crate::game::pickup::{activate_score, can_grab, item_kind};
    let muzzle = glam::Vec3::from(eye).trunc().to_array();
    let forward = crate::game::spawn::angle_forward(view);
    let inv = inventory(host, slot);
    let weapons = host.weapons.clone();
    let mut scored: Vec<(Activate, f32, [f32; 3])> = Vec::new();
    for (id, item) in live_items(host) {
        let Some(kind) = item_kind(item.index as usize) else {
            continue;
        };
        if !can_grab(
            &inv,
            kind,
            item.owner.map(usize::from),
            slot,
            false,
            &weapons,
        ) {
            continue;
        }
        let centre = origin_of(host, cx, id);
        if let Some(s) = activate_score(muzzle, forward, centre) {
            scored.push((Activate::Item(id), s, centre));
        }
    }
    let player = host
        .ents
        .handle(slot as u32)
        .map_or([0.0; 3], |c| origin_of(host, cx, c));
    let grenade_ms = host.client_grenade_ms.get(slot).copied().unwrap_or(0);
    let on_ground = host.client_on_ground.get(slot).copied().unwrap_or(false);
    let mut turrets: Vec<EntId> = host.turrets.keys().copied().collect();
    turrets.sort();
    for id in turrets {
        if host.ents.get(id).is_none() {
            continue;
        }
        let origin = origin_of(host, cx, id);
        let yaw = angles_of(host, cx, id)[1];
        let rec = &host.turrets[&id];
        if !crate::game::turret::usable(rec, origin, yaw, player, grenade_ms, on_ground) {
            continue;
        }
        let centre = [origin[0], origin[1], origin[2] + TURRET_CENTRE_Z];
        if let Some(s) = activate_score(muzzle, forward, centre) {
            scored.push((Activate::Turret(id), s, centre));
        }
    }
    scored.sort_by(|a, b| a.1.total_cmp(&b.1));
    let world = host.world.clone();
    scored.into_iter().find_map(|(hit, _, c)| {
        let blocked = world.as_ref().is_some_and(|w| {
            let tr = w.collision.point_trace(
                muzzle.into(),
                c.into(),
                vcod_common::collision::CONTENTS_SOLID | vcod_common::collision::CONTENTS_GLASS,
                false,
            );
            tr.fraction < 1.0 && w.collision.entity_num(&tr) == ENTITYNUM_WORLD
        });
        (!blocked).then_some(hit)
    })
}

/// `Touch_Item` (section 7) for `slot` on item `id`, `touched` for the walk
/// and not the use key: the arithmetic, then its writes.
pub fn touch(host: &mut GameHost, cx: &mut Cx, id: EntId, slot: usize, touched: bool) {
    use crate::game::pickup::{ItemCounts, ItemKind, ItemView};
    let Some(state) = host.ents.get(id).and_then(|e| e.item) else {
        return;
    };
    if state.taken {
        return;
    }
    let count_atom = cx.intern_folded("count");
    let count = match host.get_field(cx, id, count_atom) {
        Value::Int(n) => n,
        _ => 0,
    };
    let classname_atom = cx.intern_folded("classname");
    let classname = match host.get_field(cx, id, classname_atom) {
        Value::String(s) => cx.resolve(s).to_string(),
        _ => String::new(),
    };
    let mut counts = ItemCounts {
        count,
        clip: state.clip,
    };
    let before = inventory(host, slot);
    let mut inv = before;
    let pools = host.cvars.get("g_weaponAmmoPools") == "1";
    let weapons = host.weapons.clone();
    let out = crate::game::pickup::touch_item(
        &mut inv,
        &ItemView {
            index: state.index as usize,
            owner: state.owner.map(usize::from),
            classname: &classname,
        },
        &mut counts,
        slot,
        touched,
        &weapons,
        pools,
        &mut || host.rand_int(),
    );
    write_back(host, slot, &before, &inv);
    let _ = host.set_field(cx, id, count_atom, Value::Int(counts.count));
    if let Some(i) = host.ents.get_mut(id).and_then(|e| e.item.as_mut()) {
        i.clip = counts.clip;
    }
    if let Some(line) = out.log {
        log::info!("script: {line}");
        host.script_log.push(line);
    }
    for c in out.commands {
        host.client_commands.push((slot, c));
    }
    if !out.taken {
        return;
    }
    let origin = origin_of(host, cx, id);
    let angles_atom = cx.intern_folded("angles");
    let angles = match host.get_field(cx, id, angles_atom) {
        Value::Vector(v) => v,
        _ => [0.0; 3],
    };
    let swapped = out.drop.and_then(|d| {
        launch_weapon(host, cx, slot, d, DropAt::Exactly { origin, angles })
            .inspect_err(|e| {
                log::warn!(
                    "client {slot}: swap drop of weapon {} failed, the weapon is lost: {e:?}",
                    d.weapon
                )
            })
            .ok()
    });
    if let Some(player) = host.ents.handle(slot as u32) {
        let args = match crate::game::pickup::item_kind(state.index as usize) {
            Some(ItemKind::Weapon(_)) => vec![
                Value::Entity(player),
                swapped.map_or(Value::Undefined, Value::Entity),
            ],
            _ => vec![Value::Entity(player)],
        };
        host.item_notifies.push((id, "trigger", args));
    }
    if let Some(event) = out.event {
        host.client_sim_ops.push((
            slot,
            crate::game::host::SimOp::Event {
                event,
                parm: i32::from(state.index),
            },
        ));
    }
    if let Some(e) = host.ents.get_mut(id) {
        if let Some(i) = e.item.as_mut() {
            i.taken = true;
        }
        e.think = None;
        e.nextthink = 0;
    }
    if state.dropped {
        let at = host.level_time_ms + FREE_AFTER_PICKUP_MS;
        host.ents.schedule(id, ThinkFn::Free, at);
    }
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
        vm.with_cx(|cx| host.run_entity_thinks(cx, OWNER_LOCKOUT_MS));
        let ents = vm.with_cx(|cx| crate::game::wire::packet_entities(&mut host, cx, p));
        assert_eq!(ents[&id.0].field_i32(p, "clientNum"), 254);
    }

    /// A death drop lands the way retail's `G_BounceItem` lays it: its box
    /// clear of the floor, facing the dropper's yaw, with a weapon's 90
    /// degrees of roll. The dropper stands where pmove leaves a grounded
    /// player, a fraction above the floor, so a trace from its bare origin
    /// would start inside the floor.
    #[test]
    fn a_feet_drop_faces_the_droppers_yaw_and_takes_the_weapon_roll() {
        let (mut vm, mut host) = fixture();
        host.world = Some(std::rc::Rc::new(crate::world::World {
            collision: vcod_common::collision::test_world(&[]),
            vis: vcod_common::bsp::Visibility::none(),
            spawn: ([0.0, 0.0, 64.0], 0.0),
        }));
        let carbine = crate::configstrings::weapon_index("m1carbine_mp").unwrap() as u8;
        let d = Dropped {
            weapon: carbine,
            count: 400,
            clip: 15,
        };
        vm.with_cx(|cx| {
            let c = host.ents.spawn_client(cx, 0, None).unwrap();
            for (name, v) in [
                ("origin", [16.0, -32.0, 0.125]),
                ("angles", [10.0, 45.0, 0.0]),
            ] {
                let f = cx.intern_folded(name);
                host.set_field(cx, c, f, Value::Vector(v)).unwrap();
            }
            let id = launch_weapon(&mut host, cx, 0, d, DropAt::Feet).unwrap();
            let origin = cx.intern_folded("origin");
            let Value::Vector(o) = host.get_field(cx, id, origin) else {
                panic!("the drop has an origin");
            };
            let rest = 1.0 + vcod_common::collision::SURFACE_CLIP_EPSILON;
            assert!((o[2] - rest).abs() < 1e-3, "z {}", o[2]);
            let angles = cx.intern_folded("angles");
            let Value::Vector(a) = host.get_field(cx, id, angles) else {
                panic!("the drop has angles");
            };
            assert!(a[0].abs() < 1e-3, "pitch {}", a[0]);
            assert!((a[1] - 45.0).abs() < 1e-3, "yaw {}", a[1]);
            assert!((a[2] - 90.0).abs() < 1e-3, "roll {}", a[2]);
        });
    }
}
