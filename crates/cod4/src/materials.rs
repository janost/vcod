//! A map's materials as the renderer's [`Material`]s, the way the CoD2 port
//! first drew its own: one stage, the colour map (times the lightmap on a
//! lit world set), under the state the lit technique's `GfxStateBits`
//! give ([`crate::state`]). A lit world set that replaces, alpha tests or
//! blends also carries [`PhongLit`], so the CoD2 lit pipeline draws it
//! over the pages [`crate::mapworld`] builds, with its normal, specular and
//! detail maps; a lit model set (`mc_l_*`, `mc_ambient_t0c0`) the same
//! off the model lighting volume. `docs/research/cod4-world.md` sections
//! 3.2 and 9.

use crate::map::{Map, MapMaterial, sampler_hash};
use crate::state::{AlphaTest, Cull as GfxCull, DepthTest, Factor, GfxState};
use crate::techset::{Blend, Family};
use vcod_common::material::{
    AlphaFunc, AlphaGen, BlendFactor, Bundle, Clamp, Cod4Light, Cod4Lit, CookieRole, Cull,
    DepthFunc, Falloff, FalloffKind, ImageRef, Material, MaterialLib, ModelLight, PhongLit,
    PolygonOffset, RgbGen, SORT_DECAL, SORT_OPAQUE, SkyParms, Stage, SurfaceBits, TcGen, Technique,
    TexEnv, TexSemantic, TextureBinding,
};

/// `images/<name>.iwi`: where an image without zone pixels lives.
pub fn image_path(name: &str) -> String {
    crate::image::iwi_path(name)
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

/// The stage's blend: none for (One, Zero). A set whose pixel shader
/// premultiplies its colour by its alpha (`TechClass::premultiplied`)
/// draws a source factor of One as SrcAlpha, the colour vcod's stage
/// writes not being premultiplied: the `b0` sets' (One, InvSrcAlpha) and
/// the add sets' (One, One) (cod4-world.md 8.1).
fn blend_of(st: &GfxState, premultiplied: bool) -> Option<(BlendFactor, BlendFactor)> {
    let (s, d, _) = st.blend?;
    match (s, d) {
        (Factor::One, Factor::Zero) => None,
        (Factor::One, d) if premultiplied => Some((BlendFactor::SrcAlpha, factor(d))),
        (s, d) => Some((factor(s), factor(d))),
    }
}

/// The sort a CoD1 sort value keeps CoD4's `sortKey` order with, as the
/// CoD2 port's: unblended opaque, blended keys under the decals' sort.
fn sort_of(sort_key: u8, blended: bool) -> f32 {
    let k = f32::from(sort_key);
    match (blended, sort_key) {
        (false, _) => SORT_OPAQUE,
        (true, 0..=16) => SORT_DECAL - (16.0 - k) * 0.01,
        (true, _) => 15.0 + (k - 17.0) * 0.1,
    }
}

fn bundle(image: ImageRef) -> Bundle {
    Bundle {
        image,
        anim: None,
        clamp: Clamp::NONE,
        tcmods: Vec::new(),
        tc_gen: TcGen::Base,
        env: TexEnv::Modulate,
    }
}

fn semantic(sampler: Option<&str>) -> Option<TexSemantic> {
    Some(match sampler? {
        "colorMap" => TexSemantic::Color,
        "detailMap" => TexSemantic::Detail,
        "normalMap" => TexSemantic::Normal,
        "specularMap" => TexSemantic::Specular,
        _ => return None,
    })
}

/// The image a stage draws: `colorMap`, else the material's first image.
pub fn color_image(m: &MapMaterial) -> ImageRef {
    m.textures
        .iter()
        .find(|t| t.sampler == Some("colorMap") && t.image.is_some())
        .or_else(|| m.textures.iter().find(|t| t.image.is_some()))
        .and_then(|t| t.image.as_deref())
        .map_or(ImageRef::White, |n| ImageRef::Path(image_path(n)))
}

/// `m` as the renderer draws it.
pub fn compile(m: &MapMaterial) -> Material {
    let class = m.class;
    let family = class.map_or(Family::Other, |c| c.family);
    let st = m.draw_state;
    let mut out = Material {
        name: m.name.clone(),
        technique: Technique::None,
        sort: SORT_OPAQUE,
        surface: SurfaceBits {
            sky: family == Family::Sky,
            // A world distortion set (`wc/office_brickglass`) refracts
            // the frame's copy as the effects' own sets do: the client's
            // `cod4_worldfx` draws it from `Material::distortion`
            // (cod4-world.md 9.5, cod4-effects.md 17).
            nodraw: family == Family::Invisible || st.is_none(),
            trans: false,
            water: family == Family::Water,
            nonsolid: false,
            nolightmap: false,
            nodlight: false,
        },
        cull: Cull::Back,
        depth_bias: None,
        nofog: false,
        nopicmip: false,
        nomipmaps: false,
        sky: None,
        sunfile: None,
        deforms: Vec::new(),
        sprite: None,
        textures: m
            .textures
            .iter()
            .filter_map(|t| {
                Some(TextureBinding {
                    semantic: semantic(t.sampler)?,
                    image: ImageRef::Path(image_path(t.image.as_deref()?)),
                })
            })
            .collect(),
        stages: Vec::new(),
        dlight: None,
        sort_images: None,
        dropped_stages: 0,
        model_axes: None,
        phong: None,
        vertex_multiply: false,
        falloff: None,
        premultiplied: false,
        eye_offset: None,
        reflex: None,
        model_light: ModelLight::None,
        probe: None,
        smodel: None,
        feather: None,
        feather_view_depth: false,
        // `floatz_build_atest` kills by the colour map's alpha as the lit
        // state's test does
        floatz: m
            .order
            .draws_floatz()
            .then(|| m.draw_state.is_some_and(|d| d.alpha_test.is_some())),
        entity_cull: false,
        distortion: None,
        cookie: CookieRole {
            sun_caster: m.caster_state.is_some(),
            ..CookieRole::default()
        },
        water: None,
    };
    let Some(st) = st.filter(|_| !out.surface.nodraw) else {
        return out;
    };
    if family == Family::Sky {
        // `sky.hlsl` samples the cube along the view direction.
        out.technique = Technique::Sky;
        out.nofog = true;
        out.sort = 2.0;
        out.sky = Some(SkyParms {
            env: match color_image(m) {
                ImageRef::Path(p) => p,
                _ => String::new(),
            },
            cloud_height: 512.0,
        });
        return out;
    }
    let lit_world = family == Family::Lit && class.is_some_and(|c| c.world);
    let reflex = family == Family::ReflexSight;
    let premultiplied = class.is_some_and(|c| c.premultiplied);
    let blend = blend_of(&st, premultiplied);
    let mut bundles = vec![bundle(color_image(m))];
    if lit_world {
        bundles.push(bundle(ImageRef::Lightmap));
        out.textures.push(TextureBinding {
            semantic: TexSemantic::Lightmap,
            image: ImageRef::Lightmap,
        });
    }
    let multiply = class.is_some_and(|c| c.blend == Blend::Multiply);
    let stage = Stage {
        bundles,
        blend: blend.clone(),
        depth_write: Some(st.depth_write && st.depth_test != DepthTest::Disabled),
        alpha_func: st.alpha_test.map(|a| match a {
            AlphaTest::Gt0 => AlphaFunc::Gt0,
            AlphaTest::Lt128 => AlphaFunc::Lt128,
            AlphaTest::Ge128 => AlphaFunc::Ge128,
        }),
        depth_func: match st.depth_test {
            DepthTest::Equal => DepthFunc::Equal,
            DepthTest::Always | DepthTest::Disabled => DepthFunc::Always,
            DepthTest::Less | DepthTest::LessEqual => DepthFunc::LessEqual,
        },
        rgb_gen: RgbGen::ExactVertex,
        alpha_gen: AlphaGen::Vertex,
    };
    out.technique = Technique::Stages;
    out.sort = sort_of(m.sort_key, blend.is_some());
    out.cull = match st.cull {
        GfxCull::Back => Cull::Back,
        GfxCull::None | GfxCull::Front => Cull::None,
    };
    out.entity_cull = st.cull == GfxCull::Back;
    out.depth_bias = match st.polygon_offset {
        0 => None,
        1 => Some(PolygonOffset::Single),
        _ => Some(PolygonOffset::Double),
    };
    // `mul.hlsl`, the multiply decals' shader as on CoD2.
    out.vertex_multiply = multiply && blend.is_some();
    if reflex {
        out.reflex = Some(constant(m, "detailScale").map_or([1.0, 1.0], |c| [c[0], c[1]]));
    }
    if let Some(c) = class {
        if c.dist_falloff {
            out.falloff = falloff(m, FalloffKind::Distance);
        } else if c.angle_falloff {
            out.falloff = falloff(m, FalloffKind::Angle);
        }
        out.nofog = c.nofog;
        if c.eye_offset {
            out.eye_offset = constant(m, "eyeOffsetParms").map(|p| p[0]);
        }
        // `zfeather.hlsl` fades by `featherParms.x` times the view depth
        // and its gap to the scene's (cod4-effects.md 14)
        // `distortion_zfeather.hlsl` takes no fade: its `floatZ` keeps a
        // nearer surface out of the bend
        if family == Family::Distortion {
            out.distortion =
                Some(constant(m, "distortionScale").map_or([0.0; 2], |p| [p[0], p[1]]));
            // sortKey 0, first of the emissive pass, which retail draws
            // after the lit pass and the scene copy: in vcod's one list,
            // first past the decals (cod4-effects.md 17)
            out.sort = SORT_DECAL + 0.001;
        } else if c.zfeather {
            out.feather = Some(constant(m, "featherParms").map_or(0.0, |p| p[0]));
            out.feather_view_depth = true;
        }
    }
    // only (InvDestColor, One) is left needing it: blend_of moved the rest
    out.premultiplied = premultiplied;
    let lit_blend = matches!(
        class.map(|c| c.blend),
        Some(Blend::Replace | Blend::AlphaTest | Blend::Blend)
    );
    let set = m.techset.as_deref().unwrap_or("").trim_start_matches(',');
    let light = if lit_world {
        Some(Cod4Light::Lightmap)
    } else if family == Family::Lit {
        Some(Cod4Light::Grid)
    } else if set == "mc_ambient_t0c0" {
        Some(Cod4Light::GridAmbient)
    } else if family == Family::Water {
        Some(Cod4Light::Water)
    } else {
        None
    };
    let lit_blend = lit_blend || matches!(light, Some(Cod4Light::GridAmbient | Cod4Light::Water));
    if let Some(light) = light.filter(|_| lit_blend) {
        let water = light == Cod4Light::Water;
        // water_l_sun reads its normal map and the probe, and writes the
        // vertex alpha
        let inputs = match water {
            true => LitInputs {
                normal: true,
                specular: true,
                detail: false,
            },
            false => lit_inputs(set),
        };
        let has = |sampler| m.textures.iter().any(|t| t.sampler == Some(sampler));
        out.phong = Some(PhongLit {
            specular: inputs.specular && (water || has("specularMap")),
            // `(detail + colour - 0.5) * vertex`, as CoD2's `_dtl`
            detail: inputs.detail && has("detailMap"),
            alpha: water || st.alpha_test.is_some() || blend.is_some(),
            detail_scale: constant(m, "detailScale").map_or([1.0, 1.0], |c| [c[0], c[1]]),
            cod4: Some(Cod4Lit {
                normal_map: inputs.normal && has("normalMap"),
                // `envMapParms` is set on every `s0` material the census
                // reads; this default only keeps a stub drawable
                env: constant(m, "envMapParms").unwrap_or([0.0, 1.0, 5.0, 1.0]),
                light,
                water_color: constant(m, "waterColor").unwrap_or([0.0; 4]),
                flag: class.is_some_and(|c| c.flag),
                sun_shadow: class.is_some_and(|c| c.sun_shadow),
            }),
        });
    }
    out.sort_images = Some(stage.bundles.iter().map(|b| b.image.clone()).collect());
    out.stages = vec![stage];
    out
}

/// What a lit set's name says its shaders read past the colour map: the
/// last part's two-letter codes, `n0` a normal map, `s0` a specular map
/// (and the reflection probe), `d0` a detail map (`wc_l_sm_r0c0n0s0`;
/// cod4-world.md 4).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LitInputs {
    pub normal: bool,
    pub specular: bool,
    pub detail: bool,
}

pub fn lit_inputs(techset: &str) -> LitInputs {
    let last = techset.rsplit('_').next().unwrap_or("").as_bytes();
    let mut out = LitInputs::default();
    for code in last.chunks(2) {
        match code {
            b"n0" => out.normal = true,
            b"s0" => out.specular = true,
            b"d0" => out.detail = true,
            _ => {}
        }
    }
    out
}

/// The material constant `name` (its `R_HashString` hash).
fn constant(m: &MapMaterial, name: &str) -> Option<[f32; 4]> {
    let h = sampler_hash(name);
    m.constants.iter().find(|c| c.0 == h).map(|c| c.1)
}

/// A falloff material's `falloffBeginColor`, `falloffEndColor` and
/// `falloffParms`: x scale and y bias for the distance, z and w for the
/// angle; `None` when one is missing.
fn falloff(m: &MapMaterial, kind: FalloffKind) -> Option<Falloff> {
    let (begin, end, parms) = (
        constant(m, "falloffBeginColor")?,
        constant(m, "falloffEndColor")?,
        constant(m, "falloffParms")?,
    );
    let (scale, bias) = match kind {
        FalloffKind::Distance => (parms[0], parms[1]),
        FalloffKind::Angle => (parms[2], parms[3]),
    };
    Some(Falloff {
        begin: [begin[0], begin[1], begin[2]],
        end: [end[0], end[1], end[2]],
        scale,
        bias,
        kind,
    })
}

/// Every material of `map` compiled, keyed as the renderer looks them up.
pub fn material_lib(map: &Map) -> MaterialLib {
    MaterialLib::from_materials(
        map.materials.iter().map(|m| {
            let first = match color_image(m) {
                ImageRef::Path(p) => Some(p),
                _ => None,
            };
            (compile(m), first)
        }),
        0,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::map::MapTexture;
    use crate::techset::classify;

    fn material(techset: &str, bits: [u32; 2]) -> MapMaterial {
        let mut entries = vec![255u8; 34];
        entries[7] = 0;
        MapMaterial {
            name: "wc/test".into(),
            techset: Some(techset.into()),
            class: Some(classify(techset)),
            sort_key: 4,
            textures: vec![MapTexture {
                semantic: 2,
                name_hash: 0,
                sampler: Some("colorMap"),
                image: Some("test_col".into()),
            }],
            state_bits: vec![bits],
            state_bits_entry: entries.clone(),
            draw_state: crate::state::draw_state(&entries, &[bits]),
            caster_state: None,
            constants: Vec::new(),
            water: None,
            order: Default::default(),
        }
    }

    /// The last part of a lit set's name: `r0c0d0n0s0` reads every map,
    /// `t0c0` none past the colour, and the `_sm`/`_hsm` parts are not it.
    #[test]
    fn lit_inputs_read_the_name_codes() {
        let all = lit_inputs("wc_l_sm_r0c0d0n0s0");
        assert!(all.normal && all.specular && all.detail);
        assert_eq!(lit_inputs("wc_l_hsm_t0c0"), LitInputs::default());
        let n = lit_inputs("mc_l_sm_b0c0n0");
        assert!(n.normal && !n.specular && !n.detail);
    }

    #[test]
    fn a_lit_world_set_draws_phong_over_its_colour_map() {
        let m = compile(&material("wc_l_sm_r0c0n0s0", [0x1812_8812, 0xD]));
        assert!(m.phong.is_some());
        assert_eq!(m.stages.len(), 1);
        assert_eq!(m.stages[0].blend, None);
        assert_eq!(
            m.stages[0].bundles[0].image,
            ImageRef::Path("images/test_col.iwi".into())
        );
        assert_eq!(m.stages[0].bundles[1].image, ImageRef::Lightmap);
        let t = compile(&material("wc_l_sm_t0c0", [0x1812_B012, 0xD]));
        assert_eq!(t.stages[0].alpha_func, Some(AlphaFunc::Ge128));
        assert!(t.phong.unwrap().alpha);
    }

    #[test]
    fn decals_multiply_and_tools_draw_nothing() {
        let m = compile(&material("wc_unlit_multiply", [0x1928_8931, 0xC]));
        assert_eq!(
            m.stages[0].blend,
            Some((BlendFactor::Zero, BlendFactor::SrcColor))
        );
        assert!(m.vertex_multiply && m.phong.is_none());
        assert_eq!(m.stages[0].depth_write, Some(false));
        let t = compile(&material("wc_tools", [0x1812_8812, 0xD]));
        assert!(t.surface.nodraw && t.stages.is_empty());
        let s = compile(&material("wc_sky", [0x1812_8812, 0xC]));
        assert_eq!(s.technique, Technique::Sky);
    }

    /// `mc/mtl_weapon_reflex_red_dot` (common_mp): (One, InvSrcAlpha)
    /// drawn as an alpha blend, its `detailScale` carried for the dot.
    #[test]
    fn a_reflex_sight_carries_its_detail_scale() {
        let mut m = material("mc_reflexsight", [0x1928_8962, 0xC]);
        m.constants = vec![(sampler_hash("detailScale"), [25.0, 25.0, 0.0, 0.0])];
        let c = compile(&m);
        assert_eq!(c.reflex, Some([25.0, 25.0]));
        assert_eq!(
            c.stages[0].blend,
            Some((BlendFactor::SrcAlpha, BlendFactor::OneMinusSrcAlpha))
        );
        assert_eq!(
            compile(&material("mc_l_sm_r0c0s0", [0x1812_8812, 0xD])).reflex,
            None
        );
    }

    #[test]
    fn a_light_shaft_screens_by_its_angle_falloff() {
        // wc/ch_godray01's state and constants on mp_crash
        let mut m = material("wc_unlit_falloff_add", [0x1812_492A, 0xC]);
        m.constants = vec![
            (sampler_hash("falloffBeginColor"), [1.0; 4]),
            (sampler_hash("falloffEndColor"), [0.5; 4]),
            (sampler_hash("falloffParms"), [0.0053, -0.053, 2.13, -0.064]),
        ];
        let c = compile(&m);
        assert_eq!(
            c.stages[0].blend,
            Some((BlendFactor::OneMinusDstColor, BlendFactor::One))
        );
        assert!(c.premultiplied && c.cull == Cull::None);
        let f = c.falloff.unwrap();
        assert_eq!(
            (f.scale, f.bias, f.kind),
            (2.13, -0.064, FalloffKind::Angle)
        );
        // the add sets premultiply: (One, One) draws as (SrcAlpha, One)
        let a = compile(&material("wc_unlit_add", [0x1812_8922, 0xC]));
        assert_eq!(
            a.stages[0].blend,
            Some((BlendFactor::SrcAlpha, BlendFactor::One))
        );
        // an effect set's eye offset and fog
        let mut e = material("mc_effect_falloff_add_nofog_eyeoffset", [0x1812_8922, 0xC]);
        e.constants = vec![(sampler_hash("eyeOffsetParms"), [15.0, 0.0, 0.0, 0.0])];
        let e = compile(&e);
        assert_eq!(e.eye_offset, Some(15.0));
        assert!(e.nofog);
        assert_eq!(e.feather, None);
        // a zfeather set fades by its featherParms.x on the view depth
        let mut z = material("effect_zfeather", [0x1812_8922, 0xC]);
        z.constants = vec![(sampler_hash("featherParms"), [0.05, 0.0, 0.0, 0.0])];
        let z = compile(&z);
        assert_eq!(z.feather, Some(0.05));
        assert!(z.feather_view_depth);
        // the world's distortion set draws its normal map opaque, bent
        // by its distortionScale past the split
        let mut d = material("wc_distortion_scale_zfeather", [0x1812_8812, 0xD]);
        d.constants = vec![(sampler_hash("distortionScale"), [8.0, 8.0, 0.0, 0.0])];
        let d = compile(&d);
        assert!(!d.surface.nodraw && d.stages.len() == 1);
        assert!(d.stages[0].blend.is_none());
        assert_eq!(d.distortion, Some([8.0, 8.0]));
        assert!(d.sort > SORT_DECAL);
        // an effect set draws its offset map with its distortionScale,
        // the constant common_mp's heat haze names
        assert_eq!(sampler_hash("distortionScale"), 0xf37b_6913);
        let mut fx = material("distortion_scale_zfeather", [0x1812_8922, 0xC]);
        fx.constants = vec![(sampler_hash("distortionScale"), [5.0, 6.0, 0.0, 0.0])];
        let fx = compile(&fx);
        assert!(!fx.surface.nodraw && fx.stages.len() == 1);
        assert_eq!(fx.distortion, Some([5.0, 6.0]));
        assert!(fx.sort > SORT_DECAL && fx.sort < vcod_common::material::SORT_BLEND);
        assert_eq!(fx.feather, None);
    }

    #[test]
    fn a_doorway_lightener_doubles_by_its_falloff() {
        // wc/hdrportal_lighten's state and constants on mp_crash
        let mut m = material("wc_unlit_distfalloff", [0x1928_8939, 0xC]);
        m.constants = vec![
            (sampler_hash("falloffBeginColor"), [0.81, 0.81, 0.80, 1.0]),
            (sampler_hash("falloffEndColor"), [0.5, 0.5, 0.5, 0.5]),
            (sampler_hash("falloffParms"), [0.0059, -0.18, 2.03, -0.36]),
        ];
        let c = compile(&m);
        assert_eq!(
            c.stages[0].blend,
            Some((BlendFactor::DstColor, BlendFactor::SrcColor))
        );
        let f = c.falloff.unwrap();
        assert_eq!(
            (f.begin[0], f.end[0], f.scale, f.bias, f.kind),
            (0.81, 0.5, 0.0059, -0.18, FalloffKind::Distance)
        );
        assert!(!c.vertex_multiply);
        // without its constants it has no falloff to draw
        let bare = compile(&material("wc_unlit_distfalloff", [0x1928_8939, 0xC]));
        assert_eq!(bare.falloff, None);
    }
}
