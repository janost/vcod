//! The static models (`misc_model`) a locational trace clips after the
//! world: each placement one BVH prim, its surfaces walked in file order
//! (docs/research/cod11-combat.md 2.7.2).

use super::clip::{ModelSurf, clip_surfs};
use super::walk::TraceInput;
use super::{CollisionWorld, ModelTri, Prim, Trace};
use glam::{Mat3, Vec3};
use std::ops::Range;

/// The placed static models [`CollisionWorld::with_statics`] clips, each
/// model's surfaces stored once in its own frame.
#[derive(Clone, Default)]
pub struct StaticModels {
    /// Every model's collision triangles in the model's frame.
    pub tris: Vec<ModelTri>,
    /// Every model's surfaces, runs into `tris` in file order.
    pub surfs: Vec<ModelSurf>,
    pub placed: Vec<StaticModel>,
}

/// One `misc_model`: where it stands and which surfaces it clips.
#[derive(Clone, Debug)]
pub struct StaticModel {
    pub origin: Vec3,
    /// Model frame to world, `modelscale` in: the angles' axis with each
    /// row scaled, whose inverse retail stores (`cod_lnxded` 0x80514f1..
    /// 0x805155c).
    pub axis: Mat3,
    /// Into [`StaticModels::surfs`].
    pub surfs: Range<usize>,
}

impl StaticModels {
    /// World-space triangles, each its own placement, unculled by bounds:
    /// a clip a triangle at a time, for a game whose static walk is not
    /// read (CoD2) and for hand-built worlds.
    pub fn loose(tris: &[ModelTri]) -> Self {
        let mut out = StaticModels::default();
        for mt in tris {
            let [a, b, c] = mt.tri;
            if (b - a).cross(c - a).length_squared() < 1e-6 {
                continue;
            }
            let i = out.tris.len();
            out.tris.push(*mt);
            out.surfs.push(ModelSurf {
                tris: i..i + 1,
                rot: glam::Quat::IDENTITY,
                pos: Vec3::ZERO,
                mins: Vec3::NEG_INFINITY,
                maxs: Vec3::INFINITY,
            });
            out.placed.push(StaticModel {
                origin: Vec3::ZERO,
                axis: Mat3::IDENTITY,
                surfs: i..i + 1,
            });
        }
        out
    }

    pub fn is_empty(&self) -> bool {
        self.placed.is_empty()
    }
}

/// A placement as the world keeps it.
#[derive(Clone, Debug)]
pub(super) struct StaticPlacement {
    origin: Vec3,
    /// World to model frame: retail's stored inverse (0x8063f38).
    to_model: Mat3,
    surfs: Range<usize>,
}

/// Each placement's BVH prim, boxing every surface's stored bounds (its
/// triangles where it has none), and the placements.
pub(super) fn static_prims(
    statics: &StaticModels,
) -> (Vec<(Prim, Vec3, Vec3)>, Vec<StaticPlacement>) {
    let mut prims = Vec::with_capacity(statics.placed.len());
    let mut placed = Vec::with_capacity(statics.placed.len());
    for p in &statics.placed {
        let (mut lo, mut hi) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
        let mut grow = |v: Vec3| {
            let w = p.axis * v + p.origin;
            lo = lo.min(w);
            hi = hi.max(w);
        };
        for s in &statics.surfs[p.surfs.clone()] {
            if s.mins.is_finite() && s.maxs.is_finite() {
                let to_model = s.rot.inverse();
                for corner in 0..8 {
                    let pick = |bit: u32, k: usize| {
                        if corner & bit == 0 {
                            s.mins[k]
                        } else {
                            s.maxs[k]
                        }
                    };
                    let c = Vec3::new(pick(1, 0), pick(2, 1), pick(4, 2));
                    grow(to_model * (c - s.pos));
                }
            } else {
                for mt in &statics.tris[s.tris.clone()] {
                    mt.tri.iter().for_each(|&v| grow(v));
                }
            }
        }
        if lo.cmpgt(hi).any() {
            continue;
        }
        prims.push((
            Prim::Static(placed.len() as u32),
            lo - Vec3::splat(0.25),
            hi + Vec3::splat(0.25),
        ));
        placed.push(StaticPlacement {
            origin: p.origin,
            to_model: p.axis.inverse(),
            surfs: p.surfs.clone(),
        });
    }
    (prims, placed)
}

impl CollisionWorld {
    /// The static models' pass of a locational trace, after the world:
    /// each placement the segment reaches, nearest first, walks its
    /// surfaces with the segment moved into its frame (`cod_lnxded`
    /// 0x8051984). The surface gate reads the fraction the world and the
    /// models before it left (combat doc 2.7.2).
    pub(super) fn clip_statics(&self, sw: &TraceInput, trace: &mut Trace) {
        if self.models_root != u32::MAX {
            self.static_node(self.models_root, sw, trace);
        }
    }

    fn static_node(&self, i: u32, sw: &TraceInput, trace: &mut Trace) {
        let meets = |lo: Vec3, hi: Vec3, fraction: f32| {
            let (a, b) = sw.reach(fraction);
            a.cmple(hi).all() && b.cmpge(lo).all() && !sw.misses(lo, hi)
        };
        let node = &self.nodes[i as usize];
        if !meets(node.lo, node.hi, trace.fraction) {
            return;
        }
        if node.count > 0 {
            let first = node.first as usize;
            for &(prim, lo, hi) in &self.prims[first..first + node.count as usize] {
                if let Prim::Static(p) = prim
                    && meets(lo, hi, trace.fraction)
                {
                    self.clip_static(p as usize, sw, trace);
                }
            }
            return;
        }
        let along = |n: u32| {
            let node = &self.nodes[n as usize];
            ((node.lo + node.hi) * 0.5 - sw.start).dot(sw.end - sw.start)
        };
        let (near, far) = if along(node.first) <= along(node.second) {
            (node.first, node.second)
        } else {
            (node.second, node.first)
        };
        self.static_node(near, sw, trace);
        self.static_node(far, sw, trace);
    }

    /// One placement's surface walk. `SV_Trace` clips the static models
    /// after the world and before the entities, each pass taking only a
    /// nearer fraction, so a static model wins a tie with a submodel and
    /// loses one with model 0 (bsp-ibsp59-format.md, "The leaf walk").
    fn clip_static(&self, p: usize, sw: &TraceInput, trace: &mut Trace) {
        let s = &self.statics[p];
        let (start, end) = (
            s.to_model * (sw.start - s.origin),
            s.to_model * (sw.end - s.origin),
        );
        let takes_tie = match trace.hit {
            Some(Prim::Brush(b)) => self.brushes[b as usize].model != 0,
            Some(Prim::Terrain(p)) => self.terrain_model[p as usize] != 0,
            Some(Prim::Patch(p)) => self.patches[p as usize].model != 0,
            _ => false,
        };
        let surfs = &self.static_surfs[s.surfs.clone()];
        if clip_surfs(
            trace,
            start,
            end,
            &self.model_tris,
            surfs,
            sw.mask,
            takes_tie,
        ) && s.to_model != Mat3::IDENTITY
        {
            // The hit's normal back through the inverse's transpose
            // (0x80663e4), then normalized (0x8065a38).
            trace.normal = (s.to_model.transpose() * trace.normal).normalize();
        }
    }
}
