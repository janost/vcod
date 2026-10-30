//! The fog and vision sets of every stock 1.7 MP map
//! (docs/research/cod4-world.md 11): each map's scripts but
//! mp_cargoship's and mp_killhouse's set an exp fog with a positive halfway distance, and the vision set the game type
//! names after the prematch (`vision/<map>.vision`), the prematch's
//! `mpintro` and the `default` a client starts from are in `common_mp`
//! with every film key (some leave a glow key out). Prints each map's fog and film
//! (`--nocapture`). Vacuous without `COD4_DIR`.

use vcod_cod4::look::{KEYS, map_fog, visions};
use vcod_cod4::testing::cod4_install;
use vcod_cod4::walk::{WalkOptions, walk};
use vcod_cod4::zone::Zone;

#[test]
fn every_stock_map_has_its_fog_and_vision() {
    let Some(install) = cod4_install() else {
        return;
    };
    let raw = |zone: &str| {
        let z = Zone::open_path(&install.zone("english", zone)).unwrap();
        walk(z, WalkOptions::default()).unwrap().assets.raw_files
    };
    let common = raw("common_mp");
    let mut maps: Vec<String> = install
        .zones("english")
        .unwrap()
        .iter()
        .filter_map(|p| p.file_stem()?.to_str().map(str::to_owned))
        .filter(|n| n.starts_with("mp_") && !n.ends_with("_load"))
        .collect();
    maps.sort();
    assert_eq!(maps.len(), 21, "{maps:?}");
    let mut names: Vec<&str> = maps.iter().map(String::as_str).collect();
    names.extend(["mpintro", "default"]);
    let mut own = Vec::new();
    for name in &names {
        // the maps after the first release carry theirs in their own zone
        let in_map;
        let file = match common.iter().find(|r| {
            r.name
                .eq_ignore_ascii_case(&format!("vision/{name}.vision"))
        }) {
            Some(f) => f,
            None => {
                own.push(*name);
                in_map = raw(name);
                &in_map
                    .iter()
                    .find(|r| {
                        r.name
                            .eq_ignore_ascii_case(&format!("vision/{name}.vision"))
                    })
                    .unwrap_or_else(|| panic!("no vision/{name}.vision"))
                    .clone()
            }
        };
        let text = String::from_utf8_lossy(&file.data).to_ascii_lowercase();
        let missing: Vec<&str> = KEYS
            .into_iter()
            .filter(|k| {
                let key = k.to_ascii_lowercase();
                !text
                    .lines()
                    .any(|l| l.split_whitespace().next() == Some(key.as_str()))
            })
            .collect();
        if !missing.is_empty() {
            println!("{name}: leaves out {missing:?}");
        }
        assert!(
            missing.iter().all(|k| k.starts_with("r_glow")),
            "{name}: {missing:?}"
        );
    }
    println!("in their own zone: {own:?}");
    let mut fogless = Vec::new();
    let mut glowing = Vec::new();
    for map in &maps {
        let zone = raw(map);
        let mut sets = visions(&common);
        sets.extend(visions(&zone));
        let v = &sets[map.as_str()];
        let film = format!(
            "film {} contrast {} brightness {} desaturation {}; glow {} cutoff {} \
             desaturation {} intensity {:?} radius {:?}",
            v.film.enable,
            v.film.contrast,
            v.film.brightness,
            v.film.desaturation,
            v.glow.enable,
            v.glow.bloom_cutoff,
            v.glow.bloom_desaturation,
            v.glow.bloom_intensity,
            v.glow.radius,
        );
        if v.glow.enable {
            glowing.push(map.as_str());
        }
        match map_fog(&zone, map) {
            Some(fog) => {
                assert!(fog.density > 0.0 && fog.start >= 0.0, "{map}: {fog:?}");
                println!(
                    "{map}: fog start {} halfway {:.0} colour {:?}; {film}",
                    fog.start,
                    std::f32::consts::LN_2 / fog.density,
                    fog.color,
                );
            }
            None => {
                fogless.push(map.as_str());
                println!("{map}: no fog; {film}");
            }
        }
    }
    // mp_cargoship's call is commented out, mp_killhouse has none
    assert_eq!(fogless, ["mp_cargoship", "mp_killhouse"]);
    // the sets that turn the glow on (cod4-world.md 11.4)
    assert_eq!(
        glowing,
        [
            "mp_bloc",
            "mp_bog",
            "mp_cargoship",
            "mp_citystreets",
            "mp_convoy",
            "mp_crash_snow",
            "mp_pipeline",
            "mp_vacant"
        ]
    );
}
