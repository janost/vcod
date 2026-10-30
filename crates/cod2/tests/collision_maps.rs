//! The CoD2 clip map on the 15 stock MP maps: every spawn holds a standing
//! player and drops him onto a floor, point traces agree with a brute-force
//! clip over every brush and triangle, and a few traces through mp_toujane's
//! doors and windows meet what the geometry says. Needs `COD2_DIR`.

use glam::Vec3;
use vcod_cod2::collision::{ClipMap, MASK_PLAYERSOLID, MASK_SHOT};
use vcod_cod2::d3dbsp::D3dBsp;
use vcod_common::testing;

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

const PLAYER_MINS: Vec3 = Vec3::new(-15.0, -15.0, 0.0);
const PLAYER_MAXS: Vec3 = Vec3::new(15.0, 15.0, 70.0);

fn load(fs: &vcod_common::pk3::Pk3Fs, map: &str) -> D3dBsp {
    let data = fs
        .read(&format!("maps/mp/{map}.d3dbsp"))
        .unwrap_or_else(|| panic!("{map} not in the iwds"));
    D3dBsp::parse(&data).unwrap_or_else(|e| panic!("{map}: {e:#}"))
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

/// `GScr_PlaceSpawnPoint` (Mac 0x19f4b8), which every stock gametype runs
/// on its spawns: up 128, then down onto the floor, then a position test
/// whose `allsolid` retail warns about. The placed origin and the down
/// trace.
fn place_spawn(clip: &ClipMap, o: Vec3) -> (Vec3, vcod_cod2::collision::ClipTrace) {
    let lerp = |a: Vec3, b: Vec3, f: f32| {
        Vec3::from(std::array::from_fn::<f32, 3, _>(|k| {
            ((f64::from(b[k]) - f64::from(a[k])) * f64::from(f) + f64::from(a[k])) as f32
        }))
    };
    let top = o + Vec3::new(0.0, 0.0, 128.0);
    let up = clip.trace(o, top, PLAYER_MINS, PLAYER_MAXS, MASK_PLAYERSOLID);
    let from = lerp(o, top, up.fraction);
    let to = Vec3::new(from.x, from.y, from.z - 262_144.0);
    let down = clip.trace(from, to, PLAYER_MINS, PLAYER_MAXS, MASK_PLAYERSOLID);
    (lerp(from, to, down.fraction), down)
}

#[test]
fn every_spawn_holds_a_standing_player() {
    let Some(fs) = testing::cod2_fs() else {
        return;
    };
    for map in MAPS {
        let b = load(&fs, map);
        let clip = vcod_cod2::map::clip_map(&b).unwrap_or_else(|e| panic!("{map}: {e:#}"));
        let spots = spawns(&b.entities);
        assert!(!spots.is_empty(), "{map}");
        let (mut raw_solid, mut worst) = (0, 0.0f32);
        for &o in &spots {
            raw_solid += usize::from(
                clip.trace(o, o, PLAYER_MINS, PLAYER_MAXS, MASK_PLAYERSOLID)
                    .startsolid,
            );
            let (p, down) = place_spawn(&clip, o);
            // The drop may start against a ledge it then leaves: retail
            // warns only when the placed spot is solid.
            assert!(
                down.fraction < 1.0 && down.normal.z > 0.0,
                "{map}: no floor under {o}: {down:?}"
            );
            let t = clip.trace(p, p, PLAYER_MINS, PLAYER_MAXS, MASK_PLAYERSOLID);
            assert!(!t.startsolid, "{map}: {o} placed at {p} is in solid: {t:?}");
            worst = worst.max((o.z - p.z).abs());
        }
        eprintln!(
            "{map}: {} spawns, {raw_solid} start solid where the map put them, \
             placed at most {worst} from it",
            spots.len()
        );
    }
}

/// A point segment against every model-0 brush and every triangle a leaf
/// reaches, in no order: the fraction a walk in any order must find.
struct Brute<'a> {
    b: &'a D3dBsp,
    brushes: Vec<usize>,
    tris: Vec<(usize, u32)>,
}

impl<'a> Brute<'a> {
    fn new(b: &'a D3dBsp) -> Self {
        let m = &b.models[0];
        let brushes = (m.first_brush as usize..(m.first_brush + m.brush_count) as usize).collect();
        let mut tris = Vec::new();
        let mut seen = vec![false; b.coll_aabb_trees.len()];
        let mut stack: Vec<(usize, u32)> = Vec::new();
        for l in &b.leafs {
            for t in l.first_coll_aabb..l.first_coll_aabb + l.coll_aabb_count {
                let contents =
                    b.materials[b.coll_aabb_trees[t as usize].material as usize].content_flags;
                stack.push((t as usize, contents));
            }
        }
        while let Some((t, contents)) = stack.pop() {
            if std::mem::replace(&mut seen[t], true) {
                continue;
            }
            let n = &b.coll_aabb_trees[t];
            let first = n.child_or_partition as usize;
            if n.child_count > 0 {
                stack.extend((first..first + n.child_count as usize).map(|c| (c, contents)));
            } else {
                let p = &b.coll_partitions[first];
                let ft = p.first_tri as usize;
                tris.extend((ft..ft + p.tri_count as usize).map(|i| (i, contents)));
            }
        }
        tris.sort_unstable();
        tris.dedup();
        Brute { b, brushes, tris }
    }

    fn trace(&self, start: Vec3, end: Vec3, mask: u32) -> f32 {
        let mut best = 1.0f32;
        for &i in &self.brushes {
            let bx = self.b.brush_box(i);
            if bx.contents & mask == 0 {
                continue;
            }
            let br = &self.b.brushes[i];
            let sides = &self.b.brush_sides[br.first_side as usize..][..br.num_sides as usize];
            let mut planes: Vec<(Vec3, f32)> = Vec::new();
            for k in 0..3 {
                let mut n = Vec3::ZERO;
                n[k] = -1.0;
                planes.push((n, -bx.mins[k]));
                n[k] = 1.0;
                planes.push((n, bx.maxs[k]));
            }
            for s in &sides[6..] {
                let p = &self.b.planes[s.plane_or_bound as usize];
                planes.push((p.normal, p.dist));
            }
            let (mut enter, mut leave) = (0.0f32, best);
            let mut met = false;
            let mut hit = true;
            for (n, d) in planes {
                let d1 = n.dot(start) - d;
                let d2 = n.dot(end) - d;
                if d1 > 0.0 && (d2 >= 0.125 || d2 >= d1) {
                    hit = false;
                    break;
                }
                if d1 <= 0.0 && d2 <= 0.0 {
                    continue;
                }
                if d1 > d2 {
                    let f = (d1 - 0.125) / (d1 - d2);
                    if f > enter || !met {
                        enter = enter.max(f);
                        met = true;
                    }
                } else {
                    leave = leave.min(d1 / (d1 - d2));
                }
            }
            if hit && met && enter < leave {
                best = best.min(enter);
            }
        }
        for &(i, contents) in &self.tris {
            if contents & mask == 0 {
                continue;
            }
            let t = &self.b.coll_tris[i];
            let n = Vec3::new(t.plane[0], t.plane[1], t.plane[2]);
            let (d1, d2) = (n.dot(start) - t.plane[3], n.dot(end) - t.plane[3]);
            if !(d1 > 0.0 && d2 < 0.0) {
                continue;
            }
            let f = ((d1 - 0.125) / (d1 - d2)).max(0.0);
            let hit = start + (end - start) * (d1 / (d1 - d2));
            let s = Vec3::new(t.svec[0], t.svec[1], t.svec[2]).dot(hit) - t.svec[3];
            let u = Vec3::new(t.tvec[0], t.tvec[1], t.tvec[2]).dot(hit) - t.tvec[3];
            if s >= -0.001 && u >= -0.001 && s + u <= 1.001 {
                best = best.min(f);
            }
        }
        best
    }
}

/// A small xorshift, so the segments repeat run to run.
struct Rng(u64);

impl Rng {
    fn unit(&mut self) -> f32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 >> 40) as f32 / (1u64 << 24) as f32
    }
}

#[test]
fn point_traces_match_a_brute_force_clip() {
    let Some(fs) = testing::cod2_fs() else {
        return;
    };
    for map in MAPS {
        let b = load(&fs, map);
        let clip = vcod_cod2::map::clip_map(&b).unwrap();
        let brute = Brute::new(&b);
        let spots = spawns(&b.entities);
        let mut rng = Rng(0x9e37_79b9_7f4a_7c15);
        let (mut met, mut off) = (0, Vec::new());
        for i in 0..400 {
            // From a spawn's eye out to a random point 2000 units around.
            let s = spots[i % spots.len()] + Vec3::new(0.0, 0.0, 60.0);
            let d = Vec3::new(rng.unit() - 0.5, rng.unit() - 0.5, rng.unit() - 0.6) * 4000.0;
            for mask in [MASK_SHOT, MASK_PLAYERSOLID] {
                let ours = clip.trace(s, s + d, Vec3::ZERO, Vec3::ZERO, mask).fraction;
                let theirs = brute.trace(s, s + d, mask);
                met += usize::from(theirs < 1.0);
                if (ours - theirs).abs() > 1e-4 {
                    off.push((s, d, mask, ours, theirs));
                }
            }
        }
        assert!(met > 400, "{map}: only {met} of 800 segments met anything");
        assert!(
            off.is_empty(),
            "{map}: {} disagree: {:?}",
            off.len(),
            &off[..off.len().min(5)]
        );
    }
}
/// mp_toujane's doorway at y 1692 (brushes 3650, 3648 and the jamb from x
/// 954: a 54-wide gap, x 900..954, under a lintel from z 246), and a window
/// in the plaster wall north of (968, 3348) whose sill and lintel are trim
/// brushes (the lintel 1740 is 0x2080: shots stop, players do not).
#[test]
fn toujane_doors_and_windows_meet_their_brushes() {
    let Some(fs) = testing::cod2_fs() else {
        return;
    };
    let b = load(&fs, "mp_toujane");
    let clip = vcod_cod2::map::clip_map(&b).unwrap();
    let point = |s: Vec3, d: Vec3, mask| clip.trace(s, s + d, Vec3::ZERO, Vec3::ZERO, mask);
    let near = |a: f32, b: f32| (a - b).abs() < 1e-3;

    // The door: the wall's face is y 1692, met 0.125 short.
    let ahead = Vec3::Y * 200.0;
    for x in [890.0, 964.0] {
        let t = point(Vec3::new(x, 1646.0, 196.0), ahead, MASK_PLAYERSOLID);
        assert!(near(t.fraction * 200.0, 45.875), "jamb at x {x}: {t:?}");
        assert_eq!(t.normal, Vec3::NEG_Y);
    }
    let gap = point(Vec3::new(927.0, 1646.0, 196.0), ahead, MASK_PLAYERSOLID);
    assert_eq!(gap.fraction, 1.0, "{gap:?}");
    let lintel = point(Vec3::new(927.0, 1646.0, 250.0), ahead, MASK_PLAYERSOLID);
    assert!(near(lintel.fraction * 200.0, 45.875), "{lintel:?}");
    // A standing player walks through; a capsule of radius 32 stops on the
    // jambs, 32 + 0.125 short of the wall.
    let from = Vec3::new(927.0, 1646.0, 156.375);
    let walk = clip.trace(
        from,
        from + Vec3::Y * 600.0,
        PLAYER_MINS,
        PLAYER_MAXS,
        MASK_PLAYERSOLID,
    );
    assert!(
        walk.fraction * 600.0 > 100.0 && !walk.startsolid,
        "{walk:?}"
    );
    let wide = Vec3::new(32.0, 32.0, 0.0);
    let fat = clip.trace(
        from,
        from + Vec3::Y * 600.0,
        -wide,
        wide + Vec3::Z * 70.0,
        MASK_PLAYERSOLID,
    );
    assert!(
        near(fat.fraction * 600.0, 1692.0 - 32.125 - 1646.0),
        "{fat:?}"
    );
    assert_eq!(fat.normal, Vec3::NEG_Y);

    // The window: the wall at 194.278 along the ray, the trims 2.56 nearer.
    let p = Vec3::new(968.0, 3348.0, 49.125);
    let dir = Vec3::new(-0.195_090_38, -0.980_785_25, 0.0) * 800.0;
    let at = |dz: f32, mask| point(p + Vec3::Z * dz, dir, mask);
    for dz in [10.0, 100.0] {
        let wall = at(dz, MASK_SHOT);
        assert!(
            (wall.fraction * 800.0 - 194.278).abs() < 0.01,
            "{dz}: {wall:?}"
        );
        assert!((wall.normal - Vec3::new(-0.461_748_6, 0.887_010_8, 0.0)).length() < 1e-5);
    }
    let sill = at(25.0, MASK_SHOT);
    assert!((sill.fraction * 800.0 - 191.714).abs() < 0.01, "{sill:?}");
    assert_eq!(at(50.0, MASK_SHOT).fraction, 1.0);
    let trim = at(93.0, MASK_SHOT);
    assert!((trim.fraction * 800.0 - 191.714).abs() < 0.01, "{trim:?}");
    assert_eq!(trim.contents, 0x2080);
    assert_eq!(at(93.0, MASK_PLAYERSOLID).fraction, 1.0);
    // The 64-unit opening takes no standing player.
    let climb = clip.trace(p, p + dir, PLAYER_MINS, PLAYER_MAXS, MASK_PLAYERSOLID);
    assert!(climb.fraction * 800.0 < 194.278, "{climb:?}");
}

/// The engine's collision world for a CoD2 map traces the world through the
/// clip map: the same fraction, flags and contents for a player's drop.
#[test]
fn the_engine_world_traces_through_the_clip_map() {
    let Some(fs) = testing::cod2_fs() else {
        return;
    };
    for map in ["mp_toujane", "mp_brecourt"] {
        let b = load(&fs, map);
        let clip = vcod_cod2::map::clip_map(&b).unwrap();
        let w = vcod_cod2::world::from_bsp(&b);
        let cw = vcod_cod2::map::collision_world(&b, &w, &[]).unwrap();
        for o in spawns(&b.entities) {
            let (from, to) = (o + Vec3::Z, o - Vec3::Z * 256.0);
            let ours = clip.trace(from, to, PLAYER_MINS, PLAYER_MAXS, MASK_PLAYERSOLID);
            let t = cw.move_trace(from, to, PLAYER_MINS, PLAYER_MAXS, MASK_PLAYERSOLID);
            assert_eq!(
                (t.fraction, t.normal, t.surface_flags, t.startsolid),
                (
                    ours.fraction,
                    ours.normal,
                    ours.surface_flags,
                    ours.startsolid
                ),
                "{map} {o}"
            );
            assert_eq!(cw.hit_contents(&t), ours.contents, "{map} {o}");
            let inside = o + Vec3::Z * 30.0;
            assert_eq!(
                cw.point_contents(inside) & !0x20,
                clip.point_contents(inside) & !0x20
            );
        }
    }
}

/// Foliage brushes (contents 2) reach the clip, and `PM_FoliageSounds`'
/// position test (lnxded 0x80e6784: the body's box at 0.75, 0.9 tall,
/// under mask 2) starts solid inside one and clear of the player mask's
/// solids there. Prints each map's count.
#[test]
fn foliage_brushes_answer_the_foliage_sound_test() {
    let Some(fs) = testing::cod2_fs() else {
        return;
    };
    let (mins, maxs) = (PLAYER_MINS * 0.75, Vec3::new(11.25, 11.25, 63.0));
    let mut found = 0;
    for map in MAPS {
        let b = load(&fs, map);
        let clip = vcod_cod2::map::clip_map(&b).unwrap();
        let m = &b.models[0];
        let foliage: Vec<usize> = (m.first_brush as usize
            ..(m.first_brush + m.brush_count) as usize)
            .filter(|&i| b.materials[b.brushes[i].material as usize].content_flags & 2 != 0)
            .collect();
        let mut hit = 0;
        for &i in &foliage {
            let s = &b.brush_sides[b.brushes[i].first_side as usize..][..6];
            let lo = Vec3::new(s[0].bound(), s[2].bound(), s[4].bound());
            let hi = Vec3::new(s[1].bound(), s[3].bound(), s[5].bound());
            let c = (lo + hi) * 0.5 - Vec3::new(0.0, 0.0, 31.5);
            let t = clip.trace(c, c, mins, maxs, 2);
            hit += usize::from(t.startsolid);
        }
        eprintln!(
            "{map}: {} foliage brushes, {hit} met by the test",
            foliage.len()
        );
        assert_eq!(hit, foliage.len(), "{map}");
        found += foliage.len();
    }
    assert!(found > 0, "no stock map has a foliage brush");
}
