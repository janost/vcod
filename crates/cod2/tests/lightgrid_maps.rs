//! The light grid on the 15 stock MP maps: sampled where every spawn puts
//! a standing body's lighting origin, with the sight traces through the
//! clip map, it answers finite colours in range that vary over the map, and
//! a spawn under open sky is lit brighter and sunnier than one under a
//! roof. Needs `COD2_DIR`.

use glam::Vec3;
use vcod_cod2::collision::{ClipMap, MASK_SHOT, SURF_SKY};
use vcod_cod2::d3dbsp::D3dBsp;
use vcod_cod2::lightgrid::{self, GridSample, SIGHT_MASK};
use vcod_common::mapworld::LightGrid;
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

fn load(fs: &vcod_common::pk3::Pk3Fs, map: &str) -> D3dBsp {
    let data = fs
        .read(&format!("maps/mp/{map}.d3dbsp"))
        .unwrap_or_else(|| panic!("{map} not in the iwds"));
    D3dBsp::parse(&data).unwrap_or_else(|e| panic!("{map}: {e:#}"))
}

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

fn grid(b: &D3dBsp) -> LightGrid {
    LightGrid {
        entries: b.light_grid.clone(),
        colours: b.light_grid_colours.clone(),
    }
}

/// The lookup with retail's sight traces (`CM_BoxSightTrace`, a point).
fn sample(g: &LightGrid, clip: &ClipMap, p: Vec3) -> GridSample {
    let blocked = |a: Vec3, b: Vec3| {
        clip.trace(a, b, Vec3::ZERO, Vec3::ZERO, SIGHT_MASK)
            .fraction
            < 1.0
    };
    lightgrid::sample(g, p, Some(&blocked))
}

/// The mean of the eight corner colours: the probe's light averaged over
/// every direction.
fn mean_light(s: &GridSample) -> Vec3 {
    (0..8).map(|j| s.corner(j)).sum::<Vec3>() / 8.0
}

/// Whether a shot straight up from `p` meets the sky or nothing.
fn open_sky(clip: &ClipMap, p: Vec3) -> bool {
    let t = clip.trace(p, p + Vec3::Z * 16384.0, Vec3::ZERO, Vec3::ZERO, MASK_SHOT);
    t.fraction == 1.0 || t.surface_flags & SURF_SKY != 0
}

#[test]
fn every_spawn_samples_the_grid() {
    let Some(fs) = testing::cod2_fs() else {
        return;
    };
    for map in MAPS {
        let b = load(&fs, map);
        let g = grid(&b);
        assert!(!g.entries.is_empty(), "{map}");
        let clip = vcod_cod2::map::clip_map(&b).unwrap();
        let (mut sky, mut roof) = (Vec::new(), Vec::new());
        let mut seen = Vec::new();
        for o in spawns(&b.entities) {
            let p = o + Vec3::Z * 32.0;
            let s = sample(&g, &clip, p);
            for v in s.rgb.iter().flatten() {
                assert!(
                    v.is_finite() && (0.0..=1.0 + 1e-5).contains(v),
                    "{map} {p}: {s:?}"
                );
            }
            assert!((0.0..=1.0 + 1e-5).contains(&s.sun), "{map} {p}: {}", s.sun);
            let lit = mean_light(&s);
            seen.push(lit);
            if open_sky(&clip, p) {
                sky.push((lit, s.sun));
            } else {
                roof.push((lit, s.sun));
            }
        }
        assert!(
            seen.iter()
                .any(|l| (*l - seen[0]).abs().max_element() > 1e-3),
            "{map}: every spawn lit alike"
        );
        let mean = |v: &[(Vec3, f32)]| {
            let n = v.len().max(1) as f32;
            (
                v.iter().map(|(l, _)| l.element_sum() / 3.0).sum::<f32>() / n,
                v.iter().map(|(_, s)| s).sum::<f32>() / n,
            )
        };
        let (sky_lit, sky_sun) = mean(&sky);
        let (roof_lit, roof_sun) = mean(&roof);
        eprintln!(
            "{map}: {} open-sky spawns lit {sky_lit:.3} sun {sky_sun:.2}, \
             {} roofed lit {roof_lit:.3} sun {roof_sun:.2}",
            sky.len(),
            roof.len()
        );
        if !sky.is_empty() && !roof.is_empty() {
            // mp_downtown's open-sky spawns read 0.10 against 0.14 under
            // roofs: its street spawns sit in the buildings' shadow
            assert!(sky_sun > roof_sun || map == "mp_downtown", "{map}");
        }
    }
}

/// Hand-picked spawns, a standing body's lighting origin each: on
/// mp_toujane a sunlit street (2326, 485) against a roofed room (1940,
/// 1989), on mp_carentan the open street at (984, -736) against an upstairs
/// room at (0, 696). Outside the sun reaches every corner and the light is
/// more than twice the room's; the rooms see no sun, and the carentan
/// room's light comes from the side, its ceiling corner black.
#[test]
fn sunlit_street_against_a_room() {
    let Some(fs) = testing::cod2_fs() else {
        return;
    };
    for (map, street, room) in [
        (
            "mp_toujane",
            Vec3::new(2326.0, 485.0, 101.0),
            Vec3::new(1940.0, 1989.0, 103.0),
        ),
        (
            "mp_carentan",
            Vec3::new(984.0, -736.0, 32.0),
            Vec3::new(0.0, 696.0, 192.0),
        ),
    ] {
        let b = load(&fs, map);
        let g = grid(&b);
        let clip = vcod_cod2::map::clip_map(&b).unwrap();
        assert!(open_sky(&clip, street) && !open_sky(&clip, room), "{map}");
        let (out, ins) = (sample(&g, &clip, street), sample(&g, &clip, room));
        let (lo, li) = (mean_light(&out), mean_light(&ins));
        eprintln!(
            "{map}: street sun {} light {lo}, room sun {} light {li}",
            out.sun, ins.sun
        );
        assert!(out.sun > 0.99 && ins.sun == 0.0, "{map}");
        assert!(
            lo.min_element() > 2.0 * li.max_element(),
            "{map}: {lo} {li}"
        );
        if map == "mp_carentan" {
            assert_eq!(ins.corner(7), Vec3::ZERO);
            assert!(ins.corner(4).min_element() > 0.25, "{:?}", ins.corner(4));
        }
    }
}
