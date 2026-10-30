//! Every world and static model surface material of the 21 stock CoD4 MP
//! maps, by what its technique and state bits say retail draws and what
//! vcod's pipelines draw for it (`vcod_cod4::materials::compile`, then
//! the stage or lit pipeline the renderer picks): blend, premultiplied
//! colour, depth write, alpha test, cull, polygon offset, the vertex
//! shader's colour (falloff, eye offset, feather), a distortion set's
//! bend, fog and lighting, a lit
//! set's normal, specular, reflection probe and detail inputs, and
//! that the blended draws sort in retail's `sortKey` order. A
//! combination that differs fails unless [`ALLOWED`] names its set with
//! the reason; an [`ALLOWED`] entry nothing matches fails too. Prints the
//! census (`--nocapture`). Needs `COD4_DIR`; passes vacuously without it.
//! `docs/research/cod4-world.md` section 9.5.

use super::*;
use std::collections::BTreeMap;
use vcod_cod4::map::{Map, MapMaterial, NO_LIGHTMAP};
use vcod_cod4::state::{AlphaTest, Cull as GfxCull, DepthTest, Factor};
use vcod_cod4::techset::Family;
use vcod_common::material::Technique;

/// An allowed difference: its name, a test on the set's name, the reason.
type Allowed = (&'static str, fn(&str) -> bool, &'static str);

/// Sets vcod deliberately draws unlike retail.
const ALLOWED: &[Allowed] = &[
    (
        "ambient world",
        |t| t == "wc_ambient_t0c0",
        "vertcol_shaded_fog's lerp to materialColor is not drawn",
    ),
    (
        "stub",
        |t| t == "-",
        "a material stub resolved from another zone is not loaded",
    ),
];

/// One draw's facts, on an unpremultiplied colour.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct Draw {
    /// Colour source and destination factors; `None` unblended.
    blend: Option<(String, String)>,
    /// The colour is multiplied by its alpha where no factor carries it.
    premul: bool,
    depth_write: bool,
    alpha_test: Option<&'static str>,
    two_sided: bool,
    offset: u8,
    /// `plain`, `distance falloff`, `angle falloff`, `flag wave`.
    vertex: &'static str,
    eye_offset: bool,
    feather: bool,
    /// A distortion set bending the frame's copy.
    refract: bool,
    fog: bool,
    /// `none`, `lightmap`, `model light`, `sky`, `water`, `material tint`.
    light: &'static str,
    /// A lit set's inputs past the colour map (`n0`, `s0`, `d0`), bound
    /// and read: the normal map, the specular map, the reflection probe
    /// the specular adds to, the detail map.
    normal_map: bool,
    specular: bool,
    reflection: bool,
    detail: bool,
}

fn wgpu_factor(f: Factor) -> wgpu::BlendFactor {
    use wgpu::BlendFactor as W;
    match f {
        Factor::Zero => W::Zero,
        Factor::One => W::One,
        Factor::SrcColor => W::Src,
        Factor::InvSrcColor => W::OneMinusSrc,
        Factor::SrcAlpha => W::SrcAlpha,
        Factor::InvSrcAlpha => W::OneMinusSrcAlpha,
        Factor::DestAlpha => W::DstAlpha,
        Factor::InvDestAlpha => W::OneMinusDstAlpha,
        Factor::DestColor => W::Dst,
        Factor::InvDestColor => W::OneMinusDst,
    }
}

fn pair(s: wgpu::BlendFactor, d: wgpu::BlendFactor) -> (String, String) {
    (format!("{s:?}"), format!("{d:?}"))
}

/// What retail draws `m` with: its draw technique's state bits and what
/// its set's name says its shaders do (cod4-world.md 4, 8, 9.5); `None`
/// when it draws nothing in the lit view. `lightmap`: the surface has one.
fn retail(m: &MapMaterial, lightmap: bool) -> Option<Draw> {
    let Some(c) = m.class else {
        return Some(stub_draw());
    };
    let st = m.draw_state?;
    if c.family == Family::Invisible {
        return None;
    }
    if c.family == Family::Sky {
        return Some(sky_draw());
    }
    let premul_src = c.premultiplied;
    let (blend, premul) = match st.blend {
        None | Some((Factor::One, Factor::Zero, _)) => (None, false),
        Some((Factor::One, d, _)) if premul_src => (
            Some(pair(wgpu::BlendFactor::SrcAlpha, wgpu_factor(d))),
            false,
        ),
        Some((s, d, _)) => (Some(pair(wgpu_factor(s), wgpu_factor(d))), premul_src),
    };
    let t = m.techset.as_deref().unwrap_or("");
    let inputs = match c.family {
        Family::Lit => vcod_cod4::materials::lit_inputs(t),
        _ => Default::default(),
    };
    Some(Draw {
        blend,
        premul,
        depth_write: st.depth_write && st.depth_test != DepthTest::Disabled,
        alpha_test: st.alpha_test.map(|a| match a {
            AlphaTest::Gt0 => "gt0",
            AlphaTest::Lt128 => "lt128",
            AlphaTest::Ge128 => "ge128",
        }),
        two_sided: st.cull != GfxCull::Back,
        offset: st.polygon_offset.min(2),
        vertex: if c.dist_falloff {
            "distance falloff"
        } else if c.angle_falloff {
            "angle falloff"
        } else if t.contains("_flag_") {
            "flag wave"
        } else {
            "plain"
        },
        eye_offset: c.eye_offset,
        feather: c.zfeather && c.family != Family::Distortion,
        refract: c.family == Family::Distortion,
        fog: !c.nofog,
        light: match c.family {
            Family::Lit if c.world && lightmap => "lightmap",
            Family::Lit if c.world => "lightmap (surface has none)",
            Family::Lit => "model light",
            Family::Water => "water",
            _ if t == "mc_ambient_t0c0" => "model light",
            _ if t == "wc_ambient_t0c0" => "material tint",
            _ => "none",
        },
        normal_map: inputs.normal,
        specular: inputs.specular,
        reflection: inputs.specular,
        detail: inputs.detail,
    })
}

fn sky_draw() -> Draw {
    Draw {
        blend: None,
        premul: false,
        depth_write: false,
        alpha_test: None,
        two_sided: false,
        offset: 0,
        vertex: "plain",
        eye_offset: false,
        feather: false,
        refract: false,
        fog: false,
        light: "sky",
        normal_map: false,
        specular: false,
        reflection: false,
        detail: false,
    }
}

/// A material whose set lives in another zone: drawn by retail, how unknown.
fn stub_draw() -> Draw {
    Draw {
        light: "stub",
        ..sky_draw()
    }
}

/// What vcod draws `sh` with: the stage pipeline [`stage_draw_facts`]
/// picks, or the CoD2 lit one for a lit world batch on a lightmap page.
fn vcod(sh: &Material, lightmap: bool) -> Option<Draw> {
    if sh.surface.nodraw {
        return None;
    }
    if sh.technique == Technique::Sky {
        return Some(sky_draw());
    }
    let (v, two_sided, bias) = stage_draw_facts(sh, 0)?;
    let st = &sh.stages[0];
    // a CoD4 model set off the model lighting volume, every stock map
    // carrying one (`lightgrid_census`)
    let model = cod2::draws_cod4_model(sh, v);
    let water = cod2::draws_cod4_water(sh, v);
    if water {
        // water_l_sun's inputs are its own; the census compares the light
        let lit = cod2::variant_state(v).expect("a lit variant");
        return Some(Draw {
            blend: lit.1.map(|b| pair(b.color.src_factor, b.color.dst_factor)),
            premul: false,
            depth_write: lit.2,
            alpha_test: sh.stages[0].alpha_func.as_ref().map(|a| match a {
                AlphaFunc::Gt0 => "gt0",
                AlphaFunc::Lt128 => "lt128",
                AlphaFunc::Ge128 => "ge128",
            }),
            two_sided,
            offset: match sh.depth_bias.filter(|_| bias) {
                None => 0,
                Some(PolygonOffset::Single) => 1,
                Some(_) => 2,
            },
            vertex: "plain",
            eye_offset: false,
            feather: false,
            refract: false,
            fog: !sh.nofog,
            light: "water",
            normal_map: false,
            specular: false,
            reflection: false,
            detail: false,
        });
    }
    let lit = model || (sh.phong.is_some() && lightmap && cod2::variant_state(v).is_some());
    let (entry, blend, depth_write) = match cod2::variant_state(v).filter(|_| lit) {
        Some((_, blend, dw)) => ("fs_main", blend, dw),
        None => stage_variant_state(v),
    };
    let lightmap_bundle = st.bundles.iter().any(|b| b.image == ImageRef::Lightmap);
    Some(Draw {
        blend: blend.map(|b| pair(b.color.src_factor, b.color.dst_factor)),
        premul: entry == "fs_stage_premul",
        depth_write,
        alpha_test: st.alpha_func.as_ref().map(|a| match a {
            AlphaFunc::Gt0 => "gt0",
            AlphaFunc::Lt128 => "lt128",
            AlphaFunc::Ge128 => "ge128",
        }),
        two_sided,
        offset: match sh.depth_bias.filter(|_| bias) {
            None => 0,
            Some(PolygonOffset::Single) => 1,
            Some(_) => 2,
        },
        vertex: match sh.falloff.map(|f| f.kind) {
            None if sh.phong.and_then(|p| p.cod4).is_some_and(|c| c.flag) => "flag wave",
            None => "plain",
            Some(FalloffKind::Distance) => "distance falloff",
            Some(FalloffKind::Angle) => "angle falloff",
        },
        eye_offset: sh.eye_offset.is_some(),
        // the stage pipelines' feathered and distortion draws
        feather: cod4_worldfx::world_fx(sh, v) == cod4_worldfx::WorldFx::Feather,
        refract: cod4_worldfx::world_fx(sh, v) == cod4_worldfx::WorldFx::Distort,
        fog: !sh.nofog,
        light: if model {
            "model light"
        } else if lit || lightmap_bundle {
            "lightmap"
        } else {
            "none"
        },
        normal_map: lit && sh.phong.and_then(|p| p.cod4).is_some_and(|c| c.normal_map),
        specular: lit && sh.phong.is_some_and(|p| p.specular),
        // the surface's probe, every stock map carrying its cubes
        reflection: lit && sh.phong.is_some_and(|p| p.specular),
        detail: lit && sh.phong.is_some_and(|p| p.detail),
    })
}

#[derive(Default)]
struct Row {
    world: usize,
    smodel: usize,
    maps: std::collections::BTreeSet<String>,
    materials: std::collections::BTreeSet<String>,
}

#[test]
fn cod4_material_census_draws_every_combination_as_retail() {
    let Some(install) = vcod_cod4::testing::cod4_install() else {
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
    assert_eq!(maps.len(), 21, "{maps:?}");
    // (set, retail, vcod) -> surfaces
    let mut rows: BTreeMap<(String, Option<Draw>, Option<Draw>), Row> = BTreeMap::new();
    // a blended draw's sortKey -> vcod's sort values for it
    let mut sorts: BTreeMap<u8, (f32, f32)> = BTreeMap::new();
    for name in &maps {
        let (map, _) = Map::load(&install, "english", name).unwrap();
        let compiled: Vec<Material> = map
            .materials
            .iter()
            .map(vcod_cod4::materials::compile)
            .collect();
        // a water set's shader reads two constants its material carries
        for m in &map.materials {
            if m.class.is_some_and(|c| c.family == Family::Water) {
                for k in ["waterColor", "envMapParms"] {
                    let h = vcod_cod4::map::sampler_hash(k);
                    let c = m.constants.iter().find(|c| c.0 == h);
                    assert!(c.is_some(), "{name} {}: no {k}", m.name);
                }
            }
        }
        let mut add = |mi: usize, lightmap: bool, smodel: bool| {
            let m = &map.materials[mi];
            let sh = &compiled[mi];
            if sh.stages.first().is_some_and(|st| st.blend.is_some()) {
                let e = sorts.entry(m.sort_key).or_insert((sh.sort, sh.sort));
                *e = (e.0.min(sh.sort), e.1.max(sh.sort));
            }
            let set = m.techset.clone().unwrap_or_else(|| "-".into());
            let key = (set, retail(m, lightmap), vcod(&compiled[mi], lightmap));
            let row = rows.entry(key).or_default();
            *if smodel {
                &mut row.smodel
            } else {
                &mut row.world
            } += 1;
            row.maps.insert(name.clone());
            if row.materials.len() < 3 {
                row.materials.insert(m.name.clone());
            }
        };
        for s in &map.surfaces {
            add(s.material as usize, s.lightmap != NO_LIGHTMAP, false);
        }
        for sm in &map.static_models {
            for s in sm.mesh.iter().flat_map(|i| &map.models[*i].surfaces) {
                add(s.material as usize, false, true);
            }
        }
    }
    let mut used = vec![false; ALLOWED.len()];
    let mut failed = Vec::new();
    for ((set, want, got), row) in &rows {
        let same = want == got;
        let allowed = (!same)
            .then(|| ALLOWED.iter().position(|(_, f, _)| f(set)))
            .flatten();
        if let Some(i) = allowed {
            used[i] = true;
        }
        let mark = match (same, allowed) {
            (true, _) => "same".to_string(),
            (false, Some(i)) => format!("allowed ({})", ALLOWED[i].0),
            (false, None) => "DIFFERS".to_string(),
        };
        println!(
            "{set} world {} smodel {} maps {} {:?}: {mark}\n  retail {want:?}\n  vcod   {got:?}",
            row.world,
            row.smodel,
            row.maps.len(),
            row.materials
        );
        if !same && allowed.is_none() {
            failed.push(format!("{set} {:?}", row.materials));
        }
    }
    // vcod's sort keeps the blended draws in retail's sortKey order
    println!("blended sortKey -> vcod sort: {sorts:?}");
    for ((k0, s0), (k1, s1)) in sorts.iter().zip(sorts.iter().skip(1)) {
        assert!(s0.1 < s1.0, "sortKey {k0} {s0:?} not under {k1} {s1:?}");
    }
    let surfaces: usize = rows.values().map(|r| r.world + r.smodel).sum();
    println!("{} combinations over {surfaces} surfaces", rows.len());
    for (i, (name, _, _)) in ALLOWED.iter().enumerate() {
        assert!(used[i], "ALLOWED entry {name} matches nothing");
    }
    assert!(failed.is_empty(), "{failed:#?}");
}

/// Weapon and entity materials the entity pipelines draw unlike retail,
/// past [`ALLOWED`]: none.
const ENTITY_ALLOWED: &[Allowed] = &[];

/// Effect sets the fx pass deliberately draws unlike retail.
const FX_ALLOWED: &[Allowed] = &[(
    "fx inverse add",
    |t| t.contains("_add"),
    "the fx pass draws (InvDestColor, One) as a plain add (cod4-effects.md 9)",
)];

/// What the fx pass draws an effect stage with ([`fx_stage_draw`]): the
/// blend its [`FxBlend`] and `FX_VERT_TEX_ALPHA` amount to on an
/// unpremultiplied colour, its alpha test, fog and feather.
fn fx_draw(sh: &Material) -> Option<Draw> {
    use wgpu::BlendFactor as W;
    if sh.surface.nodraw {
        return None;
    }
    let st = sh.stages.first()?;
    let (blend, flags) = fx_stage_draw(sh, st);
    let blend = match blend {
        FxBlend::Opaque => None,
        FxBlend::Add if flags & FX_VERT_TEX_ALPHA != 0 => Some(pair(W::SrcAlpha, W::One)),
        FxBlend::Add => Some(pair(W::One, W::One)),
        FxBlend::Screen => Some(pair(W::One, W::OneMinusSrc)),
        FxBlend::Alpha => Some(pair(W::SrcAlpha, W::OneMinusSrcAlpha)),
    };
    Some(Draw {
        blend,
        alpha_test: [
            (FX_VERT_ALPHA_GT0, "gt0"),
            (FX_VERT_ALPHA_LT128, "lt128"),
            (FX_VERT_ALPHA_GE128, "ge128"),
        ]
        .into_iter()
        .find_map(|(f, n)| (flags & f != 0).then_some(n)),
        fog: flags & FX_VERT_NOFOG == 0,
        feather: sh.feather.is_some(),
        ..fx_facts()
    })
}

/// The facts [`fx_draw`] compares, the rest at a constant.
fn fx_facts() -> Draw {
    Draw {
        premul: false,
        depth_write: false,
        two_sided: false,
        offset: 0,
        vertex: "plain",
        eye_offset: false,
        light: "none",
        ..sky_draw()
    }
}

/// What the entity and view model pipelines draw `sh` with
/// (`entity_material`): [`vcod`]'s facts, under the stage's own blend pair,
/// a lit model set off the entity's block through the probe path
/// (`entity_material::cod4`, cod4-world.md 3.3), resolved as the live
/// client resolves a skin: `entity_material::load` on the install's iwd
/// search path `fs`.
fn entity_draw(sh: &Material, fs: &Pk3Fs) -> Option<Draw> {
    let mut d = vcod(sh, false)?;
    let lib = vcod_common::material::MaterialLib::from_materials([(sh.clone(), None)], 0);
    let mut warns = vcod_common::shader::WarnSet::default();
    let ent = crate::entity_material::load(fs, &lib, &sh.name, &mut warns);
    d.blend = sh.stages[0].blend.as_ref().map(|(s, t)| {
        pair(
            crate::entity_material::wgpu_blend_factor(s),
            crate::entity_material::wgpu_blend_factor(t),
        )
    });
    d.premul = false;
    // the entity pipelines draw no world fx
    d.feather = false;
    d.refract = false;
    // the probe path lights a stage whose material has a probe technique
    let lit = ent.probe.is_some()
        && crate::entity_material::is_lit(&ent)
        && crate::entity_material::lights_cod4_grid(&ent);
    let phong = ent.phong.filter(|_| lit);
    d.light = if lit { "model light" } else { "none" };
    d.normal_map = phong.and_then(|p| p.cod4).is_some_and(|c| c.normal_map);
    d.specular = phong.is_some_and(|p| p.specular);
    // the specular adds the entity's probe (cod4-world.md 3.3)
    d.reflection = d.specular;
    d.detail = phong.is_some_and(|p| p.detail);
    Some(d)
}

/// Retail's draw of an effect material, as far as [`fx_draw`] compares.
fn fx_retail(m: &MapMaterial) -> Option<Draw> {
    let d = retail(m, false)?;
    Some(Draw {
        blend: d.blend,
        alpha_test: d.alpha_test,
        fog: d.fog,
        feather: d.feather,
        ..fx_facts()
    })
}

/// `common_mp`'s weapon and effect materials (cod4-client-view.md 4.5,
/// cod4-effects.md 13): every material on a weapon's gun, world, hands
/// and knife models through the pipelines the view model and the
/// entities draw with, and every effect element's through the fx pass, against what
/// its state bits and pixel shader give. A difference fails unless
/// [`ALLOWED`] (weapons) or [`FX_ALLOWED`] (effects) names its set.
/// Needs `COD4_DIR`.
#[test]
fn cod4_weapon_and_effect_materials_draw_as_retail() {
    use vcod_cod4::assets::fx::FxVisual;
    use vcod_cod4::walk::{WalkOptions, walk};
    let Some(install) = vcod_cod4::testing::cod4_install() else {
        return;
    };
    let z = vcod_cod4::zone::Zone::open_path(&install.zone("english", "common_mp")).unwrap();
    let a = walk(z, WalkOptions { keep_bulk: false }).unwrap().assets;
    let fs = crate::cod4_fly::open_fs(install.root(), None, None).unwrap();
    let mut weapon = std::collections::BTreeSet::new();
    // by index: `Id` has no order
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
            weapon.extend(
                a.xmodels[id.index()]
                    .materials
                    .iter()
                    .flatten()
                    .map(|m| m.index()),
            );
        }
    }
    let mut effect = std::collections::BTreeSet::new();
    for e in a.fx.iter().flat_map(|f| &f.elems) {
        for v in &e.visuals {
            if let FxVisual::Material(Some(id)) = v {
                effect.insert(id.index());
            }
        }
    }
    type Key<'a> = (&'a str, String, Option<Draw>, Option<Draw>);
    let mut rows: BTreeMap<Key, Vec<String>> = BTreeMap::new();
    for (kind, ids) in [("weapon", &weapon), ("effect", &effect)] {
        for id in ids {
            let m = vcod_cod4::map::map_material(&a, vcod_cod4::assets::Id::new(*id));
            let sh = vcod_cod4::materials::compile(&m);
            let (want, got) = match kind {
                "weapon" => (retail(&m, false), entity_draw(&sh, &fs)),
                _ => (fx_retail(&m), fx_draw(&sh)),
            };
            let set = m.techset.clone().unwrap_or_else(|| "-".into());
            rows.entry((kind, set, want, got)).or_default().push(m.name);
        }
    }
    let mut used = vec![false; FX_ALLOWED.len()];
    let mut failed = Vec::new();
    for ((kind, set, want, got), names) in &rows {
        let same = want == got;
        let list: Vec<&Allowed> = if *kind == "weapon" {
            ALLOWED.iter().chain(ENTITY_ALLOWED).collect()
        } else {
            FX_ALLOWED.iter().collect()
        };
        let allowed = (!same)
            .then(|| list.iter().position(|(_, f, _)| f(set)))
            .flatten();
        if let (Some(i), "effect") = (allowed, *kind) {
            used[i] = true;
        }
        let mark = match (same, allowed) {
            (true, _) => "same".to_string(),
            (false, Some(i)) => format!("allowed ({})", list[i].0),
            (false, None) => "DIFFERS".to_string(),
        };
        println!(
            "{kind} {set} {} {:?}: {mark}\n  retail {want:?}\n  vcod   {got:?}",
            names.len(),
            &names[..names.len().min(3)]
        );
        if !same && allowed.is_none() {
            failed.push(format!("{kind} {set} {names:?}"));
        }
    }
    println!(
        "{} weapon and {} effect materials",
        weapon.len(),
        effect.len()
    );
    for (i, (name, _, _)) in FX_ALLOWED.iter().enumerate() {
        assert!(used[i], "FX_ALLOWED entry {name} matches nothing");
    }
    assert!(weapon.len() > 150 && effect.len() > 100);
    assert!(failed.is_empty(), "{failed:#?}");
}
