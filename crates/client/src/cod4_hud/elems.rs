//! The playerstate's hudelems: their text, tweens, colours, pulse and
//! material sizes.

use vcod_cod4::hud::{self as h, HudElem};

use super::assets::Loaded;
use super::text::glow_text;
use super::{Cod4Hud, image_of, quad};
use crate::console::shell::Shell;
use crate::hud::cod2elem::{self, ScreenPlacement};
use crate::hud::hudelem::format_g;
use crate::hud::{HudQuad, font};
use crate::play::cod4::{Cod4Session, client_name};

impl Cod4Hud {
    /// One pass of `CG_Draw2dHudElems`: the elements
    /// of that pass by `sort`, text through the
    /// label and the text or value, materials by their configstring's
    /// image. Waypoints (in the world) are left out.
    #[expect(clippy::too_many_arguments)]
    pub(super) fn draw_elems(
        &self,
        l: &Loaded,
        s: &Cod4Session,
        elems: &[HudElem],
        foreground: bool,
        sp: &ScreenPlacement,
        now: i32,
        shell: &Shell,
        out: &mut Vec<HudQuad>,
    ) {
        let mut order: Vec<&HudElem> = elems
            .iter()
            .filter(|e| (e.flags & h::HE_FLAG_FOREGROUND != 0) == foreground)
            .filter(|e| e.kind != h::kind::WAYPOINT)
            .collect();
        order.sort_by(|a, b| a.sort.total_cmp(&b.sort));
        let k = sp.virtual_to_real[1];
        for e in order {
            let mut rgba = colour(e.color, e.from_color, e.fade_start_time, e.fade_time, now);
            if rgba[3] <= 0.0 {
                continue;
            }
            let at = as_cod2(e);
            if e.kind == h::kind::MATERIAL {
                let name = self.configstring(s, h::CS_MATERIALS + e.material as u16);
                let Some(tex) = image_of(l, &name) else {
                    continue;
                };
                let (w, h) = material_size(e, now);
                let [x, y] = cod2elem::position(sp, &at, (w, h), now);
                let rw = sp.axis(0, 0.0, w, e.align_screen >> 3 & 7).1;
                let rh = sp.axis(1, 0.0, h, e.align_screen & 7).1;
                out.push(quad(x, y, rw, rh, rgba, tex, false));
                continue;
            }
            let Some(t) = self.elem_text(s, e, now, shell) else {
                continue;
            };
            let t = pulse(e, &t, now, &mut rgba);
            if t.is_empty() || rgba[3] <= 0.0 {
                continue;
            }
            let (font_enum, base) = match e.font {
                1 => (4, 0.5),
                2 => (5, 1.0 / 3.0),
                3 => (6, 0.25),
                4 => (2, 0.25),
                5 => (3, 0.25),
                _ => (0, 0.25),
            };
            let scale = base * e.font_scale;
            // `GetHudElemInfo` hands `UI_GetFontHandle` the real scale,
            // which scales it again.
            let f = l.fonts.pick(font_enum, scale * k, sp);
            let px = scale / f.unit_scale();
            let w = font::measure(f, &t, px);
            let h = (48.0 * scale) as i32 as f32;
            let [x, y] = cod2elem::position(sp, &at, (w, h), now);
            // `CG_DrawHudElemText`: the element's `glowColor`, unfaded.
            let glow = e.glow_color.map(|c| f32::from(c) / 255.0);
            glow_text(f, &t, x, y, px * k, rgba, glow, out);
        }
    }

    /// `GetHudElemInfo`'s label and text, `None` for a kind drawn
    /// otherwise.
    pub(crate) fn elem_text(
        &self,
        s: &Cod4Session,
        e: &HudElem,
        now: i32,
        shell: &Shell,
    ) -> Option<String> {
        // `CG_TranslateHudElemMessage`: localized, then each `[{+cmd}]`
        // as its keys (the directive parse, iw3mp.exe 0x438040).
        let loc = |i: i32| -> String {
            if i == 0 {
                return String::new();
            }
            let key = self.configstring(s, h::CS_LOCALIZED_STRINGS + i as u16);
            h::replace_directives(&self.message(&key), &|d| self.key_binding(shell, d))
        };
        let t = match e.kind {
            h::kind::TEXT => loc(e.text),
            h::kind::VALUE => format_g(e.value),
            h::kind::PLAYERNAME => {
                let n = e.value.round() as u32;
                s.client()
                    .snapshots
                    .last()
                    .and_then(|x| x.clients.iter().find(|c| c.0[0] == n))
                    .map(client_name)
                    .unwrap_or_default()
            }
            h::kind::TIMER_DOWN | h::kind::TIMER_UP => {
                let ms = elem_time(e, now) / 1000;
                let (hh, mm, ss) = (ms / 3600, ms % 3600 / 60, ms % 60);
                if hh != 0 {
                    format!("{hh}:{mm:02}:{ss:02}")
                } else {
                    format!("{mm}:{ss:02}")
                }
            }
            h::kind::TENTHS_TIMER_DOWN | h::kind::TENTHS_TIMER_UP => {
                let t = elem_time(e, now) / 100;
                let (hh, mm, ss, tt) = (t / 36000, t % 36000 / 600, t % 600 / 10, t % 10);
                if hh != 0 {
                    format!("{hh}:{mm:02}:{ss:02}.{tt}")
                } else {
                    format!("{mm}:{ss:02}.{tt}")
                }
            }
            _ => return None,
        };
        Some(h::consolidate(&loc(e.label), &t))
    }
}

/// How far into a hudelem tween `now` is, 0..=1, or `None` when there is
/// none or it is over (the element then shows its end value). A tween
/// is a pair of fields, a start time and a length in ms; `fadeOverTime`,
/// `scaleOverTime` and `moveOverTime` each write one (cod4-hud.md 5).
fn tween(start: i32, length: i32, now: i32) -> Option<f32> {
    let into = now - start;
    (length > 0 && into < length).then(|| (into.max(0) as f32 / length as f32).min(1.0))
}

/// `from` toward `to` by `frac`, `to` once the tween is over.
fn tweened<const N: usize>(from: [f32; N], to: [f32; N], frac: Option<f32>) -> [f32; N] {
    match frac {
        Some(f) => std::array::from_fn(|i| from[i] + (to[i] - from[i]) * f),
        None => to,
    }
}

/// The element's colour: `fromColor` to `color` over `fadeTime` from
/// `fadeStartTime`, each byte over 255. INFERRED (cod4-hud.md 5).
pub(super) fn colour(to: [u8; 4], from: [u8; 4], start: i32, time: i32, now: i32) -> [f32; 4] {
    let unit = |c: [u8; 4]| c.map(|b| f32::from(b) / 255.0);
    tweened(unit(from), unit(to), tween(start, time, now))
}

/// A material's or a waypoint's drawn width and height: `fromWidth`,
/// `fromHeight` to `width`, `height` over `scaleTime` from
/// `scaleStartTime` (`scaleOverTime`, the stock progress bar's fill), in
/// virtual units. INFERRED (cod4-hud.md 5).
pub(super) fn material_size(e: &HudElem, now: i32) -> (f32, f32) {
    let size = |w: i32, h: i32| [w as f32, h as f32];
    let [w, h] = tweened(
        size(e.from_width, e.from_height),
        size(e.width, e.height),
        tween(e.scale_start_time, e.scale_time, now),
    );
    (w, h)
}

/// A timer's ms at `now`: an up timer's since `time`, a down timer's
/// left until it, rounded up to the unit it shows (a second, a tenth);
/// never below 0.
fn elem_time(e: &HudElem, now: i32) -> i32 {
    let unit = match e.kind {
        h::kind::TIMER_DOWN => 1000,
        h::kind::TENTHS_TIMER_DOWN => 100,
        _ => return (now - e.time).max(0),
    };
    (e.time - now + unit - 1).max(0)
}

/// `setPulseFX`: letters come in one per `fxLetterTime` from
/// `fxBirthTime`, and from `fxDecayStartTime` the text fades over
/// `fxDecayDuration` (an approximation of the cgame's per-letter decay).
fn pulse(e: &HudElem, text: &str, now: i32, rgba: &mut [f32; 4]) -> String {
    if e.fx_birth_time == 0 || e.fx_letter_time <= 0 {
        return text.to_string();
    }
    let t = now - e.fx_birth_time;
    if t < 0 {
        return String::new();
    }
    if e.fx_decay_start_time > 0 && t > e.fx_decay_start_time {
        let d = (t - e.fx_decay_start_time) as f32 / e.fx_decay_duration.max(1) as f32;
        rgba[3] *= (1.0 - d).clamp(0.0, 1.0);
    }
    let shown = (t / e.fx_letter_time + 1) as usize;
    text.chars().take(shown).collect()
}

/// The fields `cod2elem::position` places by, which CoD4 packs as CoD2.
fn as_cod2(e: &HudElem) -> vcod_cod2::hudelem::HudElem {
    vcod_cod2::hudelem::HudElem {
        kind: e.kind,
        x: e.x,
        y: e.y,
        align_org: e.align_org,
        align_screen: e.align_screen,
        from_x: e.from_x,
        from_y: e.from_y,
        from_align_org: e.from_align_org,
        from_align_screen: e.from_align_screen,
        move_start_time: e.move_start_time,
        move_time: e.move_time,
        ..Default::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use super::super::tests::replay;

    /// `setPulseFX`: nothing before the birth, a letter per
    /// `fxLetterTime` from it, and past `fxDecayStartTime` the text fades
    /// over `fxDecayDuration`.
    #[test]
    fn pulse_fx_types_the_letters_in_then_decays() {
        let e = HudElem {
            fx_birth_time: 1000,
            fx_letter_time: 100,
            fx_decay_start_time: 2000,
            fx_decay_duration: 1000,
            ..Default::default()
        };
        let at = |now| {
            let mut rgba = [1.0; 4];
            (pulse(&e, "HELLO", now, &mut rgba), rgba[3])
        };
        assert_eq!(at(999), (String::new(), 1.0));
        assert_eq!(at(1000), ("H".into(), 1.0));
        assert_eq!(at(1250), ("HEL".into(), 1.0));
        assert_eq!(at(3000), ("HELLO".into(), 1.0));
        assert_eq!(at(3500), ("HELLO".into(), 0.5));
        assert_eq!(at(4500), ("HELLO".into(), 0.0));
        let plain = HudElem::default();
        assert_eq!(pulse(&plain, "HELLO", 0, &mut [1.0; 4]), "HELLO");
    }

    /// A value hudelem prints as C's `%g`: six significant digits, an
    /// exponent from 1e6 up.
    #[test]
    fn a_value_hudelem_prints_like_percent_g() {
        assert_eq!(format_g(5.0), "5");
        assert_eq!(format_g(0.1), "0.1");
        assert_eq!(format_g(-2.5), "-2.5");
        assert_eq!(format_g(1234.5678), "1234.57");
        assert_eq!(format_g(1_000_000.0), "1e+06");
    }

    /// The hudelem tweens at a few times: `fadeOverTime`'s colour,
    /// `scaleOverTime`'s material and waypoint sizes, and the timers' ms.
    #[test]
    fn hudelem_tweens_over_time() {
        let e = HudElem {
            width: 100,
            height: 20,
            from_width: 10,
            from_height: 4,
            scale_start_time: 1000,
            scale_time: 400,
            time: 5000,
            ..Default::default()
        };
        let sizes: Vec<_> = [900, 1000, 1100, 1300, 1400, 2000]
            .map(|t| (material_size(&e, t), material_size(&e, t).1))
            .into();
        assert_eq!(
            sizes,
            [
                ((10.0, 4.0), 4.0),
                ((10.0, 4.0), 4.0),
                ((32.5, 8.0), 8.0),
                ((77.5, 16.0), 16.0),
                ((100.0, 20.0), 20.0),
                ((100.0, 20.0), 20.0),
            ]
        );
        let still = HudElem { scale_time: 0, ..e };
        assert_eq!(material_size(&still, 1100), (100.0, 20.0));
        assert_eq!(material_size(&still, 1100).1, 20.0);
        let (to, from) = ([255, 0, 102, 255], [0, 255, 0, 0]);
        assert_eq!(colour(to, from, 1000, 500, 900), [0.0, 1.0, 0.0, 0.0]);
        assert_eq!(colour(to, from, 1000, 500, 1250), [0.5, 0.5, 0.2, 0.5]);
        assert_eq!(colour(to, from, 1000, 500, 1500), [1.0, 0.0, 0.4, 1.0]);
        assert_eq!(colour(to, from, 1000, 0, 900), [1.0, 0.0, 0.4, 1.0]);
        let timer = |kind| HudElem { kind, ..e };
        let ms = |kind, now| elem_time(&timer(kind), now);
        assert_eq!(ms(h::kind::TIMER_DOWN, 3000), 2999);
        assert_eq!(ms(h::kind::TIMER_DOWN, 4001), 1998);
        assert_eq!(ms(h::kind::TIMER_DOWN, 6000), 0);
        assert_eq!(ms(h::kind::TENTHS_TIMER_DOWN, 4950), 149);
        assert_eq!(ms(h::kind::TENTHS_TIMER_DOWN, 5200), 0);
        assert_eq!(ms(h::kind::TIMER_UP, 5300), 300);
        assert_eq!(ms(h::kind::TENTHS_TIMER_UP, 4000), 0);
        assert_eq!(ms(h::kind::CLOCK_DOWN, 5300), 300);
    }

    /// Retail's prematch (cod4-hud.md 13): `matchStartTimer`'s text and
    /// down timer, in the objective font at the centre, "Waiting for
    /// teams to be ready..." over the 15 s wait, then "Match begins in:"
    /// over the 5 s count, both gone at 19800; both `hideWhenInMenu`, so
    /// a menu up takes their glyphs off the screen.
    #[test]
    fn prematch_capture_draws_the_wait_text_and_the_countdown() {
        let shell = Shell::new();
        let mut seen = std::collections::BTreeMap::<String, (i32, i32)>::new();
        let mut hidden = None;
        let (mut before, mut after) = (0, 0);
        let (hud, _) = replay("play-1.7-prematch.txt", |hud, s, now| {
            let Some(snap) = s.client().snapshots.last() else {
                return;
            };
            for e in h::hud_elems(&snap.ps).iter().filter(|e| e.sort == 1001.0) {
                assert_ne!(e.flags & h::HE_FLAG_HIDE_WHEN_IN_MENU, 0);
                if let Some(t) = hud.elem_text(s, e, now, &shell) {
                    seen.entry(t).or_insert((now, now)).1 = now;
                }
            }
            if hidden.is_none() && now > 10_000 && hud.loaded.as_ref().is_some_and(Option::is_some)
            {
                let size = (640.0, 480.0);
                let shown = hud.build(s, size, now, None, &shell).len();
                let gs = s.gamestate().unwrap().clone();
                s.menus.command(b"t 8", &gs);
                assert!(s.menus.open().is_some());
                let menu = hud.build(s, size, now, None, &shell).len();
                s.menus.command(b"u", &gs);
                hidden = Some(shown - menu);
            }
            // `ui_hud_hardcore` reads 1 until the prematch's end clears it
            // (`d 247 0`, level time 19850): no ammo counter till then.
            if let Some(Some(l)) = &hud.loaded {
                let snap = s.client().snapshots.last().unwrap();
                if hud.hardcore(s) {
                    before = now;
                    let n = hud.weapon_info(l, s, snap, 0).len();
                    assert!(now < 9300 || n > 0, "the weapon info is there to hide");
                } else if after == 0 && now > 9200 {
                    after = now;
                }
            }
        });
        if hud.loaded.is_none() {
            return;
        }
        let span = |t: &str| {
            *seen
                .get(t)
                .unwrap_or_else(|| panic!("no {t:?} in {seen:?}"))
        };
        let wait = span("Waiting for teams to be ready...");
        let begins = span("Match begins in: ");
        assert!(
            wait.0 < 7000 && (14_700..14_800).contains(&wait.1),
            "{wait:?}"
        );
        assert!((14_700..14_800).contains(&begins.0), "{begins:?}");
        assert!((19_700..19_800).contains(&begins.1), "{begins:?}");
        // The 15 s wait counts from 0:09 at the join; the 5 s one from 0:05.
        assert_eq!(span("0:09").0, wait.0);
        assert!(span("0:05").1 > begins.0 && span("0:01").1 == begins.1);
        assert_eq!(seen.len(), 11, "{seen:?}");
        // The wait line's glyphs and its timer's (31 + 4 letters, a shadow
        // each) leave with a menu up, and the spawn's other
        // `hideWhenInMenu` elements with them.
        assert!(hidden.is_some_and(|n| n >= 2 * (31 + 4)), "{hidden:?}");
        assert!(
            (19_700..20_000).contains(&before) && after > before,
            "{before} {after}"
        );
    }
}
