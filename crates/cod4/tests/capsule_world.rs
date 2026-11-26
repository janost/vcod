//! Capsules against mp_crash's clip map (`vcod_cod4::ode::collide`,
//! cod4-player-anims.md 10.6). `COD4_DIR` unset passes vacuously.

use glam::Vec3;
use vcod_cod4::collision::ClipMap;
use vcod_cod4::map::Map;
use vcod_cod4::ode::collide::{Capsule, MAX_CONTACTS, collide_world};
use vcod_cod4::ragdoll::CLIP_MASK;
use vcod_cod4::testing::cod4_install;

fn crash() -> Option<(Map, ClipMap)> {
    let install = cod4_install()?;
    let (map, walked) = Map::load(&install, "english", "mp_crash").unwrap();
    let clip = ClipMap::from_assets(&walked.assets).unwrap();
    Some((map, clip))
}

/// Every face of mp_crash's brushes with edges winds (0x5998e0) on its
/// plane and clockwise seen from outside, but the few whose corners fall
/// within a unit of each other (178 of 54109): the fan's `(w0, w2, w1)`
/// then runs counterclockwise round the outward normal, as the triangle
/// collider's edge planes need.
#[test]
fn crashs_brush_windings_run_clockwise_from_outside() {
    let Some((_, clip)) = crash() else { return };
    let (mut faces, mut empty) = (0, 0);
    for b in 0..clip.world.brushes.len() - 1 {
        let f = clip.brush_faces(b);
        for i in 0..f.planes.len() {
            if f.adjacent[i].is_empty() {
                continue;
            }
            faces += 1;
            let w: Vec<Vec3> = f.winding(i, 0x100).into_iter().map(Vec3::from).collect();
            if w.is_empty() {
                empty += 1;
                continue;
            }
            let (n, d) = f.planes[i];
            let n = Vec3::from(n);
            assert!(
                w.iter().all(|p| (n.dot(*p) - d).abs() < 0.05),
                "brush {b} face {i}"
            );
            let area =
                (1..w.len() - 1).fold(Vec3::ZERO, |a, k| a + (w[k] - w[0]).cross(w[k + 1] - w[0]));
            assert!(area.dot(n) < 0.0, "brush {b} face {i}");
        }
    }
    assert_eq!((faces, empty), (54109, 178));
}

/// A level capsule laid half a unit under the trace's floor under each of
/// mp_crash's `mp_tdm_spawn`s: wherever the floor under its length is one
/// flat plane, both ends of its segment meet it straight up, 0.375 deep
/// over a brush (the trace stops its 0.125 skin over one) and 0.5 over
/// terrain. Terrain a tenth of a unit under a floor brush (by spawn
/// (1504, 512)) adds its own contacts, its triangles' normals.
#[test]
fn a_capsule_on_crashs_floors_takes_their_normal() {
    let Some((map, clip)) = crash() else { return };
    let floor = |x: f32, y: f32, top: f32| {
        let (a, b) = (Vec3::new(x, y, top), Vec3::new(x, y, top - 160.0));
        let t = clip.trace(a, b, Vec3::ZERO, Vec3::ZERO, CLIP_MASK);
        (!t.startsolid && t.fraction < 1.0).then_some((a.z + (b.z - a.z) * t.fraction, t.normal))
    };
    let mut tested = 0;
    for e in &map.entities {
        if !e
            .iter()
            .any(|(k, v)| k == "classname" && v == "mp_tdm_spawn")
        {
            continue;
        }
        let Some((_, o)) = e.iter().find(|(k, _)| k == "origin") else {
            continue;
        };
        let v: Vec<f32> = o
            .split_whitespace()
            .filter_map(|s| s.parse().ok())
            .collect();
        let s = Vec3::new(v[0], v[1], v[2]);
        let Some((z, n)) = floor(s.x, s.y, s.z + 48.0) else {
            continue;
        };
        // Probed every 2 units under the capsule and a unit past it.
        let flat = (-8..=8).all(|i| {
            (-3..=3).all(|j| {
                floor(s.x + 2.0 * i as f32, s.y + 2.0 * j as f32, s.z + 48.0)
                    .is_some_and(|(fz, fnm)| fnm.z > 0.999 && (fz - z).abs() < 0.01)
            })
        });
        if !(flat && n.z > 0.999) {
            continue;
        }
        let cap = Capsule {
            centre: [s.x, s.y, z + 4.5],
            axis: [1.0, 0.0, 0.0],
            radius: 5.0,
            half_len: 10.0,
        };
        let c = collide_world(&clip, &cap, CLIP_MASK, MAX_CONTACTS);
        let up: Vec<_> = c
            .iter()
            .filter(|g| Vec3::from(g.normal).abs_diff_eq(Vec3::Z, 1e-3))
            .collect();
        assert!(up.len() >= 2, "{s}: {c:?}");
        for g in up {
            let brush = (g.depth - 0.375).abs() < 1e-4;
            assert!(brush || (g.depth - 0.5).abs() < 1e-3, "{s}: {g:?}");
        }
        assert!(c.iter().all(|g| g.normal[2] > 0.9), "{s}: {c:?}");
        tested += 1;
    }
    assert!(tested >= 8, "{tested} spawns on flat floor");
}
