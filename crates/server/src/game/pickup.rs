//! Item pickup's arithmetic: who may grab what, how much a grab gives,
//! which slot a new weapon takes and what it pushes out. Pure over an
//! [`Inventory`] and the item's two counts; `crate::game::item` builds both
//! from the host and writes the result back. Addresses are in
//! docs/research/cod11-items.md.

use crate::weapons::{PlayerWeapons, WeaponTable};
use vcod_common::pmove::weapon::NUM_AMMO;
use vcod_common::weapon::WeaponDef;

pub const EV_ITEM_PICKUP: i32 = 146;
pub const EV_AMMO_PICKUP: i32 = 148;

/// A `bg_itemlist` row's `giType`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ItemKind {
    /// By configstring 7 index, which is the row.
    Weapon(u8),
    /// Rows 65 and 66, whose `giTag` names no weapon. No stock path spawns
    /// one, and ours refuses every grab of it (section 11).
    Ammo,
    Health {
        quantity: i32,
    },
}

pub fn item_kind(index: usize) -> Option<ItemKind> {
    match index {
        1..=64 => crate::items::item_name(index).map(|_| ItemKind::Weapon(index as u8)),
        65 | 66 => Some(ItemKind::Ammo),
        67 => Some(ItemKind::Health { quantity: 10 }),
        68 => Some(ItemKind::Health { quantity: 25 }),
        69 => Some(ItemKind::Health { quantity: 50 }),
        _ => None,
    }
}

/// The part of a player a pickup reads and writes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Inventory {
    pub weapons: PlayerWeapons,
    pub ammo: [i16; NUM_AMMO],
    pub clip: [i16; NUM_AMMO],
    pub health: i32,
    pub max_health: i32,
    /// `other->health > 0`, `Touch_Item`'s first gate.
    pub alive: bool,
}

/// An item's `count` (`ent+0x250`, the entity field) and clip
/// (`ent+0x2cc`): 0 is unset, -1 empty.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ItemCounts {
    pub count: i32,
    pub clip: i32,
}

pub struct ItemView<'a> {
    pub index: usize,
    /// `s.clientNum` while the dropper's lockout holds.
    pub owner: Option<usize>,
    pub classname: &'a str,
}

/// What `Drop_Weapon` leaves on the ground: the weapon and its item counts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Dropped {
    pub weapon: u8,
    pub count: i32,
    pub clip: i32,
}

#[derive(Debug, Default, PartialEq)]
pub struct Outcome {
    /// The pickup function returned non-zero: the event, the `"trigger"`
    /// notify and the removal all follow.
    pub taken: bool,
    pub event: Option<i32>,
    /// Reliable commands for the player, in order.
    pub commands: Vec<String>,
    /// `G_LogPrintf`'s line, written before the pickup function runs.
    pub log: Option<String>,
    /// A swap's outgoing weapon, to be dropped where the item lay.
    pub drop: Option<Dropped>,
}

/// `BG_PlayerTouchesItem` (section 1) inside `G_TouchTriggers`' query box;
/// every item's link box is 1 unit each way.
pub fn touches(player: [f32; 3], item: [f32; 3]) -> bool {
    const REACH_XY: f32 = 36.0;
    const ABOVE: f32 = 18.0;
    const BELOW: f32 = 88.0;
    const ITEM_HALF: f32 = 1.0;
    let q = crate::game::trigger::TOUCH_BOX;
    let d = [
        player[0] - item[0],
        player[1] - item[1],
        player[2] - item[2],
    ];
    (0..3).all(|i| d[i].abs() <= q[i] + ITEM_HALF)
        && d[0].abs() <= REACH_XY
        && d[1].abs() <= REACH_XY
        && (-BELOW..=ABOVE).contains(&d[2])
}

/// `BG_GivePlayerWeapon` (section 5): a held weapon is left alone; a new one
/// takes the empty slot its class allows, if any, and every weapon on its
/// alt-fire chain is held beside it without a slot.
fn give(inv: &mut Inventory, table: &WeaponTable, weapon: usize) {
    if inv.weapons.holds(weapon) {
        return;
    }
    let slot = empty_slot_for(&inv.weapons, table.slot(weapon)).unwrap_or(0);
    inv.weapons.give(weapon, slot);
    if let Some(alt) = crate::items::alt_weapon_index(weapon) {
        inv.weapons.give(alt, 0);
    }
}

/// `BG_TakePlayerWeapon` (section 8), alt-fire chain included.
fn take(inv: &mut Inventory, weapon: usize) {
    if !inv.weapons.holds(weapon) {
        return;
    }
    inv.weapons.take(weapon);
    if let Some(alt) = crate::items::alt_weapon_index(weapon) {
        inv.weapons.take(alt);
    }
}

/// `BG_IsPlayerWeaponInSlot` with `followAlt` set (section 5): the weapon or
/// its alt-fire partner sits in a slot.
fn in_a_slot(weapons: &PlayerWeapons, weapon: usize) -> bool {
    let alt = crate::items::alt_weapon_index(weapon);
    weapons.slots[1..=5]
        .iter()
        .any(|&s| s != 0 && (s as usize == weapon || Some(s as usize) == alt))
}

/// `BG_GetMaxPickupableAmmo` (section 3). Negative once a shared cap is
/// over its limit, which is how `add_ammo` trims it.
pub fn max_pickupable(inv: &Inventory, table: &WeaponTable, weapon: usize) -> i32 {
    let Some(def) = table.get(weapon) else {
        return 0;
    };
    if let Some(cap) = def.shared_cap_index {
        let (mut seen_ammo, mut seen_clip) = (Vec::new(), Vec::new());
        let mut used = 0;
        for (i, d) in table.defs().iter().enumerate() {
            let Some(d) = d else { continue };
            if d.shared_cap_index != Some(cap) || !inv.weapons.holds(i) {
                continue;
            }
            if d.clip_only {
                if !seen_clip.contains(&d.clip_index) {
                    seen_clip.push(d.clip_index);
                    used += i32::from(inv.clip[d.clip_index]);
                }
            } else if !seen_ammo.contains(&d.ammo_index) {
                seen_ammo.push(d.ammo_index);
                used += i32::from(inv.ammo[d.ammo_index]);
            }
        }
        return def.shared_ammo_cap - used;
    }
    if def.clip_only {
        def.clip_size as i32 - i32::from(inv.clip[def.clip_index])
    } else {
        def.max_ammo as i32 - i32::from(inv.ammo[def.ammo_index])
    }
}

/// `Add_Ammo` (section 4.2). Returns what the player gained, reserve and
/// clip together.
pub fn add_ammo(
    inv: &mut Inventory,
    table: &WeaponTable,
    weapon: usize,
    count: i32,
    fill_clip: bool,
) -> i32 {
    let Some(def) = table.get(weapon) else {
        return 0;
    };
    let (a, c) = (def.ammo_index, def.clip_index);
    let before = i32::from(inv.ammo[a]) + i32::from(inv.clip[c]);
    let mut ammo = i32::from(inv.ammo[a]) + count;
    let mut clip = i32::from(inv.clip[c]);
    if def.clip_only {
        give(inv, table, weapon);
    }
    if fill_clip || def.clip_only {
        let moved = (def.clip_size as i32 - clip).min(ammo);
        ammo -= moved;
        clip += moved;
    }
    ammo = if def.clip_only {
        0
    } else {
        ammo.min(def.max_ammo as i32)
    };
    clip = clip.min(def.clip_size as i32);
    inv.ammo[a] = ammo as i16;
    inv.clip[c] = clip as i16;
    if def.shared_cap_index.is_some() {
        let over = max_pickupable(inv, table, weapon);
        if over < 0 {
            if def.clip_only {
                let left = i32::from(inv.clip[c]) + over;
                if left <= 0 {
                    inv.clip[c] = 0;
                    take(inv, weapon);
                    return 0;
                }
                inv.clip[c] = left as i16;
            } else {
                inv.ammo[a] = (i32::from(inv.ammo[a]) + over).max(0) as i16;
            }
        }
    }
    i32::from(inv.ammo[a]) + i32::from(inv.clip[c]) - before
}

/// `BG_CanItemBeGrabbed` (section 3).
pub fn can_grab(
    inv: &Inventory,
    kind: ItemKind,
    owner: Option<usize>,
    slot: usize,
    touched: bool,
    table: &WeaponTable,
) -> bool {
    if owner == Some(slot) {
        return false;
    }
    match kind {
        ItemKind::Weapon(w) if inv.weapons.holds(w as usize) => {
            max_pickupable(inv, table, w as usize) > 0
        }
        ItemKind::Weapon(_) => !touched,
        ItemKind::Ammo => false,
        ItemKind::Health { .. } => inv.health < inv.max_health,
    }
}

/// `BG_GetEmptySlotForWeapon` (section 5): a primary-class weapon takes
/// `primary`, else `primaryb`; the other classes their own slot.
pub fn empty_slot_for(weapons: &PlayerWeapons, class: usize) -> Option<usize> {
    match class {
        1 | 2 => [1, 2].into_iter().find(|&s| weapons.slots[s] == 0),
        3..=5 => (weapons.slots[class] == 0).then_some(class),
        _ => None,
    }
}

/// `Drop_Weapon`'s effect on the dropper (section 8): the rounds go with the
/// item, 0 written as -1, and the weapon is taken.
pub fn drop_weapon(inv: &mut Inventory, table: &WeaponTable, weapon: u8) -> Option<Dropped> {
    let w = weapon as usize;
    if !inv.weapons.holds(w) {
        return None;
    }
    let Some(def) = table.get(w) else {
        take(inv, w);
        return None;
    };
    if def.clip_only && inv.clip[def.clip_index] == 0 {
        take(inv, w);
        return None;
    }
    let or_empty = |v: i16| if v == 0 { -1 } else { i32::from(v) };
    let dropped = Dropped {
        weapon,
        count: or_empty(inv.ammo[def.ammo_index]),
        clip: or_empty(inv.clip[def.clip_index]),
    };
    inv.ammo[def.ammo_index] = 0;
    inv.clip[def.clip_index] = 0;
    take(inv, w);
    Some(dropped)
}

/// `Pickup_Health` (section 6). Returns the amount the `f` line carries.
/// Round-and-cap is retail's truncate-then-re-round only at `maxHealth` 100.
pub fn pickup_health(inv: &mut Inventory, quantity: i32, count: i32) -> i32 {
    let cap = if quantity == 5 || quantity == 100 {
        2 * inv.max_health
    } else {
        inv.max_health
    };
    let amount = if count != 0 { count } else { quantity };
    let add = (inv.max_health as f32 * amount as f32 * 0.01).round() as i32;
    inv.health = (inv.health + add).min(cap);
    amount
}

/// `Pickup_Weapon`'s reading of the item's counts (section 4.1), which it
/// writes back: returns `(reserve, clip)`, both at least 0.
fn resolve_counts(
    def: &WeaponDef,
    it: &mut ItemCounts,
    rand: &mut dyn FnMut() -> i32,
) -> (i32, i32) {
    let reserve = if it.count < 0 {
        0
    } else {
        if it.count == 0 {
            let (lo, hi) = if def.drop_ammo_max < def.drop_ammo_min {
                (def.drop_ammo_max, def.drop_ammo_min)
            } else {
                (def.drop_ammo_min, def.drop_ammo_max)
            };
            it.count = if lo == 0 && hi == 0 {
                let u = 1.0 - f64::from(rand()) / 2_147_483_648.0;
                (u * 0.5 * (f64::from(def.clip_size) - 1.0) + 0.5) as i32 + 1
            } else if hi < 0 {
                0
            } else if hi == lo {
                lo
            } else {
                rand() % (hi - lo) + lo
            };
            it.count = it.count.max(0);
        }
        it.count = it.count.min(def.max_ammo as i32);
        it.count
    };
    let clip = if it.clip < 0 {
        0
    } else {
        if it.clip == 0 {
            it.clip = (def.clip_size as i32).min(it.count.max(0));
            it.count -= it.clip;
        }
        it.clip = it.clip.min(def.clip_size as i32);
        it.clip
    };
    (reserve.min(it.count.max(0)), clip)
}

fn ammo_line(def: &WeaponDef) -> String {
    let kind = if def.clip_only { "CLIPONLY_" } else { "" };
    format!("f \"GAME_PICKUP_{kind}AMMO\u{14}{}\"", def.display_name)
}

/// Which weapon a pickup of unowned `w` pushes out (section 5), `Err` for
/// the refusal.
fn outgoing(inv: &Inventory, table: &WeaponTable, w: usize) -> Result<Option<u8>, ()> {
    let class = table.slot(w);
    if empty_slot_for(&inv.weapons, class).is_some() {
        return Ok(None);
    }
    let held = inv.weapons.current as usize;
    if held != 0 && table.slot(held) == class {
        return Ok(Some(held as u8));
    }
    if (3..=5).contains(&class) {
        return Ok(Some(inv.weapons.slots[class]).filter(|&x| x != 0));
    }
    let own_empty = table
        .get(w)
        .is_none_or(|d| inv.ammo[d.ammo_index] == 0 && inv.clip[d.clip_index] == 0);
    if own_empty {
        Ok(Some(inv.weapons.slots[1]).filter(|&x| x != 0))
    } else {
        Err(())
    }
}

#[allow(clippy::too_many_arguments)]
fn pickup_weapon(
    inv: &mut Inventory,
    it: &mut ItemCounts,
    w: u8,
    table: &WeaponTable,
    ammo_pools: bool,
    touched: bool,
    rand: &mut dyn FnMut() -> i32,
    out: &mut Outcome,
) -> bool {
    let wi = w as usize;
    let Some(def) = table.get(wi) else {
        return false;
    };
    let (reserve, clip) = resolve_counts(def, it, rand);
    if inv.weapons.holds(wi) {
        let offered = reserve + clip;
        let gain = add_ammo(inv, table, wi, offered, false);
        if gain != 0 {
            out.commands.push(ammo_line(def));
        }
        // What is left is written back to the item's own fields, the
        // reserve's overflow into the clip (0x4d297..0x4d2ff).
        if gain != offered {
            it.count -= gain;
            if it.count <= 0 {
                it.clip += it.count;
                it.count = -1;
                if it.clip <= 0 {
                    it.clip = -1;
                }
            }
            if ammo_pools && (it.count > 0 || it.clip > 0) {
                return false;
            }
        }
        out.event = Some(EV_AMMO_PICKUP);
        return true;
    }
    // A `ps.weapon` no longer held refuses every unowned grab, silently
    // (section 5), until pmove clears it.
    let current = inv.weapons.current as usize;
    if current != 0 && !inv.weapons.holds(current) {
        return false;
    }
    // Case 5: a held weapon in no slot is never swapped out.
    if current != 0
        && !in_a_slot(&inv.weapons, current)
        && empty_slot_for(&inv.weapons, table.slot(wi)).is_none()
    {
        return false;
    }
    let out_weapon = match outgoing(inv, table, wi) {
        Ok(o) => o,
        Err(()) => {
            out.commands
                .push("f \"GAME_CANT_GET_PRIMARY_WEAP_MESSAGE\"".to_string());
            return false;
        }
    };
    if let Some(o) = out_weapon {
        // An empty `clipOnly` weapon is taken with nothing dropped, and then
        // the new one is not given (0x4d160).
        out.drop = drop_weapon(inv, table, o);
        if out.drop.is_none() {
            return false;
        }
    }
    give(inv, table, wi);
    if !touched {
        out.commands.push(format!("a {w}"));
    }
    // The item's clip replaces the player's, and `fillClip` is never set
    // from here since the clip read is floored at 0 (section 4.3).
    let kept = clip.min(def.clip_size as i32);
    inv.clip[def.clip_index] = kept as i16;
    add_ammo(inv, table, wi, reserve + clip - kept, false);
    out.event = Some(EV_ITEM_PICKUP);
    true
}

/// `Touch_Item` (section 7), minus the writes to the world: the caller
/// raises the event, the notifies and the removal off the [`Outcome`].
/// `slot` is the player's client number.
#[allow(clippy::too_many_arguments)]
pub fn touch_item(
    inv: &mut Inventory,
    item: &ItemView,
    counts: &mut ItemCounts,
    slot: usize,
    touched: bool,
    table: &WeaponTable,
    ammo_pools: bool,
    rand: &mut dyn FnMut() -> i32,
) -> Outcome {
    let mut out = Outcome::default();
    if !inv.alive {
        return out;
    }
    let Some(kind) = item_kind(item.index) else {
        return out;
    };
    if !can_grab(inv, kind, item.owner, slot, touched, table) {
        return out;
    }
    out.log = Some(match kind {
        ItemKind::Weapon(w) => format!(
            "Weapon: {slot} {}",
            crate::items::item_name(w as usize).unwrap_or_default()
        ),
        _ => format!("Item: {slot} {}", item.classname),
    });
    out.taken = match kind {
        ItemKind::Weapon(w) => {
            pickup_weapon(inv, counts, w, table, ammo_pools, touched, rand, &mut out)
        }
        ItemKind::Health { quantity } => {
            let amount = pickup_health(inv, quantity, counts.count);
            out.commands
                .push(format!("f \"GAME_PICKUP_HEALTH\u{15}{amount}\""));
            out.event = Some(EV_ITEM_PICKUP);
            true
        }
        ItemKind::Ammo => false,
    };
    out
}

/// `G_GetActivateEnt`'s first pass for one grabbable candidate (section
/// 2.1): `None` outside the query box, past 128 units or outside the 0.76
/// cone, else the score the candidates sort on, lowest first.
pub fn activate_score(muzzle: [f32; 3], forward: [f32; 3], centre: [f32; 3]) -> Option<f32> {
    const BOX: [f32; 3] = [192.0, 192.0, 96.0];
    const RANGE: f32 = 128.0;
    const COS: f32 = 0.76;
    let d = [
        centre[0] - muzzle[0],
        centre[1] - muzzle[1],
        centre[2] - muzzle[2],
    ];
    if (0..3).any(|i| d[i].abs() > BOX[i] + 1.0) {
        return None;
    }
    let dist = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
    if dist > RANGE || dist == 0.0 {
        return None;
    }
    let dot = (d[0] * forward[0] + d[1] * forward[1] + d[2] * forward[2]) / dist;
    if dot <= 0.0 || dot < COS {
        return None;
    }
    Some((1.0 - (dot - COS) / 0.24) * 256.0 + dist)
}

/// `G_CheckForCursorHints`' value for an item (section 2.3), for the one
/// `activate_ent` picked.
pub fn cursor_hint(kind: ItemKind, owned: bool) -> i32 {
    match kind {
        ItemKind::Weapon(w) if owned => i32::from(w) + 0x49,
        ItemKind::Weapon(w) => i32::from(w) + 9,
        ItemKind::Health { .. } => 7,
        // `can_grab` refuses every ammo item, so `activate_ent` never picks one.
        ItemKind::Ammo => 0,
    }
}

/// The stock files' numbers for the six weapons the pickup tests use
/// (docs/research/cod11-items.md, the weapon file table), indexed as the
/// real table is, so the arithmetic runs without paks.
#[cfg(test)]
pub(crate) fn tests_table() -> WeaponTable {
    use crate::configstrings::weapon_index;
    fn def(keys: &[(&str, &str)]) -> WeaponDef {
        let map = keys
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        WeaponDef::from_map(&map)
    }
    let mut defs = vec![None; crate::items::NUM_ITEMS];
    let mut put = |name: &str, keys: &[(&str, &str)]| {
        defs[weapon_index(name).unwrap()] = Some(def(keys));
    };
    put(
        "colt_mp",
        &[
            ("weaponSlot", "pistol"),
            ("clipSize", "7"),
            ("maxAmmo", "56"),
            ("ammoName", "colt"),
            ("clipName", "colt"),
        ],
    );
    put(
        "fg42_mp",
        &[
            ("weaponSlot", "primary"),
            ("clipSize", "20"),
            ("maxAmmo", "320"),
            ("dropAmmoMin", "100"),
            ("dropAmmoMax", "200"),
            ("ammoName", "fg42"),
            ("clipName", "fg42"),
            ("displayName", "WEAPON_FG42"),
        ],
    );
    put(
        "fraggrenade_mp",
        &[
            ("weaponSlot", "grenade"),
            ("clipOnly", "1"),
            ("clipSize", "3"),
            ("maxAmmo", "3"),
            ("sharedAmmoCap", "3"),
            ("sharedAmmoCapName", "grenades"),
            ("ammoName", "usgrenade"),
            ("clipName", "usgrenade"),
            ("displayName", "WEAPON_M2FRAGGRENADE"),
        ],
    );
    put(
        "m1carbine_mp",
        &[
            ("weaponSlot", "primary"),
            ("clipSize", "15"),
            ("maxAmmo", "400"),
            ("dropAmmoMin", "90"),
            ("dropAmmoMax", "150"),
            ("ammoName", "m1carbine"),
            ("clipName", "m1carbine"),
            ("displayName", "WEAPON_M1A1CARBINE"),
        ],
    );
    put(
        "panzerfaust_mp",
        &[
            ("weaponSlot", "primary"),
            ("clipOnly", "1"),
            ("clipSize", "1"),
            ("maxAmmo", "1"),
            ("ammoName", "panzerfaust"),
            ("clipName", "panzerfaust"),
            ("displayName", "WEAPON_PANZERFAUST"),
        ],
    );
    put(
        "stielhandgranate_mp",
        &[
            ("weaponSlot", "grenade"),
            ("clipOnly", "1"),
            ("clipSize", "3"),
            ("maxAmmo", "3"),
            ("sharedAmmoCap", "3"),
            ("sharedAmmoCapName", "grenades"),
            ("ammoName", "germangrenade"),
            ("clipName", "germangrenade"),
        ],
    );
    WeaponTable::from_defs(defs)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::configstrings::weapon_index;

    fn table() -> WeaponTable {
        tests_table()
    }

    fn idx(name: &str) -> u8 {
        weapon_index(name).unwrap() as u8
    }

    /// The americans' spawn: carbine in primary, colt in pistol, frag in
    /// grenade, all full, carbine in hand, 100 health.
    fn allies(t: &WeaponTable) -> Inventory {
        let mut inv = Inventory {
            weapons: PlayerWeapons::default(),
            ammo: [0; NUM_AMMO],
            clip: [0; NUM_AMMO],
            health: 100,
            max_health: 100,
            alive: true,
        };
        for (name, slot) in [("m1carbine_mp", 1), ("colt_mp", 3), ("fraggrenade_mp", 4)] {
            let w = idx(name) as usize;
            let d = t.get(w).unwrap();
            inv.weapons.give(w, slot);
            inv.clip[d.clip_index] = d.clip_size as i16;
            if !d.clip_only {
                inv.ammo[d.ammo_index] = d.max_ammo as i16;
            }
        }
        inv.weapons.current = idx("m1carbine_mp");
        inv
    }

    fn no_rand() -> impl FnMut() -> i32 {
        || 0
    }

    fn view(index: u8) -> ItemView<'static> {
        ItemView {
            index: index as usize,
            owner: None,
            classname: "mpweapon",
        }
    }

    #[test]
    fn the_touch_box_is_36_across_and_18_up_88_down_inside_the_query() {
        let item = [0.0, 0.0, 0.0];
        assert!(touches([0.0, 0.0, 1.0], item));
        assert!(touches([36.0, -36.0, 0.0], item));
        assert!(!touches([36.5, 0.0, 0.0], item));
        assert!(touches([0.0, 0.0, 18.0], item));
        assert!(!touches([0.0, 0.0, 18.5], item));
        // The item above the player: the query box (52 + the item's 1) ends
        // the reach long before `BG_PlayerTouchesItem`'s 88 would.
        assert!(touches([0.0, 0.0, -53.0], item));
        assert!(!touches([0.0, 0.0, -53.5], item));
    }

    #[test]
    fn a_full_carbine_takes_nothing_and_a_fired_one_takes_the_gap() {
        let t = table();
        let mut inv = allies(&t);
        let c = idx("m1carbine_mp") as usize;
        assert_eq!(max_pickupable(&inv, &t, c), 0);
        inv.ammo[t.get(c).unwrap().ammo_index] = 390;
        assert_eq!(max_pickupable(&inv, &t, c), 10);
    }

    /// Both stock frags share one cap of 3, counted off every held grenade's
    /// clip (`BG_GetMaxPickupableAmmo`'s first arm).
    #[test]
    fn the_frags_share_a_cap_of_three() {
        let t = table();
        let mut inv = allies(&t);
        let frag = idx("fraggrenade_mp") as usize;
        let stiel = idx("stielhandgranate_mp") as usize;
        let frag_item = ItemKind::Weapon(frag as u8);
        assert_eq!(max_pickupable(&inv, &t, frag), 0);
        assert!(!can_grab(&inv, frag_item, None, 0, true, &t));
        inv.clip[t.get(frag).unwrap().clip_index] = 1;
        assert_eq!(max_pickupable(&inv, &t, frag), 2);
        assert!(can_grab(&inv, frag_item, None, 0, true, &t));
        inv.weapons.give(stiel, 4);
        inv.clip[t.get(stiel).unwrap().clip_index] = 1;
        assert_eq!(max_pickupable(&inv, &t, stiel), 1);
        assert_eq!(max_pickupable(&inv, &t, frag), 1);
    }

    #[test]
    fn walking_over_an_unowned_weapon_takes_nothing() {
        let t = table();
        let mut inv = allies(&t);
        let mut counts = ItemCounts { count: 90, clip: 0 };
        let out = touch_item(
            &mut inv,
            &view(idx("fg42_mp")),
            &mut counts,
            0,
            true,
            &t,
            false,
            &mut no_rand(),
        );
        assert!(!out.taken);
        assert_eq!(out.log, None);
        assert_eq!(inv, allies(&t));
    }

    /// The capture's use1: a placed fg42 (`count 90`) into `primaryb`, clip
    /// 20, reserve 70, `a 6`, event 146, and `weapons[0]` 4368 to 4560 with
    /// the alt mode's bit 7 beside the fg42's 6 (section 12.4).
    #[test]
    fn the_use_key_takes_a_placed_fg42_into_primaryb() {
        let t = table();
        let mut inv = allies(&t);
        let fg = idx("fg42_mp");
        let d = t.get(fg as usize).unwrap();
        let mut counts = ItemCounts { count: 90, clip: 0 };
        let out = touch_item(
            &mut inv,
            &view(fg),
            &mut counts,
            0,
            false,
            &t,
            false,
            &mut no_rand(),
        );
        assert!(out.taken);
        assert_eq!(out.event, Some(EV_ITEM_PICKUP));
        assert_eq!(out.commands, vec![format!("a {fg}")]);
        assert_eq!(out.log.as_deref(), Some("Weapon: 0 fg42_mp"));
        assert_eq!(out.drop, None);
        assert_eq!(inv.weapons.slots[2], fg);
        assert_eq!(inv.clip[d.clip_index], 20);
        assert_eq!(inv.ammo[d.ammo_index], 70);
        assert_eq!(inv.weapons.held as u32, 4560);
        assert_eq!(inv.weapons.slot_words()[0], 0x04060C00);
    }

    /// The capture's touch2: the second fg42 onto one already held, by touch.
    #[test]
    fn walking_over_a_held_weapon_takes_its_ammo() {
        let t = table();
        let mut inv = allies(&t);
        let fg = idx("fg42_mp");
        let d = t.get(fg as usize).unwrap();
        inv.weapons.give(fg as usize, 2);
        inv.clip[d.clip_index] = 20;
        inv.ammo[d.ammo_index] = 70;
        let mut counts = ItemCounts { count: 90, clip: 0 };
        let out = touch_item(
            &mut inv,
            &view(fg),
            &mut counts,
            0,
            true,
            &t,
            false,
            &mut no_rand(),
        );
        assert!(out.taken);
        assert_eq!(out.event, Some(EV_AMMO_PICKUP));
        assert_eq!(
            out.commands,
            vec!["f \"GAME_PICKUP_AMMO\u{14}WEAPON_FG42\"".to_string()]
        );
        assert_eq!(inv.ammo[d.ammo_index], 160);
        assert_eq!(inv.clip[d.clip_index], 20);
    }

    /// One round short of full: the gain is 1. Under `g_weaponAmmoPools 0`
    /// the item goes anyway; under 1 it stays, 69 in reserve and the clip
    /// untouched, with no event.
    #[test]
    fn a_partial_take_consumes_the_item_unless_ammo_pools_is_on() {
        let t = table();
        let fg = idx("fg42_mp");
        let d = t.get(fg as usize).unwrap();
        for pools in [false, true] {
            let mut inv = allies(&t);
            inv.weapons.give(fg as usize, 2);
            inv.clip[d.clip_index] = 20;
            inv.ammo[d.ammo_index] = 319;
            let mut counts = ItemCounts { count: 90, clip: 0 };
            let out = touch_item(
                &mut inv,
                &view(fg),
                &mut counts,
                0,
                true,
                &t,
                pools,
                &mut no_rand(),
            );
            assert_eq!(inv.ammo[d.ammo_index], 320);
            assert_eq!(out.taken, !pools);
            if pools {
                assert_eq!(out.event, None);
                assert_eq!(
                    counts,
                    ItemCounts {
                        count: 69,
                        clip: 20
                    }
                );
            }
        }
    }

    /// The capture's use2: the carbine in hand, both primaries full, a placed
    /// panzerfaust. The carbine goes with its whole reserve and clip; the
    /// panzerfaust's zero drop range still yields its one round.
    #[test]
    fn the_use_key_swaps_the_held_primary_for_a_panzerfaust() {
        let t = table();
        let mut inv = allies(&t);
        let (fg, pf, carbine) = (idx("fg42_mp"), idx("panzerfaust_mp"), idx("m1carbine_mp"));
        inv.weapons.give(fg as usize, 2);
        let mut counts = ItemCounts { count: 0, clip: 0 };
        let out = touch_item(
            &mut inv,
            &view(pf),
            &mut counts,
            0,
            false,
            &t,
            false,
            &mut no_rand(),
        );
        assert!(out.taken);
        assert_eq!(
            out.drop,
            Some(Dropped {
                weapon: carbine,
                count: 400,
                clip: 15
            })
        );
        assert_eq!(inv.weapons.slots[1], pf);
        assert!(!inv.weapons.holds(carbine as usize));
        let d = t.get(pf as usize).unwrap();
        assert_eq!(inv.clip[d.clip_index], 1);
        assert_eq!(inv.ammo[d.ammo_index], 0);
        assert_eq!(out.commands, vec![format!("a {pf}")]);
    }

    /// The capture's three grabs in order, use1, use2 and late, each ending
    /// on the `weapons[0]` and `weaponslots[0]` words the capture read
    /// (sections 12.4, 12.6 and 12.7). The pickup writes no `ps.weapon`: the
    /// capture's 0 and `EV_RAISE_WEAPON` on a swap frame are pmove's.
    #[test]
    fn the_capture_s_grabs_end_on_its_weapon_words() {
        let t = table();
        let mut inv = allies(&t);
        let (fg, pf, carbine) = (idx("fg42_mp"), idx("panzerfaust_mp"), idx("m1carbine_mp"));
        assert_eq!(
            (inv.weapons.held as u32, inv.weapons.slot_words()[0]),
            (4368, 0x04000C00)
        );
        let grab = |inv: &mut Inventory, w: u8, counts: &mut ItemCounts| {
            touch_item(inv, &view(w), counts, 0, false, &t, false, &mut no_rand())
        };

        assert!(grab(&mut inv, fg, &mut ItemCounts { count: 90, clip: 0 }).taken);
        assert_eq!(
            (inv.weapons.held as u32, inv.weapons.slot_words()[0]),
            (4560, 0x04060C00)
        );

        let out = grab(&mut inv, pf, &mut ItemCounts { count: 0, clip: 0 });
        assert_eq!(inv.weapons.current, carbine);
        assert_eq!(
            (inv.weapons.held as u32, inv.weapons.slot_words()[0]),
            (8389072, 0x04061700)
        );

        // The probe answered `a 23` before the late tap.
        inv.weapons.current = pf;
        let dropped = out.drop.unwrap();
        let mut counts = ItemCounts {
            count: dropped.count,
            clip: dropped.clip,
        };
        let out = grab(&mut inv, carbine, &mut counts);
        assert!(out.taken);
        assert_eq!(out.event, Some(EV_ITEM_PICKUP));
        assert_eq!(out.commands, vec![format!("a {carbine}")]);
        assert_eq!(out.drop.map(|d| d.weapon), Some(pf));
        assert_eq!(
            (inv.weapons.held as u32, inv.weapons.slot_words()[0]),
            (4560, 0x04060C00)
        );
        let d = t.get(carbine as usize).unwrap();
        assert_eq!((inv.clip[d.clip_index], inv.ammo[d.ammo_index]), (15, 400));
        assert_eq!(inv.clip[t.get(pf as usize).unwrap().clip_index], 0);
    }

    /// A primary picked up with a pistol in hand drops `primary` whichever
    /// primary that is, because the test that decides reads the new weapon's
    /// own ammo (`Pickup_Weapon`'s fourth arm).
    #[test]
    fn a_pistol_in_hand_drops_slot_one() {
        let t = table();
        let mut inv = allies(&t);
        let (fg, pf, carbine) = (idx("fg42_mp"), idx("panzerfaust_mp"), idx("m1carbine_mp"));
        inv.weapons.give(fg as usize, 2);
        inv.weapons.current = idx("colt_mp");
        let out = touch_item(
            &mut inv,
            &view(pf),
            &mut ItemCounts { count: 0, clip: 0 },
            0,
            false,
            &t,
            false,
            &mut no_rand(),
        );
        assert_eq!(out.drop.map(|d| d.weapon), Some(carbine));
        assert_eq!(inv.weapons.slots[1], pf);
        assert_eq!(inv.weapons.slots[2], fg);
    }

    /// The same with the new weapon's own ammo index already holding rounds:
    /// nothing is picked up and the player is told.
    #[test]
    fn a_pistol_in_hand_and_ammo_for_the_new_weapon_refuses() {
        let t = table();
        let mut inv = allies(&t);
        let pf = idx("panzerfaust_mp");
        inv.weapons.give(idx("fg42_mp") as usize, 2);
        inv.weapons.current = idx("colt_mp");
        inv.ammo[t.get(pf as usize).unwrap().ammo_index] = 1;
        let before = inv;
        let out = touch_item(
            &mut inv,
            &view(pf),
            &mut ItemCounts { count: 0, clip: 0 },
            0,
            false,
            &t,
            false,
            &mut no_rand(),
        );
        assert!(!out.taken);
        assert_eq!(
            out.commands,
            vec!["f \"GAME_CANT_GET_PRIMARY_WEAP_MESSAGE\"".to_string()]
        );
        assert_eq!(inv, before);
    }

    #[test]
    fn the_dropper_cannot_take_its_own_drop_until_the_owner_clears() {
        let t = table();
        let inv = allies(&t);
        let fg = ItemKind::Weapon(idx("fg42_mp"));
        assert!(!can_grab(&inv, fg, Some(0), 0, false, &t));
        assert!(can_grab(&inv, fg, Some(1), 0, false, &t));
        assert!(can_grab(&inv, fg, None, 0, false, &t));
    }

    #[test]
    fn health_heals_by_the_quantity_and_is_refused_at_full() {
        let t = table();
        let med = ItemView {
            index: 68,
            owner: None,
            classname: "item_health",
        };
        for (from, to) in [(50, 75), (90, 100)] {
            let mut inv = allies(&t);
            inv.health = from;
            let out = touch_item(
                &mut inv,
                &med,
                &mut ItemCounts { count: 0, clip: 0 },
                3,
                true,
                &t,
                false,
                &mut no_rand(),
            );
            assert!(out.taken);
            assert_eq!(inv.health, to);
            assert_eq!(out.event, Some(EV_ITEM_PICKUP));
            assert_eq!(
                out.commands,
                vec!["f \"GAME_PICKUP_HEALTH\u{15}25\"".to_string()]
            );
            assert_eq!(out.log.as_deref(), Some("Item: 3 item_health"));
        }
        let mut inv = allies(&t);
        let out = touch_item(
            &mut inv,
            &med,
            &mut ItemCounts { count: 0, clip: 0 },
            3,
            true,
            &t,
            false,
            &mut no_rand(),
        );
        assert!(!out.taken);
    }

    #[test]
    fn a_dead_player_takes_nothing() {
        let t = table();
        let mut inv = allies(&t);
        inv.health = 0;
        inv.alive = false;
        let med = ItemView {
            index: 68,
            owner: None,
            classname: "item_health",
        };
        assert!(
            !touch_item(
                &mut inv,
                &med,
                &mut ItemCounts { count: 0, clip: 0 },
                0,
                true,
                &t,
                false,
                &mut no_rand()
            )
            .taken
        );
    }

    /// `Drop_Weapon`: 0 rounds is written -1 on the item, and a `clipOnly`
    /// weapon with an empty clip is taken with nothing left behind.
    #[test]
    fn a_drop_carries_the_rounds_and_an_empty_clip_only_weapon_leaves_nothing() {
        let t = table();
        let mut inv = allies(&t);
        let carbine = idx("m1carbine_mp");
        let d = t.get(carbine as usize).unwrap();
        inv.clip[d.clip_index] = 0;
        assert_eq!(
            drop_weapon(&mut inv, &t, carbine),
            Some(Dropped {
                weapon: carbine,
                count: 400,
                clip: -1
            })
        );
        assert_eq!((inv.ammo[d.ammo_index], inv.clip[d.clip_index]), (0, 0));
        assert!(!inv.weapons.holds(carbine as usize));
        let pf = idx("panzerfaust_mp");
        inv.weapons.give(pf as usize, 1);
        assert_eq!(drop_weapon(&mut inv, &t, pf), None);
        assert!(!inv.weapons.holds(pf as usize));
        assert_eq!(drop_weapon(&mut inv, &t, idx("fg42_mp")), None);
    }

    /// A dropped carbine picked back up: the item's -1 clip reads as 0, which
    /// overwrites the player's clip, and `Add_Ammo`'s `fillClip` is never set
    /// from here, so the reserve stays reserve (section 4.3).
    #[test]
    fn an_empty_clip_on_the_item_stays_empty() {
        let t = table();
        let mut inv = allies(&t);
        let carbine = idx("m1carbine_mp");
        let d = t.get(carbine as usize).unwrap();
        inv.weapons.take(carbine as usize);
        inv.weapons.current = idx("colt_mp");
        inv.ammo[d.ammo_index] = 0;
        inv.clip[d.clip_index] = 5;
        let mut counts = ItemCounts {
            count: 30,
            clip: -1,
        };
        let out = touch_item(
            &mut inv,
            &view(carbine),
            &mut counts,
            0,
            false,
            &t,
            false,
            &mut no_rand(),
        );
        assert!(out.taken);
        assert_eq!((inv.clip[d.clip_index], inv.ammo[d.ammo_index]), (0, 30));
    }

    /// Swapping out an empty `clipOnly` weapon: `Drop_Weapon` takes it and
    /// drops nothing, and with nothing dropped `Pickup_Weapon` returns 0
    /// before it gives the new one (section 5).
    #[test]
    fn swapping_out_an_empty_panzerfaust_loses_it_and_takes_nothing() {
        let t = table();
        let mut inv = allies(&t);
        let (fg, pf, carbine) = (idx("fg42_mp"), idx("panzerfaust_mp"), idx("m1carbine_mp"));
        inv.weapons.take(carbine as usize);
        inv.weapons.give(pf as usize, 1);
        inv.weapons.give(fg as usize, 2);
        inv.weapons.current = pf;
        let out = touch_item(
            &mut inv,
            &view(carbine),
            &mut ItemCounts { count: 30, clip: 0 },
            0,
            false,
            &t,
            false,
            &mut no_rand(),
        );
        assert!(!out.taken);
        assert_eq!(out.drop, None);
        assert!(!inv.weapons.holds(pf as usize));
        assert!(!inv.weapons.holds(carbine as usize));
        assert_eq!(inv.weapons.slots[1], 0);
    }

    /// Between a swap and the pmove step that clears `ps.weapon`, the weapon
    /// in hand is one the player no longer holds, and retail refuses every
    /// unowned grab before it looks at a slot (section 5).
    #[test]
    fn a_weapon_gone_from_hand_refuses_an_unowned_grab() {
        let t = table();
        let mut inv = allies(&t);
        inv.weapons.take(idx("m1carbine_mp") as usize);
        let before = inv;
        let out = touch_item(
            &mut inv,
            &view(idx("fg42_mp")),
            &mut ItemCounts { count: 90, clip: 0 },
            0,
            false,
            &t,
            false,
            &mut no_rand(),
        );
        assert!(!out.taken);
        assert!(out.commands.is_empty());
        assert_eq!(inv, before);
    }

    /// Case 5: a held weapon that sits in no slot, with no empty slot for
    /// the new one, refuses the grab without a message.
    #[test]
    fn a_slotless_weapon_in_hand_is_never_swapped_out() {
        let t = table();
        let mut inv = allies(&t);
        let pf = idx("panzerfaust_mp");
        inv.weapons.give(idx("fg42_mp") as usize, 2);
        inv.weapons.give(pf as usize, 0);
        inv.weapons.current = pf;
        let before = inv;
        let out = touch_item(
            &mut inv,
            &view(idx("stielhandgranate_mp")),
            &mut ItemCounts { count: 0, clip: 0 },
            0,
            false,
            &t,
            false,
            &mut no_rand(),
        );
        assert!(!out.taken);
        assert!(out.commands.is_empty());
        assert_eq!(inv, before);
    }

    /// An alt mode in hand counts as in its base weapon's slot, so it is not
    /// case 5: the grenade swap goes through case 3.
    #[test]
    fn an_alt_mode_in_hand_sits_in_its_base_weapon_s_slot() {
        let t = table();
        let mut inv = allies(&t);
        let (fg, semi) = (idx("fg42_mp"), idx("fg42_semi_mp"));
        inv.weapons.give(fg as usize, 2);
        inv.weapons.give(semi as usize, 0);
        inv.weapons.current = semi;
        let out = touch_item(
            &mut inv,
            &view(idx("stielhandgranate_mp")),
            &mut ItemCounts { count: 0, clip: 0 },
            0,
            false,
            &t,
            false,
            &mut no_rand(),
        );
        assert!(out.taken);
        assert_eq!(out.drop.map(|d| d.weapon), Some(idx("fraggrenade_mp")));
        assert_eq!(inv.weapons.slots[4], idx("stielhandgranate_mp"));
    }

    /// The owned arm's leftover is written to the item's own fields: a
    /// `count` of -1 goes 5 further below zero and that comes off the clip,
    /// 15 to 9 (0x4d2a0..0x4d2cf), where subtracting the gain from the
    /// resolved reserve of 0 would leave 10.
    #[test]
    fn a_negative_count_takes_its_overflow_off_the_clip() {
        let t = table();
        let mut inv = allies(&t);
        let carbine = idx("m1carbine_mp");
        let d = t.get(carbine as usize).unwrap();
        inv.ammo[d.ammo_index] = 395;
        let mut counts = ItemCounts {
            count: -1,
            clip: 15,
        };
        let out = touch_item(
            &mut inv,
            &view(carbine),
            &mut counts,
            0,
            true,
            &t,
            true,
            &mut no_rand(),
        );
        assert!(!out.taken);
        assert_eq!(inv.ammo[d.ammo_index], 400);
        assert_eq!(counts, ItemCounts { count: -1, clip: 9 });
    }

    /// Under `g_weaponAmmoPools 1` a partial take keeps the item, but the
    /// ammo line has already gone out on the gain (0x4d23b..0x4d28f).
    #[test]
    fn a_pools_refusal_still_sends_the_ammo_line() {
        let t = table();
        let mut inv = allies(&t);
        let fg = idx("fg42_mp");
        let d = t.get(fg as usize).unwrap();
        inv.weapons.give(fg as usize, 2);
        inv.clip[d.clip_index] = 20;
        inv.ammo[d.ammo_index] = 319;
        let out = touch_item(
            &mut inv,
            &view(fg),
            &mut ItemCounts { count: 90, clip: 0 },
            0,
            true,
            &t,
            true,
            &mut no_rand(),
        );
        assert!(!out.taken);
        assert_eq!(
            out.commands,
            vec!["f \"GAME_PICKUP_AMMO\u{14}WEAPON_FG42\"".to_string()]
        );
    }

    #[test]
    fn the_activate_cone_is_128_units_and_cosine_point_76() {
        let fwd = [1.0, 0.0, 0.0];
        let s = activate_score([0.0; 3], fwd, [60.0, 0.0, 0.0]).unwrap();
        assert!((s - 60.0).abs() < 1e-3, "{s}");
        assert!(activate_score([0.0; 3], fwd, [128.5, 0.0, 0.0]).is_none());
        assert!(activate_score([0.0; 3], fwd, [40.0, 40.0, 0.0]).is_none());
    }

    #[test]
    fn the_cursor_hint_is_9_or_73_past_the_weapon_and_7_for_health() {
        assert_eq!(cursor_hint(ItemKind::Weapon(6), false), 15);
        assert_eq!(cursor_hint(ItemKind::Weapon(6), true), 79);
        assert_eq!(cursor_hint(ItemKind::Health { quantity: 25 }, false), 7);
    }
}
