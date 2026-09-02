//! The weapon half of a player's state: which weapons a client holds, which
//! one sits in each weapon slot, and which one it spawned holding. The stock
//! spawn builtins (`giveWeapon`, `setSpawnWeapon`) write it and
//! `ClientSim::to_wire` renders it into `ps.weapons`, `ps.weaponslots` and
//! `ps.weapon`.

use vcod_common::pk3::Pk3Fs;
use vcod_common::pmove::weapon;
use vcod_common::weapon::WeaponDef;

/// The retail `weaponSlot` name table, `.data` 0x7c940. Index 0 is `"none"`,
/// so a weapon's own slot is 1..=5. Object model doc, section 20.
pub const SLOT_NAMES: [&str; 6] = [
    "none",
    "primary",
    "primaryb",
    "pistol",
    "grenade",
    "smokegrenade",
];

/// `ps.weaponslots` is eight bytes carried by the two 32-bit netfields
/// `weaponslots[0]` and `weaponslots[4]`.
pub const NUM_SLOTS: usize = weapon::NUM_SLOTS;

/// One player's weapons, in the wire's own terms.
#[derive(Clone, Copy, Default, PartialEq, Eq, Debug)]
pub struct PlayerWeapons {
    /// `ps.weapons`: bit N set for weapon index N, the 1-based configstring 7
    /// position. Two 32-bit netfields, so bit 63 is the highest that travels.
    /// Decoded against the retail captures in the object model doc, section 20.
    pub held: u64,
    /// `ps.weaponslots`: the weapon index occupying each slot, 0 for empty.
    pub slots: [u8; NUM_SLOTS],
    /// `ps.weapon`, the weapon `setSpawnWeapon` chose.
    pub current: u8,
}

impl PlayerWeapons {
    /// `giveWeapon`: hold the weapon and put it in its file's `weaponSlot`.
    /// A weapon whose file names no slot — including every weapon on a host
    /// with no paks mounted — is still held; only the slot byte goes unset.
    pub fn give(&mut self, index: usize, slot: usize) {
        if index < u64::BITS as usize {
            weapon::give_slot(&mut self.held, &mut self.slots, index as u8, slot);
        }
    }

    /// Whether the player holds that weapon: retail's
    /// `COM_BitCheck(ps.weapons, index)`.
    pub fn holds(&self, index: usize) -> bool {
        index < u64::BITS as usize && weapon::bit_set(self.held, index as u8)
    }

    /// The two 32-bit words `weaponslots[0]` and `weaponslots[4]` carry.
    pub fn slot_words(&self) -> [i32; 2] {
        weapon::pack_slots(&self.slots)
    }
}

/// The `weaponSlot` a weapon file names, as its index in [`SLOT_NAMES`].
/// `None` when there are no paks to read, when the file has no `weaponSlot`
/// key, or when it names a slot the table does not have.
pub fn weapon_slot(fs: Option<&Pk3Fs>, name: &str) -> Option<usize> {
    let bytes = fs?.read(&format!("weapons/mp/{name}"))?;
    let map = vcod_common::xmodel::parse_weapon(&String::from_utf8_lossy(&bytes));
    let named = map.get("weaponSlot")?;
    SLOT_NAMES
        .iter()
        .position(|s| *s == named.as_str())
        .filter(|i| *i > 0)
}

/// Every weapon file in [`crate::configstrings::WEAPON_LIST`], parsed once at
/// map load and indexed the way the wire is (`crate::items::NUM_ITEMS`
/// entries, 0 unused, 1.. the CS 7 order). The animscript reads
/// [`WeaponTable::class`] every frame, so parsing happens once here rather
/// than per-frame.
pub struct WeaponTable {
    defs: Vec<Option<WeaponDef>>,
}

/// The name-table walk `docs/protocol-1.1.md` ("How `ammo[]` and
/// `ammoclip[]` are indexed") describes: lowercase, look up, append on a
/// miss. Ammo and clip names each get their own table, so the two indexes
/// never share a namespace.
fn name_index(table: &mut Vec<String>, name: &str) -> usize {
    let lower = name.to_ascii_lowercase();
    match table.iter().position(|n| *n == lower) {
        Some(i) => i,
        None => {
            table.push(lower);
            table.len() - 1
        }
    }
}

impl WeaponTable {
    /// `crate::items::NUM_ITEMS` `None`s: every index resolves, none carry a
    /// weapon. For tests and hosts with no paks mounted.
    pub fn empty() -> WeaponTable {
        WeaponTable {
            defs: vec![None; crate::items::NUM_ITEMS],
        }
    }

    /// Walks [`crate::configstrings::WEAPON_LIST`] in wire order, parsing
    /// each weapon file and assigning its ammo/clip indexes. A missing file
    /// logs and leaves that slot `None` rather than failing the map load.
    pub fn load(fs: &Pk3Fs) -> WeaponTable {
        let mut ammo_names: Vec<String> = Vec::new();
        let mut clip_names: Vec<String> = Vec::new();
        let mut defs: Vec<Option<WeaponDef>> = vec![None; crate::items::NUM_ITEMS];
        for (i, name) in crate::configstrings::WEAPON_LIST.split(' ').enumerate() {
            match vcod_common::weapon::load(fs, name) {
                Ok(mut def) => {
                    def.ammo_index = name_index(&mut ammo_names, &def.ammo_name);
                    def.clip_index = name_index(&mut clip_names, &def.clip_name);
                    defs[i + 1] = Some(def);
                }
                Err(e) => log::warn!("weapon table: {name}: {e:#}"),
            }
        }
        WeaponTable { defs }
    }

    pub fn get(&self, index: usize) -> Option<&WeaponDef> {
        self.defs.get(index).and_then(|d| d.as_ref())
    }

    pub fn defs(&self) -> &[Option<WeaponDef>] {
        &self.defs
    }

    /// The `weaponclass` condition in `mp/playeranim.script` tests against
    /// this. `""` for the empty slot 0 and for any index with no weapon.
    pub fn class(&self, index: usize) -> &str {
        self.get(index).map_or("", |d| d.weapon_class.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::configstrings::weapon_index;

    /// The wire words the `playerstate_ab` captures pin, decoded from
    /// `crates/server/tests/fixtures/playerstate/mp_carentan-dm.txt`: an
    /// americans join holds `colt_mp` (4), `fraggrenade_mp` (8) and
    /// `m1carbine_mp` (12), and the slot bytes put the carbine in `primary`
    /// and the colt in `pistol`.
    #[test]
    fn the_wire_words_match_the_retail_capture() {
        let mut w = PlayerWeapons::default();
        w.give(weapon_index("colt_mp").unwrap(), 3);
        w.give(weapon_index("fraggrenade_mp").unwrap(), 4);
        w.give(weapon_index("m1carbine_mp").unwrap(), 1);
        w.current = weapon_index("m1carbine_mp").unwrap() as u8;

        assert_eq!(w.held as u32 as i32, 4368);
        assert_eq!(w.slot_words(), [67111936, 8]);
        assert_eq!(w.current, 12);
    }

    /// The russian branch of the same capture: `luger_mp` (11),
    /// `rgd-33russianfrag_mp` (27) and `mosin_nagant_mp` (18).
    #[test]
    fn the_other_nationality_matches_too() {
        let mut w = PlayerWeapons::default();
        w.give(weapon_index("luger_mp").unwrap(), 3);
        w.give(weapon_index("rgd-33russianfrag_mp").unwrap(), 4);
        w.give(weapon_index("mosin_nagant_mp").unwrap(), 1);
        assert_eq!(w.held as u32 as i32, 134481920);
        assert_eq!(w.slot_words(), [184553984, 27]);
    }

    /// A weapon with no slot is still held: nothing but the slot byte depends
    /// on the weapon file, so a host with no paks mounted still reports the
    /// right `ps.weapons`.
    #[test]
    fn a_weapon_with_no_slot_is_still_held() {
        let mut w = PlayerWeapons::default();
        w.give(weapon_index("colt_mp").unwrap(), 0);
        assert_eq!(w.held, 1 << 4);
        assert_eq!(w.slots, [0; NUM_SLOTS]);
    }

    /// The three slots the stock loadout fills, read out of the shipped
    /// weapon files. Needs the paks; without them there is nothing to read.
    #[test]
    fn the_stock_loadout_slots_come_from_the_weapon_files() {
        let Some(fs) = vcod_common::testing::game_fs() else {
            return;
        };
        let fs = Some(&fs);
        assert_eq!(weapon_slot(fs, "m1carbine_mp"), Some(1));
        assert_eq!(weapon_slot(fs, "colt_mp"), Some(3));
        assert_eq!(weapon_slot(fs, "fraggrenade_mp"), Some(4));
        assert_eq!(weapon_slot(fs, "no_such_weapon_mp"), None);
    }

    /// The `weaponclass` condition in `mp/playeranim.script` is this field.
    /// Values read straight out of the shipped weapon files, through the
    /// table rather than the standalone lookup `PlayerWeapons` used to use.
    #[test]
    fn a_weapon_file_names_its_class() {
        let Some(fs) = vcod_common::testing::game_fs() else {
            return;
        };
        let t = WeaponTable::load(&fs);
        assert_eq!(t.class(weapon_index("m1carbine_mp").unwrap()), "rifle");
        assert_eq!(t.class(weapon_index("colt_mp").unwrap()), "pistol");
        assert_eq!(t.class(weapon_index("thompson_mp").unwrap()), "smg");
        assert_eq!(t.class(weapon_index("fraggrenade_mp").unwrap()), "grenade");
        assert_eq!(
            t.class(weapon_index("panzerfaust_mp").unwrap()),
            "rocketlauncher"
        );
    }

    /// A host with no paks mounted still runs; every condition that reads the
    /// class simply fails to match.
    #[test]
    fn a_weapon_class_needs_no_paks_to_be_asked_for() {
        let t = WeaponTable::empty();
        assert_eq!(t.class(weapon_index("m1carbine_mp").unwrap()), "");
    }

    /// The ammo/clip index rule (docs/protocol-1.1.md, "How `ammo[]` and
    /// `ammoclip[]` are indexed"): walk `WEAPON_LIST` in order, lowercase
    /// each def's ammo/clip name, and hand out the next free slot on a miss.
    #[test]
    fn the_table_indexes_by_configstring_7() {
        let Some(fs) = vcod_common::testing::game_fs() else {
            return;
        };
        let t = WeaponTable::load(&fs);
        let carbine = weapon_index("m1carbine_mp").unwrap();
        assert_eq!(t.get(carbine).unwrap().damage, 45);
        assert_eq!(t.class(carbine), "rifle");
        assert_eq!(t.class(0), "");

        // Different weapons with unrelated ammo names never share either index.
        let colt = weapon_index("colt_mp").unwrap();
        assert_ne!(
            t.get(carbine).unwrap().ammo_index,
            t.get(colt).unwrap().ammo_index
        );
        assert_ne!(
            t.get(carbine).unwrap().clip_index,
            t.get(colt).unwrap().clip_index
        );

        // bar_mp and bar_slow_mp share `ammoName BAR` (case differs from the
        // lowercased table key) in the shipped weapon files, so they share
        // an ammo index; every other weapon-def field still parses per file.
        let bar = t.get(weapon_index("bar_mp").unwrap()).unwrap();
        let bar_slow = t.get(weapon_index("bar_slow_mp").unwrap()).unwrap();
        assert_eq!(bar.ammo_index, bar_slow.ammo_index);

        // The index namespaces are per-name, not per-weapon: the distinct
        // count never exceeds the distinct lowercased name count.
        let names: std::collections::HashSet<_> = crate::configstrings::WEAPON_LIST
            .split(' ')
            .filter_map(|w| t.get(weapon_index(w).unwrap()))
            .map(|d| d.ammo_name.to_ascii_lowercase())
            .collect();
        let indexes: std::collections::HashSet<_> = crate::configstrings::WEAPON_LIST
            .split(' ')
            .filter_map(|w| t.get(weapon_index(w).unwrap()))
            .map(|d| d.ammo_index)
            .collect();
        assert_eq!(names.len(), indexes.len());
    }
}
