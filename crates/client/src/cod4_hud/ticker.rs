//! The compass ticker: the heading tape, its marks and distance text.

use vcod_cod4::hud as h;

use super::assets::Loaded;
use super::compass::{OBJECTIVE_SIZE, angle_normalize_180};
use super::text::text;
use super::{Cod4Hud, image_of, quad};
use crate::hud::cod2elem::ScreenPlacement;
use crate::hud::{HudQuad, font};
use crate::play::cod4::Cod4Session;

impl Cod4Hud {
    /// `compass_old`'s tickertape (cod4-hud.md 22; `iw3mp.exe` 0x428310):
    /// `minimapTickerBorder`, then the tape's material across
    /// `minimapTicker` from S `c - 0.25` to `c + 0.25`, `c` the map's turn
    /// over 360, and each active or current objective's icon along it at
    /// its bearing with its distance under it (0x428190).
    pub(super) fn draw_ticker(
        &self,
        l: &Loaded,
        s: &Cod4Session,
        sp: &ScreenPlacement,
        (eye, yaw): (glam::Vec3, f32),
        out: &mut Vec<HudQuad>,
    ) {
        let to_screen = |item: &h::MenuRect, [x, y, w, h]: [f32; 4]| {
            let (sx, sw) = sp.axis(0, x, w, item.horz_align);
            let (sy, sh) = sp.axis(1, y, h, item.vert_align);
            (sx, sy, sw, sh)
        };
        let backed = |name: &str| {
            let item = l.compass.get(name)?;
            let tex = item.background.as_deref().and_then(|m| image_of(l, m))?;
            Some((item, tex))
        };
        if let Some((border, tex)) = backed("minimapTickerBorder") {
            let (x, y, w, h) = to_screen(border, border.rect);
            out.push(quad(x, y, w, h, [1.0; 4], tex, false));
        }
        let Some((tape, tex)) = backed("minimapTicker") else {
            return;
        };
        let (x, y, w, h) = to_screen(tape, tape.rect);
        let turn = self.north_yaw(s) - yaw;
        for (from, to, s0, s1) in ticker_spans(turn / 360.0) {
            let (qx, qw) = (x + from * w, (to - from) * w);
            out.push(HudQuad::rect_uv(
                [qx, y, qw, h],
                [s0, 0.0, s1, 1.0],
                [1.0; 4],
                tex.clone(),
            ));
        }
        let Some(snap) = s.client().snapshots.last() else {
            return;
        };
        let font = l.fonts.pick(0, TICKER_TEXT_SCALE, sp);
        let px = TICKER_TEXT_SCALE / font.unit_scale();
        let text_height = (48.0 * TICKER_TEXT_SCALE) as i32 as f32;
        let [tx, ty, tw, th] = tape.rect;
        for mark in self.objective_marks(s, snap) {
            if mark.at == glam::Vec3::ZERO {
                continue;
            }
            let (along, metres) = ticker_mark([eye.x, eye.y], [mark.at.x, mark.at.y], yaw);
            let centre = [tx + tw * along, ty + th * 0.5];
            if let Some(icon) = image_of(l, &mark.material) {
                let size = OBJECTIVE_SIZE;
                let r = [centre[0] - size * 0.5, centre[1] - size * 0.5, size, size];
                let (qx, qy, qw, qh) = to_screen(tape, r);
                out.push(quad(qx, qy, qw, qh, [1.0; 4], icon, false));
            }
            let label = distance_text(metres);
            let width = font::measure(font, &label, px);
            let top = centre[1] + OBJECTIVE_SIZE + 2.0 - text_height;
            let (qx, qy, _, _) = to_screen(tape, [centre[0] - width * 0.5, top, 0.0, 0.0]);
            let scale = px * sp.virtual_to_real[1];
            text(font, &label, qx, qy, scale, TICKER_TEXT_COLOUR, out);
        }
    }
}

/// The `minimapTicker` item's text scale (`menu_dump`), which its
/// distances draw at, and their yellow-green (cod4-hud.md 22).
const TICKER_TEXT_SCALE: f32 = 0.55;
const TICKER_TEXT_COLOUR: [f32; 4] = [0.9, 1.0, 0.1, 1.0];
/// `compassTickertapeStretch` 0.5 (0x42588b): the share of the tape's
/// texture shown at once.
const TAPE_STRETCH: f32 = 0.5;
/// `compassObjectiveDetailDist` 10 m: nearer, a distance shows tenths.
const DETAIL_METRES: f32 = 10.0;
/// Inches to metres (the double at 0x70b048).
const METRES_PER_UNIT: f32 = 0.0254;

/// Where an objective sits along the tape, 0 at the left end to 1 at the
/// right, and how far it is in metres (cod4-hud.md 22). The tape's middle
/// is the view's yaw; a bearing 90 degrees either side reaches an end.
fn ticker_mark(eye: [f32; 2], goal: [f32; 2], yaw: f32) -> (f32, f32) {
    let to = [goal[0] - eye[0], goal[1] - eye[1]];
    let metres = to[0].hypot(to[1]) * METRES_PER_UNIT;
    let leftward = angle_normalize_180(to[1].atan2(to[0]).to_degrees() - yaw);
    ((0.5 - leftward / 180.0).clamp(0.0, 1.0), metres)
}

/// A distance under a tape icon: whole metres from 10 out, tenths
/// (truncated) inside.
fn distance_text(metres: f32) -> String {
    if metres >= DETAIL_METRES {
        format!("{}m", metres as i32)
    } else {
        let tenths = (metres * 10.0) as i32;
        format!("{}.{}m", tenths / 10, tenths % 10)
    }
}

/// The tape's visible S range, `TAPE_STRETCH` wide around `centre`, as
/// `(x0, x1, s0, s1)` spans of the rect (fractions of its width): one,
/// or two where the range crosses a whole number, so the image never wraps
/// inside a quad.
fn ticker_spans(centre: f32) -> Vec<(f32, f32, f32, f32)> {
    let start = centre - TAPE_STRETCH * 0.5;
    let start = start - start.floor();
    let end = start + TAPE_STRETCH;
    if end <= 1.0 {
        return vec![(0.0, 1.0, start, end)];
    }
    let split = (1.0 - start) / TAPE_STRETCH;
    vec![(0.0, split, start, 1.0), (split, 1.0, 0.0, end - 1.0)]
}

#[cfg(test)]
mod tests {
    use super::*;

    use super::super::tests::close;

    /// An objective on the tape: its bearing against the view's, 90
    /// degrees either side at the ends, and its distance in metres, whole
    /// from 10 out and cut to tenths inside (cod4-hud.md 22).
    #[test]
    fn the_ticker_places_an_objective_by_its_bearing() {
        let cases = [
            ([0.0, 0.0], [1000.0, 0.0], 0.0, 0.0, [0.5, 25.4]),
            (
                [0.0, 0.0],
                [1000.0, 300.0],
                0.0,
                0.0,
                [0.40722644, 26.518377],
            ),
            ([0.0, 0.0], [0.0, -1000.0], 0.0, 0.0, [1.0, 25.4]),
            (
                [100.0, 50.0],
                [-300.0, 400.0],
                30.0,
                170.0,
                [0.6732551, 13.500284],
            ),
            (
                [100.0, 50.0],
                [-300.0, -400.0],
                30.0,
                170.0,
                [0.17574191, 15.292825],
            ),
            ([0.0, 0.0], [-100.0, 1.0], 0.0, 0.0, [0.0, 2.5401268]),
            (
                [0.0, 0.0],
                [300.0, 200.0],
                10.0,
                350.0,
                [0.25727743, 9.1581],
            ),
        ];
        for (eye, goal, north, yaw, want) in cases {
            let _ = north;
            let (percent, metres) = ticker_mark(eye, goal, yaw);
            close([percent, metres], want);
        }
        let texts = [12.7, 10.0, 9.97, 0.05, 3.333, 0.0].map(distance_text);
        assert_eq!(texts, ["12m", "10m", "9.9m", "0.0m", "3.3m", "0.0m"]);
        let spans = [
            (0.1, vec![(0.0, 0.3, 0.85, 1.0), (0.3, 1.0, 0.0, 0.35)]),
            (0.5, vec![(0.0, 1.0, 0.25, 0.75)]),
            (-0.3, vec![(0.0, 1.0, 0.45, 0.95)]),
            (0.9, vec![(0.0, 0.7, 0.65, 1.0), (0.7, 1.0, 0.0, 0.15)]),
            (0.25, vec![(0.0, 1.0, 0.0, 0.5)]),
            (-1.0, vec![(0.0, 0.5, 0.75, 1.0), (0.5, 1.0, 0.0, 0.25)]),
        ];
        for (centre, want) in spans {
            let got = ticker_spans(centre);
            assert_eq!(got.len(), want.len(), "{centre}: {got:?}");
            for (g, w) in got.into_iter().zip(want) {
                close([g.0, g.1, g.2, g.3], [w.0, w.1, w.2, w.3]);
            }
        }
    }
}
