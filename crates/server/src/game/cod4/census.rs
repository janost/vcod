//! The builtin census: every `.gsc` the stock 1.7 zones carry (the base
//! zones' gametypes and common scripts, each stock map's own) compiles
//! against `cod4_lnxded`'s tables, and every builtin it calls has a
//! handler on vcod-server or a row in [`LEFT_OUT`] saying why not. Needs
//! `COD4_DIR`; without it the test passes.

use std::collections::BTreeSet;
use std::sync::Arc;

use super::*;
use crate::game::builtins::set::BuiltinSet;

/// Names the stock scripts call that vcod-server leaves without a handler,
/// each with why. A reached one fails its thread with `MissingBuiltin`.
/// Empty since the hardpoint vehicles and the physics calls landed
/// (cod4-gsc.md 3.2).
const LEFT_OUT: &[(&str, &str)] = &[];

/// The stock MP maps: every `mp_*.ff` but the `_load` screens.
fn stock_maps(install: &vcod_cod4::install::Install) -> Vec<String> {
    let mut maps: Vec<String> = install
        .zones("english")
        .unwrap()
        .iter()
        .filter_map(|p| p.file_stem()?.to_str().map(str::to_string))
        .filter(|n| n.starts_with("mp_") && !n.ends_with("_load"))
        .collect();
    maps.sort();
    maps
}

fn walk_zone(install: &vcod_cod4::install::Install, name: &str) -> Arc<vcod_cod4::assets::Assets> {
    let path = install.zone("english", name);
    let zone = vcod_cod4::zone::Zone::open_path(&path).unwrap();
    Arc::new(
        vcod_cod4::walk::walk(zone, Default::default())
            .unwrap()
            .assets,
    )
}

#[test]
fn cod4_builtin_census() {
    let Some(install) = vcod_cod4::testing::cod4_install() else {
        return;
    };
    let maps = stock_maps(&install);
    assert!(maps.len() >= 16, "{maps:?}");
    let set = BuiltinSet::cod4();
    let mut source = vcod_cod4::source::Cod4Source::open(&install).unwrap();
    for z in BASE_ZONES {
        source.add_zone(walk_zone(&install, z));
    }
    let mut missing: BTreeSet<String> = BTreeSet::new();
    let mut unknown: BTreeSet<String> = BTreeSet::new();
    let mut files = 0;
    for map in &maps {
        source.add_zone(walk_zone(&install, map));
        let src = Rc::new(source);
        let scripts: Vec<String> = src
            .list("", ".gsc")
            .into_iter()
            .map(|p| p.trim_end_matches(".gsc").to_string())
            .collect();
        files = files.max(scripts.len());
        let mut vm = vcod_gsc::Vm::new();
        let mut loader = vcod_gsc::Loader::with_dialect(
            Box::new(Cod4Scripts::new(src.clone(), None)),
            vcod_gsc::Dialect::Cod4,
        );
        loader.set_builtins(set.table());
        for e in &scripts {
            loader
                .load(&mut vm, e)
                .unwrap_or_else(|err| panic!("{map} {e}: {err:?}"));
        }
        drop(loader);
        unknown.extend(set.unknown_calls(&vm));
        missing.extend(set.unimplemented_calls(&vm));
        source = Rc::try_unwrap(src).ok().expect("one owner left");
        source.pop_zone();
    }
    assert!(unknown.is_empty(), "outside 1.7's tables: {unknown:?}");
    let allowed: BTreeSet<String> = LEFT_OUT.iter().map(|(n, _)| n.to_string()).collect();
    let unexplained: Vec<&String> = missing.difference(&allowed).collect();
    let stale: Vec<&String> = allowed.difference(&missing).collect();
    eprintln!(
        "census: {} maps, up to {files} scripts each, {} unanswered, {} left out",
        maps.len(),
        missing.len(),
        allowed.len()
    );
    assert!(
        unexplained.is_empty(),
        "{} builtins without a handler or a reason:\n{}",
        unexplained.len(),
        unexplained
            .iter()
            .map(|s| s.as_str())
            .collect::<Vec<_>>()
            .join("\n")
    );
    assert!(
        stale.is_empty(),
        "left out but answered or no longer called: {stale:?}"
    );
}
