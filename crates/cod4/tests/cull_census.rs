//! What the cell walk draws across a map's views: `CULL_CENSUS=<map>`
//! looks from every spawn point (8 yaws, 3 pitches, the client's 80
//! degrees across 16:9) and prints the triangles and soups the renderer
//! would draw (the static models at the LOD `StaticLod::pick` names),
//! world and static model apart, the mean, the median and the heaviest
//! 5% of the views, and the heaviest views themselves. `CULL_OLD=1`
//! walks without 1.7's small-portal rule (`r_portalMinClipArea`,
//! `r_portalMinRecurseDepth`), as vcod did before (cod4-client-perf.md
//! 35). `CULL_TURN=<degrees>` then turns from each spawn point in steps
//! of that, level, and prints the walk's mean time and how many steps
//! kept the last step's cell set and its soup set.

use glam::{Mat4, Vec3};
use std::sync::Arc;
use vcod_cod4::map::Map;
use vcod_cod4::mapworld::{LightmapImages, to_mapworld};
use vcod_cod4::source::Cod4Source;
use vcod_cod4::testing::cod4_install;
use vcod_common::mapworld::{MapWorld, cod4_lod_ramp};
use vcod_common::vis::{Frustum, WorldVis};

fn load(name: &str) -> Option<MapWorld> {
    load_with_techsets(name).map(|(w, _)| w)
}

/// The map's world and, per material, its techset's name.
fn load_with_techsets(name: &str) -> Option<(MapWorld, Vec<String>)> {
    let install = cod4_install()?;
    let mut src = Cod4Source::open(&install).unwrap();
    let (map, walked) = Map::load(&install, "english", name).unwrap();
    src.add_zone(Arc::new(walked.assets));
    let lightmaps: Vec<LightmapImages> = map
        .lightmaps
        .iter()
        .map(|[p, s]| LightmapImages {
            primary: src.image(p.as_deref().unwrap()).unwrap(),
            secondary: src.image(s.as_deref().unwrap()).unwrap(),
        })
        .collect();
    let techsets = map
        .materials
        .iter()
        .map(|m| m.techset.clone().unwrap_or_default())
        .collect();
    Some((to_mapworld(&map, &lightmaps).unwrap().0, techsets))
}

fn view(eye: Vec3, yaw: f32, pitch: f32) -> Mat4 {
    let (y, p) = (yaw.to_radians(), pitch.to_radians());
    let fwd = Vec3::new(p.cos() * y.cos(), p.cos() * y.sin(), p.sin());
    let aspect = 16.0 / 9.0;
    let fov_y = 2.0 * ((40f32).to_radians().tan() / aspect).atan();
    let proj = glam::camera::rh::proj::directx::perspective(fov_y, aspect, 4.0, 30000.0);
    proj * glam::camera::rh::view::look_to_mat4(eye, fwd, Vec3::Z)
}

/// The soups the renderer draws at one view, the static models at their
/// picked LOD; and per soup whether it is a static model's.
fn drawn_soups(
    w: &MapWorld,
    vis: &WorldVis,
    eye: Vec3,
    yaw: f32,
    pitch: f32,
) -> (Vec<bool>, Vec<bool>, usize) {
    let m = view(eye, yaw, pitch);
    let mut v = vis.visible(eye, &Frustum::from_view_proj(m), Some(m));
    let smodel = lod_pick(w, &mut v.soups, eye);
    let cells = v.cells.iter().filter(|&&c| c).count();
    (v.soups, smodel, cells)
}

/// Keeps each static model's soups at the LOD the renderer picks at
/// `eye`; per soup whether it is a static model's.
fn lod_pick(w: &MapWorld, soups: &mut [bool], eye: Vec3) -> Vec<bool> {
    let fov_y = 2.0 * ((40f32).to_radians().tan() * 9.0 / 16.0).atan();
    let (scale, bias) = cod4_lod_ramp(fov_y.to_degrees(), 1.0, 0.0);
    let mut smodel = vec![false; w.soups.len()];
    for sl in &w.static_lods {
        let (Some(&(_, first, _)), Some(&(_, last, n))) = (sl.lods.first(), sl.lods.last()) else {
            continue;
        };
        let all = first as usize..(last + n) as usize;
        smodel[all.clone()].fill(true);
        if !soups[all.clone()].iter().any(|&s| s) {
            continue;
        }
        let keep = sl.pick(eye.into(), scale, bias).map(|l| {
            let (_, f, n) = sl.lods[l];
            f as usize..(f + n) as usize
        });
        for si in all {
            soups[si] = keep.as_ref().is_some_and(|k| k.contains(&si));
        }
    }
    smodel
}

/// The renderer's drawn set at one view: `(world tris, smodel tris,
/// soups, cells)`.
pub fn drawn(
    w: &MapWorld,
    vis: &WorldVis,
    eye: Vec3,
    yaw: f32,
    pitch: f32,
) -> (u64, u64, u64, usize) {
    let (soups, smodel, cells) = drawn_soups(w, vis, eye, yaw, pitch);
    let (mut wt, mut st, mut n) = (0, 0, 0);
    for (s, &on) in soups.iter().enumerate() {
        if !on {
            continue;
        }
        n += 1;
        let t = u64::from(w.soups[s].index_count / 3);
        if smodel[s] { st += t } else { wt += t }
    }
    (wt, st, n, cells)
}

pub fn spawn_eyes(w: &MapWorld) -> Vec<Vec3> {
    vcod_common::bsp::entity_block_pairs(&w.entities)
        .iter()
        .filter(|e| {
            e.iter()
                .any(|(k, v)| k == "classname" && v.starts_with("mp_") && v.contains("spawn"))
        })
        .filter_map(|e| {
            let o = &e.iter().find(|(k, _)| k == "origin")?.1;
            let v: Vec<f32> = o
                .split_whitespace()
                .filter_map(|x| x.parse().ok())
                .collect();
            Some(Vec3::new(v[0], v[1], v[2] + 60.0))
        })
        .collect()
}

/// `(tris, world, smodel, soups, cells, eye, yaw, pitch)`.
type Row = (u64, u64, u64, u64, usize, Vec3, i32, f32);

#[test]
fn cull_census() {
    let Ok(name) = std::env::var("CULL_CENSUS") else {
        return;
    };
    let Some(mut w) = load(&name) else {
        return;
    };
    if std::env::var("CULL_OLD").is_ok_and(|v| v == "1") {
        w.portal_min_clip_area = 0.0;
        w.portal_min_recurse_depth = 0;
    }
    let vis = WorldVis::build(&w);
    if let Some(step) = std::env::var("CULL_TURN")
        .ok()
        .and_then(|v| v.parse::<f32>().ok())
    {
        turn(&name, &w, &vis, step);
    }
    let mut rows = Vec::new();
    for eye in spawn_eyes(&w) {
        for yaw in (0..360).step_by(45) {
            for pitch in [-20.0, 0.0, 25.0] {
                let (wt, st, n, c) = drawn(&w, &vis, eye, yaw as f32, pitch);
                rows.push((wt + st, wt, st, n, c, eye, yaw, pitch));
            }
        }
    }
    rows.sort_by_key(|r| r.0);
    let k = rows.len();
    let mean = |f: &dyn Fn(&Row) -> u64, r: &[Row]| {
        r.iter().map(f).sum::<u64>() as f64 / r.len().max(1) as f64
    };
    let top = &rows[k - k / 20..];
    for (label, r) in [("all", &rows[..]), ("p95+", top)] {
        println!(
            "{name} {label}: views {} tris {:.0} world {:.0} smodel {:.0} soups {:.0} cells {:.1}",
            r.len(),
            mean(&|x| x.0, r),
            mean(&|x| x.1, r),
            mean(&|x| x.2, r),
            mean(&|x| x.3, r),
            r.iter().map(|x| x.4).sum::<usize>() as f64 / r.len().max(1) as f64
        );
    }
    println!("{name} median tris {}", rows[k / 2].0);
    for r in top.iter().rev().take(12) {
        println!(
            "  {} world {} smodel {} soups {} cells {} eye {:?} yaw {} pitch {}",
            r.0,
            r.1,
            r.2,
            r.3,
            r.4,
            r.5.to_array(),
            r.6,
            r.7
        );
    }
}

/// A view's draw batches, near enough: its soups' (material, lightmap).
type Batches = std::collections::BTreeSet<(u16, u16)>;

/// `CULL_TURN`'s sweep: from each spawn eye a full turn in `step`
/// degree steps.
fn turn(name: &str, w: &MapWorld, vis: &WorldVis, step: f32) {
    let n = (360.0 / step).round() as usize;
    let (mut views, mut same_cells, mut same_soups) = (0usize, 0usize, 0usize);
    let mut same_batches = 0usize;
    let batches = |soups: &[bool]| -> Batches {
        soups
            .iter()
            .enumerate()
            .filter(|&(_, &on)| on)
            .map(|(s, _)| (w.soups[s].material, w.soups[s].lightmap))
            .collect()
    };
    let mut cells_drawn = 0usize;
    let mut nodes = 0usize;
    let (mut tris, mut whole_tris) = (0u64, 0u64);
    let mut walk = std::time::Duration::ZERO;
    let count = |soups: &[bool]| -> u64 {
        soups
            .iter()
            .enumerate()
            .filter(|&(_, &on)| on)
            .map(|(s, _)| u64::from(w.soups[s].index_count / 3))
            .sum()
    };
    for eye in spawn_eyes(w) {
        let mut last: Option<(Vec<bool>, Vec<bool>, Batches)> = None;
        for i in 0..=n {
            let m = view(eye, i as f32 * step, 0.0);
            let t = std::time::Instant::now();
            let v = vis.visible(eye, &Frustum::from_view_proj(m), Some(m));
            walk += t.elapsed();
            nodes += v.stats.nodes_tested;
            let (soups, _, _) = drawn_soups(w, vis, eye, i as f32 * step, 0.0);
            cells_drawn += v.cells.iter().filter(|&&c| c).count();
            let mut whole = vis.cell_soups(&v.cells);
            lod_pick(w, &mut whole, eye);
            tris += count(&soups);
            whole_tris += count(&whole);
            let b = batches(&soups);
            if let Some((c, s, lb)) = &last {
                views += 1;
                same_cells += usize::from(*c == v.cells);
                same_soups += usize::from(*s == soups);
                same_batches += usize::from(*lb == b);
            }
            last = Some((v.cells, soups, b));
        }
    }
    println!(
        "{name} turn {step} deg: steps {views} walk {:.3} ms nodes {:.0} cells {:.1} same cells {:.1}% same soups {:.1}% same batches {:.1}% tris {:.0} cells' whole {:.0}",
        walk.as_secs_f64() * 1000.0 / (views + spawn_eyes(w).len()).max(1) as f64,
        nodes as f64 / (views + spawn_eyes(w).len()).max(1) as f64,
        cells_drawn as f64 / (views + spawn_eyes(w).len()).max(1) as f64,
        100.0 * same_cells as f64 / views.max(1) as f64,
        100.0 * same_soups as f64 / views.max(1) as f64,
        100.0 * same_batches as f64 / views.max(1) as f64,
        tris as f64 / (views + spawn_eyes(w).len()).max(1) as f64,
        whole_tris as f64 / (views + spawn_eyes(w).len()).max(1) as f64,
    );
}

#[test]
fn cull_census_view() {
    let Ok(spec) = std::env::var("CULL_VIEW") else {
        return;
    };
    let v: Vec<&str> = spec.split_whitespace().collect();
    let Some(w) = load(v[0]) else {
        return;
    };
    let f = |i: usize| v[i].parse::<f32>().unwrap();
    let (eye, yaw, pitch) = (Vec3::new(f(1), f(2), f(3)), f(4), f(5));
    let vis = WorldVis::build(&w);
    let m = view(eye, yaw, pitch);
    let vv = vis.visible(eye, &Frustum::from_view_proj(m), Some(m));
    let fov_y = 2.0 * ((40f32).to_radians().tan() * 9.0 / 16.0).atan();
    let (scale, bias) = cod4_lod_ramp(fov_y.to_degrees(), 1.0, 0.0);
    let mut rows = Vec::new();
    for sl in &w.static_lods {
        let (Some(&(_, first, _)), Some(&(_, last, n))) = (sl.lods.first(), sl.lods.last()) else {
            continue;
        };
        if !vv.soups[first as usize..(last + n) as usize]
            .iter()
            .any(|&s| s)
        {
            continue;
        }
        let Some(l) = sl.pick(eye.into(), scale, bias) else {
            continue;
        };
        let (_, f, n) = sl.lods[l];
        let tris: u32 = (f..f + n)
            .map(|s| u32::from(w.soups[s as usize].index_count / 3))
            .sum();
        let d = (Vec3::from(sl.origin) - eye).length();
        rows.push((
            tris,
            d,
            l,
            sl.lods.iter().map(|x| x.0).collect::<Vec<_>>(),
            sl.cull_dist,
            sl.scale,
        ));
    }
    rows.sort_by_key(|a| std::cmp::Reverse(a.0));
    println!(
        "{} smodels, {} tris",
        rows.len(),
        rows.iter().map(|r| r.0).sum::<u32>()
    );
    for r in rows.iter().take(40) {
        println!(
            "  tris {} dist {:.0} lod {} dists {:?} cull {} scale {}",
            r.0, r.1, r.2, r.3, r.4, r.5
        );
    }
}

/// `CULL_TECHSETS=<map>`: the triangles each techset draws summed over
/// [`cull_census`]' views, the heaviest first, and per view the distinct
/// batches (material, lightmap, probe, primary light) against the
/// distinct materials among the drawn soups: the world draws against
/// what one draw per material would be (cod4-client-perf.md 21).
#[test]
fn cull_techsets() {
    let Ok(name) = std::env::var("CULL_TECHSETS") else {
        return;
    };
    let Some((w, techsets)) = load_with_techsets(&name) else {
        return;
    };
    let vis = WorldVis::build(&w);
    let mut by_set: std::collections::BTreeMap<&str, u64> = Default::default();
    let (mut views, mut batches, mut materials) = (0u64, 0u64, 0u64);
    for eye in spawn_eyes(&w) {
        for yaw in (0..360).step_by(45) {
            for pitch in [-20.0, 0.0, 25.0] {
                let (soups, _, _) = drawn_soups(&w, &vis, eye, yaw as f32, pitch);
                let mut b = std::collections::BTreeSet::new();
                let mut m = std::collections::BTreeSet::new();
                for (s, _) in soups.iter().enumerate().filter(|(_, on)| **on) {
                    let soup = &w.soups[s];
                    let set = techsets[soup.material as usize].as_str();
                    *by_set.entry(set).or_default() += u64::from(soup.index_count / 3);
                    let probe = w.soup_probes.get(s).copied().unwrap_or(0);
                    let light = w.soup_lights.get(s).copied().unwrap_or(0);
                    b.insert((soup.material, soup.lightmap, probe, light));
                    m.insert(soup.material);
                }
                views += 1;
                batches += b.len() as u64;
                materials += m.len() as u64;
            }
        }
    }
    println!(
        "{name}: {views} views, mean {:.0} batches, {:.0} materials",
        batches as f64 / views as f64,
        materials as f64 / views as f64
    );
    let mut sets: Vec<_> = by_set.into_iter().collect();
    sets.sort_by_key(|s| std::cmp::Reverse(s.1));
    let total: u64 = sets.iter().map(|s| s.1).sum();
    for (set, t) in sets.iter().take(20) {
        println!("  {set} {:.1}%", 100.0 * *t as f64 / total as f64);
    }
}

/// `CULL_DRAWS=<map>`: the world's draw calls per view, ours against a
/// model of 1.7's (cod4-client-perf.md 22). Ours: one per batch (material,
/// lightmap tile, probe, primary light). 1.7's: the visible surfaces in
/// sorted order, a draw surface per run of one key (material, lightmap,
/// probe, primary light) cut every 128 surfaces
/// (`R_AddAllBspDrawSurfacesRangeCamera`, iw3mp.exe 0x63c4d0), and under
/// `r_pretess` one draw per change of `firstVertex`, lightmap or probe
/// inside it (`R_PreTessBspDrawSurfs`, 0x63c220). Static model soups are
/// counted apart: 1.7 draws them through its own path. Over the spawn
/// views, and at `CULL_DRAWS_VIEWS="x y z yaw pitch; ..."` one by one.
#[test]
fn cull_draws() {
    let Ok(name) = std::env::var("CULL_DRAWS") else {
        return;
    };
    let Some(install) = cod4_install() else {
        return;
    };
    let mut src = Cod4Source::open(&install).unwrap();
    let (map, walked) = Map::load(&install, "english", &name).unwrap();
    src.add_zone(Arc::new(walked.assets));
    let lightmaps: Vec<LightmapImages> = map
        .lightmaps
        .iter()
        .map(|[p, s]| LightmapImages {
            primary: src.image(p.as_deref().unwrap()).unwrap(),
            secondary: src.image(s.as_deref().unwrap()).unwrap(),
        })
        .collect();
    let (w, built) = to_mapworld(&map, &lightmaps).unwrap();
    let sorted = &map.sorted_surfaces;
    let identity = sorted.iter().enumerate().all(|(k, &s)| k == s as usize);
    let mut shared_fv = 0usize;
    for k in 1..sorted.len() {
        let (a, b) = (
            &map.surfaces[sorted[k - 1] as usize],
            &map.surfaces[sorted[k] as usize],
        );
        if a.first_vertex == b.first_vertex {
            shared_fv += 1;
        }
    }
    println!(
        "{name}: {} surfaces, {} sorted (identity {identity}), {} loose, {} tiles over {} lightmaps, {shared_fv} sorted neighbours share firstVertex",
        map.surfaces.len(),
        sorted.len(),
        built.loose_surfaces,
        built.tiles,
        map.lightmaps.len()
    );
    let vis = WorldVis::build(&w);
    // (ours world, ours smodel, ours world under 1.7's lightmap key,
    // 1.7's draw surfaces, 1.7's draws, 1.7 draws without pretess)
    let count = |eye: Vec3, yaw: f32, pitch: f32| -> [u64; 6] {
        let (soups, smodel, _) = drawn_soups(&w, &vis, eye, yaw, pitch);
        let (mut ours, mut ours_sm, mut ours_lm) = (
            std::collections::BTreeSet::new(),
            std::collections::BTreeSet::new(),
            std::collections::BTreeSet::new(),
        );
        for (s, _) in soups.iter().enumerate().filter(|(_, on)| **on) {
            let soup = &w.soups[s];
            let probe = w.soup_probes.get(s).copied().unwrap_or(0);
            let light = w.soup_lights.get(s).copied().unwrap_or(0);
            if smodel[s] {
                ours_sm.insert((soup.material, soup.lightmap, probe, light));
            } else {
                ours.insert((soup.material, soup.lightmap, probe, light));
                if s < sorted.len() {
                    ours_lm.insert((
                        soup.material,
                        map.surfaces[sorted[s] as usize].lightmap,
                        probe,
                        light,
                    ));
                }
            }
        }
        let (mut surfs, mut draws, mut plain) = (0u64, 0u64, 0u64);
        let mut run: Vec<usize> = Vec::new();
        let mut key = None;
        let mut flush = |run: &mut Vec<usize>| {
            if run.is_empty() {
                return;
            }
            surfs += 1;
            let mut last = None;
            let mut last_plain: Option<(u32, u32)> = None;
            for &k in run.iter() {
                let s = &map.surfaces[sorted[k] as usize];
                let at = (s.first_vertex, s.lightmap, s.reflection_probe);
                if last != Some(at) {
                    draws += 1;
                    last = Some(at);
                }
                // R_DrawTrianglesLit: one draw per run of indices that follow on
                let follows =
                    last_plain.is_some_and(|(fv, end)| fv == s.first_vertex && end == s.base_index);
                if !follows {
                    plain += 1;
                }
                last_plain = Some((s.first_vertex, s.base_index + 3 * u32::from(s.tri_count)));
            }
            run.clear();
        };
        for k in (0..sorted.len()).filter(|&k| soups[k] && !smodel[k]) {
            let s = &map.surfaces[sorted[k] as usize];
            let kk = (s.material, s.lightmap, s.reflection_probe, s.primary_light);
            if key != Some(kk) || run.len() >= 128 {
                flush(&mut run);
                key = Some(kk);
            }
            run.push(k);
        }
        flush(&mut run);
        [
            ours.len() as u64,
            ours_sm.len() as u64,
            ours_lm.len() as u64,
            surfs,
            draws,
            plain,
        ]
    };
    let label = "ours world, ours smodel, ours world on 1.7's lightmaps, 1.7 draw surfaces, 1.7 draws, 1.7 draws without r_pretess";
    if let Ok(spec) = std::env::var("CULL_DRAWS_VIEWS") {
        for v in spec.split(';') {
            let v: Vec<f32> = v.split_whitespace().map(|x| x.parse().unwrap()).collect();
            let c = count(Vec3::new(v[0], v[1], v[2]), v[3], v[4]);
            println!("{name} view {v:?}: {c:?} ({label})");
        }
    }
    let mut sum = [0u64; 6];
    let mut views = 0u64;
    for eye in spawn_eyes(&w) {
        for yaw in (0..360).step_by(45) {
            for pitch in [-20.0, 0.0, 25.0] {
                let c = count(eye, yaw as f32, pitch);
                for i in 0..6 {
                    sum[i] += c[i];
                }
                views += 1;
            }
        }
    }
    println!(
        "{name}: {views} views, mean {:?} ({label})",
        sum.map(|s| (s as f64 / views as f64 * 10.0).round() / 10.0)
    );
}
