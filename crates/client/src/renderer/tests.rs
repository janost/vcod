use super::*;
use glam::Mat4;
use vcod_common::shader::{
    BlendFactor, Bundle, DepthFunc, PolygonOffset, SORT_ADDITIVE, Stage, TcMod, Wave, WaveForm,
};

#[test]
fn gather_copies_visible_ranges_batch_by_batch() {
    use vcod_common::mesh::IndexRange;
    let src: Vec<u32> = (0..12).collect();
    let ranges = [
        IndexRange {
            batch: 1,
            first: 6,
            count: 3,
        },
        IndexRange {
            batch: 0,
            first: 0,
            count: 3,
        },
        IndexRange {
            batch: 1,
            first: 9,
            count: 3,
        },
    ];
    let mut out = Vec::new();
    let mut scratch = Vec::new();
    let draws = gather(&src, ranges, 3, &mut out, &mut scratch);
    assert_eq!(out, vec![0, 1, 2, 6, 7, 8, 9, 10, 11]);
    assert_eq!(
        draws,
        vec![
            DrawRange {
                batch: 0,
                first: 0,
                count: 3
            },
            DrawRange {
                batch: 1,
                first: 3,
                count: 6
            },
        ]
    );
    let draws = gather(&src, [], 3, &mut out, &mut scratch);
    assert!(draws.is_empty() && out.is_empty());
}

#[test]
fn packs_identity_block_and_bone_bases() {
    let id = Mat4::IDENTITY.to_cols_array();
    let bones_a = vec![Mat4::from_translation(glam::Vec3::X); 3];
    let items = [
        (0usize, id, None),                // static: identity block
        (1, id, Some(bones_a.as_slice())), // skinned, 3 bones
        (0, id, None),
    ];
    let (raw, mats) = pack_instances(&items);
    assert_eq!(raw.len(), 3);
    assert_eq!(raw[0].bone_base, 0);
    assert_eq!(raw[2].bone_base, 0);
    assert_eq!(raw[1].bone_base, MAX_INSTANCE_BONES as u32);
    assert_eq!(mats.len(), MAX_INSTANCE_BONES + 3);
    assert_eq!(mats[0], Mat4::IDENTITY.to_cols_array());
    assert_eq!(mats[MAX_INSTANCE_BONES], bones_a[0].to_cols_array());
}

#[test]
fn hud_colour_scales_rgb_by_identity_light_and_keeps_alpha() {
    assert_eq!(hud_colour([0.0, 1.0, 0.5, 0.6], 0.5), [0.0, 0.5, 0.25, 0.6]);
    assert_eq!(hud_colour([0.15; 4], 1.0), [0.15; 4]);
}

/// Lit gens and implicit materials take identityLight; identity,
/// exactVertex and const draw their colour as it is (0x4ffa60).
#[test]
fn colour_scale_follows_the_first_stage_gen() {
    let with = |rgb_gen: RgbGen| {
        Material::from_shader(&Shader {
            name: "t".into(),
            stages: vec![Stage {
                bundles: vec![],
                blend: None,
                depth_write: None,
                alpha_func: None,
                depth_func: DepthFunc::LessEqual,
                rgb_gen,
                alpha_gen: AlphaGen::Identity,
            }],
            ..Default::default()
        })
    };
    assert_eq!(colour_scale(None, 0.5), 0.5);
    assert_eq!(colour_scale(Some(&with(RgbGen::Vertex)), 0.5), 0.5);
    assert_eq!(colour_scale(Some(&with(RgbGen::ExactVertex)), 0.5), 1.0);
    assert_eq!(colour_scale(Some(&with(RgbGen::Identity)), 0.5), 1.0);
    assert_eq!(
        colour_scale(Some(&with(RgbGen::IdentityLighting)), 1.0),
        1.0
    );
}

/// `body` is a block's inside; newlines end a line as in a script.
fn parse_fx(body: &str) -> Material {
    let text = format!("t {{\n{body}\n}}");
    let (name, toks) = vcod_common::shader::split_blocks(&text).remove(0);
    Material::from_shader(&vcod_common::shader::parse_shader(
        &name,
        &toks,
        &mut vcod_common::shader::WarnSet::new(),
    ))
}

/// The fx pass draws each stage on its own blendFunc and fogs it unless
/// the material is `nofog`; only `GL_SRC_ALPHA GL_ONE` reads the
/// texture's alpha among the additive ones, and an unblended stage is
/// opaque (CoDMP.exe 0x4d57b0, 0x4fbc71).
#[test]
fn fx_stage_draw_follows_the_stage_blend_and_nofog() {
    let draw = |body: &str| {
        let sh = parse_fx(body);
        fx_stage_draw(&sh, &sh.stages[0])
    };
    assert_eq!(
        draw("{ map a.tga blendFunc blend rgbGen vertex }"),
        (FxBlend::Alpha, 0)
    );
    assert_eq!(
        draw("{ map a.tga blendFunc GL_ONE GL_ONE rgbGen vertex }"),
        (FxBlend::Add, 0)
    );
    assert_eq!(
        draw("{ map a.tga blendFunc add rgbGen vertex }"),
        (FxBlend::Add, 0)
    );
    assert_eq!(
        draw("{ map a.tga blendFunc GL_SRC_ALPHA GL_ONE rgbGen vertex }"),
        (FxBlend::Add, FX_VERT_TEX_ALPHA)
    );
    // UO's flamethrowerFire* and flameSmoke*
    assert_eq!(
        draw("nofog { map a.dds blendFunc GL_ONE GL_ONE_MINUS_SRC_COLOR rgbGen vertex }"),
        (FxBlend::Screen, FX_VERT_NOFOG)
    );
    assert_eq!(
        draw(
            "nofog { map a.dds blendFunc GL_SRC_ALPHA GL_ONE_MINUS_SRC_ALPHA \
             alphaGen vertex rgbGen vertex }"
        ),
        (FxBlend::Alpha, FX_VERT_NOFOG)
    );
    // UO's smoke/smk_t_fire_*: additive and nofog
    assert_eq!(
        draw("nofog { map a.dds blendFunc add rgbGen vertex }"),
        (FxBlend::Add, FX_VERT_NOFOG)
    );
    // gfx/impact/bark_gib and the other debris: unblended, cut at 128
    assert_eq!(
        draw("{ map a.tga rgbGen vertex alphaGen vertex alphaFunc GE128 depthWrite }"),
        (FxBlend::Opaque, FX_VERT_ALPHA_GE128)
    );
}

/// The view weapon's light pick reads the same list as every other
/// model's: the cgame's and the entities' lights, capped, and none
/// under `r_dynamiclight 0` (BUG-123).
#[test]
fn the_model_light_pick_takes_both_lists_under_the_cvar() {
    let light = |x: f32| SceneLight {
        origin: glam::Vec3::new(x, 0.0, 0.0),
        color: glam::Vec3::ONE,
        intensity: 1.0,
    };
    let scene = vec![light(1.0)];
    let ents = vec![light(2.0), light(3.0)];
    assert!(model_scene_lights(&scene, &ents, 0).is_empty());
    for mode in [1, 2] {
        let got = model_scene_lights(&scene, &ents, mode);
        assert_eq!(
            got.iter().map(|l| l.origin.x).collect::<Vec<_>>(),
            [1.0, 2.0, 3.0]
        );
    }
    let many = vec![light(0.0); crate::fx::sim::MAX_SCENE_LIGHTS + 5];
    assert_eq!(
        model_scene_lights(&many, &ents, 1).len(),
        crate::fx::sim::MAX_SCENE_LIGHTS
    );
}

/// Every base stage draws, in order, on its own image and tcMods; a
/// `perLight` stage is the dlight pass's and a `$lightmap` bundle 1 is
/// not drawn (CoDMP.exe 0x500b30, 0x4f3699).
#[test]
fn fx_stage_draws_every_base_stage() {
    assert_eq!(fx_stage_draws(None, "gfx/x", 0.0).len(), 1);
    assert_eq!(fx_stage_draws(None, "gfx/x", 0.0)[0].image, "gfx/x");
    let clamp = FX_VERT_CLAMP_S | FX_VERT_CLAMP_T;
    assert_eq!(fx_stage_draws(None, "gfx/x", 0.0)[0].flags, clamp);
    // gfx/impact/bullethit_plaster: the perLight overlay stays out
    let hit = parse_fx(
        "{ map gfx/impact/bullethit_plaster blendfunc blend rgbGen vertex \
         nextbundle map $lightmap } \
         { perlight map gfx/impact/bullethit_plaster blendfunc GL_SRC_ALPHA GL_ONE \
         rgbGen vertex nextbundle map $dlight }",
    );
    let draws = fx_stage_draws(Some(&hit), "t", 0.0);
    assert_eq!(draws.len(), 1);
    assert_eq!(draws[0].image, "gfx/impact/bullethit_plaster");
    assert_eq!(draws[0].blend, FxBlend::Alpha);
    // no `clamp`: the image wraps
    assert_eq!(draws[0].flags, 0);
    // gfx/effects/smoke/roller: two blended stages, each turning
    let roller = parse_fx(
        "sort additive \
         { map clamp /gfx/effects/smoke/roller blendfunc blend rgbGen vertex tcMod rotate -90 } \
         { map clamp /gfx/effects/smoke/smoke_frnt blendfunc blend rgbGen vertex tcMod rotate 80 }",
    );
    let draws = fx_stage_draws(Some(&roller), "t", 0.25);
    assert_eq!(draws.len(), 2);
    assert_eq!(draws[0].image, "gfx/effects/smoke/roller");
    assert_eq!(draws[1].image, "gfx/effects/smoke/smoke_frnt");
    assert_eq!(draws[0].flags, clamp);
    assert_eq!(draws[0].uv, bundle_affine(&[TcMod::Rotate(-90.0)], 0.25));
    assert_eq!(draws[1].uv, bundle_affine(&[TcMod::Rotate(80.0)], 0.25));
    // UO's impact/bullet_fabric_a: a blended stage and an additive glow
    let fabric = parse_fx(
        "{ map a.dds blendfunc blend rgbGen vertex nextbundle map $lightmap } \
         { map glow.dds blendFunc add rgbGen wave sin .90 .25 .5 .5 } \
         { perlight map a.dds blendfunc blend rgbGen identity nextbundle map $dlight }",
    );
    let blends: Vec<FxBlend> = fx_stage_draws(Some(&fabric), "t", 0.0)
        .iter()
        .map(|d| d.blend)
        .collect();
    assert_eq!(blends, [FxBlend::Alpha, FxBlend::Add]);
    // gfx/effects/animated/animfire: the frame at the shader time
    let fire =
        parse_fx("{\nanimMap 4\nmap a.tga\nmap b.tga\nmap c.tga\nblendFunc add\nrgbGen vertex\n}");
    let image = |t| fx_stage_draws(Some(&fire), "t", t)[0].image;
    assert_eq!(image(0.0), "a.tga");
    assert_eq!(image(0.5), "c.tga");
    assert_eq!(image(0.8), "a.tga");
}

/// A stage's colour follows its gens, not the sprite's colour alone
/// (CoDMP.exe 0x4ffa60): identity is opaque white, vertex the sprite's
/// colour times identityLight with its alpha, a wave ignores the sprite.
#[test]
fn fx_stage_rgba_follows_the_stage_gens() {
    let v = [0.8, 0.6, 0.4, 0.5];
    let rgba = |body: &str| {
        let sh = parse_fx(body);
        fx_stage_rgba(Some(&sh.stages[0]), v, 0.5, 0.0)
    };
    assert_eq!(fx_stage_rgba(None, v, 0.5, 0.0), [0.4, 0.3, 0.2, 0.5]);
    assert_eq!(
        rgba("{ map a.tga blendfunc blend rgbGen vertex }"),
        [0.4, 0.3, 0.2, 0.5]
    );
    assert_eq!(
        rgba("{ map a.tga blendfunc blend rgbGen exactVertex }"),
        [0.8, 0.6, 0.4, 0.5]
    );
    // UO's impact/bullet_sand_a: the sprite's fade never reaches it
    assert_eq!(
        rgba("{ map a.dds blendfunc blend rgbgen identity }"),
        [1.0, 1.0, 1.0, 1.0]
    );
    assert_eq!(
        rgba("{ map a.tga blendfunc blend rgbGen identityLighting }"),
        [0.5, 0.5, 0.5, 1.0]
    );
    assert_eq!(
        rgba("{ map a.tga blendfunc blend rgbGen vertex alphaGen const 0.25 }"),
        [0.4, 0.3, 0.2, 0.25]
    );
    // an `alphaGen identity` after the rgbGen stays; case 0 drops the
    // copied alpha unless identityLight is 1
    let late = parse_fx("{ map a.tga blendfunc blend rgbGen vertex alphaGen identity }");
    assert_eq!(fx_stage_rgba(Some(&late.stages[0]), v, 0.5, 0.0)[3], 1.0);
    assert_eq!(fx_stage_rgba(Some(&late.stages[0]), v, 1.0, 0.0)[3], 0.5);
    let late = parse_fx("{ map a.tga blendfunc blend rgbGen exactVertex alphaGen identity }");
    assert_eq!(fx_stage_rgba(Some(&late.stages[0]), v, 1.0, 0.0)[3], 1.0);
    // GL_ONE GL_ONE reads no source alpha
    assert_eq!(
        rgba("{ map a.tga blendFunc add rgbGen vertex }"),
        [0.4, 0.3, 0.2, 1.0]
    );
    // a table wave is scaled by identityLight, then clamped; alpha 1
    let sh = parse_fx("{ map a.tga blendFunc add rgbGen wave sin .90 .25 .5 .5 }");
    let RgbGen::Wave(w) = &sh.stages[0].rgb_gen else {
        panic!("wave gen");
    };
    let k = (rgb_wave_value(w, 0.3) * 0.5).clamp(0.0, 1.0);
    assert_eq!(
        fx_stage_rgba(Some(&sh.stages[0]), v, 0.5, 0.3),
        [k, k, k, 1.0]
    );
}

/// Each fx pipeline's colour blend is retail's glBlendFunc for its
/// `FxBlend`; the additive two leave the target's alpha alone.
#[test]
fn fx_blend_states_match_the_gl_pairs() {
    use wgpu::BlendFactor as W;
    let pair = |b: FxBlend| {
        let c = fx_blend_state(b).unwrap().color;
        (c.src_factor, c.dst_factor)
    };
    assert_eq!(pair(FxBlend::Alpha), (W::SrcAlpha, W::OneMinusSrcAlpha));
    assert_eq!(pair(FxBlend::Add), (W::One, W::One));
    assert_eq!(pair(FxBlend::Screen), (W::One, W::OneMinusSrc));
    assert_eq!(fx_blend_state(FxBlend::Opaque), None);
    for b in [FxBlend::Add, FxBlend::Screen] {
        let a = fx_blend_state(b).unwrap().alpha;
        assert_eq!((a.src_factor, a.dst_factor), (W::One, W::One));
    }
}

#[test]
fn resolve_fx_path_drops_empty_and_missing_names_but_keeps_a_known_texture() {
    let Some(fs) = vcod_common::testing::game_fs() else {
        return;
    };
    let no_mapping = HashMap::new();

    // An emitter with no `shaders` key hands over an empty string.
    assert_eq!(resolve_fx_path(&no_mapping, &fs, ""), None);

    assert_eq!(
        resolve_fx_path(&no_mapping, &fs, "fx/does/not/exist.tga"),
        None
    );

    assert_eq!(
        resolve_fx_path(&no_mapping, &fs, "gfx/impact/bullethole1.tga"),
        Some("gfx/impact/bullethole1.tga".to_string())
    );

    // Extensionless material name, as fx::sim hands over.
    assert_eq!(
        resolve_fx_path(&no_mapping, &fs, "gfx/impact/bullethole1"),
        Some("gfx/impact/bullethole1.tga".to_string())
    );

    assert_eq!(resolve_fx_path(&no_mapping, &fs, "fx/does/not/exist"), None);

    let mut mapped = HashMap::new();
    mapped.insert(
        "some_material".to_string(),
        "gfx/impact/bullethole1.tga".to_string(),
    );
    assert_eq!(
        resolve_fx_path(&mapped, &fs, "some_material"),
        Some("gfx/impact/bullethole1.tga".to_string())
    );
}

#[test]
fn resolve_fx_path_resolves_default_hit_efx_shader_names() {
    let Some(fs) = vcod_common::testing::game_fs() else {
        return;
    };
    let no_mapping = HashMap::new();
    for name in [
        "gfx/impact/bullethole1",
        "gfx/impact/dustlayer1",
        "gfx/impact/sparkflash",
        "gfx/impact/stone_piece1",
        "gfx/impact/stone_piece2",
    ] {
        assert!(
            resolve_fx_path(&no_mapping, &fs, name).is_some(),
            "{name} should resolve to a real texture, not drop its quads"
        );
    }
}

/// Weapon `killIcon`s name `.tga` while the art ships as `.dds`.
#[test]
fn resolve_fx_path_falls_back_to_the_real_extension_when_the_named_one_is_missing() {
    let Some(fs) = vcod_common::testing::game_fs() else {
        return;
    };
    let no_mapping = HashMap::new();
    assert_eq!(
        resolve_fx_path(&no_mapping, &fs, "gfx/hud/hud@death_kar98.tga"),
        Some("gfx/hud/hud@death_kar98.dds".to_string())
    );
}

/// `gfx/misc/tracer`'s image lives under a different path and
/// `gfx/effects/whitesmoke`'s is written with a leading slash; both only
/// resolve through the fxshaders material map.
#[test]
fn resolve_fx_path_uses_fxshaders_materials() {
    let Some(fs) = vcod_common::testing::game_fs() else {
        return;
    };
    let shaders = ShaderLib::load(&fs);
    assert_eq!(
        resolve_fx_path(shaders.image_map(), &fs, "gfx/misc/tracer"),
        Some("textures/sfx/tracer.jpg".to_string())
    );
    assert_eq!(
        resolve_fx_path(shaders.image_map(), &fs, "gfx/effects/whitesmoke"),
        Some("gfx/effects/whitesmoke.tga".to_string())
    );
    for name in [
        "gfx/effects/muzflash2",
        "gfx/effects/muzflash2a",
        "gfx/effects/mg42a",
        "gfx/misc/tracer",
    ] {
        assert!(
            resolve_fx_path(shaders.image_map(), &fs, name).is_some(),
            "{name} should resolve"
        );
        assert!(shaders.is_additive(name), "{name} should be additive");
    }
}

#[test]
fn resolve_fx_path_resolves_bc_compressed_effect_textures() {
    let Some(fs) = vcod_common::testing::game_fs() else {
        return;
    };
    let no_mapping = HashMap::new();
    for name in ["gfx/effects/pjsmoke", "gfx/impact/cratered"] {
        let path = resolve_fx_path(&no_mapping, &fs, name);
        assert!(path.is_some(), "{name} should resolve to a real texture");
        let img = vcod_common::assets::load_path_image(&fs, path.as_ref().unwrap());
        assert!(
            matches!(img.data, ImageData::Bc { .. }),
            "{name} should decode as BC-compressed, exercising the upload_image path"
        );
    }
}

/// A CoD2 font's material draws its colour map, not the loose
/// `fonts/gamefonts.tga` beside the font files.
#[test]
fn cod2_hud_names_resolve_through_their_material() {
    let Some(fs) = vcod_common::testing::cod2_fs() else {
        return;
    };
    let lib = material_lib(&fs);
    assert_eq!(
        cod2_hud_image(&fs, &lib, "fonts/gamefonts"),
        Some("images/gamefonts.iwi")
    );
    assert_eq!(cod2_hud_image(&fs, &lib, "fonts/no_such_material"), None);
}

/// `white` and `$whiteimage` are the engine built-in; `black` shares its
/// texture (pak4 `scripts/hud.shader`: both stage `map $whiteimage`,
/// `black` differs only by `rgbGen const 0`, which `set_hud_quads`
/// applies to the quad colour).
#[test]
fn hud_texture_path_resolves_the_white_and_black_built_ins() {
    let Some(fs) = vcod_common::testing::game_fs() else {
        return;
    };
    let no_mapping = HashMap::new();
    assert_eq!(hud_texture_path("white", &no_mapping, &fs), HudTex::White);
    assert_eq!(
        hud_texture_path("$whiteimage", &no_mapping, &fs),
        HudTex::White
    );
    assert_eq!(hud_texture_path("black", &no_mapping, &fs), HudTex::Black);
}

/// UO's health back is `hud@health.dds` in pakuo00; 1.1's pak5
/// `hud@health.tga`, the name hud.menu gives, is a white disc.
#[test]
fn uo_health_back_is_the_uo_dds() {
    let Some(fs) = vcod_common::testing::uo_fs() else {
        return;
    };
    assert_eq!(
        hud_texture_path("gfx/hud/hud@health.tga", &HashMap::new(), &fs),
        HudTex::Path("gfx/hud/hud@health.dds".to_string())
    );
}

/// UO's `nofog` fx materials (`scripts/flamethrower.shader`,
/// `fxshaders/smoke.shader`) draw unfogged, each on its own blend; the
/// impacts draw their base stages, not the `perLight` overlay
/// (`fxshaders/impact.shader`).
#[test]
fn uo_fx_materials_draw_their_base_stages_on_their_blends() {
    let Some(fs) = vcod_common::testing::uo_fs() else {
        return;
    };
    let lib = ShaderLib::load(&fs).materials();
    let alpha = (FxBlend::Alpha, 0);
    for (name, want) in [
        ("flamethrowerFire12", vec![(FxBlend::Screen, FX_VERT_NOFOG)]),
        ("flameSmoke1", vec![(FxBlend::Alpha, FX_VERT_NOFOG)]),
        ("smoke/smk_t_fire_a", vec![(FxBlend::Add, FX_VERT_NOFOG)]),
        ("impact/bullet_dirt_a", vec![alpha]),
        ("impact/bullet_sand_a", vec![alpha]),
        ("impact/bullet_fabric_a", vec![alpha, (FxBlend::Add, 0)]),
    ] {
        let sh = lib.get(name);
        assert!(sh.is_some(), "{name} has no script");
        let got: Vec<(FxBlend, u32)> = fx_stage_draws(sh, name, 0.0)
            .iter()
            .map(|d| (d.blend, d.flags & !(FX_VERT_CLAMP_S | FX_VERT_CLAMP_T)))
            .collect();
        assert_eq!(got, want, "{name}");
    }
    // rgbgen identity: white and opaque whatever the sprite's fade
    let sand = lib.get("impact/bullet_sand_a").unwrap();
    assert_eq!(
        fx_stage_rgba(Some(&sand.stages[0]), [0.5, 0.5, 0.5, 0.2], 0.5, 0.0),
        [1.0; 4]
    );
}

#[test]
fn hud_texture_path_resolves_a_material_a_plain_path_and_a_miss() {
    let Some(fs) = vcod_common::testing::game_fs() else {
        return;
    };
    let shaders = ShaderLib::load(&fs);
    assert_eq!(
        hud_texture_path("hudStanceStand", shaders.image_map(), &fs),
        HudTex::Path("gfx/hud/stance_stand.dds".to_string())
    );
    assert_eq!(
        hud_texture_path("gfx/hud/hud@compassback", &HashMap::new(), &fs),
        HudTex::Path("gfx/hud/hud@compassback.tga".to_string())
    );
    assert_eq!(
        hud_texture_path("gfx/hud/does_not_exist", &HashMap::new(), &fs),
        HudTex::Missing
    );
    // A style 3 window's image falls back to the default shader.
    let style3 = |n: &str| hud_texture_path(&hud::default_if_missing(n), &HashMap::new(), &fs);
    assert_eq!(style3("gfx/hud/does_not_exist"), HudTex::Default);
    assert_eq!(
        style3("gfx/hud/hud@compassback"),
        HudTex::Path("gfx/hud/hud@compassback.tga".to_string())
    );
}

#[test]
fn the_default_shader_is_its_own_hud_texture() {
    let fs = Pk3Fs::empty();
    assert_eq!(
        hud_texture_path(hud::DEFAULT_SHADER, &HashMap::new(), &fs),
        HudTex::Default
    );
    let img = opaque(assets::default_image());
    let ImageData::Rgba8(px) = &img.data else {
        panic!("rgba")
    };
    assert!(px.as_chunks::<4>().0.iter().all(|t| t[3] == 255));
}

#[test]
fn over_cap_bone_set_truncates() {
    let id = Mat4::IDENTITY.to_cols_array();
    let big = vec![Mat4::IDENTITY; MAX_INSTANCE_BONES + 40];
    let (raw, mats) = pack_instances(&[(0, id, Some(big.as_slice()))]);
    assert_eq!(raw[0].bone_base, MAX_INSTANCE_BONES as u32);
    assert_eq!(mats.len(), 2 * MAX_INSTANCE_BONES); // identity block + truncated set
}

// ---- StageParams ----

#[test]
fn stage_params_is_std140_shaped_and_pod_roundtrips() {
    assert_eq!(std::mem::size_of::<StageParams>(), 288);
    let p = StageParams {
        uv0: [1.0, 2.0, 3.0, 4.0, 5.0, 6.0],
        uv1: [7.0, 8.0, 9.0, 10.0, 11.0, 12.0],
        turb01: [13.0, 14.0, 15.0, 16.0],
        tint: [17.0, 18.0, 19.0, 20.0],
        flags: STAGE_FLAG_VERTEX_RGB | STAGE_FLAG_VERTEX_ALPHA,
        uv0_rate: [0.0; 3],
        vec0_s: [1.0, 0.0, 0.0, 0.0],
        vec0_t: [0.0, 1.0, 0.0, 0.0],
        vec1_s: [0.0, 0.0, 1.0, 0.0],
        vec1_t: [0.0, 0.0, 0.0, 1.0],
        eye_off: [9.0, 8.0, 7.0, 0.0],
        deform: [
            [1.0, 2.0, 3.0, 4.0],
            [5.0, 6.0, 7.0, 0.0],
            [1.0, 0.0, 0.0, 0.0],
        ],
        deform_kinds: [DEFORM_WAVE, DEFORM_BULGE, DEFORM_MOVE, 0],
        falloff: [[0.0; 4]; 2],
        fx: [0.0; 4],
    };
    // slot-shaped write/read, like the UBO dynamic-offset path
    let mut slot = [0u8; std::mem::size_of::<StageParams>()];
    slot[..bytemuck::bytes_of(&p).len()].copy_from_slice(bytemuck::bytes_of(&p));
    let q: &StageParams = bytemuck::try_from_bytes(&slot).unwrap();
    assert_eq!(*q, p);
}

/// Sky dome stages ride the same UBO slots with the eye-offset flag set.
/// `tcGen environment` dots the reflection with the drawing frame's y
/// and z, or with a static prop's model axes once the batch carries them.
#[test]
fn stage_params_carry_the_environment_axes() {
    let sh = Material::from_shader(&vcod_common::shader::parse_shader(
        "metal_env",
        &["{", "map", "env.jpg", "tcgen", "environment", "}"],
        &mut vcod_common::shader::WarnSet::new(),
    ));
    let p = stage_params(&sh, 0, 0.0).unwrap();
    assert_eq!(p.flags & STAGE_FLAG_BUNDLE0_ENV, STAGE_FLAG_BUNDLE0_ENV);
    assert_eq!(
        [p.vec0_s, p.vec0_t],
        [[0.0, 1.0, 0.0, 0.0], [0.0, 0.0, 1.0, 0.0]]
    );
    let turned = Material {
        model_axes: Some([[-1.0, 0.0, 0.0], [0.0, 0.0, 1.0]]),
        ..sh
    };
    let p = stage_params(&turned, 0, 0.0).unwrap();
    assert_eq!(
        [p.vec0_s, p.vec0_t],
        [[-1.0, 0.0, 0.0, 0.0], [0.0, 0.0, 1.0, 0.0]]
    );
}

/// A CoD4 reflex sight's stage carries its code and `detailScale` to
/// viewmodel.wgsl, which projects the dot (cod4-client-view.md 4.5).
#[test]
fn stage_params_mark_a_reflex_sight() {
    let sh = Material::from_shader(&vcod_common::shader::parse_shader(
        "mc/mtl_weapon_reflex_red_dot",
        &["{", "map", "dot.jpg", "}"],
        &mut vcod_common::shader::WarnSet::new(),
    ));
    assert_eq!(stage_params(&sh, 0, 0.0).unwrap().deform_kinds[3], 0);
    let dot = Material {
        reflex: Some([25.0, 25.0]),
        ..sh
    };
    let p = stage_params(&dot, 0, 0.0).unwrap();
    assert_eq!(p.deform_kinds[3], DEFORM_COD4_REFLEX);
    assert_eq!(p.eye_off, [25.0, 25.0, 0.0, 0.0]);
}

#[test]
fn stage_params_eye_offset_flag_marks_dome_draws() {
    let sh = Material::from_shader(&Shader {
        name: "test/skydome".into(),
        stages: vec![Stage {
            bundles: vec![Bundle {
                image: ImageRef::Path("clouds".into()),
                anim: None,
                clamp: Clamp::NONE,
                tcmods: vec![TcMod::Scroll(0.004, 0.004), TcMod::Scale(3.0, 3.0)],
                tc_gen: TcGen::Base,
                env: TexEnv::Modulate,
            }],
            blend: Some((BlendFactor::SrcAlpha, BlendFactor::OneMinusSrcAlpha)),
            depth_write: None,
            alpha_func: None,
            depth_func: DepthFunc::LessEqual,
            rgb_gen: RgbGen::Identity,
            alpha_gen: AlphaGen::Identity,
        }],
        ..Default::default()
    });
    let mut p = stage_params(&sh, 0, 5.0).unwrap();
    p.flags |= STAGE_FLAG_EYE_OFFSET;
    p.eye_off = [100.0, 200.0, 300.0, 0.0];
    assert_eq!(p.flags & STAGE_FLAG_EYE_OFFSET, STAGE_FLAG_EYE_OFFSET);
    // the affine still carries scroll+scale for the dome's raw-radian uvs
    assert_eq!(
        p.uv0,
        bundle_affine(&[TcMod::Scroll(0.004, 0.004), TcMod::Scale(3.0, 3.0)], 5.0)
    );
    // world draws never set the flag; the default params stay put
    let world_p = stage_params(&sh, 0, 5.0).unwrap();
    assert_eq!(world_p.flags & STAGE_FLAG_EYE_OFFSET, 0);
    assert_eq!(world_p.eye_off, [0.0; 4]);
}

#[test]
fn stages_past_the_shadow_mask_shade_themselves_unless_they_add_or_filter() {
    use BlendFactor::*;
    let flagged = |blend: Option<(BlendFactor, BlendFactor)>, sort: Option<f32>| {
        let sh = Material::from_shader(&Shader {
            stages: vec![Stage {
                bundles: vec![Bundle {
                    image: ImageRef::Path("a".into()),
                    anim: None,
                    clamp: Clamp::NONE,
                    tcmods: vec![],
                    tc_gen: TcGen::Base,
                    env: TexEnv::Modulate,
                }],
                blend,
                depth_write: None,
                alpha_func: None,
                depth_func: DepthFunc::LessEqual,
                rgb_gen: RgbGen::Identity,
                alpha_gen: AlphaGen::Identity,
            }],
            sort,
            ..Default::default()
        });
        stage_params(&sh, 0, 0.0).unwrap().flags & STAGE_FLAG_SUN_SHADOW != 0
    };
    assert!(flagged(Some((SrcAlpha, OneMinusSrcAlpha)), None));
    assert!(flagged(Some((One, OneMinusSrcAlpha)), None));
    // opaque stages and decals sort before the mask, which darkens them
    assert!(!flagged(None, None));
    assert!(!flagged(
        Some((SrcAlpha, OneMinusSrcAlpha)),
        Some(SORT_DECAL)
    ));
    assert!(flagged(None, Some(vcod_common::shader::SORT_BANNER)));
    // additive light and filters over the darkened frame
    assert!(!flagged(Some((One, One)), None));
    assert!(!flagged(Some((SrcAlpha, One)), None));
    assert!(!flagged(Some((DstColor, Zero)), None));
    assert!(!flagged(Some((Zero, OneMinusSrcColor)), Some(SORT_BLEND)));
}

/// A world stage with `rgbGen vertex` or `exactVertex` and no earlier
/// alphaGen reads the vertex alpha (CoDMP.exe 0x4f8d70): mp_bocage's
/// rockface decals and mp_sicily's shoreline fade by it.
#[test]
fn stage_params_reads_vertex_alpha_under_a_vertex_rgbgen() {
    let flags = |body: &str| stage_params(&parse_fx(body), 0, 0.0).unwrap().flags;
    for body in [
        "{ map a.tga blendFunc blend rgbGen exactVertex }",
        "{ map a.tga alphaFunc GE128 rgbGen vertex }",
        "{ map a.tga blendFunc GL_SRC_ALPHA GL_ONE alphaGen identity rgbGen vertex }",
    ] {
        assert_ne!(flags(body) & STAGE_FLAG_VERTEX_ALPHA, 0, "{body}");
    }
    for body in [
        "{ map a.tga blendFunc blend rgbGen exactVertex alphaGen identity }",
        "{ map a.tga blendFunc blend alphaGen const 0.5 rgbGen vertex }",
        "{ map a.tga blendFunc blend rgbGen identity }",
    ] {
        assert_eq!(flags(body) & STAGE_FLAG_VERTEX_ALPHA, 0, "{body}");
    }
}

#[test]
fn stage_params_encodes_const_gens_lightmap_and_alphafunc() {
    let sh = Material::from_shader(&Shader {
        name: "test/mat".into(),
        stages: vec![Stage {
            bundles: vec![
                Bundle {
                    image: ImageRef::Path("a".into()),
                    anim: None,
                    clamp: Clamp::NONE,
                    tcmods: vec![TcMod::Scroll(0.25, 0.0)],
                    tc_gen: TcGen::Base,
                    env: TexEnv::Modulate,
                },
                Bundle {
                    image: ImageRef::Lightmap,
                    anim: None,
                    clamp: Clamp::NONE,
                    tcmods: vec![],
                    tc_gen: TcGen::Base,
                    env: TexEnv::Modulate,
                },
            ],
            blend: Some((BlendFactor::SrcAlpha, BlendFactor::OneMinusSrcAlpha)),
            depth_write: None,
            alpha_func: Some(AlphaFunc::Ge128),
            depth_func: DepthFunc::LessEqual,
            rgb_gen: RgbGen::Const([0.5, 0.25, 0.125]),
            alpha_gen: AlphaGen::Const(0.75),
        }],
        ..Default::default()
    });
    let p = stage_params(&sh, 0, 2.0).unwrap();
    // an alpha blend sorts past the shadow mask and shades itself
    assert_eq!(
        p.flags,
        STAGE_FLAG_BUNDLE1_LIGHTMAP | STAGE_FLAG_ALPHAFUNC_GE128 | STAGE_FLAG_SUN_SHADOW
    );
    assert_eq!(p.uv0, bundle_affine(&[TcMod::Scroll(0.25, 0.0)], 2.0));
    assert_eq!(p.uv1, [1.0, 0.0, 0.0, 1.0, 0.0, 0.0]);
    assert_eq!(p.tint, [0.5, 0.25, 0.125, 0.75]);
    assert_eq!(p.turb01, [0.0; 4]);
}

/// Every stage blended onto `GL_ONE` fogs towards black (0x4d2ad0's
/// `(state & 0xf0) == 0x20`), whatever its source factor; the rest fog
/// towards the fog colour.
#[test]
fn stage_params_fog_a_stage_blended_onto_one_towards_black() {
    let with = |blend| {
        let sh = Material::from_shader(&Shader {
            name: "test/glow".into(),
            stages: vec![Stage {
                bundles: vec![Bundle {
                    image: ImageRef::Path("a".into()),
                    anim: None,
                    clamp: Clamp::NONE,
                    tcmods: vec![],
                    tc_gen: TcGen::Base,
                    env: TexEnv::Modulate,
                }],
                blend,
                depth_write: None,
                alpha_func: None,
                depth_func: DepthFunc::LessEqual,
                rgb_gen: RgbGen::Identity,
                alpha_gen: AlphaGen::Identity,
            }],
            ..Default::default()
        });
        stage_params(&sh, 0, 0.0).unwrap().flags & STAGE_FLAG_FOG_BLACK != 0
    };
    use BlendFactor::*;
    assert!(with(Some((One, One))));
    assert!(with(Some((SrcAlpha, One))));
    assert!(with(Some((DstColor, One))));
    assert!(!with(Some((SrcAlpha, OneMinusSrcAlpha))));
    assert!(!with(Some((DstColor, Zero))));
    assert!(!with(None));
}

#[test]
fn stage_params_vector_bundle_sets_flag_and_carries_basis() {
    let sh = Material::from_shader(&Shader {
        name: "test/vec".into(),
        stages: vec![Stage {
            bundles: vec![Bundle {
                image: ImageRef::White,
                anim: None,
                clamp: Clamp::NONE,
                tcmods: vec![],
                tc_gen: TcGen::Vector([1.0, 0.0, 0.0, 0.0, 1.0, 0.0]),
                env: TexEnv::Modulate,
            }],
            blend: None,
            depth_write: None,
            alpha_func: None,
            depth_func: DepthFunc::LessEqual,
            rgb_gen: RgbGen::Vertex,
            alpha_gen: AlphaGen::Vertex,
        }],
        ..Default::default()
    });
    let p = stage_params(&sh, 0, 0.0).unwrap();
    assert_eq!(
        p.flags,
        STAGE_FLAG_BUNDLE0_VECTOR
            | STAGE_FLAG_VERTEX_RGB
            | STAGE_FLAG_VERTEX_RGB_HALF
            | STAGE_FLAG_VERTEX_ALPHA
    );
    assert_eq!(p.vec0_s, [1.0, 0.0, 0.0, 0.0]);
    assert_eq!(p.vec0_t, [0.0, 1.0, 0.0, 0.0]);
    assert_eq!(p.vec1_s, [0.0; 4]);
    // vertex gens leave tint white; the flags carry the instruction
    assert_eq!(p.tint, [1.0, 1.0, 1.0, 1.0]);
}

#[test]
fn stage_params_wave_gens_evaluate_at_time() {
    let rgb_wave = Wave {
        form: WaveForm::Sin,
        base: 1.0,
        amp: 0.5,
        phase: 0.25,
        freq: 0.0,
    };
    let a_wave = Wave {
        form: WaveForm::Sawtooth,
        base: 0.0,
        amp: 1.0,
        phase: 0.25,
        freq: 0.0,
    };
    let sh = Material::from_shader(&Shader {
        name: "test/wave".into(),
        stages: vec![Stage {
            bundles: vec![Bundle {
                image: ImageRef::Path("a".into()),
                anim: None,
                clamp: Clamp::NONE,
                tcmods: vec![],
                tc_gen: TcGen::Base,
                env: TexEnv::Modulate,
            }],
            blend: None,
            depth_write: None,
            alpha_func: None,
            depth_func: DepthFunc::LessEqual,
            rgb_gen: RgbGen::Wave(rgb_wave.clone()),
            alpha_gen: AlphaGen::Wave(a_wave.clone()),
        }],
        ..Default::default()
    });
    let p = stage_params(&sh, 0, 123.0).unwrap();
    let rv = vcod_common::shader::wave_value(&rgb_wave, 123.0);
    let av = vcod_common::shader::wave_value(&a_wave, 123.0);
    // the rgb wave scales by identityLight in the shader, which clamps
    // after it (0x514c00): 1.5 reaches it whole, to draw 0.75 at an
    // identityLight of 0.5; alpha waves clamp here
    assert!((rv - 1.5).abs() < 1e-3, "{rv}");
    assert_eq!(p.tint[..3], [rv; 3]);
    assert!((p.tint[3] - av).abs() < 1e-6 && (av - 0.25).abs() < 1e-6);
    assert_eq!(p.flags, STAGE_FLAG_TINT_LIGHT);
}

/// One block of script text, parsed.
fn parsed(text: &str) -> Material {
    use vcod_common::shader::{WarnSet, parse_shader, split_blocks};
    let (name, body) = split_blocks(text).into_iter().next().expect("a block");
    Material::from_shader(&parse_shader(&name, &body, &mut WarnSet::new()))
}

/// deform.wgsl's kind codes are the ones `stage_params` writes.
#[test]
fn deform_wgsl_codes_are_the_renderer_ones() {
    let src = include_str!("../deform.wgsl");
    for (name, code) in [
        ("DEFORM_WAVE", DEFORM_WAVE),
        ("DEFORM_FLAP_S", DEFORM_FLAP_S),
        ("DEFORM_FLAP_T", DEFORM_FLAP_T),
        ("DEFORM_NORMAL", DEFORM_NORMAL),
        ("DEFORM_BULGE", DEFORM_BULGE),
        ("DEFORM_MOVE", DEFORM_MOVE),
        ("DEFORM_FORM_SHIFT", DEFORM_FORM_SHIFT),
        (
            "FORM_SQUARE",
            deform_form(&WaveForm::Square) >> DEFORM_FORM_SHIFT,
        ),
        (
            "FORM_TRIANGLE",
            deform_form(&WaveForm::Triangle) >> DEFORM_FORM_SHIFT,
        ),
        (
            "FORM_SAWTOOTH",
            deform_form(&WaveForm::Sawtooth) >> DEFORM_FORM_SHIFT,
        ),
        (
            "FORM_INVERSE_SAWTOOTH",
            deform_form(&WaveForm::InverseSawtooth) >> DEFORM_FORM_SHIFT,
        ),
        (
            "FORM_NOISE",
            deform_form(&WaveForm::Noise) >> DEFORM_FORM_SHIFT,
        ),
    ] {
        assert!(
            src.contains(&format!("const {name}: u32 = {code}u;")),
            "{name}"
        );
    }
    assert_eq!(deform_form(&WaveForm::Sin), 0, "table_entry's fallthrough");
}

/// cod2_model.wgsl's stage flags are the ones `stage_params` writes.
#[test]
fn cod2_model_wgsl_flags_are_the_renderer_ones() {
    let src = include_str!("../cod2_model.wgsl");
    for (name, flag) in [
        ("F_COD2_PROBE", STAGE_FLAG_COD2_PROBE),
        ("F_COD2_ALPHA", STAGE_FLAG_COD2_ALPHA),
        ("F_COD2_SPEC", STAGE_FLAG_COD2_SPEC),
        ("F_COD2_DETAIL", STAGE_FLAG_COD2_DETAIL),
        ("F_COD2_SCROLL", STAGE_FLAG_COD2_SCROLL),
        ("F_OBJECTIVE", STAGE_FLAG_OBJECTIVE),
    ] {
        assert!(
            src.contains(&format!("const {name}: u32 = {flag}u;")),
            "{name}"
        );
    }
    let flag = format!("const COD2_FLAG: u32 = {DEFORM_COD2_FLAG}u;");
    assert!(include_str!("../dynamic_model.wgsl").contains(&flag));
    assert!(src.contains(&format!("const COD4_FLAG: u32 = {DEFORM_COD4_FLAG}u;")));
}

/// noise.wgsl spells out the noise tables `deformVertexes normal` reads
/// on the GPU; they must be vcod_common's, entry for entry.
#[test]
fn noise_wgsl_tables_are_the_common_ones() {
    let src = include_str!("../noise.wgsl");
    let array = |name: &str| -> Vec<String> {
        let start = src.find(&format!("var<private> {name}:")).expect(name);
        let body = &src[start..];
        let open = body.find(">(").unwrap() + 2;
        let close = body.find(");").unwrap();
        body[open..close]
            .split(',')
            .map(|t| t.trim().to_string())
            .filter(|t| !t.is_empty())
            .collect()
    };
    let perm: Vec<u8> = array("NOISE_PERM")
        .iter()
        .map(|t| t.trim_end_matches('u').parse().unwrap())
        .collect();
    let table: Vec<f32> = array("NOISE_TABLE")
        .iter()
        .map(|t| t.parse().unwrap())
        .collect();
    assert_eq!(perm, vcod_common::shader::NOISE_PERM);
    assert_eq!(table, vcod_common::shader::NOISE_TABLE);
}

/// The feather's depth copy lands before the first model surface, or
/// at the split when no model comes first.
#[test]
fn feather_copy_goes_before_the_models() {
    use SortedItem::*;
    let items = [World(0), World(1), Model(0), World(2), Fx(0)];
    assert_eq!(feather_copy_at(&items, 4), 2);
    assert_eq!(feather_copy_at(&items, 1), 1);
    assert_eq!(feather_copy_at(&[World(0), Fx(0)], 1), 1);
}

/// A CoD2 `lprobe_flag*` probe stage carries the flag in the deform
/// kinds' spare fourth slot, which deform.wgsl's three records never
/// read; any other probe stage leaves it 0.
#[test]
fn stage_params_mark_a_flag_stage() {
    let mut sh = parsed("t\n{\n{\nmap a.tga\nrgbGen lightingDiffuse\n}\n}");
    let mut pr = vcod_common::material::ProbeLit {
        specular: true,
        alpha: true,
        detail: false,
        scroll: false,
        flag: true,
        detail_scale: [1.0, 1.0],
    };
    sh.probe = Some(pr);
    let p = stage_params(&sh, 0, 0.0).unwrap();
    assert_ne!(p.flags & STAGE_FLAG_COD2_PROBE, 0);
    assert_eq!(p.deform_kinds, [0, 0, 0, DEFORM_COD2_FLAG]);
    pr.flag = false;
    sh.probe = Some(pr);
    assert_eq!(stage_params(&sh, 0, 0.0).unwrap().deform_kinds, [0; 4]);
}

/// The deform records reach the shader in script order, each in its
/// slot with retail's deform number (0x5149b0 runs them in order).
#[test]
fn stage_params_carry_the_deforms_in_script_order() {
    let sh = parsed(
        "t\n{\ndeformVertexes bulge 3 4 5\ndeformVertexes move 0 0 2 square 1 2 0 0\ndeformVertexes normal .5 2\n{\nmap a.tga\n}\n}",
    );
    assert!(stage_animated(&sh, 0));
    let p = stage_params(&sh, 0, 2.0).unwrap();
    assert_eq!(
        p.deform_kinds,
        [DEFORM_BULGE, DEFORM_MOVE, DEFORM_NORMAL, 0]
    );
    assert_eq!(p.deform[0], [3.0, 4.0, 10.0, 0.0]);
    // square at phase 0 reads +1: 2 * (1 + 2 * 1) along z
    assert_eq!(p.deform[1], [0.0, 0.0, 6.0, 0.0]);
    assert_eq!(p.deform[2], [0.5, 4.0, 0.0, 0.0]);
    // reordered, the slots follow
    let sh = parsed(
        "t\n{\ndeformVertexes normal .5 2\ndeformVertexes wave 64 sin 0 2 0 1\n{\nmap a.tga\n}\n}",
    );
    let p = stage_params(&sh, 0, 2.0).unwrap();
    assert_eq!(p.deform_kinds, [DEFORM_NORMAL, DEFORM_WAVE, 0, 0]);
    // a sprite holds a record slot but draws through its own flag
    let sh = parsed(
        "t\n{\ndeformVertexes autosprite\ndeformVertexes move 0 0 2 square 1 2 0 0\n{\nmap a.tga\n}\n}",
    );
    let p = stage_params(&sh, 0, 2.0).unwrap();
    assert_eq!(p.deform_kinds, [DEFORM_MOVE, 0, 0, 0]);
    let plain = parsed("t\n{\n{\nmap a.tga\n}\n}");
    let p = stage_params(&plain, 0, 2.0).unwrap();
    assert_eq!(p.deform_kinds, [0; 4]);
    assert!(!stage_animated(&plain, 0));
}

#[test]
fn stage_params_noise_glow_flickers_without_identity_light() {
    // UO ba.shader's fire window (pakuo01)
    let sh = parsed(
        "textures/ardennes/windows/firewindow\n{\n{\nmap a.dds\nrgbgen wave noise 0.4 0.1 0 10\nblendfunc blend\n}\n}",
    );
    let tints: Vec<f32> = (0..20)
        .map(|i| {
            let p = stage_params(&sh, 0, i as f32 * 0.05).unwrap();
            assert_eq!(p.flags & STAGE_FLAG_TINT_LIGHT, 0);
            p.tint[0]
        })
        .collect();
    assert!(tints.iter().all(|v| (0.3..=0.5).contains(v)));
    assert!(tints.iter().any(|v| (v - 0.4).abs() > 0.02), "{tints:?}");
}

/// A table rgbGen wave clamps below at 0 here and above only after the
/// shader's identityLight scale; a noise wave, unscaled, clamps here.
#[test]
fn stage_params_rgb_wave_clamps_after_identity_light() {
    let low = parsed("t\n{\n{\nmap a.tga\nrgbGen wave sin -1 0.5 0 0\n}\n}");
    assert_eq!(stage_params(&low, 0, 0.0).unwrap().tint[..3], [0.0; 3]);
    let high = parsed("t\n{\n{\nmap a.tga\nrgbGen wave sin 3 0 0 0\n}\n}");
    let p = stage_params(&high, 0, 0.0).unwrap();
    assert_eq!(p.tint[..3], [3.0; 3]);
    assert_ne!(p.flags & STAGE_FLAG_TINT_LIGHT, 0);
    let noise = parsed("t\n{\n{\nmap a.tga\nrgbGen wave noise 3 0 0 1\n}\n}");
    assert_eq!(stage_params(&noise, 0, 0.0).unwrap().tint[..3], [1.0; 3]);
}

#[test]
fn stage_params_identity_gens_stay_white_without_flags() {
    for rgb in [RgbGen::Identity, RgbGen::ExactVertex] {
        let sh = Material::from_shader(&Shader {
            name: "test/id".into(),
            stages: vec![Stage {
                bundles: vec![Bundle {
                    image: ImageRef::Path("a".into()),
                    anim: None,
                    clamp: Clamp::NONE,
                    tcmods: vec![],
                    tc_gen: TcGen::Base,
                    env: TexEnv::Modulate,
                }],
                blend: None,
                depth_write: None,
                alpha_func: None,
                depth_func: DepthFunc::LessEqual,
                rgb_gen: rgb.clone(),
                alpha_gen: AlphaGen::Identity,
            }],
            ..Default::default()
        });
        let p = stage_params(&sh, 0, 9.0).unwrap();
        assert_eq!(p.tint, [1.0, 1.0, 1.0, 1.0], "{rgb:?}");
        // ExactVertex routes through the vertex-colour flag like Vertex
        assert_eq!(
            p.flags,
            u32::from(rgb == RgbGen::ExactVertex) * STAGE_FLAG_VERTEX_RGB
        );
    }
}

/// identityLighting and constLighting take identityLight in the
/// shader; const passes through (0x4ffa60's colour switch).
#[test]
fn stage_params_identity_light_flags_the_lighting_gens() {
    let cases: &[(RgbGen, [f32; 3], u32)] = &[
        (RgbGen::IdentityLighting, [1.0; 3], STAGE_FLAG_TINT_LIGHT),
        (
            RgbGen::ConstLighting([0.6, 0.65, 0.7]),
            [0.6, 0.65, 0.7],
            STAGE_FLAG_TINT_LIGHT,
        ),
        (RgbGen::Const([0.6, 0.65, 0.7]), [0.6, 0.65, 0.7], 0),
    ];
    for (rgb, want, flags) in cases {
        let sh = Material::from_shader(&Shader {
            name: "test/il".into(),
            stages: vec![Stage {
                bundles: vec![Bundle {
                    image: ImageRef::Path("a".into()),
                    anim: None,
                    clamp: Clamp::NONE,
                    tcmods: vec![],
                    tc_gen: TcGen::Base,
                    env: TexEnv::Modulate,
                }],
                blend: None,
                depth_write: None,
                alpha_func: None,
                depth_func: DepthFunc::LessEqual,
                rgb_gen: rgb.clone(),
                alpha_gen: AlphaGen::Identity,
            }],
            ..Default::default()
        });
        let p = stage_params(&sh, 0, 9.0).unwrap();
        assert_eq!(p.tint[..3], want[..], "{rgb:?}");
        assert_eq!(p.flags, *flags, "{rgb:?}");
    }
}

/// Every WGSL module parses and validates, so a shader edit fails here
/// instead of at pipeline creation.
#[test]
fn quad_half_extent_is_the_shorter_half_side() {
    // a 2x6 stretch quad, ring order
    let v = [
        [0.0, 0.0, 0.0],
        [6.0, 0.0, 0.0],
        [6.0, 0.0, 2.0],
        [0.0, 0.0, 2.0],
    ];
    assert_eq!(quad_half_extent(&v), 1.0);
}

#[test]
fn hud_images_take_retail_flags_by_registration() {
    use vcod_common::shader::{WarnSet, parse_shader, split_blocks};
    let text = "hudA\n{\nnomipmaps\n{\nmap clamp gfx/a.tga\n}\n}\n\
                hudB\n{\n{\nmap gfx/b.tga\n}\n}\n";
    let lib: HashMap<String, Material> = split_blocks(text)
        .into_iter()
        .map(|(name, body)| {
            (
                name.to_lowercase(),
                Material::from_shader(&parse_shader(&name, &body, &mut WarnSet::new())),
            )
        })
        .collect();
    let mips: HashSet<String> = [hud_image_key("GFX/HUD/hud@death_kar98.tga")].into();
    let image = |n: &str| hud_image(n, lib.get(&n.to_lowercase()), &mips);
    let flags = |mip, picmip, clamp| HudImage { mip, picmip, clamp };
    assert_eq!(image("hudA"), flags(false, true, true));
    // a script's keywords count whatever trap registered it
    assert_eq!(image("hudB"), flags(true, true, false));
    // implicit: RE_RegisterShader's 3, RE_RegisterShaderNoMip's 0x30
    assert_eq!(
        image("gfx/hud/hud@death_kar98.tga"),
        flags(true, true, false)
    );
    assert_eq!(image("ui/assets/x.tga"), flags(false, false, true));
}

#[test]
fn weapon_icons_register_mipmapped() {
    let Some(fs) = vcod_common::testing::game_fs() else {
        return;
    };
    // configstring 7's table, not every weapon file
    let weapons = vcod_common::weapon_table::from_configstring(&fs, "kar98k_mp luger_mp");
    let names = hud_mip_names(&weapons);
    assert!(names.contains("gfx/hud/hud@death_kar98"));
    assert!(!names.contains("gfx/hud/hud@death_m1carbine"));
    assert!(names.contains("gfx/2d/net"));
    assert!(hud_mip_names(&[]).contains("gfx/2d/net"));
}

/// Every module the client builds, as its `create_shader_module` site
/// builds it, parses and validates with no GPU; every `.wgsl` file under
/// src is part of one of them.
#[test]
fn wgsl_modules_validate() {
    use wgpu::naga;
    let modules = [
        ("shader", WORLD_WGSL),
        ("dynamic_model", crate::entity_material::DYNAMIC_WGSL),
        ("viewmodel", crate::entity_material::VIEWMODEL_WGSL),
        ("fx", FX_WGSL),
        ("sky", SKY_WGSL),
        ("cod2_world", cod2::COD2_WORLD_WGSL),
        ("sky_cube", cod2::SKY_CUBE_WGSL),
        ("cod2_water", cod2_water::COD2_WATER_WGSL),
        ("gamma", include_str!("../gamma.wgsl")),
        ("hud", include_str!("../hud.wgsl")),
        ("hud_text", include_str!("../hud_text.wgsl")),
        ("bloom", include_str!("../bloom.wgsl")),
        ("post", include_str!("../post.wgsl")),
        ("screen_blend", include_str!("../screen_blend.wgsl")),
        ("ssao", include_str!("../ssao.wgsl")),
        ("sun_flare", include_str!("../sun_flare.wgsl")),
        ("sun_shafts", include_str!("../sun_shafts.wgsl")),
        ("soft_particles", include_str!("../soft_particles.wgsl")),
        ("cod4_dof", include_str!("../cod4_dof.wgsl")),
        ("cod4_sunshadow", crate::cod4_sunshadow::CAST_WGSL),
        ("cod4_spot_blit", crate::cod4_sunshadow::SPOT_BLIT_WGSL),
        ("cod2_glow", include_str!("../cod2_glow.wgsl")),
        ("cod2_cookie", cod2_cookie::COOKIE_WGSL),
        ("fx_cloud", fx_cloud::FX_CLOUD_WGSL),
        ("scene_copy", include_str!("../scene_copy.wgsl")),
        ("shadow_cast", include_str!("../shadow_cast.wgsl")),
        ("shadow_cut", crate::shadows::SHADOW_CUT_WGSL),
        ("shadow_mask", include_str!("../shadow_mask.wgsl")),
    ];
    for (name, src) in modules {
        for (samples, src) in [(4, depth_wgsl_at(src, 4)), (1, depth_wgsl_at(src, 1))] {
            let module = naga::front::wgsl::parse_str(&src)
                .unwrap_or_else(|e| panic!("{name}: {}", e.emit_to_string(&src)));
            naga::valid::Validator::new(
                naga::valid::ValidationFlags::all(),
                naga::valid::Capabilities::default(),
            )
            .validate(&module)
            .unwrap_or_else(|e| panic!("{name} at {samples}x: {}", e.emit_to_string(&src)));
        }
    }
    let mut dirs = vec![std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src")];
    let mut files = 0;
    while let Some(dir) = dirs.pop() {
        for e in std::fs::read_dir(&dir).unwrap() {
            let path = e.unwrap().path();
            if path.is_dir() {
                dirs.push(path);
            } else if path.extension().is_some_and(|x| x == "wgsl") {
                let text = std::fs::read_to_string(&path).unwrap();
                assert!(
                    modules.iter().any(|(_, src)| src.contains(&text)),
                    "{} is in no module this test validates",
                    path.display()
                );
                files += 1;
            }
        }
    }
    assert!(files > 0, "no .wgsl file found");
}

#[test]
fn stage_params_single_bundle_leaves_slot_one_neutral() {
    let sh = Material::from_shader(&Shader {
        name: "test/one".into(),
        stages: vec![Stage {
            bundles: vec![Bundle {
                image: ImageRef::Path("a".into()),
                anim: None,
                clamp: Clamp::NONE,
                tcmods: vec![TcMod::Scale(2.0, 3.0)],
                tc_gen: TcGen::Base,
                env: TexEnv::Modulate,
            }],
            blend: None,
            depth_write: None,
            alpha_func: Some(AlphaFunc::Lt128),
            depth_func: DepthFunc::LessEqual,
            rgb_gen: RgbGen::Identity,
            alpha_gen: AlphaGen::Identity,
        }],
        ..Default::default()
    });
    let p = stage_params(&sh, 0, 1.0).unwrap();
    assert_eq!(p.flags, STAGE_FLAG_ALPHAFUNC_LT128);
    assert_eq!(p.uv1, [1.0, 0.0, 0.0, 1.0, 0.0, 0.0]);
    assert_eq!(p.turb01[2..], [0.0, 0.0]);
    assert_eq!(p.vec1_s, [0.0; 4]);
}

/// A `clampmap` bundle samples through the clamp sampler; images carry
/// no sampler of their own.
#[test]
fn stage_params_flag_a_clamped_bundle() {
    let mut sh = Material::from_shader(&Shader {
        name: "test/clamp".into(),
        stages: vec![Stage {
            bundles: vec![Bundle {
                image: ImageRef::Path("a".into()),
                anim: None,
                clamp: Clamp::NONE,
                tcmods: Vec::new(),
                tc_gen: TcGen::Base,
                env: TexEnv::Modulate,
            }],
            blend: None,
            depth_write: None,
            alpha_func: None,
            depth_func: DepthFunc::LessEqual,
            rgb_gen: RgbGen::Identity,
            alpha_gen: AlphaGen::Identity,
        }],
        ..Default::default()
    });
    let clamp_flags = |sh: &Material| {
        stage_params(sh, 0, 0.0).unwrap().flags
            & (STAGE_FLAG_CLAMP_S | STAGE_FLAG_CLAMP_T | STAGE_FLAG_CLAMP1_S | STAGE_FLAG_CLAMP1_T)
    };
    assert_eq!(clamp_flags(&sh), 0);
    sh.stages[0].bundles[0].clamp = Clamp::BOTH;
    assert_eq!(clamp_flags(&sh), STAGE_FLAG_CLAMP_S | STAGE_FLAG_CLAMP_T);
    // `clampy` on bundle 0 and `clampx` on bundle 1 each keep their axis
    // and their bundle (0x4ec6e9, 0x4ec70f)
    sh.stages[0].bundles[0].clamp = Clamp { s: false, t: true };
    let mut b1 = sh.stages[0].bundles[0].clone();
    b1.clamp = Clamp { s: true, t: false };
    sh.stages[0].bundles.push(b1);
    assert_eq!(clamp_flags(&sh), STAGE_FLAG_CLAMP_T | STAGE_FLAG_CLAMP1_S);
}

#[test]
fn stage_params_turb_rides_separate_from_affine() {
    let tcmods = vec![
        TcMod::Scroll(0.5, 0.5),
        TcMod::Turb {
            amp: 3.0,
            phase: 0.1,
            freq: 0.5,
        },
    ];
    let sh = Material::from_shader(&Shader {
        name: "test/turb".into(),
        stages: vec![Stage {
            bundles: vec![Bundle {
                image: ImageRef::Path("a".into()),
                anim: None,
                clamp: Clamp::NONE,
                tcmods: tcmods.clone(),
                tc_gen: TcGen::Base,
                env: TexEnv::Modulate,
            }],
            blend: None,
            depth_write: None,
            alpha_func: None,
            depth_func: DepthFunc::LessEqual,
            rgb_gen: RgbGen::Identity,
            alpha_gen: AlphaGen::Identity,
        }],
        ..Default::default()
    });
    let p = stage_params(&sh, 0, 4.0).unwrap();
    assert_eq!(p.uv0, bundle_affine(&tcmods, 4.0));
    assert_eq!(p.turb01, [3.0, 0.1 + 0.5 * 4.0, 0.0, 0.0]);
}

#[test]
fn stage_params_flag_flap_and_sprites() {
    // skins/shipflag_np: flap s, the wave riding the first deform record
    let parse = |body: &str| {
        let (name, body) = vcod_common::shader::split_blocks(body)
            .into_iter()
            .next()
            .expect("one block");
        Material::from_shader(&vcod_common::shader::parse_shader(
            &name,
            &body,
            &mut vcod_common::shader::WarnSet::new(),
        ))
    };
    let flag = parse("f { cull none deformVertexes flap s 32 sin 4 8 0 .75 { map a.tga } }");
    let p = stage_params(&flag, 0, 2.0).expect("one stage");
    assert_eq!(p.deform_kinds, [DEFORM_FLAP_S, 0, 0, 0]);
    // the phase 0 + 2 * 0.75 goes up as its fraction: one table turn
    assert_eq!(p.deform[0], [4.0, 8.0, 0.5, 1.0 / 32.0]);
    // a square wave names its table in the kind's upper bits
    let square = parse("f { deformVertexes wave 100 square 1 2 0.25 3 { map a.tga } }");
    let p = stage_params(&square, 0, 1.0).expect("one stage");
    assert_eq!(p.deform_kinds[0], DEFORM_WAVE | 1 << DEFORM_FORM_SHIFT);
    assert_eq!(p.deform[0], [1.0, 2.0, 0.25, 0.01]);
    assert!(stage_animated(&flag, 0));
    // a rate of 0 drops the spread (one phase for every vertex)
    let still = parse("f { deformVertexes flap t 32 sin 4 8 .5 0 { map a.tga } }");
    let p = stage_params(&still, 0, 2.0).expect("one stage");
    assert_eq!(p.deform_kinds, [DEFORM_FLAP_T, 0, 0, 0]);
    assert_eq!(p.deform[0], [4.0, 8.0, 0.5, 0.0]);
    // skins/light_sprite: autosprite, and its lightingDiffuse-free stage
    let sprite = parse(
        "s { deformvertexes autosprite { map a.tga blendfunc add rgbGen identityLighting } }",
    );
    let p = stage_params(&sprite, 0, 0.0).expect("one stage");
    assert_eq!(p.flags & STAGE_FLAG_AUTOSPRITE, STAGE_FLAG_AUTOSPRITE);
    assert!(!stage_animated(&sprite, 0));
    let beam = parse("b { deformVertexes autosprite2 { map a.tga } }");
    let p = stage_params(&beam, 0, 0.0).expect("one stage");
    assert_eq!(
        p.flags & (STAGE_FLAG_AUTOSPRITE | STAGE_FLAG_AUTOSPRITE2),
        STAGE_FLAG_AUTOSPRITE2
    );
}

#[test]
fn stage_params_out_of_range_stage_is_none() {
    let sh = Material::from_shader(&Shader::default());
    assert_eq!(stage_params(&sh, 0, 0.0), None);
}

#[test]
fn anim_frame_index_floors_wraps_and_clamps() {
    assert_eq!(anim_frame_index(5.0, 3, 0.0), 0);
    assert_eq!(anim_frame_index(5.0, 3, 0.21), 1);
    assert_eq!(anim_frame_index(5.0, 3, 0.999), 1); // floor(4.995)=4, 4%3
    assert_eq!(anim_frame_index(5.0, 3, 10.0), 2); // 50 % 3
    assert_eq!(anim_frame_index(0.0, 3, 5.0), 0);
    assert_eq!(anim_frame_index(-1.0, 3, 5.0), 0);
    assert_eq!(anim_frame_index(5.0, 0, 5.0), 0);
    assert_eq!(anim_frame_index(5.0, 3, -1.0), 0);
}

// ---- StageBatch classification ----

use vcod_common::shader::{SkyParms, SurfaceBits};

/// One-bundle shader from blend pair plus top-level knobs.
fn script(blend: Option<(BlendFactor, BlendFactor)>, sort: Option<f32>, offset: bool) -> Material {
    Material::from_shader(&Shader {
        name: "test/script".into(),
        stages: vec![Stage {
            bundles: vec![Bundle {
                image: ImageRef::Path("a".into()),
                anim: None,
                clamp: Clamp::NONE,
                tcmods: vec![],
                tc_gen: TcGen::Base,
                env: TexEnv::Modulate,
            }],
            blend,
            depth_write: None,
            alpha_func: None,
            depth_func: DepthFunc::LessEqual,
            rgb_gen: RgbGen::Identity,
            alpha_gen: AlphaGen::Identity,
        }],
        sort,
        polygon_offset: offset.then_some(PolygonOffset::Single),
        ..Default::default()
    })
}

#[test]
fn opaque_and_blend_variants_follow_class_and_depth_write() {
    let (v, _, _) = stage_draw_facts(&script(None, None, false), 0).unwrap();
    assert_eq!(v, StageVariant::Opaque);

    let alpha = Some((BlendFactor::SrcAlpha, BlendFactor::OneMinusSrcAlpha));
    let (v, _, _) = stage_draw_facts(&script(alpha.clone(), None, false), 0).unwrap();
    assert_eq!(v, StageVariant::Blend);

    // explicit depthWrite keeps writing
    let mut sh = script(alpha, None, false);
    sh.stages[0].depth_write = Some(true);
    assert_eq!(
        stage_draw_facts(&sh, 0).unwrap().0,
        StageVariant::BlendDepthWrite
    );

    // polygonOffset biases the stage
    assert!(stage_draw_facts(&script(None, None, true), 0).unwrap().2);
}

/// A CoD4 light shaft (`falloff_add`, cod4-world.md 9.5) draws its
/// premultiplied colour (OneMinusDstColor, One) through
/// `fs_stage_premul`, its angle falloff and eye offset in the params;
/// the same pair from a CoD1 script keeps the faded additive.
#[test]
fn cod4_falloff_add_screens_its_premultiplied_colour() {
    let mut sh = script(
        Some((BlendFactor::OneMinusDstColor, BlendFactor::One)),
        None,
        false,
    );
    assert_eq!(
        stage_draw_facts(&sh, 0).unwrap().0,
        StageVariant::AdditiveFaded
    );
    sh.premultiplied = true;
    sh.falloff = Some(vcod_common::material::Falloff {
        begin: [0.5, 0.5, 0.5],
        end: [0.0, 0.0, 0.0],
        scale: 1.5,
        bias: -0.2,
        kind: FalloffKind::Angle,
    });
    sh.eye_offset = Some(4.0);
    assert_eq!(
        stage_draw_facts(&sh, 0).unwrap().0,
        StageVariant::AdditiveInvDst
    );
    let (entry, blend, depth_write) = stage_variant_state(StageVariant::AdditiveInvDst);
    let c = blend.unwrap().color;
    assert_eq!(
        (entry, c.src_factor, c.dst_factor, depth_write),
        (
            "fs_stage_premul",
            wgpu::BlendFactor::OneMinusDst,
            wgpu::BlendFactor::One,
            false
        )
    );
    let p = stage_params(&sh, 0, 0.0).unwrap();
    assert_eq!(p.falloff, [[0.5, 0.5, 0.5, 1.5], [0.0, 0.0, 0.0, -0.2]]);
    assert_eq!(p.eye_off, [0.0, 0.0, 4.0, 1.0]);
}

/// A CoD4 doorway lightener (`hdrportal_lighten`, cod4-world.md 9.4)
/// draws (DstColor, SrcColor) with its falloff in the stage params,
/// never as the alpha blend that painted mp_crash's doorways white.
#[test]
fn cod4_distfalloff_doubles_the_framebuffer() {
    let mut sh = script(
        Some((BlendFactor::DstColor, BlendFactor::SrcColor)),
        None,
        false,
    );
    assert_eq!(stage_draw_facts(&sh, 0).unwrap().0, StageVariant::Blend);
    assert_eq!(stage_params(&sh, 0, 0.0).unwrap().falloff, [[0.0; 4]; 2]);
    sh.falloff = Some(vcod_common::material::Falloff {
        begin: [0.8, 0.8, 0.8],
        end: [0.5, 0.5, 0.5],
        scale: 0.006,
        bias: -0.18,
        kind: FalloffKind::Distance,
    });
    assert_eq!(
        stage_draw_facts(&sh, 0).unwrap().0,
        StageVariant::Multiply2x
    );
    let (_, blend, depth_write) = stage_variant_state(StageVariant::Multiply2x);
    let c = blend.unwrap().color;
    assert_eq!(
        (c.src_factor, c.dst_factor, depth_write),
        (wgpu::BlendFactor::Dst, wgpu::BlendFactor::Src, false)
    );
    assert_eq!(
        stage_params(&sh, 0, 0.0).unwrap().falloff,
        [[0.8, 0.8, 0.8, 0.006], [0.5, 0.5, 0.5, -0.18]]
    );
}

/// Additive depthWrite must reach the pipeline (Task-6 carry-over): the
/// variants exist instead of silently drawing without depth writes.
#[test]
fn additive_depth_write_selects_a_depth_writing_variant() {
    let one_one = Some((BlendFactor::One, BlendFactor::One));
    let src_one = Some((BlendFactor::SrcAlpha, BlendFactor::One));

    let mut sh = script(one_one, None, false);
    assert_eq!(
        stage_draw_facts(&sh, 0).unwrap().0,
        StageVariant::AdditiveFixed
    );
    sh.stages[0].depth_write = Some(true);
    assert_eq!(
        stage_draw_facts(&sh, 0).unwrap().0,
        StageVariant::AdditiveFixedDepthWrite
    );

    let mut sh = script(src_one, None, false);
    assert_eq!(
        stage_draw_facts(&sh, 0).unwrap().0,
        StageVariant::AdditiveFaded
    );
    sh.stages[0].depth_write = Some(true);
    assert_eq!(
        stage_draw_facts(&sh, 0).unwrap().0,
        StageVariant::AdditiveFadedDepthWrite
    );
}

#[test]
fn animated_params_track_tcmods_and_wave_gens() {
    let mut sh = script(None, None, false);
    assert!(!stage_animated(&sh, 0));
    sh.stages[0].bundles[0].tcmods.push(TcMod::Scroll(0.1, 0.0));
    assert!(stage_animated(&sh, 0));

    let mut sh = script(None, None, false);
    sh.stages[0].rgb_gen = RgbGen::Wave(Wave {
        form: WaveForm::Sin,
        base: 1.0,
        amp: 0.0,
        phase: 0.0,
        freq: 1.0,
    });
    assert!(stage_animated(&sh, 0));

    // scale/transform are static affines; no rewrite needed
    let mut sh = script(None, None, false);
    sh.stages[0].bundles[0].tcmods.push(TcMod::Scale(2.0, 2.0));
    assert!(!stage_animated(&sh, 0));
}

/// Sky materials classify through their block's surface bits so the sky
/// pass can pick them up.
#[test]
fn surface_bits_carry_sky_and_nodraw_classification() {
    let mut bits = SurfaceBits::default();
    assert!(!bits.sky && !bits.nodraw);
    bits.sky = true;
    let sh = Material::from_shader(&Shader {
        name: "test/sky".into(),
        stages: vec![],
        sky: Some(SkyParms {
            env: "textures/skies/x".into(),
            cloud_height: 512.0,
        }),
        surface: bits.clone(),
        ..Default::default()
    });
    // either signal routes to Sky at load time
    assert!(sh.surface.sky || sh.sky.is_some());
    assert!(mesh::material_kind("textures/skies/x", true, false, true) == MaterialKind::Sky);
}

#[test]
fn one_list_merges_world_marks_models_and_fx_by_sort_in_that_order_on_ties() {
    use SortedItem::{Fx, Mark, Model, World};
    use vcod_common::shader::SORT_DECAL;
    let world = [SORT_OPAQUE, SORT_DECAL, SORT_BLEND, SORT_ADDITIVE];
    // marks sort decal (polygonOffset2): after the opaques, before blends
    let marks = [SORT_DECAL];
    let models = [SORT_OPAQUE, SORT_BLEND];
    // an opaque effect material draws among the opaques, not after
    // every world blend
    let fx = [SORT_OPAQUE, SORT_BLEND, SORT_ADDITIVE, 22.0];
    assert_eq!(
        merge_by_sort(&world, &marks, &models, &fx),
        [
            World(0),
            Model(0),
            Fx(0),
            World(1),
            Mark(0),
            World(2),
            Model(1),
            Fx(1),
            World(3),
            Fx(2),
            Fx(3)
        ]
    );
    assert_eq!(merge_by_sort(&[], &[], &[], &[SORT_BLEND]), [Fx(0)]);
    assert_eq!(merge_by_sort(&[SORT_BLEND], &[], &[], &[]), [World(0)]);
}

#[test]
fn shaders_of_one_sort_order_by_their_first_stage_images() {
    let e = |sort, images| SortEntry { sort, images };
    let places = sorted_places(&[
        // registered first, but blend sorts after every opaque
        e(SORT_BLEND, Some([1, 0])),
        e(SORT_OPAQUE, Some([9, 3])),
        e(SORT_OPAQUE, Some([4, 3])),
        // the same texture on a lower lightmap goes before it
        e(SORT_OPAQUE, Some([4, 2])),
        // equal images: after the earlier one
        e(SORT_OPAQUE, Some([4, 3])),
        e(vcod_common::shader::SORT_OCEAN, Some([20, 0])),
    ]);
    assert_eq!(places, [5, 4, 2, 1, 3, 0]);
    // a shader without bundle 1 does not hold back a newcomer with the
    // same bundle 0, nor does a stageless one of the same sort
    let places = sorted_places(&[
        e(SORT_OPAQUE, Some([4, 0])),
        e(SORT_OPAQUE, None),
        e(SORT_OPAQUE, Some([4, 7])),
    ]);
    assert_eq!(places, [2, 1, 0]);
}

/// 1.7's order inside one sort: the batch whose soup comes first in
/// the lit order draws first; a batch no soup reached, and the
/// entries past the world's, keep their registration order after.
#[test]
fn cod4_batches_of_one_sort_draw_in_lit_order() {
    use vcod_common::mesh::IndexRange;
    let e = |sort| SortEntry { sort, images: None };
    let range = |batch| {
        Some(IndexRange {
            batch,
            first: 0,
            count: 3,
        })
    };
    let entries = [
        e(SORT_OPAQUE),
        e(SORT_OPAQUE),
        e(SORT_BLEND),
        e(SORT_OPAQUE),
        e(SORT_OPAQUE),
        e(SORT_OPAQUE),
    ];
    // batch 1's first soup is 2nd in the lit order, batch 0's 5th; a
    // cell's missing range takes no place
    let ranges = [range(0), range(1), None, range(0), range(3), range(2)];
    let order = [4, 1, 0, 7, 9, 3];
    assert_eq!(cod4_places(&entries, &ranges, &order), [1, 0, 5, 2, 3, 4]);
}

/// A fade lerps every field from where it stood; a change of mode or a
/// zero fade snaps; a cleared fog's next target snaps too.
#[test]
fn fog_fades_within_a_mode_and_snaps_across_one() {
    let exp = |density: f32, fade_ms| FogParams {
        near: 0.0,
        far: 0.0,
        density,
        color: [0.0; 3],
        fade_ms,
        start: 0.0,
    };
    let linear = |far: f32, fade_ms| FogParams {
        near: 100.0,
        far,
        density: 1.0,
        color: [1.0; 3],
        fade_ms,
        start: 0.0,
    };
    let mut f = FogState::default();
    f.set_target(exp(0.2, 1000), 0.0);
    assert_eq!(f.live.density, 0.2, "the first fog snaps");
    f.set_target(exp(0.6, 1000), 1.0);
    f.advance(1.25);
    assert!((f.live.density - 0.3).abs() < 1e-6, "{}", f.live.density);
    assert!(f.target.is_some());
    // a new target mid-fade starts from what is displayed
    f.set_target(exp(0.0, 1000), 1.25);
    f.advance(1.75);
    assert!((f.live.density - 0.15).abs() < 1e-6, "{}", f.live.density);
    f.set_target(linear(2000.0, 1000), 2.0);
    assert!(f.live.linear && f.live.far == 2000.0, "a mode change snaps");
    assert!(f.target.is_none());
    f.set_target(linear(1000.0, 0), 3.0);
    assert_eq!(f.live.far, 1000.0, "no fade snaps");
    f.clear();
    assert!(!f.set);
    f.set_target(linear(3000.0, 1000), 4.0);
    assert_eq!(f.live.far, 3000.0, "the fog after a clear snaps");
}

fn vm_surface(
    first_index: u32,
    index_count: u32,
    base_vertex: i32,
    bind_group: usize,
) -> VmSurface {
    VmSurface {
        first_index,
        index_count,
        base_vertex,
        bind_group,
        src: 0,
    }
}

fn ent_skin(stages: usize) -> Rc<EntSkin> {
    let st = crate::entity_material::EntStageDraw {
        pipeline: 0,
        image: crate::entity_material::StageImage::Skin,
        slot: 0,
        params: 0,
        prepass: true,
        floatz: false,
        rigid: true,
    };
    Rc::new(EntSkin {
        stages: vec![st; stages],
        lit: true,
        sort: SORT_OPAQUE,
        order: 0,
        cookie_caster: false,
        sun_caster: false,
        sun_cut: None,
    })
}

/// An entity surface joins the next items while each is the same
/// instance's next surface on the same one-stage skin, bind group and
/// base vertex with its indices following on, and no stop or gun
/// comes between; each broken condition ends the run there.
#[test]
fn an_entity_run_joins_only_contiguous_surfaces_of_one_skin() {
    let ent = |inst, surf, gun| EntDraw {
        inst,
        surf,
        sort: SORT_OPAQUE,
        order: 0,
        gun,
    };
    let skin = ent_skin(1);
    let base = || {
        (
            vec![
                vm_surface(0, 6, 0, 0),
                vm_surface(6, 3, 0, 0),
                vm_surface(9, 12, 0, 0),
                vm_surface(21, 3, 0, 0),
            ],
            vec![skin.clone(); 4],
            vec![
                ent(0, 0, false),
                ent(0, 1, false),
                ent(0, 2, false),
                ent(0, 3, false),
            ],
        )
    };
    let items: Vec<SortedItem> = (0..4).map(SortedItem::Model).collect();
    let run =
        |(surfs, skins, draws): &(Vec<VmSurface>, Vec<Rc<EntSkin>>, Vec<EntDraw>),
         k,
         stops: &[Option<usize>]| { ent_run(&items, k, draws, (surfs, skins), stops) };
    let all = base();
    assert_eq!(run(&all, 0, &[None]), 4);
    assert_eq!(run(&all, 1, &[None]), 3);
    assert_eq!(run(&all, 0, &[Some(2)]), 2, "a stop");
    let mut c = base();
    c.2[2].gun = true;
    assert_eq!(run(&c, 0, &[None]), 2, "the gun");
    let mut c = base();
    c.2[2].inst = 1;
    assert_eq!(run(&c, 0, &[None]), 2, "another instance");
    let mut c = base();
    c.2[2].surf = 3;
    assert_eq!(run(&c, 0, &[None]), 2, "not the next surface");
    let mut c = base();
    c.1[2] = ent_skin(1);
    assert_eq!(run(&c, 0, &[None]), 2, "an equal skin, not the same one");
    let mut c = base();
    c.0[2].bind_group = 1;
    assert_eq!(run(&c, 0, &[None]), 2, "another bind group");
    let mut c = base();
    c.0[2].base_vertex = 100;
    assert_eq!(run(&c, 0, &[None]), 2, "another base vertex");
    let mut c = base();
    c.0[2].first_index = 10;
    assert_eq!(run(&c, 0, &[None]), 2, "a gap in the indices");
    let mut c = base();
    c.1 = vec![ent_skin(2); 4];
    assert_eq!(run(&c, 0, &[None]), 1, "a two-stage skin draws alone");
    let items = [
        SortedItem::Model(0),
        SortedItem::Fx(0),
        SortedItem::Model(1),
    ];
    assert_eq!(
        ent_run(&items, 0, &all.2, (&all.0, &all.1), &[None]),
        1,
        "another kind of item"
    );
}

/// `keep` is by the model's surfaces, and an empty one uploads no
/// surface, so the filter reads each uploaded surface's `src`.
#[test]
fn a_hidden_surface_is_picked_by_its_source_index() {
    let surfs: Vec<VmSurface> = [0, 2, 3, 5]
        .into_iter()
        .map(|src| VmSurface {
            src,
            ..vm_surface(0, 3, 0, 0)
        })
        .collect();
    assert_eq!(kept_surfaces(&surfs, &[]), [0, 1, 2, 3]);
    assert_eq!(kept_surfaces(&surfs, &[true, true, false, true]), [0, 2, 3]);
    assert_eq!(kept_surfaces(&surfs, &[false, false, true, false]), [1, 3]);
    assert!(kept_surfaces(&surfs, &[false; 6]).is_empty());
}

/// A run whose commands held from the last frame draws through a
/// bundle made once; a changed run replays and drops its bundle; the
/// runs past this frame's last go; a new target drops them all.
#[test]
fn world_runs_bundle_once_their_commands_hold() {
    let Some((device, queue)) = headless::test_device(headless::Need::Any) else {
        return;
    };
    set_msaa_samples(1);
    let tex = |format, usage| {
        device
            .create_texture(&wgpu::TextureDescriptor {
                label: None,
                size: wgpu::Extent3d {
                    width: 64,
                    height: 64,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage,
                view_formats: &[],
            })
            .create_view(&Default::default())
    };
    let colour = tex(SCENE_FORMAT, wgpu::TextureUsages::RENDER_ATTACHMENT);
    let depth = tex(DEPTH_FORMAT, wgpu::TextureUsages::RENDER_ATTACHMENT);
    let buf = || {
        device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: 64,
            usage: wgpu::BufferUsages::VERTEX,
            mapped_at_creation: false,
        })
    };
    let (a, b) = (buf(), buf());
    let rec = |v: &wgpu::Buffer| {
        let mut r = Recording::default();
        r.vertices(0, v.slice(..));
        r
    };
    let mut wb = WorldBundles::default();
    let frame = |wb: &mut WorldBundles, runs: &[&wgpu::Buffer]| {
        let mut enc = device.create_command_encoder(&Default::default());
        {
            let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: None,
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &colour,
                    resolve_target: None,
                    depth_slice: None,
                    ops: wgpu::Operations::default(),
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &depth,
                    depth_ops: Some(wgpu::Operations::default()),
                    stencil_ops: None,
                }),
                ..Default::default()
            });
            wb.begin(Some(SCENE_FORMAT));
            for v in runs {
                wb.draw(
                    &mut pass,
                    &device,
                    rec(v),
                    crate::frame_prof::Count::Rebundled,
                );
            }
            wb.end();
        }
        queue.submit([enc.finish()]);
    };
    let bundled = |wb: &WorldBundles| wb.runs.iter().map(|r| r.1.is_some()).collect::<Vec<_>>();
    frame(&mut wb, &[&a, &b]);
    assert_eq!(bundled(&wb), [false, false], "a new run replays");
    frame(&mut wb, &[&a, &b]);
    assert_eq!(bundled(&wb), [true, true], "held runs bundle");
    frame(&mut wb, &[&a, &a]);
    assert_eq!(
        bundled(&wb),
        [true, false],
        "a changed run drops its bundle"
    );
    frame(&mut wb, &[&a]);
    assert_eq!(bundled(&wb), [true], "runs past the last go");
    wb.begin(None);
    assert!(wb.runs.is_empty(), "a new target drops every run");
}

#[test]
fn image_numbers_follow_first_load_and_ignore_case_and_extension() {
    let mut n = ImageNumbers::default();
    assert_eq!(n.number("$white"), 1);
    assert_eq!(n.number("textures/a/Wall.tga"), 2);
    assert_eq!(n.number("textures/a/wall"), 2);
    assert_eq!(n.number("textures/a/floor.jpg"), 3);
}

/// The stock world types (pak4 `shadertypes/world/`): wood's gloss
/// alpha is read by nothing, foliage and the masked types cut, a decal
/// blends.
#[test]
fn a_world_type_draws_opaque_without_an_alpha_test_or_a_first_blend() {
    let wood = "{\n\tsurfaceparm wood\n\t{\n\t\tmap $texturename\n\t\trgbGen exactVertex\n\t}\n\t{\n\t\tmap $lightmap\n\t\tblendFunc filter\n\t}\n}\n";
    let foliage = "{\n\tsurfaceparm foliage\n\tsurfaceparm nomarks\n\t//cull disable\n\t{\n\t\tmap $texturename\n\t\trgbGen exactVertex\n\t\talphaFunc GE128\n\t\tdepthWrite\n\tnextbundle\n\t\tmap $lightmap\n\t}\n}\n";
    let decal = "{\n\tsurfaceparm nonsolid\n\tpolygonOffset\n\t{\n\t\tmap clamp $texturename\n\t\trgbGen exactVertex\n\t\tblendFunc blend\n\tnextbundle\n\t\tmap $lightmap\n\t}\n}\n";
    assert!(stype_draws_opaque(wood));
    assert!(!stype_draws_opaque(foliage));
    assert!(!stype_draws_opaque(decal));
    assert!(!stype_draws_opaque(""));
}

/// mp_chateau's glazed doors: the image's alpha masks the panes, and the
/// wood type draws it opaque, so nothing of the door may cut.
#[test]
fn chateaus_door_draws_opaque() {
    let Some(fs) = vcod_common::testing::game_fs() else {
        return;
    };
    let name = "textures/austria/doors/wood@chateau_exteriordoor";
    let mut img = assets::load_material_image(&fs, &ShaderLib::load(&fs), name);
    assert!(assets::has_cutout_alpha(&img));
    assert!(world_type_draws_opaque(&fs, name));
    assets::force_opaque(&mut img);
    assert!(!assets::has_cutout_alpha(&img));
    assert!(!world_type_draws_opaque(
        &fs,
        "textures/austria/background/foliage_clamp@truck_single"
    ));
}

/// A prop skin draws one-sided unless its template says `cull none`,
/// as UO's `*_cullnone` types do.
#[test]
fn a_prop_skin_is_two_sided_only_by_its_template() {
    let Some(fs) = vcod_common::testing::game_fs() else {
        return;
    };
    assert!(!prop_skin_two_sided(&fs, "metal_masked@fordsteer.dds"));
    assert!(!prop_skin_two_sided(&fs, "kwagon.tga"));
    let cullnone = fs.list_prefix("skins/foliage_masked_cullnone@");
    if let Some(skin) = cullnone.first() {
        assert!(prop_skin_two_sided(&fs, &skin["skins/".len()..]), "{skin}");
    }
}

/// mp_chateau's staff car: `metal_env@staffcar.tga` keeps its
/// reflectivity in alpha, which `fs_prop`'s cut read as a mask and so
/// dropped the body. Its template blends the skin over the
/// environment map, so it goes through the stages with the skin bound.
#[test]
fn a_prop_skin_draws_as_its_template_says() {
    let Some(fs) = vcod_common::testing::game_fs() else {
        return;
    };
    let skin = "metal_env@staffcar.tga";
    assert!(assets::has_cutout_alpha(&assets::load_skin_image(
        &fs, skin
    )));
    let PropSkinDraw::Stages(sh) = prop_skin_draw(&fs, skin) else {
        panic!("metal_env takes the stage path");
    };
    assert_eq!(sh.stages.len(), 2);
    assert!(matches!(
        &sh.stages[1].bundles[0].image,
        ImageRef::Path(p) if p == "skins/metal_env@staffcar.tga"
    ));
    assert!(matches!(
        prop_skin_draw(&fs, "glass_blend_in@mercedeswindow.dds"),
        PropSkinDraw::Stages(_)
    ));
    assert!(matches!(
        prop_skin_draw(&fs, "metal_masked@fordsteer.dds"),
        PropSkinDraw::Cut
    ));
    assert!(matches!(
        prop_skin_draw(&fs, "metal@staffcartread.dds"),
        PropSkinDraw::Opaque
    ));
    assert!(matches!(
        prop_skin_draw(&fs, "kwagon.tga"),
        PropSkinDraw::Opaque
    ));
    // a typed skin without its template draws retail's default shader
    assert!(matches!(
        prop_skin_draw(&fs, "no_such_type@staffcar.tga"),
        PropSkinDraw::Stages(sh) if sh.name == "<default>"
    ));
}

#[test]
fn world_draws_follow_the_sorted_list_with_a_shaders_stages_together() {
    let call = |pass| DrawCall {
        bind_group: 0,
        pass,
    };
    // batch 0: implicit opaque; 1: an opaque base with an additive glow
    // stage; 2: an alphaFunc cutout scripted `sort blend`; 3: a plain
    // opaque scripted shader whose texture loaded before batch 0's
    let calls = [
        call(Pass::Opaque),
        call(Pass::Stage),
        call(Pass::Stage),
        call(Pass::Stage),
    ];
    let stages = vec![vec![], vec![0, 1], vec![2], vec![3]];
    let keys = [
        (SORT_OPAQUE, 1),
        (SORT_OPAQUE, 2),
        (SORT_BLEND, 3),
        (SORT_OPAQUE, 0),
    ];
    let draws: Vec<DrawRange> = (0..4)
        .map(|batch| DrawRange {
            batch,
            first: 0,
            count: 3,
        })
        .collect();
    let mut out = Vec::new();
    order_draws(&keys, &calls, &stages, &draws, &mut out);
    let got: Vec<_> = out
        .iter()
        .map(|fd| match fd.kind {
            DrawRef::Legacy(_) => (fd.src, None, fd.sort),
            DrawRef::Stage(sb) => (fd.src, Some(sb), fd.sort),
        })
        .collect();
    assert_eq!(
        got,
        [
            (3, Some(3), SORT_OPAQUE),
            (0, None, SORT_OPAQUE),
            // the glow stage right after its base, at the shader's sort
            (1, Some(0), SORT_OPAQUE),
            (1, Some(1), SORT_OPAQUE),
            // the cutout at its own sort, after an entity of sort 5
            (2, Some(2), SORT_BLEND),
        ]
    );
    let world: Vec<f32> = out.iter().map(|fd| fd.sort).collect();
    let items = merge_by_sort(&world, &[], &[SORT_OPAQUE, SORT_BLEND], &[]);
    use SortedItem::{Model, World};
    assert_eq!(
        items,
        [
            World(0),
            World(1),
            World(2),
            World(3),
            Model(0),
            World(4),
            Model(1)
        ]
    );
}

#[test]
fn entity_draws_sort_by_material_sort_then_load_order_then_entity() {
    let skin = |sort, order| EntSkin {
        stages: Vec::new(),
        lit: true,
        sort,
        order,
        cookie_caster: false,
        sun_caster: false,
        sun_cut: None,
    };
    let (blend, opaque_late, opaque_early) = (skin(15.0, 0), skin(5.0, 2), skin(5.0, 1));
    let surfs = [
        (0, 0, &blend),
        (0, 1, &opaque_late),
        (1, 0, &opaque_early),
        (2, 0, &opaque_late),
    ];
    let got: Vec<_> = sorted_ent_draws(surfs.into_iter(), std::iter::empty())
        .iter()
        .map(|d| (d.inst, d.surf))
        .collect();
    assert_eq!(got, [(1, 0), (0, 1), (2, 0), (0, 0)]);
}

#[test]
fn the_gun_interleaves_with_the_world_and_entities_by_material() {
    let skin = |sort, order| EntSkin {
        stages: Vec::new(),
        lit: true,
        sort,
        order,
        cookie_caster: false,
        sun_caster: false,
        sun_cut: None,
    };
    let (flesh, rifle, glass, smoke) = (
        skin(SORT_OPAQUE, 0),
        skin(SORT_OPAQUE, 1),
        skin(SORT_BLEND, 2),
        skin(16.0, 3),
    );
    // a player in the rifle's material and a blend2 smoke card
    let ents = [(0, 0, &flesh), (0, 1, &rifle), (1, 0, &smoke)];
    // the gun: hands, the same rifle material, a blended scope lens
    let gun = [(0, 0, &glass), (0, 1, &rifle), (1, 0, &flesh)];
    let draws = sorted_ent_draws(ents.into_iter(), gun.into_iter());
    let got: Vec<_> = draws.iter().map(|d| (d.gun, d.inst, d.surf)).collect();
    assert_eq!(
        got,
        [
            // one material: the entities, then the gun (a later number)
            (false, 0, 0),
            (true, 1, 0),
            (false, 0, 1),
            (true, 0, 1),
            (true, 0, 0),
            (false, 1, 0),
        ]
    );

    // World ocean and opaque, then the models of sort 5, then an
    // opaque-class see-through cutout; the dlights after the last opaque
    // world draw, ahead of the models. The gun's lens (blend) draws
    // before a world blend2 and the smoke (blend2), not after every
    // world translucent.
    use SortedItem::{Model, World};
    let world = [3.0, SORT_OPAQUE, SORT_OPAQUE, 7.0, SORT_BLEND, 16.0];
    let sorts: Vec<f32> = draws.iter().map(|d| d.sort).collect();
    let items = merge_by_sort(&world, &[], &sorts, &[]);
    assert_eq!(
        items,
        [
            World(0),
            World(1),
            World(2),
            Model(0),
            Model(1),
            Model(2),
            Model(3),
            World(3),
            World(4),
            Model(4),
            World(5),
            Model(5)
        ]
    );
    assert_eq!(dlight_slot(&items, &world), 3);
    assert_eq!(dlight_slot(&[], &[]), 0);
}

/// The SSAO split follows the last draw of any list sorting up to
/// decal, past an opaque effect, a mark run and a decal model, before every blend.
#[test]
fn ssao_splits_after_the_last_opaque_or_decal_draw() {
    let world = [SORT_OPAQUE, SORT_DECAL, SORT_BLEND];
    let models = [SORT_OPAQUE, SORT_DECAL, SORT_ADDITIVE];
    let fx = [SORT_OPAQUE, SORT_BLEND];
    let marks = [SORT_DECAL];
    let items = merge_by_sort(&world, &marks, &models, &fx);
    // world 0, model 0, fx 0, world 1, mark 0, model 1 | world 2, fx 1, model 2
    assert_eq!(ssao_split(&items, &world, &marks, &models, &fx), 6);
    assert_eq!(ssao_split(&[], &[], &[], &[], &[]), 0);
    let blends = [SORT_BLEND];
    let only = merge_by_sort(&blends, &[], &[], &[]);
    assert_eq!(ssao_split(&only, &blends, &[], &[], &[]), 0);
}

#[test]
fn a_model_handle_from_an_unloaded_map_indexes_nothing() {
    let h = ModelHandle::first_map(2);
    assert_eq!(h.live(0, 5), Some(2));
    // the next map's table, after one unload_world, already has 5 handles
    assert_eq!(h.live(1, 5), None);
    assert_eq!(h.live(0, 2), None);
}
