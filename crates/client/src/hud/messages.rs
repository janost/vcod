//! The engine's two message windows the cgame draws: game messages (`e`,
//! `f`, `iPrintLn`) above the compass and bold messages (`c`, `g`,
//! `iPrintLnBold`, `announcement`) centred over the crosshair. CoDMP.exe
//! keeps both as console-line windows (0x408be0, 0x409920); the layout and
//! timing are in docs/research/cod11-chat.md. Kill lines (the cgame's trap
//! 4) go into the game window: docs/research/cod11-hud-protocol.md 1.1.

use super::HudQuad;
use super::font::{Slot, UiFonts};
use super::hudelem::Virtual;
use super::player::{menu_text_rgba, text_width};

/// The ring each window keeps.
const SLOTS: usize = 8;
/// How many of the oldest slots a new line forces into their fade.
const FORCED: usize = 3;
/// A new line slides the window up over this long, and fades in over it.
const SLIDE_MS: i32 = 250;
const FADE_IN_MS: i32 = 250;
const FADE_OUT_MS: i32 = 500;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    /// `con_gamemessagetime` 5 s, at (6, 357), lines 12 apart.
    Game,
    /// `con_boldgamemessagetime` 8 s, centred on x 320 above y 180, lines 16
    /// apart.
    Bold,
    /// CoD2's subtitles (`Con_DrawSubtitles`, Mac 0x15dbd4): above y 400
    /// at x 120 (`cg_subtitlePosX`, `cg_subtitlePosY`), lines 12 apart
    /// (`cg_subtitleCharHeight`), each line on for the time its print
    /// asks ([`Window::push_for`]), 5 s when it asks none.
    Subtitle,
}

impl Kind {
    fn life_ms(self) -> i32 {
        match self {
            Kind::Game | Kind::Subtitle => 5000,
            Kind::Bold => 8000,
        }
    }

    fn line_height(self) -> f32 {
        match self {
            Kind::Game | Kind::Subtitle => 12.0,
            Kind::Bold => 16.0,
        }
    }

    /// The cvar that sets the characters a line holds before it wraps,
    /// and its default: `cg_gameMessageWidth` 70 and
    /// `cg_gameBoldMessageWidth` 60, archived (cgame cvar table, 1.1
    /// 0x300750c0, 1.5 0x300770b0, UO 0x30085950; cod11-chat.md 3.3).
    pub fn width_cvar(self) -> (&'static str, usize) {
        match self {
            Kind::Game => ("cg_gameMessageWidth", 70),
            Kind::Bold => ("cg_gameBoldMessageWidth", 60),
            Kind::Subtitle => ("cg_subtitleWidthStandard", 80),
        }
    }

    /// Font 0 at 0.25, or font 4 at a third.
    fn font<'a>(self, fonts: &'a UiFonts, v: &Virtual) -> (&'a super::font::Font, f32) {
        let (slot, scale) = match self {
            Kind::Game | Kind::Subtitle => (Slot::Default, 0.25),
            Kind::Bold => (Slot::BigFixed, 1.0 / 3.0),
        };
        (fonts.pick(slot, scale, 480.0 * v.scale), scale)
    }
}

/// One obituary as trap 4 writes it: the attacker's name and a space in
/// its team colour, the icon, then a space and the victim's name in theirs.
/// Names are already `killfeed::line_name`d.
#[derive(Clone, Debug, PartialEq)]
pub struct KillLine {
    pub attacker: Option<(String, [f32; 3])>,
    pub icon: String,
    /// The weapon's `wideKillIcon`: trap 4's width 2.8, else 1.4.
    pub icon_wide: bool,
    /// CoD2's `flipKillIcon`: the icon drawn mirrored left to right
    /// (`R_DrawConsoleText`, Mac 0xece9a). 1.1 has no such key.
    pub icon_flip: bool,
    pub victim: (String, [f32; 3]),
}

/// The icon's width and height codes, trap 4's 1.4 and 2.8 times 32
/// truncated (CoDMP.exe 0x4021fd).
const ICON_CODE: i32 = 44;
const WIDE_ICON_CODE: i32 = 89;

#[derive(Clone)]
enum Body {
    Text(String),
    Kill(KillLine),
}

#[derive(Clone)]
struct Line {
    body: Body,
    start: i32,
    end: i32,
}

/// One window's ring. A slot with `start` 0 is empty.
pub struct Window {
    kind: Kind,
    /// Characters a line holds, [`line_width`] of the kind's cvar.
    pub width: usize,
    slots: [Option<Line>; SLOTS],
    current: usize,
    /// Where the newest line's baseline goes, in place of the kind's own
    /// (CoD2's game window, cod2-hud.md 14).
    pub origin: Option<(f32, f32)>,
}

impl Window {
    pub fn new(kind: Kind) -> Window {
        Window {
            kind,
            width: kind.width_cvar().1,
            slots: Default::default(),
            current: 0,
            origin: None,
        }
    }

    /// A localized message as traps 2 and 3 write it (CoDMP.exe 0x401e52,
    /// 0x401fc2): every `\n` ends a line, an empty one too, and each part
    /// wraps at the window's width. The colour runs on across parts.
    pub fn push(&mut self, text: &str, now_ms: i32) {
        self.push_for(text, now_ms, self.kind.life_ms());
    }

    /// [`Self::push`] with each line on for `life_ms` (`CL_ConsolePrint`'s
    /// duration, Mac 0x15f344); 0 or less takes the kind's.
    pub fn push_for(&mut self, text: &str, now_ms: i32, life_ms: i32) {
        let life = if life_ms > 0 {
            life_ms
        } else {
            self.kind.life_ms()
        };
        let mut colour = '7';
        let parts: Vec<&str> = text.split('\n').collect();
        for (i, part) in parts.iter().enumerate() {
            if part.is_empty() {
                if i + 1 < parts.len() {
                    self.add_line(Body::Text(String::new()), now_ms, life);
                }
                continue;
            }
            for line in wrap(part, self.width, &mut colour) {
                self.add_line(Body::Text(line), now_ms, life);
            }
        }
    }

    /// A kill line, on `con_gamemessagetime` like any game message.
    pub fn push_kill(&mut self, line: KillLine, now_ms: i32) {
        self.add_line(Body::Kill(line), now_ms, self.kind.life_ms());
    }

    #[cfg(test)]
    pub fn kill_lines(&self) -> Vec<&KillLine> {
        self.slots
            .iter()
            .flatten()
            .filter_map(|l| match &l.body {
                Body::Kill(k) => Some(k),
                Body::Text(_) => None,
            })
            .collect()
    }

    #[cfg(test)]
    pub fn is_empty(&self) -> bool {
        self.slots.iter().all(Option::is_none)
    }

    pub fn clear(&mut self) {
        self.slots = Default::default();
        self.current = 0;
    }

    /// `0x408be0`: the line takes the current slot, and the three oldest
    /// that would outlive the next 500 ms start their fade now.
    fn add_line(&mut self, body: Body, now: i32, life_ms: i32) {
        self.slots[self.current] = Some(Line {
            body,
            start: now,
            end: now + life_ms,
        });
        self.current = (self.current + 1) % SLOTS;
        for i in 0..FORCED {
            if let Some(l) = &mut self.slots[(self.current + i) % SLOTS]
                && now < l.end - FADE_OUT_MS
            {
                l.start += FADE_OUT_MS - l.end + now;
                l.end = now + FADE_OUT_MS;
            }
        }
    }

    /// `0x409920`: newest at the bottom, each older line one line height
    /// above, the stack pushed down while a new line slides in.
    pub fn build(&mut self, fonts: &UiFonts, now: i32, v: &Virtual, out: &mut Vec<HudQuad>) {
        let h = self.kind.line_height();
        let (x0, mut y) = self.origin.unwrap_or(match self.kind {
            Kind::Game => (6.0, 357.0),
            Kind::Bold => (320.0, 180.0),
            Kind::Subtitle => (120.0, 400.0),
        });
        for l in self.slots.iter().flatten() {
            if (0..SLIDE_MS).contains(&(now - l.start)) {
                y += ((1.0 - (now - l.start) as f32 / SLIDE_MS as f32) * h).round();
            }
        }
        let (font, scale) = self.kind.font(fonts, v);
        for k in 1..=SLOTS {
            let slot = &mut self.slots[(self.current + SLOTS - k) % SLOTS];
            let Some(l) = slot else { continue };
            if now >= l.end {
                *slot = None;
                continue;
            }
            let age = now - l.start;
            let alpha = if age < FADE_IN_MS {
                age.max(0) as f32 / FADE_IN_MS as f32
            } else if l.end - now < FADE_OUT_MS {
                (l.end - now) as f32 / FADE_OUT_MS as f32
            } else {
                1.0
            };
            y -= h;
            let text = match &l.body {
                Body::Text(text) => text,
                Body::Kill(k) => {
                    kill_line(k, font, scale, (x0, y + 12.0), alpha, v, out);
                    continue;
                }
            };
            let x = match self.kind {
                Kind::Game | Kind::Subtitle => x0,
                Kind::Bold => x0 - (text_width(font, text, scale) / 2.0).trunc(),
            };
            menu_text_rgba(
                font,
                text,
                (x, y + 12.0),
                scale,
                [1.0, 1.0, 1.0, alpha],
                v,
                out,
            );
        }
    }
}

/// The width 0x408f80 wraps at: the cvar's integer, or the console's
/// line width (0x4088f0: `max(vidWidth, 640) / 8 - 2`) when it is under 1
/// or over that.
pub fn line_width(cvar: i32, vid_width: u32) -> usize {
    let console = vid_width.max(640) as i32 / 8 - 2;
    (if cvar < 1 || cvar > console {
        console
    } else {
        cvar
    }) as usize
}

/// CoDMP.exe 0x408f80 for the game and bold windows: one part of a message
/// cut into console lines of at most `limit` characters. A part longer than
/// `limit` (colour codes not counted, 0x4aa6c0; `^8`/`^9` on 1.5 and UO,
/// 0x409760 and 0x409dc0) is balanced over
/// `ceil(len / limit)` lines: a line also ends after a space once it holds
/// `len / lines`. Before each character the rest of its word (bytes above a
/// space, at most `limit`) moves to a new line when it would overrun, unless
/// it fills a whole line; so a word longer than `limit` breaks where its
/// last `limit - 1` characters start. A space opening a line after a wrap
/// is dropped. `colour` is the running `^N`, which each line reopens with
/// (cod11-chat.md 3.3).
fn wrap(text: &str, limit: usize, colour: &mut char) -> Vec<String> {
    let s: Vec<char> = text.chars().collect();
    let code = |i: usize| {
        (s[i] == '^')
            .then(|| s.get(i + 1).copied())
            .flatten()
            .filter(|&c| super::font::is_colour_code(c))
    };
    let mut len = 0;
    let mut i = 0;
    while i < s.len() {
        if code(i).is_some() {
            i += 2;
            continue;
        }
        len += usize::from(s[i] != '\n' && s[i] != '\r');
        i += 1;
    }
    let soft = if len > limit {
        len / len.div_ceil(limit)
    } else {
        limit
    };

    let mut lines = Vec::new();
    let mut cells: Vec<(char, char)> = Vec::new();
    let mut col = 0;
    let mut end_line = |cells: &mut Vec<(char, char)>, col: &mut usize| {
        let mut line = String::new();
        let mut drawn = '7';
        let used = cells.iter().rposition(|c| c.0 != ' ').map_or(0, |n| n + 1);
        for &(ch, c) in &cells[..used] {
            if c != drawn {
                line.push('^');
                line.push(c);
                drawn = c;
            }
            line.push(ch);
        }
        lines.push(line);
        cells.clear();
        *col = 0;
    };
    let mut wrapped = false;
    let mut i = 0;
    while i < s.len() {
        if let Some(c) = code(i) {
            *colour = c;
            i += 2;
            continue;
        }
        let ch = s[i];
        let word = s[i..].iter().take(limit).take_while(|&&c| c > ' ').count();
        if word != limit && col + word > limit {
            end_line(&mut cells, &mut col);
            wrapped = true;
        }
        i += 1;
        match ch {
            '\n' => end_line(&mut cells, &mut col),
            '\r' => col = 0,
            ' ' if col == 0 && wrapped => {}
            _ => {
                if col < cells.len() {
                    cells[col] = (ch, *colour);
                } else {
                    cells.push((ch, *colour));
                }
                col += 1;
                if col >= limit || (col >= soft && ch == ' ') {
                    end_line(&mut cells, &mut col);
                    wrapped = true;
                }
            }
        }
    }
    if col > 0 {
        end_line(&mut cells, &mut col);
    }
    lines
}

/// The console-line draw's kill segments (CoDMP.exe 0x4dfa60): each text
/// run advances x by its truncated width; the icon is `scale * 48` times its
/// code over 32 on each side, its top at `baseline - (scale * 38.4 + h) / 2`,
/// white at the line's alpha.
fn kill_line(
    k: &KillLine,
    font: &super::font::Font,
    scale: f32,
    (x0, baseline): (f32, f32),
    alpha: f32,
    v: &Virtual,
    out: &mut Vec<HudQuad>,
) {
    let mut x = x0;
    let run = |s: &str, [r, g, b]: [f32; 3], x: &mut f32, out: &mut Vec<HudQuad>| {
        menu_text_rgba(font, s, (*x, baseline), scale, [r, g, b, alpha], v, out);
        *x += text_width(font, s, scale).trunc();
    };
    if let Some((name, rgb)) = &k.attacker {
        run(&format!("{name} "), *rgb, &mut x, out);
    }
    let unit = scale * 48.0 / 32.0;
    let w = unit
        * if k.icon_wide {
            WIDE_ICON_CODE
        } else {
            ICON_CODE
        } as f32;
    let h = unit * ICON_CODE as f32;
    let top = baseline - (scale * 38.4 + h) * 0.5;
    let uvs = if k.icon_flip {
        [[1.0, 0.0], [0.0, 0.0], [0.0, 1.0], [1.0, 1.0]]
    } else {
        [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]]
    };
    out.push(v.quad_uv(x, top, w, h, uvs, [1.0, 1.0, 1.0, alpha], &k.icon));
    x += w;
    run(&format!(" {}", k.victim.0), k.victim.1, &mut x, out);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn live(w: &Window) -> Vec<(String, i32, i32)> {
        w.slots
            .iter()
            .flatten()
            .map(|l| {
                let text = match &l.body {
                    Body::Text(t) => t.clone(),
                    Body::Kill(k) => k.victim.0.clone(),
                };
                (text, l.start, l.end)
            })
            .collect()
    }

    fn wrapped(text: &str, limit: usize) -> Vec<String> {
        wrap(text, limit, &mut '7')
    }

    /// Ten nine-letter words, 99 characters: two lines, the first ending
    /// after the space that reaches 49.
    #[test]
    fn a_long_line_wraps_into_balanced_lines() {
        let words: Vec<String> = (1..=10).map(|n| format!("word{n:05}")).collect();
        let text = words.join(" ");
        assert_eq!(
            wrapped(&text, 70),
            [words[..5].join(" "), words[5..].join(" ")]
        );
        assert_eq!(wrapped("short line", 70), ["short line"]);
        // The second of two spaces opens the next line, and is dropped.
        let text = format!("{}  {}", words[..5].join(" "), words[5..].join(" "));
        assert_eq!(wrapped(&text, 70)[1], words[5..].join(" "));
    }

    /// A word that won't fit moves down; one longer than the line breaks
    /// where its last 69 characters start.
    #[test]
    fn a_word_moves_down_and_a_long_one_breaks() {
        let text = format!("{} {}", "a".repeat(10), "b".repeat(65));
        assert_eq!(wrapped(&text, 70), ["a".repeat(10), "b".repeat(65)]);
        assert_eq!(
            wrapped(&"x".repeat(100), 70),
            ["x".repeat(31), "x".repeat(69)]
        );
    }

    /// The running colour reopens each line; codes count for no width.
    #[test]
    fn a_wrapped_line_keeps_its_colour() {
        let text = format!("^1{} {}", "a".repeat(40), "b".repeat(35));
        assert_eq!(
            wrapped(&text, 70),
            [
                format!("^1{}", "a".repeat(40)),
                format!("^1{}", "b".repeat(35))
            ]
        );
        assert_eq!(wrapped(&format!("^2{}", "c".repeat(70)), 70).len(), 1);
    }

    /// Every `\n` ends a line, an empty one too; a trailing one does not
    /// add one. The bold window wraps at 60.
    #[test]
    fn a_message_takes_a_slot_per_line() {
        let mut w = Window::new(Kind::Game);
        w.push("a\n\nb\n", 0);
        let texts: Vec<_> = live(&w).into_iter().map(|l| l.0).collect();
        assert_eq!(texts, ["a", "", "b"]);
        let mut w = Window::new(Kind::Bold);
        w.push(&format!("{} {}", "a".repeat(40), "b".repeat(25)), 0);
        assert_eq!(live(&w).len(), 2);
    }

    /// A subtitle wraps at 80 and stays for the time its print asks, the
    /// kind's 5 s when it asks none.
    #[test]
    fn a_subtitle_stays_as_long_as_its_print_asks() {
        let mut w = Window::new(Kind::Subtitle);
        assert_eq!(w.width, 80);
        w.push_for("radio", 100, 7000);
        w.push_for("other", 100, 0);
        let ends: Vec<i32> = w.slots.iter().flatten().map(|l| l.end).collect();
        assert_eq!(ends, [7100, 5100]);
    }

    /// The width cvar holds between 1 and the console's line width; 0 or
    /// past it takes the console's (78 at 640 wide, 238 at 1920).
    #[test]
    fn the_width_cvar_is_clamped_to_the_console() {
        assert_eq!(line_width(70, 1920), 70);
        assert_eq!(line_width(0, 1920), 238);
        assert_eq!(line_width(200, 1024), 126);
        assert_eq!(line_width(70, 320), 70);
        assert_eq!(line_width(-5, 320), 78);
        let mut w = Window::new(Kind::Game);
        w.width = 20;
        w.push(&format!("{} {}", "a".repeat(15), "b".repeat(15)), 0);
        assert_eq!(live(&w).len(), 2);
    }

    /// On 1.5 and UO `^8`/`^9` are codes: they count for no width and reopen
    /// the next line.
    #[test]
    fn team_codes_wrap_as_codes_on_15() {
        let text = format!("^9{} {}", "a".repeat(40), "b".repeat(35));
        crate::hud::font::set_team_colours(true, &|_| None);
        let lines = wrapped(&text, 70);
        crate::hud::font::set_team_colours(false, &|_| None);
        assert_eq!(
            lines,
            [
                format!("^9{}", "a".repeat(40)),
                format!("^9{}", "b".repeat(35))
            ]
        );
        assert_eq!(wrapped(&format!("^9{}", "c".repeat(69)), 70).len(), 2);
    }

    #[test]
    fn a_game_line_lives_five_seconds() {
        let mut w = Window::new(Kind::Game);
        w.push("hello", 1000);
        assert_eq!(live(&w), [("hello".to_string(), 1000, 6000)]);
    }

    /// The sixth line in a ring of eight forces the oldest into its fade.
    #[test]
    fn a_full_window_fades_its_oldest_line() {
        let mut w = Window::new(Kind::Game);
        for i in 0..6 {
            w.push(&format!("m{i}"), 1000 + i);
        }
        let first = live(&w).into_iter().find(|l| l.0 == "m0").unwrap();
        assert_eq!(first.2, 1005 + FADE_OUT_MS);
        let second = live(&w).into_iter().find(|l| l.0 == "m1").unwrap();
        assert_eq!(second.2, 1001 + 5000);
    }

    /// A kill line sits in the game window at x 6 above the compass, names
    /// in their team colours either side of a 16.5-unit icon (33.375 wide).
    #[test]
    fn a_kill_line_draws_in_the_game_window() {
        let Some(fs) = vcod_common::testing::game_fs() else {
            return;
        };
        let fonts = UiFonts::load(&fs).unwrap();
        let v = Virtual::new((640.0, 480.0));
        let red = [1.0, 0.5, 0.5];
        let blue = [0.5, 0.5, 1.0];
        let icon = |wide| {
            let mut w = Window::new(Kind::Game);
            w.push_kill(
                KillLine {
                    attacker: Some(("Att".into(), red)),
                    icon: "gfx/hud/death_melee".into(),
                    icon_wide: wide,
                    icon_flip: false,
                    victim: ("Vic".into(), blue),
                },
                1000,
            );
            let mut out = Vec::new();
            w.build(&fonts, 2000, &v, &mut out);
            let q = out
                .iter()
                .position(|q| q.texture == "gfx/hud/death_melee")
                .unwrap();
            (out, q)
        };
        let (out, q) = icon(false);
        let [tl, tr, br, _] = out[q].verts;
        assert!((tr[0] - tl[0] - 16.5).abs() < 1e-3, "{tl:?} {tr:?}");
        assert!((br[1] - tl[1] - 16.5).abs() < 1e-3);
        // One line: baseline 357, icon top 357 - (0.25 * 38.4 + 16.5) / 2.
        assert!((tl[1] - 343.95).abs() < 1e-3, "{tl:?}");
        assert!(tl[0] > 6.0 && tl[0] < 40.0, "{tl:?}");
        let rgb = |q: &HudQuad| [q.rgba[0], q.rgba[1], q.rgba[2]];
        assert!(out[..q].iter().any(|g| rgb(g) == red));
        assert!(out[q + 1..].iter().any(|g| rgb(g) == blue));
        assert!(
            out.iter()
                .all(|g| g.verts[0][0] > 4.0 && g.verts[0][1] > 330.0)
        );
        let (out, q) = icon(true);
        let [tl, tr, _, _] = out[q].verts;
        assert!((tr[0] - tl[0] - 33.375).abs() < 1e-3);
    }
}
