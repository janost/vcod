//! Which stock materials take part in the shadow cookies
//! (cod2-materials.md 9.8): a material's set naming `shadowcookie caster`
//! draws into its entity's cookie, one naming `shadowcookie receiver`
//! takes them on the world; `compile_all` carries both as
//! `Material::cookie`. Prints the census. Needs `COD2_DIR`; passes
//! vacuously without it.

use std::collections::BTreeMap;
use vcod_cod2::material::{MaterialFile, TechniqueSet, TechniqueType as T};
use vcod_cod2::materials;

#[test]
fn shadow_cookie_roles_census() {
    let Some(fs) = vcod_common::testing::cod2_fs() else {
        return;
    };
    let (lib, _) = materials::compile_all(&fs).unwrap();
    // set -> (caster, receiver, materials)
    let mut census: BTreeMap<String, (bool, bool, usize)> = BTreeMap::new();
    let mut sets: BTreeMap<String, Option<TechniqueSet>> = BTreeMap::new();
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
        let Some(set) = set else {
            continue;
        };
        let caster = set.get(T::ShadowcookieCaster).is_some();
        let receiver = set.get(T::ShadowcookieReceiver).is_some();
        let m = lib.get(&file.name).unwrap();
        assert_eq!(
            (m.cookie.caster, m.cookie.receiver),
            (caster, receiver),
            "{}",
            file.name
        );
        if caster || receiver {
            census
                .entry(file.technique_set.clone())
                .or_insert((caster, receiver, 0))
                .2 += 1;
        }
    }
    println!("set: caster, receiver, materials");
    for (set, (c, r, n)) in &census {
        println!("  {set}: {c} {r} {n}");
    }
    // the phong_replace family and ambient_replace cast and take them,
    // phong_add only takes them, every other set (the alpha-tested ones
    // included) neither
    assert!(
        census
            .keys()
            .all(|s| s.starts_with("phong_replace") || s == "ambient_replace" || s == "phong_add")
    );
    assert_eq!(
        census.get("phong_add").map(|r| (r.0, r.1)),
        Some((false, true))
    );
    assert_eq!(
        census.get("phong_replace").map(|r| (r.0, r.1)),
        Some((true, true))
    );
}
