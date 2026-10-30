//! A 1.7 install's view weapons (docs/research/cod4-client-view.md 4):
//! every `common_mp` weapon with a gun and hands has both models skinned
//! and every view anim it names held by a zone and decoded.
//! Vacuous without `COD4_DIR`.

use vcod_cod4::anim::{clip, model};
use vcod_cod4::assets::Assets;
use vcod_cod4::testing::cod4_install;
use vcod_cod4::walk::{WalkOptions, walk};
use vcod_cod4::zone::Zone;

fn zone(name: &str) -> Option<Assets> {
    let install = cod4_install()?;
    let z = Zone::open_path(&install.zone("english", name)).unwrap();
    Some(walk(z, WalkOptions { keep_bulk: true }).unwrap().assets)
}

#[test]
fn common_mp_view_weapons_build() {
    let (Some(map), Some(common)) = (zone("mp_crash"), zone("common_mp")) else {
        return;
    };
    let zones = [&map, &common];
    let whole = |n: &str| {
        zones.iter().find_map(|a| {
            let x = a.xmodels.iter().find(|x| x.name == n)?;
            model(a, x, |id| a.materials[id.index()].name.clone())
                .unwrap()
                .map(|m| (m, x.num_bones))
        })
    };
    let (mut built, mut unheld, mut knives) = (0, Vec::new(), 0);
    for w in common.weapons.iter() {
        let name = |k: &str| {
            let id = match k {
                "gunModel" => w.model_lists.get(k)?.first().copied().flatten()?,
                _ => *w.models.get(k)?,
            };
            let x = &common.xmodels[id.index()];
            Some(x.name.trim_start_matches(',').to_string())
        };
        let (Some(gun), Some(hands)) = (name("gunModel"), name("handModel")) else {
            continue;
        };
        let (Some((g, _)), Some((h, _))) = (whole(&gun), whole(&hands)) else {
            // The placeholder's gun is a stub here.
            unheld.push(w.name.clone());
            continue;
        };
        assert!(
            h.bones.iter().any(|b| b.name == "tag_weapon"),
            "{}: {hands} has no tag_weapon",
            w.name
        );
        assert!(!g.lods.is_empty() && !h.lods.is_empty());
        // The knife a melee grafts at `tag_knife_attach`
        // (the knife test, cod4-client-view.md 4.2).
        if let Some(knife) = name("knifeModel") {
            assert!(whole(&knife).is_some(), "{}: knife {knife}", w.name);
            assert!(
                h.bones.iter().any(|b| b.name == "tag_knife_attach"),
                "{}: {hands} has no tag_knife_attach",
                w.name
            );
            knives += 1;
        }
        for (i, a) in w.xanims.iter().enumerate().skip(1) {
            let Some(a) = a.as_deref().filter(|a| !a.is_empty()) else {
                continue;
            };
            let found = zones.iter().find_map(|z| {
                Some((
                    *z,
                    z.xanims.iter().find(|x| x.name.eq_ignore_ascii_case(a))?,
                ))
            });
            let Some((z, x)) = found else {
                panic!("{}: view anim {i} {a} in no zone", w.name);
            };
            clip(z, x).unwrap_or_else(|e| panic!("{}: {a}: {e:#}", w.name));
        }
        if w.name == "m16_mp" {
            println!("m16_mp: gun {gun}, hands {hands}, anims {:?}", w.xanims);
        }
        built += 1;
    }
    println!("{built} view weapons ({knives} with a knife), models not held whole: {unheld:?}");
    assert!(knives > 40, "{knives}");
    assert!(built > 40, "{built}");
    assert_eq!(unheld, ["defaultweapon_mp"]);
}
