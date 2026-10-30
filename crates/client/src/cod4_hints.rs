//! The cursor hint (`cursorhints` menu, owner draw 72): what the use key
//! or the frag key would take under the crosshair, off the playerstate's
//! `cursorHint` and `cursorHintString` (docs/research/cod4-hud.md 20). A
//! child of `cod4_hud`, whose loaded zones it draws with.

use super::*;
use vcod_cod4::menuexp;

/// The menu and its owner draw.
pub(super) const CURSOR_HINTS_MENU: &str = "cursorhints";
const OD_CURSOR_HINT: i32 = 72;
/// `cg_hintFadeTime` (1.7 default 100, `iw3mp.exe` 0x43ad38): how long a
/// hint stays, fading over its last 100 ms, once the playerstate drops it.
const HINT_FADE_MS: i32 = 100;
/// `cg_weaponHintsCoD1Style` (1.7 default 1, 0x43acfb): a weapon's name
/// goes on the text's line and its icon under it.
const COD1_STYLE: bool = true;
/// The configstring of hint string 0 (the use string, 0x443140).
const CS_HINT_STRINGS: u16 = 277;
/// `cursorHint` 1 is the text alone; 2..4 `hint_usable`, `hint_health`,
/// `hint_friendly`; 5..=132 a weapon, its number plus 4.
const HINT_NOICON: u32 = 1;
const HINT_HEALTH: u32 = 3;
const HINT_MATERIALS: [&str; 3] = ["hint_usable", "hint_health", "hint_friendly"];
const FIRST_WEAPON_HINT: u32 = 5;
const LAST_WEAPON_HINT: u32 = 132;
/// `weapClass` 7, a turret; `offhandClass` 1, a frag.
const WEAPCLASS_TURRET: i32 = 7;
const OFFHAND_FRAG: i32 = 1;

/// `cg.cursorHintTime`, `cursorHintFade`, `cursorHintIcon` and
/// `cursorHintString`, which `CG_UpdateCursorHints` (0x443000) keeps
/// while the playerstate names a hint.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct CursorHint {
    pub time: i32,
    pub fade: i32,
    pub icon: u32,
    pub string: i32,
}

impl CursorHint {
    /// `CG_UpdateCursorHints` at cg time `now` on the drawn playerstate's
    /// `cursorHint` and `cursorHintString`.
    pub(crate) fn step(&mut self, hint: u32, string: i32, now: i32) {
        if hint != 0 {
            *self = CursorHint {
                time: now,
                fade: HINT_FADE_MS,
                icon: hint,
                string,
            };
        }
    }

    /// `CG_FadeAlpha(time, cursorHintTime, cursorHintFade, 100)`: whole
    /// until the last 100 ms of the fade.
    pub(crate) fn alpha(&self, now: i32) -> f32 {
        let left = self.fade - (now - self.time);
        if left < 100 { left as f32 / 100.0 } else { 1.0 }
    }
}

/// `CG_DrawCursorhint`'s pieces, in 640 grid units off the item's
/// alignment: text at a baseline, and the icon.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct HintDraw {
    pub texts: Vec<(String, f32, f32)>,
    pub icon: Option<(String, [f32; 4])>,
}

/// A hint's icon: its material and `hudIconRatio` (0 square, 1 twice as
/// wide, 2 also half as high; cod4-hud.md 20).
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct HintIcon {
    pub material: String,
    pub ratio: i32,
}

impl HintIcon {
    fn square(material: &str) -> HintIcon {
        HintIcon {
            material: material.to_string(),
            ratio: 0,
        }
    }

    /// Width and height over the item's.
    fn scale(&self) -> (f32, f32) {
        match self.ratio {
            1 => (2.0, 1.0),
            2 => (2.0, 0.5),
            _ => (1.0, 1.0),
        }
    }
}

/// What a hint shows: an icon, a prompt and the weapon's name.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct HintSubject {
    pub icon: Option<HintIcon>,
    pub text: Option<String>,
    pub name: Option<String>,
}

/// Places `subject` in the item's `rect` (x, y, w, h), its text lines
/// `line` high and as wide as `measure` says (cod4-hud.md 20): every
/// piece centred on the item's x, the text's baseline half a line under
/// its y. With a name (CoD1 style), the prompt and `" " + name` side by
/// side, centred on their widths without the space, the icon a line and
/// a half under the icon's top; with a prompt alone, prompt then icon in
/// one row; an icon alone sits on the item's y. `None` for a text-only
/// hint with no text.
pub(crate) fn lay_out_hint(
    subject: HintSubject,
    rect: [f32; 4],
    line: f32,
    measure: impl Fn(&str) -> f32,
) -> Option<HintDraw> {
    let [x, y, w, h] = rect;
    let baseline = y + line * 0.5;
    let text = subject.text.filter(|t| !t.is_empty());
    let Some(icon) = subject.icon else {
        let t = text?;
        let left = measure(&t) * -0.5;
        return Some(HintDraw {
            texts: vec![(t, left, baseline)],
            icon: None,
        });
    };
    let (sx, sy) = icon.scale();
    let size = (w * sx, h * sy);
    let icon_top = y - h * sy * 0.5;
    let place = |left: f32, top: f32| Some((icon.material.clone(), [left, top, size.0, size.1]));
    let name = subject.name.filter(|_| COD1_STYLE);
    let draw = match (text, name) {
        (Some(t), Some(name)) => {
            let prompt = measure(&t);
            let left = (prompt + measure(&name)) * -0.5;
            HintDraw {
                texts: vec![
                    (t, left, baseline),
                    (format!(" {name}"), left + prompt, baseline),
                ],
                icon: place(size.0 * -0.5, icon_top + line * 1.5),
            }
        }
        (Some(t), None) => {
            let prompt = measure(&t);
            let left = (size.0 + prompt) * -0.5;
            HintDraw {
                texts: vec![(t, left, baseline)],
                icon: place(left + prompt, icon_top),
            }
        }
        // A wide icon (ratio 1 or 2) shifts by half the item's width more.
        (None, _) => {
            let shift = if sx > 1.0 { w * -0.5 } else { 0.0 };
            HintDraw {
                texts: Vec::new(),
                icon: place(x - (w + shift) * 0.5, y),
            }
        }
    };
    Some(draw)
}

impl Cod4Hud {
    /// Steps [`CursorHint`] off the newest snapshot at `now`.
    pub(super) fn step_cursor_hint(&mut self, s: &Cod4Session, now: i32) {
        if let Some(snap) = s.client().snapshots.last() {
            let u = |n: &str| snap.ps.get(n).unwrap_or(0);
            self.hint
                .step(u("cursorHint"), u("cursorHintString") as i32, now);
        }
    }

    /// The hint's text and icon as `CG_DrawCursorhint` (0x4431e0) lays
    /// them out at `now` for the `cursorhints` item `it` drawn at `sp`,
    /// and its alpha; `None` when nothing draws (cod4-hud.md 20). Widths
    /// are the owner draw's font picked at `sp` (`UI_TextWidth`, 0x4435fe),
    /// not the 640 grid's: past 1080p that is a bigger font with other
    /// advances, and the text ran into the name.
    pub(super) fn cursor_hint_draw(
        &self,
        l: &Loaded,
        s: &Cod4Session,
        it: &vcod_cod4::scriptmenu::LayoutItem,
        shell: &Shell,
        sp: &ScreenPlacement,
        now: i32,
    ) -> Option<(HintDraw, f32)> {
        let alpha = self.hint.alpha(now) * it.fore_color[3];
        if alpha <= 0.0 || self.hint.icon == 0 {
            return None;
        }
        let snap = s.client().snapshots.last()?;
        // vcod-server's dead end frame leaves a dead body's last hint; the
        // HUD draws none past `PM_DEAD` (cod4-hud.md 20).
        if snap.ps.get("pm_type").unwrap_or(0) >= PM_DEAD {
            return None;
        }
        let subject = self.hint_subject(l, s, &snap.ps, shell)?;
        let font = l.fonts.pick(it.font, it.text_scale, sp);
        let measure = |t: &str| font::measure(font, t, it.text_scale / font.unit_scale());
        let line = (48.0 * it.text_scale) as i32 as f32;
        Some((lay_out_hint(subject, it.rect, line, measure)?, alpha))
    }

    /// What the hint names: its icon and the strings beside it.
    fn hint_subject(
        &self,
        l: &Loaded,
        s: &Cod4Session,
        ps: &vcod_cod4::net::delta::PlayerState,
        shell: &Shell,
    ) -> Option<HintSubject> {
        let key_of = |cmd: &str| {
            shell
                .key_text(cmd, &l.localized)
                .unwrap_or_else(|| self.localize_key("KEY_USE"))
        };
        // A `PLATFORM_*` prompt with its `&&1` the key bound to `cmd`.
        let prompt =
            |text: &str, cmd: &str| self.localize_key(text).replacen("&&1", &key_of(cmd), 1);
        // The use string (0x443140): hint string `cursorHintString`'s
        // configstring, localized, `&&1` the use key; none when empty.
        let use_string = || -> Option<String> {
            let cs = self.configstring(s, CS_HINT_STRINGS + u16::try_from(self.hint.string).ok()?);
            (!cs.is_empty()).then(|| self.message(&cs).replacen("&&1", &key_of("+activate"), 1))
        };
        let hint = self.hint.icon;
        if hint == HINT_NOICON {
            return Some(HintSubject {
                icon: None,
                text: use_string(),
                name: None,
            });
        }
        if !(FIRST_WEAPON_HINT..=LAST_WEAPON_HINT).contains(&hint) {
            let material = HINT_MATERIALS.get(hint.checked_sub(2)? as usize)?;
            let text = match (self.hint.string >= 0, hint) {
                (true, _) => use_string(),
                (false, HINT_HEALTH) => Some(prompt("PLATFORM_PICKUPHEALTH", "+activate")),
                _ => None,
            };
            return Some(HintSubject {
                icon: Some(HintIcon::square(material)),
                text,
                name: None,
            });
        }
        let list = crate::play::cod4::weapon_list(s.gamestate()?);
        let def_of = |n: usize| l.weapon_hud.get(list.get(n.checked_sub(1)?)?);
        let def = def_of((hint - 4) as usize)?;
        let icon = HintIcon {
            material: def.hud_icon.clone()?,
            ratio: def.hud_icon_ratio,
        };
        let name = Some(self.message(&def.display_name));
        // The weapon's prompt (0x443050): a turret takes the use string; a
        // frag offhand the throw back; another item the pickup; a primary
        // the swap once two primaries are held.
        let (text, name) = if def.weap_class == WEAPCLASS_TURRET {
            (use_string(), name)
        } else if def.inventory_type != 0 {
            if def.offhand_class == OFFHAND_FRAG {
                (Some(prompt("PLATFORM_THROWBACKGRENADE", "+frag")), None)
            } else {
                (Some(prompt("PLATFORM_PICKUPNEWWEAPON", "+activate")), name)
            }
        } else {
            let holds = |n: usize| {
                ps.get(&format!("weapons[{}]", n / 32)).unwrap_or(0) & 1 << (n % 32) != 0
            };
            let primaries_held = (1..=list.len())
                .filter(|&n| holds(n) && def_of(n).is_some_and(|d| d.inventory_type == 0))
                .count();
            let text = if primaries_held >= 2 {
                "PLATFORM_SWAPWEAPONS"
            } else {
                "PLATFORM_PICKUPNEWWEAPON"
            };
            (Some(prompt(text, "+activate")), name)
        };
        Some(HintSubject {
            icon: Some(icon),
            text,
            name,
        })
    }

    /// The `cursorhints` menu's visible items' hints as quads.
    pub(super) fn draw_cursor_hint(
        &self,
        l: &Loaded,
        s: &Cod4Session,
        env: &HudEnv,
        sp: &ScreenPlacement,
        now: i32,
        out: &mut Vec<HudQuad>,
    ) {
        let Some(layout) = l
            .script_menus
            .get(CURSOR_HINTS_MENU)
            .and_then(|m| m.layout.as_ref())
        else {
            return;
        };
        for it in &layout.items {
            if it.owner_draw != OD_CURSOR_HINT
                || !(it.exps.visible.is_empty()
                    || menuexp::truth(&it.exps.visible, env).unwrap_or(false))
            {
                continue;
            }
            let Some((d, alpha)) = self.cursor_hint_draw(l, s, it, env.shell, sp, now) else {
                continue;
            };
            let mut rgba = it.fore_color;
            rgba[3] = alpha;
            let item = |rect: [f32; 4]| h::MenuItem {
                rect: h::MenuRect {
                    rect,
                    horz_align: it.align[0],
                    vert_align: it.align[1],
                    background: None,
                },
                owner_draw: it.owner_draw,
                font: it.font,
                text_scale: it.text_scale,
                text_style: it.text_style,
                fore_color: rgba,
                text_align: it.text_align,
                text: None,
            };
            for (t, x, y) in d.texts {
                let draw = OwnerDraw::Text {
                    text: t,
                    x,
                    item: item([x, y, 0.0, 0.0]),
                    rgba,
                };
                self.draw_owner(l, sp, &draw, out);
            }
            if let Some((material, rect)) = d.icon {
                let draw = OwnerDraw::Pic {
                    material,
                    item: item(rect),
                };
                self.draw_owner(l, sp, &draw, out);
            }
        }
    }

    /// A localized key's text, else the key.
    fn localize_key(&self, key: &str) -> String {
        self.localized()
            .and_then(|l| l.get(key))
            .unwrap_or(key)
            .to_string()
    }

    /// The cursor hint as a frame of `size` would draw it, what the live
    /// gate reads.
    #[cfg(test)]
    pub(crate) fn cursor_hint_now(
        &self,
        s: &Cod4Session,
        shell: &Shell,
        size: (f32, f32),
        now: i32,
    ) -> Option<(HintDraw, f32)> {
        let Some(Some(l)) = &self.loaded else {
            return None;
        };
        let layout = l.script_menus.get(CURSOR_HINTS_MENU)?.layout.as_ref()?;
        let it = layout
            .items
            .iter()
            .find(|i| i.owner_draw == OD_CURSOR_HINT)?;
        let sp = ScreenPlacement::cod4(size.0, size.1);
        self.cursor_hint_draw(l, s, it, shell, &sp, now)
    }

    /// A weapon's `hudIcon` material, once loaded.
    #[cfg(test)]
    pub(crate) fn weapon_icon(&self, name: &str) -> Option<String> {
        let Some(Some(l)) = &self.loaded else {
            return None;
        };
        l.weapon_hud.get(name)?.hud_icon.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::{CursorHint, HintDraw, HintIcon, HintSubject, lay_out_hint};

    /// The stock item's rect and line (scale 0.375), ten units a letter.
    fn lay_out(
        icon: Option<(&str, i32)>,
        text: Option<&str>,
        name: Option<&str>,
    ) -> Option<HintDraw> {
        let subject = HintSubject {
            icon: icon.map(|(m, ratio)| HintIcon {
                material: m.to_string(),
                ratio,
            }),
            text: text.map(str::to_string),
            name: name.map(str::to_string),
        };
        lay_out_hint(subject, [0.0, 60.0, 40.0, 40.0], 18.0, |t| {
            t.len() as f32 * 10.0
        })
    }

    fn texts(v: &[(&str, f32)]) -> Vec<(String, f32, f32)> {
        v.iter().map(|&(t, x)| (t.to_string(), x, 69.0)).collect()
    }

    /// Each layout cod4-hud.md 20 names: the text alone, prompt and name
    /// over the icon, prompt then icon, the icon alone, wide icons.
    #[test]
    fn cod4_hints_lay_out_each_shape() {
        let icon = |rect| Some(("m".to_string(), rect));
        let draw = |t: &[(&str, f32)], i| HintDraw {
            texts: texts(t),
            icon: i,
        };
        assert_eq!(
            lay_out(None, Some("abc"), None),
            Some(draw(&[("abc", -15.0)], None))
        );
        assert_eq!(lay_out(None, Some(""), None), None);
        assert_eq!(lay_out(None, None, None), None);
        assert_eq!(
            lay_out(Some(("m", 0)), Some("Press"), Some("M16")),
            Some(draw(
                &[("Press", -40.0), (" M16", 10.0)],
                icon([-20.0, 67.0, 40.0, 40.0])
            ))
        );
        assert_eq!(
            lay_out(Some(("m", 2)), Some("Press"), Some("M16")),
            Some(draw(
                &[("Press", -40.0), (" M16", 10.0)],
                icon([-40.0, 77.0, 80.0, 20.0])
            ))
        );
        assert_eq!(
            lay_out(Some(("m", 0)), Some("Press"), None),
            Some(draw(&[("Press", -45.0)], icon([5.0, 40.0, 40.0, 40.0])))
        );
        assert_eq!(
            lay_out(Some(("m", 0)), None, Some("M16")),
            Some(draw(&[], icon([-20.0, 60.0, 40.0, 40.0])))
        );
        assert_eq!(
            lay_out(Some(("m", 1)), Some(""), None),
            Some(draw(&[], icon([-10.0, 60.0, 80.0, 40.0])))
        );
    }

    /// The hint stays whole while the playerstate names it and for the
    /// fade's first ms after, then is gone: `cg_hintFadeTime` 100 fades
    /// over its whole length.
    #[test]
    fn the_hint_holds_then_fades_out() {
        let mut h = CursorHint::default();
        h.step(9, -1, 1000);
        h.step(0, -1, 1016);
        assert_eq!(h.alpha(1000), 1.0);
        assert_eq!(h.alpha(1050), 0.5);
        assert!(h.alpha(1100) <= 0.0);
        assert_eq!((h.icon, h.string), (9, -1));
    }
}
