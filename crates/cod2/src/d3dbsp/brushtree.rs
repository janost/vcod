//! The server's load-time k-d tree over a leaf's (or a submodel's) brush
//! list, `CMod_PartionLeafBrushes_r` (`cod2_lnxded` 0x08052d16) and its
//! split scorer 0x08052bb2 (doc "What the trace reads"). The trace walks
//! brushes in the tree's order, so ties between brushes follow it.

use anyhow::{Context, Result, bail, ensure};

pub use vcod_common::clipmap::BrushNode;

/// A brush as the tree sees it: its axial bounds and runtime contents.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BrushBox {
    pub mins: [f32; 3],
    pub maxs: [f32; 3],
    pub contents: u32,
}

/// A built tree; node 0 is the root.
#[derive(Clone, Debug, PartialEq)]
pub struct BrushTree {
    pub nodes: Vec<BrushNode>,
}

/// Retail's float `min` (0x0805523a over 0x08055388): `a` unless `b - a`
/// is negative or NaN.
fn fmin(a: f32, b: f32) -> f32 {
    if b - a >= 0.0 { a } else { b }
}

/// Retail's int `min` (0x0805525e over 0x080553d2).
fn imin(a: i32, b: i32) -> i32 {
    if b.wrapping_sub(a) < 0 { b } else { a }
}

/// One write into the list, refused past its end.
fn put(buf: &mut [u16], at: usize, b: u16) -> Result<()> {
    match buf.get_mut(at) {
        Some(slot) => *slot = b,
        None => bail!("brush tree writes past the list's end"),
    }
    Ok(())
}

/// Recursion bound against hostile input; retail has none.
const MAX_DEPTH: usize = 1024;

struct Builder<'a> {
    boxes: &'a [BrushBox],
    nodes: Vec<BrushNode>,
}

impl Builder<'_> {
    fn bx(&self, b: u16) -> Result<&BrushBox> {
        self.boxes
            .get(b as usize)
            .with_context(|| format!("brush {b} out of range"))
    }

    /// 0x08052bb2: the candidate `d` on `axis` scored over `list`, and the
    /// split it moves to, the middle of the gap.
    fn score(
        &self,
        list: &[u16],
        axis: usize,
        d: f32,
        mins: &[f32; 3],
        maxs: &[f32; 3],
    ) -> Result<(f32, f32)> {
        let (mut hi, mut lo) = (-1i32, -1i32);
        let (mut hi_min, mut lo_max) = (f32::MAX, -f32::MAX);
        for &b in list {
            let bx = self.bx(b)?;
            if d <= bx.mins[axis] {
                hi += 1;
                if bx.mins[axis] < hi_min {
                    hi_min = bx.mins[axis];
                }
            } else if bx.maxs[axis] <= d {
                lo += 1;
                if lo_max < bx.maxs[axis] {
                    lo_max = bx.maxs[axis];
                }
            }
        }
        let n = imin(lo, hi);
        // x87: the sum and the halving stay extended, one rounding to f32.
        let mid = ((lo_max as f64 + hi_min as f64) * 0.5) as f32;
        let score = if n < 1 {
            0.0
        } else {
            fmin(hi_min - mins[axis], maxs[axis] - lo_max) * n as f32
        };
        Ok((score, mid))
    }

    /// One `CMod_PartionLeafBrushes_r` call over `buf[start..start + count]`.
    fn build(
        &mut self,
        buf: &mut [u16],
        start: usize,
        count: usize,
        mins: [f32; 3],
        maxs: [f32; 3],
        depth: usize,
    ) -> Result<usize> {
        ensure!(depth < MAX_DEPTH, "brush tree deeper than {MAX_DEPTH}");
        let me = self.nodes.len();
        self.nodes.push(BrushNode::Leaf {
            first: 0,
            count: 0,
            contents: 0,
        });
        let list = buf
            .get(start..start + count)
            .with_context(|| format!("brush run {start}+{count} past the list's end"))?
            .to_vec();

        let (mut best, mut best_axis, mut best_d) = (0.0f32, None, 0.0f32);
        for axis in 0..3 {
            for &b in &list {
                let bx = *self.bx(b)?;
                for cand in [bx.mins[axis], bx.maxs[axis]] {
                    let (score, mid) = self.score(&list, axis, cand, &mins, &maxs)?;
                    if best < score {
                        (best, best_axis, best_d) = (score, Some(axis), mid);
                    }
                }
            }
        }

        let Some(axis) = best_axis else {
            ensure!(
                count <= i16::MAX as usize,
                "leaf brush count {count} exceeded"
            );
            let mut contents = 0;
            for &b in &list {
                contents |= self.bx(b)?.contents;
            }
            self.nodes[me] = BrushNode::Leaf {
                first: start as u32,
                count: count as u16,
                contents,
            };
            return Ok(me);
        };
        let d = best_d;
        let mut write = start;

        let mut contents = 0;
        let mut straddle = 0;
        for &b in &list {
            let bx = self.bx(b)?;
            if bx.mins[axis] < d && d < bx.maxs[axis] {
                put(buf, write + straddle, b)?;
                straddle += 1;
            }
        }
        if straddle > 0 {
            let s = self.build(buf, write, straddle, mins, maxs, depth + 1)?;
            contents = self.nodes[s].contents();
            write += straddle;
        }

        // One clearance over both sides: retail never resets it.
        let mut clearance = f32::MAX;
        let mut children = [0u16; 2];
        for (side, child) in children.iter_mut().enumerate() {
            let mut n = 0;
            for &b in &list {
                let bx = *self.bx(b)?;
                let gap = if side == 0 {
                    (d <= bx.mins[axis]).then(|| bx.mins[axis] - d)
                } else {
                    (bx.maxs[axis] <= d).then(|| d - bx.maxs[axis])
                };
                if let Some(gap) = gap {
                    clearance = fmin(clearance, gap);
                    put(buf, write + n, b)?;
                    n += 1;
                }
            }
            let (mut cmins, mut cmaxs) = (mins, maxs);
            if side == 0 {
                cmins[axis] = d + clearance;
            } else {
                cmaxs[axis] = d - clearance;
            }
            let c = self.build(buf, write, n, cmins, cmaxs, depth + 1)?;
            let off = c - me;
            ensure!(
                off <= u16::MAX as usize,
                "brush tree child offset {off} exceeded"
            );
            *child = off as u16;
            contents |= self.nodes[c].contents();
            write += n;
        }
        self.nodes[me] = BrushNode::Split {
            axis: axis as u8,
            dist: d,
            clearance,
            straddlers: straddle > 0,
            children,
            contents,
        };
        Ok(me)
    }
}

/// Builds the tree over `buf[start..start + count]` (brush numbers into
/// `boxes`), rewriting that run in place into the order the tree's leaves
/// clip, as 0x080531d8 does. The root's bounds are the brushes' union,
/// unwidened. A brush whose bounds sit on a split from both sides is listed
/// on both, and retail then writes past the run: that write lands in `buf`
/// past `count` when `buf` has room, and errors when it does not.
pub fn partition_brushes(
    boxes: &[BrushBox],
    buf: &mut [u16],
    start: usize,
    count: usize,
) -> Result<BrushTree> {
    let run = buf
        .get(start..start + count)
        .with_context(|| format!("brush run {start}+{count} past the list's end"))?;
    let (mut mins, mut maxs) = ([f32::MAX; 3], [-f32::MAX; 3]);
    for &b in run {
        let bx = boxes
            .get(b as usize)
            .with_context(|| format!("brush {b} out of range"))?;
        for k in 0..3 {
            if bx.mins[k] < mins[k] {
                mins[k] = bx.mins[k];
            }
            if maxs[k] < bx.maxs[k] {
                maxs[k] = bx.maxs[k];
            }
        }
    }
    let mut b = Builder {
        boxes,
        nodes: Vec::new(),
    };
    b.build(buf, start, count, mins, maxs, 0)?;
    Ok(BrushTree { nodes: b.nodes })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bx(x: [f32; 2], y: [f32; 2], contents: u32) -> BrushBox {
        BrushBox {
            mins: [x[0], y[0], 0.0],
            maxs: [x[1], y[1], 1.0],
            contents,
        }
    }

    /// A 2x2 grid of unit boxes scores x and y alike: the strict `>` keeps
    /// the first axis tried, x.
    #[test]
    fn equal_scores_keep_the_first_candidate() {
        let boxes = [
            bx([0.0, 1.0], [0.0, 1.0], 1),
            bx([2.0, 3.0], [0.0, 1.0], 2),
            bx([0.0, 1.0], [2.0, 3.0], 4),
            bx([2.0, 3.0], [2.0, 3.0], 8),
        ];
        let mut buf = vec![0, 1, 2, 3];
        let t = partition_brushes(&boxes, &mut buf, 0, 4).unwrap();
        let BrushNode::Split {
            axis,
            dist,
            straddlers,
            children,
            contents,
            ..
        } = t.nodes[0]
        else {
            panic!("root is a leaf: {:?}", t.nodes);
        };
        assert_eq!((axis, dist, straddlers, contents), (0, 1.5, false, 15));
        // High side (x >= 1.5) first, then the low side, each in list order.
        assert_eq!(buf, [1, 3, 0, 2]);
        assert_eq!(children, [1, 2]);
        assert_eq!(
            t.nodes[1],
            BrushNode::Leaf {
                first: 0,
                count: 2,
                contents: 10
            }
        );
        assert_eq!(
            t.nodes[2],
            BrushNode::Leaf {
                first: 2,
                count: 2,
                contents: 5
            }
        );
    }

    /// Widen the y gap and y outscores x.
    #[test]
    fn a_wider_gap_wins_the_axis() {
        let boxes = [
            bx([0.0, 1.0], [0.0, 1.0], 1),
            bx([2.0, 3.0], [0.0, 1.0], 1),
            bx([0.0, 1.0], [4.0, 5.0], 1),
            bx([2.0, 3.0], [4.0, 5.0], 1),
        ];
        let mut buf = vec![0, 1, 2, 3];
        let t = partition_brushes(&boxes, &mut buf, 0, 4).unwrap();
        let BrushNode::Split { axis, dist, .. } = t.nodes[0] else {
            panic!()
        };
        assert_eq!((axis, dist), (1, 2.5));
        assert_eq!(buf, [2, 3, 0, 1]);
    }

    /// A brush across the split goes first, into its own subtree right
    /// after the node, then the high side, then the low side.
    #[test]
    fn straddlers_come_first() {
        let boxes = [
            bx([0.0, 1.0], [0.0, 1.0], 1),
            bx([2.0, 3.0], [0.0, 1.0], 2),
            bx([0.0, 1.0], [2.0, 3.0], 4),
            bx([2.0, 3.0], [2.0, 3.0], 8),
            bx([0.0, 3.0], [0.0, 1.0], 16),
        ];
        let mut buf = vec![0, 1, 2, 3, 4];
        let t = partition_brushes(&boxes, &mut buf, 0, 5).unwrap();
        assert_eq!(buf, [4, 1, 3, 0, 2]);
        let BrushNode::Split {
            axis,
            straddlers,
            children,
            clearance,
            contents,
            ..
        } = t.nodes[0]
        else {
            panic!()
        };
        assert_eq!((axis, straddlers, clearance, contents), (0, true, 0.5, 31));
        assert_eq!(
            t.nodes[1],
            BrushNode::Leaf {
                first: 0,
                count: 1,
                contents: 16
            }
        );
        assert_eq!(children, [2, 3]);
    }

    /// Counts start at -1: two brushes a side are needed for any split.
    #[test]
    fn three_brushes_stay_a_leaf() {
        let boxes = [
            bx([0.0, 1.0], [0.0, 1.0], 1),
            bx([2.0, 3.0], [0.0, 1.0], 1),
            bx([4.0, 5.0], [0.0, 1.0], 1),
        ];
        let mut buf = vec![2, 0, 1];
        let t = partition_brushes(&boxes, &mut buf, 0, 3).unwrap();
        assert_eq!(t.nodes.len(), 1);
        assert_eq!(buf, [2, 0, 1]);
    }

    #[test]
    fn bad_input_errors() {
        let boxes = [bx([0.0, 1.0], [0.0, 1.0], 1)];
        assert!(partition_brushes(&boxes, &mut [0, 5], 0, 2).is_err());
        assert!(partition_brushes(&boxes, &mut [0], 0, 2).is_err());
    }

    /// A flat brush on the split is listed on both sides; with no room
    /// past the run the write errors instead of overrunning.
    #[test]
    fn a_doubly_listed_brush_needs_room() {
        let boxes = [
            bx([0.0, 1.0], [0.0, 1.0], 1),
            bx([0.0, 1.0], [0.0, 1.0], 1),
            bx([1.0, 1.0], [0.0, 1.0], 1),
            bx([1.0, 2.0], [0.0, 1.0], 1),
            bx([1.0, 3.0], [0.0, 1.0], 1),
        ];
        let mut exact = vec![0, 1, 2, 3, 4];
        assert!(partition_brushes(&boxes, &mut exact, 0, 5).is_err());
        let mut room = vec![0, 1, 2, 3, 4, 9];
        let t = partition_brushes(&boxes, &mut room, 0, 5).unwrap();
        assert_eq!(room, [2, 3, 4, 0, 1, 2]);
        let BrushNode::Split { dist, children, .. } = t.nodes[0] else {
            panic!()
        };
        assert_eq!((dist, children), (1.0, [1, 2]));
    }
}
