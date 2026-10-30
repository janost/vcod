//! The HUD's zone menus through a client joined to vcod-server's `war` on
//! mp_crash (docs/research/cod4-hud.md 23): the bars, the `scoreboard`
//! menu `+scores` opens, the mantle hint and the invalid command line.

use super::*;
use crate::cod4_hud::Cod4Hud;
use crate::console::shell::Shell;
use crate::play::input::Action;

/// The client's frame for `ms`: the session stepped, its events into the
/// HUD; returns the HUD's clock.
fn run(s: &mut Cod4Session, hud: &mut Cod4Hud, t0: Instant, ms: u64) -> i32 {
    let end = since(t0) + Duration::from_millis(ms);
    let mut now = 0;
    loop {
        s.step(since(t0).as_secs_f64() * 1000.0).unwrap();
        let fired = s.take_fired();
        now = s.scene().map_or(now, |x| x.render_time);
        hud.update(s, &fired, now);
        clk_sleep(Duration::from_millis(16));
        if since(t0) >= end {
            return now;
        }
    }
}

/// What the gate saw, in order.
#[derive(Debug)]
struct Seen {
    xp_bar: bool,
    score_bars: bool,
    board: Vec<String>,
    board_line: bool,
    xp_under_board: bool,
    mantle: Option<String>,
    mantle_icon: bool,
    no_frag: Option<(String, f32)>,
    breath: Option<crate::cod4_hud::Hint>,
}

/// A joined player's HUD draws the `xpbar` and `scorebars` menus; Tab
/// opens the `scoreboard` menu (its header line, the game type or the
/// score line, the team scores) and hides the XP bar under it
/// (`uiActive()`); in front of mp_crash's brush 10804 the mantle hint
/// names its key and draws `hint_mantle`; a second frag press with the
/// one frag thrown blinks `WEAPON_NO_FRAG_GRENADE`; the M16's sight,
/// which has no overlay, shows no hold breath hint.
#[test]
fn cod4_live_bars_board_and_hints_draw_from_their_menus() {
    let sets = [
        ("sv_cheats", "1"),
        ("scr_game_playerwaittime", "1"),
        ("scr_game_matchstarttime", "1"),
    ];
    let Some(r) = live_join_on("mp_crash", crash_world(), &sets, |s, spawned, t0, _| {
        spawned.as_ref()?;
        let install = vcod_cod4::testing::cod4_install()?;
        let mut hud = Cod4Hud::default();
        hud.load(&install.root().join("main"), "mp_crash");
        let shell = Shell::new();
        let size = (640.0, 480.0);
        let drawn = |hud: &Cod4Hud, s: &Cod4Session, now: i32, m: &str| {
            let tex = hud.texture_of(m);
            hud.build(s, size, now, None, &shell)
                .iter()
                .any(|q| Some(&q.texture) == tex.as_ref())
        };
        let mut now = run(s, &mut hud, t0, 2500);
        let xp_bar = drawn(&hud, s, now, "hud_xpticker480");
        let score_bars = drawn(&hud, s, now, "scorebar_backdrop");
        hud.set_scores(true, now);
        now = run(s, &mut hud, t0, 300);
        let board = hud.zone_menu_texts(s, "scoreboard", &shell, now);
        let board_line = drawn(&hud, s, now, "line_horizontal_scorebar");
        let xp_under_board = drawn(&hud, s, now, "hud_xpticker480");
        hud.set_scores(false, now);
        // `setviewpos` turns the view by `delta_angles` off the cmd's.
        s.view = [0.0, 0.0];
        run(s, &mut hud, t0, 100);
        s.command("setviewpos -204 1680 290 90");
        run(s, &mut hud, t0, 1000);
        // Up against the wall: the ledge probe reaches 16 past it.
        s.input.key(Action::Forward, true);
        now = run(s, &mut hud, t0, 400);
        let mantle = hud.hint_at(s, "mantlehint", &shell, now).map(|h| h.text);
        let mantle_icon = drawn(&hud, s, now, "hint_mantle");
        s.input.key(Action::Forward, false);
        // The one frag, then a press with none left.
        for wait in [3000, 300] {
            s.input.key(Action::Frag, true);
            run(s, &mut hud, t0, 600);
            s.input.key(Action::Frag, false);
            now = run(s, &mut hud, t0, wait);
        }
        let no_frag = hud
            .hint_at(s, "invalidcmdhint", &shell, now)
            .map(|h| (h.text, h.rgba[3]));
        // The M16's sight has no overlay: no hold breath hint.
        s.input.key(Action::Ads, true);
        now = run(s, &mut hud, t0, 800);
        let breath = hud.hint_at(s, "holdbreathhint", &shell, now);
        s.input.key(Action::Ads, false);
        Some(Seen {
            xp_bar,
            score_bars,
            board,
            board_line,
            xp_under_board,
            mantle,
            mantle_icon,
            no_frag,
            breath,
        })
    }) else {
        return;
    };
    let seen = r.expect("never spawned");
    assert!(seen.xp_bar && seen.score_bars, "{seen:?}");
    assert!(seen.board_line && !seen.xp_under_board, "{seen:?}");
    // Both team scores, the game type (`timeLeft() % 8 < 4`) and the
    // countdown of war's 10 minutes.
    let board: Vec<&str> = seen.board.iter().map(String::as_str).collect();
    assert_eq!(board[..3], ["0", "0", "Team Deathmatch"], "{seen:?}");
    assert!(board[3].starts_with(" 9:5"), "{seen:?}");
    let mantle = seen.mantle.as_deref().unwrap_or_default();
    assert!(mantle.contains("Space") && seen.mantle_icon, "{seen:?}");
    let (text, alpha) = seen.no_frag.clone().expect("no invalid command hint");
    assert!(text.to_ascii_lowercase().contains("grenade"), "{seen:?}");
    assert!((0.0..1.0).contains(&alpha), "{seen:?}");
    assert!(seen.breath.is_none(), "{seen:?}");
}

/// The `scorebars` menu while our team trails (`b` from the server with
/// axis ahead): the backdrop's `rectH` reads -64 and `UI_DrawHandlePic`
/// flips it in place, from its own corner, so it stays round the team
/// icon (cod4-hud.md 23). The playtest drew it a backdrop's height above.
#[test]
fn cod4_live_trailing_score_bars_flip_the_backdrop_in_place() {
    let sets = [
        ("scr_game_playerwaittime", "1"),
        ("scr_game_matchstarttime", "1"),
    ];
    let Some(r) = live_join_on("mp_crash", crash_world(), &sets, |s, spawned, t0, ctl| {
        spawned.as_ref()?;
        let install = vcod_cod4::testing::cod4_install()?;
        let mut hud = Cod4Hud::default();
        hud.load(&install.root().join("main"), "mp_crash");
        let shell = Shell::new();
        let backdrop = |hud: &Cod4Hud, s: &Cod4Session, now: i32| {
            let tex = hud.texture_of("scorebar_backdrop");
            hud.build(s, (640.0, 480.0), now, None, &shell)
                .into_iter()
                .filter(|q| Some(&q.texture) == tex.as_ref())
                .map(|q| (q.verts, q.uvs))
                .collect::<Vec<_>>()
        };
        let now = run(s, &mut hud, t0, 2500);
        let level = backdrop(&hud, s, now);
        ctl.send(Ctl::Command("b 0 30 10 750".into())).ok()?;
        let now = run(s, &mut hud, t0, 300);
        Some((level, backdrop(&hud, s, now)))
    }) else {
        return;
    };
    let (level, trailing) = r.expect("never spawned");
    // Level: the menu's 2 plus -45, 64 high, from the bottom (480).
    assert_eq!(level.len(), 1, "{level:?}");
    let (v, uv) = level[0];
    assert_eq!((v[0], v[2]), ([4.0, 437.0], [132.0, 501.0]), "{level:?}");
    assert_eq!((uv[0], uv[2]), ([0.0, 0.0], [1.0, 1.0]), "{level:?}");
    // Trailing: y -65 + 2, still 64 down, the picture upside down.
    assert_eq!(trailing.len(), 1, "{trailing:?}");
    let (v, uv) = trailing[0];
    assert_eq!((v[0], v[2]), ([4.0, 417.0], [132.0, 481.0]), "{trailing:?}");
    assert_eq!((uv[0], uv[2]), ([0.0, 1.0], [1.0, 0.0]), "{trailing:?}");
}

/// One drawn quad's box, `[x0, y0, x1, y1]` in window pixels.
fn bbox(qs: &[crate::hud::HudQuad]) -> Option<[f32; 4]> {
    let mut b: Option<[f32; 4]> = None;
    for v in qs.iter().flat_map(|q| q.verts) {
        let r = b.get_or_insert([v[0], v[1], v[0], v[1]]);
        *r = [
            r[0].min(v[0]),
            r[1].min(v[1]),
            r[2].max(v[0]),
            r[3].max(v[1]),
        ];
    }
    b
}

/// The screens the placement gate draws at: (w, h, fullscreen,
/// `vc_windowAspect`), all under `r_aspectRatio auto`.
const SCREENS: [(f32, f32, bool, bool); 5] = [
    (3840.0, 1600.0, true, false),
    (3840.0, 1600.0, true, true),
    (3840.0, 1600.0, false, false),
    (1920.0, 1080.0, true, false),
    (1920.0, 1080.0, true, true),
];

/// Every quad the HUD and the held board draw on a joined player at each
/// of [`SCREENS`], boxed per texture (the board as one), keyed by screen.
type Rects = Vec<((f32, f32, bool, bool), BTreeMap<String, [f32; 4]>)>;

/// The HUD of a player on vcod-server's war at 3840x1600 and 1920x1080,
/// 1.7's placement and `vc_windowAspect 1`'s (cod4-front-end.md 10.2):
/// under `vc_windowAspect 1` the board is centred on the window, the
/// compass and the score bars keep to its left edge and the weapon info
/// to its right, as 1.7 puts them at 16:9. `HUD_RECTS_DUMP=<path>`
/// writes every box.
#[test]
fn cod4_live_hud_places_by_the_window_under_window_aspect() {
    let sets = [
        ("scr_game_playerwaittime", "1"),
        ("scr_game_matchstarttime", "1"),
    ];
    let Some(r) = live_join_on("mp_crash", crash_world(), &sets, |s, spawned, t0, _| {
        spawned.as_ref()?;
        let install = vcod_cod4::testing::cod4_install()?;
        let mut hud = Cod4Hud::default();
        hud.load(&install.root().join("main"), "mp_crash");
        let shell = Shell::new();
        let mut now = run(s, &mut hud, t0, 2500);
        if hud.set_scores(true, now) {
            s.command("score");
        }
        now = run(s, &mut hud, t0, 1000);
        let view = Some((glam::Vec3::ZERO, 0.0));
        let mut out: Rects = Vec::new();
        for screen in SCREENS {
            let (w, h, full, wa) = screen;
            let sc = crate::camera::Cod4Screen::of("auto", w, h, full, wa);
            crate::hud::cod2elem::set_cod4_screen(&sc);
            let mut by: BTreeMap<String, Vec<crate::hud::HudQuad>> = BTreeMap::new();
            hud.set_scores(false, now);
            for q in hud.build(s, (w, h), now, view, &shell) {
                // `xpbar`'s backing: `white`, 8 high on the screen's foot
                let foot = q.verts[2][1] == h
                    && (q.verts[2][1] - q.verts[0][1] - 8.0 * h / 480.0).abs() < 0.01;
                let key = if q.texture == "white" && foot {
                    "<xp backing>".to_string()
                } else {
                    q.texture.clone()
                };
                by.entry(key).or_default().push(q);
            }
            hud.set_scores(true, now);
            by.insert("<board>".into(), hud.board_quads(s, (w, h), &shell));
            let boxes = by
                .into_iter()
                .filter_map(|(k, v)| Some((k, bbox(&v)?)))
                .collect();
            out.push((screen, boxes));
        }
        crate::hud::cod2elem::set_cod4_screen(&crate::camera::Cod4Screen::of(
            "auto", 640.0, 480.0, false, false,
        ));
        Some(out)
    }) else {
        return;
    };
    let rects = r.expect("never spawned");
    if let Ok(path) = std::env::var("HUD_RECTS_DUMP") {
        let mut s = String::new();
        for ((w, h, full, wa), boxes) in &rects {
            for (k, b) in boxes {
                s += &format!(
                    "{w}x{h} full {} wa {} {k} {:.2} {:.2} {:.2} {:.2}\n",
                    *full as u8, *wa as u8, b[0], b[1], b[2], b[3]
                );
            }
        }
        std::fs::write(path, s).unwrap();
    }
    let at = |w: f32, full: bool, wa: bool| {
        &rects
            .iter()
            .find(|((sw, _, f, a), _)| *sw == w && *f == full && *a == wa)
            .unwrap()
            .1
    };
    // `vc_windowAspect 1` draws the HUD as 1.7 draws it in a window of
    // that size: square pixels, the sides at the window's edges; only the
    // XP bar's strip widens to the window.
    let strip = ["<xp backing>", "images/hud_xpticker480ws.iwi"];
    let rest = |b: &BTreeMap<String, [f32; 4]>| {
        let mut b = b.clone();
        b.retain(|k, _| !strip.contains(&k.as_str()));
        b
    };
    assert_eq!(rest(at(3840.0, true, true)), rest(at(3840.0, false, false)));
    assert_eq!(at(1920.0, true, true), at(1920.0, true, false));
    let wide = at(3840.0, true, true);
    let close = |a: f32, b: f32| (a - b).abs() < 0.5;
    // 854 grid units, `wideScreen` 1 at 16:9: the whole foot at 1920x1080,
    // and widened to it at 3840x1600, as 1.7 stretched does
    for (w, full, wa) in [
        (1920.0, true, false),
        (3840.0, true, false),
        (3840.0, true, true),
    ] {
        let b = at(w, full, wa)["<xp backing>"];
        assert!(b[0] == 0.0 && b[2] >= w, "{w} {wa} {b:?}");
    }
    let windowed = at(3840.0, false, false)["<xp backing>"];
    assert!(close(windowed[2], 854.0 * 1600.0 / 480.0), "{windowed:?}");
    // the ticks along it widen with it
    let ticks = |b: &BTreeMap<String, [f32; 4]>| b["images/hud_xpticker480ws.iwi"][2];
    assert!(
        close(ticks(wide), ticks(at(3840.0, true, false))),
        "{wide:?}"
    );
    let board = wide["<board>"];
    assert!(close(board[0] + board[2], 3840.0), "{board:?}");
    // `minimapBG` at -8 of 640 by 1600 / 480
    let bg = wide["images/minimap_background.iwi"];
    assert!(close(bg[0], -8.0 * 1600.0 / 480.0), "{bg:?}");
    let back = wide["images/scorebar_backdrop.iwi"];
    assert!(close(back[0], 4.0 * 1600.0 / 480.0), "{back:?}");
    // the weapon info's frag icon as far in from the right as at 16:9
    let frag = |w: f32, b: &BTreeMap<String, [f32; 4]>| {
        let k = b["images/hud_us_grenade.iwi"];
        (w - k[2]) / (w / 1920.0).min(1600.0 / 1080.0)
    };
    let narrow = at(1920.0, true, true);
    assert!(close(frag(3840.0, wide), frag(1920.0, narrow)), "{wide:?}");
}
