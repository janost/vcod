//! A watched CoD4 session's 2D layer: the playerstate's hudelems, the game
//! message window (obituaries as kill lines, `iPrintLn`), the bold game
//! messages (`iPrintLnBold`), the chat ring, the obituary's centre print
//! and the last score list (docs/research/cod4-hud.md). Fonts, images,
//! strings and window placements are the zones' (`vcod_cod4::hud`), loaded
//! on first use beside the map's.
//!
//! [`Cod4Hud::update`] reads what the session brought (server commands,
//! obituary events), [`Cod4Hud::build`] draws it on the snapshot's clock.

mod menus;

#[cfg(test)]
pub(crate) use menus::Hint;

use std::collections::HashMap;
use std::path::Path;

use vcod_cod4::hud::{self as h, Command, HudAssets, HudElem, KillIcon, MessageWindow, Scores};
use vcod_cod4::net::delta::EntityState;
use vcod_cod4::net::parse::Snapshot;
use vcod_common::localize::Localized;

use crate::cod4_radar::{COMPASS_RANGE, MapAxes, SWEEP_HALF};
use crate::console::shell::Shell;
use crate::hud::HudQuad;
use crate::hud::cod2elem::{self, ScreenPlacement};
use crate::hud::font::{self, Font, Glyph};
use crate::play::cod4::{Cod4Session, EV_OBITUARY, Fired, client_name, ent_u32};

/// `cg_chatHeight` 8 and `cg_chatTime` 12000 (`iw3mp.exe` 0x43ce07,
/// 0x43cdd0), drawn at `cg_hudChatPosition` (5, 204).
const CHAT_LINES: usize = 8;
const CHAT_MS: i32 = 12_000;
const CHAT_POS: (f32, f32) = (5.0, 204.0);
/// `CG_AddToTeamChat` wraps a line past 52 characters.
const CHAT_WIDTH: usize = 52;
/// The chat lines' text scale and font height (`cg_chatHeight`'s lines,
/// cod4-hud.md 4).
const CHAT_SCALE: f32 = 0.28125;
/// `con_gameMsgWindowNMsgTime` and `LineCount` per window (`iw3mp.exe`
/// 0x6bdc7c, 0x6bdc8c), fade in 0.25 s (0.75 s for window 2), fade out
/// 0.5 s (0.01 s for window 1).
const MSG_MS: [i32; 4] = [5000, 8000, 5000, 5000];
const MSG_LINES: [usize; 4] = [4, 5, 7, 5];
const FADE_IN_MS: [i32; 4] = [250, 250, 750, 250];
const FADE_OUT_MS: [i32; 4] = [500, 10, 500, 500];
/// `cg_centertime` 5 s; the centre print sits 150 under the screen's
/// middle in the `centerobituary` menu's font scale.
const CENTER_MS: i32 = 5000;
const CENTER_Y: f32 = 150.0;
const CENTER_SCALE: f32 = 0.4583;
/// The vote configstrings (`CG_ConfigStringModified`, `iw3mp.exe`
/// 0x44afd0): 13 `<end> <sv_serverid>`, 14 the text, 15 and 16 the counts.
const CS_VOTE_TIME: u16 = 13;
const CS_VOTE_STRING: u16 = 14;
const CS_VOTE_YES: u16 = 15;
const CS_VOTE_NO: u16 = 16;
/// `CS_GAMEENDTIME`: when the match ends, in level time.
const CS_GAME_END_TIME: u16 = 11;
/// `eFlags`: this player voted.
const EF_VOTED: u32 = 0x10_0000;
/// `eFlags`: on a turret.
const EF_TURRET_ACTIVE: u32 = 0x2_0000;
/// An `ET_PLAYER`'s `eFlags` 0x400000 and the playerstate's 0x800000:
/// `pingPlayer`'s flash on the body and on `iCompassPlayerInfo`'s player
/// (`iw3mp.exe` 0x429717, 0x429856).
const EF_PING: u32 = 0x40_0000;
const EF_COMPASS_PING: u32 = 0x80_0000;
/// 1.7's `ET_HELICOPTER` and `ET_PLANE`, the compass's owner draws 155
/// and 156.
const ET_HELICOPTER: u32 = 12;
const ET_PLANE: u32 = 13;

/// A `clientState_t` field by name.
fn client_word(c: &vcod_cod4::net::delta::ClientState, name: &str) -> u32 {
    vcod_cod4::net::fields::CLIENT_STATE
        .iter()
        .find(|f| f.name == name)
        .map_or(0, |f| c.0[f.word()])
}
/// `cg_hudVotePosition`'s default (`iw3mp.exe` 0x43bb44: 5 and 220).
const VOTE_POSITION: [f32; 2] = [5.0, 220.0];
/// `CG_Obituary`'s icon: 1.4 font heights square, twice as wide for a
/// 2:1 or 4:1 icon, half as tall for 4:1.
const ICON_SIZE: f32 = 1.4;
/// `ui_smallFont`, `ui_bigFont`, `ui_extraBigFont` (`iw3mp.exe` 0x541d94,
/// 0x541def, 0x541e4a).
const UI_SMALL: f32 = 0.25;
const UI_BIG: f32 = 0.4;
const UI_EXTRA_BIG: f32 = 0.55;

/// The UI fonts `UI_GetFontHandle` picks from.
pub(crate) struct Fonts {
    small: Font,
    normal: Font,
    console: Font,
    big: Font,
    bold: Font,
    extra_big: Font,
    objective: Font,
}

impl Fonts {
    fn load(a: &HudAssets) -> Option<Fonts> {
        let f = |name: &str| {
            let (f, page) = a.font(name)?;
            Some(to_font(f, page.as_deref()?))
        };
        Some(Fonts {
            small: f("fonts/smallFont")?,
            normal: f("fonts/normalFont")?,
            console: f("fonts/consoleFont")?,
            big: f("fonts/bigFont")?,
            bold: f("fonts/boldFont")?,
            extra_big: f("fonts/extraBigFont")?,
            objective: f("fonts/objectiveFont")?,
        })
    }

    /// `UI_GetFontHandle`: enums 2, 3, 5
    /// and 6 name a font, 4 picks small, normal or bold and anything else
    /// small, normal, big or extra big by the scale on a 480-line screen.
    pub(crate) fn pick(&self, font_enum: i32, scale: f32, sp: &ScreenPlacement) -> &Font {
        let s = sp.virtual_to_real[1] * scale;
        match font_enum {
            2 => &self.big,
            3 => &self.small,
            5 => &self.console,
            6 => &self.objective,
            4 if s <= UI_SMALL => &self.small,
            4 if s < UI_BIG => &self.normal,
            4 => &self.bold,
            _ if s <= UI_SMALL => &self.small,
            _ if s >= UI_EXTRA_BIG => &self.extra_big,
            _ if s >= UI_BIG => &self.big,
            _ => &self.normal,
        }
    }
}

/// A CoD4 `Font_s` as the HUD's [`Font`]: glyph pixels scaled by
/// `48 / pixelHeight` as CoD2's (`R_NormalizedTextScale`), on its page's
/// `.iwi`.
fn to_font(f: &vcod_cod4::assets::ui::Font, page: &str) -> Font {
    let none = Glyph {
        height: 0,
        width: 0,
        top: 0.0,
        bearing: 0.0,
        advance: 0.0,
        image_width: 0,
        image_height: 0,
        s: 0.0,
        t: 0.0,
        s2: 0.0,
        t2: 0.0,
        page: 0,
    };
    let mut glyphs = vec![none; 256];
    for g in &f.glyphs {
        if let Some(slot) = glyphs.get_mut(usize::from(g.letter)) {
            *slot = Glyph {
                height: i32::from(g.pixel_height),
                width: i32::from(g.pixel_width),
                top: -f32::from(g.y0),
                bearing: f32::from(g.x0),
                advance: f32::from(g.dx),
                image_width: i32::from(g.pixel_width),
                image_height: i32::from(g.pixel_height),
                s: g.st[0],
                t: g.st[1],
                s2: g.st[2],
                t2: g.st[3],
                page: 0,
            };
        }
    }
    // A letter the font lacks draws as `?`, as unknown bytes do.
    let q = glyphs[usize::from(b'?')];
    for g in glyphs.iter_mut().skip(32) {
        if g.advance == 0.0 {
            *g = q;
        }
    }
    let max_height = (0x20u8..=0x7e)
        .map(|b| glyphs[b as usize].top as i32)
        .max()
        .unwrap_or(0)
        .max(1);
    let glyph_scale = 48.0 / f.pixel_height.max(1) as f32;
    let page = format!("images/{page}.iwi");
    Font {
        size: f.pixel_height.max(1) as u32,
        glyphs,
        glyph_scale,
        line_advance: f.pixel_height as f32 * glyph_scale,
        max_height,
        page: page.clone(),
        pages: vec![page],
    }
}

/// The console's `fonts/consoleFont` (`cls.consoleFont`) out of
/// `localized_code_post_gfx_mp` of the install whose `main/` is
/// `main_dir` (cod4-hud.md 1); `None` (warned) when that zone lacks it.
pub(crate) fn console_font(main_dir: &Path) -> Option<Font> {
    let install = main_dir
        .parent()
        .and_then(vcod_cod4::install::Install::at)?;
    let a = HudAssets::load_zones(&install, ["localized_code_post_gfx_mp"])
        .inspect_err(|e| log::warn!("cod4 console: {e:#}"))
        .ok()?;
    let (f, page) = a.font("fonts/consoleFont")?;
    Some(to_font(f, page.as_deref()?))
}

/// What the zones gave, once loaded.
struct Loaded {
    fonts: Fonts,
    images: HashMap<String, String>,
    localized: std::sync::Arc<Localized>,
    kill_icons: HashMap<String, KillIcon>,
    windows: HashMap<i32, MessageWindow>,
    compass: HashMap<String, h::MenuRect>,
    rank_icons: HashMap<(i32, i32), String>,
    rank_levels: HashMap<i32, String>,
    weapon_hud: HashMap<String, h::WeaponHud>,
    weapon_info: HashMap<String, h::MenuItem>,
    killcam: Vec<h::MenuItem>,
    script_menus: HashMap<String, vcod_cod4::scriptmenu::ScriptMenu>,
    tables: HashMap<String, h::Table>,
    gametypes: HashMap<String, String>,
    ui_lists: UiLists,
    /// The startup configs' dvars by lowercased name, the last `set` of
    /// each (`h::startup_dvars`).
    startup_dvars: HashMap<String, String>,
    /// The configs the Options pages `exec` (`play::cod4_menus::MENU_CFGS`),
    /// by name.
    menu_cfgs: HashMap<String, String>,
    /// The last [`Loaded::ammo_slots`] and the list it was for.
    slots: std::cell::RefCell<Option<AmmoSlots>>,
}

type AmmoSlots = (std::sync::Arc<[String]>, std::sync::Arc<[(usize, usize)]>);

impl Loaded {
    /// [`h::ammo_slots`] of the weapon list over the level's weapon defs,
    /// kept while the list stays the same one
    /// ([`crate::play::cod4::weapon_list`] hands out one per change).
    fn ammo_slots(&self, list: &std::sync::Arc<[String]>) -> std::sync::Arc<[(usize, usize)]> {
        let mut kept = self.slots.borrow_mut();
        match &*kept {
            Some((l, slots)) if std::sync::Arc::ptr_eq(l, list) => slots.clone(),
            _ => {
                let slots: std::sync::Arc<[(usize, usize)]> =
                    h::ammo_slots(list, &self.weapon_hud).into();
                *kept = Some((list.clone(), slots.clone()));
                slots
            }
        }
    }
}

/// The install's startup configs' dvars ([`h::startup_dvars`]) and the
/// Options pages' configs out of its iwds, none when they do not open.
fn startup_dvars(
    install: &vcod_cod4::install::Install,
) -> (HashMap<String, String>, HashMap<String, String>) {
    let dirs = install.iwd_dirs();
    match vcod_common::pk3::Pk3Fs::open_iwd_layers(&dirs[0], &dirs[1..]) {
        Ok(fs) => (
            h::startup_dvars(|n| fs.read(n))
                .into_iter()
                .map(|(k, v)| (k.to_ascii_lowercase(), v))
                .collect(),
            crate::play::cod4_menus::MENU_CFGS
                .iter()
                .filter_map(|&n| {
                    Some((
                        n.to_string(),
                        String::from_utf8_lossy(&fs.read(n)?).into_owned(),
                    ))
                })
                .collect(),
        ),
        Err(e) => {
            log::warn!("cod4 hud: no startup configs: {e:#}");
            Default::default()
        }
    }
}

/// The call vote's lists (cod4-hud.md 12.6).
type UiLists = crate::play::cod4_menus::Lists;

/// The zones' script menus by lowercased name.
pub(crate) type ScriptMenuDefs = HashMap<String, vcod_cod4::scriptmenu::ScriptMenu>;

/// What the script menus draw with (`cod4_menu`).
pub(crate) struct MenuAssets<'a> {
    pub(crate) fonts: &'a Fonts,
    l: &'a Loaded,
}

impl MenuAssets<'_> {
    /// A material's HUD texture.
    pub(crate) fn image(&self, material: &str) -> Option<String> {
        image_of(self.l, material)
    }

    /// `UI_SafeTranslateString`: a localized key's text, else the key.
    pub(crate) fn translate<'b>(&'b self, key: &'b str) -> &'b str {
        self.l.localized.get(key).unwrap_or(key)
    }
}

/// One piece of a message line.
#[derive(Clone, Debug, PartialEq)]
enum Piece {
    Text(String),
    /// A kill icon: its material and size in font heights.
    Icon {
        material: String,
        w: f32,
        h: f32,
        flip: bool,
    },
}

#[derive(Clone, Debug, PartialEq)]
struct Line {
    pieces: Vec<Piece>,
    at: i32,
}

/// What [`Cod4Hud`] took in, for the replay gate.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct HudCounts {
    pub(crate) kill_lines: usize,
    pub(crate) game_messages: usize,
    pub(crate) bold_messages: usize,
    pub(crate) chat_lines: usize,
    pub(crate) center_prints: usize,
    pub(crate) scores: usize,
    pub(crate) config_strings: usize,
    pub(crate) dvars: usize,
}

/// A level's HUD zones read off the window's thread
/// ([`Cod4Hud::read_level`]).
pub(crate) struct LevelZones {
    map: String,
    fs_game: Option<String>,
    loaded: Option<Box<Loaded>>,
}

#[derive(Default)]
pub(crate) struct Cod4Hud {
    /// Loaded on first use; `None` inside once that failed.
    loaded: Option<Option<Box<Loaded>>>,
    /// The `fs_game` `loaded` was loaded under.
    fs_game: Option<String>,
    /// The map whose zone `loaded` holds; `None` for the front end's or
    /// the connect screen's.
    zones_map: Option<String>,
    /// Configstrings `d` changed since the gamestate.
    cs: HashMap<u16, String>,
    /// The server's `v` dvars.
    dvars: HashMap<String, String>,
    chat: Vec<Line>,
    /// Game message windows 0 (game messages, kill lines) and 1 (bold).
    windows: [Vec<Line>; 2],
    center: Option<(String, i32)>,
    pub(crate) scores: Option<Scores>,
    /// `cg.teamScores` by `team_t` (`iw3mp.exe` 0x79dbe4): `b` writes all
    /// four, `G` axis's and `H` allies' (cod4-hud.md 2).
    team_scores: [i32; 4],
    /// The gamestate's weapon list, which `eventParm` indexes 1-based.
    weapons: std::sync::Arc<[String]>,
    pub(crate) counts: HudCounts,
    /// `cg.showScores`: `+scores` held.
    show_scores: bool,
    /// `cg.scoreFadeTime`: when `-scores` let the board go.
    score_fade_at: Option<i32>,
    /// `cg.scoresRequestTime`: the last `score` asked for.
    scores_asked: Option<i32>,
    /// `cg.scoresTop`, 0 until the next draw centres it; the draw sets it
    /// (centring on our row), so it lives in a cell.
    scores_top: std::cell::Cell<i32>,
    /// What the last draw left for the scroll keys: `scoresOffBottom` and
    /// the board's total lines.
    scores_off_bottom: std::cell::Cell<(bool, i32)>,
    /// The chat field `chatmodepublic` / `chatmodeteam` opened.
    pub(crate) chat_field: Option<crate::hud::ChatField>,
    /// The `s` (true) and `k` (false) commands since the last
    /// [`Self::take_local_sounds`], each as the alias its slot named when
    /// it came.
    local_sounds: Vec<(String, bool)>,
    /// The compass's enemy pings (cod4-hud.md 15).
    radar: crate::cod4_radar::Radar,
    /// The helicopters and planes the compass draws.
    vehicles: crate::cod4_radar::Vehicles,
    /// `cg.lastClipFlashTime`: the last update the held clip was not low.
    clip_flash_from: i32,
    /// The low-health overlay and the hit direction icons (cod4-hud.md 19).
    damage: crate::cod4_damage::Cod4Damage,
    /// `cg.invalidCmdHintType` and its time (cod4-hud.md 23).
    invalid_cmd: Option<(u8, i32)>,
    /// The cursor hint's fade (cod4-hud.md 20).
    hint: hints::CursorHint,
}

/// `cg_scoreboardPingGraph` and the `cg_ScoresPing_*` dvars off the
/// shell, 1.7's defaults where unset.
fn ping_graph(shell: &Shell) -> crate::cod4_scoreboard::PingGraph {
    let d = crate::cod4_scoreboard::PingGraph::default();
    let int = |n: &str, def: i32| shell.cvar(n).map_or(def, vcod_cod4::menuexp::atoi);
    let colour = |n: &str, def: [f32; 4]| {
        shell.cvar(n).map_or(def, |v| {
            let mut c = def;
            for (k, x) in v.split_whitespace().take(4).enumerate() {
                c[k] = x.parse().unwrap_or(def[k]);
            }
            c
        })
    };
    crate::cod4_scoreboard::PingGraph {
        enabled: int("cg_scoreboardPingGraph", 0) != 0,
        max_bars: int("cg_ScoresPing_MaxBars", d.max_bars).clamp(1, 10),
        interval: int("cg_ScoresPing_Interval", d.interval).clamp(1, 500),
        high: colour("cg_ScoresPing_HighColor", d.high),
        med: colour("cg_ScoresPing_MedColor", d.med),
        low: colour("cg_ScoresPing_LowColor", d.low),
        bg: colour("cg_ScoresPing_BgColor", d.bg),
    }
}

impl Cod4Hud {
    /// `+scores` / `-scores` at drawn time `now` (`CG_ScoresDown`,
    /// `CG_ScoresUp`); true when a `score` request is due.
    pub(crate) fn set_scores(&mut self, down: bool, now: i32) -> bool {
        if !down {
            if self.show_scores {
                self.show_scores = false;
                self.scores_top.set(0);
                self.score_fade_at = Some(now);
            }
            return false;
        }
        if !self.show_scores {
            self.scores_top.set(0);
        }
        self.show_scores = true;
        self.score_request(now)
    }

    /// `Scoreboard_HandleInput` (`iw3mp.exe` 0x44a2c0): while `+scores`
    /// is held a press of PGDN, KP_PGDN or MWHEELDOWN scrolls the board
    /// `cg_scoreboardScrollStep` lines down while it runs off the bottom,
    /// PGUP, KP_PGUP or MWHEELUP up to the first line; true when the key
    /// is the board's, which then reaches no bind (`CL_KeyEvent`).
    pub(crate) fn scoreboard_key(&mut self, key: &str, step: i32) -> bool {
        if !self.show_scores {
            return false;
        }
        let top = self.scores_top.get();
        match key {
            "PGDN" | "KP_PGDN" | "MWHEELDOWN" => {
                let (off_bottom, total) = self.scores_off_bottom.get();
                if off_bottom {
                    self.scores_top.set((top + step).min(total));
                }
            }
            "PGUP" | "KP_PGUP" | "MWHEELUP" => {
                if top > 1 {
                    self.scores_top.set((top - step).max(1));
                }
            }
            _ => return false,
        }
        true
    }

    /// `UpdateScores`: while the board draws, a `score` every 2 s.
    pub(crate) fn score_request(&mut self, now: i32) -> bool {
        if self.scores_asked.is_some_and(|t| t + 2000 >= now) {
            return false;
        }
        self.scores_asked = Some(now);
        true
    }

    /// The board's alpha at `now`: 1 while shown or at intermission,
    /// then a 100 ms fade (`CG_FadeColor(time, scoreFadeTime, 100, 100)`);
    /// `None` once gone.
    pub(crate) fn scoreboard_fade(&self, snap: &Snapshot, now: i32) -> Option<f32> {
        if self.show_scores || snap.ps.get("pm_type") == Some(h::PM_INTERMISSION) {
            return Some(1.0);
        }
        let t = now - self.score_fade_at?;
        (0..100).contains(&t).then(|| 1.0 - t as f32 / 100.0)
    }

    /// The zones' fonts and strings for `map`, out of the install whose
    /// `main/` is `main_dir`.
    #[cfg(test)]
    pub(crate) fn load(&mut self, main_dir: &Path, map: &str) {
        self.load_under(main_dir, map, None);
    }

    /// [`Self::load`] under `fs_game`, the mod's `mod.ff` over the stock
    /// zones; loaded again when `map` or `fs_game` changes: a level's
    /// minimap image and the strings of its gametype scripts ride its own
    /// zone. True when it loaded.
    pub(crate) fn load_under(&mut self, main_dir: &Path, map: &str, fs_game: Option<&str>) -> bool {
        self.load_zones(main_dir, Some(map), fs_game)
    }

    /// The front end's zones (`HudAssets::load_front`) under `fs_game`,
    /// for the main menu (`cod4_front`); loaded again when `fs_game`
    /// changes. True when it loaded.
    pub(crate) fn load_front(&mut self, main_dir: &Path, fs_game: Option<&str>) -> bool {
        self.load_zones(main_dir, None, fs_game)
    }

    /// `map`'s zones over the HUD's, or with none the front end's.
    fn load_zones(&mut self, main_dir: &Path, map: Option<&str>, fs_game: Option<&str>) -> bool {
        if self.loaded.is_some()
            && self.fs_game.as_deref() == fs_game
            && self.zones_map.as_deref() == map
        {
            return false;
        }
        self.zones_map = map.map(str::to_string);
        self.load_with(main_dir, fs_game, |i| match map {
            Some(map) => HudAssets::load(i, map),
            None => HudAssets::load_front(i),
        })
    }

    /// The stock HUD zones alone, no map's: what the connect screen draws
    /// with before the gamestate names one.
    pub(crate) fn load_connect(&mut self, main_dir: &Path) -> bool {
        self.zones_map = None;
        self.load_with(main_dir, None, |i| HudAssets::load_zones(i, h::HUD_ZONES))
    }

    /// A level's zones as [`Self::load_under`] reads them, on any
    /// thread, for [`Self::install_level`]: `assets` when the map's load
    /// built them off its own walks (`cod4_fly::load_from`), else read.
    pub(crate) fn read_level(
        main_dir: &Path,
        map: &str,
        fs_game: Option<&str>,
        assets: Option<HudAssets>,
    ) -> LevelZones {
        LevelZones {
            map: map.to_string(),
            fs_game: fs_game.map(str::to_string),
            loaded: Self::read(main_dir, fs_game, |i| match assets {
                Some(a) => Ok(a),
                None => HudAssets::load(i, map),
            }),
        }
    }

    /// `z` in place of the zones held, as [`Self::load_under`] of its map
    /// would have loaded them; false, keeping the held ones, when they are
    /// that map's already.
    pub(crate) fn install_level(&mut self, z: LevelZones) -> bool {
        if self.loaded.is_some()
            && self.fs_game == z.fs_game
            && self.zones_map.as_deref() == Some(&z.map)
        {
            return false;
        }
        self.zones_map = Some(z.map);
        self.fs_game = z.fs_game;
        self.loaded = Some(z.loaded);
        true
    }

    fn load_with(
        &mut self,
        main_dir: &Path,
        fs_game: Option<&str>,
        assets: impl FnOnce(&vcod_cod4::install::Install) -> anyhow::Result<HudAssets>,
    ) -> bool {
        self.fs_game = fs_game.map(str::to_string);
        self.loaded = Some(Self::read(main_dir, fs_game, assets));
        true
    }

    fn read(
        main_dir: &Path,
        fs_game: Option<&str>,
        assets: impl FnOnce(&vcod_cod4::install::Install) -> anyhow::Result<HudAssets>,
    ) -> Option<Box<Loaded>> {
        main_dir
            .parent()
            .and_then(vcod_cod4::install::Install::at)
            .map(|i| i.with_fs_game(fs_game))
            .ok_or_else(|| anyhow::anyhow!("{} is no CoD4 install", main_dir.display()))
            .and_then(|i| Ok((assets(&i)?, startup_dvars(&i))))
            .inspect_err(|e| log::warn!("cod4 hud: {e:#}"))
            .ok()
            .and_then(|(a, (startup_dvars, menu_cfgs))| {
                let fonts = Fonts::load(&a);
                if fonts.is_none() {
                    log::warn!("cod4 hud: the zones lack the UI fonts");
                }
                let mut localized = Localized::default();
                for (k, v) in &a.localize {
                    localized.insert(k, v);
                }
                Some(Box::new(Loaded {
                    fonts: fonts?,
                    images: a.images,
                    localized: std::sync::Arc::new(localized),
                    kill_icons: a.kill_icons,
                    windows: a.windows,
                    compass: a.compass,
                    rank_icons: a.rank_icons,
                    rank_levels: a.rank_levels,
                    weapon_hud: a.weapon_hud,
                    weapon_info: a.weapon_info,
                    killcam: a.killcam,
                    script_menus: a.script_menus,
                    tables: a.tables,
                    ui_lists: UiLists {
                        maps: a.arenas,
                        gametypes: a
                            .gametype_order
                            .iter()
                            .map(|g| {
                                let key = a.gametypes.get(&g.to_ascii_lowercase());
                                (g.clone(), key.cloned().unwrap_or_else(|| g.clone()))
                            })
                            .collect(),
                    },
                    gametypes: a.gametypes,
                    startup_dvars,
                    menu_cfgs,
                    slots: Default::default(),
                }))
            })
    }

    /// The zones' script menus and string tables, once loaded
    /// (`play::cod4_menus`).
    pub(crate) fn script_menus(&self) -> Option<(&ScriptMenuDefs, &HashMap<String, h::Table>)> {
        let l = self.loaded.as_ref()?.as_ref()?;
        Some((&l.script_menus, &l.tables))
    }

    /// The startup configs' dvars, once loaded: what the menus read
    /// under the server's (`play::cod4_menus`).
    pub(crate) fn startup_dvars(&self) -> Option<&HashMap<String, String>> {
        Some(&self.loaded.as_ref()?.as_ref()?.startup_dvars)
    }

    /// The Options pages' configs, once loaded.
    pub(crate) fn menu_cfgs(&self) -> Option<&HashMap<String, String>> {
        Some(&self.loaded.as_ref()?.as_ref()?.menu_cfgs)
    }

    /// The call vote's map and game type lists, once loaded.
    pub(crate) fn ui_lists(&self) -> Option<&UiLists> {
        Some(&self.loaded.as_ref()?.as_ref()?.ui_lists)
    }

    /// The fonts, images and strings the script menus draw with.
    pub(crate) fn menu_assets(&self) -> Option<MenuAssets<'_>> {
        let l = self.loaded.as_ref()?.as_ref()?;
        Some(MenuAssets { fonts: &l.fonts, l })
    }

    fn localized(&self) -> Option<&Localized> {
        Some(&self.loaded.as_ref()?.as_ref()?.localized)
    }

    /// The zones' strings, shared with the script menus' `locString()`
    /// and `keyBinding()`.
    pub(crate) fn strings(&self) -> Option<std::sync::Arc<Localized>> {
        Some(self.loaded.as_ref()?.as_ref()?.localized.clone())
    }

    /// What the menus' cgame calls read off the newest snapshot and the
    /// last `score` reply (cod4-hud.md 12.2): the board, the killcam,
    /// death, spectating, intermission, the clip, night vision, the team
    /// scores and the player's own row. The HUD's fades read 1: vcod
    /// fades none of the three (`hud_fade_*` 0, never).
    pub(crate) fn menu_game(&self, s: &Cod4Session, now: i32) -> vcod_cod4::menuexp::GameView {
        use vcod_cod4::menuexp::{GameView, ScoreRow};
        let mut g = GameView {
            fade_dpad: 1.0,
            fade_weapon: 1.0,
            fade_compass: 1.0,
            ..GameView::default()
        };
        let own = s.gamestate().map_or(0, |gs| gs.client_num);
        g.team_scores = self.team_scores;
        if let Some(sc) = &self.scores {
            g.ranked = sc.rows.iter().map(|r| r.score).collect();
            g.own = sc.rows.iter().find(|r| r.client == own).map(|r| ScoreRow {
                score: r.score,
                ping: r.ping,
                deaths: r.deaths,
                kills: r.kills,
            });
        }
        let Some(snap) = s.client().snapshots.last() else {
            return g;
        };
        let u = |name: &str| snap.ps.get(name).unwrap_or(0);
        // The time left: configstring 11, `setGameEndTime`'s
        // level time (`CG_ParseGameEndTime`), against the snapshot's.
        let end = h::atoi(&self.configstring(s, CS_GAME_END_TIME));
        g.time_left = Some((end - snap.server_time) / 1000);
        g.scoreboard = self.scoreboard_fade(snap, now).is_some();
        g.killcam = in_killcam(snap);
        g.dead = u("pm_type") >= PM_DEAD;
        g.intermission = snap.ps.get("pm_type") == Some(h::PM_INTERMISSION);
        g.following = u("otherFlags") & 2 != 0;
        g.selecting_location = u("locationSelectionInfo") != 0;
        g.night_vision = crate::cod4_look::night_vision(u("weapFlags"));
        // `CG_GetPlayerClipAmmoCount`: the held weapon's clip slot.
        if let (Some(l), Some(gs)) = (self.loaded.as_ref().and_then(|l| l.as_ref()), s.gamestate())
        {
            let list = crate::play::cod4::weapon_list(gs);
            let slots = l.ammo_slots(&list);
            let w = u("weapon") as usize;
            if let Some(&(_, clip)) = w.checked_sub(1).and_then(|i| slots.get(i)) {
                g.clip_ammo = snap.ps.ammoclip.get(clip).copied().unwrap_or(0);
            }
        }
        g.action_slots = std::array::from_fn(|k| action_slot(s, snap, k).usable);
        g.fade_dpad = if u("weapFlags") & 0x80 != 0 { 0.0 } else { 1.0 };
        g
    }

    /// The HUD menus' expression context: the session's menus' with the
    /// HUD's dvars (the configstring mirror, then `v`), our team off the
    /// snapshot and the cgame state of [`Self::menu_game`].
    fn env<'a>(&'a self, s: &'a Cod4Session, now: i32, shell: &'a Shell) -> HudEnv<'a> {
        let intermission = s
            .client()
            .snapshots
            .last()
            .is_some_and(|x| x.ps.get("pm_type") == Some(h::PM_INTERMISSION));
        HudEnv {
            hud: self,
            s,
            game: self.menu_game(s, now),
            team: s.own_team(),
            shell,
            board_menu: self.show_scores || intermission,
        }
    }

    /// Menu `name`'s `visible when`, as the HUD's context reads it; `None`
    /// before the zones load or when it does not evaluate.
    fn menu_visible(&self, l: &Loaded, name: &str, env: &HudEnv) -> Option<bool> {
        let layout = l.script_menus.get(name)?.layout.as_ref()?;
        if layout.visible.is_empty() {
            return Some(true);
        }
        vcod_cod4::menuexp::truth(&layout.visible, env).ok()
    }

    /// The `dpad` menu (cod4-hud.md 15) as owner draws: each visible
    /// action slot's (166..169, `CG_DrawPlayerActionSlot`: the
    /// slot's weapon's `dpadIcon` sized by its ratio and its rounds as
    /// `%3i`, red at none, brighter while it is the selection; the night
    /// vision icon) and the `[key]` under it.
    fn dpad(&self, l: &Loaded, s: &Cod4Session, snap: &Snapshot, env: &HudEnv) -> Vec<OwnerDraw> {
        use vcod_cod4::menuexp;
        let mut out = Vec::new();
        let Some(layout) = l
            .script_menus
            .get(DPAD_MENU)
            .and_then(|m| m.layout.as_ref())
        else {
            return out;
        };
        let fade = env.game.fade_dpad;
        let Some(gs) = s.gamestate().filter(|_| fade != 0.0) else {
            return out;
        };
        let list = crate::play::cod4::weapon_list(gs);
        let slots = l.ammo_slots(&list);
        let ps = &snap.ps;
        let item = |it: &vcod_cod4::scriptmenu::LayoutItem, rect: [f32; 4], rgba| h::MenuItem {
            rect: h::MenuRect {
                rect,
                horz_align: it.align[0],
                vert_align: it.align[1],
                background: None,
            },
            owner_draw: it.owner_draw,
            font: it.font,
            text_scale: it.text_scale,
            text_style: it.text_style,
            fore_color: rgba,
            text_align: it.text_align,
            text: None,
        };
        for it in &layout.items {
            if it.dynamic_flags & vcod_cod4::scriptmenu::WINDOW_VISIBLE == 0
                || !(it.exps.visible.is_empty()
                    || menuexp::truth(&it.exps.visible, env).unwrap_or(false))
            {
                continue;
            }
            match it.owner_draw {
                od @ 166..=169 => {
                    let slot = action_slot(s, snap, (od - 166) as usize);
                    let mut rgba = it.fore_color;
                    if slot.active {
                        for c in &mut rgba[..3] {
                            *c *= 1.2;
                        }
                        rgba[3] = 1.0;
                    }
                    rgba[3] *= fade;
                    let r = it.rect;
                    if slot.night_vision {
                        out.push(OwnerDraw::Pic {
                            material: NVG_ICON.into(),
                            item: item(it, r, rgba),
                        });
                        continue;
                    }
                    let w = usize::from(slot.weapon);
                    let Some(d) = w
                        .checked_sub(1)
                        .and_then(|i| l.weapon_hud.get(list.get(i)?))
                    else {
                        continue;
                    };
                    // The icon's ratio (cod4-hud.md 15.2): 4:1 twice as
                    // wide at half height, 2:1 twice as wide.
                    let [x, y, mut iw, mut ih] = r;
                    let mut iy = y;
                    match d.dpad_icon_ratio {
                        2 => {
                            iw *= 2.0;
                            iy += ih * 0.25;
                            ih *= 0.5;
                        }
                        1 => iw *= 2.0,
                        _ => {}
                    }
                    if let Some(m) = &d.dpad_icon {
                        out.push(OwnerDraw::Pic {
                            material: m.clone(),
                            item: item(it, [x, iy, iw, ih], rgba),
                        });
                    }
                    // `BG_WeaponAmmo`: the clip and the reserve.
                    let (ammo, clip) = slots[w - 1];
                    let rounds = ps.ammoclip.get(clip).copied().unwrap_or(0)
                        + ps.ammo.get(ammo).copied().unwrap_or(0);
                    if rounds == 0 {
                        rgba[..3].copy_from_slice(&LOW_AMMO_RGB);
                    }
                    // The count's place (cod4-hud.md 15.2).
                    let mut tx = x + r[2] - 13.0;
                    if matches!(d.dpad_icon_ratio, 1 | 2) {
                        tx += r[2];
                    }
                    out.push(OwnerDraw::Text {
                        text: format!("{rounds:3}"),
                        x: tx,
                        item: item(it, [tx, y + r[3] + 3.0, 0.0, 0.0], rgba),
                        rgba,
                    });
                }
                0 if it.kind == 0 => {
                    let t = menuexp::string(&it.exps.text, env).unwrap_or_default();
                    if t.is_empty() {
                        continue;
                    }
                    let mut rgba = it.fore_color;
                    if !it.exps.forecolor_a.is_empty() {
                        rgba[3] = menuexp::float(&it.exps.forecolor_a, env).unwrap_or(0.0);
                    }
                    // Measured on the 640x480 grid.
                    let grid = ScreenPlacement::new(640.0, 480.0);
                    let f = l.fonts.pick(it.font, it.text_scale, &grid);
                    let tw = font::measure(f, &t, it.text_scale / f.unit_scale());
                    let th = (48.0 * it.text_scale) as i32 as f32;
                    let [x, y] = crate::cod4_menu::text_origin(it, it.rect, tw, th);
                    out.push(OwnerDraw::Text {
                        text: t,
                        x,
                        item: item(it, [x, y, 0.0, 0.0], rgba),
                        rgba,
                    });
                }
                _ => {}
            }
        }
        out
    }

    /// The `talkers` menu's four owner draws (`CG_DrawTalkerNum`,
    /// `iw3mp.exe` 0x443de4): talker `n` of [`Cod4Session::talkers`] the
    /// roster names, `voice_on` in the item's rect and the name 2 past it,
    /// centred on the rect's height. `cg_drawTalk` (`NONE`, `ALL`,
    /// `FRIENDLY`, `ENEMY`; `ALL` by default) picks whom. An enemy is a
    /// talker off our team while we are on one (0x443e43). An enemy
    /// heard through Eavesdrop (our `ps.perks` bit 9, not at the
    /// intermission) draws `perk_parabolicIcon` and the name in the enemy
    /// team's colour (0x443eaf).
    fn talkers(
        &self,
        l: &Loaded,
        s: &Cod4Session,
        snap: &Snapshot,
        shell: &Shell,
    ) -> Vec<OwnerDraw> {
        let mut out = Vec::new();
        let Some(layout) = l
            .script_menus
            .get(TALKERS_MENU)
            .and_then(|m| m.layout.as_ref())
        else {
            return out;
        };
        let mode = match shell
            .cvar("cg_drawTalk")
            .map(str::to_ascii_uppercase)
            .as_deref()
        {
            None | Some("ALL") | Some("1") => 1,
            Some("NONE") | Some("0") => 0,
            Some("FRIENDLY") | Some("2") => 2,
            Some("ENEMY") | Some("3") => 3,
            Some(_) => 1,
        };
        if mode == 0 {
            return out;
        }
        let talkers = s.talkers();
        let own_team = s.own_team();
        let perks = snap.ps.get("perks").unwrap_or(0);
        let intermission = snap.ps.get("pm_type") == Some(h::PM_INTERMISSION);
        for it in &layout.items {
            let Some(n) = it
                .owner_draw
                .checked_sub(OD_TALKER)
                .filter(|n| (0..4).contains(n))
            else {
                continue;
            };
            let Some(&client) = talkers.get(n as usize) else {
                continue;
            };
            let Some(c) = snap.clients.iter().find(|c| c.0[0] == client) else {
                continue;
            };
            let name = crate::play::cod4::client_name(c);
            let team = crate::play::cod4::client_team(c);
            let enemy = own_team != 0 && own_team != team;
            if (mode == 2 && enemy) || (mode == 3 && !enemy) {
                continue;
            }
            let mut text_rgba = it.fore_color;
            let material = if !intermission && enemy && perks >> 9 & 1 != 0 {
                let v = self
                    .server_dvar(s, "g_TeamColor_EnemyTeam")
                    .unwrap_or_default();
                let mut c = v
                    .split_whitespace()
                    .map(|x| x.parse::<f32>().unwrap_or(1.0));
                for t in &mut text_rgba[..3] {
                    *t = c.next().unwrap_or(1.0);
                }
                self.server_dvar(s, "perk_parabolicIcon")
                    .unwrap_or_else(|| PARABOLIC_ICON.into())
            } else {
                VOICE_ON.into()
            };
            let item = |rect: [f32; 4], rgba| h::MenuItem {
                rect: h::MenuRect {
                    rect,
                    horz_align: it.align[0],
                    vert_align: it.align[1],
                    background: None,
                },
                owner_draw: it.owner_draw,
                font: it.font,
                text_scale: it.text_scale,
                text_style: it.text_style,
                fore_color: rgba,
                text_align: it.text_align,
                text: None,
            };
            let [x, y, w, hh] = it.rect;
            out.push(OwnerDraw::Pic {
                material,
                item: item(it.rect, it.fore_color),
            });
            let th = (48.0 * it.text_scale) as i32 as f32;
            let (tx, ty) = (x + w + 2.0, th + y + (hh - th) / 2.0);
            out.push(OwnerDraw::Text {
                text: name,
                x: tx,
                item: item([tx, ty, 0.0, 0.0], text_rgba),
                rgba: text_rgba,
            });
        }
        out
    }

    /// A server message's text (`SEH_LocalizeTextMessage`); its key as
    /// itself before the strings are loaded.
    fn message(&self, text: &str) -> String {
        match self.localized() {
            Some(l) => l.message(text),
            None => text.replace(['\u{14}', '\u{15}'], ""),
        }
    }

    /// `GetKeyBindingLocalizedString` (`iw3mp.exe` 0x552460): the one or
    /// two keys bound to `cmd` by their `KEY_*` names, two joined by
    /// `KEY_OR`, else `KEY_UNBOUND` (0x4380a8).
    fn key_binding(&self, shell: &Shell, cmd: &str) -> String {
        let empty = Localized::default();
        let loc = self.localized().unwrap_or(&empty);
        shell
            .key_text(cmd, loc)
            .unwrap_or_else(|| loc.get("KEY_UNBOUND").unwrap_or("KEY_UNBOUND").to_string())
    }

    /// The `playLocalSound` and `stopLocalSound` aliases the server sent
    /// since the last call, in order; true starts one.
    pub(crate) fn take_local_sounds(&mut self) -> Vec<(String, bool)> {
        std::mem::take(&mut self.local_sounds)
    }

    /// Configstring `i`: a `d` change, else the gamestate's.
    pub(crate) fn configstring(&self, s: &Cod4Session, i: u16) -> String {
        String::from_utf8_lossy(self.configstring_bytes(s, i)).into_owned()
    }

    /// [`Self::configstring`] as stored, without a copy.
    fn configstring_bytes<'a>(&'a self, s: &'a Cod4Session, i: u16) -> &'a [u8] {
        match self.cs.get(&i) {
            Some(v) => v.as_bytes(),
            None => s.gamestate().map_or(&[], |g| g.configstring(i)),
        }
    }

    /// A server dvar: the configstrings' mirror (names at
    /// `CS_CODINFO`..=147, each value 128 slots on), else a `v` command's.
    /// The HUD's menus ask some fifty a frame, so the names are compared
    /// where they lie (cod4-client-perf.md 33).
    fn server_dvar(&self, s: &Cod4Session, name: &str) -> Option<String> {
        (h::CS_CODINFO..h::CS_CODINFO_VALUE)
            .find(|&i| {
                self.configstring_bytes(s, i)
                    .eq_ignore_ascii_case(name.as_bytes())
            })
            .map(|i| self.configstring(s, i + h::CS_CODINFO_VALUE - h::CS_CODINFO))
            .or_else(|| self.dvars.get(name).cloned())
    }

    /// What the menus' `gameType()`, `gameTypeName()` and
    /// `gameTypeDescription()` read: serverinfo's `g_gametype`, its
    /// display name translated (the name itself for a game type the zones
    /// do not list, `UI_GetGameTypeDisplayName`) and `cg_objectiveText`
    /// localized (`CG_GetGametypeDescription`, cod4-hud.md 12.2).
    pub(crate) fn gametype_strings(&self, s: &Cod4Session) -> (String, String, String) {
        let info = self.configstring(s, 0);
        let parts: Vec<&str> = info.split('\\').collect();
        let gametype = parts
            .iter()
            .skip(1)
            .step_by(2)
            .zip(parts.iter().skip(2).step_by(2))
            .find(|(k, _)| k.eq_ignore_ascii_case("g_gametype"))
            .map_or_else(String::new, |(_, v)| v.to_string());
        let name = self
            .loaded
            .as_ref()
            .and_then(|l| l.as_ref())
            .and_then(|l| {
                let key = l.gametypes.get(&gametype.to_ascii_lowercase())?;
                Some(l.localized.get(key).unwrap_or(key).to_string())
            })
            .unwrap_or_else(|| gametype.clone());
        let objective = self
            .server_dvar(s, "cg_objectiveText")
            .map(|t| self.message(&t))
            .unwrap_or_default();
        (gametype, name, objective)
    }

    /// `ui_hud_hardcore`, which the prematch sets (cod4-hud.md 13): the
    /// `compass_old` and `weaponinfo` menus' `visible when` read it 0.
    fn hardcore(&self, s: &Cod4Session) -> bool {
        self.server_dvar(s, "ui_hud_hardcore")
            .is_some_and(|v| vcod_cod4::menuexp::atoi(&v) != 0)
    }

    /// The configstrings' dvar mirror by lowercased name, for the script
    /// menus.
    pub(crate) fn mirrored_dvars(&self, s: &Cod4Session) -> HashMap<String, String> {
        (h::CS_CODINFO..h::CS_CODINFO_VALUE)
            .filter_map(|i| {
                let name = self.configstring(s, i);
                (!name.is_empty()).then(|| {
                    let v = self.configstring(s, i + h::CS_CODINFO_VALUE - h::CS_CODINFO);
                    (name.to_ascii_lowercase(), v)
                })
            })
            .collect()
    }

    /// Takes in the session's new server commands and this step's fired
    /// events at server time `now`.
    pub(crate) fn update(&mut self, s: &mut Cod4Session, fired: &[Fired], now: i32) {
        if let Some(gs) = s.gamestate() {
            self.weapons = crate::play::cod4::weapon_list(gs);
        }
        for text in s.take_commands() {
            let Some(cmd) = Command::parse(&text) else {
                continue;
            };
            match cmd {
                Command::Chat(t) => {
                    let t = self.message(&t).replace('\u{15}', "");
                    // Mute Players' pick (`play::cod4_menus`).
                    if s.menus.chat_muted(&t) {
                        continue;
                    }
                    for l in wrap_chat(&t) {
                        self.counts.chat_lines += 1;
                        push_line(&mut self.chat, vec![Piece::Text(l)], now, CHAT_LINES);
                    }
                }
                Command::GameMessage(t) => {
                    self.counts.game_messages += 1;
                    let t = self.message(&t);
                    push_line(
                        &mut self.windows[0],
                        vec![Piece::Text(t)],
                        now,
                        MSG_LINES[0],
                    );
                }
                Command::BoldMessage(t) => {
                    self.counts.bold_messages += 1;
                    let t = self.message(&t);
                    push_line(
                        &mut self.windows[1],
                        vec![Piece::Text(t)],
                        now,
                        MSG_LINES[1],
                    );
                }
                Command::Scores(sc) => {
                    self.counts.scores += 1;
                    // `CG_ParseScores`: a first list centres the board again.
                    if self.scores.as_ref().is_none_or(|s| s.rows.is_empty()) {
                        self.scores_top.set(0);
                    }
                    // `b`'s team 1 is axis (opfor), team 2 allies (marines).
                    self.team_scores = [0, sc.team_scores[0], sc.team_scores[1], 0];
                    self.scores = Some(sc);
                }
                Command::ConfigString(i, v) => {
                    self.counts.config_strings += 1;
                    self.cs.insert(i, v);
                }
                Command::Dvars(d) => {
                    self.counts.dvars += d.len();
                    self.dvars.extend(d);
                }
                // `G` and `H` (`iw3mp.exe` 0x44c38a, 0x44c3ac): what the
                // bars read between `score` replies.
                Command::TeamScore { team, score } => self.team_scores[team & 3] = score,
                Command::LocalSound(i) | Command::StopLocalSound(i) => {
                    let start = matches!(cmd, Command::LocalSound(_));
                    let alias = self.configstring(s, crate::cod4_audio::CS_SOUND_ALIASES + i);
                    if !alias.is_empty() {
                        self.local_sounds.push((alias, start));
                    }
                }
            }
        }
        for f in fired.iter().filter(|f| f.id == EV_OBITUARY) {
            let c = s.client();
            let Some(snap) = c
                .snapshots
                .iter()
                .rev()
                .find(|x| x.server_time == f.server_time)
            else {
                continue;
            };
            let Some(e) = f
                .entity
                .and_then(|n| snap.entities.iter().find(|e| e.number() == n))
            else {
                continue;
            };
            let own = c.gamestate.as_ref().map_or(0, |g| g.client_num as u32);
            self.obituary(snap, e, own, now);
        }
        self.step_radar(s, fired, now);
        self.update_hints(fired, now);
        self.damage.update(&s.client().snapshots, now, s.view);
        self.step_cursor_hint(s, now);
        if !self.low_clip(s) {
            self.clip_flash_from = now;
        }
    }

    /// One frame of the compass's radar at `now`: the newest snapshot's
    /// client states, players, helicopters and planes, `iCompassPlayerInfo`
    /// and the frame's shots (cod4-hud.md 15).
    fn step_radar(&mut self, s: &Cod4Session, fired: &[Fired], now: i32) {
        use crate::cod4_radar::{Body, Frame, Info, Vehicle};
        let Some(snap) = s.client().snapshots.last() else {
            return;
        };
        // Every helicopter and plane in the snapshot (cod4-hud.md 15.7).
        for e in snap
            .entities
            .iter()
            .filter(|e| matches!(e.etype(), ET_HELICOPTER | ET_PLANE))
        {
            let o = crate::play::cod4::position_at(e, now);
            let tao = ent_u32(e, "lerp.u.vehicle.teamAndOwnerIndex");
            self.vehicles.seen(
                now,
                Vehicle {
                    entity: e.number(),
                    etype: e.etype(),
                    last_update: now,
                    pos: [o.x, o.y],
                    yaw: crate::play::cod4::yaw_at(e, now),
                    team: tao & 3,
                    owner: tao >> 2,
                },
            );
        }
        let shots = self.shots(snap, fired);
        let mut infos = [Info::default(); 64];
        for c in &snap.clients {
            if let Some(i) = infos.get_mut(c.0[0] as usize) {
                *i = Info {
                    valid: true,
                    team: client_word(c, "team"),
                    perks: client_word(c, "perks"),
                };
            }
        }
        let bodies: Vec<Body> = snap
            .entities
            .iter()
            .filter(|e| e.etype() == crate::play::cod4::ET_PLAYER)
            .map(|e| {
                let o = crate::play::cod4::origin_of(e);
                let client = ent_u32(e, "clientNum");
                let eflags = ent_u32(e, "lerp.eFlags");
                Body {
                    client: client as usize,
                    pos: [o.x, o.y],
                    turret: eflags & EF_TURRET_ACTIVE != 0,
                    yaw: crate::play::cod4::ent_f32(e, "lerp.apos.trBase[1]"),
                    pinged: eflags & EF_PING != 0,
                    talking: s.is_talking(client),
                }
            })
            .collect();
        let ps = &snap.ps;
        let f = |n: &str| ps.get_f32(n).unwrap_or(0.0);
        let map = self.minimap(s).map(|(_, m)| m);
        self.radar.step(&Frame {
            time: now,
            radar: ps.get("radarEnabled").unwrap_or(0) != 0,
            map,
            me: ps.get("clientNum").unwrap_or(0) as usize,
            infos: &infos,
            bodies: &bodies,
            compass_info: ps.get("iCompassPlayerInfo").unwrap_or(0),
            compass_pinged: ps.get("eFlags").unwrap_or(0) & EF_COMPASS_PING != 0,
            view: [f("origin[0]"), f("origin[1]")],
            shots: &shots,
        });
    }

    /// The fire sound's length the audio gave a shot from `client` this
    /// frame ([`crate::cod4_radar::Radar::hold_shot`]).
    pub(crate) fn hold_shot(&mut self, client: usize, ms: i32) {
        self.radar.hold_shot(client, ms);
    }

    /// The shots the compass hears (cod4-hud.md 15.7): another
    /// player's unsilenced, non-grenade fire event where he stands, and a
    /// turret's at the turret, its gunner the event's parm. A player's
    /// shot holds 0 ms here; the audio then hands the fire sound's length
    /// to [`Self::hold_shot`].
    fn shots(&self, snap: &Snapshot, fired: &[Fired]) -> Vec<crate::cod4_radar::Shot> {
        use crate::play::cod4::ET_PLAYER;
        let mut out = Vec::new();
        let Some(Some(l)) = &self.loaded else {
            return out;
        };
        let me = snap.ps.get("clientNum").unwrap_or(0);
        let player = |n: u32| {
            snap.entities
                .iter()
                .find(|e| e.number() == n && e.etype() == ET_PLAYER)
        };
        for f in fired {
            let Some(n) = f.entity else { continue };
            match f.name {
                "EV_FIRE_WEAPON" | "EV_FIRE_WEAPON_LASTSHOT" => {
                    let Some(e) = f.state.as_ref().filter(|e| e.etype() == ET_PLAYER) else {
                        continue;
                    };
                    let client = ent_u32(e, "clientNum");
                    let def = (f.weapon as usize)
                        .checked_sub(1)
                        .and_then(|w| self.weapons.get(w))
                        .and_then(|w| l.weapon_hud.get(w));
                    if client == me || def.is_none_or(|d| d.silenced || d.weap_type == 1) {
                        continue;
                    }
                    out.push(crate::cod4_radar::Shot {
                        client: client as usize,
                        pos: [f.origin.x, f.origin.y],
                        hold_ms: 0,
                    });
                }
                "EV_FIRE_WEAPON_MG42" => {
                    let Some(gunner) = player(f.parm) else {
                        continue;
                    };
                    if snap.ps.get("viewlocked_entNum") == Some(n) {
                        continue;
                    }
                    out.push(crate::cod4_radar::Shot {
                        client: ent_u32(gunner, "clientNum") as usize,
                        pos: [f.origin.x, f.origin.y],
                        hold_ms: 50,
                    });
                }
                _ => {}
            }
        }
        out
    }

    /// The crosshair and a sight's overlay (`cod4_crosshair`) off the
    /// drawn playerstate `ps` at a fov whose half tangents are `tan`
    /// (x, y); a gunner's off the turret he is locked to; nothing until
    /// the zones load, for a dead body or a spectator, or under a script
    /// menu (cod4-hud.md 17).
    pub(crate) fn crosshair(
        &self,
        s: &Cod4Session,
        ps: &crate::cod4_viewmodel::ViewPs,
        tan: [f32; 2],
        size: (f32, f32),
        shell: &Shell,
    ) -> Vec<HudQuad> {
        let Some(Some(l)) = &self.loaded else {
            return Vec::new();
        };
        if ps.pm_type >= PM_SPECTATOR || s.menus.open().is_some() {
            return Vec::new();
        }
        let Some(gs) = s.gamestate() else {
            return Vec::new();
        };
        let list = crate::play::cod4::weapon_list(gs);
        let def_of = |w: u32| {
            (w as usize)
                .checked_sub(1)
                .and_then(|i| list.get(i))
                .and_then(|n| l.weapon_hud.get(n))
        };
        // `CG_PlayerTurretWeaponIdx`: the locked turret entity's weapon.
        let turret = s
            .client()
            .snapshots
            .last()
            .filter(|_| ps.viewlocked != 0 && ps.viewlocked_ent != ENTITYNUM_NONE)
            .and_then(|snap| {
                let e = snap
                    .entities
                    .iter()
                    .find(|e| e.number() == ps.viewlocked_ent)?;
                def_of(crate::play::cod4::ent_u32(e, "weapon"))
            });
        static NO_WEAPON: std::sync::OnceLock<vcod_cod4::hud::WeaponHud> =
            std::sync::OnceLock::new();
        let Some(def) = def_of(ps.view_weapon())
            .or_else(|| turret.map(|_| NO_WEAPON.get_or_init(Default::default)))
        else {
            return Vec::new();
        };
        let rgb = |n: &str| {
            let v = self.server_dvar(s, n).unwrap_or_default();
            let mut c = v
                .split_whitespace()
                .map(|x| x.parse::<f32>().unwrap_or(1.0));
            [0; 3].map(|_| c.next().unwrap_or(1.0))
        };
        let view = crate::cod4_crosshair::View {
            ps,
            def,
            tan_half_fov_y: tan[1],
            tan_half_fov_x: tan[0],
            turret,
            to_ads: ps.pm_flags & crate::cod4_viewmodel::PMF_SIGHT != 0,
            team_rgb: [rgb("g_TeamColor_MyTeam"), rgb("g_TeamColor_EnemyTeam")],
        };
        let sp = ScreenPlacement::cod4(size.0, size.1);
        crate::cod4_crosshair::quads(
            &view,
            &crate::cod4_crosshair::Dvars::read(shell),
            &sp,
            &|m| image_of(l, m),
        )
    }

    /// The `low health` menu's owner draw 112: its background material
    /// over its rect in its colour, at the pulse's alpha
    /// (`CG_DrawPlayerLowHealthOverlay`, 0x442e70).
    fn draw_low_health(&self, l: &Loaded, sp: &ScreenPlacement, now: i32, out: &mut Vec<HudQuad>) {
        let alpha = self.damage.overlay_alpha(now);
        let Some(layout) = l
            .script_menus
            .get(LOW_HEALTH_MENU)
            .and_then(|m| m.layout.as_ref())
            .filter(|_| alpha != 0.0)
        else {
            return;
        };
        for it in layout
            .items
            .iter()
            .filter(|it| it.owner_draw == OD_LOW_HEALTH)
        {
            let Some(tex) = it.background.as_deref().and_then(|m| image_of(l, m)) else {
                continue;
            };
            let [x, y, w, h] = it.rect;
            let (rx, rw) = sp.axis(0, x, w, it.align[0]);
            let (ry, rh) = sp.axis(1, y, h, it.align[1]);
            let mut rgba = it.fore_color;
            rgba[3] = alpha;
            out.push(quad(rx, ry, rw, rh, rgba, tex, false));
        }
    }

    /// The low-health overlay's alpha at `now`, what the live gate reads.
    #[cfg(test)]
    pub(crate) fn low_health_alpha(&self, now: i32) -> f32 {
        self.damage.overlay_alpha(now)
    }

    /// The last hit's damage kick (cod4-client-view.md 14).
    pub(crate) fn damage_kick(&self) -> crate::play::cod4_kick::DamageKick {
        self.damage.kick()
    }

    /// The hit direction slots, what the live gate reads.
    #[cfg(test)]
    pub(crate) fn damage_slots(&self) -> [crate::cod4_damage::Icon; 8] {
        *self.damage.icons()
    }

    /// The hit direction icons (`cod4_damage`) around the screen's centre
    /// for a view at `yaw`, at drawn time `now`; none under a sight's
    /// overlay or a shellshock's screen blend, or before the zones load.
    pub(crate) fn damage_icons(
        &self,
        s: &Cod4Session,
        ps: &crate::cod4_viewmodel::ViewPs,
        size: (f32, f32),
        now: i32,
        yaw: f32,
    ) -> Vec<HudQuad> {
        let Some(Some(l)) = &self.loaded else {
            return Vec::new();
        };
        let Some(tex) = image_of(l, crate::cod4_damage::HIT_DIRECTION) else {
            return Vec::new();
        };
        let scoped = s
            .gamestate()
            .map(crate::play::cod4::weapon_list)
            .and_then(|list| {
                let i = (ps.view_weapon() as usize).checked_sub(1)?;
                l.weapon_hud.get(list.get(i)?)
            })
            .is_some_and(|def| crate::cod4_crosshair::overlay_up(ps, def));
        let (parms, start, duration) = crate::play::cod4_shock::of_session(s);
        let flashed = crate::play::cod4_shock::flashbanged(parms.as_ref(), start, duration, now);
        let sp = ScreenPlacement::cod4(size.0, size.1);
        self.damage
            .icon_quads(&sp, now, yaw, scoped || flashed, &tex)
    }

    /// The waypoint hudelems in the world (`CG_AddDrawSurfsFor3dHudElems`,
    /// each one `iw3mp.exe` 0x43a190..0x43a2b4): an element of kind 13
    /// with an alpha, no offscreen material and a material is a sprite at
    /// `(x, y, z)` in its lerped colour, its height (scaled from
    /// `fromHeight` over `scaleTime`) as its world radius, or with a
    /// `value` above 0 that height times 0.0043 of the screen
    /// (cod4-hud.md 16). Bomb Squad's `waypoint_bombsquad` icons are these.
    pub(crate) fn world_quads(
        &self,
        s: &Cod4Session,
        view: &crate::cod2_world_hud::WorldView,
        now: i32,
    ) -> Vec<HudQuad> {
        use crate::cod2_world_hud::{CONSTANT_SCALE, Size};
        let mut out = Vec::new();
        let Some(Some(l)) = &self.loaded else {
            return out;
        };
        let Some(snap) = s.client().snapshots.last() else {
            return out;
        };
        let mut elems: Vec<HudElem> = h::hud_elems(&snap.ps)
            .into_iter()
            .filter(|e| e.kind == h::kind::WAYPOINT && e.offscreen_material == 0)
            .collect();
        elems.sort_by(|a, b| a.sort.total_cmp(&b.sort));
        for e in &elems {
            let rgba = colour(e.color, e.from_color, e.fade_start_time, e.fade_time, now);
            if rgba[3] <= 0.0 || e.material == 0 {
                continue;
            }
            let name = self.configstring(s, h::CS_MATERIALS + e.material as u16);
            let Some(tex) = image_of(l, &name) else {
                continue;
            };
            let height = material_size(e, now).1;
            if height == 0.0 {
                continue;
            }
            let size = if e.value > 0.0 {
                Size::Constant(height * CONSTANT_SCALE)
            } else {
                Size::World {
                    radius: height,
                    min: 0.0,
                }
            };
            out.extend(view.sprite(glam::Vec3::new(e.x, e.y, e.z), size, rgba, &tex));
        }
        out
    }

    /// The waypoints with an offscreen material, in the 2D pass
    /// (`iw3mp.exe` 0x439a40, `cod4_waypoints`): each at its
    /// target entity when it names one in the snapshot (a player raised
    /// by his stance), else at its own point; none for our own entity
    /// (cod4-hud.md 16.3).
    pub(crate) fn offscreen_waypoints(
        &self,
        s: &Cod4Session,
        eye: &crate::cod4_waypoints::Eye,
        now: i32,
    ) -> Vec<HudQuad> {
        use crate::cod4_waypoints::{POINTER, dvar::PLAYER_OFFSET};
        let mut out = Vec::new();
        let (Some(Some(l)), Some(snap)) = (&self.loaded, s.client().snapshots.last()) else {
            return out;
        };
        let own = snap.ps.get("clientNum").unwrap_or(u32::MAX);
        let pointer = image_of(l, POINTER);
        let mut elems: Vec<HudElem> = h::hud_elems(&snap.ps)
            .into_iter()
            .filter(|e| e.kind == h::kind::WAYPOINT && e.offscreen_material != 0)
            .collect();
        elems.sort_by(|a, b| a.sort.total_cmp(&b.sort));
        for e in &elems {
            let rgba = colour(e.color, e.from_color, e.fade_start_time, e.fade_time, now);
            if rgba[3] <= 0.0 {
                continue;
            }
            let name = self.configstring(s, h::CS_MATERIALS + e.offscreen_material as u16);
            let Some(tex) = image_of(l, &name) else {
                continue;
            };
            let at = if e.target_ent as u32 == ENTITYNUM_NONE {
                glam::Vec3::new(e.x, e.y, e.z)
            } else if e.target_ent as u32 == own {
                continue;
            } else {
                let Some(t) = snap
                    .entities
                    .iter()
                    .find(|x| x.number() == e.target_ent as u32)
                else {
                    continue;
                };
                let mut at = crate::play::cod4::origin_of(t);
                if t.etype() == crate::play::cod4::ET_PLAYER {
                    let f = crate::play::cod4::ent_u32(t, "lerp.eFlags");
                    at.z += match f {
                        f if f & 8 != 0 => PLAYER_OFFSET[0],
                        f if f & 4 != 0 => PLAYER_OFFSET[1],
                        _ => PLAYER_OFFSET[2],
                    };
                }
                at
            };
            crate::cod4_waypoints::quads(eye, at, rgba, &tex, pointer.as_deref(), &mut out);
        }
        out
    }

    /// A material's HUD texture once the zones load, to find its quads.
    #[cfg(test)]
    pub(crate) fn texture_of(&self, material: &str) -> Option<String> {
        image_of(self.loaded.as_ref()?.as_ref()?, material)
    }

    /// A weapon's HUD fields.
    #[cfg(test)]
    pub(crate) fn weapon_hud(&self, weapon: &str) -> Option<&h::WeaponHud> {
        self.loaded.as_ref()?.as_ref()?.weapon_hud.get(weapon)
    }

    /// A weapon's `dpadIcon` material.
    #[cfg(test)]
    pub(crate) fn dpad_icon(&self, weapon: &str) -> Option<String> {
        self.loaded
            .as_ref()?
            .as_ref()?
            .weapon_hud
            .get(weapon)?
            .dpad_icon
            .clone()
    }

    /// The held clip is low (`CG_CheckPlayerForLowClipSpecific`).
    fn low_clip(&self, s: &Cod4Session) -> bool {
        let (Some(Some(l)), Some(snap)) = (&self.loaded, s.client().snapshots.last()) else {
            return false;
        };
        held_ammo(l, s, snap).is_some_and(|w| w.low_clip())
    }

    /// `CG_Obituary`: a kill line in
    /// game message window 0, `^8` for the drawn player's team and `^9`
    /// for the other, and a centre print for the drawn player's kill or
    /// death; nothing in the killcam (`ps.deltaTime` set).
    fn obituary(&mut self, snap: &Snapshot, e: &EntityState, own: u32, now: i32) {
        let victim = ent_u32(e, "otherEntityNum");
        let attacker = ent_u32(e, "attackerEntityNum");
        let parm = ent_u32(e, "eventParm");
        let in_killcam = in_killcam(snap);
        let client = |n: u32| snap.clients.iter().find(|c| c.0[0] == n);
        let Some(v) = client(victim) else { return };
        let team = |c: &vcod_cod4::net::delta::ClientState| {
            vcod_cod4::net::fields::CLIENT_STATE
                .iter()
                .find(|f| f.name == "team")
                .map_or(0, |f| c.0[f.word()])
        };
        let me = client(own).map_or(0, team);
        let colour = |t: u32| match (t, me) {
            (1 | 2, 1 | 2) if t == me => '8',
            (1 | 2, 1 | 2) => '9',
            _ => '7',
        };
        let victim_name = client_name(v);
        let attacker_c = (attacker < 64).then(|| client(attacker)).flatten();
        if attacker < 64 && attacker_c.is_none() {
            return;
        }
        let attacker_name = match attacker_c {
            Some(a) if attacker != victim => Some((client_name(a), colour(team(a)))),
            _ => None,
        };
        let icon = self.kill_icon(parm);
        if !in_killcam {
            let teammate = attacker_c.is_some_and(|a| team(a) != 0 && team(a) == team(v));
            let tail = if teammate { "\u{14}CGAME_TEAMMATE" } else { "" };
            let print = if attacker == own && attacker != victim {
                Some(format!("CGAME_YOUKILLED\u{15}{victim_name}{tail}"))
            } else if victim == own && attacker_c.is_some() && attacker != victim {
                let name = attacker_name.as_ref().map_or("", |(n, _)| n.as_str());
                Some(format!("CGAME_YOUWEREKILLED\u{15}{name}{tail}"))
            } else {
                None
            };
            if let Some(p) = print {
                self.counts.center_prints += 1;
                self.center = Some((self.message(&p), now));
            }
            let mut pieces = Vec::new();
            if let Some((n, c)) = attacker_name {
                pieces.push(Piece::Text(format!("^{c}{n}^7 ")));
            }
            pieces.push(icon);
            pieces.push(Piece::Text(format!(" ^{}{victim_name}", colour(team(v)))));
            self.counts.kill_lines += 1;
            push_line(&mut self.windows[0], pieces, now, MSG_LINES[0]);
        }
    }

    /// The obituary's icon: a means of death (`eventParm & 0x80`) picks one
    /// of the `killicon*` materials, else the weapon's `killIcon`.
    fn kill_icon(&self, parm: u32) -> Piece {
        let icon = |m: &str, w: f32| Piece::Icon {
            material: m.to_string(),
            w,
            h: ICON_SIZE,
            flip: false,
        };
        if parm & 0x80 != 0 {
            return icon(
                match parm & 0x7f {
                    7 => "killiconmelee",
                    8 => "killiconheadshot",
                    9 => "killiconcrush",
                    11 => "killiconfalling",
                    12 => "killiconsuicide",
                    15 => "killiconimpact",
                    _ => "killicondied",
                },
                ICON_SIZE,
            );
        }
        let weapon = self.weapon_name(parm);
        let Some(k) = self
            .loaded
            .as_ref()
            .and_then(|l| l.as_ref())
            .and_then(|l| l.kill_icons.get(weapon.as_deref()?))
        else {
            return icon("killicondied", ICON_SIZE);
        };
        let (w, h) = match k.ratio {
            1 => (2.0 * ICON_SIZE, ICON_SIZE),
            2 => (2.0 * ICON_SIZE, 0.5 * ICON_SIZE),
            _ => (ICON_SIZE, ICON_SIZE),
        };
        Piece::Icon {
            material: k.material.clone(),
            w,
            h,
            flip: k.flip,
        }
    }

    fn weapon_name(&self, n: u32) -> Option<String> {
        self.weapons.get((n as usize).checked_sub(1)?).cloned()
    }
}

/// A chat line broken as `CG_AddToTeamChat` breaks it (cod4-hud.md 4):
/// once a line holds more than [`CHAT_WIDTH`] characters, colour codes
/// not counted, the next character starts a new line, which takes along
/// whatever followed the line's last space. A carried tail counts every
/// character it holds, colour codes too.
fn wrap_chat(text: &str) -> Vec<String> {
    let mut lines = Vec::new();
    let mut line = String::new();
    let mut counted = 0;
    for (piece, width) in chat_pieces(text) {
        if counted > CHAT_WIDTH {
            let tail = line
                .rfind(' ')
                .map_or_else(String::new, |sp| line.split_off(sp + 1));
            counted = tail.chars().count();
            lines.push(std::mem::replace(&mut line, tail));
        }
        line.push_str(piece);
        counted += width;
    }
    if !line.is_empty() {
        lines.push(line);
    }
    lines
}

/// `text` as colour codes (`^` and a digit, no width) and single
/// characters (width 1).
fn chat_pieces(text: &str) -> impl Iterator<Item = (&str, usize)> {
    let mut rest = text;
    std::iter::from_fn(move || {
        let mut chars = rest.chars();
        let first = chars.next()?;
        let code = first == '^' && chars.next().is_some_and(|d| d.is_ascii_digit());
        let len = if code { 2 } else { first.len_utf8() };
        let (piece, tail) = rest.split_at(len);
        rest = tail;
        Some((piece, usize::from(!code)))
    })
}

fn push_line(lines: &mut Vec<Line>, pieces: Vec<Piece>, at: i32, keep: usize) {
    lines.push(Line { pieces, at });
    if lines.len() > keep {
        lines.remove(0);
    }
}

/// The clip graphic's pictures by `ammoCounterClip` 1..=5
/// (`CG_RegisterGraphics`).
const CLIP_MATERIALS: [&str; 5] = [
    "ammo_counter_bullet",
    "ammo_counter_riflebullet",
    "ammo_counter_shotgunshell",
    "ammo_counter_rocket",
    "ammo_counter_beltbullet",
];

/// The clip graphic's rounds (cod4-hud.md 9), the first nearest the
/// item's corner `base`: each of a `size` clip of `kind` as a real-pixel
/// rect `[x, y, w, h]`. The sizes are physical pixels, not the 640x480
/// grid's.
fn clip_rounds(kind: i32, size: i32, base: [f32; 2]) -> Vec<[f32; 4]> {
    let [bx, by] = base;
    let count = size.max(0) as usize;
    // Kinds 1..=4 run leftward, centred on the corner's height:
    // (width, height, pitch from one round's left to the next).
    let row = match kind {
        1 => Some((4.0, 8.0, 4.0)),
        2 => Some((32.0, 8.0, 40.0)),
        3 => Some((16.0, 8.0, 20.0)),
        4 => Some((64.0, 16.0, 72.0)),
        _ => None,
    };
    if let Some((w, h, pitch)) = row {
        let top = by - 0.5 * h;
        return (0..count)
            .map(|n| [bx - w - pitch * n as f32, top, w, h])
            .collect();
    }
    if kind != 5 {
        return Vec::new();
    }
    // The belt (`iw3mp.exe` 0x4228d0): 8 by 4 links in rows of 20, each
    // row 2 above the last, the even rows leftward from 8 left of the
    // corner and the odd ones back rightward from 160 left of it.
    let top = by + (count / 20) as f32 - 2.0;
    (0..count)
        .map(|i| {
            let (row, col) = (i / 20, (i % 20) as f32);
            let left = if row % 2 == 0 { col + 1.0 } else { 20.0 - col };
            [bx - 8.0 * left, top - 2.0 * row as f32, 8.0, 4.0]
        })
        .collect()
}

/// `cg.inKillCam`: the drawn playerstate is an archived one
/// (`ps.deltaTime` set, `CG_TransitionKillcam`).
/// The action slots' menu and the night vision slot's icon
/// (`cgMedia.hudIconNVG`).
const DPAD_MENU: &str = "dpad";
/// The low-health overlay's menu and its owner draw
/// (`CG_DrawPlayerLowHealthOverlay`, cod4-hud.md 19).
const LOW_HEALTH_MENU: &str = "low health";
const OD_LOW_HEALTH: i32 = 112;
const NVG_ICON: &str = "hud_icon_nvg";
/// The talker list's menu and its four owner draws (`CG_TALKER`, 193..196;
/// `CG_DrawTalkerNum`, `iw3mp.exe` 0x443de4), and the voice icons.
const TALKERS_MENU: &str = "talkers";
const OD_TALKER: i32 = 193;
const VOICE_ON: &str = "voice_on";
const VOICE_OFF: &str = "voice_off";
/// `perk_parabolicIcon`'s default (`iw3mp.exe` 0x6c2000).
const PARABOLIC_ICON: &str = "specialty_parabolic";

/// One action slot as `CG_ActionSlotIsUsable` and the slot's active
/// test read it.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct ActionSlot {
    usable: bool,
    active: bool,
    /// The weapon it shows, 0 for none.
    weapon: u8,
    night_vision: bool,
}

/// Action slot `k` (0..4) of the drawn playerstate: a weapon slot is
/// usable while its weapon is held and active while selected; an alt
/// slot while the selection or a held primary has an alt mode; a night
/// vision slot always, active while the goggles are on.
fn action_slot(s: &Cod4Session, snap: &Snapshot, k: usize) -> ActionSlot {
    use crate::play::cod4_input::{holds, slot_kind};
    let ps = &snap.ps;
    let u = |n: &str| ps.get(n).unwrap_or(0);
    let held: [u32; 4] = std::array::from_fn(|i| u(&format!("weapons[{i}]")));
    let selected = s.input.weapons().0;
    match u(&format!("actionSlotType[{k}]")) {
        slot_kind::WEAPON => {
            let w = u(&format!("actionSlotParam[{k}]")) as u8;
            let usable = w != 0 && holds(held, w);
            ActionSlot {
                usable,
                active: w != 0 && w == selected,
                weapon: if usable { w } else { 0 },
                night_vision: false,
            }
        }
        slot_kind::ALT_WEAPON => {
            let (w, active) = s.alt_toggle_weapon();
            ActionSlot {
                usable: w != 0,
                active,
                weapon: w,
                night_vision: false,
            }
        }
        slot_kind::NIGHT_VISION => ActionSlot {
            usable: true,
            active: crate::cod4_look::night_vision(u("weapFlags")),
            weapon: 0,
            night_vision: true,
        },
        _ => ActionSlot::default(),
    }
}

/// What the HUD menus' expressions read (`Cod4Hud::env`).
struct HudEnv<'a> {
    hud: &'a Cod4Hud,
    s: &'a Cod4Session,
    game: vcod_cod4::menuexp::GameView,
    team: i32,
    shell: &'a Shell,
    /// The UI's `scoreboard` menu is up: `+scores` held or the
    /// intermission (cod4-hud.md 23).
    board_menu: bool,
}

impl vcod_cod4::menuexp::Env for HudEnv<'_> {
    fn dvar(&self, name: &str) -> Option<String> {
        self.hud
            .server_dvar(self.s, name)
            .or_else(|| self.s.menus.env().dvar(name))
    }
    fn local(&self, name: &str) -> Option<vcod_cod4::menuexp::Value> {
        self.s.menus.env().local(name)
    }
    fn stat(&self, index: i32) -> i32 {
        self.s.menus.env().stat(index)
    }
    fn team(&self) -> i32 {
        self.team
    }
    fn table_lookup(&self, table: &str, column: i32, value: &str, ret: i32) -> String {
        self.s.menus.env().table_lookup(table, column, value, ret)
    }
    fn game(&self) -> Option<&vcod_cod4::menuexp::GameView> {
        Some(&self.game)
    }
    /// `UI_GetKeyBindingLocalizedStringSingle`: the first key's `KEY_*`
    /// text, `KEY_UNBOUND`'s with none.
    fn key_binding(&self, cmd: &str) -> String {
        let empty = Localized::default();
        let loc = self.hud.localized().unwrap_or(&empty);
        match self.shell.keys_bound_to(cmd).first() {
            Some(k) => loc.get(&format!("KEY_{k}")).unwrap_or(k).to_string(),
            None => loc.get("KEY_UNBOUND").unwrap_or("KEY_UNBOUND").to_string(),
        }
    }
    fn localize(&self, text: &str) -> String {
        self.hud.message(text)
    }
    fn gametype(&self) -> String {
        self.hud.gametype_strings(self.s).0
    }
    fn gametype_name(&self) -> String {
        self.hud.gametype_strings(self.s).1
    }
    fn gametype_description(&self) -> String {
        self.hud.gametype_strings(self.s).2
    }
    fn ui_active(&self) -> bool {
        self.s.menus.open().is_some() || self.board_menu
    }
    fn menu_open(&self, name: &str) -> bool {
        (self.board_menu && name.eq_ignore_ascii_case(menus::SCOREBOARD_MENU))
            || self.s.menus.env().menu_open(name)
    }
}

/// `colorLowAmmo` (1, 0.3, 0.3; `iw3mp.exe` rdata 0x6be598).
const LOW_AMMO_RGB: [f32; 3] = [1.0, 0.3, 0.3];

/// The low clip's flash (cod4-hud.md 15.5): the clip graphic's `fore`
/// colour eased toward [`LOW_AMMO_RGB`] by `sin(t / (40 pi)) / 2 + 1/2`,
/// `t` the ms since the clip last read not low.
fn low_clip_flash(fore: [f32; 4], since_ms: i32) -> [f32; 4] {
    let period = 40.0 * std::f32::consts::PI;
    let wave = (since_ms as f32 / period).sin();
    let toward = 0.5 + wave * 0.5;
    let mut rgba = fore;
    for (c, red) in rgba[..3].iter_mut().zip(LOW_AMMO_RGB) {
        *c += toward * (red - *c);
    }
    rgba
}
/// `CG_CheckPlayerForLowAmmoSpecific`: a reserve at or under a fifth of
/// the player's maximum reads low.
const LOW_RESERVE: f32 = 0.2;

/// The held weapon's ammo, as the low-ammo checks read it.
#[derive(Clone, Copy, Debug, PartialEq)]
struct HeldAmmo {
    clip: i32,
    clip_size: i32,
    clip_only: bool,
    threshold: f32,
    reserve: i32,
    /// `BG_GetAmmoPlayerMax`.
    max: i32,
}

impl HeldAmmo {
    /// `CG_CheckPlayerForLowClipSpecific`: not a clip-only weapon, and a
    /// clip at or under `lowAmmoWarningThreshold` of its size.
    fn low_clip(&self) -> bool {
        let clip = self.clip.min(999);
        let size = self.clip_size.min(999);
        !self.clip_only && clip >= 0 && size > 0 && clip as f32 <= self.threshold * size as f32
    }

    /// `CG_CheckPlayerForLowAmmoSpecific`: the reserve at or under a fifth
    /// of the player's maximum.
    fn low_reserve(&self) -> bool {
        self.max >= 0 && self.max.min(999) as f32 * LOW_RESERVE >= self.reserve.min(999) as f32
    }
}

/// The drawn playerstate's held weapon's ammo (`ps.weapon`), `None` for
/// no weapon, a spectator or a dead player.
fn held_ammo(l: &Loaded, s: &Cod4Session, snap: &Snapshot) -> Option<HeldAmmo> {
    let ps = &snap.ps;
    let u = |n: &str| ps.get(n).unwrap_or(0);
    if u("pm_type") >= PM_SPECTATOR {
        return None;
    }
    let list = crate::play::cod4::weapon_list(s.gamestate()?);
    let slots = l.ammo_slots(&list);
    let w = (u("weapon") as usize).checked_sub(1)?;
    let d = l.weapon_hud.get(list.get(w)?)?;
    let (ammo, clip) = slots[w];
    let clip_n = ps.ammoclip.get(clip).copied().unwrap_or(0);
    // `BG_GetAmmoPlayerMax`: a shared cap, a clip-only weapon's clip, else
    // the sum over every held weapon on this ammo slot.
    let max = if d.shared_ammo_cap > 0 {
        d.shared_ammo_cap
    } else if d.clip_only {
        d.clip_size
    } else {
        (0..list.len())
            .filter(|&k| u(&format!("weapons[{}]", (k + 1) / 32)) >> ((k + 1) % 32) & 1 != 0)
            .filter(|&k| slots[k].0 == ammo)
            .filter_map(|k| l.weapon_hud.get(&list[k]))
            .map(|d| d.max_ammo)
            .sum()
    };
    Some(HeldAmmo {
        clip: clip_n,
        clip_size: d.clip_size,
        clip_only: d.clip_only,
        threshold: d.low_ammo_threshold,
        reserve: if d.clip_only {
            clip_n
        } else {
            ps.ammo.get(ammo).copied().unwrap_or(0)
        },
        max,
    })
}

fn in_killcam(snap: &Snapshot) -> bool {
    snap.ps.get("deltaTime").unwrap_or(0) != 0
}

/// `cg_hudSayPosition`'s default (5, 180).
const SAY_POS: (f32, f32) = (5.0, 180.0);

/// `Con_DrawSay`: `EXE_SAY: ` or `EXE_SAYTEAM: ` and the typed
/// line with its cursor, white, font 0 at 16 px over a 48 px em (a 768
/// line screen or less), its top 24 under `cg_hudSayPosition`.
fn draw_say(
    l: &Loaded,
    field: &crate::hud::ChatField,
    sp: &ScreenPlacement,
    out: &mut Vec<HudQuad>,
) {
    const SCALE: f32 = 16.0 / 48.0;
    let key = if field.team { "EXE_SAYTEAM" } else { "EXE_SAY" };
    let label = format!("{}: ", l.localized.get(key).unwrap_or(key));
    let f = l.fonts.pick(0, SCALE, sp);
    let px = SCALE / f.unit_scale();
    let th = (48.0 * SCALE).trunc();
    let (x, _) = sp.axis(0, SAY_POS.0, 0.0, 1);
    let (y, _) = sp.axis(1, SAY_POS.1 + 24.0, th, 1);
    let k = sp.virtual_to_real[1];
    let w = font::layout(f, &label, x, y, px * k, [1.0; 4], out);
    font::layout(
        f,
        &format!("{}_", field.text),
        x + w,
        y,
        px * k,
        [1.0; 4],
        out,
    );
}

/// `pmtype_t`: `PM_DEAD` and up hide `HE_FLAG_HIDE_WHEN_DEAD` elements;
/// `PM_SPECTATOR` and up draw no weapon HUD.
const PM_DEAD: u32 = 7;
const PM_SPECTATOR: u32 = 4;

/// One `weaponinfo` owner draw, placed by its menu item.
#[derive(Clone, Debug, PartialEq)]
enum OwnerDraw {
    /// Text at `x` on the item's baseline.
    Text {
        text: String,
        x: f32,
        item: h::MenuItem,
        rgba: [f32; 4],
    },
    /// The clip graphic (117): the clip's rounds as a row of `kind`'s
    /// pictures leftward of the item's corner, those fired grey.
    Clip {
        kind: i32,
        size: i32,
        rounds: i32,
        item: h::MenuItem,
        /// The live rounds' colour, flashing on a low clip (cod4-hud.md
        /// 15.5).
        rgba: [f32; 4],
    },
    /// `CG_DrawPlayerWeaponName`: ending 28 left of the item's right edge.
    Name {
        text: String,
        item: h::MenuItem,
    },
    Pic {
        material: String,
        item: h::MenuItem,
    },
}

impl Cod4Hud {
    /// The HUD at server time `now` on a `size` window, empty until the
    /// fonts load or a snapshot arrives.
    pub(crate) fn build(
        &self,
        s: &Cod4Session,
        size: (f32, f32),
        now: i32,
        view: Option<(glam::Vec3, f32)>,
        shell: &Shell,
    ) -> Vec<HudQuad> {
        let mut out = Vec::new();
        let Some(Some(l)) = &self.loaded else {
            return out;
        };
        let Some(snap) = s.client().snapshots.last() else {
            return out;
        };
        let sp = ScreenPlacement::cod4(size.0, size.1);
        let dvar = |n: &str| self.server_dvar(s, n);
        // `^8` the drawn player's team, `^9` the other (cod4-hud.md 3).
        font::set_team_colours(true, &|n: &str| match n {
            "g_TeamColor_Allies" => dvar("g_TeamColor_MyTeam"),
            "g_TeamColor_Axis" => dvar("g_TeamColor_EnemyTeam"),
            _ => None,
        });
        // `CG_Draw2D`'s intermission arm (`DrawIntermission`):
        // the board and the chat, then the say field, nothing else.
        if snap.ps.get("pm_type") == Some(h::PM_INTERMISSION) {
            for p in self.scoreboard(l, s, snap, &sp, 1.0, shell) {
                self.draw_piece(l, &sp, &p, &mut out);
            }
            let env = self.env(s, now, shell);
            let board = menus::SCOREBOARD_MENU;
            self.draw_zone_menu(l, board, &env, s, snap, shell, &sp, now, &mut out);
            self.draw_chat(l, &sp, now, &mut out);
            if let Some(field) = &self.chat_field {
                draw_say(l, field, &sp, &mut out);
            }
            return out;
        }
        // `cg.inKillCam`: the snapshot carries `ps.deltaTime`
        // (`CG_TransitionKillcam`); the `killcam` menu draws
        // under everything (cod4-hud.md 11).
        if in_killcam(snap) {
            self.draw_killcam(l, &sp, &mut out);
        }
        let dead = snap.ps.get("pm_type").unwrap_or(0) >= PM_DEAD;
        let in_menu = s.menus.open().is_some();
        let elems: Vec<HudElem> = h::hud_elems(&snap.ps)
            .into_iter()
            .filter(|e| !(dead && e.flags & h::HE_FLAG_HIDE_WHEN_DEAD != 0))
            .filter(|e| !(in_menu && e.flags & h::HE_FLAG_HIDE_WHEN_IN_MENU != 0))
            .collect();
        self.draw_elems(l, s, &elems, false, &sp, now, shell, &mut out);
        // Each HUD menu's `visible when` (cod4-hud.md 15): the compass
        // also in hardcore while our team's UAV is up; `ui_hud_hardcore`
        // alone when a menu's expression does not evaluate.
        let hardcore = self.hardcore(s);
        let env = self.env(s, now, shell);
        let shown = |m: &str| self.menu_visible(l, m, &env).unwrap_or(!hardcore);
        if shown(LOW_HEALTH_MENU) {
            self.draw_low_health(l, &sp, now, &mut out);
        }
        if let Some(v) = view.filter(|_| !dead && shown(h::COMPASS_MENU)) {
            self.draw_compass(l, s, v, now, &sp, &mut out);
        }
        if shown(h::WEAPON_INFO_MENU) {
            for d in self.weapon_info(l, s, snap, now) {
                self.draw_owner(l, &sp, &d, &mut out);
            }
        }
        if shown(DPAD_MENU) {
            for d in self.dpad(l, s, snap, &env) {
                self.draw_owner(l, &sp, &d, &mut out);
            }
        }
        if shown(TALKERS_MENU) {
            for d in self.talkers(l, s, snap, shell) {
                self.draw_owner(l, &sp, &d, &mut out);
            }
        }
        // The bar and hint menus, item by item (cod4-hud.md 23).
        for m in menus::BAR_MENUS.into_iter().chain(menus::HINT_MENUS) {
            self.draw_zone_menu(l, m, &env, s, snap, shell, &sp, now, &mut out);
        }
        if !dead && shown(hints::CURSOR_HINTS_MENU) {
            self.draw_cursor_hint(l, s, &env, &sp, now, &mut out);
        }
        // The location selector's full map (cod4-hud.md 15.8).
        self.draw_fullscreen_map(l, s, &env, shell, view, &sp, &mut out);
        for i in 0..2 {
            self.draw_window(l, i, &sp, now, &mut out);
        }
        let board = self.scoreboard_fade(snap, now);
        if board.is_none() {
            self.draw_chat(l, &sp, now, &mut out);
        }
        self.draw_center(l, &sp, now, &mut out);
        if let Some(field) = &self.chat_field {
            draw_say(l, field, &sp, &mut out);
        }
        self.draw_elems(l, s, &elems, true, &sp, now, shell, &mut out);
        if let Some(fade) = board {
            for p in self.scoreboard(l, s, snap, &sp, fade, shell) {
                self.draw_piece(l, &sp, &p, &mut out);
            }
            if self.show_scores {
                let board = menus::SCOREBOARD_MENU;
                self.draw_zone_menu(l, board, &env, s, snap, shell, &sp, now, &mut out);
            }
            // The chat draws over the board (`chatOverScoreboard`).
            self.draw_chat(l, &sp, now, &mut out);
        }
        self.draw_vote(l, s, snap, &sp, now, shell, &mut out);
        if s.interrupted() {
            draw_interrupted(l, &sp, now, &mut out);
        }
        out
    }

    /// `CG_DrawVote` (`iw3mp.exe` 0x42ed50), while configstring 13 names
    /// a vote of this server (`%d %d`, its end and `sv_serverid`; 15 and
    /// 16 the counts, 14 the localized text; `CG_ConfigStringModified`,
    /// 0x44afd0): at `cg_hudVotePosition` (5, 220) two lines a font size
    /// apart, size 10 over a 768-line window, else 16, scale size / 48,
    /// `CGAME_VOTE(<seconds left>):<text>` and the tallies, each with the
    /// key bound to `vote yes` or `vote no` (the command itself when none
    /// is) until the player voted (`eFlags` 0x100000). cod4-hud.md 14.
    #[allow(clippy::too_many_arguments)]
    fn draw_vote(
        &self,
        l: &Loaded,
        s: &Cod4Session,
        snap: &Snapshot,
        sp: &ScreenPlacement,
        now: i32,
        shell: &Shell,
        out: &mut Vec<HudQuad>,
    ) {
        let Some(end) = self.vote_end(s) else {
            return;
        };
        let loc = |k: &str| l.localized.get(k).unwrap_or(k).to_string();
        let count = |i: u16| h::atoi(&self.configstring(s, i));
        let what = self.message(&self.configstring(s, CS_VOTE_STRING));
        let secs = ((end - now) / 1000).max(0);
        let first = format!("{}({secs}):{what}", loc("CGAME_VOTE"));
        let (yes, no) = (count(CS_VOTE_YES), count(CS_VOTE_NO));
        let second = if snap.ps.get("eFlags").unwrap_or(0) & EF_VOTED != 0 {
            format!("{}:{yes}, {}:{no}", loc("CGAME_YES"), loc("CGAME_NO"))
        } else {
            let key = |cmd: &str| match shell.key_text(cmd, &l.localized) {
                Some(_) => self.key_binding(shell, cmd),
                None => cmd.to_string(),
            };
            format!(
                "{}({}):{yes}, {}({}):{no}",
                loc("CGAME_YES"),
                key("vote yes"),
                loc("CGAME_NO"),
                key("vote no")
            )
        };
        let size = if sp.real_size[1] > 768.0 { 10.0 } else { 16.0 };
        let scale = size / 48.0;
        let f = l.fonts.pick(0, scale, sp);
        let px = scale / f.unit_scale();
        let [x, y] = VOTE_POSITION;
        for (k, line) in [first, second].iter().enumerate() {
            // UI_DrawText's y is the baseline, a font size under the last.
            let baseline = y + size * (k + 1) as f32;
            let h = (48.0 * scale) as i32 as f32;
            let (rx, _) = sp.axis(0, x, 0.0, 1);
            let (ry, _) = sp.axis(1, baseline - h, h, 1);
            text(f, line, rx, ry, px * sp.virtual_to_real[1], [1.0; 4], out);
        }
    }

    /// The running vote's end in server time, when configstring 13 names
    /// one of this server's (`sv_serverid` in the system info).
    fn vote_end(&self, s: &Cod4Session) -> Option<i32> {
        let cs = self.configstring(s, CS_VOTE_TIME);
        let mut w = cs.split_whitespace().map(h::atoi);
        let (end, id) = (w.next()?, w.next()?);
        let ours = crate::play::cod4::info_value(self.configstring(s, 1).as_bytes(), "sv_serverid")
            .map_or(0, |v| h::atoi(&v));
        (end != 0 && id == ours).then_some(end)
    }

    /// The `weaponinfo` menu's owner draws off the drawn
    /// playerstate: the stock (119), the clip (117, its rounds), the
    /// weapon's name (81), and the frag and second offhand's icons (103,
    /// 104) and counts (105, 106). Nothing for a spectator, a dead
    /// player, one on a turret or with the HUD's weapon off
    /// (`weapFlags` 0x80). cod4-hud.md 9.
    fn weapon_info(
        &self,
        l: &Loaded,
        s: &Cod4Session,
        snap: &Snapshot,
        now: i32,
    ) -> Vec<OwnerDraw> {
        let mut out = Vec::new();
        let ps = &snap.ps;
        let u = |n: &str| ps.get(n).unwrap_or(0);
        if u("pm_type") >= PM_SPECTATOR || u("weapFlags") & 0x80 != 0 || u("eFlags") & 0x20000 != 0
        {
            return out;
        }
        let Some(gs) = s.gamestate() else {
            return out;
        };
        let list = crate::play::cod4::weapon_list(gs);
        let slots = l.ammo_slots(&list);
        let def = |w: usize| {
            l.weapon_hud
                .get(list.get(w.checked_sub(1)?)?)
                .map(|d| (d, slots[w - 1]))
        };
        let held: Vec<usize> = (1..list.len() + 1)
            .filter(|&b| u(&format!("weapons[{}]", b / 32)) >> (b % 32) & 1 != 0)
            .collect();
        let clip = |slot: usize| ps.ammoclip.get(slot).copied().unwrap_or(0);
        let stock = |slot: usize| ps.ammo.get(slot).copied().unwrap_or(0);
        let item = |name: &str| l.weapon_info.get(name);
        let text = |out: &mut Vec<OwnerDraw>, it: &h::MenuItem, t: String, x: f32, rgba| {
            out.push(OwnerDraw::Text {
                text: t,
                x,
                item: it.clone(),
                rgba,
            })
        };
        let weapon = u("weapon") as usize;
        let low = held_ammo(l, s, snap);
        if let Some((d, (ammo, cl))) = def(weapon) {
            if let Some(it) = item("ammoStock") {
                // `CG_CheckPlayerForLowAmmoSpecific`: the reserve in red.
                let mut rgba = it.fore_color;
                if low.is_some_and(|w| w.low_reserve()) {
                    rgba[..3].copy_from_slice(&LOW_AMMO_RGB);
                }
                text(
                    &mut out,
                    it,
                    format!("{:3}", stock(ammo)),
                    it.rect.rect[0],
                    rgba,
                );
            }
            // An alt weapon's (6) is not drawn: the walk keeps no
            // `altWeapon`.
            if let Some(it) = item("clipGraphic")
                && (1..=5).contains(&d.ammo_counter_clip)
            {
                let rgba = if low.is_some_and(|w| w.low_clip()) {
                    low_clip_flash(it.fore_color, now - self.clip_flash_from)
                } else {
                    it.fore_color
                };
                out.push(OwnerDraw::Clip {
                    kind: d.ammo_counter_clip,
                    size: d.clip_size,
                    rounds: clip(cl),
                    item: it.clone(),
                    rgba,
                });
            }
            if let Some(it) = item("weapname") {
                let mut name = self.message(&d.display_name);
                if !d.mode_name.is_empty() {
                    name = format!("{name} / {}", self.message(&d.mode_name));
                }
                out.push(OwnerDraw::Name {
                    text: name,
                    item: it.clone(),
                });
            }
        }
        let second = if u("offhandSecondary") == 1 { 3 } else { 2 };
        for (icon, count, class) in [
            ("offhandFragIcon", "offHandFragAmmo", 1),
            ("offhandSmokeIcon", "offHandSmokeAmmo", second),
        ] {
            let of_class: Vec<(usize, i32)> = held
                .iter()
                .filter_map(|&w| {
                    let (d, (_, cl)) = def(w)?;
                    (d.offhand_class == class).then_some((w, clip(cl)))
                })
                .collect();
            // The class's first weapon with rounds, else its first
            // (cod4-hud.md 9).
            let Some(&(best, _)) = of_class.iter().find(|(_, n)| *n > 0).or(of_class.first())
            else {
                continue;
            };
            if let (Some(it), Some(m)) =
                (item(icon), def(best).and_then(|(d, _)| d.hud_icon.clone()))
            {
                out.push(OwnerDraw::Pic {
                    material: m,
                    item: it.clone(),
                });
            }
            if let Some(it) = item(count) {
                let n: i32 = of_class.iter().map(|(_, n)| n).sum();
                let rgba = if n != 0 {
                    it.fore_color
                } else {
                    [0.89, 0.18, 0.01, it.fore_color[3]]
                };
                text(&mut out, it, n.to_string(), it.rect.rect[0], rgba);
            }
        }
        out
    }

    /// One owner draw at its item's rect and alignment.
    fn draw_owner(&self, l: &Loaded, sp: &ScreenPlacement, d: &OwnerDraw, out: &mut Vec<HudQuad>) {
        let put_text =
            |out: &mut Vec<HudQuad>, it: &h::MenuItem, t: &str, x: f32, rgba: [f32; 4]| {
                let scale = it.text_scale;
                let f = l.fonts.pick(it.font, scale, sp);
                let px = scale / f.unit_scale();
                let h = (48.0 * scale) as i32 as f32;
                let (rx, _) = sp.axis(0, x, 0.0, it.rect.horz_align);
                let (ry, _) = sp.axis(1, it.rect.rect[1] - h, h, it.rect.vert_align);
                text(f, t, rx, ry, px * sp.virtual_to_real[1], rgba, out);
            };
        let width = |it: &h::MenuItem, t: &str| {
            let f = l.fonts.pick(it.font, it.text_scale, sp);
            font::measure(f, t, it.text_scale / f.unit_scale())
        };
        match d {
            OwnerDraw::Text {
                text: t,
                x,
                item,
                rgba,
            } => put_text(out, item, t, *x, *rgba),
            OwnerDraw::Clip {
                kind,
                size,
                rounds,
                item,
                rgba: live,
            } => {
                let r = &item.rect;
                let (bx, _) = sp.axis(0, r.rect[0], 0.0, r.horz_align);
                let (by, _) = sp.axis(1, r.rect[1], 0.0, r.vert_align);
                for (i, [x, y, w, h]) in clip_rounds(*kind, *size, [bx, by]).into_iter().enumerate()
                {
                    let mut rgba = *live;
                    if i as i32 >= *rounds {
                        rgba[..3].copy_from_slice(&[0.3; 3]);
                    }
                    if let Some(tex) = image_of(l, CLIP_MATERIALS[*kind as usize - 1]) {
                        out.push(quad(x, y, w, h, rgba, tex, false));
                    }
                }
            }
            OwnerDraw::Name { text: t, item } => {
                let x = item.rect.rect[2] + item.rect.rect[0] - width(item, t) - 28.0;
                put_text(out, item, t, x, item.fore_color);
            }
            OwnerDraw::Pic { material, item } => {
                if let Some(tex) = image_of(l, material) {
                    let [x, y, w, h] = item.rect.rect;
                    let (rx, rw) = sp.axis(0, x, w, item.rect.horz_align);
                    let (ry, rh) = sp.axis(1, y, h, item.rect.vert_align);
                    out.push(quad(rx, ry, rw, rh, item.fore_color, tex, false));
                }
            }
        }
    }

    /// The obituary's centre print for `cg_centertime`, 150 under the
    /// screen's middle.
    fn draw_center(&self, l: &Loaded, sp: &ScreenPlacement, now: i32, out: &mut Vec<HudQuad>) {
        let Some((line, at)) = &self.center else {
            return;
        };
        if now - at >= CENTER_MS {
            return;
        }
        let f = l.fonts.pick(0, CENTER_SCALE, sp);
        let px = CENTER_SCALE / f.unit_scale();
        let w = font::measure(f, line, px);
        let h = (48.0 * CENTER_SCALE) as i32 as f32;
        let (x, _) = sp.axis(0, -0.5 * w, w, 2);
        let (y, _) = sp.axis(1, CENTER_Y - h, h, 2);
        text(f, line, x, y, px * sp.virtual_to_real[1], [1.0; 4], out);
    }

    /// The `killcam` menu: its letterbox bars, the `white` material in
    /// their colour, and its title centred on the item's x at its
    /// baseline (cod4-hud.md 11).
    fn draw_killcam(&self, l: &Loaded, sp: &ScreenPlacement, out: &mut Vec<HudQuad>) {
        for it in &l.killcam {
            let [x, y, w, h] = it.rect.rect;
            if let Some(t) = &it.text {
                let t = l.localized.get(t).unwrap_or(t);
                let f = l.fonts.pick(it.font, it.text_scale, sp);
                let px = it.text_scale / f.unit_scale();
                let tw = font::measure(f, t, px);
                let fh = (48.0 * it.text_scale) as i32 as f32;
                let tx = match it.text_align {
                    1 => x + 0.5 * w - 0.5 * tw,
                    2 => x + w - tw,
                    _ => x,
                };
                let (rx, _) = sp.axis(0, tx, tw, it.rect.horz_align);
                let (ry, _) = sp.axis(1, y + h - fh, fh, it.rect.vert_align);
                text(f, t, rx, ry, px * sp.virtual_to_real[1], it.fore_color, out);
            } else if let Some(tex) = it.rect.background.as_deref().and_then(|m| image_of(l, m)) {
                let (rx, rw) = sp.axis(0, x, w, it.rect.horz_align);
                let (ry, rh) = sp.axis(1, y, h, it.rect.vert_align);
                out.push(quad(rx, ry, rw, rh, it.fore_color, tex, false));
            }
        }
    }

    /// The board's pieces on the grid (`cod4_scoreboard::layout`).
    fn scoreboard(
        &self,
        l: &Loaded,
        s: &Cod4Session,
        snap: &Snapshot,
        sp: &ScreenPlacement,
        fade: f32,
        shell: &Shell,
    ) -> Vec<crate::cod4_scoreboard::Piece> {
        // `CG_DrawScoreboard` draws the backdrop whether or not a `b`
        // came: until one does, the board has no rows.
        let none = Scores::default();
        let scores = self.scores.as_ref().unwrap_or(&none);
        let gs = s.gamestate();
        let cs = |i: u16| self.configstring(s, i);
        let dvar = |n: &str| self.server_dvar(s, n);
        let localize = |k: &str| match self.localized() {
            Some(loc) => loc.get(k).map_or_else(|| k.to_string(), str::to_string),
            None => k.to_string(),
        };
        let font = |scale: f32| l.fonts.pick(h::scoreboard::FONT, scale, sp);
        let rank_icon =
            |r: i32, p: i32| l.rank_icons.get(&(r, p)).filter(|m| !m.is_empty()).cloned();
        let rank_level = |r: i32| l.rank_levels.get(&r).cloned();
        let talk_icon = |c: u32| {
            if s.menus.is_muted(c) {
                Some(VOICE_OFF)
            } else {
                s.is_talking(c).then_some(VOICE_ON)
            }
        };
        let host = gs
            .and_then(|g| crate::play::cod4::info_value(g.configstring(0), "sv_hostname"))
            .unwrap_or_default();
        let mut inputs = crate::cod4_scoreboard::Inputs {
            scores,
            snap,
            own: gs.map_or(0, |g| g.client_num as u32),
            grid_width: sp.real_size[0] / sp.virtual_to_real[0],
            fade,
            host_name: host,
            configstring: &cs,
            dvar: &dvar,
            localize: &localize,
            font: &font,
            rank_icon: &rank_icon,
            rank_level: &rank_level,
            talk_icon: &talk_icon,
            top: self.scores_top.get(),
            ping_graph: ping_graph(shell),
        };
        // `CG_DrawScoreboard`: a board not yet placed opens on our row.
        if inputs.top <= 0 {
            inputs.top = crate::cod4_scoreboard::centred_top(&inputs);
            self.scores_top.set(inputs.top);
        }
        let board = crate::cod4_scoreboard::layout(&inputs);
        let total = crate::cod4_scoreboard::total_lines(&inputs);
        self.scores_off_bottom.set((board.off_bottom, total));
        board.pieces
    }

    /// The board's pieces on the newest snapshot, as `build` draws them.
    #[cfg(test)]
    pub(crate) fn board_pieces(
        &self,
        s: &Cod4Session,
        shell: &Shell,
    ) -> Vec<crate::cod4_scoreboard::Piece> {
        let (Some(Some(l)), Some(snap)) = (&self.loaded, s.client().snapshots.last()) else {
            return Vec::new();
        };
        let sp = ScreenPlacement::new(1280.0, 960.0);
        self.scoreboard(l, s, snap, &sp, 1.0, shell)
    }

    /// The board's pieces as `build` puts them on a `size` window, text
    /// and pictures alike.
    #[cfg(test)]
    pub(crate) fn board_quads(
        &self,
        s: &Cod4Session,
        size: (f32, f32),
        shell: &Shell,
    ) -> Vec<HudQuad> {
        let (Some(Some(l)), Some(snap)) = (&self.loaded, s.client().snapshots.last()) else {
            return Vec::new();
        };
        let sp = ScreenPlacement::cod4(size.0, size.1);
        let mut out = Vec::new();
        for p in self.scoreboard(l, s, snap, &sp, 1.0, shell) {
            self.draw_piece(l, &sp, &p, &mut out);
        }
        out
    }

    /// A piece on the grid, left and top aligned (`UI_DrawText`,
    /// `UI_DrawHandlePic` with `horzAlign` 1, `vertAlign` 0).
    fn draw_piece(
        &self,
        l: &Loaded,
        sp: &ScreenPlacement,
        p: &crate::cod4_scoreboard::Piece,
        out: &mut Vec<HudQuad>,
    ) {
        use crate::cod4_scoreboard::Piece as P;
        match p {
            P::Text {
                text: t,
                x,
                y,
                scale,
                rgba,
            } => {
                let f = l.fonts.pick(h::scoreboard::FONT, *scale, sp);
                let px = scale / f.unit_scale();
                let h = (48.0 * scale) as i32 as f32;
                let (rx, _) = sp.axis(0, *x, 0.0, 1);
                let (ry, _) = sp.axis(1, y - h, h, 0);
                text(f, t, rx, ry, px * sp.virtual_to_real[1], *rgba, out);
            }
            P::Pic {
                material,
                x,
                y,
                w,
                h,
                rgba,
            } => {
                if let Some(tex) = image_of(l, material) {
                    let (rx, rw) = sp.axis(0, *x, *w, 1);
                    let (ry, rh) = sp.axis(1, *y, *h, 0);
                    out.push(quad(rx, ry, rw, rh, *rgba, tex, false));
                }
            }
        }
    }

    /// One pass of `CG_Draw2dHudElems`: the elements
    /// of that pass by `sort`, text through the
    /// label and the text or value, materials by their configstring's
    /// image. Waypoints (in the world) are left out.
    #[allow(clippy::too_many_arguments)]
    fn draw_elems(
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

    /// A game message window (`gamemessages` 0, `boldgamemessages` 1) at
    /// its menu item's rect: lines that have lived less than the window's
    /// message time, fading in and out, growing up from the rect in mode
    /// 1 and down in mode 0.
    fn draw_window(
        &self,
        l: &Loaded,
        i: usize,
        sp: &ScreenPlacement,
        now: i32,
        out: &mut Vec<HudQuad>,
    ) {
        let Some(w) = l.windows.get(&(i as i32)) else {
            return;
        };
        let life = MSG_MS[i];
        let lines: Vec<&Line> = self.windows[i]
            .iter()
            .filter(|x| now - x.at < life)
            .collect();
        let f = l.fonts.pick(w.font, w.text_scale, sp);
        let n = lines.len();
        for (row, line) in lines.iter().enumerate() {
            let age = now - line.at;
            let a = (age as f32 / FADE_IN_MS[i] as f32)
                .min((life - age) as f32 / FADE_OUT_MS[i] as f32)
                .clamp(0.0, 1.0);
            let step = match w.mode {
                1 => -((n - 1 - row) as f32),
                _ => row as f32,
            };
            self.draw_line(f, w, w.y + step * 48.0 * w.text_scale, line, a, sp, out);
        }
    }

    /// One message line on the window's grid line `y`, its pieces left to
    /// right; an icon is font heights wide and tall, centred on the text.
    #[allow(clippy::too_many_arguments)]
    fn draw_line(
        &self,
        f: &Font,
        w: &MessageWindow,
        y: f32,
        line: &Line,
        alpha: f32,
        sp: &ScreenPlacement,
        out: &mut Vec<HudQuad>,
    ) {
        let Some(Some(l)) = &self.loaded else { return };
        let scale = w.text_scale;
        let px = scale / f.unit_scale();
        let h = (48.0 * scale) as i32 as f32;
        let width: f32 = line
            .pieces
            .iter()
            .map(|p| match p {
                Piece::Text(t) => font::measure(f, t, px),
                Piece::Icon { w, .. } => w * h,
            })
            .sum();
        let x = match w.alignment {
            1 => w.x - 0.5 * width,
            2 => w.x - width,
            _ => w.x,
        };
        let (mut rx, _) = sp.axis(0, x, width, w.horz_align);
        let (ry, rh) = sp.axis(1, y - h, h, w.vert_align);
        let k = sp.virtual_to_real[1];
        for p in &line.pieces {
            match p {
                Piece::Text(t) => {
                    rx += text(f, t, rx, ry, px * k, [1.0, 1.0, 1.0, alpha], out);
                }
                Piece::Icon {
                    material,
                    w: iw,
                    h: ih,
                    flip,
                } => {
                    let (qw, qh) = (iw * rh, ih * rh);
                    if let Some(tex) = image_of(l, material) {
                        let qy = ry + 0.5 * (rh - qh);
                        out.push(quad(rx, qy, qw, qh, [1.0, 1.0, 1.0, alpha], tex, *flip));
                    }
                    rx += qw;
                }
            }
        }
    }

    /// The chat ring at `cg_hudChatPosition`, oldest on top, each line
    /// for `cg_chatTime`.
    fn draw_chat(&self, l: &Loaded, sp: &ScreenPlacement, now: i32, out: &mut Vec<HudQuad>) {
        let f = l.fonts.pick(0, CHAT_SCALE, sp);
        let px = CHAT_SCALE / f.unit_scale();
        let h = (48.0 * CHAT_SCALE) as i32 as f32;
        let live = self.chat.iter().filter(|c| now - c.at < CHAT_MS);
        for (row, line) in live.enumerate() {
            let Some(Piece::Text(t)) = line.pieces.first() else {
                continue;
            };
            let (x, _) = sp.axis(0, CHAT_POS.0, 0.0, 1);
            let (y, _) = sp.axis(1, CHAT_POS.1 + row as f32 * h, h, 1);
            text(f, t, x, y, px * sp.virtual_to_real[1], [1.0; 4], out);
        }
    }
}

/// [`font::layout`] with its shadow faded with the text: the shared
/// layout draws every shadow at 0.8 alpha.
/// `CG_DrawDisconnect` (`iw3mp.exe` 0x42f930): `CGAME_CONNECTIONINTERUPTED`
/// at scale 0.5 centred on the 640 grid, its baseline at y 100 (0x6f9e58),
/// white in style 3, and unless `cg.time & 0x200` the `net_disconnect`
/// material at (296, 320), 48 by 48 (0x6c7f44, 0x6c7f48, 0x70bdcc).
/// cod4-hud.md 14.
fn draw_interrupted(l: &Loaded, sp: &ScreenPlacement, now: i32, out: &mut Vec<HudQuad>) {
    let line = l
        .localized
        .get("CGAME_CONNECTIONINTERUPTED")
        .unwrap_or("Connection Interrupted");
    let scale = 0.5;
    let f = l.fonts.pick(0, scale, sp);
    let px = scale / f.unit_scale();
    let w = font::measure(f, line, px);
    let h = (48.0 * scale) as i32 as f32;
    let (x, _) = sp.axis(0, ((640.0 - w) / 2.0).trunc(), w, 0);
    let (y, _) = sp.axis(1, 100.0 - h, h, 0);
    text(f, line, x, y, px * sp.virtual_to_real[1], [1.0; 4], out);
    if now & 0x200 == 0
        && let Some(tex) = image_of(l, "net_disconnect")
    {
        let (x, w) = sp.axis(0, 296.0, 48.0, 0);
        let (y, h) = sp.axis(1, 320.0, 48.0, 0);
        out.push(quad(x, y, w, h, [1.0; 4], tex, false));
    }
}

pub(crate) fn text(
    f: &Font,
    s: &str,
    x: f32,
    y: f32,
    scale: f32,
    rgba: [f32; 4],
    out: &mut Vec<HudQuad>,
) -> f32 {
    let from = out.len();
    let w = font::layout(f, s, x, y, scale, rgba, out);
    for q in &mut out[from..] {
        if q.rgba == [0.0, 0.0, 0.0, 0.8] {
            q.rgba[3] *= rgba[3];
        }
    }
    w
}

/// [`text`] with its glow (`R_AddCmdDrawTextWithEffects`): nothing more
/// when `glow`'s alpha is 0, else after the text each glyph again from
/// the font's glow material (`fonts/gamefonts_pc_glow`: the page,
/// `SrcAlpha One`) four times, 1.75 by 1.125 its size, centred on it and
/// pushed two glyph pixels to each corner, in `glow`'s colour times 0.1
/// at the glyph's own alpha (`SetDrawText2DGlowParms` `iw3mp.exe`
/// 0x5f6aa0, the pass at 0x6148d6..0x614aff; cod4-hud.md 22).
#[allow(clippy::too_many_arguments)]
pub(crate) fn glow_text(
    f: &Font,
    s: &str,
    x: f32,
    y: f32,
    scale: f32,
    rgba: [f32; 4],
    glow: [f32; 4],
    out: &mut Vec<HudQuad>,
) -> f32 {
    let from = out.len();
    let w = text(f, s, x, y, scale, rgba, out);
    if glow[3] == 0.0 {
        return w;
    }
    let px = f.glyph_scale * f.unit_scale() * scale;
    // `font::layout` pushes each glyph after its shadow.
    let glyphs: Vec<([f32; 4], [[f32; 2]; 4], f32)> = out[from..]
        .iter()
        .skip(1)
        .step_by(2)
        .map(|q| {
            let [x0, y0] = q.verts[0];
            let [x1, y1] = q.verts[2];
            ([x0, y0, x1 - x0, y1 - y0], q.uvs, q.rgba[3])
        })
        .collect();
    let page = format!("{}{}", crate::hud::ADDITIVE, f.page);
    for ([gx, gy, gw, gh], uvs, a) in glyphs {
        let (w, h) = (gw * 1.75, gh * 1.125);
        for (ox, oy) in GLOW_OFFSETS {
            let qx = gx - 0.375 * gw + ox * 2.0 * px;
            let qy = gy - 0.0625 * gh + oy * 2.0 * px;
            out.push(HudQuad {
                verts: [[qx, qy], [qx + w, qy], [qx + w, qy + h], [qx, qy + h]],
                uvs,
                rgba: [glow[0] * 0.1, glow[1] * 0.1, glow[2] * 0.1, a],
                texture: page.clone(),
            });
        }
    }
    w
}

/// The glow pass's four offsets in glyph pixels times 2 (`iw3mp.exe`
/// 0x700bec).
const GLOW_OFFSETS: [(f32, f32); 4] = [(-1.0, -1.0), (-1.0, 1.0), (1.0, -1.0), (1.0, 1.0)];

impl Cod4Hud {
    /// configstring 822, the north yaw in degrees (0 when unset).
    fn north_yaw(&self, s: &Cod4Session) -> f32 {
        self.configstring(s, h::CS_NORTH_YAW)
            .trim()
            .parse()
            .unwrap_or(0.0)
    }

    /// configstring 823, `compass_map_<map> x0 y0 x1 y1`: the minimap's
    /// material and where it lies (cod4-hud.md 6). A corner that does not
    /// parse reads 0; `None` while the string is empty.
    pub(super) fn minimap(&self, s: &Cod4Session) -> Option<(String, MapAxes)> {
        let cs = self.configstring(s, h::CS_MINIMAP);
        let mut words = h::tokenize(&cs).into_iter();
        let material = words.next()?;
        let mut n = [0.0f32; 4];
        for (slot, w) in n.iter_mut().zip(words) {
            *slot = w.parse().unwrap_or(0.0);
        }
        let map = MapAxes::new(self.north_yaw(s), [n[0], n[1]], [n[2], n[3]]);
        Some((material, map))
    }

    /// Each active or current objective of the newest snapshot (cod4-hud.md
    /// 10, 22): its icon (configstring 2002 + `icon`), where it is (its
    /// entity's place, else its own origin) and whether it is current.
    fn objective_marks(&self, s: &Cod4Session, snap: &Snapshot) -> Vec<ObjectiveMark> {
        let field = |o: &vcod_cod4::net::delta::Objective, name: &str| {
            vcod_cod4::net::fields::OBJECTIVE
                .iter()
                .find(|f| f.name == name)
                .map_or(0, |f| o.words[f.word()])
        };
        snap.ps
            .objectives
            .iter()
            .filter(|o| o.state == OBJST_ACTIVE || o.state == OBJST_CURRENT)
            .map(|o| {
                let ent = field(o, "entNum");
                let entity = (ent != ENTITYNUM_NONE)
                    .then(|| snap.entities.iter().find(|e| e.number() == ent))
                    .flatten();
                let at = entity.map_or_else(
                    || {
                        let axis = |k: usize| f32::from_bits(field(o, &format!("origin[{k}]")));
                        glam::Vec3::new(axis(0), axis(1), axis(2))
                    },
                    crate::play::cod4::origin_of,
                );
                ObjectiveMark {
                    material: self.configstring(s, h::CS_MATERIALS + field(o, "icon") as u16),
                    at,
                    current: o.state == OBJST_CURRENT,
                }
            })
            .collect()
    }

    /// `compass_old` (cod4-hud.md 6, 15.7): its background, the
    /// tickertape, the minimap turned with the view, the UAV's sweep over
    /// it, the pointers, friendlies, enemies, helicopters and planes, and
    /// last the player's arrow.
    fn draw_compass(
        &self,
        l: &Loaded,
        s: &Cod4Session,
        (eye, yaw): (glam::Vec3, f32),
        now: i32,
        sp: &ScreenPlacement,
        out: &mut Vec<HudQuad>,
    ) {
        let to_screen = |item: &h::MenuRect, [x, y, w, h]: [f32; 4]| {
            let (sx, sw) = sp.axis(0, x, w, item.horz_align);
            let (sy, sh) = sp.axis(1, y, h, item.vert_align);
            (sx, sy, sw, sh)
        };
        let backed = |name: &str| {
            let item = l.compass.get(name)?;
            let tex = item.background.as_deref().and_then(|m| image_of(l, m))?;
            Some((item, tex))
        };
        if let Some((bg, tex)) = backed("minimapBG") {
            let (x, y, w, h) = to_screen(bg, bg.rect);
            out.push(quad(x, y, w, h, [1.0; 4], tex, false));
        }
        self.draw_ticker(l, s, sp, (eye, yaw), out);
        let Some(map_item) = l.compass.get("mini_map") else {
            return;
        };
        let (mx, my, mw, mh) = to_screen(map_item, map_item.rect);
        let over_map = |uvs, texture| HudQuad {
            verts: [[mx, my], [mx + mw, my], [mx + mw, my + mh], [mx, my + mh]],
            uvs,
            rgba: [1.0; 4],
            texture,
        };
        let minimap = self.minimap(s);
        if let Some((material, map)) = &minimap
            && let Some(tex) = image_of(l, material)
        {
            out.push(over_map(minimap_uvs(map, [eye.x, eye.y], yaw), tex));
        }
        let uav = s
            .client()
            .snapshots
            .last()
            .is_some_and(|snap| snap.ps.get("radarEnabled").unwrap_or(0) != 0);
        if uav
            && let Some((_, map)) = &minimap
            && let Some(tex) = image_of(l, "compass_radarline")
        {
            let progress = self.radar.progress_at(now);
            out.push(over_map(sweep_uvs(map, progress, [eye.x, eye.y], yaw), tex));
        }
        if let Some(icons_item) = l.compass.get("compassfriendlies") {
            let crosshair = backed("compasspointers").map(|(_, tex)| tex);
            for icon in self.compass_icons(l, s, eye, yaw) {
                let Some(tex) = image_of(l, &icon.material) else {
                    continue;
                };
                let [cx, cy] = icon.centre;
                if icon.lines
                    && let Some(line) = &crosshair
                {
                    let [rx, ry, rw, rh] = icons_item.rect;
                    for r in [[rx, cy - 1.0, rw, 2.0], [cx - 1.0, ry, 2.0, rh]] {
                        let (x, y, w, h) = to_screen(icons_item, r);
                        out.push(quad(x, y, w, h, [1.0; 4], line.clone(), false));
                    }
                }
                let (x, y, _, _) = to_screen(icons_item, [cx, cy, 0.0, 0.0]);
                let (_, _, w, h) = to_screen(icons_item, [0.0, 0.0, icon.size, icon.size]);
                out.push(spun_quad([x, y], [w, h], -icon.turn, icon.alpha, tex));
            }
        }
        if let Some((arrow, tex)) = backed("compassplayer") {
            let [rx, ry, rw, rh] = arrow.rect;
            let at = [
                rx + 0.5 * (rw - PLAYER_ARROW),
                ry + 0.5 * (rh - PLAYER_ARROW),
                PLAYER_ARROW,
                PLAYER_ARROW,
            ];
            let (x, y, w, h) = to_screen(arrow, at);
            out.push(quad(x, y, w, h, [1.0; 4], tex, false));
        }
    }

    /// `compass_old`'s tickertape (cod4-hud.md 22; `iw3mp.exe` 0x428310):
    /// `minimapTickerBorder`, then the tape's material across
    /// `minimapTicker` from S `c - 0.25` to `c + 0.25`, `c` the map's turn
    /// over 360, and each active or current objective's icon along it at
    /// its bearing with its distance under it (0x428190).
    fn draw_ticker(
        &self,
        l: &Loaded,
        s: &Cod4Session,
        sp: &ScreenPlacement,
        (eye, yaw): (glam::Vec3, f32),
        out: &mut Vec<HudQuad>,
    ) {
        let to_screen = |item: &h::MenuRect, [x, y, w, h]: [f32; 4]| {
            let (sx, sw) = sp.axis(0, x, w, item.horz_align);
            let (sy, sh) = sp.axis(1, y, h, item.vert_align);
            (sx, sy, sw, sh)
        };
        let backed = |name: &str| {
            let item = l.compass.get(name)?;
            let tex = item.background.as_deref().and_then(|m| image_of(l, m))?;
            Some((item, tex))
        };
        if let Some((border, tex)) = backed("minimapTickerBorder") {
            let (x, y, w, h) = to_screen(border, border.rect);
            out.push(quad(x, y, w, h, [1.0; 4], tex, false));
        }
        let Some((tape, tex)) = backed("minimapTicker") else {
            return;
        };
        let (x, y, w, h) = to_screen(tape, tape.rect);
        let turn = self.north_yaw(s) - yaw;
        for (from, to, s0, s1) in ticker_spans(turn / 360.0) {
            let (qx, qw) = (x + from * w, (to - from) * w);
            out.push(HudQuad {
                verts: [[qx, y], [qx + qw, y], [qx + qw, y + h], [qx, y + h]],
                uvs: [[s0, 0.0], [s1, 0.0], [s1, 1.0], [s0, 1.0]],
                rgba: [1.0; 4],
                texture: tex.clone(),
            });
        }
        let Some(snap) = s.client().snapshots.last() else {
            return;
        };
        let font = l.fonts.pick(0, TICKER_TEXT_SCALE, sp);
        let px = TICKER_TEXT_SCALE / font.unit_scale();
        let text_height = (48.0 * TICKER_TEXT_SCALE) as i32 as f32;
        let [tx, ty, tw, th] = tape.rect;
        for mark in self.objective_marks(s, snap) {
            if mark.at == glam::Vec3::ZERO {
                continue;
            }
            let (along, metres) = ticker_mark([eye.x, eye.y], [mark.at.x, mark.at.y], yaw);
            let centre = [tx + tw * along, ty + th * 0.5];
            if let Some(icon) = image_of(l, &mark.material) {
                let size = OBJECTIVE_SIZE;
                let r = [centre[0] - size * 0.5, centre[1] - size * 0.5, size, size];
                let (qx, qy, qw, qh) = to_screen(tape, r);
                out.push(quad(qx, qy, qw, qh, [1.0; 4], icon, false));
            }
            let label = distance_text(metres);
            let width = font::measure(font, &label, px);
            let top = centre[1] + OBJECTIVE_SIZE + 2.0 - text_height;
            let (qx, qy, _, _) = to_screen(tape, [centre[0] - width * 0.5, top, 0.0, 0.0]);
            let scale = px * sp.virtual_to_real[1];
            text(font, &label, qx, qy, scale, TICKER_TEXT_COLOUR, out);
        }
    }

    /// The compass's icons on the `compassfriendlies` rect (cod4-hud.md 10,
    /// 15): each active or current objective's, each teammate's arrow
    /// turned by his yaw, each enemy the radar pinged, and the
    /// helicopters and planes. Friendlies need us on a team. An icon past
    /// the compass's edge sits on the edge (`compassClampIcons` 1).
    fn compass_icons(
        &self,
        l: &Loaded,
        s: &Cod4Session,
        eye: glam::Vec3,
        yaw: f32,
    ) -> Vec<CompassIcon> {
        let mut out = Vec::new();
        let (Some(r), Some(snap)) = (
            l.compass.get("compassfriendlies"),
            s.client().snapshots.last(),
        ) else {
            return out;
        };
        let rect = r.rect;
        let centre = [rect[0] + rect[2] * 0.5, rect[1] + rect[3] * 0.5];
        let (ys, yc) = yaw.to_radians().sin_cos();
        let at = |x: f32, y: f32| {
            let o = world_to_compass(rect, [yc, ys], eye, glam::Vec3::new(x, y, 0.0));
            [centre[0] + o[0], centre[1] + o[1]]
        };
        let icon =
            |material: String, centre: [f32; 2], size: f32, turn: f32, alpha: f32| CompassIcon {
                material,
                centre,
                size,
                turn,
                lines: false,
                alpha,
            };
        let me = snap.ps.get("clientNum").unwrap_or(0);
        let team_of = |n: u32| {
            snap.clients
                .iter()
                .find(|c| c.0[0] == n)
                .map_or(0, |c| client_word(c, "team"))
        };
        let my_team = team_of(me);
        for mark in self.objective_marks(s, snap) {
            out.push(CompassIcon {
                lines: mark.current,
                ..icon(
                    mark.material,
                    at(mark.at.x, mark.at.y),
                    OBJECTIVE_SIZE,
                    0.0,
                    1.0,
                )
            });
        }
        // The radar's teammates (0x429880). Under a shot's or a talker's
        // ping at full alpha the arrow itself is not drawn.
        if matches!(my_team, 1 | 2) {
            for f in self.radar.friendlies(self.radar.time(), me as usize) {
                let spot = at(f.pos[0], f.pos[1]);
                if f.chat_icon {
                    out.push(icon(
                        "objective_friendly_chat".into(),
                        spot,
                        FRIENDLY_SIZE,
                        0.0,
                        1.0,
                    ));
                    continue;
                }
                let arrow = (f.firing != Some(1.0) && f.voice != Some(1.0))
                    .then_some(("compassping_friendly_mp", 1.0));
                let voice = f.voice.map(|a| ("compassping_friendlyyelling_mp", a));
                let shot = f.firing.map(|a| ("compassping_friendlyfiring_mp", a));
                for (material, alpha) in arrow.into_iter().chain(voice).chain(shot) {
                    out.push(icon(
                        material.into(),
                        spot,
                        FRIENDLY_SIZE,
                        f.yaw - yaw,
                        alpha,
                    ));
                }
            }
        }
        // The radar's enemy pings, upright; none for a spectator.
        if my_team != 3 {
            let show_all = self
                .server_dvar(s, "g_compassShowEnemies")
                .is_some_and(|v| vcod_cod4::menuexp::atoi(&v) != 0);
            for p in self.radar.pings(self.radar.time(), show_all) {
                let material = match p.firing {
                    true => "compassping_enemyfiring",
                    false => "compassping_enemy",
                };
                out.push(icon(
                    material.into(),
                    at(p.pos[0], p.pos[1]),
                    FRIENDLY_SIZE,
                    0.0,
                    p.alpha,
                ));
            }
        }
        // Helicopters and planes (owner draws 155 and 156, cod4-hud.md
        // 15.7): `_busy` for another team's (in free for all another
        // owner's), `_friendly` for ours, turned by their yaw.
        let own = s.gamestate().map_or(me, |g| g.client_num as u32);
        let own_team = team_of(own);
        for (etype, kind, size) in [
            (ET_HELICOPTER, "helicopter", HELICOPTER_SIZE),
            (ET_PLANE, "airstrike", PLANE_SIZE),
        ] {
            for v in self.vehicles.drawn(self.radar.time(), etype) {
                let theirs = match own_team {
                    0 => own != v.owner,
                    team => team != v.team,
                };
                let side = if theirs { "busy" } else { "friendly" };
                let material = format!("compass_objpoint_{kind}_{side}");
                out.push(icon(
                    material,
                    at(v.pos[0], v.pos[1]),
                    size,
                    v.yaw - yaw,
                    1.0,
                ));
            }
        }
        out
    }
}

/// `compassPlayerWidth` / `Height` 18.75 (cod4-hud.md 6).
const PLAYER_ARROW: f32 = 18.75;
/// `compassFriendlyWidth` / `Height` 18.75 (0x6c6884) times `compassSize`
/// 1, `compassObjectiveWidth` / `Height` 20 (0x6df498); cod4-hud.md 10.
const FRIENDLY_SIZE: f32 = 18.75;
const OBJECTIVE_SIZE: f32 = 20.0;
/// A helicopter's icon is 40 square and a plane's 20, times `compassSize`
/// (cod4-hud.md 15.7).
const HELICOPTER_SIZE: f32 = 40.0;
const PLANE_SIZE: f32 = 20.0;
/// Objective states 1 (active) and 4 (current).
const OBJST_ACTIVE: u32 = 1;
const OBJST_CURRENT: u32 = 4;
/// An objective with this entity sits at its own origin.
const ENTITYNUM_NONE: u32 = 1023;

/// The `minimapTicker` item's text scale (`menu_dump`), which its
/// distances draw at, and their yellow-green (cod4-hud.md 22).
const TICKER_TEXT_SCALE: f32 = 0.55;
const TICKER_TEXT_COLOUR: [f32; 4] = [0.9, 1.0, 0.1, 1.0];
/// `compassTickertapeStretch` 0.5 (0x42588b): the share of the tape's
/// texture shown at once.
const TAPE_STRETCH: f32 = 0.5;
/// `compassObjectiveDetailDist` 10 m: nearer, a distance shows tenths.
const DETAIL_METRES: f32 = 10.0;
/// Inches to metres (the double at 0x70b048).
const METRES_PER_UNIT: f32 = 0.0254;

/// An objective the compass and the full map draw.
pub(super) struct ObjectiveMark {
    pub material: String,
    pub at: glam::Vec3,
    pub current: bool,
}

/// One icon on the compass, on the grid: its centre, size and turn.
#[derive(Clone, Debug, PartialEq)]
struct CompassIcon {
    material: String,
    centre: [f32; 2],
    size: f32,
    /// Degrees, counterclockwise on screen.
    turn: f32,
    /// A current objective's crosshair through it.
    lines: bool,
    alpha: f32,
}

/// A `size` quad centred on `centre`, turned `degrees` clockwise on
/// screen.
fn spun_quad(
    centre: [f32; 2],
    size: [f32; 2],
    degrees: f32,
    alpha: f32,
    texture: String,
) -> HudQuad {
    let (sin, cos) = degrees.to_radians().sin_cos();
    let corner = |u: f32, v: f32| {
        let (dx, dy) = (u * 0.5 * size[0], v * 0.5 * size[1]);
        [
            centre[0] + dx * cos - dy * sin,
            centre[1] + dx * sin + dy * cos,
        ]
    };
    HudQuad {
        verts: [
            corner(-1.0, -1.0),
            corner(1.0, -1.0),
            corner(1.0, 1.0),
            corner(-1.0, 1.0),
        ],
        uvs: [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]],
        rgba: [1.0, 1.0, 1.0, alpha],
        texture,
    }
}

/// The texture coordinates at a screen-aligned square's corners (upper
/// left, upper right, lower right, lower left) for a window of the
/// texture centred on `centre`, `half` its half size, turned `degrees`.
fn window_uvs(centre: [f32; 2], half: [f32; 2], degrees: f32) -> [[f32; 2]; 4] {
    let (sin, cos) = degrees.to_radians().sin_cos();
    [(-1.0, -1.0), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)].map(|(a, b): (f32, f32)| {
        let (u, v) = (a * cos - b * sin, a * sin + b * cos);
        [centre[0] + u * half[0], centre[1] + v * half[1]]
    })
}

/// The minimap under the compass (cod4-hud.md 6): centred on the view's
/// place on the map, `compassMaxRange` across, turned by `northYaw -
/// viewYaw` so the view points up.
fn minimap_uvs(map: &MapAxes, eye: [f32; 2], yaw: f32) -> [[f32; 2]; 4] {
    let half = map.extent.map(|e| 0.5 * COMPASS_RANGE / e);
    window_uvs(map.fraction(eye), half, map.north_yaw - yaw)
}

/// The UAV's sweep over the compass (cod4-hud.md 15.7): `compass_radarline`
/// placed by the view's distance from the line, turned as the map is.
fn sweep_uvs(map: &MapAxes, progress: f32, eye: [f32; 2], yaw: f32) -> [[f32; 2]; 4] {
    let s = crate::cod4_radar::sweep_texture(map, progress, eye);
    window_uvs([s, 0.0], [SWEEP_HALF; 2], map.north_yaw - yaw)
}

/// Where an objective sits along the tape, 0 at the left end to 1 at the
/// right, and how far it is in metres (cod4-hud.md 22). The tape's middle
/// is the view's yaw; a bearing 90 degrees either side reaches an end.
fn ticker_mark(eye: [f32; 2], goal: [f32; 2], yaw: f32) -> (f32, f32) {
    let to = [goal[0] - eye[0], goal[1] - eye[1]];
    let metres = to[0].hypot(to[1]) * METRES_PER_UNIT;
    let leftward = angle_normalize_180(to[1].atan2(to[0]).to_degrees() - yaw);
    ((0.5 - leftward / 180.0).clamp(0.0, 1.0), metres)
}

/// A distance under a tape icon: whole metres from 10 out, tenths
/// (truncated) inside.
fn distance_text(metres: f32) -> String {
    if metres >= DETAIL_METRES {
        format!("{}m", metres as i32)
    } else {
        let tenths = (metres * 10.0) as i32;
        format!("{}.{}m", tenths / 10, tenths % 10)
    }
}

/// The tape's visible S range, `TAPE_STRETCH` wide around `centre`, as
/// `(x0, x1, s0, s1)` spans of the rect (fractions of its width): one,
/// or two where the range crosses a whole number, so the image never wraps
/// inside a quad.
fn ticker_spans(centre: f32) -> Vec<(f32, f32, f32, f32)> {
    let start = centre - TAPE_STRETCH * 0.5;
    let start = start - start.floor();
    let end = start + TAPE_STRETCH;
    if end <= 1.0 {
        return vec![(0.0, 1.0, start, end)];
    }
    let split = (1.0 - start) / TAPE_STRETCH;
    vec![(0.0, split, start, 1.0), (split, 1.0, 0.0, end - 1.0)]
}

/// An angle in -180..180.
fn angle_normalize_180(a: f32) -> f32 {
    let a = a.rem_euclid(360.0);
    if a > 180.0 { a - 360.0 } else { a }
}

/// A world point on the compass rect, off its centre (cod4-hud.md 10):
/// the offset from the eye at the rect's height over `compassMaxRange` an
/// inch, the view's yaw vector `up` pointing up the screen. A point past
/// an edge is pulled in along its line from the centre until it sits on
/// the rect (`compassClampIcons`).
fn world_to_compass(rect: [f32; 4], up: [f32; 2], eye: glam::Vec3, at: glam::Vec3) -> [f32; 2] {
    let scale = rect[3] / COMPASS_RANGE;
    let d = [(at.x - eye.x) * scale, (at.y - eye.y) * scale];
    let right = [up[1], -up[0]];
    let screen = [
        d[0] * right[0] + d[1] * right[1],
        -(d[0] * up[0] + d[1] * up[1]),
    ];
    let fit = (0..2)
        .map(|k| {
            let half = rect[2 + k] * 0.5;
            if screen[k].abs() > half {
                half / screen[k].abs()
            } else {
                1.0
            }
        })
        .fold(1.0, f32::min);
    screen.map(|v| v * fit)
}

/// `materialIndex`'s or a kill icon's image as a HUD texture path.
/// A material's colour map as the HUD's texture name; `white`, whose
/// image is the engine's `$white`, as the HUD's own white.
fn image_of(l: &Loaded, material: &str) -> Option<String> {
    if material.eq_ignore_ascii_case("white") {
        return Some("white".into());
    }
    let img = l.images.get(&material.to_ascii_lowercase())?;
    Some(format!("images/{img}.iwi"))
}

pub(crate) fn quad(
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    rgba: [f32; 4],
    texture: String,
    flip: bool,
) -> HudQuad {
    let (u0, u1) = if flip { (1.0, 0.0) } else { (0.0, 1.0) };
    HudQuad {
        verts: [[x, y], [x + w, y], [x + w, y + h], [x, y + h]],
        uvs: [[u0, 0.0], [u1, 0.0], [u1, 1.0], [u0, 1.0]],
        rgba,
        texture,
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
fn colour(to: [u8; 4], from: [u8; 4], start: i32, time: i32, now: i32) -> [f32; 4] {
    let unit = |c: [u8; 4]| c.map(|b| f32::from(b) / 255.0);
    tweened(unit(from), unit(to), tween(start, time, now))
}

/// A material's or a waypoint's drawn width and height: `fromWidth`,
/// `fromHeight` to `width`, `height` over `scaleTime` from
/// `scaleStartTime` (`scaleOverTime`, the stock progress bar's fill), in
/// virtual units. INFERRED (cod4-hud.md 5).
fn material_size(e: &HudElem, now: i32) -> (f32, f32) {
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

/// `%g` of a value: six significant digits, no trailing zeros.
fn format_g(v: f32) -> String {
    if v.fract() == 0.0 && v.abs() < 1e6 {
        return format!("{}", v as i64);
    }
    let s = format!("{v:.6}");
    s.trim_end_matches('0').trim_end_matches('.').to_string()
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
pub(crate) mod tests {
    //! The HUD half of the CoD4 replay gate: the committed captures through
    //! the session's steps and [`Cod4Hud::update`] at 60 frames a second,
    //! as `cod4_client` runs them; with `COD4_DIR` the HUD is built too.

    use super::*;

    fn close<const N: usize>(got: [f32; N], want: [f32; N]) {
        let off = got.iter().zip(&want).any(|(g, w)| (g - w).abs() > 1e-3);
        assert!(!off, "{got:?} != {want:?}");
    }

    /// The minimap's texture window: centred on the view's place on the
    /// map, `compassMaxRange` across, turned by `northYaw - viewYaw`; and
    /// the sweep's line over it (cod4-hud.md 6, 15.7).
    #[test]
    fn the_minimap_and_the_sweep_turn_with_the_view() {
        let cases = [
            (
                0.0,
                [-2000.0, 3000.0],
                [3000.0, -1000.0],
                [500.0, 200.0],
                30.0,
                [
                    0.27311707, 0.59150636, 0.8143829, 0.84150636, 1.1268829, 0.40849364,
                    0.58561707, 0.15849365,
                ],
                [
                    [
                        3.8602352, -0.4575317, 6.025298, -1.7075317, 7.275298, 0.4575317, 5.110235,
                        1.7075317,
                    ],
                    [
                        1.2995749, -0.4575317, 3.4646382, -1.7075317, 4.714638, 0.4575317,
                        2.5495749, 1.7075317,
                    ],
                    [
                        -2.541415, -0.4575317, -0.3763516, -1.7075317, 0.8736484, 0.4575317,
                        -1.291415, 1.7075317,
                    ],
                ],
            ),
            (
                90.0,
                [-1000.0, 1500.0],
                [2500.0, -1000.0],
                [-300.0, 100.0],
                200.0,
                [
                    -0.013454467,
                    1.2008562,
                    -0.25775456,
                    0.2611637,
                    0.41345447,
                    -0.08085638,
                    0.65775454,
                    0.8588362,
                ],
                [
                    [
                        2.7206764, 1.6021409, 1.865626, -0.7470906, 4.2148576, -1.6021409,
                        5.069908, 0.7470906,
                    ],
                    [
                        0.31001616,
                        1.6021409,
                        -0.54503417,
                        -0.7470906,
                        1.8041973,
                        -1.6021409,
                        2.6592476,
                        0.7470906,
                    ],
                    [
                        -3.305974, 1.6021409, -4.161024, -0.7470906, -1.8117925, -1.6021409,
                        -0.9567424, 0.7470906,
                    ],
                ],
            ),
            (
                0.0,
                [0.0, 0.0],
                [0.0, 0.0],
                [100.0, 100.0],
                0.0,
                [-1.35, -1.35, 1.15, -1.35, 1.15, 1.15, -1.35, 1.15],
                [
                    [
                        1.417767, -1.25, 3.917767, -1.25, 3.917767, 1.25, 1.417767, 1.25,
                    ],
                    [
                        -0.24289322,
                        -1.25,
                        2.2571068,
                        -1.25,
                        2.2571068,
                        1.25,
                        -0.24289322,
                        1.25,
                    ],
                    [
                        -2.7338834,
                        -1.25,
                        -0.23388326,
                        -1.25,
                        -0.23388326,
                        1.25,
                        -2.7338834,
                        1.25,
                    ],
                ],
            ),
            (
                -45.0,
                [-3000.0, 0.0],
                [0.0, -2000.0],
                [-2500.0, -100.0],
                -120.0,
                [
                    -0.8499998,
                    0.5530127,
                    -1.7650634,
                    -0.12999997,
                    1.6499997,
                    -0.3130127,
                    2.5650635,
                    0.36999997,
                ],
                [
                    [
                        3.3688078,
                        -1.5309311,
                        4.0158553,
                        0.88388336,
                        1.6010408,
                        1.5309311,
                        0.9539933,
                        -0.88388336,
                    ],
                    [
                        2.2202797,
                        -1.5309311,
                        2.8673272,
                        0.88388336,
                        0.45251286,
                        1.5309311,
                        -0.1945349,
                        -0.88388336,
                    ],
                    [
                        0.49748755,
                        -1.5309311,
                        1.1445352,
                        0.88388336,
                        -1.2702793,
                        1.5309311,
                        -1.9173269,
                        -0.88388336,
                    ],
                ],
            ),
        ];
        let flat = |q: [[f32; 2]; 4]| -> [f32; 8] { std::array::from_fn(|i| q[i / 2][i % 2]) };
        for (north, ul, lr, eye, yaw, map, sweeps) in cases {
            let m = MapAxes::new(north, ul, lr);
            close(flat(minimap_uvs(&m, eye, yaw)), map);
            for (p, want) in [0.0, 0.3, 0.75].into_iter().zip(sweeps) {
                close(flat(sweep_uvs(&m, p, eye, yaw)), want);
            }
        }
    }

    /// An objective on the tape: its bearing against the view's, 90
    /// degrees either side at the ends, and its distance in metres, whole
    /// from 10 out and cut to tenths inside (cod4-hud.md 22).
    #[test]
    fn the_ticker_places_an_objective_by_its_bearing() {
        let cases = [
            ([0.0, 0.0], [1000.0, 0.0], 0.0, 0.0, [0.5, 25.4]),
            (
                [0.0, 0.0],
                [1000.0, 300.0],
                0.0,
                0.0,
                [0.40722644, 26.518377],
            ),
            ([0.0, 0.0], [0.0, -1000.0], 0.0, 0.0, [1.0, 25.4]),
            (
                [100.0, 50.0],
                [-300.0, 400.0],
                30.0,
                170.0,
                [0.6732551, 13.500284],
            ),
            (
                [100.0, 50.0],
                [-300.0, -400.0],
                30.0,
                170.0,
                [0.17574191, 15.292825],
            ),
            ([0.0, 0.0], [-100.0, 1.0], 0.0, 0.0, [0.0, 2.5401268]),
            (
                [0.0, 0.0],
                [300.0, 200.0],
                10.0,
                350.0,
                [0.25727743, 9.1581],
            ),
        ];
        for (eye, goal, north, yaw, want) in cases {
            let _ = north;
            let (percent, metres) = ticker_mark(eye, goal, yaw);
            close([percent, metres], want);
        }
        let texts = [12.7, 10.0, 9.97, 0.05, 3.333, 0.0].map(distance_text);
        assert_eq!(texts, ["12m", "10m", "9.9m", "0.0m", "3.3m", "0.0m"]);
        let spans = [
            (0.1, vec![(0.0, 0.3, 0.85, 1.0), (0.3, 1.0, 0.0, 0.35)]),
            (0.5, vec![(0.0, 1.0, 0.25, 0.75)]),
            (-0.3, vec![(0.0, 1.0, 0.45, 0.95)]),
            (0.9, vec![(0.0, 0.7, 0.65, 1.0), (0.7, 1.0, 0.0, 0.15)]),
            (0.25, vec![(0.0, 1.0, 0.0, 0.5)]),
            (-1.0, vec![(0.0, 0.5, 0.75, 1.0), (0.5, 1.0, 0.0, 0.25)]),
        ];
        for (centre, want) in spans {
            let got = ticker_spans(centre);
            assert_eq!(got.len(), want.len(), "{centre}: {got:?}");
            for (g, w) in got.into_iter().zip(want) {
                close([g.0, g.1, g.2, g.3], [w.0, w.1, w.2, w.3]);
            }
        }
    }

    /// A point on the compass: off the eye at the rect's height over
    /// `compassMaxRange` an inch, the view's yaw up, and one past the
    /// edge pulled back onto it (cod4-hud.md 10).
    #[test]
    fn world_points_land_on_the_compass() {
        let cases = [
            (
                30.0f32,
                [0.0, 0.0, 0.0],
                [500.0, 200.0, 50.0],
                [3.1332335, -21.74692],
            ),
            (0.0, [100.0, 100.0, 0.0], [100.0, 1100.0, 0.0], [-40.8, 0.0]),
            (
                90.0,
                [0.0, 0.0, 0.0],
                [5000.0, 100.0, 0.0],
                [51.0, -1.0199977],
            ),
            (
                -150.0,
                [10.0, -20.0, 0.0],
                [-3000.0, 4000.0, 0.0],
                [51.0, -6.103284],
            ),
        ];
        for (yaw, eye, at, want) in cases {
            let (s, c) = yaw.to_radians().sin_cos();
            let got = world_to_compass(
                [6.0, 18.0, 102.0, 102.0],
                [c, s],
                glam::Vec3::from(eye),
                glam::Vec3::from(at),
            );
            close(got, want);
        }
    }

    const FIXTURES: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../cod4/tests/fixtures/net");

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

    /// A map change reloads the zones: the next level's minimap image is
    /// in, where the stale load had only the last map's, and the selector's
    /// `@PLATFORM_PRESS_TO_SET_AIRSTRIKE` reads as its text on both. The
    /// selector seen in play after a rotation to mp_backlot drew no map under a
    /// raw key (C4-70).
    #[test]
    fn a_new_map_reloads_the_zones_and_the_selector_reads_localized() {
        let Some(install) = vcod_cod4::testing::cod4_install() else {
            return;
        };
        let main = install.root().join("main");
        let shell = crate::console::shell::Shell::new();
        let mut hud = Cod4Hud::default();
        assert!(hud.load_under(&main, "mp_crash", None));
        assert!(hud.texture_of("compass_map_mp_crash").is_some());
        assert!(!hud.load_under(&main, "mp_crash", None), "same map: kept");
        assert!(hud.load_under(&main, "mp_backlot", None));
        assert!(hud.texture_of("compass_map_mp_backlot").is_some());
        let texts = hud.fullscreen_texts(&shell);
        println!("{texts:?}");
        assert!(!texts.is_empty());
        assert!(
            texts
                .iter()
                .all(|t| !t.contains('@') && !t.contains("PLATFORM_")),
            "{texts:?}"
        );
    }

    /// The capture replayed; `build` gets every built frame's quads.
    fn replay(
        name: &str,
        build: impl FnMut(&Cod4Hud, &mut Cod4Session, i32),
    ) -> (Cod4Hud, Cod4Session) {
        replay_in(name, false, build)
    }

    /// [`replay`], with mp_crash's world loaded when `world` is set (the
    /// level's weapon defs, which the alt mode's action slot reads).
    fn replay_in(
        name: &str,
        world: bool,
        mut build: impl FnMut(&Cod4Hud, &mut Cod4Session, i32),
    ) -> (Cod4Hud, Cod4Session) {
        let text = std::fs::read_to_string(format!("{FIXTURES}/{name}")).unwrap();
        let mut s = Cod4Session::open_capture(&text).unwrap();
        if let (true, Some(install)) = (world, vcod_cod4::testing::cod4_install()) {
            let w = crate::cod4_fly::load_world(install.root(), "mp_crash").unwrap();
            s.set_world(std::sync::Arc::new(w));
        }
        let mut hud = Cod4Hud::default();
        if let Some(install) = vcod_cod4::testing::cod4_install() {
            hud.load(&install.root().join("main"), "mp_crash");
            assert!(matches!(hud.loaded, Some(Some(_))), "the HUD's zones load");
        }
        let mut ms = 0.0;
        while !s.finished() {
            ms += 1000.0 / 60.0;
            s.step(ms).unwrap();
            let fired = s.take_fired();
            let Some(scene) = s.scene() else { continue };
            hud.update(&mut s, &fired, scene.server_time);
            build(&hud, &mut s, scene.render_time);
        }
        (hud, s)
    }

    /// The kill capture's shooter empties his M16 to 9 of 30: from that
    /// snapshot on the clip graphic's rounds pulse toward red
    /// (`low_clip_flash`) until the reload fills it, and never before; the
    /// reserve turns red once the reload leaves 30 of 180 (a fifth or
    /// under), and only then. cod4-hud.md 15.
    #[test]
    fn kill_capture_pulses_the_shooters_low_clip() {
        if vcod_cod4::testing::cod4_install().is_none() {
            return;
        }
        let (mut low, mut pulsed, mut white_low, mut red_high) = (0, 0, 0, 0);
        let (mut stock_red, mut stock_wrong) = (0, 0);
        replay("play-1.7-kill-shooter.txt", |hud, s, now| {
            let (Some(Some(l)), Some(snap)) = (&hud.loaded, s.client().snapshots.last()) else {
                return;
            };
            let held = held_ammo(l, s, snap);
            let is_low = held.is_some_and(|w| w.low_clip());
            let reserve_low = held.is_some_and(|w| w.low_reserve());
            for d in hud.weapon_info(l, s, snap, now) {
                match d {
                    OwnerDraw::Clip { rgba, item, .. } => {
                        let red = rgba[1] < item.fore_color[1] - 0.01;
                        match (is_low, red) {
                            (true, true) => pulsed += 1,
                            (true, false) => white_low += 1,
                            (false, true) => red_high += 1,
                            _ => {}
                        }
                        low += usize::from(is_low);
                    }
                    OwnerDraw::Text { item, rgba, .. } if item.owner_draw == 119 => {
                        let red = rgba != item.fore_color;
                        stock_red += usize::from(red);
                        stock_wrong += usize::from(red != reserve_low);
                    }
                    _ => {}
                }
            }
        });
        println!("low {low} pulsed {pulsed} white {white_low} red_high {red_high}");
        assert!(low > 150, "{low}");
        assert_eq!(red_high, 0);
        // The pulse is a sine: it reads red most of a low span, white at
        // its troughs.
        assert!(pulsed > low / 2 && white_low > 0, "{pulsed} {white_low}");
        assert!(stock_red > 0);
        assert_eq!(stock_wrong, 0);
    }

    /// The walk capture's spawn gives slot 1 night vision and slot 3 the
    /// M16's grenade launcher (`actionSlotType` 3 and 2): the `dpad` menu
    /// draws the goggles' icon, the launcher's `dpadIcon` with its rounds
    /// and the `[key]` under each, and nothing on slots 2 and 4.
    #[test]
    fn walk_capture_draws_the_action_slots() {
        if vcod_cod4::testing::cod4_install().is_none() {
            return;
        }
        let mut seen: Vec<OwnerDraw> = Vec::new();
        let mut shell = Shell::new();
        shell.use_cod4_binds();
        replay_in("play-1.7-walk.txt", true, |hud, s, now| {
            let (Some(Some(l)), Some(snap)) = (&hud.loaded, s.client().snapshots.last()) else {
                return;
            };
            let env = hud.env(s, now, &shell);
            if hud.menu_visible(l, DPAD_MENU, &env) == Some(true) {
                let d = hud.dpad(l, s, snap, &env);
                if !d.is_empty() {
                    seen = d;
                }
            }
        });
        let pics: Vec<&str> = seen
            .iter()
            .filter_map(|d| match d {
                OwnerDraw::Pic { material, .. } => Some(material.as_str()),
                _ => None,
            })
            .collect();
        let texts: Vec<&str> = seen
            .iter()
            .filter_map(|d| match d {
                OwnerDraw::Text { text, .. } => Some(text.as_str()),
                _ => None,
            })
            .collect();
        println!("{pics:?} {texts:?}");
        assert_eq!(pics.len(), 2, "{pics:?}");
        assert_eq!(pics[0], NVG_ICON);
        // `BG_WeaponAmmo`: the launcher's clip and its reserve, one each.
        assert!(texts.contains(&"  2"), "{texts:?}");
        assert!(
            texts.contains(&"[N]") && texts.contains(&"[5]"),
            "{texts:?}"
        );
    }

    /// A vote of this server's (configstring 13's second number is
    /// `sv_serverid`) draws its two lines until its end; one naming
    /// another server id draws nothing.
    #[test]
    fn cod4_vote_draws_while_it_runs() {
        if vcod_cod4::testing::cod4_install().is_none() {
            return;
        }
        let mut last = 0;
        let (mut hud, s) = replay("play-1.7-kill-target.txt", |_, _, now| last = now);
        let shell = Shell::new();
        let size = (1280.0, 960.0);
        let without = hud.build(&s, size, last, None, &shell).len();
        let id = crate::play::cod4::info_value(hud.configstring(&s, 1).as_bytes(), "sv_serverid")
            .unwrap();
        hud.cs.insert(13, format!("{} {id}", last + 20_000));
        hud.cs.insert(14, "GAME_VOTE_MAPRESTART".into());
        hud.cs.insert(15, "1".into());
        hud.cs.insert(16, "0".into());
        let with = hud.build(&s, size, last, None, &shell).len();
        assert!(with > without + 20, "{without} quads, {with} with the vote");
        hud.cs
            .insert(13, format!("{} {}", last + 20_000, h::atoi(&id) + 1));
        assert_eq!(hud.build(&s, size, last, None, &shell).len(), without);
    }

    /// Retail's prematch sends the leader dialog as `s <index>` right
    /// after the `d` naming its alias: each plays as the alias of its
    /// slot when it came.
    #[test]
    fn prematch_leader_dialog_plays_as_local_sounds() {
        let (mut hud, _) = replay("play-1.7-prematch.txt", |_, _, _| {});
        let sounds: Vec<(String, bool)> = hud.take_local_sounds();
        let names: Vec<&str> = sounds.iter().map(|(a, _)| a.as_str()).collect();
        assert_eq!(
            names,
            ["mp_spawn_usa", "US_1mc_team_deathmtch", "US_1mc_boost"]
        );
        assert!(sounds.iter().all(|s| s.1));
    }

    /// `+scores` before any `b` still draws the board's backdrop, as a
    /// live server that has not answered `score` leaves it; the bars hide
    /// under the board's menu (`uiActive()`).
    #[test]
    fn cod4_scores_held_with_no_list_draws_the_board() {
        if vcod_cod4::testing::cod4_install().is_none() {
            return;
        }
        let mut last = 0;
        let (mut hud, s) = replay("play-1.7-kill-target.txt", |_, _, now| last = now);
        assert!(hud.scores.is_none(), "the capture carries no `b`");
        let shell = Shell::new();
        let size = (1280.0, 960.0);
        let bars = hud.bar_quads(&s, &shell, last);
        let without = hud.build(&s, size, last, None, &shell).len() - bars;
        hud.set_scores(true, last);
        let with = hud.build(&s, size, last, None, &shell).len();
        assert_eq!(
            hud.bar_quads(&s, &shell, last),
            0,
            "the bars under the board"
        );
        assert!(with > without + 4, "{without} quads, {with} with the board");
    }

    /// CoD4's iwds hold no `.dat` font: the console draws in the zone's
    /// `consoleFont`, its typed line among the glyphs.
    #[test]
    fn cod4_console_draws_its_typed_line() {
        use winit::keyboard::KeyCode;
        let Some(install) = vcod_cod4::testing::cod4_install() else {
            return;
        };
        let font = console_font(&install.root().join("main")).expect("consoleFont");
        let mut c = crate::console::Console::with_font(Some(font));
        let glyphs = |c: &crate::console::Console| {
            c.build(640.0, 480.0, 512)
                .iter()
                .filter(|q| q.texture == "images/gamefonts_pc.iwi")
                .count()
        };
        c.toggle();
        c.update(1000.0, 3.0, false);
        let empty = glyphs(&c);
        assert!(empty > 0, "the prompt and the version draw");
        for (k, t) in [(KeyCode::KeyH, "h"), (KeyCode::KeyI, "i")] {
            assert_eq!(c.key(k, Some(t), |_| Vec::new()), None);
        }
        assert_eq!(glyphs(&c), empty + 2);
        let line = c.key(KeyCode::Enter, None, |_| Vec::new());
        assert_eq!(line.as_deref(), Some("hi"));
    }

    #[test]
    fn kill_capture_prints_one_kill_line_and_the_victims_centre_print() {
        let mut most = 0;
        let mut textures = std::collections::BTreeSet::new();
        let (hud, s) = replay("play-1.7-kill-target.txt", |hud, s, now| {
            let q = hud.build(
                s,
                (1280.0, 960.0),
                now,
                Some((glam::Vec3::ZERO, 90.0)),
                &Shell::new(),
            );
            most = most.max(q.len());
            textures.extend(q.into_iter().map(|q| q.texture));
        });
        // `^8` and `^9`'s colours off the configstring mirror (cod4-hud.md 3).
        assert_eq!(
            hud.server_dvar(&s, "g_TeamColor_MyTeam").as_deref(),
            Some("0.6 0.8 0.6 0")
        );
        assert_eq!(
            hud.server_dvar(&s, "g_TeamColor_EnemyTeam").as_deref(),
            Some("1 0.45098 0.501961 0")
        );
        let c = &hud.counts;
        // The killcam replays the obituary; only the live one prints
        // (`ps.deltaTime`, cod4-hud.md 3).
        assert_eq!(c.kill_lines, 1, "{c:?}");
        assert_eq!(c.center_prints, 1, "{c:?}");
        assert_eq!((c.chat_lines, c.game_messages, c.bold_messages), (0, 0, 0));
        assert_eq!(c.scores, 0, "the capture carries no `b`");
        assert_eq!(c.config_strings, 10, "{c:?}");
        assert!(c.dvars > 50, "{c:?}");
        let line = &hud.windows[0][0].pieces;
        assert_eq!(line[0], Piece::Text("^9shooter^7 ".into()));
        assert!(matches!(&line[1], Piece::Icon { .. }), "{line:?}");
        assert_eq!(line[2], Piece::Text(" ^8target".into()));
        if hud.loaded.is_some() {
            let (center, _) = hud.center.as_ref().unwrap();
            assert_eq!(center, "Killed by shooter");
            assert!(
                matches!(&line[1], Piece::Icon { material, flip: true, .. } if material == "hud_icon_m16a4_grenade")
            );
            println!("most quads {most}, textures {textures:?}");
            assert!(textures.contains("images/gamefonts_pc.iwi"), "{textures:?}");
            assert!(textures.iter().any(|t| t.contains("m16a4")), "{textures:?}");
            assert!(most > 100, "{most} quads");
            assert!(
                textures.contains("images/compass_map_mp_crash.iwi"),
                "{textures:?}"
            );
        }
    }

    /// The score capture's target holds `+scores` from the first frame:
    /// its `b` replies fill the board with both players, the shooter's
    /// kill and the target's death in their rows (cod4-hud.md 7).
    #[test]
    fn score_capture_draws_both_players_on_the_board() {
        let mut held = false;
        let mut board: Vec<crate::cod4_scoreboard::Piece> = Vec::new();
        let mut quads = 0;
        let text =
            std::fs::read_to_string(format!("{FIXTURES}/play-1.7-score-target.txt")).unwrap();
        let mut s = Cod4Session::open_capture(&text).unwrap();
        let mut hud = Cod4Hud::default();
        if let Some(install) = vcod_cod4::testing::cod4_install() {
            hud.load(&install.root().join("main"), "mp_crash");
        }
        let mut ms = 0.0;
        while !s.finished() {
            ms += 1000.0 / 60.0;
            s.step(ms).unwrap();
            let fired = s.take_fired();
            let Some(scene) = s.scene() else { continue };
            if !held {
                held = true;
                hud.set_scores(true, scene.render_time);
            }
            hud.update(&mut s, &fired, scene.server_time);
            if let (Some(Some(l)), Some(snap)) = (&hud.loaded, s.client().snapshots.last()) {
                let sp = ScreenPlacement::new(1280.0, 960.0);
                board = hud.scoreboard(l, &s, snap, &sp, 1.0, &Shell::new());
                quads = hud
                    .build(&s, (1280.0, 960.0), scene.render_time, None, &Shell::new())
                    .len();
            }
        }
        // `b 2 10 0 750 1 10 <ping> 0 0 1 0 0 0 <ping> 1 0 0 0`: the
        // shooter (client 1) 10 points and a kill, the target a death.
        let sc = hud.scores.as_ref().expect("a `b` came");
        assert!(hud.counts.scores >= 10, "{:?}", hud.counts);
        assert_eq!((sc.team_scores, sc.score_limit), ([10, 0], 750));
        let row = |c: i32| sc.rows.iter().find(|r| r.client == c).unwrap();
        assert_eq!((row(1).score, row(1).kills, row(1).deaths), (10, 1, 0));
        assert_eq!((row(0).score, row(0).kills, row(0).deaths), (0, 0, 1));
        if hud.loaded.is_none() {
            return;
        }
        let texts: Vec<(&str, f32, f32)> = board
            .iter()
            .filter_map(|p| match p {
                crate::cod4_scoreboard::Piece::Text { text, x, y, .. } => {
                    Some((text.as_str(), *x, *y))
                }
                _ => None,
            })
            .collect();
        println!("{texts:?}, {quads} quads");
        let at = |t: &str| {
            texts
                .iter()
                .find(|x| x.0 == t)
                .unwrap_or_else(|| panic!("no {t}: {texts:?}"))
        };
        // Both banners (`MPUI_MARINES_SHORT`, `MPUI_OPFOR_SHORT`), our
        // own (allies) first, each with its one player.
        let (allies, opfor) = (at("Marines"), at("OpFor"));
        assert!(allies.2 < opfor.2, "{texts:?}");
        let (target, shooter) = (at("target"), at("shooter"));
        assert!(allies.2 < target.1.max(target.2) && target.2 < opfor.2 && opfor.2 < shooter.2);
        assert!(
            texts.iter().any(|t| t.0 == "10" && t.2 == shooter.2),
            "{texts:?}"
        );
        assert!(
            texts.iter().filter(|t| t.0 == "( 1 )").count() == 2,
            "{texts:?}"
        );
        for h in ["Score", "Kills", "Assists", "Deaths", "Ping"] {
            at(h);
        }
        let pics = board
            .iter()
            .filter(|p| matches!(p, crate::cod4_scoreboard::Piece::Pic { .. }))
            .count();
        assert!(pics >= 4, "{board:?}");
        assert!(quads > 100, "{quads}");
    }

    /// The killcam's respawn line names the `+activate` key: vcod's
    /// default bind, F (cod4-hud.md 8).
    #[test]
    fn kill_capture_names_the_activate_key() {
        let shell = Shell::new();
        let mut texts = std::collections::BTreeSet::new();
        let (hud, _) = replay("play-1.7-kill-target.txt", |hud, s, now| {
            let Some(snap) = s.client().snapshots.last() else {
                return;
            };
            for e in h::hud_elems(&snap.ps) {
                if let Some(t) = hud.elem_text(s, &e, now, &shell) {
                    texts.insert(t);
                }
            }
        });
        if hud.loaded.is_none() {
            return;
        }
        println!("{texts:?}");
        assert!(texts.iter().all(|t| !t.contains("[{")), "{texts:?}");
        assert!(texts.contains("Press^3 F ^7to respawn"), "{texts:?}");
    }

    /// The target's own weapon HUD while it holds the M16: 60 in stock,
    /// the clip's count, the rifle's name and both offhands, one each
    /// (cod4-hud.md 9).
    #[test]
    fn kill_capture_draws_the_targets_weapon_info() {
        let mut seen: Vec<OwnerDraw> = Vec::new();
        let (hud, _) = replay("play-1.7-kill-target.txt", |hud, s, _| {
            let (Some(Some(l)), Some(snap)) = (&hud.loaded, s.client().snapshots.last()) else {
                return;
            };
            if snap.ps.get("weapon") == Some(27) && snap.ps.get("pm_type") == Some(0) {
                seen = hud.weapon_info(l, s, snap, 0);
            }
        });
        if hud.loaded.is_none() {
            return;
        }
        println!("{seen:#?}");
        let texts: Vec<&str> = seen
            .iter()
            .filter_map(|d| match d {
                OwnerDraw::Text { text, .. } | OwnerDraw::Name { text, .. } => Some(text.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(texts[0], " 60", "{texts:?}");
        assert!(texts[1].starts_with("M16"), "{texts:?}");
        assert_eq!(&texts[2..], ["1", "1"], "{texts:?}");
        // The clip as the magazine's 30 bullets, not a count.
        assert!(
            seen.iter().any(|d| matches!(d,
                OwnerDraw::Clip { kind: 1, size: 30, rounds, .. } if (1..=30).contains(rounds))),
            "{seen:?}"
        );
        let pics = seen
            .iter()
            .filter(|d| matches!(d, OwnerDraw::Pic { .. }))
            .count();
        assert_eq!(pics, 2, "{seen:?}");
        // Drawn at 1280x960 the bullets touch no other piece: the
        // number the clip once drew sat under the frag icon.
        let Some(Some(l)) = &hud.loaded else { return };
        let sp = ScreenPlacement::new(1280.0, 960.0);
        let boxes = |d: &OwnerDraw| {
            let mut q = Vec::new();
            hud.draw_owner(l, &sp, d, &mut q);
            q.iter()
                .map(|q| [q.verts[0], q.verts[2]])
                .collect::<Vec<_>>()
        };
        let (clip, rest): (Vec<_>, Vec<_>) = seen
            .iter()
            .partition(|d| matches!(d, OwnerDraw::Clip { .. }));
        let bullets = boxes(clip[0]);
        assert_eq!(bullets.len(), 30);
        for d in rest {
            for [a0, a1] in boxes(d) {
                for [b0, b1] in &bullets {
                    let apart =
                        a1[0] <= b0[0] || b1[0] <= a0[0] || a1[1] <= b0[1] || b1[1] <= a0[1];
                    assert!(apart, "{d:?} {a0:?}..{a1:?} meets a bullet {b0:?}..{b1:?}");
                }
            }
        }
    }

    /// The low clip's flash, fixed to the bit against the colours the
    /// owner draw computed before its rewrite: halfway at the start, the
    /// full red near 197 ms (20 pi squared) on.
    #[test]
    fn the_low_clip_flash_eases_toward_red() {
        let cases: [([f32; 4], i32, [f32; 4]); 8] = [
            ([1.0, 1.0, 1.0, 1.0], 0, [1.0, 0.65, 0.65, 1.0]),
            ([1.0, 1.0, 1.0, 1.0], 1, [1.0, 0.6472148, 0.6472148, 1.0]),
            ([1.0, 1.0, 1.0, 1.0], 188, [1.0, 0.3009771, 0.3009771, 1.0]),
            ([1.0, 1.0, 1.0, 1.0], 377, [1.0, 0.6006324, 0.6006324, 1.0]),
            ([1.0, 1.0, 1.0, 1.0], 4321, [1.0, 0.5900641, 0.5900641, 1.0]),
            (
                [0.8, 0.9, 0.7, 0.65],
                31,
                [0.9244196, 0.52674127, 0.45116085, 0.65],
            ),
            (
                [0.8, 0.9, 0.7, 0.65],
                250,
                [0.99136424, 0.3259073, 0.31727153, 0.65],
            ),
            (
                [0.8, 0.9, 0.7, 0.65],
                1000,
                [0.9994621, 0.30161363, 0.30107576, 0.65],
            ),
        ];
        for (fore, ms, want) in cases {
            assert_eq!(low_clip_flash(fore, ms), want, "{fore:?} at {ms} ms");
        }
    }

    /// The magazine's bullets run left of the corner, 4 by 8 pixels each
    /// and touching; the belt stacks upward.

    #[test]
    fn the_clip_graphic_lays_out_its_rounds() {
        let m = clip_rounds(1, 3, [100.0, 50.0]);
        assert_eq!(
            m,
            [
                [96.0, 46.0, 4.0, 8.0],
                [92.0, 46.0, 4.0, 8.0],
                [88.0, 46.0, 4.0, 8.0]
            ]
        );
        let b = clip_rounds(5, 2, [100.0, 50.0]);
        assert_eq!(b, [[92.0, 48.0, 8.0, 4.0], [84.0, 48.0, 8.0, 4.0]]);
        // A row of 20 leftward, the next back rightward 2 higher, the
        // third leftward again; two rows put the first a unit lower.
        let b = clip_rounds(5, 42, [100.0, 50.0]);
        assert_eq!(b.len(), 42);
        for (i, at) in [
            (0, [92.0, 50.0]),
            (19, [-60.0, 50.0]),
            (20, [-60.0, 48.0]),
            (39, [92.0, 48.0]),
            (40, [92.0, 46.0]),
            (41, [84.0, 46.0]),
        ] {
            assert_eq!(b[i], [at[0], at[1], 8.0, 4.0], "round {i}");
        }
        assert!(clip_rounds(6, 1, [0.0, 0.0]).is_empty());
        assert!(clip_rounds(0, 4, [0.0, 0.0]).is_empty());
        assert!(clip_rounds(1, 0, [10.0, 10.0]).is_empty());
        assert!(clip_rounds(3, -2, [10.0, 10.0]).is_empty());
        let base = [612.5, 431.25];
        assert_eq!(
            clip_rounds(2, 2, base),
            [[580.5, 427.25, 32.0, 8.0], [540.5, 427.25, 32.0, 8.0]]
        );
        assert_eq!(
            clip_rounds(3, 3, base),
            [
                [596.5, 427.25, 16.0, 8.0],
                [576.5, 427.25, 16.0, 8.0],
                [556.5, 427.25, 16.0, 8.0]
            ]
        );
        assert_eq!(
            clip_rounds(4, 2, base),
            [[548.5, 423.25, 64.0, 16.0], [476.5, 423.25, 64.0, 16.0]]
        );
        assert_eq!(
            clip_rounds(5, 5, base),
            [
                [604.5, 429.25, 8.0, 4.0],
                [596.5, 429.25, 8.0, 4.0],
                [588.5, 429.25, 8.0, 4.0],
                [580.5, 429.25, 8.0, 4.0],
                [572.5, 429.25, 8.0, 4.0]
            ]
        );
    }

    /// The target's killcam draws the `killcam` menu over the killer's
    /// view: both letterbox bars and the 7 letters of `KILLCAM` on every
    /// frame of it, never outside; the script's tenths timer counts down
    /// beside it (cod4-hud.md 11).
    #[test]
    fn kill_capture_draws_the_killcam_letterbox_title_and_timer() {
        let (mut frames, mut drawn, mut outside) = (0, 0, 0);
        let mut timers = std::collections::BTreeSet::new();
        let shell = Shell::new();
        let (hud, _) = replay("play-1.7-kill-target.txt", |hud, s, now| {
            let (Some(Some(l)), Some(snap)) = (&hud.loaded, s.client().snapshots.last()) else {
                return;
            };
            let sp = ScreenPlacement::new(1280.0, 960.0);
            let mut q = Vec::new();
            if in_killcam(snap) {
                frames += 1;
                hud.draw_killcam(l, &sp, &mut q);
                for e in h::hud_elems(&snap.ps).iter().filter(|e| e.kind == 9) {
                    timers.extend(hud.elem_text(s, e, now, &shell));
                }
            }
            let all = hud.build(s, (1280.0, 960.0), now, None, &shell);
            let bars = |q: &[HudQuad]| {
                q.iter()
                    .filter(|q| q.rgba == [0.49, 0.05, 0.03, 0.5])
                    .count()
            };
            if in_killcam(snap) {
                let title = q.iter().filter(|q| q.rgba == [1.0; 4]).count();
                if bars(&q) == 2 && title == 7 && bars(&all) == 2 {
                    drawn += 1;
                    // The bars span the window: 112 of 480 rows each.
                    assert_eq!(
                        q[0].verts,
                        [[0.0, 0.0], [1280.0, 0.0], [1280.0, 224.0], [0.0, 224.0]]
                    );
                }
            } else if bars(&all) != 0 {
                outside += 1;
            }
        });
        if hud.loaded.is_none() {
            return;
        }
        println!("{frames} killcam frames, timers {timers:?}");
        assert!(frames > 200, "{frames}");
        assert_eq!(drawn, frames);
        assert_eq!(outside, 0);
        assert!(
            timers.contains("0:02.5") && timers.contains("0:00.0"),
            "{timers:?}"
        );
    }

    /// The obituary reaches the screen: the kill line's M16 icon draws
    /// for the message window's 5 s and the centre print's letters, 150
    /// under the middle, for `cg_centertime`'s, both from the death on,
    /// on into the killcam, which starts 2 s on (cod4-hud.md 3).
    #[test]
    fn kill_capture_draws_the_obituary_line_and_centre_print() {
        let (mut icon, mut centre) = (Vec::new(), Vec::new());
        let (hud, _) = replay("play-1.7-kill-target.txt", |hud, s, now| {
            let q = hud.build(s, (1280.0, 960.0), now, None, &Shell::new());
            // `hud_icon_m16a4_grenade`'s image.
            if q.iter().any(|q| q.texture == "images/hud_m16a4_m203.iwi") {
                icon.push(now);
            }
            let Some(Some(l)) = &hud.loaded else { return };
            let mut c = Vec::new();
            hud.draw_center(l, &ScreenPlacement::new(1280.0, 960.0), now, &mut c);
            // Every quad of it in the frame's HUD.
            if !c.is_empty()
                && c.iter().all(|c| {
                    q.iter()
                        .any(|x| x.verts == c.verts && x.texture == c.texture)
                })
            {
                centre.push(now);
            }
        });
        if hud.loaded.is_none() {
            return;
        }
        let span = |v: &[i32]| v.last().unwrap() - v.first().unwrap();
        println!(
            "icon {:?}..{:?}, centre {:?}..{:?}",
            icon.first(),
            icon.last(),
            centre.first(),
            centre.last()
        );
        assert!((4900..=5100).contains(&span(&icon)), "{}", span(&icon));
        assert!((4900..=5100).contains(&span(&centre)), "{}", span(&centre));
        assert!(icon[0] < 50_000 && centre[0] < 50_000);
    }

    /// The walker's compass pings its idle teammate (`mate`, allies)
    /// once both are in the world; no enemy shows without a UAV, and the
    /// war round carries no objective (cod4-hud.md 10).
    #[test]
    fn walk_capture_pings_the_teammate_on_the_compass() {
        let (mut frames, mut pinged, mut others) = (0, 0, 0);
        let mut tex = false;
        let (hud, _) = replay("play-1.7-walk.txt", |hud, s, now| {
            let (Some(Some(l)), Some(scene)) = (&hud.loaded, s.scene()) else {
                return;
            };
            frames += 1;
            let icons = hud.compass_icons(l, s, scene.eye, scene.angles.y);
            for i in &icons {
                if i.material == "compassping_friendly_mp" {
                    pinged += 1;
                    let [x, y] = i.centre;
                    assert!(
                        (6.0..=108.0).contains(&x) && (18.0..=120.0).contains(&y),
                        "{i:?}"
                    );
                } else {
                    others += 1;
                }
            }
            if !icons.is_empty() && !tex {
                // `compassping_friendly_mp` draws `common_mp`'s image by
                // reference (`,compassping_green_hollow_mp`).
                let q = hud.build(
                    s,
                    (1280.0, 960.0),
                    now,
                    Some((scene.eye, scene.angles.y)),
                    &Shell::new(),
                );
                tex |= q
                    .iter()
                    .any(|q| q.texture == "images/compassping_green_hollow_mp.iwi");
            }
        });
        if hud.loaded.is_none() {
            return;
        }
        println!("{frames} frames, {pinged} pings, {others} others");
        assert!(pinged > frames / 2, "{pinged} of {frames}");
        assert_eq!(others, 0);
        assert!(tex, "the ping's image draws");
    }

    /// Every committed 1.7 capture's playerstate carries
    /// `iCompassPlayerInfo` 0, the walk capture's while its teammate,
    /// whom `G_GetNonPVSPlayerInfo` scans without a UAV, stands far off:
    /// 1.7's `SV_inSnapshot` finds every playing client in the snapshot
    /// (cod4-hud.md 15.3), as vcod-server's snapshot is.
    #[test]
    fn retail_captures_send_no_compass_player_info() {
        let mut mates = 0;
        for name in [
            "play-1.7-walk.txt",
            "play-1.7-spec.txt",
            "play-1.7-fly.txt",
            "play-1.7-kill-shooter.txt",
            "play-1.7-kill-target.txt",
            "play-1.7-mantle.txt",
            "play-1.7-menus-together.txt",
            "play-1.7-prematch.txt",
            "play-1.7-score-target.txt",
        ] {
            let mut last = None;
            replay(name, |_, s, _| {
                let Some(snap) = s.client().snapshots.last() else {
                    return;
                };
                if last == Some(snap.server_time) {
                    return;
                }
                last = Some(snap.server_time);
                assert_eq!(snap.ps.get("iCompassPlayerInfo").unwrap_or(0), 0, "{name}");
                let me = snap.ps.get("clientNum").unwrap_or(0);
                let team = |n: u32| {
                    snap.clients
                        .iter()
                        .find(|c| c.0[0] == n)
                        .map_or(0, |c| client_word(c, "team"))
                };
                mates += snap
                    .entities
                    .iter()
                    .filter(|e| e.etype() == crate::play::cod4::ET_PLAYER)
                    .filter(|e| {
                        let n = ent_u32(e, "clientNum");
                        n != me && matches!(team(me), 1 | 2) && team(n) == team(me)
                    })
                    .count();
            });
        }
        assert!(mates > 100, "{mates}");
    }

    #[test]
    fn spectator_capture_carries_the_walkers_chat_line() {
        let (hud, _) = replay("play-1.7-spec.txt", |_, _, _| {});
        assert_eq!(hud.counts.chat_lines, 1, "{:?}", hud.counts);
        let Some(Piece::Text(t)) = hud.chat[0].pieces.first() else {
            panic!()
        };
        assert_eq!(t, "^9walker^7: ^7vcod probe here");
        assert_eq!(hud.counts.kill_lines, 0);
    }

    /// Fails with the whole new text when `actual` moved off the pin.
    fn pin(tag: &str, actual: &str, expected: &str) {
        assert!(actual == expected, "{tag} moved:\n{actual}");
    }

    /// Chat and server texts to wrap: colour codes, long words, runs of
    /// spaces and localized (multibyte) text.
    pub(crate) fn wrap_corpus() -> Vec<String> {
        let mut v: Vec<String> = [
            "",
            "short",
            "^",
            "trailing caret^",
            "^^1 caret pair ^x not a code",
            "  leading and trailing spaces  ",
        ]
        .map(String::from)
        .to_vec();
        v.push(format!("^1{} tail", "word ".repeat(12)));
        for n in [40, 41, 42, 52, 53, 54, 58, 59, 60, 120] {
            v.push("a".repeat(n));
        }
        v.push(format!("{} {}", "x".repeat(50), "y".repeat(30)));
        v.push(format!("{} ^3{}", "a".repeat(50), "b".repeat(60)));
        v.push("^1a^2b^3c^4d^5e^6f^7g^8h^9i^0j ".repeat(6));
        v.push(format!("{}   {}", "c".repeat(45), "d".repeat(45)));
        v.push(format!("{} {}", "c".repeat(30), "d".repeat(40)));
        v.push("Привет мир, это длинное сообщение ".repeat(3));
        v.push("日本語のテキストは空白がない".repeat(4));
        v.push(format!("{}ä{}", "e".repeat(57), "f".repeat(10)));
        v.push("^2Shooter^7: ^7gg, nice shot from across the whole map, really nice one".into());
        v
    }

    #[test]
    fn chat_wrap_pins() {
        let s: String = wrap_corpus()
            .iter()
            .map(|t| format!("{t:?} => {:?}\n", wrap_chat(t)))
            .collect();
        pin(
            "chat",
            &s,
            r##""" => []
"short" => ["short"]
"^" => ["^"]
"trailing caret^" => ["trailing caret^"]
"^^1 caret pair ^x not a code" => ["^^1 caret pair ^x not a code"]
"  leading and trailing spaces  " => ["  leading and trailing spaces  "]
"^1word word word word word word word word word word word word  tail" => ["^1word word word word word word word word word word ", "word word  tail"]
"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa" => ["aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"]
"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa" => ["aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"]
"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa" => ["aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"]
"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa" => ["aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"]
"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa" => ["aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"]
"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa" => ["aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa", "a"]
"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa" => ["aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa", "aaaaa"]
"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa" => ["aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa", "aaaaaa"]
"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa" => ["aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa", "aaaaaaa"]
"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa" => ["aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa", "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa", "aaaaaaaaaaaaaa"]
"xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx yyyyyyyyyyyyyyyyyyyyyyyyyyyyyy" => ["xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx ", "yyyyyyyyyyyyyyyyyyyyyyyyyyyyyy"]
"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa ^3bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb" => ["aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa ", "^3bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb", "bbbbbbbbb"]
"^1a^2b^3c^4d^5e^6f^7g^8h^9i^0j ^1a^2b^3c^4d^5e^6f^7g^8h^9i^0j ^1a^2b^3c^4d^5e^6f^7g^8h^9i^0j ^1a^2b^3c^4d^5e^6f^7g^8h^9i^0j ^1a^2b^3c^4d^5e^6f^7g^8h^9i^0j ^1a^2b^3c^4d^5e^6f^7g^8h^9i^0j " => ["^1a^2b^3c^4d^5e^6f^7g^8h^9i^0j ^1a^2b^3c^4d^5e^6f^7g^8h^9i^0j ^1a^2b^3c^4d^5e^6f^7g^8h^9i^0j ^1a^2b^3c^4d^5e^6f^7g^8h^9i^0j ", "^1a^2b^3c^4d^5e^6f^7g^8h^9i^0j ^1a^2b^3c^4d^5e^6f^7g^8h^9i^0j "]
"ccccccccccccccccccccccccccccccccccccccccccccc   ddddddddddddddddddddddddddddddddddddddddddddd" => ["ccccccccccccccccccccccccccccccccccccccccccccc   ", "ddddddddddddddddddddddddddddddddddddddddddddd"]
"cccccccccccccccccccccccccccccc dddddddddddddddddddddddddddddddddddddddd" => ["cccccccccccccccccccccccccccccc ", "dddddddddddddddddddddddddddddddddddddddd"]
"Привет мир, это длинное сообщение Привет мир, это длинное сообщение Привет мир, это длинное сообщение " => ["Привет мир, это длинное сообщение Привет мир, это ", "длинное сообщение Привет мир, это длинное сообщение "]
"日本語のテキストは空白がない日本語のテキストは空白がない日本語のテキストは空白がない日本語のテキストは空白がない" => ["日本語のテキストは空白がない日本語のテキストは空白がない日本語のテキストは空白がない日本語のテキストは空白", "がない"]
"eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeäffffffffff" => ["eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee", "eeeeäffffffffff"]
"^2Shooter^7: ^7gg, nice shot from across the whole map, really nice one" => ["^2Shooter^7: ^7gg, nice shot from across the whole map, ", "really nice one"]
"##,
        );
    }

    #[test]
    fn chat_wraps_past_52_characters_at_a_space() {
        let long = format!("^1{} tail", "word ".repeat(12));
        let lines = wrap_chat(&long);
        assert_eq!(lines.len(), 2, "{lines:?}");
        assert!(lines[0].ends_with(' '), "{lines:?}");
        assert!(lines[1].ends_with("tail"));
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

#[path = "cod4_fullmap.rs"]
mod fullmap;

#[path = "cod4_connect.rs"]
pub(crate) mod connect;

#[path = "cod4_hints.rs"]
pub(crate) mod hints;

#[path = "cod4_grenades.rs"]
pub(crate) mod grenades;
