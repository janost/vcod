//! Where a thread's queries reached: while [`recorded`] runs, every trace,
//! position test and contents query grows a box by the volume it swept,
//! so a caller can tell which placements its answer rode on. Off, a query
//! pays one thread-local read. And the movable half of a world's collision
//! as boxes ([`ClipState`]), whose changes between two snapshots say which
//! recorded answers still hold (the nav build's walk memo,
//! docs/research/bot-navigation.md 2.5.1).

use super::CollisionWorld;
use super::Prim;
use glam::Vec3;
use std::cell::Cell;
use std::collections::BTreeMap;
use std::sync::atomic::Ordering;

/// An axial box, `lo > hi` on every axis when empty.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Reach {
    pub lo: Vec3,
    pub hi: Vec3,
}

impl Reach {
    pub const EMPTY: Reach = Reach {
        lo: Vec3::INFINITY,
        hi: Vec3::NEG_INFINITY,
    };

    pub const ALL: Reach = Reach {
        lo: Vec3::NEG_INFINITY,
        hi: Vec3::INFINITY,
    };

    pub fn is_empty(&self) -> bool {
        self.lo.cmpgt(self.hi).any()
    }

    pub fn union(self, o: Reach) -> Reach {
        Reach {
            lo: self.lo.min(o.lo),
            hi: self.hi.max(o.hi),
        }
    }

    /// Whether the two boxes, each grown by `pad`, overlap.
    pub fn meets(&self, o: &Reach, pad: f32) -> bool {
        !self.is_empty()
            && !o.is_empty()
            && (self.lo - pad).cmple(o.hi).all()
            && (o.lo - pad).cmple(self.hi).all()
    }
}

thread_local! {
    /// Whether a recording is in force, and its box so far.
    static REACH: Cell<(bool, Reach)> = const { Cell::new((false, Reach::EMPTY)) };
}

/// Runs `f` and returns what it answered and the box every collision query
/// it made on this thread swept. Nests: an outer recording takes the
/// inner's box too.
pub fn recorded<R>(f: impl FnOnce() -> R) -> (R, Reach) {
    struct Restore((bool, Reach));
    impl Drop for Restore {
        fn drop(&mut self) {
            let (on, outer) = self.0;
            REACH.with(|r| {
                let (_, inner) = r.get();
                r.set((on, if on { outer.union(inner) } else { outer }));
            });
        }
    }
    let restore = Restore(REACH.with(|r| r.replace((true, Reach::EMPTY))));
    let out = f();
    let (_, inner) = REACH.with(|r| r.get());
    drop(restore);
    (out, inner)
}

/// Adds `b` to the recording in force, if any: what a caller answering
/// from a box it recorded earlier owes the recording around it.
pub fn reach(b: Reach) {
    REACH.with(|r| {
        let (on, now) = r.get();
        if on {
            r.set((true, now.union(b)));
        }
    });
}

/// A box `mins..maxs` swept from `start` to `end`.
pub(super) fn sweep(start: Vec3, end: Vec3, mins: Vec3, maxs: Vec3) {
    REACH.with(|r| {
        let (on, now) = r.get();
        if on {
            let b = Reach {
                lo: start.min(end) + mins,
                hi: start.max(end) + maxs,
            };
            r.set((true, now.union(b)));
        }
    });
}

/// One movable thing a trace may meet, by what names it across snapshots.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Item {
    /// The arithmetic every trace runs as.
    Arith,
    /// A lump-27 submodel: its link, pose, entity and contents.
    Model(u32),
    /// Every world copy together: each is the whole world's clip.
    Copies,
    /// One entity's submodel instance.
    Instance(u32),
}

/// The movable half of a world's collision at one moment: per [`Item`], the
/// bits that say how it clips and the box it can clip in.
#[derive(Clone, Debug, Default)]
pub struct ClipState {
    items: BTreeMap<Item, (Vec<u32>, Reach)>,
}

impl ClipState {
    /// The boxes in which a query can answer differently over `now` than
    /// over `self`: each item that changed, appeared or went, where it
    /// clipped before and where it clips now.
    pub fn changed(&self, now: &ClipState) -> Vec<Reach> {
        let mut out = Vec::new();
        for (k, (bits, b)) in &self.items {
            match now.items.get(k) {
                Some((nb, nr)) if nb == bits && nr == b => {}
                Some((_, nr)) => out.extend([*b, *nr]),
                None => out.push(*b),
            }
        }
        for (k, (_, b)) in &now.items {
            if !self.items.contains_key(k) {
                out.push(*b);
            }
        }
        out.retain(|b| !b.is_empty());
        out
    }
}

impl CollisionWorld {
    /// Per submodel, the box its spawn-placed prims (brushes, terrain
    /// partitions, patches) cover; empty for one with none.
    fn model_boxes(&self) -> Vec<Reach> {
        let mut boxes = vec![Reach::EMPTY; self.model_count()];
        for &(prim, lo, hi) in &self.prims {
            let m = match prim {
                Prim::Brush(b) => self.brushes[b as usize].model,
                Prim::Terrain(p) => self.terrain_model[p as usize],
                Prim::Patch(p) => self.patches[p as usize].model,
                _ => 0,
            } as usize;
            if let Some(b) = boxes.get_mut(m) {
                *b = b.union(Reach { lo, hi });
            }
        }
        boxes
    }

    /// The links, poses, entity numbers, contents, world copies and model
    /// instances as they stand, with where each clips.
    pub fn clip_state(&self) -> ClipState {
        let bits = |v: Vec3| v.to_array().map(f32::to_bits);
        let mut items = BTreeMap::new();
        items.insert(
            Item::Arith,
            (vec![self.arith.load(Ordering::Acquire) as u32], Reach::ALL),
        );
        let boxes = self.model_boxes();
        let poses = self.poses.read().unwrap_or_else(|e| e.into_inner()).clone();
        for (m, spawn) in boxes.into_iter().enumerate().skip(1) {
            let contents = self.model_contents[m].load(Ordering::Relaxed);
            let mut b = vec![
                self.model_linked(m) as u32,
                self.model_entity[m].load(Ordering::Relaxed),
                contents as u32,
                (contents >> 32) as u32,
            ];
            let mut at = spawn;
            if let Some((_, p)) = poses.iter().find(|(pm, _)| *pm == m) {
                b.extend(bits(p.origin));
                p.axis.iter().for_each(|&a| b.extend(bits(a)));
                at = at.union(Reach { lo: p.lo, hi: p.hi });
            }
            items.insert(Item::Model(m as u32), (b, at));
        }
        let copies = self.copies.read().unwrap_or_else(|e| e.into_inner());
        if !copies.is_empty() {
            let mut b = Vec::new();
            for c in copies.iter() {
                b.extend([c.entity, c.contents]);
                b.extend(bits(c.origin));
                c.axis.iter().flatten().for_each(|&a| b.extend(bits(a)));
            }
            items.insert(Item::Copies, (b, Reach::ALL));
        }
        drop(copies);
        for i in self
            .instances
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
        {
            let mut b = vec![i.model as u32, i.contents];
            b.extend(bits(i.pose.origin));
            i.pose.axis.iter().for_each(|&a| b.extend(bits(a)));
            let at = Reach {
                lo: i.pose.lo,
                hi: i.pose.hi,
            };
            items.insert(Item::Instance(i.entity), (b, at));
        }
        ClipState { items }
    }
}
