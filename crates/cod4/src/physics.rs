//! A `physicsLaunch`ed part on the client: the rigid body iw3mp.exe's
//! `CG_CreatePhysicsObject` (0x434c70) hands ODE's fx world, stood in by a
//! small impulse solver that sweeps the body's corners through the clip
//! map. Retail's model and where this one differs: cod4-gsc.md 3.6.

use crate::assets::Assets;
use crate::assets::model::{PhysGeomList, XModel};
use crate::ragdoll::{Sweep, Trace};
use glam::{Mat3, Quat, Vec3};
use std::collections::HashMap;
use std::sync::Arc;
use vcod_common::turretpose::{angles_quat, quat_angles};

/// `PhysGeomInfo.type`s the solver shapes (cod4-gsc.md 3.6).
const GEOM_BOX: i32 = 1;
const GEOM_CYLINDER: i32 = 4;
/// A cylinder's points round each cap.
const RING: usize = 8;
/// ODE's `bounce_vel` on every contact (0x5a609e, 0.1): slower approaches
/// do not bounce.
const BOUNCE_VEL: f32 = 0.1;
/// Contact solver passes per step (vcod's).
const ITERATIONS: usize = 10;

/// What a part's `PhysPreset` gives the body: `mass` (+8), `bounce`
/// (+12), `friction` (+16) and `bulletForceScale` (+20).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Preset {
    pub mass: f32,
    pub bounce: f32,
    pub friction: f32,
    pub bullet_force_scale: f32,
}

impl Preset {
    /// From the asset's 0x2C bytes.
    pub fn from_raw(raw: &[u8]) -> Option<Preset> {
        let f = |o: usize| Some(f32::from_le_bytes(raw.get(o..o + 4)?.try_into().ok()?));
        Some(Preset {
            mass: f(8)?,
            bounce: f(12)?,
            friction: f(16)?,
            bullet_force_scale: f(20)?,
        })
    }
}

/// The `phys_*` dvars the fx world reads, at their 1.7 defaults
/// (cod4-gsc.md 3.6).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Dvars {
    /// `phys_gravity` (-800).
    pub gravity: f32,
    /// `phys_bulletUpBias` (0.5) and `phys_bulletSpinScale` (3).
    pub bullet_up_bias: f32,
    pub bullet_spin_scale: f32,
    /// `phys_frictionScale` (1).
    pub friction_scale: f32,
    /// `phys_dragLinear` (0.03) and `phys_dragAngular` (0.5), per second.
    pub drag_linear: f32,
    pub drag_angular: f32,
    /// `phys_autoDisableLinear` (20), `phys_autoDisableAngular` (1) and
    /// `phys_autoDisableTime` (0.9 s).
    pub sleep_linear: f32,
    pub sleep_angular: f32,
    pub sleep_time: f32,
    /// `phys_contact_erp` (0.8) and `phys_mcv` (20): how fast a corner in
    /// the world is pushed out.
    pub erp: f32,
    pub max_correcting: f32,
    /// `msecStep`'s floor for the fx world (17 ms).
    pub step_ms: i32,
}

impl Default for Dvars {
    fn default() -> Self {
        Dvars {
            gravity: -800.0,
            bullet_up_bias: 0.5,
            bullet_spin_scale: 3.0,
            friction_scale: 1.0,
            drag_linear: 0.03,
            drag_angular: 0.5,
            sleep_linear: 20.0,
            sleep_angular: 1.0,
            sleep_time: 0.9,
            erp: 0.8,
            max_correcting: 20.0,
            step_ms: 17,
        }
    }
}

/// A part's collision and mass in its model's frame: the points that meet
/// the world, the centre of mass and the inertia per unit of mass about it.
#[derive(Clone, Debug, PartialEq)]
pub struct Shape {
    pub points: Vec<Vec3>,
    pub com: Vec3,
    pub inertia: Mat3,
}

impl Shape {
    /// `Phys_ObjSetCollisionFromXModel` (0x5a5710): the model's `physGeoms`
    /// when it has boxes or cylinders, else a box on its bounds.
    pub fn of(model: &XModel) -> Shape {
        model
            .phys_geoms
            .as_ref()
            .and_then(Shape::of_geoms)
            .unwrap_or_else(|| Shape::of_bounds(model.mins.into(), model.maxs.into()))
    }

    /// A box on `mins`..`maxs`, its centre the centre of mass; any flat
    /// axis takes retail's 100-unit cube (0x5a587a).
    pub fn of_bounds(mins: Vec3, maxs: Vec3) -> Shape {
        let (mins, maxs) = if (0..3).any(|k| mins[k] == maxs[k]) {
            (Vec3::splat(-50.0), Vec3::splat(50.0))
        } else {
            (mins, maxs)
        };
        let c = (mins + maxs) * 0.5;
        let h = (maxs - mins) * 0.5;
        Shape {
            points: corners(c, Mat3::IDENTITY, h),
            com: c,
            inertia: box_inertia(h),
        }
    }

    /// The boxes' corners and the cylinders' rims, with the list's
    /// `PhysMass`; `None` when the list has no box or cylinder.
    fn of_geoms(list: &PhysGeomList) -> Option<Shape> {
        let mut points = Vec::new();
        for g in list.geoms.iter().filter(|g| !g.brush) {
            let axis = Mat3::from_cols(
                g.orientation[0].into(),
                g.orientation[1].into(),
                g.orientation[2].into(),
            );
            let at = Vec3::from(g.offset);
            let h = Vec3::from(g.half_lengths);
            match g.kind {
                GEOM_BOX => points.extend(corners(at, axis, h)),
                // The axis is the first row, `halfLengths` the half height
                // and the radius.
                GEOM_CYLINDER => {
                    for end in [-h.x, h.x] {
                        for i in 0..RING {
                            let a = i as f32 * std::f32::consts::TAU / RING as f32;
                            let local = Vec3::new(end, h.y * a.cos(), h.y * a.sin());
                            points.push(at + axis * local);
                        }
                    }
                }
                _ => {}
            }
        }
        if points.is_empty() {
            return None;
        }
        let m = list.moments.map(|m| if m > 0.0 { m } else { 1.0 });
        let p = list.products;
        Some(Shape {
            points,
            com: list.center_of_mass.into(),
            inertia: Mat3::from_cols(
                Vec3::new(m[0], p[0], p[1]),
                Vec3::new(p[0], m[1], p[2]),
                Vec3::new(p[1], p[2], m[2]),
            ),
        })
    }
}

/// What `CG_CreatePhysicsObject` reads off a part's model: its shape and
/// its preset.
#[derive(Clone, Debug, PartialEq)]
pub struct PartModel {
    pub shape: Shape,
    pub preset: Preset,
}

/// Every xmodel of `zones` naming a preset one of them holds, by lower-case
/// name, the first zone naming either winning. A zone that only refers to
/// a preset lists it as `,name` with no body.
pub fn part_models(zones: &[&Assets]) -> HashMap<String, Arc<PartModel>> {
    let mut presets: HashMap<String, Preset> = HashMap::new();
    for z in zones {
        for p in z.phys_presets.iter().filter(|p| !p.name.starts_with(',')) {
            if let Some(v) = Preset::from_raw(&p.raw) {
                presets.entry(p.name.to_ascii_lowercase()).or_insert(v);
            }
        }
    }
    let mut out = HashMap::new();
    for z in zones {
        for m in &z.xmodels {
            let Some(p) = m.phys_preset else { continue };
            let name = z.phys_presets[p.index()].name.trim_start_matches(',');
            let Some(preset) = presets.get(&name.to_ascii_lowercase()) else {
                continue;
            };
            out.entry(m.name.to_ascii_lowercase()).or_insert_with(|| {
                Arc::new(PartModel {
                    shape: Shape::of(m),
                    preset: *preset,
                })
            });
        }
    }
    out
}

/// A box's eight corners: centre `at`, axes the columns of `axis`.
fn corners(at: Vec3, axis: Mat3, h: Vec3) -> Vec<Vec3> {
    (0..8)
        .map(|i| {
            let s = |b: usize| if i >> b & 1 == 0 { -1.0 } else { 1.0 };
            at + axis * Vec3::new(s(0) * h.x, s(1) * h.y, s(2) * h.z)
        })
        .collect()
}

/// A solid box's inertia per unit of mass, `h` its half sizes.
fn box_inertia(h: Vec3) -> Mat3 {
    let s = h * h / 3.0;
    Mat3::from_diagonal(Vec3::new(s.y + s.z, s.x + s.z, s.x + s.y))
}

/// One corner's contact with the world in a step.
struct Contact {
    /// The corner off the centre of mass, world axes.
    r: Vec3,
    n: Vec3,
    /// What the contact's normal speed is pushed to at least.
    target: f32,
    k_n: f32,
    t: [Vec3; 2],
    k_t: [f32; 2],
    /// The accumulated normal and friction impulses.
    lambda: f32,
    lambda_t: [f32; 2],
}

/// One launched part.
#[derive(Clone, Debug)]
pub struct Body {
    /// Centre of mass and rotation; linear and angular velocity, world.
    pub pos: Vec3,
    pub rot: Quat,
    pub vel: Vec3,
    pub avel: Vec3,
    mass: f32,
    /// Inertia and its inverse about the centre of mass, body axes.
    inertia: Mat3,
    inv_inertia: Mat3,
    /// The centre of mass in the model's frame, and the points off it.
    com: Vec3,
    points: Vec<Vec3>,
    bounce: f32,
    friction: f32,
    /// Seconds below both sleep speeds.
    idle: f32,
    pub asleep: bool,
    /// `Phys_RunFrame`'s clocks: the time simulated to, and the frame's
    /// start, whose state ([`Self::from`]) the drawn pose lerps from.
    sim_ms: i32,
    from_ms: i32,
    from: (Vec3, Quat),
    /// Steps taken.
    pub steps: u32,
}

impl Body {
    /// `Phys_ObjCreate` (0x5a4fb0) at the entity's `origin` and `angles`
    /// (its `trBase`s), at rest, at time `now`.
    pub fn new(origin: Vec3, angles: Vec3, shape: &Shape, preset: &Preset, now: i32) -> Body {
        let rot = angles_quat(angles.to_array());
        let mass = preset.mass.max(0.001);
        let inertia = shape.inertia * mass;
        let pos = origin + rot * shape.com;
        Body {
            pos,
            rot,
            vel: Vec3::ZERO,
            avel: Vec3::ZERO,
            mass,
            inertia,
            inv_inertia: inertia.inverse(),
            com: shape.com,
            points: shape.points.iter().map(|p| *p - shape.com).collect(),
            bounce: preset.bounce.min(1.0),
            friction: preset.friction,
            idle: 0.0,
            asleep: false,
            sim_ms: now,
            from_ms: now,
            from: (pos, rot),
            steps: 0,
        }
    }

    /// `Phys_ObjBulletImpact` (0x5a5a20) as `CG_CreatePhysicsObject` calls
    /// it with the launch: `force`'s direction tipped up by
    /// `phys_bulletUpBias`, its length the speed, at `contact` moved out
    /// from the centre `phys_bulletSpinScale` times as far again, scaled by
    /// the preset's `bulletForceScale`.
    pub fn launch(&mut self, contact: Vec3, force: Vec3, scale: f32, d: &Dvars) {
        let speed = force.length();
        let dir = (force.normalize_or_zero() + Vec3::Z * d.bullet_up_bias).normalize_or_zero();
        let at = contact + (contact - self.pos) * d.bullet_spin_scale;
        let r = at - self.pos;
        let c = r.cross(dir);
        let arm = c.length();
        let c = c.normalize_or_zero();
        let closing = speed - (self.vel.dot(dir) + self.avel.dot(c) * arm);
        // A NaN closes nothing either.
        if closing.is_nan() || closing <= 0.0 {
            return;
        }
        // 0x5a5bc3: the axis is turned by the body's rotation, not into
        // its frame, before the inertia reads it.
        let u = self.rot * c;
        let mut denom = self.mass + 0.5;
        if arm != 0.0 {
            denom += 0.5 * self.mass * arm * arm / u.dot(self.inertia * u);
        }
        let j = closing * self.mass * scale / denom;
        self.impulse(r, dir * j);
        self.wake();
    }

    fn wake(&mut self) {
        self.asleep = false;
        self.idle = 0.0;
    }

    /// The world inverse inertia.
    fn inv_inertia_world(&self) -> Mat3 {
        let r = Mat3::from_quat(self.rot);
        r * self.inv_inertia * r.transpose()
    }

    fn impulse(&mut self, r: Vec3, p: Vec3) {
        self.vel += p / self.mass;
        self.avel += self.inv_inertia_world() * r.cross(p);
    }

    /// `Phys_RunFrame`'s clock (0x5a6e02): up to `time`, in at most two
    /// steps of at least `step_ms`, each at least half what is left, so
    /// the body runs up to a step ahead and [`Self::pose`] lerps.
    pub fn advance(&mut self, time: i32, trace: &mut Trace, d: &Dvars) {
        if time.wrapping_sub(self.sim_ms) <= 0 {
            return;
        }
        self.from_ms = self.sim_ms;
        self.from = (self.pos, self.rot);
        let mut left = 2;
        while time.wrapping_sub(self.sim_ms) > 0 {
            let step = (time.wrapping_sub(self.sim_ms) / left).max(d.step_ms);
            left = (left - 1).max(1);
            self.step(step as f32 / 1000.0, trace, d);
            self.sim_ms = self.sim_ms.wrapping_add(step);
        }
    }

    /// The drawn origin and angles at `time`: the frame's start state
    /// lerped toward the last step's.
    pub fn pose(&self, time: i32) -> (Vec3, Vec3) {
        let span = self.sim_ms.wrapping_sub(self.from_ms);
        let f = if span > 0 {
            (time.wrapping_sub(self.from_ms) as f32 / span as f32).clamp(0.0, 1.0)
        } else {
            1.0
        };
        let pos = self.from.0.lerp(self.pos, f);
        let rot = self.from.1.slerp(self.rot, f).normalize();
        (pos - rot * self.com, Vec3::from(quat_angles(rot)))
    }

    /// The model's origin now.
    pub fn origin(&self) -> Vec3 {
        self.pos - self.rot * self.com
    }

    /// Every collision point in world space.
    pub fn world_points(&self) -> impl Iterator<Item = Vec3> + '_ {
        self.points.iter().map(|p| self.pos + self.rot * *p)
    }

    /// One step of `dt` seconds: gravity, the corners' contacts solved as
    /// impulses with Coulomb friction and the preset's bounce, the move,
    /// the drag, the sleep test.
    pub fn step(&mut self, dt: f32, trace: &mut Trace, d: &Dvars) {
        if self.asleep {
            return;
        }
        self.steps += 1;
        let (v0, w0) = (self.vel, self.avel);
        self.vel.z += d.gravity * dt;
        let mut contacts: Vec<Contact> = Vec::new();
        let mut touched = vec![false; self.points.len()];
        for pass in 0..2 {
            let found = self.gather(dt, trace, d, v0, w0, &mut touched);
            if found.is_empty() && pass > 0 {
                break;
            }
            contacts.extend(found);
            self.solve(&mut contacts, d);
        }
        let old = self.pos;
        self.pos += self.vel * dt;
        let w = self.avel;
        let dq = Quat::from_xyzw(w.x, w.y, w.z, 0.0) * self.rot;
        self.rot = (self.rot + dq * (0.5 * dt)).normalize();
        // The centre itself never crosses a surface in one step.
        let s = trace(old, self.pos, 0.0);
        if !s.startsolid && s.fraction < 1.0 {
            self.pos = old + (self.pos - old) * s.fraction;
            let into = self.vel.dot(s.normal);
            if into < 0.0 {
                self.vel -= s.normal * into;
            }
        }
        self.vel *= (1.0 - d.drag_linear * dt).max(0.0);
        self.avel *= (1.0 - d.drag_angular * dt).max(0.0);
        if self.vel.length_squared() < d.sleep_linear * d.sleep_linear
            && self.avel.length_squared() < d.sleep_angular * d.sleep_angular
        {
            self.idle += dt;
            if self.idle > d.sleep_time {
                self.asleep = true;
                self.vel = Vec3::ZERO;
                self.avel = Vec3::ZERO;
            }
        } else {
            self.idle = 0.0;
        }
    }

    /// The contacts of the points not yet touching: each swept by its
    /// velocity over the step; one starting in the world is traced out
    /// from the centre.
    fn gather(
        &self,
        dt: f32,
        trace: &mut Trace,
        d: &Dvars,
        v0: Vec3,
        w0: Vec3,
        touched: &mut [bool],
    ) -> Vec<Contact> {
        let inv_iw = self.inv_inertia_world();
        let mut out = Vec::new();
        for (i, l) in self.points.iter().enumerate() {
            if touched[i] {
                continue;
            }
            let r = self.rot * *l;
            let p = self.pos + r;
            let travel = (self.vel + self.avel.cross(r)) * dt;
            let s = trace(p, p + travel, 0.0);
            let (n, sep) = if s.startsolid {
                let out = trace(self.pos, p, 0.0);
                if out.startsolid || out.fraction >= 1.0 {
                    continue;
                }
                let hit = self.pos + (p - self.pos) * out.fraction;
                (out.normal, (p - hit).dot(out.normal))
            } else if s.fraction < 1.0 {
                let hit = p + travel * s.fraction;
                (s.normal, (p - hit).dot(s.normal).max(0.0))
            } else {
                continue;
            };
            touched[i] = true;
            let approach = (v0 + w0.cross(r)).dot(n);
            let target = if sep < 0.0 {
                (d.erp * -sep / dt).min(d.max_correcting)
            } else if approach < -BOUNCE_VEL && -approach * dt >= sep {
                -self.bounce * approach
            } else {
                -sep / dt
            };
            let k = |dir: Vec3| {
                let rn = r.cross(dir);
                1.0 / self.mass + rn.dot(inv_iw * rn)
            };
            let t0 = n.any_orthonormal_vector();
            let t = [t0, n.cross(t0)];
            out.push(Contact {
                r,
                n,
                target,
                k_n: k(n),
                t,
                k_t: [k(t[0]), k(t[1])],
                lambda: 0.0,
                lambda_t: [0.0; 2],
            });
        }
        out
    }

    /// Sequential impulses: each contact's normal speed held at or over its
    /// target, its friction within `friction * phys_frictionScale` of the
    /// normal impulse.
    fn solve(&mut self, contacts: &mut [Contact], d: &Dvars) {
        let mu = self.friction * d.friction_scale;
        for _ in 0..ITERATIONS {
            for c in contacts.iter_mut() {
                let r = c.r;
                let vrel = |b: &Body| b.vel + b.avel.cross(r);
                let vn = vrel(self).dot(c.n);
                let next = (c.lambda + (c.target - vn) / c.k_n).max(0.0);
                let dl = next - c.lambda;
                c.lambda = next;
                self.impulse(r, c.n * dl);
                for k in 0..2 {
                    let vt = vrel(self).dot(c.t[k]);
                    let cap = mu * c.lambda;
                    let next = (c.lambda_t[k] - vt / c.k_t[k]).clamp(-cap, cap);
                    let dl = next - c.lambda_t[k];
                    c.lambda_t[k] = next;
                    self.impulse(r, c.t[k] * dl);
                }
            }
        }
    }
}

/// A point trace through `clip` under the world mask the fx world's
/// bodies meet (`0x2806c91`, iw3mp.exe 0x5a6acf).
pub fn clip_trace(clip: &crate::collision::ClipMap) -> impl FnMut(Vec3, Vec3, f32) -> Sweep + '_ {
    move |a: Vec3, b: Vec3, r: f32| {
        let e = Vec3::splat(r);
        let t = clip.trace(a, b, -e, e, crate::ragdoll::CLIP_MASK);
        Sweep {
            fraction: t.fraction,
            normal: t.normal,
            startsolid: t.startsolid,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The CoD trace's 0.125 skin off a surface.
    const SKIN: f32 = 0.125;

    /// A world of half-spaces `n . p <= dist` that are solid, traced as
    /// points with the kernels' skin.
    fn planes(solid: &[(Vec3, f32)]) -> impl FnMut(Vec3, Vec3, f32) -> Sweep + '_ {
        move |a: Vec3, b: Vec3, _r: f32| {
            let mut best = Sweep {
                fraction: 1.0,
                normal: Vec3::ZERO,
                startsolid: false,
            };
            for &(n, dist) in solid {
                let da = n.dot(a) - dist;
                let db = n.dot(b) - dist;
                if da <= 0.0 {
                    return Sweep {
                        fraction: 0.0,
                        normal: n,
                        startsolid: true,
                    };
                }
                if db < SKIN && db < da {
                    let f = ((da - SKIN) / (da - db)).clamp(0.0, 1.0);
                    if f < best.fraction {
                        best = Sweep {
                            fraction: f,
                            normal: n,
                            startsolid: false,
                        };
                    }
                }
            }
            best
        }
    }

    fn door() -> (Shape, Preset) {
        // vehicle_80s_sedan1's door: bounds off the zone, `car_door`.
        let shape = Shape::of_bounds(
            Vec3::new(-41.97, -10.69, -16.0),
            Vec3::new(0.02, 0.01, 28.75),
        );
        let preset = Preset {
            mass: 25.0,
            bounce: 0.1,
            friction: 0.5,
            bullet_force_scale: 0.6,
        };
        (shape, preset)
    }

    fn run(b: &mut Body, ms: i32, trace: &mut Trace, d: &Dvars) {
        let mut t = b.sim_ms;
        while t < ms {
            t += 16;
            b.advance(t, trace, d);
        }
    }

    #[test]
    fn a_free_body_falls_at_phys_gravity() {
        let (shape, preset) = door();
        let d = Dvars {
            drag_linear: 0.0,
            ..Dvars::default()
        };
        let mut b = Body::new(Vec3::Z * 10000.0, Vec3::ZERO, &shape, &preset, 0);
        let mut none = |_a: Vec3, _b: Vec3, _r: f32| Sweep {
            fraction: 1.0,
            normal: Vec3::ZERO,
            startsolid: false,
        };
        let z0 = b.origin().z;
        // 17 ms steps for a second.
        for _ in 0..60 {
            b.step(0.017, &mut none, &d);
        }
        let t = 60.0 * 0.017;
        assert!((b.vel.z + 800.0 * t).abs() < 1e-2, "{}", b.vel.z);
        // Semi-implicit Euler falls a step's worth further than the curve.
        let fell = z0 - b.origin().z;
        assert!((fell - 400.0 * t * t).abs() < 800.0 * t * 0.017, "{fell}");
    }

    #[test]
    fn the_launch_throws_by_the_impulse_formula() {
        let shape = Shape::of_bounds(Vec3::splat(-5.0), Vec3::splat(5.0));
        let preset = Preset {
            mass: 15.0,
            bounce: 0.1,
            friction: 0.5,
            bullet_force_scale: 0.6,
        };
        let mut b = Body::new(Vec3::ZERO, Vec3::ZERO, &shape, &preset, 0);
        // At the centre no arm: J = speed * m * scale / (m + 0.5), along
        // the force tipped up by 0.5.
        b.launch(Vec3::ZERO, Vec3::X * 1000.0, 0.6, &Dvars::default());
        let dir = Vec3::new(1.0, 0.0, 0.5).normalize();
        let want = dir * (1000.0 * 0.6 / 15.5);
        assert!(b.vel.distance(want) < 1e-3, "{} {want}", b.vel);
        assert_eq!(b.avel, Vec3::ZERO);
        // Off the centre it spins and flies slower.
        let mut s = Body::new(Vec3::ZERO, Vec3::ZERO, &shape, &preset, 0);
        s.launch(Vec3::Y * 4.0, Vec3::X * 1000.0, 0.6, &Dvars::default());
        assert!(s.vel.length() < b.vel.length());
        assert!(s.avel.z < -1.0, "{}", s.avel);
        // A body already flying faster along the push takes nothing.
        let mut f = b.clone();
        f.vel = dir * 2000.0;
        f.launch(Vec3::ZERO, Vec3::X * 1000.0, 0.6, &Dvars::default());
        assert_eq!(f.vel, dir * 2000.0);
    }

    #[test]
    fn a_dropped_box_bounces_by_its_preset() {
        let shape = Shape::of_bounds(Vec3::splat(-8.0), Vec3::splat(8.0));
        let preset = Preset {
            mass: 18.0,
            bounce: 0.8,
            friction: 0.8,
            bullet_force_scale: 0.6,
        };
        let floor = [(Vec3::Z, 0.0)];
        let mut trace = planes(&floor);
        let d = Dvars::default();
        let mut b = Body::new(Vec3::Z * 200.0, Vec3::ZERO, &shape, &preset, 0);
        let mut peak_after = 0.0f32;
        let mut down = false;
        for _ in 0..120 {
            b.step(0.017, &mut trace, &d);
            if b.vel.z > 0.0 {
                down = true;
            }
            if down {
                peak_after = peak_after.max(b.origin().z);
            }
        }
        // Dropped from 192 over the floor it comes back up most of the way
        // (0.8 bounce: 0.64 of the height, less a step's loss).
        assert!(down);
        assert!(peak_after > 8.0 + 192.0 * 0.5, "{peak_after}");
        assert!(peak_after < 8.0 + 192.0 * 0.7, "{peak_after}");
    }

    #[test]
    fn a_launched_door_lands_slides_and_sleeps_on_the_floor() {
        let (shape, preset) = door();
        let floor = [(Vec3::Z, 0.0), (-Vec3::X, -600.0)];
        let mut trace = planes(&floor);
        let d = Dvars::default();
        let mut b = Body::new(
            Vec3::Z * 60.0,
            Vec3::new(0.0, 30.0, 0.0),
            &shape,
            &preset,
            0,
        );
        // `_destructible.gsc`'s blast: 4500 x 1.0 off the explosion, at the
        // door's own origin (its hinge), which the spin scale throws out to
        // four times its arm: the door spins more than it flies.
        b.launch(b.origin(), Vec3::new(1.0, 0.3, 0.2) * 4500.0, 0.6, &d);
        assert!((30.0..200.0).contains(&b.vel.length()), "{}", b.vel);
        assert!(b.avel.length() > 10.0, "{}", b.avel);
        run(&mut b, 10_000, &mut trace, &d);
        assert!(b.asleep, "{b:?}");
        for p in b.world_points() {
            assert!(p.z > -0.01 && p.x < 600.01, "{p}");
        }
        // Resting: its lowest points on the floor.
        let low = b.world_points().map(|p| p.z).fold(f32::MAX, f32::min);
        assert!(low < 1.0, "{low}");
        let (o, _) = b.pose(b.sim_ms);
        assert!(o.distance(b.origin()) < 1e-3);
    }

    #[test]
    fn a_fast_part_into_a_corner_stays_out_of_the_world() {
        let (shape, preset) = door();
        // A floor, a wall 100 units ahead and one 60 to the side.
        let solid = [(Vec3::Z, 0.0), (-Vec3::X, -100.0), (Vec3::Y, -60.0)];
        let mut trace = planes(&solid);
        let d = Dvars::default();
        let mut b = Body::new(
            Vec3::Z * 50.0,
            Vec3::new(0.0, 0.0, 20.0),
            &shape,
            &preset,
            0,
        );
        b.vel = Vec3::new(900.0, -400.0, 300.0);
        b.avel = Vec3::new(4.0, -9.0, 12.0);
        let mut t = 0;
        while t < 10_000 && !b.asleep {
            t += 16;
            b.advance(t, &mut trace, &d);
            for p in b.world_points() {
                assert!(p.z > -0.01 && p.x < 100.01 && p.y > -60.01, "{p} at {t}");
            }
        }
        assert!(b.asleep, "{b:?}");
    }

    #[test]
    fn a_tilted_box_settles_flat() {
        let shape = Shape::of_bounds(Vec3::new(-20.0, -10.0, -3.0), Vec3::new(20.0, 10.0, 3.0));
        let preset = Preset {
            mass: 15.0,
            bounce: 0.1,
            friction: 0.5,
            bullet_force_scale: 0.6,
        };
        let floor = [(Vec3::Z, 0.0)];
        let mut trace = planes(&floor);
        let d = Dvars::default();
        let mut b = Body::new(
            Vec3::Z * 40.0,
            Vec3::new(35.0, 20.0, 50.0),
            &shape,
            &preset,
            0,
        );
        run(&mut b, 8000, &mut trace, &d);
        assert!(b.asleep);
        // On a face: the box's thin axis upright.
        let up = b.rot * Vec3::Z;
        assert!(up.z.abs() > 0.99, "{up}");
        let low = b.world_points().map(|p| p.z).fold(f32::MAX, f32::min);
        assert!((0.0..1.0).contains(&low), "{low}");
    }

    #[test]
    fn the_frame_clock_takes_at_most_two_steps_and_lerps() {
        let (shape, preset) = door();
        let mut none = |_a: Vec3, _b: Vec3, _r: f32| Sweep {
            fraction: 1.0,
            normal: Vec3::ZERO,
            startsolid: false,
        };
        let d = Dvars::default();
        let mut b = Body::new(Vec3::Z * 1000.0, Vec3::ZERO, &shape, &preset, 100);
        b.advance(105, &mut none, &d);
        assert_eq!((b.steps, b.sim_ms), (1, 117));
        // 12 ms short of the step: the pose sits 5/17 of the way.
        let (o, _) = b.pose(105);
        let f = (o.z - b.from.0.z + b.com.z) / (b.pos.z - b.from.0.z);
        assert!((f - 5.0 / 17.0).abs() < 1e-3, "{f}");
        b.advance(110, &mut none, &d);
        assert_eq!(b.steps, 1);
        // A 300 ms frame: two steps of 150.
        b.advance(417, &mut none, &d);
        assert_eq!((b.steps, b.sim_ms), (3, 417));
    }

    #[test]
    fn a_models_geoms_give_its_points_and_mass() {
        let list = PhysGeomList {
            geoms: vec![crate::assets::model::PhysGeom {
                brush: false,
                kind: GEOM_BOX,
                orientation: [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
                offset: [0.0, -1.0, 12.75],
                half_lengths: [18.0, 18.5, 11.75],
            }],
            center_of_mass: [0.0, -1.0, 12.75],
            moments: [159.9, 153.9, 221.9],
            products: [0.0; 3],
        };
        let s = Shape::of_geoms(&list).unwrap();
        assert_eq!(s.points.len(), 8);
        assert_eq!(s.com, Vec3::new(0.0, -1.0, 12.75));
        assert!(s.points.contains(&Vec3::new(18.0, 17.5, 24.5)));
        assert_eq!(s.inertia.x_axis.x, 159.9);
        // `me_corrugated_metal8x8`'s moments are a box's per unit mass.
        let b = box_inertia(Vec3::new(48.0, 1.0, 48.0));
        assert!((b.x_axis.x - 768.33).abs() < 0.01);
    }
}
