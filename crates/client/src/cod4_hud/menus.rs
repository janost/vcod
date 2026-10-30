//! The HUD's zone menus drawn item by item off their expressions
//! (docs/research/cod4-hud.md 23): the `xpbar`, `scorebars` and
//! `scorebar` menus, the `scoreboard` menu's header while `+scores` holds
//! it open, and the hint menus' owner draws: 71 the hold breath line, 80
//! the mantle line and its icon, 113 the invalid command line.

use vcod_cod4::menuexp;
use vcod_cod4::net::parse::Snapshot;
use vcod_cod4::scriptmenu::{ITEM_TYPE_OWNERDRAW, LayoutItem, WINDOW_VISIBLE};

use super::{Cod4Hud, HudEnv, Loaded, MenuAssets, image_of, quad, text};
use crate::console::shell::Shell;
use crate::hud::HudQuad;
use crate::hud::cod2elem::ScreenPlacement;
use crate::hud::font;
use crate::play::cod4::{Cod4Session, Fired};

/// The bar menus, in the zone's menu list order (`common_mp`).
pub(super) const BAR_MENUS: [&str; 3] = ["xpbar", "scorebars", "scorebar"];
/// The hint menus, in the zone's menu list order.
pub(super) const HINT_MENUS: [&str; 3] = ["mantlehint", "invalidcmdhint", "holdbreathhint"];
/// The UI menu `+scores` opens (`iw3mp.exe` 0x42aaa0, string 0x6e8f3c).
pub(super) const SCOREBOARD_MENU: &str = "scoreboard";

const OD_HOLD_BREATH: i32 = 71;
const OD_MANTLE: i32 = 80;
const OD_INVALID_CMD: i32 = 113;

/// `cg_invalidCmdHintDuration` 1800 and `cg_invalidCmdHintBlinkInterval`
/// 600 (`iw3mp.exe` 0x43da20, 0x43da68).
const INVALID_CMD_MS: i32 = 1800;
const INVALID_CMD_BLINK_MS: i32 = 600;
/// `mantleState.flags`: a mantle is offered (`MANTLE_HINT`).
const MANTLE_HINT: u32 = 0x8;
/// `usercmd.buttons`: `+holdbreath`.
const BUTTON_HOLD_BREATH: u32 = 0x2000;
/// `weapClass` 9, an item.
const WEAPCLASS_ITEM: i32 = 9;

/// `CG_SetInvalidCmdHint`'s type off a player view event: 67
/// `EV_NO_FRAG_GRENADE_HINT`, 68 `_SPECIAL_`, 69 `EV_TARGET_TOO_CLOSE_HINT`,
/// 70 `EV_TARGET_NOT_ENOUGH_CLEARANCE`, 71 `EV_LOCKON_REQUIRED_HINT`.
fn invalid_cmd_of_event(id: u32) -> Option<u8> {
    Some(match id {
        67 => 2,
        68 => 3,
        69 => 7,
        70 => 9,
        71 => 8,
        _ => return None,
    })
}

/// `CG_DrawInvalidCmdHint`'s string by type (`iw3mp.exe` 0x443c60, its
/// jump table at 0x443dbc); 4 draws nothing.
fn invalid_cmd_key(kind: u8) -> Option<&'static str> {
    Some(match kind {
        1 => "WEAPON_NO_AMMO",
        2 => "WEAPON_NO_FRAG_GRENADE",
        3 => "WEAPON_NO_SPECIAL_GRENADE",
        5 => "GAME_STAND_BLOCKED",
        6 => "GAME_CROUCH_BLOCKED",
        7 => "WEAPON_TARGET_TOO_CLOSE",
        8 => "WEAPON_LOCKON_REQUIRED",
        9 => "WEAPON_TARGET_NOT_ENOUGH_CLEARANCE",
        _ => return None,
    })
}

/// A hint owner draw's pieces on the 640x480 grid, placed by its item's
/// alignment: a line with its baseline at `at`, and a picture.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Hint {
    pub(crate) text: String,
    pub(crate) at: [f32; 2],
    pub(crate) rgba: [f32; 4],
    pub(crate) pic: Option<(&'static str, [f32; 4])>,
}

impl Cod4Hud {
    /// The frame's player view events: a hint event sets the invalid
    /// command line, stamped only when its type changes
    /// (`CG_SetInvalidCmdHint`); past `cg_invalidCmdHintDuration` it
    /// clears.
    pub(super) fn update_hints(&mut self, fired: &[Fired], now: i32) {
        for f in fired.iter().filter(|f| f.entity.is_none()) {
            if let Some(k) = invalid_cmd_of_event(f.id)
                && self.invalid_cmd.map(|(old, _)| old) != Some(k)
            {
                self.invalid_cmd = Some((k, now));
            }
        }
        if self
            .invalid_cmd
            .is_some_and(|(_, at)| at + INVALID_CMD_MS < now)
        {
            self.invalid_cmd = None;
        }
    }

    /// Menu `name` when its `visible when` holds: each visible item's
    /// window and text (`Item_Paint`), a hint owner draw by its number.
    /// Returns the texts drawn.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn draw_zone_menu(
        &self,
        l: &Loaded,
        name: &str,
        env: &HudEnv,
        s: &Cod4Session,
        snap: &Snapshot,
        shell: &Shell,
        sp: &ScreenPlacement,
        now: i32,
        out: &mut Vec<HudQuad>,
    ) -> Vec<String> {
        let mut texts = Vec::new();
        let Some(layout) = l.script_menus.get(name).and_then(|m| m.layout.as_ref()) else {
            return texts;
        };
        if !(layout.visible.is_empty() || menuexp::truth(&layout.visible, env).unwrap_or(false)) {
            return texts;
        }
        let a = MenuAssets { fonts: &l.fonts, l };
        let f = |st: &vcod_cod4::assets::ui::Statement| menuexp::float(st, env).unwrap_or(0.0);
        let strip = strip_align(&layout.items, sp);
        for it in &layout.items {
            let e = &it.exps;
            if it.dynamic_flags & WINDOW_VISIBLE == 0
                || !(e.visible.is_empty() || menuexp::truth(&e.visible, env).unwrap_or(false))
            {
                continue;
            }
            // `Item_Paint`'s rect expressions: x and y off the menu's.
            let mut r = it.rect;
            if !e.rect_x.is_empty() {
                r[0] = f(&e.rect_x) + layout.rect[0];
            }
            if !e.rect_y.is_empty() {
                r[1] = f(&e.rect_y) + layout.rect[1];
            }
            if !e.rect_w.is_empty() {
                r[2] = f(&e.rect_w);
            }
            if !e.rect_h.is_empty() {
                r[3] = f(&e.rect_h);
            }
            // `vc_windowAspect 1`: the strip and what lies along it widen
            // to the window.
            if strip == Some(it.align) {
                r[0] *= sp.strip_scale;
                r[2] *= sp.strip_scale;
            }
            let mut fore = it.fore_color;
            if !e.forecolor_a.is_empty() {
                fore[3] = f(&e.forecolor_a);
            }
            if it.kind == ITEM_TYPE_OWNERDRAW {
                if let Some(h) = self.hint(l, it, r, s, snap, shell, now) {
                    draw_hint(l, it, &h, sp, out);
                    texts.push(h.text);
                }
                continue;
            }
            let material = if e.material.is_empty() {
                it.background.clone()
            } else {
                menuexp::string(&e.material, env).ok()
            }
            .map(|m| material_name(l, &m));
            let text = match (&it.text, e.text.is_empty()) {
                (Some(t), _) => Some(t.clone()),
                (None, false) if matches!(it.kind, 0 | 1) => menuexp::string(&e.text, env).ok(),
                _ => None,
            };
            let drawn = crate::cod4_menu::hud_item(
                &a,
                it,
                r,
                fore,
                material.as_deref(),
                text.as_deref().filter(|t| !t.is_empty()),
                sp,
                out,
            );
            texts.extend(drawn.map(|(t, _)| t));
        }
        texts
    }

    /// The texts menu `name` draws this frame, for the gates.
    #[cfg(test)]
    pub(crate) fn zone_menu_texts(
        &self,
        s: &Cod4Session,
        name: &str,
        shell: &Shell,
        now: i32,
    ) -> Vec<String> {
        self.zone_menu_test(s, name, shell, now).0
    }

    /// The quads the bar menus draw this frame, for the gates.
    #[cfg(test)]
    pub(crate) fn bar_quads(&self, s: &Cod4Session, shell: &Shell, now: i32) -> usize {
        BAR_MENUS
            .iter()
            .map(|m| self.zone_menu_test(s, m, shell, now).1)
            .sum()
    }

    #[cfg(test)]
    fn zone_menu_test(
        &self,
        s: &Cod4Session,
        name: &str,
        shell: &Shell,
        now: i32,
    ) -> (Vec<String>, usize) {
        let (Some(Some(l)), Some(snap)) = (&self.loaded, s.client().snapshots.last()) else {
            return Default::default();
        };
        let env = self.env(s, now, shell);
        let sp = ScreenPlacement::new(640.0, 480.0);
        let mut out = Vec::new();
        let texts = self.draw_zone_menu(l, name, &env, s, snap, shell, &sp, now, &mut out);
        (texts, out.len())
    }

    /// Hint menu `menu`'s owner draw this frame at its item's rect, `None`
    /// when it draws nothing; for the gates.
    #[cfg(test)]
    pub(crate) fn hint_at(
        &self,
        s: &Cod4Session,
        menu: &str,
        shell: &Shell,
        now: i32,
    ) -> Option<Hint> {
        let l = self.loaded.as_ref()?.as_ref()?;
        let snap = s.client().snapshots.last()?;
        let it = l
            .script_menus
            .get(menu)?
            .layout
            .as_ref()?
            .items
            .iter()
            .find(|it| it.kind == ITEM_TYPE_OWNERDRAW)?;
        self.hint(l, it, it.rect, s, snap, shell, now)
    }

    /// Owner draw `it.owner_draw` of a hint menu over `rect`, `None` when
    /// it draws nothing this frame. It reads the newest snapshot's
    /// playerstate where retail reads the predicted one.
    #[allow(clippy::too_many_arguments)]
    fn hint(
        &self,
        l: &Loaded,
        it: &LayoutItem,
        rect: [f32; 4],
        s: &Cod4Session,
        snap: &Snapshot,
        shell: &Shell,
        now: i32,
    ) -> Option<Hint> {
        let u = |n: &str| snap.ps.get(n).unwrap_or(0);
        let tr = |k: &str| l.localized.get(k).unwrap_or(k).to_string();
        let grid = ScreenPlacement::new(640.0, 480.0);
        let fnt = l.fonts.pick(it.font, it.text_scale, &grid);
        let width = |t: &str| font::measure(fnt, t, it.text_scale / fnt.unit_scale());
        // `UI_GetKeyBindingLocalizedString` down a list: the first bound.
        let key = |cmds: &[&str]| {
            cmds.iter()
                .find(|c| shell.key_text(c, &l.localized).is_some())
                .map_or_else(
                    || self.key_binding(shell, cmds[cmds.len() - 1]),
                    |c| self.key_binding(shell, c),
                )
        };
        let [x, y, w, h] = rect;
        match it.owner_draw {
            OD_HOLD_BREATH => {
                // `CG_DrawHoldBreathHint` (0x443900).
                if u("weapFlags") & 4 != 0 {
                    return None;
                }
                let held = if u("weapFlags") & 2 != 0 {
                    u("offHandIndex")
                } else {
                    u("weapon")
                } as usize;
                let def = l.weapon_hud.get(self.weapons.get(held.checked_sub(1)?)?)?;
                let scoped = snap.ps.get_f32("fWeaponPosFrac") == Some(1.0);
                let pm_type = u("pm_type") as i32;
                let holding = s.input.buttons(pm_type, s.talking) & BUTTON_HOLD_BREATH != 0;
                if def.reticle.overlay_reticle == 0
                    || def.weap_class == WEAPCLASS_ITEM
                    || !scoped
                    || holding
                    || u("holdBreathTimer") != 0
                {
                    return None;
                }
                let t = tr("PLATFORM_HOLD_BREATH").replace(
                    "&&1",
                    &key(&["+holdbreath", "+melee_breath", "+breath_sprint"]),
                );
                Some(Hint {
                    at: [x - (width(&t) * 0.5).round(), y],
                    text: t,
                    rgba: [1.0; 4],
                    pic: None,
                })
            }
            OD_MANTLE => {
                // `CG_DrawMantleHint` (0x443ab0).
                if u("mantleState.flags") & MANTLE_HINT == 0 {
                    return None;
                }
                let t = tr("PLATFORM_MANTLE").replace("&&1", &key(&["+gostand", "+moveup"]));
                let tw = width(&t);
                let th = (48.0 * it.text_scale) as i32 as f32;
                let tx = x - (w + tw) * 0.5;
                Some(Hint {
                    at: [tx, th * 0.5 + y],
                    text: t,
                    rgba: it.fore_color,
                    pic: Some(("hint_mantle", [tx + tw, y - h * 0.5, w, h])),
                })
            }
            OD_INVALID_CMD => {
                // `CG_DrawInvalidCmdHint` (0x443c60): blinks up from 0
                // every `cg_invalidCmdHintBlinkInterval`.
                let (kind, at) = self.invalid_cmd?;
                if at + INVALID_CMD_MS < now {
                    return None;
                }
                let t = tr(invalid_cmd_key(kind)?);
                let mut rgba = it.fore_color;
                rgba[3] = (now - at).rem_euclid(INVALID_CMD_BLINK_MS) as f32
                    / INVALID_CMD_BLINK_MS as f32;
                Some(Hint {
                    at: [x - (width(&t) * 0.5).round(), y],
                    text: t,
                    rgba,
                    pic: None,
                })
            }
            _ => None,
        }
    }
}

/// The alignment of a menu's strip, a `LEFT` item from x 0 or less across
/// the whole screen its pieces were made for (640 grid units at 4:3, 854
/// at 16:9: `xpbar`'s backing, cod4-front-end.md 10.2), when the screen
/// placement widens strips (`vc_windowAspect 1`). Items of that alignment
/// scale across with it.
fn strip_align(items: &[LayoutItem], sp: &ScreenPlacement) -> Option<[i32; 2]> {
    if sp.strip_scale <= 1.0 {
        return None;
    }
    items
        .iter()
        .find(|it| it.align[0] == 1 && it.rect[0] <= 0.0 && it.rect[0] + it.rect[2] >= 640.0)
        .map(|it| it.align)
}

/// A menu's material name as the HUD's images key it: one the images
/// lack read without its leading `,` (`,white`, the engine's own image).
fn material_name(l: &Loaded, m: &str) -> String {
    let m = m.to_ascii_lowercase();
    match image_of(l, &m) {
        Some(_) => m,
        None => m.trim_start_matches(',').to_string(),
    }
}

/// A hint's line, then its picture, at the item's alignment.
fn draw_hint(l: &Loaded, it: &LayoutItem, h: &Hint, sp: &ScreenPlacement, out: &mut Vec<HudQuad>) {
    let f = l.fonts.pick(it.font, it.text_scale, sp);
    let px = it.text_scale / f.unit_scale();
    let th = (48.0 * it.text_scale) as i32 as f32;
    let (rx, _) = sp.axis(0, h.at[0], 0.0, it.align[0]);
    let (ry, _) = sp.axis(1, h.at[1] - th, th, it.align[1]);
    text(f, &h.text, rx, ry, px * sp.virtual_to_real[1], h.rgba, out);
    if let Some((m, [x, y, w, hh])) = h.pic
        && let Some(tex) = image_of(l, m)
    {
        let (rx, rw) = sp.axis(0, x, w, it.align[0]);
        let (ry, rh) = sp.axis(1, y, hh, it.align[1]);
        out.push(quad(rx, ry, rw, rh, h.rgba, tex, false));
    }
}
