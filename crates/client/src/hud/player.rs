//! The native player HUD: crosshair, health, ammo and weapon name, stance,
//! compass with objectives, cursor hint and damage direction, at the rects
//! pak0 `ui_mp/hud.menu` gives. How the cgame draws each:
//! docs/research/cod11-hud-protocol.md, section 9.

use super::font::{self, Font};
use super::hudelem::{self, Virtual, CS_SHADERS};
use super::HudQuad;
use vcod_common::localize::Localized;
use vcod_common::net::msg::Objective;
use vcod_common::weapon::WeaponDef;

/// Hint strings (`serverCursorHintString`) index configstrings from here.
pub const CS_HINT_STRINGS: usize = 1212;
/// `northyaw`, the compass's north in world yaw degrees.
pub const CS_NORTHYAW: usize = 11;

const EF_CROUCH: i32 = 0x20;
const EF_PRONE: i32 = 0x40;
const WHITE: [f32; 4] = [1.0; 4];

/// The playerstate's damage-feedback fields; they never clear, so only a
/// changed `event` is a new hit.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DamageFeedback {
    pub event: i32,
    pub yaw: i32,
    pub pitch: i32,
    pub count: i32,
}

/// What the native HUD reads off the playerstate being drawn.
pub struct PlayerView<'a> {
    pub client_num: i32,
    pub health: i32,
    pub max_health: i32,
    pub eflags: i32,
    pub weapon: Option<&'a WeaponDef>,
    pub ammo: &'a [i16; 64],
    pub ammoclip: &'a [i16; 64],
    /// 0..255.
    pub aim_spread_scale: f32,
    pub ads_frac: f32,
    /// `pm_flags` 0x80: the sight is going up rather than coming down.
    pub ads_held: bool,
    /// World degrees.
    pub view_yaw: f32,
    pub eye: [f32; 3],
    /// Horizontal and vertical, degrees.
    pub fov: (f32, f32),
    pub objectives: &'a [Objective],
    pub cursor_hint: i32,
    /// Signed; below 0 is none.
    pub cursor_hint_string: i32,
    pub damage: DamageFeedback,
}

/// What the native HUD reads from outside the playerstate.
pub struct Context<'a> {
    /// Configstring 7's defs, index = weapon number (`weapon_table`).
    pub weapons: &'a [Option<WeaponDef>],
    pub configstrings: &'a [String],
    pub loc: &'a Localized,
    pub font: &'a Font,
    /// An entity's current origin, for objectives placed on one.
    pub entity_origin: &'a dyn Fn(i32) -> Option<[f32; 3]>,
}

/// The two pieces of state the native HUD keeps across frames.
#[derive(Default)]
pub struct PlayerHud {
    pub damage: DamageIndicators,
    health_lag: HealthLag,
}

impl PlayerHud {
    /// `now` is the client's clock in ms. Whether to draw at all (a
    /// spectator in free flight, the intermission) is the caller's call.
    pub fn build(
        &mut self,
        p: &PlayerView,
        cx: &Context,
        now: i32,
        screen: (f32, f32),
        out: &mut Vec<HudQuad>,
    ) {
        let v = Virtual::new(screen);
        self.damage.feed(p.damage, now);

        let north = cx
            .configstrings
            .get(CS_NORTHYAW)
            .and_then(|s| s.trim().parse::<f32>().ok())
            .unwrap_or(0.0);
        compass(p, cx, north, &v, out);
        stance(p.eflags, &v, out);
        let frac = health_fraction(p.health, p.max_health);
        let lag = self.health_lag.step(p.client_num, frac, now);
        health(frac, lag, &v, out);
        if let Some(def) = p.weapon {
            weapon_info(def, p, cx, &v, out);
            crosshair(def, p, &v, out);
        }
        cursor_hint(p, cx, now, &v, out);
        self.damage.build(p.view_yaw, now, &v, out);
    }
}

/// Text at a virtual baseline, at a menu `textscale`.
fn menu_text(
    font: &Font,
    s: &str,
    (x, baseline): (f32, f32),
    textscale: f32,
    v: &Virtual,
    out: &mut Vec<HudQuad>,
) {
    let top = baseline - text_height(font, textscale);
    hudelem::text(
        font,
        s,
        (x, top),
        textscale * v.scale / font.unit_scale(),
        WHITE,
        v,
        out,
    );
}

fn text_width(font: &Font, s: &str, textscale: f32) -> f32 {
    font::measure(font, s, textscale / font.unit_scale())
}

fn text_height(font: &Font, textscale: f32) -> f32 {
    font.max_height as f32 * font.glyph_scale * textscale
}

fn stance(eflags: i32, v: &Virtual, out: &mut Vec<HudQuad>) {
    let material = if eflags & EF_PRONE != 0 {
        "hudStanceProne"
    } else if eflags & EF_CROUCH != 0 {
        "hudStanceCrouch"
    } else {
        "hudStanceStand"
    };
    out.push(v.quad(100.0, 434.375, 40.0, 40.0, WHITE, material));
}

/// The health bar's filled share, 0..1.
pub fn health_fraction(health: i32, max_health: i32) -> f32 {
    if health == 0 || max_health == 0 {
        return 0.0;
    }
    (health as f32 / max_health as f32).clamp(0.0, 1.0)
}

/// `lag` is the red tail left by health just lost.
fn health(frac: f32, lag: f32, v: &Virtual, out: &mut Vec<HudQuad>) {
    const X: f32 = 502.0;
    const W: f32 = 128.0;
    out.push(v.quad(
        501.0,
        460.0,
        130.0,
        12.0,
        WHITE,
        "gfx/hud/hud@health_back.tga",
    ));
    if frac > 0.0 {
        let mut rgba = [0.7, 0.4, 0.0, 1.0];
        if frac > 0.5 {
            rgba[0] *= 2.0 * (1.0 - frac);
            rgba[2] *= 2.0 * (1.0 - frac);
        } else {
            rgba[1] = (frac + 0.2) * rgba[1] + 0.3;
        }
        let uvs = [[0.0, 0.0], [frac, 0.0], [frac, 1.0], [0.0, 1.0]];
        out.push(v.quad_uv(
            X,
            461.0,
            W * frac,
            10.0,
            uvs,
            rgba,
            "gfx/hud/hud@health_bar.tga",
        ));
    }
    if lag > frac {
        let uvs = [[frac, 0.0], [lag, 0.0], [lag, 1.0], [frac, 1.0]];
        out.push(v.quad_uv(
            X + W * frac,
            461.0,
            W * (lag - frac),
            10.0,
            uvs,
            [1.0, 0.0, 0.0, 1.0],
            "gfx/hud/hud@health_bar.tga",
        ));
    }
    out.push(v.quad(
        488.0,
        460.0,
        12.0,
        12.0,
        WHITE,
        "gfx/hud/hud@health_cross.tga",
    ));
}

impl HealthLag {
    /// The tail holds a frame, then drains at 1.2 bars a second.
    fn step(&mut self, client: i32, frac: f32, now: i32) -> f32 {
        let dt = self.last_now.map_or(0, |last| (now - last).max(0));
        self.last_now = Some(now);
        if self.client != Some(client) || self.shown <= frac {
            self.client = Some(client);
            self.shown = frac;
            self.hold = 1;
        } else if self.hold == 0 {
            self.shown -= dt as f32 * 0.0012;
            if self.shown <= frac {
                self.shown = frac;
                self.hold = 1;
            }
        } else {
            self.hold = (self.hold - dt).max(0);
        }
        self.shown
    }
}

/// The weapon name with its backdrop, and the ammo counter with its own.
fn weapon_info(def: &WeaponDef, p: &PlayerView, cx: &Context, v: &Virtual, out: &mut Vec<HudQuad>) {
    let translate = |key: &str| cx.loc.get(key).unwrap_or(key).to_string();
    let name = match def.mode_name.as_str() {
        "" => translate(&def.display_name),
        mode => format!("{} / {}", translate(&def.display_name), translate(mode)),
    };
    let w = text_width(cx.font, &name, 0.3);
    out.push(v.quad(
        562.5 - (w + 36.0),
        431.0,
        w + 36.0,
        20.0,
        WHITE,
        "gfx/hud/hud@weaponnameback.tga",
    ));
    menu_text(cx.font, &name, (562.5 - w - 28.0, 446.0), 0.3, v, out);

    out.push(v.quad(
        557.5,
        421.625,
        80.0,
        40.0,
        WHITE,
        "gfx/hud/hud@ammocounterback.tga",
    ));
    let (x, w, baseline) = (570.0, 55.0, 444.625);
    let clip = p.ammoclip.get(def.clip_index).copied().unwrap_or(0) as i32;
    let reserve = if def.clip_only {
        clip
    } else {
        p.ammo.get(def.ammo_index).copied().unwrap_or(0) as i32
    };
    let reserve = reserve.min(999).to_string();
    let centred = |s: &str| x + (w - text_width(cx.font, s, 0.21)) / 2.0;
    if def.clip_only {
        menu_text(
            cx.font,
            &reserve,
            (centred(&reserve), baseline),
            0.21,
            v,
            out,
        );
        return;
    }
    let clip = format!("{:2}", clip.min(999));
    menu_text(cx.font, &clip, (x, baseline), 0.21, v, out);
    menu_text(cx.font, "|", (centred("|"), baseline), 0.21, v, out);
    let right = x + w - text_width(cx.font, &reserve, 0.21);
    menu_text(cx.font, &reserve, (right, baseline), 0.21, v, out);
}

/// How far each crosshair arm sits off the centre, in virtual pixels, `x`
/// for the side arms and `y` for the top and bottom ones.
pub fn arm_offset(
    def: &WeaponDef,
    eflags: i32,
    aim_spread_scale: f32,
    shrink: f32,
    (fov_x, fov_y): (f32, f32),
) -> (f32, f32) {
    let min = if eflags & EF_PRONE != 0 {
        def.hip_spread_prone_min
    } else if eflags & EF_CROUCH != 0 {
        def.hip_spread_ducked_min
    } else {
        def.hip_spread_stand_min
    };
    let spread = (aim_spread_scale / 255.0 * (def.hip_spread_max - min) + min) * shrink;
    (
        (640.0 / fov_x * spread).max(def.reticle_min_ofs),
        (480.0 / fov_y * spread).max(def.reticle_min_ofs),
    )
}

/// The crosshair's quads, none at full sight or with no reticle. Its images
/// are sized in window pixels; only the arms' travel scales with the screen.
pub fn crosshair(def: &WeaponDef, p: &PlayerView, v: &Virtual, out: &mut Vec<HudQuad>) {
    if p.ads_frac >= 1.0 {
        return;
    }
    let mut shrink = 1.0;
    let tail = if p.ads_held {
        def.ads_crosshair_in_frac
    } else {
        def.ads_crosshair_out_frac
    };
    if p.ads_frac > 0.0 && tail > 0.0 {
        let into = p.ads_frac - (1.0 - tail);
        if into > 0.0 {
            shrink = 1.0 - 0.5 * into / tail;
        }
    }
    let [cx, cy] = v.point(320.0, 240.0);
    let px_quad = |x: f32, y: f32, s: f32, uvs: [[f32; 2]; 4], texture: &str| HudQuad {
        verts: [[x, y], [x + s, y], [x + s, y + s], [x, y + s]],
        uvs,
        rgba: WHITE,
        texture: texture.to_string(),
    };

    if let Some(center) = &def.reticle_center {
        let s = def.reticle_center_size * shrink;
        let uvs = [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]];
        out.push(px_quad(cx - s / 2.0, cy - s / 2.0, s, uvs, center));
    }
    let Some(side) = &def.reticle_side else {
        return;
    };
    let (ox, oy) = arm_offset(def, p.eflags, p.aim_spread_scale, shrink, p.fov);
    let s = def.reticle_side_size * shrink;
    // Top, right, bottom, left: direction, the arm's corner in sizes, and a
    // one-pixel nudge on the top and left arms.
    const DIR: [[f32; 2]; 4] = [[0.0, -1.0], [1.0, 0.0], [0.0, 1.0], [-1.0, 0.0]];
    const CORNER: [[f32; 2]; 4] = [[-0.5, -1.0], [0.0, -0.5], [-0.5, 0.0], [-1.0, -0.5]];
    const NUDGE: [[f32; 2]; 4] = [[0.0, -1.0], [0.0, 0.0], [0.0, 0.0], [-1.0, 0.0]];
    for i in 0..4 {
        let [dx, dy] = DIR[i];
        let pull = def.hip_reticle_side_pos * s;
        let x = cx + dx * ox * v.scale + s * CORNER[i][0] + NUDGE[i][0] - dx * pull;
        let y = cy + dy * oy * v.scale + s * CORNER[i][1] + NUDGE[i][1] - dy * pull;
        // The bottom and left arms flip the image; the side arms turn it.
        let (t0, t1) = if i < 2 { (0.0, 1.0) } else { (1.0, 0.0) };
        let uvs = if i % 2 == 0 {
            [[0.0, t0], [1.0, t0], [1.0, t1], [0.0, t1]]
        } else {
            [[0.0, t1], [0.0, t0], [1.0, t0], [1.0, t1]]
        };
        out.push(px_quad(x, y, s, uvs, side));
    }
}

/// The compass rect's centre, where objective bearings are measured from.
const COMPASS_CENTRE: (f32, f32) = (55.0, 425.0);

fn compass(p: &PlayerView, cx: &Context, north_yaw: f32, v: &Virtual, out: &mut Vec<HudQuad>) {
    let (x, y, size) = (-25.0, 345.0, 160.0);
    let half = size / 2.0;
    let corners = [[-half, -half], [half, -half], [half, half], [-half, half]];
    let turn = p.view_yaw - north_yaw;
    for material in ["gfx/hud/hud@compassback.tga", "gfx/hud/hud@compassface.tga"] {
        out.push(v.rotated(COMPASS_CENTRE, corners, turn, WHITE, material));
    }
    out.push(v.quad(x, y, size, size, WHITE, "gfx/hud/hud@compasshighlight.tga"));
    out.push(v.quad(
        x + 60.0,
        y + 50.0,
        40.0,
        40.0,
        WHITE,
        "gfx/hud/hud@compass_arrow.tga",
    ));

    for obj in p.objectives.iter().filter(|o| o.state == 4) {
        let target = match obj.ent_num {
            0x3ff => None,
            n => (cx.entity_origin)(n),
        }
        .unwrap_or_else(|| obj.origin_f32());
        let Some(icon) = objective_icon(cx.configstrings, obj.icon, target[2] - p.eye[2]) else {
            continue;
        };
        let (ix, iy) = compass_point(p.view_yaw, p.eye, target);
        out.push(v.quad(ix - 10.0, iy - 10.0, 20.0, 20.0, WHITE, &icon));
    }
}

/// The objective's material with its extension dropped and `_up` or
/// `_down` added when it sits more than 70 units above or below the eye.
fn objective_icon(cs: &[String], icon: i32, dz: f32) -> Option<String> {
    let name = cs
        .get(CS_SHADERS + icon.max(0) as usize)
        .map(String::as_str)?;
    if icon == 0 || name.is_empty() {
        return None;
    }
    let stem = name.split('.').next().unwrap_or(name);
    let suffix = if dz > 70.0 {
        "_up"
    } else if dz < -70.0 {
        "_down"
    } else {
        ""
    };
    Some(format!("{stem}{suffix}"))
}

/// The virtual centre of an objective's compass icon for a viewer at `eye`
/// looking along `view_yaw`: bearing off straight up, counter-clockwise for
/// a bearing to the left, out to 43.75 at 1024 units and beyond.
pub fn compass_point(view_yaw: f32, eye: [f32; 3], target: [f32; 3]) -> (f32, f32) {
    let (dx, dy) = (target[0] - eye[0], target[1] - eye[1]);
    let bearing = (dy.atan2(dx).to_degrees() - view_yaw).to_radians();
    let r = 43.75 * ((dx * dx + dy * dy).sqrt() / 1024.0).clamp(0.0, 1.0);
    (
        COMPASS_CENTRE.0 - bearing.sin() * r,
        COMPASS_CENTRE.1 - bearing.cos() * r,
    )
}

/// The icon a `serverCursorHint` shows, `None` for none.
pub fn hint_material(hint: i32, weapons: &[Option<WeaponDef>]) -> Option<String> {
    const NAMED: [&str; 8] = [
        "hintActivate",
        "hintNoActivate",
        "hintDoor",
        "hintNoDoor",
        "hintMg42",
        "hintHealth",
        "hintLadder",
        "hintFriendly",
    ];
    let weapon_icon = |w: i32, icon: fn(&WeaponDef) -> &Option<String>| {
        weapons
            .get(w as usize)
            .and_then(Option::as_ref)
            .and_then(|def| icon(def).clone())
            .unwrap_or_else(|| "hintActivate".to_string())
    };
    match hint {
        2..=9 => Some(NAMED[hint as usize - 2].to_string()),
        10..=73 => Some(weapon_icon(hint - 9, |d| &d.hud_icon)),
        74..=137 => Some(weapon_icon(hint - 73, |d| &d.ammo_icon)),
        _ => None,
    }
}

/// The hint icon, pulsing, and the hint string above it with its `[%s]`
/// filled with the use key.
fn cursor_hint(p: &PlayerView, cx: &Context, now: i32, v: &Virtual, out: &mut Vec<HudQuad>) {
    let Some(material) = hint_material(p.cursor_hint, cx.weapons) else {
        return;
    };
    let (x, y, w, h) = (300.0, 325.0, 40.0, 40.0);
    let pulse = ((now as f32 * 0.006_666_667).sin() + 1.0) * 0.5;
    let wide = (10..=73).contains(&p.cursor_hint)
        && cx
            .weapons
            .get(p.cursor_hint as usize - 9)
            .and_then(Option::as_ref)
            .is_some_and(|d| d.wide_list_icon);
    let (qx, qw) = if wide { (x - w / 2.0, w * 2.0) } else { (x, w) };
    out.push(v.quad(qx, y, qw, h, [1.0, 1.0, 1.0, pulse], &material));

    if p.cursor_hint_string < 0 {
        return;
    }
    let key = cx
        .configstrings
        .get(CS_HINT_STRINGS + p.cursor_hint_string as usize)
        .map(String::as_str)
        .unwrap_or("");
    if key.is_empty() {
        return;
    }
    let s = cx.loc.get(key).unwrap_or(key).replace("[%s]", "[F]");
    let tw = text_width(cx.font, &s, 0.21);
    let baseline = y - text_height(cx.font, 0.21);
    menu_text(cx.font, &s, (x + (w - tw) / 2.0, baseline), 0.21, v, out);
}

/// Up to eight hit-direction icons, each shown for 2 s from the hit.
#[derive(Default)]
pub struct DamageIndicators {
    last: Option<DamageFeedback>,
    /// (start ms, world yaw degrees the damage travelled along).
    slots: [Option<(i32, f32)>; 8],
}

const DAMAGE_ICON_MS: i32 = 2000;

impl DamageIndicators {
    /// A hit is a changed `event` with a non-zero `count`; yaw and pitch
    /// both 255 is damage with no direction. The first playerstate seen is
    /// only the baseline.
    pub fn feed(&mut self, fb: DamageFeedback, now: i32) {
        let last = self.last.replace(fb);
        if last.is_none_or(|l| l.event == fb.event) || fb.count == 0 {
            return;
        }
        if fb.yaw == 255 && fb.pitch == 255 {
            return;
        }
        let slot = (0..self.slots.len())
            .min_by_key(|&i| self.slots[i].map_or(i32::MIN, |(start, _)| start))
            .expect("eight slots");
        self.slots[slot] = Some((now, fb.yaw as f32 / 255.0 * 360.0));
    }

    fn live(&self, now: i32) -> impl Iterator<Item = (i32, f32)> + '_ {
        self.slots
            .iter()
            .flatten()
            .map(move |&(start, yaw)| (now - start, yaw))
            .filter(|&(age, _)| (0..DAMAGE_ICON_MS).contains(&age))
    }

    /// Each icon below the screen centre turned by the view against the
    /// hit's direction, opaque for the first second and fading over the
    /// second.
    pub fn build(&self, view_yaw: f32, now: i32, v: &Virtual, out: &mut Vec<HudQuad>) {
        let corners = [[-64.0, 32.0], [64.0, 32.0], [64.0, 96.0], [-64.0, 96.0]];
        for (age, yaw) in self.live(now) {
            let alpha = (2.0 - 2.0 * age as f32 / DAMAGE_ICON_MS as f32).min(1.0);
            let rgba = [1.0, 1.0, 1.0, alpha];
            out.push(v.rotated(
                (320.0, 240.0),
                corners,
                view_yaw - yaw,
                rgba,
                "hudHitDirection",
            ));
        }
    }
}

#[derive(Default)]
struct HealthLag {
    client: Option<i32>,
    shown: f32,
    hold: i32,
    last_now: Option<i32>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hud::hudelem::tests::test_font;

    fn carbine() -> WeaponDef {
        WeaponDef {
            reticle_side: Some("gfx/reticle/side_skinny.tga".into()),
            reticle_side_size: 8.0,
            reticle_center_size: 4.0,
            hip_spread_stand_min: 1.5,
            hip_spread_ducked_min: 1.0,
            hip_spread_prone_min: 0.5,
            hip_spread_max: 5.0,
            ads_crosshair_in_frac: 1.0,
            ads_crosshair_out_frac: 0.2,
            display_name: "WEAPON_M1A1CARBINE".into(),
            hud_icon: Some("gfx/icons/hud@m1carbine.tga".into()),
            ammo_icon: Some("gfx/icons/hud@ammo2.tga".into()),
            clip_index: 10,
            ammo_index: 10,
            ..Default::default()
        }
    }

    fn view<'a>(ammo: &'a [i16; 64], objectives: &'a [Objective]) -> PlayerView<'a> {
        PlayerView {
            client_num: 0,
            health: 100,
            max_health: 100,
            eflags: 0,
            weapon: None,
            ammo,
            ammoclip: ammo,
            aim_spread_scale: 0.0,
            ads_frac: 0.0,
            ads_held: false,
            view_yaw: 0.0,
            eye: [0.0; 3],
            fov: (80.0, 64.0),
            objectives,
            cursor_hint: 0,
            cursor_hint_string: -1,
            damage: DamageFeedback::default(),
        }
    }

    fn close(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-3
    }

    #[test]
    fn crosshair_arms_open_with_the_spread_scale() {
        let def = carbine();
        // Standing, 1.5 degrees at scale 0 and 5 at 255, 640/80 px a degree.
        let (x, y) = arm_offset(&def, 0, 0.0, 1.0, (80.0, 64.0));
        assert!(close(x, 12.0) && close(y, 11.25), "{x} {y}");
        let (x, y) = arm_offset(&def, 0, 255.0, 1.0, (80.0, 64.0));
        assert!(close(x, 40.0) && close(y, 37.5), "{x} {y}");
        // Prone reads its own minimum, and `reticleMinOfs` floors it.
        let (x, _) = arm_offset(&def, EF_PRONE, 0.0, 1.0, (80.0, 64.0));
        assert!(close(x, 4.0), "{x}");
        let floored = WeaponDef {
            reticle_min_ofs: 17.0,
            ..carbine()
        };
        assert_eq!(
            arm_offset(&floored, EF_PRONE, 0.0, 1.0, (80.0, 64.0)).0,
            17.0
        );
    }

    #[test]
    fn crosshair_arms_neither_overlap_at_zero_nor_leave_the_screen_at_full() {
        let def = carbine();
        let ammo = [0i16; 64];
        let v = Virtual::new((1920.0, 1080.0));
        for spread in [0.0, 255.0] {
            let p = PlayerView {
                aim_spread_scale: spread,
                ..view(&ammo, &[])
            };
            let mut out = Vec::new();
            crosshair(&def, &p, &v, &mut out);
            assert_eq!(out.len(), 4, "four arms, no centre image");
            let (cx, cy) = (960.0, 540.0);
            let top = &out[0].verts;
            assert!(top[2][1] <= cy && top[3][1] <= cy, "top arm below centre");
            let right = &out[1].verts;
            assert!(right[0][0] >= cx, "right arm left of centre");
            for q in &out {
                for [x, y] in q.verts {
                    assert!((0.0..=1920.0).contains(&x) && (0.0..=1080.0).contains(&y));
                }
            }
        }
    }

    #[test]
    fn crosshair_hides_at_full_sight() {
        let ammo = [0i16; 64];
        let p = PlayerView {
            ads_frac: 1.0,
            ..view(&ammo, &[])
        };
        let mut out = Vec::new();
        crosshair(&carbine(), &p, &Virtual::new((640.0, 480.0)), &mut out);
        assert!(out.is_empty());
    }

    #[test]
    fn health_bar_is_the_health_share() {
        assert_eq!(health_fraction(50, 100), 0.5);
        assert_eq!(health_fraction(100, 100), 1.0);
        assert_eq!(health_fraction(130, 100), 1.0);
        assert_eq!(health_fraction(-5, 100), 0.0);
        assert_eq!(health_fraction(50, 0), 0.0);

        let font = test_font();
        let ammo = [0i16; 64];
        let cs = vec![String::new(); 2048];
        let origin = |_: i32| None;
        let cx = Context {
            weapons: &[],
            configstrings: &cs,
            loc: &Localized::default(),
            font: &font,
            entity_origin: &origin,
        };
        let p = PlayerView {
            health: 50,
            ..view(&ammo, &[])
        };
        let mut out = Vec::new();
        PlayerHud::default().build(&p, &cx, 0, (640.0, 480.0), &mut out);
        let bar = out
            .iter()
            .find(|q| q.texture == "gfx/hud/hud@health_bar.tga")
            .expect("health bar");
        assert!(close(bar.verts[1][0] - bar.verts[0][0], 64.0));
        assert!(close(bar.uvs[1][0], 0.5), "cropped, not squeezed");
    }

    #[test]
    fn objective_due_north_of_a_player_facing_east_sits_left() {
        // World yaw 0 is +x (east here) and 90 is +y (north).
        let (x, y) = compass_point(0.0, [0.0; 3], [0.0, 512.0, 0.0]);
        // Half the 1024-unit range: half the 43.75 radius, left of (55, 425).
        assert!(close(x, 55.0 - 21.875) && close(y, 425.0), "{x} {y}");
        let (x, y) = compass_point(90.0, [0.0; 3], [0.0, 4096.0, 0.0]);
        assert!(close(x, 55.0) && close(y, 425.0 - 43.75), "{x} {y}");
    }

    #[test]
    fn damage_icon_only_on_an_event_change() {
        let mut d = DamageIndicators::default();
        let hit = |event: i32, count: i32| DamageFeedback {
            event,
            yaw: 64,
            pitch: 0,
            count,
        };
        // The first playerstate seen is the baseline, not a hit.
        d.feed(hit(3, 20), 1_000);
        assert_eq!(d.live(1_000).count(), 0);
        d.feed(hit(4, 20), 1_050);
        assert_eq!(d.live(1_100).count(), 1);
        // The same fields again: no new hit.
        d.feed(hit(4, 20), 1_100);
        assert_eq!(d.live(1_150).count(), 1);
        // A change with no damage, and damage from nowhere, add nothing.
        d.feed(hit(5, 0), 1_200);
        d.feed(
            DamageFeedback {
                event: 6,
                yaw: 255,
                pitch: 255,
                count: 10,
            },
            1_250,
        );
        assert_eq!(d.live(1_300).count(), 1);
        assert_eq!(d.live(3_100).count(), 0, "gone 2 s after the hit");
    }

    #[test]
    fn damage_from_the_left_points_left() {
        let mut d = DamageIndicators::default();
        d.feed(DamageFeedback::default(), 0);
        // Viewer faces yaw 0; the hit travelled along yaw 270 (toward -y),
        // so it came from +y, the viewer's left.
        d.feed(
            DamageFeedback {
                event: 1,
                yaw: 191,
                pitch: 0,
                count: 30,
            },
            0,
        );
        let mut out = Vec::new();
        d.build(0.0, 100, &Virtual::new((640.0, 480.0)), &mut out);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].texture, "hudHitDirection");
        let mx = out[0].verts.iter().map(|v| v[0]).sum::<f32>() / 4.0;
        let my = out[0].verts.iter().map(|v| v[1]).sum::<f32>() / 4.0;
        assert!(mx < 320.0 - 50.0 && (my - 240.0).abs() < 2.0, "{mx} {my}");
    }

    #[test]
    fn cursor_hint_maps_health_weapons_and_none() {
        let weapons = vec![None, Some(carbine())];
        assert_eq!(hint_material(7, &weapons).as_deref(), Some("hintHealth"));
        assert_eq!(hint_material(2, &weapons).as_deref(), Some("hintActivate"));
        assert_eq!(hint_material(0, &weapons), None);
        assert_eq!(hint_material(1, &weapons), None);
        assert_eq!(hint_material(255, &weapons), None);
        assert_eq!(
            hint_material(9 + 1, &weapons).as_deref(),
            Some("gfx/icons/hud@m1carbine.tga")
        );
        assert_eq!(
            hint_material(73 + 1, &weapons).as_deref(),
            Some("gfx/icons/hud@ammo2.tga")
        );
        // A weapon with no icon falls back to the use hint.
        let bare = vec![None, None, Some(WeaponDef::default())];
        assert_eq!(hint_material(9 + 2, &bare).as_deref(), Some("hintActivate"));
    }
}
