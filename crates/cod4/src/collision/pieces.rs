//! What iw3mp.exe 1.7's world collider (0x5a8110) hands a geom's kind
//! collider, in its order, and a brush's faces as those colliders read
//! them: each face's plane, its neighbours round it and its winding
//! (0x5998e0). cod4-player-anims.md 10.6.

use glam::Vec3;
use vcod_common::clipmap::BrushNode;

use super::ClipMap;

/// The most leaves 0x5a8110 asks `CM_BoxLeafnums` (0x4f2c60) for.
const MAX_LEAFS: usize = 0x400;

/// One collide's worth of the world, in 0x5a8110's order.
#[derive(Clone, Debug, PartialEq)]
pub enum Piece {
    /// A brush of the world's list, once per leaf it is reached from
    /// (0x5a7a60 keeps no check count).
    Brush(usize),
    /// An AABB tree partition's triangles (each once, by check count) with
    /// its leaf node's material's surface flags (0x5a7dc4).
    Tris { tris: Vec<[Vec3; 3]>, surface: u32 },
}

/// A brush as the colliders read it: faces `0..6` the axial ones in
/// 0x596ae0's order (face `i` on axis `i >> 1`, its minimum side when
/// `i & 1` is 0), then the sides.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct BrushFaces {
    /// Each face's plane `(n, d)`, `n . p <= d` inside.
    pub planes: Vec<([f32; 3], f32)>,
    /// Each face's neighbours in winding order, as face indices.
    pub adjacent: Vec<Vec<u8>>,
    /// Each face's material's surface flags (0x5a7970).
    pub surface: Vec<u32>,
}

impl BrushFaces {
    /// 0x596ae0's six axial planes off `mins..maxs`: `-x <= -mins.x`,
    /// `x <= maxs.x`, then y and z.
    pub fn axial_planes(mins: [f32; 3], maxs: [f32; 3]) -> [([f32; 3], f32); 6] {
        std::array::from_fn(|i| {
            let k = i >> 1;
            let mut n = [0.0; 3];
            if i & 1 == 0 {
                n[k] = -1.0;
                (n, -mins[k])
            } else {
                n[k] = 1.0;
                (n, maxs[k])
            }
        })
    }

    /// Face `f`'s winding (0x5998e0): each corner the meet of its plane
    /// with two neighbours in turn, the last neighbour and the first
    /// meeting first. A meet whose determinant reads under 0.001 is
    /// skipped (0x70b278), a corner within a unit of the last kept one is
    /// overwritten by the next (the float 1.0 at 0x6be570 on the squared
    /// distance, 0x403310), the last goes when it is within a unit of the
    /// first, and fewer than three corners, or more than `max` neighbours,
    /// give none.
    pub fn winding(&self, f: usize, max: usize) -> Vec<[f32; 3]> {
        let adj = &self.adjacent[f];
        let n = adj.len();
        if n < 3 || n > max {
            return Vec::new();
        }
        let w = |x: f32| x as f64;
        let p = self.planes[f];
        let mut q = self.planes[adj[n - 1] as usize];
        let mut out: Vec<[f32; 3]> = Vec::with_capacity(n);
        let mut count = 0;
        for &a in adj {
            let r = self.planes[a as usize];
            let ((pn, pd), (qn, qd), (rn, rd)) = (p, q, r);
            // 0x599a04..0x599a48: the x parts of Q x R, R x P and P x Q.
            let qr_x = (w(rn[2]) * w(qn[1]) - w(qn[2]) * w(rn[1])) as f32;
            let rp_x = (w(rn[1]) * w(pn[2]) - w(rn[2]) * w(pn[1])) as f32;
            let pq_x = (w(pn[1]) * w(qn[2]) - w(pn[2]) * w(qn[1])) as f32;
            let a0 = (w(pn[0]) * w(qr_x)) as f32;
            let a1 = (w(qn[0]) * w(rp_x)) as f32;
            let a2 = (w(rn[0]) * w(pq_x)) as f32;
            let det = ((w(a1) + w(a0)) + w(a2)) as f32;
            q = r;
            if det.abs() < 0.001 {
                continue;
            }
            let inv = (1.0 / w(det)) as f32;
            let sum = |b: f32, a: f32, c: f32| (w(b) + w(a)) + w(c);
            let x = {
                let a = (w(qr_x) * w(pd)) as f32;
                let b = (w(rp_x) * w(qd)) as f32;
                let c = (w(pq_x) * w(rd)) as f32;
                (sum(b, a, c) * w(inv)) as f32
            };
            let y = {
                let a = ((w(rn[0]) * w(qn[2]) - w(qn[0]) * w(rn[2])) * w(pd)) as f32;
                let b = ((w(rn[2]) * w(pn[0]) - w(rn[0]) * w(pn[2])) * w(qd)) as f32;
                let c = ((w(qn[0]) * w(pn[2]) - w(qn[2]) * w(pn[0])) * w(rd)) as f32;
                (sum(b, a, c) * w(inv)) as f32
            };
            let z = {
                let a = ((w(qn[0]) * w(rn[1]) - w(rn[0]) * w(qn[1])) * w(pd)) as f32;
                let b = (w(qd) * (w(rn[0]) * w(pn[1]) - w(rn[1]) * w(pn[0]))) as f32;
                let c = ((w(qn[1]) * w(pn[0]) - w(qn[0]) * w(pn[1])) * w(rd)) as f32;
                (w(inv) * sum(b, a, c)) as f32
            };
            let v = [x, y, z];
            out.truncate(count);
            out.push(v);
            if count == 0 || dist_sq(out[count - 1], v) >= 1.0 {
                count += 1;
            }
        }
        out.truncate(count);
        if count > 1 && dist_sq(out[0], out[count - 1]) < 1.0 {
            out.pop();
        }
        if out.len() < 3 {
            out.clear();
        }
        out
    }
}

/// 0x403310: `|a - b|^2`, each difference and the sum stored.
fn dist_sq(a: [f32; 3], b: [f32; 3]) -> f32 {
    let d = [0, 1, 2].map(|k| (a[k] as f64 - b[k] as f64) as f32);
    let s = |x: f32| x as f64 * x as f64;
    ((s(d[0]) + s(d[1])) + s(d[2])) as f32
}

impl ClipMap {
    /// Brush `b`'s faces, their neighbours from the zone's brush edges.
    pub fn brush_faces(&self, b: usize) -> BrushFaces {
        let w = &self.world;
        let brush = &w.brushes[b];
        let mut planes: Vec<([f32; 3], f32)> =
            BrushFaces::axial_planes(brush.mins.to_array(), brush.maxs.to_array()).to_vec();
        let surf = |m: u16| w.materials.get(usize::from(m)).map_or(0, |&(s, _)| s);
        let mut surface: Vec<u32> = (0..6)
            .map(|i| surf(brush.axial_material[i & 1][i >> 1]))
            .collect();
        for &(pi, m) in &brush.sides {
            let p = w.planes[pi as usize];
            planes.push((p.normal.to_array(), p.dist));
            surface.push(surf(m));
        }
        let adjacent = self
            .brush_adjacent
            .get(b)
            .cloned()
            .unwrap_or_else(|| vec![Vec::new(); planes.len()]);
        BrushFaces {
            planes,
            adjacent,
            surface,
        }
    }

    /// The pieces of the world 0x5a8110 collides a geom centred on
    /// `centre` whose box is `lo..hi` against under `mask`: of each leaf
    /// the box touches
    /// (`CM_BoxLeafnums`, at most 0x400), the brushes its brush tree
    /// reaches (0x5a7bc0, 0x5a7a60), then its AABB trees' partitions
    /// (0x5a7ea0, 0x5a7d20).
    pub fn world_pieces(&self, centre: Vec3, lo: Vec3, hi: Vec3, mask: u32) -> Vec<Piece> {
        let w = &self.world;
        let mut out = Vec::new();
        let mut seen = vec![false; w.partitions.len()];
        let mut leafs = w.box_leafs(lo, hi);
        leafs.truncate(MAX_LEAFS);
        for l in leafs {
            let leaf = w.leafs[l];
            // 0x5a7bc0: the leaf's bounds strictly over the box's.
            if leaf.brush_contents & mask != 0
                && (0..3).all(|k| leaf.mins[k] < hi[k] && leaf.maxs[k] > lo[k])
                && leaf.brush_node != 0
            {
                self.brush_tree(leaf.brush_node as usize, lo, hi, mask, &mut out);
            }
            if leaf.terrain_contents & mask == 0 {
                continue;
            }
            let first = leaf.first_aabb as usize;
            for root in first..first + leaf.aabb_count as usize {
                let m = w.aabbs[root].material as usize;
                if w.materials.get(m).is_some_and(|&(_, c)| c & mask != 0) {
                    self.aabb_tree(root, [centre, lo, hi], &mut seen, &mut out);
                }
            }
        }
        out
    }

    /// 0x5a7a60: a node's straddlers first, then the side above `dist`,
    /// then the side below.
    fn brush_tree(&self, mut n: usize, lo: Vec3, hi: Vec3, mask: u32, out: &mut Vec<Piece>) {
        let w = &self.world;
        loop {
            let node = w.brush_nodes[n];
            if node.contents() & mask == 0 {
                return;
            }
            match node {
                BrushNode::Leaf { first, count, .. } => {
                    for &b in &w.leaf_brush_list[first as usize..][..count as usize] {
                        let brush = &w.brushes[b as usize];
                        if brush.contents & mask != 0
                            && (0..3).all(|k| !(lo[k] > brush.maxs[k]) && !(hi[k] < brush.mins[k]))
                        {
                            out.push(Piece::Brush(b as usize));
                        }
                    }
                    return;
                }
                BrushNode::Split {
                    axis,
                    dist,
                    straddlers,
                    children,
                    ..
                } => {
                    if straddlers {
                        self.brush_tree(n + 1, lo, hi, mask, out);
                    }
                    let k = axis as usize;
                    if dist < lo[k] {
                        n += children[0] as usize;
                        continue;
                    }
                    if !(dist > hi[k]) {
                        self.brush_tree(n + children[0] as usize, lo, hi, mask, out);
                    }
                    n += children[1] as usize;
                }
            }
        }
    }

    /// 0x5a7d20: a node culled by 0x5a7c40 (the box's half size plus the
    /// node's under the distance from the geom's centre to the node's on an
    /// axis) stops the walk; the children go in order; a leaf's partition
    /// is taken once.
    fn aabb_tree(&self, t: usize, geom: [Vec3; 3], seen: &mut [bool], out: &mut Vec<Piece>) {
        let w = &self.world;
        let node = w.aabbs[t];
        let [c, lo, hi] = geom;
        let culled = (0..3).any(|k| {
            let d = (c[k] as f64 - node.origin[k] as f64) as f32;
            let g = (hi[k] as f64 - lo[k] as f64) as f32;
            let s = (g as f64 * 0.5 + node.half_size[k] as f64) as f32;
            s < d.abs()
        });
        if culled {
            return;
        }
        if node.child_count > 0 {
            let first = node.child_or_partition as usize;
            for ch in first..first + usize::from(node.child_count) {
                self.aabb_tree(ch, geom, seen, out);
            }
            return;
        }
        let p = node.child_or_partition as usize;
        if std::mem::replace(&mut seen[p], true) {
            return;
        }
        let Some(m) = &w.indexed else { return };
        let part = w.partitions[p];
        let first = part.first_tri as usize;
        let tris = (first..first + usize::from(part.tri_count))
            .filter_map(|i| m.tris.get(i))
            .map(|t| t.map(|v| m.verts[usize::from(v)]))
            .collect();
        let surface = w
            .materials
            .get(node.material as usize)
            .map_or(0, |&(s, _)| s);
        out.push(Piece::Tris { tris, surface });
    }
}
