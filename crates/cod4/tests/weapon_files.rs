//! Every stock `weapons/mp/*` file of the install's iwds read through the
//! 1.7 loader's field table (`vcod_cod4::weapon_file`) against the
//! `common_mp` zone's compiled weapon of the same name: every number,
//! string, sound alias, model and material the struct carries must match, which
//! proves the table, the name tables and the post-load steps. A
//! millisecond field (type 7) is computed both ways off the file's
//! seconds: the zone must hold the compiler's product truncated unstored,
//! the loader 1.7's runtime one, stored as a float first, so a field reads
//! one over the zone's exactly where the arithmetic says
//! (cod4-fastfile.md, "Weapon files"); the inverse sight times follow
//! each side's own stored ms. Passes vacuously without `$COD4_DIR`.

use std::collections::BTreeMap;
use vcod_cod4::source::Cod4Source;
use vcod_cod4::testing::cod4_install;
use vcod_cod4::walk::walk;
use vcod_cod4::weapon_file::{self, FIELDS};
use vcod_cod4::zone::Zone;
use vcod_common::asset_source::AssetSource;

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
    // The loader's last step: `fOOPosAnimLength` off a sight time's ms,
    // the default bits without one (`weapon_file::finish`).
    let inverse = |ms: i32, fallback: u32| {
        if ms > 0 {
            ((1.0 / f64::from(ms)) as f32).to_bits()
        } else {
            fallback
        }
    };
    for w in &a.weapons {
        let Some(t) = weapon_file::load(&source, &w.name) else {
            continue;
        };
        let t = t.unwrap_or_else(|e| panic!("{}: {e:#}", w.name));
        checked += 1;
        let text = source.read(&format!("weapons/mp/{}", w.name)).unwrap();
        let body = vcod_common::text::latin1(text.strip_prefix(b"WEAPONFILE").unwrap());
        let words: Vec<&str> = body
            .strip_prefix('\\')
            .unwrap_or(&body)
            .split('\\')
            .collect();
        let value = |key: &str| {
            words
                .chunks(2)
                .find(|kv| kv[0].eq_ignore_ascii_case(key))
                .and_then(|kv| kv.get(1).copied())
                .filter(|v| !v.is_empty())
        };
        let int = |b: &[u8]| i32::from_le_bytes(b.try_into().unwrap());
        let bits = |b: &[u8]| u32::from_le_bytes(b.try_into().unwrap());
        for &(key, at, ty) in &numeric {
            let (z, f) = (&w.raw[at..at + 4], &t.weapon.raw[at..at + 4]);
            if ty == 7 {
                let secs = value(key).map(|v| vcod_cod4::menuexp::atof(v) as f32);
                let compiler = secs.map_or(0, vcod_common::weapon::stored_ms);
                let runtime = secs.map_or(0, weapon_file::stored_ms);
                if (int(z), int(f)) != (compiler, runtime) {
                    misses.entry(key.into()).or_default().push(format!(
                        "{} zone {} file {} want {compiler} {runtime}",
                        w.name,
                        int(z),
                        int(f)
                    ));
                }
                rounded_up += usize::from(runtime != compiler);
                continue;
            }
            // `adsTransInTime` and `adsTransOutTime` (1156, 1160).
            let sight = match at {
                2040 => Some((1156, 0x3b5a_740e)),
                2044 => Some((1160, 0x3b03_126f)),
                _ => None,
            };
            if let Some((ms_at, fallback)) = sight {
                let want = (
                    inverse(int(&w.raw[ms_at..ms_at + 4]), fallback),
                    inverse(int(&t.weapon.raw[ms_at..ms_at + 4]), fallback),
                );
                if (bits(z), bits(f)) != want {
                    misses.entry(key.into()).or_default().push(format!(
                        "{} zone {:#x} file {:#x}",
                        w.name,
                        bits(z),
                        bits(f)
                    ));
                }
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
        for (k, id) in &w.materials {
            let z = vcod_cod4::map::stub_name(&a.materials[id.index()].name);
            if !t
                .materials
                .get(k)
                .is_some_and(|f| f.eq_ignore_ascii_case(z))
            {
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
    eprintln!("{rounded_up} millisecond fields read one over the zone's");
    assert!(
        rounded_up > 0,
        "no millisecond field read one over the zone's"
    );
    assert!(misses.is_empty(), "{} fields differ", misses.len());
}
