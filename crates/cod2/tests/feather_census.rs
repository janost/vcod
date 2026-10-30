//! Which stock materials draw with a depth feather (cod2-materials.md 9.5):
//! every material whose set names an emissive technique, by its technique
//! set and the technique it runs under retail's default `r_zFeather 1`
//! ("emissive feathered no fog"), and what `compile_all` makes of it.
//! Prints the census. Needs `COD2_DIR`; passes vacuously without it.

use std::collections::BTreeMap;
use vcod_cod2::material::{MaterialFile, TechniqueSet, TechniqueType as T};
use vcod_cod2::materials;
use vcod_common::material::SORT_DECAL;

#[test]
fn feathered_effects_census() {
    let Some(fs) = vcod_common::testing::cod2_fs() else {
        return;
    };
    let (lib, _) = materials::compile_all(&fs).unwrap();
    // set -> (feathered technique, plain emissive technique, materials,
    // featherParms.x values)
    type Row = (String, String, usize, BTreeMap<String, usize>);
    let mut census: BTreeMap<String, Row> = BTreeMap::new();
    let mut sets: BTreeMap<String, Option<TechniqueSet>> = BTreeMap::new();
    let (mut feathered, mut emissive_only) = (0, 0);
    for path in fs.list_prefix("materials/") {
        if path.matches('/').count() != 1 {
            continue;
        }
        let Some(Ok(file)) = fs.read(&path).map(|b| MaterialFile::parse(&b)) else {
            continue;
        };
        let set = sets.entry(file.technique_set.clone()).or_insert_with(|| {
            let src = fs.read(&format!(
                "materials/techniquesets/{}.techset",
                file.technique_set
            ));
            src.and_then(|s| TechniqueSet::parse(&String::from_utf8_lossy(&s)).ok())
        });
        // a set no pak holds draws the colour map only (compile_all's fallbacks)
        let Some(set) = set else {
            continue;
        };
        let plain = set.get(T::EmissiveNoFog);
        let feather = set.get(T::EmissiveFeatheredNoFog);
        if plain.is_none() && feather.is_none() {
            continue;
        }
        // every emissive set names the feathered type, so retail's
        // r_zFeather pass skips none of them (4.1's null check)
        assert!(feather.is_some(), "{} {}", file.name, file.technique_set);
        let feather = feather.unwrap();
        let m = lib.get(&file.name).unwrap();
        let fp = file.constant("featherParms").map(|c| c.literal[0]);
        if feather.starts_with("zfeather") {
            feathered += 1;
            assert!(
                fp.is_some_and(|x| x > 0.0),
                "{} has no featherParms",
                file.name
            );
            assert_eq!(m.feather, fp, "{}", file.name);
            // the fx pass reads the scene depth copy only past the decals
            assert!(m.sort > SORT_DECAL, "{} sorts {}", file.name, m.sort);
        } else {
            emissive_only += 1;
            assert_eq!(m.feather, None, "{}", file.name);
        }
        let row = census.entry(file.technique_set.clone()).or_insert_with(|| {
            (
                feather.to_string(),
                plain.unwrap_or("-").to_string(),
                0,
                BTreeMap::new(),
            )
        });
        row.2 += 1;
        *row.3
            .entry(fp.map_or("-".into(), |x| format!("{x}")))
            .or_default() += 1;
    }
    println!("set: feathered technique / plain emissive, materials, featherParms.x");
    for (set, (f, p, n, fps)) in &census {
        println!("  {set}: {f} / {p}, {n}, {fps:?}");
    }
    println!("{feathered} feathered, {emissive_only} emissive without a feather");
    let gfx_mist = lib.get("gfx_mist").unwrap();
    assert_eq!(gfx_mist.feather, Some(0.025));
    assert!(feathered > 0 && emissive_only > 0);
}
