//! The grenade indicators (`CG_DrawGrenadeIndicators`, `iw3mp.exe`
//! 0x42ce90): for each resting frag near the drawn player a pointer and an
//! icon around the screen's centre, toward it, pulsing
//! (docs/research/cod4-hud.md 21). A child of `cod4_hud`, whose loaded
//! zones it draws with.

use super::*;
use glam::Vec3;

/// The `cg_hudGrenade*` and `bg_maxGrenadeIndicatorSpeed` 1.7 defaults
/// (cod4-hud.md 21; registered at `iw3mp.exe` 0x43b54e..0x43b9f1 and
/// 0x40a315). `cg_hudGrenadeIconInScope` and `-EnabledFlash` are 0, so a
/// sight's overlay hides the indicators and a flashbang never shows.
const FRAG_RANGE: f32 = 250.0;
const HEIGHT_REACH: f32 = 104.0;
/// Grid units from the centre to the pointer's pivot and the icon's middle.
const RING_RADIUS: f32 = 50.0;
const ICON_SIDE: f32 = 25.0;
const POINTER_SIZE: [f32; 2] = [25.0, 12.0];
const POINTER_PIVOT: [f32; 2] = [12.0, 27.0];
const PULSE_HZ: f32 = 1.7;
const PULSE_RANGE: [f32; 2] = [0.3, 1.85];
/// Faster than this a frag is still flying.
const RESTING_SPEED: f32 = 20.0;
/// What a frame takes at most.
const MAX_GRENADES: usize = 32;
/// `weapType` 1, a grenade; `projExplosionType` 0 (grenade) or 6 (heavy).
const WEAPTYPE_GRENADE: i32 = 1;
const FRAG_EXPLOSIONS: [i32; 2] = [0, 6];
/// A `cursorHint` from 5 on names a weapon, here the frag to throw back.
const FIRST_WEAPON_HINT: u32 = 5;
const FRAG_ICON: &str = "hud_grenadeicon";
const THROWBACK_ICON: &str = "hud_grenadethrowback";
const POINTER: &str = "hud_grenadepointer";
/// `pm_type` 4, a spectator, and `otherFlags` 2, a follower: no
/// indicators for either.
const PM_SPECTATOR: u32 = 4;
const OF_FOLLOWING: u32 = 2;

/// A grenade the frame takes: where it is and its icon's material.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct HudGrenade {
    pub origin: Vec3,
    pub icon: &'static str,
}

/// The position check (0x42cbf0) on a frag `offset` from the feet: within
/// its reach (250, or its blast `radius` when larger), at most 104 under
/// the feet, and above them under 104 plus the eye's height or the
/// `ceiling`, whichever is lower.
pub(crate) fn in_range(offset: Vec3, radius: i32, view_height: f32, ceiling: f32) -> bool {
    let reach = FRAG_RANGE.max(radius as f32);
    if offset.length_squared() > reach * reach {
        return false;
    }
    let rise = offset.z;
    if rise < 0.0 {
        -rise <= HEIGHT_REACH
    } else {
        rise <= ceiling.min(HEIGHT_REACH + view_height)
    }
}

/// The indicators' shared alpha at cg time `now`: a 1.7 Hz sine between
/// the pulse's two ends, clamped to 0..1.
fn pulse_alpha(now: i32) -> f32 {
    let [lo, hi] = PULSE_RANGE;
    let half = (hi - lo) * 0.5;
    let wave = (now as f32 * 0.006_283_185 * PULSE_HZ).sin();
    (wave * half + lo + half).clamp(0.0, 1.0)
}

/// One grenade's pointer (0x42c8f0), then its icon (0x42ca90), for a frag
/// `toward` off the feet with the view at `view_yaw`. Both sit
/// [`RING_RADIUS`] from the centre on the frag's bearing, straight up for
/// one ahead; the pointer is turned about its pivot to face it, the icon
/// stays upright.
fn draw_indicator(
    sp: &ScreenPlacement,
    toward: Vec3,
    view_yaw: f32,
    rgba: [f32; 4],
    [pointer, icon]: [Option<String>; 2],
    out: &mut Vec<HudQuad>,
) {
    let mid_x = sp.axis(0, 0.0, 0.0, 2).0;
    let mid_y = sp.axis(1, 0.0, 0.0, 2).0;
    let [sx, sy] = sp.virtual_to_real;
    let bearing = angle_normalize_360(toward.y.atan2(toward.x).to_degrees() - view_yaw);
    let (right, up) = bearing.to_radians().sin_cos();
    if let Some(texture) = pointer {
        let pivot = [
            mid_x - sx * RING_RADIUS * right,
            mid_y - sy * RING_RADIUS * up,
        ];
        let (sin, cos) = (-bearing).to_radians().sin_cos();
        let [w, h] = POINTER_SIZE;
        let [px, py] = POINTER_PIVOT;
        let corner = |gx: f32, gy: f32| {
            let (x, y) = (sx * (gx - px), sy * (gy - py));
            [pivot[0] + x * cos - y * sin, pivot[1] + x * sin + y * cos]
        };
        out.push(HudQuad {
            verts: [
                corner(0.0, 0.0),
                corner(w, 0.0),
                corner(w, h),
                corner(0.0, h),
            ],
            uvs: [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]],
            rgba,
            texture,
        });
    }
    if let Some(texture) = icon {
        let left = mid_x + sx * (-ICON_SIDE * 0.5 - RING_RADIUS * right);
        let top = mid_y + sy * (-ICON_SIDE * 0.5 - RING_RADIUS * up);
        let (w, h) = (sx * ICON_SIDE, sy * ICON_SIDE);
        out.push(quad(left, top, w, h, rgba, texture, false));
    }
}

impl Cod4Hud {
    /// The grenades `CG_AddHudGrenade` (0x42cd50) takes off the newest
    /// snapshot's missiles at `now` for the player `ps`: a timed frag or
    /// heavy grenade at rest within range, at most 32; the one the cursor
    /// hint names takes the throw back icon. `ceiling` is a trace, asked
    /// only of a resting frag (cod4-client-perf.md 38).
    pub(crate) fn hud_grenades(
        &self,
        s: &Cod4Session,
        ps: &crate::cod4_viewmodel::ViewPs,
        ceiling: &dyn Fn() -> f32,
        now: i32,
    ) -> Vec<HudGrenade> {
        use crate::play::cod4::{ET_MISSILE, ent_f32, ent_u32, position_at, weapon_list};
        let (Some(Some(l)), Some(gs), Some(snap)) =
            (&self.loaded, s.gamestate(), s.client().snapshots.last())
        else {
            return Vec::new();
        };
        let names = weapon_list(gs);
        let def_of = |weapon: u32| {
            let name = names.get((weapon as usize).checked_sub(1)?)?;
            l.weapon_hud.get(name)
        };
        let hinted = (snap.ps.get("cursorHint").unwrap_or(0) >= FIRST_WEAPON_HINT)
            .then(|| snap.ps.get("cursorHintEntIndex").unwrap_or(1023));
        let resting = |e: &EntityState| {
            let speed2: f32 = (0..3)
                .map(|k| ent_f32(e, &format!("lerp.pos.trDelta[{k}]")).powi(2))
                .sum();
            speed2 <= RESTING_SPEED * RESTING_SPEED
        };
        snap.entities
            .iter()
            .filter(|e| e.etype() == ET_MISSILE)
            .filter_map(|e| {
                let def = def_of(ent_u32(e, "weapon"))?;
                let frag = def.weap_type == WEAPTYPE_GRENADE
                    && FRAG_EXPLOSIONS.contains(&def.proj_explosion)
                    && def.timed_detonation;
                if !frag || !resting(e) {
                    return None;
                }
                let origin = position_at(e, now);
                let offset = origin - ps.origin;
                if !in_range(
                    offset,
                    def.explosion_radius,
                    ps.view_height_current,
                    ceiling(),
                ) {
                    return None;
                }
                let icon = if hinted == Some(e.number()) {
                    THROWBACK_ICON
                } else {
                    FRAG_ICON
                };
                Some(HudGrenade { origin, icon })
            })
            .take(MAX_GRENADES)
            .collect()
    }

    /// `CG_DrawGrenadeIndicators` for the drawn playerstate `ps` at cg
    /// time `now` on a `size` window: none for a spectator or a follower
    /// or under a sight's overlay.
    pub(crate) fn grenade_indicators(
        &self,
        s: &Cod4Session,
        ps: &crate::cod4_viewmodel::ViewPs,
        ceiling: &dyn Fn() -> f32,
        size: (f32, f32),
        now: i32,
    ) -> Vec<HudQuad> {
        let Some(Some(l)) = &self.loaded else {
            return Vec::new();
        };
        if ps.pm_type == PM_SPECTATOR || ps.other_flags & OF_FOLLOWING != 0 {
            return Vec::new();
        }
        let held = s.gamestate().and_then(|gs| {
            let names = crate::play::cod4::weapon_list(gs);
            let name = names.get((ps.view_weapon() as usize).checked_sub(1)?)?;
            l.weapon_hud.get(name)
        });
        if held.is_some_and(|def| crate::cod4_crosshair::overlay_up(ps, def)) {
            return Vec::new();
        }
        let sp = ScreenPlacement::cod4(size.0, size.1);
        let rgba = [1.0, 1.0, 1.0, pulse_alpha(now)];
        let mut out = Vec::new();
        for g in self.hud_grenades(s, ps, ceiling, now) {
            let images = [image_of(l, POINTER), image_of(l, g.icon)];
            draw_indicator(
                &sp,
                g.origin - ps.origin,
                ps.angles.y,
                rgba,
                images,
                &mut out,
            );
        }
        out
    }
}

fn angle_normalize_360(a: f32) -> f32 {
    a.rem_euclid(360.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fmt::Write;

    /// 250 out, or the blast radius when larger; up to 104 plus the view's
    /// height above the feet unless a ceiling is nearer, 104 below.
    #[test]
    fn the_range_and_height_checks() {
        let v = 60.0;
        let far = f32::MAX;
        assert!(in_range(Vec3::new(249.0, 0.0, 0.0), 0, v, far));
        assert!(!in_range(Vec3::new(251.0, 0.0, 0.0), 0, v, far));
        assert!(in_range(Vec3::new(255.0, 0.0, 0.0), 256, v, far));
        assert!(in_range(Vec3::new(0.0, 0.0, 164.0), 0, v, far));
        assert!(!in_range(Vec3::new(0.0, 0.0, 165.0), 0, v, far));
        assert!(!in_range(Vec3::new(0.0, 0.0, 50.0), 0, v, 40.0));
        assert!(in_range(Vec3::new(0.0, 0.0, -104.0), 0, v, far));
        assert!(!in_range(Vec3::new(0.0, 0.0, -105.0), 0, v, far));
    }

    /// The pointer and icon quads and the pulse's alpha over a set of
    /// windows, bearings and times, against the digest the drawing gave
    /// before its rewrite.
    #[test]
    fn the_indicators_draw_their_pinned_quads() {
        let mut s = String::new();
        let windows = [
            (640.0, 480.0, 1.0),
            (1280.0, 720.0, 1.0),
            (2560.0, 1080.0, 1.0),
            (1280.0, 1024.0, 1.25),
        ];
        let offsets = [
            Vec3::new(100.0, 0.0, 0.0),
            Vec3::new(0.0, 100.0, 5.0),
            Vec3::new(-30.0, -40.0, -10.0),
            Vec3::new(12.5, -170.25, 0.0),
            Vec3::new(-200.0, 3.0, 40.0),
        ];
        for (w, h, a) in windows {
            let sp = ScreenPlacement::scr_place(w, h, a);
            for (i, off) in offsets.into_iter().enumerate() {
                for view_yaw in [0.0, 45.0, -135.5, 359.0, 271.25] {
                    let now = 1000 + 137 * i as i32 + view_yaw as i32 * 11;
                    let icon = if i % 2 == 0 {
                        FRAG_ICON
                    } else {
                        THROWBACK_ICON
                    };
                    let images = [Some(format!("img/{POINTER}")), Some(format!("img/{icon}"))];
                    let mut out = Vec::new();
                    let rgba = [1.0, 1.0, 1.0, pulse_alpha(now)];
                    draw_indicator(&sp, off, view_yaw, rgba, images, &mut out);
                    writeln!(s, "# {w}x{h}@{a} {off:?} yaw {view_yaw} now {now}").unwrap();
                    for q in &out {
                        writeln!(s, "{} {:?} {:?} {:?}", q.texture, q.rgba, q.verts, q.uvs)
                            .unwrap();
                    }
                }
            }
        }
        for now in [0, 1, 50, 147, 294, 500, 1000, 123_456, 9_999_999] {
            writeln!(s, "# alpha {now} {:?}", pulse_alpha(now)).unwrap();
        }
        let want = include_str!("../tests/fixtures/cod4_grenades_pins.txt");
        assert!(
            s == want,
            "the grenade indicators moved off the pinned ones"
        );
    }
}
