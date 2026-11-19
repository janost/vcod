//! A box tree over the static models, so a point trace tests only the
//! models whose box its segment can reach. The walk it feeds still visits
//! them in index order, so the result is the flat walk's.

use glam::Vec3;

use super::{StaticModel, box_misses};

/// Models a leaf holds at most.
const LEAF: usize = 8;

#[derive(Clone, Debug)]
struct Node {
    lo: Vec3,
    hi: Vec3,
    /// A leaf's run in `order`, or an inner node's right child (its left
    /// child is the next node) with `count` 0.
    first: u32,
    count: u32,
}

#[derive(Clone, Debug, Default)]
pub(super) struct StaticTree {
    nodes: Vec<Node>,
    order: Vec<u32>,
}

impl StaticTree {
    pub(super) fn build(models: &[StaticModel]) -> StaticTree {
        let mut t = StaticTree {
            nodes: Vec::new(),
            order: (0..models.len() as u32).collect(),
        };
        if !models.is_empty() {
            t.split(models, 0, models.len());
        }
        t
    }

    fn split(&mut self, models: &[StaticModel], from: usize, to: usize) {
        let run = &mut self.order[from..to];
        let (lo, hi) = run.iter().fold(
            (Vec3::splat(f32::INFINITY), Vec3::splat(f32::NEG_INFINITY)),
            |(lo, hi), &i| {
                let m = &models[i as usize];
                (lo.min(m.absmin), hi.max(m.absmax))
            },
        );
        let at = self.nodes.len();
        self.nodes.push(Node {
            lo,
            hi,
            first: from as u32,
            count: (to - from) as u32,
        });
        if to - from <= LEAF {
            return;
        }
        let e = hi - lo;
        let axis = if e.x >= e.y && e.x >= e.z {
            0
        } else if e.y >= e.z {
            1
        } else {
            2
        };
        let centre = |i: &u32| {
            let m = &models[*i as usize];
            m.absmin[axis] + m.absmax[axis]
        };
        let mid = (to - from) / 2;
        run.select_nth_unstable_by(mid, |a, b| centre(a).total_cmp(&centre(b)));
        self.nodes[at].count = 0;
        self.split(models, from, from + mid);
        self.nodes[at].first = self.nodes.len() as u32;
        self.split(models, from + mid, to);
    }

    /// Into `out`, ascending: every model whose box the segment meets
    /// before `fraction`, and maybe some more.
    pub(super) fn candidates(&self, start: Vec3, end: Vec3, fraction: f32, out: &mut Vec<u32>) {
        out.clear();
        if self.nodes.is_empty() {
            return;
        }
        let mut stack = vec![0usize];
        while let Some(n) = stack.pop() {
            let node = &self.nodes[n];
            if box_misses(start, end, node.lo, node.hi, fraction) {
                continue;
            }
            if node.count > 0 {
                let run = node.first as usize..(node.first + node.count) as usize;
                out.extend_from_slice(&self.order[run]);
            } else {
                stack.push(node.first as usize);
                stack.push(n + 1);
            }
        }
        out.sort_unstable();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    /// Every model whose box a segment meets is a candidate, in index
    /// order, so the walk over them is the flat walk's.
    #[test]
    fn the_candidates_hold_every_box_the_segment_meets_in_order() {
        let mut seed = 7u32;
        let mut r = move |span: f32| {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            (seed >> 8) as f32 / (1 << 24) as f32 * span
        };
        let models: Vec<StaticModel> = (0..500)
            .map(|_| {
                let lo = Vec3::new(r(4000.0), r(4000.0), r(400.0));
                let hi = lo + Vec3::new(r(200.0), r(200.0), r(100.0));
                StaticModel {
                    origin: lo,
                    inv_axis: [Vec3::X, Vec3::Y, Vec3::Z],
                    absmin: lo,
                    absmax: hi,
                    contents: 1,
                    no_collision: false,
                    surfs: Arc::new(Vec::new()),
                }
            })
            .collect();
        let tree = StaticTree::build(&models);
        let mut out = Vec::new();
        for _ in 0..2000 {
            let a = Vec3::new(r(4000.0), r(4000.0), r(500.0));
            let b = a + Vec3::new(r(2000.0) - 1000.0, r(2000.0) - 1000.0, r(200.0) - 100.0);
            let f = r(1.0);
            tree.candidates(a, b, f, &mut out);
            assert!(out.is_sorted());
            let want: Vec<u32> = (0..models.len() as u32)
                .filter(|&i| {
                    let m = &models[i as usize];
                    !box_misses(a, b, m.absmin, m.absmax, f)
                })
                .collect();
            assert!(want.iter().all(|i| out.binary_search(i).is_ok()), "{a} {b}");
        }
    }
}
