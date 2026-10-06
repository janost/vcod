//! The scope overlay a weapon with `adsOverlayShader` or `adsOverlayReticle`
//! draws down the sight, and the gun aim it is centred on
//! (docs/research/cod11-hud-protocol.md, "Scope overlay").

use super::HudQuad;
use super::player::DamageFeedback;
use vcod_common::pmove::PlayerState;
use vcod_common::pmove::aim::{self, AimInput, AimState, DamageKick};
use vcod_common::weapon::{OverlayReticle, WeaponDef};

/// How far into the zoom tail the sight is, once past 0.01: then the overlay
/// is up, the viewmodel hidden and the crosshair's alpha scaled by
/// `1 - frac`. `raising` picks `adsZoomInFrac` over `adsZoomOutFrac`, as
/// the zoom does.
pub fn overlay_frac(def: &WeaponDef, ads_frac: f32, raising: bool) -> Option<f32> {
    if def.ads_overlay_shader.is_none() && def.ads_overlay_reticle == OverlayReticle::None {
        return None;
    }
    if ads_frac == 0.0 {
        return None;
    }
    let tail = if raising {
        def.ads_zoom_in_frac
    } else {
        def.ads_zoom_out_frac
    };
    let mut frac = ads_frac - (1.0 - tail);
    if frac > 0.0 {
        frac /= tail;
    }
    (frac > 0.01).then_some(frac)
}

/// The aim block's gun half run on the client, so the scope sits where the
/// server's bullet leaves, with the damage kick the cgame works out from
/// the playerstate's feedback bytes.
#[derive(Default)]
pub struct GunAim {
    state: AimState,
    last_ms: Option<i32>,
    kick: DamageKick,
    /// `(clientNum, damageEvent)` last seen; a new client is a new baseline.
    last_hit: Option<(i32, i32)>,
}

impl GunAim {
    /// `CG_DamageFeedback`'s kick half (`0x300287f0`, hud doc "Scope
    /// overlay"): a changed `damageEvent` with a non-zero `damageCount` on
    /// the same client kicks the gun from `now_ms`, along `ps`'s view.
    pub fn feed(&mut self, client: i32, fb: DamageFeedback, ps: &PlayerState, now_ms: i32) {
        let Some((last_client, last_event)) = self.last_hit.replace((client, fb.event)) else {
            return;
        };
        if last_client != client || last_event == fb.event || fb.count == 0 || now_ms == 0 {
            return;
        }
        let view = [-ps.pitch.to_degrees(), ps.yaw.to_degrees(), 0.0];
        self.kick = damage_kick(fb, view, now_ms);
    }

    /// The gun's angles off the view for `ps` (whose yaw and pitch are the
    /// view), stepped to `now_ms`, wire convention.
    pub fn step(
        &mut self,
        def: Option<&WeaponDef>,
        ps: &PlayerState,
        now_ms: i32,
    ) -> Option<[f32; 3]> {
        let msec = self.last_ms.map_or(0, |last| (now_ms - last).clamp(0, 200));
        self.last_ms = Some(now_ms);
        let input = AimInput {
            def,
            view: [-ps.pitch.to_degrees(), ps.yaw.to_degrees(), 0.0],
            msec,
            now_ms,
            kick: self.kick,
        };
        aim::gun_angles(ps, &mut self.state, &input)
    }
}

/// `0x300287f0`'s kick off the feedback bytes: `damageCount * 0.2` clamped
/// to 5..90, straight up the view for yaw and pitch both 255, otherwise
/// split along and across `view` (wire degrees) by the direction the bytes
/// name, each read as `byte / 255 * 360`.
pub fn damage_kick(fb: DamageFeedback, view: [f32; 3], time_ms: i32) -> DamageKick {
    let kick = (fb.count as f32 * 0.2).clamp(5.0, 90.0);
    if fb.yaw == 255 && fb.pitch == 255 {
        return DamageKick {
            time_ms,
            pitch: -kick,
            side: 0.0,
        };
    }
    let byte = |b: i32| (b as f32 / 255.0 * 360.0).to_radians();
    let (sy, cy) = byte(fb.yaw).sin_cos();
    let (sp, cp) = byte(fb.pitch).sin_cos();
    let dir = [cp * cy, cp * sy, -sp];
    let axis = aim::angles_to_axis(view);
    let dot = |a: [f32; 3]| a[0] * dir[0] + a[1] * dir[1] + a[2] * dir[2];
    DamageKick {
        time_ms,
        pitch: kick * dot(axis[0]),
        side: -kick * dot(axis[1]),
    }
}

/// Where the gun points, window pixels: its forward projected through the
/// drawn fovs (degrees). The screen centre when it points behind the view.
pub fn gun_point(gun: [f32; 3], (fov_x, fov_y): (f32, f32), (w, h): (f32, f32)) -> [f32; 2] {
    // Forward in the view's own frame: along it, to its left, up.
    let [fwd, left, up] = aim::angles_to_axis(gun)[0];
    let tx = (fov_x.to_radians() / 2.0).tan();
    let ty = (fov_y.to_radians() / 2.0).tan();
    if fwd <= 0.0 || tx <= 0.0 || ty <= 0.0 {
        return [w / 2.0, h / 2.0];
    }
    [
        w / 2.0 * (1.0 - left / (tx * fwd)),
        h / 2.0 * (1.0 - up / (ty * fwd)),
    ]
}

/// Window-pixel rect with retail's `DrawStretchPic` texture corners: `(s0,
/// t0)` at the top left, `(s1, t1)` at the bottom right.
fn rect(
    (x, y, w, h): (f32, f32, f32, f32),
    [s0, t0, s1, t1]: [f32; 4],
    rgba: [f32; 4],
    texture: &str,
) -> HudQuad {
    HudQuad {
        verts: [[x, y], [x + w, y], [x + w, y + h], [x, y + h]],
        uvs: [[s0, t0], [s1, t0], [s1, t1], [s0, t1]],
        rgba,
        texture: texture.to_string(),
    }
}

/// The overlay centred on `c` (window pixels): the image four times,
/// mirrored about `c`, black bands from it to the window edges sampled off
/// the image's own edge, then the reticle lines. Retail scales x and y
/// separately by 640 and 480; vcod uses the HUD's one height scale, so the
/// circle stays round on a wide window and the bands take up the rest.
pub fn build(def: &WeaponDef, [cx, cy]: [f32; 2], (sw, sh): (f32, f32), out: &mut Vec<HudQuad>) {
    const WHITE: [f32; 4] = [1.0; 4];
    const BLACK: [f32; 4] = [0.0, 0.0, 0.0, 1.0];
    const FULL: [f32; 4] = [0.0, 0.0, 1.0, 1.0];
    let scale = sh / 480.0;
    let w = def.ads_overlay_width * scale;
    let h = def.ads_overlay_height * scale;

    if let Some(shader) = &def.ads_overlay_shader {
        let (left, top) = (cx - w, cy - h);
        out.push(rect((left, top, w, h), FULL, WHITE, shader));
        out.push(rect((cx, top, w, h), [1.0, 0.0, 0.0, 1.0], WHITE, shader));
        out.push(rect((left, cy, w, h), [0.0, 1.0, 1.0, 0.0], WHITE, shader));
        out.push(rect((cx, cy, w, h), [1.0, 1.0, 0.0, 0.0], WHITE, shader));
        // The side bands read the image's left column, top and bottom its
        // top row; both are the corner's solid black.
        let column = [0.0, 0.0, 0.0, 1.0];
        let row = [0.0, 0.0, 1.0, 0.0];
        if left > 0.0 {
            out.push(rect((0.0, 0.0, left, sh), column, WHITE, shader));
        }
        let right = cx + w;
        if right < sw {
            out.push(rect((right, 0.0, sw - right, sh), column, WHITE, shader));
        }
        if top > 0.0 {
            out.push(rect((left, 0.0, 2.0 * w, top), row, WHITE, shader));
        }
        let bottom = cy + h;
        if bottom < sh {
            out.push(rect(
                (left, bottom, 2.0 * w, sh - bottom),
                row,
                WHITE,
                shader,
            ));
        }
    }

    // The lines are 3 window pixels thick, nudged back one.
    let vline = "hudSoftLine";
    let hline = "hudSoftLineH";
    match def.ads_overlay_reticle {
        OverlayReticle::None => {}
        OverlayReticle::Crosshair => {
            if let Some(center) = &def.reticle_center {
                let s = def.reticle_center_size * scale;
                let at = (cx - s / 2.0, cy - s / 2.0, s, s);
                out.push(rect(at, FULL, WHITE, center));
            }
        }
        OverlayReticle::Fg42 | OverlayReticle::Gewehr43 => {
            out.push(rect((cx - 1.0, cy, 3.0, h * 0.9), FULL, BLACK, vline));
            let bar = (w * 0.75, 3.0);
            out.push(rect(
                (cx - w * 0.9, cy - 1.0, bar.0, bar.1),
                FULL,
                BLACK,
                hline,
            ));
            out.push(rect(
                (cx + w * 0.15, cy - 1.0, bar.0, bar.1),
                FULL,
                BLACK,
                hline,
            ));
        }
        OverlayReticle::Springfield => {
            out.push(rect(
                (cx - 1.0, cy - h * 0.9, 3.0, h * 1.8),
                FULL,
                BLACK,
                vline,
            ));
            out.push(rect(
                (cx - w * 0.9, cy - 1.0, w * 1.8, 3.0),
                FULL,
                BLACK,
                hline,
            ));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    /// `kar98k_sniper_mp`'s overlay and zoom keys as pak0 spells them.
    fn sniper() -> WeaponDef {
        let mut m = HashMap::new();
        for (k, v) in [
            ("aimDownSight", "1"),
            ("adsZoomFov", "16"),
            ("adsZoomInFrac", "0.42"),
            ("adsZoomOutFrac", "0.08"),
            ("adsOverlayShader", "ui/assets/reticle_circle_quarter"),
            ("adsOverlayReticle", "FG42"),
            ("adsOverlayWidth", "220"),
            ("adsOverlayHeight", "220"),
        ] {
            m.insert(k.to_string(), v.to_string());
        }
        WeaponDef::from_map(&m)
    }

    /// The overlay comes up 1% into the zoom tail: past `1 - 0.42 + 0.0042`
    /// rising, past `1 - 0.08 + 0.0008` falling, and never off a sightless
    /// file.
    #[test]
    fn the_overlay_starts_a_hundredth_into_the_zoom_tail() {
        let def = sniper();
        assert_eq!(overlay_frac(&def, 0.0, true), None);
        assert_eq!(overlay_frac(&def, 0.584, true), None);
        let up = overlay_frac(&def, 0.6, true).expect("up");
        assert!((up - 0.02 / 0.42).abs() < 1e-5, "{up}");
        let full = overlay_frac(&def, 1.0, true).expect("full");
        assert!((full - 1.0).abs() < 1e-5, "{full}");
        assert_eq!(overlay_frac(&def, 0.6, false), None, "the out tail is 0.08");
        assert!(overlay_frac(&def, 0.95, false).is_some());
        assert_eq!(overlay_frac(&WeaponDef::default(), 1.0, true), None);
    }

    /// A level gun sits on the centre; a gun yawed left by half the
    /// horizontal fov sits on the left edge, and one pitched down by half
    /// the vertical fov on the bottom edge.
    #[test]
    fn the_gun_projects_through_the_drawn_fov() {
        let screen = (1600.0, 900.0);
        let fov = (16.0, 9.2);
        assert_eq!(gun_point([0.0; 3], fov, screen), [800.0, 450.0]);
        let [x, y] = gun_point([0.0, fov.0 / 2.0, 0.0], fov, screen);
        assert!(x.abs() < 1e-2 && (y - 450.0).abs() < 1e-2, "{x} {y}");
        let [x, y] = gun_point([fov.1 / 2.0, 0.0, 0.0], fov, screen);
        assert!(
            (x - 800.0).abs() < 1e-2 && (y - 900.0).abs() < 0.5,
            "{x} {y}"
        );
    }

    /// Four mirrored copies meet at the centre; on a 16:9 window both side
    /// bands fill out to the edges and the top and bottom ones are there
    /// because 440 virtual pixels fall short of 480.
    #[test]
    fn the_overlay_tiles_and_fills_the_window() {
        let def = sniper();
        let mut out = Vec::new();
        build(&def, [960.0, 540.0], (1920.0, 1080.0), &mut out);
        let shader = "ui/assets/reticle_circle_quarter";
        let images: Vec<_> = out.iter().filter(|q| q.texture == shader).collect();
        assert_eq!(images.len(), 8, "four quarters, four bands");
        let w = 220.0 * 1080.0 / 480.0;
        assert_eq!(images[0].verts[0], [960.0 - w, 540.0 - w]);
        assert_eq!(images[0].verts[2], [960.0, 540.0]);
        assert_eq!(
            images[3].uvs[0],
            [1.0, 1.0],
            "bottom right mirrors both ways"
        );
        assert_eq!(images[4].verts[2], [960.0 - w, 1080.0], "left band");
        assert_eq!(images[5].verts[0], [960.0 + w, 0.0], "right band");
        let lines = out.iter().filter(|q| q.texture.starts_with("hudSoftLine"));
        assert_eq!(lines.count(), 3, "the FG42 post and two bars");
    }

    /// `0x300287f0`'s kick: 0.2 per point of `damageCount` inside 5..90,
    /// straight up for an undirected hit, and split by the bytes' direction
    /// against the view otherwise.
    #[test]
    fn the_damage_kick_reads_the_feedback_bytes() {
        let fb = |yaw, pitch, count| DamageFeedback {
            event: 1,
            yaw,
            pitch,
            count,
        };
        let k = damage_kick(fb(255, 255, 10), [0.0; 3], 7);
        assert_eq!((k.time_ms, k.pitch, k.side), (7, -5.0, 0.0));
        let k = damage_kick(fb(255, 255, 127), [0.0; 3], 7);
        assert_eq!(k.pitch, -25.4);
        // Along a level view at yaw 0: all pitch.
        let k = damage_kick(fb(0, 0, 200), [0.0; 3], 7);
        assert!(
            (k.pitch - 40.0).abs() < 1e-4 && k.side.abs() < 1e-4,
            "{k:?}"
        );
        // The same direction seen from yaw 90 runs across the view.
        let k = damage_kick(fb(0, 0, 200), [0.0, 90.0, 0.0], 7);
        assert!(
            k.pitch.abs() < 1e-4 && (k.side - 40.0).abs() < 1e-4,
            "{k:?}"
        );
    }

    /// The first playerstate is a baseline, a changed `damageEvent` kicks,
    /// and a different client (a follow switch) is a new baseline.
    #[test]
    fn a_changed_damage_event_kicks_the_gun() {
        let ps = PlayerState::spawn(glam::Vec3::ZERO, 0.0);
        let fb = |event| DamageFeedback {
            event,
            yaw: 255,
            pitch: 255,
            count: 50,
        };
        let mut gun = GunAim::default();
        gun.feed(3, fb(4), &ps, 1000);
        assert_eq!(gun.kick, DamageKick::default());
        gun.feed(3, fb(4), &ps, 1100);
        assert_eq!(gun.kick, DamageKick::default());
        gun.feed(5, fb(6), &ps, 1200);
        assert_eq!(gun.kick, DamageKick::default(), "new client");
        gun.feed(5, fb(7), &ps, 1300);
        assert_eq!(gun.kick.time_ms, 1300);
        assert_eq!(gun.kick.pitch, -10.0);
    }

    /// The four stock MP files with a scope, and the lines each draws.
    #[test]
    fn the_stock_scopes_parse() {
        let Some(fs) = vcod_common::testing::game_fs() else {
            return;
        };
        for (name, reticle) in [
            ("springfield_mp", OverlayReticle::Springfield),
            ("kar98k_sniper_mp", OverlayReticle::Fg42),
            ("mosin_nagant_sniper_mp", OverlayReticle::Fg42),
            ("fg42_mp", OverlayReticle::Fg42),
        ] {
            let def = vcod_common::weapon::load(&fs, name).expect(name);
            assert_eq!(
                def.ads_overlay_shader.as_deref(),
                Some("ui/assets/reticle_circle_quarter"),
                "{name}"
            );
            assert_eq!(def.ads_overlay_reticle, reticle, "{name}");
            assert_eq!(
                (def.ads_overlay_width, def.ads_overlay_height),
                (220.0, 220.0)
            );
        }
        let plain = vcod_common::weapon::load(&fs, "kar98k_mp").expect("kar98k");
        assert_eq!(overlay_frac(&plain, 1.0, true), None);
    }
}
