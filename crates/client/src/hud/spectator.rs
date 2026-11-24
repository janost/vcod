//! A spectator's lines while the board is down: the keys that cycle and
//! stop the follow, and whom it follows (cgame 1.1 0x30018550 and
//! 0x30018710, 1.5 0x30019740 and 0x300198e0, UO 0x3001bd50 and
//! 0x3001bee0; docs/research/cod11-spectator-follow.md 11.2).

use super::HudQuad;
use super::font::{Slot, TextStyle, UiFonts};
use super::hudelem::Virtual;
use super::player::{menu_text_rgba, text_width};
use vcod_common::localize::Localized;
use vcod_common::net::flags::{
    PMF_FOLLOW, PMF_FORCED_FOLLOW, PMF_MAY_CYCLE, PMF_MAY_FREE, PMF_OWN_VIEW,
};

/// Font 0 at 0.2083 (0x3e555555) for the key lines, 1/3 for the name.
const KEY_SCALE: f32 = 0.208_333_33;
const NAME_SCALE: f32 = 1.0 / 3.0;
const KEY_X: f32 = 240.0;
const KEY_LINE: f32 = 10.0;

/// Which game's rules the lines follow.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Rules {
    V1,
    V15,
    Uo,
}

/// The key lines `pm_flags` (vcod's layout) allows: whether the cycle pair
/// and the stop line draw, and the first line's baseline. 1.1 draws the
/// pair unless the follow is forced or the view is the client's own, and
/// the stop line while following; 1.5 and UO draw each on its permission
/// bit, the cycle's on a follow being the absence of the forced bit
/// (`flags::pm_flags_v6_to_v1`).
fn key_lines(rules: Rules, pm_flags: i32) -> Option<(bool, bool, f32)> {
    match rules {
        Rules::V1 => (pm_flags & (PMF_FORCED_FOLLOW | PMF_OWN_VIEW) == 0).then_some((
            true,
            pm_flags & PMF_FOLLOW != 0,
            426.0,
        )),
        Rules::V15 | Rules::Uo => {
            let cycle = pm_flags & PMF_MAY_CYCLE != 0
                || pm_flags & (PMF_FOLLOW | PMF_FORCED_FOLLOW) == PMF_FOLLOW;
            let stop = pm_flags & PMF_MAY_FREE != 0;
            let y = if rules == Rules::Uo { 416.0 } else { 426.0 };
            (cycle || stop).then_some((cycle, stop, y))
        }
    }
}

/// What [`lines`] draws: each line's text and baseline.
#[derive(Debug, Default)]
pub struct Lines {
    pub keys: Vec<(String, f32)>,
    pub following: Option<(String, f32)>,
}

/// The lines as text and baseline: `descriptive` is `cg_descriptiveText`,
/// `name` the followed client's (`None` without client info, drawn `?`).
/// A cycle key reads as its command when unbound; the stop line, on
/// `toggle cl_run` else `+speed`, goes when neither is bound.
pub fn lines(
    rules: Rules,
    pm_flags: i32,
    descriptive: bool,
    name: Option<&str>,
    loc: &Localized,
    bound: &dyn Fn(&str) -> Option<String>,
) -> Lines {
    let line = |key: &str, cmd: &str| {
        let fmt = loc.get(key).unwrap_or(key);
        fmt.replacen("%s", cmd, 1)
    };
    let mut keys = Vec::new();
    if let Some((cycle, stop, mut y)) = key_lines(rules, pm_flags).filter(|_| descriptive) {
        if cycle {
            for (key, cmd) in [
                ("CGAME_FOLLOWNEXTPLAYER", "+attack"),
                ("CGAME_FOLLOWPREVIOUSPLAYER", "+melee"),
            ] {
                let k = bound(cmd).unwrap_or_else(|| cmd.to_string());
                keys.push((line(key, &k), y));
                y += KEY_LINE;
            }
        }
        if stop && let Some(k) = bound("toggle cl_run").or_else(|| bound("+speed")) {
            keys.push((line("CGAME_FOLLOWSTOP", &k), y));
        }
    }
    let following = (pm_flags & PMF_FOLLOW != 0).then(|| {
        let text = loc.message(&format!("CGAME_FOLLOWING\u{15}: {}", name.unwrap_or("?")));
        (text, if rules == Rules::Uo { 404.0 } else { 414.0 })
    });
    Lines { keys, following }
}

/// Draws [`lines`]: the keys at x 240, the name centred, style 3, white.
pub fn build(
    Lines { keys, following }: &Lines,
    fonts: &UiFonts,
    v: &Virtual,
    out: &mut Vec<HudQuad>,
) {
    let style = TextStyle::SHADOWED;
    let font = fonts.pick(Slot::Default, KEY_SCALE, 480.0 * v.scale);
    for (text, y) in keys {
        menu_text_rgba(font, text, (KEY_X, *y), KEY_SCALE, [1.0; 4], style, v, out);
    }
    if let Some((text, y)) = following {
        let font = fonts.pick(Slot::Default, NAME_SCALE, 480.0 * v.scale);
        let w = text_width(font, text, NAME_SCALE).trunc();
        let x = (640.0 - w) * 0.5;
        menu_text_rgba(font, text, (x, *y), NAME_SCALE, [1.0; 4], style, v, out);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn loc() -> Localized {
        let mut l = Localized::default();
        for (k, v) in [
            (
                "CGAME_FOLLOWNEXTPLAYER",
                "- Press [%s] to follow next player",
            ),
            (
                "CGAME_FOLLOWPREVIOUSPLAYER",
                "- Press [%s] to follow previous player",
            ),
            ("CGAME_FOLLOWSTOP", "- Press [%s] to stop following"),
            ("CGAME_FOLLOWING", "following"),
        ] {
            l.insert(k, v);
        }
        l
    }

    fn texts(rules: Rules, pm_flags: i32, bound: &dyn Fn(&str) -> Option<String>) -> Vec<String> {
        let Lines { keys, following } = lines(rules, pm_flags, true, Some("Bob"), &loc(), bound);
        keys.into_iter()
            .chain(following)
            .map(|(t, y)| format!("{y} {t}"))
            .collect()
    }

    /// 1.1 (0x30018550): the pair for any view but a forced follow or the
    /// client's own, the stop line only while following and bound; an
    /// unbound cycle key reads as its command.
    #[test]
    fn the_1_1_key_lines_follow_its_view_bits() {
        let run = |c: &str| (c == "toggle cl_run").then(|| "SHIFT".to_string());
        let mouse = |c: &str| (c == "+attack").then(|| "MOUSE1".to_string());
        assert_eq!(
            texts(Rules::V1, 0, &mouse),
            [
                "426 - Press [MOUSE1] to follow next player",
                "436 - Press [+melee] to follow previous player",
            ]
        );
        assert_eq!(
            texts(Rules::V1, PMF_FOLLOW, &run),
            [
                "426 - Press [+attack] to follow next player",
                "436 - Press [+melee] to follow previous player",
                "446 - Press [SHIFT] to stop following",
                "414 following: Bob",
            ]
        );
        assert_eq!(
            texts(Rules::V1, PMF_FOLLOW | PMF_FORCED_FOLLOW, &run),
            ["414 following: Bob"]
        );
        assert!(texts(Rules::V1, PMF_OWN_VIEW, &run).is_empty());
        // Neither stop key bound: no stop line.
        assert_eq!(texts(Rules::V1, PMF_FOLLOW, &|_| None).len(), 3);
    }

    /// 1.5 (0x30019740) and UO (0x3001bd50) draw on the permissions; UO's
    /// lines sit ten units higher (0x3001bda4, 0x3001bf7d).
    #[test]
    fn the_1_5_and_uo_key_lines_follow_the_permissions() {
        let run = |c: &str| (c == "+speed").then(|| "SHIFT".to_string());
        let stop = "- Press [SHIFT] to stop following";
        assert!(texts(Rules::V15, 0, &run).is_empty());
        assert_eq!(
            texts(Rules::V15, PMF_MAY_FREE, &run),
            [format!("426 {stop}")]
        );
        assert_eq!(
            texts(Rules::Uo, PMF_FOLLOW | PMF_MAY_FREE, &run),
            [
                "416 - Press [+attack] to follow next player".to_string(),
                "426 - Press [+melee] to follow previous player".into(),
                format!("436 {stop}"),
                "404 following: Bob".into(),
            ]
        );
        assert_eq!(texts(Rules::V15, PMF_MAY_CYCLE, &run).len(), 2);
        assert_eq!(
            texts(Rules::V15, PMF_FOLLOW | PMF_FORCED_FOLLOW, &run),
            ["414 following: Bob"]
        );
    }
}
