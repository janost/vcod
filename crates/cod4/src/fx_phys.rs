//! The FX system's physics models: a model element (type 5) with flag
//! 0x8000000 spawns as a body in physics world 1 (`FX_SpawnElem`
//! 0x4a25b0 into 0x4a1ff0), draws at the body's pose (0x498340) and
//! frees it when the element dies (0x4a2909..0x4a291d). The spawn draws
//! off `fx_randomTable` at the element's seed, so a spawn here lands
//! where 1.7's does (cod4-effects.md 9.1.1).

use std::sync::Arc;

use crate::assets::Assets;
use crate::assets::fx::{Fx, FxVisual};
use crate::fx::{ElemDef, ElemType, Range, flags};
use crate::ode::math::{M3, V3};
use crate::ode::obj::ObjPreset;
use crate::ode::phys::{PhysWorld, normalize};
use crate::ode::world::{BodyId, PhysDvars};
use crate::ragdoll::iw;

fn w(x: f32) -> f64 {
    x as f64
}

fn f(x: f64) -> f32 {
    x as f32
}

/// `fx_randomTable` (iw3mp.exe 0x6bce70): 479 floats, then its first 28
/// again, so an index up to the seed plus 27 reads straight through.
#[rustfmt::skip]
const RANDOM_TABLE: [u32; 479] = include!("fx_random_table.in");

/// The table's entry `i`, wrapped as the copy past its end wraps it.
pub fn random(i: usize) -> f32 {
    f32::from_bits(RANDOM_TABLE[i % RANDOM_TABLE.len()])
}

/// The entry's low 16 bits, which the integer draws scale (0x4a20bd).
fn random_word(i: usize) -> i32 {
    (RANDOM_TABLE[i % RANDOM_TABLE.len()] & 0xffff) as i32
}

/// `base + (word * (amplitude + 1) >> 16)`, an integer range's draw.
fn draw_int((base, amp): (i32, i32), i: usize) -> i32 {
    base.wrapping_add(random_word(i).wrapping_mul(amp.wrapping_add(1)) >> 16)
}

fn draw(r: Range, i: usize) -> f32 {
    f(w(random(i)) * w(r.amplitude) + w(r.base))
}

/// The table holds 479 entries (0x1df, every `div`'s divisor).
const SEEDS: u32 = 479;

/// An effect's random seed off the CRT's `rand()` (0x4a12d2..0x4a12e9):
/// `rand * 479 >> 15`.
pub fn effect_seed(rand: i32) -> u16 {
    ((rand * SEEDS as i32) >> 15) as u16
}

/// An effect's seed off its start time, when an element of it or of an
/// effect it plays carries flag 0x1000 ([`timed_seed`], 0x4a12b4..0x4a12cc):
/// the MSVC LCG's next draw off the time, times 479, over 32768.
pub fn effect_seed_at(msec: i32) -> u16 {
    let r = (msec as u32).wrapping_mul(0x343fd).wrapping_add(0x269ec3) >> 17;
    (r.wrapping_mul(SEEDS) >> 15) as u16
}

/// A one-shot element's count (0x4a3220): `spawn.base`, plus a draw at
/// the effect's seed when the amplitude is set.
pub fn one_shot_count(def: &ElemDef, effect_seed: u16) -> i32 {
    if def.spawn.1 == 0 {
        return def.spawn.0;
    }
    draw_int(def.spawn, effect_seed as usize + 19)
}

/// `FX_SpawnElem`'s start and seed (0x4a25fb..0x4a2660) for the
/// `sequence`th of an element spawned at `msec`: the start takes
/// `spawnDelayMsec`, its amplitude drawn at `(296 sequence + effect seed +
/// start) % 479` off the whole sequence, and the seed is the same sum off
/// its low byte (the `movzx` at 0x4a263a).
pub fn elem_seed(def: &ElemDef, effect_seed: u16, msec: i32, sequence: u32) -> (i32, u16) {
    let mix = |sequence: u32, begin: i32| {
        (sequence
            .wrapping_mul(0x128)
            .wrapping_add(begin as u32)
            .wrapping_add(u32::from(effect_seed))
            % SEEDS) as usize
    };
    let mut begin = def.spawn_delay_msec.0.wrapping_add(msec);
    if def.spawn_delay_msec.1 != 0 {
        let at = mix(sequence, begin) + 18;
        begin = begin.wrapping_add(draw_int((0, def.spawn_delay_msec.1), at));
    }
    (begin, mix(sequence & 0xff, begin) as u16)
}

/// The longest a non-runner element lives past its spawn (0x4a2fa0 off no
/// loop count): `spawnDelayMsec`'s and `lifeSpanMsec`'s bases and
/// amplitudes summed (0x4a2fbe..0x4a2fc7).
fn max_life(def: &ElemDef) -> i32 {
    let (d, l) = (def.spawn_delay_msec, def.life_span_msec);
    d.0.wrapping_add(d.1).wrapping_add(l.0).wrapping_add(l.1)
}

/// 0x4a30e0: the sequences and times an update over `(from, to]` spawns of
/// looping element `def`, on the grid of its `intervalMsec` from the
/// effect's start `start` and under its count (0x4a314b..0x4a3207). An
/// update past 128 ms first moves `from` up to `to` less the element's
/// longest life plus 1 (0x4a310f..0x4a3147). Sequence 0 is the effect's own
/// spawn (0x4a33b0); an interval of 0, which retail divides by, spawns
/// nothing more (vcod's).
pub fn looping_spawns(def: &ElemDef, start: i32, mut from: i32, to: i32) -> Vec<(u32, i32)> {
    let (interval, count) = def.spawn;
    if interval <= 0 {
        return Vec::new();
    }
    let span = to.wrapping_sub(from);
    if to != i32::MAX && span > 0x80 {
        let keep = max_life(def).min(span).wrapping_add(1);
        if span > keep {
            from = from.wrapping_add(span - keep);
        }
    }
    let mut k = from.wrapping_sub(start) / interval + 1;
    let mut at = interval.wrapping_mul(k).wrapping_add(start);
    let mut out = Vec::new();
    while at <= to && k < count {
        out.push((k as u32, at));
        k += 1;
        at = at.wrapping_add(interval);
    }
    out
}

/// The frame an effect plays in: its origin and its axes as rows
/// (forward, left, up), 0x561100's of its quaternion.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EffectFrame {
    pub origin: V3,
    pub axes: M3,
}

impl EffectFrame {
    /// The frame an effect keeps as quaternion `q` (`x y z w`) and
    /// `origin`, its axes read back through 0x561100 wherever the spawn
    /// turns by them (0x4a5a07, 0x4a5b00, 0x4a5da0, 0x4a5dc7).
    pub fn new(origin: V3, q: iw::IwQuat) -> EffectFrame {
        EffectFrame {
            origin,
            axes: iw::unit_axes(q),
        }
    }

    /// `FX_SpawnEffect`'s frame off the caller's `axis` (rows): kept as
    /// its `AxisToQuat` (0x562980 at 0x4a13ef, effect +0x28).
    pub fn of_axis(origin: V3, axis: &M3) -> EffectFrame {
        EffectFrame::new(origin, axis_to_quat(axis))
    }
}

/// `FX_GetSpawnOrigin` (0x4a5cb0): the element's `spawnOrigin` drawn at
/// entries 6..8, through the effect's frame under flag 0x2 (0x4a5a00),
/// else added to its origin as it stands.
fn spawn_origin(def: &ElemDef, frame: &EffectFrame, seed: usize) -> V3 {
    let o: V3 = std::array::from_fn(|k| draw(def.spawn_origin[k], seed + 6 + k));
    if def.flags & flags::SPAWN_RELATIVE_TO_EFFECT == 0 {
        return std::array::from_fn(|k| f(w(frame.origin[k]) + w(o[k])));
    }
    // 0x4a5a00: the x axis's share added to the origin, then y's, then
    // z's, each sum stored.
    let a = &frame.axes;
    let mut out: V3 = std::array::from_fn(|k| f(w(a[0][k]) * w(o[0]) + w(frame.origin[k])));
    for k in 0..3 {
        let t = f(w(a[1][k]) * w(o[1]) + w(out[k]));
        out[k] = f(w(a[2][k]) * w(o[2]) + w(t));
    }
    out
}

/// 0x49f8e0: a unit direction off entries 9 and 10, `z = 2 r10 - 1` and
/// the angle `r9 * 2 pi` (the float's double at 0x70b078).
fn sphere_dir(seed: usize) -> V3 {
    let z = f(w(random(seed + 10)) * 2.0 - 1.0);
    let s = f(w(f(1.0 - w(z) * w(z))).sqrt());
    let theta = f(w(random(seed + 9)) * w(std::f32::consts::TAU));
    let (sin, cos) = (f(w(theta).sin()), f(w(theta).cos()));
    [f(w(cos) * w(s)), f(w(s) * w(sin)), z]
}

/// `FX_GetSpawnOffset` (0x4a5ab0): `out` moved by the spawn offset. A
/// sphere (spawn bits 0x10) along [`sphere_dir`] by the radius drawn at
/// entry 11, in the world's axes; a cylinder (0x20) round the effect's x
/// axis at the angle `r9 * 2 pi`, the radius at 11 and the height at 10
/// along it; none, nothing.
fn spawn_offset(def: &ElemDef, frame: &EffectFrame, seed: usize, out: &mut V3) {
    match def.flags & flags::SPAWN_OFFSET_MASK {
        0 => {}
        flags::SPAWN_OFFSET_SPHERE => {
            let d = sphere_dir(seed);
            let r = draw(def.spawn_offset_radius, seed + 11);
            for k in 0..3 {
                out[k] = f(w(d[k]) * w(r) + w(out[k]));
            }
        }
        _ => {
            let r = draw(def.spawn_offset_radius, seed + 11);
            let theta = f(w(random(seed + 9)) * w(std::f32::consts::TAU));
            let a = f(w(theta).cos() as f32 as f64 * w(r));
            let b = f(w(r) * w(f(w(theta).sin())));
            let ax = &frame.axes;
            for k in 0..3 {
                out[k] = f((w(ax[1][k]) * w(a) + w(out[k])) + w(ax[2][k]) * w(b));
            }
            let h = draw(def.spawn_offset_height, seed + 10);
            for k in 0..3 {
                out[k] = f(w(out[k]) + w(ax[0][k]) * w(h));
            }
        }
    }
}

/// 0x55fed0: the unit vector off the axis `v` is shortest along (the lower
/// index on a tie), less its share along `v`, normalized.
fn perpendicular(v: V3) -> V3 {
    let sq = v.map(|x| f(w(x) * w(x)));
    let mut k = usize::from(sq[0] > sq[1]);
    if sq[k] > sq[2] {
        k = 2;
    }
    let d = f(-w(v[k]));
    let mut out = v.map(|x| f(w(x) * w(d)));
    out[k] = f(w(out[k]) + 1.0);
    normalize(out).0
}

fn cross(a: V3, b: V3) -> V3 {
    [
        f(w(a[1]) * w(b[2]) - w(a[2]) * w(b[1])),
        f(w(a[2]) * w(b[0]) - w(a[0]) * w(b[2])),
        f(w(a[0]) * w(b[1]) - w(a[1]) * w(b[0])),
    ]
}

/// The element's frame (0x4a5d30) by its run bits: the world's at 0, the
/// effect's under 0x40 and 0x80, and under 0xc0 one at the spawn origin
/// moved by the offset, forward along the offset (`(1, 0, 0)` for none;
/// 0x4a5dcc..0x4a5eb6).
fn elem_frame(def: &ElemDef, frame: &EffectFrame, seed: usize) -> EffectFrame {
    match def.flags & flags::RUN_MASK {
        0 => EffectFrame {
            origin: [0.0; 3],
            axes: [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
        },
        flags::RUN_MASK => {
            let mut origin = spawn_origin(def, frame, seed);
            let mut fwd = [0.0; 3];
            spawn_offset(def, frame, seed, &mut fwd);
            for k in 0..3 {
                origin[k] = f(w(origin[k]) + w(fwd[k]));
            }
            let (mut fwd, len) = normalize(fwd);
            if len == 0.0 {
                fwd = [1.0, 0.0, 0.0];
            }
            let (left, up) = if def.flags & flags::SPAWN_OFFSET_MASK == flags::SPAWN_OFFSET_SPHERE {
                // 0x55fa30: up perpendicular, left up x forward
                let up = perpendicular(fwd);
                (cross(up, fwd), up)
            } else {
                // 0x401000's |z| against the double 0.999 at 0x70b270
                let side = if w(fwd[2]).abs() < 0.999 {
                    [0.0, 0.0, 1.0]
                } else {
                    [0.0, 1.0, 0.0]
                };
                let left = normalize(cross(side, fwd)).0;
                (left, cross(fwd, left))
            };
            EffectFrame {
                origin,
                axes: [fwd, left, up],
            }
        }
        _ => *frame,
    }
}

/// The element's origin in its frame (0x4a2220): none under run bits
/// 0xc0; else the spawn origin moved by the offset, and under 0x40 and
/// 0x80 taken into the effect's frame.
fn elem_origin(def: &ElemDef, frame: &EffectFrame, seed: usize) -> V3 {
    let run = def.flags & flags::RUN_MASK;
    if run == flags::RUN_MASK {
        return [0.0; 3];
    }
    let mut o = spawn_origin(def, frame, seed);
    spawn_offset(def, frame, seed, &mut o);
    if run == 0 {
        return o;
    }
    let d: V3 = std::array::from_fn(|k| f(w(o[k]) - w(frame.origin[k])));
    let a = &frame.axes;
    std::array::from_fn(|i| f((w(a[i][0]) * w(d[0]) + w(a[i][1]) * w(d[1])) + w(a[i][2]) * w(d[2])))
}

/// 0x4a6500 into 0x4a62f0: the angles at `ms` (`spawnAngles` at entries
/// 12..14 plus `angularVelocity` at 3..5 times `ms`), their forward, left
/// and up each turned into `frame`'s axes.
fn elem_axes(def: &ElemDef, frame: &M3, seed: usize, ms: f32) -> M3 {
    let ang: V3 = std::array::from_fn(|k| {
        let a = draw(def.spawn_angles[k], seed + 12 + k);
        let r = draw(def.angular_velocity[k], seed + 3 + k);
        f(w(r) * w(ms) + w(a))
    });
    let sc = |x: f32| (f(w(x).sin()), f(w(x).cos()));
    let (sp, cp) = sc(ang[0]);
    let (sy, cy) = sc(ang[1]);
    let (sr, cr) = sc(ang[2]);
    let fwd = [f(w(cp) * w(cy)), f(w(cp) * w(sy)), f(-w(sp))];
    let srsp = w(sr) * w(sp);
    let left = [
        f(srsp * w(cy) - w(cr) * w(sy)),
        f(w(cr) * w(cy) + w(sy) * srsp),
        f(w(sr) * w(cp)),
    ];
    let crsp = w(cr) * w(sp);
    let up = [
        f(w(sr) * w(sy) + w(cy) * crsp),
        f(w(sy) * crsp - w(sr) * w(cy)),
        f(w(cr) * w(cp)),
    ];
    [fwd, left, up].map(|v| {
        std::array::from_fn(|i| {
            f((w(v[0]) * w(frame[0][i]) + w(v[1]) * w(frame[1][i])) + w(v[2]) * w(frame[2][i]))
        })
    })
}

/// `AxisToQuat` (0x562980): the first of the four candidates whose stored
/// squared length is 1 or more (the last when none is), over its stored
/// square root; `[x, y, z, w]`.
pub fn axis_to_quat(a: &M3) -> iw::IwQuat {
    let m: [f32; 9] = std::array::from_fn(|k| a[k / 3][k % 3]);
    let sq = |x: f32| w(x) * w(x);
    let c0 = [
        f(w(m[5]) - w(m[7])),
        f(w(m[6]) - w(m[2])),
        f(w(m[1]) - w(m[3])),
        f(((w(m[4]) + w(m[0])) + w(m[8])) + 1.0),
    ];
    let (sa, sb, sc) = (sq(c0[1]), sq(c0[0]), sq(c0[2]));
    let n0 = f(sq(c0[3]) + ((sa + sb) + sc));
    let c1 = [
        f(w(m[6]) + w(m[2])),
        f(w(m[7]) + w(m[5])),
        f(((w(m[8]) - w(m[4])) - w(m[0])) + 1.0),
        c0[2],
    ];
    let (se, sf, sg) = (sq(c1[1]), sq(c1[0]), sq(c1[2]));
    let n1 = f(sc + ((sf + se) + sg));
    let c2 = [
        f(((w(m[0]) - w(m[4])) - w(m[8])) + 1.0),
        f(w(m[3]) + w(m[1])),
        c1[0],
        c0[0],
    ];
    let (si, sj) = (sq(c2[1]), sq(c2[0]));
    let n2 = f(sb + (sf + (sj + si)));
    let c3 = [
        c2[1],
        f(((w(m[4]) - w(m[0])) - w(m[8])) + 1.0),
        c1[1],
        c0[1],
    ];
    let n3 = f((se + (si + sq(c3[1]))) + sa);
    let (c, n) = if n0 >= 1.0 {
        (c0, n0)
    } else if n1 >= 1.0 {
        (c1, n1)
    } else if n2 >= 1.0 {
        (c2, n2)
    } else {
        (c3, n3)
    };
    let r = f(w(n).sqrt());
    let k = f(1.0 / w(r));
    c.map(|x| f(w(x) * w(k)))
}

/// 0x4a5f30 at 0 ms into a life of `life` ms: the velocity graph's first
/// interval at `u = 0`, its two samples weighted `count (1 - u)` and
/// `count u`, each axis's amplitude by entries 0..2; the world graph's
/// (flag 0x2000000) and the local one's turned into `frame` (0x1000000),
/// each times 1000.
fn elem_velocity(def: &ElemDef, frame: &M3, seed: usize, life: f32) -> V3 {
    let r: V3 = std::array::from_fn(|k| random(seed + k));
    let mut out = [0.0f32; 3];
    let count = def.vel_intervals();
    if def.vel.len() < 2 {
        return out;
    }
    let cf = count as f32;
    let x = f(w(f(0.0 / w(life))) * w(cf));
    let i = (x.floor() as usize).min(count - 1);
    let u = f(w(x) - i as f64);
    let w1 = f(w(u) * w(cf));
    let w0 = f(w(cf) - w(w1));
    let at = |pick: fn(&crate::fx::VelSample) -> &[Range; 3]| -> V3 {
        let (a, b) = (pick(&def.vel[i]), pick(&def.vel[i + 1]));
        std::array::from_fn(|k| {
            let t = f((w(a[k].amplitude) * w(r[k]) + w(a[k].base)) * w(w0));
            f(w(t) + (w(b[k].amplitude) * w(r[k]) + w(b[k].base)) * w(w1))
        })
    };
    if def.flags & flags::HAS_VELOCITY_WORLD != 0 {
        let v = at(|s| &s.world.velocity);
        for k in 0..3 {
            out[k] = f(w(v[k]) * 1000.0 + w(out[k]));
        }
    }
    if def.flags & flags::HAS_VELOCITY_LOCAL != 0 {
        let v = at(|s| &s.local.velocity);
        let turned: V3 = std::array::from_fn(|i| {
            f((w(frame[0][i]) * w(v[0]) + w(frame[1][i]) * w(v[1])) + w(frame[2][i]) * w(v[2]))
        });
        for k in 0..3 {
            out[k] = f(w(turned[k]) * 1000.0 + w(out[k]));
        }
    }
    out
}

/// Whether element `def` spawns as a world 1 body (0x4a2829, 0x4a2838).
pub fn is_physics(def: &ElemDef) -> bool {
    def.elem_type == Some(ElemType::Model) && def.flags & flags::PHYSICS != 0
}

/// What 0x4a1ff0 builds a body from: its position and axes (rows), its
/// velocity and spin, the visual it picked and the element's life.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Spawn {
    pub pos: V3,
    pub axes: M3,
    pub vel: V3,
    pub avel: V3,
    pub visual: usize,
    pub life: i32,
}

/// 0x4a1ff0 for element `def` with `visuals` models at `seed`: the
/// element's frame and its origin in it, the axes the angles at 0 ms give
/// through `AxisToQuat` and back (0x562980, 0x560fb0), the velocity at 0
/// ms of the life drawn at entry 17, the visual at entry 21, and the
/// angular velocity per second as `(roll, pitch, yaw)` (0x4a21d1..0x4a21ef).
pub fn spawn(def: &ElemDef, visuals: usize, frame: &EffectFrame, seed: u16) -> Spawn {
    let s = seed as usize;
    let ef = elem_frame(def, frame, s);
    let o = elem_origin(def, frame, s);
    let a = &ef.axes;
    let pos = std::array::from_fn(|i| {
        f(
            ((w(o[0]) * w(a[0][i]) + w(ef.origin[i])) + w(o[1]) * w(a[1][i]))
                + w(o[2]) * w(a[2][i]),
        )
    });
    let axes = iw::axes(axis_to_quat(&elem_axes(def, a, s, 0.0)));
    let life = draw_int(def.life_span_msec, s + 17);
    let vel = elem_velocity(def, a, s, life as f32);
    let av: V3 = std::array::from_fn(|k| f(w(draw(def.angular_velocity[k], s + 3 + k)) * 1000.0));
    let visual = if visuals <= 1 {
        0
    } else {
        ((random_word(s + 21) * visuals as i32) >> 16) as usize
    };
    Spawn {
        pos,
        axes,
        vel,
        avel: [av[2], av[0], av[1]],
        visual,
        life,
    }
}

/// The element's scale `ms` into its life (0x498399..0x4983e8): each
/// visual sample's `base + amplitude r28`, lerped across the interval the
/// life fraction falls in.
pub fn scale_at(def: &ElemDef, seed: u16, ms: i32, life: i32) -> f32 {
    let r = random(seed as usize + 28);
    let n = def.vis_intervals();
    let Some(first) = def.vis.first() else {
        return 1.0;
    };
    if n == 0 {
        return f(w(first.amplitude.scale) * w(r) + w(first.base.scale));
    }
    let x = (ms.clamp(0, life.max(1)) as f32 / life.max(1) as f32) * n as f32;
    let i = (x.floor() as usize).min(n - 1);
    let u = x - i as f32;
    let at = |k: usize| f(w(def.vis[k].amplitude.scale) * w(r) + w(def.vis[k].base.scale));
    at(i) * (1.0 - u) + at(i + 1) * u
}

impl PhysWorld {
    /// 0x4a21b4..0x4a21ef: `Phys_ObjCreate` in world 1 at the spawn, the
    /// collision off the visual's model (0x5a5710), then the spin written
    /// over the body's. None when the pool is full: retail frees the
    /// element then (0x4a2851..0x4a286f).
    pub fn fx_body(
        &mut self,
        s: &Spawn,
        model: &crate::assets::model::XModel,
        preset: &ObjPreset,
        dv: &PhysDvars,
    ) -> Option<BodyId> {
        let id = self.obj_create(s.pos, &s.axes, s.vel, preset, dv)?;
        self.set_collision_from_xmodel(id, model);
        self.world.body_mut(id).avel = s.avel;
        Some(id)
    }
}

/// A physics model element of an effect: its def, its place among the
/// effect's elements, its visuals' model names, whether it loops, and
/// whether it names an impact, death or emitted effect, which keeps
/// `FX_SpawnElem` from refusing it when its life is already past (0x4a0c60
/// at 0x4a26e5).
#[derive(Clone, Debug)]
pub struct PhysElem {
    pub def: ElemDef,
    pub index: usize,
    pub models: Vec<String>,
    pub looping: bool,
    pub chains: bool,
}

/// An effect's physics model elements, `msecLoopingLife` (effect +0xc),
/// and whether its seed comes off its start time (an element of it, or of
/// an effect it plays, with flag 0x1000) rather than `rand()`.
#[derive(Clone, Debug)]
pub struct PhysEffect {
    pub elems: Vec<PhysElem>,
    pub looping_life: i32,
    pub timed_seed: bool,
}

/// An element's flags (`FxElemDef` +0).
fn raw_flags(e: &crate::assets::fx::FxElem) -> u32 {
    u32::from_le_bytes(e.raw[0..4].try_into().unwrap())
}

/// 0x4a0ea0: whether an element of `fx`, or of an effect one of them
/// plays (its death, impact and emitted effects, a runner's visuals), has
/// flag 0x1000 (0x4a0ed4..0x4a0f9f). `find` names an effect by name;
/// `seen` stops a cycle, which retail would recurse through for ever
/// (vcod's).
fn timed_seed<'a>(
    fx: &'a Fx,
    find: &dyn Fn(&str) -> Option<&'a Fx>,
    seen: &mut Vec<&'a str>,
) -> bool {
    if seen.contains(&fx.name.as_str()) {
        return false;
    }
    seen.push(&fx.name);
    fx.elems.iter().any(|e| {
        let runners = e.visuals.iter().filter_map(|v| match v {
            FxVisual::Effect(Some(n)) => Some(n),
            _ => None,
        });
        raw_flags(e) & 0x1000 != 0
            || [&e.effect_on_death, &e.effect_on_impact, &e.effect_emitted]
                .into_iter()
                .flatten()
                .chain(runners)
                .any(|n| find(n).is_some_and(|f| timed_seed(f, find, seen)))
    })
}

impl PhysEffect {
    /// `fx`'s looping and one-shot physics model elements, its models
    /// named off `a` and its other effects found through `find`; None when
    /// it has none.
    pub fn of<'a>(
        a: &Assets,
        fx: &'a Fx,
        find: &dyn Fn(&str) -> Option<&'a Fx>,
    ) -> Option<PhysEffect> {
        let looping = fx.looping.max(0) as usize;
        let end = looping + fx.one_shot.max(0) as usize;
        let elems: Vec<PhysElem> = fx
            .elems
            .iter()
            .enumerate()
            .take(end)
            .filter_map(|(index, e)| {
                let def = ElemDef::of(e);
                is_physics(&def).then(|| PhysElem {
                    def,
                    index,
                    models: e
                        .visuals
                        .iter()
                        .filter_map(|v| match v {
                            FxVisual::Model(Some(m)) => Some(a.xmodels[m.index()].name.clone()),
                            _ => None,
                        })
                        .collect(),
                    looping: index < looping,
                    chains: e.effect_on_impact.is_some()
                        || e.effect_on_death.is_some()
                        || e.effect_emitted.is_some(),
                })
            })
            .collect();
        if elems.is_empty() {
            return None;
        }
        Some(PhysEffect {
            elems,
            looping_life: fx.msec_looping_life,
            timed_seed: timed_seed(fx, find, &mut Vec::new()),
        })
    }
}

/// Every effect of `zones` with a physics model element, by name; the
/// first zone holding a name wins, for the effect and the ones it plays.
pub fn effects(zones: &[&Assets]) -> std::collections::HashMap<String, Arc<PhysEffect>> {
    let find = |n: &str| {
        zones
            .iter()
            .find_map(|z| z.fx.iter().find(|f| f.name.eq_ignore_ascii_case(n)))
    };
    let mut out = std::collections::HashMap::new();
    for z in zones {
        for fx in &z.fx {
            if out.contains_key(&fx.name) {
                continue;
            }
            if let Some(p) = PhysEffect::of(z, fx, &find) {
                out.insert(fx.name.clone(), Arc::new(p));
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::assets::fx::FxElem;

    /// A model element of `flags` whose words at `+o` are `v`.
    fn elem(flags: u32, words: &[(usize, i32)]) -> FxElem {
        let mut raw = vec![0u8; 0xfc];
        raw[0..4].copy_from_slice(&flags.to_le_bytes());
        for &(o, v) in words {
            raw[o..o + 4].copy_from_slice(&v.to_le_bytes());
        }
        raw[0xb0] = 5;
        FxElem {
            elem_type: 5,
            visuals: Vec::new(),
            effect_on_impact: None,
            effect_on_death: None,
            effect_emitted: None,
            vel_samples: None,
            vis_samples: None,
            raw,
            trail: None,
        }
    }

    fn fx(name: &str, elems: Vec<FxElem>) -> Fx {
        Fx {
            name: name.into(),
            flags: 0,
            msec_looping_life: 0,
            looping: 0,
            one_shot: elems.len() as i32,
            emission: 0,
            elems,
        }
    }

    /// 0x4a30e0 on `explosions/grenadeexp_wood`'s element 1 (every 5 ms,
    /// 30 of them, a life of 1 s): each update spawns the grid's times
    /// past its start up to its end, under the count, and one past 128 ms
    /// first drops the spawns whose life would be over.
    #[test]
    fn looping_spawns_walk_the_interval_grid_under_the_count() {
        let def = ElemDef::of(&elem(flags::PHYSICS, &[(4, 5), (8, 30), (0x30, 1000)]));
        assert_eq!(
            looping_spawns(&def, 1000, 1000, 1017),
            [(1, 1005), (2, 1010), (3, 1015)]
        );
        assert_eq!(
            looping_spawns(&def, 1000, 1017, 1034),
            [(4, 1020), (5, 1025), (6, 1030)]
        );
        let tail = looping_spawns(&def, 1000, 1130, 1200);
        assert_eq!(tail.last(), Some(&(29, 1145)), "sequence 29 is the 30th");
        let short = ElemDef::of(&elem(flags::PHYSICS, &[(4, 5), (8, 1000), (0x30, 50)]));
        assert_eq!(looping_spawns(&short, 0, 0, 300).first(), Some(&(50, 250)));
        let none = ElemDef::of(&elem(flags::PHYSICS, &[(4, 0), (8, 30)]));
        assert!(looping_spawns(&none, 0, 0, 100).is_empty());
    }

    /// `FX_SpawnElem` draws the delay off the whole sequence and the
    /// element's seed off its low byte.
    #[test]
    fn the_elements_seed_takes_the_sequences_low_byte() {
        let def = ElemDef::of(&elem(flags::PHYSICS, &[(0x2c, 100)]));
        let (b0, s0) = elem_seed(&def, 7, 1000, 0);
        let (b1, s1) = elem_seed(&def, 7, 1000, 256);
        assert_ne!(b0, b1, "the delay's draw reads the whole sequence");
        assert_eq!(
            (i32::from(s0), i32::from(s1)),
            ((b0 + 7) % 479, (b1 + 7) % 479)
        );
    }

    /// 0x4a0ea0 finds flag 0x1000 on an effect a death effect's runner
    /// plays, and a cycle of effects ends.
    #[test]
    fn a_timed_seed_follows_the_chained_and_run_effects() {
        let mut parent = elem(flags::PHYSICS, &[]);
        parent.effect_on_death = Some("b".into());
        let mut runner = elem(0, &[]);
        runner.elem_type = 10;
        runner.visuals = vec![FxVisual::Effect(Some("c".into()))];
        let mut back = elem(0, &[]);
        back.effect_on_impact = Some("a".into());
        let all = [
            fx("a", vec![parent]),
            fx("b", vec![runner, back]),
            fx("c", vec![elem(0x1000, &[])]),
        ];
        let find = |n: &str| all.iter().find(|f| f.name == n);
        assert!(timed_seed(&all[0], &find, &mut Vec::new()));
        let untimed = [all[0].clone(), all[1].clone(), fx("c", vec![elem(0, &[])])];
        let find = |n: &str| untimed.iter().find(|f| f.name == n);
        assert!(!timed_seed(&untimed[0], &find, &mut Vec::new()));
    }
}
