//! The cgame's centre print: the one line `CG_Obituary` raises when the
//! drawn player kills someone ("You killed X"). Layout, timing and the
//! obituary's rules are in docs/research/cod11-hud-protocol.md, section 1,
//! "The centre print".

use super::HudQuad;
use super::font::{Slot, UiFonts};
use super::hudelem::Virtual;
use super::player::{menu_text_rgba, text_height, text_width};

/// `cg_centertime`'s default, seconds.
const CENTER_TIME_S: i32 = 5;
/// The print's clock starts this far past the print, so it holds for
/// `cg_centertime` plus this.
const HOLD_MS: i32 = 2000;
/// `CG_FadeColor`'s fade over the last 100 ms.
const FADE_MS: i32 = 100;
/// A line wraps at its first space past this many characters, and draws at
/// most this many.
const LINE_CHARS: usize = 75;

/// `CG_Obituary`'s print: y 400 (UO 390), character width 9.6.
pub const OBITUARY_Y: i32 = 400;
pub const OBITUARY_Y_UO: i32 = 390;
const OBITUARY_CHAR_WIDTH: f32 = 9.6;
/// CoD2's `cg_centerPrintY` default (registered in `CG_Init`, Mac 0x145214).
const COD2_CENTER_Y: f32 = -60.0;

/// The text `CG_Obituary` formats for a kill by the drawn player, before
/// localization. `victim` is the victim's name, `teammate` a kill on the
/// killer's own (non-zero) team.
pub fn obituary_message(victim: &str, teammate: bool) -> String {
    // `strncpy(..., 0x1f)` then `"^7"`.
    let name: String = victim.chars().take(31).collect();
    if teammate {
        format!("CGAME_YOUKILLED\u{15}^1%s^7 {name}^7\u{14}CGAME_TEAMMATE")
    } else {
        format!("CGAME_YOUKILLED\u{15}{name}^7")
    }
}

#[derive(Default)]
pub struct CenterPrint {
    text: String,
    lines: i32,
    /// `cg.centerPrintTime`: the print's time plus [`HOLD_MS`]; 0 is none.
    start: i32,
    y: i32,
    char_width: i32,
    /// CoD2's layout: `cg_centerPrintY` (-60) up from the bottom, not `y`
    /// down from the top (cod2-hud.md 15).
    pub cod2: bool,
}

impl CenterPrint {
    /// `CG_PriorityCenterPrint` (0x300159e0) for an already localized `text`.
    pub fn print(&mut self, text: &str, y: i32, now_ms: i32) {
        self.text = wrap(text);
        self.lines = line_count(&self.text);
        self.start = now_ms + HOLD_MS;
        self.y = y;
        self.char_width = OBITUARY_CHAR_WIDTH as i32;
    }

    pub fn clear(&mut self) {
        self.start = 0;
    }

    /// `CG_DrawCenterString` (0x30015b40): each line centred on x 320, the
    /// block's baselines 1.2 text heights apart starting a height above half
    /// the block over `y`.
    pub fn build(&mut self, fonts: &UiFonts, now_ms: i32, v: &Virtual, out: &mut Vec<HudQuad>) {
        let Some(alpha) = self.alpha(now_ms) else {
            self.start = 0;
            return;
        };
        let scale = self.char_width as f32 * 0.03125;
        let font = fonts.pick(Slot::Default, scale, 480.0 * v.scale);
        // Traps 0x35 and 0x34 return ints.
        let h = text_height(font, scale).trunc();
        let mut y = if self.cod2 {
            480.0 + COD2_CENTER_Y - h * 1.2 * self.lines as f32
        } else {
            self.y as f32 - h * 0.5 * self.lines as f32 - h
        };
        for line in self.text.split('\n') {
            let line: String = line.chars().take(LINE_CHARS).collect();
            let w = text_width(font, &line, scale).trunc();
            let x = if self.cod2 {
                320.0 - w * 0.5
            } else {
                (640.0 - w) * 0.5
            };
            menu_text_rgba(font, &line, (x, y), scale, [1.0, 1.0, 1.0, alpha], v, out);
            y += h * 1.2;
        }
    }

    /// `CG_FadeColor(centerPrintTime, cg_centertime * 1000)` (0x30019a30).
    pub(super) fn alpha(&self, now_ms: i32) -> Option<f32> {
        if self.start == 0 {
            return None;
        }
        let total = CENTER_TIME_S * 1000;
        let t = now_ms - self.start;
        if t >= total {
            return None;
        }
        let left = total - t;
        Some(if left < FADE_MS {
            left as f32 * 0.01
        } else {
            1.0
        })
    }
}

/// A space becomes a line break once the line has run [`LINE_CHARS`]
/// characters.
fn wrap(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let (mut count, mut pending) = (0, false);
    for c in text.chars() {
        if c == '\n' {
            (count, pending) = (0, false);
            out.push(c);
            continue;
        }
        count += 1;
        pending |= count >= LINE_CHARS;
        if pending && c == ' ' {
            (count, pending) = (0, false);
            out.push('\n');
        } else {
            out.push(c);
        }
    }
    out
}

/// One line, plus one per `\n` and per literal `\` `n` pair.
fn line_count(text: &str) -> i32 {
    let mut lines = 1;
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\n' {
            lines += 1;
        } else if c == '\\' && chars.peek() == Some(&'n') {
            chars.next();
            lines += 1;
        }
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_print_holds_seven_seconds_and_fades_its_last_tenth() {
        let mut cp = CenterPrint::default();
        cp.print("You killed Bob^7", OBITUARY_Y, 1000);
        assert_eq!(cp.alpha(1000), Some(1.0));
        assert_eq!(cp.alpha(7900), Some(1.0));
        assert!((cp.alpha(7950).unwrap() - 0.5).abs() < 1e-6);
        assert_eq!(cp.alpha(8000), None);
    }

    #[test]
    fn a_long_line_breaks_at_the_first_space_past_75() {
        let text = format!("{} {} tail", "a".repeat(70), "b".repeat(10));
        let wrapped = wrap(&text);
        assert_eq!(
            wrapped,
            format!("{} {}\ntail", "a".repeat(70), "b".repeat(10))
        );
        assert_eq!(line_count(&wrapped), 2);
        assert_eq!(line_count("one\\ntwo"), 2);
    }

    #[test]
    fn the_obituary_message_appends_the_colour_reset() {
        assert_eq!(obituary_message("Bob", false), "CGAME_YOUKILLED\u{15}Bob^7");
        assert_eq!(
            obituary_message("Bob", true),
            "CGAME_YOUKILLED\u{15}^1%s^7 Bob^7\u{14}CGAME_TEAMMATE"
        );
        assert_eq!(obituary_message(&"x".repeat(40), false).len(), 16 + 31 + 2);
    }
}
