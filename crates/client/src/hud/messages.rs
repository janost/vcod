//! The engine's two message windows the cgame draws: game messages (`e`,
//! `f`, `iPrintLn`) above the compass and bold messages (`c`, `g`,
//! `iPrintLnBold`, `announcement`) centred over the crosshair. CoDMP.exe
//! keeps both as console-line windows (0x408be0, 0x409920); the layout and
//! timing are in docs/research/cod11-chat.md.

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
}

impl Kind {
    fn life_ms(self) -> i32 {
        match self {
            Kind::Game => 5000,
            Kind::Bold => 8000,
        }
    }

    fn line_height(self) -> f32 {
        match self {
            Kind::Game => 12.0,
            Kind::Bold => 16.0,
        }
    }

    /// Font 0 at 0.25, or font 4 at a third.
    fn font<'a>(self, fonts: &'a UiFonts, v: &Virtual) -> (&'a super::font::Font, f32) {
        let (slot, scale) = match self {
            Kind::Game => (Slot::Default, 0.25),
            Kind::Bold => (Slot::BigFixed, 1.0 / 3.0),
        };
        (fonts.pick(slot, scale, 480.0 * v.scale), scale)
    }
}

#[derive(Clone, Default)]
struct Line {
    text: String,
    start: i32,
    end: i32,
}

/// One window's ring. A slot with `start` 0 is empty.
pub struct Window {
    kind: Kind,
    slots: [Option<Line>; SLOTS],
    current: usize,
}

impl Window {
    pub fn new(kind: Kind) -> Window {
        Window {
            kind,
            slots: Default::default(),
            current: 0,
        }
    }

    /// A localized message, one line per `\n`.
    pub fn push(&mut self, text: &str, now_ms: i32) {
        for line in text.split('\n').filter(|l| !l.is_empty()) {
            self.add_line(line, now_ms);
        }
    }

    /// `0x408be0`: the line takes the current slot, and the three oldest
    /// that would outlive the next 500 ms start their fade now.
    fn add_line(&mut self, text: &str, now: i32) {
        self.slots[self.current] = Some(Line {
            text: text.to_string(),
            start: now,
            end: now + self.kind.life_ms(),
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
        let (x0, mut y) = match self.kind {
            Kind::Game => (6.0, 357.0),
            Kind::Bold => (320.0, 180.0),
        };
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
            let x = match self.kind {
                Kind::Game => x0,
                Kind::Bold => x0 - (text_width(font, &l.text, scale) / 2.0).trunc(),
            };
            menu_text_rgba(
                font,
                &l.text,
                (x, y + 12.0),
                scale,
                [1.0, 1.0, 1.0, alpha],
                v,
                out,
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn live(w: &Window) -> Vec<(String, i32, i32)> {
        w.slots
            .iter()
            .flatten()
            .map(|l| (l.text.clone(), l.start, l.end))
            .collect()
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
}
