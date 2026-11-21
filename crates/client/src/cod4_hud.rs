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

mod fullmap;

pub(crate) mod connect;

pub(crate) mod hints;

pub(crate) mod grenades;

mod assets;
mod board;
mod compass;
mod elems;
mod env;
mod fonts;
mod messages;
mod text;
mod ticker;
mod weapon_info;
mod world;

use assets::Loaded;
pub(crate) use assets::{LevelZones, MenuAssets};
pub(crate) use fonts::console_font;
use messages::{draw_say, push_line, wrap_chat};
use text::draw_interrupted;
pub(crate) use text::{glow_text, text};
use vcod_common::net::Msec;
use weapon_info::{DPAD_MENU, LOW_HEALTH_MENU, TALKERS_MENU};

#[cfg(test)]
pub(crate) use menus::Hint;

use std::collections::HashMap;

use vcod_cod4::hud::{self as h, Command, HudElem, Scores};
use vcod_cod4::net::delta::EntityState;
use vcod_cod4::net::parse::Snapshot;

use crate::cod4_radar::MapAxes;
use crate::console::shell::Shell;
use crate::hud::HudQuad;
use crate::hud::cod2elem::ScreenPlacement;
use crate::hud::font::{self, Font};
use crate::play::cod4::{Cod4Session, ET_HELICOPTER, ET_PLANE, EV_OBITUARY, Fired, client_u32};

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

/// A line of text at `scale`, grid units: the 48-unit font height,
/// truncated as the layouts read it.
fn text_height(scale: f32) -> f32 {
    (48.0 * scale) as i32 as f32
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

#[derive(Default)]
pub(crate) struct Cod4Hud {
    /// Loaded on first use; `None` inside once that failed.
    loaded: Option<Option<Box<Loaded>>>,
    /// The `fs_game` `loaded` was loaded under.
    fs_game: Option<String>,
    /// The map whose zone `loaded` holds; `None` for the front end's or
    /// the connect screen's.
    zones_map: Option<String>,
    /// Configstrings `d` changed since the gamestate, and the session's
    /// gamestate count they were read under (a capture's gamestate takes
    /// no `d`, so the HUD keeps them).
    cs: HashMap<u16, String>,
    level_gamestates: u32,
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

impl Cod4Hud {
    /// The `playLocalSound` and `stopLocalSound` aliases the server sent
    /// since the last call, in order; true starts one.
    pub(crate) fn take_local_sounds(&mut self) -> Vec<(String, bool)> {
        std::mem::take(&mut self.local_sounds)
    }

    /// A new gamestate restarts the cgame (`CL_ParseGamestate`): the last
    /// level's `d` strings, scores, message windows, centre print, compass
    /// pings, damage overlay and hints go.
    fn new_level(&mut self) {
        self.cs.clear();
        self.scores = None;
        self.team_scores = [0; 4];
        self.windows = Default::default();
        self.center = None;
        self.radar = Default::default();
        self.damage = Default::default();
        self.invalid_cmd = None;
        self.hint = Default::default();
    }

    /// Takes in the session's new server commands and this step's fired
    /// events at server time `now`.
    pub(crate) fn update(&mut self, s: &mut Cod4Session, fired: &[Fired], now: i32) {
        if let Some(gs) = s.gamestate() {
            self.weapons = crate::play::cod4::weapon_list(gs);
        }
        if self.level_gamestates != s.gamestate_count() {
            self.level_gamestates = s.gamestate_count();
            self.new_level();
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
                .find(|x| x.server_time == Msec::from_raw(f.server_time))
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
}

/// `cg.inKillCam`: the drawn playerstate is an archived one
/// (`ps.deltaTime` set, `CG_TransitionKillcam`).
fn in_killcam(snap: &Snapshot) -> bool {
    snap.ps.get("deltaTime").unwrap_or(0) != 0
}

/// `PM_DEAD` and up hide `HE_FLAG_HIDE_WHEN_DEAD` elements;
/// `PM_SPECTATOR` and up draw no weapon HUD.
use vcod_cod4::net::flags::{PM_DEAD, PM_SPECTATOR};

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
}

/// A material's colour map (`materialIndex`'s or a kill icon's) as the
/// HUD's texture name; `white`, whose image is the engine's `$white`, as
/// the HUD's own white.
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
    HudQuad::rect_uv([x, y, w, h], [u0, 0.0, u1, 1.0], rgba, texture)
}

#[cfg(test)]
pub(crate) mod tests {
    //! The HUD half of the CoD4 replay gate: the committed captures through
    //! the session's steps and [`Cod4Hud::update`] at 60 frames a second,
    //! as `cod4_client` runs them; with `COD4_DIR` the HUD is built too.

    use super::*;

    pub(super) fn close<const N: usize>(got: [f32; N], want: [f32; N]) {
        let off = got.iter().zip(&want).any(|(g, w)| (g - w).abs() > 1e-3);
        assert!(!off, "{got:?} != {want:?}");
    }

    pub(super) const FIXTURES: &str =
        concat!(env!("CARGO_MANIFEST_DIR"), "/../cod4/tests/fixtures/net");

    /// The capture replayed; `build` gets every built frame's quads.
    pub(super) fn replay(
        name: &str,
        build: impl FnMut(&Cod4Hud, &mut Cod4Session, i32),
    ) -> (Cod4Hud, Cod4Session) {
        replay_in(name, false, build)
    }

    /// [`replay`], with mp_crash's world loaded when `world` is set (the
    /// level's weapon defs, which the alt mode's action slot reads).
    pub(super) fn replay_in(
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

    /// A `d` of the last level masks the gamestate only until a new
    /// gamestate comes: then the HUD reads the new table.
    #[test]
    fn a_new_gamestate_drops_the_last_levels_d_strings() {
        let (mut hud, mut s) = replay("play-1.7-prematch.txt", |_, _, _| {});
        let own = String::from_utf8_lossy(s.gamestate().unwrap().configstring(11)).into_owned();
        hud.cs.insert(11, "last level".into());
        hud.update(&mut s, &[], 0);
        assert_eq!(hud.configstring(&s, 11), "last level");
        s.add_gamestate_for_test();
        hud.update(&mut s, &[], 0);
        assert_eq!(hud.configstring(&s, 11), own);
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
}
