//! The full map: `common_mp`'s `fullscreenmap`
//! menu while the location selector is up, and the same owner draws
//! (181..188) in the team and class menus (docs/research/cod4-hud.md
//! 15.8). A child of `cod4_hud`, whose loaded zones it draws with.

use super::*;
use vcod_cod4::menuexp;

/// `cg_hudMapBorderWidth` 2, `cg_hudMapPlayerWidth` / `Height` 20 and
/// `cg_hudMapFriendlyWidth` / `Height` 15: 1.7's defaults (the floats
/// `iw3mp.exe` loads at 0x42646e, 0x4263b4, 0x4262fd).
const BORDER: f32 = 2.0;
const PLAYER_SIZE: f32 = 20.0;
const FRIENDLY_SIZE: f32 = 15.0;
/// `compassObjectiveWidth` / `Height` 20.
const OBJECTIVE_SIZE: f32 = 20.0;
/// The selector materials' configstrings (`CS_LOC_SEL_MTLS`, three).
const CS_LOC_SEL_MTLS: u16 = 827;

/// The full map's owner draws.
pub(super) mod od {
    pub const MAP: i32 = 181;
    pub const POINTERS: i32 = 182;
    pub const PLAYER: i32 = 183;
    pub const FRIENDLIES: i32 = 185;
    pub const LOCATOR: i32 = 186;
    pub const BORDER: i32 = 187;
    pub const ENEMIES: i32 = 188;
}

/// The menu the selector shows.
pub(super) const FULLSCREEN_MENU: &str = "fullscreenmap";

/// The item's rect `r` shrunk to the map's aspect and centred, less the
/// border, at most a quarter of a side (cod4-hud.md 15.8).
fn fit_map(map: &MapAxes, r: [f32; 4]) -> [f32; 4] {
    let [x, y, w, h] = r;
    let aspect = map.extent[0] / map.extent[1];
    let fitted = if w / h >= aspect {
        let fw = h * aspect;
        [x + (w - fw) * 0.5, y, fw, h]
    } else {
        let fh = w / aspect;
        [x, y + (h - fh) * 0.5, w, fh]
    };
    let b = BORDER.min(fitted[2] * 0.25).min(fitted[3] * 0.25);
    [
        fitted[0] + b,
        fitted[1] + b,
        fitted[2] - 2.0 * b,
        fitted[3] - 2.0 * b,
    ]
}

/// A world point on the fitted rect `r`, off its centre: its place on the
/// map scaled to the rect (cod4-hud.md 15.8).
fn on_map(map: &MapAxes, r: [f32; 4], at: [f32; 2]) -> [f32; 2] {
    let [e, s] = map.fraction(at);
    [(e - 0.5) * r[2], (s - 0.5) * r[3]]
}

impl Cod4Hud {
    #[allow(clippy::too_many_arguments)]
    /// The `fullscreenmap` menu's visible items, its owner draws and the
    /// "press fire" text: drawn while the selector is up (its items'
    /// `visible when` read `selectingLocation()`).
    pub(super) fn draw_fullscreen_map(
        &self,
        l: &Loaded,
        s: &Cod4Session,
        env: &HudEnv,
        shell: &Shell,
        view: Option<(glam::Vec3, f32)>,
        sp: &ScreenPlacement,
        out: &mut Vec<HudQuad>,
    ) {
        let Some(layout) = l
            .script_menus
            .get(FULLSCREEN_MENU)
            .and_then(|m| m.layout.as_ref())
        else {
            return;
        };
        for it in &layout.items {
            if it.dynamic_flags & vcod_cod4::scriptmenu::WINDOW_VISIBLE == 0
                || !(it.exps.visible.is_empty()
                    || menuexp::truth(&it.exps.visible, env).unwrap_or(false))
            {
                continue;
            }
            if it.owner_draw != 0 {
                self.full_map_owner_draw(l, s, it, it.rect, it.align, view, sp, out);
            } else if let Some(t) = &it.text {
                let t = self.menu_text(t, shell);
                let f = l.fonts.pick(it.font, it.text_scale, sp);
                let px = it.text_scale / f.unit_scale();
                let tw = font::measure(f, &t, px);
                let fh = (48.0 * it.text_scale) as i32 as f32;
                let [x, y, w, hh] = it.rect;
                let tx = match it.text_align {
                    1 => x + 0.5 * w - 0.5 * tw,
                    2 => x + w - tw,
                    _ => x,
                };
                let (rx, _) = sp.axis(0, tx, tw, it.align[0]);
                let (ry, _) = sp.axis(1, y + hh - fh, fh, it.align[1]);
                text(
                    f,
                    &t,
                    rx,
                    ry,
                    px * sp.virtual_to_real[1],
                    it.fore_color,
                    out,
                );
            }
        }
    }

    /// A menu item's text as drawn: an `@KEY` localized (the selector's
    /// `@PLATFORM_PRESS_TO_SET_AIRSTRIKE`, once drawn raw), then each
    /// `[{+cmd}]` as its key.
    pub(super) fn menu_text(&self, t: &str, shell: &Shell) -> String {
        let t = match (t.strip_prefix('@'), self.localized()) {
            (Some(key), Some(l)) => l.get(key).unwrap_or(key).to_string(),
            (Some(key), None) => key.to_string(),
            (None, _) => self.message(t),
        };
        h::replace_directives(&t, &|d| self.key_binding(shell, d))
    }

    /// The `fullscreenmap` text items' texts as [`Self::menu_text`] draws
    /// them.
    #[cfg(test)]
    pub(crate) fn fullscreen_texts(&self, shell: &Shell) -> Vec<String> {
        let Some(Some(l)) = &self.loaded else {
            return Vec::new();
        };
        let items = l
            .script_menus
            .get(FULLSCREEN_MENU)
            .and_then(|m| m.layout.as_ref());
        items
            .into_iter()
            .flat_map(|layout| &layout.items)
            .filter_map(|it| it.text.as_deref())
            .map(|t| self.menu_text(t, shell))
            .collect()
    }

    /// The open script menu's full-map owner draws (the team and class
    /// menus' minimap), over its own quads.
    pub(crate) fn menu_owner_draws(
        &self,
        s: &Cod4Session,
        size: (f32, f32),
        view: Option<(glam::Vec3, f32)>,
    ) -> Vec<HudQuad> {
        let mut out = Vec::new();
        let (Some(Some(l)), Some(open)) = (&self.loaded, s.menus.open()) else {
            return out;
        };
        let Some(layout) = &open.layout else {
            return out;
        };
        let sp = ScreenPlacement::cod4(size.0, size.1);
        for it in &layout.items {
            if (od::MAP..=od::ENEMIES).contains(&it.owner_draw) && s.menus.item_visible(it) {
                let (rect, align) = s.menus.rect(it, layout);
                self.full_map_owner_draw(l, s, it, rect, align, view, &sp, &mut out);
            }
        }
        out
    }

    /// One full-map owner draw (181..188) on the grid rect `rect`
    /// (cod4-hud.md 15.8).
    #[allow(clippy::too_many_arguments)]
    fn full_map_owner_draw(
        &self,
        l: &Loaded,
        s: &Cod4Session,
        it: &vcod_cod4::scriptmenu::LayoutItem,
        rect: [f32; 4],
        align: [i32; 2],
        view: Option<(glam::Vec3, f32)>,
        sp: &ScreenPlacement,
        out: &mut Vec<HudQuad>,
    ) {
        let Some((material, map)) = self.minimap(s) else {
            return;
        };
        let r = fit_map(&map, rect);
        let place = |x: f32, y: f32, w: f32, hh: f32| {
            let (rx, rw) = sp.axis(0, x, w, align[0]);
            let (ry, rh) = sp.axis(1, y, hh, align[1]);
            [rx, ry, rw, rh]
        };
        let centre = [r[0] + r[2] * 0.5, r[1] + r[3] * 0.5];
        let icon = |out: &mut Vec<HudQuad>,
                    material: &str,
                    at: [f32; 2],
                    size: f32,
                    angle: f32,
                    alpha: f32| {
            let Some(tex) = image_of(l, material) else {
                return;
            };
            let o = on_map(&map, r, at);
            let x = centre[0] + o[0] - size * 0.5;
            let y = centre[1] + o[1] - size * 0.5;
            out.push(turned(
                place(x, y, size, size),
                angle,
                [1.0, 1.0, 1.0, alpha],
                tex,
            ));
        };
        let background = it.background.as_deref();
        match it.owner_draw {
            od::MAP => {
                if let Some(tex) = image_of(l, &material) {
                    let [x, y, w, hh] = place(r[0], r[1], r[2], r[3]);
                    out.push(quad(x, y, w, hh, it.fore_color, tex, false));
                }
            }
            od::LOCATOR => {
                let info = s.selecting_location();
                let radius = (info >> 2) as f32 / 63.0;
                let slot = (info & 3).checked_sub(1);
                let material = slot.map(|k| self.configstring(s, CS_LOC_SEL_MTLS + k as u16));
                if let (Some(mat), true) = (material, radius > 0.0)
                    && let Some(tex) = image_of(l, &mat)
                {
                    let ([lx, ly, lw, lh], [st0, st1]) = locator_quad(r, s.selector.at, radius);
                    let [x, y, w, hh] = place(lx, ly, lw, lh);
                    out.push(HudQuad {
                        verts: [[x, y], [x + w, y], [x + w, y + hh], [x, y + hh]],
                        uvs: [
                            [st0[0], st0[1]],
                            [st1[0], st0[1]],
                            [st1[0], st1[1]],
                            [st0[0], st1[1]],
                        ],
                        rgba: it.fore_color,
                        texture: tex,
                    });
                }
            }
            od::POINTERS => {
                let Some(snap) = s.client().snapshots.last() else {
                    return;
                };
                for mark in self.objective_marks(s, snap) {
                    let at = [mark.at.x, mark.at.y];
                    icon(out, &mark.material, at, OBJECTIVE_SIZE, 0.0, 1.0);
                }
            }
            od::FRIENDLIES => {
                let Some(snap) = s.client().snapshots.last() else {
                    return;
                };
                let me = snap.ps.get("clientNum").unwrap_or(0);
                let team = |n: u32| {
                    snap.clients
                        .iter()
                        .find(|c| c.0[0] == n)
                        .map_or(0, |c| client_word(c, "team"))
                };
                let mine = team(me);
                if !matches!(mine, 1 | 2) {
                    return;
                }
                for e in snap
                    .entities
                    .iter()
                    .filter(|e| e.etype() == crate::play::cod4::ET_PLAYER)
                {
                    let n = crate::play::cod4::ent_u32(e, "clientNum");
                    if n == me || team(n) != mine {
                        continue;
                    }
                    let yaw = crate::play::cod4::ent_f32(e, "lerp.apos.trBase[1]");
                    let at = crate::play::cod4::origin_of(e);
                    let angle = angle_delta(map.north_yaw, yaw);
                    icon(
                        out,
                        "compassping_friendly_mp",
                        [at.x, at.y],
                        FRIENDLY_SIZE,
                        angle,
                        1.0,
                    );
                }
            }
            od::ENEMIES => {
                let show_all = self
                    .server_dvar(s, "g_compassShowEnemies")
                    .is_some_and(|v| menuexp::atoi(&v) != 0);
                for p in self.radar.pings(self.radar.time(), show_all) {
                    let material = if p.firing {
                        "compassping_enemyfiring"
                    } else {
                        "compassping_enemy"
                    };
                    icon(out, material, p.pos, FRIENDLY_SIZE, 0.0, p.alpha);
                }
            }
            od::PLAYER => {
                if let (Some((eye, yaw)), Some(mat)) = (view, background) {
                    icon(
                        out,
                        mat,
                        [eye.x, eye.y],
                        PLAYER_SIZE,
                        angle_delta(map.north_yaw, yaw),
                        1.0,
                    );
                }
            }
            od::BORDER => {
                let Some(tex) = background.and_then(|b| image_of(l, b)) else {
                    return;
                };
                for ([px, py, pw, ph], st) in border_pieces(r) {
                    let [qx, qy, qw, qh] = place(px, py, pw, ph);
                    out.push(HudQuad {
                        verts: [[qx, qy], [qx + qw, qy], [qx + qw, qy + qh], [qx, qy + qh]],
                        uvs: [
                            [st[0], st[1]],
                            [st[2], st[1]],
                            [st[2], st[3]],
                            [st[0], st[3]],
                        ],
                        rgba: it.fore_color,
                        texture: tex.clone(),
                    });
                }
            }
            _ => {}
        }
    }
}

/// The selector's material as a square `radius` of the rect's height on
/// a side, centred on the cursor (`at`, 0..1 across the rect), cut to
/// the rect: the visible rect and its texture's S and T at the upper
/// left and lower right corners (cod4-hud.md 15.8).
fn locator_quad(r: [f32; 4], at: [f32; 2], radius: f32) -> ([f32; 4], [[f32; 2]; 2]) {
    let side = radius * r[3];
    let mut shown = [0.0; 4];
    let mut st = [[0.0; 2]; 2];
    for k in 0..2 {
        let cursor = r[k] + r[k + 2] * at[k];
        let near = (cursor - side * 0.5).max(r[k]);
        let far = (cursor + side * 0.5).min(r[k] + r[k + 2]);
        shown[k] = near;
        shown[k + 2] = far - near;
        st[0][k] = (near - cursor) / side + 0.5;
        st[1][k] = (far - cursor) / side + 0.5;
    }
    (shown, st)
}

/// `map_border` around the fitted rect `r` (cod4-hud.md 15.8): a corner
/// square twice the border wide on each corner, showing a quarter of the
/// texture, and the edges between them stretching the texture's middle
/// line. Each piece is its rect and its `[s0, t0, s1, t1]`.
fn border_pieces(r: [f32; 4]) -> Vec<([f32; 4], [f32; 4])> {
    let b = BORDER.min(r[2] * 0.5).min(r[3] * 0.5);
    // Along one axis: the near corner, the stretch, the far corner, each
    // as (start, length, texture from, texture to).
    let bands = |start: f32, len: f32| {
        [
            (start - b, 2.0 * b, 0.0, 0.5),
            (start + b, len - 2.0 * b, 0.5, 0.5),
            (start + len - b, 2.0 * b, 0.5, 1.0),
        ]
    };
    let (columns, rows) = (bands(r[0], r[2]), bands(r[1], r[3]));
    let mut out = Vec::with_capacity(8);
    for (i, c) in columns.iter().enumerate() {
        for (j, w) in rows.iter().enumerate() {
            if (i, j) != (1, 1) {
                out.push(([c.0, w.0, c.1, w.1], [c.2, w.2, c.3, w.3]));
            }
        }
    }
    out
}

/// `AngleDelta(a, b)`: `a - b` in -180..180.
fn angle_delta(a: f32, b: f32) -> f32 {
    let d = (a - b) % 360.0;
    if d > 180.0 {
        d - 360.0
    } else if d < -180.0 {
        d + 360.0
    } else {
        d
    }
}

/// A real-pixel rect turned `angle` degrees clockwise about its centre.
fn turned([x, y, w, h]: [f32; 4], angle: f32, rgba: [f32; 4], texture: String) -> HudQuad {
    let (s, c) = angle.to_radians().sin_cos();
    let centre = [x + 0.5 * w, y + 0.5 * h];
    let corner = |u: f32, v: f32| {
        let (dx, dy) = (u * 0.5 * w, v * 0.5 * h);
        [centre[0] + dx * c - dy * s, centre[1] + dx * s + dy * c]
    };
    HudQuad {
        verts: [
            corner(-1.0, -1.0),
            corner(1.0, -1.0),
            corner(1.0, 1.0),
            corner(-1.0, 1.0),
        ],
        uvs: [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]],
        rgba,
        texture,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close<const N: usize>(got: [f32; N], want: [f32; N]) {
        let off = got.iter().zip(&want).any(|(g, w)| (g - w).abs() > 1e-3);
        assert!(!off, "{got:?} != {want:?}");
    }

    /// A map's size along east and south (1000 where one is 0), the item's
    /// rect fitted to its aspect less the border, and world points placed
    /// off that rect's centre (cod4-hud.md 15.8).
    #[test]
    fn the_full_map_fits_its_rect_and_places_points() {
        let tilted = MapAxes::new(30.0, [-1500.0, 2200.0], [2600.0, -900.0]);
        close(tilted.extent, [4734.6787, -2000.7041]);
        let square = MapAxes::new(0.0, [-1000.0, 1000.0], [1000.0, -3000.0]);
        close(square.extent, [4000.0, -2000.0]);
        let flat = MapAxes::new(0.0, [5.0, 5.0], [5.0, 5.0]);
        close(flat.extent, [1000.0, 1000.0]);
        let menu = [-100.0, -100.0, 400.0, 400.0];
        let cases = [
            (
                &tilted,
                menu,
                [336.65063, -336.65063, -473.30127, 873.3013],
                [
                    [-28.78152, -349.77112],
                    [236.65063, -436.65063],
                    [-42.79582, 18.117397],
                    [-351.87286, 347.7843],
                ],
            ),
            (
                &square,
                menu,
                [300.0, -300.0, -400.0, 800.0],
                [
                    [100.0, 0.0],
                    [320.0, -600.0],
                    [130.0, 320.0],
                    [-100.0, 1200.0],
                ],
            ),
            (
                &tilted,
                [0.0, 0.0, 100.0, 300.0],
                [227.48798, -177.48798, -354.97595, 654.97595],
                [
                    [-21.58614, -262.32834],
                    [177.48798, -327.48798],
                    [-32.096867, 13.588048],
                    [-263.90463, 260.83823],
                ],
            ),
            (
                &flat,
                [0.0, 0.0, 6.0, 4.0],
                [2.0, 1.0, 2.0, 2.0],
                [[-0.99, -0.99], [-5.39, 2.01], [-1.59, -2.59], [3.01, -6.99]],
            ),
        ];
        let points = [
            [0.0, 0.0],
            [-1500.0, 2200.0],
            [800.0, 300.0],
            [3000.0, -2000.0],
        ];
        for (m, item, want, placed) in cases {
            let r = fit_map(m, item);
            close(r, want);
            for (at, want) in points.iter().zip(placed) {
                close(on_map(m, r, *at), want);
            }
        }
    }

    /// The selector's square on the cursor, clipped to the rect with its
    /// texture coordinates cut to match.
    #[test]
    fn the_locator_clips_to_the_map() {
        let cases = [
            (
                [10.0, 20.0, 300.0, 200.0],
                [0.5, 0.5],
                0.2,
                [140.0, 100.0, 40.0, 40.0],
                [0.0, 0.0, 1.0, 1.0],
            ),
            (
                [10.0, 20.0, 300.0, 200.0],
                [0.02, 0.97],
                0.5,
                [10.0, 164.0, 56.0, 56.0],
                [0.44, 0.0, 1.0, 0.56],
            ),
            (
                [0.0, 0.0, 100.0, 100.0],
                [0.9, 0.1],
                3.0,
                [0.0, 0.0, 100.0, 100.0],
                [0.2, 0.46666667, 0.53333336, 0.8],
            ),
        ];
        for (r, at, radius, rect, st) in cases {
            let (got, [st0, st1]) = locator_quad(r, at, radius);
            close(got, rect);
            close([st0[0], st0[1], st1[0], st1[1]], st);
        }
    }

    /// `map_border`'s corners twice the border square on the rect's
    /// corners, its edges between them on the texture's middle line.
    #[test]
    fn the_border_frames_the_map() {
        let mut got = border_pieces([10.0, 20.0, 300.0, 200.0]);
        let mut want = vec![
            ([8.0, 218.0, 4.0, 4.0], [0.0, 0.5, 0.5, 1.0]),
            ([8.0, 18.0, 4.0, 4.0], [0.0, 0.0, 0.5, 0.5]),
            ([308.0, 218.0, 4.0, 4.0], [0.5, 0.5, 1.0, 1.0]),
            ([308.0, 18.0, 4.0, 4.0], [0.5, 0.0, 1.0, 0.5]),
            ([8.0, 22.0, 4.0, 196.0], [0.0, 0.5, 0.5, 0.5]),
            ([308.0, 22.0, 4.0, 196.0], [0.5, 0.5, 1.0, 0.5]),
            ([12.0, 18.0, 296.0, 4.0], [0.5, 0.0, 0.5, 0.5]),
            ([12.0, 218.0, 296.0, 4.0], [0.5, 0.5, 0.5, 1.0]),
        ];
        let key = |p: &([f32; 4], [f32; 4])| p.0.map(f32::to_bits);
        got.sort_by_key(key);
        want.sort_by_key(key);
        assert_eq!(got, want);
        // A rect under twice the border: the border is half its side.
        let small = border_pieces([0.0, 0.0, 3.0, 3.0]);
        assert!(
            small.iter().all(|(r, _)| r[2].max(r[3]) == 3.0),
            "{small:?}"
        );
    }
}
