//! Top-center status header: team scores, gametype, elapsed time since level
//! start. Configstrings per docs/research/cod11-hud-protocol.md, section 5.
//! There is no round-timer configstring, so the clock is not a countdown.

use super::font::{self, Font};
use super::HudQuad;
use vcod_common::net::info_value_for_key;

/// Serverinfo; `g_gametype` is one of its keys.
pub const CS_SERVERINFO: usize = 0;
/// Written by `setteamscore("axis", ...)` (doc section 5, "Which team score is which").
pub const CS_AXIS_SCORE: usize = 5;
/// Written by `setteamscore("allies", ...)`.
pub const CS_ALLIES_SCORE: usize = 6;
/// `level.startTime`, server-clock ms.
pub const CS_LEVEL_START_TIME: usize = 13;

/// First header line; the timer follows one `line_height` below.
const TOP_Y: f32 = 12.0;

pub struct Status {
    pub gametype: String,
    pub allies: i32,
    pub axis: i32,
    /// `"mm:ss"` since level start; `None` without a parsable CS 13.
    pub timer: Option<String>,
}

/// Missing or unparsable strings default to 0/empty; a mid-connect frame
/// has empty configstrings. `scoreboard_totals` (the last `b` reply)
/// overrides CS 5/6, which can lag it (doc section 5, "Header vs.
/// scoreboard totals").
pub fn read_status(
    cs: &[String],
    server_time_ms: i32,
    scoreboard_totals: Option<(i32, i32)>,
) -> Status {
    let gametype = cs
        .get(CS_SERVERINFO)
        .and_then(|si| info_value_for_key(si, "g_gametype"))
        .unwrap_or("")
        .to_string();
    let (axis, allies) = scoreboard_totals.unwrap_or_else(|| {
        let axis = cs
            .get(CS_AXIS_SCORE)
            .and_then(|s| s.parse().ok())
            .unwrap_or(0);
        let allies = cs
            .get(CS_ALLIES_SCORE)
            .and_then(|s| s.parse().ok())
            .unwrap_or(0);
        (axis, allies)
    });
    let timer = cs
        .get(CS_LEVEL_START_TIME)
        .and_then(|s| s.parse::<i32>().ok())
        .map(|start| {
            let elapsed_s = (server_time_ms - start).max(0) / 1000;
            format!("{:02}:{:02}", elapsed_s / 60, elapsed_s % 60)
        });
    Status {
        gametype,
        allies,
        axis,
        timer,
    }
}

/// Two centered lines: `"{allies}  {gametype}  {axis}"`, then the timer.
pub fn build(s: &Status, font: &Font, screen_w: f32, out: &mut Vec<HudQuad>) {
    let scale = super::HUD_SCALE;
    let color = font::COLORS[7];

    let line = format!("{}  {}  {}", s.allies, s.gametype, s.axis);
    let w = font::measure(font, &line, scale);
    font::layout(font, &line, (screen_w - w) / 2.0, TOP_Y, scale, color, out);

    if let Some(timer) = &s.timer {
        let w = font::measure(font, timer, scale);
        let y = TOP_Y + font.line_height(scale);
        font::layout(font, timer, (screen_w - w) / 2.0, y, scale, color, out);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timer_counts_from_level_start() {
        let mut cs = vec![String::new(); 1024];
        cs[CS_LEVEL_START_TIME] = "100000".into();
        cs[CS_SERVERINFO] = "\\g_gametype\\tdm\\mapname\\mp_carentan".into();
        cs[CS_ALLIES_SCORE] = "3".into();
        cs[CS_AXIS_SCORE] = "5".into();
        let s = read_status(&cs, 100_000 + 83_000, None);
        assert_eq!((s.gametype.as_str(), s.allies, s.axis), ("tdm", 3, 5));
        assert_eq!(s.timer.as_deref(), Some("01:23"));
    }

    #[test]
    fn missing_configstrings_default_to_zero_and_no_timer() {
        let cs = vec![String::new(); 1024];
        let s = read_status(&cs, 5_000, None);
        assert_eq!((s.gametype.as_str(), s.allies, s.axis), ("", 0, 0));
        assert_eq!(s.timer, None);
    }

    #[test]
    fn scoreboard_totals_override_configstring_scores() {
        // Values from the 2026-08-24 mp_chateau capture (doc section 5).
        let mut cs = vec![String::new(); 1024];
        cs[CS_SERVERINFO] = "\\g_gametype\\tdm\\mapname\\mp_chateau".into();
        cs[CS_AXIS_SCORE] = "219".into();
        cs[CS_ALLIES_SCORE] = "214".into();
        let s = read_status(&cs, 5_000, Some((221, 231)));
        assert_eq!((s.axis, s.allies), (221, 231));

        // No reply yet: CS values show through.
        let s = read_status(&cs, 5_000, None);
        assert_eq!((s.axis, s.allies), (219, 214));
    }

    #[test]
    fn header_is_centered() {
        let Some(fs) = vcod_common::testing::game_fs() else {
            return;
        };
        let f = crate::hud::font::load_font(&fs, 24).unwrap();
        let s = Status {
            gametype: "tdm".into(),
            allies: 1,
            axis: 2,
            timer: Some("00:10".into()),
        };
        let mut out = Vec::new();
        build(&s, &f, 1280.0, &mut out);
        let xs: Vec<f32> = out
            .iter()
            .flat_map(|q| q.verts.iter().map(|v| v[0]))
            .collect();
        let (min, max) = (
            xs.iter().cloned().fold(f32::MAX, f32::min),
            xs.iter().cloned().fold(f32::MIN, f32::max),
        );
        assert!(((min + max) / 2.0 - 640.0).abs() < 2.0);
    }
}
