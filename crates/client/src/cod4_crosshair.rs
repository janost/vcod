//! The CoD4 crosshair and a sight's overlay, drawn off the drawn
//! playerstate and the view weapon's def as 1.7's `CG_DrawCrosshair`
//! (`iw3mp.exe` ..0x43143e) does: on a turret the gun's own reticle; else
//! the sight's overlay while it zooms in, then the centre dot and the four
//! side pieces, pushed out by the spread and faded as it grows
//! (docs/research/cod4-hud.md 17).

use crate::cod4_viewmodel::ViewPs;
use crate::hud::HudQuad;
use crate::hud::cod2elem::ScreenPlacement;
use vcod_cod4::hud::{Reticle, WeaponHud};

/// The crosshair dvars, 1.7's defaults (cod4-hud.md 17).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Dvars {
    /// `cg_drawCrosshair` and `ui_drawCrosshair`.
    pub draw: bool,
    pub alpha: f32,
    pub alpha_min: f32,
    pub dynamic: bool,
    pub draw_gun: bool,
    pub enemy_colour: bool,
    /// `cg_drawTurretCrosshair` (1, archived: `iw3mp.exe` 0x43b194).
    pub draw_turret: bool,
}

impl Default for Dvars {
    fn default() -> Self {
        Dvars {
            draw: true,
            alpha: 1.0,
            alpha_min: 0.5,
            dynamic: false,
            draw_gun: true,
            enemy_colour: true,
            draw_turret: true,
        }
    }
}

impl Dvars {
    /// The shell's values over the defaults.
    pub fn read(shell: &crate::console::shell::Shell) -> Dvars {
        let d = Dvars::default();
        let flag = |n: &str, def: bool| shell.cvar(n).map_or(def, |v| v.trim() != "0");
        let num = |n: &str, def: f32| {
            shell
                .cvar(n)
                .and_then(|v| v.trim().parse().ok())
                .unwrap_or(def)
        };
        Dvars {
            draw: flag("cg_drawCrosshair", d.draw) && flag("ui_drawCrosshair", true),
            alpha: num("cg_crosshairAlpha", d.alpha).clamp(0.0, 1.0),
            alpha_min: num("cg_crosshairAlphaMin", d.alpha_min).clamp(0.0, 1.0),
            dynamic: flag("cg_crosshairDynamic", d.dynamic),
            draw_gun: flag("cg_drawGun", d.draw_gun),
            enemy_colour: flag("cg_crosshairEnemyColor", d.enemy_colour),
            draw_turret: flag("cg_drawTurretCrosshair", d.draw_turret),
        }
    }
}

/// What one frame's crosshair reads.
pub(crate) struct View<'a> {
    pub ps: &'a ViewPs,
    /// The view weapon's def (`ViewPs::view_weapon`).
    pub def: &'a WeaponHud,
    /// The drawn fov's half-angle tangents.
    pub tan_half_fov_y: f32,
    pub tan_half_fov_x: f32,
    /// On a turret (`viewlocked`), the def of the gun the playerstate
    /// is locked to (`viewlocked_entNum`'s `weapon`).
    pub turret: Option<&'a WeaponHud>,
    /// The sight is coming up rather than going down: the `In` fractions
    /// apply, else the `Out` ones.
    pub to_ads: bool,
    /// `g_TeamColor_MyTeam` and `g_TeamColor_EnemyTeam`.
    pub team_rgb: [[f32; 3]; 2],
}

/// `eFlags` 0x300: on a turret, whose crosshair this is not.
const EF_TURRET: u32 = 0x300;
/// `weapFlags` bits naming who is under the crosshair (cod4-hud.md 17).
const WF_FRIENDLY: u32 = 8;
const WF_ENEMY: u32 = 0x10;
/// `weapType` 1: a grenade, whose dot grows while it cooks.
const WEAPTYPE_GRENADE: i32 = 1;
/// The grid's half height, which the spread and the sight's pitch project
/// onto (the 240 at `iw3mp.exe` 0x70b548).
const HALF_GRID_H: f32 = 240.0;

/// The frame's quads, `texture` naming a material's image.
pub(crate) fn quads(
    v: &View,
    dv: &Dvars,
    sp: &ScreenPlacement,
    texture: &dyn Fn(&str) -> Option<String>,
) -> Vec<HudQuad> {
    let mut c = Canvas {
        sp,
        texture,
        quads: Vec::new(),
    };
    if v.ps.eflags & EF_TURRET != 0 {
        if let Some(gun) = v.turret {
            c.turret(v, gun, dv);
        }
    } else if v.ps.view_weapon() != 0 {
        c.weapon(v, dv);
    }
    c.quads
}

/// `CG_GetWeapReticleZoom` true: the held weapon's sight overlay is up.
pub(crate) fn overlay_up(ps: &ViewPs, def: &WeaponHud) -> bool {
    let to_ads = ps.pm_flags & crate::cod4_viewmodel::PMF_SIGHT != 0;
    overlay_zoom(&def.reticle, ps.pos_frac, to_ads).is_some()
}

/// How far the sight's lerp `pos` has run past where a phase starts,
/// `1 - frac` of the way, before dividing by `frac`; the `In` fraction
/// while the sight comes up, `Out` while it goes down.
fn past_start(pos: f32, fracs: [f32; 2], to_ads: bool) -> (f32, f32) {
    let frac = if to_ads { fracs[0] } else { fracs[1] };
    (pos - (1.0 - frac), frac)
}

/// The overlay's alpha (`CG_GetWeapReticleZoom`): the lerp past
/// `1 - adsZoomInFrac` over that fraction, capped at 1; `None` at 0.01
/// or under, at rest, or for a weapon with no overlay.
fn overlay_zoom(r: &Reticle, pos: f32, to_ads: bool) -> Option<f32> {
    let scoped = r.overlay.is_some() || r.overlay_reticle != 0;
    if !scoped || pos == 0.0 {
        return None;
    }
    let (mut zoom, frac) = past_start(pos, r.ads_zoom_frac, to_ads);
    if zoom > 0.0 {
        zoom /= frac;
    }
    (zoom > 0.01).then_some(zoom.min(1.0))
}

/// `CG_TransitionToAds` (0x430c80): the crosshair's scale and its drop
/// in grid units as the sight comes up past `1 - adsCrosshairInFrac`,
/// down to half and `adsAimPitch` below the centre at full sight.
fn sight_shrink(v: &View, pos: f32) -> (f32, f32) {
    let r = &v.def.reticle;
    let (run, frac) = past_start(pos, r.ads_crosshair_frac, v.to_ads);
    if run <= 0.0 || frac == 0.0 {
        return (1.0, 0.0);
    }
    let t = run / frac;
    let drop = t * HALF_GRID_H / v.tan_half_fov_y * r.ads_aim_pitch.to_radians().tan();
    (1.0 - t * 0.5, drop)
}

/// `CG_CalcCrosshairPosition` (0x42ff80): the gun's direction projected
/// at the drawn fov, in grid units off the centre, x right and y down
/// (the -320 and -240 at 0x70b358 and 0x70b350). The centre while the gun
/// points along the view or behind the eye.
fn aim_point(v: &View) -> [f32; 2] {
    match v.ps.gun_dir {
        Some([ahead, left, up]) if ahead > 0.0 && v.tan_half_fov_x > 0.0 => [
            left / (ahead * v.tan_half_fov_x) * -320.0,
            up / (ahead * v.tan_half_fov_y) * -240.0,
        ],
        _ => [0.0, 0.0],
    }
}

/// `CG_CalcCrosshairColor` (0x430960): white, or with
/// `crosshairColorChange` the team's colour over a friend and the enemy's
/// over an enemy (under `cg_crosshairEnemyColor`), at `alpha` times
/// `cg_crosshairAlpha`. The name fade-in's wait is not modelled.
fn tint(v: &View, dv: &Dvars, alpha: f32) -> [f32; 4] {
    let flags = v.ps.weap_flags;
    let [r, g, b] = match v.def.reticle.color_change {
        true if flags & WF_FRIENDLY != 0 => v.team_rgb[0],
        true if flags & WF_ENEMY != 0 && dv.enemy_colour => v.team_rgb[1],
        _ => [1.0; 3],
    };
    [r, g, b, alpha * dv.alpha]
}

/// The crosshair test (0x430b50): the dvars on, and no weapon state
/// 1..4, 7 or 12..14 (raising, dropping, sprinting and the like).
fn allowed(ps: &ViewPs, dv: &Dvars) -> bool {
    let busy = matches!(ps.weaponstate, 1..=4 | 7 | 12..=14);
    dv.draw && !busy
}

/// `CG_CalcReticleSpread` (0x430e40): how far out from the aim point the
/// side pieces of grid size `side` sit at the sight's `scale`: the hip
/// spread `aimSpreadScale` has reached between its two ends
/// (`Reticle::spread`), projected at the drawn fov, at least
/// `reticleMinOfs`, less `hipReticleSidePos` of a piece.
fn side_gap(v: &View, side: f32, scale: f32) -> f32 {
    let ps = v.ps;
    let r = &v.def.reticle;
    let (lo, hi) = r.spread(
        ps.view_height_current,
        ps.perks,
        ps.spread_override_state,
        ps.spread_override,
    );
    let degrees = ((hi - lo) * (ps.aim_spread_scale / 255.0) + lo) * scale;
    let projected = degrees.to_radians().tan() * HALF_GRID_H / v.tan_half_fov_y;
    projected.max(r.min_ofs as f32) - r.side_pos * side
}

/// The frame's quads as they are drawn.
struct Canvas<'a> {
    sp: &'a ScreenPlacement,
    texture: &'a dyn Fn(&str) -> Option<String>,
    quads: Vec<HudQuad>,
}

impl Canvas<'_> {
    fn image(&self, material: Option<&str>) -> Option<String> {
        material.and_then(self.texture)
    }

    /// The overlay's image: the low-res one on a window of 480 lines or
    /// fewer when the weapon has one.
    fn overlay_image(&self, r: &Reticle) -> Option<String> {
        let low_res = self.sp.real_size[1] <= 480.0;
        let pick = match (&r.overlay_low_res, low_res) {
            (Some(low), true) => Some(low),
            _ => r.overlay.as_ref(),
        };
        self.image(pick.map(String::as_str))
    }

    /// On a turret (0x4311e0..0x431294): a gun with a sight overlay draws
    /// it whole; else `CG_DrawTurretCrossHair` (0x430a50) the gun's
    /// `reticleCenter` square at the centre in the crosshair's colour,
    /// under `cg_drawTurretCrosshair` and a `cg_crosshairAlpha` of at
    /// least 0.01.
    fn turret(&mut self, v: &View, gun: &WeaponHud, dv: &Dvars) {
        let r = &gun.reticle;
        if r.overlay.is_some() {
            if let Some(img) = self.overlay_image(r) {
                self.scope(r.overlay_size, [1.0; 4], &img);
            }
            return;
        }
        if !dv.draw_turret || dv.alpha < 0.01 {
            return;
        }
        if let Some(img) = self.image(r.center.as_deref()) {
            let size = r.center_size as f32;
            let corner = -0.5 * size;
            self.piece([corner, corner], size, 0.0, tint(v, dv, 1.0), img);
        }
    }

    /// `CG_DrawWeapReticle` (0x4308c0): the sight's overlay at its zoom,
    /// returning the alpha the crosshair keeps.
    fn sight_overlay(&mut self, v: &View) -> f32 {
        let r = &v.def.reticle;
        let Some(zoom) = overlay_zoom(r, v.ps.pos_frac, v.to_ads) else {
            return 1.0;
        };
        if let Some(img) = self.overlay_image(r) {
            self.scope(r.overlay_size, [1.0, 1.0, 1.0, zoom], &img);
        }
        1.0 - zoom
    }

    /// A held weapon's crosshair: the overlay, the ADS aim indicator, the
    /// centre dot and the sides.
    fn weapon(&mut self, v: &View, dv: &Dvars) {
        let ps = v.ps;
        let r = &v.def.reticle;
        let sight = ps.pos_frac;
        let full_sight = sight == 1.0;
        let rgba = tint(v, dv, self.sight_overlay(v));
        if rgba[3] < 0.01 || (full_sight && dv.draw_gun) || !allowed(ps, dv) {
            return;
        }
        let aim = aim_point(v);
        let (scale, drop) = if sight == 0.0 {
            (1.0, 0.0)
        } else {
            sight_shrink(v, sight)
        };
        // The ADS aim indicator (0x430bc0): with the gun hidden, a dot at
        // the aim point growing as the sight comes up.
        if sight != 0.0
            && !dv.draw_gun
            && scale < 1.0
            && let Some(img) = self.image(r.center.as_deref())
        {
            let size = (1.5 - scale) * r.center_size as f32;
            self.centred(aim, size, rgba, img);
        }
        if full_sight && !dv.draw_gun {
            return;
        }
        // `cg_crosshairDynamic` 0: the centre, dropped by the sight's
        // pitch; 1: the aim point.
        let at = if dv.dynamic { aim } else { [0.0, drop] };
        // `CG_DrawReticleCenter` (0x430d50); a cooking grenade's dot grows
        // a tenth of a unit for each 10 ms left in the fuse's second.
        if let Some(img) = self.image(r.center.as_deref()) {
            let cooking =
                v.def.weap_type == WEAPTYPE_GRENADE && r.cook_off_hold && ps.grenade_time_left != 0;
            let grow = if cooking {
                (ps.grenade_time_left % 1000) as f32 / 100.0
            } else {
                0.0
            };
            self.centred(at, (r.center_size as f32 + grow) * scale, rgba, img);
        }
        self.sides(v, dv, at, scale, rgba);
    }

    /// `CG_DrawReticleSides` (0x430f60). It runs the overlay's pass again
    /// for its own alpha, so a zooming sight's overlay is drawn twice.
    fn sides(&mut self, v: &View, dv: &Dvars, [x, y]: [f32; 2], scale: f32, rgba: [f32; 4]) {
        let r = &v.def.reticle;
        let Some(img) = self.image(r.side.as_deref()) else {
            return;
        };
        let kept = self.sight_overlay(v);
        let fade = kept * dv.alpha * (1.0 - v.ps.aim_spread_scale / 255.0);
        let alpha = fade.max(dv.alpha_min);
        if alpha < 0.01 {
            return;
        }
        let rgba = [rgba[0], rgba[1], rgba[2], alpha];
        let side = r.side_size as f32 * scale;
        let gap = side_gap(v, side, scale);
        let nudge = side / 8.0;
        let half = side * 0.5;
        // Top, right, bottom and left, each turned a quarter further.
        let corners = [
            [x - half, y - side - gap - nudge],
            [x + gap, y - half],
            [x - half - nudge, y + gap],
            [x - side - gap - nudge, y - half - nudge],
        ];
        for (corner, turn) in corners.into_iter().zip([0.0, 90.0, 180.0, 270.0]) {
            self.piece(corner, side, turn, rgba, img.clone());
        }
    }

    /// A `size` square centred on the grid point `at`.
    fn centred(&mut self, [x, y]: [f32; 2], size: f32, rgba: [f32; 4], img: String) {
        self.piece([x - size * 0.5, y - size * 0.5], size, 0.0, rgba, img);
    }

    /// A `size` square at the grid corner `corner` off the screen's
    /// centre, turned `turn` degrees clockwise about its middle.
    fn piece(&mut self, [x, y]: [f32; 2], size: f32, turn: f32, rgba: [f32; 4], img: String) {
        let (left, w) = self.sp.axis(0, x, size, 2);
        let (top, h) = self.sp.axis(1, y, size, 2);
        let mid = [left + 0.5 * w, top + 0.5 * h];
        let (sin, cos) = turn.to_radians().sin_cos();
        let at = |sx: f32, sy: f32| {
            let (dx, dy) = (sx * 0.5 * w, sy * 0.5 * h);
            [mid[0] + dx * cos - dy * sin, mid[1] + dx * sin + dy * cos]
        };
        self.quads.push(HudQuad {
            verts: [at(-1.0, -1.0), at(1.0, -1.0), at(1.0, 1.0), at(-1.0, 1.0)],
            uvs: [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]],
            rgba,
            texture: img,
        });
    }

    /// A sight's overlay (cod4-hud.md 17, "The overlay"): `size` grid
    /// units about the centre, mirrored into four quarters when it fits a
    /// quarter of the grid, and the window around it filled by stretching
    /// its edge texels.
    fn scope(&mut self, [w, h]: [f32; 2], rgba: [f32; 4], img: &str) {
        let sp = self.sp;
        let to_real = |x: f32, y: f32, w: f32, h: f32| {
            let (rx, rw) = sp.axis(0, x, w, 2);
            let (ry, rh) = sp.axis(1, y, h, 2);
            [rx, ry, rw, rh]
        };
        let mut stretched = |rect: [f32; 4], st: [f32; 4]| {
            self.quads.push(rect_quad(rect, st, rgba, img));
        };
        let inner = if w <= 320.0 && h <= 240.0 {
            // Each quarter flips its texture away from the centre.
            for (x, y, s0, t0) in [
                (-w, -h, 0.0, 0.0),
                (0.0, -h, 1.0, 0.0),
                (-w, 0.0, 0.0, 1.0),
                (0.0, 0.0, 1.0, 1.0),
            ] {
                stretched(to_real(x, y, w, h), [s0, t0, 1.0 - s0, 1.0 - t0]);
            }
            to_real(-w, -h, 2.0 * w, 2.0 * h)
        } else {
            let whole = to_real(-0.5 * w, -0.5 * h, w, h);
            stretched(whole, [0.0, 0.0, 1.0, 1.0]);
            whole
        };
        let [x0, y0, iw, ih] = inner;
        let (x1, y1) = (x0 + iw, y0 + ih);
        let [win_w, win_h] = sp.real_size;
        // The bars take the texture's corner texel's column or row.
        let column = [0.0, 0.0, 0.0, 1.0];
        let row = [0.0, 0.0, 1.0, 0.0];
        let bars = [
            (x0 > 0.0, [0.0, 0.0, x0, win_h], column),
            (win_w > x1, [x1, 0.0, win_w - x1, win_h], column),
            (y0 > 0.0, [x0, 0.0, iw, y0], row),
            (win_h > y1, [x0, y1, iw, win_h - y1], row),
        ];
        for (_, rect, st) in bars.into_iter().filter(|b| b.0) {
            stretched(rect, st);
        }
    }
}

/// A real-pixel rect `[x, y, w, h]` with its texture's `[s0, t0, s1, t1]`.
fn rect_quad(
    [x, y, w, h]: [f32; 4],
    [s0, t0, s1, t1]: [f32; 4],
    rgba: [f32; 4],
    img: &str,
) -> HudQuad {
    HudQuad {
        verts: [[x, y], [x + w, y], [x + w, y + h], [x, y + h]],
        uvs: [[s0, t0], [s1, t0], [s1, t1], [s0, t1]],
        rgba,
        texture: img.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fmt::Write;

    fn rifle() -> WeaponHud {
        let mut d = WeaponHud::default();
        let r = &mut d.reticle;
        r.center = Some("reticle_center".into());
        r.side = Some("reticle_side".into());
        r.center_size = 4;
        r.side_size = 16;
        r.min_ofs = 10;
        r.side_pos = 0.25;
        r.color_change = true;
        r.hip_spread = [3.0, 2.5, 2.0, 10.0, 8.0, 6.0];
        r.ads_zoom_frac = [0.5, 0.3];
        r.ads_crosshair_frac = [0.4, 0.6];
        r.ads_aim_pitch = 1.5;
        d
    }

    fn sniper(size: [f32; 2]) -> WeaponHud {
        let mut d = rifle();
        d.reticle.overlay = Some("scope".into());
        d.reticle.overlay_low_res = Some("scope_lowres".into());
        d.reticle.overlay_size = size;
        d
    }

    fn ps() -> ViewPs {
        ViewPs {
            pm_type: 0,
            weapon: 3,
            weaponstate: 0,
            view_height_current: 60.0,
            aim_spread_scale: 100.0,
            ..ViewPs::default()
        }
    }

    /// Every quad's texture, colour, corners and texture coordinates, at
    /// full float precision.
    fn dump(q: &[HudQuad]) -> String {
        let mut s = String::new();
        for q in q {
            writeln!(s, "{} {:?} {:?} {:?}", q.texture, q.rgba, q.verts, q.uvs).unwrap();
        }
        s
    }

    fn draw(ps: &ViewPs, def: &WeaponHud, dv: &Dvars, size: [f32; 2], to_ads: bool) -> String {
        draw_with(ps, def, None, dv, size, to_ads, &|m| {
            Some(format!("img/{m}"))
        })
    }

    fn draw_with(
        ps: &ViewPs,
        def: &WeaponHud,
        turret: Option<&WeaponHud>,
        dv: &Dvars,
        size: [f32; 2],
        to_ads: bool,
        texture: &dyn Fn(&str) -> Option<String>,
    ) -> String {
        let v = View {
            ps,
            def,
            tan_half_fov_y: 0.7,
            tan_half_fov_x: 0.9,
            turret,
            to_ads,
            team_rgb: [[0.2, 0.4, 1.0], [1.0, 0.1, 0.1]],
        };
        let sp = ScreenPlacement::scr_place(size[0], size[1], 1.0);
        dump(&quads(&v, dv, &sp, texture))
    }

    /// A digest of every case below, fixed before the module's rewrite so
    /// the rewrite draws the same quads to the bit.
    fn all_cases() -> String {
        let dv = Dvars::default();
        let mut s = String::new();
        let mut case = |name: &str, out: String| {
            writeln!(s, "# {name}\n{out}").unwrap();
        };
        let wide = [1280.0, 720.0];
        let vga = [640.0, 480.0];
        case("hip", draw(&ps(), &rifle(), &dv, vga, false));
        case("hip wide", draw(&ps(), &rifle(), &dv, wide, false));
        let mut p = ps();
        p.weap_flags = 0x10;
        p.view_height_current = 30.0;
        p.perks = 2;
        p.aim_spread_scale = 255.0;
        case("enemy crouched", draw(&p, &rifle(), &dv, wide, false));
        p.weap_flags = 8;
        p.view_height_current = 11.0;
        p.aim_spread_scale = 0.0;
        case("friend prone", draw(&p, &rifle(), &dv, wide, false));
        let no_enemy = Dvars {
            enemy_colour: false,
            alpha: 0.7,
            alpha_min: 0.2,
            ..dv
        };
        p.weap_flags = 0x10;
        p.aim_spread_scale = 200.0;
        case(
            "enemy colour off",
            draw(&p, &rifle(), &no_enemy, wide, false),
        );
        let mut p = ps();
        p.spread_override_state = 2;
        p.spread_override = 1;
        case("override", draw(&p, &rifle(), &dv, wide, false));
        for pos in [0.3, 0.6, 0.85, 1.0] {
            let mut p = ps();
            p.pos_frac = pos;
            p.gun_dir = Some([0.99, 0.05, -0.03]);
            for to_ads in [true, false] {
                case(
                    &format!("rifle pos {pos} in {to_ads}"),
                    draw(&p, &rifle(), &dv, wide, to_ads),
                );
                let dynamic = Dvars {
                    dynamic: true,
                    draw_gun: false,
                    ..dv
                };
                case(
                    &format!("rifle gunless dynamic pos {pos} in {to_ads}"),
                    draw(&p, &rifle(), &dynamic, wide, to_ads),
                );
                for (size, w) in [([200.0, 200.0], wide), ([400.0, 300.0], vga)] {
                    case(
                        &format!("sniper {size:?} {w:?} pos {pos} in {to_ads}"),
                        draw(&p, &sniper(size), &dv, w, to_ads),
                    );
                }
            }
        }
        let mut p = ps();
        p.gun_dir = Some([-0.5, 0.2, 0.1]);
        let dynamic = Dvars {
            dynamic: true,
            ..dv
        };
        case("gun behind", draw(&p, &rifle(), &dynamic, wide, false));
        p.gun_dir = Some([0.8, -0.3, 0.2]);
        case("gun turned", draw(&p, &rifle(), &dynamic, wide, false));
        let mut nade = rifle();
        nade.weap_type = WEAPTYPE_GRENADE;
        nade.reticle.cook_off_hold = true;
        let mut p = ps();
        p.grenade_time_left = 2345;
        case("cooking", draw(&p, &nade, &dv, wide, false));
        nade.reticle.cook_off_hold = false;
        case("not cooking", draw(&p, &nade, &dv, wide, false));
        for state in [1, 4, 5, 7, 12, 15] {
            let mut p = ps();
            p.weaponstate = state;
            case(
                &format!("state {state}"),
                draw(&p, &rifle(), &dv, wide, false),
            );
        }
        let mut p = ps();
        p.weapon = 0;
        case("no weapon", draw(&p, &rifle(), &dv, wide, false));
        p.weap_flags = 2;
        p.off_hand = 5;
        case("off hand", draw(&p, &rifle(), &dv, wide, false));
        let off = Dvars { draw: false, ..dv };
        case("dvar off", draw(&ps(), &rifle(), &off, wide, false));
        let faint = Dvars {
            alpha: 0.005,
            alpha_min: 0.0,
            ..dv
        };
        case("faint", draw(&ps(), &rifle(), &faint, wide, false));
        let no_side = |m: &str| (m != "reticle_side").then(|| format!("img/{m}"));
        case(
            "no side image",
            draw_with(&ps(), &rifle(), None, &dv, wide, false, &no_side),
        );
        let mut plain = rifle();
        plain.reticle.color_change = false;
        let mut p = ps();
        p.weap_flags = 0x10;
        case("no colour change", draw(&p, &plain, &dv, wide, false));
        let mut p = ps();
        p.eflags = EF_TURRET;
        let mg = rifle();
        let tex = |m: &str| Some(format!("img/{m}"));
        case(
            "turret",
            draw_with(&p, &rifle(), Some(&mg), &dv, wide, false, &tex),
        );
        p.weap_flags = 8;
        case(
            "turret friend",
            draw_with(&p, &rifle(), Some(&mg), &dv, wide, false, &tex),
        );
        let no_turret = Dvars {
            draw_turret: false,
            ..dv
        };
        case(
            "turret dvar off",
            draw_with(&p, &rifle(), Some(&mg), &no_turret, wide, false, &tex),
        );
        case(
            "turret scope",
            draw_with(
                &p,
                &rifle(),
                Some(&sniper([300.0, 250.0])),
                &dv,
                vga,
                false,
                &tex,
            ),
        );
        case(
            "turret none",
            draw_with(&p, &rifle(), None, &dv, wide, false, &tex),
        );
        for pos in [0.0, 0.5, 0.52, 0.8, 1.0] {
            for sight in [0, crate::cod4_viewmodel::PMF_SIGHT] {
                let mut p = ps();
                p.pos_frac = pos;
                p.pm_flags = sight;
                writeln!(
                    s,
                    "# overlay_up {pos} {sight}: {} {}",
                    overlay_up(&p, &sniper([200.0, 200.0])),
                    overlay_up(&p, &rifle())
                )
                .unwrap();
            }
        }
        s
    }

    /// The digest, fixed against the module before its rewrite.
    #[test]
    fn the_crosshair_draws_its_pinned_quads() {
        let got = all_cases();
        if let Ok(path) = std::env::var("CROSSHAIR_PIN_DUMP") {
            std::fs::write(path, &got).unwrap();
        }
        let want = include_str!("../tests/fixtures/cod4_crosshair_pins.txt");
        assert!(
            got == want,
            "the crosshair's quads moved off the pinned ones"
        );
    }
}
