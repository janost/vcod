//! The weapon index table: every weapon file in configstring 7's order, with
//! the ammo, clip and shared-cap indexes the wire's arrays are read by
//! (docs/protocol-1.1.md, "How `ammo[]` and `ammoclip[]` are indexed"). The
//! server's `WeaponTable` and the client's predictor both index through
//! [`assign_indices`].

use crate::asset_source::AssetSource;
use crate::pmove::weapon::NUM_AMMO;
use crate::weapon::WeaponDef;

/// The only classnames `bg_itemlist` (`game.mp.i386.so` `.data` 0x7b9d8)
/// carries compiled into the binary, indices 65-69, each with the world model at its record's +0x8. Indices
/// 1-64 are `emptyitem_"wNN"` placeholders whose model fields are the empty
/// string; a weapon's real classname and models reach its slot at runtime
/// from the mounted paks' weapon files, so a weapon's index comes from
/// configstring 7 and its models from the weapon file
/// (`crates/server/src/items.rs`). All five rows carry a null +0xc.
pub const STATIC_ITEMS: &[(usize, &str, &str)] = &[
    (
        65,
        "item_ammo_stielhandgranate_open",
        "xmodel/ammo_stielhandgranate1",
    ),
    (
        66,
        "item_ammo_stielhandgranate_closed",
        "xmodel/ammo_stielhandgranate2",
    ),
    (67, "item_health_small", "xmodel/health_small"),
    (68, "item_health", "xmodel/health_medium"),
    (69, "item_health_large", "xmodel/health_large"),
];

/// The compiled-in row at item `index` on protocol `p`: `STATIC_ITEMS`
/// follow the protocol's `max_weapons` weapon rows, 65..69 on 1.1 and 1.5,
/// 129..133 on United Offensive (coduo-diff.md 12). `(classname, model)`.
pub fn static_item(
    p: &crate::net::protocol::Cod1Wire,
    index: usize,
) -> Option<(&'static str, &'static str)> {
    let row = index.checked_sub(p.max_weapons + 1)?;
    STATIC_ITEMS.get(row).map(|&(_, name, model)| (name, model))
}

/// Index 0 `None`, then one entry per space-separated configstring 7 name in
/// wire order, indexed by [`assign_indices`]. A file that fails to load
/// leaves its slot `None`.
pub fn from_configstring(fs: &dyn AssetSource, cs7: &str) -> Vec<Option<WeaponDef>> {
    let mut defs: Vec<Option<WeaponDef>> = std::iter::once(None)
        .chain(cs7.split(' ').map(|name| {
            crate::weapon::load(fs, name)
                .map_err(|e| log::warn!("weapon table: {name}: {e:#}"))
                .ok()
        }))
        .collect();
    assign_indices(&mut defs);
    link_alts(&mut defs, cs7.split(' '));
    defs
}

/// Fills each def's `alt_index` from its `altWeapon` name against `names`,
/// configstring 7's order (index = position + 1).
pub fn link_alts<'a>(defs: &mut [Option<WeaponDef>], names: impl Iterator<Item = &'a str>) {
    let names: Vec<String> = names.map(str::to_ascii_lowercase).collect();
    for def in defs.iter_mut().flatten() {
        let alt = def.alt_weapon.to_ascii_lowercase();
        def.alt_index = names.iter().position(|n| *n == alt).map_or(0, |i| i + 1);
    }
}

/// The modes along `start`'s `altWeapon` chain (`alt_of` gives a weapon's
/// `alt_index`), `start` excluded. The walk ends at an index 0, back at
/// `start`, or at the first index seen twice, so a loop that skips `start`
/// (a hostile configstring 7 naming a weapon twice) ends too.
pub fn alt_chain(start: u8, alt_of: impl Fn(u8) -> u8) -> impl Iterator<Item = u8> {
    let mut seen = [0u64; 4];
    let mut first_visit = move |w: u8| {
        let (word, bit) = (usize::from(w >> 6), w & 63);
        let new = seen[word] >> bit & 1 == 0;
        seen[word] |= 1 << bit;
        new
    };
    first_visit(start);
    let mut at = start;
    std::iter::from_fn(move || {
        at = alt_of(at);
        (at != 0 && first_visit(at)).then_some(at)
    })
    .fuse()
}

/// Assigns the ammo, clip and shared-cap indexes in index order. Ammo and
/// clip names each get their own table, so the two indexes never share a
/// namespace. A def whose ammo or clip would index past the playerstate's
/// `NUM_AMMO` slots, which no wire can carry, is dropped to `None`.
pub fn assign_indices(defs: &mut [Option<WeaponDef>]) {
    let mut ammo_names: Vec<String> = Vec::new();
    let mut clip_names: Vec<String> = Vec::new();
    let mut cap_names: Vec<String> = Vec::new();
    for slot in defs.iter_mut() {
        let Some(def) = slot else { continue };
        let ammo = def.ammo_name.to_ascii_lowercase();
        let clip = def.clip_name.to_ascii_lowercase();
        if name_index(&ammo_names, &ammo) >= NUM_AMMO || name_index(&clip_names, &clip) >= NUM_AMMO
        {
            log::warn!(
                "weapon table: {:?}: past {} ammo or clip names",
                def.display_name,
                NUM_AMMO - 1
            );
            *slot = None;
            continue;
        }
        def.ammo_index = intern(&mut ammo_names, ammo);
        def.clip_index = intern(&mut clip_names, clip);
        def.shared_cap_index = (!def.shared_ammo_cap_name.is_empty()).then(|| {
            intern(
                &mut cap_names,
                def.shared_ammo_cap_name.to_ascii_lowercase(),
            )
        });
    }
}

/// A lowercased name's index, or the one it would take. The first name
/// handed out is 1, not 0: the retail hit capture's spawn line reads the
/// colt's clip at 3 and the carbine's at 10, one above where a 0-based walk
/// puts them.
fn name_index(table: &[String], lower: &str) -> usize {
    table.iter().position(|n| n == lower).unwrap_or(table.len()) + 1
}

/// [`name_index`], appending the name on a miss.
fn intern(table: &mut Vec<String>, lower: String) -> usize {
    let i = name_index(table, &lower);
    if i > table.len() {
        table.push(lower);
    }
    i
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn static_items_follow_the_protocols_weapon_rows() {
        use crate::net::protocol::{PROTOCOL_V1, PROTOCOL_V22};
        let small = Some(("item_health_small", "xmodel/health_small"));
        assert_eq!(static_item(&PROTOCOL_V1, 67), small);
        assert_eq!(static_item(&PROTOCOL_V22, 131), small);
        // UO's 65..69 are weapon rows.
        assert_eq!(static_item(&PROTOCOL_V22, 67), None);
        assert_eq!(static_item(&PROTOCOL_V1, 64), None);
        assert_eq!(static_item(&PROTOCOL_V1, 70), None);
        assert_eq!(
            static_item(&PROTOCOL_V22, 133).unwrap().0,
            "item_health_large"
        );
    }

    #[test]
    fn indices_start_at_one_and_follow_first_use() {
        let mk = |ammo: &str, clip: &str| {
            Some(WeaponDef {
                ammo_name: ammo.into(),
                clip_name: clip.into(),
                ..Default::default()
            })
        };
        let mut defs = vec![None, mk("a", "x"), mk("b", "y"), mk("A", "z")];
        assign_indices(&mut defs);
        let idx = |i: usize| {
            defs[i]
                .as_ref()
                .map(|d| (d.ammo_index, d.clip_index))
                .unwrap()
        };
        assert_eq!(idx(1), (1, 1));
        assert_eq!(idx(2), (2, 2));
        assert_eq!(idx(3), (1, 3));
    }

    #[test]
    fn stock_list_matches_the_retail_spawn_line() {
        let Some(fs) = crate::testing::game_fs_11() else {
            return;
        };
        // configstring 7 as retail sends it on mp_carentan dm
        let cs7 = include_str!("../../server/tests/fixtures/configstrings/mp_carentan-dm.txt")
            .lines()
            .find_map(|l| l.strip_prefix("7 "))
            .unwrap();
        let defs = from_configstring(&fs, cs7);
        let names: Vec<&str> = cs7.split(' ').collect();
        let clip = |name: &str| {
            let i = names.iter().position(|n| *n == name).unwrap() + 1;
            defs[i].as_ref().unwrap().clip_index
        };
        assert_eq!(clip("colt_mp"), 3);
        assert_eq!(clip("fraggrenade_mp"), 6);
        assert_eq!(clip("m1carbine_mp"), 10);
        // The select-fire pairs link both ways; a weapon with none reads 0.
        let alt = |name: &str| {
            let i = names.iter().position(|n| *n == name).unwrap() + 1;
            defs[i].as_ref().unwrap().alt_index
        };
        let bar = names.iter().position(|n| *n == "bar_mp").unwrap() + 1;
        assert_eq!(alt("bar_slow_mp"), bar);
        assert_eq!(alt("bar_mp"), bar + 1);
        assert_eq!(alt("m1carbine_mp"), 0);
    }

    /// A stock pair walks to its mode and stops back at the start; a loop
    /// that skips the start (`A B B` in configstring 7 gives 1->2, 2->1,
    /// 3->1) and a one-way link both end.
    #[test]
    fn the_alt_chain_ends_on_any_table() {
        let walk = |alt: &[u8], from: u8| -> Vec<u8> {
            alt_chain(from, |w| alt.get(usize::from(w)).copied().unwrap_or(0)).collect()
        };
        assert_eq!(walk(&[0, 2, 1], 1), [2]);
        assert_eq!(walk(&[0, 2, 1, 1], 3), [1, 2]);
        assert_eq!(walk(&[0, 2, 0], 1), [2]);
        assert_eq!(walk(&[0, 0, 0, 0, 5, 6, 5], 4), [5, 6]);
    }
}
