//! Items on the host: the component an item entity carries, the 32-slot
//! drop ring, `Drop_Weapon`'s and `Drop_Item`'s launch, `G_RunItem`'s
//! flight, the respawn, and the inventory the pickup arithmetic
//! (`crate::game::pickup`) runs on. Addresses are in
//! docs/research/cod11-items.md.

use crate::game::entity::{ENTITYNUM_WORLD, ThinkFn};
use crate::game::host::{GameHost, WeaponOp};
use crate::game::pickup::{Dropped, Inventory};
use glam::Vec3;
use vcod_common::collision::{CONTENTS_NODROP, CollisionWorld};
use vcod_common::net::protocol::ENTITYNUM_NONE;
use vcod_common::net::trajectory::{TR_GRAVITY, TR_LINEAR, TR_STATIONARY, Trajectory};
use vcod_common::pmove::weapon::NUM_AMMO;
use vcod_gsc::{Cx, EntId, ErrorKind, Host, Value};

/// What the `gentity_t` of a `bg_itemlist` entity carries beyond its fields.
/// The reserve is the entity's own `count` field (`ent+0x250`), so script
/// reads the same number.
#[derive(Clone, Copy, Debug, PartialEq)]
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
    /// `s.groundEntityNum`: what the landing came to rest on, `ENTITYNUM_NONE`
    /// for a script spawn still to fall, 0 for a drop in the air and for a
    /// swap's drop, which never lands (docs/research/cod11-items.md 12.6).
    pub ground: i32,
    /// `s.pos` while it is not stationary, which is what `G_RunItem` flies;
    /// `None` at rest, where the wire carries the `origin` field.
    pub pos: Option<Trajectory>,
    /// `s.apos` while a `dropItem` spin runs; the landing stops it.
    pub apos: Option<Trajectory>,
    /// `clipmask`: `LaunchItem`'s 0x81, or 0, which `G_RunItem` reads as
    /// 0x491.
    pub clipmask: u32,
    /// `s.eFlags & 0x100`. `G_RunEntity` copies the pickup's hide
    /// (`flags & 0x1000`) into it at the top of every frame (0x502e6), so
    /// the frame a respawn runs still carries it (section 14).
    pub nodraw: bool,
}

pub const DROP_RING: usize = 32;
pub const OWNER_LOCKOUT_MS: i32 = 1000;
pub const FREE_AFTER_PICKUP_MS: i32 = 100;
/// `EV_ITEM_RESPAWN`, which `RespawnItem` puts on the item (0x4ed0f).
pub use vcod_common::net::event_ids::EV_ITEM_RESPAWN;
/// `s.eFlags` bit the hide mirrors into (section 14).
pub use vcod_common::net::flags::EF_NODRAW;
/// `Drop_Weapon`'s and `Drop_Item`'s horizontal launch speed (0x74d4c,
/// 0x74e5c); the vertical one is `200 + 50 * crandom()`.
const LAUNCH_SPEED: f32 = 150.0;
const LAUNCH_UP: f32 = 200.0;
const LAUNCH_UP_SPREAD: f32 = 50.0;
/// The tag drop's spin, degrees a second, times `crandom()` (0x74d54,
/// 0x74d68, 0x74d6c).
const SPIN: [f32; 3] = [50.0, 40.0, 60.0];
/// `LaunchItem`'s `clipmask` (0x4dca1) and the 0x491 `G_RunItem` falls back
/// to when the item has none (0x4eb7f).
pub const LAUNCH_CLIPMASK: u32 = 0x81;
pub const RUN_CLIPMASK: u32 = 0x491;
/// The mask of `Drop_Weapon`'s tag trace (0x4e126) and `G_BounceItem`'s
/// start-solid trace (0x4e95f).
pub const TAG_CLIPMASK: u32 = 0x411;
/// How far `G_BounceItem` looks down from a start-solid item (0x74e48).
const START_SOLID_DROP: f32 = 128.0;

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

/// `r.contents` of an item lying in the world: `G_SpawnItem` (0x4e778) and
/// `LaunchItem` (0x4dc97).
pub const CONTENTS_ITEM: i32 = 0x407c_0108;
/// `RespawnItem`'s (0x4ece9), without the 0x100.
pub const CONTENTS_RESPAWNED: i32 = 0x407c_0008;

/// An item's contents and its link, with the box its row takes.
pub fn link(host: &mut GameHost, cx: &mut Cx, id: EntId, contents: i32) {
    let Some(index) = host.ents.get(id).and_then(|e| e.item).map(|i| i.index) else {
        return;
    };
    let (mins, maxs) = bounds(crate::game::spawn::is_weapon_row(index as usize));
    let shape = crate::game::entity::LinkShape {
        kind: crate::game::entity::LinkKind::Item,
        contents,
        mins: mins.into(),
        maxs: maxs.into(),
    };
    host.link_shaped(cx, id, shape);
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
            pos: None,
            apos: None,
            clipmask: 0,
            nodraw: false,
        });
    }
}

/// `G_SpawnItem` outside the map load (section 9): what a script `spawn` of a
/// `bg_itemlist` classname makes. Linked in place; unless `spawnflags & 1`
/// it gets `groundEntityNum` `ENTITYNUM_NONE`, which `G_RunItem` drops under
/// gravity from the frame's own run, and a weapon takes 90 degrees of roll.
pub fn spawn_in_place(host: &mut GameHost, cx: &mut Cx, id: EntId, index: usize, spawnflags: i32) {
    attach(host, id, index);
    let suspended = spawnflags & 1 != 0;
    let roll = if !suspended && crate::game::spawn::is_weapon_row(index) {
        90.0
    } else {
        0.0
    };
    let angles = cx.intern_folded("angles");
    let _ = host.set_field(cx, id, angles, Value::Vector([0.0, 0.0, roll]));
    if let Some(i) = host.ents.get_mut(id).and_then(|e| e.item.as_mut()) {
        i.ground = if suspended { 0 } else { ENTITYNUM_NONE as i32 };
    }
    link(host, cx, id, CONTENTS_ITEM);
}

/// The item's box: `G_SpawnItem` (0x4e6e1) and `LaunchItem` give a weapon
/// (-1, -1, -1) to (1, 1, 1) and any other row (-1, -1, 0) to (1, 1, 2).
pub fn bounds(weapon: bool) -> (Vec3, Vec3) {
    if weapon {
        (Vec3::splat(-1.0), Vec3::splat(1.0))
    } else {
        (Vec3::new(-1.0, -1.0, 0.0), Vec3::new(1.0, 1.0, 2.0))
    }
}

/// `rand() * -2^-31`, the draw every item constant multiplies (0x74d50,
/// 0x74e50): a uniform in (-1, 0].
fn neg_unit(host: &mut GameHost) -> f32 {
    -(host.rand_int() as f32 / 2_147_483_648.0)
}

/// The module's `crandom()` as compiled, `2 * neg_unit() - 1`: it reads
/// (-3, -1] rather than Q3's (-1, 1), so a launch climbs at 50 to 150
/// units a second and every spin runs negative (section 8).
pub fn crandom(host: &mut GameHost) -> f32 {
    2.0 * neg_unit(host) - 1.0
}

/// What one `G_RunItem` did to an item in the air.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Ran {
    /// Still in the air, at the sweep's end.
    Flew,
    /// Met something that is not ground: moved off it along its normal and
    /// dropped again from rest.
    Nudged,
    /// Came to rest on `ground`, aligned to `normal`.
    Landed { ground: u32, normal: Vec3 },
    /// Met something with its origin in `CONTENTS_NODROP`, and is freed.
    NoDrop,
}

/// `G_RunItem` (0x4eb18) and `G_BounceItem` (0x4e858) for an item whose
/// `pos` is flying: sweep from `origin` to where the arc is at `now_ms`,
/// then free, nudge or land. Every item's `physicsBounce` is 0 (stored by
/// `G_SpawnItem` at 0x4e6d7, never by `LaunchItem`), so a contact keeps no
/// velocity: a floor stops it and a wall drops it straight down. `lift` is
/// drawn only on a landing, `0.5 + 0.5 * neg_unit()` above the sweep's end.
pub fn run_flight(
    world: &CollisionWorld,
    pos: &mut Trajectory,
    origin: &mut Vec3,
    (mins, maxs): (Vec3, Vec3),
    mask: u32,
    now_ms: i32,
    lift: &mut dyn FnMut() -> f32,
) -> Ran {
    let to = pos.evaluate(now_ms);
    let mut tr = world.item_trace(*origin, to, mins, maxs, mask);
    *origin = tr.endpos;
    if tr.startsolid {
        tr.fraction = 0.0;
    }
    if tr.fraction >= 1.0 {
        return Ran::Flew;
    }
    if world.point_contents(*origin) & CONTENTS_NODROP != 0 {
        return Ran::NoDrop;
    }
    if tr.startsolid {
        let down = *origin - Vec3::Z * START_SOLID_DROP;
        tr = world.item_trace(*origin, down, mins, maxs, TAG_CLIPMASK);
    }
    pos.delta = Vec3::ZERO;
    if tr.normal.z > 0.0 {
        let mut end = tr.endpos;
        end.z += lift();
        *origin = end;
        *pos = stationary(end);
        return Ran::Landed {
            ground: world.entity_num(&tr),
            normal: tr.normal,
        };
    }
    *origin += tr.normal;
    pos.base = *origin;
    pos.tr_time = now_ms;
    Ran::Nudged
}

/// `G_SetOrigin`'s trajectory: stationary at `at`, time and delta cleared.
fn stationary(at: Vec3) -> Trajectory {
    Trajectory {
        tr_type: TR_STATIONARY,
        tr_time: 0,
        tr_duration: 0,
        base: at,
        delta: Vec3::ZERO,
    }
}

/// The `G_RunEntity` pass over every item (0x502bc): the hide mirrored into
/// `eFlags`, `G_RunItem`'s flight, then the respawn think. Runs after the
/// frame's script threads, as retail's entity loop does: a thread that reads
/// a flying item's `origin` reads the last frame's, and an item a thread
/// spawns starts falling on the frame it was spawned in (section 14).
pub fn run_items(host: &mut GameHost, cx: &mut Cx, now_ms: i32) {
    let ids: Vec<EntId> = host
        .ents
        .iter_inuse()
        .filter(|(_, e)| e.item.is_some())
        .map(|(id, _)| id)
        .collect();
    for id in ids {
        run_item(host, cx, id, now_ms);
    }
}

/// [`run_items`] for one entity, on its turn in `G_RunFrame`'s entity loop.
/// Returns whether `id` is an item this arm ran.
pub fn run_item(host: &mut GameHost, cx: &mut Cx, id: EntId, now_ms: i32) -> bool {
    // A linked item takes the runner's link arm instead (0x50385).
    if host.links.contains(id) {
        return false;
    }
    let Some(mut st) = host.ents.get(id).and_then(|e| e.item) else {
        return false;
    };
    let world = host.world.clone();
    let origin_atom = cx.intern_folded("origin");
    let angles_atom = cx.intern_folded("angles");
    {
        st.nodraw = st.taken;
        if st.ground == ENTITYNUM_NONE as i32 && st.pos.is_none_or(|p| p.tr_type != TR_GRAVITY) {
            let base = st
                .pos
                .map_or_else(|| Vec3::from(origin_of(host, cx, id)), |p| p.base);
            st.pos = Some(Trajectory {
                tr_type: TR_GRAVITY,
                tr_time: now_ms,
                tr_duration: 0,
                base,
                delta: st.pos.map_or(Vec3::ZERO, |p| p.delta),
            });
        }
        if let (Some(mut pos), Some(world)) = (st.pos, world.as_deref()) {
            let weapon = crate::game::spawn::is_weapon_row(st.index as usize);
            let mask = if st.clipmask != 0 {
                st.clipmask
            } else {
                RUN_CLIPMASK
            };
            let mut origin = Vec3::from(origin_of(host, cx, id));
            let ran = run_flight(
                &world.collision,
                &mut pos,
                &mut origin,
                bounds(weapon),
                mask,
                now_ms,
                &mut || 0.5 + 0.5 * neg_unit(host),
            );
            let _ = host.set_field(cx, id, origin_atom, Value::Vector(origin.into()));
            match ran {
                Ran::Flew | Ran::Nudged => st.pos = Some(pos),
                Ran::Landed { ground, normal } => {
                    st.pos = None;
                    st.apos = None;
                    st.ground = ground as i32;
                    let current = angles_of(host, cx, id);
                    let aligned =
                        crate::game::spawn::align_to_surface(current, normal.into(), weapon);
                    let _ = host.set_field(cx, id, angles_atom, Value::Vector(aligned));
                }
                Ran::NoDrop => {
                    host.free_entity(id);
                    return true;
                }
            }
        }
        let respawn = host.ents.get(id).is_some_and(|e| {
            e.think == Some(ThinkFn::RespawnItem) && e.nextthink != 0 && e.nextthink <= now_ms
        });
        if let Some(e) = host.ents.get_mut(id) {
            if respawn {
                // `RespawnItem` (0x4ec7c): unhidden, relinked, the event on
                // the item itself, and no further think.
                st.taken = false;
                e.events.add(EV_ITEM_RESPAWN, 0);
                e.think = None;
                e.nextthink = 0;
            }
            e.item = Some(st);
        }
        if respawn {
            link(host, cx, id, CONTENTS_RESPAWNED);
        }
    }
    true
}

/// A mover's push over the items (`crate::push::push_item`), in entity
/// order. Runs after the players' push and only when that did not stall the
/// mover; an item never stalls one. A pushed item gets the spot as its
/// `origin` and its `s.pos.trBase` (0x54b84), and its ground, unless it
/// stood on the mover, is `ENTITYNUM_NONE`, which the next `run_items`
/// drops it from (docs/research/cod11-movers.md, section 12).
pub fn push_items(host: &mut GameHost, cx: &mut Cx, step: &crate::game::mover::Step) {
    let Some(world) = host.world.clone() else {
        return;
    };
    let ids: Vec<EntId> = host
        .ents
        .iter_inuse()
        .filter(|(_, e)| e.item.is_some_and(|i| !i.taken))
        .map(|(id, _)| id)
        .collect();
    let origin_atom = cx.intern_folded("origin");
    for id in ids {
        let Some(mut st) = host.ents.get(id).and_then(|e| e.item) else {
            continue;
        };
        let (mins, maxs) = bounds(crate::game::spawn::is_weapon_row(st.index as usize));
        let body = crate::push::ItemBody {
            origin: Vec3::from(origin_of(host, cx, id)),
            mins,
            maxs,
            mask: if st.clipmask != 0 {
                st.clipmask
            } else {
                crate::push::PUSH_DEFAULT_MASK
            },
            ground: st.ground,
        };
        let Some(fit) = crate::push::push_item(step, &body, &world.collision) else {
            continue;
        };
        if st.ground != step.number as i32 {
            st.ground = ENTITYNUM_NONE as i32;
        }
        if let crate::push::ItemPush::At(at) = fit {
            if let Some(pos) = st.pos.as_mut() {
                pos.base = at;
            }
            let _ = host.set_field(cx, id, origin_atom, Value::Vector(at.into()));
        } else {
            st.ground = ENTITYNUM_NONE as i32;
        }
        if let Some(e) = host.ents.get_mut(id) {
            e.item = Some(st);
        }
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

/// Where a dropped weapon starts.
pub enum DropAt {
    /// `dropItem`'s: launched off the dropper, from `tag` on its model.
    Thrown { tag: String },
    /// A swap's drop: exactly where the item it was swapped for lay, at
    /// rest, which `Pickup_Weapon`'s `G_SetOrigin` makes it.
    Exactly { origin: [f32; 3], angles: [f32; 3] },
}

/// `LaunchItem` (0x4db98): a new item of row `index` at `origin`, flying at
/// `velocity` under gravity from this frame, locked to `owner` for
/// [`OWNER_LOCKOUT_MS`], and in the drop ring. Its angles are zero.
fn launch(
    host: &mut GameHost,
    cx: &mut Cx,
    index: usize,
    origin: Vec3,
    velocity: Vec3,
    owner: usize,
) -> Result<EntId, ErrorKind> {
    let name = crate::items::item_name(index)
        .unwrap_or_default()
        .to_string();
    let classname = crate::game::spawn::radiant_name(&name).unwrap_or(&name);
    let classname = cx.intern_exact(classname);
    let id = host.ents.spawn(cx)?;
    for (field, value) in [
        ("classname", Value::String(classname)),
        ("origin", Value::Vector(origin.into())),
        ("angles", Value::Vector([0.0; 3])),
    ] {
        let atom = cx.intern_folded(field);
        host.set_field(cx, id, atom, value)?;
    }
    host.register_item(&name);
    let now = host.level_time_ms;
    if let Some(e) = host.ents.get_mut(id) {
        e.item = Some(ItemState {
            index: index as u8,
            clip: 0,
            owner: Some(owner as u8),
            dropped: true,
            taken: false,
            ground: 0,
            pos: Some(Trajectory {
                tr_type: TR_GRAVITY,
                tr_time: now,
                tr_duration: 0,
                base: origin,
                delta: velocity,
            }),
            apos: None,
            clipmask: LAUNCH_CLIPMASK,
            nodraw: false,
        });
    }
    link(host, cx, id, CONTENTS_ITEM);
    host.ents
        .schedule(id, ThinkFn::ClearOwner, now + OWNER_LOCKOUT_MS);
    let ents = &host.ents;
    if let Some(old) = host.drop_ring.claim(|i| ents.get(i).is_some(), id) {
        host.ents.schedule(old, ThinkFn::Free, now + 1);
    }
    Ok(id)
}

/// `Drop_Weapon`'s and `Drop_Item`'s launch off player `slot` (section 8):
/// from the player's `origin` raised by half its box, along its `angles`
/// yaw at [`LAUNCH_SPEED`], climbing at `200 + 50 * crandom()`.
fn launch_off(
    host: &mut GameHost,
    cx: &mut Cx,
    slot: usize,
    index: usize,
) -> Result<EntId, ErrorKind> {
    let player = host
        .ents
        .handle(slot as u32)
        .ok_or(ErrorKind::BadType("no such player"))?;
    let yaw = angles_of(host, cx, player)[1].to_radians();
    let up = LAUNCH_UP + LAUNCH_UP_SPREAD * crandom(host);
    let velocity = Vec3::new(yaw.cos() * LAUNCH_SPEED, yaw.sin() * LAUNCH_SPEED, up);
    let mut origin = Vec3::from(origin_of(host, cx, player));
    origin.z += host.client_height.get(slot).copied().unwrap_or(0.0) * 0.5;
    launch(host, cx, index, origin, velocity, slot)
}

/// `Drop_Item(player, item, 0, 0)` (0x4ed30), `dropItem`'s arm for a
/// `bg_itemlist` row that is not a weapon: launched off the player with no
/// tag, no spin and zero angles.
pub fn drop_item(
    host: &mut GameHost,
    cx: &mut Cx,
    slot: usize,
    index: usize,
) -> Result<EntId, ErrorKind> {
    launch_off(host, cx, slot, index)
}

/// `Drop_Weapon` (0x4dd40) for a weapon `crate::game::pickup::drop_weapon`
/// took off `slot`: the item, its counts, and where it starts.
///
/// A thrown drop starts at `tag` on the dropper's posed model when the model
/// has it, swept there from the box's centre (`G_DObjGetWorldTagMatrix`,
/// 0x4e0bb); otherwise at the launch's mid-height. Either way its angles are
/// the dropper's own `angles`, not the tag's (`G_SetAngle` at 0x4e1e7 is
/// handed `ent+0x140`), and it spins at `SPIN * crandom()`.
pub fn launch_weapon(
    host: &mut GameHost,
    cx: &mut Cx,
    slot: usize,
    d: Dropped,
    at: DropAt,
) -> Result<EntId, ErrorKind> {
    let id = match &at {
        DropAt::Thrown { tag } => {
            let id = launch_off(host, cx, slot, d.weapon as usize)?;
            let player = host
                .ents
                .handle(slot as u32)
                .ok_or(ErrorKind::BadType("no such player"))?;
            if let Some(start) = tag_start(host, slot, tag) {
                let origin = cx.intern_folded("origin");
                host.set_field(cx, id, origin, Value::Vector(start.into()))?;
                if let Some(pos) = host
                    .ents
                    .get_mut(id)
                    .and_then(|e| e.item.as_mut())
                    .and_then(|i| i.pos.as_mut())
                {
                    pos.base = start;
                }
            }
            let angles = angles_of(host, cx, player);
            let atom = cx.intern_folded("angles");
            host.set_field(cx, id, atom, Value::Vector(angles))?;
            let spin = Vec3::new(
                SPIN[0] * crandom(host),
                SPIN[1] * crandom(host),
                SPIN[2] * crandom(host),
            );
            let now = host.level_time_ms;
            if let Some(i) = host.ents.get_mut(id).and_then(|e| e.item.as_mut()) {
                i.apos = Some(Trajectory {
                    tr_type: TR_LINEAR,
                    tr_time: now,
                    tr_duration: 0,
                    base: angles.into(),
                    delta: spin,
                });
            }
            id
        }
        DropAt::Exactly { origin, angles } => {
            let id = launch(
                host,
                cx,
                d.weapon as usize,
                Vec3::from(*origin),
                Vec3::ZERO,
                slot,
            )?;
            for (field, v) in [("origin", *origin), ("angles", *angles)] {
                let atom = cx.intern_folded(field);
                host.set_field(cx, id, atom, Value::Vector(v))?;
            }
            if let Some(i) = host.ents.get_mut(id).and_then(|e| e.item.as_mut()) {
                i.pos = None;
            }
            id
        }
    };
    let count = cx.intern_folded("count");
    host.set_field(cx, id, count, Value::Int(d.count))?;
    if let Some(i) = host.ents.get_mut(id).and_then(|e| e.item.as_mut()) {
        i.clip = d.clip;
    }
    Ok(id)
}

/// Where a thrown drop starts: `tag` on the dropper's model, posed as its
/// last end frame left it at its `angles` yaw, swept from the box's centre
/// with the item's box so it never starts inside a wall. `None` without the
/// model, the animtree or the paks.
fn tag_start(host: &mut GameHost, slot: usize, tag: &str) -> Option<Vec3> {
    let body = host.client_dobjs.get(slot)?.clone()?;
    let anims = host.anims.clone()?;
    let fs = host.fs.clone()?;
    let world = host.world.clone()?;
    let skel = host.hit_rigs.rig(&fs, &body.pose.assembly)?;
    let bone = skel.bone_index(tag)?;
    let inputs = body.pose.pose_inputs(&anims, host.level_time_ms);
    let rigs = &mut host.hit_rigs;
    let pose = vcod_common::playerpose::pose_player(&skel, &inputs, |n| rigs.clip(&fs, n));
    let (local, _) = pose.bone_world(&skel, bone);
    let at = body.origin + glam::Quat::from_rotation_z(body.yaw) * local;
    let centre = body.origin + (body.mins + body.maxs) * 0.5;
    let (mins, maxs) = bounds(true);
    Some(
        world
            .collision
            .item_trace(centre, at, mins, maxs, TAG_CLIPMASK)
            .endpos,
    )
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

/// What the use key and the cursor hint found: an item to take, a turret to
/// man or a `trigger_use` to fire.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Activate {
    Item(EntId),
    Turret(EntId),
    /// A `trigger_use`: contents 0x200000 puts it on the same list
    /// (docs/research/cod11-gametypes-re-bel.md 4).
    Trigger(EntId),
}

/// A turret's bounds centre above its origin: `G_SpawnTurret`'s box is
/// (-32, -32, 0) to (32, 32, 56) (`docs/research/cod11-turrets.md` 3, 4.1).
const TURRET_CENTRE_Z: f32 = 28.0;

/// `G_GetActivateEnt`'s choice (section 2.1): the best-scoring grabbable
/// item, usable turret or `trigger_use` in reach whose centre the muzzle can see past the
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
    let uses: Vec<EntId> = host
        .triggers
        .iter()
        .filter(|(_, t)| t.kind == crate::game::trigger::TriggerKind::Use)
        .map(|(id, _)| id)
        .collect();
    for id in uses {
        let (lo, hi) = crate::game::trigger::entity_abs_bounds(host, cx, id);
        let centre = [
            (lo[0] + hi[0]) * 0.5,
            (lo[1] + hi[1]) * 0.5,
            (lo[2] + hi[2]) * 0.5,
        ];
        if let Some(s) = activate_score(muzzle, forward, centre) {
            scored.push((Activate::Trigger(id), s, centre));
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
    let respawn = respawn_secs(host, cx, id, state.index as usize);
    if let Some(e) = host.ents.get_mut(id) {
        if let Some(i) = e.item.as_mut() {
            i.taken = true;
        }
        e.think = None;
        e.nextthink = 0;
    }
    // Contents 0 and a link (0x4d90c, 0x4da33): out of the tree.
    link(host, cx, id, 0);
    let now = host.level_time_ms;
    if state.dropped {
        host.ents
            .schedule(id, ThinkFn::Free, now + FREE_AFTER_PICKUP_MS);
    } else if let Some(secs) = respawn {
        host.ents
            .schedule(id, ThinkFn::RespawnItem, now + secs * 1000);
    }
}

/// `Touch_Item`'s respawn (section 7): the pickup function's value, which is
/// `g_weaponRespawn` for a `spawnflags & 8` weapon and -1 for any other
/// weapon and for health; a non-zero `wait` replaces it, a non-zero `random`
/// adds `crandom() * random` with a floor of 1. `None` for no respawn,
/// which a `wait` of -1 is too.
fn respawn_secs(host: &mut GameHost, cx: &mut Cx, id: EntId, index: usize) -> Option<i32> {
    use crate::game::pickup::{ItemKind, item_kind};
    let mut field = |name: &str| {
        let atom = cx.intern_folded(name);
        match host.get_field(cx, id, atom) {
            Value::Int(i) => i as f32,
            Value::Float(f) => f,
            _ => 0.0,
        }
    };
    let spawnflags = field("spawnflags") as i32;
    let wait = field("wait");
    let random = field("random");
    if wait == -1.0 {
        return None;
    }
    let mut secs = match item_kind(index)? {
        ItemKind::Weapon(_) if spawnflags & 8 != 0 => {
            host.cvars.get("g_weaponrespawn").parse().unwrap_or(0)
        }
        ItemKind::Weapon(_) | ItemKind::Health { .. } => -1,
        ItemKind::Ammo => 40,
    };
    if wait != 0.0 {
        secs = wait as i32;
    }
    if random != 0.0 {
        secs += (crandom(host) * random) as i32;
        secs = secs.max(1);
    }
    (secs > 0).then_some(secs)
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

    /// A thrown drop carries its dropper in `clientNum` until the lockout
    /// clears and flies on the wire: gravity from the drop's frame at 150
    /// along the dropper's yaw, climbing at 50 to 150, spinning backwards on
    /// all three axes, `groundEntityNum` 0. A swap's drop sits where the item
    /// it replaced lay, stationary, `groundEntityNum` 0 too
    /// (docs/research/cod11-items.md 12.6, 14).
    #[test]
    fn a_thrown_drop_flies_and_names_its_dropper_until_the_lockout_clears() {
        let (mut vm, mut host) = fixture();
        let fg = crate::configstrings::weapon_index("fg42_mp").unwrap() as u8;
        let d = Dropped {
            weapon: fg,
            count: 70,
            clip: 20,
        };
        host.level_time_ms = 5000;
        let (id, swap) = vm.with_cx(|cx| {
            let c = host.ents.spawn_client(cx, 3, None).unwrap();
            let f = cx.intern_folded("angles");
            host.set_field(cx, c, f, Value::Vector([0.0, 90.0, 0.0]))
                .unwrap();
            let tag = "tag_weapon_right".to_string();
            let id = launch_weapon(&mut host, cx, 3, d, DropAt::Thrown { tag }).unwrap();
            let at = DropAt::Exactly {
                origin: [826.0, 2274.0, -22.8],
                angles: [0.0, 270.0, 90.0],
            };
            (id, launch_weapon(&mut host, cx, 3, d, at).unwrap())
        });
        let p = &vcod_common::net::protocol::PROTOCOL_V1;
        let ents = vm.with_cx(|cx| crate::game::wire::packet_entities(&mut host, cx, p));
        let e = &ents[&id.0];
        assert_eq!(e.field_i32(p, "clientNum"), 3);
        assert_eq!(e.field_i32(p, "groundEntityNum"), 0);
        let pos = Trajectory::read(e, p, "pos");
        assert_eq!((pos.tr_type, pos.tr_time), (TR_GRAVITY, 5000));
        assert!(pos.delta.x.abs() < 1e-3 && (pos.delta.y - 150.0).abs() < 1e-3);
        assert!(
            pos.delta.z > 50.0 && pos.delta.z <= 150.0,
            "{}",
            pos.delta.z
        );
        let apos = Trajectory::read(e, p, "apos");
        assert_eq!(apos.tr_type, TR_LINEAR);
        assert_eq!(apos.base, Vec3::new(0.0, 90.0, 0.0));
        assert!(apos.delta.cmplt(Vec3::ZERO).all(), "{:?}", apos.delta);
        assert_eq!(ents[&swap.0].field_i32(p, "groundEntityNum"), 0);
        assert_eq!(
            Trajectory::read(&ents[&swap.0], p, "pos").tr_type,
            TR_STATIONARY
        );
        vm.with_cx(|cx| host.run_entity_thinks(cx, 5000 + OWNER_LOCKOUT_MS));
        let ents = vm.with_cx(|cx| crate::game::wire::packet_entities(&mut host, cx, p));
        assert_eq!(ents[&id.0].field_i32(p, "clientNum"), 254);
    }

    fn floor_host(world: vcod_common::collision::CollisionWorld) -> (vcod_gsc::Vm, GameHost) {
        let (vm, mut host) = fixture();
        host.world = Some(std::rc::Rc::new(crate::world::World {
            collision: world,
            vis: vcod_common::bsp::Visibility::none(),
            spawn: ([0.0, 0.0, 64.0], 0.0),
            spawn_points: Vec::new(),
            hazards: Vec::new(),
        }));
        (vm, host)
    }

    /// Frames of the item pass until `id` stops flying or is freed; the
    /// frame it stopped on.
    fn fly(vm: &mut vcod_gsc::Vm, host: &mut GameHost, id: EntId, from: i32) -> i32 {
        let mut now = from;
        for _ in 0..100 {
            now += 50;
            host.level_time_ms = now;
            vm.with_cx(|cx| run_items(host, cx, now));
            if host
                .ents
                .get(id)
                .and_then(|e| e.item)
                .is_none_or(|i| i.pos.is_none())
            {
                return now;
            }
        }
        panic!("item {id:?} never came down");
    }

    fn vec_field(vm: &mut vcod_gsc::Vm, host: &mut GameHost, id: EntId, name: &str) -> [f32; 3] {
        vm.with_cx(|cx| {
            let f = cx.intern_folded(name);
            match host.get_field(cx, id, f) {
                Value::Vector(v) => v,
                _ => panic!("{name} is not a vector"),
            }
        })
    }

    /// A thrown carbine comes down on the floor ahead of the dropper, its box
    /// resting on it with up to half a unit of lift, lying along the drop's
    /// yaw with a weapon's 90 degrees of roll and the world as its ground.
    #[test]
    fn a_thrown_drop_lands_ahead_facing_the_droppers_yaw_with_the_weapon_roll() {
        let (mut vm, mut host) = floor_host(vcod_common::collision::test_world(&[]));
        let carbine = crate::configstrings::weapon_index("m1carbine_mp").unwrap() as u8;
        let d = Dropped {
            weapon: carbine,
            count: 400,
            clip: 15,
        };
        host.client_height[0] = 70.0;
        let id = vm.with_cx(|cx| {
            let c = host.ents.spawn_client(cx, 0, None).unwrap();
            for (name, v) in [
                ("origin", [16.0, -32.0, 0.125]),
                ("angles", [0.0, 45.0, 0.0]),
            ] {
                let f = cx.intern_folded(name);
                host.set_field(cx, c, f, Value::Vector(v)).unwrap();
            }
            let tag = "tag_weapon_right".to_string();
            launch_weapon(&mut host, cx, 0, d, DropAt::Thrown { tag }).unwrap()
        });
        fly(&mut vm, &mut host, id, 0);
        let o = vec_field(&mut vm, &mut host, id, "origin");
        let rest = 1.0 + vcod_common::collision::SURFACE_CLIP_EPSILON;
        assert!(o[2] > rest && o[2] <= rest + 0.5, "z {}", o[2]);
        assert!(
            (o[0] - 16.0 - (o[1] + 32.0)).abs() < 1e-2,
            "off the yaw: {o:?}"
        );
        assert!(o[0] > 16.0 + 40.0, "landed short: {o:?}");
        let a = vec_field(&mut vm, &mut host, id, "angles");
        assert!(a[0].abs() < 1e-3, "pitch {}", a[0]);
        assert!((a[1] - 45.0).abs() < 1e-3, "yaw {}", a[1]);
        assert!((a[2] - 90.0).abs() < 1e-3, "roll {}", a[2]);
        let i = host.ents.get(id).unwrap().item.unwrap();
        assert_eq!(i.ground, ENTITYNUM_WORLD as i32);
        assert!(i.apos.is_none());
    }

    /// A wall in the arc stops the drop dead: it is moved a unit off the wall
    /// and falls straight down from there, from rest.
    #[test]
    fn a_wall_drops_the_item_straight_down() {
        let wall = (Vec3::new(64.0, -512.0, 0.0), Vec3::new(80.0, 512.0, 512.0));
        let world = vcod_common::collision::test_world(&[wall]);
        let mut pos = Trajectory {
            tr_type: TR_GRAVITY,
            tr_time: 0,
            tr_duration: 0,
            base: Vec3::new(0.0, 0.0, 48.0),
            delta: Vec3::new(150.0, 0.0, 100.0),
        };
        let mut origin = pos.base;
        let mut now = 0;
        let ran = loop {
            now += 50;
            let ran = run_flight(
                &world,
                &mut pos,
                &mut origin,
                bounds(true),
                0x81,
                now,
                &mut || 0.25,
            );
            if ran != Ran::Flew {
                break ran;
            }
        };
        assert_eq!(ran, Ran::Nudged);
        let face = 64.0 - 1.0 - vcod_common::collision::SURFACE_CLIP_EPSILON;
        assert!((origin.x - (face - 1.0)).abs() < 1e-2, "x {}", origin.x);
        assert_eq!(
            (pos.tr_type, pos.tr_time, pos.delta),
            (TR_GRAVITY, now, Vec3::ZERO)
        );
        assert_eq!(pos.base, origin);
        let x = origin.x;
        let ran = loop {
            now += 50;
            let ran = run_flight(
                &world,
                &mut pos,
                &mut origin,
                bounds(true),
                0x81,
                now,
                &mut || 0.25,
            );
            if ran != Ran::Flew {
                break ran;
            }
        };
        assert!(matches!(ran, Ran::Landed { .. }));
        assert_eq!(origin.x, x);
    }

    /// An item whose contact point is inside a nodrop brush is freed there;
    /// one that only flies through one is not, since the test runs only on
    /// a contact.
    #[test]
    fn a_landing_inside_nodrop_frees_the_item() {
        use vcod_common::collision::{CONTENTS_NODROP, CONTENTS_SOLID, synthetic_world};
        let world = synthetic_world(
            &[
                ("textures/test/solid", CONTENTS_SOLID, 0),
                ("textures/test/nodrop", CONTENTS_NODROP, 0),
            ],
            &[
                (0, [-1024.0, -1024.0, -16.0], [1024.0, 1024.0, 0.0]),
                (1, [100.0, -64.0, 0.0], [300.0, 64.0, 64.0]),
                (1, [-64.0, -64.0, 40.0], [64.0, 64.0, 80.0]),
            ],
        );
        let run = |base: Vec3, delta: Vec3| {
            let mut pos = Trajectory {
                tr_type: TR_GRAVITY,
                tr_time: 0,
                tr_duration: 0,
                base,
                delta,
            };
            let mut origin = base;
            let mut now = 0;
            loop {
                now += 50;
                let ran = run_flight(
                    &world,
                    &mut pos,
                    &mut origin,
                    bounds(true),
                    0x81,
                    now,
                    &mut || 0.25,
                );
                if ran != Ran::Flew {
                    return ran;
                }
            }
        };
        assert_eq!(run(Vec3::new(200.0, 0.0, 48.0), Vec3::ZERO), Ran::NoDrop);
        assert!(matches!(
            run(Vec3::new(0.0, 0.0, 60.0), Vec3::ZERO),
            Ran::Landed { .. }
        ));
    }

    /// A spawnflags-8 weapon comes back `g_weaponrespawn` seconds after it is
    /// taken, with `EV_ITEM_RESPAWN` on its own ring and `eFlags` 0x100 on
    /// the frame it returns; a health pack with `random` 0.5 comes back
    /// after the one-second floor; a plain one stays taken.
    #[test]
    fn a_taken_item_respawns_on_its_flags_and_fields() {
        let (mut vm, mut host) = fixture();
        host.weapons = std::rc::Rc::new(crate::game::pickup::tests_table());
        let colt = crate::configstrings::weapon_index("colt_mp").unwrap();
        host.client_weapons[0].give(colt, 3);
        host.client_vitals[0].max_health = 100;
        host.client_vitals[0].health = 40;
        host.level_time_ms = 1000;
        let (weapon, health, plain) = vm.with_cx(|cx| {
            host.ents.spawn_client(cx, 0, None).unwrap();
            let mut make = |cls: &str, flags: i32, random: f32| {
                let id = host.ents.spawn(cx).unwrap();
                let f = cx.intern_folded("classname");
                let v = Value::String(cx.intern_exact(cls));
                host.set_field(cx, id, f, v).unwrap();
                let f = cx.intern_folded("spawnflags");
                host.set_field(cx, id, f, Value::Int(flags)).unwrap();
                let f = cx.intern_folded("random");
                host.set_field(cx, id, f, Value::Float(random)).unwrap();
                spawn_in_place(
                    &mut host,
                    cx,
                    id,
                    crate::items::classname_index(cls).unwrap(),
                    flags,
                );
                id
            };
            (
                make("mpweapon_colt", 8 | 1, 0.0),
                make("item_health", 1, 0.5),
                make("item_health", 1, 0.0),
            )
        });
        vm.with_cx(|cx| {
            touch(&mut host, cx, weapon, 0, true);
            touch(&mut host, cx, health, 0, true);
            host.client_vitals[0].health = 40;
            touch(&mut host, cx, plain, 0, true);
        });
        let taken = |host: &GameHost, id: EntId| host.ents.get(id).unwrap().item.unwrap().taken;
        assert_eq!(
            (
                taken(&host, weapon),
                taken(&host, health),
                taken(&host, plain)
            ),
            (true, true, true)
        );
        let p = &vcod_common::net::protocol::PROTOCOL_V1;
        for now in (1050..=6000).step_by(50) {
            host.level_time_ms = now;
            vm.with_cx(|cx| run_items(&mut host, cx, now));
            let ents = vm.with_cx(|cx| crate::game::wire::packet_entities(&mut host, cx, p));
            assert_eq!(ents.contains_key(&health.0), now >= 2000, "health at {now}");
            assert_eq!(ents.contains_key(&weapon.0), now >= 6000, "colt at {now}");
            if now == 2000 {
                let e = &ents[&health.0];
                assert_eq!(e.field_i32(p, "eFlags"), 16 | EF_NODRAW);
                assert_eq!(e.field_i32(p, "eventSequence"), 1);
                assert_eq!(e.field_i32(p, "events[0]"), EV_ITEM_RESPAWN);
            }
            if now == 2050 {
                assert_eq!(ents[&health.0].field_i32(p, "eFlags"), 16);
            }
        }
        assert!(taken(&host, plain));
    }
}
