//! A 1.7 install's MP effects through `vcod_cod4::fx`
//! (docs/research/cod4-effects.md): every element of `common_mp`'s and
//! `mp_crash`'s effects decodes, its samples sit where the counts say,
//! and each velocity graph's summed displacement is the trapezoid sum of
//! its velocities, the integral the runtime reads back. Vacuous without
//! `COD4_DIR`.

use std::collections::BTreeMap;
use vcod_cod4::assets::Assets;
use vcod_cod4::fx::{ElemDef, ElemType, Role, VelState, roles};
use vcod_cod4::testing::cod4_install;
use vcod_cod4::walk::{WalkOptions, walk};
use vcod_cod4::zone::Zone;

fn zone(name: &str) -> Option<Assets> {
    let install = cod4_install()?;
    let z = Zone::open_path(&install.zone("english", name)).unwrap();
    Some(walk(z, WalkOptions { keep_bulk: true }).unwrap().assets)
}

/// The largest gap between a sample's summed displacement and the last
/// one's plus the segment's trapezoid.
fn trapezoid_gap(states: &[VelState]) -> f32 {
    let mut gap = 0f32;
    for w in states.windows(2) {
        for k in 0..3 {
            for amp in [false, true] {
                let pick = |r: vcod_cod4::fx::Range| if amp { r.amplitude } else { r.base };
                let want = pick(w[0].total_delta[k])
                    + 0.5 * (pick(w[0].velocity[k]) + pick(w[1].velocity[k]));
                gap = gap.max((pick(w[1].total_delta[k]) - want).abs());
            }
        }
    }
    gap
}

#[test]
fn mp_effects_decode_and_integrate() {
    let Some(common) = zone("common_mp") else {
        return;
    };
    let map = zone("mp_crash").unwrap();
    let mut kinds: BTreeMap<String, usize> = BTreeMap::new();
    let mut roles_seen: BTreeMap<String, usize> = BTreeMap::new();
    let (mut n, mut moving, mut worst) = (0, 0, 0f32);
    for a in [&common, &map] {
        for fx in a.fx.iter().filter(|f| !f.name.starts_with(',')) {
            for (role, e) in roles(fx) {
                let d = ElemDef::of(e);
                let t = d
                    .elem_type
                    .unwrap_or_else(|| panic!("{}: {}", fx.name, e.elem_type));
                *roles_seen.entry(format!("{role:?}")).or_default() += 1;
                // A null slot carries no samples (sounds, runners, lights).
                for (got, count) in [(d.vel.len(), e.raw[178]), (d.vis.len(), e.raw[179])] {
                    assert!(got == 0 || got == count as usize + 1, "{}: {got}", fx.name);
                }
                *kinds
                    .entry(format!(
                        "{t:?} vel {} vis {}",
                        !d.vel.is_empty(),
                        !d.vis.is_empty()
                    ))
                    .or_default() += 1;
                if !matches!(t, ElemType::Sound | ElemType::Runner) {
                    assert!(
                        d.life_span_msec.0 >= 0,
                        "{}: {:?}",
                        fx.name,
                        d.life_span_msec
                    );
                }
                if role == Role::Looping {
                    assert!(d.spawn.0 > 0, "{}: looping interval {:?}", fx.name, d.spawn);
                }
                let local: Vec<VelState> = d.vel.iter().map(|s| s.local).collect();
                let world: Vec<VelState> = d.vel.iter().map(|s| s.world).collect();
                let gap = trapezoid_gap(&local).max(trapezoid_gap(&world));
                // Relative to the graph's size, in units per ms.
                let size = d
                    .vel
                    .iter()
                    .flat_map(|s| [s.local, s.world])
                    .flat_map(|v| v.total_delta.into_iter().chain(v.velocity))
                    .map(|r| r.base.abs().max(r.amplitude.abs()))
                    .fold(1e-3f32, f32::max);
                worst = worst.max(gap / size);
                moving += usize::from(gap > 0.0 || size > 1e-3);
                n += 1;
            }
        }
    }
    println!(
        "{n} elements, {moving} moving, kinds {kinds:?}, roles {roles_seen:?}, worst gap {worst}"
    );
    assert!(n > 1000, "{n}");
    assert!(worst < 1e-3, "{worst}");
    // The m16's world flash: one-shot sprites that move with the effect.
    let m16 = common.weapons.iter().find(|w| w.name == "m16_mp").unwrap();
    let flash = &common.fx[m16.effects["worldFlashEffect"].index()];
    println!(
        "{}: {:?}",
        flash.name,
        roles(flash)
            .map(|(r, e)| (r, e.elem_type))
            .collect::<Vec<_>>()
    );
    assert!(flash.name.contains("muzzleflash"), "{}", flash.name);
    // `impactType` at +312: the m16's a bullet's, the frag's a grenade
    // explode's (cod4-sound.md 4).
    let impact = |name: &str| {
        let w = common.weapons.iter().find(|w| w.name == name).unwrap();
        u32::from_le_bytes(w.raw[312..316].try_into().unwrap())
    };
    let types: Vec<(String, u32)> = common
        .weapons
        .iter()
        .filter(|w| !w.name.starts_with(','))
        .map(|w| (w.name.clone(), impact(&w.name)))
        .collect();
    println!("impact types {types:?}");
    assert_eq!(impact("frag_grenade_mp"), 6);
    assert!((1..=4).contains(&impact("m16_mp")));
    assert!(types.iter().all(|(_, t)| *t <= 8), "{types:?}");
}

/// A looping element's `(intervalMsec, count)`: `count` spawns in all,
/// one an interval, 0x7fffffff for ever. Every effect's
/// `msecLoopingLife` is its looping elements' longest
/// `interval * (count - 1)`, or 0x7fffffff when one loops for ever
/// (cod4-effects.md 5).
#[test]
fn looping_counts_are_spawns_in_all() {
    let Some(common) = zone("common_mp") else {
        return;
    };
    let map = zone("mp_crash").unwrap();
    let (mut finite, mut forever) = (0, 0);
    for a in [&common, &map] {
        for fx in a.fx.iter().filter(|f| !f.name.starts_with(',')) {
            let mut life = 0i64;
            for (role, e) in roles(fx) {
                if role != Role::Looping {
                    continue;
                }
                let (interval, count) = ElemDef::of(e).spawn;
                if count == i32::MAX {
                    life = i64::from(i32::MAX);
                    forever += 1;
                } else if life != i64::from(i32::MAX) {
                    life = life.max(i64::from(interval) * i64::from(count - 1));
                    finite += 1;
                }
            }
            assert_eq!(i64::from(fx.msec_looping_life), life, "{}", fx.name);
        }
    }
    println!("{finite} finite looping elements, {forever} for ever");
    assert!(finite > 100 && forever > 50, "{finite} {forever}");
}

/// mp_crash's createfx ambience (cod4-effects.md 11): 36 one-shot effects
/// settled 15 s before the load, each `fxid` one of the eight
/// `level._effect` names its `_fx.gsc` loads, each a zone effect that
/// loops for ever; no sounds.
#[test]
fn mp_crash_places_its_createfx_ambience() {
    use vcod_cod4::fx::createfx;
    let Some(map) = zone("mp_crash") else {
        return;
    };
    let common = zone("common_mp").unwrap();
    let raw = |name: &str| {
        let r = map.raw_files.iter().find(|r| r.name == name).unwrap();
        createfx::text(&r.data)
    };
    let names = createfx::effect_map(&raw("maps/mp/mp_crash_fx.gsc"));
    assert_eq!(names.len(), 8, "{names:?}");
    let placed = createfx::placed(&raw("maps/createfx/mp_crash_fx.gsc"));
    assert_eq!(placed.error, None);
    assert_eq!(placed.effects.len(), 36);
    assert!(placed.sounds.is_empty());
    let mut by: BTreeMap<&str, usize> = BTreeMap::new();
    for e in &placed.effects {
        assert_eq!(e.delay, -15.0);
        *by.entry(&e.fxid).or_default() += 1;
        let file = &names.iter().find(|(n, _)| *n == e.fxid).unwrap().1;
        let fx = [&map, &common]
            .iter()
            .find_map(|a| a.fx.iter().find(|f| f.name.eq_ignore_ascii_case(file)))
            .unwrap_or_else(|| panic!("{file}"));
        assert_eq!(fx.msec_looping_life, i32::MAX, "{file}");
    }
    println!("{by:?}");
    assert_eq!(by["hallway_smoke_light"], 13);
    assert_eq!(by["dust_wind_spiral"], 8);
    let first = &placed.effects[0];
    assert_eq!(first.fxid, "battlefield_smokebank_S");
    assert_eq!(first.origin, [591.801, 506.885, 143.693]);
    assert_eq!(first.angles, [270.0, 0.0, 0.0]);
}
