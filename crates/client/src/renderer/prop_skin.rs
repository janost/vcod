//! How a static prop's or model's skin draws: its material, its two-sidedness,
//! its colour map and the images it may load.

use super::*;

/// How a static prop's skin draws when it has no `skins/` block, by its
/// `type@` template as the entity path resolves it (shader doc 8.2).
pub(super) enum PropSkinDraw {
    /// More than one stage, or a blend: the stage path, `$texturename`
    /// bound to the skin.
    Stages(Box<Material>),
    /// One stage that alpha-tests: `fs_prop`'s cut.
    Cut,
    /// One opaque stage, or the implicit skin: nothing reads the alpha.
    Opaque,
}

pub(super) fn prop_skin_draw(fs: &dyn AssetSource, skin: &str) -> PropSkinDraw {
    let Some(sh) =
        crate::entity_material::template(fs, skin, &mut vcod_common::shader::WarnSet::new())
    else {
        // a typed skin retail cannot build draws its default shader
        return if crate::entity_material::is_typed_skin(skin) {
            PropSkinDraw::Stages(Box::new(vcod_common::shader::default_material()))
        } else {
            PropSkinDraw::Opaque
        };
    };
    if sh.stages.len() > 1 || sh.stages[0].blend.is_some() {
        return PropSkinDraw::Stages(Box::new(Material::from_shader(
            &crate::entity_material::bind_texture_name(sh, &format!("skins/{skin}")),
        )));
    }
    if sh.stages[0].alpha_func.is_some() {
        PropSkinDraw::Cut
    } else {
        PropSkinDraw::Opaque
    }
}

/// Whether a static prop's skin draws both sides: its `type@` template
/// says `cull none` (UO's `*_cullnone` types). The implicit skin and every
/// other template keep `R_FindShader`'s zeroed cull type, which `GL_Cull`
/// (CoDMP.exe 0x4d5670) draws one-sided (cod11-client-perf.md 5).
pub(super) fn prop_skin_two_sided(fs: &dyn AssetSource, skin: &str) -> bool {
    crate::entity_material::template(fs, skin, &mut vcod_common::shader::WarnSet::new())
        .is_some_and(|sh| sh.two_sided)
}

/// The material a prop skin draws with: CoD1's `skins/<name>` script
/// block, or the CoD2 material the model names.
pub(super) fn prop_material<'a>(
    lib: &'a MaterialLib,
    cod2: bool,
    skin: &str,
) -> Option<&'a Material> {
    if cod2 {
        lib.get(skin)
    } else {
        props::skin_script(skin).and_then(|n| lib.get(&n))
    }
}

/// Whether a CoD2 static prop of `m` draws per pixel through its
/// `lprobe_smc*` technique: every stage in a variant the lit pipeline
/// takes (`cod2::draws_smodel`).
pub(super) fn cod2_smodel(m: &Material) -> bool {
    !m.stages.is_empty()
        && (0..m.stages.len())
            .all(|si| stage_draw_facts(m, si).is_some_and(|(v, ..)| cod2::draws_smodel(m, v)))
}

/// Whether a CoD2 model material draws its model's vertex colours unlit:
/// one with stages and no light probe technique (`vertcol_mul` decals,
/// `effect_add` glows, the unlit and emissive sets).
pub(super) fn cod2_model_colour(m: &Material) -> bool {
    !m.stages.is_empty() && m.model_light == ModelLight::None
}

/// The image a model surface's `$texturename` binds: its skin file, or on
/// CoD2 its material's colour map (the stages bind it by path already).
pub(super) fn model_skin_image(
    fs: &Pk3Fs,
    lib: &MaterialLib,
    decoded: &assets::DecodedImages,
    skin: &str,
) -> Image {
    if fs.game() == vcod_common::game::Game::Cod2 {
        colour_map_image(fs, lib.get(skin), decoded)
    } else {
        assets::load_skin_image(fs, skin)
    }
}

/// Every image path `materials` may load: their textures as named and
/// their stages' bundle images as [`upload_bundle_view`] resolves them.
pub(super) fn prefetch_paths<'m>(
    fs: &dyn AssetSource,
    materials: impl Iterator<Item = &'m Material>,
) -> Vec<String> {
    let mut out = Vec::new();
    let bundle = |p: &str, out: &mut Vec<String>| {
        if let Some(full) = assets::resolve_bundle_image(fs, p) {
            out.push(full);
        }
    };
    for m in materials {
        for t in &m.textures {
            if let ImageRef::Path(p) = &t.image {
                out.push(p.clone());
                bundle(p, &mut out);
            }
        }
        for b in m.stages.iter().flat_map(|st| st.bundles.iter()) {
            if let ImageRef::Path(p) = &b.image {
                bundle(p, &mut out);
            }
            for p in b.anim.iter().flat_map(|a| a.paths.iter()) {
                bundle(p, &mut out);
            }
        }
    }
    out
}

/// A CoD2 material's colour map: the image its base draw and cutout test
/// read, the default image without one.
pub(super) fn colour_map_image(
    fs: &Pk3Fs,
    material: Option<&Material>,
    prefetched: &assets::DecodedImages,
) -> Image {
    match material.and_then(|m| m.texture(TexSemantic::Color)) {
        Some(ImageRef::Path(path)) => prefetched.load_path_image(fs, path),
        _ => {
            log::debug!(
                "cod2: {} has no colour map",
                material.map_or("?", |m| &m.name)
            );
            assets::default_image()
        }
    }
}
