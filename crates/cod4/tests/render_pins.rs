//! Pins of the light grid lookup (`LightGrid::lighting_at`) and the sun
//! shadow placement (`sunshadow::setup`, `Scale::of`) on mp_crash and
//! mp_backlot: a digest of every output bit at a fixed set of points and
//! views, so a rewrite that moves one float's last bit shows
//! (cod4-world.md 3.1, 3.6). `--nocapture` prints the digests.

use glam::{Mat4, Vec3};
use vcod_cod4::collision::ClipMap;
use vcod_cod4::map::Map;
use vcod_cod4::sunshadow::{self, Scale, Settings, View};
use vcod_cod4::testing::cod4_install;

/// FNV-1a over the words fed in.
struct Digest(u64);

impl Digest {
    fn new() -> Digest {
        Digest(0xcbf2_9ce4_8422_2325)
    }
    fn bytes(&mut self, b: &[u8]) {
        for &x in b {
            self.0 = (self.0 ^ u64::from(x)).wrapping_mul(0x100_0000_01b3);
        }
    }
    fn u32(&mut self, v: u32) {
        self.bytes(&v.to_le_bytes());
    }
    fn f32(&mut self, v: f32) {
        self.u32(v.to_bits());
    }
    fn mat(&mut self, m: &Mat4) {
        m.to_cols_array().iter().for_each(|&v| self.f32(v));
    }
    fn vec4(&mut self, v: glam::Vec4) {
        v.to_array().iter().for_each(|&x| self.f32(x));
    }
}

const MAPS: [(&str, u64, u64); 2] = [
    ("mp_crash", 0x325f_bbba_74c2_a77a, 0x67de_aa3c_d9aa_155d),
    ("mp_backlot", 0x4813_72f3_bc4d_8cce, 0x73ea_64bd_822a_f8f2),
];

fn spawns(map: &Map) -> Vec<([f32; 3], f32)> {
    let mut out = Vec::new();
    for e in &map.entities {
        let get = |key: &str| e.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str());
        if !get("classname").is_some_and(|c| c.starts_with("mp_") && c.ends_with("_spawn")) {
            continue;
        }
        let Some(o) = get("origin").and_then(vcod_common::bsp::parse_vec3) else {
            continue;
        };
        let yaw = get("angles")
            .and_then(vcod_common::bsp::parse_vec3)
            .map_or(0.0, |a| a[1]);
        out.push((o, yaw));
    }
    out
}

#[test]
fn the_light_grid_lookup_holds_its_bits() {
    let Some(install) = cod4_install() else {
        return;
    };
    for (name, want, _) in MAPS {
        let (map, walked) = Map::load(&install, "english", name).unwrap();
        let g = map.light_grid.as_ref().unwrap();
        let clip = ClipMap::from_assets(&walked.assets).unwrap();
        let blocked = |a: [f32; 3], b: [f32; 3]| {
            clip.trace(
                a.into(),
                b.into(),
                Vec3::ZERO,
                Vec3::ZERO,
                vcod_cod4::lightgrid::SIGHT_MASK,
            )
            .fraction
                < 1.0
        };
        let lights = vcod_cod4::primarylight::PrimaryLights::of(&map);
        let mut points: Vec<([f32; 3], u8)> = map
            .static_models
            .iter()
            .map(|sm| (sm.lighting_origin, sm.primary_light))
            .collect();
        for (o, _) in spawns(&map) {
            let spot = lights.for_box([o[0], o[1], o[2] + 35.0], [15.0, 15.0, 35.0]);
            points.push(([o[0], o[1], o[2] + 32.0], spot));
        }
        // a lattice over the map's bounds, off the cell grid
        let (lo, hi) = (Vec3::from(map.mins), Vec3::from(map.maxs));
        for i in 0..28 {
            for j in 0..28 {
                for k in 0..7 {
                    let t = Vec3::new(i as f32 + 0.37, j as f32 + 0.61, k as f32 + 0.19)
                        / Vec3::new(28.0, 28.0, 7.0);
                    let p = lo + (hi - lo) * t;
                    let spot = lights.for_box(p.into(), [15.0, 15.0, 35.0]);
                    points.push((p.into(), spot));
                }
            }
        }
        let mut d = Digest::new();
        let (mut partial, mut missing) = (0, 0);
        for (p, light) in &points {
            let l = g.lighting_at(*p, *light, &blocked);
            l.colors.iter().for_each(|c| d.bytes(c));
            d.bytes(&[l.primary_weight, l.primary_light]);
            if l.primary_light == 0 && l.primary_weight == 255 {
                missing += 1;
            }
            if !matches!(l.primary_weight, 0 | 255) {
                partial += 1;
            }
        }
        println!(
            "{name}: {} points, {partial} partly lit, {missing} missing: {:#018x}",
            points.len(),
            d.0
        );
        assert_eq!(d.0, want, "{name}: the light grid digest");
    }
}

#[test]
fn the_sun_shadow_setup_holds_its_bits() {
    let mut d = Digest::new();
    for i in 0..=300 {
        let s = Scale::of(0.25 + i as f32 * 0.0025);
        d.f32(s.scale);
        d.f32(s.num);
        d.u32(s.size);
        d.f32(s.ratio);
    }
    println!("Scale::of: {:#018x}", d.0);
    assert_eq!(d.0, 0xe210_e7c3_23d4_5df8, "the scale digest");

    let Some(install) = cod4_install() else {
        return;
    };
    for (name, _, want) in MAPS {
        let (map, _) = Map::load(&install, "english", name).unwrap();
        let to_sun = Vec3::from(map.sun().expect("a sun").dir);
        let bounds = (Vec3::from(map.mins), Vec3::from(map.maxs));
        let spots = spawns(&map);
        let settings = [
            Settings::default(),
            Settings {
                scale: 0.5,
                ..Settings::default()
            },
            Settings {
                scale: 0.3,
                ..Settings::default()
            },
            Settings {
                scale: 0.77,
                sample_size_near: 0.5,
                ..Settings::default()
            },
            Settings {
                center: Vec3::from(spots[0].0) + Vec3::new(37.0, -91.0, 0.0),
                ..Settings::default()
            },
        ];
        let mut d = Digest::new();
        let mut views = 0;
        for (o, yaw) in &spots {
            let eye = Vec3::from(*o) + Vec3::Z * 60.0;
            for (dyaw, pitch) in [(0.0, 0.0), (90.0, -35.0), (200.0, 50.0), (315.0, 10.0)] {
                let (y, p) = (
                    (yaw + dyaw as f32).to_radians(),
                    (pitch as f32).to_radians(),
                );
                let fwd = Vec3::new(y.cos() * p.cos(), y.sin() * p.cos(), -p.sin());
                let aspect = 16.0 / 9.0;
                let fov_y = 2.0 * ((80f32.to_radians() / 2.0).tan() / aspect).atan();
                let proj =
                    glam::camera::rh::proj::directx::perspective(fov_y, aspect, 4.0, 60000.0);
                let vp = proj * glam::camera::rh::view::look_to_mat4(eye, fwd, Vec3::Z);
                let view = View::of(vp, eye, fwd, 4.0);
                for st in &settings {
                    let sh = sunshadow::setup(&view, to_sun, bounds, st);
                    for part in &sh.partitions {
                        d.mat(&part.view_proj);
                        part.viewport.iter().for_each(|&v| d.u32(v));
                        d.vec4(part.polygon_offset);
                        d.u32(part.cull.count as u32);
                        part.cull.planes.iter().for_each(|&p| d.vec4(p));
                    }
                    d.mat(&sh.lookup);
                    d.vec4(sh.switch_partition);
                    d.vec4(sh.scale);
                    views += 1;
                }
            }
        }
        println!("{name}: {views} setups: {:#018x}", d.0);
        assert_eq!(d.0, want, "{name}: the sun shadow digest");
    }
}
