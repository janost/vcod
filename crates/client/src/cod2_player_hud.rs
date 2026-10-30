//! A CoD2 player's HUD: the stock `ui_mp/hud.menu` owner draws the 1.3
//! cgame paints for a playing client (`CG_OwnerDraw`, Mac 0x18cc30), the
//! crosshair (`CG_DrawCrosshair`, Mac 0x1d0784) with a scope's overlay
//! (`CG_DrawWeapReticle`), and the red damage flash and damage direction
//! icons (`CG_Draw2D`, Mac 0x1d114a). Each item's rect, alignment, colour,
//! text scale and background come from the menu; what an owner draw reads
//! is the newest playerstate. docs/research/cod2-hud.md has the facts.

use std::sync::Arc;

use vcod_cod2::net::delta::field_index;
use vcod_cod2::net::fields::CLIENT_STATE_FIELDS;
use vcod_cod2::net::protocol::CS_WEAPONS;
use vcod_cod2::net::{ConfigStrings, Frame, PlayerView};
use vcod_cod2::pmove::weapon::{WeaponParams, spread_for_weapon};
use vcod_cod2::weapon::{self, WeaponDef};
use vcod_common::asset_source::{Asset, AssetSource};
use vcod_common::localize::Localized;
use vcod_common::ui_menu::{MenuFile, UiItem};

use crate::hud::HudQuad;
use crate::hud::cod2elem::ScreenPlacement;
use crate::hud::font::{self, Slot, UiFonts};
use crate::play::cod2_events::Event2;
use crate::play::cod2_view::{DamageKick, SightDef, SightLatch};

/// The owner draws `ui/menudefinition.h` numbers and `hud.menu` uses.
mod od {
    pub const AMMO_VALUE: i32 = 5;
    pub const AMMO_BACKDROP: i32 = 6;
    pub const STANCE: i32 = 20;
    pub const AMMOCLIP_VALUE: i32 = 70;
    pub const CURSORHINT: i32 = 72;
    pub const BAR_HEALTH: i32 = 79;
    pub const MANTLE_HINT: i32 = 80;
    pub const WEAPON_NAME: i32 = 81;
    pub const WEAPON_MODE_ICON: i32 = 83;
    pub const COMPASS: i32 = 84;
    pub const COMPASS_BACK: i32 = 85;
    pub const COMPASS_POINTERS: i32 = 86;
    pub const COMPASS_FRIENDS: i32 = 89;
    pub const BAR_HEALTH_BACK: i32 = 95;
    pub const OFFHAND_ICON_FRAG: i32 = 100;
    pub const OFFHAND_ICON_SMOKE: i32 = 101;
    pub const OFFHAND_AMMO_FRAG: i32 = 102;
    pub const OFFHAND_AMMO_SMOKE: i32 = 103;
    pub const OFFHAND_NAME_FRAG: i32 = 104;
    pub const OFFHAND_NAME_SMOKE: i32 = 105;
    pub const LOW_HEALTH_OVERLAY: i32 = 109;
}

/// `pm_type`: 4 spectator, 5 intermission, 6 dead.
const PM_SPECTATOR: i32 = 4;
/// `pm_flags` 0x400000: the view follows another client.
const PMF_FOLLOW: i32 = 0x400000;
/// `pm_flags` 0x1000000 and 0x2000000: a spectator may cycle whom he
/// follows, and may stop following.
const PMF_FOLLOW_NEXT: i32 = 0x1000000;
const PMF_FOLLOW_STOP: i32 = 0x2000000;
const PM_DEAD: i32 = 6;
/// `pm_flags` 1 prone, 2 crouched; 0x10 an offhand in hand.
const PMF_PRONE: i32 = 0x1;
const PMF_CROUCH: i32 = 0x2;
const PMF_OFFHAND: i32 = 0x10;
/// `pm_flags` 0x40: the sight is up or coming up.
const PMF_SIGHT: i32 = 0x40;
/// `pm_flags` 0x100000: the crosshair is on a teammate (it draws green).
const PMF_FRIENDLY_CROSSHAIR: i32 = 0x10_0000;
/// `eFlags` 0x300: on a turret, which draws its own crosshair.
const EF_TURRET: i32 = 0x300;
/// `eFlags` 0x400000 on a player entity: talking (the compass flash).
const EF_TALKING: i32 = 0x40_0000;
/// `eFlags` 0x20000 on a player entity: dead.
const EF_DEAD: i32 = 0x2_0000;
/// `objective_t.state` 4, `"current"`: the compass points at it.
const OBJST_CURRENT: u32 = 4;
/// `objective_t.entNum` for none.
const ENTITYNUM_NONE: i32 = 0x3ff;
/// Configstring 11, the map's `northyaw` (`CG_NorthDirectionChanged`).
const CS_NORTH_YAW: usize = 11;
/// `cg_hudCompassSize`, `cg_hudCompassMaxRange`, `cg_hudCompassMinRange`,
/// `cg_hudCompassMinRadius`, `cg_hudObjectiveMaxRange`,
/// `cg_hudObjectiveMinAlpha` at their defaults.
const COMPASS_SIZE: f32 = 1.0;
const COMPASS_MAX_RANGE: f32 = 1500.0;
const COMPASS_MIN_RANGE: f32 = 0.0;
const COMPASS_MIN_RADIUS: f32 = 0.0;
const OBJECTIVE_MAX_RANGE: f32 = 2048.0;
const OBJECTIVE_MIN_ALPHA: f32 = 1.0;
/// The compass pointers' radius at full range, grid units
/// (`CG_ApplyCompassPointerRadiusScale`'s 43.75).
const COMPASS_RADIUS: f32 = 43.75;
/// `hud_fade_stance`: the stance icon goes 1.7 s after a stance change.
const STANCE_SHOW_MS: i32 = 1700;
/// The weapon name stays 1.8 s after a switch (`CG_OwnerDraw` 0x51).
const WEAPON_NAME_MS: i32 = 1800;
/// `CG_FadeColor`'s fade for every timed HUD piece.
const FADE_MS: i32 = 700;
/// Low ammo and an empty offhand draw in this (`CG_OwnerDraw`).
const LOW_AMMO_RGB: [f32; 3] = [0.89, 0.18, 0.01];
/// `cg_hudCompassSoundPingFadeTime`: a ping fades over 2 s past its end.
const PING_FADE_MS: f32 = 2000.0;
/// An MG42's fire ping lasts 50 ms past the event (`CG_EntityEvent` 175).
const MG42_PING_MS: i32 = 50;
/// `cg_hudProneY`: the prone-blocked line's height.
const PRONE_Y: f32 = -160.0;
/// `pm_flags` 0x10000: the cmd's stance change was refused.
const PMF_REFUSED: i32 = 0x1_0000;
/// `cg_hudDamageIconWidth`, `-Height`, `-Offset`, `-Time` defaults.
const DAMAGE_ICON_W: f32 = 128.0;
const DAMAGE_ICON_H: f32 = 64.0;
const DAMAGE_ICON_OFFSET: f32 = 128.0;
const DAMAGE_ICON_MS: i32 = 2000;
/// `hud_healthOverlay_*` defaults (`CG_PulseLowHealthOverlay`).
const PULSE_START: f32 = 0.35;
const PULSE_REGEN_PAUSE_MS: i32 = 5000;
const PULSE_MS: [i32; 3] = [150, 320, 400];
const PULSE_TWO_MUL: f32 = 0.7;
const PULSE_THREE_MUL: f32 = 0.6;
const PULSE_END_MS: i32 = 700;
/// `pulseMags`, one per pulse of the overlay.
const PULSE_MAGS: [f32; 4] = [1.0, 0.8, 0.6, 0.3];

/// One `hud.menu` owner draw item, its rect already offset by its menu's
/// and its alignment the menu's when it gives none.
#[derive(Clone, Debug, PartialEq)]
struct Item {
    ownerdraw: i32,
    rect: [f32; 4],
    align: [i32; 2],
    fore: [f32; 4],
    background: Option<String>,
    text_scale: f32,
}

/// The owner draw items of the visible menus `ui_mp/hud.menu` holds.
fn load_items(fs: &dyn AssetSource) -> Vec<Item> {
    let Ok(file) = MenuFile::find(fs, "ui_mp/hud.menu") else {
        log::warn!("cod2 hud: no ui_mp/hud.menu");
        return Vec::new();
    };
    file.menus
        .into_iter()
        .filter(|m| m.visible)
        .flat_map(|m| {
            let align = m.align;
            m.items.into_iter().map(move |i| (align, i))
        })
        .filter(|(_, i)| i.visible && i.ownerdraw != 0)
        .map(|(menu_align, i): ([i32; 2], UiItem)| Item {
            ownerdraw: i.ownerdraw,
            rect: i.rect,
            align: if i.align == [0, 0] {
                menu_align
            } else {
                i.align
            },
            fore: i.fore,
            background: i.background,
            text_scale: i.text_scale,
        })
        .collect()
}

/// What the HUD reads of one weapon: its file and the level's ammo and clip
/// slots (`BG_SetupAmmoIndexes`, Mac 0x7a4e4; `BG_SetupClipIndexes`).
struct Weap {
    def: WeaponDef,
    params: WeaponParams,
    ammo: usize,
    clip: usize,
}

impl Weap {
    fn s(&self, k: &str) -> &str {
        self.def.str(k)
    }
}

/// Configstring 7's weapons, rebuilt when it changes. Index 0 is none.
#[derive(Default)]
struct Weapons {
    cs7: String,
    list: Vec<Option<Arc<Weap>>>,
}

impl Weapons {
    fn sync(&mut self, cs: &ConfigStrings, fs: &dyn AssetSource) {
        let cs7 = cs.get(CS_WEAPONS);
        if self.cs7 == cs7 && !self.list.is_empty() {
            return;
        }
        let defs: Vec<Option<WeaponDef>> = (1..)
            .map_while(|i| cs.weapon_name(i))
            .map(|n| weapon::load(fs, n).ok().map(|p| p.def))
            .collect();
        let names = |k: &'static str| {
            defs.iter()
                .map(move |d| d.as_ref().map_or("", |d| d.str(k)))
        };
        let ammo = weapon::clip_indices(names("ammoName"));
        let clip = weapon::clip_indices(names("clipName"));
        self.list = std::iter::once(None)
            .chain(defs.into_iter().enumerate().map(|(i, d)| {
                d.map(|def| {
                    Arc::new(Weap {
                        params: WeaponParams::from_def(&def, ammo[i], clip[i], 0),
                        def,
                        ammo: ammo[i],
                        clip: clip[i],
                    })
                })
            }))
            .collect();
        self.cs7 = cs7.to_string();
    }

    fn get(&self, w: i32) -> Option<&Weap> {
        self.list.get(usize::try_from(w).ok()?)?.as_deref()
    }

    /// Weapon numbers and files, 1 up.
    fn all(&self) -> impl Iterator<Item = (i32, &Weap)> {
        self.list
            .iter()
            .enumerate()
            .filter_map(|(i, w)| Some((i as i32, w.as_deref()?)))
    }
}

/// `ps.weapons[]`'s bit for `w`.
fn holds(ps: &PlayerView, w: i32) -> bool {
    let (word, b) = (w / 32, w % 32);
    (0..2).contains(&word) && w > 0 && (ps.raw.i32(&format!("weapons[{word}]")) >> b) & 1 != 0
}

/// `BG_GetTotalAmmoReserve` (Mac 0x7a9d0) without shared ammo caps (no
/// stock weapon has one): a clip-only weapon's clip, else its ammo.
fn reserve(ps: &PlayerView, w: &Weap) -> i32 {
    if w.def.flag("clipOnly") {
        ps.raw.ammoclip[w.clip]
    } else {
        ps.raw.ammo[w.ammo]
    }
}

/// `CG_CheckPlayerForLowAmmo` (Mac 0x18b852): the reserve at a fifth of
/// `maxAmmo` or under.
fn low_ammo(ps: &PlayerView, w: &Weap) -> bool {
    let max = w.def.int("maxAmmo");
    max >= 0 && reserve(ps, w).min(999) as f32 <= max.min(999) as f32 * 0.2
}

/// `CG_CheckPlayerForLowClip` (Mac 0x18b932): the clip at a third of its
/// size or under.
fn low_clip(ps: &PlayerView, w: &Weap) -> bool {
    let size = w.def.int("clipSize");
    let clip = ps.raw.ammoclip[w.clip];
    !w.def.flag("clipOnly") && clip >= 0 && size >= 1 && clip.min(999) as f32 <= size as f32 * 0.33
}

/// `CG_FadeColor` (Mac 0x1a1b0e)'s alpha: 1 for `total - fade` ms past
/// `start`, then down to 0 at `total`; `None` once over or unset.
fn fade(start: i32, now: i32, total: i32, fade: i32) -> Option<f32> {
    let t = now - start;
    if start == 0 || t >= total {
        return None;
    }
    let left = total - t;
    Some(if left < fade {
        left as f32 / fade as f32
    } else {
        1.0
    })
}

fn angle_normalize_360(a: f32) -> f32 {
    a.rem_euclid(360.0)
}

/// `AngleNormalize180`.
fn angle_normalize_180(a: f32) -> f32 {
    let a = angle_normalize_360(a);
    if a > 180.0 { a - 360.0 } else { a }
}

/// `AngleSubtract`: `a - b` into -180..180.
fn angle_subtract(a: f32, b: f32) -> f32 {
    let mut d = a - b;
    while d > 180.0 {
        d -= 360.0;
    }
    while d < -180.0 {
        d += 360.0;
    }
    d
}

/// `vectoyaw`, degrees.
fn vectoyaw(x: f32, y: f32) -> f32 {
    if x == 0.0 && y == 0.0 {
        0.0
    } else {
        angle_normalize_360(y.atan2(x).to_degrees())
    }
}

/// `CG_PulseLowHealthOverlay` (Mac 0x18b2ae)'s state: the overlay pulses
/// four times after the health drops under `hud_healthOverlay_pulseStart`.
#[derive(Default)]
struct Pulse {
    last_frac: f32,
    hurt_time: i32,
    index: usize,
    active: bool,
    phase_start: i32,
    alpha_from: f32,
    alpha_to: f32,
    phase_ms: i32,
    phase: u8,
}

impl Pulse {
    /// `CG_ResetLowHealthOverlay` (Mac 0x18a650), from `CG_Respawn`: no
    /// pulse running, the end phase's alpha (`hud_healthOverlay_phaseEnd_
    /// toAlpha` 0) held, and the health read as full.
    fn reset(&mut self) {
        self.active = false;
        self.alpha_to = 0.0;
        self.phase_ms = 0;
        self.phase = 0;
        self.index = 0;
        self.last_frac = 1.0;
    }

    fn step(&mut self, frac: f32, now: i32) {
        if frac < self.last_frac && frac < PULSE_START {
            self.hurt_time = now;
            self.index = 0;
        }
        self.last_frac = frac;
        if self.phase_start + self.phase_ms > now {
            return;
        }
        if frac < PULSE_START {
            self.active = true;
        } else if !self.active {
            return;
        }
        self.phase_start = now;
        self.alpha_from = self.alpha_to;
        if self.index > 3 {
            self.active = false;
            self.alpha_to = 0.0;
            self.phase_ms = PULSE_END_MS;
            self.phase = 0;
            return;
        }
        let mag = PULSE_MAGS[self.index];
        match self.phase {
            0 => {
                self.alpha_to = mag.clamp(0.0, 1.0);
                self.phase_ms = PULSE_MS[0];
                self.phase = 1;
            }
            1 => {
                self.alpha_to = (mag * PULSE_TWO_MUL).clamp(0.0, 1.0);
                self.phase_ms = PULSE_MS[1];
                self.phase = 2;
            }
            _ => {
                self.alpha_to = (mag * PULSE_THREE_MUL).clamp(0.0, 1.0);
                self.phase_ms = PULSE_MS[2];
                self.phase = 0;
                let cycle: i32 = PULSE_MS.iter().sum();
                if self.hurt_time + PULSE_REGEN_PAUSE_MS - 3 * cycle <= now {
                    self.index += 1;
                }
            }
        }
    }

    /// The overlay's alpha at `now` (`CG_OwnerDraw` 0x6d).
    fn alpha(&self, now: i32) -> f32 {
        let t = (now - self.phase_start).max(0);
        if self.phase_ms < 1 || self.phase_ms <= t {
            self.alpha_to
        } else {
            self.alpha_from + (self.alpha_to - self.alpha_from) * t as f32 / self.phase_ms as f32
        }
    }
}

/// What an offhand owner draw paints.
#[derive(Clone, Copy)]
enum Offhand {
    Icon,
    Count,
    Name,
}

/// One `hit_direction` icon: its start, length and the damage's yaw.
#[derive(Clone, Copy, Default)]
struct DamageIcon {
    start: i32,
    ms: i32,
    yaw: f32,
}

/// `CG_UpdateCompassOrientation` (Mac 0x1a1bbc): the face's angle
/// (`cg + 0x2c5b0`), its speed (`0x2c5b4`) and the clock (`0x2c5a8`).
#[derive(Clone, Copy, Debug, Default)]
struct CompassSpring {
    last: i32,
    angle: f32,
    speed: f32,
}

impl CompassSpring {
    /// Springs toward `target` in 5 ms steps; a clock gone back or 500 ms
    /// unseen snaps to it.
    fn step(&mut self, target: f32, now: i32) -> f32 {
        let mut dt = now.wrapping_sub(self.last);
        if now < self.last || dt as f32 > 500.0 {
            *self = CompassSpring {
                last: now,
                angle: target,
                speed: 0.0,
            };
            return self.angle;
        }
        self.last = now;
        let mut d = angle_subtract(self.angle, target);
        loop {
            if dt < 1 {
                self.angle = angle_normalize_360(d + target);
                return self.angle;
            }
            let step = if dt < 6 {
                let s = dt as f32 * 0.001;
                dt = 0;
                s
            } else {
                dt -= 5;
                5.0 * 0.001
            };
            if d.abs() < 0.25 && self.speed.abs() < 1.0 {
                self.angle = target;
                self.speed = 0.0;
                return self.angle;
            }
            d = angle_normalize_180(step * self.speed + d);
            if d > 0.0 {
                self.speed -= step * 1000.0;
            } else if d < 0.0 {
                self.speed += step * 1000.0;
            }
            let mut v = self.speed - (self.speed + self.speed) * step;
            if v <= 0.0 {
                if d < 0.0 {
                    v -= v * 3.5 * step;
                }
                let w = step + v;
                v = if w > 0.0 { 0.0 } else { w };
            } else {
                if d > 0.0 {
                    v -= v * 3.5 * step;
                }
                v -= step;
                if v < 0.0 {
                    v = 0.0;
                }
            }
            self.speed = v.clamp(-30000.0, 30000.0);
        }
    }
}

/// A client's compass ping (`cg + 0x2c5d0 + n * 0x1c`): the end of his
/// last shot's ping, whether he is an enemy, and where he fired from.
#[derive(Clone, Copy, Debug, Default)]
struct Ping {
    until: i32,
    enemy: bool,
    pos: [f32; 2],
}

/// A command's bound keys as a hint prints them.
pub type KeyLookup = dyn Fn(&str) -> Option<String>;

/// What the caller knows that the playerstate does not.
pub struct HudInput<'a> {
    /// `cg_weaponSelect`, the weapon the keys asked for.
    pub selected: u8,
    /// `+activate`'s key, for the hints' `&&1`.
    pub use_key: &'a str,
    /// `cg_drawMantleHint`.
    pub mantle_hint: bool,
    /// `cg_hudStanceHintPrints` on: a command's bound keys as the hints
    /// print them, `None` when it has none. `None` while the dvar is 0.
    pub stance_keys: Option<&'a KeyLookup>,
    /// `cg_hintFadeTime`, ms: a cursor hint's life past its last frame.
    pub hint_fade_ms: i32,
}

/// `CG_DrawPlayerStance`'s hint commands (Mac 0x36de80, 0x36de20,
/// 0x36ddc0, `C_180`..`C_182`): by the stance held (standing, crouched,
/// prone), for the stand, crouch and prone lines, the commands tried in
/// order for one with a key.
const STANCE_HINT_COMMANDS: [[&[&str]; 3]; 3] = [
    [
        &[],
        &["gocrouch", "togglecrouch", "lowerstance", "+movedown"],
        &["goprone", "+prone"],
    ],
    [
        &["+gostand", "raisestance", "+moveup"],
        &[],
        &["goprone", "lowerstance", "toggleprone", "+prone"],
    ],
    [
        &["+gostand", "toggleprone"],
        &[
            "gocrouch",
            "togglecrouch",
            "raisestance",
            "+movedown",
            "+moveup",
        ],
        &[],
    ],
];
/// Every command a stance hint names, once.
pub fn stance_hint_commands() -> impl Iterator<Item = &'static str> {
    let mut all: Vec<&'static str> = STANCE_HINT_COMMANDS
        .iter()
        .flatten()
        .flat_map(|r| r.iter().copied())
        .collect();
    all.sort_unstable();
    all.dedup();
    all.into_iter()
}

/// Each line's text: `&&1` is the key.
const STANCE_HINT_KEYS: [&str; 3] = [
    "PLATFORM_STANCEHINT_STAND",
    "PLATFORM_STANCEHINT_CROUCH",
    "PLATFORM_STANCEHINT_PRONE",
];

#[derive(Default)]
pub struct PlayerHud {
    items: Option<Vec<Item>>,
    weapons: Weapons,
    /// `(pm_flags & 3, when it last changed)`.
    stance: Option<(i32, i32)>,
    /// `(weapon, when it was selected)`, the name's clock.
    weapon_shown: Option<(i32, i32)>,
    pulse: Pulse,
    last_damage_event: Option<i32>,
    damage: [DamageIcon; 8],
    rng: u32,
    /// `(clientNum, stats[5])` of the spawn the clocks were set for.
    spawn: Option<(i32, i32)>,
    /// `CG_DamageFeedback`'s kick and red flash.
    kick: DamageKick,
    /// The zoom's in-or-out latch, for the scope overlay.
    latch: SightLatch,
    compass: CompassSpring,
    /// The fire pings by client number.
    pings: Vec<Ping>,
    /// `cg + 0x2be30`: when the prone-blocked line goes.
    prone_blocked_until: i32,
    /// `pm_flags` 0x10000 last frame.
    refused: bool,
    /// `cg + 0x2be34`, `0x2be38`: the stance the hints last saw and when
    /// it changed under `cg_hudStanceHintPrints` (0 with it off).
    hint_stance: i32,
    stance_hint_at: i32,
    /// `cg + 0x2bde8..0x2bdf4`: the cursor hint and its string last seen,
    /// when, and `cg_hintFadeTime` then.
    cursor_hint: (i32, i32, i32, i32),
    /// `cg + 0x2c5e8 + n * 0x1c`: when each teammate's compass chat flash
    /// ends.
    chat_until: Vec<i32>,
}

/// One HUD frame's drawing context.
struct Ctx<'a> {
    fonts: &'a UiFonts,
    /// The `UI_GetFontHandle` handle text draws with: 0 `Default`, 4
    /// `BigFixed`.
    slot: Slot,
    sp: ScreenPlacement,
    /// Real pixels per grid unit, vertically.
    k: f32,
    screen_w: f32,
    screen_h: f32,
    loc: &'a Localized,
    now: i32,
}

impl Ctx<'_> {
    /// `UI_DrawHandlePic`: `rect` placed by `align`, `uv` its corners.
    fn pic(
        &self,
        rect: [f32; 4],
        align: [i32; 2],
        rgba: [f32; 4],
        m: &str,
        out: &mut Vec<HudQuad>,
    ) {
        self.rotated(rect, align, 0.0, rgba, m, out);
    }

    /// `CL_DrawStretchPicPhysical`: real pixels, texture corners `uv`
    /// (`s0, t0, s1, t1`).
    fn physical(&self, r: [f32; 4], uv: [f32; 4], rgba: [f32; 4], m: &str, out: &mut Vec<HudQuad>) {
        let [x, y, w, h] = r;
        let [s0, t0, s1, t1] = uv;
        out.push(HudQuad {
            verts: [[x, y], [x + w, y], [x + w, y + h], [x, y + h]],
            uvs: [[s0, t0], [s1, t0], [s1, t1], [s0, t1]],
            rgba,
            texture: m.to_string(),
        });
    }

    /// `CL_DrawStretchPic`: `r` placed by `align`, texture corners `uv`.
    fn stretch(
        &self,
        r: [f32; 4],
        align: [i32; 2],
        uv: [f32; 4],
        rgba: [f32; 4],
        m: &str,
        out: &mut Vec<HudQuad>,
    ) {
        let (x, w) = self.sp.axis(0, r[0], r[2], align[0]);
        let (y, h) = self.sp.axis(1, r[1], r[3], align[1]);
        self.physical([x, y, w, h], uv, rgba, m, out);
    }

    /// `CG_DrawRotatedPic` (Mac 0x1a242e): placed whole, then turned
    /// `angle` degrees about its centre in real pixels.
    fn rotated(
        &self,
        r: [f32; 4],
        align: [i32; 2],
        angle: f32,
        rgba: [f32; 4],
        m: &str,
        out: &mut Vec<HudQuad>,
    ) {
        if rgba[3] <= 0.0 {
            return;
        }
        let (x, w) = self.sp.axis(0, r[0], r[2], align[0]);
        let (y, h) = self.sp.axis(1, r[1], r[3], align[1]);
        let (s, c) = angle.to_radians().sin_cos();
        let (hw, hh) = (0.5 * w, 0.5 * h);
        let (cx, cy) = (x + hw, y + hh);
        let corner = |sx: f32, sy: f32| {
            [
                cx + c * hw * sx - s * hw * sy,
                cy + s * hh * sx + c * hh * sy,
            ]
        };
        out.push(HudQuad {
            verts: [
                corner(-1.0, -1.0),
                corner(1.0, -1.0),
                corner(1.0, 1.0),
                corner(-1.0, 1.0),
            ],
            uvs: [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]],
            rgba,
            texture: m.to_string(),
        });
    }

    fn font(&self, scale: f32) -> &font::Font {
        self.fonts.pick(self.slot, scale, self.screen_h)
    }

    /// `UI_TextWidth`, grid units.
    fn width(&self, s: &str, scale: f32) -> f32 {
        let f = self.font(scale);
        font::measure(f, s, scale / f.unit_scale())
    }

    /// `UI_TextHeight`, grid units.
    fn height(&self, scale: f32) -> f32 {
        let f = self.font(scale);
        f.max_height as f32 * f.glyph_scale * scale
    }

    /// `UI_DrawText`: `s` with its baseline at grid `(x, y)` under `align`.
    #[allow(clippy::too_many_arguments)]
    fn text(
        &self,
        s: &str,
        x: f32,
        y: f32,
        align: [i32; 2],
        scale: f32,
        rgba: [f32; 4],
        out: &mut Vec<HudQuad>,
    ) {
        if s.is_empty() || rgba[3] <= 0.0 {
            return;
        }
        let f = self.font(scale);
        let top = y - self.height(scale);
        let px = self.sp.axis(0, x, 0.0, align[0]).0;
        let py = self.sp.axis(1, top, 0.0, align[1]).0;
        font::layout(f, s, px, py, scale / f.unit_scale() * self.k, rgba, out);
    }
}

fn rgba(rgb: [f32; 4], a: f32) -> [f32; 4] {
    [rgb[0], rgb[1], rgb[2], a]
}

impl PlayerHud {
    /// The HUD for `frame` on a `w` x `h` window at cgame time `now`.
    #[allow(clippy::too_many_arguments)]
    pub fn build(
        &mut self,
        frame: &Frame,
        fs: &dyn AssetSource,
        loc: &Localized,
        fonts: &UiFonts,
        (w, h): (f32, f32),
        now: i32,
        input: &HudInput,
        out: &mut Vec<HudQuad>,
    ) {
        let ps = &frame.snap.ps;
        self.weapons.sync(frame.configstrings, fs);
        self.track(frame, now, input);
        if ps.pm_type >= PM_SPECTATOR && ps.pm_type != PM_DEAD {
            return;
        }
        let cx = Ctx {
            fonts,
            slot: Slot::Default,
            sp: ScreenPlacement::new(w, h),
            k: h / 480.0,
            screen_w: w,
            screen_h: h,
            loc,
            now,
        };
        if ps.pm_type < PM_DEAD {
            self.crosshair(frame, &cx, out);
        }
        let items = self.items.take().unwrap_or_else(|| load_items(fs));
        for item in &items {
            self.owner_draw(item, frame, &cx, input, out);
        }
        self.items = Some(items);
    }

    /// `CG_Draw2D`'s line for a free spectator (`pm_type` 4, Mac
    /// 0x1d114a): `CGAME_SPECTATOR` centred 27 over the bottom, font 4 at
    /// 0.5, before the hudelems.
    pub fn spectator(
        &self,
        frame: &Frame,
        fonts: &UiFonts,
        (w, h): (f32, f32),
        loc: &Localized,
        out: &mut Vec<HudQuad>,
    ) {
        if frame.snap.ps.pm_type != PM_SPECTATOR {
            return;
        }
        let cx = Ctx {
            fonts,
            slot: Slot::BigFixed,
            sp: ScreenPlacement::new(w, h),
            k: h / 480.0,
            screen_w: w,
            screen_h: h,
            loc,
            now: 0,
        };
        let s = loc.get("CGAME_SPECTATOR").unwrap_or("CGAME_SPECTATOR");
        let x = -cx.width(s, 0.5) * 0.5;
        cx.text(s, x, -27.0, [7, 3], 0.5, [1.0; 4], out);
    }

    /// `CG_DrawCrosshairNames`' line (Mac 0x1cddb4): `s` in font 0 at
    /// 1/3, aligned (0, 0), at `cg_drawCrosshairNamesPosX` and 16 under
    /// `cg_drawCrosshairNamesPosY`, without the style 3 shadow.
    #[allow(clippy::too_many_arguments)]
    pub fn crosshair_name(
        &self,
        s: &str,
        rgba: [f32; 4],
        (x, y): (i32, i32),
        fonts: &UiFonts,
        (w, h): (f32, f32),
        loc: &Localized,
        out: &mut Vec<HudQuad>,
    ) {
        let cx = Ctx {
            fonts,
            slot: Slot::Default,
            sp: ScreenPlacement::new(w, h),
            k: h / 480.0,
            screen_w: w,
            screen_h: h,
            loc,
            now: 0,
        };
        cx.text(s, x as f32, (y + 16) as f32, [0, 0], 1.0 / 3.0, rgba, out);
    }

    /// What `CG_Draw2D` draws once the board is not up:
    /// `CG_DrawSpectatorMessage` (Mac 0x1cfeba), the follow keys a
    /// spectator's `pm_flags` 0x1000000 (next and previous) and 0x2000000
    /// (stop) allow at x 240 from `436 - 2 * UI_TextHeight` down, font 0
    /// at 0.20833 (`cg_descriptiveText` 1), then `CG_DrawFollow` (Mac
    /// 0x1ced8a): with `pm_flags` 0x400000 outside the killcam, `following`
    /// and the followed name centred at y 40 and 65, font 0 at 1/3.
    /// `keys` are the bound keys of `+attack`, `toggleads` (else
    /// `+speed`) and `+melee` (else `+melee_breath`), empty when unbound.
    #[allow(clippy::too_many_arguments)]
    pub fn follow(
        &self,
        frame: &Frame,
        fonts: &UiFonts,
        (w, h): (f32, f32),
        loc: &Localized,
        keys: &[String; 3],
        killcam: bool,
        out: &mut Vec<HudQuad>,
    ) {
        let ps = &frame.snap.ps;
        let cx = Ctx {
            fonts,
            slot: Slot::Default,
            sp: ScreenPlacement::new(w, h),
            k: h / 480.0,
            screen_w: w,
            screen_h: h,
            loc,
            now: 0,
        };
        if ps.pm_flags & (PMF_FOLLOW_NEXT | PMF_FOLLOW_STOP) != 0 {
            const SCALE: f32 = 0.208_333_33;
            let line = cx.height(SCALE).trunc();
            let mut y = line * -2.0 + 436.0;
            let mut lines = Vec::new();
            if ps.pm_flags & PMF_FOLLOW_NEXT != 0 {
                lines.push(("PLATFORM_FOLLOWNEXTPLAYER", &keys[0]));
                lines.push(("PLATFORM_FOLLOWPREVIOUSPLAYER", &keys[1]));
            }
            if ps.pm_flags & PMF_FOLLOW_STOP != 0 {
                lines.push(("PLATFORM_FOLLOWSTOP", &keys[2]));
            }
            for (text, key) in lines {
                let key = if key.is_empty() {
                    loc.get("KEY_UNBOUND").unwrap_or("KEY_UNBOUND")
                } else {
                    key.as_str()
                };
                let s = loc.get(text).unwrap_or(text).replacen("&&1", key, 1);
                cx.text(&s, 240.0, y, [0, 0], SCALE, [1.0; 4], out);
                y += line;
            }
        }
        if ps.pm_flags & PMF_FOLLOW != 0 && !killcam {
            const SCALE: f32 = 1.0 / 3.0;
            let name = frame
                .snap
                .clients
                .iter()
                .find(|c| c.number as i32 == ps.client_num)
                .map_or_else(|| "?".to_string(), |c| c.name());
            let following = loc.get("CGAME_FOLLOWING").unwrap_or("CGAME_FOLLOWING");
            for (s, y) in [(following, 40.0), (name.as_str(), 65.0)] {
                let x = -cx.width(s, SCALE) * 0.5;
                cx.text(s, x, y, [7, 1], SCALE, [1.0; 4], out);
            }
        }
    }

    /// What `CG_Draw2D` paints after the HUD menus and the hudelems: the
    /// red damage flash, then the damage direction icons.
    pub fn build_late(
        &mut self,
        frame: &Frame,
        fonts: &UiFonts,
        (w, h): (f32, f32),
        now: i32,
        loc: &Localized,
        out: &mut Vec<HudQuad>,
    ) {
        let ps = &frame.snap.ps;
        if ps.pm_type >= PM_SPECTATOR && ps.pm_type != PM_DEAD {
            return;
        }
        let cx = Ctx {
            fonts,
            slot: Slot::Default,
            sp: ScreenPlacement::new(w, h),
            k: h / 480.0,
            screen_w: w,
            screen_h: h,
            loc,
            now,
        };
        if let Some(c) = self.kick.flash(now) {
            cx.physical(
                [-10.0, -10.0, w + 10.0, h + 10.0],
                [0.0, 0.0, 1.0, 1.0],
                c,
                "white",
                out,
            );
        }
        self.damage_icons(frame, &cx, out);
        if let Some(weap) = self.view_weapon(ps) {
            let def = SightDef::of(&weap.def);
            self.latch.step(def.ads, ps.raw.f32("fWeaponPosFrac"));
        }
    }

    /// `CG_DamageFeedback`'s view kick, for the drawn view.
    pub fn damage_kick(&self) -> DamageKick {
        self.kick
    }

    /// `CG_FireWeapon`'s compass ping (Mac 0x1d823c into
    /// `CG_CompassAddWeaponPingInfo`, 0x18f060): a player other than the
    /// local one, on any team but spectator, pings where he fired; an
    /// enemy (anyone in a free-for-all) keeps his spot. A turret's
    /// `EV_FIRE_WEAPON_MG42` pings its gunner, the event's parm, for 50 ms
    /// at the turret (`CG_EntityEvent` 175, Mac 0x1e4650): only when the
    /// gunner is a player in the snapshot and the event's entity is not
    /// the local client.
    pub fn on_events(&mut self, events: &[Event2], frame: &Frame, now: i32) {
        use vcod_cod2::net::events::{
            EV_FIRE_WEAPON, EV_FIRE_WEAPON_LASTSHOT, EV_FIRE_WEAPON_MG42,
        };
        let own = frame.snap.ps.client_num;
        for ev in events {
            let (n, until) = if ev.event == EV_FIRE_WEAPON_MG42 {
                let gunner = frame
                    .snap
                    .entities
                    .iter()
                    .find(|e| e.number as i32 == ev.parm);
                if gunner.is_none_or(|g| g.etype != 1) || ev.entity as i32 == own {
                    continue;
                }
                (ev.parm, now + MG42_PING_MS)
            } else if (EV_FIRE_WEAPON..=EV_FIRE_WEAPON_LASTSHOT).contains(&ev.event)
                && ev.etype == 1
                && !ev.from_ps
            {
                // INFERRED: the ping lasts as long as the fire sound
                // `CG_PlaySoundAlias` returns; vcod starts the fade at once.
                (ev.entity as i32, now)
            } else {
                continue;
            };
            if n == own || !(0..64).contains(&n) {
                continue;
            }
            let Some(team) = team_of(frame, n) else {
                continue;
            };
            if team == 3 {
                continue;
            }
            let ours = team_of(frame, own).unwrap_or(0);
            if self.pings.len() < 64 {
                self.pings.resize(64, Ping::default());
            }
            let p = &mut self.pings[n as usize];
            p.until = until;
            p.enemy = ours == 0 || ours != team;
            if p.enemy {
                p.pos = [ev.pos.x, ev.pos.y];
            }
        }
    }

    /// The weapon `BG_GetViewmodelWeaponIndex` names: the offhand while
    /// it is up.
    fn view_weapon(&self, ps: &PlayerView) -> Option<&Weap> {
        let w = if ps.pm_flags & PMF_OFFHAND != 0 {
            ps.raw.i32("offHandIndex")
        } else {
            ps.weapon
        };
        self.weapons.get(w)
    }

    /// `CG_DrawWeapReticle` (Mac 0x1d0320): the weapon's
    /// `adsOverlayShader` past the zoom fraction at its alpha, centred,
    /// whole when wider than 320 or taller than 240, else as four mirrored
    /// quarters; `CG_DrawFrameOverlay` (0x1cdbf2) fills the screen round it
    /// with the texture's edge.
    fn scope_overlay(&self, weap: &Weap, ps: &PlayerView, cx: &Ctx, out: &mut Vec<HudQuad>) {
        let def = SightDef::of(&weap.def);
        let Some(a) = def.overlay_alpha(ps.raw.f32("fWeaponPosFrac"), self.latch.zooming_in) else {
            return;
        };
        let m = weap.s("adsOverlayShader");
        if m.is_empty() {
            return;
        }
        let c = [1.0, 1.0, 1.0, a];
        let (w, h) = (
            weap.def.float("adsOverlayWidth"),
            weap.def.float("adsOverlayHeight"),
        );
        // `CG_CalcCrosshairPosition`: a scope's gun points down the view.
        let al = [2, 2];
        let rect = if w > 320.0 || h > 240.0 {
            let r = [w * -0.5, h * -0.5, w, h];
            cx.stretch(r, al, [0.0, 0.0, 1.0, 1.0], c, m, out);
            r
        } else {
            for (r, uv) in [
                ([-w, -h, w, h], [0.0, 0.0, 1.0, 1.0]),
                ([0.0, -h, w, h], [1.0, 0.0, 0.0, 1.0]),
                ([-w, 0.0, w, h], [0.0, 1.0, 1.0, 0.0]),
                ([0.0, 0.0, w, h], [1.0, 1.0, 0.0, 0.0]),
            ] {
                cx.stretch(r, al, uv, c, m, out);
            }
            [-w, -h, w + w, h + h]
        };
        let (x0, pw) = cx.sp.axis(0, rect[0], rect[2], 2);
        let (y0, ph) = cx.sp.axis(1, rect[1], rect[3], 2);
        let (x1, y1) = (x0 + pw, y0 + ph);
        let (sw, sh) = (cx.screen_w, cx.screen_h);
        if x0 > 0.0 {
            cx.physical([0.0, 0.0, x0, sh], [0.0, 0.0, 0.0, 1.0], c, m, out);
        }
        if x1 < sw {
            cx.physical([x1, 0.0, sw - x1, sh], [0.0, 0.0, 0.0, 1.0], c, m, out);
        }
        if y0 > 0.0 {
            cx.physical([x0, 0.0, x1 - x0, y0], [0.0, 0.0, 1.0, 0.0], c, m, out);
        }
        if y1 < sh {
            cx.physical([x0, y1, x1 - x0, sh - y1], [0.0, 0.0, 1.0, 0.0], c, m, out);
        }
    }

    /// `CG_Respawn` (Mac 0x1e3c56), run when `(clientNum, stats[5])`
    /// changes: the stance and weapon clocks restart, the eight damage
    /// icons clear and the low-health overlay resets.
    fn reset_for_spawn(&mut self, spawn: (i32, i32), stance: i32, now: i32) {
        if self.spawn.replace(spawn) == Some(spawn) {
            return;
        }
        self.stance = Some((stance, now));
        self.weapon_shown = None;
        self.damage = Default::default();
        self.kick = DamageKick {
            flash_until: self.kick.flash_until,
            ..DamageKick::default()
        };
        self.pulse.reset();
    }

    /// The clocks the HUD fades on: a stance change, a weapon switch, the
    /// health pulse and the damage icons.
    fn track(&mut self, frame: &Frame, now: i32, input: &HudInput) {
        let ps = &frame.snap.ps;
        self.reset_for_spawn((ps.client_num, ps.raw.stats[5]), ps.pm_flags & 3, now);
        let stance = ps.pm_flags & (PMF_PRONE | PMF_CROUCH);
        // INFERRED: a refused stance change came off a stance key, which
        // `CG_CheckTimedMenus` (Mac 0x1ce0ae) answers by showing the
        // stance menu; the playerstate carries the refusal, not the key.
        let refused = ps.pm_flags & PMF_REFUSED != 0;
        if self.stance.is_none_or(|(s, _)| s != stance) || (refused && !self.refused) {
            self.stance = Some((stance, now));
        }
        self.refused = refused;
        if refused && self.prone_blocked_until < now {
            self.prone_blocked_until = now + 1500;
        }
        // `CG_DrawPlayerStance` (Mac 0x18bbbc): the hints' clock.
        let held = ps.pm_flags & 3;
        if input.stance_keys.is_none() {
            self.stance_hint_at = 0;
        } else if self.hint_stance != held {
            self.stance_hint_at = now;
        }
        self.hint_stance = held;
        // `CG_DrawCompassFriendlies` (Mac 0x18f13e): a live teammate whose
        // `eFlags` carry 0x400000 (talking) starts a 3 s flash once the
        // last one is over.
        if let Some(team) = team_of(frame, ps.client_num).filter(|&t| t != 0 && t != 3) {
            self.chat_until.resize(64, 0);
            for e in &frame.snap.entities {
                let n = e.number as usize;
                if e.etype == 1
                    && e.eflags & EF_DEAD == 0
                    && n < 64
                    && team_of(frame, n as i32) == Some(team)
                    && e.eflags & EF_TALKING != 0
                    && self.chat_until[n] <= now
                {
                    self.chat_until[n] = now + 3000;
                }
            }
        }
        // `CG_DrawCursorhint` (Mac 0x18a7a6): a hint in the playerstate is
        // latched with the time, so it outlives the playerstate's by
        // `cg_hintFadeTime`, its last 100 ms fading.
        let hint = ps.raw.i32("cursorHint");
        if hint != 0 {
            let string = ps.raw.i32("cursorHintString");
            self.cursor_hint = (hint, string, now, input.hint_fade_ms);
        }
        let weapon = self.shown_weapon(ps, input);
        if self.weapon_shown.is_none_or(|(w, _)| w != weapon) {
            self.weapon_shown = Some((weapon, now));
        }
        let frac = health_frac(ps);
        if ps.raw.stats[0] != 0 && ps.raw.stats[2] != 0 && ps.pm_type != PM_DEAD && frac > 0.0 {
            self.pulse.step(frac, now);
        }
        // `CG_DamageFeedback` (Mac 0x1e3e10) on a new `damageEvent`.
        let event = ps.raw.i32("damageEvent");
        let last = self.last_damage_event.replace(event);
        if last.is_some_and(|l| l != event) && ps.raw.i32("damageCount") != 0 {
            let (yaw, pitch) = (ps.raw.i32("damageYaw"), ps.raw.i32("damagePitch"));
            let view = crate::cod2_viewmodel::view_angles(frame);
            let axis = vcod_common::pmove::aim::angles_to_axis_msvc(view).map(glam::Vec3::from);
            self.kick = DamageKick::feedback(
                yaw,
                pitch,
                ps.raw.i32("damageCount"),
                axis,
                frame.snap.server_time,
                now,
            );
            if (yaw, pitch) != (255, 255) {
                let slot = (0..8)
                    .min_by_key(|&i| self.damage[i].start)
                    .expect("eight slots");
                self.rng = self.rng.wrapping_mul(1_103_515_245).wrapping_add(12345);
                let r = (self.rng >> 16 & 0x7fff) as f32 / 32768.0;
                self.damage[slot] = DamageIcon {
                    start: frame.snap.server_time,
                    ms: DAMAGE_ICON_MS,
                    yaw: angle_normalize_360(yaw as f32 / 255.0 * 360.0 + (r - 0.5) * 20.0),
                };
            }
        }
    }

    /// The counter's clip and reserve for `frame` (-1 for a part not
    /// drawn), what the probe prints. `None` without a weapon.
    pub fn counter(
        &mut self,
        frame: &Frame,
        fs: &dyn AssetSource,
        selected: u8,
    ) -> Option<(i32, i32)> {
        self.weapons.sync(frame.configstrings, fs);
        let ps = &frame.snap.ps;
        let input = HudInput {
            selected,
            use_key: "",
            mantle_hint: true,
            stance_keys: None,
            hint_fade_ms: 100,
        };
        let weap = self.weapons.get(self.shown_weapon(ps, &input))?;
        let clip = if weap.def.flag("clipOnly") {
            -1
        } else {
            ps.raw.ammoclip[weap.clip]
        };
        Some((clip, reserve(ps, weap)))
    }

    /// The weapon the counter and name show: the selection while the
    /// playerstate holds it, else the playerstate's weapon.
    fn shown_weapon(&self, ps: &PlayerView, input: &HudInput) -> i32 {
        let sel = i32::from(input.selected);
        if sel != 0 && holds(ps, sel) {
            sel
        } else {
            ps.weapon
        }
    }

    fn owner_draw(
        &mut self,
        item: &Item,
        frame: &Frame,
        cx: &Ctx,
        input: &HudInput,
        out: &mut Vec<HudQuad>,
    ) {
        let ps = &frame.snap.ps;
        let [x, y, w, h] = item.rect;
        let al = item.align;
        let bg = item.background.as_deref().unwrap_or("white");
        let now = cx.now;
        match item.ownerdraw {
            od::AMMO_VALUE | od::AMMOCLIP_VALUE => self.ammo_value(item, ps, cx, input, out),
            od::AMMO_BACKDROP => {
                let Some(weap) = self.weapons.get(self.shown_weapon(ps, input)) else {
                    return;
                };
                if ps.weapon == 0 {
                    return;
                }
                let c = if low_ammo(ps, weap) {
                    [LOW_AMMO_RGB[0], LOW_AMMO_RGB[1], LOW_AMMO_RGB[2], 1.0]
                } else {
                    item.fore
                };
                cx.pic(item.rect, al, c, bg, out);
            }
            od::STANCE => {
                let Some((stance, at)) = self.stance else {
                    return;
                };
                let Some(a) = fade(at, now, STANCE_SHOW_MS, FADE_MS) else {
                    return;
                };
                let m = if stance & PMF_PRONE != 0 {
                    "stance_prone"
                } else if stance & PMF_CROUCH != 0 {
                    "stance_crouch"
                } else {
                    "stance_stand"
                };
                cx.pic(item.rect, al, rgba(item.fore, a), m, out);
                // With the hint prints on, `stance_flash` in
                // `cg_hudStanceFlash` (white) over the icon for 1 s after a
                // change, from 0.8.
                if input.stance_keys.is_some() && now < self.stance_hint_at + 1000 {
                    let f = (self.stance_hint_at - now + 1000) as f32 / 1000.0 * 0.8;
                    cx.pic(
                        item.rect,
                        al,
                        [1.0, 1.0, 1.0, f.min(a)],
                        "stance_flash",
                        out,
                    );
                }
                // `CG_DrawPlayerStance`: `CGAME_PRONE_BLOCKED` centred at
                // `cg_hudProneY`, pulsing `|sin|` one and a half times over
                // its 1.5 s.
                if now < self.prone_blocked_until {
                    let s = cx
                        .loc
                        .get("CGAME_PRONE_BLOCKED")
                        .unwrap_or("CGAME_PRONE_BLOCKED");
                    let tw = cx.width(s, item.text_scale);
                    let left = (self.prone_blocked_until - now) as f32;
                    let a = (left / 1500.0 * 540.0).to_radians().sin().abs();
                    let c = rgba(item.fore, a);
                    cx.text(s, tw * -0.5, PRONE_Y, [7, 3], item.text_scale, c, out);
                }
                self.stance_hints(item, a, cx, input, out);
            }
            od::CURSORHINT => self.cursor_hint(item, frame, cx, input, out),
            od::MANTLE_HINT => {
                // `CG_DrawMantleHint` (Mac 0x18ae44): under
                // `cg_drawMantleHint`, while `mantleState.flags & 8`.
                if !input.mantle_hint || ps.raw.i32("mantleState.flags") & 8 == 0 {
                    return;
                }
                let s = cx.loc.get("PLATFORM_MANTLE").unwrap_or("PLATFORM_MANTLE");
                let s = s.replace("&&1", input.use_key);
                let tw = cx.width(&s, item.text_scale);
                let th = cx.height(item.text_scale);
                let tx = (tw + w) * -0.5 + x;
                let c = [1.0; 4];
                cx.text(&s, tx, th * 0.5 + y, al, item.text_scale, c, out);
                cx.pic([tx + tw, h * -0.5 + y, w, h], al, c, "hint_mantle", out);
            }
            od::WEAPON_NAME => {
                let Some((weapon, at)) = self.weapon_shown else {
                    return;
                };
                let Some(a) = fade(at, now, WEAPON_NAME_MS, FADE_MS) else {
                    return;
                };
                let Some(weap) = self.weapons.get(weapon) else {
                    return;
                };
                let name = |k: &str| {
                    let key = weap.s(k);
                    cx.loc.get(key).unwrap_or(key).to_string()
                };
                let s = if weap.s("modeName").is_empty() {
                    name("displayName")
                } else {
                    format!("{} / {}", name("displayName"), name("modeName"))
                };
                let tx = x + w - cx.width(&s, item.text_scale) - 28.0;
                cx.text(&s, tx, y, al, item.text_scale, rgba(item.fore, a), out);
            }
            od::WEAPON_MODE_ICON => {
                let Some(weap) = self.weapons.get(self.shown_weapon(ps, input)) else {
                    return;
                };
                let m = weap.s("modeIcon");
                if !m.is_empty() {
                    cx.pic(item.rect, al, item.fore, m, out);
                }
            }
            od::COMPASS_BACK => {
                let r = [
                    x,
                    y - h * (COMPASS_SIZE - 1.0),
                    w * COMPASS_SIZE,
                    h * COMPASS_SIZE,
                ];
                cx.pic(r, al, item.fore, bg, out);
            }
            od::COMPASS => {
                let north = frame
                    .configstrings
                    .get(CS_NORTH_YAW)
                    .trim()
                    .parse::<f32>()
                    .unwrap_or(0.0);
                let yaw = crate::cod2_viewmodel::view_angles(frame)[1];
                let r = [
                    x,
                    y - h * (COMPASS_SIZE - 1.0),
                    w * COMPASS_SIZE,
                    h * COMPASS_SIZE,
                ];
                let angle = self.compass.step(angle_normalize_360(yaw - north), now);
                cx.rotated(r, al, angle, item.fore, bg, out);
            }
            od::COMPASS_POINTERS => self.compass_pointers(item, frame, cx, out),
            od::COMPASS_FRIENDS => self.compass_friends(item, frame, cx, out),
            // `cg_drawHealth` is 0 by default: CoD2 MP shows no health bar.
            od::BAR_HEALTH | od::BAR_HEALTH_BACK => {}
            od::LOW_HEALTH_OVERLAY => {
                let frac = health_frac(ps);
                if ps.raw.stats[0] == 0 || ps.raw.stats[2] == 0 || ps.pm_type == PM_DEAD {
                    return;
                }
                if frac <= 0.0 {
                    return;
                }
                let a = self.pulse.alpha(now);
                cx.pic(item.rect, al, rgba(item.fore, a), bg, out);
            }
            od::OFFHAND_ICON_FRAG => self.offhand(item, ps, cx, 1, Offhand::Icon, out),
            od::OFFHAND_ICON_SMOKE => self.offhand(item, ps, cx, 2, Offhand::Icon, out),
            od::OFFHAND_AMMO_FRAG => self.offhand(item, ps, cx, 1, Offhand::Count, out),
            od::OFFHAND_AMMO_SMOKE => self.offhand(item, ps, cx, 2, Offhand::Count, out),
            od::OFFHAND_NAME_FRAG => self.offhand(item, ps, cx, 1, Offhand::Name, out),
            od::OFFHAND_NAME_SMOKE => self.offhand(item, ps, cx, 2, Offhand::Name, out),
            _ => {}
        }
    }

    /// `CG_DrawPlayerAmmoValue` (Mac 0x18c440): the clip at the rect's left,
    /// the reserve right-aligned, a `|` between; a clip-only weapon (a
    /// grenade) shows its count centred. A low clip pulses red over the
    /// clip, a low reserve draws red.
    fn ammo_value(
        &self,
        item: &Item,
        ps: &PlayerView,
        cx: &Ctx,
        input: &HudInput,
        out: &mut Vec<HudQuad>,
    ) {
        if ps.weapon == 0 {
            return;
        }
        let Some(weap) = self.weapons.get(self.shown_weapon(ps, input)) else {
            return;
        };
        let [x, y, w, _] = item.rect;
        let (al, sc) = (item.align, item.text_scale);
        let clip = (!weap.def.flag("clipOnly"))
            .then(|| ps.raw.ammoclip[weap.clip])
            .filter(|&c| c >= 0)
            .map(|c| format!("{:2}", c.min(999)));
        let rsv = Some(reserve(ps, weap))
            .filter(|&r| r >= 0)
            .map(|r| format!("{:3}", r.min(999)));
        let lowclip = clip.is_some() && low_clip(ps, weap);
        let rsv_c = if rsv.is_some() && low_ammo(ps, weap) {
            rgba(
                [LOW_AMMO_RGB[0], LOW_AMMO_RGB[1], LOW_AMMO_RGB[2], 0.0],
                item.fore[3],
            )
        } else {
            item.fore
        };
        // The red pulse: 1 to 0 over each 800 ms.
        let pulse = rgba(
            [LOW_AMMO_RGB[0], LOW_AMMO_RGB[1], LOW_AMMO_RGB[2], 0.0],
            (800 - cx.now.rem_euclid(800)) as f32 / 800.0 * item.fore[3],
        );
        match (clip, rsv) {
            (Some(c), Some(r)) => {
                cx.text(&c, x, y, al, sc, item.fore, out);
                if lowclip {
                    cx.text(&c, x, y, al, sc, pulse, out);
                }
                cx.text(&r, x + w - cx.width(&r, sc), y, al, sc, rsv_c, out);
                let bar = (w - cx.width("|", sc)) * 0.5 + x - 5.0;
                cx.text("|", bar, y, al, sc, rsv_c, out);
            }
            (Some(c), None) => {
                let tx = (w - cx.width(&c, sc)) * 0.5 + x;
                cx.text(&c, tx, y, al, sc, item.fore, out);
                if lowclip {
                    cx.text(&c, tx, y, al, sc, pulse, out);
                }
            }
            (None, Some(r)) => {
                let tx = (w - cx.width(&r, sc)) * 0.5 + x;
                cx.text(&r, tx, y, al, sc, rsv_c, out);
            }
            (None, None) => {}
        }
    }

    /// `CG_DrawOffHandIcon`, `-Ammo` and `-Name` (Mac 0x1b4f62, 0x1b5332,
    /// 0x1b4c6c) for offhand `class` (1 frag, 2 smoke). Nothing once dead
    /// or for a class not held.
    fn offhand(
        &self,
        item: &Item,
        ps: &PlayerView,
        cx: &Ctx,
        class: i32,
        what: Offhand,
        out: &mut Vec<HudQuad>,
    ) {
        if ps.pm_type >= PM_DEAD {
            return;
        }
        let held = || {
            self.weapons
                .all()
                .filter(move |(i, w)| w.def.int("offhandClass") == class && holds(ps, *i))
        };
        let available = held().find(|(_, w)| ps.raw.ammo[w.ammo] + ps.raw.ammoclip[w.clip] != 0);
        let Some(first) = available.or_else(|| held().next()) else {
            return;
        };
        let (x, y, al, sc) = (item.rect[0], item.rect[1], item.align, item.text_scale);
        match what {
            Offhand::Icon => {
                let equipped = ps.raw.i32("offHandIndex");
                let w = self
                    .weapons
                    .get(equipped)
                    .filter(|w| w.def.int("offhandClass") == class)
                    .unwrap_or(first.1);
                let m = w.s("hudIcon");
                if !m.is_empty() {
                    cx.pic(item.rect, al, item.fore, m, out);
                }
            }
            Offhand::Count => {
                let n: i32 = held().map(|(_, w)| ps.raw.ammoclip[w.clip]).sum();
                let c = if n == 0 {
                    rgba(
                        [LOW_AMMO_RGB[0], LOW_AMMO_RGB[1], LOW_AMMO_RGB[2], 0.0],
                        item.fore[3],
                    )
                } else {
                    item.fore
                };
                cx.text(&n.to_string(), x, y, al, sc, c, out);
            }
            Offhand::Name => {
                let key = ["", "WEAPON_FRAGGRENADE", "WEAPON_SMOKEGRENADE"][class as usize];
                let s = cx.loc.get(key).unwrap_or(key);
                cx.text(s, x, y, al, sc, item.fore, out);
            }
        }
    }

    /// `CG_DrawCursorhint` (Mac 0x18a7a6) at `cg_cursorHints` 4: a use
    /// trigger's string (`&&1` its key) left of `hint_usable`, the health
    /// and pickup hints, or a weapon on the ground's icon.
    /// `CG_DrawPlayerStance`'s hint prints under `cg_hudStanceHintPrints`
    /// (Mac 0x18bbbc): for 3 s after a stance change, beside the icon, a
    /// line for each other stance with a bound command
    /// ([`STANCE_HINT_COMMANDS`]), its first bound command's keys in
    /// `PLATFORM_STANCEHINT_*`; whole for 2 s, then fading, never above
    /// the icon's `fade`. The lines are centred on the icon, 1.5 apart.
    fn stance_hints(
        &self,
        item: &Item,
        fade: f32,
        cx: &Ctx,
        input: &HudInput,
        out: &mut Vec<HudQuad>,
    ) {
        let Some(keys) = input.stance_keys else {
            return;
        };
        let (at, now) = (self.stance_hint_at, cx.now);
        if now >= at + 3000 {
            return;
        }
        let row = if self.hint_stance & PMF_PRONE != 0 {
            2
        } else if self.hint_stance & PMF_CROUCH != 0 {
            1
        } else {
            0
        };
        let lines: Vec<String> = (0..3)
            .filter_map(|i| {
                let key = STANCE_HINT_COMMANDS[row][i].iter().find_map(|c| keys(c))?;
                let s = cx
                    .loc
                    .get(STANCE_HINT_KEYS[i])
                    .unwrap_or(STANCE_HINT_KEYS[i]);
                Some(s.replace("&&1", &key))
            })
            .collect();
        let a = if now < at + 2000 {
            1.0
        } else {
            (at - now + 3000) as f32 / 1000.0
        };
        let c = rgba(item.fore, a.min(fade));
        let [x, y, w, h] = item.rect;
        let th = cx.height(item.text_scale);
        let mut ty = h * 0.5 + y - 1.5;
        match lines.len() {
            1 => ty += 0.5 * th,
            3 => ty -= 0.5 * th + 1.5,
            _ => {}
        }
        for line in &lines {
            cx.text(line, x + w, ty, item.align, item.text_scale, c, out);
            ty += th + 1.5;
        }
    }

    fn cursor_hint(
        &self,
        item: &Item,
        frame: &Frame,
        cx: &Ctx,
        input: &HudInput,
        out: &mut Vec<HudQuad>,
    ) {
        let ps = &frame.snap.ps;
        let (hint, string, start, total) = self.cursor_hint;
        let Some(alpha) = fade(start, cx.now, total, 100) else {
            return;
        };
        let [x, y, w, h] = item.rect;
        let (al, sc) = (item.align, item.text_scale);
        let c = rgba(item.fore, item.fore[3] * alpha);
        let use_string = || {
            let s = frame
                .configstrings
                .get(0x4fe + usize::try_from(string).ok()?);
            (!s.is_empty()).then(|| cx.loc.message(s).replace("&&1", input.use_key))
        };
        let (icon, wide, text) = match hint {
            2 => ("gfx/icons/hint_usable".to_string(), false, use_string()),
            3 if string < 0 => {
                let s = cx
                    .loc
                    .get("PLATFORM_PICKUPHEALTH")
                    .unwrap_or("PLATFORM_PICKUPHEALTH");
                (
                    "hint_health".to_string(),
                    false,
                    Some(s.replace("&&1", input.use_key)),
                )
            }
            3 => ("hint_health".to_string(), false, use_string()),
            4 => ("hint_friendly".to_string(), false, use_string()),
            5..=132 => {
                let Some(weap) = self.weapons.get(hint - 4) else {
                    return;
                };
                let text = if weap.def.weap_class() == 7 {
                    use_string()
                } else {
                    let key = if holds(ps, hint - 4) {
                        None
                    } else if (1..=2).any(|s| slot_weapon(ps, s) == 0) {
                        Some("PLATFORM_PICKUPNEWWEAPON")
                    } else {
                        Some("PLATFORM_SWAPWEAPONS")
                    };
                    key.map(|k| cx.loc.get(k).unwrap_or(k).replace("&&1", input.use_key))
                };
                (
                    weap.s("hudIcon").to_string(),
                    weap.def.flag("wideListIcon"),
                    text,
                )
            }
            _ => return,
        };
        if icon.is_empty() {
            return;
        }
        let iw = if wide { 2.0 * w } else { w };
        match text.filter(|t| !t.is_empty()) {
            Some(t) => {
                let tw = cx.width(&t, sc);
                let tx = (iw + tw) * -0.5 + x;
                cx.text(&t, tx, cx.height(sc) * 0.5 + y, al, sc, c, out);
                cx.pic([tx + tw, -0.5 * h + y, iw, h], al, c, &icon, out);
            }
            None => {
                let shift = if wide { -0.5 * w } else { 0.0 };
                cx.pic([(w + shift) * -0.5 + x, y, iw, h], al, c, &icon, out);
            }
        }
    }

    /// `CG_OwnerDraw` 0x56: an icon toward each `"current"` objective, out
    /// to the compass's rim at `cg_hudCompassMaxRange`.
    fn compass_pointers(&self, item: &Item, frame: &Frame, cx: &Ctx, out: &mut Vec<HudQuad>) {
        let ps = &frame.snap.ps;
        let [x, y, w, h] = item.rect;
        let centre = (
            x + 0.5 * COMPASS_SIZE * w,
            y + 0.5 * COMPASS_SIZE * h - h * (COMPASS_SIZE - 1.0),
        );
        let eye = frame.eye();
        let yaw = crate::cod2_viewmodel::view_angles(frame)[1];
        for o in &ps.raw.objectives {
            if o.state != OBJST_CURRENT {
                continue;
            }
            let ent = o.f[4] as i32;
            let pos = if ent == ENTITYNUM_NONE {
                [f32::from_bits(o.f[0]), f32::from_bits(o.f[1])]
            } else {
                match frame.snap.entity(ent as u32) {
                    Some(e) => {
                        let p = frame.entity_origin(e);
                        [p.x, p.y]
                    }
                    None => continue,
                }
            };
            let (dx, dy) = (pos[0] - eye[0], pos[1] - eye[1]);
            let dist = (dx * dx + dy * dy).sqrt();
            let alpha = if dist <= COMPASS_MAX_RANGE {
                1.0
            } else if dist < OBJECTIVE_MAX_RANGE {
                let f = (dist - COMPASS_MAX_RANGE) / (OBJECTIVE_MAX_RANGE - COMPASS_MAX_RANGE);
                1.0 - (1.0 - OBJECTIVE_MIN_ALPHA) * f
            } else {
                OBJECTIVE_MIN_ALPHA
            };
            let r = COMPASS_SIZE * COMPASS_RADIUS * radius_frac(dist);
            let a = angle_normalize_360(vectoyaw(dx, dy) - yaw).to_radians();
            let size = 16.0 * COMPASS_SIZE;
            let icon = o.f[3] as i32;
            let m = (1..=127)
                .contains(&icon)
                .then(|| frame.configstrings.material(icon))
                .flatten()
                .filter(|m| !m.is_empty() && m.len() < 64)
                .unwrap_or("objective");
            let rect = [
                centre.0 - 0.5 * size - a.sin() * r,
                centre.1 - 0.5 * size - a.cos() * r,
                size,
                size,
            ];
            cx.pic(
                rect,
                item.align,
                rgba(item.fore, alpha.min(item.fore[3])),
                m,
                out,
            );
        }
    }

    /// `CG_DrawCompassFriendlies` (Mac 0x18f13e): each live teammate in the
    /// snapshot as `objective_friendly`, turned to his facing, with
    /// `compassping_friendlyfiring` over him while his ping lasts (alone at
    /// full alpha); then every enemy's ping as an unturned
    /// `compassping_enemyfiring` where he fired. No teammates in a
    /// free-for-all, nothing for a spectator.
    fn compass_friends(&self, item: &Item, frame: &Frame, cx: &Ctx, out: &mut Vec<HudQuad>) {
        let ps = &frame.snap.ps;
        let Some(team) = team_of(frame, ps.client_num) else {
            return;
        };
        if team == 3 {
            return;
        }
        let [x, y, w, h] = item.rect;
        let centre = (x + 0.5 * COMPASS_SIZE * w, y + h - 0.5 * COMPASS_SIZE * h);
        let eye = frame.eye();
        let yaw = crate::cod2_viewmodel::view_angles(frame)[1];
        let now = cx.now;
        let size = 10.0 * COMPASS_SIZE;
        let place = |dx: f32, dy: f32, dist: f32| {
            let r = COMPASS_SIZE * COMPASS_RADIUS * radius_frac(dist);
            let a = angle_normalize_360(vectoyaw(dx, dy) - yaw).to_radians();
            [
                centre.0 - 0.5 * size - a.sin() * r,
                centre.1 - 0.5 * size - a.cos() * r,
                size,
                size,
            ]
        };
        let ping_alpha = |p: &Ping| {
            if p.until == 0 || now as f32 > p.until as f32 + PING_FADE_MS {
                None
            } else if p.until < now {
                Some((now - p.until) as f32 / -PING_FADE_MS + 1.0)
            } else {
                Some(1.0)
            }
        };
        for e in &frame.snap.entities {
            if team == 0 {
                break;
            }
            if e.etype != 1 || e.eflags & EF_DEAD != 0 || e.number as i32 == ps.client_num {
                continue;
            }
            if team_of(frame, e.number as i32) != Some(team) {
                continue;
            }
            let (p, ang) = frame.entity_place(e);
            let (dx, dy) = (p.x - eye[0], p.y - eye[1]);
            let rect = place(dx, dy, (dx * dx + dy * dy).sqrt());
            let turn = angle_normalize_360(yaw - ang.y);
            // The chat flash blinks: drawn unturned as
            // `objective_friendly_chat` while the time left mod 500 is 250 or more,
            // in place of his icon and ping.
            let chat = self.chat_until.get(e.number as usize).copied().unwrap_or(0);
            if now < chat && (chat - now) % 500 >= 250 {
                cx.pic(rect, item.align, item.fore, "objective_friendly_chat", out);
                continue;
            }
            let ping = self.pings.get(e.number as usize).and_then(ping_alpha);
            if ping != Some(1.0) {
                cx.rotated(rect, item.align, turn, item.fore, "objective_friendly", out);
            }
            if let Some(a) = ping {
                let c = rgba(item.fore, a.min(item.fore[3]));
                cx.rotated(rect, item.align, turn, c, "compassping_friendlyfiring", out);
            }
        }
        for p in self.pings.iter().filter(|p| p.enemy) {
            let Some(a) = ping_alpha(p) else {
                continue;
            };
            let (dx, dy) = (p.pos[0] - eye[0], p.pos[1] - eye[1]);
            let dist = (dx * dx + dy * dy).sqrt().min(COMPASS_MAX_RANGE);
            let rect = place(dx, dy, dist);
            let c = rgba(item.fore, a.min(item.fore[3]));
            cx.pic(rect, item.align, c, "compassping_enemyfiring", out);
        }
    }

    /// `CG_DrawCrosshair` (Mac 0x1d0784): `reticleCenter` at the centre and
    /// four `reticleSide` pieces out at the weapon's spread, shrinking and
    /// fading as the sight comes up; none while raising, dropping or
    /// reloading.
    fn crosshair(&self, frame: &Frame, cx: &Ctx, out: &mut Vec<HudQuad>) {
        let ps = &frame.snap.ps;
        if ps.eflags & EF_TURRET != 0 {
            self.turret_crosshair(frame, cx, out);
            return;
        }
        let weapon = if ps.pm_flags & PMF_OFFHAND != 0 {
            ps.raw.i32("offHandIndex")
        } else {
            ps.weapon
        };
        let Some(weap) = self.weapons.get(weapon) else {
            return;
        };
        if weapon != 0 {
            self.scope_overlay(weap, ps, cx, out);
        }
        let state = ps.raw.i32("weaponstate");
        if matches!(state, 1 | 2 | 5 | 10 | 11) {
            return;
        }
        let pos = ps.raw.f32("fWeaponPosFrac");
        // INFERRED: the decompile reads the sight fraction itself here; the
        // hip crosshair shows unsighted, so it is `1 - frac`.
        let base = 1.0 - pos;
        if base < 0.01 {
            return;
        }
        let green = ps.pm_flags & PMF_FRIENDLY_CROSSHAIR != 0;
        let rb = if green { 0.25 } else { 1.0 };
        let fov_y = crate::camera::fov_y(crate::camera::DEFAULT_FOV_DEG, 4.0 / 3.0);
        let (mut scale, mut y_off) = (1.0, 0.0);
        if pos != 0.0 {
            // `adsCrosshairInFrac` while the sight comes up, the out one
            // while it goes down.
            let key = if ps.pm_flags & PMF_SIGHT != 0 {
                "adsCrosshairInFrac"
            } else {
                "adsCrosshairOutFrac"
            };
            let in_frac = weap.def.float(key);
            let f = pos - (1.0 - in_frac);
            if f > 0.0 && in_frac > 0.0 && f / in_frac > 0.0 {
                let f = f / in_frac;
                scale = 1.0 - 0.5 * f;
                y_off = 480.0 / fov_y * f * weap.def.float("adsAimPitch");
            }
        }
        let center = weap.s("reticleCenter");
        if !center.is_empty() {
            let s = weap.def.int("reticleCenterSize") as f32 * scale;
            let c = [rb, 1.0, rb, base];
            cx.pic([-0.5 * s, y_off - 0.5 * s, s, s], [2, 2], c, center, out);
        }
        let side = weap.s("reticleSide");
        if side.is_empty() {
            return;
        }
        let spread_scale = ps.raw.f32("aimSpreadScale") / 255.0;
        let alpha = (base * (1.0 - spread_scale)).max(0.0);
        let s = weap.def.int("reticleSideSize") as f32 * scale;
        let (min, max) = spread_for_weapon(ps.view_height, &weap.params);
        let spread = (min + (max - min) * spread_scale) * scale;
        let mut d = spread.to_radians().tan() * 240.0 / (0.5 * fov_y).to_radians().tan();
        d = d.max(weap.def.int("reticleMinOfs") as f32);
        d -= s * weap.def.float("hipReticleSidePos");
        let e = s * 0.125;
        let c = [rb, 1.0, rb, alpha];
        let y = y_off;
        for (r, angle) in [
            ([-0.5 * s, y - s - d - e, s, s], 0.0),
            ([d, y - 0.5 * s, s, s], 90.0),
            ([-0.5 * s - e, y + d, s, s], 180.0),
            ([-s - d - e, y - 0.5 * s - e, s, s], 270.0),
        ] {
            cx.rotated(r, [2, 2], angle, c, side, out);
        }
    }

    /// `CG_DrawTurretCrossHair` (Mac 0x1cf59c), `CG_DrawCrosshair`'s arm
    /// for a gunner with a `viewlocked_entNum`: the gun's weapon's
    /// `reticleCenter`, `reticleCenterSize` square at the centre, green
    /// on a friendly, at `cg_crosshairAlpha` (1). `cg_drawTurretCrosshair`
    /// is 1. VERIFIED (cod2-turrets.md 12).
    fn turret_crosshair(&self, frame: &Frame, cx: &Ctx, out: &mut Vec<HudQuad>) {
        let ps = &frame.snap.ps;
        let gun = ps.raw.i32("viewlocked_entNum");
        if gun == ENTITYNUM_NONE {
            return;
        }
        let Some(weap) = frame
            .snap
            .entity(gun as u32)
            .and_then(|e| self.weapons.get(e.weapon))
        else {
            return;
        };
        let center = weap.s("reticleCenter");
        if center.is_empty() {
            return;
        }
        let rb = if ps.pm_flags & PMF_FRIENDLY_CROSSHAIR != 0 {
            0.25
        } else {
            1.0
        };
        let s = weap.def.int("reticleCenterSize") as f32;
        cx.pic(
            [-0.5 * s, -0.5 * s, s, s],
            [2, 2],
            [rb, 1.0, rb, 1.0],
            center,
            out,
        );
    }

    /// `CG_Draw2D`'s `hit_direction` quads: below the centre turned toward
    /// the damage, whole for half their time, then fading.
    fn damage_icons(&self, frame: &Frame, cx: &Ctx, out: &mut Vec<HudQuad>) {
        // `cg_hudDamageIconInScope` 0: hidden while a scope overlay shows.
        let ps = &frame.snap.ps;
        if let Some(weap) = self.view_weapon(ps)
            && SightDef::of(&weap.def)
                .overlay_alpha(ps.raw.f32("fWeaponPosFrac"), self.latch.zooming_in)
                .is_some()
        {
            return;
        }
        let (cx0, cy0) = (
            cx.sp.axis(0, 320.0, 0.0, 0).0,
            cx.sp.axis(1, 240.0, 0.0, 0).0,
        );
        let yaw = crate::cod2_viewmodel::view_angles(frame)[1];
        let hw = 0.5 * DAMAGE_ICON_W;
        let near = DAMAGE_ICON_OFFSET;
        let far = DAMAGE_ICON_OFFSET + DAMAGE_ICON_H;
        let quad = [[hw, far], [-hw, far], [-hw, near], [hw, near]];
        for d in &self.damage {
            let t = cx.now - d.start;
            if t < 1 || t >= d.ms {
                continue;
            }
            let a = (t as f32 * -2.0 / d.ms as f32 + 2.0).min(1.0);
            let (s, c) = (yaw - d.yaw).to_radians().sin_cos();
            out.push(HudQuad {
                verts: quad.map(|[px, py]| [c * px + cx0 - s * py, s * px + cy0 + c * py]),
                uvs: [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]],
                rgba: [1.0, 1.0, 1.0, a],
                texture: "hit_direction".to_string(),
            });
        }
    }
}

/// `CG_CalcPlayerHealth` (Mac 0x18a5e4): `stats[0] / stats[2]` in 0..1, 0
/// for a dead body or an unset stat.
fn health_frac(ps: &PlayerView) -> f32 {
    let (hp, max) = (ps.raw.stats[0], ps.raw.stats[2]);
    if hp == 0 || max == 0 || ps.pm_type == PM_DEAD {
        return 0.0;
    }
    (hp as f32 / max as f32).clamp(0.0, 1.0)
}

/// A pointer's distance out of the compass's radius: 0 at
/// `cg_hudCompassMinRange`, 1 at the max range and past it.
fn radius_frac(dist: f32) -> f32 {
    let d = dist.clamp(COMPASS_MIN_RANGE, COMPASS_MAX_RANGE);
    COMPASS_MIN_RADIUS
        + (1.0 - COMPASS_MIN_RADIUS) * (d - COMPASS_MIN_RANGE)
            / (COMPASS_MAX_RANGE - COMPASS_MIN_RANGE)
}

/// A client's `team` (0 free, 1 axis, 2 allies, 3 spectator).
fn team_of(frame: &Frame, client: i32) -> Option<i32> {
    let c = frame
        .snap
        .clients
        .iter()
        .find(|c| c.number as i32 == client)?;
    Some(c.f[field_index(&CLIENT_STATE_FIELDS, "team")] as i32)
}

/// `weaponslots` byte `slot` (1 primary, 2 primaryb).
fn slot_weapon(ps: &PlayerView, slot: usize) -> u8 {
    let word = ps.raw.i32(&format!("weaponslots[{}]", slot / 4 * 4)) as u32;
    (word >> (8 * (slot % 4))) as u8
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fade_holds_then_falls() {
        assert_eq!(fade(0, 100, 1700, 700), None, "unset");
        assert_eq!(fade(1000, 1500, 1700, 700), Some(1.0));
        assert_eq!(fade(1000, 2350, 1700, 700), Some(0.5));
        assert_eq!(fade(1000, 2700, 1700, 700), None);
    }

    #[test]
    fn the_low_health_overlay_pulses_four_times_then_clears() {
        let mut p = Pulse::default();
        let mut peak = 0.0f32;
        let mut t = 1000;
        // Hurt to a fifth, then held there.
        while t < 1000 + 5000 {
            p.step(0.2, t);
            peak = peak.max(p.alpha(t));
            t += 10;
        }
        assert_eq!(peak, 1.0, "the first pulse is whole");
        assert!(p.index > 3 || !p.active, "four pulses, then the end phase");
        let mut t2 = t;
        while t2 < t + 2000 {
            p.step(0.2, t2);
            t2 += 10;
        }
        assert_eq!(p.alpha(t2), 0.0, "cleared after the pulses");
    }

    #[test]
    fn a_respawn_clears_the_overlay_and_the_damage_icons() {
        let mut hud = PlayerHud::default();
        for t in (1000..1400).step_by(10) {
            hud.pulse.step(0.2, t);
        }
        assert!(hud.pulse.alpha(1400) > 0.0, "pulsing while hurt");
        hud.damage[0] = DamageIcon {
            start: 1300,
            ms: DAMAGE_ICON_MS,
            yaw: 90.0,
        };
        hud.spawn = Some((0, 2));
        // The next spawn's playerstate: `stats[5]` counted up, health full.
        hud.reset_for_spawn((0, 3), 0, 1500);
        assert_eq!(hud.pulse.alpha(1500), 0.0);
        assert!(hud.damage.iter().all(|d| d.start == 0));
        hud.pulse.step(1.0, 1510);
        assert_eq!(hud.pulse.alpha(1600), 0.0, "full health stays clear");
    }

    #[test]
    fn the_compass_radius_scales_to_the_rim() {
        assert_eq!(radius_frac(0.0), 0.0);
        assert_eq!(radius_frac(750.0), 0.5);
        assert_eq!(radius_frac(4000.0), 1.0);
    }

    /// The stock `hud.menu` gives the owner draws this module paints, with
    /// the rects and alignments the cgame reads.
    #[test]
    fn stock_hud_menu_owner_draws() {
        let Some(fs) = vcod_common::testing::cod2_fs() else {
            return;
        };
        let items = load_items(&fs);
        let find = |od: i32| items.iter().find(|i| i.ownerdraw == od).cloned();
        let ammo = find(od::AMMO_VALUE).expect("the ammo counter");
        // `weaponinfo` sits at the window's bottom right.
        assert_eq!(ammo.rect, [-85.0, -15.0, 70.0, 40.0]);
        assert_eq!(ammo.align, [3, 3]);
        assert_eq!(ammo.text_scale, 0.5);
        let back = find(od::COMPASS_BACK).expect("the compass");
        assert_eq!(back.rect, [-20.0, -140.0, 160.0, 160.0]);
        assert_eq!(back.align, [1, 3]);
        assert_eq!(
            back.background.as_deref(),
            Some("gfx/hud/hud@compassback.tga")
        );
        for od in [
            od::STANCE,
            od::CURSORHINT,
            od::WEAPON_NAME,
            od::COMPASS,
            od::COMPASS_POINTERS,
            od::COMPASS_FRIENDS,
            od::LOW_HEALTH_OVERLAY,
            od::OFFHAND_ICON_FRAG,
            od::OFFHAND_AMMO_SMOKE,
            od::OFFHAND_NAME_FRAG,
        ] {
            assert!(find(od).is_some(), "owner draw {od}");
        }
    }
}
