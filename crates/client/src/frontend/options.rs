//! The stock options screens' controls (docs/research/cod11-front-end.md,
//! section 14): key binds, yes/no toggles, sliders, string lists and the name
//! field, each reading and writing the console's cvars and binds the way
//! `ui_mp_x86.dll`'s item handlers do.

use std::cell::RefCell;
use std::collections::HashMap;

use vcod_common::localize::Localized;
use vcod_common::text::atoi_msvc;
use vcod_common::ui_menu::{
    ITEM_TYPE_BIND, ITEM_TYPE_MULTI, ITEM_TYPE_NUMERICFIELD, ITEM_TYPE_SLIDER, ITEM_TYPE_YESNO,
    UiItem,
};
use vcod_common::ui_menu::{ITEM_TYPE_DECIMALFIELD, ITEM_TYPE_DVARENUM};

use winit::keyboard::KeyCode;

use super::item_rules::{
    SLIDER_H, SLIDER_W, THUMB_H, THUMB_W, enum_index, slider_value, step, thumb_x,
};
use super::paint::Painter;
use super::{Ui, UiEffect};
use crate::console::shell::{Shell, bare, quotable};
use crate::hud::HudQuad;

/// `g_bindings` (`ui_mp_x86.dll` 0x40036130, 50 records of 24 bytes): the
/// only commands a bind item can change. Any other shows as unbound. United
/// Offensive's table (`uo_ui_mp_x86.dll` 0x400406b0, 55 records) adds five,
/// `+sprint` among them, which its `options_move.menu` row binds; 1.1's
/// menus name none of them, so one table serves both.
const BIND_COMMANDS: &[&str] = &[
    "+scores",
    "+speed",
    "+forward",
    "+back",
    "+moveleft",
    "+moveright",
    "+moveup",
    "+movedown",
    "+left",
    "+right",
    "+strafe",
    "+lookup",
    "+lookdown",
    "+mlook",
    "centerview",
    "+attack",
    "weapprev",
    "weapnext",
    "weapalt",
    "scoresUp",
    "scoresDown",
    "messagemode",
    "messagemode2",
    "messagemode3",
    "messagemode4",
    "+activate",
    "+reload",
    "help",
    "+leanleft",
    "+leanright",
    "vote yes",
    "vote no",
    "mp_QuickMessage",
    "mp_quickmap",
    "mp_purchase",
    "weaponslot primary",
    "weaponslot primaryb",
    "weaponslot pistol",
    "weaponslot grenade",
    "weaponslot smokegrenade",
    "weaponslot satchel",
    "weaponslot binocular",
    "+melee",
    "+prone",
    "lowerstance",
    "raisestance",
    "togglecrouch",
    "toggleprone",
    "goprone",
    "gocrouch",
    "+gostand",
    "toggle cl_run",
    "+sprint",
    "screenshot",
    "screenshotJPEG",
];

/// `UI_KEYBINDSTATUS` from `ui/menudef.h`.
pub(super) const UI_KEYBINDSTATUS: i32 = 250;

/// `Item_Slider_Paint`'s (0x400150a0) bar, at the item's top, and thumb,
/// 2 above it ([`super::item_rules`] holds their sizes).
const SLIDER_BAR: &str = "ui/assets/slider2.tga";
const SLIDER_THUMB: &str = "ui/assets/sliderbutt_1.tga";

/// The options screens' input state.
#[derive(Default)]
pub(super) struct State {
    /// The bind item waiting for a key (`g_waitingForKey`), as (menu, item).
    waiting: Option<(usize, usize)>,
    /// The edit field taking typed text.
    pub(super) editing: Option<(usize, usize)>,
    /// `key_overstrikeMode` (CoDMP.exe 0x142f628, UI traps 0x2d and 0x2e):
    /// an edit starts it on and Insert toggles it; it outlives the edit.
    pub(super) overstrike: bool,
    /// The slider following the mouse until the button comes up.
    drag: Option<(usize, usize)>,
    /// Cvars this input set, which the shell sees only once the effects
    /// run; later script commands in the same input read them from here.
    pending: Vec<(String, String)>,
    /// Each slider's bar x as last drawn, for hit tests.
    slider_x: RefCell<HashMap<(usize, usize), f32>>,
}

impl State {
    /// Drops whatever `menu` held when it closes.
    pub(super) fn forget(&mut self, menu: usize) {
        for slot in [&mut self.waiting, &mut self.editing, &mut self.drag] {
            if slot.is_some_and(|(m, _)| m == menu) {
                *slot = None;
            }
        }
    }

    pub(super) fn clear(&mut self) {
        self.waiting = None;
        self.editing = None;
        self.drag = None;
        self.pending.clear();
    }

    /// A cvar set earlier in this input.
    pub(super) fn pending(&self, name: &str) -> Option<&str> {
        self.pending
            .iter()
            .rev()
            .find(|(n, _)| n.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }

    /// A new input starts: the shell has run the last one's effects.
    pub(super) fn new_input(&mut self) {
        self.pending.clear();
    }
}

/// An edit field's cursor and the first character it paints.
#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
pub(super) struct EditPos {
    pub(super) cursor: usize,
    pub(super) offset: usize,
}

/// `Item_TextField_HandleKey`'s 1024-byte buffer (0x40012690): the cvar's
/// text, NUL-padded, with `len` its length held to `maxChars`. The edits
/// move characters as its `memmove`s do, so a cursor past the text leaves
/// a gap that ends it.
struct Field {
    buf: Vec<char>,
    len: usize,
    max: usize,
    paint: usize,
    changed: bool,
}

/// `MAX_EDITFIELD` less one: an insert stops at 255 characters (0x400127ca).
const EDIT_MAX: usize = 255;

impl Field {
    fn read(value: &str, item: &UiItem) -> Field {
        let mut buf: Vec<char> = value.chars().take(1023).collect();
        let len = match item.max_chars {
            0 => buf.len(),
            max => buf.len().min(max),
        };
        buf.resize(1024, '\0');
        Field {
            buf,
            len,
            max: item.max_chars,
            paint: item.max_paint_chars,
            changed: false,
        }
    }

    /// The text up to the first NUL.
    fn text(&self) -> String {
        self.buf.iter().take_while(|&&c| c != '\0').collect()
    }

    /// Char 8 (0x4001271a): the character left of the cursor goes; the
    /// cvar is set either way.
    fn backspace(&mut self, pos: &mut EditPos) {
        if pos.cursor > 0 {
            let at = pos.cursor.min(self.len + 1);
            self.buf.copy_within(at..self.len + 1, at - 1);
            pos.cursor -= 1;
            if pos.cursor < pos.offset {
                pos.offset -= 1;
            }
        }
        self.changed = true;
    }

    /// Del (0x40012ac6): the character under the cursor goes.
    fn delete(&mut self, pos: EditPos) {
        if pos.cursor < self.len {
            self.buf
                .copy_within(pos.cursor + 1..self.len + 1, pos.cursor);
            self.changed = true;
        }
    }

    /// Left (0x40012a70).
    fn left(&self, pos: &mut EditPos) {
        pos.cursor = pos.cursor.saturating_sub(1);
        if pos.cursor < pos.offset {
            pos.offset -= 1;
        }
    }

    /// Right (0x40012a92): past `maxPaintChars` the paint offset follows.
    fn right(&self, pos: &mut EditPos) {
        if pos.cursor < self.len {
            if self.paint != 0 && pos.cursor >= self.paint {
                pos.offset += 1;
            }
            pos.cursor += 1;
        }
    }

    /// End (0x40012a47): with no `maxPaintChars` the offset goes to the
    /// end too, so the field paints nothing until Home or a click.
    fn end(&self, pos: &mut EditPos) {
        pos.cursor = self.len;
        if self.len > self.paint {
            pos.offset = self.len - self.paint;
        }
    }

    /// A typed character at the cursor (0x400127b6..0x40012845).
    fn type_char(&mut self, c: char, pos: &mut EditPos, overstrike: bool) -> Typed {
        if !overstrike {
            if self.len == EDIT_MAX || (self.max != 0 && self.len >= self.max) {
                return Typed::Full;
            }
            let at = pos.cursor.min(self.len + 1);
            self.buf.copy_within(at..self.len + 1, at + 1);
        } else if self.max != 0 && pos.cursor >= self.max {
            return Typed::AtMax;
        }
        let Some(slot) = self.buf.get_mut(pos.cursor) else {
            return Typed::Full;
        };
        *slot = c;
        self.changed = true;
        if pos.cursor < self.len + 1 {
            pos.cursor += 1;
            if self.paint != 0 && pos.cursor > self.paint {
                pos.offset += 1;
            }
        }
        Typed::Written
    }
}

/// What [`Field::type_char`] did with a character.
#[derive(PartialEq)]
enum Typed {
    Written,
    /// Outside overstrike the field holds `maxChars` (or 255) already.
    Full,
    /// In overstrike the cursor stands at `maxChars`.
    AtMax,
}

impl Ui {
    /// Whether a bind item is waiting for its key, which then goes to
    /// [`Ui::bind_key`] whatever it is (mouse buttons and the wheel too).
    pub fn waiting_for_key(&self) -> bool {
        self.options.waiting.is_some()
    }

    /// `set name "value"` through the console, readable at once.
    pub(super) fn set_cvar(&mut self, name: &str, value: &str, out: &mut Vec<UiEffect>) {
        let (name, value) = (bare(name), quotable(value));
        out.push(UiEffect::Command(format!("set {name} \"{value}\"")));
        self.options.pending.push((name, value));
    }

    /// `Item_HandleKey` for a click on an options control. False when the
    /// item takes no action on a click (an edit field starts editing
    /// instead).
    pub(super) fn option_click(
        &mut self,
        m: usize,
        i: usize,
        shell: &Shell,
        out: &mut Vec<UiEffect>,
    ) -> bool {
        let item = self.menus[m].items[i].clone();
        let Some(cvar) = item.cvar.clone() else {
            return true;
        };
        match item.kind {
            ITEM_TYPE_BIND => self.options.waiting = Some((m, i)),
            ITEM_TYPE_YESNO => {
                // `Item_YesNo_HandleKey`: `va("%i", !value)`.
                let on = self.cvar_value(&cvar, shell) != 0.0;
                self.set_cvar(&cvar, if on { "0" } else { "1" }, out);
            }
            ITEM_TYPE_MULTI => self.multi_step(&item, &cvar, shell, out),
            ITEM_TYPE_SLIDER => {
                if let Some(range) = item.slider {
                    let x = self.slider_x(m, i);
                    // `Item_Slider_HandleKey` (0x40012f40): inside the bar
                    // or half a thumb left of it, unclamped.
                    let c = self.cursor[0];
                    if c >= x - THUMB_W / 2.0 && c <= x + SLIDER_W {
                        let v = slider_value((c - x) / SLIDER_W, range.min, range.max);
                        self.set_cvar(&cvar, &format!("{v:.6}"), out);
                        self.options.drag = Some((m, i));
                    }
                }
            }
            k if super::is_edit_field(k) => {
                self.start_edit(m, i, true);
                return false;
            }
            _ => {}
        }
        true
    }

    /// `Item_Multi_HandleKey`: the entry after the one the cvar holds, or the
    /// second when it holds none of them (`Item_Multi_FindCvarByValue`
    /// answers 0 then).
    fn multi_step(&mut self, item: &UiItem, cvar: &str, shell: &Shell, out: &mut Vec<UiEffect>) {
        let cur = self.cvar(cvar, shell).unwrap_or_default();
        let (at, len) = if !item.str_list.is_empty() {
            let at = item
                .str_list
                .iter()
                .position(|(_, v)| v.eq_ignore_ascii_case(&cur));
            (at, item.str_list.len())
        } else {
            let f = cur.trim().parse::<f32>().unwrap_or(0.0);
            let at = item.float_list.iter().position(|(_, v)| *v == f);
            (at, item.float_list.len())
        };
        let Some(next) = step(at.unwrap_or(0), len, false) else {
            return;
        };
        let value = if item.str_list.is_empty() {
            item.float_list[next].1.to_string()
        } else {
            item.str_list[next].1.clone()
        };
        self.set_cvar(cvar, &value, out);
    }

    /// CoD2's `ITEM_TYPE_DVARENUM` arm of `Item_HandleKey` (Mac 0x16d720):
    /// the item's dvar to `%i` of the entry after the one it names (the
    /// one before on a right click), wrapping; an index in range or an
    /// entry's name in any case names one, anything else the first. An enum
    /// with no entries sets 0. Nothing moves when the index would not.
    pub(super) fn enum_step(
        &mut self,
        item: &UiItem,
        back: bool,
        shell: &Shell,
        out: &mut Vec<UiEffect>,
    ) {
        let (Some(cvar), Some(list)) = (item.cvar.as_deref(), item.enum_list.as_deref()) else {
            return;
        };
        let entries = shell.enum_domain(list).unwrap_or_default();
        let v = self.cvar(cvar, shell).unwrap_or_default();
        let at = enum_index(entries, &v, atoi_msvc);
        if let Some(next) = step(at, entries.len(), back)
            && next != at
        {
            self.set_cvar(cvar, &next.to_string(), out);
        }
    }

    /// A slider follows the mouse while its button is down, clamped to
    /// the bar (`0x40012d30`, the capture function).
    pub(super) fn drag_to(&mut self, shell: &Shell, out: &mut Vec<UiEffect>) {
        let Some((m, i)) = self.options.drag else {
            return;
        };
        let item = self.menus[m].items[i].clone();
        let (Some(cvar), Some(range)) = (item.cvar.as_ref(), item.slider) else {
            return;
        };
        let x = self.slider_x(m, i);
        let c = self.cursor[0].clamp(x, x + SLIDER_W);
        let v = slider_value((c - x) / SLIDER_W, range.min, range.max);
        if self.cvar(cvar, shell).and_then(|s| s.parse::<f32>().ok()) != Some(v) {
            self.set_cvar(cvar, &format!("{v:.6}"), out);
        }
    }

    /// Any key event the menus see (a press or a release, a mouse button
    /// or the wheel; never a repeat, which `CL_KeyEvent` keeps from the UI)
    /// ends a capture: `Item_HandleKey` (0x40013040) clears it first.
    pub fn release(&mut self) {
        self.options.drag = None;
        self.thumb_drag = None;
        self.arrow_hold = None;
    }

    /// A held slider, scroll bar thumb or scroll bar arrow.
    pub(super) fn capture_active(&self) -> bool {
        self.options.drag.is_some() || self.thumb_drag.is_some() || self.arrow_hold.is_some()
    }

    /// `Item_Bind_HandleKey` (0x40015410) while waiting: Escape cancels,
    /// Backspace unbinds the command's keys, any other key joins them. A
    /// command keeps two keys; a third replaces both.
    pub fn bind_key(&mut self, key: &str, shell: &Shell) -> Vec<UiEffect> {
        let mut out = Vec::new();
        self.options.new_input();
        let Some((m, i)) = self.options.waiting.take() else {
            return out;
        };
        let Some(cmd) = self.menus[m].items[i].cvar.clone() else {
            return out;
        };
        if !BIND_COMMANDS.iter().any(|c| c.eq_ignore_ascii_case(&cmd)) || key == "ESCAPE" {
            return out;
        }
        let bound = shell.keys_bound_to(&cmd);
        let (mut b1, mut b2) = (bound.first().copied(), bound.get(1).copied());
        let mut unbind = |k: &str| out.push(UiEffect::Command(format!("unbind {k}")));
        if key == "BACKSPACE" {
            b1.into_iter().chain(b2).for_each(&mut unbind);
            return out;
        }
        // The pressed key leaves every command first, this one included.
        if b2 == Some(key) {
            b2 = None;
        }
        if b1 == Some(key) {
            b1 = b2.take();
        }
        if let (Some(a), Some(b)) = (b1, b2) {
            unbind(a);
            unbind(b);
        }
        let cmd = quotable(&cmd);
        out.push(UiEffect::Command(format!("bind {key} \"{cmd}\"")));
        out
    }

    /// Whether an edit field is taking keys.
    pub fn editing(&self) -> bool {
        self.options.editing.is_some()
    }

    /// The cursor `Item_TextField_Paint` (0x4001473c..0x400147ac) draws in
    /// `menu`'s item `i` while an edit runs and the item has the focus: its
    /// index in the painted text and `_` in overstrike, `|` else. The
    /// renderer draws it on that character's spot (none past the painted
    /// text's end) every other 256 ms (CoDMP.exe 0x4d8110..0x4d81a3).
    pub(super) fn edit_cursor(&self, m: usize, i: usize) -> Option<(usize, char)> {
        if self.cod2.is_some() || !self.editing() || self.focus.get(&m) != Some(&i) {
            return None;
        }
        let pos = self.state[m][i].edit;
        let at = pos.cursor.checked_sub(pos.offset)?;
        let glyph = if self.options.overstrike { '_' } else { '|' };
        Some((at, glyph))
    }

    /// Starts an edit of `menu`'s item `i` as `Item_HandleKey` does
    /// (0x40013a27..0x40013a77): the cursor at 0 and overstrike on; a click
    /// zeroes the paint offset too, Enter keeps it.
    pub(super) fn start_edit(&mut self, m: usize, i: usize, click: bool) {
        let pos = &mut self.state[m][i].edit;
        pos.cursor = 0;
        if click {
            pos.offset = 0;
        }
        self.options.editing = Some((m, i));
        self.options.overstrike = true;
    }

    /// A key for the edit field (`Item_TextField_HandleKey`, 0x40012690,
    /// docs/research/cod11-front-end.md 20.5): typed text writes at the
    /// cursor, over the next character in overstrike, Backspace and Del
    /// delete, the arrows, Home and End move the cursor, Insert toggles
    /// overstrike, Enter and Escape end the edit, and Tab and the arrows up
    /// and down step the focus, taking the edit along to an edit field. The
    /// cvar follows every change.
    pub fn edit_key(&mut self, code: KeyCode, text: Option<&str>, shell: &Shell) -> Vec<UiEffect> {
        self.options.new_input();
        if self.cod2.is_some() {
            return self.edit_key_cod2(code, text, shell);
        }
        let mut out = Vec::new();
        let Some((m, i)) = self.options.editing else {
            return out;
        };
        let item = &self.menus[m].items[i];
        let accept = item.accept.clone();
        let Some(cvar) = item.cvar.clone() else {
            self.options.editing = None;
            return out;
        };
        let mut field = Field::read(&self.cvar(&cvar, shell).unwrap_or_default(), item);
        let mut pos = self.state[m][i].edit;
        // The keypad's keys only with Num Lock off, when they type nothing.
        let pad = text.is_none_or(str::is_empty);
        match code {
            KeyCode::Enter | KeyCode::NumpadEnter | KeyCode::Escape => {
                self.options.editing = None;
                // `accept` runs on Enter (1.1 0x40012914, CoD2 Mac 0x16ee5e).
                if code != KeyCode::Escape && !accept.is_empty() {
                    self.run(m, &accept, shell, &mut out);
                }
                return out;
            }
            KeyCode::Tab | KeyCode::ArrowDown | KeyCode::ArrowUp => {
                self.edit_step(code != KeyCode::ArrowUp, shell, &mut out);
                return out;
            }
            KeyCode::Numpad2 | KeyCode::Numpad8 if pad => {
                self.edit_step(code == KeyCode::Numpad2, shell, &mut out);
                return out;
            }
            KeyCode::Backspace => field.backspace(&mut pos),
            KeyCode::Delete => field.delete(pos),
            KeyCode::NumpadDecimal if pad => field.delete(pos),
            KeyCode::ArrowLeft => field.left(&mut pos),
            KeyCode::Numpad4 if pad => field.left(&mut pos),
            KeyCode::ArrowRight => field.right(&mut pos),
            KeyCode::Numpad6 if pad => field.right(&mut pos),
            KeyCode::Home => pos = EditPos::default(),
            KeyCode::Numpad7 if pad => pos = EditPos::default(),
            KeyCode::End => field.end(&mut pos),
            KeyCode::Numpad1 if pad => field.end(&mut pos),
            KeyCode::Insert => self.options.overstrike = !self.options.overstrike,
            KeyCode::Numpad0 if pad => self.options.overstrike = !self.options.overstrike,
            _ => {
                for c in text.unwrap_or("").chars() {
                    if !self.edit_char(c, shell, &mut out) {
                        break;
                    }
                }
                return out;
            }
        }
        self.state[m][i].edit = pos;
        if field.changed {
            self.set_cvar(&cvar, &field.text(), &mut out);
        }
        out
    }

    /// CoD2's edit key, vcod's own: typed text appends up to `maxChars`,
    /// Backspace deletes, Enter and Escape end the edit, and Tab and the
    /// arrows up and down step the focus, taking the edit along to an edit
    /// field. CoD2's handler (CoD2MP_s.exe 0x53d6e9..0x53dabd) moves its
    /// cursor unlike 1.1's (cod2-fonts-menus.md 4.2), so CoD2 keeps this
    /// until that is ported.
    fn edit_key_cod2(&mut self, code: KeyCode, text: Option<&str>, shell: &Shell) -> Vec<UiEffect> {
        let mut out = Vec::new();
        let Some((m, i)) = self.options.editing else {
            return out;
        };
        let item = &self.menus[m].items[i];
        // Q3's `Item_TextField_HandleKey`: a numeric field takes digits only.
        let numeric = item.kind == ITEM_TYPE_NUMERICFIELD;
        let upper = item.kind == super::ITEM_TYPE_UPREDITFIELD;
        let file_name = item.file_name_only;
        let accept = item.accept.clone();
        let goto_next = item.max_chars_goto_next;
        let (Some(cvar), max) = (item.cvar.clone(), item.max_chars) else {
            self.options.editing = None;
            return out;
        };
        let mut value = self.cvar(&cvar, shell).unwrap_or_default();
        match code {
            KeyCode::Enter | KeyCode::NumpadEnter | KeyCode::Escape => {
                self.options.editing = None;
                // CoD2's `accept` runs on Enter (Mac 0x16ee5e).
                if code != KeyCode::Escape && !accept.is_empty() {
                    self.run(m, &accept, shell, &mut out);
                }
                return out;
            }
            // The keypad's arrows only with Num Lock off, when they type
            // nothing.
            KeyCode::Tab
            | KeyCode::ArrowDown
            | KeyCode::ArrowUp
            | KeyCode::Numpad2
            | KeyCode::Numpad8
                if !matches!(code, KeyCode::Numpad2 | KeyCode::Numpad8)
                    || text.is_none_or(str::is_empty) =>
            {
                let down = !matches!(code, KeyCode::ArrowUp | KeyCode::Numpad8);
                self.edit_step(down, shell, &mut out);
                return out;
            }
            KeyCode::Backspace => {
                value.pop();
            }
            _ => {
                // A quote would end the `set` line's quoted value early.
                for c in text
                    .unwrap_or("")
                    .chars()
                    .filter(|c| !c.is_control() && *c != '"')
                    .filter(|c| !numeric || c.is_ascii_digit())
                    .filter(|&c| !file_name || c.is_ascii_alphanumeric() || c == '_' || c == '-')
                    .map(|c| if upper { c.to_ascii_uppercase() } else { c })
                {
                    if max == 0 || value.chars().count() < max {
                        value.push(c);
                        // `maxCharsGotoNext`: the character that fills the
                        // field steps on as Tab does (0x40012690).
                        if goto_next && value.chars().count() == max {
                            self.set_cvar(&cvar, &value, &mut out);
                            self.edit_step(true, shell, &mut out);
                            return out;
                        }
                    }
                }
            }
        }
        self.set_cvar(&cvar, &value, &mut out);
        out
    }

    /// One typed character for the edit field (the `K_CHAR_FLAG` arm of
    /// 0x40012690). False once the edit has ended.
    fn edit_char(&mut self, c: char, shell: &Shell, out: &mut Vec<UiEffect>) -> bool {
        let Some((m, i)) = self.options.editing else {
            return false;
        };
        let item = &self.menus[m].items[i];
        // A quote would end the `set` line's quoted value early.
        if c.is_control() || c == '"' {
            return true;
        }
        // A file-name field (CoD2) takes `I_isforfilename`'s characters.
        if item.file_name_only && !(c.is_ascii_alphanumeric() || c == '_' || c == '-') {
            return true;
        }
        // A numeric field's other character ends the edit (0x40012798).
        if item.kind == ITEM_TYPE_NUMERICFIELD && !c.is_ascii_digit() {
            self.options.editing = None;
            return false;
        }
        let c = match item.kind {
            super::ITEM_TYPE_UPREDITFIELD => c.to_ascii_uppercase(),
            _ => c,
        };
        let goto_next = item.max_chars_goto_next;
        let Some(cvar) = item.cvar.clone() else {
            return true;
        };
        let mut field = Field::read(&self.cvar(&cvar, shell).unwrap_or_default(), item);
        let mut pos = self.state[m][i].edit;
        let typed = field.type_char(c, &mut pos, self.options.overstrike);
        self.state[m][i].edit = pos;
        if typed == Typed::Written {
            self.set_cvar(&cvar, &field.text(), out);
        }
        // `maxCharsGotoNext`: a write that leaves the cursor at `maxChars`
        // steps on as Tab does, as does a character refused there in
        // overstrike (0x40012848, 0x40012958).
        let at_max = field.max > 0 && pos.cursor >= field.max;
        if goto_next && (typed == Typed::AtMax || (typed == Typed::Written && at_max)) {
            self.edit_step(true, shell, out);
        }
        true
    }

    /// The text an options control draws after its label: a list's entry,
    /// yes or no, the bound keys, the field's text, the bind status line.
    pub(super) fn option_value(
        &self,
        m: usize,
        i: usize,
        loc: &Localized,
        shell: &Shell,
    ) -> Option<String> {
        let item = &self.menus[m].items[i];
        if item.ownerdraw == UI_KEYBINDSTATUS {
            let key = if self.waiting_for_key() {
                "@EXE_KEYWAIT"
            } else {
                "@EXE_KEYCHANGE"
            };
            return Some(loc.translate(key).into_owned());
        }
        let cvar = item.cvar.as_deref()?;
        Some(match item.kind {
            // `Item_Multi_Setting`: an entry's label, or nothing.
            ITEM_TYPE_MULTI => {
                let v = self.cvar(cvar, shell).unwrap_or_default();
                let label = if item.str_list.is_empty() {
                    let f = v.trim().parse::<f32>().ok();
                    item.float_list
                        .iter()
                        .find(|(_, x)| Some(*x) == f)
                        .map(|(l, _)| l.as_str())
                } else {
                    item.str_list
                        .iter()
                        .find(|(_, x)| x.eq_ignore_ascii_case(&v))
                        .map(|(l, _)| l.as_str())
                };
                label.map_or(String::new(), |l| loc.translate(l).into_owned())
            }
            // `Item_Paint`'s `ITEM_TYPE_DVARENUM` arm (Mac 0x1704d6): the
            // entry the item's dvar names, blank for an empty enum.
            ITEM_TYPE_DVARENUM => match item.enum_list.as_deref() {
                None => "<dvarEnumList not set>".to_string(),
                Some(list) => match shell.enum_domain(list) {
                    None => "<not an enum dvar>".to_string(),
                    Some([]) => String::new(),
                    Some(e) => {
                        let v = self.cvar(cvar, shell).unwrap_or_default();
                        e[enum_index(e, &v, atoi_msvc)].clone()
                    }
                },
            },
            // `Item_TextField_Paint` (Mac 0x16994a): a decimal field shows
            // its float dvar through `Com_LocalizedFloatToString` at two
            // places.
            ITEM_TYPE_DECIMALFIELD => format!("{:.2}", self.cvar_value(cvar, shell)),
            ITEM_TYPE_YESNO => {
                let key = if self.cvar_value(cvar, shell) != 0.0 {
                    "@EXE_YES"
                } else {
                    "@EXE_NO"
                };
                loc.translate(key).into_owned()
            }
            ITEM_TYPE_BIND => {
                if self.options.waiting == Some((m, i)) {
                    return Some(String::new());
                }
                bind_text(cvar, loc, shell)
            }
            // `Item_TextField_Paint` (0x40014600): from the paint offset, at
            // most `maxPaintChars` (none with 0).
            // CoD2 keeps vcod's own look: the head, or while editing the
            // tail and a `|`.
            k if super::is_edit_field(k) && self.cod2.is_some() => {
                let v: Vec<char> = self.cvar(cvar, shell).unwrap_or_default().chars().collect();
                let n = if item.max_paint_chars == 0 {
                    v.len()
                } else {
                    item.max_paint_chars
                };
                if self.options.editing == Some((m, i)) {
                    let tail: String = v[v.len().saturating_sub(n)..].iter().collect();
                    format!("{tail}|")
                } else {
                    v.iter().take(n).collect()
                }
            }
            k if super::is_edit_field(k) => {
                let v = self.cvar(cvar, shell).unwrap_or_default();
                let from = v.chars().skip(self.state[m][i].edit.offset);
                match item.max_paint_chars {
                    0 => from.collect(),
                    n => from.take(n).collect(),
                }
            }
            _ => return None,
        })
    }

    /// The x the last frame drew a slider's bar at; the label's width it
    /// depends on needs the localized text and the font only `build` has.
    fn slider_x(&self, m: usize, i: usize) -> f32 {
        self.options
            .slider_x
            .borrow()
            .get(&(m, i))
            .copied()
            .unwrap_or(self.menus[m].items[i].rect[0])
    }

    /// `Item_Slider_Paint`'s bar and thumb.
    pub(super) fn slider_quads(
        &self,
        p: &Painter,
        at: (usize, usize),
        item: &UiItem,
        label_end: Option<f32>,
        color: [f32; 4],
        shell: &Shell,
        out: &mut Vec<HudQuad>,
    ) {
        let (Some(cvar), Some(range)) = (item.cvar.as_deref(), item.slider) else {
            return;
        };
        // 8 past the label's text rect, or the rect's left with no label.
        let x = label_end.map_or(item.rect[0], |end| end + 8.0);
        self.options.slider_x.borrow_mut().insert(at, x);
        let y = item.rect[1];
        p.quad([x, y, SLIDER_W, SLIDER_H], color, SLIDER_BAR, out);
        let v = self.cvar_value(cvar, shell);
        let span = range.max - range.min;
        let frac = if span > 0.0 {
            ((v.clamp(range.min, range.max) - range.min) / span).clamp(0.0, 1.0)
        } else {
            0.0
        };
        let thumb = thumb_x(x, frac);
        p.quad(
            [thumb - THUMB_W / 2.0, y - 2.0, THUMB_W, THUMB_H],
            color,
            SLIDER_THUMB,
            out,
        );
    }

    /// Q3's `atof` of a cvar as the menu sees it.
    fn cvar_value(&self, name: &str, shell: &Shell) -> f32 {
        self.cvar(name, shell)
            .and_then(|v| v.trim().parse().ok())
            .unwrap_or(0.0)
    }
}

/// `BindingFromName` (0x40014cf0): the first two keys bound to `cmd` as
/// "A or B", or "Unbound". Key names go through `KEY_*` in `key.str`.
fn bind_text(cmd: &str, loc: &Localized, shell: &Shell) -> String {
    let name = |k: &str| {
        loc.get(&format!("KEY_{k}"))
            .map_or_else(|| k.to_string(), str::to_string)
    };
    let in_table = BIND_COMMANDS.iter().any(|c| c.eq_ignore_ascii_case(cmd));
    let keys = if in_table {
        shell.keys_bound_to(cmd)
    } else {
        Vec::new()
    };
    match keys.as_slice() {
        [] => loc.translate("@KEY_UNBOUND").into_owned(),
        [a] => name(a),
        [a, b, ..] => format!("{} {} {}", name(a), loc.translate("@KEY_OR"), name(b)),
    }
}

#[cfg(test)]
mod tests {
    use std::time::Instant;

    use super::*;

    fn ui() -> Option<(Ui, Localized)> {
        let fs = vcod_common::testing::stock_fs()?;
        Some((Ui::new(&fs, "0"), Localized::load(&fs)))
    }

    /// Runs the console commands a menu asked for, as `main.rs` does.
    fn apply(shell: &mut Shell, effects: Vec<UiEffect>) {
        for e in effects {
            match e {
                UiEffect::Command(line) => {
                    shell.execute(&line);
                }
                UiEffect::ExecOnCvar {
                    cvar,
                    value,
                    int,
                    command,
                } => {
                    let cur = shell.cvar(&cvar).unwrap_or("").to_string();
                    if UiEffect::cvar_matches(&cur, value, int) {
                        shell.execute(&command);
                    }
                }
                _ => {}
            }
        }
    }

    fn item(ui: &Ui, menu: &str, text: &str) -> (usize, usize) {
        let m = ui.find(menu).unwrap();
        let i = ui.menus[m]
            .items
            .iter()
            .position(|i| i.text == text)
            .unwrap_or_else(|| panic!("{menu} has no {text}"));
        (m, i)
    }

    /// Moves to the item and clicks it, `dx` units in from its left.
    fn click_at(ui: &mut Ui, at: (usize, usize), dx: f32, shell: &mut Shell) {
        let r = ui.menus[at.0].items[at.1].rect;
        let e = ui.mouse_move(r[0] + dx, r[1] + 2.0, 640.0, 480.0, shell);
        apply(shell, e);
        assert_eq!(ui.hover, Some(at));
        let e = ui.click(Instant::now(), shell);
        apply(shell, e);
    }

    fn click(ui: &mut Ui, menu: &str, text: &str, shell: &mut Shell) {
        let at = item(ui, menu, text);
        click_at(ui, at, 2.0, shell);
    }

    fn open_names(ui: &Ui) -> Vec<&str> {
        ui.open.iter().map(|&m| ui.menus[m].name.as_str()).collect()
    }

    fn value(ui: &Ui, at: (usize, usize), loc: &Localized, shell: &Shell) -> String {
        ui.option_value(at.0, at.1, loc, shell).unwrap()
    }

    /// One edit field on `field` (type `kind`), with `extra` keywords.
    fn field_ui(kind: i32, extra: &str) -> (Ui, (usize, usize)) {
        let ui = crate::frontend::tests::ui_with_menu(&format!(
            r#"{{ menuDef {{ name "m" rect 0 0 640 480 visible 1
              itemDef {{ name f rect 100 100 200 20 type {kind} cvar "field" visible 1 {extra} }}
              itemDef {{ name b rect 100 200 200 20 type 1 text "B" visible 1 }} }} }}"#
        ));
        let m = ui.find("m").unwrap();
        (ui, (m, 0))
    }

    fn start(ui: &mut Ui, at: (usize, usize), value: &str) -> Shell {
        let mut shell = Shell::new();
        shell.force("field", value);
        ui.open_menu("m", &shell, &mut Vec::new());
        click_at(ui, at, 2.0, &mut shell);
        assert!(ui.editing());
        shell
    }

    fn keys(ui: &mut Ui, shell: &mut Shell, keys: &[(KeyCode, Option<&str>)]) {
        for &(code, text) in keys {
            let e = ui.edit_key(code, text, shell);
            apply(shell, e);
        }
    }

    fn pos(ui: &Ui, at: (usize, usize)) -> (usize, usize) {
        let p = ui.state[at.0][at.1].edit;
        (p.cursor, p.offset)
    }

    /// `Item_HandleKey` (0x40013a27..0x40013a77): an edit starts at the
    /// front in overstrike; Insert toggles to insert, which pushes the rest
    /// right.
    #[test]
    fn an_edit_starts_at_the_front_in_overstrike_and_insert_toggles_it() {
        let (mut ui, f) = field_ui(4, "");
        let mut shell = start(&mut ui, f, "vcod");
        assert_eq!(ui.edit_cursor(f.0, f.1), Some((0, '_')));
        keys(&mut ui, &mut shell, &[(KeyCode::KeyX, Some("x"))]);
        assert_eq!(shell.cvar("field"), Some("xcod"));
        keys(&mut ui, &mut shell, &[(KeyCode::Insert, None)]);
        assert_eq!(ui.edit_cursor(f.0, f.1), Some((1, '|')));
        keys(&mut ui, &mut shell, &[(KeyCode::KeyY, Some("y"))]);
        assert_eq!(shell.cvar("field"), Some("xycod"));
        // Overstrike outlives the edit; the next edit turns it on again.
        keys(&mut ui, &mut shell, &[(KeyCode::Escape, None)]);
        assert!(ui.edit_cursor(f.0, f.1).is_none());
        assert!(!ui.options.overstrike);
        click_at(&mut ui, f, 2.0, &mut shell);
        assert!(ui.options.overstrike);
    }

    /// The arrows, Home, End, Del and Backspace (0x4001271a, 0x40012a47..
    /// 0x40012ac6).
    #[test]
    fn the_cursor_moves_and_deletes_at_its_spot() {
        let (mut ui, f) = field_ui(4, "");
        let mut shell = start(&mut ui, f, "abcd");
        keys(
            &mut ui,
            &mut shell,
            &[(KeyCode::End, None), (KeyCode::ArrowRight, None)],
        );
        assert_eq!(pos(&ui, f).0, 4, "Right stops at the end");
        keys(
            &mut ui,
            &mut shell,
            &[(KeyCode::ArrowLeft, None), (KeyCode::Backspace, None)],
        );
        assert_eq!(shell.cvar("field"), Some("abd"));
        assert_eq!(pos(&ui, f).0, 2);
        keys(&mut ui, &mut shell, &[(KeyCode::Delete, None)]);
        assert_eq!(shell.cvar("field"), Some("ab"));
        keys(&mut ui, &mut shell, &[(KeyCode::Delete, None)]);
        assert_eq!(shell.cvar("field"), Some("ab"), "nothing under the cursor");
        keys(
            &mut ui,
            &mut shell,
            &[(KeyCode::Home, None), (KeyCode::ArrowLeft, None)],
        );
        assert_eq!(pos(&ui, f), (0, 0));
        // The keypad's keys move only with Num Lock off.
        keys(&mut ui, &mut shell, &[(KeyCode::Numpad6, None)]);
        assert_eq!(pos(&ui, f).0, 1);
        keys(&mut ui, &mut shell, &[(KeyCode::Numpad6, Some("6"))]);
        assert_eq!(shell.cvar("field"), Some("a6"));
    }

    /// `maxPaintChars` scrolls the painted text with the cursor; End on a
    /// field without it moves the paint offset past the text, which then
    /// paints nothing (0x40012a47, `Item_TextField_Paint` 0x400147b4).
    #[test]
    fn the_paint_offset_follows_the_cursor() {
        let (mut ui, f) = field_ui(4, "maxPaintChars 3");
        let mut shell = start(&mut ui, f, "abcdef");
        let loc = Localized::default();
        assert_eq!(value(&ui, f, &loc, &shell), "abc");
        keys(&mut ui, &mut shell, &[(KeyCode::End, None)]);
        assert_eq!(pos(&ui, f), (6, 3));
        assert_eq!(value(&ui, f, &loc, &shell), "def");
        assert_eq!(ui.edit_cursor(f.0, f.1), Some((3, '_')));
        keys(&mut ui, &mut shell, &[(KeyCode::ArrowLeft, None); 4]);
        assert_eq!(pos(&ui, f), (2, 2));
        keys(&mut ui, &mut shell, &[(KeyCode::ArrowRight, None); 2]);
        assert_eq!(pos(&ui, f), (4, 3), "past maxPaintChars the offset steps");
        // Enter's edit keeps the offset, a click's zeroes it.
        keys(&mut ui, &mut shell, &[(KeyCode::Enter, None)]);
        ui.start_edit(f.0, f.1, false);
        assert_eq!(pos(&ui, f), (0, 3));
        assert!(
            ui.edit_cursor(f.0, f.1).is_none(),
            "the cursor is left of the paint"
        );

        let (mut ui, f) = field_ui(4, "");
        let mut shell = start(&mut ui, f, "abc");
        keys(&mut ui, &mut shell, &[(KeyCode::End, None)]);
        assert_eq!(value(&ui, f, &loc, &shell), "");
        keys(&mut ui, &mut shell, &[(KeyCode::Escape, None)]);
        assert_eq!(value(&ui, f, &loc, &shell), "", "after the edit too");
    }

    /// Outside overstrike a full field drops a character; in it the cursor
    /// at `maxChars` does (0x400127ca, 0x40012958).
    #[test]
    fn max_chars_stops_both_modes_at_their_own_test() {
        let (mut ui, f) = field_ui(4, "maxChars 3");
        let mut shell = start(&mut ui, f, "ab");
        keys(&mut ui, &mut shell, &[(KeyCode::KeyX, Some("xyz"))]);
        assert_eq!(shell.cvar("field"), Some("xyz"));
        assert_eq!(pos(&ui, f).0, 3);
        keys(&mut ui, &mut shell, &[(KeyCode::KeyQ, Some("q"))]);
        assert_eq!(shell.cvar("field"), Some("xyz"));
        keys(
            &mut ui,
            &mut shell,
            &[(KeyCode::Home, None), (KeyCode::Insert, None)],
        );
        keys(&mut ui, &mut shell, &[(KeyCode::KeyQ, Some("q"))]);
        assert_eq!(shell.cvar("field"), Some("xyz"), "insert into a full field");
    }

    /// A numeric field's letter ends the edit (0x40012798).
    #[test]
    fn a_letter_ends_a_numeric_fields_edit() {
        let (mut ui, f) = field_ui(9, "");
        let mut shell = start(&mut ui, f, "12");
        keys(&mut ui, &mut shell, &[(KeyCode::Digit7, Some("7"))]);
        assert_eq!(shell.cvar("field"), Some("72"));
        keys(&mut ui, &mut shell, &[(KeyCode::KeyA, Some("a"))]);
        assert!(!ui.editing());
        assert_eq!(shell.cvar("field"), Some("72"));
    }

    /// CoD2 keeps vcod's own edit: typed text appends at the end, the
    /// field paints its tail and a `|`, and no cursor blinks
    /// (cod2-fonts-menus.md 4.2).
    #[test]
    fn a_cod2_edit_appends_at_the_end() {
        let (mut ui, f) = field_ui(4, "maxPaintChars 3");
        ui.cod2 = Some(Default::default());
        let mut shell = Shell::new();
        shell.force("field", "abcd");
        ui.open_menu("m", &shell, &mut Vec::new());
        ui.start_edit(f.0, f.1, true);
        keys(
            &mut ui,
            &mut shell,
            &[(KeyCode::KeyX, Some("x")), (KeyCode::Home, None)],
        );
        keys(&mut ui, &mut shell, &[(KeyCode::KeyY, Some("y"))]);
        assert_eq!(shell.cvar("field"), Some("abcdxy"));
        let loc = Localized::default();
        assert_eq!(value(&ui, f, &loc, &shell), "dxy|");
        assert!(ui.edit_cursor(f.0, f.1).is_none());
    }

    #[test]
    fn options_open_from_main_and_switch_pages() {
        let Some((mut ui, _)) = ui() else { return };
        let mut shell = Shell::new();
        let mut out = Vec::new();
        ui.open_main(&Shell::new(), &mut out);
        click(&mut ui, "main", "@MENU_OPTIONS", &mut shell);
        assert_eq!(open_names(&ui), ["options_menu", "options_look"]);
        // options_menu's own buttons sit beside the page on top of it.
        click(&mut ui, "options_menu", "@MENU_MOVE", &mut shell);
        assert_eq!(open_names(&ui), ["options_menu", "options_move"]);
        let (used, e) = ui.key(KeyCode::Escape, &shell);
        assert!(used);
        apply(&mut shell, e);
        assert_eq!(open_names(&ui), ["main"]);
    }

    #[test]
    fn a_bind_takes_two_keys_and_a_third_replaces_both() {
        let Some((mut ui, loc)) = ui() else { return };
        let mut shell = Shell::new();
        ui.open_main(&Shell::new(), &mut Vec::new());
        click(&mut ui, "main", "@MENU_OPTIONS", &mut shell);
        click(&mut ui, "options_menu", "@MENU_MOVE", &mut shell);
        let fwd = item(&ui, "options_move", "@MENU_FORWARD");
        assert_eq!(value(&ui, fwd, &loc, &shell), "W");

        click_at(&mut ui, fwd, 200.0, &mut shell);
        assert!(ui.waiting_for_key());
        let e = ui.bind_key("UPARROW", &shell);
        apply(&mut shell, e);
        assert!(!ui.waiting_for_key());
        assert_eq!(value(&ui, fwd, &loc, &shell), "W or Up Arrow");

        click_at(&mut ui, fwd, 200.0, &mut shell);
        let e = ui.bind_key("MOUSE3", &shell);
        apply(&mut shell, e);
        assert_eq!(value(&ui, fwd, &loc, &shell), "Middle Mouse");
        assert_eq!(shell.bind("W"), None);
        assert_eq!(shell.bind("UPARROW"), None);

        // A key bound elsewhere moves here; Escape cancels; Backspace clears.
        click_at(&mut ui, fwd, 200.0, &mut shell);
        let e = ui.bind_key("S", &shell);
        apply(&mut shell, e);
        assert_eq!(shell.bind("S"), Some("+forward"));
        let back = item(&ui, "options_move", "@MENU_BACKPEDAL");
        assert_eq!(value(&ui, back, &loc, &shell), "Unbound");
        click_at(&mut ui, fwd, 200.0, &mut shell);
        assert!(ui.bind_key("ESCAPE", &shell).is_empty());
        click_at(&mut ui, fwd, 200.0, &mut shell);
        let e = ui.bind_key("BACKSPACE", &shell);
        apply(&mut shell, e);
        assert_eq!(value(&ui, fwd, &loc, &shell), "Unbound");
    }

    #[test]
    fn invert_mouse_flips_m_pitch_and_the_slider_sets_sensitivity() {
        let Some((mut ui, loc)) = ui() else { return };
        let mut shell = Shell::new();
        ui.open_main(&Shell::new(), &mut Vec::new());
        click(&mut ui, "main", "@MENU_OPTIONS", &mut shell);
        let invert = item(&ui, "options_look", "@MENU_INVERT_MOUSE");
        assert_eq!(value(&ui, invert, &loc, &shell), "No");
        click_at(&mut ui, invert, 200.0, &mut shell);
        assert_eq!(shell.cvar("m_pitch"), Some("-0.022"));
        assert_eq!(value(&ui, invert, &loc, &shell), "Yes");
        click_at(&mut ui, invert, 200.0, &mut shell);
        assert_eq!(shell.cvar("m_pitch"), Some("0.022"));

        // The bar's x comes from the last frame drawn.
        ui.build(640.0, 480.0, &loc, &shell);
        let sens = item(&ui, "options_look", "@MENU_MOUSE_SENSITIVITY");
        let x = ui.slider_x(sens.0, sens.1);
        let rect = ui.menus[sens.0].items[sens.1].rect;
        assert!(x > rect[0] + 100.0, "bar after the label: {x}");
        // Half way along the 96-unit bar is half way from 1 to 30.
        click_at(&mut ui, sens, x + 48.0 - rect[0], &mut shell);
        assert!((shell.cvar_f32("sensitivity") - 15.5).abs() < 0.01);
        // Dragging past the end stops at the maximum.
        let e = ui.mouse_move(x + 200.0, rect[1] + 2.0, 640.0, 480.0, &shell);
        apply(&mut shell, e);
        assert!((shell.cvar_f32("sensitivity") - 30.0).abs() < 0.01);
        ui.release();
        let e = ui.mouse_move(x, rect[1] + 2.0, 640.0, 480.0, &shell);
        apply(&mut shell, e);
        assert!((shell.cvar_f32("sensitivity") - 30.0).abs() < 0.01);
    }

    #[test]
    fn only_typed_text_and_backspace_repeat_in_an_edit_field() {
        let Some((mut ui, loc)) = ui() else { return };
        let mut shell = Shell::new();
        shell.force("name", "vcod");
        let mut out = Vec::new();
        ui.open_main(&Shell::new(), &mut out);
        click(&mut ui, "main", "@MENU_MULTIPLAYER_OPTIONS", &mut shell);
        let field = item(&ui, "options_multi", "@MENU_PLAYER_NAME");
        click_at(&mut ui, field, 200.0, &mut shell);
        for (code, text) in [
            (KeyCode::Backspace, None),
            (KeyCode::KeyX, Some("x")),
            (KeyCode::KeyX, Some("x")),
            (KeyCode::Enter, Some("\r")),
            (KeyCode::ArrowDown, None),
            (KeyCode::Escape, None),
        ] {
            let e = ui.key_repeat(code, text, &shell);
            apply(&mut shell, e);
        }
        assert!(ui.editing(), "a repeated Enter, arrow or Esc ends nothing");
        // From the front in overstrike: Backspace at 0 deletes nothing.
        assert_eq!(value(&ui, field, &loc, &shell), "xxod");
    }

    #[test]
    fn the_name_field_edits_ui_name_and_closing_sets_name() {
        let Some((mut ui, loc)) = ui() else { return };
        let mut shell = Shell::new();
        shell.force("name", "vcod");
        let mut out = Vec::new();
        ui.open_main(&Shell::new(), &mut out);
        click(&mut ui, "main", "@MENU_MULTIPLAYER_OPTIONS", &mut shell);
        assert_eq!(open_names(&ui), ["main", "options_multi"]);
        assert_eq!(shell.cvar("ui_name"), Some("vcod"));
        let field = item(&ui, "options_multi", "@MENU_PLAYER_NAME");
        click_at(&mut ui, field, 200.0, &mut shell);
        assert!(ui.editing());
        for (code, text) in [
            (KeyCode::Backspace, None),
            (KeyCode::KeyX, Some("x")),
            (KeyCode::Quote, Some("\"")),
        ] {
            let e = ui.edit_key(code, text, &shell);
            apply(&mut shell, e);
        }
        assert_eq!(shell.cvar("ui_name"), Some("xcod"));
        assert_eq!(value(&ui, field, &loc, &shell), "xcod");
        let e = ui.edit_key(KeyCode::Enter, None, &shell);
        apply(&mut shell, e);
        assert!(!ui.editing());
        assert_eq!(
            shell.cvar("name"),
            Some("vcod"),
            "not until the menu closes"
        );
        // Main's onOpen closes options_multi, whose onClose copies it back.
        let mut out = Vec::new();
        ui.open_main(&Shell::new(), &mut out);
        apply(&mut shell, out);
        assert_eq!(shell.cvar("name"), Some("xcod"));
    }

    #[test]
    fn lists_step_through_float_and_string_values() {
        let Some((mut ui, loc)) = ui() else { return };
        let mut shell = Shell::new();
        ui.open_main(&Shell::new(), &mut Vec::new());
        click(&mut ui, "main", "@MENU_OPTIONS", &mut shell);
        click(&mut ui, "options_menu", "@MENU_GRAPHICS", &mut shell);
        // onOpen copied r_textureMode into ui_r_texturemode.
        let filter = item(&ui, "options_graphics", "@MENU_TEXTURE_FILTER");
        assert_eq!(value(&ui, filter, &loc, &shell), "Bilinear");
        click_at(&mut ui, filter, 200.0, &mut shell);
        assert_eq!(
            shell.cvar("ui_r_texturemode"),
            Some("GL_LINEAR_MIPMAP_LINEAR")
        );
        // r_mode -1 is none of the list's modes: blank, and a click picks
        // the second entry.
        let mode = item(&ui, "options_graphics", "@MENU_VIDEO_MODE");
        assert_eq!(value(&ui, mode, &loc, &shell), "");
        click_at(&mut ui, mode, 200.0, &mut shell);
        assert_eq!(shell.cvar("ui_r_mode"), Some("4"));
    }

    #[test]
    fn model_detail_round_trips_through_ui_lod() {
        let Some((mut ui, _)) = ui() else { return };
        let mut shell = Shell::new();
        ui.open_main(&Shell::new(), &mut Vec::new());
        click(&mut ui, "main", "@MENU_OPTIONS", &mut shell);
        click(&mut ui, "options_menu", "@MENU_PERFORMANCE", &mut shell);
        assert_eq!(shell.cvar("ui_lod"), Some("2"), "r_lodscale 1 reads Normal");
        let detail = item(&ui, "options_performance", "@MENU_MODEL_DETAIL");
        click_at(&mut ui, detail, 200.0, &mut shell);
        assert_eq!(shell.cvar("ui_lod"), Some("1"));
        let (_, e) = ui.key(KeyCode::Escape, &shell);
        apply(&mut shell, e);
        assert_eq!(shell.cvar("r_lodscale"), Some("0.5"));
    }

    #[test]
    fn reset_controls_execs_default_mp_cfg() {
        let Some((mut ui, _)) = ui() else { return };
        let mut shell = Shell::new();
        ui.open_main(&Shell::new(), &mut Vec::new());
        click(&mut ui, "main", "@MENU_OPTIONS", &mut shell);
        click(
            &mut ui,
            "options_menu",
            "@MENU_SET_DEFAULT_CONTROLS",
            &mut shell,
        );
        let m = ui.find("options_control_defaults").unwrap();
        let yes = (0..ui.menus[m].items.len())
            .find(|&i| {
                let it = &ui.menus[m].items[i];
                it.text == "@MENU_YES" && ui.shown(m, i, &shell)
            })
            .unwrap();
        let r = ui.menus[m].items[yes].rect;
        ui.mouse_move(r[0] + 2.0, r[1] + 2.0, 640.0, 480.0, &shell);
        let e = ui.click(Instant::now(), &shell);
        assert!(e.contains(&UiEffect::Command("exec default_mp.cfg".into())));
        assert_eq!(
            shell.execute("exec default_mp.cfg"),
            vec![crate::console::shell::Effect::Exec("default_mp.cfg".into())]
        );
    }
}
