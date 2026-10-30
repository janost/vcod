//! The game module's per-version tables a server picks with its protocol:
//! the weapon list configstring 7 carries, `bg_itemlist`'s layout and the
//! means of death. 1.1 and 1.5 share one set of tables; United Offensive has its own
//! (docs/research/coduo-diff.md 12, "Server").

use vcod_common::net::protocol::{Cod1Wire, PROTOCOL_V1, PROTOCOL_V6, PROTOCOL_V22};
use vcod_common::weapon_table::{STATIC_ITEMS, static_item};

pub struct Defs {
    pub proto: &'static Cod1Wire,
    /// Configstring 7: every `weapons/mp` file, lowercased and sorted, which
    /// numbers the weapons 1-based on the wire.
    pub weapons: &'static str,
    /// `modNames`, the enum the obituary and the damage callbacks name.
    pub mod_names: &'static [&'static str],
    /// The means of death the obituary sends as `0x80 | mod` instead of a
    /// weapon index.
    pub mod_flagged: &'static [i32],
}

/// 1.1's.
pub static V1: Defs = Defs {
    proto: &PROTOCOL_V1,
    weapons: crate::configstrings::WEAPON_LIST,
    mod_names: &crate::game::combat::MOD_NAMES,
    mod_flagged: &crate::game::combat::MOD_FLAGGED,
};

/// 1.5's: 1.1's tables under its own protocol, which is what the
/// builtins tell the two apart by.
pub static V15: Defs = Defs {
    proto: &PROTOCOL_V6,
    weapons: crate::configstrings::WEAPON_LIST,
    mod_names: &crate::game::combat::MOD_NAMES,
    mod_flagged: &crate::game::combat::MOD_FLAGGED,
};

/// United Offensive 1.51's: configstring 7 of the retail gamestate
/// (`crates/common/tests/fixtures/net/gamestate-uo.bin`), `modNames`
/// (game.mp.uo.i386.so 0xacf20) and the obituary's flag test (`obituary`,
/// 0x6a819).
pub static UO: Defs = Defs {
    proto: &PROTOCOL_V22,
    weapons: WEAPON_LIST_V22,
    mod_names: &MOD_NAMES_V22,
    mod_flagged: &[7, 8, 13, 15, 16, 17, 18, 20, 21, 26],
};

/// CoD2 1.3's: no fixed weapon list, so `weapons` is empty and a level's
/// list is the order it named each weapon (`Cod2HostState::weapons`,
/// `builtins::cod2::register_weapon`); `modNames` off cod2_lnxded 0x8187c40
/// (cod2-gsc-diff.md 4.1). Which means of death the obituary flags is
/// unread, so none is.
pub static COD2: Defs = Defs {
    proto: vcod_cod2::net::PROTOCOL_118.as_cod1_shim(),
    weapons: "",
    mod_names: &MOD_NAMES_COD2,
    mod_flagged: &[],
};

const MOD_NAMES_COD2: [&str; 15] = [
    "MOD_UNKNOWN",
    "MOD_PISTOL_BULLET",
    "MOD_RIFLE_BULLET",
    "MOD_GRENADE",
    "MOD_GRENADE_SPLASH",
    "MOD_PROJECTILE",
    "MOD_PROJECTILE_SPLASH",
    "MOD_MELEE",
    "MOD_HEAD_SHOT",
    "MOD_CRUSH",
    "MOD_TELEFRAG",
    "MOD_FALLING",
    "MOD_SUICIDE",
    "MOD_TRIGGER_HURT",
    "MOD_EXPLOSIVE",
];

const WEAPON_LIST_V22: &str = "30cal_tank_mp 50cal_tank_mp bar_mp bar_slow_mp bazooka_mp binoculars_artillery_mp binoculars_mp bren_mp colt_mp dp28_mp elefant_turret_mp enfield_mp fg42_mp fg42_semi_mp flak88_turret_mp flamethrower_mp flashgrenade_mp fraggrenade_mp gewehr43_mp kar98k_mp kar98k_sniper_mp luger_mp m1carbine_mp m1garand_mp mg30cal_mp mg34_mp mg34_tank_mp mg42_bipod_duck_mp mg42_bipod_prone_mp mg42_bipod_stand_mp mg42_tank_mp mg42_turret_mp mg50cal_tripod_stand_mp mg_sg43_stand_mp mk1britishfrag_mp mosin_nagant_mp mosin_nagant_sniper_mp mp40_mp mp44_mp mp44_semi_mp panzerfaust_mp panzeriv_turret_mp panzerschreck_mp ppsh_mp ppsh_semi_mp ptrs41_antitank_rifle_mp rgd-33russianfrag_mp satchelcharge_mp sg43_tank_mp sg43_turret_mp sherman_turret_mp smokegrenade_mp springfield_mp sten_mp sten_silenced_mp stielhandgranate_mp su152_turret_mp svt40_mp t34_turret_mp thompson_mp thompson_semi_mp tt33_mp webley_mp";

const MOD_NAMES_V22: [&str; 27] = [
    "MOD_UNKNOWN",
    "MOD_PISTOL_BULLET",
    "MOD_RIFLE_BULLET",
    "MOD_GRENADE",
    "MOD_GRENADE_SPLASH",
    "MOD_PROJECTILE",
    "MOD_PROJECTILE_SPLASH",
    "MOD_MELEE",
    "MOD_HEAD_SHOT",
    "MOD_MORTAR",
    "MOD_MORTAR_SPLASH",
    "MOD_DYNAMITE",
    "MOD_DYNAMITE_SPLASH",
    "MOD_ARTILLERY",
    "MOD_ARTILLERY_SPLASH",
    "MOD_WATER",
    "MOD_CRUSH",
    "MOD_CRUSH_TANK",
    "MOD_CRUSH_JEEP",
    "MOD_TELEFRAG",
    "MOD_FALLING",
    "MOD_SUICIDE",
    "MOD_TRIGGER_HURT",
    "MOD_EXPLOSIVE",
    "MOD_COLLISION",
    "MOD_FLAME",
    "MOD_MELEE_BINOCULARS",
];

impl Defs {
    /// The set a server speaking `p` runs.
    pub fn of(p: &Cod1Wire) -> &'static Defs {
        if p.transport.game == vcod_common::game::Game::Cod2 {
            return &COD2;
        }
        match p.transport.version {
            v if v == PROTOCOL_V22.transport.version => &UO,
            v if v == PROTOCOL_V6.transport.version => &V15,
            _ => &V1,
        }
    }

    /// A weapon's 1-based position in [`Defs::weapons`], which is what
    /// every weapon-shaped index on the wire and in the item table means.
    /// Case-sensitive, matching retail's `strcmp` (`BG_FindItem` 0x2e214).
    pub fn weapon_index(&self, name: &str) -> Option<usize> {
        self.weapons
            .split(' ')
            .position(|w| w == name)
            .map(|i| i + 1)
    }

    /// The weapon at a 1-based index.
    pub fn weapon_name(&self, index: usize) -> Option<&'static str> {
        self.weapons.split(' ').nth(index.checked_sub(1)?)
    }

    /// `bg_numItems`: row 0, the weapon rows, then the five compiled-in
    /// ammo and health rows.
    pub fn num_items(&self) -> usize {
        self.proto.max_weapons + 1 + STATIC_ITEMS.len()
    }

    /// The compiled-in rows at this version's indices
    /// (`weapon_table::static_item`): 65..69 on 1.1 and 1.5, after UO's 128
    /// weapon rows on UO.
    pub fn static_items(&self) -> impl Iterator<Item = (usize, &'static str, &'static str)> {
        (self.proto.max_weapons + 1..self.num_items())
            .filter_map(|i| static_item(self.proto, i).map(|(n, m)| (i, n, m)))
    }

    /// The enum value of a `MOD_*` name.
    pub fn mod_index(&self, name: &str) -> Option<i32> {
        self.mod_names
            .iter()
            .position(|n| *n == name)
            .map(|i| i as i32)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_uo_tables_match_retail() {
        assert_eq!(UO.weapons.split(' ').count(), 63);
        assert_eq!(UO.weapon_index("kar98k_mp"), Some(20));
        assert_eq!(UO.num_items(), 134);
        assert_eq!(V1.num_items(), 70);
        assert_eq!(
            UO.static_items().map(|(i, ..)| i).collect::<Vec<_>>(),
            [129, 130, 131, 132, 133]
        );
        assert_eq!(
            V15.static_items().collect::<Vec<_>>(),
            V1.static_items().collect::<Vec<_>>()
        );
        assert_eq!(
            V1.static_items().map(|(i, n, _)| (i, n)).last(),
            Some((69, "item_health_large"))
        );
        assert_eq!(UO.mod_index("MOD_FLAME"), Some(25));
        assert_eq!(V1.mod_index("MOD_SUICIDE"), Some(22));
        assert_eq!(UO.mod_index("MOD_SUICIDE"), Some(21));
    }

    #[test]
    fn each_protocol_gets_its_own_set() {
        for p in [&PROTOCOL_V1, &PROTOCOL_V6, &PROTOCOL_V22] {
            assert_eq!(Defs::of(p).proto.transport.version, p.transport.version);
        }
    }
}
