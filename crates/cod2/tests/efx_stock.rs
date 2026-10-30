//! `vcod_cod2::efx` against a CoD2 1.3 install (`COD2_DIR`): every stock
//! `.efx` parses as the engine parses it, and the one the engine refuses
//! is refused for its reason (docs/research/cod2-efx.md 1.4). Without the
//! install it passes vacuously.

use std::collections::BTreeMap;
use vcod_cod2::efx::{self, Kind};

#[test]
fn stock_effects_parse() {
    let Some(fs) = vcod_common::testing::cod2_fs() else {
        return;
    };
    let names: Vec<String> = fs
        .names_with_suffix(".efx")
        .into_iter()
        .filter(|n| n.starts_with("fx/"))
        .collect();
    assert_eq!(names.len(), 375);
    let mut refused = Vec::new();
    let mut kinds: BTreeMap<&str, usize> = BTreeMap::new();
    for n in &names {
        let bytes = fs.read(n).unwrap();
        match efx::parse(&bytes) {
            Ok(e) => {
                assert!(!e.primitives.is_empty(), "{n}: no primitive");
                for p in &e.primitives {
                    *kinds.entry(p.kind.name()).or_default() += 1;
                    for c in &p.channels {
                        let n = c.curve.key_count();
                        assert!(n >= 2, "{n} keys");
                        assert_eq!(c.curve.keys[0], 0.0);
                        assert_eq!(c.curve.keys[(n - 1) * (c.curve.dims + 1)], 1.0);
                    }
                }
            }
            Err(e) => refused.push(format!("{n}: {e}")),
        }
    }
    println!("{kinds:?}");
    // `rgbComponentInterpolation` is not in `_fxSpawnFlags`.
    assert_eq!(refused.len(), 1, "{refused:#?}");
    assert!(refused[0].starts_with("fx/impacts/wall_impact_small_dirt.efx"));
    assert!(refused[0].contains("rgbComponentInterpolation"));
    assert_eq!(kinds["Particle"], 858);
    assert_eq!(kinds["Cloud"], 9);
    assert_eq!(kinds["CameraShake"], 1);
    assert!(kinds.keys().all(|k| Kind::from_name(k).is_some()));
}
