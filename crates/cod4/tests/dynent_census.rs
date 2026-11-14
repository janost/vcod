//! The map's dynamic entities (`vcod_cod4::dynent`, cod4-world.md 13):
//! mp_crash's defs as the zone holds them, and a frag's blast among them
//! on the map's clip map. `COD4_DIR` unset passes vacuously.

use glam::Vec3;
use vcod_cod4::assets::Assets;
use vcod_cod4::collision::ClipMap;
use vcod_cod4::dynent::{DESTRUCT, DynEnts, defs};
use vcod_cod4::physics::clip_trace_masked;
use vcod_cod4::ragdoll::{CLIP_MASK, Explosion};
use vcod_cod4::testing::cod4_install;
use vcod_cod4::walk::{WalkOptions, walk};
use vcod_cod4::zone::Zone;

fn zone(name: &str) -> Option<Assets> {
    let install = cod4_install()?;
    let z = Zone::open_path(&install.zone("english", name)).unwrap();
    Some(walk(z, WalkOptions { keep_bulk: true }).unwrap().assets)
}

/// mp_crash holds 516 dynamic entities in list 0: 498 clutter, 18
/// destructible plates at 75 health, every one with a preset and a
/// collision model, at a unit pose quaternion.
#[test]
fn mp_crash_dynents_load_with_their_presets_and_models() {
    let (Some(map), Some(common)) = (zone("mp_crash"), zone("common_mp")) else {
        return;
    };
    let d = defs(&map, &[&common]);
    assert_eq!(d.len(), 516);
    let destruct: Vec<_> = d.iter().filter(|d| d.kind == DESTRUCT).collect();
    assert_eq!(destruct.len(), 18);
    assert!(
        destruct
            .iter()
            .all(|d| d.health == 75 && d.destroy_fx.is_none())
    );
    assert!(d.iter().all(|d| d.body.is_some()), "a def without a preset");
    assert!(
        d.iter().all(|d| d.coll.is_some()),
        "a def without collision"
    );
    assert!(d.iter().all(|d| (d.rot.length() - 1.0).abs() < 1e-3));
    // `,bottle_plastic` resolves to common_mp's: mass 0.1.
    let can = d.iter().find(|d| d.model == "com_soup_can").unwrap();
    assert_eq!(can.body.as_ref().unwrap().1.mass, 0.1);
}

/// A frag's blast (`EV_GRENADE_EXPLODE`, `explosionRadius` 256) beside a
/// cluster of mp_crash's clutter wakes at most 20 of them, flings them,
/// and every one comes to rest again out of the world within 10 s.
#[test]
fn a_blast_flings_mp_crashs_clutter_and_it_settles() {
    let (Some(map), Some(common)) = (zone("mp_crash"), zone("common_mp")) else {
        return;
    };
    let clip = ClipMap::from_assets(&map).unwrap();
    let defs = std::sync::Arc::new(defs(&map, &[&common]));
    // The densest spot: the def with the most others within 128 units.
    let near = |i: usize| {
        defs.iter()
            .filter(|d| d.origin.distance(defs[i].origin) < 128.0)
            .count()
    };
    let at = (0..defs.len()).max_by_key(|&i| near(i)).unwrap();
    let origin = defs[at].origin + Vec3::Z * 8.0;
    let mut d = DynEnts::new(defs.clone());
    d.pass_start_contents(|p| clip.point_contents(p));
    let blast = Explosion {
        origin,
        inner: 0.0,
        outer: 256.0,
        cylinder: false,
        impulse: Vec3::ZERO,
        scale: 1.0,
    };
    let broken = d.explode(&blast, [130, 50], 0);
    let awake = d.awake();
    assert!((1..=20).contains(&awake), "{awake} woken");
    let start: Vec<Vec3> = d.ents.iter().map(|e| e.origin).collect();
    let mut trace = clip_trace_masked(&clip);
    let clock = std::time::Instant::now();
    let mut t = 0;
    while t < 10_000 && d.awake() > 0 {
        t += 16;
        d.advance(t, &mut trace);
    }
    let took = clock.elapsed();
    let moved = d
        .ents
        .iter()
        .zip(&start)
        .filter(|(e, s)| e.origin.distance(**s) > 1.0)
        .count();
    println!(
        "{awake} woken at {origin}, {moved} moved, {} broken, asleep by {t} ms, {took:?}",
        broken.len()
    );
    assert_eq!(d.awake(), 0, "still awake at {t} ms");
    assert!(moved > 0);
    // Each moved one's centre of mass rests clear of what it meets.
    for (i, e) in d.ents.iter().enumerate() {
        if e.origin.distance(start[i]) <= 1.0 {
            continue;
        }
        let com = e.origin + e.rot * defs[i].body.as_ref().unwrap().0.com;
        let c = clip.point_contents(com) & CLIP_MASK & !e.pass;
        assert_eq!(c, 0, "{} {com} rests in {c:#x}", defs[i].model);
        assert!(
            e.origin.z > start[i].z - 200.0,
            "{} fell to {}",
            defs[i].model,
            e.origin
        );
    }
}
