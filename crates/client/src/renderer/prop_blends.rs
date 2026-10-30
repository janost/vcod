//! Every stock CoD2 xmodel surface material whose technique set is not
//! opaque, by the stage pipeline its first stage draws with, and how a
//! static prop colours its vertices for it: the multiply decals (the
//! crates' stencilled labels), the additive glows and the alpha blends sort
//! after the opaque models and write no depth, the alpha-tested ones draw
//! opaque, and only the light-probe sets take the grid. Prints the census.
//! Needs `COD2_DIR`; passes vacuously without it.

use super::*;
use std::collections::{BTreeMap, BTreeSet};
use vcod_cod2::material::MaterialFile;
use vcod_common::material::ModelLight;

/// Every material an xmodel LOD names, lowercased.
fn model_materials(fs: &Pk3Fs) -> BTreeSet<String> {
    let bones = |lod0: &str| {
        Ok(fs
            .read(&format!("xmodelparts/{lod0}"))
            .and_then(|d| vcod_cod2::xmodel::parse_parts(&d).ok())
            .map_or(0, |p| p.bones.len()))
    };
    let mut out = BTreeSet::new();
    for path in fs.list_prefix("xmodel/") {
        let Some(d) = fs.read(&path) else { continue };
        let Ok(d) = vcod_cod2::xmodel::parse_descriptor(&d, bones) else {
            continue;
        };
        for lod in d.present_lods() {
            out.extend(lod.materials.iter().map(|m| m.to_lowercase()));
        }
    }
    out
}

#[test]
fn stock_model_blends_take_their_pipelines() {
    let Some(fs) = vcod_common::testing::cod2_fs() else {
        return;
    };
    let lib = vcod_cod2::materials::load_lib(&fs).unwrap();
    // (technique set, variant, model colour) -> materials
    let mut census: BTreeMap<(String, String, bool), Vec<String>> = BTreeMap::new();
    for name in model_materials(&fs) {
        let Some(m) = lib.get(&name).filter(|m| !m.stages.is_empty()) else {
            continue;
        };
        let file = MaterialFile::parse(&fs.read(&format!("materials/{name}")).unwrap()).unwrap();
        let set = file.technique_set.clone();
        let (variant, _, _) = stage_draw_facts(m, 0).unwrap();
        let (_, _, depth_write) = stage_variant_state(variant);
        let own = cod2_model_colour(m);
        let st = &m.stages[0];
        match &st.blend {
            None => assert_eq!(variant, StageVariant::Opaque, "{name} {set}"),
            Some(_) => {
                assert!(m.sort > SORT_OPAQUE, "{name} {set} sort {}", m.sort);
                assert!(!depth_write, "{name} {set} {variant:?}");
            }
        }
        if set.starts_with("effect_multiply") {
            assert_eq!(variant, StageVariant::Multiply, "{name}");
            assert!(own, "{name}");
        }
        if set.starts_with("effect_add") {
            assert_eq!(variant, StageVariant::AdditiveFixed, "{name}");
            assert!(own, "{name}");
        }
        if st.blend == Some((BlendFactor::SrcAlpha, BlendFactor::OneMinusSrcAlpha)) {
            assert_eq!(variant, StageVariant::Blend, "{name} {set}");
        }
        if st.alpha_func.is_some() && st.blend.is_none() {
            assert!(depth_write, "{name} {set}");
        }
        // a lit set takes the grid, never the model's colours
        assert_eq!(own, m.model_light == ModelLight::None, "{name} {set}");
        if variant != StageVariant::Opaque || st.alpha_func.is_some() {
            census
                .entry((set, format!("{variant:?}"), own))
                .or_default()
                .push(name);
        }
    }
    for ((set, variant, own), names) in &census {
        println!(
            "{set:32} {variant:16} model colour {own:5} {:3}  {:?}",
            names.len(),
            &names[..names.len().min(3)]
        );
    }
    assert!(census.keys().any(|k| k.1 == "Multiply"));
    assert!(census.keys().any(|k| k.1 == "Blend"));
    assert!(census.keys().any(|k| k.1 == "AdditiveFixed"));
}

/// mp_toujane's crate labels (`mtl_dak_crate_decals`, a `vertcol_mul`
/// decal over `prop_crate_dak1`) keep the near-white colours the model
/// stores through the grid lighting, so the multiply leaves the wood
/// under the stencil's white; the crate's body takes the grid times the
/// greys its model stores (`lprobe_smc.hlsl`), each vertex its white
/// model's colour scaled by its own grey.
#[test]
fn toujane_crate_labels_keep_the_model_colours() {
    let Some(fs) = vcod_common::testing::cod2_fs() else {
        return;
    };
    vcod_cod2::model::register();
    let lib = vcod_cod2::materials::load_lib(&fs).unwrap();
    let data = fs.read("maps/mp/mp_toujane.d3dbsp").unwrap();
    let bsp = vcod_cod2::world::from_bytes(&data).unwrap();
    let mut lighting = vcod_common::static_light::StaticLighting::new(&bsp);
    let mut props = props::build(&fs, &bsp, &mut lighting, &|skin| props::SkinDraw {
        model_colour: lib.get(skin).is_some_and(cod2_model_colour),
        ..Default::default()
    });
    let light = crate::cod2_light::Cod2Lighting::new(&bsp).unwrap();
    // the same props, every model white
    let mut white = props::Props {
        verts: props.verts.clone(),
        vert_placement: props.vert_placement.clone(),
        vert_lit: props.vert_lit.clone(),
        vert_colour: vec![[255; 4]; props.verts.len()],
        vert_ambient: props.vert_ambient.clone(),
        vert_smodel: props.vert_smodel.clone(),
        vert_tangent: props.vert_tangent.clone(),
        indices: Vec::new(),
        batches: Vec::new(),
        ranges: Vec::new(),
        bounds: props.bounds.clone(),
        lods: Vec::new(),
    };
    light.light_props(&fs, &bsp.entities, &mut props);
    light.light_props(&fs, &bsp.entities, &mut white);
    let verts_of = |skin: &str| -> Vec<usize> {
        let b = props
            .batches
            .iter()
            .find(|b| b.skin.eq_ignore_ascii_case(skin))
            .unwrap_or_else(|| panic!("no {skin} batch"));
        let idx = &props.indices[b.first_index as usize..][..b.index_count as usize];
        idx.iter().map(|&i| i as usize).collect()
    };
    let colours = |skin: &str| -> Vec<[u8; 4]> {
        verts_of(skin)
            .iter()
            .map(|&i| props.verts[i].color)
            .collect()
    };
    let labels = colours("mtl_dak_crate_decals");
    assert!(!labels.is_empty());
    for c in &labels {
        assert!(c[..3].iter().all(|&v| v >= 231) && c[3] == 255, "{c:?}");
    }
    // the body takes the grid at half scale, under the labels' white
    let mean = |cs: &[[u8; 4]]| cs.iter().map(|c| f32::from(c[0])).sum::<f32>() / cs.len() as f32;
    let body = colours("mtl_dak_crate");
    assert!(
        mean(&body) < mean(&labels) * 0.8,
        "{} {}",
        mean(&body),
        mean(&labels)
    );
    let (mut stored, mut lit, mut grey, mut n) = (0.0, 0.0, 0.0, 0.0);
    for i in verts_of("mtl_dak_crate") {
        grey += f32::from(props.vert_colour[i][0]);
        let (got, full) = (props.verts[i].color, white.verts[i].color);
        for k in 0..3 {
            let want = f32::from(full[k]) * f32::from(props.vert_colour[i][k]) / 255.0;
            assert!((f32::from(got[k]) - want).abs() <= 1.0, "{got:?} {full:?}");
        }
        stored += f32::from(got[0]);
        lit += f32::from(full[0]);
        n += 1.0;
    }
    eprintln!(
        "crate body mean red: {:.1} as a white model, {:.1} with its greys (mean {:.1})",
        lit / n,
        stored / n,
        grey / n
    );
    assert!(grey / n < 180.0, "{}", grey / n);
}

/// Every stock xmodel surface material's technique set by the model light
/// it takes (cod2-materials.md 9.2): the `phong_*` sets probe lit, the
/// `ambient_*` ones by their model's centre, the effects and unlit sets by
/// their stages. Prints, per set, the materials, LOD 0 vertices and the
/// mean red the models store, which the lit techniques multiply by.
#[test]
fn stock_model_sets_take_their_light() {
    let Some(fs) = vcod_common::testing::cod2_fs() else {
        return;
    };
    vcod_cod2::model::register();
    let lib = vcod_cod2::materials::load_lib(&fs).unwrap();
    // set -> (light, materials, vertices, summed red)
    let mut sets: BTreeMap<String, (ModelLight, BTreeSet<String>, usize, f64)> = BTreeMap::new();
    for path in fs.list_prefix("xmodel/") {
        let Ok(m) = vcod_common::model::load(&fs, path.trim_start_matches("xmodel/")) else {
            continue;
        };
        let Some(lod) = m.lods.first() else { continue };
        for s in &lod.surfaces {
            let name = lod.materials[s.material].to_lowercase();
            let Some(mat) = lib.get(&name).filter(|m| !m.stages.is_empty()) else {
                continue;
            };
            let file =
                MaterialFile::parse(&fs.read(&format!("materials/{name}")).unwrap()).unwrap();
            let e = sets.entry(file.technique_set).or_insert((
                mat.model_light,
                BTreeSet::new(),
                0,
                0.0,
            ));
            assert_eq!(e.0, mat.model_light, "{name}");
            e.1.insert(name);
            e.2 += s.verts.len();
            e.3 += s.verts.iter().map(|v| f64::from(v.color[0])).sum::<f64>();
        }
    }
    let (mut lit, mut lit_red) = (0, 0.0);
    for (set, (light, mats, verts, red)) in &sets {
        println!(
            "{set:32} {light:8?} {:4} materials {verts:8} vertices, mean red {:5.1}",
            mats.len(),
            red / *verts as f64
        );
        let want = if set.starts_with("phong_") {
            ModelLight::Probe
        } else if set.starts_with("ambient_") {
            ModelLight::Ambient
        } else if set.starts_with("effect") || set.starts_with("unlit_") {
            ModelLight::None
        } else {
            continue;
        };
        assert_eq!(*light, want, "{set}");
        if want != ModelLight::None {
            lit += verts;
            lit_red += red;
        }
    }
    // the lit models are far from white: ignoring the colour brightened them
    assert!(lit_red / (lit as f64) < 220.0, "{}", lit_red / lit as f64);
    assert!(sets.values().any(|s| s.0 == ModelLight::Ambient));
}
