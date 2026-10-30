//! CoD2's bullet tracer: a local entity on a linear trajectory from the
//! shooter's muzzle to the hit (`CG_SpawnTracer`, Mac 0x1d9886), drawn
//! each frame as one camera-facing quad (`CG_AddMovingTracer` 0x1dfef8,
//! `CG_DrawTracer` 0x1db3a4). Who spawns one and how often is the event
//! path's (`play::cod2_fx`); docs/research/cod2-efx.md 6.7. CoD4 1.7 draws
//! the same streak as an `FX_Beam` with a corkscrew ([`Screw`],
//! docs/research/cod4-effects.md 12).

use crate::fx::sim::FxQuad;
use glam::Vec3;

/// The dvar defaults `CG_Init` registers (Mac 0x145214).
/// `cg_tracerchance`: the share of rounds that draw one.
pub const CHANCE: f32 = 0.4;
/// `cg_tracerwidth`: half the quad's width.
pub const WIDTH: f32 = 0.8;
/// `cg_tracerSpeed`, units per second.
pub const SPEED: f32 = 4500.0;
/// `cg_tracerlength`: the streak's longest.
pub const LENGTH: f32 = 160.0;
/// `cg_tracerScale`: the most an end's width grows with its distance
/// from the eye; 1 turns the growth off.
pub const SCALE: f32 = 50.0;
/// `cg_tracerScaleMinDist`: distance from the eye where growth starts.
pub const SCALE_MIN_DIST: f32 = 5000.0;
/// `cg_tracerScaleDistRange`: the distance past that over which the
/// width grows by [`SCALE`].
pub const SCALE_DIST_RANGE: f32 = 25000.0;
/// `CG_InitLocalEntities` (Mac 0x1dfe94): 0x5e00 bytes of 0xbc-byte
/// local entities.
pub const MAX_LOCAL_ENTITIES: usize = 128;
/// `CG_RegisterGraphics`' tracer material (`cgs + 0x27bbc`).
pub const MATERIAL: &str = "gfx/misc/tracer";

/// A game's tracer dvars: CoD2's above, or CoD4 1.7's ([`COD4`]).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rules {
    pub speed: f32,
    pub width: f32,
    pub length: f32,
    pub scale: f32,
    pub material: &'static str,
    /// CoD4's corkscrew; CoD2 draws one flat quad.
    pub screw: Option<Screw>,
}

/// CoD4 1.7's `cg_tracerScrewDist` and `cg_tracerScrewRadius`
/// (iw3mp.exe 0x43cc45, 0x43cca7: 100 and 0.5): `CG_DrawTracer` (0x458390)
/// cuts the beam into `len * 8 / dist` segments (8.0 at 0x70ae88) and
/// `FX_Beam_GenerateVerts` (0x492ab0) moves segment end `i` by `radius`
/// times the eight-point circle at 0x6d40c8, entry `i & 7`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Screw {
    pub dist: f32,
    pub radius: f32,
}

/// The beam's circle (iw3mp.exe 0x6d40c8): x across the beam on screen, y
/// along it.
const WIGGLE: [[f32; 2]; 8] = [
    [0.0, 1.0],
    [0.71, 0.71],
    [1.0, 0.0],
    [0.71, -0.71],
    [0.0, -1.0],
    [-0.71, -0.71],
    [-1.0, 0.0],
    [-0.71, 0.71],
];

/// The near clip the beam's screen direction is taken past (`r_znear`).
const BEAM_NEAR: f32 = 4.0;

/// CoD2's defaults (the constants above).
pub const COD2: Rules = Rules {
    speed: SPEED,
    width: WIDTH,
    length: LENGTH,
    scale: SCALE,
    material: MATERIAL,
    screw: None,
};

/// CoD4 1.7's defaults: `cg_tracerwidth` 4, `cg_tracerSpeed` 7500,
/// `cg_tracerlength` 160, `cg_tracerScale` 1 (iw3mp.exe 0x43c9ff..0x43cb20),
/// the beam drawn in `gfx_tracer` (docs/research/cod4-effects.md 12).
pub const COD4: Rules = Rules {
    speed: 7500.0,
    width: 4.0,
    length: 160.0,
    scale: 1.0,
    material: "gfx_tracer",
    screw: Some(Screw {
        dist: 100.0,
        radius: 0.5,
    }),
};

/// One tracer in flight.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Tracer {
    pub rules: Rules,
    /// `trBase`: the muzzle.
    pub muzzle: Vec3,
    pub dir: Vec3,
    /// Muzzle to hit (`le + 0x44`).
    pub dist: f32,
    /// `trTime`, seconds: the spawn, back-dated.
    pub start: f32,
    /// `endTime`, seconds: when the tail reaches the hit.
    pub end: f32,
}

impl Tracer {
    /// `CG_SpawnTracer` at `now` (seconds), back-dated `back_ms`
    /// (`(rand() % cg.frametime) / 2`, the caller's roll). The life is
    /// the whole distance at [`SPEED`], truncated to whole ms.
    pub fn new(muzzle: Vec3, hit: Vec3, now: f32, back_ms: i32) -> Tracer {
        Tracer::with(COD2, muzzle, hit, now, back_ms)
    }

    /// [`Self::new`] under `rules`.
    pub fn with(rules: Rules, muzzle: Vec3, hit: Vec3, now: f32, back_ms: i32) -> Tracer {
        let d = hit - muzzle;
        let dist = d.length();
        let start = now - back_ms as f32 / 1000.0;
        let life_ms = (dist * 1000.0 / rules.speed) as i32;
        Tracer {
            rules,
            muzzle,
            dir: d.normalize_or_zero(),
            dist,
            start,
            end: start + life_ms as f32 / 1000.0,
        }
    }

    /// `CG_AddLocalEntities`' test: drawn while `start <= now < end`.
    pub fn live(&self, now: f32) -> bool {
        self.start <= now && now < self.end
    }

    /// `CG_AddMovingTracer`: (tail, head) at `now`. The tail rides the
    /// trajectory; the head leads it by [`LENGTH`], never past the hit.
    pub fn segment(&self, now: f32) -> (Vec3, Vec3) {
        let tail = self.muzzle + self.dir * (self.rules.speed * (now - self.start));
        let left = self.dist - (tail - self.muzzle).dot(self.dir);
        (tail, tail + self.dir * left.min(self.rules.length))
    }

    /// `CG_DrawTracer` from the eye at `eye` with view axes `right` and
    /// `up`: each end `width` either side along the shot's screen-space
    /// perpendicular, the head's u 1 and the tail's 0.
    pub fn quad(&self, now: f32, eye: Vec3, right: Vec3, up: Vec3) -> FxQuad {
        let (tail, head) = self.segment(now);
        let d = head - tail;
        // `-(d . left) up + (d . up) left`, left being -right.
        let perp = (up * d.dot(right) - right * d.dot(up)).normalize_or_zero();
        let wt = rules_width(&self.rules, tail.distance(eye));
        let wh = rules_width(&self.rules, head.distance(eye));
        FxQuad {
            verts: [
                (head + perp * wh).to_array(),
                (head - perp * wh).to_array(),
                (tail - perp * wt).to_array(),
                (tail + perp * wt).to_array(),
            ],
            uvs: [[1.0, 1.0], [1.0, 0.0], [0.0, 0.0], [0.0, 1.0]],
            rgba: [1.0; 4],
            shader: self.rules.material.into(),
        }
    }
}

impl Tracer {
    /// What `build_quads` draws for it at `now` from the eye at `eye`
    /// looking along `fwd`: CoD2's one [`Self::quad`], or CoD4's beam.
    pub fn quads(
        &self,
        now: f32,
        eye: Vec3,
        fwd: Vec3,
        right: Vec3,
        up: Vec3,
        out: &mut Vec<FxQuad>,
    ) {
        match self.rules.screw {
            None => out.push(self.quad(now, eye, right, up)),
            Some(screw) => {
                let (tail, head) = self.segment(now);
                let wt = rules_width(&self.rules, tail.distance(eye));
                let wh = rules_width(&self.rules, head.distance(eye));
                beam(
                    tail,
                    head,
                    (wt, wh),
                    screw,
                    (eye, fwd, right, up),
                    self.rules.material,
                    out,
                );
            }
        }
    }
}

/// `FX_Beam_GenerateVerts` (iw3mp.exe 0x492xxx) for one beam from `begin`
/// to `end` with radii `radius` at its ends: `flat`, the beam's direction
/// on screen taken back into the world (`FX_GenerateBeam_GetFlatDelta`: the
/// ends' clip positions past the near plane, divided,
/// their difference with z and w zeroed through the inverse clip
/// matrix), and `perp`, `flat x forward`. A cap point `radius` behind
/// `begin` along `flat` (uv 0 0), then each segment end `i` of `n` at
/// `i / n` along the beam, moved `screw.radius` by the circle's entry
/// `i & 7` (x along `perp`, y along `flat`), its two side points
/// `-r perp` (uv 1 0) and `+r perp` (uv 0 1), then a cap point ahead of
/// `end` (uv 1 1). The uvs are the vertices' packed halves (0x3c00 and
/// 0x3c000000 at 0x4933dd); the body samples the image's anti-diagonal.
fn beam(
    begin: Vec3,
    end: Vec3,
    radius: (f32, f32),
    screw: Screw,
    view: (Vec3, Vec3, Vec3, Vec3),
    material: &str,
    out: &mut Vec<FxQuad>,
) {
    let (eye, fwd, right, up) = view;
    let Some(flat) = flat_delta(begin, end, eye, fwd, right, up) else {
        return;
    };
    let perp = flat.cross(fwd).normalize_or_zero();
    let len = begin.distance(end);
    // `(int)(len * 8.0 / cg_tracerScrewDist)`; retail divides by a zero
    // count at the streak's last 12 units, vcod draws one segment there
    let n = ((len * 8.0 / screw.dist) as i32).max(1);
    let shader: std::sync::Arc<str> = material.into();
    let side = |i: i32| {
        let a = i as f32 / n as f32;
        let w = WIGGLE[(i & 7) as usize];
        let base = begin.lerp(end, a) + perp * (screw.radius * w[0]) + flat * (screw.radius * w[1]);
        let r = radius.0 + (radius.1 - radius.0) * a;
        (base - perp * r, base + perp * r)
    };
    let quad = |verts: [Vec3; 4], uvs: [[f32; 2]; 4]| FxQuad {
        verts: verts.map(|v| v.to_array()),
        uvs,
        rgba: [1.0; 4],
        shader: shader.clone(),
    };
    let (a0, a1) = side(0);
    let cap = begin - flat * radius.0;
    out.push(quad(
        [cap, a0, a1, a1],
        [[0.0, 0.0], [1.0, 0.0], [0.0, 1.0], [0.0, 1.0]],
    ));
    let mut prev = (a0, a1);
    for i in 1..=n {
        let next = side(i);
        out.push(quad(
            [prev.0, next.0, next.1, prev.1],
            [[1.0, 0.0], [1.0, 0.0], [0.0, 1.0], [0.0, 1.0]],
        ));
        prev = next;
    }
    let cap = end + flat * radius.1;
    out.push(quad(
        [prev.0, cap, prev.1, prev.1],
        [[1.0, 0.0], [1.0, 1.0], [0.0, 1.0], [0.0, 1.0]],
    ));
}

/// The beam's screen direction in the world, unit length; `None` when the
/// beam lies behind the near plane or end on to the eye. The projection's
/// scales cancel: `x / z` on each axis is all the inverse keeps.
fn flat_delta(begin: Vec3, end: Vec3, eye: Vec3, fwd: Vec3, right: Vec3, up: Vec3) -> Option<Vec3> {
    let view = |p: Vec3| {
        let d = p - eye;
        Vec3::new(d.dot(right), d.dot(up), d.dot(fwd))
    };
    let (mut a, mut b) = (view(begin), view(end));
    if a.z < BEAM_NEAR && b.z < BEAM_NEAR {
        return None;
    }
    // clip the nearer end to the near plane
    if a.z < BEAM_NEAR {
        a = a + (b - a) * ((BEAM_NEAR - a.z) / (b.z - a.z));
    } else if b.z < BEAM_NEAR {
        b = b + (a - b) * ((BEAM_NEAR - b.z) / (a.z - b.z));
    }
    let dx = b.x / b.z - a.x / a.z;
    let dy = b.y / b.z - a.y / a.z;
    let d = right * dx + up * dy;
    (d.length_squared() >= 2e-6).then(|| d.normalize())
}

/// `CG_DrawTracer`'s width at an end `dist` from the eye: [`WIDTH`] up to
/// [`SCALE_MIN_DIST`], then times `(dist - min) / range * scale` held to
/// `[1, scale]`.
#[cfg(test)]
pub fn end_width(dist: f32) -> f32 {
    rules_width(&COD2, dist)
}

/// [`end_width`] under `rules`.
fn rules_width(r: &Rules, dist: f32) -> f32 {
    if r.scale == 1.0 {
        return r.width;
    }
    let d = if SCALE_MIN_DIST != 0.0 {
        dist - SCALE_MIN_DIST
    } else {
        dist
    };
    if d <= 0.0 {
        return r.width;
    }
    let f = if SCALE_DIST_RANGE <= 0.0 {
        r.scale
    } else {
        (d / SCALE_DIST_RANGE * r.scale).clamp(1.0, r.scale)
    };
    r.width * f
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The tail leaves the muzzle at 4500 u/s, the head 160 ahead until it
    /// meets the hit, and the tracer dies as the tail gets there.
    #[test]
    fn the_streak_runs_from_muzzle_to_hit() {
        let t = Tracer::new(Vec3::ZERO, Vec3::new(1000.0, 0.0, 0.0), 1.0, 0);
        // 1000 / 4500 s = 222.2 ms, truncated.
        assert!((t.end - t.start - 0.222).abs() < 1e-5);
        let (tail, head) = t.segment(1.0);
        assert_eq!((tail.x, head.x), (0.0, 160.0));
        let (tail, head) = t.segment(1.1);
        assert!((tail.x - 450.0).abs() < 0.1 && (head.x - 610.0).abs() < 0.1);
        // Near the end the head stops at the hit.
        let (tail, head) = t.segment(1.2);
        assert!((tail.x - 900.0).abs() < 0.1 && (head.x - 1000.0).abs() < 0.01);
        assert!(t.live(1.0) && t.live(1.221) && !t.live(1.2225) && !t.live(0.999));
        // A back-date starts it earlier and ends it as much earlier.
        let b = Tracer::new(Vec3::ZERO, Vec3::new(1000.0, 0.0, 0.0), 1.0, 8);
        assert!((b.start - 0.992).abs() < 1e-6 && (b.end - 1.214).abs() < 1e-5);
    }

    /// A shot across the view: the quad faces the eye, 0.8 either side up
    /// close, wider past 5000 units.
    #[test]
    fn the_quad_faces_the_eye_and_widens_far_off() {
        let t = Tracer::new(
            Vec3::new(0.0, 100.0, 0.0),
            Vec3::new(1000.0, 100.0, 0.0),
            0.0,
            0,
        );
        // Looking down +y: right is +x.
        let q = t.quad(0.0, Vec3::ZERO, Vec3::X, Vec3::Z);
        assert_eq!(&*q.shader, "gfx/misc/tracer");
        let v: Vec<Vec3> = q.verts.iter().map(|v| Vec3::from(*v)).collect();
        // Head at x 160, tail at 0, offset along z only.
        assert_eq!(v[0].x, 160.0);
        assert_eq!(v[2].x, 0.0);
        assert!((v[0].z.abs() - 0.8).abs() < 1e-6 && v[0].y == 100.0);
        assert_eq!(v[0].z, -v[1].z);
        assert_eq!(end_width(5000.0), WIDTH);
        // 5000 past the start: 10x, held at 1x below, 50x at most.
        assert!((end_width(10000.0) - WIDTH * 10.0).abs() < 1e-4);
        assert_eq!(end_width(5100.0), WIDTH);
        assert_eq!(end_width(1e6), WIDTH * SCALE);
    }

    /// CoD4's beam across the view: 160 units cut into 12 segments, a cap
    /// each end, every segment end moved half a unit round the circle;
    /// across the screen (`perp`, here z) the screw runs sin-like.
    #[test]
    fn the_cod4_beam_screws() {
        let t = Tracer::with(
            COD4,
            Vec3::new(0.0, 100.0, 0.0),
            Vec3::new(1000.0, 100.0, 0.0),
            0.0,
            0,
        );
        let mut q = Vec::new();
        // looking down +y: right +x, up +z, forward up x right
        t.quads(0.0, Vec3::ZERO, Vec3::Y, Vec3::X, Vec3::Z, &mut q);
        assert_eq!(q.len(), 14);
        assert!(q.iter().all(|q| &*q.shader == "gfx_tracer"));
        // the begin cap 4 units behind the tail along the beam
        assert!((q[0].verts[0][0] + 4.0).abs() < 1e-5);
        // segment end i's middle: x i * 160 / 12 + 0.5 cos, z 0.5 sin
        for (i, quad) in q[1..13].iter().enumerate() {
            let a = Vec3::from(quad.verts[1]);
            let b = Vec3::from(quad.verts[2]);
            let mid = (a + b) * 0.5;
            let w = WIGGLE[(i + 1) & 7];
            let x = (i + 1) as f32 * 160.0 / 12.0;
            assert!((mid.x - (x + 0.5 * w[1])).abs() < 1e-3, "{i} {mid}");
            assert!((mid.z.abs() - 0.5 * w[0].abs()).abs() < 1e-4, "{i} {mid}");
            // the sides 4 either side across the screen
            assert!((a.distance(b) - 8.0).abs() < 1e-3);
        }
        // a beam wholly behind the eye draws nothing
        let mut none = Vec::new();
        let back = Tracer::with(
            COD4,
            Vec3::new(0.0, -100.0, 0.0),
            Vec3::new(1000.0, -100.0, 0.0),
            0.0,
            0,
        );
        back.quads(0.0, Vec3::ZERO, Vec3::Y, Vec3::X, Vec3::Z, &mut none);
        assert!(none.is_empty());
    }

    /// The stock material is an additive effect whose map is `tracer`.
    #[test]
    fn the_stock_material_loads() {
        let Some(fs) = vcod_common::testing::cod2_fs() else {
            return;
        };
        let lib = vcod_cod2::materials::load_lib(&fs).unwrap();
        let m = lib.get(MATERIAL).expect(MATERIAL);
        println!("{m:?}");
        assert_eq!(m.stages.len(), 1);
        assert_eq!(format!("{:?}", m.stages[0].blend), "Some((One, One))");
    }
}
