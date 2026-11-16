//! Every stock `weapons/mp/*` file of the install's iwds read through the
//! 1.7 loader's field table (`vcod_cod4::weapon_file`) against the
//! `common_mp` zone's compiled weapon of the same name: every number,
//! string, sound alias and model the struct carries must match, which
//! proves the table, the name tables and the post-load steps. A
//! millisecond field (type 7) may read one over the zone's: the runtime
//! stores the product as a float before it truncates, the compiler did not
//! (cod4-fastfile.md, "Weapon files"). Passes vacuously without
//! `$COD4_DIR`.

use std::collections::BTreeMap;
use vcod_cod4::source::Cod4Source;
use vcod_cod4::testing::cod4_install;
use vcod_cod4::walk::walk;
use vcod_cod4::weapon_file::{self, FIELDS};
use vcod_cod4::zone::Zone;

#[test]
fn stock_weapon_files_read_as_the_zone_compiled_them() {
    let Some(install) = cod4_install() else {
        return;
    };
    let path = install.zone("english", "common_mp");
    let zone = walk(Zone::open_path(&path).unwrap(), Default::default()).unwrap();
    let a = &zone.assets;
    let source = Cod4Source::open(&install).unwrap();
    // Numeric fields: everything the table writes but pointers.
    let numeric: Vec<(&str, usize, u8)> = FIELDS
        .iter()
        .filter(|&&(_, _, ty)| !matches!(ty, 0 | 8..=11 | 23 | 33 | 34))
        .map(|&(n, o, ty)| (n, usize::from(o), ty))
        .chain([
            ("fOOPosAnimLength0", 2040, 6),
            ("fOOPosAnimLength1", 2044, 6),
        ])
        .collect();
    let mut misses: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut checked = 0;
    let mut rounded_up = 0;
    for w in &a.weapons {
        let Some(t) = weapon_file::load(&source, &w.name) else {
            continue;
        };
        let t = t.unwrap_or_else(|e| panic!("{}: {e:#}", w.name));
        checked += 1;
        let int = |b: &[u8]| i32::from_le_bytes(b.try_into().unwrap());
        let up = |at: usize| int(&t.weapon.raw[at..at + 4]) == int(&w.raw[at..at + 4]) + 1;
        for &(key, at, ty) in &numeric {
            let (z, f) = (&w.raw[at..at + 4], &t.weapon.raw[at..at + 4]);
            if ty == 7 && up(at) {
                rounded_up += 1;
                continue;
            }
            // The inverse sight times off `adsTransInTime` and
            // `adsTransOutTime` (1156, 1160) follow them.
            if (at == 2040 && up(1156)) || (at == 2044 && up(1160)) {
                continue;
            }
            if z != f {
                misses.entry(key.into()).or_default().push(format!(
                    "{} zone {:?} file {:?}",
                    w.name,
                    i32::from_le_bytes(z.try_into().unwrap()),
                    i32::from_le_bytes(f.try_into().unwrap())
                ));
            }
        }
        for (k, v) in &w.strings {
            let f = t.weapon.strings.get(k).map_or("", String::as_str);
            if !v.eq_ignore_ascii_case(f) {
                misses
                    .entry((*k).into())
                    .or_default()
                    .push(format!("{} zone {v:?} file {f:?}", w.name));
            }
        }
        for (k, v) in &w.sounds {
            if t.weapon.sounds.get(k) != Some(v) {
                misses.entry((*k).into()).or_default().push(w.name.clone());
            }
        }
        for (k, id) in &w.models {
            let z = a.xmodels[id.index()].name.trim_start_matches(',');
            if !t.models.get(k).is_some_and(|f| f.eq_ignore_ascii_case(z)) {
                misses.entry((*k).into()).or_default().push(w.name.clone());
            }
        }
        for (k, list) in &w.model_lists {
            let z: Vec<Option<String>> = list
                .iter()
                .map(|m| {
                    m.map(|id| {
                        a.xmodels[id.index()]
                            .name
                            .trim_start_matches(',')
                            .to_ascii_lowercase()
                    })
                })
                .collect();
            let f: Vec<Option<String>> = t
                .model_lists
                .get(k)
                .cloned()
                .unwrap_or_else(|| vec![None; 16])
                .into_iter()
                .map(|m| m.map(|m| m.to_ascii_lowercase()))
                .collect();
            if z != f {
                misses.entry((*k).into()).or_default().push(w.name.clone());
            }
        }
        if w.xanims != t.weapon.xanims {
            misses
                .entry("xanims".into())
                .or_default()
                .push(w.name.clone());
        }
    }
    for (k, v) in &misses {
        eprintln!("{k}: {} weapons, e.g. {:?}", v.len(), &v[..v.len().min(3)]);
    }
    assert!(checked > 100, "{checked} weapon files checked");
    // `m16_mp`'s `fireTime` 0.065 among them: 65 off the file, 64 in the
    // zone, and 65 on the live server under a mod (gdb).
    assert!(
        rounded_up > 0,
        "no millisecond field read one over the zone's"
    );
    assert!(misses.is_empty(), "{} fields differ", misses.len());
}
