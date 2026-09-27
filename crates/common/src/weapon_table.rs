//! The weapon index table: every weapon file in configstring 7's order, with
//! the ammo, clip and shared-cap indexes the wire's arrays are read by
//! (docs/protocol-1.1.md, "How `ammo[]` and `ammoclip[]` are indexed"). The
//! server's `WeaponTable` and the client's predictor both index through
//! [`assign_indices`].

use crate::pk3::Pk3Fs;
use crate::weapon::WeaponDef;

/// Index 0 `None`, then one entry per space-separated configstring 7 name in
/// wire order, indexed by [`assign_indices`]. A file that fails to load
/// leaves its slot `None`.
pub fn from_configstring(fs: &Pk3Fs, cs7: &str) -> Vec<Option<WeaponDef>> {
    let mut defs: Vec<Option<WeaponDef>> = std::iter::once(None)
        .chain(cs7.split(' ').map(|name| {
            crate::weapon::load(fs, name)
                .map_err(|e| log::warn!("weapon table: {name}: {e:#}"))
                .ok()
        }))
        .collect();
    assign_indices(&mut defs);
    defs
}

/// Assigns the ammo, clip and shared-cap indexes in index order. Ammo and
/// clip names each get their own table, so the two indexes never share a
/// namespace.
pub fn assign_indices(defs: &mut [Option<WeaponDef>]) {
    let mut ammo_names: Vec<String> = Vec::new();
    let mut clip_names: Vec<String> = Vec::new();
    let mut cap_names: Vec<String> = Vec::new();
    for def in defs.iter_mut().flatten() {
        def.ammo_index = name_index(&mut ammo_names, &def.ammo_name);
        def.clip_index = name_index(&mut clip_names, &def.clip_name);
        def.shared_cap_index = (!def.shared_ammo_cap_name.is_empty())
            .then(|| name_index(&mut cap_names, &def.shared_ammo_cap_name));
    }
}

/// Lowercase, look up, append on a miss. The first name handed out is 1, not
/// 0: the retail hit capture's spawn line reads the colt's clip at 3 and the
/// carbine's at 10, one above where a 0-based walk puts them.
fn name_index(table: &mut Vec<String>, name: &str) -> usize {
    let lower = name.to_ascii_lowercase();
    match table.iter().position(|n| *n == lower) {
        Some(i) => i + 1,
        None => {
            table.push(lower);
            table.len()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        let Some(fs) = crate::testing::game_fs() else {
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
    }
}
