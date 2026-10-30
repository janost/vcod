//! Contains routines ported from the Quake III Arena GPL source, Copyright (C) 1999-2005 Id Software, Inc..
//! See NOTICE.
//!
//! Retail's world trace order: `CM_TraceThroughTree` (`cod_lnxded`
//! 0x8055fe0) walks the trace through model 0's BSP tree near side first,
//! and each leaf it enters clips the brushes it lists, then the lump-24
//! partitions it lists, each once (0x8055608). A patch only lists in some
//! leaves, and a patch hit tests its unpushed fraction against the trace's,
//! so which leaf comes first decides some answers
//! (docs/research/bsp-ibsp59-format.md, "The leaf walk").

use crate::mapworld::{MapWorld, Partition};

const SURFACE_CLIP_EPSILON: f64 = 0.125;
/// The walk's offset at a slanted plane for anything but a point
/// (rodata 0x80cd434).
const NON_AXIAL_OFFSET: f64 = 2048.0;

/// One thing a leaf lists, as an index into the collision world's own
/// brushes, terrain partitions or patches.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Item {
    Brush(u32),
    Terrain(u32),
    Patch(u32),
}

#[derive(Clone, Debug)]
struct TreeNode {
    normal: [f32; 3],
    dist: f32,
    /// 0..3 for a plane along +x, +y, +z, else 3 (Q3's `PlaneTypeForNormal`).
    kind: u8,
    children: [i32; 2],
}

/// One thing a leaf lists and its box, which a sweep has to meet before
/// it clips it.
#[derive(Clone, Copy, Debug)]
pub struct Listing {
    pub item: Item,
    pub lo: [f32; 3],
    pub hi: [f32; 3],
}

/// Model 0's BSP tree and what each leaf lists, brushes first.
#[derive(Clone, Debug)]
pub struct LeafWalk {
    nodes: Vec<TreeNode>,
    /// Per leaf, its run in `items`.
    leaves: Vec<(u32, u32)>,
    /// Per leaf, how many of its items are brushes.
    brush_counts: Vec<u32>,
    items: Vec<Listing>,
}

/// [`LeafWalk::listed`]'s answer, indexed as lump 24's terrain and patches.
#[derive(Clone, Debug)]
pub struct Listed {
    pub terrain: Vec<bool>,
    pub patches: Vec<bool>,
}

/// A leaf the walk enters and the trace fraction it enters at.
#[derive(Clone, Copy, Debug)]
pub struct Entered {
    pub leaf: u32,
    pub from: f32,
}

impl LeafWalk {
    /// `brush[i]`, `terrain[i]` and `patch[i]` are lump 4's, the terrain's
    /// and the patches' `i` as the world built them, `None` where it kept
    /// none; `bounds` gives each its box. `None` for a map without a tree.
    pub fn new(
        bsp: &MapWorld,
        brush: &[Option<u32>],
        terrain: &[Option<u32>],
        patch: &[Option<u32>],
        bounds: impl Fn(Item) -> ([f32; 3], [f32; 3]),
    ) -> Option<Self> {
        if bsp.nodes.is_empty() || bsp.leaf_lists.len() != bsp.leafs.len() {
            return None;
        }
        let nodes = bsp
            .nodes
            .iter()
            .map(|n| {
                let p = &bsp.planes[n.plane as usize];
                TreeNode {
                    normal: p.normal,
                    dist: p.dist,
                    kind: (0..3).find(|&i| p.normal[i] == 1.0).unwrap_or(3) as u8,
                    children: n.children,
                }
            })
            .collect();
        let get = |v: &[Option<u32>], i: u32| v.get(i as usize).copied().flatten();
        let mut leaves = Vec::with_capacity(bsp.leaf_lists.len());
        let mut items = Vec::new();
        let mut brush_counts = Vec::with_capacity(bsp.leaf_lists.len());
        for lists in &bsp.leaf_lists {
            let at = items.len() as u32;
            let (first, count) = lists.brushes;
            for &b in &bsp.leaf_brushes[first as usize..][..count as usize] {
                items.extend(get(brush, b).map(Item::Brush));
            }
            brush_counts.push(items.len() as u32 - at);
            let (first, count) = lists.partitions;
            for part in &bsp.leaf_partitions[first as usize..][..count as usize] {
                items.extend(match *part {
                    Partition::Terrain(i) => get(terrain, i).map(Item::Terrain),
                    Partition::Patch(i) => get(patch, i).map(Item::Patch),
                });
            }
            leaves.push((at, items.len() as u32 - at));
        }
        let items = items
            .into_iter()
            .map(|item| {
                let (lo, hi) = bounds(item);
                Listing { item, lo, hi }
            })
            .collect();
        Some(LeafWalk {
            nodes,
            leaves,
            brush_counts,
            items,
        })
    }

    /// Per lump-24 terrain partition and patch of `bsp`, whether some leaf
    /// lists it; `None` for a map without a tree. mp_carentan's patches 69,
    /// 109, 283 and 284 and mp_brecourt's terrain partition 385 are in no
    /// list, so no trace clips them.
    pub fn listed(bsp: &MapWorld) -> Option<Listed> {
        if bsp.nodes.is_empty() || bsp.leaf_lists.len() != bsp.leafs.len() {
            return None;
        }
        let mut listed = Listed {
            terrain: vec![false; bsp.terrain.len()],
            patches: vec![false; bsp.patches.len()],
        };
        for lists in &bsp.leaf_lists {
            let (first, count) = lists.partitions;
            for part in &bsp.leaf_partitions[first as usize..][..count as usize] {
                match *part {
                    Partition::Terrain(i) => listed.terrain[i as usize] = true,
                    Partition::Patch(i) => listed.patches[i as usize] = true,
                }
            }
        }
        Some(listed)
    }

    /// What leaf `leaf` lists, brushes first, each list in its lump's order.
    pub fn items(&self, leaf: u32) -> &[Listing] {
        let (first, count) = self.leaves[leaf as usize];
        &self.items[first as usize..][..count as usize]
    }

    /// Whether leaf `leaf` lists partition `item`.
    pub fn lists(&self, leaf: u32, item: Item) -> bool {
        let skip = self.brush_counts[leaf as usize] as usize;
        self.items(leaf)[skip..].iter().any(|l| l.item == item)
    }

    /// Whether every leaf the box `lo..hi` reaches lists partition `item`.
    pub fn box_lists(&self, lo: [f32; 3], hi: [f32; 3], item: Item) -> bool {
        let mut stack = vec![0i32];
        while let Some(num) = stack.pop() {
            if num < 0 {
                if !self.lists((-(num + 1)) as u32, item) {
                    return false;
                }
                continue;
            }
            let node = &self.nodes[num as usize];
            // The box's nearest and farthest corners along the normal.
            let (mut near, mut far) = (0.0f64, 0.0f64);
            for i in 0..3 {
                let n = f64::from(node.normal[i]);
                let (a, b) = (n * f64::from(lo[i]), n * f64::from(hi[i]));
                near += a.min(b);
                far += a.max(b);
            }
            let d = f64::from(node.dist);
            if far >= d {
                stack.push(node.children[0]);
            }
            if near < d {
                stack.push(node.children[1]);
            }
        }
        true
    }

    /// The leaf holding point `p` (Q3's `CM_PointLeafnum`).
    pub fn leaf_at(&self, p: [f32; 3]) -> u32 {
        let mut num = 0i32;
        while num >= 0 {
            let node = &self.nodes[num as usize];
            let d = if node.kind < 3 {
                p[node.kind as usize] - node.dist
            } else {
                node.normal[0] * p[0] + node.normal[1] * p[1] + node.normal[2] * p[2] - node.dist
            };
            num = node.children[usize::from(d < 0.0)];
        }
        (-(num + 1)) as u32
    }

    /// The leaves a trace from `start` to `end` with half `extents` about
    /// them enters, in the walk's order; `point` is retail's `isPoint`, a
    /// zero box. The walk splits the segment at each node as
    /// `CM_TraceThroughTree` does. `visit` clips a leaf entered at a
    /// fraction and returns the trace's fraction after it; as retail's,
    /// the walk drops any node or leaf entered at or past that fraction.
    pub fn visit(
        &self,
        start: [f32; 3],
        end: [f32; 3],
        extents: [f32; 3],
        point: bool,
        mut fraction: f32,
        mut visit: impl FnMut(u32, f32) -> f32,
    ) {
        let mut stack = vec![(0i32, 0.0f32, 1.0f32, start, end)];
        while let Some((mut num, p1f, p2f, p1, p2)) = stack.pop() {
            if fraction <= p1f {
                continue;
            }
            // An axial node the sweep stays on one side of passes it
            // straight to that child, without the stack.
            let node = loop {
                if num < 0 {
                    break None;
                }
                let node = &self.nodes[num as usize];
                if node.kind < 3 {
                    let k = node.kind as usize;
                    let d = f64::from(node.dist);
                    let (t1, t2) = (f64::from(p1[k]) - d, f64::from(p2[k]) - d);
                    let offset = f64::from(extents[k]);
                    if t1 >= offset + 1.0 && t2 >= offset + 1.0 {
                        num = node.children[0];
                        continue;
                    }
                    if t1 < -offset - 1.0 && t2 < -offset - 1.0 {
                        num = node.children[1];
                        continue;
                    }
                }
                break Some(node);
            };
            let Some(node) = node else {
                fraction = visit((-(num + 1)) as u32, p1f);
                continue;
            };
            let d = f64::from(node.dist);
            let (t1, t2, offset) = if node.kind < 3 {
                let k = node.kind as usize;
                (
                    f64::from(p1[k]) - d,
                    f64::from(p2[k]) - d,
                    f64::from(extents[k]),
                )
            } else {
                let dot = |p: [f32; 3]| {
                    (0..3)
                        .map(|i| f64::from(node.normal[i]) * f64::from(p[i]))
                        .sum::<f64>()
                };
                (
                    dot(p1) - d,
                    dot(p2) - d,
                    if point { 0.0 } else { NON_AXIAL_OFFSET },
                )
            };
            if t1 >= offset + 1.0 && t2 >= offset + 1.0 {
                stack.push((node.children[0], p1f, p2f, p1, p2));
                continue;
            }
            if t1 < -offset - 1.0 && t2 < -offset - 1.0 {
                stack.push((node.children[1], p1f, p2f, p1, p2));
                continue;
            }
            let (side, frac, frac2) = if t1 < t2 {
                let idist = 1.0 / (t1 - t2);
                (
                    1,
                    (t1 - offset + SURFACE_CLIP_EPSILON) * idist,
                    (t1 + offset + SURFACE_CLIP_EPSILON) * idist,
                )
            } else if t1 > t2 {
                let idist = 1.0 / (t1 - t2);
                (
                    0,
                    (t1 + offset + SURFACE_CLIP_EPSILON) * idist,
                    (t1 - offset - SURFACE_CLIP_EPSILON) * idist,
                )
            } else {
                (0, 1.0, 0.0)
            };
            let cut = |f: f64| -> (f32, [f32; 3]) {
                let f = f.clamp(0.0, 1.0);
                let at = (f64::from(p1f) + (f64::from(p2f) - f64::from(p1f)) * f) as f32;
                let mid = std::array::from_fn(|i| {
                    (f64::from(p1[i]) + f * (f64::from(p2[i]) - f64::from(p1[i]))) as f32
                });
                (at, mid)
            };
            let (midf, mid) = cut(frac);
            let (midf2, mid2) = cut(frac2);
            // The far side goes on first, so the near side is walked first.
            stack.push((node.children[side ^ 1], midf2, p2f, mid2, p2));
            stack.push((node.children[side], p1f, midf, p1, mid));
        }
    }

    /// Every leaf [`Self::visit`] enters with nothing clipped, and the
    /// fraction it enters at.
    pub fn walk(
        &self,
        start: [f32; 3],
        end: [f32; 3],
        extents: [f32; 3],
        point: bool,
    ) -> Vec<Entered> {
        let mut out = Vec::new();
        self.visit(start, end, extents, point, 1.0, |leaf, from| {
            out.push(Entered { leaf, from });
            1.0
        });
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One plane at x = 0, its front leaf 0 and its back leaf 1.
    fn split() -> LeafWalk {
        LeafWalk {
            nodes: vec![TreeNode {
                normal: [1.0, 0.0, 0.0],
                dist: 0.0,
                kind: 0,
                children: [-1, -2],
            }],
            leaves: vec![(0, 0), (0, 0)],
            brush_counts: vec![0, 0],
            items: vec![],
        }
    }

    #[test]
    fn the_near_side_comes_first_and_the_far_side_starts_short_of_the_plane() {
        let w = split();
        let order = |s, e| {
            w.walk(s, e, [0.0; 3], true)
                .iter()
                .map(|e| (e.leaf, e.from))
                .collect::<Vec<_>>()
        };
        // The far side is entered 0.125 units before the plane.
        let far = (10.0 - 0.125) / 20.0;
        assert_eq!(
            order([10.0, 0.0, 0.0], [-10.0, 0.0, 0.0]),
            [(0, 0.0), (1, far)]
        );
        assert_eq!(
            order([-10.0, 0.0, 0.0], [10.0, 0.0, 0.0]),
            [(1, 0.0), (0, far)]
        );
        // A unit clear of the plane stays on its side.
        assert_eq!(order([1.0, 0.0, 0.0], [10.0, 0.0, 0.0]), [(0, 0.0)]);
    }
}
