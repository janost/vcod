//! A map's createfx ambience as 1.7 culls it from one view: every element
//! of every placed effect (runners followed one level), and whether
//! `FX_CullElemForSpawn` (iw3mp.exe 0x4a0b90: the spawn range, and with
//! flag 0x4 the frustum at `spawnFrustumCullRadius`) lets it spawn there
//! and what `FX_EvaluateDistanceFade` (0x498010) leaves of its alpha
//! (cod4-effects.md 5, 18).
//!
//!     COD4_DIR=... cargo run -p vcod-cod4 --example fx_ranges -- \
//!         MAP X Y Z YAW PITCH [RADIUS [FOV ASPECT]]
//!
//! X Y Z YAW PITCH as `setviewpos` takes them (the origin; the eye is 60
//! up, standing). FOV is `cg_fov` (65), ASPECT the view's (16:9), widened
//! as 1.7's `CG_GetViewFov` does past 4:3. Distances and the frustum are
//! the placed origin's, which `FX_SpawnElement` (0x4a25b0) tests: an
//! element's own offset moves where it draws, not whether it spawns.

use anyhow::{Context, Result};
use vcod_cod4::assets::Assets;
use vcod_cod4::assets::fx::{Fx, FxVisual};
use vcod_cod4::fx::{ElemDef, Range, createfx, flags, materials, roles};
use vcod_cod4::testing::cod4_install;
use vcod_cod4::walk::{WalkOptions, walk};
use vcod_cod4::zone::Zone;

fn find<'a>(zones: &'a [Assets], name: &str) -> Option<(&'a Assets, &'a Fx)> {
    zones.iter().find_map(|a| {
        a.fx.iter()
            .find(|f| f.name.eq_ignore_ascii_case(name))
            .map(|f| (a, f))
    })
}

/// `FX_ClampRangeLerp` (0x497fb0): 1 inside the base, 0 past base plus
/// amplitude, linear between.
fn clamp_lerp(dist: f32, r: Range) -> f32 {
    let x = dist - r.base;
    if x < 0.0 {
        1.0
    } else if r.amplitude > x {
        1.0 - x / r.amplitude
    } else {
        0.0
    }
}

/// `FX_EvaluateDistanceFade`'s factor at `dist`, 0..=255.
fn fade(d: &ElemDef, dist: f32) -> u32 {
    let (i, o) = (d.fade_in_range, d.fade_out_range);
    if i.amplitude == 0.0 && o.amplitude == 0.0 {
        return 255;
    }
    let fin = if i.amplitude != 0.0 {
        clamp_lerp(dist, i)
    } else {
        1.0
    };
    let fout = if o.amplitude != 0.0 {
        1.0 - clamp_lerp(dist, o)
    } else {
        1.0
    };
    (fin.min(fout) * 255.0 + 0.5) as u32
}

/// The five planes of `FX_SetNextUpdateCamera` as (normal, distance).
struct View {
    eye: [f32; 3],
    planes: Vec<([f32; 3], f32)>,
}

fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

impl View {
    fn new(eye: [f32; 3], yaw: f32, pitch: f32, fov: f32, aspect: f32) -> View {
        let (sy, cy) = yaw.to_radians().sin_cos();
        let (sp, cp) = pitch.to_radians().sin_cos();
        let fwd = [cp * cy, cp * sy, -sp];
        let left = [-sy, cy, 0.0];
        let up = [sp * cy, sp * sy, cp];
        // 1.7's fov is 4:3's, widened to the view's aspect.
        let tan_y = (fov.to_radians() / 2.0).tan() * 0.75;
        let tan_x = tan_y * aspect;
        let side = |t: f32, axis: [f32; 3], sign: f32| {
            let c = 1.0 / (t * t + 1.0).sqrt();
            let s = t * c;
            std::array::from_fn(|k| s * fwd[k] + sign * c * axis[k])
        };
        let normals = [
            fwd,
            side(tan_x, left, 1.0),
            side(tan_x, left, -1.0),
            side(tan_y, up, 1.0),
            side(tan_y, up, -1.0),
        ];
        View {
            eye,
            planes: normals.iter().map(|n| (*n, dot(eye, *n))).collect(),
        }
    }

    /// `FX_CullSphere` (0x4a6290).
    fn culls(&self, p: [f32; 3], r: f32) -> bool {
        self.planes.iter().any(|(n, d)| dot(*n, p) - d <= -r)
    }
}

fn walk_fx(zones: &[Assets], name: &str, at: [f32; 3], view: &View, depth: usize) {
    let Some((a, fx)) = find(zones, name) else {
        println!("{:depth$}{name}: not in the zones", "");
        return;
    };
    let dist = dot(
        std::array::from_fn(|k| at[k] - view.eye[k]),
        std::array::from_fn(|k| at[k] - view.eye[k]),
    )
    .sqrt();
    for (i, (role, e)) in roles(fx).enumerate() {
        let d = ElemDef::of(e);
        let ranged = d.spawn_range.amplitude == 0.0 || {
            let x = dist - d.spawn_range.base;
            x >= 0.0 && x < d.spawn_range.amplitude
        };
        let frustum = d.flags & flags::SPAWN_FRUSTUM_CULL != 0;
        let seen = !frustum || !view.culls(at, d.spawn_frustum_cull_radius);
        let verdict = match (ranged, seen) {
            (false, _) => "out of range".to_string(),
            (true, false) => "frustum culled".to_string(),
            (true, true) => format!("spawns, fade {}", fade(&d, dist)),
        };
        println!(
            "{:depth$}{name}[{i}] {role:?} {:?} flags {:#x} range {:?} fade in {:?} out {:?} cull radius {}: {verdict} {:?}",
            "",
            d.elem_type,
            d.flags,
            d.spawn_range.ends(),
            d.fade_in_range.ends(),
            d.fade_out_range.ends(),
            if frustum {
                d.spawn_frustum_cull_radius.to_string()
            } else {
                "-".into()
            },
            materials(a, e),
        );
        if ranged && seen {
            for v in &e.visuals {
                if let FxVisual::Effect(Some(sub)) = v {
                    walk_fx(zones, sub, at, view, depth + 2);
                }
            }
        }
    }
}

fn main() -> Result<()> {
    let usage = "usage: MAP X Y Z YAW PITCH [RADIUS [FOV ASPECT]]";
    let args: Vec<String> = std::env::args().skip(1).collect();
    let num = |i: usize, default: Option<f32>| -> Result<f32> {
        match args.get(i) {
            Some(a) => a.parse().context(usage),
            None => default.context(usage),
        }
    };
    let map = args.first().context(usage)?;
    let origin = [num(1, None)?, num(2, None)?, num(3, None)?];
    let eye = [origin[0], origin[1], origin[2] + 60.0];
    let view = View::new(
        eye,
        num(4, None)?,
        num(5, None)?,
        num(7, Some(65.0))?,
        num(8, Some(16.0 / 9.0))?,
    );
    let radius = num(6, Some(2000.0))?;
    let install = cod4_install().context("COD4_DIR")?;
    let open = |n: &str| -> Result<Assets> {
        let z = Zone::open_path(&install.zone("english", n))?;
        Ok(walk(z, WalkOptions { keep_bulk: true })?.assets)
    };
    let zones = [open(map)?, open("common_mp")?];
    let raw = |name: &str| {
        zones[0]
            .raw_files
            .iter()
            .find(|r| r.name == name)
            .map(|r| createfx::text(&r.data))
            .with_context(|| name.to_string())
    };
    let names = createfx::effect_map(&raw(&format!("maps/mp/{map}_fx.gsc"))?);
    let placed = createfx::placed(&raw(&format!("maps/createfx/{map}_fx.gsc"))?);
    let mut near: Vec<_> = placed
        .effects
        .iter()
        .map(|e| {
            let off = std::array::from_fn(|k| e.origin[k] - eye[k]);
            (dot(off, off).sqrt(), e)
        })
        .filter(|(d, _)| *d <= radius)
        .collect();
    near.sort_by(|a, b| a.0.total_cmp(&b.0));
    for (dist, e) in near {
        let Some((_, file)) = names.iter().find(|(n, _)| *n == e.fxid) else {
            continue;
        };
        println!("== {} ({file}) at {:?}, {dist:.0} units", e.fxid, e.origin);
        walk_fx(&zones, file, e.origin, &view, 2);
    }
    Ok(())
}
