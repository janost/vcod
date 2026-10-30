//! Every stock CoD2 xmodel, xmodelparts and xmodelsurfs parses to exact EOF
//! and holds the census facts of `docs/research/cod2-xmodel-v20-format.md`.
//! Needs `COD2_DIR`; passes vacuously without it.
use std::collections::HashMap;
use vcod_cod2::xmodel::{self, Parts, Surfs};
use vcod_common::testing::cod2_fs;

fn files(fs: &vcod_common::pk3::Pk3Fs, dir: &str) -> Vec<String> {
    fs.list_prefix(&format!("{dir}/"))
        .into_iter()
        .filter(|n| !n.ends_with('/'))
        .collect()
}

#[test]
fn every_stock_model_parses() {
    let Some(fs) = cod2_fs() else {
        return;
    };
    let mut failures = Vec::new();
    let mut parts: HashMap<String, Parts> = HashMap::new();
    for path in files(&fs, "xmodelparts") {
        match xmodel::parse_parts(&fs.read(&path).unwrap()) {
            Ok(p) => {
                parts.insert(path["xmodelparts/".len()..].to_owned(), p);
            }
            Err(e) => failures.push(format!("{path}: {e:#}")),
        }
    }
    let mut surfs: HashMap<String, Surfs> = HashMap::new();
    for path in files(&fs, "xmodelsurfs") {
        match xmodel::parse_surfs(&fs.read(&path).unwrap()) {
            Ok(s) => {
                surfs.insert(path["xmodelsurfs/".len()..].to_owned(), s);
            }
            Err(e) => failures.push(format!("{path}: {e:#}")),
        }
    }

    let descriptors = files(&fs, "xmodel");
    let mut desc_ok = 0;
    let mut weighted = 0usize;
    for path in &descriptors {
        let parsed = xmodel::parse_descriptor(&fs.read(path).unwrap(), |lod0| {
            parts
                .get(&lod0.to_lowercase())
                .map(|p| p.bones.len())
                .ok_or_else(|| anyhow::anyhow!("no xmodelparts/{lod0}"))
        });
        let d = match parsed {
            Ok(d) => d,
            Err(e) => {
                failures.push(format!("{path}: {e:#}"));
                continue;
            }
        };
        desc_ok += 1;
        let num_bones = d.bone_infos.len();
        assert_eq!(
            d.present_lods().count(),
            d.lods.iter().filter(|l| !l.name.is_empty()).count()
        );
        for lod in d.present_lods() {
            let Some(s) = surfs.get(&lod.name.to_lowercase()) else {
                failures.push(format!("{path}: no xmodelsurfs/{}", lod.name));
                continue;
            };
            assert_eq!(s.surfaces.len(), lod.materials.len(), "{path} {}", lod.name);
            if let Some(max) = s.max_bone() {
                assert!(
                    (max as usize) < num_bones,
                    "{path} {}: bone {max}",
                    lod.name
                );
            }
            for v in s.surfaces.iter().flat_map(|s| &s.vertices) {
                assert_eq!(v.weight_sum_65536(), 65536, "{path} {}", lod.name);
                weighted += !v.extras.is_empty() as usize;
            }
        }
    }
    for p in parts.values() {
        let pose = xmodel::base_pose(p);
        assert!(
            pose.iter()
                .all(|m| m.trans.is_finite() && m.trans_weight.is_finite())
        );
    }

    assert!(
        failures.is_empty(),
        "{} failures:\n{}",
        failures.len(),
        failures.join("\n")
    );
    assert_eq!(
        (desc_ok, parts.len(), surfs.len()),
        (1533, 1197, 2654),
        "stock census counts"
    );
    assert!(weighted > 0);
}
