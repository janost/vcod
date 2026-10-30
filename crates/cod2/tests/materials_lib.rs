//! `vcod_cod2::materials` against a CoD2 1.3 install (`COD2_DIR`): every
//! stock material compiles, and every colour map mp_toujane's world draws
//! resolves through the renderer's image path to a decodable iwi. Without
//! the install it passes vacuously.

use std::collections::BTreeSet;
use vcod_cod2::d3dbsp::D3dBsp;
use vcod_cod2::materials;
use vcod_common::assets;
use vcod_common::material::{ImageRef, Technique, TexSemantic};

#[test]
fn stock_materials_compile() {
    let Some(fs) = vcod_common::testing::cod2_fs() else {
        return;
    };
    let (lib, report) = materials::compile_all(&fs).unwrap();
    assert!(report.errors.is_empty(), "{:?}", report.errors);
    assert_eq!(report.compiled, 3765);
    assert_eq!(lib.len(), 3765);
    let fallbacks: usize = report.fallbacks.values().sum();
    println!(
        "fallbacks {fallbacks} over {:?}; {} warnings",
        report.fallbacks,
        report.warnings.len()
    );
    for w in &report.warnings {
        println!("  {w}");
    }
    assert_eq!(lib.warn_count(), report.warnings.len());

    let d = fs.read("maps/mp/mp_toujane.d3dbsp").unwrap();
    let bsp = D3dBsp::parse(&d).unwrap();
    let used: BTreeSet<usize> = bsp.surfaces.iter().map(|s| s.material as usize).collect();
    let (mut drawn, mut images) = (0, BTreeSet::new());
    for i in used {
        let name = &bsp.materials[i].name;
        let m = lib
            .get(name)
            .unwrap_or_else(|| panic!("{name} not compiled"));
        if m.technique != Technique::Stages {
            continue;
        }
        drawn += 1;
        if let Some(ImageRef::Path(p)) = m.texture(TexSemantic::Color) {
            images.insert(p.clone());
        }
    }
    assert!(drawn > 0 && !images.is_empty());
    let mut wavelet = 0;
    for p in &images {
        let full = assets::resolve_bundle_image(&fs, p).unwrap_or_else(|| panic!("{p} missing"));
        let raw = fs.read(&full).unwrap();
        match vcod_common::iwi::decode(&raw) {
            Ok(img) => {
                // the renderer's own path decodes the same image
                let drawn = assets::load_path_image(&fs, &full);
                assert_eq!((drawn.width, drawn.height), (img.width, img.height), "{p}");
            }
            Err(e) if e.to_string().contains("wavelet") => wavelet += 1,
            Err(e) => panic!("{p}: {e:#}"),
        }
    }
    println!(
        "mp_toujane: {drawn} drawn materials, {} colour maps, {wavelet} wavelet",
        images.len()
    );
    assert_eq!(wavelet, 0);
}

/// The scene fog (cod2-materials.md 4.1): every stock material's linear
/// and exp fog techniques agree on whether it fogs, and mp_carentan's
/// world fogs under its `setExpFog` save its sky, triangle for triangle
/// the bulk of what it draws.
#[test]
fn carentan_world_fogs_under_the_fogged_techniques() {
    let Some(fs) = vcod_common::testing::cod2_fs() else {
        return;
    };
    let (lib, report) = materials::compile_all(&fs).unwrap();
    let differ: Vec<_> = report
        .warnings
        .iter()
        .filter(|w| w.contains("linear fog"))
        .collect();
    assert!(differ.is_empty(), "{differ:?}");
    let d = fs.read("maps/mp/mp_carentan.d3dbsp").unwrap();
    let bsp = D3dBsp::parse(&d).unwrap();
    let (mut fogged, mut clear, mut sky) = (0usize, 0usize, 0usize);
    let mut clear_names = BTreeSet::new();
    for s in &bsp.surfaces {
        let name = &bsp.materials[s.material as usize].name;
        let m = lib.get(name).unwrap();
        let tris = s.index_count as usize / 3;
        match m.technique {
            Technique::Sky => {
                assert!(m.nofog, "{name}");
                sky += tris;
            }
            Technique::Stages if m.nofog => {
                clear += tris;
                clear_names.insert(name.clone());
            }
            Technique::Stages => fogged += tris,
            _ => {}
        }
    }
    println!("mp_carentan: {fogged} fogged, {clear} unfogged, {sky} sky triangles");
    println!("  unfogged: {clear_names:?}");
    assert!(sky > 0);
    assert!(fogged > 20 * clear, "{fogged} vs {clear}");
}

/// The model techniques (cod2-materials.md 9.4, 9.6, 9.7): every stock
/// material whose set names an `lprobe` technique other than
/// `objective_base` takes its switches, its static model technique
/// (`lprobe_smc*`) the same less `_scroll` and `_flag`, the `objective`
/// set's materials (the S&D bomb's `mtl_tntbomb_obj`) draw added, unfogged
/// and without depth and have no static model technique, and the census of
/// switches covers the specular, alpha, detail, scroll and flag forms.
#[test]
fn model_materials_take_their_probe_technique() {
    use std::collections::BTreeMap;
    use vcod_common::material::{BlendFactor, ModelLight};
    let Some(fs) = vcod_common::testing::cod2_fs() else {
        return;
    };
    let (lib, _) = materials::compile_all(&fs).unwrap();
    let mut census: BTreeMap<String, usize> = BTreeMap::new();
    let mut objectives = 0;
    for path in fs.list_prefix("materials/") {
        let name = path.trim_start_matches("materials/");
        if name.contains('/') {
            continue;
        }
        let Some(m) = lib.get(name) else { continue };
        match m.model_light {
            ModelLight::Probe => {
                let key = m.probe.map_or("none".to_string(), |p| {
                    format!(
                        "s {} a {} dtl {} scroll {} flag {}",
                        p.specular, p.alpha, p.detail, p.scroll, p.flag
                    )
                });
                // the 6 water sets and `$default` name techniques of
                // neither family
                if let Some(p) = m.probe {
                    let sm = m.smodel.unwrap_or_else(|| panic!("{name}"));
                    assert_eq!(
                        (sm.specular, sm.alpha, sm.detail, sm.scroll, sm.flag),
                        (p.specular, p.alpha, p.detail, false, false),
                        "{name}"
                    );
                } else {
                    assert!(m.smodel.is_none(), "{name}");
                }
                *census.entry(key).or_default() += 1;
            }
            ModelLight::Objective => {
                objectives += 1;
                let st = &m.stages[0];
                assert_eq!(
                    st.blend,
                    Some((BlendFactor::SrcAlpha, BlendFactor::One)),
                    "{name}"
                );
                assert!(!st.writes_depth(), "{name}");
                assert!(m.nofog, "{name}");
                assert!(m.probe.is_none(), "{name}");
                assert!(m.smodel.is_none(), "{name}");
            }
            _ => assert!(m.probe.is_none() && m.smodel.is_none(), "{name}"),
        }
    }
    for (k, n) in &census {
        println!("{k:40} {n}");
    }
    println!("objective {objectives}");
    assert_eq!(objectives, 31);
    assert_eq!(
        lib.get("mtl_tntbomb_obj").map(|m| m.model_light),
        Some(ModelLight::Objective)
    );
    for key in ["s true", "a true", "dtl true", "scroll true", "flag true"] {
        assert!(census.keys().any(|k| k.contains(key)), "{key}");
    }
    assert_eq!(census.get("none"), Some(&7), "{census:?}");
    let flags: usize = census
        .iter()
        .filter(|(k, _)| k.ends_with("flag true"))
        .map(|(_, n)| n)
        .sum();
    assert_eq!(flags, 4);
}

/// The colour-map fallbacks (cod2-materials.md 9.9..9.13): every set that
/// takes one is classified, the only ones default play shows are the
/// shadow cookie sets, and the code materials
/// vcod's own passes stand in for carry the blends those passes draw with:
/// `glow_apply_bloom` One, One; `glow_apply_sky_bleed` One, InvSrcAlpha;
/// `shellshock` SrcAlpha, InvSrcAlpha; the setup and filters opaque.
#[test]
fn fallback_sets_are_classified_and_code_blends_match() {
    use vcod_cod2::materials::{FallbackUse, fallback_use};
    use vcod_common::material::BlendFactor as B;
    let Some(fs) = vcod_common::testing::cod2_fs() else {
        return;
    };
    let (lib, report) = materials::compile_all(&fs).unwrap();
    let visible: BTreeSet<&str> = report
        .fallbacks
        .keys()
        .map(String::as_str)
        .filter(|s| matches!(fallback_use(s), FallbackUse::Visible(_)))
        .collect();
    println!("visible fallbacks: {visible:?}");
    for s in &visible {
        assert!(s.starts_with("shadow"), "{s}");
    }
    // the six water materials draw through the water pipeline (9.12)
    assert!(!report.fallbacks.contains_key("water"));
    // the snowflakes through the cloud pass (9.14)
    assert!(!report.fallbacks.contains_key("particle_cloud_outdoor"));
    let w = lib.get("water_elalamein").unwrap().water.as_ref().unwrap();
    assert_eq!((w.width, w.amplitude, w.wind_speed), (64, 0.06, 76.0));
    let blend = |name: &str| lib.get(name).unwrap().stages[0].blend.clone();
    assert_eq!(blend("glow_apply_bloom"), Some((B::One, B::One)));
    assert_eq!(
        blend("glow_apply_sky_bleed"),
        Some((B::One, B::OneMinusSrcAlpha))
    );
    assert_eq!(
        blend("shellshock"),
        Some((B::SrcAlpha, B::OneMinusSrcAlpha))
    );
    assert_eq!(blend("glow_setup"), None);
    for n in 1..=8 {
        assert_eq!(blend(&format!("filter_symmetric_{n}")), None);
    }
}
