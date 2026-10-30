//! Every stock CoD2 xmodel loads through `vcod_common::model::load` once
//! [`vcod_cod2::model::register`] has run. Needs `COD2_DIR`; passes
//! vacuously without it.
use std::collections::BTreeSet;
use vcod_common::model;
use vcod_common::testing::cod2_fs;

#[test]
fn every_stock_model_loads() {
    let Some(fs) = cod2_fs() else {
        return;
    };
    vcod_cod2::model::register();
    let mut failures = Vec::new();
    let mut loaded = 0;
    let mut locations = BTreeSet::new();
    for path in fs.list_prefix("xmodel/") {
        let Some(name) = path.strip_prefix("xmodel/").filter(|n| !n.is_empty()) else {
            continue;
        };
        let m = match model::load(&fs, name) {
            Ok(m) => m,
            Err(e) => {
                failures.push(format!("{name}: {e:#}"));
                continue;
            }
        };
        loaded += 1;
        assert!(!m.lods.is_empty(), "{name}");
        for lod in &m.lods {
            assert_eq!(
                lod.surfaces.len(),
                lod.materials.len(),
                "{name} {}",
                lod.name
            );
            for (i, s) in lod.surfaces.iter().enumerate() {
                assert_eq!(s.material, i, "{name} {}", lod.name);
                assert_eq!(s.indices.len() % 3, 0, "{name}");
                assert!(s.indices.iter().all(|&i| (i as usize) < s.verts.len()));
                for v in &s.verts {
                    assert!(v.pos.iter().chain(&v.normal).all(|c| c.is_finite()));
                    assert!(v.bone_indices.iter().all(|&b| (b as usize) < m.bones.len()));
                    let sum: f32 = v.bone_weights.iter().sum();
                    assert!((sum - 1.0).abs() < 1e-4, "{name}: weights sum {sum}");
                }
            }
        }
        for (i, b) in m.bones.iter().enumerate() {
            assert!(
                b.parent < i as i32,
                "{name} {}: parent {}",
                b.name,
                b.parent
            );
            assert!(b.pos.is_finite() && b.rot.is_finite(), "{name} {}", b.name);
            assert!(b.rot.is_normalized(), "{name} {}", b.name);
            assert!((b.hit_location as usize) < 19);
            locations.insert(b.hit_location);
        }
        for c in &m.collision {
            assert!(c.bone < m.bones.len());
            assert_eq!(c.contents & !0xdfff_fffb, 0, "{name}");
            assert!(c.tris.iter().flatten().all(|v| v.is_finite()));
        }
        let bones = model::load_bones(&fs, name).unwrap();
        assert_eq!(bones.len(), m.bones.len(), "{name}");
        assert!(model::bounds(&fs, name).is_some(), "{name}");
    }
    assert!(
        failures.is_empty(),
        "{} failures:\n{}",
        failures.len(),
        failures.join("\n")
    );
    assert_eq!(loaded, 1533, "stock descriptors");
    assert_eq!(locations, (0..19).collect(), "every hit location is used");
}

/// The body the format doc's hit-box census reads: every location but
/// `helmet` (the helmet model's) and `gun` (the weapon tags classify as
/// hands), boxes on the doc's bones.
#[test]
fn player_body_hit_rig() {
    let Some(fs) = cod2_fs() else {
        return;
    };
    vcod_cod2::model::register();
    let bones = model::load_bones(&fs, "playerbody_american_normandy01").unwrap();
    assert_eq!(bones.len(), 76);
    let locations: BTreeSet<u8> = bones.iter().map(|b| b.hit_location).collect();
    assert_eq!(locations, (0..18).filter(|&l| l != 1).collect());
    let bone = |n: &str| bones.iter().find(|b| b.name == n).unwrap();
    assert_eq!(bone("pelvis").hit_location, 5);
    assert_eq!(bone("j_knee_le").hit_location, 15);
    assert_eq!(bone("head").hit_location, 2);
    assert_eq!(bone("neck").hit_location, 3);
    assert_eq!(bone("tag_weapon_left").hit_location, 11);
    assert_eq!(bone("tag_weapon_right").hit_location, 10);
    assert!((bone("j_knee_le").hit_maxs.x - 16.14).abs() < 0.01);
    assert_eq!(bone("j_head").hit_maxs, glam::Vec3::ZERO);
}
