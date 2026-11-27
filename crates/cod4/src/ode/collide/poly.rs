//! 0x597800, the polygon cut every world 0 and 1 collider shares (box,
//! brush and cylinder geoms; cod4-gsc.md 3.6.4, 3.6.5, 3.6.7).

use crate::ode::math::{V3, dot, sub};

#[inline]
fn w(x: f32) -> f64 {
    x as f64
}

/// A plane `n . x = d`.
pub type Plane = (V3, f32);

/// The plane's meet with the segment from `b` to `a`: `(a - b) t + b`,
/// `t = (d - n . b) / n . (a - b)` stored; none when `n . (a - b)` stored
/// is 0.
fn meet(a: V3, b: V3, (n, d): Plane) -> Option<V3> {
    let s = dot(sub(a, b), n);
    if s == 0.0 {
        return None;
    }
    let t = ((w(d) - w(dot(b, n))) / w(s)) as f32;
    Some(std::array::from_fn(|k| {
        ((w(a[k]) - w(b[k])) * w(t) + w(b[k])) as f32
    }))
}

/// 0x597800: the polygon cut to `n . p <= d` in place (a point is out when
/// `d < n . p`, NaN included: the `fcompp` at 0x597854 sets C0 unordered),
/// its count; 0, the points left, when none is in. The last vertex in
/// before the cut and the first in after it (`exit`, `entry`) bound the run
/// out: one vertex out becomes its two meets, its neighbours' first; a
/// longer run its ends' meets with the vertices between dropped
/// (0x597440). A meet whose edge lies along the plane leaves its vertex.
pub fn clip(p: &mut Vec<V3>, plane: Plane) -> usize {
    let len = p.len();
    if len == 0 {
        return 0;
    }
    let (n, d) = plane;
    let inside = |v: V3| w(d) >= w(dot(v, n));
    let last_in = inside(p[len - 1]);
    let (mut prev, mut prev_in) = (len - 1, last_in);
    let (mut any, mut exit, mut entry) = (false, None, None);
    for i in 0..len {
        let cur_in = if i == len - 1 { last_in } else { inside(p[i]) };
        if prev_in {
            any = true;
            if !cur_in {
                exit = Some((prev, i));
            }
        } else if cur_in {
            entry = Some((prev, i));
        }
        (prev, prev_in) = (i, cur_in);
    }
    if !any {
        return 0;
    }
    let Some((a, o)) = exit else { return len };
    let Some((c, e)) = entry else { return len };
    if o == c {
        // 0x597af6: room for a second vertex after the one out
        p.insert(o + 1, p[o]);
        let n2 = len + 1;
        let s = (c + 1) % n2;
        let after = (s + 1) % n2;
        let a = if a > o { a + 1 } else { a };
        let out = p[o];
        p[s] = meet(out, p[after], plane).unwrap_or(out);
        if let Some(v) = meet(out, p[a], plane) {
            p[o] = v;
        }
        return n2;
    }
    if let Some(v) = meet(p[o], p[a], plane) {
        p[o] = v;
    }
    if let Some(v) = meet(p[c], p[e], plane) {
        p[c] = v;
    }
    // 0x597440: the vertices strictly between the run's ends go
    if (o + 1) % len != c {
        if o < c {
            p.drain(o + 1..c);
        } else {
            p.truncate(o + 1);
            p.drain(..c);
        }
    }
    p.len()
}

#[cfg(test)]
mod tests {
    use super::clip;

    /// One corner of a square past the plane `x + y <= 3`: a corner goes in
    /// after it, and the two take the meets on its edges.
    #[test]
    fn a_clip_past_one_corner_puts_a_corner_in() {
        let mut p = vec![
            [0.0, 0.0, 0.0],
            [2.0, 0.0, 0.0],
            [2.0, 2.0, 0.0],
            [0.0, 2.0, 0.0],
        ];
        assert_eq!(clip(&mut p, ([1.0, 1.0, 0.0], 3.0)), 5);
        assert_eq!(
            p,
            [
                [0.0, 0.0, 0.0],
                [2.0, 0.0, 0.0],
                [2.0, 1.0, 0.0],
                [1.0, 2.0, 0.0],
                [0.0, 2.0, 0.0]
            ]
        );
    }

    /// Two corners past `x <= 1`: each moves onto the plane along its edge
    /// to the corner kept beside it.
    #[test]
    fn a_clip_past_two_corners_moves_them_onto_the_plane() {
        let mut p = vec![
            [0.0, 0.0, 0.0],
            [2.0, 0.0, 0.0],
            [2.0, 2.0, 0.0],
            [0.0, 2.0, 0.0],
        ];
        assert_eq!(clip(&mut p, ([1.0, 0.0, 0.0], 1.0)), 4);
        assert_eq!(
            p,
            [
                [0.0, 0.0, 0.0],
                [1.0, 0.0, 0.0],
                [1.0, 2.0, 0.0],
                [0.0, 2.0, 0.0]
            ]
        );
    }

    /// A corner whose dot is NaN is out (0x597854's compare), so a polygon
    /// of NaN corners leaves nothing.
    #[test]
    fn a_nan_corner_is_out() {
        let mut p = vec![[f32::NAN, 0.0, 0.0]; 3];
        assert_eq!(clip(&mut p, ([1.0, 0.0, 0.0], 1.0)), 0);
    }
}
