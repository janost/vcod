//! Contains routines ported from the Quake III Arena GPL source, Copyright (C) 1999-2005 Id Software, Inc..
//! See NOTICE.
//!
//! The query entry points: traces, position tests and contents.

use super::build::Volume;
use super::clip::{Capsule, Shape, capsule_sweep, clip_brush_capsule, clip_segment};
use super::masks::{
    CONTENTS_NODROP, CONTENTS_WATER, MASK_MISSILE_MOVE, TRACE_MASK_MOVE, TRACE_MASK_SHOT,
};
use super::reach::sweep;
use super::walk::TraceInput;
use super::{CollisionWorld, Prim, Pusher, Trace, cm_endpos, sv_trace_endpos};
use crate::patch::Arith;
use glam::Vec3;
use std::sync::atomic::Ordering;

impl CollisionWorld {
    /// The world clip's half of a trace, into a fresh `trace`.
    fn trace_world_clip(
        &self,
        start: Vec3,
        end: Vec3,
        mins: Vec3,
        maxs: Vec3,
        mask: u32,
        trace: &mut Trace,
    ) {
        let Some(clip) = &self.world_clip else {
            return;
        };
        let h = clip.trace(start, end, mins, maxs, mask);
        trace.fraction = h.fraction;
        trace.normal = h.normal;
        trace.surface_flags = h.surface_flags;
        trace.startsolid = h.startsolid;
        trace.allsolid = h.allsolid;
        if h.fraction < 1.0 || h.startsolid {
            trace.hit = Some(Prim::Clip(h.contents));
        }
    }
}

impl CollisionWorld {
    /// A point trace against the water brushes alone, the mask 0x20 trace
    /// UO's `G_ExplodeMissile` finds the surface with: the nearest water
    /// side the segment enters, as a fraction of it, or `None`.
    pub fn water_trace(&self, start: Vec3, end: Vec3) -> Option<f32> {
        self.water_hit(start, end).map(|t| t.fraction)
    }

    /// [`Self::water_trace`] with the side it entered: its normal and its
    /// material's surface flags, what `G_RunMissile`'s splash reads
    /// (docs/research/cod11-combat.md 12.1). A start inside water reads
    /// fraction 0.
    pub fn water_hit(&self, start: Vec3, end: Vec3) -> Option<Trace> {
        sweep(start, end, Vec3::ZERO, Vec3::ZERO);
        let mut trace = Trace::clear(end);
        for v in &self.water {
            if !(start.min(end).cmple(v.hi).all() && start.max(end).cmpge(v.lo).all()) {
                continue;
            }
            clip_segment(
                &mut trace,
                start,
                end,
                &v.planes,
                &v.side_flags,
                Vec3::ZERO,
                Prim::Brush(0),
            );
            if trace.allsolid {
                trace.endpos = start;
                return Some(trace);
            }
        }
        trace.endpos = start + (end - start) * trace.fraction;
        (trace.fraction < 1.0).then_some(trace)
    }

    /// The water brushes' half of a world trace whose mask holds
    /// [`CONTENTS_WATER`], clipped into `trace` by the brush arm the world's
    /// own brushes take (UO's prone check, 0x810031; coduo-movement.md
    /// 17.1). A water side that cuts the fraction, or a start inside water,
    /// leaves `hit` at `Prim::Clip(CONTENTS_WATER)`, the trace's contents.
    pub fn clip_water(&self, start: Vec3, end: Vec3, mins: Vec3, maxs: Vec3, trace: &mut Trace) {
        let sweep = capsule_sweep(start, end, mins, maxs);
        let sphere = Capsule::of(mins, maxs).offset;
        let (lo, hi) = (start.min(end) + mins, start.max(end) + maxs);
        let before = trace.fraction;
        for v in &self.water {
            if !(lo.cmple(v.hi).all() && hi.cmpge(v.lo).all()) {
                continue;
            }
            let mut w = Trace::clear(end);
            clip_brush_capsule(
                &mut w,
                &sweep,
                sphere,
                &v.planes,
                &v.side_flags,
                Prim::Clip(CONTENTS_WATER),
                self.arith(),
                false,
            );
            if w.startsolid {
                trace.startsolid = true;
                trace.hit = Some(Prim::Clip(CONTENTS_WATER));
                if w.allsolid {
                    trace.allsolid = true;
                    trace.fraction = 0.0;
                    trace.surface_flags = 0;
                }
            } else if w.fraction < trace.fraction {
                trace.fraction = w.fraction;
                trace.normal = w.normal;
                trace.surface_flags = w.surface_flags;
                trace.hit = Some(Prim::Clip(CONTENTS_WATER));
            }
        }
        if trace.fraction < before {
            trace.copy = None;
            trace.endpos = cm_endpos(start, end, mins, maxs, trace.fraction, self.arith());
        }
    }

    /// Contents at a point: `CONTENTS_WATER` inside any water brush,
    /// [`CONTENTS_NODROP`] inside any world nodrop brush, plus the content
    /// flags of any solid brush containing it (Q3 `CM_PointContents` over the
    /// brushes that made it into the world).
    pub fn point_contents(&self, p: Vec3) -> u32 {
        sweep(p, p, Vec3::ZERO, Vec3::ZERO);
        let inside = |v: &Volume| {
            p.cmple(v.hi).all()
                && p.cmpge(v.lo).all()
                && v.planes.iter().all(|&(n, d)| n.dot(p) <= d)
        };
        let mut out = 0;
        if self.water.iter().any(inside) {
            out |= CONTENTS_WATER;
        }
        if self.nodrop.iter().any(inside) {
            out |= CONTENTS_NODROP;
        }
        if let Some(clip) = &self.world_clip {
            out |= clip.point_contents(p);
        }
        if self.nodes.is_empty() {
            return out;
        }
        // The traces' BVH: pmove samples this on every step.
        out |= self.node_contents(0, p, false);
        if self.any_copy.load(Ordering::Acquire) {
            let copies = self.copies.read().unwrap_or_else(|e| e.into_inner());
            for c in copies.iter().filter(|c| c.contents != 0) {
                out |= self.node_contents(0, c.to_local(p), true);
            }
        }
        if !self.any_posed.load(Ordering::Acquire) {
            return out;
        }
        let poses = self.poses.read().unwrap_or_else(|e| e.into_inner());
        for (model, pose) in poses.iter() {
            if !(p.cmple(pose.hi).all() && p.cmpge(pose.lo).all()) || !self.model_linked(*model) {
                continue;
            }
            let span = self.model_span[*model];
            for brush in &self.brushes[span.brushes.0 as usize..span.brushes.1 as usize] {
                if brush.planes.iter().all(|&pl| {
                    let (n, d) = pose.plane(pl, span.origin);
                    n.dot(p) <= d
                }) {
                    out |= brush.content_flags;
                }
            }
        }
        out
    }

    /// The contents of the spawn-placed brushes under node `i` that hold `p`,
    /// model 0's alone when `world_only`.
    fn node_contents(&self, i: u32, p: Vec3, world_only: bool) -> u32 {
        let node = &self.nodes[i as usize];
        if i == self.models_root || !(p.cmple(node.hi).all() && p.cmpge(node.lo).all()) {
            return 0;
        }
        if node.count == 0 {
            return self.node_contents(node.first, p, world_only)
                | self.node_contents(node.second, p, world_only);
        }
        let first = node.first as usize;
        let mut out = 0;
        for (prim, lo, hi) in &self.prims[first..first + node.count as usize] {
            if let Prim::Brush(b) = prim
                && p.cmple(*hi).all()
                && p.cmpge(*lo).all()
            {
                let brush = &self.brushes[*b as usize];
                if self.brush_linked(brush)
                    && self.brush_at_spawn(brush)
                    && (!world_only || brush.model == 0)
                    && brush.planes.iter().all(|&(n, d)| n.dot(p) <= d)
                {
                    out |= brush.content_flags;
                }
            }
        }
        out
    }

    /// [`Self::box_position_test`] against inline model 0 alone (brushes,
    /// terrain and patches, no submodel, world copy or static model):
    /// `SV_Trace`'s world clip, which ends the trace at fraction 0 before
    /// any entity is walked (`cod_lnxded` 0x80916f4).
    pub fn world_box_position_test(&self, p: Vec3, mins: Vec3, maxs: Vec3, mask: u32) -> bool {
        self.position_walk(p, mins, maxs, mask, true).startsolid
    }

    fn position_walk(&self, p: Vec3, mins: Vec3, maxs: Vec3, mask: u32, world_only: bool) -> Trace {
        sweep(p, p, mins, maxs);
        let mut trace = Trace::clear(p);
        let capsule = Capsule::of(mins, maxs);
        self.trace_world_clip(p, p, mins, maxs, mask, &mut trace);
        self.walk_tree(
            TraceInput {
                start: p,
                end: p,
                mins,
                maxs,
                capsule,
                mask,
                skip: None,
                world_only,
                boxed: true,
                sphere: capsule.offset,
                sight: false,
            },
            &mut trace,
        );
        trace
    }

    /// Whether a box at `p` is inside anything under `mask`, as `trap_Trace`
    /// with a box that does not move reads it: brushes (posed submodels and
    /// world copies included) against the box, terrain and patches by their
    /// capsule position tests. `G_TryPushingEntity`'s test of a spot
    /// (game.mp 0x54b3b, docs/research/cod11-movers.md 20).
    pub fn box_position_test(&self, p: Vec3, mins: Vec3, maxs: Vec3, mask: u32) -> bool {
        let mut trace = self.position_walk(p, mins, maxs, mask, false);
        if trace.startsolid {
            return true;
        }
        if self.any_posed.load(Ordering::Acquire) {
            let poses = self.poses.read().unwrap_or_else(|e| e.into_inner());
            let (mut moved, mut scratch) = (Vec::new(), Vec::new());
            for (model, pose) in poses.iter() {
                if !self.model_linked(*model) {
                    continue;
                }
                self.clip_model(
                    *model,
                    Some(pose),
                    p,
                    p,
                    Shape::Box(mins, maxs),
                    mask,
                    &mut trace,
                    &mut moved,
                    &mut scratch,
                );
            }
            if trace.startsolid {
                return true;
            }
        }
        if self.any_copy.load(Ordering::Acquire) {
            let copies = self.copies.read().unwrap_or_else(|e| e.into_inner());
            if copies.iter().filter(|c| c.contents & mask != 0).any(|c| {
                self.clip_copy(*c, p, p, mins, maxs, mask, true, false)
                    .startsolid
            }) {
                return true;
            }
        }
        self.trace_instances(p, p, Shape::Box(mins, maxs), mask, None, &mut trace);
        trace.startsolid
    }

    /// Movement sweep (`mins == maxs == ZERO` is a ray), retail's
    /// `trap_Trace` / `trap_TraceCapsule`: brushes and terrain, never a
    /// static model (`cod_lnxded` 0x80916f4 traces them only on the flag the
    /// locational syscall passes).
    pub fn box_trace(&self, start: Vec3, end: Vec3, mins: Vec3, maxs: Vec3) -> Trace {
        self.trace_with_mask(start, end, mins, maxs, TRACE_MASK_MOVE, false, None)
    }

    /// [`Self::box_trace`] under pmove's `tracemask`.
    pub fn move_trace(&self, start: Vec3, end: Vec3, mins: Vec3, maxs: Vec3, mask: u32) -> Trace {
        self.trace_with_mask(start, end, mins, maxs, mask, false, None)
    }

    /// A box sweep under `mask` that passes through `pusher`'s clip: a
    /// pushed body's sweep from where it stood to where the push puts it,
    /// which must not stop on the pusher it starts inside.
    pub fn box_trace_except(
        &self,
        start: Vec3,
        end: Vec3,
        mins: Vec3,
        maxs: Vec3,
        mask: u32,
        pusher: Pusher,
    ) -> Trace {
        self.trace_with_mask(start, end, mins, maxs, mask, false, Some(pusher))
    }

    /// Bullet segment, retail's `trap_LocationalTrace`: [`MASK_SHOT`](super::MASK_SHOT) (on a
    /// world with a clip, its [`WorldClip::shot_mask`](super::WorldClip::shot_mask)) and the static
    /// models.
    pub fn shot_trace(&self, start: Vec3, end: Vec3) -> Trace {
        let mask = self
            .world_clip
            .as_ref()
            .map_or(TRACE_MASK_SHOT, |c| c.shot_mask());
        self.trace_with_mask(start, end, Vec3::ZERO, Vec3::ZERO, mask, true, None)
    }

    /// A point segment with any mask, with or without the static models:
    /// retail's `trap_Trace` on a zero box is one without them.
    pub fn point_trace(&self, start: Vec3, end: Vec3, mask: u32, statics: bool) -> Trace {
        self.trace_with_mask(start, end, Vec3::ZERO, Vec3::ZERO, mask, statics, None)
    }

    /// A point segment against inline model 0 alone (brushes, terrain and
    /// patches; no submodel, world copy, model instance or static model):
    /// `CM_BoxTrace` with model 0, which walks the world's tree from node 0
    /// (CoDMP.exe 0x424961) where any other model walks its own leaf.
    pub fn world_point_trace(&self, start: Vec3, end: Vec3, mask: u32) -> Trace {
        sweep(start, end, Vec3::ZERO, Vec3::ZERO);
        let mut trace = Trace::clear(end);
        self.trace_world_clip(start, end, Vec3::ZERO, Vec3::ZERO, mask, &mut trace);
        self.walk_tree(
            TraceInput {
                start,
                end,
                mins: Vec3::ZERO,
                maxs: Vec3::ZERO,
                capsule: Capsule::of(Vec3::ZERO, Vec3::ZERO),
                mask,
                skip: None,
                world_only: true,
                boxed: false,
                sphere: Capsule::of(Vec3::ZERO, Vec3::ZERO).offset,
                sight: false,
            },
            &mut trace,
        );
        trace.endpos = cm_endpos(
            start,
            end,
            Vec3::ZERO,
            Vec3::ZERO,
            trace.fraction,
            self.arith(),
        );
        trace
    }

    /// [`Self::point_trace`] passing through `skip`'s clip: a trace whose
    /// pass entity is a brush model.
    pub fn point_trace_except(
        &self,
        start: Vec3,
        end: Vec3,
        mask: u32,
        statics: bool,
        skip: Option<Pusher>,
    ) -> Trace {
        self.trace_with_mask(start, end, Vec3::ZERO, Vec3::ZERO, mask, statics, skip)
    }

    /// A missile's segment, `G_RunMissile`'s `trap_LocationalTrace`:
    /// [`MASK_MISSILE_MOVE`] and the static models, which that syscall's
    /// `SV_Trace` walks (cod11-mantle.md, "Static models are clipped as a
    /// segment"; combat doc 12.3: a frag rests on a crate's mesh).
    pub fn missile_trace(&self, start: Vec3, end: Vec3) -> Trace {
        self.trace_with_mask(
            start,
            end,
            Vec3::ZERO,
            Vec3::ZERO,
            MASK_MISSILE_MOVE,
            true,
            None,
        )
    }

    /// An item's sweep, `G_RunItem`'s and `G_BounceItem`'s: the item's own
    /// box as a capsule (every item carries `eFlags` 0x10, which picks
    /// `trap_TraceCapsule`), the given mask, and the static models, as the
    /// missile's sweep has them (docs/research/cod11-items.md, section 8).
    pub fn item_trace(&self, start: Vec3, end: Vec3, mins: Vec3, maxs: Vec3, mask: u32) -> Trace {
        self.trace_with_mask(start, end, mins, maxs, mask, true, None)
    }

    /// `trap_Trace`'s box sweep under `mask`, passing through `skip`'s
    /// clip: brushes take the box, terrain and patches the capsule (the
    /// engine fakes a box as a capsule only against terrain, 0x80536c8),
    /// [`Capsule::wide`]. What a UO vehicle's slide and probes sweep.
    pub fn box_sweep(
        &self,
        start: Vec3,
        end: Vec3,
        mins: Vec3,
        maxs: Vec3,
        mask: u32,
        skip: Option<Pusher>,
    ) -> Trace {
        self.trace_shape(start, end, mins, maxs, mask, false, skip, true)
    }

    pub(super) fn trace_with_mask(
        &self,
        start: Vec3,
        end: Vec3,
        mins: Vec3,
        maxs: Vec3,
        mask: u32,
        statics: bool,
        skip: Option<Pusher>,
    ) -> Trace {
        self.trace_shape(start, end, mins, maxs, mask, statics, skip, false)
    }

    #[expect(clippy::too_many_arguments)]
    pub(super) fn trace_shape(
        &self,
        start: Vec3,
        end: Vec3,
        mins: Vec3,
        maxs: Vec3,
        mask: u32,
        statics: bool,
        skip: Option<Pusher>,
        boxed: bool,
    ) -> Trace {
        let (skip_model, skip_copy) = match skip {
            Some(Pusher::Model(m)) => (Some(m), None),
            Some(Pusher::Copy(e)) => (None, Some(e)),
            None => (None, None),
        };
        if boxed {
            // A wide capsule's spheres can reach past the box's top and
            // floor ([`Capsule::wide`]).
            let (c, h) = ((mins + maxs) * 0.5, ((maxs - mins) * 0.5).max_element());
            sweep(start, end, c - 2.0 * h, c + 2.0 * h);
        } else {
            sweep(start, end, mins, maxs);
        }
        let mut trace = Trace::clear(end);

        let mut scratch = Vec::new();
        let capsule = if boxed {
            Capsule::wide(mins, maxs)
        } else {
            Capsule::of(mins, maxs)
        };
        self.trace_world_clip(start, end, mins, maxs, mask, &mut trace);
        let sw = TraceInput {
            start,
            end,
            mins,
            maxs,
            capsule,
            mask,
            skip: skip_model,
            world_only: false,
            boxed,
            sphere: capsule.offset,
            sight: false,
        };
        self.walk_tree(sw, &mut trace);
        // Retail's static clip writes its own end point (`cod_lnxded`
        // 0x8051a5b..0x8051ab9), as an entity's does.
        let world_fraction = trace.fraction;
        if statics {
            self.clip_statics(&sw, &mut trace);
        }
        self.trace_posed(
            start,
            end,
            mins,
            maxs,
            capsule,
            mask,
            skip_model,
            boxed,
            &mut trace,
            &mut scratch,
        );
        self.trace_copies(start, end, mins, maxs, mask, skip_copy, boxed, &mut trace);
        self.trace_instances(
            start,
            end,
            if boxed {
                Shape::Box(mins, maxs)
            } else {
                Shape::Capsule(capsule, mins, maxs)
            },
            mask,
            skip_copy,
            &mut trace,
        );
        let arith = self.arith();
        trace.endpos = if arith != Arith::Msvc && trace.fraction < world_fraction {
            sv_trace_endpos(start, end, trace.fraction, arith)
        } else {
            cm_endpos(start, end, mins, maxs, trace.fraction, arith)
        };
        trace
    }
}

impl CollisionWorld {
    /// A one-line name for what a trace hit, for reports.
    pub fn describe(&self, prim: Prim) -> String {
        match prim {
            Prim::Brush(b) => {
                let br = &self.brushes[b as usize];
                format!(
                    "brush {} {} cf={:#x}",
                    br.bsp_index, br.material, br.content_flags
                )
            }
            Prim::Tri(t) => format!(
                "terrain tri {t} sf={:#x} cf={:#x}",
                self.tris_surf[t as usize], self.tris_contents[t as usize]
            ),
            Prim::Terrain(p) => format!("terrain partition {p}"),
            Prim::Patch(p) => {
                let patch = &self.patches[p as usize];
                format!(
                    "patch {} sf={:#x} cf={:#x}",
                    patch.index, patch.surface_flags, patch.content_flags
                )
            }
            Prim::Model(t) => {
                let mt = &self.model_tris[t as usize];
                format!(
                    "xmodel tri {t} cf={:#x} sf={:#x} at {:?} in its model's frame",
                    mt.contents, mt.surface_flags, mt.tri[0]
                )
            }
            Prim::Static(p) => format!("static model {p}"),
            Prim::Body(e) => format!("body {e}"),
            Prim::Clip(cf) => format!("world clip cf={cf:#x}"),
        }
    }
}
