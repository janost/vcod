//! Every stock MP map converts to a `MapWorld` that holds the invariants the
//! renderer, the cell vis and the collision model index by, and the
//! converted world draws cells and stops a fall at a spawn. Needs
//! `COD2_DIR`.

use glam::{Mat4, Vec3};
use vcod_common::mapworld::{LIGHTMAP_SIZE, MapWorld, NO_LIGHTMAP, Partition};
use vcod_common::testing;
use vcod_common::vis::{Frustum, WorldVis};

const MAPS: [&str; 15] = [
    "mp_breakout",
    "mp_brecourt",
    "mp_burgundy",
    "mp_carentan",
    "mp_dawnville",
    "mp_decoy",
    "mp_downtown",
    "mp_farmhouse",
    "mp_harbor",
    "mp_leningrad",
    "mp_matmata",
    "mp_railyard",
    "mp_rhine",
    "mp_toujane",
    "mp_trainstation",
];

fn within(first: u32, count: u32, len: usize) -> bool {
    first as u64 + count as u64 <= len as u64
}

fn check(map: &str, w: &MapWorld) {
    // Render geometry.
    assert_eq!(w.tangents.len(), w.verts.len(), "{map}");
    for p in &w.lightmaps {
        assert_eq!(p.len(), LIGHTMAP_SIZE * LIGHTMAP_SIZE * 3, "{map}");
    }
    assert!(!w.lightmaps.is_empty(), "{map}");
    for (i, s) in w.soups.iter().enumerate() {
        assert!((s.material as usize) < w.materials.len(), "{map} soup {i}");
        assert!(
            s.lightmap == NO_LIGHTMAP || (s.lightmap as usize) < w.lightmaps.len(),
            "{map} soup {i} lightmap {}",
            s.lightmap
        );
        assert!(
            within(s.first_vertex, s.vertex_count.into(), w.verts.len()),
            "{map} soup {i}"
        );
        assert!(
            within(s.first_index, s.index_count.into(), w.indices.len()),
            "{map} soup {i}"
        );
        assert_eq!(s.index_count % 3, 0, "{map} soup {i}");
        let idx = &w.indices[s.first_index as usize..][..s.index_count as usize];
        assert!(idx.iter().all(|&x| x < s.vertex_count), "{map} soup {i}");
    }

    // Cells, portals, cull groups, render trees.
    for (i, c) in w.cells.iter().enumerate() {
        assert!(
            (c.first_aabb as usize) < w.aabb_nodes.len(),
            "{map} cell {i}"
        );
        assert!(
            within(c.first_portal, c.portal_count, w.portals.len()),
            "{map} cell {i}"
        );
        assert!(
            within(c.first_cull_index, c.cull_count, w.cull_indices.len()),
            "{map} cell {i}"
        );
    }
    for (i, p) in w.portals.iter().enumerate() {
        assert!((p.plane as usize) < w.planes.len(), "{map} portal {i}");
        assert!((p.cell as usize) < w.cells.len(), "{map} portal {i}");
        assert!(
            within(p.first_vert, p.vert_count, w.portal_verts.len()),
            "{map} portal {i}"
        );
    }
    for (i, n) in w.aabb_nodes.iter().enumerate() {
        assert!(
            within(n.first_soup, n.soup_count, w.soups.len()),
            "{map} aabb {i}"
        );
        assert!(
            i + 1 + n.child_count as usize <= w.aabb_nodes.len(),
            "{map} aabb {i}"
        );
    }
    for g in &w.cull_groups {
        assert!(within(g.first_soup, g.soup_count, w.soups.len()), "{map}");
    }
    assert!(
        w.cull_indices
            .iter()
            .all(|&c| (c as usize) < w.cull_groups.len()),
        "{map}"
    );

    // The tree: from node 0 every node is reached exactly once, every child
    // is in range, and every leaf the tree reaches has a PVS row.
    let mut seen = vec![false; w.nodes.len()];
    let mut leaf_seen = vec![false; w.leafs.len()];
    let mut stack = vec![0i32];
    while let Some(i) = stack.pop() {
        if i < 0 {
            let l = (-(i + 1)) as usize;
            assert!(l < w.leafs.len(), "{map} leaf {l}");
            leaf_seen[l] = true;
            continue;
        }
        let n = i as usize;
        assert!(n < w.nodes.len(), "{map} node {n}");
        assert!(!seen[n], "{map} node {n} reached twice");
        seen[n] = true;
        assert!((w.nodes[n].plane as usize) < w.planes.len(), "{map}");
        stack.extend(w.nodes[n].children);
    }
    assert!(seen.iter().all(|&s| s), "{map}: a node off the tree");
    let pvs = w.pvs.as_ref().expect("every stock map ships a pvs");
    assert!(pvs.row_bytes * 8 >= pvs.cluster_count, "{map}");
    for (l, leaf) in w.leafs.iter().enumerate() {
        assert!(leaf.cluster < pvs.cluster_count as i32, "{map} leaf {l}");
        assert!(leaf.cell < w.cells.len() as i32, "{map} leaf {l}");
        if leaf_seen[l] && leaf.cluster >= 0 {
            assert!(pvs.visible(leaf.cluster, leaf.cluster), "{map} leaf {l}");
        }
    }

    // Collision.
    for (i, b) in w.brushes.iter().enumerate() {
        assert!(b.num_sides >= 6, "{map} brush {i}");
        assert!((b.material as usize) < w.materials.len(), "{map} brush {i}");
        let sides = &w.brush_sides[b.first_side as usize..][..b.num_sides as usize];
        assert!(
            sides[6..]
                .iter()
                .all(|s| (s.plane_or_dist as usize) < w.planes.len()),
            "{map} brush {i}"
        );
        assert!(
            sides
                .iter()
                .all(|s| (s.material as usize) < w.materials.len()),
            "{map} brush {i}"
        );
    }
    for m in &w.models {
        assert!(
            within(m.first_brush, m.num_brushes, w.brushes.len()),
            "{map}"
        );
        assert!(within(m.first_soup, m.num_soups, w.soups.len()), "{map}");
        assert!(
            within(m.partitions.0, m.partitions.1, w.leaf_partitions.len()),
            "{map}"
        );
    }
    assert_eq!(w.leaf_lists.len(), w.leafs.len(), "{map}");
    for l in &w.leaf_lists {
        assert!(
            within(l.brushes.0, l.brushes.1, w.leaf_brushes.len()),
            "{map}"
        );
        assert!(
            within(l.partitions.0, l.partitions.1, w.leaf_partitions.len()),
            "{map}"
        );
    }
    assert!(
        w.leaf_brushes
            .iter()
            .all(|&b| (b as usize) < w.brushes.len()),
        "{map}"
    );
    assert!(w.patches.is_empty(), "{map}");
    for p in &w.leaf_partitions {
        let Partition::Terrain(t) = *p else {
            panic!("{map}: a patch partition");
        };
        assert!((t as usize) < w.terrain.len(), "{map}");
    }
    for (i, t) in w.terrain.iter().enumerate() {
        assert!((t.material as usize) < w.materials.len(), "{map} part {i}");
        assert!(
            t.index_count > 0 && t.index_count % 3 == 0,
            "{map} part {i}"
        );
        assert!(
            within(t.first_vert, t.vert_count.into(), w.collision_verts.len()),
            "{map} part {i}"
        );
        let idx = &w.collision_indices[t.first_index as usize..][..t.index_count as usize];
        assert!(idx.iter().all(|&x| x < t.vert_count), "{map} part {i}");
    }
    assert!(w.entities.contains("worldspawn"), "{map}");
}

/// The spawn origins of the entity text.
fn spawns(entities: &str) -> Vec<Vec3> {
    vcod_common::bsp::entity_blocks(entities)
        .into_iter()
        .filter(|b| {
            b.get("classname")
                .is_some_and(|c| c.starts_with("mp_") && c.ends_with("_spawn"))
        })
        .filter_map(|b| vcod_common::bsp::parse_vec3(b.get("origin")?))
        .map(Vec3::from_array)
        .collect()
}

#[test]
fn stock_maps_convert_to_a_world() {
    let Some(fs) = testing::cod2_fs() else {
        return;
    };
    for map in MAPS {
        let data = fs
            .read(&format!("maps/mp/{map}.d3dbsp"))
            .unwrap_or_else(|| panic!("{map} not in the iwds"));
        let b = vcod_cod2::d3dbsp::D3dBsp::parse(&data).unwrap_or_else(|e| panic!("{map}: {e:#}"));
        let w = vcod_cod2::world::from_bsp(&b);
        check(map, &w);

        // From a spawn: the vis finds the camera cell and draws something,
        // and a fall from just above the spawn lands within reach.
        let spots = spawns(&w.entities);
        assert!(!spots.is_empty(), "{map}: no spawns");
        let vis = WorldVis::build(&w);
        let col = vcod_cod2::map::collision_world(&b, &w, &[]).unwrap();
        let (mut in_cell, mut landed) = (0, 0);
        for &p in &spots {
            let eye = p + Vec3::new(0.0, 0.0, 60.0);
            if vis.cell_for_point(eye).is_some() {
                in_cell += 1;
                let view = glam::camera::rh::view::look_to_mat4(eye, Vec3::X, Vec3::Z);
                let proj = glam::camera::rh::proj::directx::perspective(1.4, 1.333, 4.0, 20000.0);
                let m: Mat4 = proj * view;
                let v = vis.visible(eye, &Frustum::from_view_proj(m), Some(m));
                assert!(v.soups.iter().any(|&s| s), "{map}: nothing drawn at {p}");
            }
            let t = col.point_trace(
                p + Vec3::new(0.0, 0.0, 16.0),
                p - Vec3::new(0.0, 0.0, 256.0),
                1,
                false,
            );
            if t.fraction < 1.0 && !t.startsolid {
                landed += 1;
            }
        }
        assert_eq!(in_cell, spots.len(), "{map}: spawns outside every cell");
        assert_eq!(landed, spots.len(), "{map}: spawns with no floor");
    }
}

/// The frustum culls soups: from the spawns, looking round the compass,
/// a view marks under half of each map's soups. A soup bounded by its
/// whole vertex run (a material batch) is map-sized, and every view drew
/// 70-85% of them (cod2-d3dbsp-format.md, "Surface bounds").
#[test]
fn the_frustum_culls_soups_from_the_spawns() {
    let Some(fs) = testing::cod2_fs() else {
        return;
    };
    for map in MAPS {
        let data = fs.read(&format!("maps/mp/{map}.d3dbsp")).unwrap();
        let b = vcod_cod2::d3dbsp::D3dBsp::parse(&data).unwrap();
        let w = vcod_cod2::world::from_bsp(&b);
        let vis = WorldVis::build(&w);
        let (mut soups, mut views) = (0usize, 0usize);
        for &p in spawns(&w.entities).iter().take(16) {
            let eye = p + Vec3::new(0.0, 0.0, 60.0);
            for k in 0..8 {
                let a = k as f32 * std::f32::consts::TAU / 8.0;
                let view = glam::camera::rh::view::look_to_mat4(
                    eye,
                    Vec3::new(a.cos(), a.sin(), 0.0),
                    Vec3::Z,
                );
                let proj = glam::camera::rh::proj::directx::perspective(1.4, 1.777, 4.0, 20000.0);
                let m: Mat4 = proj * view;
                soups += vis
                    .visible(eye, &Frustum::from_view_proj(m), Some(m))
                    .stats
                    .soups;
                views += 1;
            }
        }
        let share = soups as f64 / views as f64 / w.soups.len() as f64;
        assert!(
            share < 0.5,
            "{map}: a view marks {:.0}% of the soups",
            share * 100.0
        );
    }
}

/// Not a check: the CoD1 recursive cell walk against retail's queued one
/// (`WorldVis::walk_queued`, CoD2's `R_VisitPortals`, cod2-client-perf.md
/// 9) from every spawn looking round the compass: soups each marks, the
/// soups only one of them marks, cells visited and the walk's time.
/// Maps from `VCOD_PROFILE_MAPS`, every stock map without.
#[test]
#[ignore]
fn cell_walks_side_by_side() {
    let Some(fs) = testing::cod2_fs() else {
        return;
    };
    let maps = std::env::var("VCOD_PROFILE_MAPS").ok();
    let maps: Vec<&str> = match &maps {
        Some(m) => m.split_whitespace().collect(),
        None => MAPS.to_vec(),
    };
    for map in maps {
        let data = fs.read(&format!("maps/mp/{map}.d3dbsp")).unwrap();
        let b = vcod_cod2::d3dbsp::D3dBsp::parse(&data).unwrap();
        let mut w = vcod_cod2::world::from_bsp(&b);
        let queued = WorldVis::build(&w);
        w.portals_only = false;
        let recursive = WorldVis::build(&w);
        let (mut marks, mut only, mut visits, mut ms) =
            ([0usize; 2], [0usize; 2], [0usize; 2], [0f64; 2]);
        let mut views = 0;
        for &p in &spawns(&w.entities) {
            let eye = p + Vec3::new(0.0, 0.0, 60.0);
            for k in 0..8 {
                let a = k as f32 * std::f32::consts::TAU / 8.0;
                let view = glam::camera::rh::view::look_to_mat4(
                    eye,
                    Vec3::new(a.cos(), a.sin(), 0.0),
                    Vec3::Z,
                );
                let proj = glam::camera::rh::proj::directx::perspective(1.4, 1.777, 4.0, 20000.0);
                let m: Mat4 = proj * view;
                let f = Frustum::from_view_proj(m);
                let mut sets = Vec::new();
                for (i, vis) in [&recursive, &queued].into_iter().enumerate() {
                    let t = std::time::Instant::now();
                    let v = vis.visible(eye, &f, Some(m));
                    ms[i] += t.elapsed().as_secs_f64() * 1000.0;
                    marks[i] += v.stats.soups;
                    visits[i] += v.stats.cells_visited;
                    sets.push(v.soups);
                }
                for (&r, &q) in sets[0].iter().zip(&sets[1]) {
                    only[0] += usize::from(r && !q);
                    only[1] += usize::from(q && !r);
                }
                views += 1;
            }
        }
        // the intermission spot, where cod4_perf.sh's spectator stands
        if let Some(spot) = vcod_common::bsp::entity_blocks(&w.entities)
            .into_iter()
            .find(|b| b.get("classname").map(String::as_str) == Some("mp_global_intermission"))
        {
            let eye = Vec3::from(vcod_common::bsp::parse_vec3(&spot["origin"]).unwrap());
            let ang = spot
                .get("angles")
                .and_then(|a| vcod_common::bsp::parse_vec3(a))
                .unwrap_or([0.0; 3]);
            let (yaw, pitch) = (ang[1].to_radians(), ang[0].to_radians());
            let fwd = Vec3::new(
                yaw.cos() * pitch.cos(),
                yaw.sin() * pitch.cos(),
                -pitch.sin(),
            );
            let view = glam::camera::rh::view::look_to_mat4(eye, fwd, Vec3::Z);
            let proj = glam::camera::rh::proj::directx::perspective(1.4, 1.72, 4.0, 20000.0);
            let m: Mat4 = proj * view;
            let f = Frustum::from_view_proj(m);
            for (name, vis) in [("recursive", &recursive), ("queued", &queued)] {
                let t = std::time::Instant::now();
                for _ in 0..49 {
                    vis.visible(eye, &f, Some(m));
                }
                let v = vis.visible(eye, &f, Some(m));
                eprintln!(
                    "INTERMISSION {map} {name}: {} soups, {} visits, {:.3} ms",
                    v.stats.soups,
                    v.stats.cells_visited,
                    t.elapsed().as_secs_f64() * 1000.0 / 50.0
                );
            }
        }
        let per = |x: usize| x as f64 / views as f64;
        eprintln!(
            "WALK {map}: {views} views, {} cells; recursive {:.0} soups, {:.1} visits, {:.3} ms; queued {:.0} soups, {:.1} visits, {:.3} ms; only recursive {:.1}, only queued {:.1}",
            recursive.cell_count(),
            per(marks[0]),
            per(visits[0]),
            ms[0] / views as f64,
            per(marks[1]),
            per(visits[1]),
            ms[1] / views as f64,
            per(only[0]),
            per(only[1]),
        );
    }
}
