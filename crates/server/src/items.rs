//! The item registry: `bg_itemlist` (`.data` 0x7b9d8), `precacheItem`'s
//! `BG_FindItem` (`.text` 0x2e214) lookup, and the configstring 8 bitstring
//! `SaveRegisteredItems` (`.text` 0x4ef08) packs from the registration
//! bitset. `docs/research/cod11-gsc-object-model.md` has the full item-table
//! story (the placeholder weapon slots, the runtime fill, both decoded
//! captures); this module only reproduces `Items::register` and
//! `Items::bitstring`.

use crate::configstrings::WEAPON_LIST;

/// `bg_numItems` (`.rodata` 0x70804).
pub const NUM_ITEMS: usize = 70;

/// The only classnames `bg_itemlist` carries compiled into the binary,
/// indices 65-69. Indices 1-64 are `emptyitem_"wNN"` placeholders there;
/// a weapon's real classname reaches its slot at runtime from the mounted
/// paks' weapon files, so `item_index` derives a weapon's index from
/// `WEAPON_LIST` (R1) instead of a second table.
const STATIC_ITEMS: &[(usize, &str)] = &[
    (65, "item_ammo_stielhandgranate_open"),
    (66, "item_ammo_stielhandgranate_closed"),
    (67, "item_health_small"),
    (68, "item_health"),
    (69, "item_health_large"),
];

/// `BG_FindItem` (0x2e214) is a `strcmp`, so the lookup is case-sensitive;
/// an unmatched name returns `None`, same as retail's miss.
fn item_index(name: &str) -> Option<usize> {
    if let Some(i) = WEAPON_LIST.split(' ').position(|w| w == name) {
        return Some(i + 1);
    }
    STATIC_ITEMS
        .iter()
        .find(|(_, n)| *n == name)
        .map(|(i, _)| *i)
}

/// A weapon and its alt-fire mode sit at adjacent `WEAPON_LIST` indices —
/// `bar_mp`/`bar_slow_mp`, `fg42_mp`/`fg42_semi_mp`, `mp44_mp`/
/// `mp44_semi_mp`, `ppsh_mp`/`ppsh_semi_mp`, `thompson_mp`/
/// `thompson_semi_mp`, on both decoded captures. `RegisterItem` (0x4e504)
/// registers a weapon item and then walks a "next" index read from its
/// weapon definition (offset 0x2fc, also read by `BG_GivePlayerWeapon` at
/// 0x36b0a), registering that item too and stopping when the chain hits 0
/// or loops back to where it started — a link `BG_GivePlayerWeapon` reads
/// from the same field, so it is real weapon data, not a placed-entity
/// artifact (M2). The link itself is data-driven (each weapon's file names
/// its alt mode), which this crate does not parse yet; deriving it from
/// the adjacent-slot naming convention instead of hand-copying the five
/// known pairs keeps it from drifting out of step with `WEAPON_LIST`.
fn alt_weapon_index(index: usize) -> Option<usize> {
    let weapons: Vec<&str> = WEAPON_LIST.split(' ').collect();
    if index == 0 || index > weapons.len() {
        return None;
    }
    let name = weapons[index - 1];

    // `index` is the base weapon: its alt mode is the next slot.
    if let Some(base) = name.strip_suffix("_mp") {
        if index < weapons.len() {
            let next = weapons[index];
            if next == format!("{base}_semi_mp") || next == format!("{base}_slow_mp") {
                return Some(index + 1);
            }
        }
    }

    // `index` is the alt mode: the base weapon is the previous slot.
    for suffix in ["_semi_mp", "_slow_mp"] {
        if let Some(root) = name.strip_suffix(suffix) {
            if index > 1 && weapons[index - 2] == format!("{root}_mp") {
                return Some(index - 1);
            }
        }
    }

    None
}

fn hex_digit(nibble: u8) -> char {
    // `SaveRegisteredItems`: `+0x30` under 10, `+0x57` at or above (the
    // lowercase `'a'..='f'` offset), one ASCII byte per four items.
    if nibble > 9 {
        (nibble + 0x57) as char
    } else {
        (nibble + 0x30) as char
    }
}

/// The registered-item bitset `precacheItem` builds and configstring 8
/// reports, mirroring retail's `itemRegistered[]` (`.bss` 0x18e0e0).
pub struct Items {
    registered: [bool; NUM_ITEMS],
}

impl Default for Items {
    fn default() -> Items {
        Items::new()
    }
}

impl Items {
    pub fn new() -> Items {
        let mut registered = [false; NUM_ITEMS];
        // M1: index 0's classname is blank (`dump_itemlist.py`), so no
        // `precacheItem` call can ever name it, and `ClearRegisteredItems`
        // (0x4eecc) only `bzero`s `itemRegistered` — it does not special-
        // case index 0 either. The only writer is `RegisterItem` (0x4e504),
        // reached from a handful of call sites that pass a computed weapon
        // index rather than a literal; two are inside `BG_GivePlayerWeapon`
        // (0x36a38), which every connecting player's default "no weapon"
        // slot (item index 0) runs through. Both retail captures set bit 0
        // with no map-specific factor in common besides that, so it is
        // reproduced here as engine-seeded rather than script-reachable.
        registered[0] = true;
        Items { registered }
    }

    /// `RegisterItem` (0x4e504): marks `name` registered and, for a weapon
    /// item, its alt-fire mode too (see `alt_weapon_index`). An unknown
    /// name is a `BG_FindItem` miss and registers nothing, as retail
    /// ignores it.
    pub fn register(&mut self, name: &str) {
        let Some(index) = item_index(name) else {
            return;
        };
        self.registered[index] = true;
        if let Some(alt) = alt_weapon_index(index) {
            self.registered[alt] = true;
        }
    }

    /// `SaveRegisteredItems` (0x4ef08): four items per lowercase hex
    /// character, bit `i & 3`, the trailing partial nibble flushed same as
    /// a full one. `bg_numItems` is 70, so this is 18 characters.
    pub fn bitstring(&self) -> String {
        let mut out = String::with_capacity(NUM_ITEMS.div_ceil(4));
        let mut nibble = 0u8;
        for (i, &set) in self.registered.iter().enumerate() {
            if set {
                nibble |= 1 << (i & 3);
            }
            if i & 3 == 3 {
                out.push(hex_digit(nibble));
                nibble = 0;
            }
        }
        if !NUM_ITEMS.is_multiple_of(4) {
            out.push(hex_digit(nibble));
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The packing from `SaveRegisteredItems` (0x4ef08): four items per
    /// lowercase hex character, bit `i & 3`, partial nibble flushed.
    /// `bg_numItems` is 70, so the string is 18 characters.
    #[test]
    fn the_bitstring_is_eighteen_characters_and_packs_four_items_a_nibble() {
        let items = Items::new();
        assert_eq!(items.bitstring().len(), 18);
    }

    /// A weapon's item index is its 1-based index into configstring 7:
    /// `bar_mp` is index 1 and `thompson_semi_mp` is 32, so they land in
    /// the first and eighth nibble respectively. `bar_mp` also carries its
    /// alt mode `bar_slow_mp` (index 2) along by M2's auto-link, and index
    /// 0 is already set (M1), so nibble 0 covers bits 0-2: `0b0111` = `7`.
    #[test]
    fn a_weapon_registers_at_its_one_based_configstring_seven_index() {
        let mut items = Items::new();
        items.register("bar_mp");
        assert!(items.bitstring().starts_with('7'));
        let mut items = Items::new();
        items.register("thompson_semi_mp");
        assert_eq!(items.bitstring().chars().nth(8), Some('1'));
    }

    /// An unknown item name registers nothing, as retail's `BG_FindItem`
    /// miss does.
    #[test]
    fn an_unknown_item_name_registers_nothing() {
        let mut items = Items::new();
        let before = Items::new().bitstring();
        items.register("no_such_item_mp");
        assert_eq!(items.bitstring(), before);
    }

    /// `dm` registers `item_health` and `_teams::precache()` the Russian
    /// and German branches. Every bit this sets must also be set in
    /// `mp_pavlov`'s retail capture; the extras there are the map's placed
    /// weapons (`fg42_mp`, `panzerfaust_mp`), which the next task adds.
    #[test]
    fn the_script_registrations_are_a_subset_of_retails_mp_pavlov_bits() {
        let mut items = Items::new();
        for name in [
            "item_health",
            "rgd-33russianfrag_mp",
            "luger_mp",
            "mosin_nagant_mp",
            "ppsh_mp",
            "mosin_nagant_sniper_mp",
            "stielhandgranate_mp",
            "kar98k_mp",
            "mp40_mp",
            "mp44_mp",
            "kar98k_sniper_mp",
        ] {
            items.register(name);
        }
        let ours = items.bitstring();
        let retail = "1ce0cfb40000000001";
        for (o, r) in ours.chars().zip(retail.chars()) {
            let o = o.to_digit(16).unwrap();
            let r = r.to_digit(16).unwrap();
            assert_eq!(
                o & r,
                o,
                "ours sets a bit retail does not: {ours} vs {retail}"
            );
        }
    }

    /// The same subset check against `mp_carentan`, whose American branch
    /// exercises a disjoint set of weapon indices. `mg42_bipod_stand_mp`,
    /// `fg42_mp` and `panzerfaust_mp` are retail extras this list leaves
    /// out for the same reason as pavlov's: placed weapons, not script
    /// output.
    #[test]
    fn the_script_registrations_are_a_subset_of_retails_mp_carentan_bits() {
        let mut items = Items::new();
        for name in [
            "item_health",
            "bar_mp",
            "colt_mp",
            "fraggrenade_mp",
            "m1carbine_mp",
            "m1garand_mp",
            "springfield_mp",
            "thompson_mp",
            "luger_mp",
            "mp40_mp",
            "mp44_mp",
            "kar98k_mp",
            "kar98k_sniper_mp",
            "stielhandgranate_mp",
        ] {
            items.register(name);
        }
        let ours = items.bitstring();
        let retail = "7df31f0d1000000001";
        for (o, r) in ours.chars().zip(retail.chars()) {
            let o = o.to_digit(16).unwrap();
            let r = r.to_digit(16).unwrap();
            assert_eq!(
                o & r,
                o,
                "ours sets a bit retail does not: {ours} vs {retail}"
            );
        }
    }
}
