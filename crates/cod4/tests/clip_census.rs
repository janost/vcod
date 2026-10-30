//! `cod4_clip_census`: every stock MP map's clip map built
//! ([`ClipMap::from_assets`]) and checked against itself
//! (`docs/research/cod4-collision.md`, "The census"):
//!
//! - every `mp_*_spawn` stands where `placeSpawnPoint` puts it (a capsule
//!   sweep 128 up, then down to the ground): the sweep down lands and the
//!   capsule there starts clear;
//! - a point inside each brush (its bounds' centre, when no side plane
//!   cuts it off) reads the brush's contents through `CM_PointContents`
//!   on the model that lists it;
//! - point and capsule sweeps between spawn pairs end with a fraction in
//!   0..=1, and the static models' point clip finishes;
//! - every stored plane type and sign byte equals `CMod_LoadPlanes`' rule.
//!
//! Prints a line per map; `--nocapture` shows them. `COD4_MAPS=a,b` keeps
//! only maps whose name holds one of those words.

use glam::Vec3;
use vcod_cod4::collision::{ClipMap, MASK_PLAYERSOLID, MASK_SHOT};
use vcod_cod4::map::Map;
use vcod_cod4::testing::cod4_install;
use vcod_common::clipmap::ClipTrace;

const PLAYER_MINS: Vec3 = Vec3::new(-15.0, -15.0, 0.0);
const PLAYER_MAXS: Vec3 = Vec3::new(15.0, 15.0, 70.0);

/// Spawns retail's data leaves inside a brush wherever `placeSpawnPoint`
/// puts them: mp_strike's two axis spawns at (1312, 152x) overlap the
/// playerclip brushes 12874..12877 (x up to 1297.5) by half a unit, which
/// the capsule meets as its box on an axial side
/// (`docs/research/cod4-collision.md`, "The census").
const STUCK_SPAWNS: [(&str, &str, [f32; 3]); 2] = [
    (
        "maps/mp/mp_strike.d3dbsp",
        "mp_sab_spawn_axis",
        [1312.0, 1524.0, 44.0],
    ),
    (
        "maps/mp/mp_strike.d3dbsp",
        "mp_ctf_spawn_axis",
        [1312.0, 1520.0, 52.0],
    ),
];

#[derive(Default, Debug)]
struct Counts {
    spawns: usize,
    /// Origins whose capsule starts solid before the placement.
    origin_solid: usize,
    /// The deepest placement drop.
    drop: f32,
    spawn_solid: Vec<String>,
    spawn_falls: Vec<String>,
    brushes: usize,
    brushes_tested: usize,
    brushes_wrong: Vec<String>,
    brushes_unlisted: usize,
    pair_traces: usize,
    pair_hits: usize,
    static_hits: usize,
    plane_rule_misses: usize,
    unreached_leafs: usize,
}

fn origin(e: &[(String, String)]) -> Option<Vec3> {
    let v = &e.iter().find(|(k, _)| k == "origin")?.1;
    let p: Vec<f32> = v
        .split_whitespace()
        .filter_map(|s| s.parse().ok())
        .collect();
    (p.len() == 3).then(|| Vec3::new(p[0], p[1], p[2]))
}

fn sane(t: &ClipTrace) -> bool {
    (0.0..=1.0).contains(&t.fraction) && t.normal.is_finite()
}

fn census(map: &Map, clip: &ClipMap) -> Counts {
    let mut c = Counts::default();
    let spawns: Vec<(String, Vec3)> = map
        .entities
        .iter()
        .filter_map(|e| {
            let class = &e.iter().find(|(k, _)| k == "classname")?.1;
            (class.starts_with("mp_") && class.contains("_spawn"))
                .then(|| Some((class.clone(), origin(e)?)))?
        })
        .collect();
    c.spawns = spawns.len();
    for (class, o) in &spawns {
        let at = *o + Vec3::new(0.0, 0.0, 0.125);
        if clip
            .trace(at, at, PLAYER_MINS, PLAYER_MAXS, MASK_PLAYERSOLID)
            .startsolid
        {
            c.origin_solid += 1;
        }
        // `placeSpawnPoint`: a sweep 128 up, then down to the
        // ground, under the player's mask (0x2810011).
        let up = clip.trace(
            *o,
            *o + Vec3::new(0.0, 0.0, 128.0),
            PLAYER_MINS,
            PLAYER_MAXS,
            MASK_PLAYERSOLID,
        );
        let top = o.lerp(*o + Vec3::new(0.0, 0.0, 128.0), up.fraction);
        let bottom = top - Vec3::new(0.0, 0.0, 262_144.0);
        let down = clip.trace(top, bottom, PLAYER_MINS, PLAYER_MAXS, MASK_PLAYERSOLID);
        let placed = top.lerp(bottom, down.fraction);
        if down.fraction >= 1.0 {
            c.spawn_falls.push(format!("{class} {o}"));
            continue;
        }
        c.drop = c.drop.max(o.z - placed.z);
        if clip
            .trace(placed, placed, PLAYER_MINS, PLAYER_MAXS, MASK_PLAYERSOLID)
            .startsolid
            && !STUCK_SPAWNS.contains(&(clip.name.as_str(), class.as_str(), [o.x, o.y, o.z]))
        {
            c.spawn_solid.push(format!("{class} {o} placed {placed}"));
        }
    }

    let w = &clip.world;
    let mut owner = vec![None; w.brushes.len()];
    for model in 0..clip.submodels.len().max(1) {
        for b in clip.model_brushes(model) {
            owner[b as usize].get_or_insert(model);
        }
    }
    // The last brush is the box brush placeholder.
    c.brushes = w.brushes.len() - 1;
    for (i, b) in w.brushes[..c.brushes].iter().enumerate() {
        let Some(model) = owner[i] else {
            c.brushes_unlisted += 1;
            continue;
        };
        // Off the centre a little, three ways: a point on a node's plane
        // reads the front leaf, which need not list a brush that only
        // touches it (mp_backlot brush 12195 at x 24). A brush passes when
        // one inside point reads it.
        let points: Vec<Vec3> = [
            Vec3::new(0.013, 0.017, 0.011),
            Vec3::new(-0.021, 0.007, -0.019),
            Vec3::new(0.003, -0.023, 0.029),
        ]
        .iter()
        .map(|&j| (b.mins + b.maxs) * 0.5 + (b.maxs - b.mins) * j)
        .filter(|p| {
            b.sides.iter().all(|&(pl, _)| {
                let pl = w.planes[pl as usize];
                p.dot(pl.normal) < pl.dist - 0.01
            })
        })
        .collect();
        if points.is_empty() || b.contents == 0 {
            continue;
        }
        c.brushes_tested += 1;
        let reads: Vec<u32> = points
            .iter()
            .map(|&p| clip.point_contents_model(model, p))
            .collect();
        if !reads.iter().any(|&got| got & b.contents == b.contents) {
            c.brushes_wrong.push(format!(
                "brush {i} model {model} {:#x} read {reads:x?} at {points:?}",
                b.contents
            ));
        }
    }

    let up = Vec3::new(0.0, 0.0, 40.0);
    for pair in spawns.windows(2) {
        let (a, b) = (pair[0].1 + up, pair[1].1 + up);
        for (mins, maxs, mask) in [
            (Vec3::ZERO, Vec3::ZERO, MASK_SHOT),
            (PLAYER_MINS, PLAYER_MAXS, MASK_PLAYERSOLID),
        ] {
            let t = clip.trace(a, b, mins, maxs, mask);
            assert!(sane(&t), "{}: {a} -> {b}: {t:?}", clip.name);
            c.pair_traces += 1;
            c.pair_hits += usize::from(t.fraction < 1.0);
        }
        let mut t = ClipTrace::clear();
        clip.trace_static_models(a, b, MASK_SHOT, &mut t);
        assert!(sane(&t), "{}: static {a} -> {b}: {t:?}", clip.name);
    }
    // A ray down through each static model's box centre.
    for sm in &clip.static_models {
        let mid = (sm.absmin + sm.absmax) * 0.5;
        let (a, b) = (
            Vec3::new(mid.x, mid.y, sm.absmax.z + 8.0),
            Vec3::new(mid.x, mid.y, sm.absmin.z - 8.0),
        );
        let mut t = ClipTrace::clear();
        if clip.trace_static_models(a, b, MASK_SHOT, &mut t).is_some() {
            assert!(sane(&t), "{}: static down {a}: {t:?}", clip.name);
            c.static_hits += 1;
        }
    }

    c.unreached_leafs = w.leafs.len() - clip.world_leafs().len();
    for p in &w.planes {
        let n = p.normal;
        let kind = (0..3).find(|&k| n[k] == 1.0).unwrap_or(3) as u8;
        let signbits = (0..3).fold(0u8, |s, k| s | (u8::from(n[k] < 0.0) << k));
        c.plane_rule_misses += usize::from(kind != p.kind || signbits != p.signbits);
    }
    c
}

#[test]
fn cod4_clip_census() {
    let Some(install) = cod4_install() else {
        return;
    };
    let filter: Vec<String> = std::env::var("COD4_MAPS")
        .map(|v| v.split(',').map(str::to_owned).collect())
        .unwrap_or_default();
    let mut maps: Vec<String> = install
        .zones("english")
        .unwrap()
        .iter()
        .filter_map(|p| p.file_stem()?.to_str().map(str::to_owned))
        .filter(|n| n.starts_with("mp_") && !n.ends_with("_load"))
        .filter(|n| filter.is_empty() || filter.iter().any(|f| n.contains(f.as_str())))
        .collect();
    maps.sort();
    if filter.is_empty() {
        assert_eq!(maps.len(), 21, "{maps:?}");
    }
    let mut bad = Vec::new();
    for name in &maps {
        let (map, walked) = Map::load(&install, "english", name).unwrap();
        let clip = ClipMap::from_assets(&walked.assets).unwrap();
        let c = census(&map, &clip);
        println!(
            "{name}: spawns {} (origin solid {}, placed solid {}, falling {}, drop {:.1}), brushes {} tested {} wrong {} unlisted {}, \
             pair traces {} hit {}, static models {} hit {}, plane rule misses {}, leafs past the tree {}",
            c.spawns,
            c.origin_solid,
            c.spawn_solid.len(),
            c.spawn_falls.len(),
            c.drop,
            c.brushes,
            c.brushes_tested,
            c.brushes_wrong.len(),
            c.brushes_unlisted,
            c.pair_traces,
            c.pair_hits,
            clip.static_models.len(),
            c.static_hits,
            c.plane_rule_misses,
            c.unreached_leafs,
        );
        for s in c
            .spawn_solid
            .iter()
            .chain(&c.spawn_falls)
            .chain(&c.brushes_wrong)
            .take(8)
        {
            println!("  {s}");
        }
        if !(c.spawn_solid.is_empty() && c.spawn_falls.is_empty() && c.brushes_wrong.is_empty())
            || c.spawns == 0
            || c.brushes_tested == 0
        {
            bad.push(name.clone());
        }
    }
    assert!(bad.is_empty(), "maps failing the census: {bad:?}");
}
