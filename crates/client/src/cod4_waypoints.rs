//! A waypoint hudelem with an offscreen material (`setWaypoint(true,
//! material)`), drawn in the 2D hudelem pass as 1.7 draws it (`iw3mp.exe`
//! 0x439a40..0x439c50): its icon where the point lands on screen, held
//! inside the padded, round-cornered edge when the point is off it, with
//! `hud_offscreenobjectivepointer` beside it pointing out
//! (docs/research/cod4-hud.md 16.3).

use crate::hud::HudQuad;
use glam::{Vec2, Vec3};

/// The `waypoint*` dvars at 1.7's defaults (cod4-hud.md 16.3), in
/// 480-line units.
pub(crate) mod dvar {
    pub const ICON_SIZE: [f32; 2] = [36.0, 36.0];
    pub const POINTER_SIZE: [f32; 2] = [25.0, 12.0];
    pub const POINTER_DISTANCE: f32 = 30.0;
    pub const DISTANCE_THRESHOLD_ALPHA: f32 = 30.0;
    /// Left, right, top, bottom.
    pub const PAD: [f32; 4] = [103.0, 0.0, 0.0, 30.0];
    pub const CORNER_RADIUS: f32 = 105.0;
    pub const SCALE_LENGTH: f32 = 500.0;
    pub const SCALE_SMALLEST: f32 = 1.0;
    pub const DIST_SCALE_RANGE: [f32; 2] = [1000.0, 3000.0];
    pub const DIST_SCALE_SMALLEST: f32 = 0.8;
    pub const TWEAK_Y: f32 = -17.0;
    /// Above a player's origin: prone, crouched, standing.
    pub const PLAYER_OFFSET: [f32; 3] = [30.0, 56.0, 74.0];
}

/// The pointer's material (`cgMedia.offscreenObjectivePointer`, the name
/// at `iw3mp.exe` 0x6cbe5c).
pub(crate) const POINTER: &str = "hud_offscreenobjectivepointer";

/// The drawn view (`cg.refdef`): the eye, its axes and the window.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Eye {
    pub origin: Vec3,
    pub forward: Vec3,
    pub left: Vec3,
    pub up: Vec3,
    /// `tanHalfFovX`, `tanHalfFovY`.
    pub tan: [f32; 2],
    /// Real pixels.
    pub size: [f32; 2],
}

impl Eye {
    /// The pixel a world point lands on (`iw3mp.exe` 0x439320). One behind
    /// the eye has no pixel; it gives its sideways direction instead,
    /// grown past the window so the edge clamp catches it.
    fn screen(&self, at: Vec3) -> Vec2 {
        let d = at - self.origin;
        let depth = d.dot(self.forward);
        let across = Vec2::new(d.dot(self.left), d.dot(self.up));
        let size = Vec2::from(self.size);
        if depth >= 0.0 {
            let ndc = across / Vec2::from(self.tan) / depth;
            return size * 0.5 * (Vec2::ONE - ndc);
        }
        past_the_window(-across, size)
    }
}

/// A behind-the-eye direction `v` pushed off a `size` window: an axis
/// under 0.001 (the float at 0x70b278) reads 0.001, both under it point
/// straight down two windows, and `v` is multiplied by the width until
/// its x clears it, then by the height until its y does.
fn past_the_window(v: Vec2, size: Vec2) -> Vec2 {
    const TINY: f32 = 0.001;
    let tiny = v.abs().cmplt(Vec2::splat(TINY));
    if tiny.all() {
        return Vec2::new(v.x, size.y * 2.0);
    }
    let mut v = Vec2::select(tiny, Vec2::splat(TINY), v);
    for k in 0..2 {
        while v[k].abs() < size[k] {
            v *= size[k];
        }
    }
    v
}

/// Holds a pixel inside a `size` window less `pad` (left, right, top,
/// bottom) with corners rounded by `radius` (`waypointOffscreenRoundedCorners`
/// on; the clamp at 0x439510, which reads the corner dvars at 0x43966e
/// and 0x439681). Off a side the point slides along the line to the
/// window's centre until it meets that side, x's first; past a corner's
/// arc, onto the arc. Returns the held point and, when it moved, the unit
/// way out to where it was and how far.
fn onto_edge(p: Vec2, size: [f32; 2], pad: [f32; 4], radius: f32) -> (Vec2, Option<(Vec2, f32)>) {
    let half = Vec2::from(size) * 0.5;
    let lo = Vec2::new(pad[0], pad[2]) - half;
    let hi = half - Vec2::new(pad[1], pad[3]);
    let mut c = p - half;
    let mut moved = false;
    for k in 0..2 {
        let side = if c[k] < lo[k] {
            lo[k]
        } else if c[k] > hi[k] {
            hi[k]
        } else {
            continue;
        };
        c *= side / c[k];
        moved = true;
    }
    let low_side = c.cmplt(Vec2::ZERO);
    let arc_centre = Vec2::select(low_side, lo + radius, hi - radius);
    let out = (c - arc_centre).normalize_or_zero();
    let past_arc = Vec2::select(low_side, -out, out).cmpgt(Vec2::ZERO).all();
    if past_arc && arc_centre.distance(c) > radius {
        c = arc_centre + out * radius;
        moved = true;
    }
    let held = c + half;
    (
        held,
        moved.then(|| ((p - held).normalize_or_zero(), held.distance(p))),
    )
}

/// A size factor that runs from 1 at `start` down to `smallest` at
/// `start + length`.
struct Shrink {
    start: f32,
    length: f32,
    smallest: f32,
}

impl Shrink {
    fn at(&self, d: f32) -> f32 {
        if d <= self.start {
            1.0
        } else if d >= self.start + self.length {
            self.smallest
        } else {
            let f = (d - self.start) / self.length;
            f * self.smallest + (1.0 - f)
        }
    }
}

/// The icon by its distance from the eye: whole inside 1000 units, 0.8
/// past 3000 (`waypointDistScale*`).
const DISTANCE_SHRINK: Shrink = Shrink {
    start: dvar::DIST_SCALE_RANGE[0],
    length: dvar::DIST_SCALE_RANGE[1] - dvar::DIST_SCALE_RANGE[0],
    smallest: dvar::DIST_SCALE_SMALLEST,
};

/// One offscreen-capable waypoint at world point `at`, its icon's texture
/// and `pointer`'s, in colour `rgba`, pushed onto `out` pointer first.
pub(crate) fn quads(
    eye: &Eye,
    at: Vec3,
    rgba: [f32; 4],
    icon: &str,
    pointer: Option<&str>,
    out: &mut Vec<HudQuad>,
) {
    use dvar::*;
    // Every dvar is in 480-line units.
    let unit = eye.size[1] / 480.0;
    let pointer_size = Vec2::from(POINTER_SIZE) * unit;
    let pointer_gap = POINTER_DISTANCE * unit;
    let keep_clear = pointer_size.y * 0.5 + pointer_gap;
    let pad = PAD.map(|p| p * unit + keep_clear);
    let projected = eye.screen(at) + Vec2::new(0.0, TWEAK_Y * unit);
    let (spot, moved) = onto_edge(projected, eye.size, pad, CORNER_RADIUS * unit);
    let mut icon_scale = DISTANCE_SHRINK.at(at.distance(eye.origin));
    if let Some((way_out, dist)) = moved.filter(|&(_, d)| d > 0.0) {
        let edge_shrink = Shrink {
            start: 0.0,
            length: SCALE_LENGTH * unit,
            smallest: SCALE_SMALLEST,
        };
        icon_scale *= edge_shrink.at(dist);
        if let Some(tex) = pointer {
            let fade_in = (DISTANCE_THRESHOLD_ALPHA * unit).max(0.1);
            let mut colour = rgba;
            colour[3] *= (dist / fade_in).min(1.0);
            let turn = way_out.x.atan2(-way_out.y);
            let centre = spot + way_out * pointer_gap;
            out.push(turned(centre, pointer_size, turn, colour, tex));
        }
    }
    let icon_size = Vec2::from(ICON_SIZE) * unit * icon_scale;
    out.push(turned(spot, icon_size, 0.0, rgba, icon));
}

/// A `size` quad centred on `centre`, turned `angle` radians clockwise,
/// corners in [`HudQuad`]'s ring order.
fn turned(centre: Vec2, size: Vec2, angle: f32, rgba: [f32; 4], tex: &str) -> HudQuad {
    let (sin, cos) = angle.sin_cos();
    let across = Vec2::new(cos, sin) * (size.x * 0.5);
    let down = Vec2::new(-sin, cos) * (size.y * 0.5);
    let at = |a: f32, d: f32| (centre + across * a + down * d).to_array();
    HudQuad {
        verts: [at(-1.0, -1.0), at(1.0, -1.0), at(1.0, 1.0), at(-1.0, 1.0)],
        uvs: [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]],
        rgba,
        texture: tex.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 1280x960 window (two pixels a 480-line unit) looking down +x.
    fn eye() -> Eye {
        Eye {
            origin: Vec3::ZERO,
            forward: Vec3::X,
            left: Vec3::Y,
            up: Vec3::Z,
            tan: [1.0, 0.75],
            size: [1280.0, 960.0],
        }
    }

    fn close(a: f32, b: f32) -> bool {
        (a - b).abs() <= 1e-3 * b.abs().max(1.0)
    }

    fn quads_at(at: Vec3) -> Vec<HudQuad> {
        let mut out = Vec::new();
        quads(
            &eye(),
            at,
            [1.0, 0.5, 0.25, 0.8],
            "icon",
            Some(POINTER),
            &mut out,
        );
        out
    }

    fn assert_quad(q: &HudQuad, verts: [[f32; 2]; 4], alpha: f32, tex: &str) {
        assert_eq!(q.texture, tex);
        assert!(close(q.rgba[3], alpha), "alpha {} != {alpha}", q.rgba[3]);
        for (v, w) in q.verts.iter().zip(verts) {
            assert!(
                close(v[0], w[0]) && close(v[1], w[1]),
                "{:?} != {verts:?}",
                q.verts
            );
        }
    }

    /// Where `screen` puts a point ahead, behind, and straight behind or
    /// nearly so (the 0.001 stand-in and the push past the window).
    #[test]
    fn cod4_waypoints_project_points() {
        let e = eye();
        let cases = [
            (Vec3::new(100.0, 10.0, 20.0), Vec2::new(576.0, 352.0)),
            (Vec3::new(-100.0, 30.0, -5.0), Vec2::new(-38400.0, 6400.0)),
            (Vec3::new(-100.0, 0.0, 0.0), Vec2::new(0.0, 1920.0)),
            (Vec3::new(-100.0, 0.0, 0.5), Vec2::new(1638.4001, -819200.0)),
            (
                Vec3::new(-50.0, 0.0002, 2.0),
                Vec2::new(1638.4001, -3276800.0),
            ),
        ];
        for (at, want) in cases {
            let got = e.screen(at);
            assert!(
                close(got.x, want.x) && close(got.y, want.y),
                "{at}: {got} != {want}"
            );
        }
    }

    /// The edge clamp on a 1280x960 window with doubled default pads: a
    /// corner point onto the rounded corner, a side point onto the side,
    /// points inside left alone.
    #[test]
    fn cod4_waypoints_clamp_to_the_padded_edge() {
        let size = [1280.0, 960.0];
        let pad = [206.0, 0.0, 0.0, 60.0];
        let cases = [
            (
                Vec2::new(10.0, 10.0),
                Vec2::new(212.56467, 157.90332),
                Some((Vec2::new(-0.8076279, -0.5896925), 250.81435)),
            ),
            (Vec2::new(700.0, 100.0), Vec2::new(700.0, 100.0), None),
            (
                Vec2::new(-300.0, 500.0),
                Vec2::new(206.0, 489.23404),
                Some((Vec2::new(-0.99977374, 0.021271788), 506.11453)),
            ),
            (Vec2::new(640.0, 480.0), Vec2::new(640.0, 480.0), None),
            (
                Vec2::new(1270.0, 950.0),
                Vec2::new(1182.3481, 867.4201),
                Some((Vec2::new(0.7278516, 0.68573457), 120.425446)),
            ),
        ];
        for (p, want, moved) in cases {
            let (got, got_moved) = onto_edge(p, size, pad, 210.0);
            assert!(
                close(got.x, want.x) && close(got.y, want.y),
                "{p}: {got} != {want}"
            );
            match (got_moved, moved) {
                (None, None) => {}
                (Some((d, l)), Some((wd, wl))) => {
                    assert!(
                        close(d.x, wd.x) && close(d.y, wd.y) && close(l, wl),
                        "{p}: {d} {l}"
                    );
                }
                other => panic!("{p}: {other:?}"),
            }
        }
    }

    /// `distance_scale` from 1 inside 1000 units to 0.8 past 3000.
    #[test]
    fn cod4_waypoints_shrink_with_distance() {
        for (d, want) in [
            (0.0, 1.0),
            (1000.0, 1.0),
            (1500.0, 0.95),
            (2999.0, 0.8001),
            (3000.0, 0.8),
            (5000.0, 0.8),
        ] {
            assert!(close(DISTANCE_SHRINK.at(d), want), "{d}");
        }
    }

    /// The quads drawn for a point on screen near and far, off each edge,
    /// off a corner and behind: the icon's corners and the pointer's.
    #[test]
    fn cod4_waypoints_draw_icons_and_pointers() {
        const ICON: &str = "icon";
        type Drawn<'a> = &'a [([[f32; 2]; 4], &'a str)];
        let cases: [(Vec3, Drawn); 7] = [
            (
                Vec3::new(500.0, 20.0, 10.0),
                &[(
                    [
                        [578.4, 397.2],
                        [650.4, 397.2],
                        [650.4, 469.2],
                        [578.4, 469.2],
                    ],
                    ICON,
                )],
            ),
            (
                Vec3::new(2500.0, 0.0, 0.0),
                &[(
                    [
                        [609.4, 415.4],
                        [670.6, 415.4],
                        [670.6, 476.6],
                        [609.4, 476.6],
                    ],
                    ICON,
                )],
            ),
            (
                Vec3::new(100.0, 300.0, 0.0),
                &[
                    (
                        [
                            [205.56863, 497.31085],
                            [206.45392, 447.31866],
                            [230.45016, 447.7436],
                            [229.56488, 497.73578],
                        ],
                        POINTER,
                    ),
                    (
                        [
                            [242.0, 437.58957],
                            [314.0, 437.58957],
                            [314.0, 509.58957],
                            [242.0, 509.58957],
                        ],
                        ICON,
                    ),
                ],
            ),
            (
                Vec3::new(100.0, 0.0, 120.0),
                &[
                    (
                        [[615.0, 0.0], [665.0, 0.0], [665.0, 24.0], [615.0, 24.0]],
                        POINTER,
                    ),
                    (
                        [[604.0, 36.0], [676.0, 36.0], [676.0, 108.0], [604.0, 108.0]],
                        ICON,
                    ),
                ],
            ),
            (
                Vec3::new(-2000.0, 0.0, 64.0),
                &[
                    (
                        [
                            [615.0046, 0.0],
                            [665.0046, 0.0],
                            [665.00433, 24.000238],
                            [615.00433, 23.999762],
                        ],
                        POINTER,
                    ),
                    (
                        [
                            [607.6076, 39.603683],
                            [672.4002, 39.603683],
                            [672.4002, 104.39632],
                            [607.6076, 104.39632],
                        ],
                        ICON,
                    ),
                ],
            ),
            (
                Vec3::new(100.0, 150.0, 140.0),
                &[
                    (
                        [
                            [248.7901, 126.91094],
                            [283.29492, 90.7251],
                            [300.66412, 107.28743],
                            [266.1593, 143.47327],
                        ],
                        POINTER,
                    ),
                    (
                        [
                            [282.15015, 122.505005],
                            [354.15015, 122.505005],
                            [354.15015, 194.505],
                            [282.15015, 194.505],
                        ],
                        ICON,
                    ),
                ],
            ),
            (
                Vec3::new(100.0, -95.0, -60.0),
                &[
                    (
                        [
                            [1226.526, 782.2146],
                            [1196.2274, 821.9889],
                            [1177.1356, 807.44556],
                            [1207.4342, 767.67126],
                        ],
                        POINTER,
                    ),
                    (
                        [
                            [1118.1016, 722.4718],
                            [1190.1016, 722.4718],
                            [1190.1016, 794.4718],
                            [1118.1016, 794.4718],
                        ],
                        ICON,
                    ),
                ],
            ),
        ];
        for (at, want) in cases {
            let got = quads_at(at);
            assert_eq!(got.len(), want.len(), "{at}");
            for (q, (verts, tex)) in got.iter().zip(want) {
                assert_quad(q, *verts, 0.8, tex);
            }
        }
    }

    /// The pointer fades in over its threshold: a point clamped 30 pixels
    /// down onto the top edge (72 pixels in: half the pointer's 24 and its
    /// 60) draws it at half the colour's alpha.
    #[test]
    fn cod4_waypoints_fade_the_pointer_near_the_edge() {
        let mut out = Vec::new();
        quads(
            &eye(),
            Vec3::new(100.0, 0.0, 63.125),
            [1.0; 4],
            "icon",
            Some(POINTER),
            &mut out,
        );
        assert_eq!(out.len(), 2);
        assert_quad(
            &out[0],
            [[615.0, 0.0], [665.0, 0.0], [665.0, 24.0], [615.0, 24.0]],
            0.5,
            POINTER,
        );
        assert_quad(
            &out[1],
            [[604.0, 36.0], [676.0, 36.0], [676.0, 108.0], [604.0, 108.0]],
            1.0,
            "icon",
        );
    }
}
