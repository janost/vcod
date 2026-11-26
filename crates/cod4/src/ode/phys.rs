//! iw3mp.exe 1.7's frame glue over the ODE port, one [`PhysWorld`] per
//! world: `Phys_RunToTime` (0x5a6d50), `Phys_RunFrame` (0x5a6440),
//! `Phys_PostStep` (0x5a6640) with `Phys_DoBodyOncePerFrame` (0x5a62f0),
//! the after-frame checks (0x5a6c10) and their sweep (0x5a6930); and the
//! pushes, `Phys_ObjAddForce` (0x5a5900) and `Phys_ObjBulletImpact`
//! (0x5a5a20). cod4-player-anims.md 10.7.1.

use glam::Vec3;

use super::Body;
use super::body::DISABLED;
use super::collide::{self, Capsule, Obj, Pieces};
use super::math::{M3, V3};
use super::world::{BodyId, Params, PhysDvars, World};
use crate::ragdoll::{CLIP_MASK, Sweep};

#[inline]
fn w(x: f32) -> f64 {
    x as f64
}

/// Retail's three worlds (`Phys_Init` 0x5a2cd0 makes them at 0xd5ec6e0).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum WorldId {
    /// The map's dynamic entities.
    Dynent = 0,
    /// Launched parts and FX physics models.
    Fx = 1,
    Ragdoll = 2,
}

impl WorldId {
    pub fn index(self) -> usize {
        self as usize
    }
}

/// `g_phys_msecStep` at load (0x71e798, in `.data`: `Phys_Init` leaves it).
pub const MSEC_STEP_START: [i32; 3] = [17, 17, 17];
/// `msecStep`'s floor and ceiling by world (0x71e7a4, 0x71e7b0).
pub const MSEC_STEP_MIN: [i32; 3] = [17, 17, 17];
pub const MSEC_STEP_MAX: [i32; 3] = [67, 67, 34];
/// `0.001f` widened (0x70af98): a step's seconds per ms.
const MS_TO_S: f64 = 0.001_000_000_047_497_451_3;
/// World 2's angular damping a step (the double at 0x70b488).
const AVEL_DAMP: f64 = -0.009_999_999_776_482_582;
/// `sqrt(1/2)` as a float widened (the double at 0x70ae70).
const HALF_SQRT2: f64 = 0.707_106_769_084_930_4;
/// The sweep box's share of the geom's bounds (the double at 0x70b3e8).
const SWEEP_SHARE: f64 = 0.200_000_002_980_232_24;
/// A thin geom's sweep is a point this big (the floats at 0x70b460, 0x70b278).
const POINT: f32 = 0.001;

/// What a world's bodies sweep against after a frame, and collide with.
pub trait Clip: Pieces {
    /// A box sweep from `a` to `b` under the worlds' mask 0x2806c91, the
    /// box `mins..maxs` about the point.
    fn sweep(&self, a: Vec3, b: Vec3, mins: Vec3, maxs: Vec3) -> Sweep;
    /// The world model's bounds (cmodel 0).
    fn bounds(&self) -> (Vec3, Vec3);
}

impl Clip for crate::collision::ClipMap {
    fn sweep(&self, a: Vec3, b: Vec3, mins: Vec3, maxs: Vec3) -> Sweep {
        let t = self.trace(a, b, mins, maxs, CLIP_MASK);
        Sweep {
            fraction: t.fraction,
            normal: t.normal,
            startsolid: t.startsolid,
        }
    }

    fn bounds(&self) -> (Vec3, Vec3) {
        self.submodels
            .first()
            .map_or((Vec3::splat(-f32::MAX), Vec3::splat(f32::MAX)), |m| {
                (m.mins, m.maxs)
            })
    }
}

/// A world's clock (the block at `0xcc54484 + 0xcc w`): the time at the
/// last frame's first step, the time stepped to, the lerp between them
/// (`timeNowLerpFrac`); and its `g_phys_msecStep`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Clock {
    pub last: i32,
    pub now: i32,
    pub frac: f32,
    pub msec_step: i32,
}

/// The ODE geom classes 0x5a9a20 sizes (geom +4).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GeomClass {
    Box = 1,
    Brush = 12,
    Cylinder = 13,
    Capsule = 14,
}

/// A body's first geom, as far as the glue reads it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Geom {
    /// Class 14 (0x5a8e10): radius (+0x50), half length (+0x54).
    Capsule { radius: f32, half_len: f32 },
    /// A class whose geom is not ported yet (P2 of the world 0/1 port).
    Unported(GeomClass),
}

/// The glue met a geom class it has no arm for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Unported(pub GeomClass);

impl std::fmt::Display for Unported {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "geom class {:?} is not ported", self.0)
    }
}

impl std::error::Error for Unported {}

impl Geom {
    /// 0x5a9a20's box about the body: the capsule's arm of 0x5a67d0
    /// (0x5a68b7), `m = r sqrt(1/2)` unless `2 r + h` is less, the box
    /// `-m..m`. The box (0x5a9a44), brush (0x5a684f) and cylinder
    /// (0x5a688b) arms come with their geoms.
    pub fn bounds(&self) -> Result<(V3, V3), Unported> {
        match *self {
            Geom::Capsule { radius, half_len } => {
                let t = (w(radius) * HALF_SQRT2) as f32;
                let s = w(radius) * 2.0 + w(half_len);
                let m = if s < w(t) { s as f32 } else { t };
                Ok(([-m; 3], [m; 3]))
            }
            Geom::Unported(c) => Err(Unported(c)),
        }
    }
}

/// A body's phys object (its userdata, body +0xc): what the near callback
/// reads and writes (`obj`), the pose the frame's steps began from (+0x10,
/// and the axes at +0x1c), and its first geom.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PhysObj {
    pub obj: Obj,
    pub prev: (V3, M3),
    pub geom: Geom,
}

/// One of retail's worlds: the ODE world, its clock, each body's phys
/// object and the space's geoms.
#[derive(Clone, Debug)]
pub struct PhysWorld {
    pub id: WorldId,
    pub world: World,
    pub clock: Clock,
    /// By body index.
    objs: Vec<Option<PhysObj>>,
    /// The bodies' geoms in the space's order, newest first (0x5a9f00).
    space: Vec<BodyId>,
}

impl PhysWorld {
    /// The world as `Phys_Init` leaves it (the loop at 0x5a2d1f; the
    /// clock block zeroed by the memset at 0x5a2cf6).
    pub fn new(id: WorldId) -> PhysWorld {
        let mut world = World::new();
        world.params = Params::phys_init(id);
        PhysWorld {
            id,
            world,
            clock: Clock {
                last: 0,
                now: 0,
                frac: 0.0,
                msec_step: MSEC_STEP_START[id.index()],
            },
            objs: Vec::new(),
            space: Vec::new(),
        }
    }

    /// A bare body; [`Self::add_obj`] gives it its object and geom.
    pub fn create_body(&mut self) -> BodyId {
        self.world.create_body()
    }

    /// The body's phys object, its geom put at the head of the space.
    pub fn add_obj(&mut self, id: BodyId, o: PhysObj) {
        let i = id.0 as usize;
        if self.objs.len() <= i {
            self.objs.resize(i + 1, None);
        }
        self.objs[i] = Some(o);
        self.space.insert(0, id);
    }

    /// The body, its object and its geom.
    pub fn destroy_body(&mut self, id: BodyId) {
        if let Some(o) = self.objs.get_mut(id.0 as usize) {
            *o = None;
        }
        self.space.retain(|&b| b != id);
        self.world.destroy_body(id);
    }

    pub fn obj(&self, id: BodyId) -> Option<&PhysObj> {
        self.objs.get(id.0 as usize)?.as_ref()
    }

    pub fn obj_mut(&mut self, id: BodyId) -> Option<&mut PhysObj> {
        self.objs.get_mut(id.0 as usize)?.as_mut()
    }

    /// `Phys_RunToTime` (0x5a6d50): a time behind the frame's start
    /// rewinds the lerp (0x5a6cd0); one past the clock copies each body's
    /// pose to its object, runs at most two steps of `max(msecStep, owed
    /// / steps left)` ms (an unsigned divide, a signed max), each
    /// [`Self::run_frame`] then [`Self::post_step`], then
    /// [`Self::after_frame`]; last the lerp from the frame's start to the
    /// last step's end. `hook` is the world's own collision (`+0x90` of its
    /// block, ragdoll self collision on world 2), run each step before the
    /// space's.
    pub fn run_to_time(
        &mut self,
        target: i32,
        dv: &PhysDvars,
        contact: &collide::Dvars,
        clip: &dyn Clip,
        hook: &mut dyn FnMut(&mut PhysWorld),
    ) -> Result<(), Unported> {
        let c = &mut self.clock;
        if target < c.last {
            let f = if c.now > c.last {
                ((target - c.last) as f64 / (c.now - c.last) as f64) as f32
            } else {
                1.0
            };
            if c.frac > f {
                if (0.0..=1.0).contains(&f) {
                    c.frac = f;
                } else {
                    c.frac = 1.0;
                    c.last = target;
                    c.now = target;
                }
            }
        }
        if self.clock.now < target {
            self.clock.last = self.clock.now;
            for k in 0..self.world.bodies().len() {
                let id = self.world.bodies()[k];
                let b = self.world.body(id);
                let pose = (b.pos, axes_of(&b.r));
                if let Some(o) = self.obj_mut(id) {
                    o.prev = pose;
                }
            }
            let mut left = 2u32;
            loop {
                let owed = ((target.wrapping_sub(self.clock.now) as u32) / left) as i32;
                let msec = self.clock.msec_step.max(owed);
                left -= 1;
                let dt = (msec as u32 as f64 * MS_TO_S) as f32;
                self.run_frame(dt, dv, contact, clip, hook)?;
                self.clock.now = self.clock.now.wrapping_add(msec);
                self.post_step(dt, dv);
                // `left` can't run out first: the last step takes all owed
                if self.clock.now >= target || left == 0 {
                    break;
                }
            }
            self.after_frame(clip, dv.csl)?;
        }
        let c = &mut self.clock;
        c.frac = if c.now > c.last {
            ((target - c.last) as f64 / (c.now - c.last) as f64) as f32
        } else {
            1.0
        };
        Ok(())
    }

    /// `Phys_RunFrame` (0x5a6440): the dvars stored, the world's hook,
    /// each awake geom against the world newest first (0x5aa100 into the
    /// near callback 0x5a5f20), the world's step. `phys_collUseEntities`
    /// (body against body, 0x5a65b8) is off by default and not ported.
    fn run_frame(
        &mut self,
        dt: f32,
        dv: &PhysDvars,
        contact: &collide::Dvars,
        clip: &dyn Clip,
        hook: &mut dyn FnMut(&mut PhysWorld),
    ) -> Result<(), Unported> {
        self.world.params.apply_phys_frame(dv, self.id);
        hook(self);
        for g in 0..self.space.len() {
            let id = self.space[g];
            let body = self.world.body(id);
            if body.flags & DISABLED != 0 {
                continue;
            }
            let Some(&o) = self.obj(id) else { continue };
            let cap = match o.geom {
                Geom::Capsule { radius, half_len } => Capsule {
                    centre: body.pos,
                    axis: [body.r[0][0], body.r[1][0], body.r[2][0]],
                    radius,
                    half_len,
                },
                Geom::Unported(c) => return Err(Unported(c)),
            };
            let mut obj = o.obj;
            collide::near_world(
                &mut self.world,
                id,
                &mut obj,
                &cap,
                clip,
                contact,
                self.id.index(),
            );
            if let Some(o) = self.obj_mut(id) {
                o.obj = obj;
            }
        }
        self.world.quick_step(dt);
        Ok(())
    }

    /// `Phys_PostStep` (0x5a6640): awake bodies' accumulators cleared, the
    /// contacts emptied, [`drag`] on each body, the next step's length
    /// off the count it found awake ([`msec_step`]).
    fn post_step(&mut self, dt: f32, dv: &PhysDvars) {
        self.world.zero_forces();
        self.world.empty_contacts();
        let mut awake = 0;
        for k in 0..self.world.bodies().len() {
            let id = self.world.bodies()[k];
            if drag(self.id, self.world.body_mut(id), dt, dv) {
                awake += 1;
            }
        }
        self.clock.msec_step = msec_step(self.id, awake);
    }

    /// 0x5a6c10 on each awake body after a frame's steps: [`Self::sweep`],
    /// then one outside the world model's bounds is put to sleep.
    fn after_frame(&mut self, clip: &dyn Clip, csl: f32) -> Result<(), Unported> {
        let (mins, maxs) = clip.bounds();
        for k in 0..self.world.bodies().len() {
            let id = self.world.bodies()[k];
            if self.world.body(id).flags & DISABLED != 0 {
                continue;
            }
            self.sweep(id, clip, csl)?;
            let body = self.world.body_mut(id);
            let p = body.pos;
            if (0..3).any(|k| p[k] > maxs[k] || p[k] < mins[k]) {
                body.flags |= DISABLED;
            }
        }
        Ok(())
    }

    /// 0x5a6930: a box sweep from where the frame began to the body, when
    /// it moved or its object's state is 0 or 1. The box is the geom's
    /// bounds (`m` their top x): a point when `m` is under `2 phys_csl`,
    /// else a fifth of the bounds, or `max(m - 2 phys_csl, 0.001)` when
    /// `m` less that fifth is under `2 phys_csl`. The state becomes 1 when
    /// the sweep starts solid, else 2. A hit not started solid pulls the
    /// body back to where it stopped (a point's fraction less `m / 2` over
    /// the move's length first, at least 0), and a fraction under 0.001
    /// stills it.
    fn sweep(&mut self, id: BodyId, clip: &dyn Clip, csl: f32) -> Result<(), Unported> {
        let Some(&o) = self.obj(id) else {
            return Ok(());
        };
        let (lo, hi) = o.geom.bounds()?;
        let m = hi[0];
        let csl2 = (w(csl) + w(csl)) as f32;
        let thin = csl2 > m;
        let (mins, maxs) = if thin {
            ([-POINT; 3], [POINT; 3])
        } else {
            let lo5 = lo.map(|x| (w(x) * SWEEP_SHARE) as f32);
            let hi5 = hi.map(|x| (w(x) * SWEEP_SHARE) as f32);
            if w(m) - w(hi5[0]) < w(csl2) {
                let e = fmax((w(m) - w(csl2)) as f32, POINT);
                ([-e; 3], [e; 3])
            } else {
                (lo5, hi5)
            }
        };
        let prev = o.prev.0;
        let mut obj = o.obj;
        let body = self.world.body_mut(id);
        if body.pos != prev || obj.state == 0 || obj.state == 1 {
            let tr = clip.sweep(
                Vec3::from(prev),
                Vec3::from(body.pos),
                Vec3::from(mins),
                Vec3::from(maxs),
            );
            obj.state = if tr.startsolid { 1 } else { 2 };
            if tr.fraction < 1.0 && !tr.startsolid {
                let d: V3 = std::array::from_fn(|k| (w(body.pos[k]) - w(prev[k])) as f32);
                let mut f = tr.fraction;
                if thin {
                    let l2 = ((w(d[0]) * w(d[0]) + w(d[1]) * w(d[1])) + w(d[2]) * w(d[2])) as f32;
                    f = fmax(0.0, (w(f) - 0.5 * w(m) * w(rsqrt(l2))) as f32);
                }
                body.pos = std::array::from_fn(|k| (w(prev[k]) + w(d[k]) * w(f)) as f32);
                if tr.fraction < POINT {
                    body.lvel = [0.0; 3];
                    body.avel = [0.0; 3];
                }
            }
        }
        if let Some(o) = self.obj_mut(id) {
            o.obj = obj;
        }
        Ok(())
    }
}

/// `Phys_DoBodyOncePerFrame` (0x5a62f0) after a step of `dt` seconds:
/// false for a sleeping body; else worlds 0 and 1 scale its velocities by
/// `max(1 - phys_dragLinear dt, 0)` and `max(1 - phys_dragAngular dt, 0)`
/// (0x5a6310..0x5a63cf), world 2 adds `-0.01` of its angular velocity
/// (0x5a63d0..0x5a6429).
pub fn drag(world: WorldId, b: &mut Body, dt: f32, dv: &PhysDvars) -> bool {
    if b.flags & DISABLED != 0 {
        return false;
    }
    if world == WorldId::Ragdoll {
        b.avel = b.avel.map(|x| (w(x) + w((w(x) * AVEL_DAMP) as f32)) as f32);
    } else {
        let k = |d: f32| fmax((1.0 - w(d) * w(dt)) as f32, 0.0);
        let (kl, ka) = (k(dv.drag_linear), k(dv.drag_angular));
        b.lvel = b.lvel.map(|x| (w(kl) * w(x)) as f32);
        b.avel = b.avel.map(|x| (w(ka) * w(x)) as f32);
    }
    true
}

/// The next step's length off the bodies awake (0x5a6738..0x5a67bb):
/// `min + trunc((max - min) t)`, `t = clamp((awake - 32) / 18, 0, 1)`
/// (the double 18 at 0x70b330, 1.0 at 0x70ad08), the world's floor and
/// ceiling [`MSEC_STEP_MIN`], [`MSEC_STEP_MAX`].
pub fn msec_step(world: WorldId, awake: i32) -> i32 {
    let x = ((awake - 32) as f64 / 18.0) as f32;
    let t = if x <= 0.0 {
        0.0
    } else if ((w(x) - 1.0) as f32) < 0.0 {
        x
    } else {
        1.0
    };
    let (lo, hi) = (MSEC_STEP_MIN[world.index()], MSEC_STEP_MAX[world.index()]);
    lo + ((hi - lo) as f64 * w(t)) as i32
}

/// `Phys_ObjAddForce` (0x5a5900): `force` times `1000 / msec_step` (the
/// named world's, the double at 0x70af28), stored, at world point
/// `point` ([`Body::add_force_at_pos`]), and the body woken
/// (0x5a5968..0x5a5981).
pub fn obj_add_force(body: &mut Body, msec_step: i32, point: V3, force: V3) {
    let per_s = (1000.0 / msec_step as f64) as f32;
    let f = force.map(|x| (w(x) * w(per_s)) as f32);
    body.add_force_at_pos(f, point);
    body.enable();
}

/// `Phys_ObjBulletImpact` (0x5a5a20) on `body` at `contact` along `dir`
/// at `speed`, scaled by `scale`, for the world whose `msec_step` is
/// given. 0x5a5990: `dir` gets `phys_bulletUpBias` on z and is
/// normalized, the contact moves out to `p + (p - x) phys_bulletSpinScale`
/// (`x` the body's position). With `r` that point less `x` and `n`, `L`
/// the direction and length of `r x dir`, the closing speed is `speed -
/// (v . dir + (w . n) L)`; at or below 0 nothing happens. Else `J =
/// closing m scale / (m + 0.5 + m L^2 / 2 (u . I u))`, `u = R n` (the
/// rotation, not its transpose; the `L^2` term only when `L` is not 0),
/// and `J dir` goes to [`obj_add_force`] at the moved point.
pub fn obj_bullet_impact(
    body: &mut Body,
    msec_step: i32,
    contact: V3,
    dir: V3,
    speed: f32,
    scale: f32,
    dv: &PhysDvars,
) {
    let x = body.pos;
    let mut a = dir;
    a[2] = (w(dv.bullet_up_bias) + w(a[2])) as f32;
    let a = normalize(a).0;
    let s = dv.bullet_spin_scale;
    let out: V3 = std::array::from_fn(|k| (w((w(contact[k]) - w(x[k])) as f32) * w(s)) as f32);
    let p: V3 = std::array::from_fn(|k| (w(contact[k]) + w(out[k])) as f32);
    let v = body.lvel;
    let vd = ((w(v[1]) * w(a[1]) + w(v[0]) * w(a[0])) + w(v[2]) * w(a[2])) as f32;
    let cl0 = (w(speed) - w(vd)) as f32;
    let r: V3 = std::array::from_fn(|k| (w(p[k]) - w(x[k])) as f32);
    let n = [
        (w(r[1]) * w(a[2]) - w(r[2]) * w(a[1])) as f32,
        (w(r[2]) * w(a[0]) - w(r[0]) * w(a[2])) as f32,
        (w(r[0]) * w(a[1]) - w(r[1]) * w(a[0])) as f32,
    ];
    let (n, l) = normalize(n);
    let rot = body.r;
    let u: V3 = std::array::from_fn(|i| {
        ((w(rot[i][1]) * w(n[1]) + w(rot[i][0]) * w(n[0])) + w(rot[i][2]) * w(n[2])) as f32
    });
    let inertia = body.mass.i;
    let iu: V3 = std::array::from_fn(|i| {
        ((w(inertia[i][1]) * w(u[1]) + w(inertia[i][0]) * w(u[0])) + w(inertia[i][2]) * w(u[2]))
            as f32
    });
    let wv = body.avel;
    let wn = ((w(n[1]) * w(wv[1]) + w(n[0]) * w(wv[0])) + w(n[2]) * w(wv[2])) as f32;
    let closing = (w(cl0) - w(wn) * w(l)) as f32;
    if closing <= 0.0 {
        return;
    }
    let m = body.mass.mass;
    let mut den = (w(m) + 0.5) as f32;
    if l != 0.0 {
        let q = ((w(u[0]) * w(iu[0]) + w(u[1]) * w(iu[1])) + w(u[2]) * w(iu[2])) as f32;
        den = (w(den) + (w(l) * w(l) * w(m) * 0.5) / w(q)) as f32;
    }
    let j = ((w(closing) * (w(m) * w(scale))) / w(den)) as f32;
    let force = a.map(|x| (w(j) * w(x)) as f32);
    obj_add_force(body, msec_step, p, force);
}

/// 0x4093c0: `b` when `a - b` stored is under 0, else `a`.
pub(crate) fn fmax(a: f32, b: f32) -> f32 {
    if ((w(a) - w(b)) as f32) < 0.0 { b } else { a }
}

/// 0x4076d0: `v` over its stored length (by 1 when that is not over 0),
/// and the length.
pub(crate) fn normalize(v: V3) -> (V3, f32) {
    let l2 = ((w(v[0]) * w(v[0]) + w(v[1]) * w(v[1])) + w(v[2]) * w(v[2])) as f32;
    let l = w(l2).sqrt() as f32;
    let d = if -l >= 0.0 { 1.0 } else { l };
    let inv = (1.0 / w(d)) as f32;
    (v.map(|x| (w(x) * w(inv)) as f32), l)
}

/// 0x403a90: `0x5f3759df` less half the bits, one Newton step.
pub(crate) fn rsqrt(x: f32) -> f32 {
    let y = f32::from_bits(0x5f37_59df - (x.to_bits() >> 1));
    (w(y) * (1.5 - w((w(x) * 0.5) as f32) * w(y) * w(y))) as f32
}

/// The rotation's columns, the axes the phys object keeps.
pub(crate) fn axes_of(r: &M3) -> M3 {
    std::array::from_fn(|i| std::array::from_fn(|j| r[j][i]))
}
