//! Every stock MP map's light grid (`vcod_cod4::lightgrid`): the rows'
//! run-length columns add up and cover every entry, and a lookup at each
//! static model's lighting origin finds grid corners (none falls back to
//! the missing colour), nor does an entity's lookup for a player standing
//! on each spawn point (cod4-world.md 3.3); `ProbeFinder` picks every
//! static model's stored reflection probe at its lighting origin, and a
//! probe in range for each spawn point (9.7). Prints a line per map.
//! cod4-world.md 3.1.

use vcod_cod4::collision::ClipMap;
use vcod_cod4::lightgrid::LightGrid;
use vcod_cod4::map::Map;
use vcod_cod4::testing::cod4_install;

#[test]
fn light_grid_rows_cover_the_entries_and_light_every_static_model() {
    let Some(install) = cod4_install() else {
        return;
    };
    let mut maps: Vec<String> = install
        .zones("english")
        .unwrap()
        .iter()
        .filter_map(|p| p.file_stem()?.to_str().map(str::to_owned))
        .filter(|n| n.starts_with("mp_") && !n.ends_with("_load"))
        .collect();
    maps.sort();
    assert_eq!(maps.len(), 21);
    let mut outside = std::collections::BTreeSet::new();
    for name in &maps {
        let (map, walked) = Map::load(&install, "english", name).unwrap();
        let g: &LightGrid = map.light_grid.as_ref().expect("the walk keeps the grid");
        let covered = g.check_rows().unwrap();
        assert_eq!(covered, g.entries.len(), "{name}: rows cover the entries");
        let clip = ClipMap::from_assets(&walked.assets).unwrap();
        let blocked = |a: [f32; 3], b: [f32; 3]| {
            clip.trace(
                a.into(),
                b.into(),
                glam::Vec3::ZERO,
                glam::Vec3::ZERO,
                vcod_cod4::lightgrid::SIGHT_MASK,
            )
            .fraction
                < 1.0
        };
        let probes = map.probe_finder();
        let (mut ground, mut missing, mut sunlit, mut dark) = (0, 0, 0, 0);
        let mut probe_differs = Vec::new();
        let (mut spot_models, mut spot_votes) = (0, 0);
        for sm in &map.static_models {
            if u32::from(sm.primary_light) > map.sun_light_index {
                spot_models += 1;
                let l = g.lighting_at(sm.lighting_origin, sm.primary_light, &blocked);
                if l.primary_light == sm.primary_light {
                    spot_votes += 1;
                }
            }
            if probes.at(sm.lighting_origin) != sm.reflection_probe {
                probe_differs.push((sm.lighting_origin, sm.reflection_probe));
            }
            if sm.ground_lighting != [0; 4] {
                ground += 1;
                continue;
            }
            let l = g.lighting_at(sm.lighting_origin, sm.primary_light, &blocked);
            if l.primary_light == 0 && l.primary_weight == 255 {
                missing += 1;
            }
            match l.primary_weight {
                255 => sunlit += 1,
                0 => dark += 1,
                _ => {}
            }
        }
        // a standing player's lighting origin, 32 over each spawn point
        let (mut spawns, mut spawns_missing, mut spawns_spot) = (0, 0, 0);
        let lights = vcod_cod4::primarylight::PrimaryLights::of(&map);
        for e in &map.entities {
            let get = |key: &str| e.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str());
            if !get("classname").is_some_and(|c| c.starts_with("mp_") && c.ends_with("_spawn")) {
                continue;
            }
            let Some(o) = get("origin").and_then(vcod_common::bsp::parse_vec3) else {
                continue;
            };
            let at = [o[0], o[1], o[2] + 32.0];
            // a standing player's box, 15 across and 70 up
            let spot = lights.for_box([o[0], o[1], o[2] + 35.0], [15.0, 15.0, 35.0]);
            assert!(spot == 0 || u32::from(spot) > map.sun_light_index);
            let l = g.lighting_at(at, spot, &blocked);
            if l.primary_light == spot && spot != 0 {
                spawns_spot += 1;
            }
            let probe = probes.at(at);
            assert!(
                usize::from(probe) < map.reflection_probes.len(),
                "{name}: spawn {o:?} probe {probe}"
            );
            spawns += 1;
            if l.primary_light == 0 && l.primary_weight == 255 {
                spawns_missing += 1;
                outside.insert((name.clone(), o.map(|v| v as i32)));
            }
        }
        println!(
            "{name}: {} entries, {} colour sets, regions {}, sun {}; {} static models: \
             {ground} ground lit, {sunlit} in full light, {dark} without, {missing} missing; \
             {spawns} spawns, {spawns_missing} missing; {} static model probes differ",
            g.entries.len(),
            g.colors.len(),
            g.has_light_regions,
            g.sun_primary_light,
            map.static_models.len(),
            probe_differs.len(),
        );
        // every spot and omni light names a light def with an image
        for l in map
            .primary_lights
            .iter()
            .skip(map.sun_light_index as usize + 1)
        {
            let def = l.def.as_deref().expect("a primary light's def");
            let d = walked.assets.light_defs.iter().find(|d| d.name == def);
            assert!(
                d.is_some_and(|d| d.image.is_some()),
                "{name}: light def {def}"
            );
        }
        eprintln!(
            "{name}: {} primary lights, {spot_models} static models on a spot or omni, {spot_votes} \
             lit by it, {spawns_spot} spawns lit by one",
            map.primary_lights.len()
        );
        assert_eq!(missing, 0, "{name}: a static model outside the grid");
        // the zone's probe is the one R_CalcReflectionProbeIndex picks at
        // the lighting origin, as an entity's is
        assert_eq!(probe_differs, [], "{name}: static model probes");
        assert!(spawns > 0, "{name}: no spawn point");
    }
    assert!(
        outside.is_empty(),
        "spawn points outside the grid: {outside:?}"
    );
}
