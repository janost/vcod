//! The load-time half of the sprite deforms (CoDMP.exe 0x514080 autosprite,
//! 0x514330 autosprite2; docs/research/cod11-shader-scripts.md section 8).
//! Retail rebuilds each quad on the CPU every frame from the view axes. vcod
//! moves everything view-independent to load: the quad's centre and size go
//! into the vertex, and `vs_stage` (client shader.wgsl) finishes the quad on
//! the GPU. `autosprite_corner` and `autosprite2_corner` are the CPU mirrors
//! of that vertex-shader step, for the tests.

use crate::mapworld::DrawVert;
use glam::Vec3;

/// Half the quad's corner distance becomes the sprite's half-width: the
/// `0.707` at CoDMP.exe 0x5691cc.
pub const AUTOSPRITE_RADIUS_SCALE: f32 = 0.707;

/// RB_AddQuadStamp's corners in vertex order, as `(s, t)`: top-left,
/// top-right, bottom-right, bottom-left from the eye (0x50fe20 with st
/// 0,0,1,1).
const STAMP_ST: [[f32; 2]; 4] = [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]];
/// The stamp's two triangles.
const STAMP_INDICES: [u32; 6] = [0, 1, 3, 3, 1, 2];

/// Rewrites every four vertices as one autosprite quad: position the
/// four's centre, uv the stamp corner, `lm_uv[0]` the half-width, colour
/// the first vertex's (retail stamps that colour on all four). Returns the
/// stamp indices for the quads, offset by `base`. A trailing partial quad
/// is left untouched and gets no indices; retail warns "Autosprite shader
/// %s had odd vertex count" there.
pub fn encode_autosprite(verts: &mut [DrawVert], base: u32) -> Vec<u32> {
    let quads = verts.len() / 4;
    let mut out = Vec::with_capacity(quads * 6);
    for (q, quad) in verts.as_chunks_mut::<4>().0.iter_mut().enumerate() {
        let mid = quad.iter().map(|v| Vec3::from(v.pos)).sum::<Vec3>() * 0.25;
        let radius = (Vec3::from(quad[0].pos) - mid).length() * AUTOSPRITE_RADIUS_SCALE;
        let color = quad[0].color;
        for (v, st) in quad.iter_mut().zip(STAMP_ST) {
            v.pos = mid.to_array();
            v.uv = st;
            v.lm_uv = [radius, 0.0];
            v.color = color;
        }
        out.extend(STAMP_INDICES.map(|i| base + (q * 4) as u32 + i));
    }
    out
}

/// Where `vs_stage` puts an encoded autosprite vertex: `left` and `up` are
/// the unit view axes (retail's `viewParms.or.axis[1]` and `[2]`).
pub fn autosprite_corner(v: &DrawVert, left: Vec3, up: Vec3) -> Vec3 {
    let r = v.lm_uv[0];
    Vec3::from(v.pos) + left * ((1.0 - 2.0 * v.uv[0]) * r) + up * ((1.0 - 2.0 * v.uv[1]) * r)
}

/// Rewrites each quad (six indices, two triangles, relative to `base`) for
/// autosprite2. The two shortest of the six triangle edges are the quad's
/// ends; each end's vertices move to its midpoint, take the axis between
/// the two midpoints as their normal and their signed half end-length in
/// `lm_uv[0]`. Retail keeps the indices and picks each vertex's side from
/// the edge's direction in the index list; vcod picks the side that puts
/// the vertex back where it was when the quad is seen face on, which keeps
/// the texture unmirrored the same way. A quad whose two shortest edges
/// share a vertex is not a ribbon and is left alone.
pub fn encode_autosprite2(verts: &mut [DrawVert], indices: &[u32], base: u32) {
    for tri2 in indices.as_chunks::<6>().0 {
        let local = tri2.map(|i| i.wrapping_sub(base) as usize);
        if local.iter().any(|&i| i >= verts.len()) {
            continue;
        }
        let pos = |i: usize| Vec3::from(verts[i].pos);
        let edges =
            [(0, 1), (1, 2), (2, 0), (3, 4), (4, 5), (5, 3)].map(|(a, b)| (local[a], local[b]));
        // the shortest two, the first strictly shorter (Q3's AutospriteDeform2 walk)
        let mut best = [(f32::MAX, edges[0]); 2];
        for (a, b) in edges {
            let l = (pos(a) - pos(b)).length_squared();
            if l < best[0].0 {
                best[1] = best[0];
                best[0] = (l, (a, b));
            } else if l < best[1].0 {
                best[1] = (l, (a, b));
            }
        }
        let [(l0, (a0, b0)), (l1, (a1, b1))] = best;
        if [a1, b1].iter().any(|v| *v == a0 || *v == b0) {
            continue;
        }
        let mid0 = (pos(a0) + pos(b0)) * 0.5;
        let mid1 = (pos(a1) + pos(b1)) * 0.5;
        let major = mid1 - mid0;
        // face on: looking against the quad's normal
        let mut normal: Vec3 = [a0, b0, a1, b1]
            .iter()
            .map(|&i| Vec3::from(verts[i].normal))
            .sum();
        if normal.length_squared() < 1e-12 {
            normal = (pos(local[1]) - pos(local[0])).cross(pos(local[2]) - pos(local[0]));
        }
        let minor = major.cross(-normal).normalize_or_zero();
        let plan = [([a0, b0], mid0, l0), ([a1, b1], mid1, l1)].map(|(ends, at, l)| {
            (
                ends,
                at,
                0.5 * l.sqrt(),
                ends.map(|v| (pos(v) - at).dot(minor)),
            )
        });
        for (ends, at, half, side) in plan {
            for (k, &v) in ends.iter().enumerate() {
                // the other vertex's side when this one sits on the axis
                let s = if side[k] != 0.0 {
                    side[k]
                } else {
                    -side[1 - k]
                };
                verts[v].pos = at.to_array();
                verts[v].normal = major.to_array();
                verts[v].lm_uv = [if s < 0.0 { -half } else { half }, 0.0];
            }
        }
    }
}

/// Where `vs_stage` puts an encoded autosprite2 vertex: out from its end's
/// midpoint along `major x forward`, `forward` the unit view direction.
pub fn autosprite2_corner(v: &DrawVert, forward: Vec3) -> Vec3 {
    let minor = Vec3::from(v.normal).cross(forward).normalize_or_zero();
    Vec3::from(v.pos) + minor * v.lm_uv[0]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vert(p: [f32; 3]) -> DrawVert {
        DrawVert {
            pos: p,
            uv: [0.3, 0.7],
            lm_uv: [0.0; 2],
            normal: [0.0, 0.0, 1.0],
            color: [9, 8, 7, 6],
        }
    }

    #[test]
    fn autosprite_faces_the_view_at_the_quads_size() {
        // a 16-unit square in the xz plane centred on (100, 0, 50)
        let mut v: Vec<DrawVert> = [
            [92., 0., 42.],
            [108., 0., 42.],
            [108., 0., 58.],
            [92., 0., 58.],
        ]
        .map(vert)
        .to_vec();
        v[1].color = [1, 2, 3, 4];
        let idx = encode_autosprite(&mut v, 10);
        assert_eq!(idx, vec![10, 11, 13, 13, 11, 12]);
        assert!(
            v.iter()
                .all(|v| v.pos == [100.0, 0.0, 50.0] && v.color == [9, 8, 7, 6])
        );
        // half-width: the centre-to-corner distance times 0.707
        let r = (8.0f32 * 8.0 * 2.0).sqrt() * AUTOSPRITE_RADIUS_SCALE;
        assert!((v[0].lm_uv[0] - r).abs() < 1e-4);
        // looking down +x: left is +y, up +z; the quad lies in the view plane
        let (left, up) = (Vec3::Y, Vec3::Z);
        let c: Vec<Vec3> = v.iter().map(|v| autosprite_corner(v, left, up)).collect();
        assert!(c.iter().all(|p| (p.x - 100.0).abs() < 1e-5));
        // stamp order: top-left, top-right, bottom-right, bottom-left
        let mid = Vec3::new(100.0, 0.0, 50.0);
        assert!((c[0] - (mid + (left + up) * r)).length() < 1e-4);
        assert!((c[1] - (mid + (-left + up) * r)).length() < 1e-4);
        assert!((c[2] - (mid - (left + up) * r)).length() < 1e-4);
        assert!((c[3] - (mid + (left - up) * r)).length() < 1e-4);
    }

    #[test]
    fn autosprite_leaves_a_partial_quad() {
        let mut v: Vec<DrawVert> = (0..6).map(|i| vert([i as f32, 0.0, 0.0])).collect();
        let idx = encode_autosprite(&mut v, 0);
        assert_eq!(idx.len(), 6);
        assert_eq!(v[4].pos, [4.0, 0.0, 0.0]);
        assert_eq!(v[5].uv, [0.3, 0.7]);
    }

    #[test]
    fn autosprite2_swings_about_the_long_axis() {
        // a beam 4 wide along x from 0 to 100, lying flat (normal +z)
        let mut v: Vec<DrawVert> = [[0., -2., 0.], [100., -2., 0.], [100., 2., 0.], [0., 2., 0.]]
            .map(vert)
            .to_vec();
        let idx = [0, 1, 2, 0, 2, 3];
        encode_autosprite2(&mut v, &idx, 0);
        // the ends' midpoints and the axis between them
        assert_eq!(v[0].pos, [0.0, 0.0, 0.0]);
        assert_eq!(v[3].pos, [0.0, 0.0, 0.0]);
        assert_eq!(v[1].pos, [100.0, 0.0, 0.0]);
        assert!(
            v.iter()
                .all(|v| v.normal[0].abs() == 100.0 && v.normal[1..] == [0.0, 0.0])
        );
        assert!(v.iter().all(|v| (v.lm_uv[0].abs() - 2.0).abs() < 1e-5));
        // seen from above (forward -z) the beam keeps its original shape
        let fwd = -Vec3::Z;
        let c: Vec<Vec3> = v.iter().map(|v| autosprite2_corner(v, fwd)).collect();
        for (p, want) in
            c.iter()
                .zip([[0., -2., 0.], [100., -2., 0.], [100., 2., 0.], [0., 2., 0.]])
        {
            assert!((*p - Vec3::from(want)).length() < 1e-4, "{p} vs {want:?}");
        }
        // seen along y, it turns to span z instead, still on its axis
        let c: Vec<Vec3> = v.iter().map(|v| autosprite2_corner(v, Vec3::Y)).collect();
        assert!(
            c.iter()
                .all(|p| p.y.abs() < 1e-5 && (p.z.abs() - 2.0).abs() < 1e-4)
        );
        assert!(c[0].z == -c[3].z && c[1].z == -c[2].z);
    }
}
