//! Pins what the renderer derives from every stock material: pipeline
//! variants, stage params, fx and dlight draws, sort entries. The digest was
//! taken before the renderer moved from `Shader` to `Material`; a change in
//! it is a change in pixels. `MATERIAL_KEYS_DUMP=<path>` writes the text the
//! digest covers, for a diff against a dump from the base commit.

use super::*;
use crate::entity_material;
use std::fmt::Write as _;

const T: f32 = 1.37;

fn fnv1a(text: &str) -> u64 {
    text.bytes().fold(0xcbf29ce484222325, |h, b| {
        (h ^ u64::from(b)).wrapping_mul(0x100000001b3)
    })
}

fn dump_material(
    out: &mut String,
    sh: &Material,
    blob: Option<&Material>,
    images: &mut ImageNumbers,
) {
    let name = sh.name.clone();
    let _ = writeln!(
        out,
        "== {name} sort {:?} bias {:?} reflects {} sprite {:?} nofog {} lit {}",
        sh.sort,
        sh.depth_bias,
        sh.reflects(),
        sh.sprite,
        sh.nofog,
        entity_material::is_lit(sh),
    );
    let _ = writeln!(
        out,
        " surface {:?} sky {:?} sun {:?} nopicmip {} nomipmaps {} deforms {:?}",
        sh.surface, sh.sky, sh.sunfile, sh.nopicmip, sh.nomipmaps, sh.deforms,
    );
    let mut number = |r: &ImageRef| match r {
        ImageRef::Path(p) => images.number(p),
        ImageRef::Lightmap => images.number("$lightmap"),
        ImageRef::White => images.number("*white"),
    };
    let _ = writeln!(out, "sort entry {:?}", staged_sort_entry(sh, &mut number));
    for idx in 0..sh.stages.len() {
        let st = &sh.stages[idx];
        let _ = writeln!(
            out,
            " stage {idx} facts {:?} animated {} dlight {:?} fx {:?} rgba {:?}",
            stage_draw_facts(sh, idx),
            stage_animated(sh, idx),
            dlight::DlightBlend::of(st),
            fx_stage_draw(sh, st),
            fx_stage_rgba(Some(st), [0.3, 0.6, 0.9, 0.5], 0.5, T),
        );
        let _ = writeln!(out, "  p0 {:?}", stage_params(sh, idx, 0.0));
        let _ = writeln!(out, "  pt {:?}", stage_params(sh, idx, T));
    }
    for d in fx_stage_draws(Some(sh), &name, T) {
        let _ = writeln!(
            out,
            " fxdraw {} {:?} {} {:?}",
            d.image, d.blend, d.flags, d.uv
        );
    }
    for (i, dl) in dlight::stage_shaders(sh, blob).iter().enumerate() {
        let _ = writeln!(
            out,
            " dl {i} {:?} {:?} {:?}",
            dl.stages.first().and_then(dlight::DlightBlend::of),
            stage_draw_facts(dl, 0),
            stage_params(dl, 0, T),
        );
    }
}

#[test]
fn stock_material_pipeline_keys_hold() {
    let Some(fs) = vcod_common::testing::stock_fs_11() else {
        return;
    };
    let shaders = ShaderLib::load(&fs);
    let lib = shaders.materials();
    let blob = lib.get("dlightshader");
    let mut names: Vec<&str> = lib.names().collect();
    names.sort_unstable();
    let mut out = String::new();
    let mut images = ImageNumbers::default();
    for name in &names {
        let sh = lib.get(name).unwrap();
        let _ = writeln!(
            out,
            "{name}: additive {} offset {}",
            shaders.is_additive(name),
            shaders.uses_polygon_offset(name)
        );
        dump_material(&mut out, sh, blob, &mut images);
    }
    let mut stypes = fs.names_with_suffix(".stype");
    stypes.sort_unstable();
    let mut warns = vcod_common::shader::WarnSet::new();
    for path in &stypes {
        let kind = path.rsplit('/').next().unwrap().trim_end_matches(".stype");
        let text = String::from_utf8_lossy(&fs.read(path).unwrap()).into_owned();
        if let Some(sh) = entity_material::from_stype(&format!("{kind}@x"), &text, &mut warns) {
            dump_material(&mut out, &Material::from_shader(&sh), blob, &mut images);
        }
    }
    dump_material(&mut out, &entity_material::implicit("x"), blob, &mut images);
    dump_material(
        &mut out,
        &vcod_common::shader::default_material(),
        blob,
        &mut images,
    );
    if let Ok(path) = std::env::var("MATERIAL_KEYS_DUMP") {
        std::fs::write(path, &out).unwrap();
    }
    assert_eq!(
        (names.len(), stypes.len(), fnv1a(&out)),
        (582, 129, 3180396038756088198),
        "a stock material draws differently"
    );
}
