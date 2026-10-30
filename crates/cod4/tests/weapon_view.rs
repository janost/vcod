//! A 1.7 install's weapon models (docs/research/cod4-client-view.md 4.2,
//! 4.5): each view gun's `hideTags` hide the attachments the weapon lacks,
//! and every material of every weapon's gun, world, hands and knife
//! models compiles to the state its bits give; a destructible car's
//! `hidePart`s keep one pair of its four glass layers
//! (cod4-client-view.md 15). Vacuous without `COD4_DIR`.

use std::collections::BTreeMap;
use vcod_cod4::anim::{hide_part_bits, model_hiding, model_parts};
use vcod_cod4::assets::Assets;
use vcod_cod4::assets::material::Material;
use vcod_cod4::assets::model::XModel;
use vcod_cod4::map::map_material;
use vcod_cod4::materials::compile;
use vcod_cod4::state::{AlphaTest, Cull, DepthTest, Factor};
use vcod_cod4::techset::Family;
use vcod_cod4::testing::cod4_install;
use vcod_cod4::walk::{WalkOptions, walk};
use vcod_cod4::zone::Zone;
use vcod_common::material::{AlphaFunc, BlendFactor, Cull as MatCull};

fn common_mp() -> Option<Assets> {
    let install = cod4_install()?;
    let z = Zone::open_path(&install.zone("english", "common_mp")).unwrap();
    Some(walk(z, WalkOptions { keep_bulk: true }).unwrap().assets)
}

fn gun<'a>(a: &'a Assets, weapon: &str) -> (&'a XModel, Vec<String>) {
    let w = a.weapons.iter().find(|w| w.name == weapon).unwrap();
    let id = w.model_lists["gunModel"][0].unwrap();
    (&a.xmodels[id.index()], w.hide_tags(a))
}

/// The materials `weapon`'s view gun keeps under its `hideTags`.
fn kept_materials(a: &Assets, weapon: &str) -> Vec<String> {
    let (x, tags) = gun(a, weapon);
    let hide = hide_part_bits(a, x, &tags);
    let m = model_hiding(a, x, hide, |id| a.materials[id.index()].name.clone())
        .unwrap()
        .unwrap();
    m.lods[0].materials.clone()
}

/// `mp5_mp` hides the ACOG and the reflex sight its gun model carries,
/// `mp5_acog_mp` keeps the ACOG, and the plain W1200 hides the reflex
/// sight its reflex variant keeps (the live playtest's two sights).
#[test]
fn hide_tags_hide_the_attachments_a_weapon_lacks() {
    let Some(a) = common_mp() else {
        return;
    };
    let (x, tags) = gun(&a, "mp5_mp");
    assert_eq!(tags, ["tag_acog", "tag_ak47_mount", "tag_reflex_sight"]);
    assert_ne!(hide_part_bits(&a, x, &tags), [0; 4]);
    let has = |mats: &[String], s: &str| mats.iter().any(|m| m.contains(s));
    let plain = kept_materials(&a, "mp5_mp");
    assert!(!has(&plain, "acog") && !has(&plain, "reflex"), "{plain:?}");
    let acog = kept_materials(&a, "mp5_acog_mp");
    assert!(
        has(&acog, "acog_lens") && !has(&acog, "reflex_red_dot"),
        "{acog:?}"
    );
    let w1200 = kept_materials(&a, "winchester1200_mp");
    assert!(!has(&w1200, "reflex"), "{w1200:?}");
    let reflex = kept_materials(&a, "winchester1200_reflex_mp");
    assert!(has(&reflex, "reflex_red_dot") && has(&reflex, "reflex_lens"));
    // Every hide tag names a bone of its gun (`CG_RegisterWeapon` prints
    // an error for one that does not).
    let mut tagged = 0;
    for w in &a.weapons {
        let Some(Some(id)) = w.model_lists.get("gunModel").and_then(|l| l.first()) else {
            continue;
        };
        let x = &a.xmodels[id.index()];
        for t in w.hide_tags(&a) {
            let bits = hide_part_bits(&a, x, std::slice::from_ref(&t));
            assert_ne!(bits, [0; 4], "{}: {} has no {t}", w.name, x.name);
            tagged += 1;
        }
    }
    println!("{tagged} hide tags");
    assert!(tagged > 100, "{tagged}");
}

/// The models a weapon draws with: its guns, world models, hands and
/// knives, by name.
fn weapon_models(a: &Assets) -> BTreeMap<String, &XModel> {
    let mut out = BTreeMap::new();
    for w in &a.weapons {
        let lists = ["gunModel", "worldModel"]
            .iter()
            .filter_map(|k| w.model_lists.get(k))
            .flatten()
            .flatten();
        let singles = ["handModel", "knifeModel", "worldKnifeModel"]
            .iter()
            .filter_map(|k| w.models.get(k));
        for id in lists.chain(singles) {
            let x = &a.xmodels[id.index()];
            out.insert(x.name.trim_start_matches(',').to_string(), x);
        }
    }
    out
}

/// Every material on every weapon model of `common_mp` compiles to the
/// blend, alpha test, cull and depth write its lit state bits give
/// (cod4-world.md 8), in a family vcod draws; a reflex sight's dot draws
/// through its own technique with its `detailScale`.
#[test]
fn weapon_model_materials_draw_their_state_bits() {
    let Some(a) = common_mp() else {
        return;
    };
    let models = weapon_models(&a);
    let mut seen = BTreeMap::new();
    for x in models.values() {
        for id in x.materials.iter().flatten() {
            let m = map_material(&a, *id);
            seen.entry(m.name.clone()).or_insert(m);
        }
    }
    let mut families: BTreeMap<String, usize> = BTreeMap::new();
    for m in seen.values() {
        let c = compile(m);
        let set = m.techset.clone().unwrap_or_default();
        let family = m.class.map(|c| c.family);
        assert!(
            family != Some(Family::Other),
            "{}: {set} unclassified",
            m.name
        );
        *families.entry(set.clone()).or_default() += 1;
        let Some(st) = m.draw_state else {
            assert!(c.surface.nodraw, "{}: no drawing technique", m.name);
            continue;
        };
        let stage = &c.stages[0];
        let want_blend = match st.blend {
            None | Some((Factor::One, Factor::Zero, _)) => None,
            // a premultiplying set's One draws as SrcAlpha (cod4-world.md 8.1)
            Some((s, d, _)) => Some((
                match s {
                    Factor::One if m.class.is_some_and(|c| c.premultiplied) => {
                        BlendFactor::SrcAlpha
                    }
                    s => factor(s),
                },
                factor(d),
            )),
        };
        assert_eq!(stage.blend, want_blend, "{}: {set} blend", m.name);
        let want_alpha = st.alpha_test.map(|a| match a {
            AlphaTest::Gt0 => AlphaFunc::Gt0,
            AlphaTest::Lt128 => AlphaFunc::Lt128,
            AlphaTest::Ge128 => AlphaFunc::Ge128,
        });
        assert_eq!(stage.alpha_func, want_alpha, "{}: alpha test", m.name);
        let want_cull = match st.cull {
            Cull::Back => MatCull::Back,
            _ => MatCull::None,
        };
        assert_eq!(c.cull, want_cull, "{}: cull", m.name);
        let depth_write = st.depth_write && st.depth_test != DepthTest::Disabled;
        assert_eq!(stage.depth_write, Some(depth_write), "{}", m.name);
        assert_eq!(
            c.reflex.is_some(),
            family == Some(Family::ReflexSight),
            "{}",
            m.name
        );
    }
    println!(
        "{} models, {} materials: {families:?}",
        models.len(),
        seen.len()
    );
    let red_dot = compile(&seen["mc/mtl_weapon_reflex_red_dot"]);
    assert_eq!(red_dot.reflex, Some([25.0, 25.0]));
    assert!(models.len() > 300, "{}", models.len());
}

fn factor(f: Factor) -> BlendFactor {
    match f {
        Factor::Zero => BlendFactor::Zero,
        Factor::One => BlendFactor::One,
        Factor::SrcColor => BlendFactor::SrcColor,
        Factor::InvSrcColor => BlendFactor::OneMinusSrcColor,
        Factor::SrcAlpha => BlendFactor::SrcAlpha,
        Factor::InvSrcAlpha => BlendFactor::OneMinusSrcAlpha,
        Factor::DestAlpha => BlendFactor::DstAlpha,
        Factor::InvDestAlpha => BlendFactor::OneMinusDstAlpha,
        Factor::DestColor => BlendFactor::DstColor,
        Factor::InvDestColor => BlendFactor::OneMinusDstColor,
    }
}

/// The pixel shaders the weapons', effects' and marks' draw techniques
/// run, by name, and whether each writes its colour times its alpha, read
/// off its bytecode (`--example techset_shaders common_mp <set>`,
/// `tools/re/d3d9dis.py`; cod4-effects.md 13): the add shaders' last
/// colour write multiplies by the alpha they write, the `b0` lit ones'
/// colour map times vertex colour is scaled by its alpha first, the rest
/// write their colour as is. `reflexsight.hlsl` is cod4-client-view.md
/// 4.5's.
const PIXEL_SHADERS: &[(&str, bool)] = &[
    ("vertcol_simple.hlsl", false),
    ("vertcol_simple_fog.hlsl", false),
    ("vertcol_simple_add_fog.hlsl", true),
    ("vertcol_simple_add.hlsl", true),
    ("zfeather.hlsl", false),
    ("zfeather_nf.hlsl", false),
    ("zfeather_add.hlsl", true),
    ("zfeather_add_nf.hlsl", true),
    ("particle_cloud.hlsl", false),
    ("particle_cloud_add.hlsl", true),
    ("particle_cloud_outdoor.hlsl", false),
    ("particle_cloud_outdoor_add.hlsl", true),
    ("reflexsight.hlsl", true),
    ("lp_b0c0_sm3.hlsl", true),
    ("lp_b0c0n0_sm3.hlsl", true),
    ("lp_b0c0s0_sm3.hlsl", true),
    ("lp_b0c0n0s0_sm3.hlsl", true),
    ("lm_b0c0_sm3.hlsl", true),
    ("lm_b0c0n0_sm3.hlsl", true),
    ("lm_b0c0s0_sm3.hlsl", true),
    ("lm_b0c0n0s0_sm3.hlsl", true),
];

/// The draw technique's pixel shader of material `id`: the first of
/// `DRAW_TECHNIQUES` whose state entry the material names, out of `a`'s
/// set or, for a set `a` holds as a stub, `other`'s of that name.
fn draw_shader(a: &Assets, other: &Assets, id: vcod_cod4::assets::Id<Material>) -> Option<String> {
    let m = &a.materials[id.index()];
    let name = a.techsets[m.techset?.index()].name.trim_start_matches(',');
    let t = vcod_cod4::state::DRAW_TECHNIQUES
        .iter()
        .copied()
        .find(|&t| {
            m.state_bits_entry
                .get(t)
                .is_some_and(|&e| usize::from(e) < m.state_bits.len())
        })?;
    [a, other].into_iter().find_map(|z| {
        let set = z
            .techsets
            .iter()
            .find(|s| !s.name.starts_with(',') && s.name == name)?;
        let tech = &z.techniques[(*set.techniques.get(t)?)?.index()];
        Some(tech.passes.first()?.pixel_shader.as_ref()?.name.clone())
    })
}

/// Every blended material on a weapon model, an effect element or a mark
/// of `common_mp`: its set's class premultiplies exactly where its draw
/// technique's pixel shader does ([`PIXEL_SHADERS`]; a shader not listed
/// fails). The ACOG reticle's `mc_effect` runs `vertcol_simple_fog.hlsl`
/// under (One, InvSrcAlpha): not premultiplied, its colour map's texels
/// are.
#[test]
fn weapon_and_effect_materials_premultiply_as_their_shaders() {
    use vcod_cod4::assets::fx::FxVisual;
    let Some(a) = common_mp() else {
        return;
    };
    // The sets `common_mp` holds as stubs (`effect_nofog`).
    let install = cod4_install().unwrap();
    let z = Zone::open_path(&install.zone("english", "code_post_gfx_mp")).unwrap();
    let post = walk(z, WalkOptions { keep_bulk: true }).unwrap().assets;
    let mut ids = Vec::new();
    for x in weapon_models(&a).values() {
        ids.extend(x.materials.iter().flatten().copied());
    }
    for e in a.fx.iter().flat_map(|f| &f.elems) {
        for v in &e.visuals {
            match v {
                FxVisual::Material(Some(id)) => ids.push(*id),
                FxVisual::Mark(m) => ids.extend(m.iter().flatten().copied()),
                _ => {}
            }
        }
    }
    let mut rows: BTreeMap<(String, String, bool), usize> = BTreeMap::new();
    let mut unknown = Vec::new();
    let mut seen = std::collections::BTreeSet::new();
    for id in ids {
        let m = map_material(&a, id);
        let (Some(c), Some(st)) = (m.class, m.draw_state) else {
            continue;
        };
        if st.blend.is_none() || c.family == Family::Distortion || !seen.insert(m.name.clone()) {
            continue;
        }
        let set = m.techset.clone().unwrap_or_default();
        let Some(shader) = draw_shader(&a, &post, id) else {
            unknown.push(format!("{}: {set} no shader", m.name));
            continue;
        };
        match PIXEL_SHADERS.iter().find(|(n, _)| *n == shader) {
            Some(&(_, premul)) => {
                assert_eq!(c.premultiplied, premul, "{}: {set} runs {shader}", m.name)
            }
            None => unknown.push(format!("{}: {set} runs {shader}", m.name)),
        }
        *rows.entry((set, shader, c.premultiplied)).or_default() += 1;
    }
    for ((set, shader, premul), n) in &rows {
        println!("{set} {shader} premultiplied {premul}: {n}");
    }
    assert!(unknown.is_empty(), "{unknown:#?}");
    let reticle = map_material(&a, material_id(&a, "mc/mtl_weapon_acog_reticle"));
    assert!(!reticle.class.unwrap().premultiplied);
    assert_eq!(
        reticle.draw_state.unwrap().blend.map(|b| (b.0, b.1)),
        Some((Factor::One, Factor::InvSrcAlpha))
    );
    assert!(rows.len() > 20, "{rows:?}");
}

fn material_id(a: &Assets, name: &str) -> vcod_cod4::assets::Id<Material> {
    vcod_cod4::assets::Id::new(a.materials.iter().position(|m| m.name == name).unwrap())
}

/// `_destructible.gsc` hides each part's later states' tags at the
/// setup: the six windows' and four lights' `_d` tags. Their bits are the
/// walk capture's `partBits` (0x242a, 0x92480000), and under them the
/// sedan keeps its intact glass, inside and out, and none of the
/// shattered; a cracked window (its tag hidden, its `_d` shown) the
/// other way round.
#[test]
fn a_destructible_sedans_parts_keep_one_glass_pair() {
    let Some(install) = cod4_install() else {
        return;
    };
    let z = Zone::open_path(&install.zone("english", "mp_crash")).unwrap();
    let a = walk(z, WalkOptions { keep_bulk: true }).unwrap().assets;
    let x = a
        .xmodels
        .iter()
        .find(|x| x.name == "vehicle_80s_sedan1_red_destructible_mp")
        .unwrap();
    let windows = [
        "front",
        "back",
        "left_front",
        "right_front",
        "left_back",
        "right_back",
    ];
    let lights = ["left_front", "right_front", "left_back", "right_back"];
    let tags = |suffix: &str, lights_too: bool| -> Vec<String> {
        let w = windows.iter().map(|w| format!("tag_glass_{w}{suffix}"));
        let l = lights.iter().map(|l| format!("tag_light_{l}_d"));
        w.chain(l.filter(|_| lights_too)).collect()
    };
    let setup = hide_part_bits(&a, x, &tags("_d", true));
    assert_eq!(setup, [0x242a, 0x9248_0000, 0, 0], "{setup:08x?}");
    let kept = |hide: [u32; 4]| {
        let (m, bits) = model_parts(&a, x, hide, |id| a.materials[id.index()].name.clone())
            .unwrap()
            .unwrap();
        assert_eq!(bits.len(), m.lods[0].surfaces.len());
        let mut names: Vec<String> = m.lods[0]
            .surfaces
            .iter()
            .map(|s| m.lods[0].materials[s.material].clone())
            .filter(|n| n.contains("glass"))
            .collect();
        names.sort();
        names.dedup();
        names
    };
    let all = kept([0; 4]);
    assert_eq!(all.len(), 4, "{all:?}");
    let intact = kept(setup);
    assert!(
        intact.len() == 2 && intact.iter().all(|n| !n.contains("shattered")),
        "{intact:?}"
    );
    let cracked = kept(hide_part_bits(&a, x, &tags("", false)));
    assert!(
        cracked.len() == 2 && cracked.iter().all(|n| n.contains("shattered")),
        "{cracked:?}"
    );
}
