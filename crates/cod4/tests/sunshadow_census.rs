//! Which stock world and static model materials cast into the sun's shadow
//! map (a `build_shadowmap_color` state, `vcod_cod4::state::caster_state`)
//! and with what state, over every stock map; `--nocapture` prints the
//! techsets by how they cast (docs/research/cod4-world.md 3.6).
//!
//! Every caster draws both faces with no polygon offset of its own state's
//! kind past the decal one, alpha-tested at 128 exactly when its set is a
//! `t0` (or the `ambient_t0c0` and `mc_shadowcaster` sets); the lit `r0`
//! and `t0` sets, both `shadowcaster` sets and `ambient_t0c0` cast, and no
//! blended, additive, unlit, effect, sky, water or flag set does.

use std::collections::BTreeMap;
use vcod_cod4::map::Map;
use vcod_cod4::state::{AlphaTest, Cull};
use vcod_cod4::techset::{Blend, Family};
use vcod_cod4::testing::cod4_install;

#[test]
fn casters_by_techset() {
    let Some(install) = cod4_install() else {
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
    let mut by_set: BTreeMap<String, BTreeMap<String, usize>> = BTreeMap::new();
    for name in &maps {
        let (map, _) = Map::load(&install, "english", name).unwrap();
        let mut used = vec![false; map.materials.len()];
        for s in &map.surfaces {
            used[s.material as usize] = true;
        }
        for m in map.models.iter().flat_map(|m| &m.surfaces) {
            used[m.material as usize] = true;
        }
        for (m, _) in map.materials.iter().zip(&used).filter(|(_, u)| **u) {
            let set = m.techset.as_deref().unwrap_or_default();
            let Some(class) = m.class else {
                assert!(m.caster_state.is_none(), "{name}: {}", m.name);
                continue;
            };
            let lit_solid = class.family == Family::Lit
                && matches!(class.blend, Blend::Replace | Blend::AlphaTest)
                && !class.flag;
            let casts =
                lit_solid || set.ends_with("_shadowcaster") || set.ends_with("ambient_t0c0");
            assert_eq!(
                m.caster_state.is_some(),
                casts,
                "{name}: {} ({set})",
                m.name
            );
            if let Some(st) = m.caster_state {
                assert_eq!(st.cull, Cull::None, "{name}: {}", m.name);
                assert!(st.polygon_offset <= 1, "{name}: {}", m.name);
                let tested = class.blend == Blend::AlphaTest
                    || set == "mc_shadowcaster"
                    || set.ends_with("ambient_t0c0");
                let want = tested.then_some(AlphaTest::Ge128);
                assert_eq!(st.alpha_test, want, "{name}: {} ({set})", m.name);
            }
            let how = match m.caster_state {
                None => "none".to_owned(),
                Some(st) => format!(
                    "{:?} {:?} offset {}",
                    st.alpha_test, st.cull, st.polygon_offset
                ),
            };
            *by_set
                .entry(m.techset.clone().unwrap_or_default())
                .or_default()
                .entry(how)
                .or_default() += 1;
        }
    }
    for (set, hows) in &by_set {
        println!("{set}: {hows:?}");
    }
}

/// Every stock map's lit world surfaces by their `primaryLightIndex`: the
/// sun's, a spot or omni light past it, or 0; `--nocapture` prints the
/// counts (docs/research/cod4-world.md 3.5).
#[test]
fn world_surfaces_by_primary_light() {
    let Some(install) = cod4_install() else {
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
    let mut src = vcod_cod4::source::Cod4Source::open(&install).unwrap();
    for name in &maps {
        let (map, walked) = Map::load(&install, "english", name).unwrap();
        src.add_zone(std::sync::Arc::new(walked.assets));
        // each lightmap's primary: L8 as RGBA, its red the sun's visibility
        let primaries: Vec<(usize, Vec<u8>)> = map
            .lightmaps
            .iter()
            .map(|[p, _]| {
                let img = src.image(p.as_deref().unwrap()).unwrap();
                let vcod_common::assets::ImageData::Rgba8(px) = img.data else {
                    panic!("{name}: a compressed primary");
                };
                (img.width as usize, px)
            })
            .collect();
        src.pop_zone();
        let mut counts: BTreeMap<String, usize> = BTreeMap::new();
        // surfaces with no primary light whose vertices' primary texels
        // average past half
        let mut none_lit = 0usize;
        for s in &map.surfaces {
            let lit = map.materials[s.material as usize]
                .class
                .is_some_and(|c| c.family == Family::Lit && c.world);
            if !lit {
                continue;
            }
            let i = usize::from(s.primary_light);
            if i == 0 && s.lightmap != vcod_cod4::map::NO_LIGHTMAP {
                let (w, px) = &primaries[usize::from(s.lightmap)];
                let h = px.len() / 4 / w;
                let tris = s.base_index as usize..s.base_index as usize + 3 * s.tri_count as usize;
                let (mut sum, mut n) = (0u32, 0u32);
                for &k in &map.indices[tris] {
                    let v = &map.vertices[s.first_vertex as usize + usize::from(k)];
                    let x = ((v.lm_uv[0] * *w as f32) as usize).min(w - 1);
                    let y = ((v.lm_uv[1] * h as f32) as usize).min(h - 1);
                    sum += u32::from(px[4 * (y * w + x)]);
                    n += 1;
                }
                if n > 0 && sum / n > 128 {
                    none_lit += 1;
                }
            }
            let kind = if i == 0 {
                "none".to_owned()
            } else if i as u32 == map.sun_light_index {
                "sun".to_owned()
            } else {
                let l = &map.primary_lights[i];
                assert!((i as u32) > map.sun_light_index, "{name}: {i}");
                format!("kind {}", l.kind)
            };
            *counts.entry(kind).or_default() += 1;
        }
        let mut models: BTreeMap<String, usize> = BTreeMap::new();
        for m in &map.static_models {
            let i = u32::from(m.primary_light);
            let kind = match i {
                0 => "none",
                _ if i == map.sun_light_index => "sun",
                _ => "light",
            };
            *models.entry(kind.into()).or_default() += 1;
        }
        println!(
            "{name}: {counts:?}, no light yet primary past half {none_lit}; static models {models:?}"
        );
    }
}

/// The world's sun casters each partition draws from every spawn point's
/// standing view, looking along the spawn's yaw: the cells whose box
/// reaches the partition's square (C4-31's cull) against the cells the
/// partition's planes keep (`R_SetupShadowSurfacesDpvs`), at 512 and at
/// `CASTER_CELL` units. `--nocapture` prints the triangles a view draws
/// on average per map; `COD4_MAPS=a,b` keeps maps naming one of those.
/// The planes keep no more than the square (within a hundredth of all
/// casters) and never drop a cell around the eye; the static models'
/// camera-picked LODs cast fewer triangles than LOD 0.
#[test]
fn caster_culling_from_the_spawn_views() {
    use glam::{Mat4, Vec3};
    use vcod_cod4::mapworld::{LightmapImages, to_mapworld};
    use vcod_cod4::sunshadow::{self, CASTER_CELL, CasterGroup, Settings, View};
    let Some(install) = cod4_install() else {
        return;
    };
    let filter: Vec<String> = std::env::var("COD4_MAPS")
        .map(|v| v.split(',').map(str::to_owned).collect())
        .unwrap_or_else(|_| vec!["mp_crash".into(), "mp_bog".into()]);
    let mut src = vcod_cod4::source::Cod4Source::open(&install).unwrap();
    for name in &filter {
        let (map, walked) = Map::load(&install, "english", name).unwrap();
        src.add_zone(std::sync::Arc::new(walked.assets));
        let lightmaps: Vec<LightmapImages> = map
            .lightmaps
            .iter()
            .map(|[p, s]| LightmapImages {
                primary: src.image(p.as_deref().unwrap()).unwrap(),
                secondary: src.image(s.as_deref().unwrap()).unwrap(),
            })
            .collect();
        src.pop_zone();
        let (w, _) = to_mapworld(&map, &lightmaps).unwrap();
        let mut soup_model = vec![0u32; w.soups.len()];
        for (mi, m) in w.models.iter().enumerate().skip(1) {
            let first = m.first_soup as usize;
            for s in soup_model.iter_mut().skip(first).take(m.num_soups as usize) {
                *s = mi as u32;
            }
        }
        let to_sun = Vec3::from(w.sun.expect("a sun").dir);
        let bounds = (Vec3::from(w.models[0].mins), Vec3::from(w.models[0].maxs));
        let coarse = sunshadow::caster_groups(&w, &soup_model, 512.0);
        let fine = sunshadow::caster_groups(&w, &soup_model, CASTER_CELL);
        let tris = |g: &CasterGroup| g.indices.len() as u64 / 3;
        let all: u64 = fine.iter().map(tris).sum();
        let in_square = |g: &CasterGroup, vp: &Mat4| {
            let (mut lo, mut hi) = (glam::Vec2::splat(f32::MAX), glam::Vec2::splat(-f32::MAX));
            for i in 0..8 {
                let c = Vec3::select(
                    glam::BVec3::new(i & 1 == 0, i & 2 == 0, i & 4 == 0),
                    g.lo,
                    g.hi,
                );
                let p = vp.project_point3(c).truncate();
                lo = lo.min(p);
                hi = hi.max(p);
            }
            lo.x <= 1.0 && hi.x >= -1.0 && lo.y <= 1.0 && hi.y >= -1.0
        };
        // the static models: LOD 0 against the camera's pick (the rigid
        // ramp at the view's fov, 65 degrees vertical; cod4-world.md 3.6)
        let smodels = sunshadow::smodel_casters(&w);
        let (ramp_scale, ramp_bias) = vcod_common::mapworld::cod4_lod_ramp(65.0, 1.0, 0.0);
        let lod_tris = |m: &sunshadow::SmodelCaster, l: usize| -> u64 {
            m.lods[l].iter().map(|r| r.1.len() as u64 / 3).sum()
        };
        let mut smodel_sums = [0u64; 2];
        let (mut views, mut sums) = (0u64, [[0u64; 2]; 3]);
        for e in &map.entities {
            let get = |key: &str| e.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str());
            if !get("classname").is_some_and(|c| c.starts_with("mp_") && c.ends_with("_spawn")) {
                continue;
            }
            let Some(o) = get("origin").and_then(vcod_common::bsp::parse_vec3) else {
                continue;
            };
            let yaw = get("angles")
                .and_then(vcod_common::bsp::parse_vec3)
                .map_or(0.0, |a| a[1])
                .to_radians();
            let eye = Vec3::from(o) + Vec3::Z * 60.0;
            let fwd = Vec3::new(yaw.cos(), yaw.sin(), 0.0);
            let aspect = 16.0 / 9.0;
            let fov_y = 2.0 * ((80f32.to_radians() / 2.0).tan() / aspect).atan();
            let proj = glam::camera::rh::proj::directx::perspective(fov_y, aspect, 4.0, 60000.0);
            let vp = proj * glam::camera::rh::view::look_to_mat4(eye, fwd, Vec3::Z);
            let sh = sunshadow::setup(
                &View::of(vp, eye, fwd, 4.0),
                to_sun,
                bounds,
                &Settings::default(),
            );
            views += 1;
            for part in &sh.partitions {
                for m in smodels.iter().filter(|m| !part.cull.culls(m.lo, m.hi)) {
                    smodel_sums[0] += lod_tris(m, 0);
                    if let Some(l) = m.lod.pick(eye.into(), ramp_scale, ramp_bias) {
                        smodel_sums[1] += lod_tris(m, l);
                    }
                }
            }
            for (p, part) in sh.partitions.iter().enumerate() {
                sums[0][p] += coarse
                    .iter()
                    .filter(|g| in_square(g, &part.view_proj))
                    .map(tris)
                    .sum::<u64>();
                sums[1][p] += coarse
                    .iter()
                    .filter(|g| !part.cull.culls(g.lo, g.hi))
                    .map(tris)
                    .sum::<u64>();
                sums[2][p] += fine
                    .iter()
                    .filter(|g| !part.cull.culls(g.lo, g.hi))
                    .map(tris)
                    .sum::<u64>();
                // a cell around the eye: the near partition keeps it
                if p == 0 {
                    let around = |g: &&CasterGroup| g.lo.cmple(eye).all() && g.hi.cmpge(eye).all();
                    for g in fine.iter().filter(around) {
                        assert!(
                            !part.cull.culls(g.lo, g.hi),
                            "{name}: the cell around the eye at {o:?} culled"
                        );
                    }
                }
            }
        }
        assert!(views > 0, "{name}: no spawn points");
        // the picked LODs cast fewer triangles than LOD 0 everywhere
        assert!(smodel_sums[1] < smodel_sums[0], "{name}: {smodel_sums:?}");
        println!(
            "{name}: static model caster tris in the partitions a view, LOD 0 {}k, picked {}k",
            smodel_sums[0] / views / 1000,
            smodel_sums[1] / views / 1000
        );
        for (square, planes) in sums[0].iter().zip(&sums[1]) {
            assert!(*planes <= square + all * views / 100, "{name}");
        }
        let k = |v: u64| v / views / 1000;
        println!(
            "{name}: {views} views, {}k caster tris in {} groups of {CASTER_CELL} ({} of 512); \
             near/far drawn: square at 512 {}k/{}k, planes at 512 {}k/{}k, \
             planes at {CASTER_CELL} {}k/{}k",
            all / 1000,
            fine.len(),
            coarse.len(),
            k(sums[0][0]),
            k(sums[0][1]),
            k(sums[1][0]),
            k(sums[1][1]),
            k(sums[2][0]),
            k(sums[2][1]),
        );
    }
}

/// `R_CopyLightDefAttenuationImage`'s row: at zoom 1 the def's texels with
/// its first and last once more on each side; at zoom `z` (a page `z`
/// lightmaps wide) `z + z / 2` of the first, each pair lerped at `(2j + 1)
/// / 2z` rounding to nearest, then as many of the last.
fn copied_row(row: &[[u8; 4]], z: usize) -> Vec<[u8; 4]> {
    let (first, last) = (row[0], row[row.len() - 1]);
    if z == 1 {
        return [&[first], row, &[last]].concat();
    }
    let end = z + z / 2;
    let mut out = vec![first; end];
    for k in 0..row.len() - 1 {
        for lerp in (1..=2 * z).step_by(2) {
            out.push([0, 1, 2, 3].map(|c| {
                let (a, b) = (row[k][c] as usize, row[k + 1][c] as usize);
                ((z + lerp * b + (2 * z - lerp) * a) / (2 * z)) as u8
            }));
        }
    }
    out.extend(std::iter::repeat_n(last, end));
    out
}

/// The world's spot and omni falloff (cod4-world.md 3.5): every stock
/// lightmap page's secondary holds each light def's attenuation row in its
/// first row at `lmapLookupStart`, as `R_CopyLightDefAttenuationImage`
/// lays it out for the page's width, and the lookup at
/// `lightFalloffPlacement` (the page's linear, clamped sampler) reads it
/// within 1.5 / 255 of the def's own row through a linear clamped filter
/// (the zoomed copy's lerps round).
#[test]
fn secondary_lightmaps_hold_the_falloff_rows() {
    let Some(install) = cod4_install() else {
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
    let rgba = |img: &vcod_common::assets::Image| match &img.data {
        vcod_common::assets::ImageData::Rgba8(px) => px.as_chunks::<4>().0.to_vec(),
        _ => panic!("a compressed lightmap or falloff"),
    };
    let mut src = vcod_cod4::source::Cod4Source::open(&install).unwrap();
    let (mut checked, mut worst) = (0, 0f32);
    for name in &maps {
        let (map, walked) = Map::load(&install, "english", name).unwrap();
        let defs: Vec<(String, i32)> = walked
            .assets
            .light_defs
            .iter()
            .filter_map(|d| {
                let img = &walked.assets.images[d.image?.index()];
                Some((
                    img.name.trim_start_matches(',').to_string(),
                    d.lmap_lookup_start,
                ))
            })
            .collect();
        src.add_zone(std::sync::Arc::new(walked.assets));
        let rows: Vec<(Vec<[u8; 4]>, usize)> = defs
            .iter()
            .map(|(img, start)| {
                let a = src.image(img).unwrap();
                (rgba(&a)[..a.width as usize].to_vec(), *start as usize)
            })
            .collect();
        for [_, s] in &map.lightmaps {
            let page = src.image(s.as_deref().unwrap()).unwrap();
            let z = page.width as usize / 512;
            let texels = rgba(&page);
            for (row, start) in &rows {
                let copy = copied_row(row, z);
                let at = z * (start - 1);
                assert_eq!(
                    &texels[at..at + copy.len()],
                    &copy[..],
                    "{name}: {s:?} at {start}"
                );
                checked += 1;
                // retail's lookup against the def's row, both linear and
                // clamped, over the falloff's range
                let w = row.len() as f32;
                let lerp = |px: &[[u8; 4]], x: f32, c: usize| {
                    let x = (x - 0.5).clamp(0.0, px.len() as f32 - 1.0);
                    let (i, f) = (x.floor() as usize, x.fract());
                    let j = (i + 1).min(px.len() - 1);
                    (px[i][c] as f32 * (1.0 - f) + px[j][c] as f32 * f) / 255.0
                };
                for step in 0..=256 {
                    let t = step as f32 / 256.0;
                    let u = t * w / 512.0 + *start as f32 / 512.0;
                    for c in 0..3 {
                        let retail = lerp(&texels[..page.width as usize], u * page.width as f32, c);
                        worst = worst.max((retail - lerp(row, t * w, c)).abs());
                    }
                }
            }
        }
        src.pop_zone();
    }
    println!("{checked} falloff rows checked; the lookups differ by {worst} at most");
    assert!(checked > 0);
    assert!(worst <= 1.5 / 255.0 + 1e-6, "{worst}");
}
