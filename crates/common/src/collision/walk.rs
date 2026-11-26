//! Contains routines ported from the Quake III Arena GPL source, Copyright (C) 1999-2005 Id Software, Inc..
//! See NOTICE.
//!
//! The BVH walk and the leaf-order replay.

use super::clip::NEAR_TIE;
use super::clip::{
    Capsule, TIE_WINDOW, capsule_sweep, clip_brush_box, clip_brush_capsule, clip_patch,
    clip_segment_model, patch_meets, sight_meets_brush,
};
use super::{CollisionWorld, Prim, Trace};
use crate::leafwalk::{Item, LeafWalk, Listing};
use crate::patch::Arith;
use crate::terrain::CapsuleSweep;
use glam::Vec3;

impl CollisionWorld {
    /// The tree's half of a trace.
    pub(super) fn walk_tree(&self, sweep: TraceInput, trace: &mut Trace) {
        if self.nodes.is_empty() {
            return;
        }
        // Brushes, then the partitions: retail's leaf walk clips a
        // leaf's brushes before its partitions (`cod_lnxded` 0x8055608),
        // and a contact at fraction 0 ends the walk, so a box resting on
        // a brush floor beside a patch reads the floor. The brush walk
        // records the leaves it enters; the partition pass visits a
        // subsequence of them in the same order (its fraction is never
        // larger), so it replays the list instead of walking again.
        let entry = *trace;
        NEAR_TIE.set(false);
        let mut walk = Walk {
            sweep,
            leaves: [0; LEAF_LOG],
            logged: 0,
            patch_met: std::cell::Cell::new(false),
            rest: false,
        };
        self.trace_node(0, &mut walk, true, trace);
        if walk.logged <= LEAF_LOG && !two_walks() {
            for &leaf in &walk.leaves[..walk.logged] {
                // A met patch throws this pass away.
                if walk.patch_met.get() {
                    break;
                }
                self.clip_leaf(leaf, &walk, false, trace);
            }
        } else {
            walk.logged = usize::MAX;
            self.trace_node(0, &mut walk, false, trace);
        }
        // A sweep that meets a patch starts over: model 0 in retail's leaf
        // order, then the submodels and the static models as before. So
        // does one whose contact nearly ties another, as a capsule standing
        // across two floor brushes, a brush and a terrain partition or two
        // partitions within the terrain's back-off do, and one stopped at
        // fraction 0 (a box always, a capsule all in solid or with a second
        // contact there), where the walk ends on whatever got there first:
        // retail's order names the contact. So does one stopped on terrain
        // no leaf on its walk lists, which retail never clips, and one
        // stopped on any model-0 brush (bsp-ibsp59-format.md, "The leaf
        // walk"). The cheap tests go first.
        let replay = walk.patch_met.get()
            || trace.fraction < 1.0
                && (NEAR_TIE.get()
                    || self.on_world_brush(trace)
                    || trace.fraction <= 0.0
                        && (sweep.boxed || trace.allsolid || self.ties_at_zero(&sweep, trace))
                    || self.terrain_off_the_walk(&sweep, trace));
        if replay && let Some(leaves) = &self.leaf_walk {
            *trace = entry;
            self.clip_leaf_walk(leaves, &sweep, trace);
            let mut rest = Walk {
                sweep,
                leaves: [0; LEAF_LOG],
                logged: usize::MAX,
                patch_met: std::cell::Cell::new(false),
                rest: true,
            };
            self.trace_node(0, &mut rest, true, trace);
            self.trace_node(0, &mut rest, false, trace);
        }
    }

    /// Whether the trace stopped on a model-0 terrain partition that no leaf
    /// the walk enters short of the contact lists. Retail clips a partition
    /// only from a leaf that lists it, so a sweep that reaches terrain from
    /// outside its leaves (out of the world, or under it) never meets it
    /// there, while the BVH does (bsp-ibsp59-format.md, "The leaf walk").
    fn terrain_off_the_walk(&self, sw: &TraceInput, trace: &Trace) -> bool {
        let (Some(Prim::Tri(t)), Some(leaves)) = (trace.hit, &self.leaf_walk) else {
            return false;
        };
        if self.tri_listed.get(t as usize).copied().unwrap_or(false) {
            return false;
        }
        let part = self.terrain.parts.partition_point(|p| p.first <= t) - 1;
        if self.terrain_model[part] != 0 {
            return false;
        }
        let item = Item::Terrain(part as u32);
        let lists = |leaf: u32| leaves.lists(leaf, item);
        // The leaf holding the contact lists the partition in the world, and
        // the walk enters it short of the contact; only a miss walks.
        let dir = sw.end - sw.start;
        let centre = sw.start + dir * trace.fraction + sw.capsule.center;
        let below = if trace.normal.z > 0.0 {
            -sw.capsule.offset
        } else {
            sw.capsule.offset
        };
        let contact = centre + below - trace.normal * sw.capsule.radius;
        if lists(leaves.leaf_at(contact.to_array())) {
            return false;
        }
        let mid = (sw.mins + sw.maxs) * 0.5;
        let mut listed = false;
        leaves.visit(
            (sw.start + mid).to_array(),
            (sw.end + mid).to_array(),
            (sw.maxs - mid).to_array(),
            sw.mins == Vec3::ZERO && sw.maxs == Vec3::ZERO,
            trace.fraction,
            |leaf, _| {
                listed |= lists(leaf);
                // A listing ends the walk.
                if listed { 0.0 } else { trace.fraction }
            },
        );
        !listed
    }

    /// Whether the trace stopped on a model-0 brush. Every such trace
    /// replays through the leaf walk, which names the brush retail's order
    /// meets first and never clips one no leaf on the walk lists
    /// (bsp-ibsp59-format.md, "The leaf walk").
    fn on_world_brush(&self, trace: &Trace) -> bool {
        matches!(trace.hit, Some(Prim::Brush(b)) if self.brushes[b as usize].model == 0)
    }

    /// Whether a capsule the BVH stopped at fraction 0 has a second contact
    /// there that retail's walk may have met first. The walk ends at 0 on
    /// whatever got there first, so the other prim was never clipped and
    /// nothing noted the tie. Each prim at the start box is clipped on its
    /// own: one reaching the tie window that reads otherwise (normal,
    /// surface, solid flags, or a brush of another model), or a model-0
    /// patch, is the tie (bsp-ibsp59-format.md, "The leaf walk").
    fn ties_at_zero(&self, sw: &TraceInput, trace: &Trace) -> bool {
        let model = |hit: Option<Prim>| match hit {
            Some(Prim::Brush(b)) => Some(self.brushes[b as usize].model),
            _ => None,
        };
        let key = |t: &Trace| {
            (
                t.normal.to_array().map(f32::to_bits),
                t.surface_flags,
                t.startsolid,
                t.allsolid,
                model(t.hit),
            )
        };
        let (lo, hi) = sw.bounds(0.0);
        let mut stack = crate::leafwalk::WalkStack::new(0u32);
        while let Some(i) = stack.pop() {
            let node = &self.nodes[i as usize];
            if !sw.statics && i == self.models_root
                || !(lo.cmple(node.hi).all() && hi.cmpge(node.lo).all())
            {
                continue;
            }
            if node.count == 0 {
                stack.push(node.first);
                stack.push(node.second);
                continue;
            }
            let first = node.first as usize;
            for &(prim, plo, phi) in &self.prims[first..first + node.count as usize] {
                if !(lo.cmple(phi).all() && hi.cmpge(plo).all()) || Some(prim) == trace.hit {
                    continue;
                }
                if let Prim::Patch(p) = prim
                    && self.leaf_walk.is_some()
                    && self.patches[p as usize].model == 0
                {
                    let patch = &self.patches[p as usize];
                    if patch.content_flags & sw.mask != 0
                        && patch_meets(sw.start, sw.end, sw.capsule, patch, self.arith())
                    {
                        return true;
                    }
                    continue;
                }
                let mut alone = Trace::clear(sw.end);
                self.clip_prim(prim, sw, &mut alone);
                if (alone.startsolid || f64::from(alone.fraction) <= TIE_WINDOW)
                    && key(&alone) != key(trace)
                {
                    return true;
                }
            }
        }
        false
    }
}

impl CollisionWorld {
    /// Nearest child first; the sweep box shrinks with `trace.fraction`, so
    /// prims past the current hit and its tie window are never clipped.
    /// Clips the brushes or everything else, and logs each leaf it enters
    /// while `walk.logged` is not `usize::MAX`.
    pub(super) fn trace_node(&self, i: u32, walk: &mut Walk, brushes: bool, trace: &mut Trace) {
        let sw = &walk.sweep;
        // The brush walk enters the static models' subtree only to log it
        // for the partition pass of a trace that clips them.
        if !sw.statics && i == self.models_root {
            return;
        }
        let node = &self.nodes[i as usize];
        if walk.rest && !node.rest {
            return;
        }
        let (lo, hi) = sw.reach(trace.fraction);
        if !(lo.cmple(node.hi).all() && hi.cmpge(node.lo).all())
            || (!node.loose && sw.misses(node.lo, node.hi))
        {
            return;
        }
        if node.count > 0 {
            if walk.logged != usize::MAX {
                if let Some(slot) = walk.leaves.get_mut(walk.logged) {
                    *slot = i;
                }
                walk.logged += 1;
            }
            self.clip_leaf(i, walk, brushes, trace);
            return;
        }
        let (start, dir) = (sw.start, sw.end - sw.start);
        let along = |n: u32| {
            let node = &self.nodes[n as usize];
            ((node.lo + node.hi) * 0.5 - start).dot(dir)
        };
        let (near, far) = if along(node.first) <= along(node.second) {
            (node.first, node.second)
        } else {
            (node.second, node.first)
        };
        for n in [near, far] {
            self.trace_node(n, walk, brushes, trace);
        }
    }

    /// Leaf `i`'s brushes, or its other prims, against `sw`; nothing when
    /// the leaf's box misses the sweep cut at the current fraction.
    fn clip_leaf(&self, i: u32, walk: &Walk, brushes: bool, trace: &mut Trace) {
        let sw = &walk.sweep;
        let node = &self.nodes[i as usize];
        let (lo, hi) = sw.reach(trace.fraction);
        if !(lo.cmple(node.hi).all() && hi.cmpge(node.lo).all()) {
            return;
        }
        let first = node.first as usize;
        for (prim, plo, phi) in &self.prims[first..first + node.count as usize] {
            // Retail's leaf walk stops once the fraction reaches 0
            // (`cod_lnxded` 0x8055608, 0x8055fe0).
            if trace.fraction <= 0.0 {
                return;
            }
            // The node test once more per prim: a leaf's box is the union
            // of up to four, and setting up a brush's planes costs far
            // more than this.
            if !(lo.cmple(*phi).all() && hi.cmpge(*plo).all())
                || brushes != matches!(prim, Prim::Brush(_))
                || (TraceInput::tight(*prim) && sw.misses(*plo, *phi))
            {
                continue;
            }
            match *prim {
                // The leaf walk has clipped model 0.
                Prim::Brush(b) if walk.rest && self.brushes[b as usize].model == 0 => continue,
                Prim::Terrain(p) if walk.rest && self.terrain_model[p as usize] == 0 => continue,
                Prim::Patch(p) if walk.rest && self.patches[p as usize].model == 0 => continue,
                // Model 0's patches clip only from the leaves that list them,
                // and a point's patch clip compares its unpushed fraction, so
                // its answer rides the order too: the leaf walk clips them.
                Prim::Patch(p)
                    if !walk.rest
                        && self.leaf_walk.is_some()
                        && self.patches[p as usize].model == 0 =>
                {
                    let patch = &self.patches[p as usize];
                    if patch.content_flags & sw.mask != 0
                        && patch_meets(sw.start, sw.end, sw.capsule, patch, self.arith())
                    {
                        walk.patch_met.set(true);
                        return;
                    }
                }
                prim => self.clip_prim(prim, sw, trace),
            }
        }
    }

    /// One prim against `sw`, whatever its box: the mask and link tests,
    /// then its clip.
    fn clip_prim(&self, prim: Prim, sw: &TraceInput, trace: &mut Trace) {
        let TraceInput {
            start,
            end,
            mins,
            maxs,
            capsule,
            mask,
            statics,
            skip,
            world_only,
            boxed,
            sphere,
            sight,
        } = *sw;
        match prim {
            Prim::Brush(b) => {
                let brush = &self.brushes[b as usize];
                if brush.content_flags & mask == 0
                    || !self.brush_linked(brush)
                    || !self.brush_at_spawn(brush)
                    || skip == Some(brush.model as usize)
                    || (world_only && brush.model != 0)
                {
                    return;
                }
                let same_model = matches!(trace.hit,
                    Some(Prim::Brush(o)) if self.brushes[o as usize].model == brush.model);
                if sight {
                    if let Some(f) = sight_meets_brush(start, end, &brush.planes)
                        && f < trace.fraction
                    {
                        trace.fraction = f;
                        trace.hit = Some(prim);
                    }
                } else if boxed {
                    clip_brush_box(
                        trace,
                        start,
                        end,
                        mins,
                        maxs,
                        &brush.planes,
                        &brush.side_surface_flags,
                        prim,
                        self.arith(),
                        same_model,
                    );
                } else {
                    let sweep = capsule_sweep(start, end, mins, maxs);
                    clip_brush_capsule(
                        trace,
                        &sweep,
                        sphere,
                        &brush.planes,
                        &brush.side_surface_flags,
                        prim,
                        self.arith(),
                        same_model,
                    );
                }
            }
            Prim::Terrain(p) => {
                if !self.model_clips(self.terrain_model[p as usize], sw) {
                    return;
                }
                self.clip_terrain(p, start, end, mins, maxs, capsule, mask, trace);
            }
            Prim::Tri(_) => unreachable!("the BVH holds partitions"),
            Prim::Patch(p) => {
                let patch = &self.patches[p as usize];
                if patch.content_flags & mask == 0 || !self.model_clips(patch.model, sw) {
                    return;
                }
                clip_patch(trace, start, end, capsule, patch, self.arith(), prim);
            }
            Prim::Model(t) => {
                let mt = &self.model_tris[t as usize];
                if !statics || mt.contents & mask == 0 {
                    return;
                }
                // `SV_Trace` clips the static models after the world and
                // before the entities, each pass taking only a nearer
                // fraction, so a static model wins a tie with a submodel
                // and loses one with model 0 (bsp-ibsp59-format.md, "The
                // leaf walk").
                let entity_hit = match trace.hit {
                    Some(Prim::Brush(b)) => self.brushes[b as usize].model != 0,
                    Some(Prim::Terrain(p)) => self.terrain_model[p as usize] != 0,
                    Some(Prim::Patch(p)) => self.patches[p as usize].model != 0,
                    _ => false,
                };
                clip_segment_model(trace, start, end, mt, prim, entity_hit);
            }
            // Never stored in world.prims; only movetrace.rs and the world
            // clip construct them.
            Prim::Body(_) | Prim::Clip(_) => unreachable!(),
        }
    }

    /// Model 0 in retail's order, for a sweep that meets a patch: each leaf
    /// `CM_TraceThroughTree` enters short of the fraction clips the brushes
    /// it lists, then its partitions, each prim once (`cod_lnxded`
    /// 0x8055fe0, 0x8055608). A patch takes a facet only where its unpushed
    /// fraction is within the trace's, so a brush or patch met in an
    /// earlier leaf can keep it out (bsp-ibsp59-format.md, "The leaf walk").
    pub(super) fn clip_leaf_walk(&self, leaves: &LeafWalk, sw: &TraceInput, trace: &mut Trace) {
        thread_local! {
            /// Retail's check counts: the walk's number per item it clipped.
            static SEEN: std::cell::RefCell<(u32, Vec<u32>)> =
                const { std::cell::RefCell::new((0, Vec::new())) };
        }
        let (nb, nt) = (self.brushes.len(), self.terrain.parts.len());
        let slots = nb + nt + self.patches.len();
        let mid = (sw.mins + sw.maxs) * 0.5;
        SEEN.with_borrow_mut(|(walk, seen)| {
            if seen.len() < slots || *walk == u32::MAX {
                *seen = vec![0; slots];
                *walk = 0;
            }
            *walk += 1;
            let walk = *walk;
            leaves.visit(
                (sw.start + mid).to_array(),
                (sw.end + mid).to_array(),
                (sw.maxs - mid).to_array(),
                sw.mins == Vec3::ZERO && sw.maxs == Vec3::ZERO,
                trace.fraction,
                |leaf, _| {
                    let (mut lo, mut hi) = sw.bounds(trace.fraction);
                    for &Listing {
                        item,
                        lo: plo,
                        hi: phi,
                    } in leaves.items(leaf)
                    {
                        // The BVH's box test: a prim clear of the sweep cut
                        // at the fraction can neither start it solid nor cut
                        // it shorter, here or in a later leaf.
                        if !(lo.cmple(Vec3::from_array(phi)).all()
                            && hi.cmpge(Vec3::from_array(plo)).all())
                        {
                            continue;
                        }
                        let (slot, prim) = match item {
                            Item::Brush(b) => (b as usize, Prim::Brush(b)),
                            Item::Terrain(p) => (nb + p as usize, Prim::Terrain(p)),
                            Item::Patch(p) => (nb + nt + p as usize, Prim::Patch(p)),
                        };
                        if seen[slot] == walk {
                            continue;
                        }
                        seen[slot] = walk;
                        self.clip_prim(prim, sw, trace);
                        if trace.fraction <= 0.0 {
                            break;
                        }
                        (lo, hi) = sw.bounds(trace.fraction);
                    }
                    trace.fraction
                },
            );
        });
    }

    /// Terrain partition `part` against a sweep (docs/research/cod11-mantle.md,
    /// "Terrain is a swept sphere, a patch is a facet"): retail's capsule clip
    /// (`terrain.rs`), or for a point trace its point arm (0x8052894) on each
    /// triangle. A trace already at fraction 0 has stopped, as retail's tree
    /// walk does.
    #[expect(clippy::too_many_arguments)]
    pub(super) fn clip_terrain(
        &self,
        part: u32,
        start: Vec3,
        end: Vec3,
        mins: Vec3,
        maxs: Vec3,
        capsule: Capsule,
        mask: u32,
        trace: &mut Trace,
    ) {
        let tp = self.terrain.parts[part as usize];
        let first = tp.first as usize;
        if self.tris_contents[first] & mask == 0 || trace.fraction <= 0.0 {
            return;
        }
        let surface_flags = self.tris_surf[first];
        if capsule.radius == 0.0 {
            let (s, e) = (start + capsule.center, end + capsule.center);
            if !tp.touches_segment(s.to_array(), e.to_array(), self.arith()) {
                return;
            }
            let (s, e) = (s.to_array(), e.to_array());
            let hit = if self.arith() == Arith::Stored {
                self.terrain.point_clip_stored(&tp, s, e, trace.fraction)
            } else {
                self.terrain.point_clip(&tp, s, e, trace.fraction)
            };
            if let Some((f, normal, t)) = hit {
                trace.fraction = f;
                trace.enter = f;
                trace.normal = Vec3::from_array(normal);
                trace.surface_flags = surface_flags;
                trace.hit = Some(Prim::Tri(t));
            }
            return;
        }
        let sweep = CapsuleSweep {
            radius: capsule.radius,
            ..capsule_sweep(start, end, mins, maxs)
        };
        if !tp.touches(&sweep) {
            return;
        }
        if start == end {
            if self
                .terrain
                .position_test(&tp, &sweep, self.arith() == Arith::Stored)
            {
                trace.startsolid = true;
                trace.allsolid = true;
                trace.fraction = 0.0;
                trace.normal = Vec3::ZERO;
                trace.surface_flags = surface_flags;
                trace.hit = Some(Prim::Terrain(part));
            }
            return;
        }
        if let Some(h) =
            self.terrain
                .clip_capsule(&tp, &sweep, trace.fraction, self.arith() == Arith::Stored)
        {
            trace.fraction = h.fraction;
            trace.enter = h.fraction + crate::terrain::FRACTION_EPS;
            trace.normal = Vec3::from_array(h.normal);
            trace.surface_flags = surface_flags;
            trace.hit = Some(Prim::Tri(h.tri));
            trace.startsolid |= h.startsolid;
        }
    }
}

impl CollisionWorld {
    /// Only the tests call this now; `box_trace` walks the BVH itself via `trace_node`.
    #[cfg(test)]
    pub(super) fn candidates(&self, lo: Vec3, hi: Vec3, out: &mut Vec<Prim>) {
        if self.nodes.is_empty() {
            return;
        }
        // the root is nodes[0]: the one tree's root, or with static models
        // the synthetic node over the world's and the models' subtrees
        let mut stack = vec![0u32];
        while let Some(i) = stack.pop() {
            let node = &self.nodes[i as usize];
            if !(lo.cmple(node.hi).all() && hi.cmpge(node.lo).all()) {
                continue;
            }
            if node.count > 0 {
                let start = node.first as usize;
                for (prim, _, _) in &self.prims[start..start + node.count as usize] {
                    out.push(*prim);
                }
            } else {
                stack.push(node.first);
                stack.push(node.second);
            }
        }
    }
}

/// A trace's fixed inputs, as `trace_node` and `clip_leaf` read them.
#[derive(Clone, Copy)]
pub(super) struct TraceInput {
    pub(super) start: Vec3,
    pub(super) end: Vec3,
    pub(super) mins: Vec3,
    pub(super) maxs: Vec3,
    pub(super) capsule: Capsule,
    pub(super) mask: u32,
    pub(super) statics: bool,
    pub(super) skip: Option<usize>,
    /// Model 0's brushes and the terrain alone: a world copy's clip.
    pub(super) world_only: bool,
    /// Brushes take the box rather than the capsule: `trap_Trace`'s box, which
    /// retail fakes as a capsule only against terrain (0x80536c8).
    pub(super) boxed: bool,
    /// The capsule's sphere offset against brushes: `capsule.offset`, turned
    /// into a turned world copy's frame. Terrain and patches keep the
    /// upright offset, as retail's do.
    pub(super) sphere: Vec3,
    /// A point sight trace: brushes take [`sight_meets_brush`] in place of
    /// the box clip.
    pub(super) sight: bool,
}

impl TraceInput {
    /// A prim nothing meets off its box, so [`Self::misses`] may cull it:
    /// a model triangle or a terrain partition. A model-0 patch's leaf-walk
    /// clip met mp_carentan's patch 9 56 units outside its box, and a
    /// brush notes a near tie (`note_contact`) for a segment 160 units off
    /// its box, which sends the trace through retail's leaf order: culling
    /// either changed which surface a trace named.
    pub(super) fn tight(prim: Prim) -> bool {
        matches!(prim, Prim::Model(_) | Prim::Terrain(_))
    }

    /// The box the sweep covers up to `fraction`, padded a unit.
    fn bounds(&self, fraction: f32) -> (Vec3, Vec3) {
        let cur_end = self.start + (self.end - self.start) * fraction;
        (
            self.start.min(cur_end) + self.mins - Vec3::ONE,
            self.start.max(cur_end) + self.maxs + Vec3::ONE,
        )
    }

    /// [`Self::bounds`] past `fraction` by the tie window: a prim whose
    /// contact lands just past the hit cannot cut the trace, but clipped
    /// it notes the near tie that sends the sweep through retail's leaf
    /// order (`note_contact`), where it may be met first.
    fn reach(&self, fraction: f32) -> (Vec3, Vec3) {
        self.bounds((f64::from(fraction) + TIE_WINDOW).min(1.0) as f32)
    }

    /// Whether a long sweep's whole segment, `start` to `end`, passes
    /// clear of the box `lo..hi` grown by the swept shape and a margin, so
    /// nothing in it can touch the trace. The margin (a unit, a hundredth
    /// of the box's size for a model triangle's barycentric slack, the
    /// turned capsule's sphere offset) covers every clip's epsilon; the
    /// box test of [`Self::reach`] stays the one that cuts by fraction. A
    /// short sweep's box already hugs it and is never tested: this only
    /// spares a long ray (the 15000-unit look-at trace, a sight line) the
    /// prims its box takes in off its line. Only [`Self::tight`] prims
    /// are culled.
    fn misses(&self, lo: Vec3, hi: Vec3) -> bool {
        const LONG: f32 = 256.0;
        let d = self.end - self.start;
        if d.length_squared() < LONG * LONG {
            return false;
        }
        let pad = Vec3::splat(1.0 + 0.01 * (hi - lo).max_element() + self.sphere.length());
        let (lo, hi) = (lo - self.maxs - pad, hi - self.mins + pad);
        let (mut t0, mut t1) = (0.0f32, 1.0f32);
        for a in 0..3 {
            let s = self.start[a];
            if d[a] == 0.0 {
                if s < lo[a] || s > hi[a] {
                    return true;
                }
                continue;
            }
            let (ta, tb) = ((lo[a] - s) / d[a], (hi[a] - s) / d[a]);
            t0 = t0.max(ta.min(tb));
            t1 = t1.min(ta.max(tb));
            if t0 > t1 {
                return true;
            }
        }
        false
    }
}

#[cfg(test)]
thread_local! {
    /// Set by a test: the partition pass walks the tree again, as it did
    /// before the leaf log, to hold the replay to the same answer.
    pub(super) static TWO_WALKS: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

fn two_walks() -> bool {
    #[cfg(test)]
    return TWO_WALKS.get();
    #[cfg(not(test))]
    false
}

/// The leaves a brush walk logs for its partition pass; a walk that enters
/// more walks the tree again instead.
pub(super) const LEAF_LOG: usize = 64;

pub(super) struct Walk {
    pub(super) sweep: TraceInput,
    pub(super) leaves: [u32; LEAF_LOG],
    /// Leaves entered so far (past `LEAF_LOG` when some went unlogged);
    /// `usize::MAX` stops the logging.
    pub(super) logged: usize,
    /// Set when the sweep meets a patch of model 0, which `walk_tree`
    /// then clips in retail's leaf order instead.
    pub(super) patch_met: std::cell::Cell<bool>,
    /// After that leaf walk: clips only what it does not, the submodels'
    /// brushes and the static models.
    pub(super) rest: bool,
}
