//! Text and its glow, and a line cut off by a newer one.

use super::assets::Loaded;
use super::{image_of, quad};
use crate::hud::HudQuad;
use crate::hud::cod2elem::ScreenPlacement;
use crate::hud::font::{self, Font};

/// `CG_DrawDisconnect` (`iw3mp.exe` 0x42f930): `CGAME_CONNECTIONINTERUPTED`
/// at scale 0.5 centred on the 640 grid, its baseline at y 100 (0x6f9e58),
/// white in style 3, and unless `cg.time & 0x200` the `net_disconnect`
/// material at (296, 320), 48 by 48 (0x6c7f44, 0x6c7f48, 0x70bdcc).
/// cod4-hud.md 14.
pub(super) fn draw_interrupted(l: &Loaded, sp: &ScreenPlacement, now: i32, out: &mut Vec<HudQuad>) {
    let line = l
        .localized
        .get("CGAME_CONNECTIONINTERUPTED")
        .unwrap_or("Connection Interrupted");
    let scale = 0.5;
    let f = l.fonts.pick(0, scale, sp);
    let px = scale / f.unit_scale();
    let w = font::measure(f, line, px);
    let h = (48.0 * scale) as i32 as f32;
    let (x, _) = sp.axis(0, ((640.0 - w) / 2.0).trunc(), w, 0);
    let (y, _) = sp.axis(1, 100.0 - h, h, 0);
    text(f, line, x, y, px * sp.virtual_to_real[1], [1.0; 4], out);
    if now & 0x200 == 0
        && let Some(tex) = image_of(l, "net_disconnect")
    {
        let (x, w) = sp.axis(0, 296.0, 48.0, 0);
        let (y, h) = sp.axis(1, 320.0, 48.0, 0);
        out.push(quad(x, y, w, h, [1.0; 4], tex, false));
    }
}

/// [`font::layout`] with its shadow faded with the text: the shared
/// layout draws every shadow at 0.8 alpha.
pub(crate) fn text(
    f: &Font,
    s: &str,
    x: f32,
    y: f32,
    scale: f32,
    rgba: [f32; 4],
    out: &mut Vec<HudQuad>,
) -> f32 {
    let from = out.len();
    let w = font::layout(f, s, x, y, scale, rgba, out);
    for q in &mut out[from..] {
        if q.rgba == [0.0, 0.0, 0.0, 0.8] {
            q.rgba[3] *= rgba[3];
        }
    }
    w
}

/// [`text`] with its glow (`R_AddCmdDrawTextWithEffects`): nothing more
/// when `glow`'s alpha is 0, else after the text each glyph again from
/// the font's glow material (`fonts/gamefonts_pc_glow`: the page,
/// `SrcAlpha One`) four times, 1.75 by 1.125 its size, centred on it and
/// pushed two glyph pixels to each corner, in `glow`'s colour times 0.1
/// at the glyph's own alpha (`SetDrawText2DGlowParms` `iw3mp.exe`
/// 0x5f6aa0, the pass at 0x6148d6..0x614aff; cod4-hud.md 22).
pub(crate) fn glow_text(
    f: &Font,
    s: &str,
    x: f32,
    y: f32,
    scale: f32,
    rgba: [f32; 4],
    glow: [f32; 4],
    out: &mut Vec<HudQuad>,
) -> f32 {
    let from = out.len();
    let w = text(f, s, x, y, scale, rgba, out);
    if glow[3] == 0.0 {
        return w;
    }
    let px = f.glyph_scale * f.unit_scale() * scale;
    // `font::layout` pushes each glyph after its shadow.
    let glyphs: Vec<([f32; 4], [[f32; 2]; 4], f32)> = out[from..]
        .iter()
        .skip(1)
        .step_by(2)
        .map(|q| {
            let [x0, y0] = q.verts[0];
            let [x1, y1] = q.verts[2];
            ([x0, y0, x1 - x0, y1 - y0], q.uvs, q.rgba[3])
        })
        .collect();
    let page = format!("{}{}", crate::hud::ADDITIVE, f.page);
    for ([gx, gy, gw, gh], uvs, a) in glyphs {
        let (w, h) = (gw * 1.75, gh * 1.125);
        for (ox, oy) in GLOW_OFFSETS {
            let qx = gx - 0.375 * gw + ox * 2.0 * px;
            let qy = gy - 0.0625 * gh + oy * 2.0 * px;
            out.push(HudQuad {
                verts: [[qx, qy], [qx + w, qy], [qx + w, qy + h], [qx, qy + h]],
                uvs,
                rgba: [glow[0] * 0.1, glow[1] * 0.1, glow[2] * 0.1, a],
                texture: page.clone(),
            });
        }
    }
    w
}

/// The glow pass's four offsets in glyph pixels times 2 (`iw3mp.exe`
/// 0x700bec).
const GLOW_OFFSETS: [(f32, f32); 4] = [(-1.0, -1.0), (-1.0, 1.0), (1.0, -1.0), (1.0, 1.0)];
