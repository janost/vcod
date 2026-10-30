//! CoD4 1.7's trail elements (type 3). A moving effect leaves a
//! cross-section of its `FxTrailDef` behind every `splitDist` units of
//! its travel; each one lives and fades as an element would, and
//! neighbouring sections are joined by quads. The behaviour, its gates and
//! its evidence labels are in docs/research/cod4-effects.md 16.

use std::collections::VecDeque;
use std::sync::Arc;

use glam::Vec3;
use vcod_cod4::assets::fx::FxTrail;

use super::cod2::{Element, Live};
use super::efx::R1;
use super::sim::{FxQuad, Rng};

/// vcod's cap on the sections one trail holds; the oldest goes first.
const MAX_SECTIONS: usize = 512;

/// A trail element's definition: what each cross-section draws and how
/// long it lives.
#[derive(Clone, Debug)]
pub struct TrailDef {
    pub def: Arc<FxTrail>,
    pub life_ms: R1,
    pub delay_ms: R1,
    /// The material's angle falloff, on an `effect_falloff` set.
    pub falloff: Option<Falloff>,
}

/// An `*_falloff` set's vertex shader (`vertcol_simple_fog_fo_dtex`,
/// cod4-effects.md 16): `f = saturate(dot(view, normal)^2 * parms[0] +
/// parms[1])` scales the vertex alpha, and the colour by
/// `lerp(end, begin, f)`, so a sheet seen edge-on fades out.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Falloff {
    /// `falloffParms.zw`.
    pub parms: [f32; 2],
    pub begin: [f32; 4],
    pub end: [f32; 4],
}

impl Falloff {
    /// The shader's factor for a vertex at `pos` with unit `normal`, seen
    /// from `eye`.
    pub fn factor(&self, eye: Vec3, pos: Vec3, normal: Vec3) -> f32 {
        let d = (pos - eye).normalize_or_zero().dot(normal);
        (d * d * self.parms[0] + self.parms[1]).clamp(0.0, 1.0)
    }

    /// Scales a colour by factor `k`: the colour by `lerp(end, begin, k)`,
    /// the alpha by `k`.
    fn apply(&self, rgba: &mut [f32; 4], k: f32) {
        let (rgb, alpha) = rgba.split_at_mut(3);
        for ((c, b), e) in rgb.iter_mut().zip(self.begin).zip(self.end) {
            *c *= e + (b - e) * k;
        }
        alpha[0] *= k;
    }
}

/// A dropped cross-section: its element state, the fx time it starts
/// drawing, how far along the trail it fell, and the effect's right and up
/// axes at the drop.
struct Section {
    live: Live,
    start: f32,
    along: f32,
    ordinal: u32,
    right: Vec3,
    up: Vec3,
}

impl Section {
    /// Life used up at `now`, 0 at the start and 1 at the end; negative
    /// while the delay runs.
    fn age_frac(&self, now: f32) -> f32 {
        (now - self.start) * 1000.0 / self.live.life_ms
    }

    fn is_spent(&self, now: f32) -> bool {
        self.age_frac(now) >= 1.0
    }
}

/// A pose of the effect the trail rides: origin and axes.
type Pose = (Vec3, [Vec3; 3]);

/// A trail running on a moving effect.
pub(crate) struct Trail {
    el: Arc<Element>,
    def: TrailDef,
    shader: Arc<str>,
    /// The pose and fx time the last advance saw.
    prev_pose: Pose,
    prev_time: f32,
    /// Units the effect has travelled since the trail started.
    travelled: f32,
    dropped: u32,
    sections: VecDeque<Section>,
    /// The effect's tag stopped posing: no more cross-sections.
    pub(crate) ended: bool,
}

impl Trail {
    /// A trail started at `now` on the effect pose `pose`. Two sections
    /// fall at once at distance 0 (cod4-effects.md 16).
    pub(crate) fn start(
        el: Arc<Element>,
        def: TrailDef,
        shader: Arc<str>,
        pose: Pose,
        now: f32,
        rng: &mut Rng,
    ) -> Trail {
        let mut trail = Trail {
            el,
            def,
            shader,
            prev_pose: pose,
            prev_time: now,
            travelled: 0.0,
            dropped: 0,
            sections: VecDeque::new(),
            ended: false,
        };
        for _ in 0..2 {
            trail.push_section(pose, now, 0.0, rng);
        }
        trail
    }

    /// Appends a section dropped at `pose` and fx time `at`, `along` units
    /// down the trail; its life and delay are the element's draws.
    fn push_section(&mut self, (origin, axes): Pose, at: f32, along: f32, rng: &mut Rng) {
        let life_ms = rng.range(self.def.life_ms.a, self.def.life_ms.b).max(1.0);
        let delay_s = rng.range(self.def.delay_ms.a, self.def.delay_ms.b) * 0.001;
        let live = Live::spawn(
            self.el.clone(),
            rng,
            (origin, axes),
            Vec3::Z,
            life_ms,
            &self.shader,
            self.dropped,
        );
        if self.sections.len() == MAX_SECTIONS {
            self.sections.pop_front();
        }
        self.sections.push_back(Section {
            live,
            start: at + delay_s,
            along,
            ordinal: self.dropped,
            right: axes[1],
            up: axes[2],
        });
        self.dropped += 1;
    }

    /// Moves the trail to `pose` at `now`; `None` means the effect's tag
    /// is gone and the trail stops growing. Each multiple of `splitDist`
    /// the travel passes (the move's own end excluded) drops a section at
    /// the pose and time interpolated there. Spent sections then leave,
    /// except the newest spent one, which the tail still reaches back to.
    pub(crate) fn advance(&mut self, pose: Option<Pose>, now: f32, rng: &mut Rng) {
        match pose {
            Some(pose) if !self.ended => self.travel(pose, now, rng),
            _ => self.ended = true,
        }
        while self.sections.get(1).is_some_and(|s| s.is_spent(now)) {
            self.sections.pop_front();
        }
    }

    fn travel(&mut self, pose: Pose, now: f32, rng: &mut Rng) {
        let (from, from_axes) = self.prev_pose;
        let (from_t, from_d) = (self.prev_time, self.travelled);
        let step = from.distance(pose.0);
        let split = self.def.def.split_dist.max(1) as f32;
        let to_d = from_d + step;
        let first_mark = (from_d / split).floor() as u64 + 1;
        let marks = (first_mark..)
            .map(|k| k as f32 * split)
            .take_while(|&d| d < to_d)
            .take(MAX_SECTIONS);
        for mark in marks {
            let f = (mark - from_d) / step;
            let axes = std::array::from_fn(|k| from_axes[k].lerp(pose.1[k], f).normalize_or_zero());
            let at = from_t + (now - from_t) * f;
            self.push_section((from.lerp(pose.0, f), axes), at, mark, rng);
        }
        self.prev_pose = pose;
        self.prev_time = now;
        self.travelled = to_d;
    }

    /// Nothing left to draw or drop.
    pub(crate) fn done(&self, now: f32) -> bool {
        self.ended && self.sections.iter().all(|s| s.is_spent(now))
    }

    /// The cross-sections drawn at `now`, in drop order. A section still
    /// in its delay draws nothing. The trail's very first section starts
    /// transparent. A live section that follows a spent one gets a point
    /// ahead of it where the spent one's neighbour runs out of life, which
    /// carries the spent one's size and colour.
    fn cuts(&self, now: f32, repeat: f32, u_shift: f32) -> Vec<Cut> {
        let mut cuts = Vec::with_capacity(self.sections.len() + 1);
        let mut behind: Option<(Cut, f32)> = None;
        for (idx, s) in self.sections.iter().enumerate() {
            let frac = s.age_frac(now);
            if frac < 0.0 {
                continue;
            }
            let mut cut = Cut::of(s, now, frac.min(1.0), s.along / repeat + u_shift);
            if frac < 1.0 {
                if idx == 0 && s.ordinal == 0 {
                    cut.rgba[3] = 0.0;
                }
                if let Some((old, old_frac)) = behind.filter(|(_, f)| idx > 0 && *f >= 1.0) {
                    cuts.push(old.toward(&cut, (1.0 - old_frac) / (frac - old_frac)));
                }
                cuts.push(cut);
            }
            behind = Some((cut, frac));
        }
        cuts
    }

    /// The trail's quads at `now`, seen from `eye`: for each index pair of
    /// the cut, one quad per neighbouring pair of drawn sections. `u` runs
    /// along the trail (distance over `repeatDist`, wrapped to the first
    /// section, scrolled by `scrollTimeMsec`), `v` is the vertex's own.
    pub(crate) fn quads(&self, now: f32, eye: Vec3, out: &mut Vec<FxQuad>) {
        let def = &self.def.def;
        if def.verts.is_empty() || def.inds.len() < 2 || self.shader.is_empty() {
            return;
        }
        let Some(head) = self.sections.front() else {
            return;
        };
        let repeat = def.repeat_dist.max(1) as f32;
        let scroll = match def.scroll_ms {
            0 => 0.0,
            ms => {
                let period = ms.unsigned_abs() as f32;
                let phase = (now * 1000.0).rem_euclid(period) / period;
                if ms > 0 { phase } else { 1.0 - phase }
            }
        };
        let u_shift = scroll - (head.along / repeat).floor();
        let cuts = self.cuts(now, repeat, u_shift);
        let vert = |k: u16| &def.verts[usize::from(k).min(def.verts.len() - 1)];
        for (a, b) in cuts.iter().zip(cuts.iter().skip(1)) {
            let mean = std::array::from_fn::<f32, 4, _>(|c| 0.5 * (a.rgba[c] + b.rgba[c]));
            for &[i, j] in def.inds.as_chunks::<2>().0 {
                let (vi, vj) = (vert(i), vert(j));
                let corners = [
                    a.point(vi.pos),
                    a.point(vj.pos),
                    b.point(vj.pos),
                    b.point(vi.pos),
                ];
                let mut rgba = mean;
                if let Some(fo) = &self.def.falloff {
                    // The shader fades per vertex; a quad has one colour,
                    // so it takes the factor at its centre.
                    let normal =
                        (a.dir(vi.normal) + a.dir(vj.normal) + b.dir(vi.normal) + b.dir(vj.normal))
                            .normalize_or_zero();
                    let centre = corners.iter().sum::<Vec3>() / 4.0;
                    fo.apply(&mut rgba, fo.factor(eye, centre, normal));
                }
                // `u` is the first texture coordinate, along the trail
                // (cod4-effects.md 16, the packed vertex).
                out.push(FxQuad {
                    verts: corners.map(|p| p.to_array()),
                    uvs: [[a.u, vi.tex], [a.u, vj.tex], [b.u, vj.tex], [b.u, vi.tex]],
                    rgba,
                    shader: self.shader.clone(),
                });
            }
        }
    }
}

/// One drawn cross-section: centre, axes, size, colour and `u`.
#[derive(Clone, Copy)]
struct Cut {
    centre: Vec3,
    right: Vec3,
    up: Vec3,
    size: [f32; 2],
    rgba: [f32; 4],
    u: f32,
}

impl Cut {
    /// `s` drawn at `now`, its channels sampled at life fraction `frac`.
    fn of(s: &Section, now: f32, frac: f32, u: f32) -> Cut {
        let rgb = s.live.rgb(frac);
        Cut {
            centre: s.live.position((now - s.start).max(0.0)),
            right: s.right,
            up: s.up,
            size: [s.live.size(frac), s.live.size2(frac)],
            rgba: [rgb.x, rgb.y, rgb.z, s.live.alpha(frac)],
            u,
        }
    }

    /// This cut moved a fraction `f` of the way to `next`'s place, axes
    /// and `u`; size and colour stay this cut's.
    fn toward(&self, next: &Cut, f: f32) -> Cut {
        Cut {
            centre: self.centre.lerp(next.centre, f),
            right: self.right.lerp(next.right, f),
            up: self.up.lerp(next.up, f),
            u: self.u + (next.u - self.u) * f,
            ..*self
        }
    }

    /// The world point of a cut vertex at `pos` (in sizes).
    fn point(&self, pos: [f32; 2]) -> Vec3 {
        self.centre + self.right * (pos[0] * self.size[0]) + self.up * (pos[1] * self.size[1])
    }

    /// A cut vertex's normal in the world.
    fn dir(&self, n: [f32; 2]) -> Vec3 {
        self.right * n[0] + self.up * n[1]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use vcod_cod2::efx::{Kind, Primitive};
    use vcod_cod4::assets::fx::FxTrailVertex;

    /// A flat ribbon: two points across, one index pair.
    fn ribbon() -> TrailDef {
        TrailDef {
            def: Arc::new(FxTrail {
                scroll_ms: 0,
                repeat_dist: 100,
                split_dist: 10,
                verts: vec![
                    FxTrailVertex {
                        pos: [-1.0, 0.0],
                        normal: [0.0, 1.0],
                        tex: 0.0,
                    },
                    FxTrailVertex {
                        pos: [1.0, 0.0],
                        normal: [0.0, 1.0],
                        tex: 1.0,
                    },
                ],
                inds: vec![0, 1],
            }),
            life_ms: R1 {
                a: 1000.0,
                b: 1000.0,
            },
            delay_ms: R1 { a: 0.0, b: 0.0 },
            falloff: None,
        }
    }

    /// `smoke_geotrail_rpg`'s `gfx_smoke_trail02_godray` falloff: a flat
    /// ribbon seen from above draws whole, seen edge-on not at all.
    #[test]
    fn a_falloff_ribbon_fades_edge_on() {
        let el = Arc::new(Element {
            prim: Primitive::new(Kind::Particle, 0),
            atlas: Default::default(),
        });
        let mut rng = Rng::new(1);
        let axis = [Vec3::X, Vec3::Y, Vec3::Z];
        let mut def = ribbon();
        def.falloff = Some(Falloff {
            parms: [11.517542, -0.34729624],
            begin: [1.0; 4],
            end: [1.0, 1.0, 1.0, 0.5],
        });
        let mut t = Trail::start(el, def, "trail".into(), (Vec3::ZERO, axis), 0.0, &mut rng);
        t.advance(Some((Vec3::new(100.0, 0.0, 0.0), axis)), 0.1, &mut rng);
        let alpha = |eye: Vec3| {
            let mut q = Vec::new();
            t.quads(0.1, eye, &mut q);
            q[5].rgba[3]
        };
        let full = {
            let mut q = Vec::new();
            t.quads(0.1, Vec3::new(55.0, 0.0, 1000.0), &mut q);
            q[5].rgba[3]
        };
        assert!(full > 0.0);
        assert!((alpha(Vec3::new(55.0, 0.0, 1000.0)) - full).abs() < 1e-6);
        assert_eq!(alpha(Vec3::new(55.0, 1000.0, 0.0)), 0.0, "edge-on");
    }

    /// A trail on an effect moving 100 units drops a section every
    /// `splitDist` (10) and draws a quad between each neighbouring pair.
    #[test]
    fn a_moving_trail_drops_a_section_every_split_distance() {
        let el = Arc::new(Element {
            prim: Primitive::new(Kind::Particle, 0),
            atlas: Default::default(),
        });
        let mut rng = Rng::new(1);
        let axis = [Vec3::X, Vec3::Y, Vec3::Z];
        let mut t = Trail::start(
            el,
            ribbon(),
            "trail".into(),
            (Vec3::ZERO, axis),
            0.0,
            &mut rng,
        );
        assert_eq!(t.sections.len(), 2);
        t.advance(Some((Vec3::new(100.0, 0.0, 0.0), axis)), 0.1, &mut rng);
        // Two at the start, then 10, 20, .. 90: the next falls at 100,
        // where the move ends.
        assert_eq!(
            t.sections.len(),
            11,
            "{:?}",
            t.sections.iter().map(|s| s.along).collect::<Vec<_>>()
        );
        let mut q = Vec::new();
        t.quads(0.1, Vec3::new(50.0, 0.0, 100.0), &mut q);
        assert_eq!(q.len(), 10);
        // The texture's u runs a tenth of a repeat a section.
        assert!((q[5].uvs[2][0] - q[5].uvs[0][0] - 0.1).abs() < 1e-5);
        // The tag gone, the sections live out and the trail ends.
        t.advance(None, 0.2, &mut rng);
        assert!(!t.done(0.2));
        assert!(t.done(1.2));
    }
}
