//! The stock MP maps' areas against their PVS: no cluster a leaf of area 1
//! or above holds is in a leaf of area 0, and no PVS row joins the two in
//! either direction, so `SV_AddEntitiesVisibleFromPoint`'s area test sends
//! no entity the PVS test would not (docs/protocol-1.1.md, "Which entities a
//! client is sent", "Areas").

use vcod_common::{bsp, testing};

#[test]
fn no_small_area_cluster_sees_or_is_seen_from_area_0() {
    let fs = testing::uo_fs().or_else(testing::stock_fs);
    let Some(fs) = fs else {
        return;
    };
    let mut maps: Vec<String> = fs
        .find_maps()
        .into_iter()
        .filter(|m| m.to_ascii_lowercase().starts_with("mp_"))
        .collect();
    maps.sort();
    maps.dedup();
    assert!(!maps.is_empty(), "no MP map in the install");
    for map in &maps {
        let entry = fs.resolve_map(map).unwrap();
        let b = bsp::parse(&fs.read(&entry).unwrap()).unwrap();
        let clusters = |small: bool| -> std::collections::BTreeSet<i32> {
            b.leafs
                .iter()
                .filter(|l| l.cluster >= 0 && (l.area > 0) == small && l.area >= 0)
                .map(|l| l.cluster)
                .collect()
        };
        let (zero, small) = (clusters(false), clusters(true));
        assert!(zero.is_disjoint(&small), "{map}: a cluster in two areas");
        let Some(pvs) = b.pvs.as_ref() else {
            continue;
        };
        for &a in &zero {
            for &s in &small {
                assert!(
                    !pvs.visible(a, s) && !pvs.visible(s, a),
                    "{map}: clusters {a} (area 0) and {s} see each other"
                );
            }
        }
    }
}
