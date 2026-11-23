//! The `h`/`i` chat lines, top left, as `cgame_mp_x86.dll` keeps and draws
//! them: `CG_AddToTeamChat` (0x3002c920) and its draw (0x300150b0). Layout
//! and timing are in docs/research/cod11-chat.md.

use super::HudQuad;
use super::font::{Slot, TextStyle, UiFonts, trap_width};
use super::hudelem::Virtual;
use super::player::{menu_text_rgba, text_width};

/// The ring's size; `cg_chatHeight` (default 8) only lowers it.
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

/// The cgame's chat cvars, archived in every cgame's table (1.1 rows at
/// 0x30074ed4 and 0x30074ec4, 1.5 0x30076e94/0x30076e84, UO
/// 0x30085734/0x30085724), and whether the wrap is UO's
/// (cod11-chat.md 3.2). The HUD refreshes them each frame, as
/// `CG_UpdateCvars` does.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rules {
    /// `cg_chatHeight`: below 1 the chat is off and its ring empties.
    pub height: i32,
    /// `cg_chatTime`, ms: below 1 the same.
    pub time_ms: i32,
    /// UO's continuation opens with the message's first code too.
    pub uo: bool,
}

impl Default for Rules {
    fn default() -> Rules {
        Rules {
            height: CHAT_LINES as i32,
            time_ms: CHAT_TIME_MS,
            uo: false,
        }
    }
}

impl Rules {
    /// The cvars as their vmCvars' integers; an unset one keeps its default.
    pub fn read<'a>(cvar: impl Fn(&str) -> Option<&'a str>, uo: bool) -> Rules {
        let int =
            |name: &str, default: i32| cvar(name).map_or(default, vcod_common::text::atoi_msvc);
        Rules {
            height: int("cg_chatHeight", CHAT_LINES as i32),
            time_ms: int("cg_chatTime", CHAT_TIME_MS),
            uo,
        }
    }

    /// The ring's modulus, `min(cg_chatHeight, 8)`, `None` below 1: what
    /// both `CG_AddToTeamChat` and the draw index slots by, read afresh
    /// each call.
    fn height(&self) -> Option<i32> {
        (self.height >= 1).then(|| self.height.min(CHAT_LINES as i32))
    }
}

/// Whether `CG_ServerCommand` takes a chat line at all: a nonzero
/// `cg_teamChatsOnly` drops every `h` before it is localized, heard, kept
/// or printed, never an `i` (cod11-chat.md 3).
pub fn takes(team: bool, team_chats_only: Option<&str>) -> bool {
    team || team_chats_only.is_none_or(|v| vcod_common::text::atoi_msvc(v) == 0)
}

/// The chat ring as the cgame keeps it: 8 slots of 0x10f bytes
/// (0x301d51c0), each with the ms its line arrived (0x301d5a38), and two
/// counters that only count up, `head` the next line's (0x301d5a58) and
/// `tail` the oldest held (0x301d5a5c). A line `n` sits in slot
/// `n % min(cg_chatHeight, 8)`, the height read at each push and draw, so
/// lowering it under the lines held reads them back out of order
/// (cod11-chat.md 3.2).
#[derive(Default)]
pub struct Chat {
    slots: [(String, i32); CHAT_LINES],
    head: i32,
    tail: i32,
    rules: Rules,
}

impl Chat {
    pub fn new() -> Chat {
        Chat::default()
    }

    /// This frame's cvars and protocol.
    pub fn set_rules(&mut self, rules: Rules) {
        self.rules = rules;
    }

    /// One localized chat line: cut to 149 bytes, the `\x19`s out, wrapped
    /// at 90 visible characters on the last space, each continuation opened
    /// with the colour the line had reached. A height or time below 1
    /// zeroes both counters and keeps nothing (CG_AddToTeamChat's head).
    pub fn push(&mut self, text: &str, now_ms: i32) {
        let Some(h) = self.rules.height().filter(|_| self.rules.time_ms >= 1) else {
            self.head = 0;
            self.tail = 0;
            return;
        };
        let mut text: String = text.chars().filter(|&c| c != '\u{19}').collect();
        if text.len() > MAX_LINE {
            let mut end = MAX_LINE;
            while !text.is_char_boundary(end) {
                end -= 1;
            }
            text.truncate(end);
        }
        for line in wrap(&text, self.rules.uo) {
            self.slots[self.head.rem_euclid(h) as usize] = (line, now_ms);
            self.head += 1;
        }
        if self.head - self.tail > h {
            self.tail = self.head - h;
        }
    }

    /// Test hook; production reads go through [`Self::build`].
    #[cfg(test)]
    pub fn is_empty(&self) -> bool {
        self.head == self.tail
    }

    /// The draw's walk (0x300150b0): the tail steps past one line older
    /// than `cg_chatTime` a frame, then every line from the newest down to
    /// the tail as (row, text, arrival), row -1 the newest. A slot is read
    /// at this frame's height, whatever height wrote it.
    fn walk(&mut self, now_ms: i32) -> Vec<(i32, String, i32)> {
        let Some(h) = self.rules.height() else {
            return Vec::new();
        };
        if self.tail == self.head {
            return Vec::new();
        }
        let slot = |n: i32| n.rem_euclid(h) as usize;
        if self.rules.time_ms < now_ms - self.slots[slot(self.tail)].1 {
            self.tail += 1;
        }
        (self.tail..self.head)
            .rev()
            .map(|n| {
                let (line, t) = &self.slots[slot(n)];
                (n - self.head, line.clone(), *t)
            })
            .collect()
    }

    /// A line's alpha: 1, or `(cg_chatTime - age) * 0.005` inside its last
    /// 200 ms; `None` at 0 or less, which the draw skips.
    fn alpha(&self, now_ms: i32, t: i32) -> Option<f32> {
        let left = self.rules.time_ms as f32 - (now_ms - t) as f32;
        let alpha = if left > FADE_MS as f32 {
            1.0
        } else {
            left * (1.0 / FADE_MS as f32)
        };
        (alpha > 0.0).then_some(alpha)
    }

    /// Each live line from the newest up, 10 units apart with the newest's
    /// baseline at 83: a `hudColorBar` strip behind it in the viewer's team
    /// colour at a quarter, the speaker's name (up to the first `^7`) in that
    /// colour, the rest white, in style 3 (0x300152fb). `team_rgb` is the
    /// viewer's team colour.
    pub fn build(
        &mut self,
        fonts: &UiFonts,
        team_rgb: [f32; 3],
        now_ms: i32,
        v: &Virtual,
        out: &mut Vec<HudQuad>,
    ) {
        let font = fonts.pick(Slot::Default, SCALE, 480.0 * v.scale);
        for (row, line, t) in self.walk(now_ms) {
            let Some(alpha) = self.alpha(now_ms, t) else {
                continue;
            };
            let row = row as f32;
            let [r, g, b] = team_rgb;
            // Trap 0x34 returns an int (R_TextWidth, CoDMP.exe 0x4df310).
            let width = trap_width(font, &line, SCALE);
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
                let style = TextStyle::SHADOWED;
                menu_text_rgba(font, name, (x, baseline), SCALE, rgba, style, v, out);
                x += trap_width(font, name, SCALE);
            }
            menu_text_rgba(
                font,
                rest,
                (x, baseline),
                SCALE,
                [1.0, 1.0, 1.0, alpha],
                TextStyle::SHADOWED,
                v,
                out,
            );
        }
    }
}

impl Chat {
    /// CoD2's `CG_DrawChatMessages` (Mac 0x1cf0d0; cod2-hud.md 14): the
    /// newest line's strip at `cg_hudChatPosition`'s y (`pos`, 150 by
    /// default, truncated) less 10, each older one 10 above, left-aligned
    /// on the screen. The strip starts at x 0 and takes the colour of the
    /// line's leading `^N` (white without one) at a quarter; the text,
    /// codes and all, starts at the position's x truncated (5) in white.
    pub fn build_cod2(
        &mut self,
        fonts: &UiFonts,
        now_ms: i32,
        sp: &super::cod2elem::ScreenPlacement,
        pos: [f32; 2],
        out: &mut Vec<HudQuad>,
    ) {
        let x0 = pos[0].trunc();
        let y0 = pos[1].trunc();
        let screen_h = sp.real_size[1];
        let font = fonts.pick(Slot::Default, SCALE, screen_h);
        let k = sp.virtual_to_real[1];
        for (row, line, t) in self.walk(now_ms) {
            let Some(alpha) = self.alpha(now_ms, t) else {
                continue;
            };
            let line = line.as_str();
            let top = y0 + 10.0 * row as f32;
            let lead = line
                .strip_prefix('^')
                .and_then(|r| r.chars().next())
                .and_then(|c| c.to_digit(10))
                .map_or([1.0; 3], |d| {
                    let c = super::font::COLORS.get(d as usize).unwrap_or(&[1.0; 4]);
                    [c[0], c[1], c[2]]
                });
            let width = text_width(font, line, SCALE).trunc();
            let (x, w) = sp.axis(0, 0.0, width + 24.0, 1);
            let (y, h) = sp.axis(1, top, 10.0, 1);
            let rgba = [lead[0] * 0.25, lead[1] * 0.25, lead[2] * 0.25, alpha * 0.6];
            out.push(HudQuad::rect([x, y, w, h], rgba, "hudColorBar"));
            let (tx, _) = sp.axis(0, x0, 0.0, 1);
            let (ty, _) = sp.axis(1, top + 9.0 - 48.0 * SCALE, 0.0, 1);
            let px = SCALE / font.unit_scale() * k;
            let rgba = [1.0, 1.0, 1.0, alpha];
            // `UI_DrawText` style 3 (CoD2MP_s.exe 0x4c79ae).
            let style = TextStyle::COD2;
            super::font::layout_styled(font, line, tx, ty, px, rgba, style, k, out);
        }
    }
}

/// CoD2's `Con_DrawSay` (Mac 0x15d074; cod2-hud.md 14): `EXE_SAY: ` or
/// `EXE_SAYTEAM: ` at `cg_hudSayPosition` (`pos`, (5, 126) by default)
/// plus 24 down, the typed line after it, font 0 at 0.20833, left- and
/// top-aligned.
pub fn field_cod2(
    field: &super::ChatField,
    fonts: &UiFonts,
    loc: &vcod_common::localize::Localized,
    sp: &super::cod2elem::ScreenPlacement,
    pos: [f32; 2],
    out: &mut Vec<HudQuad>,
) {
    let key = if field.team { "EXE_SAYTEAM" } else { "EXE_SAY" };
    let label = format!("{}: ", loc.get(key).unwrap_or(key));
    let font = fonts.pick(Slot::Default, SCALE, sp.real_size[1]);
    let k = sp.virtual_to_real[1];
    let px = SCALE / font.unit_scale() * k;
    let (y, _) = sp.axis(1, pos[1] + 24.0, 0.0, 1);
    let white = [1.0; 4];
    let (x, _) = sp.axis(0, pos[0], 0.0, 1);
    // Style 3 through `R_AddCmdDrawText`, unsnapped (CoD2MP_s.exe 0x406dd4).
    let style = TextStyle::Cod2 {
        style: 3,
        snap: false,
    };
    super::font::layout_styled(font, &label, x, y, px, white, style, k, out);
    let lw = text_width(font, &label, SCALE).trunc();
    let (x, _) = sp.axis(0, pos[0] + lw, 0.0, 1);
    // `Field_Draw` (0x409b00) passes style 3 too (0x409c67).
    let text = format!("{}_", field.text);
    super::font::layout_styled(font, &text, x, y, px, white, style, k, out);
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
    // Both style 3 (CoDMP.exe 0x409e05, and 0x40c500 for font 0).
    let style = TextStyle::SHADOWED;
    menu_text_rgba(
        font,
        &label,
        (8.0, baseline),
        FIELD_SCALE,
        white,
        style,
        v,
        out,
    );
    let x = text_width(font, &label, FIELD_SCALE).trunc() + 8.0;
    let typed = format!("{}_", field.text);
    menu_text_rgba(
        font,
        &typed,
        (x, baseline),
        FIELD_SCALE,
        white,
        style,
        v,
        out,
    );
}

/// `CG_AddToTeamChat`'s line breaking. 1.5's (cgame 0x3002de50) and UO's
/// (0x30039390) take `^8`/`^9` as codes too
/// ([`super::font::is_colour_code`]). UO's continuation opens with `^` and
/// the message's first code ahead of the last one; with no code read yet
/// that first is byte 0xff, which no renderer reads as a code (`uo`).
fn wrap(text: &str, uo: bool) -> Vec<String> {
    let chars: Vec<char> = text.chars().collect();
    let mut lines = Vec::new();
    let mut line = String::new();
    let mut visible = 0;
    let mut colour = '7';
    let mut first = '\u{ff}';
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
            if uo {
                line.push('^');
                line.push(first);
            }
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
            Some(&d) if c == '^' && super::font::is_colour_code(d) => {
                line.push(c);
                line.push(d);
                colour = d;
                if first == '\u{ff}' {
                    first = d;
                }
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
        let lines = wrap(&format!("Bob^7: ^3{x88} tail"), false);
        assert_eq!(
            lines,
            [
                "Bob^7:".to_string(),
                format!("^3^3{x88}"),
                "^3tail".to_string()
            ]
        );
    }

    /// UO opens a continuation with the message's first code, then the
    /// last; byte 0xff while none was read (cod11-chat.md 3.2).
    #[test]
    fn a_uo_continuation_opens_with_the_first_and_the_last_code() {
        let x88 = "x".repeat(88);
        let lines = wrap(&format!("^1Bob^7: ^3{x88} tail"), true);
        assert_eq!(lines[1], format!("^1^3^3{x88}"));
        assert_eq!(lines[2], "^1^3tail");
        let x95 = "x".repeat(95);
        let lines = wrap(&x95, true);
        assert_eq!(lines, [x95[..90].to_string(), "^\u{ff}^7xxxxx".into()]);
        assert_eq!(wrap(&x95, false)[1], "^7xxxxx");
    }

    #[test]
    fn team_chats_only_drops_the_public_lines() {
        assert!(takes(false, None));
        assert!(takes(false, Some("0")));
        assert!(!takes(false, Some("1")));
        assert!(takes(true, Some("1")));
    }

    #[test]
    fn chat_height_and_time_cap_and_switch_off_the_ring() {
        let mut c = Chat::new();
        let cvars = |h: &'static str, t: &'static str| {
            move |n: &str| match n {
                "cg_chatHeight" => Some(h),
                "cg_chatTime" => Some(t),
                _ => None,
            }
        };
        assert_eq!(Rules::read(|_| None, false), Rules::default());
        c.set_rules(Rules::read(cvars("3", "12000"), false));
        for i in 0..5 {
            c.push(&format!("m{i}"), 0);
        }
        assert_eq!(texts(&mut c), ["m4", "m3", "m2"]);
        // Above 8 the ring still holds 8.
        c.set_rules(Rules::read(cvars("20", "12000"), false));
        for i in 0..10 {
            c.push(&format!("m{i}"), 0);
        }
        assert_eq!(texts(&mut c).len(), CHAT_LINES);
        // Off: the next line empties the ring and is not kept.
        c.set_rules(Rules::read(cvars("8", "0"), false));
        c.push("gone", 0);
        assert!(c.is_empty());
        c.set_rules(Rules::read(cvars("0", "12000"), false));
        c.push("gone", 0);
        assert!(c.is_empty());
    }

    #[test]
    fn chat_time_sets_the_fade() {
        let Some(fs) = vcod_common::testing::game_fs() else {
            return;
        };
        let fonts = UiFonts::load(&fs).unwrap();
        let v = Virtual::new((640.0, 480.0));
        let mut c = Chat::new();
        c.set_rules(Rules {
            time_ms: 1000,
            ..Rules::default()
        });
        c.push("hi", 0);
        let mut out = Vec::new();
        c.build(&fonts, [1.0; 3], 900, &v, &mut out);
        assert!((out[0].rgba[3] - 0.3).abs() < 1e-4);
        out.clear();
        c.build(&fonts, [1.0; 3], 1001, &v, &mut out);
        assert!(out.is_empty());
    }

    #[test]
    fn the_ring_keeps_eight_lines() {
        let mut c = Chat::new();
        for i in 0..10 {
            c.push(&format!("m{i}\u{19}"), 0);
        }
        let held = texts(&mut c);
        assert_eq!(held.len(), CHAT_LINES);
        assert_eq!(held[0], "m9");
        assert_eq!(held[7], "m2");
    }

    /// The draw's lines, newest first, at time 0.
    fn texts(c: &mut Chat) -> Vec<String> {
        c.walk(0).into_iter().map(|(_, l, _)| l).collect()
    }

    /// Lowering `cg_chatHeight` under the lines held reads the ring at the
    /// new modulus: retail's walk goes from the newest down to the tail it
    /// kept, slot `n % 3`, so lines repeat in a garbled order; the next
    /// push moves the tail up to the new height.
    #[test]
    fn a_lowered_height_reads_the_ring_at_its_new_modulus() {
        let mut c = Chat::new();
        for i in 0..8 {
            c.push(&format!("m{i}"), 0);
        }
        c.set_rules(Rules {
            height: 3,
            ..Rules::default()
        });
        let rows: Vec<(i32, String)> = c.walk(0).into_iter().map(|(r, l, _)| (r, l)).collect();
        let want = ["m1", "m0", "m2", "m1", "m0", "m2", "m1", "m0"];
        assert_eq!(rows.len(), 8);
        for (k, (row, line)) in rows.iter().enumerate() {
            assert_eq!(*row, -(k as i32) - 1);
            assert_eq!(line, want[k]);
        }
        // m8 lands in slot 8 % 3 = 2 and the tail moves to 9 - 3.
        c.push("m8", 0);
        assert_eq!(texts(&mut c), ["m8", "m1", "m0"]);
    }

    /// The tail steps past one expired line a frame, read at the slot the
    /// current height gives it.
    #[test]
    fn the_draw_drops_one_expired_line_a_frame() {
        let mut c = Chat::new();
        c.push("a", 0);
        c.push("b", 0);
        c.push("c", 5000);
        let t = CHAT_TIME_MS + 1;
        assert_eq!(c.walk(t).len(), 2);
        assert_eq!(c.walk(t).len(), 1);
        assert_eq!(c.walk(t).len(), 1);
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
