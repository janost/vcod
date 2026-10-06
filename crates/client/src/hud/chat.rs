//! The `h`/`i` chat lines, top left, as `cgame_mp_x86.dll` keeps and draws
//! them: `CG_AddToTeamChat` (0x3002c920) and its draw (0x300150b0). Layout
//! and timing are in docs/research/cod11-chat.md.

use super::HudQuad;
use super::font::{Slot, UiFonts};
use super::hudelem::Virtual;
use super::player::{menu_text_rgba, text_width};

/// `cg_chatHeight`'s default; the ring never holds more.
pub const CHAT_LINES: usize = 8;
/// `cg_chatTime`'s default, ms.
pub const CHAT_TIME_MS: i32 = 12000;
/// A line fades over the last 200 ms of its time.
const FADE_MS: i32 = 200;
/// Visible characters before a line wraps; colour codes do not count.
const WRAP: usize = 90;
/// The cgame copies 0x95 bytes of the localized line.
const MAX_LINE: usize = 149;
/// Text scale of font 0 (0x3e555555).
const SCALE: f32 = 0.208_333_33;

/// The chat ring, newest last, each line with the ms it arrived.
#[derive(Default)]
pub struct Chat {
    lines: Vec<(String, i32)>,
}

impl Chat {
    pub fn new() -> Chat {
        Chat::default()
    }

    /// One localized chat line: cut to 149 bytes, the `\x19`s out, wrapped
    /// at 90 visible characters on the last space, each continuation opened
    /// with the colour the line had reached.
    pub fn push(&mut self, text: &str, now_ms: i32) {
        let mut text: String = text.chars().filter(|&c| c != '\u{19}').collect();
        if text.len() > MAX_LINE {
            let mut end = MAX_LINE;
            while !text.is_char_boundary(end) {
                end -= 1;
            }
            text.truncate(end);
        }
        for line in wrap(&text) {
            self.lines.push((line, now_ms));
        }
        let excess = self.lines.len().saturating_sub(CHAT_LINES);
        self.lines.drain(..excess);
    }

    /// Test hook; production reads go through [`Self::build`].
    #[cfg(test)]
    pub fn is_empty(&self) -> bool {
        self.lines.is_empty()
    }

    /// Each live line from the newest up, 10 units apart with the newest's
    /// baseline at 83: a `hudColorBar` strip behind it in the viewer's team
    /// colour at a quarter, the speaker's name (up to the first `^7`) in that
    /// colour, the rest white. `team_rgb` is the viewer's team colour.
    pub fn build(
        &mut self,
        fonts: &UiFonts,
        team_rgb: [f32; 3],
        now_ms: i32,
        v: &Virtual,
        out: &mut Vec<HudQuad>,
    ) {
        // Retail drops one expired line a frame, oldest first.
        if self
            .lines
            .first()
            .is_some_and(|&(_, t)| now_ms - t > CHAT_TIME_MS)
        {
            self.lines.remove(0);
        }
        let font = fonts.pick(Slot::Default, SCALE, 480.0 * v.scale);
        for (k, (line, t)) in self.lines.iter().rev().enumerate() {
            let left = CHAT_TIME_MS - (now_ms - t);
            let alpha = if left > FADE_MS {
                1.0
            } else {
                left as f32 / FADE_MS as f32
            };
            if alpha <= 0.0 {
                continue;
            }
            let row = -(k as f32) - 1.0;
            let [r, g, b] = team_rgb;
            let width = text_width(font, line, SCALE);
            out.push(v.quad(
                0.0,
                row * 10.0 + 84.0,
                width + 24.0,
                10.0,
                [r * 0.25, g * 0.25, b * 0.25, alpha * 0.6],
                "hudColorBar",
            ));
            let baseline = row * 10.0 + 93.0;
            let (name, rest) = match line.find("^7") {
                Some(at) if at > 0 => line.split_at(at),
                _ => ("", line.as_str()),
            };
            let mut x = 8.0;
            if !name.is_empty() {
                let rgba = [r, g, b, alpha];
                menu_text_rgba(font, name, (x, baseline), SCALE, rgba, v, out);
                x += text_width(font, name, SCALE);
            }
            menu_text_rgba(
                font,
                rest,
                (x, baseline),
                SCALE,
                [1.0, 1.0, 1.0, alpha],
                v,
                out,
            );
        }
    }
}

/// The chat field while `messagemode` is open (CoDMP.exe 0x409d80, from
/// the cgame's trap 0x1e at y 100): `EXE_SAY:` or `EXE_SAYTEAM:` at x 8,
/// the typed line after it, both at a third of font 0.
pub fn field(
    field: &super::ChatField,
    fonts: &UiFonts,
    loc: &vcod_common::localize::Localized,
    v: &Virtual,
    out: &mut Vec<HudQuad>,
) {
    const FIELD_SCALE: f32 = 1.0 / 3.0;
    let key = if field.team { "EXE_SAYTEAM" } else { "EXE_SAY" };
    let label = format!("{}:", loc.get(key).unwrap_or(key));
    let font = fonts.pick(Slot::Default, FIELD_SCALE, 480.0 * v.scale);
    let baseline = 116.0;
    let white = [1.0; 4];
    menu_text_rgba(font, &label, (8.0, baseline), FIELD_SCALE, white, v, out);
    let x = text_width(font, &label, FIELD_SCALE).trunc() + 8.0;
    let typed = format!("{}_", field.text);
    menu_text_rgba(font, &typed, (x, baseline), FIELD_SCALE, white, v, out);
}

/// `CG_AddToTeamChat`'s line breaking.
fn wrap(text: &str) -> Vec<String> {
    let chars: Vec<char> = text.chars().collect();
    let mut lines = Vec::new();
    let mut line = String::new();
    let mut visible = 0;
    let mut colour = '7';
    // Where in `line` and `chars` the last space was.
    let mut last_space: Option<(usize, usize)> = None;
    let mut i = 0;
    while i < chars.len() {
        if visible >= WRAP {
            if let Some((at, from)) = last_space {
                line.truncate(at);
                i = from + 1;
            }
            lines.push(std::mem::take(&mut line));
            line.push('^');
            line.push(colour);
            visible = 0;
            last_space = None;
            if i >= chars.len() {
                break;
            }
        }
        let c = chars[i];
        match chars.get(i + 1) {
            Some(&d) if c == '^' && ('0'..='7').contains(&d) => {
                line.push(c);
                line.push(d);
                colour = d;
                i += 2;
            }
            _ => {
                if c == ' ' {
                    last_space = Some((line.len(), i));
                }
                line.push(c);
                visible += 1;
                i += 1;
            }
        }
    }
    lines.push(line);
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The break goes back to the last space however far back it is, and
    /// the text after it is read again onto the next line.
    #[test]
    fn a_long_line_breaks_on_its_last_space_and_keeps_its_colour() {
        let x88 = "x".repeat(88);
        let lines = wrap(&format!("Bob^7: ^3{x88} tail"));
        assert_eq!(
            lines,
            [
                "Bob^7:".to_string(),
                format!("^3^3{x88}"),
                "^3tail".to_string()
            ]
        );
    }

    #[test]
    fn the_ring_keeps_eight_lines() {
        let mut c = Chat::new();
        for i in 0..10 {
            c.push(&format!("m{i}\u{19}"), 0);
        }
        assert_eq!(c.lines.len(), CHAT_LINES);
        assert_eq!(c.lines[0].0, "m2");
    }

    #[test]
    fn a_line_fades_out_over_its_last_200_ms() {
        let Some(fs) = vcod_common::testing::game_fs() else {
            return;
        };
        let fonts = UiFonts::load(&fs).unwrap();
        let v = Virtual::new((640.0, 480.0));
        let mut c = Chat::new();
        c.push("Bob^7: ^7hi", 0);
        let mut out = Vec::new();
        c.build(&fonts, [1.0; 3], CHAT_TIME_MS - 100, &v, &mut out);
        // The strip, then shadow and glyph per character.
        assert_eq!(out[0].texture, "hudColorBar");
        assert!((out[0].rgba[3] - 0.3).abs() < 1e-4);
        assert!((out.last().unwrap().rgba[3] - 0.5).abs() < 1e-4);
        out.clear();
        c.build(&fonts, [1.0; 3], CHAT_TIME_MS + 1, &v, &mut out);
        assert!(out.is_empty());
    }
}
