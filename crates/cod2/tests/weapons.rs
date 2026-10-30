//! Every stock CoD2 weapon file parses through `vcod_cod2::weapon` with no
//! key outside the loader's table and its known stale keys. Needs
//! `COD2_DIR`; passes vacuously without it.
use vcod_cod2::weapon::{self, node};
use vcod_common::testing::cod2_fs;

#[test]
fn every_stock_weapon_parses_with_no_unknown_key() {
    let Some(fs) = cod2_fs() else {
        return;
    };
    let mut parsed = 0;
    let mut stale = std::collections::BTreeSet::new();
    for path in fs.list_prefix("weapons/mp/") {
        let Some(name) = path.strip_prefix("weapons/mp/").filter(|n| !n.is_empty()) else {
            continue;
        };
        let p = weapon::load(&fs, name).unwrap_or_else(|e| panic!("{name}: {e:#}"));
        assert!(p.unknown.is_empty(), "{name}: unknown keys {:?}", p.unknown);
        stale.extend(p.stale);
        let d = &p.def;
        assert!(
            !d.str("idleAnim").is_empty() || d.str("gunModel").is_empty(),
            "{name}"
        );
        parsed += 1;
    }
    assert_eq!(parsed, 42, "the stock weapons/mp files");
    assert_eq!(stale.len(), weapon::STALE_KEYS.len(), "{stale:?}");
}

#[test]
fn the_kar98k_reads_as_retail_stores_it() {
    let Some(fs) = cod2_fs() else {
        return;
    };
    let d = weapon::load(&fs, "kar98k_mp").unwrap().def;
    assert_eq!(weapon::WEAPON_TYPES[d.weap_type() as usize], "bullet");
    assert_eq!(weapon::WEAPON_CLASSES[d.weap_class() as usize], "rifle");
    assert_eq!(d.str("gunModel"), "viewmodel_kar98k");
    assert!(d.flag("aimDownSight"));
    assert!(d.int("fireTime") > 0 && d.int("rechamberTime") > 0);
    assert!(d.float("locHead") > 1.0, "the head multiplies the damage");
    assert_ne!(d.view_anim(node::RECHAMBER), d.view_anim(node::IDLE));
}
