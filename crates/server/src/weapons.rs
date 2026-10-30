//! The weapon half of a player's state: which weapons a client holds, which
//! one sits in each weapon slot, and which one it spawned holding. The stock
//! spawn builtins (`giveWeapon`, `setSpawnWeapon`) write it and
//! `ClientSim::to_wire` renders it into `ps.weapons`, `ps.weaponslots` and
//! `ps.weapon`.

use vcod_common::pk3::Pk3Fs;
use vcod_common::pmove::weapon;
use vcod_common::weapon::WeaponDef;

/// The retail `weaponSlot` name table. Object model doc, section 20.
pub use vcod_common::weapon::SLOT_NAMES;

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

    /// `BG_TakePlayerWeapon` (`game.mp.i386.so` 0x36b78): the weapon is
    /// dropped and every slot it stood in goes empty. One the player does not
    /// hold is a no-op, which is what makes taking an empty slot's `0` one.
    /// Retail's search for another held weapon of the same `weaponSlot` to
    /// move in behind it is not modelled: no stock loadout gives two of one
    /// slot.
    pub fn take(&mut self, index: usize) {
        if !self.holds(index) {
            return;
        }
        self.held &= !(1u64 << index);
        for s in self.slots.iter_mut().filter(|s| **s as usize == index) {
            *s = 0;
        }
    }

    /// `BG_GivePlayerWeapon` (`game.mp.i386.so` 0x36a38): [`Self::give`],
    /// and every mode on the weapon's `altWeapon` chain held beside it
    /// without a slot.
    pub fn give_chain(&mut self, table: &WeaponTable, index: usize, slot: usize) {
        if self.holds(index) {
            return;
        }
        self.give(index, slot);
        let mut alt = table.get(index).map_or(0, |d| d.alt_index);
        while alt != 0 && !self.holds(alt) {
            self.give(alt, 0);
            alt = table.get(alt).map_or(0, |d| d.alt_index);
        }
    }

    /// `BG_TakePlayerWeapon` (0x36b78) with the `altWeapon` chain: the slot
    /// the weapon or any of its modes stands in goes empty, and every mode is
    /// dropped.
    pub fn take_chain(&mut self, table: &WeaponTable, index: usize) {
        if !self.holds(index) {
            return;
        }
        let mut chain = vec![index];
        let mut alt = table.get(index).map_or(0, |d| d.alt_index);
        while alt != 0 && !chain.contains(&alt) {
            chain.push(alt);
            alt = table.get(alt).map_or(0, |d| d.alt_index);
        }
        for s in self
            .slots
            .iter_mut()
            .filter(|s| chain.contains(&(**s as usize)))
        {
            *s = 0;
        }
        for w in chain {
            if self.holds(w) {
                self.held &= !(1u64 << w);
            }
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

/// Every weapon file in configstring 7 ([`crate::defs::Defs::weapons`]),
/// parsed once at map load and indexed the way the wire is (`bg_numItems`
/// entries, 0 unused, 1.. the CS 7 order). The animscript reads
/// [`WeaponTable::class`] every frame, so parsing happens once here rather
/// than per-frame.
pub struct WeaponTable {
    defs: Vec<Option<WeaponDef>>,
    list: &'static crate::defs::Defs,
}

impl WeaponTable {
    /// No weapon at any index. For tests and hosts with no paks mounted.
    pub fn empty() -> WeaponTable {
        WeaponTable {
            defs: Vec::new(),
            list: &crate::defs::V1,
        }
    }

    /// Walks the weapon list in wire order, parsing each weapon file. A
    /// missing file logs and leaves that slot `None` rather than failing
    /// the map load.
    pub fn load(fs: &Pk3Fs, list: &'static crate::defs::Defs) -> WeaponTable {
        let mut defs: Vec<Option<WeaponDef>> = vec![None; list.num_items()];
        for (i, name) in list.weapons.split(' ').enumerate() {
            match vcod_common::weapon::load(fs, name) {
                Ok(def) => defs[i + 1] = Some(def),
                Err(e) => log::warn!("weapon table: {name}: {e:#}"),
            }
        }
        vcod_common::weapon_table::link_alts(&mut defs, list.weapons.split(' '));
        WeaponTable {
            list,
            ..WeaponTable::from_defs(defs)
        }
    }

    /// Indexes the parsed files through
    /// `vcod_common::weapon_table::assign_indices`, the walk the client's
    /// table runs too. A test hands it hand-built defs.
    pub fn from_defs(mut defs: Vec<Option<WeaponDef>>) -> WeaponTable {
        vcod_common::weapon_table::assign_indices(&mut defs);
        WeaponTable {
            defs,
            list: &crate::defs::V1,
        }
    }

    /// A CoD2 level's table: index 0 empty, then `defs` in the order the
    /// level named each weapon (`builtins::cod2::register_weapon`), indexed
    /// and alt-linked as [`WeaponTable::load`] does.
    pub fn cod2(names: &[String], defs: Vec<Option<WeaponDef>>) -> WeaponTable {
        let mut defs: Vec<Option<WeaponDef>> = std::iter::once(None).chain(defs).collect();
        vcod_common::weapon_table::link_alts(&mut defs, names.iter().map(String::as_str));
        let mut table = WeaponTable {
            list: &crate::defs::COD2,
            ..WeaponTable::from_defs(defs)
        };
        // CoD2's `BG_SetupAmmoIndexes` / `BG_SetupClipIndexes`: slot 0 is
        // the empty name, so a weapon with no clip shares it, and the
        // match is case-sensitive (`vcod_cod2::weapon::clip_indices`;
        // mp_carentan's axis clips at 12, 14 and 15 live,
        // cod2-protocol.md 15.6).
        let present: Vec<usize> = (0..table.defs.len())
            .filter(|&i| table.defs[i].is_some())
            .collect();
        let names_of = |f: fn(&WeaponDef) -> &str, defs: &[Option<WeaponDef>]| {
            vcod_cod2::weapon::clip_indices(
                present
                    .iter()
                    .map(|&i| f(defs[i].as_ref().unwrap()).to_string())
                    .collect::<Vec<_>>()
                    .iter()
                    .map(String::as_str)
                    .collect::<Vec<_>>(),
            )
        };
        let ammo = names_of(|d| &d.ammo_name, &table.defs);
        let clip = names_of(|d| &d.clip_name, &table.defs);
        for (k, &i) in present.iter().enumerate() {
            let d = table.defs[i].as_mut().unwrap();
            d.ammo_index = ammo[k];
            d.clip_index = clip[k];
        }
        table
    }

    /// The weapon list and item rows the table was loaded from.
    pub fn list(&self) -> &'static crate::defs::Defs {
        self.list
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

    /// `index` and every weapon its `altWeapon` chain reaches, in chain
    /// order: the walk `RegisterItem` (0x4e504) and `BG_GivePlayerWeapon`
    /// (0x36a38) make over `def+0x2fc`, stopping at 0 or back at the start.
    pub fn alt_chain(&self, index: usize) -> Vec<usize> {
        let mut chain = vec![index];
        let mut alt = self.get(index).map_or(0, |d| d.alt_index);
        while alt != 0 && !chain.contains(&alt) {
            chain.push(alt);
            alt = self.get(alt).map_or(0, |d| d.alt_index);
        }
        chain
    }

    /// The `SLOT_NAMES` index the weapon file's `weaponSlot` names, 0 for a
    /// weapon with none.
    pub fn slot(&self, index: usize) -> usize {
        self.get(index).map_or(0, WeaponDef::slot_type)
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

    /// The `weaponclass` condition in `mp/playeranim.script` is this field.
    /// Values read straight out of the shipped weapon files, through the
    /// table rather than the standalone lookup `PlayerWeapons` used to use.
    #[test]
    fn a_weapon_file_names_its_class() {
        let Some(fs) = vcod_common::testing::game_fs() else {
            return;
        };
        let t = WeaponTable::load(&fs, &crate::defs::V1);
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

    /// The index the first distinct name gets is 1, and slot 0 goes unused:
    /// the retail hit capture's spawn line puts the stock allies loadout at
    /// `clip=3:7,6:3,10:15 ammo=3:56,10:400`, one above where a 0-based walk
    /// lands them. The line is read out of the fixture rather than copied, so
    /// a retaken capture moves the expectation with it.
    #[test]
    fn the_first_index_handed_out_is_one() {
        let Some(fs) = vcod_common::testing::game_fs_11() else {
            return;
        };
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/playerstate/mp_carentan-tdm-hit-target.txt"
        );
        let text = std::fs::read_to_string(path).unwrap_or_else(|e| panic!("read {path}: {e}"));
        let spawn = text
            .lines()
            .find(|l| l.starts_with("!trace "))
            .unwrap_or_else(|| panic!("{path}: no !trace line"));
        // `clip=3:7,6:3,10:15`, the non-zero entries as index:value.
        let array = |key: &str| -> Vec<(usize, i32)> {
            spawn
                .split_whitespace()
                .find_map(|t| t.strip_prefix(key))
                .unwrap_or_else(|| panic!("{path}: the spawn line has no {key}"))
                .split(',')
                .map(|e| {
                    let (i, v) = e.split_once(':').expect("index:value");
                    (i.parse().unwrap(), v.parse().unwrap())
                })
                .collect()
        };
        let clip = array("clip=");
        let ammo = array("ammo=");

        let t = WeaponTable::load(&fs, &crate::defs::V1);
        let def = |name: &str| t.get(weapon_index(name).unwrap()).unwrap();
        assert_eq!(def("colt_mp").clip_index, 3);
        assert_eq!(def("fraggrenade_mp").clip_index, 6);
        assert_eq!(def("m1carbine_mp").clip_index, 10);
        assert_eq!(def("colt_mp").ammo_index, 3);
        assert_eq!(def("m1carbine_mp").ammo_index, 10);

        // The rounds at those indexes are the spawn loadout `giveMaxAmmo`
        // hands out: a full clip and `maxAmmo` in reserve.
        let at = |a: &[(usize, i32)], i: usize| a.iter().find(|(k, _)| *k == i).map(|(_, v)| *v);
        assert_eq!(at(&clip, 3), Some(def("colt_mp").clip_size as i32));
        assert_eq!(at(&clip, 6), Some(def("fraggrenade_mp").clip_size as i32));
        assert_eq!(at(&clip, 10), Some(def("m1carbine_mp").clip_size as i32));
        assert_eq!(at(&ammo, 3), Some(def("colt_mp").max_ammo as i32));
        assert_eq!(at(&ammo, 10), Some(def("m1carbine_mp").max_ammo as i32));
    }

    /// Both stock frags name the one `grenades` cap, so they share an index;
    /// a rifle has none. And a weapon's slot is its file's `weaponSlot`.
    #[test]
    fn the_shared_cap_and_the_slot_come_off_the_table() {
        let Some(fs) = vcod_common::testing::game_fs() else {
            return;
        };
        let t = WeaponTable::load(&fs, &crate::defs::V1);
        let get = |n: &str| t.get(weapon_index(n).unwrap()).unwrap();
        assert!(get("fraggrenade_mp").shared_cap_index.is_some());
        assert_eq!(
            get("fraggrenade_mp").shared_cap_index,
            get("stielhandgranate_mp").shared_cap_index
        );
        assert_eq!(get("m1carbine_mp").shared_cap_index, None);
        assert_eq!(t.slot(weapon_index("fg42_mp").unwrap()), 1);
        assert_eq!(t.slot(weapon_index("m1carbine_mp").unwrap()), 1);
        assert_eq!(t.slot(weapon_index("colt_mp").unwrap()), 3);
        assert_eq!(t.slot(weapon_index("fraggrenade_mp").unwrap()), 4);
        assert_eq!(t.slot(0), 0);
    }

    /// The ammo/clip index rule (docs/protocol-1.1.md, "How `ammo[]` and
    /// `ammoclip[]` are indexed"): walk `WEAPON_LIST` in order, lowercase
    /// each def's ammo/clip name, and hand out the next free slot on a miss.
    #[test]
    fn the_table_indexes_by_configstring_7() {
        let Some(fs) = vcod_common::testing::game_fs() else {
            return;
        };
        let t = WeaponTable::load(&fs, &crate::defs::V1);
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
