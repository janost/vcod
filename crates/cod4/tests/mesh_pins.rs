//! Pins the indexed-triangle trace kernels (`clipmap/mesh4.rs`) and the
//! terrain border arm on real 1.7 clip maps: point and capsule sweeps at
//! every 16th collision triangle's face, past its edges and onto its
//! corners, sideways capsules at its borders' tops, and capsule position
//! tests beside the face (`docs/research/cod4-collision.md` 2, 3). Each
//! map's digest folds every result's fraction, normal, flags and
//! material bit for bit, so a rewrite of the kernels that moves any of
//! them by a last bit fails here. `--nocapture` prints the counts.

use glam::Vec3;
use vcod_cod4::collision::{ClipMap, MASK_PLAYERSOLID, MASK_SHOT};
use vcod_cod4::map::Map;
use vcod_cod4::testing::cod4_install;
use vcod_common::clipmap::ClipTrace;

const PLAYER_MINS: Vec3 = Vec3::new(-15.0, -15.0, 0.0);
const PLAYER_MAXS: Vec3 = Vec3::new(15.0, 15.0, 70.0);
const SMALL_MINS: Vec3 = Vec3::new(-4.0, -4.0, -4.0);
const SMALL_MAXS: Vec3 = Vec3::new(4.0, 4.0, 4.0);

struct Pin {
    hash: u64,
    traces: usize,
    hits: usize,
    startsolid: usize,
    walkable: usize,
}

impl Pin {
    fn new() -> Pin {
        Pin {
            hash: 0xcbf2_9ce4_8422_2325,
            traces: 0,
            hits: 0,
            startsolid: 0,
            walkable: 0,
        }
    }

    fn fold(&mut self, x: u32) {
        for b in x.to_le_bytes() {
            self.hash = (self.hash ^ u64::from(b)).wrapping_mul(0x100_0000_01b3);
        }
    }

    fn add(&mut self, t: &ClipTrace) {
        self.traces += 1;
        self.hits += usize::from(t.fraction < 1.0);
        self.startsolid += usize::from(t.startsolid);
        self.walkable += usize::from(t.walkable && t.fraction < 1.0);
        self.fold(t.fraction.to_bits());
        for k in 0..3 {
            self.fold(t.normal[k].to_bits());
        }
        self.fold(
            u32::from(t.allsolid) | u32::from(t.startsolid) << 1 | u32::from(t.walkable) << 2,
        );
        self.fold(t.contents);
        self.fold(t.surface_flags);
        self.fold(t.material.map_or(u32::MAX, u32::from));
    }
}

fn pin(clip: &ClipMap) -> Pin {
    let w = &clip.world;
    let m = w.indexed.as_ref().expect("an indexed mesh");
    let mut p = Pin::new();
    let trace = |p: &mut Pin, a: Vec3, b: Vec3, mins: Vec3, maxs: Vec3, mask: u32| {
        p.add(&clip.trace(a, b, mins, maxs, mask));
    };
    let up = Vec3::Z;
    for tri in m.tris.iter().step_by(16) {
        let [v0, v1, v2] = tri.map(|v| m.verts[usize::from(v)]);
        let n = (v0 - v2).cross(v0 - v1);
        if n.length() < 1e-3 {
            continue;
        }
        let n = n.normalize();
        let c = (v0 + v1 + v2) / 3.0;
        // The face, front to back and back to front, and aimed at a corner.
        trace(
            &mut p,
            c + 16.0 * n,
            c - 16.0 * n,
            Vec3::ZERO,
            Vec3::ZERO,
            MASK_SHOT,
        );
        trace(
            &mut p,
            c - 16.0 * n,
            c + 16.0 * n,
            Vec3::ZERO,
            Vec3::ZERO,
            MASK_SHOT,
        );
        trace(
            &mut p,
            c + 16.0 * n,
            v0 - 4.0 * n,
            Vec3::ZERO,
            Vec3::ZERO,
            MASK_SHOT,
        );
        // A capsule dropped on the face, past each edge's middle and past
        // each corner, outward in the triangle's plane.
        let mut drops = vec![c];
        for (a, b, o) in [(v0, v1, v2), (v1, v2, v0), (v2, v0, v1)] {
            let mid = (a + b) * 0.5;
            drops.push(mid + (mid - o).normalize_or_zero() * 9.0);
            drops.push(a + (a - c).normalize_or_zero() * 6.0);
        }
        for d in drops {
            trace(
                &mut p,
                d + 64.0 * up,
                d - 32.0 * up,
                PLAYER_MINS,
                PLAYER_MAXS,
                MASK_PLAYERSOLID,
            );
            trace(
                &mut p,
                d + 24.0 * n,
                d - 8.0 * n,
                SMALL_MINS,
                SMALL_MAXS,
                MASK_PLAYERSOLID,
            );
        }
        // Sideways capsules at the face from four sides, at feet heights
        // round it.
        for (dx, dy) in [(1.0, 0.0), (0.0, 1.0), (-1.0, 0.0), (0.7, -0.7)] {
            let side = Vec3::new(dx, dy, 0.0) * 48.0;
            for dz in [-60.0, -30.0, -2.0, 0.5, 12.0] {
                let at = c + Vec3::new(0.0, 0.0, dz);
                trace(
                    &mut p,
                    at + side,
                    at - side,
                    PLAYER_MINS,
                    PLAYER_MAXS,
                    MASK_PLAYERSOLID,
                );
            }
        }
        // Position tests beside the face.
        for k in [0.0, 4.0, 14.9, 15.2, 30.0] {
            let at = c + k * n - 35.0 * up;
            trace(&mut p, at, at, PLAYER_MINS, PLAYER_MAXS, MASK_PLAYERSOLID);
            let at = c + k * n;
            trace(&mut p, at, at, SMALL_MINS, SMALL_MAXS, MASK_PLAYERSOLID);
        }
    }
    // Sideways capsules at every 4th border, toward its wall, at heights
    // that put the top beside the straight part, at either sphere and
    // clear of both.
    for b in w.borders.iter().step_by(4) {
        let [a, bb, cc] = b.dist_eq;
        let wall = Vec3::new(a, bb, 0.0);
        for t in [0.1, 0.5, 0.97] {
            let s = b.start + t * b.length;
            let top = b.z_base + b.z_slope * (s - b.start);
            let on = Vec3::new(a * cc + bb * s, bb * cc - a * s, top);
            for dz in [
                -75.0, -62.0, -52.0, -40.0, -30.0, -12.0, 5.0, 12.0, 18.0, 30.0,
            ] {
                let at = on + Vec3::new(0.0, 0.0, dz);
                trace(
                    &mut p,
                    at + 40.0 * wall,
                    at - 40.0 * wall,
                    PLAYER_MINS,
                    PLAYER_MAXS,
                    MASK_PLAYERSOLID,
                );
                let skew = Vec3::new(-bb, a, 0.0) * 30.0;
                trace(
                    &mut p,
                    at + 40.0 * wall + skew,
                    at - 20.0 * wall - skew,
                    PLAYER_MINS,
                    PLAYER_MAXS,
                    MASK_PLAYERSOLID,
                );
            }
        }
    }
    p
}

/// Each map's digest, as the kernels traced before their rewrite, under
/// 1.7's `CM_Trace` set-up: the shifted end kept on the x87 stack
/// (`cod4_lnxded` 0x811ce9f..0x811cfb8, cod4-collision.md 2), which the
/// kill-cam entity gate's dead body slide matches bit for bit.
const PINS: [(&str, u64); 3] = [
    ("mp_crash", 0x0a51_1444_af60_dca0),
    ("mp_backlot", 0xa403_a62c_8249_152a),
    ("mp_carentan", 0x5ac7_ad8b_6115_680d),
];

#[test]
fn mesh_kernels_trace_as_pinned() {
    let Some(install) = cod4_install() else {
        return;
    };
    let mut bad = Vec::new();
    for (name, want) in PINS {
        let (_, walked) = Map::load(&install, "english", name).unwrap();
        let clip = ClipMap::from_assets(&walked.assets).unwrap();
        let p = pin(&clip);
        println!(
            "{name}: traces {} hits {} startsolid {} walkable {} digest {:#018x}",
            p.traces, p.hits, p.startsolid, p.walkable, p.hash
        );
        if p.hash != want {
            bad.push(format!("{name} {:#018x}", p.hash));
        }
    }
    assert!(bad.is_empty(), "{bad:?}");
}
