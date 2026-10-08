//! The drop-down console: scrollback, input line, history, completion and
//! the retail look (`Con_DrawSolidConsole`, docs/research/cod11-console.md).
//! [`shell`] runs what is typed into it.

pub mod keys;
pub mod log;
pub mod shell;

use std::collections::VecDeque;

use winit::keyboard::KeyCode;

use crate::hud::HudQuad;
use crate::hud::font::{self, Font};
use vcod_common::pk3::Pk3Fs;

const SCROLLBACK_LINES: usize = 1024;
/// Retail's history ring (`& 0x1f`) and field size.
const HISTORY: usize = 32;
const FIELD_MAX: usize = 255;
/// Retail's console cell and row, window px at any resolution.
const CHAR_W: f32 = 8.0;
const ROW_H: f32 = 16.0;
/// Rows a Page Up / Page Down or a wheel notch moves.
const PAGE_ROWS: usize = 2;
/// The `console` shader is `$whiteimage` at `rgbGen constLighting 0.15`.
const BACKGROUND: [f32; 4] = [0.15, 0.15, 0.15, 1.0];
const SEPARATOR: [f32; 4] = [0.0, 0.0, 0.0, 0.6];
const WHITE: [f32; 4] = [1.0; 4];

pub struct Console {
    pub open: bool,
    /// Drawn height as a fraction of the screen; slides toward the target.
    frac: f32,
    lines: VecDeque<String>,
    /// Wrapped rows scrolled back from the newest.
    scroll: usize,
    field: String,
    /// Byte index into `field`, which holds ASCII only.
    cursor: usize,
    history: Vec<String>,
    /// `history.len()` while editing a fresh line.
    history_pos: usize,
    /// `fontImage_18`, retail's `consoleFont`. `None` draws no text.
    font: Option<Font>,
}

impl Console {
    pub fn new(fs: &Pk3Fs) -> Console {
        let font = match font::load_font(fs, 18) {
            Ok(f) => Some(f),
            Err(e) => {
                ::log::warn!("console: {e}, drawing no text");
                None
            }
        };
        Console {
            open: false,
            frac: 0.0,
            lines: VecDeque::new(),
            scroll: 0,
            field: String::new(),
            cursor: 0,
            history: Vec::new(),
            history_pos: 0,
            font,
        }
    }

    /// `toggleconsole` (CoDMP.exe 0x4083a0) clears the field either way.
    pub fn toggle(&mut self) {
        self.open = !self.open;
        self.field.clear();
        self.cursor = 0;
        self.history_pos = self.history.len();
    }

    /// Puts `text` in the input field with the cursor at its end.
    pub fn set_input(&mut self, text: &str) {
        self.field = text.to_string();
        self.cursor = self.field.len();
    }

    pub fn print(&mut self, text: &str) {
        for line in text.lines() {
            if self.lines.len() == SCROLLBACK_LINES {
                self.lines.pop_front();
            }
            self.lines.push_back(line.to_string());
        }
    }

    pub fn clear(&mut self) {
        self.lines.clear();
        self.scroll = 0;
    }

    /// Takes what the logger and [`log::print`] collected since last frame.
    pub fn drain_log(&mut self) {
        for line in log::drain() {
            self.print(&line);
        }
    }

    pub fn scroll(&mut self, up: bool) {
        self.scroll = if up {
            self.scroll + PAGE_ROWS
        } else {
            self.scroll.saturating_sub(PAGE_ROWS)
        };
    }

    /// A key while the console is open. Returns the submitted line on Enter,
    /// echoed to the scrollback as retail's `]%s`. `complete` lists the
    /// command and cvar names for a prefix.
    pub fn key(
        &mut self,
        code: KeyCode,
        text: Option<&str>,
        complete: impl FnOnce(&str) -> Vec<String>,
    ) -> Option<String> {
        match code {
            KeyCode::Enter | KeyCode::NumpadEnter => {
                let line = std::mem::take(&mut self.field);
                self.cursor = 0;
                log::print(&format!("]{line}"));
                if !line.trim().is_empty() {
                    if self.history.last() != Some(&line) {
                        if self.history.len() == HISTORY {
                            self.history.remove(0);
                        }
                        self.history.push(line.clone());
                    }
                    self.scroll = 0;
                }
                self.history_pos = self.history.len();
                return Some(line);
            }
            KeyCode::Tab => self.complete(complete),
            KeyCode::Backspace if self.cursor > 0 => {
                self.cursor -= 1;
                self.field.remove(self.cursor);
            }
            KeyCode::Delete if self.cursor < self.field.len() => {
                self.field.remove(self.cursor);
            }
            KeyCode::ArrowLeft => self.cursor = self.cursor.saturating_sub(1),
            KeyCode::ArrowRight => self.cursor = (self.cursor + 1).min(self.field.len()),
            KeyCode::Home => self.cursor = 0,
            KeyCode::End => self.cursor = self.field.len(),
            KeyCode::ArrowUp if self.history_pos > 0 => {
                self.history_pos -= 1;
                self.recall();
            }
            KeyCode::ArrowDown if self.history_pos < self.history.len() => {
                self.history_pos += 1;
                self.recall();
            }
            KeyCode::PageUp => self.scroll(true),
            KeyCode::PageDown => self.scroll(false),
            _ => {
                for c in text.unwrap_or("").chars() {
                    if (' '..='~').contains(&c)
                        && c != '`'
                        && c != '~'
                        && self.field.len() < FIELD_MAX
                    {
                        self.field.insert(self.cursor, c);
                        self.cursor += 1;
                    }
                }
            }
        }
        None
    }

    fn recall(&mut self) {
        self.field = self
            .history
            .get(self.history_pos)
            .cloned()
            .unwrap_or_default();
        self.cursor = self.field.len();
    }

    /// Retail's completion: one match replaces the command word and adds a
    /// space, several are listed and the word runs to their common prefix.
    /// Only the first word completes.
    fn complete(&mut self, names: impl FnOnce(&str) -> Vec<String>) {
        let slash = self.field.starts_with(['/', '\\']);
        let word = self.field.trim_start_matches(['/', '\\']);
        if word.is_empty() || word.contains(' ') {
            return;
        }
        let matches = names(word);
        let prefix = if slash { &self.field[..1] } else { "" };
        match matches.as_slice() {
            [] => return,
            [one] => self.field = format!("{prefix}{one} "),
            many => {
                log::print(&format!("]{}", self.field));
                for m in many {
                    log::print(&format!("    {m}"));
                }
                self.field = format!("{prefix}{}", common_prefix(many));
            }
        }
        self.cursor = self.field.len();
    }

    /// Slides toward its target: half the screen open, gone closed, the
    /// whole screen at once when `full` (disconnected with no menu up). Retail
    /// moves `scr_conspeed` screens a second.
    pub fn update(&mut self, dt_ms: f32, conspeed: f32, full: bool) {
        if full {
            self.frac = 1.0;
            return;
        }
        let target = if self.open { 0.5 } else { 0.0 };
        let step = dt_ms * conspeed * 0.001;
        self.frac = if self.frac < target {
            (self.frac + step).min(target)
        } else {
            (self.frac - step).max(target)
        };
    }

    /// Whether anything of it is on screen.
    pub fn visible(&self) -> bool {
        self.frac > 0.0
    }

    /// The console as HUD quads for a `w` x `h` window, drawn over
    /// everything else. `realtime_ms` blinks the cursor.
    pub fn build(&self, w: f32, h: f32, realtime_ms: u64) -> Vec<HudQuad> {
        let mut out = Vec::new();
        let lines = (self.frac * h).min(h).floor();
        if lines <= 0.0 {
            return out;
        }
        // The background and the separator are placed on the 640x480 grid.
        let sy = h / 480.0;
        let mut y = self.frac * 480.0 - 2.0;
        if y < 1.0 {
            y = 0.0;
        } else {
            out.push(rect(0.0, 0.0, w, y * sy, BACKGROUND));
        }
        out.push(rect(0.0, y * sy, w, 2.0 * sy, SEPARATOR));

        let Some(font) = &self.font else { return out };
        // Text is laid out in window px: 8 px cells, 16 px rows, glyphs at a
        // third of the font's own scale, each row's baseline 16 px below it.
        let scale = 1.0 / (3.0 * font.unit_scale());
        let glyph_top = font.max_height as f32 * font.glyph_scale / 3.0;
        let text = |out: &mut Vec<HudQuad>, s: &str, x: f32, row_y: f32| {
            let top = row_y + ROW_H - glyph_top;
            font::layout_cells(font, s, x, top, scale, CHAR_W, WHITE, out);
        };

        let version = concat!("vcod ", env!("CARGO_PKG_VERSION"));
        text(
            &mut out,
            version,
            w - version.len() as f32 * CHAR_W,
            lines - 18.0,
        );

        let cols = ((w.max(640.0) / CHAR_W) as usize).saturating_sub(2).max(1);
        let rows = self.rows(cols);
        let mut row_y = lines - 48.0;
        let scroll = self.scroll.min(rows.len().saturating_sub(1));
        if scroll > 0 {
            for col in (0..cols).step_by(4) {
                text(&mut out, "^", CHAR_W + col as f32 * CHAR_W, row_y);
            }
            row_y -= ROW_H;
        }
        for row in rows.iter().rev().skip(scroll) {
            if row_y < -ROW_H {
                break;
            }
            text(&mut out, row, CHAR_W, row_y);
            row_y -= ROW_H;
        }

        let input_y = lines - 32.0;
        text(&mut out, "]", CHAR_W, input_y);
        text(&mut out, &self.field, 2.0 * CHAR_W, input_y);
        if (realtime_ms >> 8) & 1 == 0 {
            let x = 2.0 * CHAR_W + self.cursor as f32 * CHAR_W;
            text(&mut out, "_", x, input_y);
        }
        out
    }

    /// The scrollback wrapped to `cols`, oldest first.
    fn rows(&self, cols: usize) -> Vec<String> {
        self.lines.iter().flat_map(|l| wrap(l, cols)).collect()
    }
}

fn rect(x: f32, y: f32, w: f32, h: f32, rgba: [f32; 4]) -> HudQuad {
    HudQuad {
        verts: [[x, y], [x + w, y], [x + w, y + h], [x, y + h]],
        uvs: [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]],
        rgba,
        texture: "white".into(),
    }
}

/// Breaks `line` every `cols` printable characters; a row that starts
/// mid-colour carries the colour code it inherits.
fn wrap(line: &str, cols: usize) -> Vec<String> {
    let mut rows = Vec::new();
    let mut row = String::new();
    let mut count = 0;
    let mut color: Option<char> = None;
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '^'
            && let Some(&d) = chars.peek()
            && d.is_ascii_digit()
            && d <= '7'
        {
            chars.next();
            row.push('^');
            row.push(d);
            color = Some(d);
            continue;
        }
        if count == cols {
            rows.push(std::mem::take(&mut row));
            count = 0;
            if let Some(d) = color {
                row.push('^');
                row.push(d);
            }
        }
        row.push(c);
        count += 1;
    }
    rows.push(row);
    rows
}

fn common_prefix(names: &[String]) -> String {
    let first = names[0].as_str();
    let mut len = first.len();
    for n in &names[1..] {
        len = first
            .bytes()
            .zip(n.bytes())
            .take(len)
            .take_while(|(a, b)| a.eq_ignore_ascii_case(b))
            .count();
    }
    first[..len].to_string()
}

/// What an entered line runs (CoDMP.exe 0x40d050): a leading `/` or `\`
/// marks a command; in game, anything else is chat (`say`); outside one,
/// everything is a command. An empty line runs nothing.
pub fn command_for(line: &str, in_game: bool) -> Option<String> {
    if let Some(cmd) = line.strip_prefix(['/', '\\']) {
        return Some(cmd.to_string());
    }
    if line.trim().is_empty() {
        None
    } else if in_game {
        Some(format!("say {line}"))
    } else {
        Some(line.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn console() -> Console {
        Console {
            open: true,
            frac: 0.0,
            lines: VecDeque::new(),
            scroll: 0,
            field: String::new(),
            cursor: 0,
            history: Vec::new(),
            history_pos: 0,
            font: None,
        }
    }

    fn type_line(c: &mut Console, s: &str) -> Option<String> {
        c.key(KeyCode::KeyA, Some(s), |_| Vec::new());
        c.key(KeyCode::Enter, None, |_| Vec::new())
    }

    #[test]
    fn enter_submits_and_history_recalls() {
        let mut c = console();
        assert_eq!(type_line(&mut c, "echo one").as_deref(), Some("echo one"));
        type_line(&mut c, "echo two");
        c.key(KeyCode::ArrowUp, None, |_| Vec::new());
        assert_eq!(c.field, "echo two");
        c.key(KeyCode::ArrowUp, None, |_| Vec::new());
        assert_eq!(c.field, "echo one");
        c.key(KeyCode::ArrowDown, None, |_| Vec::new());
        c.key(KeyCode::ArrowDown, None, |_| Vec::new());
        assert_eq!(c.field, "");
    }

    #[test]
    fn editing_at_the_cursor() {
        let mut c = console();
        c.key(KeyCode::KeyA, Some("ac"), |_| Vec::new());
        c.key(KeyCode::ArrowLeft, None, |_| Vec::new());
        c.key(KeyCode::KeyB, Some("b"), |_| Vec::new());
        assert_eq!(c.field, "abc");
        c.key(KeyCode::Home, None, |_| Vec::new());
        c.key(KeyCode::Delete, None, |_| Vec::new());
        assert_eq!(c.field, "bc");
        // The toggle key never types.
        c.key(KeyCode::Backquote, Some("`"), |_| Vec::new());
        assert_eq!(c.field, "bc");
    }

    #[test]
    fn tab_completes_one_and_lists_many() {
        let names = |p: &str| {
            ["unbind", "unbindall", "quit"]
                .iter()
                .filter(|n| n.starts_with(p))
                .map(|n| n.to_string())
                .collect()
        };
        let mut c = console();
        c.key(KeyCode::KeyQ, Some("/qu"), |_| Vec::new());
        c.key(KeyCode::Tab, None, names);
        assert_eq!(c.field, "/quit ");
        let mut c = console();
        c.key(KeyCode::KeyU, Some("un"), |_| Vec::new());
        c.key(KeyCode::Tab, None, names);
        assert_eq!(c.field, "unbind");
    }

    #[test]
    fn lines_run_as_commands_or_chat() {
        assert_eq!(command_for("/quit", true).as_deref(), Some("quit"));
        assert_eq!(command_for("\\quit", false).as_deref(), Some("quit"));
        assert_eq!(command_for("hello", true).as_deref(), Some("say hello"));
        assert_eq!(
            command_for("connect x", false).as_deref(),
            Some("connect x")
        );
        assert_eq!(command_for("  ", true), None);
    }

    #[test]
    fn wrapping_carries_the_colour() {
        assert_eq!(wrap("abcdef", 4), ["abcd", "ef"]);
        assert_eq!(wrap("^1abcdef", 4), ["^1abcd", "^1ef"]);
        assert_eq!(wrap("", 4), [""]);
    }

    #[test]
    fn slides_at_conspeed_and_snaps_full() {
        let mut c = console();
        c.update(100.0, 3.0, false);
        assert!((c.frac - 0.3).abs() < 1e-6);
        c.update(100.0, 3.0, false);
        assert_eq!(c.frac, 0.5);
        c.open = false;
        c.update(1000.0, 3.0, false);
        assert_eq!(c.frac, 0.0);
        c.update(1.0, 3.0, true);
        assert_eq!(c.frac, 1.0);
    }

    #[test]
    fn draws_background_and_separator_on_the_480_grid() {
        let mut c = console();
        c.frac = 0.5;
        let q = c.build(1280.0, 960.0, 0);
        // (0.5 * 480 - 2) * 2 = 476 px of background, a 4 px separator.
        assert_eq!(q[0].verts[2], [1280.0, 476.0]);
        assert_eq!(q[1].verts[0], [0.0, 476.0]);
        assert_eq!(q[1].verts[2], [1280.0, 480.0]);
        assert_eq!(q[1].rgba, SEPARATOR);
    }
}

#[cfg(test)]
mod font_tests {
    /// `fontImage_18` is an 8x16 cell font at a third of its `glyphScale`,
    /// which is what fills retail's 8 px columns and 16 px rows.
    #[test]
    fn console_glyphs_are_8_by_16_px() {
        let Some(fs) = vcod_common::testing::game_fs() else {
            return;
        };
        let f = crate::hud::font::load_font(&fs, 18).unwrap();
        let s = f.glyph_scale / 3.0;
        for c in b"aW]_" {
            let g = &f.glyphs[*c as usize];
            assert_eq!(
                (g.image_width as f32 * s, g.image_height as f32 * s),
                (8.0, 16.0)
            );
        }
    }
}
