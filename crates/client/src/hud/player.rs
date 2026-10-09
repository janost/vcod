//! The native player HUD: crosshair, health, ammo, weapon name and fire-mode
//! icon, stance, compass with teammates and objectives, cursor hint and
//! damage direction, at the rects
//! pak0 `ui_mp/hud.menu` gives. How the cgame draws each:
//! docs/research/cod11-hud-protocol.md, section 9.

use super::HudQuad;
use super::font::{self, Font};
use super::friends::{CompassFriends, Sighting};
use super::hudelem::{self, CS_SHADERS, Virtual};
use super::scope::{self, GunAim};
use crate::turret::EF_MOUNTED;
use vcod_common::localize::Localized;
use vcod_common::net::msg::Objective;
use vcod_common::pmove::weapon::{SpreadStance, hip_spread_min};
use vcod_common::weapon::{SightDirection, WeaponDef};

/// Hint strings (`serverCursorHintString`) index configstrings from here.
pub use vcod_common::net::protocol::{CS_HINT_STRINGS, CS_NORTHYAW};

/// `cg_crosshairAlpha` and `cg_crosshairAlphaMin` at their cvar-table defaults.
const CROSSHAIR_ALPHA: f32 = 1.0;
const CROSSHAIR_ALPHA_MIN: f32 = 0.7;
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
    /// The mounted gun's def, while riding one.
    pub turret: Option<&'a WeaponDef>,
    pub ammo: &'a [i16; 64],
    pub ammoclip: &'a [i16; 64],
    /// 0..255.
    pub aim_spread_scale: f32,
    /// What the hip minimum is read off, timed against retail's
    /// `cg.snap->serverTime`.
    pub spread_stance: SpreadStance,
    pub ads_frac: f32,
    /// World degrees.
    pub view_yaw: f32,
    pub eye: [f32; 3],
    /// Horizontal and vertical, degrees.
    pub fov: (f32, f32),
    /// The gun's angles off the view, which the scope overlay is centred
    /// on; `None` centres it.
    pub gun_angles: Option<[f32; 3]>,
    pub objectives: &'a [Objective],
    /// Teammates seen this frame, for the compass.
    pub friends: &'a [Sighting],
    /// `pm_type` below 4: the crosshair and a mounted gun's reticle draw.
    pub alive: bool,
    pub cursor_hint: i32,
    /// Signed; below 0 is none.
    pub cursor_hint_string: i32,
    pub damage: DamageFeedback,
    /// `stats[5]`, the server's spawn counter; a new value is a respawn.
    pub spawn_count: i32,
    /// `pm_flags` 0x8000, the server refusing a dive to prone.
    pub prone_blocked: bool,
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
    /// The key text a command is bound to, `None` while unbound.
    pub bound_key: &'a dyn Fn(&str) -> Option<String>,
}

/// The state the native HUD keeps across frames.
#[derive(Default)]
pub struct PlayerHud {
    pub damage: DamageIndicators,
    health_lag: HealthLag,
    sight: SightDirection,
    pub gun: GunAim,
    friends: CompassFriends,
    pub stance: StanceFlash,
    pub weapon_name: WeaponNameFade,
    compass_spring: CompassSpring,
    prone_blocked: ProneBlocked,
    /// The playerstate's `clientNum` last frame; a new one is a new baseline
    /// for the damage feedback.
    client: Option<i32>,
}

impl PlayerHud {
    /// The HUD is not drawn this frame: start the next from a fresh
    /// baseline, so a hit taken meanwhile does not flash on return. The
    /// stance flash, the weapon name's stamp and the compass spring keep
    /// their own, as retail's statics do.
    pub fn hidden(&mut self) {
        *self = PlayerHud {
            stance: std::mem::take(&mut self.stance),
            weapon_name: std::mem::take(&mut self.weapon_name),
            compass_spring: std::mem::take(&mut self.compass_spring),
            ..PlayerHud::default()
        };
    }

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
        if self.client.replace(p.client_num) != Some(p.client_num) {
            self.damage = DamageIndicators::default();
        }
        self.damage.feed(p.damage, now);
        let raising = self.sight.step(p.weapon, p.ads_frac);
        // Drawn first, so the menu HUD sits on top of it.
        let scoped = p
            .weapon
            .filter(|_| p.eflags & EF_MOUNTED == 0)
            .filter(|def| scope::overlay_frac(def, p.ads_frac, raising).is_some());
        if let Some(def) = scoped {
            let at = scope::gun_point(p.gun_angles.unwrap_or_default(), p.fov, screen);
            scope::build(def, at, screen, out);
        }

        let north = cx
            .configstrings
            .get(CS_NORTHYAW)
            .and_then(|s| s.trim().parse::<f32>().ok())
            .unwrap_or(0.0);
        self.friends.feed(now, p.friends.iter().copied());
        let face_yaw = self.compass_spring.step(p.view_yaw - north, now);
        compass(p, cx, face_yaw, &mut self.friends, now, &v, out);
        if let Some(alpha) = self.prone_blocked.step(p.prone_blocked, now) {
            prone_blocked(cx, alpha, &v, out);
        }
        let bits = (p.spread_stance.prone, p.spread_stance.ducked);
        let flash = self.stance.step(bits, now);
        if let Some(alpha) = self.stance.hint_alpha(now) {
            stance_hints(bits, cx, alpha, &v, out);
        }
        stance(bits, flash, &v, out);
        let frac = health_fraction(p.health, p.max_health);
        let lag = self.health_lag.step(p.client_num, frac, now);
        health(frac, lag, &v, out);
        let name_alpha = self.weapon_name.step((p.client_num, p.spawn_count), now);
        if let Some(def) = p.weapon {
            weapon_info(def, p, cx, name_alpha, &v, out);
            if p.alive && p.eflags & EF_MOUNTED == 0 {
                crosshair(def, p, raising, &v, out);
            }
        }
        // Mounted, the gun's reticle replaces the weapon's, carried weapon or not.
        if p.alive
            && p.eflags & EF_MOUNTED != 0
            && let Some(def) = p.turret
        {
            turret_reticle(def, &v, out);
        }
        cursor_hint(p, cx, now, &v, out);
        // `cg_hudDamageIconInScope` 0.
        if scoped.is_none() {
            self.damage.build(p.view_yaw, now, &v, out);
        }
    }
}

/// Text at a virtual baseline, at a menu `textscale`.
pub(super) fn menu_text(
    font: &Font,
    s: &str,
    (x, baseline): (f32, f32),
    textscale: f32,
    v: &Virtual,
    out: &mut Vec<HudQuad>,
) {
    menu_text_rgba(font, s, (x, baseline), textscale, WHITE, v, out);
}

/// [`menu_text`] in `rgba`, which a `^N` code overrides and `^7` restores.
pub(super) fn menu_text_rgba(
    font: &Font,
    s: &str,
    (x, baseline): (f32, f32),
    textscale: f32,
    rgba: [f32; 4],
    v: &Virtual,
    out: &mut Vec<HudQuad>,
) {
    let top = baseline - text_height(font, textscale);
    hudelem::text(
        font,
        s,
        (x, top),
        textscale * v.scale / font.unit_scale(),
        rgba,
        v,
        out,
    );
}

pub(super) fn text_width(font: &Font, s: &str, textscale: f32) -> f32 {
    font::measure(font, s, textscale / font.unit_scale())
}

pub(super) fn text_height(font: &Font, textscale: f32) -> f32 {
    font.max_height as f32 * font.glyph_scale * textscale
}

/// The icon for `pm_flags`' prone and ducked bits, and the flash over it
/// at `flash` alpha.
fn stance((prone, ducked): (bool, bool), flash: Option<f32>, v: &Virtual, out: &mut Vec<HudQuad>) {
    let material = if prone {
        "hudStanceProne"
    } else if ducked {
        "hudStanceCrouch"
    } else {
        "hudStanceStand"
    };
    out.push(v.quad(100.0, 434.375, 40.0, 40.0, WHITE, material));
    if let Some(alpha) = flash {
        // `cg_hudStanceFlash_r`, `_g` and `_b`.
        let rgba = [1.0, 1.0, 0.3, alpha];
        out.push(v.quad(100.0, 434.375, 40.0, 40.0, rgba, "hudStanceFlash"));
    }
}

/// When the stance last changed. Kept across the HUD hiding, as retail's
/// statics are, so a stance changed meanwhile flashes on return.
#[derive(Default)]
pub struct StanceFlash {
    changed: i32,
    last: Option<(bool, bool)>,
}

impl StanceFlash {
    /// The flash's alpha this frame: 0.8 at the change, gone a second later.
    /// The first stance seen counts as a change. Assumes
    /// `cg_hudStanceHintPrints` 1, which pak0's `configure_mp.cfg` sets.
    pub fn step(&mut self, bits: (bool, bool), now: i32) -> Option<f32> {
        if now < self.changed || self.last.replace(bits) != Some(bits) {
            self.changed = now;
        }
        let left = self.changed + 1000 - now;
        (left > 0).then_some(left as f32 * 0.001 * 0.8)
    }

    /// The key hints' alpha after [`StanceFlash::step`]: 1 for 2 s from the
    /// change, then fading out over the third.
    pub fn hint_alpha(&self, now: i32) -> Option<f32> {
        let left = self.changed + 3000 - now;
        (left > 0).then(|| (left as f32 * 0.001).min(1.0))
    }
}

/// `CGAME_STANCEHINT_JUMP`, `_STAND`, `_CROUCH` and `_PRONE`, each with the
/// commands it names in that stance, tried in order until one is bound
/// (0x30023f50's tables).
const STANCE_HINTS: [&str; 4] = [
    "CGAME_STANCEHINT_JUMP",
    "CGAME_STANCEHINT_STAND",
    "CGAME_STANCEHINT_CROUCH",
    "CGAME_STANCEHINT_PRONE",
];

fn stance_hint_commands((prone, ducked): (bool, bool)) -> [&'static [&'static str]; 4] {
    if prone {
        [
            &[],
            &["+gostand", "toggleprone"],
            &[
                "gocrouch",
                "togglecrouch",
                "raisestance",
                "+movedown",
                "+moveup",
            ],
            &[],
        ]
    } else if ducked {
        [
            &[],
            &["+gostand", "raisestance", "+moveup"],
            &[],
            &["goprone", "lowerstance", "toggleprone", "+prone"],
        ]
    } else {
        [
            &["+gostand", "+moveup"],
            &[],
            &["gocrouch", "togglecrouch", "lowerstance", "+movedown"],
            &["goprone", "+prone"],
        ]
    }
}

/// The stance moves the bound keys offer, printed right of the stance icon
/// and centred on it, one line each.
fn stance_hints(bits: (bool, bool), cx: &Context, alpha: f32, v: &Virtual, out: &mut Vec<HudQuad>) {
    let lines = stance_hint_lines(bits, cx.bound_key, cx.loc);
    let h = text_height(cx.font, 0.21);
    let mut y = 434.375 + 40.0 * 0.5 - 1.5;
    match lines.len() {
        1 => y += h * 0.5,
        3 => y -= h * 0.5 + 1.5,
        _ => {}
    }
    for line in lines {
        menu_text_rgba(
            cx.font,
            &line,
            (140.0, y),
            0.21,
            [1.0, 1.0, 1.0, alpha],
            v,
            out,
        );
        y += h + 1.5;
    }
}

/// The hint lines for the stance's moves, in jump, stand, crouch, prone
/// order, each with the key of its first bound command.
fn stance_hint_lines(
    bits: (bool, bool),
    bound_key: &dyn Fn(&str) -> Option<String>,
    loc: &Localized,
) -> Vec<String> {
    STANCE_HINTS
        .iter()
        .zip(stance_hint_commands(bits))
        .filter_map(|(label, cmds)| {
            let key = cmds.iter().find_map(|c| bound_key(c))?;
            let text = loc.get(label).unwrap_or(label);
            Some(text.replacen("%s", &key, 1))
        })
        .collect()
}

/// `CGAME_PRONE_BLOCKED` across the screen centre, blinking three times.
fn prone_blocked(cx: &Context, alpha: f32, v: &Virtual, out: &mut Vec<HudQuad>) {
    let text = cx
        .loc
        .get("CGAME_PRONE_BLOCKED")
        .unwrap_or("CGAME_PRONE_BLOCKED");
    let x = 320.0 - text_width(cx.font, text, 0.21) * 0.5;
    menu_text_rgba(
        cx.font,
        text,
        (x, 270.0),
        0.21,
        [1.0, 1.0, 1.0, alpha],
        v,
        out,
    );
}

/// When the prone-blocked notice runs out: 1.5 s after the flag is seen
/// with no notice running.
#[derive(Default)]
struct ProneBlocked {
    until: i32,
}

impl ProneBlocked {
    fn step(&mut self, blocked: bool, now: i32) -> Option<f32> {
        if blocked && self.until < now {
            self.until = now + 1500;
        }
        let left = self.until - now;
        // 0.36 degrees a ms: |sin| peaks three times in 1.5 s.
        (left > 0).then(|| (left as f32 * 0.36f32.to_radians()).sin().abs())
    }
}

/// The weapon name's stamp (0x3020c920): set by a respawn, a new followed
/// client and every weapon-select bind; the name shows for 1.8 s from it.
#[derive(Default)]
pub struct WeaponNameFade {
    stamp: Option<i32>,
    spawn: Option<(i32, i32)>,
    selected: bool,
}

impl WeaponNameFade {
    /// A weapon-select bind or the server's `a` command; stamps at the next
    /// [`WeaponNameFade::step`].
    pub fn select(&mut self) {
        self.selected = true;
    }

    /// The name's alpha this frame for the drawn `(clientNum, stats[5])`.
    pub fn step(&mut self, spawn: (i32, i32), now: i32) -> Option<f32> {
        if self.spawn.replace(spawn) != Some(spawn) || std::mem::take(&mut self.selected) {
            self.stamp = Some(now);
        }
        fade_color_alpha(self.stamp, 1800, now)
    }
}

/// `0x30019a30`: full for `total` ms from `start`, the last 100 fading out.
pub fn fade_color_alpha(start: Option<i32>, total: i32, now: i32) -> Option<f32> {
    let start = start.filter(|&s| s != 0)?;
    let left = total - (now - start);
    (now - start < total).then_some(if left < 100 { left as f32 * 0.01 } else { 1.0 })
}

/// `ANGLE2SHORT` then `SHORT2ANGLE`: the angle wrapped into 0..360 on a
/// 1/65536 turn grid, truncated as the cgame's `_ftol` does.
fn angle_mod(a: f32) -> f32 {
    let short = (f64::from(a) * f64::from(65536.0f32 / 360.0)) as i32 & 0xffff;
    short as f32 * (360.0 / 65536.0)
}

/// `a - b` in -180..180.
fn angle_sub(a: f32, b: f32) -> f32 {
    let mut d = a - b;
    while d > 180.0 {
        d -= 360.0;
    }
    while d < -180.0 {
        d += 360.0;
    }
    d
}

/// The compass face's spring (0x30019bd0): the drawn yaw chases the view's
/// in 5 ms steps, pulled at 1000 deg/s/s, damped at 2/s and at a further
/// 3.5/s while it swings away. More than 500 ms since the last step snaps it.
#[derive(Default)]
pub struct CompassSpring {
    last: i32,
    yaw: f32,
    /// Degrees a second.
    speed: f32,
}

impl CompassSpring {
    /// The face's yaw for a view yaw less `northyaw` of `target`.
    pub fn step(&mut self, target: f32, now: i32) -> f32 {
        let target = angle_mod(target);
        let elapsed = now - self.last;
        if self.last > now || elapsed > 500 {
            self.last = now;
            self.yaw = target;
            self.speed = 0.0;
            return self.yaw;
        }
        self.last = now;
        let mut delta = angle_sub(self.yaw, target);
        let mut left = elapsed;
        while left > 0 {
            let ms = left.min(5);
            left -= ms;
            let dt = ms as f32 * 0.001;
            if delta.abs() < 0.25 && self.speed.abs() < 1.0 {
                self.speed = 0.0;
                self.yaw = target;
                return target;
            }
            delta = angle_mod(self.speed * dt + delta);
            if delta > 180.0 {
                delta -= 360.0;
            }
            if delta > 0.0 {
                self.speed -= dt * 1000.0;
            } else if delta < 0.0 {
                self.speed += dt * 1000.0;
            }
            self.speed -= self.speed * dt * 2.0;
            // A small constant drag, then a stop rather than a reversal.
            if self.speed > 0.0 {
                if delta > 0.0 {
                    self.speed -= self.speed * dt * 3.5;
                }
                self.speed -= dt;
                if self.speed < 0.0 {
                    self.speed = 0.0;
                    continue;
                }
            } else {
                if delta < 0.0 {
                    self.speed -= self.speed * dt * 3.5;
                }
                self.speed += dt;
                if self.speed > 0.0 {
                    self.speed = 0.0;
                    continue;
                }
            }
            self.speed = self.speed.clamp(-30000.0, 30000.0);
        }
        self.yaw = angle_mod(delta + target);
        self.yaw
    }
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
/// `name_alpha` is the name's fade; `None` leaves it and its backdrop out.
fn weapon_info(
    def: &WeaponDef,
    p: &PlayerView,
    cx: &Context,
    name_alpha: Option<f32>,
    v: &Virtual,
    out: &mut Vec<HudQuad>,
) {
    let translate = |key: &str| cx.loc.get(key).unwrap_or(key).to_string();
    let name = match def.mode_name.as_str() {
        "" => translate(&def.display_name),
        mode => format!("{} / {}", translate(&def.display_name), translate(mode)),
    };
    // hud.menu's item order: name back, ammo back, mode icon, name, ammo.
    let w = text_width(cx.font, &name, 0.3);
    let faded = name_alpha.map(|a| [1.0, 1.0, 1.0, a]);
    if let Some(rgba) = faded {
        out.push(v.quad(
            562.5 - (w + 36.0),
            431.0,
            w + 36.0,
            20.0,
            rgba,
            "gfx/hud/hud@weaponnameback.tga",
        ));
    }
    out.push(v.quad(
        557.5,
        421.625,
        80.0,
        40.0,
        WHITE,
        "gfx/hud/hud@ammocounterback.tga",
    ));
    if let Some(icon) = &def.mode_icon {
        out.push(v.quad(537.5, 430.375, 20.0, 20.0, WHITE, icon));
    }
    if let Some(rgba) = faded {
        menu_text_rgba(cx.font, &name, (562.5 - w - 28.0, 446.0), 0.3, rgba, v, out);
    }

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
    stance: &SpreadStance,
    aim_spread_scale: f32,
    shrink: f32,
    (fov_x, fov_y): (f32, f32),
) -> (f32, f32) {
    let min = hip_spread_min(def, stance);
    let spread = (aim_spread_scale / 255.0 * (def.hip_spread_max - min) + min) * shrink;
    (
        (640.0 / fov_x * spread).max(def.reticle_min_ofs),
        (480.0 / fov_y * spread).max(def.reticle_min_ofs),
    )
}

/// The crosshair's quads, none at full sight or with no reticle. Its images
/// are sized in window pixels; only the arms' travel scales with the screen.
/// `raising` picks `adsCrosshairInFrac` over `adsCrosshairOutFrac`. Under a
/// scope overlay it fades by what is left of the zoom tail.
pub fn crosshair(
    def: &WeaponDef,
    p: &PlayerView,
    raising: bool,
    v: &Virtual,
    out: &mut Vec<HudQuad>,
) {
    let fade = 1.0 - scope::overlay_frac(def, p.ads_frac, raising).unwrap_or(0.0);
    if fade * CROSSHAIR_ALPHA < 0.01 || p.ads_frac >= 1.0 {
        return;
    }
    let mut shrink = 1.0;
    let tail = if raising {
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
    let px_quad = |x: f32, y: f32, s: f32, uvs: [[f32; 2]; 4], rgba, texture: &str| HudQuad {
        verts: [[x, y], [x + s, y], [x + s, y + s], [x, y + s]],
        uvs,
        rgba,
        texture: texture.to_string(),
    };

    if let Some(center) = &def.reticle_center {
        let s = def.reticle_center_size * shrink;
        let uvs = [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]];
        let rgba = [1.0, 1.0, 1.0, CROSSHAIR_ALPHA * fade];
        out.push(px_quad(cx - s / 2.0, cy - s / 2.0, s, uvs, rgba, center));
    }
    let Some(side) = &def.reticle_side else {
        return;
    };
    let (ox, oy) = arm_offset(def, &p.spread_stance, p.aim_spread_scale, shrink, p.fov);
    let arm_rgba = [1.0, 1.0, 1.0, arm_alpha(p.aim_spread_scale, fade)];
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
        out.push(px_quad(x, y, s, uvs, arm_rgba, side));
    }
}

/// 0x30016610: the mounted gun's `reticleCenter`, `reticleCenterSize`
/// virtual units square on the screen's centre, at `cg_crosshairAlpha`.
/// Unlike the weapon reticle's, the size takes the screen scale.
pub fn turret_reticle(def: &WeaponDef, v: &Virtual, out: &mut Vec<HudQuad>) {
    let Some(center) = &def.reticle_center else {
        return;
    };
    let s = def.reticle_center_size;
    let rgba = [1.0, 1.0, 1.0, CROSSHAIR_ALPHA];
    out.push(v.quad(320.0 - s / 2.0, 240.0 - s / 2.0, s, s, rgba, center));
}

/// The arms fade as the spread opens and as a scope comes up, down to
/// `cg_crosshairAlphaMin`.
pub fn arm_alpha(aim_spread_scale: f32, fade: f32) -> f32 {
    ((1.0 - aim_spread_scale / 255.0) * fade * CROSSHAIR_ALPHA).max(CROSSHAIR_ALPHA_MIN)
}

/// The compass rect's centre, where objective bearings are measured from.
pub const COMPASS_CENTRE: (f32, f32) = (55.0, 425.0);
/// How far from the centre a mark at `cg_hudCompassMaxRange` or beyond sits.
pub const COMPASS_RADIUS: f32 = 43.75;

/// `face_yaw` turns the back and the face: the view's yaw less `northyaw`,
/// through [`CompassSpring`].
fn compass(
    p: &PlayerView,
    cx: &Context,
    face_yaw: f32,
    friends: &mut CompassFriends,
    now: i32,
    v: &Virtual,
    out: &mut Vec<HudQuad>,
) {
    let (x, y, size) = (-25.0, 345.0, 160.0);
    let half = size / 2.0;
    let corners = [[-half, -half], [half, -half], [half, half], [-half, half]];
    let turn = face_yaw;
    // hud.menu's item order: back, highlight, face, needle.
    let back = "gfx/hud/hud@compassback.tga";
    out.push(v.rotated(COMPASS_CENTRE, corners, turn, WHITE, back));
    out.push(v.quad(x, y, size, size, WHITE, "gfx/hud/hud@compasshighlight.tga"));
    let face = "gfx/hud/hud@compassface.tga";
    out.push(v.rotated(COMPASS_CENTRE, corners, turn, WHITE, face));
    out.push(v.quad(
        x + 60.0,
        y + 50.0,
        40.0,
        40.0,
        WHITE,
        "gfx/hud/hud@compass_arrow.tga",
    ));
    friends.build(now, p.client_num, (p.view_yaw, p.eye), v, out);

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
    let (dx, dy) = compass_offset(view_yaw, eye, target);
    (COMPASS_CENTRE.0 + dx, COMPASS_CENTRE.1 + dy)
}

/// [`compass_point`] less the compass centre.
pub fn compass_offset(view_yaw: f32, eye: [f32; 3], target: [f32; 3]) -> (f32, f32) {
    let (dx, dy) = (target[0] - eye[0], target[1] - eye[1]);
    let bearing = (dy.atan2(dx).to_degrees() - view_yaw).to_radians();
    let r = COMPASS_RADIUS * ((dx * dx + dy * dy).sqrt() / 1024.0).clamp(0.0, 1.0);
    (-bearing.sin() * r, -bearing.cos() * r)
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
/// and `[{+activate}]` filled with the use key.
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
    // The configstring is a packed message (`RE_PRESS_TO_PICKUP\x14...`),
    // localized and key-bound the way a game message is (0x300229b0).
    let s = super::bind_keys(&cx.loc.message(key)).replace("[%s]", "[F]");
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
    /// MSVC `rand`'s state, for the yaw jitter.
    seed: u32,
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
        let yaw = fb.yaw as f32 / 255.0 * 360.0;
        let jitter = (self.rand() as f32 / 32768.0 - 0.5) * 20.0;
        self.slots[slot] = Some((now, angle_mod(jitter + yaw)));
    }

    /// 0..32767, MSVC's `rand` LCG.
    fn rand(&mut self) -> u32 {
        self.seed = self.seed.wrapping_mul(214_013).wrapping_add(2_531_011);
        (self.seed >> 16) & 0x7fff
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
    use crate::play::input::EF_PRONE;

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
            turret: None,
            ammo,
            ammoclip: ammo,
            aim_spread_scale: 0.0,
            spread_stance: SpreadStance::default(),
            ads_frac: 0.0,
            view_yaw: 0.0,
            eye: [0.0; 3],
            fov: (80.0, 64.0),
            gun_angles: None,
            objectives,
            friends: &[],
            alive: true,
            cursor_hint: 0,
            cursor_hint_string: -1,
            damage: DamageFeedback::default(),
            spawn_count: 0,
            prone_blocked: false,
        }
    }

    fn close(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-3
    }

    #[test]
    fn crosshair_arms_open_with_the_spread_scale() {
        let def = carbine();
        // Standing, 1.5 degrees at scale 0 and 5 at 255, 640/80 px a degree.
        let stand = SpreadStance::default();
        let (x, y) = arm_offset(&def, &stand, 0.0, 1.0, (80.0, 64.0));
        assert!(close(x, 12.0) && close(y, 11.25), "{x} {y}");
        let (x, y) = arm_offset(&def, &stand, 255.0, 1.0, (80.0, 64.0));
        assert!(close(x, 40.0) && close(y, 37.5), "{x} {y}");
        // Prone reads its own minimum, and `reticleMinOfs` floors it.
        let prone = SpreadStance {
            prone: true,
            ..stand
        };
        let (x, _) = arm_offset(&def, &prone, 0.0, 1.0, (80.0, 64.0));
        assert!(close(x, 4.0), "{x}");
        let floored = WeaponDef {
            reticle_min_ofs: 17.0,
            ..carbine()
        };
        assert_eq!(arm_offset(&floored, &prone, 0.0, 1.0, (80.0, 64.0)).0, 17.0);
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
            crosshair(&def, &p, false, &v, &mut out);
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
        crosshair(
            &carbine(),
            &p,
            false,
            &Virtual::new((640.0, 480.0)),
            &mut out,
        );
        assert!(out.is_empty());
    }

    #[test]
    fn a_prone_raise_shrinks_by_the_in_frac() {
        let def = WeaponDef {
            aim_down_sight: true,
            ads_crosshair_in_frac: 0.5,
            ads_crosshair_out_frac: 0.2,
            ..carbine()
        };
        let mut sight = SightDirection::default();
        // Prone never sets pm_flags 0x80; only the fraction leaving 0 counts.
        assert!(!sight.step(Some(&def), 0.0));
        assert!(sight.step(Some(&def), 0.6));
        assert!(sight.step(Some(&def), 0.8), "held while the sight moves");
        let ammo = [0i16; 64];
        let p = PlayerView {
            eflags: EF_PRONE,
            ads_frac: 0.8,
            ..view(&ammo, &[])
        };
        let v = Virtual::new((640.0, 480.0));
        let side = |raising: bool| {
            let mut out = Vec::new();
            crosshair(&def, &p, raising, &v, &mut out);
            out[0].verts[1][0] - out[0].verts[0][0]
        };
        // 0.3 into the in-frac's 0.5 tail shrinks it to 0.7; 0.8 is not yet
        // inside the out-frac's 0.2 tail.
        assert!(
            close(side(true), def.reticle_side_size * 0.7),
            "{}",
            side(true)
        );
        assert!(close(side(false), def.reticle_side_size));

        // Leaving 1 downward clears it; a weapon without a sight holds it.
        assert!(sight.step(Some(&def), 1.0));
        assert!(!sight.step(Some(&def), 0.9));
        assert!(!sight.step(None, 0.0));
        assert!(!sight.step(None, 0.5));
    }

    #[test]
    fn a_mounted_gun_draws_no_weapon_crosshair() {
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
            bound_key: &|_| None,
        };
        let def = carbine();
        let arms = |eflags: i32| {
            let p = PlayerView {
                eflags,
                weapon: Some(&def),
                ..view(&ammo, &[])
            };
            let mut out = Vec::new();
            PlayerHud::default().build(&p, &cx, 0, (640.0, 480.0), &mut out);
            out.iter()
                .filter(|q| Some(&q.texture) == def.reticle_side.as_ref())
                .count()
        };
        assert_eq!(arms(0), 4);
        assert_eq!(arms(0xC000), 0);
    }

    /// Down a settled scope the overlay is drawn first, under the menu HUD,
    /// and neither the crosshair nor a fresh hit's icon is drawn over it.
    #[test]
    fn a_raised_scope_draws_the_overlay_under_the_hud() {
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
            bound_key: &|_| None,
        };
        let def = WeaponDef {
            aim_down_sight: true,
            ads_zoom_in_frac: 0.05,
            ads_zoom_out_frac: 0.05,
            ads_overlay_shader: Some("ui/assets/reticle_circle_quarter".into()),
            ads_overlay_reticle: vcod_common::weapon::OverlayReticle::Springfield,
            ads_overlay_width: 220.0,
            ads_overlay_height: 220.0,
            ..carbine()
        };
        let p = PlayerView {
            weapon: Some(&def),
            ads_frac: 1.0,
            damage: DamageFeedback {
                event: 1,
                count: 10,
                ..Default::default()
            },
            ..view(&ammo, &[])
        };
        let mut hud = PlayerHud::default();
        let mut out = Vec::new();
        hud.build(&p, &cx, 0, (640.0, 480.0), &mut out);
        assert_eq!(out[0].texture, "ui/assets/reticle_circle_quarter");
        let has = |t: &str| out.iter().any(|q| q.texture == t);
        assert!(has("hudSoftLineH"));
        assert!(has("gfx/hud/hud@health_back.tga"));
        assert!(!has("gfx/reticle/side_skinny.tga"), "no crosshair");
        assert!(!has("hudHitDirection"), "no damage icon in the scope");
    }

    #[test]
    fn a_mounted_gun_draws_its_own_reticle_on_the_centre() {
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
            bound_key: &|_| None,
        };
        let mg = WeaponDef {
            reticle_center: Some("gfx/reticle/mg42_cross.tga".into()),
            reticle_center_size: 32.0,
            ..Default::default()
        };
        let reticles = |eflags: i32, turret: Option<&WeaponDef>| {
            let p = PlayerView {
                eflags,
                turret,
                ..view(&ammo, &[])
            };
            let mut out = Vec::new();
            PlayerHud::default().build(&p, &cx, 0, (1920.0, 1080.0), &mut out);
            out.into_iter()
                .filter(|q| Some(&q.texture) == mg.reticle_center.as_ref())
                .collect::<Vec<_>>()
        };
        let drawn = reticles(0xC000, Some(&mg));
        let [q] = drawn.as_slice() else {
            panic!("{} reticles", drawn.len());
        };
        // 32 virtual units at 1080/480, centred on 960x540.
        assert_eq!(q.verts[0], [924.0, 504.0]);
        assert_eq!(q.verts[2], [996.0, 576.0]);
        assert!(reticles(0, Some(&mg)).is_empty(), "not mounted");
        assert!(reticles(0xC000, None).is_empty(), "no gun def");
    }

    #[test]
    fn crosshair_arms_fade_with_the_spread_down_to_the_floor() {
        assert_eq!(arm_alpha(0.0, 1.0), 1.0);
        assert!(close(arm_alpha(51.0, 1.0), 0.8));
        assert_eq!(arm_alpha(255.0, 1.0), CROSSHAIR_ALPHA_MIN);
        assert_eq!(arm_alpha(0.0, 0.5), CROSSHAIR_ALPHA_MIN, "a scope's fade");
        let ammo = [0i16; 64];
        let p = PlayerView {
            aim_spread_scale: 255.0,
            ..view(&ammo, &[])
        };
        let mut out = Vec::new();
        crosshair(
            &carbine(),
            &p,
            false,
            &Virtual::new((640.0, 480.0)),
            &mut out,
        );
        assert!(out.iter().all(|q| q.rgba[3] == CROSSHAIR_ALPHA_MIN));
    }

    #[test]
    fn compass_draws_in_the_menu_item_order() {
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
            bound_key: &|_| None,
        };
        let mut out = Vec::new();
        let v = Virtual::new((640.0, 480.0));
        compass(
            &view(&ammo, &[]),
            &cx,
            0.0,
            &mut CompassFriends::default(),
            0,
            &v,
            &mut out,
        );
        let order: Vec<&str> = out.iter().map(|q| q.texture.as_str()).collect();
        assert_eq!(
            order,
            [
                "gfx/hud/hud@compassback.tga",
                "gfx/hud/hud@compasshighlight.tga",
                "gfx/hud/hud@compassface.tga",
                "gfx/hud/hud@compass_arrow.tga",
            ]
        );
    }

    #[test]
    fn the_stance_flash_fades_over_a_second_from_each_change() {
        let mut f = StanceFlash::default();
        let close_to = |a: Option<f32>, b: f32| a.is_some_and(|a| close(a, b));
        // The first stance seen is a change.
        assert!(close_to(f.step((false, false), 10_000), 0.8));
        assert!(close_to(f.step((false, false), 10_500), 0.4));
        assert_eq!(f.step((false, false), 11_000), None);
        // Crouching restarts it; going prone from there again.
        assert!(close_to(f.step((false, true), 20_000), 0.8));
        assert!(close_to(f.step((true, true), 20_250), 0.8));
        assert!(close_to(f.step((true, true), 20_500), 0.6));
        // A clock that ran backward restarts it too.
        assert!(close_to(f.step((true, true), 5_000), 0.8));
    }

    #[test]
    fn stance_hints_show_for_three_seconds_fading_over_the_last() {
        let mut f = StanceFlash::default();
        f.step((false, false), 10_000);
        assert_eq!(f.hint_alpha(10_000), Some(1.0));
        assert_eq!(f.hint_alpha(11_999), Some(1.0));
        assert!(f.hint_alpha(12_500).is_some_and(|a| close(a, 0.5)));
        assert_eq!(f.hint_alpha(13_000), None);
    }

    #[test]
    fn stance_hints_name_the_first_bound_command_of_each_move() {
        let mut loc = Localized::default();
        loc.parse_into(
            "cgame",
            "REFERENCE STANCEHINT_JUMP\nLANG_ENGLISH \"Press [%s] to jump\"\n\
             REFERENCE STANCEHINT_STAND\nLANG_ENGLISH \"Press [%s] to stand\"\n\
             REFERENCE STANCEHINT_CROUCH\nLANG_ENGLISH \"Press [%s] to crouch\"\n\
             REFERENCE STANCEHINT_PRONE\nLANG_ENGLISH \"Press [%s] to go prone\"\n",
        );
        // The stock binds.
        let stock = |cmd: &str| {
            let key = match cmd {
                "+gostand" => "SPACE",
                "gocrouch" => "C",
                "goprone" => "CTRL",
                _ => return None,
            };
            Some(key.to_string())
        };
        assert_eq!(
            stance_hint_lines((false, false), &stock, &loc),
            [
                "Press [SPACE] to jump",
                "Press [C] to crouch",
                "Press [CTRL] to go prone"
            ]
        );
        assert_eq!(
            stance_hint_lines((false, true), &stock, &loc),
            ["Press [SPACE] to stand", "Press [CTRL] to go prone"]
        );
        assert_eq!(
            stance_hint_lines((true, false), &stock, &loc),
            ["Press [SPACE] to stand", "Press [C] to crouch"]
        );
        // A later command in a row stands in for an unbound first one.
        let moveup = |cmd: &str| (cmd == "+moveup").then(|| "U".to_string());
        assert_eq!(
            stance_hint_lines((true, false), &moveup, &loc),
            ["Press [U] to crouch"]
        );
    }

    #[test]
    fn the_prone_blocked_notice_blinks_for_a_second_and_a_half() {
        let mut b = ProneBlocked::default();
        assert_eq!(b.step(false, 1_000), None);
        // Armed when the flag is seen; held flags do not re-arm it early.
        assert!(b.step(true, 2_000).is_some_and(|a| close(a, 0.0)));
        assert!(b.step(true, 2_250).is_some_and(|a| close(a, 1.0)));
        assert!(b.step(false, 2_500).is_some_and(|a| close(a, 0.0)));
        assert_eq!(b.step(false, 3_500), None);
        assert!(b.step(true, 3_600).is_some(), "re-armed once it ran out");
    }

    #[test]
    fn the_weapon_name_shows_for_1800_ms_from_each_stamp() {
        assert_eq!(fade_color_alpha(None, 1800, 5_000), None);
        assert_eq!(fade_color_alpha(Some(1_000), 1800, 1_000), Some(1.0));
        assert_eq!(fade_color_alpha(Some(1_000), 1800, 2_700), Some(1.0));
        assert!(fade_color_alpha(Some(1_000), 1800, 2_750).is_some_and(|a| close(a, 0.5)));
        assert_eq!(fade_color_alpha(Some(1_000), 1800, 2_800), None);

        let mut w = WeaponNameFade::default();
        // The first playerstate is a spawn.
        assert_eq!(w.step((0, 1), 10_000), Some(1.0));
        assert_eq!(w.step((0, 1), 12_000), None);
        w.select();
        assert_eq!(w.step((0, 1), 13_000), Some(1.0));
        assert_eq!(w.step((0, 1), 15_000), None);
        // A respawn, and a new followed client.
        assert_eq!(w.step((0, 2), 16_000), Some(1.0));
        assert_eq!(w.step((3, 2), 20_000), Some(1.0));
    }

    #[test]
    fn a_faded_weapon_name_drops_name_and_backdrop_but_not_the_ammo() {
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
            bound_key: &|_| None,
        };
        let def = carbine();
        let p = PlayerView {
            weapon: Some(&def),
            ..view(&ammo, &[])
        };
        let mut hud = PlayerHud::default();
        let has = |out: &[HudQuad], t: &str| out.iter().any(|q| q.texture == t);
        let mut out = Vec::new();
        hud.build(&p, &cx, 10_000, (640.0, 480.0), &mut out);
        assert!(has(&out, "gfx/hud/hud@weaponnameback.tga"));
        out.clear();
        hud.build(&p, &cx, 12_000, (640.0, 480.0), &mut out);
        assert!(!has(&out, "gfx/hud/hud@weaponnameback.tga"));
        assert!(has(&out, "gfx/hud/hud@ammocounterback.tga"));
    }

    #[test]
    fn the_compass_spring_swings_past_and_settles() {
        let mut s = CompassSpring::default();
        // The first step snaps.
        assert_eq!(s.step(0.0, 10_000), 0.0);
        // A 90-degree turn: the face lags, then overshoots, then settles.
        let mut yaws = Vec::new();
        for t in (10_016..=14_000).step_by(16) {
            yaws.push(angle_sub(s.step(90.0, t), 0.0));
        }
        assert!(yaws[0] < 1.0, "lags the view: {}", yaws[0]);
        let peak = yaws.iter().copied().fold(f32::MIN, f32::max);
        assert!(peak > 90.5, "overshoots: {peak}");
        assert_eq!(
            *yaws.last().unwrap(),
            angle_mod(90.0),
            "settles on the view"
        );
        // A gap over 500 ms snaps it.
        assert_eq!(s.step(200.0, 14_600), angle_mod(200.0));
        // Within the window, a split frame steps as one: 5 ms at a time.
        let mut a = CompassSpring::default();
        let mut b = CompassSpring::default();
        a.step(0.0, 0);
        b.step(0.0, 0);
        a.step(45.0, 10);
        assert_eq!(a.step(45.0, 30), {
            b.step(45.0, 15);
            b.step(45.0, 30)
        });
    }

    #[test]
    fn the_compass_spring_turns_the_short_way_round() {
        let mut s = CompassSpring::default();
        s.step(350.0, 0);
        let y = s.step(10.0, 100);
        assert!(!(10.0..=350.0).contains(&y), "went the long way: {y}");
    }

    #[test]
    fn damage_icons_jitter_within_ten_degrees() {
        let mut d = DamageIndicators::default();
        d.feed(DamageFeedback::default(), 0);
        let mut seen = Vec::new();
        for event in 1..=8 {
            d.feed(
                DamageFeedback {
                    event,
                    yaw: 64,
                    pitch: 0,
                    count: 10,
                },
                event,
            );
        }
        for (_, yaw) in d.live(10) {
            assert!((yaw - 64.0 / 255.0 * 360.0).abs() <= 10.0, "{yaw}");
            seen.push(yaw);
        }
        assert_eq!(seen.len(), 8);
        assert!(seen.iter().any(|&y| y != seen[0]), "not jittered");
    }

    #[test]
    fn the_stance_icon_reads_pm_flags_and_the_flash_sits_on_it() {
        let v = Virtual::new((640.0, 480.0));
        let mut out = Vec::new();
        stance((true, true), Some(0.5), &v, &mut out);
        assert_eq!(out[0].texture, "hudStanceProne");
        assert_eq!(out[1].texture, "hudStanceFlash");
        assert_eq!(out[1].rgba, [1.0, 1.0, 0.3, 0.5]);
        assert_eq!(out[0].verts, out[1].verts);
        out.clear();
        stance((false, true), None, &v, &mut out);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].texture, "hudStanceCrouch");
    }

    #[test]
    fn a_select_fire_gun_shows_its_mode_icon_beside_the_ammo() {
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
            bound_key: &|_| None,
        };
        let icons = |def: &WeaponDef| {
            let p = PlayerView {
                weapon: Some(def),
                ..view(&ammo, &[])
            };
            let mut out = Vec::new();
            PlayerHud::default().build(&p, &cx, 0, (640.0, 480.0), &mut out);
            out.into_iter()
                .filter(|q| q.texture.contains("weaponmode"))
                .collect::<Vec<_>>()
        };
        let thompson = WeaponDef {
            mode_icon: Some("gfx/hud/hud@weaponmode_full.tga".into()),
            ..carbine()
        };
        let drawn = icons(&thompson);
        let [q] = drawn.as_slice() else {
            panic!("{} icons", drawn.len());
        };
        assert_eq!(q.verts[0], [537.5, 430.375]);
        assert_eq!(q.verts[2], [557.5, 450.375]);
        assert!(icons(&carbine()).is_empty());
    }

    #[test]
    fn a_dead_view_keeps_the_hud_but_not_the_crosshair() {
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
            bound_key: &|_| None,
        };
        let def = carbine();
        let p = PlayerView {
            weapon: Some(&def),
            alive: false,
            ..view(&ammo, &[])
        };
        let mut out = Vec::new();
        PlayerHud::default().build(&p, &cx, 0, (640.0, 480.0), &mut out);
        let has = |t: &str| out.iter().any(|q| q.texture == t);
        assert!(has("gfx/hud/hud@health_back.tga"));
        assert!(!has("gfx/reticle/side_skinny.tga"));
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
            bound_key: &|_| None,
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
        // Up to 10 degrees of jitter either way.
        assert!(mx < 320.0 - 50.0 && (my - 240.0).abs() < 12.0, "{mx} {my}");
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
