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

/// A round fired down onto one of mp_crash's soup cans meets its model,
/// and throws it; two of a 40-damage weapon's rounds break a 75-health
/// plate, which then neither draws nor meets a third.
#[test]
fn rounds_throw_mp_crashs_clutter_and_break_a_plate() {
    let (Some(map), Some(common)) = (zone("mp_crash"), zone("common_mp")) else {
        return;
    };
    let defs = std::sync::Arc::new(defs(&map, &[&common]));
    let mut d = DynEnts::new(defs.clone());
    let can = defs.iter().position(|d| d.model == "com_soup_can").unwrap();
    let top = defs[can].origin + Vec3::Z * 64.0;
    let (struck, broken) = d.bullet(top, defs[can].origin - Vec3::Z * 8.0, 40, 0);
    let s = struck.expect("the round missed the can");
    assert_eq!(s.index, can);
    assert!(broken.is_empty());
    assert!(s.at.z > defs[can].origin.z && s.at.z < top.z, "{}", s.at);
    let b = d.ents[can].body.as_ref().unwrap();
    assert!(b.vel.z < -10.0, "{}", b.vel);
    assert_eq!(d.awake(), 1);
    let plate = defs.iter().position(|d| d.kind == DESTRUCT).unwrap();
    let p = defs[plate].origin;
    // Along its face's normal, whichever axis it is.
    let mut hits = 0;
    for _ in 0..3 {
        for dir in [Vec3::X, Vec3::Y, Vec3::Z] {
            let (s, broken) = d.bullet(p + dir * 32.0, p - dir * 32.0, 40, 0);
            if s.is_some_and(|s| s.index == plate) {
                hits += 1;
                // A plate has no `destroyFx`: nothing to play.
                assert!(broken.is_empty());
                assert_eq!(d.ents[plate].drawn, hits < 2, "hit {hits}");
                break;
            }
        }
    }
    assert_eq!(hits, 2, "the plate took {hits} rounds");
    assert!(!d.ents[plate].drawn && !d.ents[plate].live);
}

/// mp_broadcast's televisions (`com_tv1_screen`, 1 health) break on a
/// round into their `destroyFx`, at their pose.
#[test]
fn a_round_breaks_a_television_into_its_effect() {
    let (Some(map), Some(common)) = (zone("mp_broadcast"), zone("common_mp")) else {
        return;
    };
    let defs = std::sync::Arc::new(defs(&map, &[&common]));
    let mut d = DynEnts::new(defs.clone());
    let tv = defs.iter().position(|d| d.model == "com_tv1_screen").unwrap();
    assert_eq!(defs[tv].health, 1);
    let p = defs[tv].origin + defs[tv].rot * Vec3::Z * 8.0;
    let mut broken = Vec::new();
    for dir in [Vec3::X, Vec3::Y, -Vec3::X, -Vec3::Y] {
        broken = d.bullet(p + dir * 48.0, p - dir * 48.0, 40, 0).1;
        if !broken.is_empty() {
            break;
        }
    }
    let b = broken.first().expect("no round broke the set");
    assert_eq!(b.index, tv);
    assert_eq!(b.fx.as_deref(), Some("explosions/tv_explosion_mp"));
    assert_eq!(b.origin, defs[tv].origin);
    assert!(!d.ents[tv].drawn);
}
