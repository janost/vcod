//! Which stock MP maps draw a sun, and what of it: the active sky's `.sun`
//! file on a map with a lump-30 sun light, its disc sprite and its flare
//! image (docs/research/cod11-shader-scripts.md section 8, "The sun").

use vcod_common::{assets, bsp, pk3::Pk3Fs, shader::ShaderLib, sun};

/// `(map, sky, disc sprite, flare image)` per stock MP map; `None` where
/// retail draws none.
fn census(fs: &Pk3Fs) -> Vec<(String, String, Option<String>, Option<String>)> {
    let lib = ShaderLib::load(fs).materials();
    let mut maps: Vec<String> = fs
        .find_maps()
        .into_iter()
        .filter(|m| m.starts_with("mp_"))
        .collect();
    maps.sort();
    maps.dedup();
    maps.iter()
        .map(|map| {
            let entry = fs.resolve_map(map).unwrap();
            let b = bsp::parse(&fs.read(&entry).unwrap()).unwrap();
            let kinds = sun::material_kinds(&lib, &b);
            let sky = sun::active_sky(&lib, &b, &kinds).map(|(n, _)| n);
            let s = sky
                .as_ref()
                .and_then(|n| lib.get(n))
                .and_then(|sh| sun::load(fs, sh, &b));
            let disc = s
                .as_ref()
                .and_then(|s| s.file.sprite.clone())
                .filter(|p| assets::resolve_bundle_image(fs, p).is_some());
            let flare = s
                .as_ref()
                .and_then(|s| s.file.flare.as_ref())
                .and_then(|f| {
                    let path = lib.image(f).unwrap_or(f).to_string();
                    assets::resolve_bundle_image(fs, &path).map(|_| path)
                });
            (map.clone(), sky.unwrap_or_default(), disc, flare)
        })
        .collect()
}

type Row = (&'static str, Option<&'static str>, Option<&'static str>);

/// The 1.1 maps that draw any of the sun; the other eight name no `.sun`
/// file a stock pak ships (mp_hurtgen's sky names none) and draw nothing.
const SUNS_11: &[Row] = &[
    ("mp_carentan", None, Some("gfx/sun/global_flare")),
    (
        "mp_chateau",
        Some("gfx/sun/moon_full"),
        Some("gfx/sun/moon_flare"),
    ),
    ("mp_dawnville", None, Some("gfx/sun/global_flare")),
    ("mp_harbor", None, Some("gfx/sun/global_flare")),
];

/// United Offensive's, under 1.1's rules (no UO client binary to read).
const SUNS_UO: &[Row] = &[
    (
        "mp_berlin",
        Some("gfx/sun/moon_full"),
        Some("gfx/sun/moon_flare"),
    ),
    ("mp_carentan", None, Some("gfx/sun/global_flare")),
    ("mp_cassino", None, Some("gfx/sun/global_flare")),
    (
        "mp_chateau",
        Some("gfx/sun/moon_full"),
        Some("gfx/sun/moon_flare"),
    ),
    ("mp_dawnville", None, Some("gfx/sun/global_flare")),
    (
        "mp_foy",
        Some("gfx/sun/global"),
        Some("gfx/sun/global_flare"),
    ),
    ("mp_harbor", None, Some("gfx/sun/global_flare")),
    ("mp_neuville", None, Some("gfx/sun/global_flare")),
    ("mp_rhinevalley", None, Some("gfx/sun/global_flare")),
    (
        "mp_sicily",
        Some("gfx/sun/lowsun"),
        Some("gfx/sun/lowsun_flare"),
    ),
    ("mp_stalingrad", None, Some("gfx/sun/global_flare")),
    ("mp_tigertown", None, Some("gfx/sun/global_flare")),
    ("mp_uo_carentan", None, Some("gfx/sun/global_flare")),
    ("mp_uo_dawnville", None, Some("gfx/sun/global_flare")),
    ("mp_uo_harbor", None, Some("gfx/sun/global_flare")),
    ("mp_uo_stanjel", None, Some("gfx/sun/global_flare")),
];

/// Runs over the install's stock paks alone (`--vanilla`), so a third-party
/// `.sun` file cannot move it; 1.1's table at a 1.1 install, UO's at a UO one.
#[test]
fn stock_mp_suns() {
    let dir = vcod_common::testing::game_dir();
    let (base, want, total) = if vcod_common::testing::uo_fs().is_some() {
        (dir.join("uo"), SUNS_UO, 35)
    } else if vcod_common::testing::game_fs_11().is_some() {
        (dir.join("main"), SUNS_11, 12)
    } else {
        return;
    };
    let Ok(fs) = Pk3Fs::open_search(&base, None, None, true) else {
        return;
    };
    let rows = census(&fs);
    assert_eq!(rows.len(), total, "{rows:?}");
    assert!(
        rows.iter().all(|r| !r.1.is_empty()),
        "every MP map has a sky: {rows:?}"
    );
    let got: Vec<(&str, Option<&str>, Option<&str>)> = rows
        .iter()
        .filter(|r| r.2.is_some() || r.3.is_some())
        .map(|r| (r.0.as_str(), r.2.as_deref(), r.3.as_deref()))
        .collect();
    assert_eq!(got, want);
}

/// mp_carentan's spawns, all outdoors: a sky soup comes through the cell
/// walk however the eye looks (a sky soup's box spans the map, so the
/// frustum passes it), so the sun is added there (`sun::sky_drawn`).
#[test]
fn carentan_spawns_walk_to_the_sky() {
    use glam::Vec3;
    let Some(fs) = vcod_common::testing::game_fs_11() else {
        return;
    };
    let b = bsp::parse(&fs.read("maps/mp/mp_carentan.bsp").unwrap()).unwrap();
    let lib = ShaderLib::load(&fs).materials();
    let kinds = sun::material_kinds(&lib, &b);
    let world = vcod_common::vis::WorldVis::build(&b);
    let sky: Vec<usize> = (0..b.models[0].num_soups as usize)
        .filter(|&si| kinds[b.soups[si].material as usize] == vcod_common::mesh::MaterialKind::Sky)
        .collect();
    assert!(!sky.is_empty());
    let spawns: Vec<Vec3> = bsp::entity_blocks(&b.entities)
        .iter()
        .filter(|e| e.get("classname").map(String::as_str) == Some("mp_teamdeathmatch_spawn"))
        .filter_map(|e| {
            let v: Vec<f32> = e["origin"]
                .split_whitespace()
                .filter_map(|t| t.parse().ok())
                .collect();
            (v.len() == 3).then(|| Vec3::new(v[0], v[1], v[2] + 60.0))
        })
        .collect();
    assert!(spawns.len() > 10, "{}", spawns.len());
    let sees_sky = |eye: Vec3, dir: Vec3| {
        let view = glam::camera::rh::view::look_to_mat4(eye, dir, Vec3::X);
        let proj =
            glam::camera::rh::proj::directx::perspective(80f32.to_radians(), 1.33, 4.0, 10000.0);
        let vp = proj * view;
        let v = world.visible(
            eye,
            &vcod_common::vis::Frustum::from_view_proj(vp),
            Some(vp),
        );
        sky.iter().any(|&si| v.soups[si])
    };
    for dir in [Vec3::Z, -Vec3::Z] {
        let n = spawns.iter().filter(|&&e| sees_sky(e, dir)).count();
        assert_eq!(n, spawns.len(), "looking {dir}");
    }
}
