//! The weapon-slot strip the cgame draws across the top right after a
//! weapon select (`cgame_mp_x86.dll` 0x30037790): each held slot's
//! `hudIcon` with its `weaponslot` key under it, the selected slot grown to
//! twice the size and the other modes of a select-fire gun below it. UO's
//! (`uo_cgame_mp_x86.dll` 0x30046bb0) draws its seven slots the same way.
//! docs/research/cod11-hud-protocol.md, "The weapon-slot strip".

use super::HudQuad;
use super::font::{self, Font};
use super::hudelem::Virtual;
use vcod_common::localize::Localized;
use vcod_common::weapon::{SLOT_NAMES, WeaponDef};
use vcod_common::weapon_table::alt_chain;

/// What the strip reads: the selection and the predicted playerstate's
/// weapons, and the select stamp's fade (the weapon name's, 0x30019a30 on
/// 0x3020c920 for 1800 ms).
pub struct StripView<'a> {
    pub selected: u8,
    /// `ps.weaponslots`, index = slot.
    pub slots: [u8; 8],
    /// `ps.weapons`.
    pub held: u64,
    pub weapons: &'a [Option<WeaponDef>],
    pub alpha: Option<f32>,
    /// United Offensive: slots 7..1, not 5..1, and a clock that never
    /// steps back.
    pub uo: bool,
}

impl StripView<'_> {
    /// The last slot drawn: 1.1's 5, UO's 7 (`satchel`, `binocular`).
    fn last_slot(&self) -> usize {
        if self.uo { 7 } else { 5 }
    }

    fn holds(&self, w: u8) -> bool {
        w < 64 && self.held & 1 << w != 0
    }

    fn def(&self, w: u8) -> Option<&WeaponDef> {
        self.weapons.get(usize::from(w)).and_then(Option::as_ref)
    }

    fn alt(&self, w: u8) -> u8 {
        self.def(w)
            .map_or(0, |d| u8::try_from(d.alt_index).unwrap_or(0))
    }

    fn wide(&self, w: u8) -> bool {
        self.def(w).is_some_and(|d| d.wide_list_icon)
    }

    fn icon(&self, w: u8) -> Option<&str> {
        self.def(w).and_then(|d| d.hud_icon.as_deref())
    }

    /// 0x3000f5c0 (UO 0x30011460) with the alt chain followed: the slot
    /// 1..=5 (UO 1..=7) holding `w` or one of its modes, 0 for none or a
    /// weapon not carried.
    fn slot_of(&self, w: u8) -> usize {
        if !self.holds(w) {
            return 0;
        }
        std::iter::once(w)
            .chain(alt_chain(w, |a| self.alt(a)))
            .find_map(|at| (1..=self.last_slot()).find(|&n| self.slots[n] == at))
            .unwrap_or(0)
    }

    /// How many modes `w`'s `altWeapon` chain runs through, `w` included.
    fn modes(&self, w: u8) -> usize {
        1 + alt_chain(w, |a| self.alt(a)).count()
    }
}

/// The strip's statics: each slot's grow fraction, the last draw time and
/// the alt-mode slide. Kept while the HUD hides, cleared on a gamestate
/// (0x3002ca60).
#[derive(Default)]
pub struct WeaponStrip {
    /// `0x3020c92c + slot * 4` (UO `0x3048ae50`), 0..1.
    scale: [f32; 8],
    /// `0x3020c944`.
    last_draw: i32,
    /// `0x30096c20`: +1 or -1 at a mode switch, back to 0 over 150 ms.
    alt_anim: f32,
    /// `0x300eef10`, `0x300eef14`.
    alt_slot: usize,
    alt_weapon: u8,
}

/// Grey for the slots not selected, white for the selected one and the
/// key names (traps 0x48 with 0.5 and 1.0).
fn grey(a: f32) -> [f32; 4] {
    [0.5, 0.5, 0.5, a]
}

fn white(a: f32) -> [f32; 4] {
    [1.0, 1.0, 1.0, a]
}

impl WeaponStrip {
    /// A gamestate: every slot shrunk and the clock reset.
    pub fn reset(&mut self) {
        *self = WeaponStrip::default();
    }

    /// One frame of the strip at `now` (ms). `key_text` is the key bound to
    /// a command, `None` while unbound; `font` the console font.
    #[allow(clippy::too_many_arguments)]
    pub fn build(
        &mut self,
        s: &StripView,
        now: i32,
        font: &Font,
        key_text: &dyn Fn(&str) -> Option<String>,
        loc: &Localized,
        v: &Virtual,
        out: &mut Vec<HudQuad>,
    ) {
        // The clock steps on every call, drawn or not; UO clamps a step back
        // to 0 (0x30046bce).
        let mut dt = (now - std::mem::replace(&mut self.last_draw, now)) as f32;
        if s.uo {
            dt = dt.max(0.0);
        }
        let Some(a) = s.alpha else { return };
        let sel = s.selected;
        let sel_slot = s.slot_of(sel);
        let mut x = 632.0;
        for slot in (1..=s.last_slot()).rev() {
            let w = s.slots[slot];
            if w == 0 || !s.holds(w) {
                x -= 32.0;
                self.scale[slot] = 0.0;
                x -= 2.0;
                continue;
            }
            let Some(icon) = s.icon(w) else {
                out.push(v.quad(x - 32.0, 8.0, 32.0, 32.0, grey(a), "hudNoWeaponIcon"));
                x -= 32.0;
                self.scale[slot] = 0.0;
                x -= 2.0;
                continue;
            };
            let scale = &mut self.scale[slot];
            if slot == sel_slot {
                if *scale < 1.0 && dt != 0.0 {
                    *scale = (*scale + dt * 0.01).min(1.0);
                }
                let scale = *scale;
                if s.modes(sel) == 1 {
                    self.alt_anim = 0.0;
                    self.alt_slot = slot;
                    self.alt_weapon = sel;
                    let h = (scale + 1.0) * 32.0;
                    let wd = if s.wide(w) { h * 2.0 } else { h };
                    x -= wd;
                    out.push(v.quad(x, 8.0, wd, h, white(a), icon));
                } else {
                    x = self.alt_modes(s, slot, w, icon, scale, dt, a, x, v, out);
                }
            } else {
                if *scale > 0.0 && dt != 0.0 {
                    *scale = (*scale - dt * 0.01).max(0.0);
                }
                let h = (*scale + 1.0) * 32.0;
                let wd = if s.wide(w) { h * 2.0 } else { h };
                x -= wd;
                out.push(v.quad(x, 8.0, wd, h, grey(a), icon));
            }
            slot_key(slot, x, a, font, key_text, loc, v, out);
            x -= 2.0;
        }
    }

    /// The selected slot of a select-fire gun: the mode in hand on top, the
    /// other modes under it, the two sliding past each other for 150 ms
    /// after a mode switch. Returns the strip's new left edge.
    #[allow(clippy::too_many_arguments)]
    fn alt_modes(
        &mut self,
        s: &StripView,
        slot: usize,
        w: u8,
        icon: &str,
        scale: f32,
        dt: f32,
        a: f32,
        x: f32,
        v: &Virtual,
        out: &mut Vec<HudQuad>,
    ) -> f32 {
        let sel = s.selected;
        let n = s.modes(sel);
        if self.alt_slot != slot {
            self.alt_slot = slot;
            self.alt_weapon = sel;
            self.alt_anim = 0.0;
        } else if self.alt_weapon != sel {
            self.alt_anim = if s.alt(self.alt_weapon) == sel {
                1.0
            } else {
                -1.0
            };
            self.alt_weapon = sel;
        }
        if self.alt_anim > 0.0 {
            self.alt_anim = (self.alt_anim - dt / 150.0).max(0.0);
        } else if self.alt_anim < 0.0 {
            self.alt_anim = (self.alt_anim + dt / 150.0).min(0.0);
        }
        let anim = self.alt_anim;
        let t = anim.abs();
        let hs = (scale + 1.0) * 32.0;
        let sel_w = if s.wide(w) { hs * 2.0 } else { hs };
        let cx = x - sel_w * 0.5;
        // The rows the modes sit on, 1 the top; row 0 is the colour's alpha,
        // which retail's stack holds below the array and a backward slide
        // reads (unreachable with the stock two-mode chains).
        let mut rows = vec![a, 8.0, hs + 10.0];
        while rows.len() <= n {
            let last = rows[rows.len() - 1];
            rows.push(last + 34.0);
        }
        // C's `%`, which keeps the sign.
        let row = |k: i32| rows[(1 + k % n as i32) as usize];
        let mut m = s.alt(sel);
        while m != 0 && m != sel {
            let (mut bw, mut bh) = (if s.wide(m) { 64.0 } else { 32.0 }, 32.0);
            let mut y = rows[2];
            if anim != 0.0 {
                y = rows[2] - (rows[2] - row(if anim > 0.0 { 2 } else { 0 })) * t;
                if anim > 0.0 || n == 2 {
                    let k = t * scale + 1.0;
                    bw *= k;
                    bh = 32.0 * k;
                }
            }
            if let Some(i) = s.icon(m) {
                out.push(v.quad(cx - bw * 0.5, y, bw, bh, grey(a), i));
            }
            m = s.alt(m);
        }
        let bw = if s.wide(w) { 64.0 } else { 32.0 };
        let (y, k) = if anim == 0.0 {
            (8.0, scale + 1.0)
        } else {
            let y = 8.0 - (8.0 - row(if anim > 0.0 { 1 } else { -1 })) * t;
            (y, (1.0 - t) * scale + 1.0)
        };
        out.push(v.quad(cx - k * bw * 0.5, y, k * bw, k * 32.0, white(a), icon));
        x - sel_w
    }
}

/// The key bound to `weaponslot <name>` under the slot's left edge
/// (0x30037620): the console font at 0.25 in 6-unit cells, a black copy a
/// unit down and right first.
#[allow(clippy::too_many_arguments)]
fn slot_key(
    slot: usize,
    x: f32,
    a: f32,
    font: &Font,
    key_text: &dyn Fn(&str) -> Option<String>,
    loc: &Localized,
    v: &Virtual,
    out: &mut Vec<HudQuad>,
) {
    let text = key_text(&format!("weaponslot {}", SLOT_NAMES[slot]))
        .unwrap_or_else(|| loc.translate("@KEY_UNBOUND").into_owned());
    let px_scale = 0.25 * v.scale / font.unit_scale();
    let top = 13.6 - font.max_height as f32 * font.glyph_scale * 0.25;
    for (dx, rgba) in [(1.0, [0.0, 0.0, 0.0, a]), (0.0, white(a))] {
        let [px, py] = v.point(x + dx, top + dx);
        font::layout_cells(font, &text, px, py, px_scale, 6.0 * v.scale, rgba, out);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn def(icon: &str, wide: bool, alt: usize) -> Option<WeaponDef> {
        Some(WeaponDef {
            hud_icon: Some(icon.into()),
            wide_list_icon: wide,
            alt_index: alt,
            ..WeaponDef::default()
        })
    }

    /// 1 a rifle, 2/3 a select-fire pair, 4 a pistol.
    fn table() -> Vec<Option<WeaponDef>> {
        vec![
            None,
            def("rifle", true, 0),
            def("auto", true, 3),
            def("semi", true, 2),
            def("pistol", false, 0),
        ]
    }

    fn view(weapons: &[Option<WeaponDef>], selected: u8, slots: [u8; 8]) -> StripView<'_> {
        StripView {
            selected,
            slots,
            held: 0b11110,
            weapons,
            alpha: Some(1.0),
            uo: false,
        }
    }

    /// configstring 7 `A B B` gives 1->2, 2->1, 3->1: the walks from 3
    /// loop without passing it, and end.
    #[test]
    fn alt_walks_end_on_a_loop_that_skips_the_start() {
        let w = vec![
            None,
            def("auto", true, 2),
            def("semi", true, 1),
            def("semi", true, 1),
            def("pistol", false, 0),
        ];
        let s = view(&w, 3, [0; 8]);
        assert_eq!(s.slot_of(3), 0);
        assert_eq!(s.modes(3), 3);
        let mut strip = WeaponStrip::default();
        quads(&mut strip, &s, 0);
    }

    fn quads(strip: &mut WeaponStrip, s: &StripView, now: i32) -> Vec<(String, [f32; 4])> {
        let v = Virtual::new((640.0, 480.0));
        let mut out = Vec::new();
        let loc = Localized::default();
        let font = crate::hud::hudelem::tests::test_font();
        strip.build(s, now, &font, &|_| None, &loc, &v, &mut out);
        out.into_iter()
            .filter(|q| q.texture != "fonts/test")
            .map(|q| {
                let [x0, y0] = q.verts[0];
                let [x1, y1] = q.verts[2];
                (q.texture, [x0, y0, x1 - x0, y1 - y0])
            })
            .collect()
    }

    /// Right to left from 632: an empty slot keeps 34 units, the selected
    /// one grows to double size in 100 ms, the rest stay 32 high in grey.
    #[test]
    fn the_strip_lays_slots_out_from_the_right() {
        let t = table();
        let s = view(&t, 4, [0, 1, 0, 4, 0, 0, 0, 0]);
        let mut strip = WeaponStrip::default();
        quads(&mut strip, &s, 10_000);
        let q = quads(&mut strip, &s, 10_100);
        // Slots 5 and 4 empty: 632 - 68 = 564. The pistol, selected, is
        // 64 square; slot 2 empty; the rifle 64x32 wide.
        assert_eq!(q[0], ("pistol".into(), [500.0, 8.0, 64.0, 64.0]));
        assert_eq!(q[1], ("rifle".into(), [400.0, 8.0, 64.0, 32.0]));
    }

    /// A mode switch slides the new mode up from the row below and the old
    /// one down into it over 150 ms, centred on the slot's box.
    #[test]
    fn a_mode_switch_slides_the_modes_past_each_other() {
        let t = table();
        let mut strip = WeaponStrip::default();
        let s = view(&t, 2, [0, 2, 0, 0, 0, 0, 0, 0]);
        quads(&mut strip, &s, 10_000);
        let idle = quads(&mut strip, &s, 10_200);
        // At rest, four empty slots in at 496: the mode in hand 128x64 on
        // top, the other 64x32 below at 64 + 10, both centred on 432.
        assert_eq!(idle[0], ("semi".into(), [400.0, 74.0, 64.0, 32.0]));
        assert_eq!(idle[1], ("auto".into(), [368.0, 8.0, 128.0, 64.0]));
        let s = view(&t, 3, [0, 3, 0, 0, 0, 0, 0, 0]);
        let start = quads(&mut strip, &s, 10_200);
        assert_eq!(start[0], ("auto".into(), [368.0, 8.0, 128.0, 64.0]));
        assert_eq!(start[1], ("semi".into(), [400.0, 74.0, 64.0, 32.0]));
        let half = quads(&mut strip, &s, 10_275);
        assert_eq!(half[1], ("semi".into(), [384.0, 41.0, 96.0, 48.0]));
        let done = quads(&mut strip, &s, 10_350);
        assert_eq!(done[1], ("semi".into(), [368.0, 8.0, 128.0, 64.0]));
    }

    /// UO's strip starts at slot 7: binoculars rightmost, then the satchel,
    /// then 5..1, and the binoculars' selection grows that slot.
    #[test]
    fn uo_draws_the_satchel_and_binocular_slots() {
        let mut t = table();
        t.push(def("satchel", false, 0));
        t.push(def("binoculars", false, 0));
        let slots = [0, 1, 0, 4, 0, 0, 5, 6];
        let mut s = view(&t, 6, slots);
        s.held = 0b111_1110;
        s.uo = true;
        let mut strip = WeaponStrip::default();
        quads(&mut strip, &s, 10_000);
        let q = quads(&mut strip, &s, 10_100);
        assert_eq!(q[0], ("binoculars".into(), [568.0, 8.0, 64.0, 64.0]));
        assert_eq!(q[1], ("satchel".into(), [534.0, 8.0, 32.0, 32.0]));
        // Slots 5 and 4 empty: 532 - 68 = 464.
        assert_eq!(q[2], ("pistol".into(), [432.0, 8.0, 32.0, 32.0]));
        // 1.1 stops at slot 5 and never draws the binoculars.
        s.uo = false;
        let q = quads(&mut strip, &s, 10_200);
        assert_eq!(q[0], ("pistol".into(), [532.0, 8.0, 32.0, 32.0]));
        assert!(q.iter().all(|(t, _)| t != "binoculars" && t != "satchel"));
    }

    /// Nothing draws once the select stamp's fade has run out, and a held
    /// weapon with no icon stands in as `hudNoWeaponIcon`.
    #[test]
    fn the_strip_fades_with_the_stamp_and_stands_in_for_a_missing_icon() {
        let mut t = table();
        t[4] = Some(WeaponDef::default());
        let mut s = view(&t, 1, [0, 1, 0, 4, 0, 0, 0, 0]);
        let mut strip = WeaponStrip::default();
        let q = quads(&mut strip, &s, 10_000);
        assert_eq!(q[0], ("hudNoWeaponIcon".into(), [532.0, 8.0, 32.0, 32.0]));
        s.alpha = None;
        assert!(quads(&mut strip, &s, 10_050).is_empty());
    }
}
