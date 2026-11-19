//! The item registry: `bg_itemlist` (`.data` 0x7b9d8), `precacheItem`'s
//! `BG_FindItem` (`.text` 0x2e214) lookup, and the configstring 8 bitstring
//! `SaveRegisteredItems` (`.text` 0x4ef08) packs from the registration
//! bitset. `docs/research/cod11-gsc-object-model.md` has the full item-table
//! story (the placeholder weapon slots, the runtime fill, both decoded
//! captures); this module only reproduces `Items::register`,
//! `Items::bitstring` and the item models `RegisterItem` precaches.

use crate::defs::Defs;
use crate::weapons::WeaponTable;
use vcod_common::pk3::Pk3Fs;

/// `BG_FindItem` (0x2e214) is a `strcmp`, so the lookup is case-sensitive;
/// an unmatched name returns `None`, same as retail's miss.
fn item_index(defs: &Defs, name: &str) -> Option<usize> {
    if let Some(i) = defs.weapon_index(name) {
        return Some(i);
    }
    defs.static_items()
        .find(|(_, n, _)| *n == name)
        .map(|(i, ..)| i)
}

/// Whether `BG_FindItem` knows `name`.
pub fn is_item(defs: &Defs, name: &str) -> bool {
    item_index(defs, name).is_some()
}

/// The `bg_itemlist` row a spawned classname names, the match `G_CallSpawn`
/// makes before its `spawns` table: a weapon's Radiant name or one of the
/// five compiled-in rows.
pub fn classname_index(defs: &Defs, classname: &str) -> Option<usize> {
    crate::game::spawn::radiant_weapon(classname)
        .and_then(|w| defs.weapon_index(w))
        .or_else(|| {
            defs.static_items()
                .find(|(_, n, _)| *n == classname)
                .map(|(i, ..)| i)
        })
}

/// The item name at `index`, the inverse of `item_index`.
pub(crate) fn item_name(defs: &Defs, index: usize) -> Option<&'static str> {
    if let Some(name) = defs.weapon_name(index) {
        return Some(name);
    }
    defs.static_items()
        .find(|(i, ..)| *i == index)
        .map(|(_, n, _)| n)
}

/// The models `RegisterItem` precaches for an item, `bg_itemlist`'s +0x8
/// and +0xc (and UO's pickup model). For the five compiled-in rows they are in the binary; a
/// weapon's pair is filled at runtime, so it comes from the weapon file's
/// `worldModel` and `projectileModel`, kept verbatim because the model
/// configstring holds the spelling the file uses. Without a filesystem
/// (every unit test that does not mount the paks) a weapon contributes
/// nothing.
pub fn item_models(defs: &Defs, fs: Option<&Pk3Fs>, name: &str) -> Vec<String> {
    if let Some((.., model)) = defs.static_items().find(|(_, n, _)| *n == name) {
        return vec![model.to_string()];
    }
    let Some(fs) = fs else {
        return Vec::new();
    };
    let Some(bytes) = fs.read(&format!("weapons/mp/{name}")) else {
        return Vec::new();
    };
    let map = vcod_common::xmodel::parse_weapon(&String::from_utf8_lossy(&bytes));
    // UO's files add an `attachModel` and a `pickupModel`, which UO's
    // `RegisterItem` takes in that order before the projectile (coduo-diff.md
    // "Items").
    [
        "worldModel",
        "attachModel",
        "pickupModel",
        "projectileModel",
    ]
    .iter()
    .filter_map(|k| map.get(k))
    .filter(|v| !v.is_empty())
    .cloned()
    .collect()
}

/// `bg_itemlist`'s `+0x18` for the five compiled-in rows, the name
/// `RegisterItem`'s late-registration error prints (1.1d, 1.5b and UO
/// alike, read with `gameso.py`); a weapon row's is blank there, so the
/// error takes the weapon file's internal name (`def+4`) instead.
fn pickup_name(classname: &str) -> Option<&'static str> {
    Some(match classname {
        "item_ammo_stielhandgranate_open" => "Stielhandgranate_mp Ammo Open",
        "item_ammo_stielhandgranate_closed" => "Stielhandgranate_mp Ammo Closed",
        "item_health_small" => "Small Health",
        "item_health" => "Med Health",
        "item_health_large" => "Large Health",
        _ => return None,
    })
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
#[derive(Clone)]
pub struct Items {
    defs: &'static Defs,
    registered: Vec<bool>,
}

impl Default for Items {
    fn default() -> Items {
        Items::new(&crate::defs::V1)
    }
}

impl Items {
    pub fn new(defs: &'static Defs) -> Items {
        let mut registered = vec![false; defs.num_items()];
        // M1: index 0's classname is blank (`dump_itemlist.py`), so no
        // `precacheItem` call can ever name it, and `ClearRegisteredItems`
        // (0x4eecc) only `bzero`s `itemRegistered`; it does not special-
        // case index 0 either. The only writer is `RegisterItem` (0x4e504),
        // reached from a handful of call sites that pass a computed weapon
        // index rather than a literal; two are inside `BG_GivePlayerWeapon`
        // (0x36a38), which every connecting player's default "no weapon"
        // slot (item index 0) runs through. Both retail captures set bit 0
        // with no map-specific factor in common besides that, so it is
        // reproduced here as engine-seeded rather than script-reachable.
        registered[0] = true;
        Items { defs, registered }
    }

    /// `RegisterItem` (0x4e504): marks `name` registered and, for a weapon,
    /// every weapon on its `altWeapon` chain (object model doc 15, M2). An
    /// item already registered is left alone; a new one past
    /// `level.initializing` is the script error retail raises. An unknown
    /// name is a `BG_FindItem` miss and registers nothing. Returns the items
    /// registered, the named one first, which `GameHost::register_item`
    /// walks to precache their models.
    pub fn register(
        &mut self,
        name: &str,
        weapons: &WeaponTable,
        initializing: bool,
    ) -> Result<Vec<&'static str>, String> {
        let Some(index) = item_index(self.defs, name) else {
            return Ok(Vec::new());
        };
        if self.registered[index] {
            return Ok(Vec::new());
        }
        let own = item_name(self.defs, index).unwrap_or_default();
        if !initializing {
            let shown = pickup_name(own).unwrap_or(own);
            return Err(format!(
                "game tried to register the item '{shown}' after initialization finished"
            ));
        }
        let chain = if index <= self.defs.proto.max_weapons {
            weapons.alt_chain(index)
        } else {
            vec![index]
        };
        let mut done = Vec::new();
        for i in chain {
            if let Some(r) = self.registered.get_mut(i) {
                *r = true;
                done.extend(item_name(self.defs, i));
            }
        }
        Ok(done)
    }

    /// `SaveRegisteredItems` (0x4ef08): four items per lowercase hex
    /// character, bit `i & 3`, the trailing partial nibble flushed same as
    /// a full one. `bg_numItems` is 70 (UO 134), so this is 18 characters
    /// (34).
    pub fn bitstring(&self) -> String {
        let n = self.registered.len();
        let mut out = String::with_capacity(n.div_ceil(4));
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
        if !n.is_multiple_of(4) {
            out.push(hex_digit(nibble));
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every configstring 7 weapon, the five stock alt-fire pairs naming
    /// each other through `altWeapon` as their files do.
    fn alt_table() -> WeaponTable {
        let pairs = [
            ("bar_mp", "bar_slow_mp"),
            ("fg42_mp", "fg42_semi_mp"),
            ("mp44_mp", "mp44_semi_mp"),
            ("ppsh_mp", "ppsh_semi_mp"),
            ("thompson_mp", "thompson_semi_mp"),
        ];
        let names = crate::defs::V1.weapons.split(' ');
        let mut defs: Vec<Option<vcod_common::weapon::WeaponDef>> = std::iter::once(None)
            .chain(names.clone().map(|name| {
                let alt = pairs
                    .iter()
                    .find_map(|&(a, b)| (a == name).then_some(b).or((b == name).then_some(a)));
                let map = alt
                    .map(|a| ("altWeapon".to_string(), a.to_string()))
                    .into_iter()
                    .collect();
                Some(vcod_common::weapon::WeaponDef::from_map(&map))
            }))
            .collect();
        vcod_common::weapon_table::link_alts(&mut defs, names);
        WeaponTable::from_defs(defs)
    }

    fn register(items: &mut Items, name: &str) {
        items.register(name, &alt_table(), true).unwrap();
    }

    /// The packing from `SaveRegisteredItems` (0x4ef08): four items per
    /// lowercase hex character, bit `i & 3`, partial nibble flushed.
    /// `bg_numItems` is 70, so the string is 18 characters.
    #[test]
    fn the_bitstring_is_eighteen_characters_and_packs_four_items_a_nibble() {
        let items = Items::default();
        assert_eq!(items.bitstring().len(), 18);
    }

    /// A weapon's item index is its 1-based index into configstring 7:
    /// `bar_mp` is index 1 and `thompson_semi_mp` is 32, so they land in
    /// the first and eighth nibble respectively. `bar_mp` also carries its
    /// alt mode `bar_slow_mp` (index 2) along by M2's auto-link, and index
    /// 0 is already set (M1), so nibble 0 covers bits 0-2: `0b0111` = `7`.
    #[test]
    fn a_weapon_registers_at_its_one_based_configstring_seven_index() {
        let mut items = Items::default();
        register(&mut items, "bar_mp");
        assert!(items.bitstring().starts_with('7'));
        let mut items = Items::default();
        register(&mut items, "thompson_semi_mp");
        assert_eq!(items.bitstring().chars().nth(8), Some('1'));
    }

    /// The alt link is the weapon file's `altWeapon`, not configstring 7
    /// adjacency: an alt mode registers its base, and a weapon whose file
    /// names no alt registers alone even beside a `_semi_mp` neighbour.
    #[test]
    fn the_alt_link_follows_the_weapon_files() {
        let mut items = Items::default();
        assert_eq!(
            items.register("thompson_semi_mp", &alt_table(), true),
            Ok(vec!["thompson_semi_mp", "thompson_mp"])
        );
        let mut items = Items::default();
        assert_eq!(
            items.register("bar_mp", &WeaponTable::empty(), true),
            Ok(vec!["bar_mp"])
        );
    }

    /// Past `level.initializing` a new item is `RegisterItem`'s script
    /// error and sets nothing; one already registered passes and indexes
    /// nothing again. A compiled-in row names itself by its pickup name.
    #[test]
    fn a_new_item_past_initialization_is_an_error() {
        let mut items = Items::default();
        register(&mut items, "bar_mp");
        let before = items.bitstring();
        assert_eq!(
            items.register("bar_slow_mp", &alt_table(), false),
            Ok(vec![])
        );
        assert_eq!(
            items.register("mp40_mp", &alt_table(), false),
            Err("game tried to register the item 'mp40_mp' after initialization finished".into())
        );
        assert_eq!(
            items.register("item_health_small", &alt_table(), false),
            Err(
                "game tried to register the item 'Small Health' after initialization finished"
                    .into()
            )
        );
        assert_eq!(items.bitstring(), before);
    }

    /// An unknown item name registers nothing, as retail's `BG_FindItem`
    /// miss does.
    #[test]
    fn an_unknown_item_name_registers_nothing() {
        let mut items = Items::default();
        let before = Items::default().bitstring();
        register(&mut items, "no_such_item_mp");
        assert_eq!(items.bitstring(), before);
    }

    /// `dm` registers `item_health` and `_teams::precache()` the Russian
    /// and German branches. Every bit this sets must also be set in
    /// `mp_pavlov`'s retail capture; the extras there are the map's placed
    /// weapons (`fg42_mp`, `panzerfaust_mp`), which the next task adds.
    #[test]
    fn the_script_registrations_are_a_subset_of_retails_mp_pavlov_bits() {
        let mut items = Items::default();
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
            register(&mut items, name);
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
        let mut items = Items::default();
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
            register(&mut items, name);
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
