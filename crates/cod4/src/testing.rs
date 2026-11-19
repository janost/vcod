//! Install lookups for tests. Each returns `None` without its environment
//! variable, so CI (no game data) passes, and panics when the variable is set
//! but names no install ([`vcod_common::testing::install_root`]).

use crate::install::Install;

/// The client install `$COD4_DIR` names (`zone/english/`, `main/`).
pub fn cod4_install() -> Option<Install> {
    Install::at(vcod_common::testing::install_root(
        crate::install::ENV,
        "cod4",
        "zone",
    )?)
}

/// The retail 1.7 dedicated server's install, `$COD4_LNXDED_DIR`: its own
/// `zone/english/` holds the subset of zones the server ships with.
pub fn cod4_lnxded_install() -> Option<Install> {
    Install::at(vcod_common::testing::install_root(
        "COD4_LNXDED_DIR",
        "cod4_lnxded",
        "zone",
    )?)
}

/// The stock MP map zones' names, sorted: every `zone/english/mp_*.ff` but
/// the `_load` screens. `COD4_MAPS=<a,b>` keeps those whose name holds one
/// of the parts; without it the 21 of 1.7 are asserted.
pub fn stock_maps(install: &Install) -> Vec<String> {
    let filter: Vec<String> = std::env::var("COD4_MAPS")
        .map(|v| v.split(',').map(str::to_owned).collect())
        .unwrap_or_default();
    let mut maps: Vec<String> = install
        .zones("english")
        .unwrap()
        .iter()
        .filter_map(|p| p.file_stem()?.to_str().map(str::to_owned))
        .filter(|n| n.starts_with("mp_") && !n.ends_with("_load"))
        .filter(|n| filter.is_empty() || filter.iter().any(|f| n.contains(f.as_str())))
        .collect();
    maps.sort();
    if filter.is_empty() {
        assert_eq!(maps.len(), 21, "{maps:?}");
    }
    maps
}
