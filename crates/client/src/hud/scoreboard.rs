//! The scoreboard: the `b` server command, requested on demand
//! (docs/research/cod11-hud-protocol.md, sections 3-4), drawn as the cgame
//! draws it (section 3.1, `0x3002b650` and the functions it calls; 1.5 and
//! United Offensive draw the same board). CoD2's `CG_DrawScoreboard` (Mac
//! 0x1c6e30) is the same board with its own [`Style`]
//! (docs/research/cod2-hud.md 13).

use super::HudQuad;
use super::font::{Slot, UiFonts};
use super::hudelem::Virtual;
use super::player::{menu_text_rgba, text_height, text_width};
use vcod_common::localize::Localized;

/// The stock client's re-request cadence while the board is up (doc section 4).
pub const REQUEST_INTERVAL: f32 = 2.0;

/// `cl[+0x217c] == 3` (doc section 3). Team 0 is "none", which every row
/// carries in `dm`.
const SPECTATOR_TEAM: i32 = 3;

/// The fade after the key goes up: `0x30019a30(100, release)`.
const FADE_MS: i32 = 100;

/// The table's left edge, and the bottom no banner or row crosses.
const LEFT: f32 = 129.0;
const BOTTOM: f32 = 432.0;
/// Where the table starts, above the objective text.
const TOP: f32 = 52.0;
/// The table's width, and its width beside the scroll bar.
const WIDTH: f32 = 382.0;
const WIDTH_SCROLLED: f32 = 374.0;
const ROW_H: f32 = 12.0;
/// The room a banner needs below it before it draws.
const BANNER_STEP: f32 = 24.0;
const SECTION_GAP: f32 = 4.0;

/// The five columns (`0x30060d38`): what each shows, its share of the
/// table's width, its header and whether it is right-aligned.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Column {
    /// CoD2's talk icon ([`BoardView::voice`]).
    Voice,
    Icon,
    Name,
    Score,
    Deaths,
    Ping,
}

type Columns = [(Column, f32, &'static str, bool)];

const COLUMNS: [(Column, f32, &str, bool); 5] = [
    (Column::Icon, 0.05, "", false),
    (Column::Name, 0.41, "", false),
    (Column::Score, 0.16, "CGAME_SB_SCORE", true),
    (Column::Deaths, 0.2, "CGAME_SB_DEATHS", true),
    (Column::Ping, 0.18, "CGAME_SB_PING", true),
];

/// CoD2's `columnInfo` (Mac 0x36e6e0): voice, status icon, name, score,
/// deaths, ping.
const COLUMNS_COD2: [(Column, f32, &str, bool); 6] = [
    (Column::Voice, 0.05, "", false),
    (Column::Icon, 0.05, "", false),
    (Column::Name, 0.47, "", false),
    (Column::Score, 0.13, "CGAME_SB_SCORE", true),
    (Column::Deaths, 0.13, "CGAME_SB_DEATHS", true),
    (Column::Ping, 0.17, "CGAME_SB_PING", true),
];

/// Where one game's board differs from 1.1's.
pub struct Style {
    columns: &'static Columns,
    /// The black panel's alpha.
    panel_alpha: f32,
    /// A banner's drawn height and how far the next item starts below it.
    banner_h: f32,
    banner_step: f32,
    /// The banner text's baseline below the banner's top.
    banner_text_y: f32,
    /// The column headers', a row's and an objective line's baselines.
    header_text_y: f32,
    row_text_y: f32,
    objective_text_y: f32,
    /// A cell too wide for its column: the font shrinks in 0.02 steps
    /// (CoD2) instead of losing characters (1.1).
    shrink_cells: bool,
    /// The title's fit measures the game type at 0.41 whatever the map's
    /// scale (1.1); CoD2 measures both at the shrinking scale.
    title_fixed_left: bool,
}

/// 1.1's board (`0x3002b650`); 1.5 and United Offensive draw it too.
pub const COD1: Style = Style {
    columns: &COLUMNS,
    panel_alpha: 0.55,
    banner_h: 32.0,
    banner_step: 24.0,
    banner_text_y: 18.0,
    header_text_y: 10.5,
    row_text_y: 10.5,
    objective_text_y: 9.0,
    shrink_cells: false,
    title_fixed_left: true,
};

/// CoD2 1.3's (cod2-hud.md 13): `cg_scoreboardBannerHeight` 24 draws the
/// banner 24 / 0.75 high, the rows starting under it, its text at 24 *
/// 0.84.
pub const COD2: Style = Style {
    columns: &COLUMNS_COD2,
    panel_alpha: 0.8,
    banner_h: 32.0,
    banner_step: 32.0,
    banner_text_y: 20.16,
    header_text_y: 11.76,
    row_text_y: 10.08,
    objective_text_y: 10.08,
    shrink_cells: true,
    title_fixed_left: false,
};

/// One `b` row (doc section 3). `team` is not on the wire: `parse_scores`
/// leaves it 0 and the board fills it from the client's info.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScoreEntry {
    pub client: u32,
    pub score: i32,
    pub ping: i32,
    pub deaths: i32,
    pub status_icon: i32,
    pub team: i32,
}

/// A parsed `b` command. Totals are axis then allies (doc section 5).
/// `truncated`: `numRows` claimed more rows than the message carried.
pub struct ScoresMessage {
    pub axis: i32,
    pub allies: i32,
    pub entries: Vec<ScoreEntry>,
    pub truncated: bool,
}

/// `b <numRows> <axis> <allies> {<client> <score> <ping> <deaths> <statusIcon>}*`
/// (doc section 3). A bad header is an `Err`; a short row sequence keeps the
/// rows that parsed and sets `truncated`.
pub fn parse_scores(tokens: &[String]) -> Result<ScoresMessage, String> {
    if tokens.first().map(String::as_str) != Some("b") {
        return Err(format!("not a scores command: {:?}", tokens.first()));
    }
    let int = |i: usize, what: &str| -> Result<i32, String> {
        tokens
            .get(i)
            .ok_or_else(|| format!("missing {what}"))?
            .parse::<i32>()
            .map_err(|_| format!("bad {what}: {:?}", tokens.get(i)))
    };
    // The cgame keeps 64 rows at most (0x3002b920).
    let num_rows = int(1, "numRows")?.clamp(0, 64) as usize;
    let axis = int(2, "axisScore")?;
    let allies = int(3, "alliesScore")?;

    let mut entries = Vec::with_capacity(num_rows);
    let mut truncated = false;
    let mut idx = 4;
    for _ in 0..num_rows {
        if idx + 5 > tokens.len() {
            truncated = true;
            break;
        }
        let field = |off: usize, what: &str| -> Result<i32, String> {
            tokens[idx + off]
                .parse::<i32>()
                .map_err(|_| format!("bad {what}: {}", tokens[idx + off]))
        };
        let client = field(0, "client")?;
        entries.push(ScoreEntry {
            // Outside 0..63 reads as client 0.
            client: if (0..64).contains(&client) {
                client as u32
            } else {
                0
            },
            score: field(1, "score")?,
            ping: field(2, "ping")?,
            deaths: field(3, "deaths")?,
            status_icon: field(4, "statusIcon")?,
            team: 0,
        });
        idx += 5;
    }

    Ok(ScoresMessage {
        axis,
        allies,
        entries,
        truncated,
    })
}

/// What the board reads besides the `b` reply.
pub struct BoardView<'a> {
    /// HUD clock, seconds.
    pub now: f32,
    /// The local client's number (`CG_Init`'s), whose row is lit.
    pub own_client: Option<u32>,
    /// A client's name and team; `None` without client info, which drops
    /// its row.
    pub client: &'a dyn Fn(u32) -> Option<(String, i32)>,
    /// The client cvars the server set (`g_TeamName_*`, `g_TeamColor_*`,
    /// `g_ScoresBanner_*`, `cg_objectiveText`).
    pub cvar: &'a dyn Fn(&str) -> Option<String>,
    pub loc: &'a Localized,
    /// A command's key text, for the objective's `[{command}]` pass.
    pub bound_key: &'a dyn Fn(&str) -> Option<String>,
    /// The title row: the game type's name and the map.
    pub gametype: &'a str,
    pub map: &'a str,
    /// The footer: `sv_hostname` and the server's address.
    pub hostname: &'a str,
    pub address: Option<&'a str>,
    /// A `b` row's status icon index to its material.
    pub status_icon: &'a dyn Fn(i32) -> Option<String>,
    /// CoD2's voice column: a client's `voice_off` or `voice_on`, or none.
    pub voice: &'a dyn Fn(u32) -> Option<&'static str>,
}

/// The last parsed `b` reply and the board's own state. A malformed reply
/// keeps the previous table.
pub struct Scoreboard {
    entries: Vec<ScoreEntry>,
    pub axis_score: i32,
    pub allies_score: i32,
    /// `+scores` is held.
    held: bool,
    /// When `-scores` came, HUD seconds: the board fades out from there.
    released_at: Option<f32>,
    /// The drawn playerstate is in intermission (`pm_type` 5), which draws
    /// the board without the key (0x30018530).
    pub intermission: bool,
    /// Render-clock seconds of the next due `score` request; `f32::MIN` so
    /// the first check fires at once.
    next_request: f32,
    warned_malformed: bool,
    warned_truncated: bool,
    /// Gates [`Self::totals`] so an unrequested board does not override the
    /// header with `(0, 0)`.
    received: bool,
    /// Items (banners and rows) scrolled off the top (`0x3020c03c`).
    scroll: i32,
    /// The last draw ran out of room (`0x3020c040`); only then does a
    /// scroll down move.
    overflowed: bool,
    /// The team drawn first while the scores are level (`0x3007497c`,
    /// allies at start).
    leader: i32,
    /// Player rows the last draw put on screen.
    rows_drawn: usize,
    /// The game whose layout this board draws.
    pub style: &'static Style,
}

impl Scoreboard {
    pub fn new() -> Scoreboard {
        Scoreboard {
            entries: Vec::new(),
            axis_score: 0,
            allies_score: 0,
            held: false,
            released_at: None,
            intermission: false,
            next_request: f32::MIN,
            warned_malformed: false,
            warned_truncated: false,
            received: false,
            scroll: 0,
            overflowed: false,
            leader: 2,
            rows_drawn: 0,
            style: &COD1,
        }
    }

    /// Player rows the last [`Self::build`] laid out (drawn or scrolled
    /// past).
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn rows_drawn(&self) -> usize {
        self.rows_drawn
    }

    /// Latest (axis, allies), or `None` before the first reply. `Hud::build`
    /// prefers these over CS 5/6, which can lag (doc section 5, "Header vs.
    /// scoreboard totals").
    pub fn totals(&self) -> Option<(i32, i32)> {
        self.received
            .then_some((self.axis_score, self.allies_score))
    }

    /// Ignores anything but `b`. A parse failure keeps the last table and
    /// warns once.
    pub fn on_server_command(&mut self, tokens: &[String]) {
        if tokens.first().map(String::as_str) != Some("b") {
            return;
        }
        match parse_scores(tokens) {
            Ok(msg) => {
                if msg.truncated && !self.warned_truncated {
                    self.warned_truncated = true;
                    log::warn!(
                        "scores command: numRows claimed more rows than were present; kept the {} rows that parsed",
                        msg.entries.len()
                    );
                }
                self.axis_score = msg.axis;
                self.allies_score = msg.allies;
                self.entries = msg.entries;
                self.received = true;
            }
            Err(e) => {
                if !self.warned_malformed {
                    self.warned_malformed = true;
                    log::warn!("malformed scores command, keeping the last table: {e}");
                }
            }
        }
    }

    /// `+scores` at HUD time `now` (cgame 0x30013ce0). True when `score`
    /// is to go out, at most once in [`REQUEST_INTERVAL`]; a press that
    /// sends while the board is down also drops the old rows and the
    /// scroll.
    pub fn press(&mut self, now: f32) -> bool {
        let send = now >= self.next_request;
        if send {
            self.mark_requested(now);
            if !self.held {
                self.entries.clear();
                self.scroll = 0;
            }
        }
        self.held = true;
        send
    }

    /// `-scores` (0x30013cc0), and a `map_restart` (0x3002ca60): the board
    /// fades from `now`.
    pub fn release(&mut self, now: f32) {
        if self.held {
            self.released_at = Some(now);
        }
        self.held = false;
    }

    /// Takes the board down at once, no fade.
    pub fn hide(&mut self) {
        self.held = false;
        self.released_at = None;
    }

    pub fn held(&self) -> bool {
        self.held
    }

    /// True while the board is up and [`REQUEST_INTERVAL`] has elapsed. The
    /// caller sends `score` and then calls [`Self::mark_requested`].
    pub fn due(&self, now: f32) -> bool {
        (self.held || self.intermission) && now >= self.next_request
    }

    pub fn mark_requested(&mut self, now: f32) {
        self.next_request = now + REQUEST_INTERVAL;
    }

    /// Drop the rows; the totals, visibility and request timer stay.
    pub fn clear(&mut self) {
        self.entries.clear();
    }

    /// The board's alpha at `now`, `None` when it is not drawn: 1 while
    /// held or in intermission, then `left * 0.01` over the 100 ms fade.
    pub fn alpha(&self, now: f32) -> Option<f32> {
        if self.held || self.intermission {
            return Some(1.0);
        }
        let since = ((now - self.released_at?) * 1000.0) as i32;
        let left = FADE_MS - since;
        (since >= 0 && left > 0).then_some(left as f32 * 0.01)
    }

    /// Mouse wheel, page up and keypad page up while held
    /// (`0x30027180`): `step` items back toward the top.
    pub fn scroll_up(&mut self, step: i32) {
        if self.scroll > 0 {
            self.scroll = (self.scroll - step).max(0);
        }
    }

    /// The wheel, page down and keypad page down (`0x3002b720`): only a
    /// board that ran out of room moves, at most to its last item.
    pub fn scroll_down(&mut self, step: i32) {
        if self.overflowed {
            self.scroll = (self.scroll + step).min(self.entries.len() as i32 - 1);
        }
    }

    /// `weapnext` (`forward`) or `weapprev` while `+scores` holds the board
    /// (cgame 0x30038040, 0x300380b0; 1.5 and UO the same): the board
    /// scrolls `step` items up or down and no weapon is cycled, ahead of
    /// every other gate of the two commands. Whether the board took it.
    pub fn take_cycle(&mut self, forward: bool, step: i32) -> bool {
        if !self.held {
            return false;
        }
        if forward {
            self.scroll_up(step);
        } else {
            self.scroll_down(step);
        }
        true
    }

    /// Draws the board (`0x3002b650`): the frame and its title and footer
    /// rows, the objective, the column headers, then the team blocks.
    /// False when nothing is drawn.
    pub fn build(
        &mut self,
        b: &BoardView,
        fonts: &UiFonts,
        v: &Virtual,
        out: &mut Vec<HudQuad>,
    ) -> bool {
        let Some(alpha) = self.alpha(b.now) else {
            return false;
        };
        let d = Draw {
            fonts,
            v,
            alpha,
            style: self.style,
        };
        frame(&d, b, out);
        self.table(&d, b, out);
        true
    }

    /// `0x3002b410`.
    fn table(&mut self, d: &Draw, b: &BoardView, out: &mut Vec<HudQuad>) {
        // The parser's team: the client's if it has info, else 0 (none).
        let rows: Vec<(ScoreEntry, Option<String>)> = self
            .entries
            .iter()
            .map(|e| match (b.client)(e.client) {
                Some((name, team)) => (ScoreEntry { team, ..*e }, Some(name)),
                None => (ScoreEntry { team: 0, ..*e }, None),
            })
            .collect();
        let count = |t: i32| rows.iter().filter(|(e, _)| e.team == t).count();
        let counts = [count(0), count(1), count(2), count(3)];
        let ping = |t: i32| {
            let sum: i32 = rows
                .iter()
                .filter(|(e, _)| e.team == t)
                .map(|(e, _)| e.ping)
                .sum();
            if counts[t as usize] < 1 || sum < 1 {
                0
            } else {
                sum / counts[t as usize] as i32
            }
        };
        let teams = counts[1] != 0 || counts[2] != 0;

        self.overflowed = false;
        let mut y = objective(d, b, TOP, out);
        // `0x3002a740`: the room the whole board asks for.
        let mut need = if teams { 66.0 } else { 10.0 };
        let mut items = if teams { 2 } else { 0 };
        for t in [0, 3] {
            if counts[t] != 0 {
                need += 28.0;
                items += 1;
            }
        }
        need += ROW_H * rows.len() as f32;
        items += rows.len() as i32;
        let fits = need <= BOTTOM - y;
        let width = if fits { WIDTH } else { WIDTH_SCROLLED };
        y = headers(d, b, y + SECTION_GAP, width, out);
        let list_top = y;

        let (axis, allies) = (self.axis_score, self.allies_score);
        if teams {
            self.leader = if allies < axis {
                1
            } else if axis < allies {
                2
            } else {
                self.leader
            };
        }
        let mut cx = Cursor {
            width,
            item: 0,
            scroll: self.scroll,
            overflowed: false,
            rows: 0,
        };
        let mut section = |team: i32, y: f32, out: &mut Vec<HudQuad>| {
            let total = match team {
                1 => axis,
                2 => allies,
                _ => 0,
            };
            let y = cx.banner(d, b, team, counts[team as usize], total, ping(team), y, out);
            cx.rows(d, b, team, &rows, y, out)
        };
        if teams {
            let first = self.leader;
            y = section(first, y, out) + SECTION_GAP;
            y = section(if first == 1 { 2 } else { 1 }, y, out) + SECTION_GAP;
        }
        if counts[0] != 0 {
            y = section(0, y, out) + SECTION_GAP;
        }
        if counts[3] != 0 {
            section(3, y, out);
        }
        self.overflowed = cx.overflowed;
        self.rows_drawn = cx.rows;
        if !fits {
            scroll_bar(d, list_top, items, cx.item, self.scroll, out);
        }
        if self.scroll > items - 1 {
            self.scroll = (items - 1).max(0);
        }
    }

    /// The order the rows of `team` are drawn in: the server's.
    #[cfg(test)]
    pub(super) fn rows_for_team(&self, team: i32) -> Vec<&ScoreEntry> {
        self.entries.iter().filter(|e| e.team == team).collect()
    }
}

impl Default for Scoreboard {
    fn default() -> Self {
        Self::new()
    }
}

/// The fonts, the screen and the board's alpha.
struct Draw<'a> {
    fonts: &'a UiFonts,
    v: &'a Virtual,
    alpha: f32,
    style: &'a Style,
}

impl Draw<'_> {
    fn font(&self, scale: f32) -> &super::font::Font {
        self.fonts.pick(Slot::Default, scale, 480.0 * self.v.scale)
    }

    /// Text trap 0x36 in font 0 at a baseline.
    #[allow(clippy::too_many_arguments)]
    fn text(&self, s: &str, x: f32, y: f32, scale: f32, rgb: [f32; 3], out: &mut Vec<HudQuad>) {
        let rgba = [rgb[0], rgb[1], rgb[2], self.alpha];
        menu_text_rgba(self.font(scale), s, (x, y), scale, rgba, self.v, out);
    }

    /// [`Self::text`] in `UI_DrawText`'s style 0, no shadow: CoD2's row
    /// cell once it shrank below 0.16 (cod2-hud.md 13).
    #[allow(clippy::too_many_arguments)]
    fn text_plain(
        &self,
        s: &str,
        x: f32,
        y: f32,
        scale: f32,
        rgb: [f32; 3],
        out: &mut Vec<HudQuad>,
    ) {
        let start = out.len();
        self.text(s, x, y, scale, rgb, out);
        // The layout alternates shadow and glyph quads.
        let mut i = 0;
        out.retain(|_| {
            i += 1;
            i <= start || (i - start).is_multiple_of(2)
        });
    }

    /// Trap 0x34: whole units.
    fn width(&self, s: &str, scale: f32) -> f32 {
        text_width(self.font(scale), s, scale).floor()
    }

    /// Trap 0x35: whole units.
    fn height(&self, scale: f32) -> f32 {
        text_height(self.font(scale), scale).floor()
    }

    fn fill(&self, rect: [f32; 4], rgb: [f32; 3], a: f32, tex: &str, out: &mut Vec<HudQuad>) {
        let [x, y, w, h] = rect;
        let rgba = [rgb[0], rgb[1], rgb[2], a];
        out.push(self.v.quad(x, y, w, h, rgba, tex));
    }
}

const WHITE: [f32; 3] = [1.0; 3];

/// The scroll position and the room left as the blocks draw.
struct Cursor {
    width: f32,
    /// Banners and rows met so far, drawn or scrolled off.
    item: i32,
    scroll: i32,
    overflowed: bool,
    /// Player rows drawn.
    rows: usize,
}

impl Cursor {
    /// Whether the next item takes `h` at `y`: false for one scrolled off or
    /// past the bottom, and nothing more draws once one did not fit.
    fn take(&mut self, y: f32, h: f32) -> bool {
        if self.overflowed {
            return false;
        }
        if self.item < self.scroll {
            self.item += 1;
            return false;
        }
        if y + h > BOTTOM {
            self.overflowed = true;
            return false;
        }
        self.item += 1;
        true
    }

    /// `0x3002a970`: the team's banner, its name and player count in the
    /// team's colour, and for axis and allies the team's score and average
    /// ping in their columns. Returns the y the rows start at.
    #[allow(clippy::too_many_arguments)]
    fn banner(
        &mut self,
        d: &Draw,
        b: &BoardView,
        team: i32,
        players: usize,
        total: i32,
        ping: i32,
        y: f32,
        out: &mut Vec<HudQuad>,
    ) -> f32 {
        if !self.take(y, BANNER_STEP) {
            return y;
        }
        let cvar = |n: &str| (b.cvar)(n).unwrap_or_default();
        let key = if players == 1 {
            "CGAME_SB_PLAYER"
        } else {
            "CGAME_SB_PLAYERS"
        };
        let count = b.loc.message(&format!("{key}\u{15}{players}"));
        let (banner, title) = match team {
            0 => (cvar("g_ScoresBanner_None"), count),
            1 | 2 => {
                let side = if team == 1 { "Axis" } else { "Allies" };
                let name = cvar(&format!("g_TeamName_{side}"));
                (
                    cvar(&format!("g_ScoresBanner_{side}")),
                    format!("{} ({count})", b.loc.message(&name)),
                )
            }
            _ => (
                cvar("g_ScoresBanner_Spectators"),
                format!("{} ({count})", b.loc.message("CGAME_SPECTATORS")),
            ),
        };
        d.fill(
            [LEFT, y, self.width, d.style.banner_h],
            WHITE,
            d.alpha,
            &banner,
            out,
        );
        let color = team_color(team, b.cvar);
        let ty = y + d.style.banner_text_y;
        d.text(&title, 133.0, ty, 0.32, color, out);
        if matches!(team, 1 | 2) {
            let mut x = LEFT;
            for &(col, share, _, right) in d.style.columns {
                let s = match col {
                    Column::Score => Some(total.to_string()),
                    Column::Ping => Some(ping.to_string()),
                    _ => None,
                };
                if let Some(s) = s {
                    let off = if right {
                        self.width * share - d.width(&s, 0.32)
                    } else {
                        0.0
                    };
                    d.text(&s, x + off, ty, 0.32, color, out);
                }
                x += self.width * share;
            }
        }
        y + d.style.banner_step
    }

    /// `0x3002b0d0`: the team's rows in the server's order, every other one
    /// shaded and the local client's lit in the team's colour.
    fn rows(
        &mut self,
        d: &Draw,
        b: &BoardView,
        team: i32,
        rows: &[(ScoreEntry, Option<String>)],
        mut y: f32,
        out: &mut Vec<HudQuad>,
    ) -> f32 {
        let color = team_color(team, b.cvar);
        let mut shade = false;
        for (e, name) in rows {
            let Some(name) = name.as_ref().filter(|_| e.team == team) else {
                continue;
            };
            y = self.row(d, b, e, name, color, shade, y, out);
            self.rows += 1;
            shade = !shade;
        }
        y
    }

    /// `0x3002ace0`.
    #[allow(clippy::too_many_arguments)]
    fn row(
        &mut self,
        d: &Draw,
        b: &BoardView,
        e: &ScoreEntry,
        name: &str,
        color: [f32; 3],
        shade: bool,
        y: f32,
        out: &mut Vec<HudQuad>,
    ) -> f32 {
        if !self.take(y, ROW_H) {
            return y;
        }
        let rect = [LEFT, y, self.width, ROW_H];
        if b.own_client == Some(e.client) {
            d.fill(rect, color, d.alpha * 0.2, "white", out);
        } else if shade {
            d.fill(rect, [0.0; 3], d.alpha * 0.15, "black", out);
        }
        let spectator = e.team == SPECTATOR_TEAM;
        let mut x = LEFT;
        for &(col, share, _, right) in d.style.columns {
            let w = self.width * share;
            let s = match col {
                Column::Icon => {
                    if let Some(icon) = (b.status_icon)(e.status_icon) {
                        let off = if right { w - 13.0 } else { 0.0 };
                        d.fill([x + off, y, 13.0, 13.0], WHITE, d.alpha, &icon, out);
                    }
                    String::new()
                }
                // 13 x 13 at the column's left (`CG_DrawClientScore`,
                // Mac 0x1c51b2).
                Column::Voice => {
                    if let Some(icon) = (b.voice)(e.client) {
                        d.fill([x, y, 13.0, 13.0], WHITE, d.alpha, icon, out);
                    }
                    String::new()
                }
                Column::Name => name.to_string(),
                Column::Score if !spectator => e.score.to_string(),
                Column::Deaths if !spectator => e.deaths.to_string(),
                Column::Ping => e.ping.to_string(),
                _ => String::new(),
            };
            if !s.is_empty() {
                let (s, scale) = if d.style.shrink_cells {
                    let scale = shrink(d, &s, w, 0.22);
                    (s, scale)
                } else {
                    (clip(d, &s, w, 0.22), 0.22)
                };
                let off = if right { w - d.width(&s, scale) } else { 0.0 };
                let ty = y + d.style.row_text_y;
                // CoD2 shadows a cell (style 3) only at 0.16 or more
                // (`CG_DrawClientScore`, Mac 0x1c51b2).
                if d.style.shrink_cells && scale < SHADOW_MIN_SCALE {
                    d.text_plain(&s, x + off, ty, scale, WHITE, out);
                } else {
                    d.text(&s, x + off, ty, scale, WHITE, out);
                }
            }
            x += w;
        }
        y + ROW_H
    }
}

/// The smallest scale a CoD2 row cell keeps its shadow at (0x326398).
const SHADOW_MIN_SCALE: f32 = 0.16;

/// CoD2's `CG_DrawClientScore` (Mac 0x1c51b2): the scale less 0.02 at a
/// time until `s` fits `w`.
fn shrink(d: &Draw, s: &str, w: f32, mut scale: f32) -> f32 {
    while d.width(s, scale) > w && scale > 0.02 {
        scale -= 0.02;
    }
    scale
}

/// `s` cut a character at a time from the end until it fits `w`.
fn clip(d: &Draw, s: &str, w: f32, scale: f32) -> String {
    let mut chars: Vec<char> = s.chars().collect();
    while !chars.is_empty() && d.width(&chars.iter().collect::<String>(), scale) > w {
        chars.pop();
    }
    chars.into_iter().collect()
}

/// `0x3002a810`: `g_TeamColor_Axis` or `_Allies` read `"%f %f %f"` and
/// clamped to 0..1; white for any other team.
fn team_color(team: i32, cvar: &dyn Fn(&str) -> Option<String>) -> [f32; 3] {
    let name = match team {
        1 => "g_TeamColor_Axis",
        2 => "g_TeamColor_Allies",
        _ => return WHITE,
    };
    let s = cvar(name).unwrap_or_default();
    let mut c = [0.0; 3];
    for (slot, f) in c.iter_mut().zip(s.split_whitespace()) {
        *slot = f.parse::<f32>().unwrap_or(0.0);
    }
    c.map(|v| v.clamp(0.0, 1.0))
}

/// `0x30029ac0`: the dark panel and its white frame, the game type and the
/// map over the top rule, the host name and the address under the bottom
/// one.
fn frame(d: &Draw, b: &BoardView, out: &mut Vec<HudQuad>) {
    d.fill(
        [120.0, 22.0, 400.0, 430.0],
        [0.0; 3],
        d.alpha * d.style.panel_alpha,
        "black",
        out,
    );
    for rect in [
        [123.0, 25.0, 394.0, 2.0],
        [123.0, 447.0, 394.0, 2.0],
        [123.0, 27.0, 2.0, 420.0],
        [515.0, 27.0, 2.0, 420.0],
        [125.0, 51.0, 390.0, 1.0],
        [125.0, 432.0, 390.0, 1.0],
    ] {
        d.fill(rect, WHITE, d.alpha * 0.1, "white", out);
    }
    // The pair shrinks until it fits 386 or the scale reaches 0.075.
    let fit = |left: &str, right: &str, start: f32, step: f32, fixed_left: bool| {
        let mut s = start;
        let total = |s: f32| {
            let l = d.width(left, if fixed_left { start } else { s });
            l + 4.0 + d.width(right, s)
        };
        while total(s) > 386.0 && s > 0.075 {
            s -= step;
        }
        s
    };
    let title = b.loc.message(b.gametype);
    let s = fit(&title, b.map, 0.41, 0.025, d.style.title_fixed_left);
    let base = |row: f32, h: f32, scale: f32| row - (h - d.height(scale)) * 0.5;
    d.text(&title, LEFT, base(51.0, 24.0, 0.41), 0.41, WHITE, out);
    let x = 511.0 - (d.width(b.map, s) + 4.0);
    d.text(b.map, x, base(51.0, 24.0, s), s, WHITE, out);

    let address = b.address.unwrap_or("");
    let s = fit(address, b.hostname, 0.2, 0.01, false);
    let y = base(447.0, 14.0, s);
    d.text(b.hostname, LEFT, y, s, WHITE, out);
    let x = 511.0 - (d.width(address, s) + 4.0);
    d.text(address, x, y, s, WHITE, out);
}

/// `0x3002a1e0`: `cg_objectiveText` through the `[{command}]` pass
/// ("scoreboard objective info"), wrapped at 375 units, then a rule.
/// Returns the y under it; `top` when there is no objective.
fn objective(d: &Draw, b: &BoardView, top: f32, out: &mut Vec<HudQuad>) -> f32 {
    let raw = (b.cvar)("cg_objectiveText").unwrap_or_default();
    if raw.is_empty() {
        return top;
    }
    const SCALE: f32 = 0.24;
    let text: Vec<char> = super::bind_keys(&b.loc.message(&raw), b.bound_key)
        .chars()
        .collect();
    let mut y = top + 4.0;
    let line = |a: usize, z: usize, y: f32, out: &mut Vec<HudQuad>| {
        let s: String = text[a..z].iter().collect();
        d.text(&s, LEFT, y + d.style.objective_text_y, SCALE, WHITE, out);
    };
    let newline = |i: usize| match text.get(i) {
        Some('\n') => Some(1),
        Some('\\') if text.get(i + 1) == Some(&'n') => Some(2),
        _ => None,
    };
    let mut start = 0;
    while start < text.len() {
        if text[start] == ' ' {
            start += 1;
            continue;
        }
        if let Some(n) = newline(start) {
            start += n;
            y += ROW_H;
            continue;
        }
        let mut end = start;
        let mut space = None;
        loop {
            let next = end + 1;
            if next >= text.len() {
                line(start, next, y, out);
                start = next;
                break;
            }
            if let Some(n) = newline(next) {
                line(start, next, y, out);
                start = next + n;
                break;
            }
            let s: String = text[start..next].iter().collect();
            if d.width(&s, SCALE) < 375.0 {
                end = next;
                if text[next] == ' ' {
                    space = Some(next);
                }
                continue;
            }
            match space {
                Some(sp) => {
                    line(start, sp, y, out);
                    start = sp + 1;
                }
                None => {
                    line(start, end, y, out);
                    start = end.max(start + 1);
                }
            }
            break;
        }
        y += ROW_H;
    }
    y += 2.0;
    d.fill([125.0, y, 390.0, 1.0], WHITE, d.alpha * 0.1, "white", out);
    y + 1.0
}

/// `0x3002a650`: the localized column headers. Returns the y under them.
fn headers(d: &Draw, b: &BoardView, y: f32, width: f32, out: &mut Vec<HudQuad>) -> f32 {
    let mut x = LEFT;
    for &(_, share, key, right) in d.style.columns {
        if !key.is_empty() {
            let s = b.loc.get(key).unwrap_or(key);
            let off = if right {
                width * share - d.width(s, 0.3)
            } else {
                0.0
            };
            d.text(s, x + off, y + d.style.header_text_y, 0.3, WHITE, out);
        }
        x += width * share;
    }
    y + 14.0
}

/// `0x3002b170`: the track beside a board that does not fit, its thumb, and
/// the page keys' arrows.
fn scroll_bar(d: &Draw, top: f32, items: i32, reached: i32, scroll: i32, out: &mut Vec<HudQuad>) {
    let h = BOTTOM - top - 1.0;
    d.fill([505.0, top, 8.0, h], [0.0; 3], d.alpha * 0.5, "black", out);
    let mut thumb_y = top + 1.0;
    let mut thumb_h = h - 2.0;
    if scroll != 0 && items != 0 {
        thumb_y += scroll as f32 / items as f32 * thumb_h;
    }
    if reached - scroll > 1 && items > 1 {
        thumb_h *= (reached - scroll) as f32 / items as f32;
    }
    d.fill(
        [506.0, thumb_y, 6.0, thumb_h],
        WHITE,
        d.alpha * 0.25,
        "white",
        out,
    );
    if scroll > 0 {
        d.fill(
            [521.0, top, 16.0, 16.0],
            WHITE,
            d.alpha,
            "hudScoreboardScroll_UpArrow",
            out,
        );
        let key = "hudScoreboardScroll_UpKey";
        d.fill([521.0, top + 18.0, 16.0, 16.0], WHITE, d.alpha, key, out);
    }
    if reached < items {
        let arrow = "hudScoreboardScroll_DownArrow";
        d.fill([521.0, 415.0, 16.0, 16.0], WHITE, d.alpha, arrow, out);
        let key = "hudScoreboardScroll_DownKey";
        d.fill([521.0, 397.0, 16.0, 16.0], WHITE, d.alpha, key, out);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn toks(s: &str) -> Vec<String> {
        s.split(' ').map(String::from).collect()
    }

    #[test]
    fn parses_grammar_derived_scores_command() {
        // Synthetic, from doc section 3's grammar.
        let tokens = toks("b 3 5 8 0 12 45 100 0 1 9 60 250 3 2 3 80 400 0");
        let msg = parse_scores(&tokens).unwrap();
        assert!(!msg.truncated);
        assert_eq!(msg.axis, 5);
        assert_eq!(msg.allies, 8);
        assert_eq!(msg.entries.len(), 3);
        assert_eq!(msg.entries[0].client, 0);
        assert_eq!(msg.entries[0].score, 12);
        assert_eq!(msg.entries[0].ping, 45);
        assert_eq!(msg.entries[0].deaths, 100);
        assert_eq!(msg.entries[1].client, 1);
        assert_eq!(msg.entries[1].status_icon, 3);
        assert_eq!(msg.entries[2].client, 2);
    }

    #[test]
    fn tolerates_a_trailing_partial_row() {
        // Same fixture as net::tests::unhandled_server_commands_surface_as_tokens:
        // numRows says 2, only one full row plus a stray "80" is present.
        let tokens = toks("b 2 0 1 0 5 60 1 12 80");
        let msg = parse_scores(&tokens).unwrap();
        assert!(msg.truncated);
        assert_eq!(msg.entries.len(), 1);
        assert_eq!(msg.entries[0].client, 0);
        assert_eq!(msg.entries[0].score, 5);
    }

    #[test]
    fn malformed_scores_keeps_last_table() {
        let mut b = Scoreboard::new();
        b.on_server_command(&toks("b 1 5 8 0 10 40 100 0"));
        let before = b.entries.len();
        b.on_server_command(&toks("b garbage"));
        assert_eq!(b.entries.len(), before);
    }

    #[test]
    fn rows_keep_the_servers_order() {
        let mut b = Scoreboard::new();
        b.on_server_command(&toks("b 2 0 0 1 5 40 0 0 2 9 60 0 0"));
        let rows = b.rows_for_team(0);
        assert_eq!(rows.iter().map(|e| e.client).collect::<Vec<_>>(), [1, 2]);
    }

    #[test]
    fn request_timer_fires_immediately_then_every_interval() {
        let mut b = Scoreboard::new();
        assert!(b.press(0.0));
        assert!(!b.due(1.0));
        assert!(!b.press(1.0));
        assert!(!b.due(1.0));
        assert!(b.due(2.0));
    }

    /// A press that sends drops the last table; one inside the interval
    /// keeps it.
    #[test]
    fn a_sending_press_drops_the_old_rows() {
        let mut b = Scoreboard::new();
        b.on_server_command(&toks("b 1 0 0 1 0 0 0 0"));
        assert!(b.press(0.0));
        assert!(b.entries.is_empty());
        b.on_server_command(&toks("b 1 0 0 1 0 0 0 0"));
        b.release(0.5);
        assert!(!b.press(1.0));
        assert_eq!(b.entries.len(), 1);
        b.release(1.5);
        assert!(b.press(5.0));
        assert!(b.entries.is_empty());
    }

    #[test]
    fn hidden_board_never_requests_but_intermission_does() {
        let mut b = Scoreboard::new();
        assert!(!b.due(0.0));
        assert!(!b.due(1000.0));
        b.intermission = true;
        assert!(b.due(1000.0));
    }

    #[test]
    fn the_board_fades_for_100_ms_after_release() {
        let mut b = Scoreboard::new();
        assert_eq!(b.alpha(0.0), None);
        b.press(1.0);
        assert_eq!(b.alpha(1.0), Some(1.0));
        b.release(2.0);
        let a = b.alpha(2.05).unwrap();
        assert!((a - 0.5).abs() < 0.02, "{a}");
        assert_eq!(b.alpha(2.11), None);
        b.press(3.0);
        b.hide();
        assert_eq!(b.alpha(3.01), None);
    }

    #[test]
    fn team_colour_reads_the_cvar_and_clamps() {
        let cvar = |n: &str| match n {
            "g_TeamColor_Axis" => Some("1 0.5 2".to_string()),
            _ => None,
        };
        assert_eq!(team_color(1, &cvar), [1.0, 0.5, 1.0]);
        assert_eq!(team_color(2, &cvar), [0.0; 3]);
        assert_eq!(team_color(3, &cvar), WHITE);
    }

    struct Fixture {
        fonts: UiFonts,
        loc: Localized,
    }

    fn fixture() -> Option<Fixture> {
        let fs = vcod_common::testing::game_fs()?;
        Some(Fixture {
            fonts: UiFonts::load(&fs).unwrap(),
            loc: Localized::load(&fs),
        })
    }

    fn cvars(n: &str) -> Option<String> {
        Some(
            match n {
                "g_ScoresBanner_Axis" => "gfx/hud/hud@mpflag_german.tga",
                "g_ScoresBanner_Allies" => "gfx/hud/hud@mpflag_american.tga",
                "g_ScoresBanner_None" => "gfx/hud/hud@mpflag_none.tga",
                "g_ScoresBanner_Spectators" => "gfx/hud/hud@mpflag_spectator.tga",
                "g_TeamName_Axis" => "MPSCRIPT_GERMAN",
                "g_TeamName_Allies" => "MPSCRIPT_AMERICAN",
                "g_TeamColor_Axis" => ".6 .6 .6",
                "g_TeamColor_Allies" => ".25 .75 .25",
                _ => return None,
            }
            .to_string(),
        )
    }

    /// Clients 1 and 3 axis, 2 allies, 4 a spectator; client 9 has no info.
    fn client(c: u32) -> Option<(String, i32)> {
        let team = match c {
            1 | 3 => 1,
            2 => 2,
            4 => 3,
            _ => return None,
        };
        Some((format!("player{c}"), team))
    }

    fn draw(board: &mut Scoreboard, fx: &Fixture, own: Option<u32>) -> Vec<HudQuad> {
        let v = Virtual::new((640.0, 480.0));
        let view = BoardView {
            now: 0.0,
            own_client: own,
            client: &client,
            cvar: &cvars,
            bound_key: &|_| None,
            loc: &fx.loc,
            gametype: "TDM_TEAM_DEATHMATCH",
            map: "mp_carentan",
            hostname: "vcod",
            address: Some("127.0.0.1:28960"),
            status_icon: &|_| None,
            voice: &|_| None,
        };
        let mut out = Vec::new();
        assert!(board.build(&view, &fx.fonts, &v, &mut out));
        out
    }

    fn quad<'a>(out: &'a [HudQuad], tex: &str) -> Vec<&'a HudQuad> {
        out.iter().filter(|q| q.texture == tex).collect()
    }

    /// The leading team's banner first, each at x 129 and 382 wide, 24
    /// apart plus a row of 12 per player and a gap of 4; the local row
    /// lit in its team's colour.
    #[test]
    fn blocks_follow_the_cgames_layout() {
        let Some(fx) = fixture() else {
            return;
        };
        let mut board = Scoreboard::new();
        board.press(0.0);
        // Allies lead 7 to 3: axis 1 and 3, allies 2, spectator 4, and 9
        // with no info (counted under none, never drawn).
        board.on_server_command(&toks(
            "b 5 3 7 2 7 50 1 0 1 3 40 2 0 3 0 60 4 0 4 0 30 0 0 9 0 20 0 0",
        ));
        let out = draw(&mut board, &fx, Some(3));
        let top = |tex: &str| quad(&out, tex)[0].verts[0];
        let allies = top("gfx/hud/hud@mpflag_american.tga");
        let axis = top("gfx/hud/hud@mpflag_german.tga");
        let none = top("gfx/hud/hud@mpflag_none.tga");
        let spec = top("gfx/hud/hud@mpflag_spectator.tga");
        assert_eq!(allies[0], LEFT);
        let banner = quad(&out, "gfx/hud/hud@mpflag_american.tga")[0];
        assert_eq!(banner.verts[1][0] - banner.verts[0][0], WIDTH);
        assert_eq!(banner.verts[2][1] - banner.verts[0][1], COD1.banner_h);
        // No objective: the headers' 4 + 14 under the table's top.
        assert_eq!(allies[1], TOP + 4.0 + 14.0);
        assert_eq!(axis[1], allies[1] + 24.0 + 12.0 + 4.0);
        assert_eq!(none[1], axis[1] + 24.0 + 2.0 * 12.0 + 4.0);
        assert_eq!(spec[1], none[1] + 24.0 + 4.0);
        // Client 3 is the second axis row: lit white-textured in grey.
        let lit = quad(&out, "white")
            .into_iter()
            .find(|q| q.verts[0][1] == axis[1] + 24.0 + 12.0)
            .expect("own row lit");
        assert_eq!(lit.rgba, [0.6, 0.6, 0.6, 0.2]);
        assert_eq!(lit.verts[1][0] - lit.verts[0][0], WIDTH);
        // The axis block's first row is plain, its second the lit one, so
        // no black shade sits in the axis block.
        assert!(
            quad(&out, "black")
                .iter()
                .all(|q| q.verts[0][1] != axis[1] + 24.0 + 12.0)
        );
    }

    /// A tie keeps the last order, which starts with allies.
    #[test]
    fn a_tie_keeps_the_last_leader() {
        let Some(fx) = fixture() else {
            return;
        };
        let mut board = Scoreboard::new();
        board.press(0.0);
        let y = |out: &[HudQuad], tex: &str| quad(out, tex)[0].verts[0][1];
        board.on_server_command(&toks("b 2 0 0 1 0 0 0 0 2 0 0 0 0"));
        let out = draw(&mut board, &fx, None);
        let (ger, us) = (
            "gfx/hud/hud@mpflag_german.tga",
            "gfx/hud/hud@mpflag_american.tga",
        );
        assert!(y(&out, us) < y(&out, ger));
        board.on_server_command(&toks("b 2 1 0 1 0 0 0 0 2 0 0 0 0"));
        let out = draw(&mut board, &fx, None);
        assert!(y(&out, ger) < y(&out, us));
        board.on_server_command(&toks("b 2 4 4 1 0 0 0 0 2 0 0 0 0"));
        let out = draw(&mut board, &fx, None);
        assert!(y(&out, ger) < y(&out, us));
    }

    /// More rows than the panel holds: the table narrows to 374, a scroll
    /// bar and the down arrow show, and a scroll down moves the list.
    #[test]
    fn an_overfull_board_scrolls() {
        let Some(fx) = fixture() else {
            return;
        };
        let mut board = Scoreboard::new();
        board.press(0.0);
        let mut cmd = String::from("b 40 0 0");
        for i in 0..40 {
            cmd += &format!(" {} 0 0 0 0", [1, 2][i % 2]);
        }
        board.on_server_command(&toks(&cmd));
        let out = draw(&mut board, &fx, None);
        let banner = quad(&out, "gfx/hud/hud@mpflag_american.tga")[0];
        assert_eq!(banner.verts[1][0] - banner.verts[0][0], WIDTH_SCROLLED);
        assert_eq!(quad(&out, "hudScoreboardScroll_DownArrow").len(), 1);
        assert!(quad(&out, "hudScoreboardScroll_UpArrow").is_empty());
        board.scroll_down(3);
        let out = draw(&mut board, &fx, None);
        assert_eq!(quad(&out, "hudScoreboardScroll_UpArrow").len(), 1);
        // The allies banner and two rows scrolled off the top.
        assert!(quad(&out, "gfx/hud/hud@mpflag_american.tga").is_empty());
        board.scroll_up(3);
        let out = draw(&mut board, &fx, None);
        assert!(quad(&out, "hudScoreboardScroll_UpArrow").is_empty());
    }

    /// `weapnext` scrolls a held board up and `weapprev` down; with the
    /// board down they cycle weapons as usual.
    #[test]
    fn weapon_cycle_scrolls_a_held_board() {
        let Some(fx) = fixture() else {
            return;
        };
        let mut board = Scoreboard::new();
        assert!(!board.take_cycle(false, 3), "board down");
        board.press(0.0);
        let mut cmd = String::from("b 40 0 0");
        for i in 0..40 {
            cmd += &format!(" {} 0 0 0 0", [1, 2][i % 2]);
        }
        board.on_server_command(&toks(&cmd));
        draw(&mut board, &fx, None);
        assert!(board.take_cycle(false, 3));
        assert_eq!(board.scroll, 3);
        assert!(board.take_cycle(true, 2));
        assert_eq!(board.scroll, 1);
        assert!(board.take_cycle(true, 2));
        assert_eq!(board.scroll, 0);
        board.release(1.0);
        assert!(!board.take_cycle(false, 3));
    }

    /// A board that fits never scrolls.
    #[test]
    fn a_board_that_fits_does_not_scroll() {
        let Some(fx) = fixture() else {
            return;
        };
        let mut board = Scoreboard::new();
        board.press(0.0);
        board.on_server_command(&toks("b 1 0 0 1 0 0 0 0"));
        draw(&mut board, &fx, None);
        board.scroll_down(3);
        assert_eq!(board.scroll, 0);
    }
}
