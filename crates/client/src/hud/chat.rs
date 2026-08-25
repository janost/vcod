//! Lower-left chat log: the last [`CHAT_LINES`] chat/tchat messages, newest at the bottom.

use std::collections::VecDeque;

use super::font::{self, Font};
use super::HudQuad;

pub const CHAT_LINES: usize = 6;
pub const CHAT_LIFE: f32 = 8.0;

/// Recent chat lines (raw, color-coded text) with the time each was pushed.
pub struct Chat {
    lines: VecDeque<(String, f32)>,
}

impl Chat {
    pub fn new() -> Chat {
        Chat {
            lines: VecDeque::new(),
        }
    }

    /// `team` prefixes "(team) ", the server's own tchat convention.
    pub fn push(&mut self, text: &str, team: bool, now: f32) {
        let line = if team {
            format!("(team) {text}")
        } else {
            text.to_string()
        };
        self.lines.push_back((line, now));
        while self.lines.len() > CHAT_LINES {
            self.lines.pop_front();
        }
    }

    /// Test hook; production reads go through [`Self::build`].
    #[cfg(test)]
    pub fn is_empty(&self) -> bool {
        self.lines.is_empty()
    }

    /// Newest line at `y = screen_h - 40`, older lines one `line_height`
    /// above each. Alpha fades over the last second of `CHAT_LIFE`.
    pub fn build(&self, font: &Font, scale: f32, screen_h: f32, now: f32, out: &mut Vec<HudQuad>) {
        let line_h = font.line_height(scale);
        let alive = self
            .lines
            .iter()
            .rev()
            .filter(|(_, spawn)| now - spawn < CHAT_LIFE);
        for (n, (text, spawn)) in alive.enumerate() {
            let age = now - spawn;
            let alpha = 1.0 - (age - (CHAT_LIFE - 1.0)).clamp(0.0, 1.0);
            let y = screen_h - 40.0 - line_h * n as f32;
            let base = out.len();
            font::layout(font, text, 16.0, y, scale, font::COLORS[7], out);
            // Multiply rather than overwrite: shadow quads carry their own 0.8 alpha.
            for q in &mut out[base..] {
                q.rgba[3] *= alpha;
            }
        }
    }
}

impl Default for Chat {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chat_evicts_oldest_and_fades() {
        let Some(fs) = vcod_common::testing::game_fs() else {
            return;
        };
        let f = crate::hud::font::load_font(&fs, 16).unwrap();
        let mut c = Chat::new();
        for i in 0..(CHAT_LINES + 2) {
            c.push(&format!("m{i}"), false, 0.0);
        }
        let mut out = Vec::new();
        c.build(&f, 1.0, 720.0, 0.5, &mut out);
        // 6 lines x 2 chars x 2 quads (shadow + glyph)
        assert_eq!(out.len(), CHAT_LINES * 2 * 2);
        out.clear();
        c.build(&f, 1.0, 720.0, CHAT_LIFE + 1.0, &mut out);
        assert!(out.is_empty());
    }

    #[test]
    fn team_chat_is_prefixed() {
        let mut c = Chat::new();
        c.push("go", true, 0.0);
        assert_eq!(c.lines[0].0, "(team) go");
    }
}
