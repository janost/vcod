//! 1.5's match timeout banner (cgame 1.5 0x300195c0, drawn right after the
//! vote): "Paused", the seconds left, and the timeout message, centred on
//! the screen while configstring 1628 holds a time
//! (docs/research/cod15-protocol-diff.md, "Match timeout").

use super::HudQuad;
use super::font::{self, Slot, TextStyle, UiFonts};
use super::hudelem::Virtual;
use super::player::menu_text_rgba;
use vcod_common::localize::Localized;

/// Font 0 at half scale.
const SCALE: f32 = 0.5;

#[derive(Default)]
pub struct MatchTimeout {
    /// Local ms the timeout ends at; `None` when none runs.
    end_ms: Option<i32>,
    /// Read off a gamestate, whose time is stale: no countdown.
    from_gamestate: bool,
    /// The raw configstrings last read, to tell a change.
    seen: Option<(String, String)>,
}

impl MatchTimeout {
    /// A new gamestate: the next [`Self::update`] reads the configstrings
    /// as `CG_Init` does (0x30024290).
    pub fn on_gamestate(&mut self) {
        self.seen = None;
    }

    /// Follows configstrings 1628 and 1629 (United Offensive's 1909 and
    /// 1910), `slot` the first of the pair (`CsLayout::match_timeout`). A
    /// value is the ms left as of its arrival, which
    /// `CG_ConfigStringModified` (0x3002db40) adds to `trap_Milliseconds`;
    /// one read off the gamestate shows no count.
    pub fn update(&mut self, configstrings: &[String], slot: Option<usize>, now_ms: i32) {
        let cs = |i: Option<usize>| {
            i.and_then(|i| configstrings.get(i))
                .cloned()
                .unwrap_or_default()
        };
        let (time, text) = (cs(slot), cs(slot.map(|i| i + 1)));
        let fresh = self.seen.is_none();
        if self.seen.as_ref().is_some_and(|(t, _)| *t == time) {
            self.seen = Some((time, text));
            return;
        }
        // `atol`: leading digits, 0 for none.
        let digits: String = time
            .trim_start()
            .chars()
            .enumerate()
            .take_while(|&(i, c)| c.is_ascii_digit() || (i == 0 && c == '-'))
            .map(|(_, c)| c)
            .collect();
        let ms = digits.parse::<i32>().unwrap_or(0);
        self.end_ms = (ms != 0).then(|| now_ms.saturating_add(ms));
        self.from_gamestate = fresh && ms != 0;
        self.seen = Some((time, text));
    }

    /// The banner's text: `"%s: %i \n%s"`, or `"%s\n%s"` off a gamestate.
    pub fn text(&self, loc: &Localized, now_ms: i32) -> Option<String> {
        let end = self.end_ms?;
        let paused = loc.get("CGAME_PAUSED").unwrap_or("CGAME_PAUSED");
        let message = loc.message(self.seen.as_ref().map_or("", |(_, t)| t.as_str()));
        Some(if self.from_gamestate {
            format!("{paused}\n{message}")
        } else {
            let secs = ((end - now_ms) / 1000).max(0);
            format!("{paused}: {secs} \n{message}")
        })
    }

    /// Centred on the 640x480 screen in white, in style 6, line by line
    /// under the first. One `R_DrawText` call (trap 0x36) draws the whole
    /// string: its size comes off the int traps 0x34 and 0x35, and each `\n`
    /// starts a line at the same x, `scale` times the font's line height
    /// lower (cod15-protocol-diff.md, "Match timeout").
    pub fn build(
        &self,
        fonts: &UiFonts,
        loc: &Localized,
        now_ms: i32,
        v: &Virtual,
        out: &mut Vec<HudQuad>,
    ) {
        let Some(text) = self.text(loc, now_ms) else {
            return;
        };
        let font = fonts.pick(Slot::Default, SCALE, 480.0 * v.scale);
        let w = font::trap_width(font, &text, SCALE);
        let h = font::trap_height(font, SCALE);
        let x = (640.0 - w) * 0.5;
        let y = (480.0 - h) * 0.5;
        for (i, line) in text.split('\n').enumerate() {
            let baseline = y + i as f32 * SCALE * font.line_advance;
            let style = TextStyle::SHADOWED_MORE;
            menu_text_rgba(font, line, (x, baseline), SCALE, [1.0; 4], style, v, out);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use vcod_common::net::protocol::{CS_MATCH_TIMEOUT_TEXT_V6, CS_MATCH_TIMEOUT_V6};

    fn cs(time: &str, text: &str) -> Vec<String> {
        let mut cs = vec![String::new(); 2048];
        cs[CS_MATCH_TIMEOUT_V6] = time.into();
        cs[CS_MATCH_TIMEOUT_TEXT_V6] = text.into();
        cs
    }

    #[test]
    fn a_called_timeout_counts_down_on_local_time() {
        let loc = Localized::default();
        let mut t = MatchTimeout::default();
        t.update(&cs("", ""), Some(CS_MATCH_TIMEOUT_V6), 0);
        assert_eq!(t.text(&loc, 0), None);
        // The 1.5b server's `d 1628 15000` and `d 1629` for an axis call.
        let called = cs("15000", "PATCH_1_5_TIMEOUT_CALLED\u{14}GAME_AXIS\u{15}");
        t.update(&called, Some(CS_MATCH_TIMEOUT_V6), 1_000);
        let text = t.text(&loc, 4_500).unwrap();
        assert_eq!(text, "CGAME_PAUSED: 11 \nPATCH_1_5_TIMEOUT_CALLEDGAME_AXIS");
        assert_eq!(
            t.text(&loc, 60_000).as_deref(),
            Some("CGAME_PAUSED: 0 \nPATCH_1_5_TIMEOUT_CALLEDGAME_AXIS")
        );
        // The recovery restarts the count; the empty value ends it.
        t.update(
            &cs("5000", "PATCH_1_5_TIMEOUT_ENDING\u{15}"),
            Some(CS_MATCH_TIMEOUT_V6),
            10_000,
        );
        assert!(
            t.text(&loc, 10_000)
                .unwrap()
                .starts_with("CGAME_PAUSED: 5 \n")
        );
        t.update(
            &cs("", "PATCH_1_5_TIMEOUT_ENDING\u{15}"),
            Some(CS_MATCH_TIMEOUT_V6),
            15_000,
        );
        assert_eq!(t.text(&loc, 15_000), None);
    }

    #[test]
    fn a_timeout_read_off_the_gamestate_shows_no_count() {
        let loc = Localized::default();
        let mut t = MatchTimeout::default();
        t.on_gamestate();
        t.update(
            &cs("9000", "PATCH_1_5_TIMEOUT_ENDING\u{15}"),
            Some(CS_MATCH_TIMEOUT_V6),
            0,
        );
        assert_eq!(
            t.text(&loc, 100).as_deref(),
            Some("CGAME_PAUSED\nPATCH_1_5_TIMEOUT_ENDING")
        );
    }

    /// Style 6: each glyph's shadow two virtual units off, black at the
    /// glyph's alpha; the second line starts at the first's x, a line
    /// height lower.
    #[test]
    fn the_banner_draws_in_style_6() {
        let Some(fs) = vcod_common::testing::game_fs() else {
            return;
        };
        let fonts = UiFonts::load(&fs).unwrap();
        let v = Virtual::new((1280.0, 960.0));
        let loc = Localized::default();
        let mut t = MatchTimeout::default();
        t.update(&cs("", ""), Some(CS_MATCH_TIMEOUT_V6), 0);
        t.update(&cs("15000", "CB"), Some(CS_MATCH_TIMEOUT_V6), 0);
        let mut out = Vec::new();
        t.build(&fonts, &loc, 0, &v, &mut out);
        let (shadow, glyph) = (&out[0], &out[1]);
        assert_eq!(shadow.rgba, [0.0, 0.0, 0.0, 1.0]);
        assert_eq!(shadow.verts[0][0] - glyph.verts[0][0], 4.0);
        assert_eq!(shadow.verts[0][1] - glyph.verts[0][1], 4.0);
        // The second line's "C" under the first's, a line height lower.
        let first = t.text(&loc, 0).unwrap();
        let n = first.split('\n').next().unwrap().chars().count();
        let next = &out[2 * n + 1];
        let font = fonts.pick(Slot::Default, SCALE, 960.0);
        assert_eq!(next.verts[0][0], glyph.verts[0][0]);
        let step = next.verts[0][1] - glyph.verts[0][1];
        assert!(
            (step - SCALE * font.line_advance * 2.0).abs() < 1e-3,
            "{step}"
        );
    }
}
