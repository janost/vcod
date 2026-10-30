//! 1.7's connect screen (`SCR_DrawScreenField`'s connecting arm,
//! `UI_DrawConnectScreen` over `CG_DrawInformation`, docs/research/
//! cod4-hud.md 18): the `connect` menu (`code_post_gfx_mp`) with the
//! map's levelshot and the load bar, the game type and the map's name
//! once the gamestate names them, the stage line and a download's
//! progress. A child of `cod4_hud`, whose fonts and strings it draws with.

use super::*;
use crate::cod4_loading::{Stage, Status};

/// The menu `UI_DrawMapLevelshot` paints.
const CONNECT_MENU: &str = "connect";
/// The menu background the `_load` zone's levelshot material names.
const LEVELBRIEFING: &str = "$levelbriefing";
/// `UI_DrawConnectScreen`'s layout on the full-screen 640 grid: the text's
/// top line and the stage line's baseline, the scale.
const Y_START: f32 = 89.0;
const STATUS_Y: f32 = 145.0;
const SCALE: f32 = 0.5;
/// The message lines' first baseline and step, their colour (`iw3mp.exe`
/// 0x70be4c, 0x70b7e8, 0x6b4498).
const MESSAGE_Y: f32 = 299.0;
const LINE_STEP: f32 = 22.0;
const YELLOW: [f32; 4] = [1.0, 1.0, 0.0, 1.0];
/// `windowDef_t.style` 1 (filled), 3 (a material), 6 (the load bar).
const STYLE_FILLED: i32 = 1;
const STYLE_SHADER: i32 = 3;
const STYLE_LOADBAR: i32 = 6;
/// The `connect` menu's "did you know" box: its items carry this dynamic
/// flag and vcod draws them only when the `didyouknow` dvar has text.
const INTEL_GROUP: u32 = 0x1_0000;
/// `scrPlaceFull`: the grid stretched over the whole window.
const FULL: i32 = 4;
/// `ui_connectScreenTextGlowColor`'s 1.7 default, the glow on the game
/// type and the map's name (`iw3mp.exe` 0x542628..0x542665, read at
/// 0x54a686 and 0x54a6bb).
const TITLE_GLOW: [f32; 4] = [0.3, 0.6, 0.3, 1.0];
/// `UI_DisplayDownloadInfo`'s second column and the widest its file
/// name draws at its scale (0x70b774, 0x70bd10), the least scale it
/// shrinks to (0x70b3e8).
const SECOND_COLUMN: f32 = 200.0;
const NAME_WIDTH: f32 = 430.0;
const NAME_SCALE_MIN: f32 = 0.2;
/// The time-left estimates it averages (the ring at 0x729db8).
const ESTIMATES: usize = 80;

/// `UI_DisplayDownloadInfo`'s time-left ring: one estimate a drawn frame,
/// the line their mean over all 80 slots, the empty ones 0. Retail's is a
/// static that outlives a download; vcod's lives as long as the join.
pub(crate) struct TimeLeft {
    est: [i64; ESTIMATES],
    at: usize,
}

impl Default for TimeLeft {
    fn default() -> TimeLeft {
        TimeLeft {
            est: [0; ESTIMATES],
            at: 0,
        }
    }
}

impl TimeLeft {
    /// Takes this frame's estimate, seconds, and returns the mean.
    fn push(&mut self, secs: i64) -> i64 {
        self.est[self.at] = secs;
        self.at = (self.at + 1) % ESTIMATES;
        self.est.iter().sum::<i64>() / ESTIMATES as i64
    }
}

/// A download's rate and time-left lines at `elapsed_ms` past its start
/// (0x549fea..0x54a0d3): no rate before 4096 bytes, the rate the bytes
/// over the whole seconds gone, and with a size and a rate this frame's
/// estimate (`size / rate` less its share already down, in KB) into the
/// ring. `None` while estimating; else the rate and the mean time left.
fn rates(got: u64, size: u64, elapsed_ms: u128, ring: &mut TimeLeft) -> Option<(i64, Option<i64>)> {
    if got < 4096 {
        return None;
    }
    let (got, size) = (got as i64, size as i64);
    let secs = (elapsed_ms / 1000) as i64;
    let rate = got.checked_div(secs).unwrap_or(0);
    if size == 0 || rate == 0 {
        return Some((rate, None));
    }
    let whole = size / rate;
    let est = whole - (whole * (got / 1024)).checked_div(size / 1024).unwrap_or(0);
    Some((rate, Some(ring.push(est))))
}

impl Cod4Hud {
    /// The connect screen for `st` on a `size` window, at wall-clock ms
    /// `now` (the dots' clock); `levelshot` the `_load` zone's image.
    pub(crate) fn connect_screen(
        &self,
        st: &Status,
        levelshot: Option<&str>,
        size: (f32, f32),
        now: i32,
        ring: &mut TimeLeft,
    ) -> Vec<HudQuad> {
        let mut out = Vec::new();
        let Some(Some(l)) = &self.loaded else {
            return out;
        };
        let sp = ScreenPlacement::cod4(size.0, size.1);
        // `UI_DrawMapLevelshot`: black until the map is known.
        out.push(quad(
            0.0,
            0.0,
            size.0,
            size.1,
            [0.0, 0.0, 0.0, 1.0],
            "white".into(),
            false,
        ));
        if st.map.is_some()
            && let Some(layout) = l
                .script_menus
                .get(CONNECT_MENU)
                .and_then(|m| m.layout.as_ref())
        {
            self.connect_menu(l, layout, st, levelshot, &sp, &mut out);
        }
        let loc = |k: &str| l.localized.get(k).unwrap_or(k).to_string();
        let f = l.fonts.pick(6, SCALE, &sp);
        let centre = |out: &mut Vec<HudQuad>, t: &str, y: f32| {
            let px = SCALE / f.unit_scale();
            let w = font::measure(f, t, px);
            put_glow(
                &sp,
                f,
                t,
                centre_x(&sp, w),
                y,
                SCALE,
                [1.0; 4],
                TITLE_GLOW,
                out,
            );
        };
        let mut info = true;
        if let Some(map) = &st.map {
            let gametype = st.gametype.as_deref().unwrap_or_default();
            let gt = l
                .gametypes
                .get(&gametype.to_ascii_lowercase())
                .map_or_else(|| gametype.to_string(), |k| loc(k));
            centre(&mut out, &gt, Y_START);
            centre(&mut out, &self.map_display_name(l, map), Y_START + 30.0);
            info = false;
        }
        if st.loaded >= 1.0 {
            info = true;
        }
        // `clc.serverMessage` in yellow under `CA_CONNECTED`
        // (0x54a709..0x54a7d4), its baseline 22 lower a line.
        if st.stage == Stage::Challenging && !st.message.is_empty() {
            let px = SCALE / f.unit_scale();
            // A `print` ends in a newline, which draws nothing.
            let text: String = loc(&st.message).chars().filter(|c| *c != '\n').collect();
            for (k, t) in message_lines(&text).iter().enumerate() {
                let w = font::measure(f, t, px);
                let y = MESSAGE_Y + LINE_STEP * k as f32;
                put(&sp, f, t, centre_x(&sp, w), y, SCALE, YELLOW, &mut out);
            }
        }
        if let Some(d) = st
            .download
            .as_ref()
            .filter(|_| st.stage == Stage::AwaitingGamestate)
        {
            self.download_info(l, d, &sp, ring, &mut out);
            return out;
        }
        let line = match st.stage {
            Stage::Challenging => Some(loc("EXE_AWAITINGCHALLENGE").replace("&&1", "1")),
            Stage::AwaitingGamestate => Some(loc("EXE_AWAITINGGAMESTATE")),
            Stage::Loading | Stage::Done => Some(loc("EXE_AWAITINGHOST")),
        }
        .filter(|_| info);
        if let Some(t) = line {
            // `Text_PaintCenterWithDots`: centred without its dots, which
            // count up every half second.
            let dots = ["   ", ".  ", ".. ", "..."][((now / 500) & 3) as usize];
            let px = SCALE / f.unit_scale();
            let w = font::measure(f, &t, px);
            let x = centre_x(&sp, w);
            put(
                &sp,
                f,
                &format!("{t}{dots}"),
                x,
                STATUS_Y,
                SCALE,
                [1.0; 4],
                &mut out,
            );
        }
        out
    }

    /// `UI_GetMapDisplayName`: the arena's long name, localized; the map
    /// itself when no arena lists it.
    fn map_display_name(&self, l: &Loaded, map: &str) -> String {
        l.ui_lists
            .maps
            .iter()
            .find(|a| a.map.eq_ignore_ascii_case(map))
            .map(|a| {
                let k = a.longname.trim_start_matches('@');
                l.localized.get(k).unwrap_or(k).to_string()
            })
            .unwrap_or_else(|| map.to_string())
    }

    /// The `connect` menu (`Menu_Paint`): its background, the levelshot
    /// for `$levelbriefing`, and its items' fills, materials, text and
    /// load bar.
    fn connect_menu(
        &self,
        l: &Loaded,
        layout: &vcod_cod4::scriptmenu::Layout,
        st: &Status,
        levelshot: Option<&str>,
        sp: &ScreenPlacement,
        out: &mut Vec<HudQuad>,
    ) {
        let place = |r: [f32; 4], align: [i32; 2]| {
            let (x, w) = sp.axis(0, r[0], r[2], align[0]);
            let (y, h) = sp.axis(1, r[1], r[3], align[1]);
            [x, y, w, h]
        };
        let tex = |m: &str| match m.eq_ignore_ascii_case(LEVELBRIEFING) {
            true => levelshot.map(|s| format!("images/{s}.iwi")),
            false => image_of(l, m),
        };
        if let Some(t) = layout.background.as_deref().and_then(tex) {
            let [x, y, w, h] = place(layout.rect, layout.align);
            out.push(quad(x, y, w, h, [1.0; 4], t, false));
        }
        for it in &layout.items {
            if it.dynamic_flags & INTEL_GROUP != 0 {
                continue;
            }
            let [x, y, w, h] = place(it.rect, it.align);
            match it.style {
                STYLE_FILLED => out.push(quad(x, y, w, h, it.back_color, "white".into(), false)),
                STYLE_SHADER => {
                    if let Some(t) = it.background.as_deref().and_then(tex) {
                        out.push(quad(x, y, w, h, it.fore_color, t, false));
                    }
                }
                // `UI_DrawLoadBar`: the material's first `loaded` of its
                // width.
                STYLE_LOADBAR => {
                    if let Some(t) = it.background.as_deref().and_then(tex) {
                        let f = st.loaded;
                        let w = w * f;
                        out.push(HudQuad {
                            verts: [[x, y], [x + w, y], [x + w, y + h], [x, y + h]],
                            uvs: [[0.0, 0.0], [f, 0.0], [f, 1.0], [0.0, 1.0]],
                            rgba: it.fore_color,
                            texture: t,
                        });
                    }
                }
                _ => {}
            }
        }
    }

    /// `UI_DisplayDownloadInfo` (`iw3mp.exe` 0x549ae0..0x54a37f,
    /// cod4-hud.md 18): three translucent black bands, the bar across the
    /// screen in translucent green, the labels and the file's name (shrunk
    /// to fit), then the time left and the rate once 4096 bytes are down,
    /// and `<got> of <size> copied (n%)`.
    fn download_info(
        &self,
        l: &Loaded,
        d: &crate::cod4_loading::Download,
        sp: &ScreenPlacement,
        ring: &mut TimeLeft,
        out: &mut Vec<HudQuad>,
    ) {
        let loc = |k: &str| l.localized.get(k).unwrap_or(k).to_string();
        let f = l.fonts.pick(1, SCALE, sp);
        let grey = [0.75, 0.75, 0.75, 1.0];
        let fill = |out: &mut Vec<HudQuad>, x: f32, y: f32, w: f32, h: f32, rgba: [f32; 4]| {
            let (x, w) = sp.axis(0, x, w, FULL);
            let (y, h) = sp.axis(1, y, h, FULL);
            out.push(quad(x, y, w, h, rgba, "white".into(), false));
        };
        for (dy, h) in [(184.0, 85.0), (185.0, 83.0), (186.0, 81.0)] {
            fill(out, 0.0, Y_START + dy, 640.0, h, [0.0, 0.0, 0.0, 0.2]);
        }
        if d.size > 0 {
            let w = (d.got as f64 / d.size as f64 * 640.0) as i32 as f32;
            let green = [0.0, 1.0, 0.0, 0.15];
            for (dw, dy, dh) in [(2.0, 164.0, 5.0), (1.0, 165.0, 3.0), (0.0, 166.0, 1.0)] {
                fill(out, 0.0, Y_START + dy, w + dw, dh, green);
            }
        }
        for (key, dy) in [
            ("EXE_DOWNLOADING", 210.0),
            ("EXE_EST_TIME_LEFT", 235.0),
            ("EXE_TRANS_RATE", 260.0),
        ] {
            put(sp, f, &loc(key), 24.0, Y_START + dy, SCALE, grey, out);
        }
        let width = font::measure(f, &d.name, SCALE / f.unit_scale());
        let name_scale = if width > NAME_WIDTH {
            (SCALE * NAME_WIDTH / width).max(NAME_SCALE_MIN)
        } else {
            SCALE
        };
        put(
            sp,
            f,
            &d.name,
            SECOND_COLUMN,
            Y_START + 210.0,
            name_scale,
            grey,
            out,
        );
        let words = |k: &str| loc(k);
        let lines = progress_lines(d.got, d.size, d.started.elapsed().as_millis(), ring, &words);
        for (t, x, dy) in lines {
            let x = x.unwrap_or_else(|| centre_x(sp, font::measure(f, &t, SCALE / f.unit_scale())));
            put(sp, f, &t, x, Y_START + dy, SCALE, grey, out);
        }
    }
}

/// The server message's lines as `UI_DrawConnectScreen` splits them
/// (0x54a750..0x54a7d4, cod4-hud.md 18): by bytes, a line ends after its
/// 59th, after a space that falls past its 41st (the space kept), or at
/// the text's end.
fn message_lines(t: &str) -> Vec<String> {
    let bytes = t.as_bytes();
    let mut lines = Vec::new();
    let mut start = 0;
    for (end, &b) in bytes.iter().enumerate().map(|(i, b)| (i + 1, b)) {
        let len = end - start;
        if len >= 59 || end == bytes.len() || (b == b' ' && len > 41) {
            lines.push(String::from_utf8_lossy(&bytes[start..end]).into_owned());
            start = end;
        }
    }
    lines
}

/// The full-screen grid x that centres text `w` grid units wide (drawn
/// at the window's vertical scale) on the screen.
fn centre_x(sp: &ScreenPlacement, w: f32) -> f32 {
    320.0 - 0.5 * w * sp.virtual_to_full(1) / sp.virtual_to_full(0)
}

/// The progress half of `UI_DisplayDownloadInfo` (0x549faa..0x54a37f):
/// each line's text, its x (`None` centred) and its baseline under the
/// top line. Before 4096 bytes `estimating` and the count; then the time
/// left (or `estimating`) and the count higher up, and the rate.
fn progress_lines(
    got: u64,
    size: u64,
    elapsed_ms: u128,
    ring: &mut TimeLeft,
    words: &impl Fn(&str) -> String,
) -> Vec<(String, Option<f32>, f32)> {
    let pct = match size {
        0 => 0,
        n => (got as f64 * 100.0 / n as f64) as i64,
    };
    let copied = format!(
        "{} {} {} {} ({pct}%)",
        readable(got, words),
        words("EXE_OF"),
        readable(size, words),
        words("EXE_COPIED")
    );
    let Some((rate, left)) = rates(got, size, elapsed_ms, ring) else {
        return vec![
            (words("EXE_ESTIMATING"), None, 235.0),
            (copied, None, 340.0),
        ];
    };
    let mut out = match left {
        Some(t) => vec![
            (print_time(t, words), Some(SECOND_COLUMN), 235.0),
            (copied, None, 320.0),
        ],
        None => {
            let line = match size {
                0 => format!("({} {})", readable(got, words), words("EXE_COPIED")),
                _ => copied,
            };
            vec![(words("EXE_ESTIMATING"), None, 235.0), (line, None, 320.0)]
        }
    };
    if rate != 0 {
        let line = format!("{}/{}", readable(rate as u64, words), words("EXE_SECONDS"));
        out.push((line, Some(SECOND_COLUMN), 260.0));
    }
    out
}

/// `UI_ReadableSize` (0x549800): past a GB, MB or KB the whole units,
/// for GB and MB with the remainder's hundredths after a point (the
/// remainder times 100 in 32 bits, truncated), each unit's `EXE_*` word.
fn readable(n: u64, words: &impl Fn(&str) -> String) -> String {
    let n = n as i32;
    let point = |shift: u32, key: &str| {
        let frac = (n & ((1 << shift) - 1)).wrapping_mul(100) >> shift;
        format!("{}.{frac:02} {}", n >> shift, words(key))
    };
    match n {
        n if n > 1 << 30 => point(30, "EXE_GIGABYTE"),
        n if n > 1 << 20 => point(20, "EXE_MEGABYTE"),
        n if n > 1 << 10 => format!("{} {}", n / 1024, words("EXE_KILOBYTE")),
        n => format!("{n} {}", words("EXE_BYTES")),
    }
}

/// `UI_PrintTime` (0x549990): seconds as hours and minutes past an hour,
/// minutes and seconds past a minute, else seconds.
fn print_time(t: i64, words: &impl Fn(&str) -> String) -> String {
    match t {
        t if t > 3600 => format!(
            "{} {} {} {}",
            t / 3600,
            words("EXE_HOURS"),
            t % 3600 / 60,
            words("EXE_MINUTES")
        ),
        t if t > 60 => format!(
            "{} {} {} {}",
            t / 60,
            words("EXE_MINUTES"),
            t % 60,
            words("EXE_SECONDS")
        ),
        t => format!("{t} {}", words("EXE_SECONDS")),
    }
}

/// Text with its baseline at full-screen grid `(x, y)`.
#[allow(clippy::too_many_arguments)]
fn put(
    sp: &ScreenPlacement,
    f: &Font,
    t: &str,
    x: f32,
    y: f32,
    scale: f32,
    rgba: [f32; 4],
    out: &mut Vec<HudQuad>,
) {
    put_glow(sp, f, t, x, y, scale, rgba, [0.0; 4], out);
}

/// [`put`] with a glow ([`glow_text`]).
#[allow(clippy::too_many_arguments)]
fn put_glow(
    sp: &ScreenPlacement,
    f: &Font,
    t: &str,
    x: f32,
    y: f32,
    scale: f32,
    rgba: [f32; 4],
    glow: [f32; 4],
    out: &mut Vec<HudQuad>,
) {
    let px = scale / f.unit_scale();
    let h = (48.0 * scale) as i32 as f32;
    let (rx, _) = sp.axis(0, x, 0.0, FULL);
    let (ry, _) = sp.axis(1, y - h, h, FULL);
    glow_text(f, t, rx, ry, px * sp.virtual_to_full(1), rgba, glow, out);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 1.7's English words (`localized_code_post_gfx_mp`).
    fn words(k: &str) -> String {
        match k {
            "EXE_GIGABYTE" => "GB",
            "EXE_MEGABYTE" => "MB",
            "EXE_KILOBYTE" => "KB",
            "EXE_BYTES" => "bytes",
            "EXE_HOURS" => "hr",
            "EXE_MINUTES" => "min",
            "EXE_SECONDS" => "sec",
            "EXE_ESTIMATING" => "estimating",
            "EXE_OF" => "of",
            "EXE_COPIED" => "copied",
            k => k,
        }
        .to_string()
    }

    /// Lines break at the first space past 40 characters, keeping it, and
    /// at 59 characters with none.
    /// Fails with the whole new text when `actual` moved off the pin.
    fn pin(tag: &str, actual: &str, expected: &str) {
        assert!(actual == expected, "{tag} moved:\n{actual}");
    }

    #[test]
    fn message_wrap_pins() {
        let s: String = crate::cod4_hud::tests::wrap_corpus()
            .iter()
            .map(|t| format!("{t:?} => {:?}\n", message_lines(t)))
            .collect();
        pin(
            "message",
            &s,
            r##""" => []
"short" => ["short"]
"^" => ["^"]
"trailing caret^" => ["trailing caret^"]
"^^1 caret pair ^x not a code" => ["^^1 caret pair ^x not a code"]
"  leading and trailing spaces  " => ["  leading and trailing spaces  "]
"^1word word word word word word word word word word word word  tail" => ["^1word word word word word word word word ", "word word word word  tail"]
"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa" => ["aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"]
"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa" => ["aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"]
"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa" => ["aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"]
"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa" => ["aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"]
"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa" => ["aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"]
"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa" => ["aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"]
"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa" => ["aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"]
"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa" => ["aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"]
"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa" => ["aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa", "a"]
"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa" => ["aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa", "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa", "aa"]
"xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx yyyyyyyyyyyyyyyyyyyyyyyyyyyyyy" => ["xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx ", "yyyyyyyyyyyyyyyyyyyyyyyyyyyyyy"]
"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa ^3bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb" => ["aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa ", "^3bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb", "bbb"]
"^1a^2b^3c^4d^5e^6f^7g^8h^9i^0j ^1a^2b^3c^4d^5e^6f^7g^8h^9i^0j ^1a^2b^3c^4d^5e^6f^7g^8h^9i^0j ^1a^2b^3c^4d^5e^6f^7g^8h^9i^0j ^1a^2b^3c^4d^5e^6f^7g^8h^9i^0j ^1a^2b^3c^4d^5e^6f^7g^8h^9i^0j " => ["^1a^2b^3c^4d^5e^6f^7g^8h^9i^0j ^1a^2b^3c^4d^5e^6f^7g^8h^9i^", "0j ^1a^2b^3c^4d^5e^6f^7g^8h^9i^0j ^1a^2b^3c^4d^5e^6f^7g^8h^", "9i^0j ^1a^2b^3c^4d^5e^6f^7g^8h^9i^0j ^1a^2b^3c^4d^5e^6f^7g^", "8h^9i^0j "]
"ccccccccccccccccccccccccccccccccccccccccccccc   ddddddddddddddddddddddddddddddddddddddddddddd" => ["ccccccccccccccccccccccccccccccccccccccccccccc ", "  ddddddddddddddddddddddddddddddddddddddddddddd"]
"cccccccccccccccccccccccccccccc dddddddddddddddddddddddddddddddddddddddd" => ["cccccccccccccccccccccccccccccc dddddddddddddddddddddddddddd", "dddddddddddd"]
"Привет мир, это длинное сообщение Привет мир, это длинное сообщение Привет мир, это длинное сообщение " => ["Привет мир, это длинное ", "сообщение Привет мир, это ", "длинное сообщение Привет ", "мир, это длинное сообщение "]
"日本語のテキストは空白がない日本語のテキストは空白がない日本語のテキストは空白がない日本語のテキストは空白がない" => ["日本語のテキストは空白がない日本語のテ�", "�ストは空白がない日本語のテキストは空白�", "��ない日本語のテキストは空白がない"]
"eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeäffffffffff" => ["eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeä", "ffffffffff"]
"^2Shooter^7: ^7gg, nice shot from across the whole map, really nice one" => ["^2Shooter^7: ^7gg, nice shot from across the ", "whole map, really nice one"]
"##,
        );
    }

    #[test]
    fn the_message_wraps_as_1_7_splits_it() {
        assert_eq!(message_lines("short"), vec!["short"]);
        let long = format!("{} tail end", "a".repeat(45));
        assert_eq!(
            message_lines(&long),
            vec![format!("{} ", "a".repeat(45)), "tail end".to_string()]
        );
        let solid = "b".repeat(70);
        assert_eq!(message_lines(&solid), vec!["b".repeat(59), "b".repeat(11)]);
        // A space at 40 characters or under does not end the line.
        let early = format!("{} {}", "c".repeat(30), "d".repeat(40));
        assert_eq!(message_lines(&early)[0].len(), 59);
    }

    /// `UI_ReadableSize` truncates the hundredths and gives KB whole;
    /// `UI_PrintTime` splits at the minute and the hour.
    #[test]
    fn sizes_and_times_read_as_1_7_prints_them() {
        assert_eq!(readable(1000, &words), "1000 bytes");
        assert_eq!(readable(1025, &words), "1 KB");
        assert_eq!(readable(1_572_863, &words), "1.49 MB");
        assert_eq!(readable(5 << 20, &words), "5.00 MB");
        assert_eq!(print_time(45, &words), "45 sec");
        assert_eq!(print_time(125, &words), "2 min 5 sec");
        assert_eq!(print_time(7322, &words), "2 hr 2 min");
    }

    /// Under 4096 bytes the screen estimates; past it at 3 s the rate is
    /// the bytes over 3, and the time left the 80-frame mean, the first
    /// frame's estimate over 80.
    #[test]
    fn the_rate_and_time_left_wait_for_4096_bytes() {
        let mut ring = TimeLeft::default();
        let lines = progress_lines(2000, 1 << 20, 3000, &mut ring, &words);
        assert_eq!(lines[0], ("estimating".into(), None, 235.0));
        // Exactly 1 MB is not past it: KB.
        assert_eq!(lines[1].0, "1 KB of 1024 KB copied (0%)");
        assert_eq!(lines[1].2, 340.0);
        // 300 KB of 4 MB in 3 s: 100 KB a second, 40 s whole, 38 left,
        // the mean of one estimate and 79 empty slots 0.
        let got = 300 * 1024;
        let lines = progress_lines(got, 4 << 20, 3500, &mut ring, &words);
        let est = (4 << 20) / (got / 3) - (4 << 20) / (got / 3) * 300 / 4096;
        assert_eq!(
            lines,
            vec![
                (format!("{} sec", est / 80), Some(SECOND_COLUMN), 235.0),
                ("300 KB of 4.00 MB copied (7%)".into(), None, 320.0),
                ("100 KB/sec".into(), Some(SECOND_COLUMN), 260.0),
            ]
        );
    }
}
