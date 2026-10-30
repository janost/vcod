//! The stock example mod, `Mods/ModWarfare` of a 1.7 install, loaded as
//! retail loads `fs_game mods/ModWarfare` (`docs/research/cod4-fastfile.md`
//! 13): `mod.ff` after the common zones, its iwd over `main/`'s. Lists what
//! the mod overrides (`--nocapture`) and checks each override resolves to
//! the mod's version. Passes vacuously without `$COD4_DIR` or without the
//! mod in it.

use std::collections::BTreeSet;
use std::sync::Arc;
use vcod_cod4::assets::Assets;
use vcod_cod4::install::{Install, ZoneOrigin, zone_flags};
use vcod_cod4::source::Cod4Source;
use vcod_cod4::testing::cod4_install;
use vcod_cod4::walk::walk;
use vcod_cod4::zone::Zone;
use vcod_common::asset_source::AssetSource;
use vcod_common::pk3::Pk3Fs;

const FS_GAME: &str = "mods/ModWarfare";

fn modwarfare() -> Option<Install> {
    let install = cod4_install()?.with_fs_game(Some(FS_GAME));
    install.mod_zone().is_some().then_some(install)
}

fn walked(install: &Install, name: &str) -> Arc<Assets> {
    let f = install
        .zone_file("english", name)
        .unwrap_or_else(|| panic!("{name}: no zone"));
    let z = Zone::open_path(&f.path).unwrap();
    Arc::new(walk(z, Default::default()).unwrap().assets)
}

/// Every named asset of the types a mod overrides, as `type name`.
fn names(a: &Assets) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    let mut add = |kind: &str, name: &str| {
        out.insert(format!("{kind} {}", name.to_ascii_lowercase()));
    };
    a.menus
        .iter()
        .filter_map(|m| m.window.name.as_deref())
        .for_each(|n| add("menu", n));
    a.menu_lists.iter().for_each(|m| add("menufile", &m.name));
    a.raw_files.iter().for_each(|r| add("rawfile", &r.name));
    a.localize.iter().for_each(|l| add("localize", &l.name));
    a.materials.iter().for_each(|m| add("material", &m.name));
    a.images.iter().for_each(|i| add("image", &i.name));
    a.weapons.iter().for_each(|w| add("weapon", &w.name));
    a.string_tables
        .iter()
        .for_each(|t| add("stringtable", &t.name));
    a.xmodels.iter().for_each(|x| add("xmodel", &x.name));
    a.fx.iter().for_each(|f| add("fx", &f.name));
    a.sounds.iter().for_each(|s| add("sound", &s.name));
    out
}

#[test]
fn modwarfare_overrides_resolve_to_the_mod() {
    let Some(install) = modwarfare() else {
        return;
    };
    let mod_file = install.zone_file("english", "mod").unwrap();
    assert_eq!(mod_file.origin, ZoneOrigin::Mod);
    let startup = install.startup_zones(true);
    assert_eq!(startup.last(), Some(&("mod", zone_flags::MOD)));
    // The stock install has no mod: the same list without it.
    let stock = cod4_install().unwrap();
    assert!(stock.startup_zones(true).iter().all(|(n, _)| *n != "mod"));

    let mut src = Cod4Source::open(&install).unwrap();
    let mut stock_names = BTreeSet::new();
    let mut mod_assets = None;
    for (name, flags) in &startup {
        let a = walked(&install, name);
        if *name == "mod" {
            mod_assets = Some(a.clone());
        } else {
            stock_names.extend(names(&a));
        }
        src.add_zone_flagged(a, *flags);
    }
    // A map zone loads after the mod with lower flags: the mod still wins.
    src.add_zone_flagged(walked(&install, "mp_crash"), zone_flags::MAP);
    let mod_assets = mod_assets.unwrap();
    let in_mod = names(&mod_assets);
    let overridden: Vec<&String> = in_mod.intersection(&stock_names).collect();
    eprintln!(
        "mod.ff: {} assets, {} override stock ones:",
        in_mod.len(),
        overridden.len()
    );
    for n in &overridden {
        eprintln!("  {n}");
    }
    for n in in_mod.difference(&stock_names) {
        eprintln!("  new {n}");
    }
    // `mod.csv`'s three menu files.
    for f in [
        "changeclass_mw",
        "changeclass_marines_mw",
        "changeclass_opfor_mw",
    ] {
        let path = format!("menufile ui_mp/scriptmenus/{f}.menu");
        assert!(in_mod.contains(&path), "{path} not in mod.ff");
    }
    // The highest-priority zone carrying each mod asset is mod.ff.
    for n in &in_mod {
        let top = src
            .zones()
            .find(|z| names(z).contains(n))
            .expect("asset in no zone");
        assert!(Arc::ptr_eq(top, &mod_assets), "{n} resolves past mod.ff");
    }

    // The iwd: every file in it reads back as the mod's bytes, the
    // gametype scripts over common_mp's rawfiles included.
    let game_dir = install.game_dir().unwrap();
    let mod_iwds = Pk3Fs::open_iwd(&game_dir, None).unwrap();
    let files: Vec<String> = mod_iwds
        .names_with_suffix("")
        .into_iter()
        .filter(|n| !n.ends_with('/'))
        .collect();
    let stock_src = {
        let mut s = Cod4Source::open(&stock).unwrap();
        s.add_zone(walked(&stock, "common_mp"));
        s
    };
    let mut replaced = 0;
    for f in &files {
        let ours = mod_iwds.read(f).unwrap();
        if stock_src.exists(f) {
            replaced += 1;
            eprintln!("  iwd overrides {f}");
            assert_ne!(stock_src.read(f).as_ref(), Some(&ours), "{f} same bytes");
        } else {
            eprintln!("  iwd adds {f}");
        }
        assert_eq!(src.read(f).as_ref(), Some(&ours), "{f} not the mod's");
    }
    assert!(
        files
            .iter()
            .any(|f| f == "maps/mp/gametypes/_globallogic.gsc")
    );
    assert!(replaced > 0, "the iwd overrides nothing");
}

/// A zone neither `zone/<language>/` nor `usermaps/` holds is `None`; the
/// stock zones resolve to `zone/<language>/`.
#[test]
fn zones_resolve_stock_first() {
    let Some(install) = cod4_install() else {
        return;
    };
    let f = install.zone_file("english", "mp_crash").unwrap();
    assert_eq!(f.origin, ZoneOrigin::Stock);
    assert!(install.zone_file("english", "mp_no_such_map").is_none());
    assert!(install.zone_file("english", "mod").is_none());
}

/// The HUD and the script menus under the mod: `mod.ff`'s menus join the
/// stock ones (`changeclass_mw`, the `ocd_*` popups) and its overriding
/// materials win, as retail loads `mod` over `ui_mp` and the map.
#[test]
fn modwarfare_menus_reach_the_hud() {
    let Some(install) = modwarfare() else {
        return;
    };
    let stock = vcod_cod4::hud::HudAssets::load(&cod4_install().unwrap(), "mp_crash").unwrap();
    let modded = vcod_cod4::hud::HudAssets::load(&install, "mp_crash").unwrap();
    let mod_menus: Vec<String> = walked(&install, "mod")
        .menus
        .iter()
        .filter_map(|m| m.window.name.as_deref())
        .map(str::to_ascii_lowercase)
        .collect();
    assert!(
        mod_menus.iter().any(|m| m == "changeclass_mw"),
        "{mod_menus:?}"
    );
    for m in &mod_menus {
        assert!(modded.script_menus.contains_key(m), "{m} missing");
        assert!(!stock.script_menus.contains_key(m), "{m} is stock");
    }
    assert!(modded.script_menus.len() > stock.script_menus.len());
}
