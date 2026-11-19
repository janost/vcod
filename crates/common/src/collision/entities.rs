//! What a script moves: model instances and poses, world copies, links and entity numbers.

use super::clip::{
    Capsule, Shape, capsule_sweep, clip_brush_capsule, clip_segment, expand_brush, expand_brush_box,
};
use super::masks::NO_ENTITY_CONTENTS;
use super::walk::TraceInput;
use super::{BrushPlanes, CollisionWorld, Prim, Pusher, Trace};
use crate::net::protocol::{ENTITYNUM_NONE, ENTITYNUM_WORLD};
use glam::Vec3;
use std::sync::atomic::Ordering;

/// One entity's copy of a submodel: its pose and `r.contents`, which a
/// trace's mask must meet before the brushes are clipped.
#[derive(Clone, Copy, Debug)]
pub(super) struct ModelInstance {
    entity: u32,
    model: usize,
    pose: ModelPose,
    contents: u32,
}

/// A submodel copy out of the traces for a while
/// ([`CollisionWorld::unlink_model_instance`]).
#[derive(Debug)]
pub struct UnlinkedInstance(ModelInstance);

/// One world copy: its entity, pose and `r.contents`, which a trace's mask
/// must meet before the copy is clipped at all (`SV_ClipMoveToEntities`
/// lists entities by contents).
#[derive(Clone, Copy, Debug)]
pub(super) struct WorldCopy {
    pub(super) entity: u32,
    pub(super) origin: Vec3,
    /// `AnglesToAxis`, local to world; `None` unrotated.
    pub(super) axis: Option<[Vec3; 3]>,
    pub(super) contents: u32,
}

impl WorldCopy {
    /// [`ModelPose::to_local`]'s turn, with no spawn origin.
    pub(super) fn to_local(self, p: Vec3) -> Vec3 {
        let d = p - self.origin;
        match self.axis {
            Some(axis) => Vec3::from(axis.map(|a| crate::x87::dot(a, d) as f32)),
            None => d,
        }
    }

    /// `CM_TransformedBoxTrace`'s sphere offset in this copy's frame
    /// (`cod_lnxded` 0x80568d8): x and z along the forward and up axes as a
    /// point turns, y along the right axis where a point takes the left.
    pub(super) fn sphere(self, offset: Vec3) -> Vec3 {
        match self.axis {
            Some([f, l, u]) => Vec3::new(f.z, -l.z, u.z) * offset.z,
            None => offset,
        }
    }

    pub(super) fn to_world_dir(self, n: Vec3) -> Vec3 {
        match self.axis {
            Some([f, l, u]) => f * n.x + l * n.y + u * n.z,
            None => n,
        }
    }
}

/// One model's brushes as built: their run in `CollisionWorld::brushes`, the
/// spawn origin their planes carry, and their bounds relative to it.
#[derive(Clone, Copy, Debug, Default)]
pub(super) struct ModelSpan {
    pub(super) brushes: (u32, u32),
    pub(super) origin: Vec3,
    pub(super) lo: Vec3,
    pub(super) hi: Vec3,
}

/// Where a submodel's brushes are now: its entity's origin and the rotation
/// its angles make, retail's `SV_ClipMoveToEntities` transform of an inline
/// model by `r.currentOrigin` and `r.currentAngles`, and the world bounds
/// that leaves them in.
#[derive(Clone, Copy, Debug)]
pub(super) struct ModelPose {
    origin: Vec3,
    /// `AnglesToAxis`: forward, left, up, the columns of local to world.
    axis: [Vec3; 3],
    /// Any angle non-zero: `CM_TransformedBoxTrace` then turns the sweep
    /// into the model's frame instead of moving the brushes.
    turned: bool,
    pub(super) lo: Vec3,
    pub(super) hi: Vec3,
}

impl ModelPose {
    fn rotate(&self, v: Vec3) -> Vec3 {
        self.axis[0] * v.x + self.axis[1] * v.y + self.axis[2] * v.z
    }

    /// A world point in the frame the brushes were built in: turned back
    /// about the pose's origin and put at the spawn origin. Each dot stays
    /// on the x87 stack and is stored once (`cod_lnxded` 0x8056a7d..
    /// 0x8056bc6); at ten thousand units out a rounded product moves the
    /// point a few thousandths (movers doc 22).
    fn to_local(self, p: Vec3, spawn: Vec3) -> Vec3 {
        let d = p - self.origin;
        Vec3::from(self.axis.map(|a| crate::x87::dot(a, d) as f32)) + spawn
    }

    /// A plane of the brush as built, `n·p <= d` at the spawn `origin`, moved
    /// to this pose: `(R n)·q <= d - n·origin + (R n)·self.origin`.
    pub(super) fn plane(&self, (n, d): (Vec3, f32), spawn: Vec3) -> (Vec3, f32) {
        let rn = self.rotate(n);
        (rn, d - n.dot(spawn) + rn.dot(self.origin))
    }
}

impl CollisionWorld {
    /// Every placement a trace reads beyond the static lumps and the link
    /// bits, as bits in stored order: the arithmetic, the posed submodels,
    /// the world copies and the submodel instances (vehicles). Two worlds
    /// over the same lumps and links trace alike when these match.
    pub fn placements(&self) -> Vec<u32> {
        fn eat(out: &mut Vec<u32>, v: Vec3) {
            out.extend(v.to_array().map(f32::to_bits));
        }
        fn pose(out: &mut Vec<u32>, p: &ModelPose) {
            eat(out, p.origin);
            p.axis.iter().for_each(|&a| eat(out, a));
        }
        let mut out = vec![self.arith.load(Ordering::Acquire) as u32];
        for (m, p) in self.poses.read().unwrap_or_else(|e| e.into_inner()).iter() {
            out.push(*m as u32);
            pose(&mut out, p);
        }
        for c in self.copies.read().unwrap_or_else(|e| e.into_inner()).iter() {
            out.extend([u32::MAX, c.entity, c.contents]);
            eat(&mut out, c.origin);
            c.axis.iter().flatten().for_each(|&a| eat(&mut out, a));
        }
        for i in self
            .instances
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
        {
            out.extend([u32::MAX - 1, i.entity, i.model as u32, i.contents]);
            pose(&mut out, &i.pose);
        }
        out
    }

    /// Puts model `model`'s brushes where an entity at `origin` with
    /// `angles` holds them: a script mover's brush model follows its entity
    /// (docs/research/cod11-movers.md, section 11). The spawn placement, the
    /// entity lump's origin and no rotation, takes the pose off again. Model
    /// 0 and an index past the model count are ignored.
    pub fn set_model_pose(&self, model: usize, origin: Vec3, angles: Vec3) {
        let Some(span) = self.model_span.get(model).copied() else {
            return;
        };
        if model == 0 {
            return;
        }
        let mut poses = self.poses.write().unwrap_or_else(|e| e.into_inner());
        poses.retain(|(m, _)| *m != model);
        let spawn = origin == span.origin && angles == Vec3::ZERO;
        self.model_posed[model].store(!spawn, Ordering::Relaxed);
        if spawn {
            self.any_posed.store(!poses.is_empty(), Ordering::Release);
            return;
        }
        poses.push((model, Self::model_pose(span, origin, angles)));
        self.any_posed.store(true, Ordering::Release);
    }

    /// Model `span` at `origin` and `angles`, with the world box it then
    /// covers.
    fn model_pose(span: ModelSpan, origin: Vec3, angles: Vec3) -> ModelPose {
        let mut pose = ModelPose {
            origin,
            axis: crate::x87::rotation_matrix(angles),
            turned: angles != Vec3::ZERO,
            lo: Vec3::splat(f32::MAX),
            hi: Vec3::splat(f32::MIN),
        };
        for i in 0..8 {
            let corner = Vec3::new(
                if i & 1 == 0 { span.lo.x } else { span.hi.x },
                if i & 2 == 0 { span.lo.y } else { span.hi.y },
                if i & 4 == 0 { span.lo.z } else { span.hi.z },
            );
            let w = pose.rotate(corner) + origin;
            pose.lo = pose.lo.min(w);
            pose.hi = pose.hi.max(w);
        }
        pose
    }

    /// Puts entity `entity`'s own copy of submodel `model` at `origin` and
    /// `angles` with `r.contents` `contents`, or takes it out on `None`. A
    /// trace reports the entity on its brushes, and a `Pusher::Copy` of the
    /// entity passes them. Model 0 and an index past the model count are
    /// ignored.
    pub fn set_model_instance(&self, entity: u32, model: usize, pose: Option<(Vec3, Vec3, u32)>) {
        let Some(span) = self.model_span.get(model).copied() else {
            return;
        };
        if model == 0 {
            return;
        }
        let mut instances = self.instances.write().unwrap_or_else(|e| e.into_inner());
        instances.retain(|i| i.entity != entity);
        if let Some((origin, angles, contents)) = pose {
            instances.push(ModelInstance {
                entity,
                model,
                pose: Self::model_pose(span, origin, angles),
                contents,
            });
        }
        self.any_instance
            .store(!instances.is_empty(), Ordering::Release);
    }

    /// Takes entity `entity`'s submodel copy out of the traces, as
    /// `trap_UnlinkEntity` would, for [`Self::relink_model_instance`] to
    /// put back. `None` when the entity has none.
    pub fn unlink_model_instance(&self, entity: u32) -> Option<UnlinkedInstance> {
        let mut instances = self.instances.write().unwrap_or_else(|e| e.into_inner());
        let i = instances.iter().position(|i| i.entity == entity)?;
        let inst = instances.remove(i);
        self.any_instance
            .store(!instances.is_empty(), Ordering::Release);
        Some(UnlinkedInstance(inst))
    }

    /// Puts back a copy [`Self::unlink_model_instance`] took out, where it
    /// stood.
    pub fn relink_model_instance(&self, unlinked: UnlinkedInstance) {
        let mut instances = self.instances.write().unwrap_or_else(|e| e.into_inner());
        instances.retain(|i| i.entity != unlinked.0.entity);
        instances.push(unlinked.0);
        self.any_instance.store(true, Ordering::Release);
    }

    /// Lump 27's model count, the world's model 0 included.
    pub fn model_count(&self) -> usize {
        self.model_span.len()
    }

    /// Back to the spawn placement, as a map load or restart has it.
    pub fn reset_model_pose(&self, model: usize) {
        if let Some(span) = self.model_span.get(model) {
            self.set_model_pose(model, span.origin, Vec3::ZERO);
        }
    }

    /// Puts entity `entity`'s copy of the world clip at `origin` and
    /// `angles` with `r.contents` `contents`, or takes it out on `None`: a
    /// brush entity whose inline model is 0 (movers doc 19). Retail clips
    /// it through `CM_TransformedBoxTrace` on the world tree; vcod turns
    /// the sweep into the copy's frame and keeps its box axial there.
    pub fn set_world_copy(&self, entity: u32, pose: Option<(Vec3, Vec3, i32)>) {
        let mut copies = self.copies.write().unwrap_or_else(|e| e.into_inner());
        copies.retain(|c| c.entity != entity);
        if let Some((origin, angles, contents)) = pose {
            let axis = (angles != Vec3::ZERO).then(|| crate::x87::rotation_matrix(angles));
            copies.push(WorldCopy {
                entity,
                origin,
                axis,
                contents: contents as u32,
            });
        }
        self.any_copy.store(!copies.is_empty(), Ordering::Release);
    }

    /// A `r.contents` write on entity `entity`'s world copy, which stays
    /// where it is (`solid` / `notSolid`).
    pub fn set_world_copy_contents(&self, entity: u32, contents: i32) {
        let mut copies = self.copies.write().unwrap_or_else(|e| e.into_inner());
        for c in copies.iter_mut().filter(|c| c.entity == entity) {
            c.contents = contents as u32;
        }
    }

    /// Takes every world copy out, as a map load or restart frees their
    /// entities.
    pub fn clear_world_copies(&self) {
        self.copies
            .write()
            .unwrap_or_else(|e| e.into_inner())
            .clear();
        self.any_copy.store(false, Ordering::Release);
        self.instances
            .write()
            .unwrap_or_else(|e| e.into_inner())
            .clear();
        self.any_instance.store(false, Ordering::Release);
    }

    /// A box sweep against model `model`'s brushes alone, wherever its pose
    /// has them, under `mask`: the pusher test of `G_MoverPush`, which asks
    /// with a plain `trap_Trace` box under the entity's own clipmask
    /// (game.mp 0x554ed) whether it is in the mover's way rather than in
    /// anything's.
    pub fn model_box_trace(
        &self,
        model: usize,
        start: Vec3,
        end: Vec3,
        mins: Vec3,
        maxs: Vec3,
        mask: u32,
    ) -> Trace {
        let mut trace = Trace::clear(end);
        if model < self.model_span.len() && model != 0 && self.model_linked(model) {
            let pose = self
                .poses
                .read()
                .unwrap_or_else(|e| e.into_inner())
                .iter()
                .find(|(m, _)| *m == model)
                .map(|(_, p)| *p);
            let (mut a, mut b) = (Vec::new(), Vec::new());
            self.clip_model(
                model,
                pose.as_ref(),
                start,
                end,
                Shape::Box(mins, maxs),
                mask,
                &mut trace,
                &mut a,
                &mut b,
            );
        }
        trace.endpos = start + (end - start) * trace.fraction;
        trace
    }

    /// [`Self::model_box_trace`] against either kind of pusher. A world copy
    /// is clipped through the world tree as `trap_Trace` clips it: its
    /// brushes take the box, its terrain and patches the capsule (movers doc
    /// 21: a capsule under a sloped ceiling met it two frames late).
    pub fn pusher_box_trace(
        &self,
        pusher: Pusher,
        start: Vec3,
        end: Vec3,
        mins: Vec3,
        maxs: Vec3,
        mask: u32,
    ) -> Trace {
        let entity = match pusher {
            Pusher::Model(m) => return self.model_box_trace(m, start, end, mins, maxs, mask),
            Pusher::Copy(e) => e,
        };
        let copy = self
            .copies
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .find(|c| c.entity == entity)
            .copied();
        let mut trace = match copy.filter(|c| c.contents & mask != 0) {
            Some(c) => self.clip_copy(c, start, end, mins, maxs, mask, true),
            None => Trace::clear(end),
        };
        trace.endpos = start + (end - start) * trace.fraction;
        trace
    }

    /// Clips a sweep against one model's brushes, moved by `pose` when there
    /// is one and where the build left them otherwise. As the leaf trace a
    /// submodel's sweep goes to (1.1d 0x8055608, UO 0x8059875), the brushes
    /// clip in lump order and a brush that brings the fraction to 0 ends the
    /// model: a box it meets at 0 never reads a later brush all solid
    /// (bsp-ibsp59-format.md, "The leaf walk").
    #[expect(clippy::too_many_arguments)]
    pub(super) fn clip_model(
        &self,
        model: usize,
        pose: Option<&ModelPose>,
        start: Vec3,
        end: Vec3,
        shape: Shape,
        mask: u32,
        trace: &mut Trace,
        moved: &mut Vec<(Vec3, f32)>,
        scratch: &mut Vec<(Vec3, f32)>,
    ) {
        let span = self.model_span[model];
        if let Some(pose) = pose.filter(|p| p.turned) {
            self.clip_turned_model(model, pose, start, end, shape, mask, trace, scratch);
            return;
        }
        let moving = trace.fraction > 0.0;
        for b in span.brushes.0..span.brushes.1 {
            let brush = &self.brushes[b as usize];
            if brush.content_flags & mask == 0 {
                continue;
            }
            if moving && trace.fraction <= 0.0 {
                break;
            }
            let planes = match pose {
                Some(pose) => {
                    moved.clear();
                    moved.extend(brush.planes.iter().map(|&pl| pose.plane(pl, span.origin)));
                    &moved[..]
                }
                None => &brush.planes[..],
            };
            let (centre, sphere) = match shape {
                Shape::Capsule(c, _, _) => {
                    expand_brush(planes, c.radius, scratch);
                    (c.center, c.offset)
                }
                Shape::Box(mins, maxs) => {
                    expand_brush_box(planes, mins, maxs, scratch);
                    (Vec3::ZERO, Vec3::ZERO)
                }
            };
            clip_segment(
                trace,
                start + centre,
                end + centre,
                scratch,
                &brush.side_surface_flags,
                sphere,
                Prim::Brush(b),
            );
        }
    }

    /// [`Self::clip_model`] for a turned pose, as `CM_TransformedBoxTrace`
    /// serves an inline model with any angle set (`cod_lnxded` 0x80568d8,
    /// called from 0x8090f18 with `r.currentAngles`): the sweep's centre
    /// turned into the model's frame, the box axial there, the capsule's
    /// sphere offset turned, and the brushes clipped as built (movers doc
    /// 22). The normal comes back in the world's frame.
    #[expect(clippy::too_many_arguments)]
    fn clip_turned_model(
        &self,
        model: usize,
        pose: &ModelPose,
        start: Vec3,
        end: Vec3,
        shape: Shape,
        mask: u32,
        trace: &mut Trace,
        scratch: &mut Vec<(Vec3, f32)>,
    ) {
        let span = self.model_span[model];
        let (mins, maxs) = match shape {
            Shape::Capsule(_, mins, maxs) | Shape::Box(mins, maxs) => (mins, maxs),
        };
        let mid = (mins + maxs) * 0.5;
        let (ls, le) = (
            pose.to_local(start + mid, span.origin) - mid,
            pose.to_local(end + mid, span.origin) - mid,
        );
        let moving = trace.fraction > 0.0;
        for b in span.brushes.0..span.brushes.1 {
            let brush = &self.brushes[b as usize];
            if brush.content_flags & mask == 0 {
                continue;
            }
            if moving && trace.fraction <= 0.0 {
                break;
            }
            match shape {
                Shape::Capsule(c, _, _) => {
                    let [f, l, u] = pose.axis;
                    let sphere = Vec3::new(f.z, -l.z, u.z) * c.offset.z;
                    let sweep = capsule_sweep(ls, le, mins, maxs);
                    clip_brush_capsule(
                        trace,
                        &sweep,
                        sphere,
                        &brush.planes,
                        &brush.side_surface_flags,
                        Prim::Brush(b),
                        self.arith(),
                        false,
                    );
                }
                Shape::Box(_, _) => {
                    expand_brush_box(&brush.planes, mins, maxs, scratch);
                    clip_segment(
                        trace,
                        ls,
                        le,
                        scratch,
                        &brush.side_surface_flags,
                        Vec3::ZERO,
                        Prim::Brush(b),
                    );
                }
            }
        }
        // One call per model and trace, so a hit on its brushes is this one's.
        if matches!(trace.hit, Some(Prim::Brush(b)) if (span.brushes.0..span.brushes.1).contains(&b))
        {
            trace.normal = pose.rotate(trace.normal);
        }
    }

    /// The posed models' half of a trace: the BVH skipped their brushes.
    #[expect(clippy::too_many_arguments)]
    pub(super) fn trace_posed(
        &self,
        start: Vec3,
        end: Vec3,
        mins: Vec3,
        maxs: Vec3,
        capsule: Capsule,
        mask: u32,
        skip: Option<usize>,
        boxed: bool,
        trace: &mut Trace,
        scratch: &mut Vec<(Vec3, f32)>,
    ) {
        if !self.any_posed.load(Ordering::Acquire) {
            return;
        }
        let poses = self.poses.read().unwrap_or_else(|e| e.into_inner());
        let mut moved = Vec::new();
        for (model, pose) in poses.iter() {
            let cur_end = start + (end - start) * trace.fraction;
            let lo = start.min(cur_end) + mins - Vec3::ONE;
            let hi = start.max(cur_end) + maxs + Vec3::ONE;
            if !(lo.cmple(pose.hi).all() && hi.cmpge(pose.lo).all())
                || !self.model_linked(*model)
                || skip == Some(*model)
            {
                continue;
            }
            self.clip_model(
                *model,
                Some(pose),
                start,
                end,
                if boxed {
                    Shape::Box(mins, maxs)
                } else {
                    Shape::Capsule(capsule, mins, maxs)
                },
                mask,
                trace,
                &mut moved,
                scratch,
            );
        }
    }

    /// The entity whose link puts model `model`'s brushes in the clip, the
    /// `script_brushmodel` or other `*N` entity the map load spawned.
    pub fn set_model_entity(&self, model: usize, entity: u32) {
        if let Some(m) = self.model_entity.get(model) {
            m.store(entity, Ordering::Relaxed);
        }
    }

    /// The `r.contents` model `model`'s entity linked with: -1 for any
    /// `trap_SetBrushModel` entity no script gave other contents.
    pub fn set_model_contents(&self, model: usize, contents: i32) {
        if let Some(m) = self.model_contents.get(model) {
            m.store(u64::from(contents as u32), Ordering::Relaxed);
        }
    }

    /// The contents a round's trace (`trap_LocationalTrace`) reports for
    /// what it stopped on: a submodel's brushes report their entity's
    /// `r.contents` (-1 for a door, so the round goes on past it as past
    /// glass, cod11-combat.md 2.4, step 3); anything else
    /// [`Self::hit_contents`].
    pub fn shot_contents(&self, trace: &Trace) -> u32 {
        let model = match trace.hit {
            Some(Prim::Brush(b)) if trace.copy.is_none() => self.brushes[b as usize].model,
            Some(Prim::Patch(p)) if trace.copy.is_none() => self.patches[p as usize].model,
            _ => 0,
        };
        match self.model_contents.get(model as usize) {
            Some(c) if model != 0 && c.load(Ordering::Relaxed) != NO_ENTITY_CONTENTS => {
                c.load(Ordering::Relaxed) as u32
            }
            _ => self.hit_contents(trace),
        }
    }

    fn model_entity_num(&self, model: u32) -> u32 {
        self.model_entity
            .get(model as usize)
            .map_or(ENTITYNUM_WORLD, |m| m.load(Ordering::Relaxed))
    }

    /// Q3's `trace.entityNum`: the entity the reported contact belongs to,
    /// `ENTITYNUM_WORLD` for world geometry and `ENTITYNUM_NONE` for a trace
    /// that hit nothing.
    pub fn entity_num(&self, trace: &Trace) -> u32 {
        if let Some(e) = trace.copy {
            return e;
        }
        match trace.hit {
            Some(Prim::Brush(b)) => self
                .model_entity
                .get(self.brushes[b as usize].model as usize)
                .map_or(ENTITYNUM_WORLD, |m| m.load(Ordering::Relaxed)),
            Some(Prim::Body(e)) => e,
            Some(Prim::Patch(p)) => self.model_entity_num(self.patches[p as usize].model),
            Some(Prim::Terrain(p)) => self.model_entity_num(self.terrain_model[p as usize]),
            Some(Prim::Tri(t)) => {
                let parts = &self.terrain.parts;
                let part = parts.partition_point(|p| p.first <= t).saturating_sub(1);
                self.model_entity_num(self.terrain_model.get(part).copied().unwrap_or(0))
            }
            Some(_) => ENTITYNUM_WORLD,
            None if trace.fraction < 1.0 => ENTITYNUM_WORLD,
            None => ENTITYNUM_NONE,
        }
    }

    /// The contents word of what a trace stopped on, the locational trace's
    /// `+32` (docs/research/cod11-combat.md 3.2): a brush's material word, a
    /// prop surface's own, a triangle's kept bits. 0 for a miss and for a
    /// body, whose contents live on its entity.
    pub fn hit_contents(&self, trace: &Trace) -> u32 {
        match trace.hit {
            Some(Prim::Brush(b)) => self.brushes[b as usize].content_flags,
            Some(Prim::Tri(t)) => self.tris_contents[t as usize],
            Some(Prim::Patch(p)) => self.patches[p as usize].content_flags,
            Some(Prim::Model(t)) => self.model_tris[t as usize].contents,
            Some(Prim::Clip(contents)) => contents,
            Some(Prim::Terrain(_) | Prim::Body(_)) | None => 0,
        }
    }

    /// Whether model `model`'s brushes clip. The server clears it when the
    /// `script_brushmodel` that links them is deleted, the way retail's
    /// `G_FreeEntity` unlinks (docs/research/cod11-mantle.md, "A submodel's
    /// brushes are its entity's").
    pub fn set_model_linked(&self, model: usize, linked: bool) {
        if let Some(m) = self.model_linked.get(model) {
            m.store(linked, Ordering::Relaxed);
        }
    }

    /// Reads [`set_model_linked`](Self::set_model_linked) back. An index past
    /// the model count is `true`, matching the trace's own treatment of one.
    pub fn model_linked(&self, model: usize) -> bool {
        self.model_linked
            .get(model)
            .is_none_or(|m| m.load(Ordering::Relaxed))
    }

    /// Whether model `model`'s brushes clip where the BSP put them: linked
    /// and not moved off its spawn pose. Model 0 always.
    pub fn model_in_place(&self, model: usize) -> bool {
        self.model_linked(model)
            && !self
                .model_posed
                .get(model)
                .is_some_and(|m| m.load(Ordering::Relaxed))
    }

    /// Unlinks the brush models the stock map-load scripts take out of the
    /// clip, for a world no script and no snapshot drives (the gates; the
    /// client takes them from the snapshot, `pmove::movers`):
    /// `_gameobjects::main` `delete()`s every entity whose
    /// `script_gameobjectname` the gametype does not list (`dm.gsc:78`,
    /// `tdm.gsc:78`: their own name; `sd.gsc:123`: `sd`, `bombzone`,
    /// `blocker`), and `_load.gsc` `notsolid()`s every exploder brush model.
    pub fn unlink_script_brushes(&self, entities: &str, gametype: &str) {
        let allowed: &[&str] = match gametype {
            "sd" => &["sd", "bombzone", "blocker"],
            "re" => &["re", "retrieval"],
            g => &[g][..],
        };
        for block in crate::bsp::entity_blocks(entities) {
            let deleted = block
                .get("script_gameobjectname")
                .is_some_and(|name| !allowed.contains(&name.as_str()));
            let exploder = block
                .get("classname")
                .is_some_and(|c| c == "script_brushmodel")
                && block.contains_key("script_exploder")
                && block
                    .get("targetname")
                    .is_some_and(|t| t == "exploder" || t == "exploderchunk");
            if !deleted && !exploder {
                continue;
            }
            if let Some(n) = block
                .get("model")
                .and_then(|m| m.strip_prefix('*'))
                .and_then(|n| n.parse::<usize>().ok())
            {
                self.set_model_linked(n, false);
            }
        }
    }

    /// Whether a partition model `model` owns clips `sw`, as its brushes
    /// would: linked, where it spawned, and not the sweep's pass entity. A
    /// posed submodel's partitions are not clipped at all.
    pub(super) fn model_clips(&self, model: u32, sw: &TraceInput) -> bool {
        model == 0
            || (!sw.world_only
                && sw.skip != Some(model as usize)
                && self.model_linked(model as usize)
                && !self.model_posed[model as usize].load(Ordering::Relaxed))
    }

    pub(super) fn brush_linked(&self, brush: &BrushPlanes) -> bool {
        self.model_linked(brush.model as usize)
    }

    /// Whether the BVH's copy of this brush is where the brush is: false for
    /// a posed model's, which [`Self::trace_posed`] clips instead.
    pub(super) fn brush_at_spawn(&self, brush: &BrushPlanes) -> bool {
        !self.model_posed[brush.model as usize].load(Ordering::Relaxed)
    }
}

impl CollisionWorld {
    /// The model instances' half of a trace, each clipped where its entity
    /// stands and merged as [`Self::trace_copies`] merges a copy's.
    pub(super) fn trace_instances(
        &self,
        start: Vec3,
        end: Vec3,
        shape: Shape,
        mask: u32,
        skip: Option<u32>,
        trace: &mut Trace,
    ) {
        if !self.any_instance.load(Ordering::Acquire) {
            return;
        }
        let (mins, maxs) = match shape {
            Shape::Box(lo, hi) | Shape::Capsule(_, lo, hi) => (lo, hi),
        };
        let instances = self.instances.read().unwrap_or_else(|e| e.into_inner());
        let (mut moved, mut scratch) = (Vec::new(), Vec::new());
        for inst in instances.iter() {
            if inst.contents & mask == 0 || trace.allsolid || skip == Some(inst.entity) {
                continue;
            }
            let lo = start.min(end) + mins - Vec3::ONE;
            let hi = start.max(end) + maxs + Vec3::ONE;
            if !(lo.cmple(inst.pose.hi).all() && hi.cmpge(inst.pose.lo).all()) {
                continue;
            }
            let mut t = Trace::clear(end);
            self.clip_model(
                inst.model,
                Some(&inst.pose),
                start,
                end,
                shape,
                mask,
                &mut t,
                &mut moved,
                &mut scratch,
            );
            t.copy = (t.fraction < 1.0 || t.startsolid).then_some(inst.entity);
            if t.allsolid || t.fraction < trace.fraction {
                let startsolid = trace.startsolid || t.startsolid;
                *trace = Trace { startsolid, ..t };
            } else if t.startsolid {
                trace.startsolid = true;
            }
        }
    }
}

impl CollisionWorld {
    /// The world copies' half of a trace, each clipped on its own in its
    /// frame and merged as Q3's `SV_ClipMoveToEntities` merges an entity's
    /// trace: it wins when allsolid or strictly nearer, and a copy the sweep
    /// starts in marks the result startsolid either way.
    #[expect(clippy::too_many_arguments)]
    pub(super) fn trace_copies(
        &self,
        start: Vec3,
        end: Vec3,
        mins: Vec3,
        maxs: Vec3,
        mask: u32,
        skip: Option<u32>,
        boxed: bool,
        trace: &mut Trace,
    ) {
        if !self.any_copy.load(Ordering::Acquire) {
            return;
        }
        let copies = self.copies.read().unwrap_or_else(|e| e.into_inner());
        for c in copies.iter() {
            if c.contents & mask == 0 || trace.allsolid || skip == Some(c.entity) {
                continue;
            }
            let t = self.clip_copy(*c, start, end, mins, maxs, mask, boxed);
            if t.allsolid || t.fraction < trace.fraction {
                let startsolid = trace.startsolid || t.startsolid;
                *trace = Trace { startsolid, ..t };
            } else if t.startsolid {
                trace.startsolid = true;
            }
        }
    }

    /// One world copy alone against a sweep, in its frame; the normal comes
    /// back in the world's, `endpos` is left to the caller.
    pub(super) fn clip_copy(
        &self,
        c: WorldCopy,
        start: Vec3,
        end: Vec3,
        mins: Vec3,
        maxs: Vec3,
        mask: u32,
        boxed: bool,
    ) -> Trace {
        // `CM_TransformedBoxTrace` turns the box's centre into the copy's
        // frame, not its origin (movers doc 21, "The turned centre").
        let mid = (mins + maxs) * 0.5;
        let (ls, le) = (c.to_local(start + mid) - mid, c.to_local(end + mid) - mid);
        let capsule = Capsule::of(mins, maxs);
        let mut t = Trace::clear(le);
        self.walk_tree(
            TraceInput {
                start: ls,
                end: le,
                mins,
                maxs,
                capsule,
                mask,
                statics: false,
                skip: None,
                world_only: true,
                boxed,
                sphere: c.sphere(capsule.offset),
            },
            &mut t,
        );
        Trace {
            normal: c.to_world_dir(t.normal),
            copy: (t.fraction < 1.0 || t.startsolid).then_some(c.entity),
            ..t
        }
    }
}
