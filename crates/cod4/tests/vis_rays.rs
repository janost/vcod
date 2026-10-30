//! The cell walk against rays: from an eye, every soup whose front face a
//! ray through the view meets first must be in the walk's visible set. A
//! soup the frustum alone keeps but the walk drops, seen along a clear
//! ray, is geometry the portal walk lost (`cod4-world.md` 9.1, "The portal
//! walk"). `VIS_RAYS=<map>` sweeps that map's spawn points and prints every
//! miss (`VIS_RING=1`: points 200 and 400 units round each spawn instead,
//! `VIS_CELLGRID=1`: a grid over every cell; `VIS_GRID=1` casts 16 x 9 rays
//! a view, not 32 x 18). Eyes inside a wall or a model are skipped. What a
//! sweep still prints are sight lines through fences and hedges the map's
//! cells have no portal for, which retail cannot draw either.

use glam::{Mat4, Vec3};
use std::sync::Arc;
use vcod_cod4::map::Map;
use vcod_cod4::mapworld::{LightmapImages, to_mapworld};
use vcod_cod4::source::Cod4Source;
use vcod_cod4::testing::cod4_install;
use vcod_common::mapworld::MapWorld;
use vcod_common::vis::{Frustum, WorldVis, soup_bounds};

fn load(name: &str) -> Option<MapWorld> {
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
    Some(to_mapworld(&map, &lightmaps).unwrap().0)
}

/// The client's view: Z up, yaw 0 along +X, 80 degrees across 16:9.
fn view(eye: Vec3, yaw: f32, pitch: f32) -> (Mat4, Vec3, Vec3, Vec3) {
    let (y, p) = (yaw.to_radians(), pitch.to_radians());
    let fwd = Vec3::new(p.cos() * y.cos(), p.cos() * y.sin(), p.sin());
    let right = Vec3::new(y.sin(), -y.cos(), 0.0);
    let up = right.cross(fwd).normalize();
    let aspect = 16.0 / 9.0;
    let fov_y = 2.0 * ((40f32).to_radians().tan() / aspect).atan();
    let proj = glam::camera::rh::proj::directx::perspective(fov_y, aspect, 4.0, 30000.0);
    let m = proj * glam::camera::rh::view::look_to_mat4(eye, fwd, Vec3::Z);
    (m, fwd, right, up)
}

fn ray_box(o: Vec3, inv: Vec3, (lo, hi): (Vec3, Vec3)) -> Option<f32> {
    let t0 = (lo - o) * inv;
    let t1 = (hi - o) * inv;
    let tmin = t0.min(t1).max_element().max(0.0);
    let tmax = t0.max(t1).min_element();
    (tmin <= tmax).then_some(tmin)
}

fn ray_tri(o: Vec3, d: Vec3, a: Vec3, b: Vec3, c: Vec3) -> Option<f32> {
    let (e1, e2) = (b - a, c - a);
    let p = d.cross(e2);
    let det = e1.dot(p);
    if det.abs() < 1e-9 {
        return None;
    }
    let inv = 1.0 / det;
    let s = o - a;
    let u = s.dot(p) * inv;
    if !(0.0..=1.0).contains(&u) {
        return None;
    }
    let q = s.cross(e1);
    let v = d.dot(q) * inv;
    if v < 0.0 || u + v > 1.0 {
        return None;
    }
    let t = e2.dot(q) * inv;
    (t > 0.01).then_some(t)
}

/// Soups first hit by a grid of rays through the view that the walk left
/// out: `(soup, hits)`.
fn misses(
    w: &MapWorld,
    vis: &WorldVis,
    bounds: &[(Vec3, Vec3)],
    skip: &[bool],
    eye: Vec3,
    yaw: f32,
    pitch: f32,
) -> Vec<(usize, usize)> {
    let (m, fwd, right, up) = view(eye, yaw, pitch);
    let frustum = Frustum::from_view_proj(m);
    let walked = vis.visible(eye, &frustum, Some(m));
    let all = vis.visible_fallback(&frustum);
    let cand: Vec<usize> = (0..w.soups.len())
        .filter(|&s| all.soups[s] && !skip[s])
        .collect();
    let mut out: Vec<(usize, usize)> = Vec::new();
    let mut backs = 0;
    let (nx, ny) = std::env::var("VIS_GRID").map_or((32, 18), |_| (16, 9));
    let tx = 40f32.to_radians().tan();
    let ty = tx * 9.0 / 16.0;
    for j in 0..ny {
        for i in 0..nx {
            let sx = ((i as f32 + 0.5) / nx as f32 * 2.0 - 1.0) * tx;
            let sy = ((j as f32 + 0.5) / ny as f32 * 2.0 - 1.0) * ty;
            let d = (fwd + right * sx + up * sy).normalize();
            let inv = d.recip();
            let mut hits: Vec<(f32, usize)> = cand
                .iter()
                .filter_map(|&s| ray_box(eye, inv, bounds[s]).map(|t| (t, s)))
                .collect();
            hits.sort_by(|a, b| a.0.total_cmp(&b.0));
            let mut best: Option<(f32, usize)> = None;
            let mut any: Option<(f32, bool)> = None;
            for (t, s) in hits {
                if best.is_some_and(|(b, _)| t > b) && any.is_some_and(|(b, _)| t > b) {
                    break;
                }
                let so = &w.soups[s];
                let (fv, fi) = (so.first_vertex as usize, so.first_index as usize);
                let ix = &w.indices[fi..fi + so.index_count as usize];
                for tri in ix.as_chunks::<3>().0 {
                    let p = |k: u16| Vec3::from(w.verts[fv + k as usize].pos);
                    // a back face is culled: the ray sees past it
                    let n = Vec3::from(w.verts[fv + tri[0] as usize].normal);
                    let Some(t) = ray_tri(eye, d, p(tri[0]), p(tri[1]), p(tri[2])) else {
                        continue;
                    };
                    let back = n.dot(d) > 0.0;
                    if any.is_none_or(|(b, _)| t < b) {
                        any = Some((t, back));
                    }
                    if !back && best.is_none_or(|(b, _)| t < b) {
                        best = Some((t, s));
                    }
                }
            }
            if any.is_some_and(|(_, back)| back) {
                backs += 1;
            }
            if let Some((_, s)) = best
                && !walked.soups[s]
            {
                match out.iter_mut().find(|(o, _)| *o == s) {
                    Some(e) => e.1 += 1,
                    None => out.push((s, 1)),
                }
            }
        }
    }
    // an eye that meets back faces is inside a wall or a model
    if backs * 20 > nx * ny {
        return Vec::new();
    }
    out
}

/// The first face a ray meets, any soup not skipped: `(t, back face)`.
fn first_hit(
    w: &MapWorld,
    bounds: &[(Vec3, Vec3)],
    skip: &[bool],
    o: Vec3,
    d: Vec3,
) -> Option<(f32, bool)> {
    let inv = d.recip();
    let mut best: Option<(f32, bool)> = None;
    for s in 0..w.soups.len() {
        if skip[s] || ray_box(o, inv, bounds[s]).is_none_or(|t| best.is_some_and(|(b, _)| t > b)) {
            continue;
        }
        let so = &w.soups[s];
        let (fv, fi) = (so.first_vertex as usize, so.first_index as usize);
        for tri in w.indices[fi..fi + so.index_count as usize]
            .as_chunks::<3>()
            .0
        {
            let p = |k: u16| Vec3::from(w.verts[fv + k as usize].pos);
            if let Some(t) = ray_tri(o, d, p(tri[0]), p(tri[1]), p(tri[2]))
                && best.is_none_or(|(b, _)| t < b)
            {
                let n = Vec3::from(w.verts[fv + tri[0] as usize].normal);
                best = Some((t, n.dot(d) > 0.0));
            }
        }
    }
    best
}

/// Static model LODs past the first, which no ray should meet.
fn coarse_lods(w: &MapWorld) -> Vec<bool> {
    let mut skip = vec![false; w.soups.len()];
    for sl in &w.static_lods {
        for &(_, f, n) in sl.lods.iter().skip(1) {
            skip[f as usize..(f + n) as usize].fill(true);
        }
    }
    skip
}

#[test]
fn spawn_sweep() {
    let Ok(name) = std::env::var("VIS_RAYS") else {
        return;
    };
    let Some(w) = load(&name) else {
        return;
    };
    let vis = WorldVis::build(&w);
    let bounds: Vec<(Vec3, Vec3)> = (0..w.soups.len()).map(|s| soup_bounds(&w, s)).collect();
    let skip = coarse_lods(&w);
    let ents = vcod_common::bsp::entity_block_pairs(&w.entities);
    let eyes: Vec<Vec3> = ents
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
        .collect();
    let ring: Vec<Vec3> = eyes
        .iter()
        .flat_map(|&e| {
            (0..8).flat_map(move |k| {
                let a = (k as f32 * 45.0).to_radians();
                [200.0, 400.0].map(|r| e + Vec3::new(a.cos() * r, a.sin() * r, 0.0))
            })
        })
        .filter(|&e| vis.cell_for_point(e).is_some())
        .collect();
    let eyes: Vec<Vec3> = if std::env::var("VIS_RING").is_ok() {
        ring
    } else {
        eyes
    };
    let grid: Vec<Vec3> = w
        .cells
        .iter()
        .enumerate()
        .skip(1)
        .flat_map(|(ci, c)| {
            let step = ((c.maxs[0] - c.mins[0]) * (c.maxs[1] - c.mins[1]) / 40.0)
                .sqrt()
                .max(96.0);
            let mut out = Vec::new();
            let mut x = c.mins[0] + step * 0.5;
            while x < c.maxs[0] {
                let mut y = c.mins[1] + step * 0.5;
                while y < c.maxs[1] {
                    for dz in [60.0, 180.0] {
                        let p = Vec3::new(x, y, c.mins[2] + dz);
                        if p.z < c.maxs[2]
                            && vis.cell_for_point(p) == Some(ci)
                            && first_hit(&w, &bounds, &skip, p, -Vec3::Z)
                                .is_some_and(|(t, back)| !back && t < 250.0)
                            && first_hit(&w, &bounds, &skip, p, Vec3::Z)
                                .is_none_or(|(_, back)| !back)
                        {
                            out.push(p);
                        }
                    }
                    y += step;
                }
                x += step;
            }
            out
        })
        .collect();
    let eyes: Vec<Vec3> = if std::env::var("VIS_CELLGRID").is_ok() {
        grid
    } else {
        eyes
    };
    println!("{} eyes", eyes.len());
    let mut total = 0;
    for eye in eyes {
        for yaw in (0..360).step_by(45) {
            for pitch in [-20.0, 0.0, 25.0] {
                for (s, n) in misses(&w, &vis, &bounds, &skip, eye, yaw as f32, pitch) {
                    total += 1;
                    let (lo, hi) = bounds[s];
                    println!(
                        "eye {:?} yaw {yaw} pitch {pitch}: soup {s} {} x{n} cell {:?} box {lo:?}..{hi:?}",
                        eye.to_array(),
                        w.materials[w.soups[s].material as usize].name,
                        vis.cell_for_point(eye),
                    );
                }
            }
        }
    }
    println!("{name}: {total} misses");
}

/// mp_overgrown's two-storey house: from the ground floor (cell 24) the
/// stair hole's portal shows the upper floor's walls (cell 21). The
/// recursive walk spent its visit cap in the sky cell first and dropped
/// them, the playtest's missing house tops.
#[test]
fn overgrown_stair_hole_shows_the_upper_floor() {
    let Some(w) = load("mp_overgrown") else {
        return;
    };
    let vis = WorldVis::build(&w);
    let bounds: Vec<(Vec3, Vec3)> = (0..w.soups.len()).map(|s| soup_bounds(&w, s)).collect();
    let skip = coarse_lods(&w);
    let eye = Vec3::new(-718.5, -3784.5, -48.0);
    assert_eq!(vis.cell_for_point(eye), Some(24));
    let (m, ..) = view(eye, 225.0, 25.0);
    let walked = vis.visible(eye, &Frustum::from_view_proj(m), Some(m));
    assert!(walked.cells[21], "the upper floor's cell");
    assert!(
        walked.stats.cells_visited < 30,
        "{}",
        walked.stats.cells_visited
    );
    let missed = misses(&w, &vis, &bounds, &skip, eye, 225.0, 25.0);
    assert!(missed.is_empty(), "{missed:?}");
}
