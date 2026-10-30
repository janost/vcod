//! CoD4's live draw path, as a joined session hands it over: the
//! install's iwd search path (which reads as CoD2's), the zone's material
//! library, the view weapon posed by `cod4_viewmodel` and a body, through
//! `Renderer::set_viewmodel` and `set_dynamic_models` as `cod4_client`
//! calls them. With `VCOD_C4_SHOTS=<dir>` each frame is written there as
//! a TGA. Needs `COD4_DIR` and an adapter with BC textures (lavapipe
//! will do); passes vacuously without either.

use super::*;
use crate::cod4_viewmodel::{Cod4View, ViewPs};
use glam::Vec3;

const SIZE: (u32, u32) = (640, 384);
/// 4x MSAA, `r_aaSamples` 2-16's path (its CoD4 default is 1), so the
/// tests cover the resolve and the multisampled copies.
const SAMPLES: u32 = 4;
/// The playtest's sunny spot on mp_crash (F3 `cod4: 246 -224 192 yaw 57`).
const SUNNY: Vec3 = Vec3::new(246.0, -224.0, 192.0);
/// A spot the light grid puts out of the sun (its blocks' alpha 0).
const SHADE: Vec3 = Vec3::new(768.0, 0.0, 192.0);

struct Live {
    _slot: std::sync::MutexGuard<'static, ()>,
    r: Renderer,
    errors: pipeline_check::Errors,
    fs: Pk3Fs,
    map: crate::cod4_fly::Cod4Map,
}

fn live(map: &str, weapons: &[&str], models: &[&str]) -> Option<Live> {
    live_with(map, weapons, models, TextureSettings::default())
}

/// [`live`] with the images loaded under `textures`, as a renderer
/// `main` builds after a `vid_restart` or `r_applyPicmip`.
fn live_with(
    map: &str,
    weapons: &[&str],
    models: &[&str],
    textures: TextureSettings,
) -> Option<Live> {
    let install = vcod_cod4::testing::cod4_install()?;
    let _slot = pipeline_check::live_session_slot();
    let dir = install.root().to_path_buf();
    let fs = crate::cod4_fly::open_fs(&dir, None, None).unwrap();
    let (mut r, errors) = Renderer::headless_sized(&fs, false, SAMPLES, SIZE)?;
    r.textures = textures;
    r.set_texture_filter(textures.filter);
    r.set_gamma(1.0, crate::gamma::overbright_bits(fs.game(), false, true));
    let ents = crate::cod4_fly::EntityAssets {
        models: models.iter().map(|s| s.to_string()).collect(),
        weapons: weapons.iter().map(|s| s.to_string()).collect(),
    };
    let map = crate::cod4_fly::load(&dir, None, map, &ents).unwrap();
    r.set_material_lib(map.materials.clone());
    r.load_world(&map.bsp, &fs).unwrap();
    // the map's vision set, as `cod4_client::look_frame` hands it over
    r.set_level_look(Some(map.look.at(1000, false)));
    Some(Live {
        _slot,
        r,
        errors,
        fs,
        map,
    })
}

/// One frame from `eye` along `yaw` and `pitch` (degrees, pitch up
/// positive), at `time` seconds;
/// the frame's BGRA bytes, written as `<name>.tga` under `VCOD_C4_SHOTS`.
fn shot(
    l: &mut Live,
    (eye, yaw, pitch): (Vec3, f32, f32),
    time: f32,
    vm: Option<VmDraw>,
    name: &str,
) -> Vec<u8> {
    let fov = 80.0;
    let (yaw, pitch) = (yaw.to_radians(), pitch.to_radians());
    let (fwd, _, up) = crate::camera::basis(yaw, pitch);
    let frame = Frame {
        view_proj: crate::camera::view_proj_from(eye, yaw, pitch, 0.0, fov, l.r.aspect()),
        eye,
        fwd,
        up,
        time,
        fov_x: fov,
        cull: CullMode::On,
        hud_lines: Vec::new(),
    };
    l.r.render(frame, vm.map(|v| VmDraw { fov_x: fov, ..v }));
    let (w, h, bgra) = l.r.read_offscreen().expect("offscreen target");
    let errors = l.errors.lock().unwrap();
    assert!(errors.is_empty(), "{name}: {errors:?}");
    if let Some(dir) = std::env::var_os("VCOD_C4_SHOTS") {
        let rgba: Vec<u8> = bgra
            .chunks(4)
            .flat_map(|p| [p[2], p[1], p[0], 255])
            .collect();
        let path = std::path::Path::new(&dir).join(format!("{name}.tga"));
        image::save_buffer(path, &rgba, w, h, image::ExtendedColorType::Rgba8).unwrap();
    }
    bgra
}

/// Pixels that differ between two frames, and their summed difference.
fn diff(a: &[u8], b: &[u8]) -> (usize, u64) {
    a.chunks(4).zip(b.chunks(4)).fold((0, 0), |(n, s), (p, q)| {
        let d: u64 = (0..3).map(|i| u64::from(p[i].abs_diff(q[i]))).sum();
        (n + usize::from(d > 0), s + d)
    })
}

/// The MP5 in the hands, idle, as `cod4_client::view_weapon` hands it
/// over (its models set on the renderer), lit from `light_origin`.
fn view_weapon(l: &mut Live, light_origin: Vec3) -> VmDraw {
    let list = vec!["mp5_mp".to_string()];
    let ps = ViewPs {
        pm_type: 0,
        weapon: 1,
        ammoclip: [30; 128],
        ..Default::default()
    };
    let out = Cod4View::default().frame(&ps, &list, None, &l.map.weapons, &l.map.models, 0);
    let frame = out.frame.expect("the MP5 draws");
    let models = frame.models.expect("its models, on the first frame");
    l.r.set_viewmodel(&l.fs, &models);
    VmDraw {
        light_origin,
        ..frame.draw
    }
}

/// The view weapon and a body, lit off the light grid at the lighting
/// origin the session hands over: the same frame drawn with the sunny
/// spot's origin and with a shaded one differs on the gun and the body
/// (cod4-world.md 3.3). The session's iwd search path reads as CoD2's,
/// which once sent every CoD4 model material down CoD2's unlit path.
#[test]
fn cod4_live_gun_and_body_take_the_light_grid() {
    let body = "body_mp_usmc_assault";
    let Some(mut l) = live("mp_crash", &["mp5_mp"], &[body]) else {
        return;
    };
    let view = (SUNNY, 57.0, 0.0);
    let handle =
        l.r.upload_dynamic_model(&l.fs, &l.map.models[body])
            .expect("the body uploads");
    let (fwd, _, _) = crate::camera::basis(57f32.to_radians(), 0.0);
    let feet = SUNNY + fwd * 90.0 - Vec3::Z * 60.0;
    let place = |light_origin: Vec3| DynamicModelInstance {
        model: handle,
        transform: glam::Mat4::from_translation(feet)
            * glam::Mat4::from_rotation_z((57f32 + 180.0).to_radians()),
        bones: None,
        light_origin,
        shader_time: None,
    };
    let mut frames = Vec::new();
    for (name, o) in [("sun", SUNNY), ("shade", SHADE)] {
        l.r.set_dynamic_models(&[place(o)]);
        let vm = view_weapon(&mut l, o);
        frames.push(shot(
            &mut l,
            view,
            1.0,
            Some(vm),
            &format!("crash_lit_{name}"),
        ));
    }
    let (n, sum) = diff(&frames[0], &frames[1]);
    let (w, h) = SIZE;
    eprintln!(
        "sun against shade: {n} of {} pixels differ, by {sum}",
        w * h
    );
    // the gun and the body cover a few percent of the frame each
    assert!(n > (w * h / 50) as usize, "{n} pixels differ");
    let brightness = |f: &[u8]| -> u64 {
        f.chunks(4)
            .map(|p| u64::from(p[0]) + u64::from(p[1]) + u64::from(p[2]))
            .sum()
    };
    assert!(
        brightness(&frames[0]) > brightness(&frames[1]),
        "the sunny frame is brighter"
    );
}

/// Summed absolute differences between neighbouring pixels' brightness,
/// across and down: how much fine detail a frame shows.
fn detail(f: &[u8]) -> u64 {
    let (w, h) = (SIZE.0 as usize, SIZE.1 as usize);
    let lum = |x: usize, y: usize| -> i64 {
        let p = &f[4 * (y * w + x)..];
        i64::from(p[0]) + i64::from(p[1]) + i64::from(p[2])
    };
    let mut sum = 0;
    for y in 0..h - 1 {
        for x in 0..w - 1 {
            sum += lum(x, y).abs_diff(lum(x + 1, y)) + lum(x, y).abs_diff(lum(x, y + 1));
        }
    }
    sum
}

/// A sunlit cinderblock wall by mp_crash's first spawn
/// (`wc/me_cinderblock_wall2`, `wc_l_sm_r0c0n0s0`) shows its normal map
/// under the vision set: the frame with the map's materials as loaded
/// holds more fine detail than with every normal map left out, and most
/// of its pixels differ.
#[test]
fn cod4_live_cinderblock_wall_shows_relief() {
    let Some(mut l) = live("mp_crash", &[], &[]) else {
        return;
    };
    let (o, yaw) = l.map.spawn;
    let view = (Vec3::from(o) + Vec3::Z * 60.0, yaw + 90.0, 0.0);
    let relief = shot(&mut l, view, 1.0, None, "crash_wall_relief");
    let flat = vcod_common::material::MaterialLib::from_materials(
        l.map.bsp.materials.iter().filter_map(|m| {
            let mut sh = l.map.materials.get(&m.name)?.clone();
            if let Some(c) = sh.phong.as_mut().and_then(|p| p.cod4.as_mut()) {
                c.normal_map = false;
            }
            Some((sh, l.map.materials.image(&m.name).map(str::to_string)))
        }),
        0,
    );
    l.r.set_material_lib(flat);
    let Live { r, map, fs, .. } = &mut l;
    r.load_world(&map.bsp, fs).unwrap();
    let flat = shot(&mut l, view, 1.0, None, "crash_wall_flat");
    let (n, _) = diff(&relief, &flat);
    let (a, b) = (detail(&relief), detail(&flat));
    eprintln!("relief against flat: {n} pixels differ; detail {a} against {b}");
    assert!(n > (SIZE.0 * SIZE.1 / 2) as usize, "{n} pixels differ");
    assert!(a > b + b / 5, "detail {a} against {b}");
}

/// mp_bog's open water (`wc/aftermath_water`, `water_l_sun`) moves: two
/// frames half a second apart differ where it is drawn, its height map
/// stepped at the frame's time (cod4-world.md 9.8).
#[test]
fn cod4_live_bog_water_ripples_over_time() {
    let Some(mut l) = live("mp_bog", &[], &[]) else {
        return;
    };
    let bsp = &l.map.bsp;
    let water = bsp
        .materials
        .iter()
        .position(|m| m.name == "wc/aftermath_water")
        .expect("mp_bog's water");
    let world =
        crate::cod4_fly::load_world(vcod_cod4::testing::cod4_install().unwrap().root(), "mp_bog")
            .unwrap();
    // a water vertex with ground under it, so the water shows
    let open = bsp
        .soups
        .iter()
        .filter(|s| usize::from(s.material) == water)
        .flat_map(|s| &bsp.verts[s.first_vertex as usize..][..usize::from(s.vertex_count)])
        .map(|v| Vec3::from(v.pos))
        .find(|p| {
            let z = Vec3::ZERO;
            let (from, to) = (*p + Vec3::Z * 200.0, *p - Vec3::Z * 200.0);
            let t = world
                .clip
                .trace(from, to, z, z, vcod_cod4::lightgrid::SIGHT_MASK);
            t.fraction < 1.0 && 200.0 - 400.0 * t.fraction < -4.0
        })
        .expect("open water");
    // standing in it, looking along it
    let view = (open + Vec3::Z * 60.0, 180.0, -30.0);
    let a = shot(&mut l, view, 1.0, None, "bog_water_a");
    let b = shot(&mut l, view, 1.5, None, "bog_water_b");
    let (n, sum) = diff(&a, &b);
    eprintln!("water at {open}: {n} pixels differ, by {sum}");
    assert!(n > (SIZE.0 * SIZE.1 / 4) as usize, "{n} pixels differ");
}

/// The texture page on mp_crash (cod4-front-end.md 9.3): the world loaded
/// at `r_picmip`, `r_picmip_bump` and `r_picmip_spec` 2 shows less fine
/// detail than at 0, and a floor seen at a grazing angle shows more under
/// `r_texFilterAnisoMin` 16 than 1, both trilinear.
#[test]
fn cod4_live_picmip_blurs_and_anisotropy_sharpens() {
    let Some(mut l) = live("mp_crash", &[], &[]) else {
        return;
    };
    let (o, yaw) = l.map.spawn;
    let wall = (Vec3::from(o) + Vec3::Z * 60.0, yaw + 90.0, 0.0);
    let floor = (SUNNY, 57.0, -12.0);
    let full = detail(&shot(&mut l, wall, 1.0, None, "crash_picmip0"));
    let tri = crate::texture_settings::TextureFilter::by_name("GL_LINEAR_MIPMAP_LINEAR").unwrap();
    l.r.set_texture_filter(tri.cod4_aniso(1, 16));
    let flat = detail(&shot(&mut l, floor, 1.0, None, "crash_aniso1"));
    l.r.set_texture_filter(tri.cod4_aniso(16, 16));
    let aniso = detail(&shot(&mut l, floor, 1.0, None, "crash_aniso16"));
    eprintln!("floor detail: aniso 1 {flat}, aniso 16 {aniso}");
    assert!(aniso > flat + flat / 20, "aniso 1 {flat}, aniso 16 {aniso}");
    let mut t = TextureSettings::default();
    t.set_cod4_picmip([2.0; 3]);
    // the first session gives up its slot
    drop(l);
    let Some(mut l) = live_with("mp_crash", &[], &[], t) else {
        return;
    };
    let blurred = detail(&shot(&mut l, wall, 1.0, None, "crash_picmip2"));
    eprintln!("wall detail: picmip 0 {full}, picmip 2 {blurred}");
    assert!(
        blurred < full - full / 10,
        "picmip 0 {full}, picmip 2 {blurred}"
    );
}

/// `r_vc_depthprepass` (cod4-client-perf.md 12) draws the frame byte for
/// byte as without it: mp_crossfire's long view (C4-96's 150k triangles
/// over 22-24 cells) and mp_crash's sunny spot with the gun and a body,
/// each under the sun shadow. Prints each frame's encode-to-readback time
/// both ways (lavapipe's, CPU rasterization).
#[test]
fn cod4_live_depth_prepass_draws_the_same_pixels() {
    let body = "body_mp_usmc_assault";
    type View<'a> = (&'a str, &'a [&'a str], &'a [&'a str], (Vec3, f32, f32));
    let views: [View; 2] = [
        (
            "mp_crossfire",
            &[],
            &[],
            (Vec3::new(5232.0, -888.0, 240.0), 225.0, 0.0),
        ),
        ("mp_crash", &["mp5_mp"], &[body], (SUNNY, 57.0, 0.0)),
    ];
    for (map, weapons, models, view) in views {
        let Some(mut l) = live(map, weapons, models) else {
            return;
        };
        l.r.set_sun_shadow(true, vcod_cod4::sunshadow::Settings::default());
        let mut vm = None;
        if let Some(&model) = models.first() {
            let handle =
                l.r.upload_dynamic_model(&l.fs, &l.map.models[model])
                    .expect("the body uploads");
            let (fwd, _, _) = crate::camera::basis(view.1.to_radians(), 0.0);
            let feet = view.0 + fwd * 90.0 - Vec3::Z * 60.0;
            l.r.set_dynamic_models(&[DynamicModelInstance {
                model: handle,
                transform: glam::Mat4::from_translation(feet)
                    * glam::Mat4::from_rotation_z((view.1 + 180.0).to_radians()),
                bones: None,
                light_origin: view.0,
                shader_time: None,
            }]);
            vm = Some(view_weapon(&mut l, view.0));
        }
        let gun = |vm: &Option<VmDraw>| {
            vm.as_ref().map(|v| VmDraw {
                bone_sets: v.bone_sets.clone(),
                ..*v
            })
        };
        let frame = |l: &mut Live, on: bool| {
            l.r.set_depth_prepass(on);
            // the sun shadow's caches settle on the first frame
            let name = format!("{map}_prepass_{}", u8::from(on));
            let first = shot(l, view, 1.0, gun(&vm), &name);
            let t = std::time::Instant::now();
            const RUNS: u32 = 3;
            for _ in 0..RUNS {
                assert_eq!(shot(l, view, 1.0, gun(&vm), &name), first);
            }
            (first, t.elapsed().as_secs_f32() * 1000.0 / RUNS as f32)
        };
        let (off, off_ms) = frame(&mut l, false);
        let (on, on_ms) = frame(&mut l, true);
        let drawn = l.r.prepass_drawn.get();
        let (off2, _) = frame(&mut l, false);
        assert_eq!(off, off2, "{map}: the frame holds still");
        let (n, sum) = diff(&off, &on);
        eprintln!(
            "{map}: prepass off {off_ms:.1} ms, on {on_ms:.1} ms a frame; {} triangles, {drawn} prepass draws; {n} pixels differ by {sum}",
            l.r.vis_counts().tris.0
        );
        assert!(drawn > 0, "{map}: the prepass drew nothing");
        assert!(off == on, "{map}: {n} pixels differ by {sum}");
    }
}

/// `tools/cod4_perf.sh`'s sweep (cod4-client-perf.md 25) at its held view
/// on mp_crossfire: each renderer switch it turns off moves pixels,
/// `r_specular`, `r_vc_normalMaps`, `r_fog` (under a stand-in exp fog)
/// and, where the vision set has one, `r_vc_film`, and the frame back at the defaults is the first one
/// byte for byte.
#[test]
fn cod4_live_sweep_switches_change_the_frame() {
    let Some(mut l) = live("mp_crossfire", &[], &[]) else {
        return;
    };
    // `setviewpos 5232 -888 180 225 0`: the eye 60 over the origin
    let view = (Vec3::new(5232.0, -888.0, 240.0), 225.0, 0.0);
    // the session hands the level's `setExpFog` over off configstring 9;
    // an exp fog in its place, which `r_fog 0` must clear
    l.r.set_fog_now(Some(vcod_common::net::protocol::FogParams {
        near: 0.0,
        far: 0.0,
        density: 0.0005,
        color: [0.6, 0.65, 0.7],
        fade_ms: 0,
        start: 200.0,
    }));
    let frame = |l: &mut Live, options: Cod4Options, name: &str| {
        l.r.set_cod4_options(options);
        shot(l, view, 1.0, None, name)
    };
    let base = frame(&mut l, Cod4Options::default(), "sweep_base");
    let film = l.r.level_look.is_some_and(|k| k.film.is_some());
    let off = Cod4Options::default();
    let mut switches = vec![
        (
            "specular0",
            Cod4Options {
                specular: false,
                ..off
            },
        ),
        (
            "normalmap0",
            Cod4Options {
                normal_maps: false,
                ..off
            },
        ),
        ("fog0", Cod4Options { fog: false, ..off }),
    ];
    if film {
        switches.push(("film0", Cod4Options { film: false, ..off }));
    }
    for (name, options) in switches {
        let f = frame(&mut l, options, &format!("sweep_{name}"));
        let (n, sum) = diff(&base, &f);
        eprintln!("{name}: {n} pixels differ by {sum}");
        assert!(n > 0, "{name} moved no pixel");
    }
    let again = frame(&mut l, Cod4Options::default(), "sweep_base2");
    assert!(base == again, "the defaults draw the first frame again");
    eprintln!("film in the vision set: {film}");
}
