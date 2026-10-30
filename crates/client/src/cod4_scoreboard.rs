//! A watched CoD4 session's scoreboard: the last `b` laid out as 1.7's
//! `CG_DrawScoreboard` lays it out (docs/research/cod4-hud.md 7), drawn
//! while `+scores` is held and at intermission. [`layout`] places it on
//! the 640x480 grid (left and top aligned) as text and pictures;
//! `cod4_hud` puts those on screen.

use vcod_cod4::hud::scoreboard::{self as sb, Column};
use vcod_cod4::hud::{CS_STATUS_ICONS, Scores};
use vcod_cod4::net::parse::Snapshot;

use crate::hud::font::{self, Font};

/// One piece of the board on the grid: text whose baseline is `y`, or a
/// picture by its material.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Piece {
    Text {
        text: String,
        x: f32,
        y: f32,
        scale: f32,
        rgba: [f32; 4],
    },
    Pic {
        material: String,
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        rgba: [f32; 4],
    },
}

/// What the layout reads besides the scores.
pub(crate) struct Inputs<'a> {
    pub(crate) scores: &'a Scores,
    pub(crate) snap: &'a Snapshot,
    /// Our own client (`cg.clientNum`).
    pub(crate) own: u32,
    /// The grid's width (`virtualViewableMax - Min`).
    pub(crate) grid_width: f32,
    pub(crate) fade: f32,
    pub(crate) host_name: String,
    /// Configstring `i`.
    pub(crate) configstring: &'a dyn Fn(u16) -> String,
    /// A server dvar off the mirror.
    pub(crate) dvar: &'a dyn Fn(&str) -> Option<String>,
    /// A localized string by key.
    pub(crate) localize: &'a dyn Fn(&str) -> String,
    /// The font `UI_GetFontHandle` picks for a scale.
    pub(crate) font: &'a dyn Fn(f32) -> &'a Font,
    /// The rank icon of (rank, prestige), the rank's display level.
    pub(crate) rank_icon: &'a dyn Fn(i32, i32) -> Option<String>,
    pub(crate) rank_level: &'a dyn Fn(i32) -> Option<String>,
    /// The talking column's material for a client: `voice_off` muted,
    /// `voice_on` talking, else none (`LCT_TALKING_ICON`).
    pub(crate) talk_icon: &'a dyn Fn(u32) -> Option<&'static str>,
    /// `cg.scoresTop`: the first line drawn, 1 at the top.
    pub(crate) top: i32,
    pub(crate) ping_graph: PingGraph,
}

/// `UI_TextWidth` on the grid.
fn width(f: &Font, t: &str, scale: f32) -> f32 {
    font::measure(f, t, scale / f.unit_scale())
}

/// `UI_TextHeight`: the font's 48 units at `scale`, truncated.
fn height(scale: f32) -> f32 {
    (48.0 * scale) as i32 as f32
}

fn client_field(c: &vcod_cod4::net::delta::ClientState, name: &str) -> u32 {
    vcod_cod4::net::fields::CLIENT_STATE
        .iter()
        .find(|f| f.name == name)
        .map_or(0, |f| c.0[f.word()])
}

/// A colour dvar's `r g b a`.
fn colour(v: Option<String>) -> [f32; 4] {
    let mut c = [1.0; 4];
    for (i, n) in v.unwrap_or_default().split_whitespace().take(4).enumerate() {
        c[i] = n.parse().unwrap_or(1.0);
    }
    c
}

/// `cg_scoreboardPingGraph` and its bars' dvars, 1.7's defaults
/// (`iw3mp.exe` 0x449b40.., cod4-hud.md 7.1): off, four bars of 100 ms.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct PingGraph {
    pub(crate) enabled: bool,
    pub(crate) max_bars: i32,
    pub(crate) interval: i32,
    pub(crate) high: [f32; 4],
    pub(crate) med: [f32; 4],
    pub(crate) low: [f32; 4],
    pub(crate) bg: [f32; 4],
}

impl Default for PingGraph {
    fn default() -> Self {
        PingGraph {
            enabled: false,
            max_bars: 4,
            interval: 100,
            high: [0.8, 0.0, 0.0, 1.0],
            med: [0.8, 0.8, 0.0, 1.0],
            low: [0.0, 0.75, 0.0, 1.0],
            bg: [0.25, 0.25, 0.25, 0.5],
        }
    }
}

/// The laid-out board and what the draw leaves in `cg` for the scroll
/// keys: `scoresBottom` (the last line drawn) and `scoresOffBottom`.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Board {
    pub(crate) pieces: Vec<Piece>,
    pub(crate) bottom: i32,
    pub(crate) off_bottom: bool,
}

/// A score row with the client state the snapshot holds for it and that
/// client's team.
struct Entry<'a> {
    row: &'a vcod_cod4::hud::ScoreRow,
    client: &'a vcod_cod4::net::delta::ClientState,
    team: u32,
}

/// The rows whose client the snapshot knows (`infoValid`), in `b`'s order.
fn entries<'a>(i: &Inputs<'a>) -> Vec<Entry<'a>> {
    let known = |n: u32| i.snap.clients.iter().find(|c| c.0[0] == n);
    i.scores
        .rows
        .iter()
        .filter_map(|row| {
            let client = known(row.client as u32)?;
            Some(Entry {
                row,
                client,
                team: client_field(client, "team"),
            })
        })
        .collect()
}

/// Players per team slot 0..=3; a team number past 3 counts as 3.
fn head_count(entries: &[Entry]) -> [usize; 4] {
    entries.iter().fold([0; 4], |mut n, e| {
        n[e.team.min(3) as usize] += 1;
        n
    })
}

/// Our own team off the snapshot's client states, 0 when absent.
fn own_team(i: &Inputs) -> u32 {
    let own = i.snap.clients.iter().find(|c| c.0[0] == i.own);
    own.map_or(0, |c| client_field(c, "team"))
}

/// The backdrop's fixed points on the grid (cod4-hud.md 7, 7.1): it is
/// `cg_scoreboardWidth` by `cg_scoreboardHeight`, centred; the list runs
/// 9 in from each side.
struct Frame {
    left: f32,
    top: f32,
}

impl Frame {
    fn new(grid_width: f32) -> Frame {
        Frame {
            left: ((grid_width - sb::WIDTH) / 2.0).max(0.0),
            top: ((480.0 - sb::HEIGHT) / 2.0).max(0.0),
        }
    }

    fn list_x(&self) -> f32 {
        self.left + 9.0
    }

    fn list_width() -> f32 {
        sb::WIDTH - 18.0
    }

    /// The column headers' banner top: 30 down, past an item line and
    /// 15 more.
    fn headers_y(&self) -> f32 {
        self.top + 30.0 + sb::ITEM_HEIGHT + 4.0 + 15.0
    }

    /// No line may end past this (the backdrop's foot less 20).
    fn foot(&self) -> f32 {
        self.top + sb::HEIGHT - 20.0
    }
}

/// The lines a screen of the board holds: the list's span (the backdrop
/// less its 50 units of header and footer) past the header line, at an
/// item and its gap a line.
fn lines_on_screen() -> i32 {
    let span = sb::HEIGHT - 50.0;
    ((span - sb::BANNER_HEIGHT - sb::ITEM_HEIGHT - 4.0) / (sb::ITEM_HEIGHT + 4.0)) as i32
}

/// The board's lines (`CG_ScoreboardTotalLines`, cod4-hud.md 7.1): a
/// line per score row and one per team banner with players.
pub(crate) fn total_lines(i: &Inputs) -> i32 {
    let banners = head_count(&entries(i)).iter().filter(|&&n| n > 0).count();
    (i.scores.rows.len() + banners) as i32
}

/// The first line drawn when the board opens (cod4-hud.md 7.1): 1, or
/// for a player whose row lies past the first screen half a screen above
/// that row; 1 for a spectator.
pub(crate) fn centred_top(i: &Inputs) -> i32 {
    let team = own_team(i);
    if team == 3 {
        return 1;
    }
    let ahead = entries(i)
        .iter()
        .filter(|e| e.team == team)
        .take_while(|e| e.row.client as u32 != i.own)
        .count() as i32;
    let own_line = ahead + 1;
    let screen = lines_on_screen();
    if own_line <= screen {
        1
    } else {
        own_line - screen / 2 + 1
    }
}

/// Which lines draw (cod4-hud.md 7.1): lines before `first` are counted
/// and skipped, the first that would end past `foot` marks the board as
/// running off the bottom and nothing draws after it.
struct Scroll {
    first: i32,
    next: i32,
    foot: f32,
    overflow: bool,
}

impl Scroll {
    /// Counts a line `h` tall at `y`; whether it is drawn.
    fn admit(&mut self, y: f32, h: f32) -> bool {
        let above = self.next < self.first;
        let fits = y + h <= self.foot;
        match (self.overflow, above, fits) {
            (true, _, _) => false,
            (false, true, _) => {
                self.next += 1;
                false
            }
            (false, false, true) => {
                self.next += 1;
                true
            }
            (false, false, false) => {
                self.overflow = true;
                false
            }
        }
    }

    /// The last line drawn (`cg.scoresBottom`).
    fn last(&self) -> i32 {
        self.next - 1
    }
}

/// Collects pieces, every alpha under the board's fade.
struct Canvas<'a, 'i> {
    i: &'a Inputs<'i>,
    pieces: Vec<Piece>,
}

impl Canvas<'_, '_> {
    fn white(&self) -> [f32; 4] {
        [1.0, 1.0, 1.0, self.i.fade]
    }

    fn text(&mut self, text: impl Into<String>, [x, y]: [f32; 2], scale: f32, rgba: [f32; 4]) {
        self.pieces.push(Piece::Text {
            text: text.into(),
            x,
            y,
            scale,
            rgba,
        });
    }

    fn pic(&mut self, material: impl Into<String>, [x, y, w, h]: [f32; 4], rgba: [f32; 4]) {
        self.pieces.push(Piece::Pic {
            material: material.into(),
            x,
            y,
            w,
            h,
            rgba,
        });
    }

    fn measure(&self, t: &str, scale: f32) -> f32 {
        width((self.i.font)(scale), t, scale)
    }

    /// A list cell's text (cod4-hud.md 7): shrunk by 0.025 until it fits
    /// `w`, placed by `align` (0 left, 1 centred, 2 right less 4) and set
    /// on the item line at `y` by `cg_scoreboardTextOffset`; `drop` lowers
    /// it further.
    #[allow(clippy::too_many_arguments)]
    fn cell(
        &mut self,
        t: &str,
        [x, y, w]: [f32; 3],
        align: i32,
        scale: f32,
        drop: f32,
        rgba: [f32; 4],
    ) {
        let mut s = scale;
        while s > 0.025 && self.measure(t, s) > w {
            s -= 0.025;
        }
        let spare = w - self.measure(t, s);
        let dx = match align {
            1 => spare * 0.5,
            2 => spare - 4.0,
            _ => 0.0,
        };
        let baseline = (height(s) + sb::ITEM_HEIGHT) * sb::TEXT_OFFSET + (drop + y);
        self.text(t, [x + dx, baseline], s, rgba);
    }
}

/// The board at 1.7's dvars from line `i.top` (`cg.scoresTop`, at least
/// 1): the backdrop's host name, then the score list
/// (`CG_DrawScoreboard_ScoresList`, `iw3mp.exe` 0x4496d0), cod4-hud.md 7
/// and 7.1. An empty `b` draws the host name alone.
pub(crate) fn layout(i: &Inputs) -> Board {
    let frame = Frame::new(i.grid_width);
    let mut c = Canvas {
        i,
        pieces: Vec::new(),
    };
    host_name(&mut c, &frame);
    if i.scores.rows.is_empty() {
        return Board {
            pieces: c.pieces,
            ..Board::default()
        };
    }
    let entries = entries(i);
    let mut scroll = Scroll {
        first: i.top.max(1),
        next: 1,
        foot: frame.foot(),
        overflow: false,
    };
    column_headers(&mut c, &frame);
    // At the top the first banner shares the headers' line (0x449739).
    let mut y = frame.headers_y();
    if scroll.first > 1 {
        y += sb::BANNER_HEIGHT + 4.0;
    }
    for team in team_order(&head_count(&entries), own_team(i)) {
        y = draw_team(&mut c, &frame, &mut scroll, &entries, team, y) + 4.0;
    }
    if scroll.first > 1 || scroll.overflow {
        scroll_bar(&mut c, &frame, &scroll, total_lines(i));
    }
    Board {
        pieces: c.pieces,
        bottom: scroll.last(),
        off_bottom: scroll.overflow,
    }
}

/// The host name at the backdrop's foot, its scale stepped down by 0.01
/// (to no less than 0.075) until it fits the backdrop less 14.
fn host_name(c: &mut Canvas, frame: &Frame) {
    let name = c.i.host_name.as_str();
    let fits = |c: &Canvas, s: f32| c.measure(name, s) + 4.0 <= sb::WIDTH - 14.0;
    let mut scale = sb::LIST_SCALE;
    while scale > 0.075 && !fits(c, scale) {
        scale -= 0.01;
    }
    let y = frame.top + sb::HEIGHT - 5.0 - (14.0 - height(scale)) * 0.5;
    let white = c.white();
    c.text(name, [frame.list_x(), y], scale, white);
}

/// Each titled column's localized header, centred in its column on the
/// headers' line at 0.85 of `cg_scoreboardHeaderFontScale`.
fn column_headers(c: &mut Canvas, frame: &Frame) {
    let scale = sb::HEADER_FONT_SCALE * 0.85;
    let baseline = frame.headers_y() + sb::BANNER_HEIGHT;
    let white = c.white();
    let mut x = frame.list_x();
    for (_, share, key, _) in sb::COLUMNS {
        let w = share * Frame::list_width();
        if !key.is_empty() {
            let title = (c.i.localize)(key);
            let dx = (w - c.measure(&title, scale)) * 0.5;
            c.text(title, [x + dx, baseline], scale, white);
        }
        x += w;
    }
}

/// The teams in draw order: ours first of the two playing teams (axis
/// for team 1, else allies), both whenever either has a player; then the
/// free players and the spectators where they have any.
fn team_order(players: &[usize; 4], own: u32) -> Vec<u32> {
    let mut order = Vec::with_capacity(4);
    if players[1] > 0 || players[2] > 0 {
        order.extend(if own == 1 { [1, 2] } else { [2, 1] });
    }
    order.extend([0u32, 3].into_iter().filter(|&t| players[t as usize] > 0));
    order
}

/// The `g_TeamIcon_*` and team name dvars' suffix (`CG_TeamColor`).
fn team_suffix(team: u32) -> &'static str {
    ["Free", "Axis", "Allies", "Spectator"][team.min(3) as usize]
}

/// A team's banner line and then its rows, each line through `scroll`;
/// returns the `y` under the last line drawn.
fn draw_team(
    c: &mut Canvas,
    frame: &Frame,
    scroll: &mut Scroll,
    entries: &[Entry],
    team: u32,
    mut y: f32,
) -> f32 {
    let members: Vec<&Entry> = entries.iter().filter(|e| e.team == team).collect();
    if scroll.admit(y, sb::BANNER_HEIGHT) {
        banner(c, frame, team, members.len(), y);
        y += sb::BANNER_HEIGHT + 4.0;
    }
    let mut tint = colour((c.i.dvar)(&format!("g_ScoresColor_{}", team_suffix(team))));
    tint[3] = c.i.fade;
    for e in members {
        if scroll.admit(y, sb::ITEM_HEIGHT) {
            score_line(c, frame, e, team, tint, y);
            y += sb::ITEM_HEIGHT + 4.0;
        }
    }
    y
}

/// A team's banner: its icon (a banner-high square), its name and
/// `( n )`, 8 apart.
fn banner(c: &mut Canvas, frame: &Frame, team: u32, players: usize, y: f32) {
    let i = c.i;
    let suffix = team_suffix(team);
    let icon = (i.dvar)(&format!("g_TeamIcon_{suffix}")).filter(|m| !m.is_empty());
    let name = match team {
        0 => String::new(),
        3 => (i.localize)("CGAME_SPECTATORS"),
        _ => (i.localize)(&(i.dvar)(&format!("g_TeamName_{suffix}")).unwrap_or_default()),
    };
    let h = sb::BANNER_HEIGHT;
    let white = c.white();
    let mut x = frame.list_x();
    if let Some(icon) = icon {
        c.pic(icon, [x, y, h, h], white);
        x += h + 8.0;
    }
    let count_x = x + c.measure(&name, sb::LIST_SCALE) + 8.0;
    c.text(name, [x, y + h], sb::LIST_SCALE, white);
    c.text(
        format!("( {players} )"),
        [count_x, y + h],
        sb::LIST_SCALE,
        white,
    );
}

/// `CG_DrawClientScore`: the row's backing in the team's colour at half
/// alpha, then its columns; our own row's text in `cg_scoreboardMyColor`.
/// A spectator's row leaves the score columns empty.
fn score_line(c: &mut Canvas, frame: &Frame, e: &Entry, team: u32, tint: [f32; 4], y: f32) {
    let i = c.i;
    let ih = sb::ITEM_HEIGHT;
    let list_w = Frame::list_width();
    let x0 = frame.list_x();
    c.pic(
        "white",
        [x0, y, list_w, ih],
        [tint[0], tint[1], tint[2], tint[3] * 0.5],
    );
    let alpha = tint[3];
    let plain = [1.0, 1.0, 1.0, alpha];
    let text_rgba = if e.row.client as u32 == i.own {
        [sb::MY_COLOR[0], sb::MY_COLOR[1], sb::MY_COLOR[2], alpha]
    } else {
        plain
    };
    let r = e.row;
    let mut x = x0;
    let playing = team != 3;
    for (kind, share, _, align) in sb::COLUMNS {
        let w = share * list_w;
        let text = match kind {
            Column::Name => Some(crate::play::cod4::client_name(e.client)),
            Column::Ping => Some(r.ping.to_string()),
            Column::Score if playing => Some(r.score.to_string()),
            Column::Kills if playing => Some(r.kills.to_string()),
            Column::Assists if playing => Some(r.assists.to_string()),
            Column::Deaths if playing => Some(r.deaths.to_string()),
            _ => None,
        };
        let cell = [x, y, w];
        match (text, kind) {
            (Some(t), _) => c.cell(&t, cell, align, sb::LIST_SCALE, 0.0, text_rgba),
            (None, Column::StatusIcon) => {
                let icon = (1..=8)
                    .contains(&r.status_icon)
                    .then(|| (i.configstring)(CS_STATUS_ICONS + r.status_icon as u16))
                    .filter(|m| !m.is_empty());
                if let Some(m) = icon {
                    let dx = if align == 2 { w - ih } else { 0.0 };
                    c.pic(m, [x + dx, y, ih, ih], plain);
                }
            }
            (None, Column::TalkingIcon) => {
                if let Some(m) = (i.talk_icon)(r.client as u32) {
                    c.pic(m, [x, y, ih, ih], plain);
                }
            }
            (None, Column::RankIcon) => rank(c, e.client, cell, align, plain),
            _ => {}
        }
        x += w;
    }
    if i.ping_graph.enabled {
        let area = [x, y, list_w * sb::PING_WIDTH, ih];
        ping_bars(c, &i.ping_graph, r.ping, area);
    }
}

/// The rank column: the icon of the client's rank and prestige, and past
/// it the rank's level at `cg_scoreboardRankFontScale`, a quarter of
/// that font's height lower than a cell's text.
fn rank(
    c: &mut Canvas,
    client: &vcod_cod4::net::delta::ClientState,
    [x, y, w]: [f32; 3],
    align: i32,
    rgba: [f32; 4],
) {
    let i = c.i;
    let level = client_field(client, "rank") as i32;
    let prestige = client_field(client, "prestige") as i32;
    let Some(icon) = (i.rank_icon)(level, prestige) else {
        return;
    };
    let ih = sb::ITEM_HEIGHT;
    c.pic(icon, [x, y, ih, ih], rgba);
    if let Some(shown) = (i.rank_level)(level) {
        let s = sb::RANK_FONT_SCALE;
        c.cell(&shown, [x + ih, y, w], align, s, height(s) * 0.25, rgba);
    }
}

/// `CG_DrawClientPing` (`iw3mp.exe` 0x4487b0, cod4-hud.md 7.1): a backing
/// 8 right of the columns, then the bars a ping earns, rising left to
/// right, all one colour picked along high, medium and low by how many.
fn ping_bars(c: &mut Canvas, g: &PingGraph, ping: i32, [x, y, w, h]: [f32; 4]) {
    let fade = c.i.fade;
    let faded = |mut rgba: [f32; 4]| {
        rgba[3] *= fade;
        rgba
    };
    c.pic("white", [x + 8.0 - 1.0, y, w + 2.0, h], faded(g.bg));
    let most = g.max_bars.max(1);
    let earned = (most - ping / g.interval.max(1)).max(1);
    let rgba = faded(bar_colour(g, earned, most));
    let bar_w = (w / most as f32 - 1.0).max(1.0);
    let mut bar_x = x + 8.0;
    for n in 1..=earned {
        let bar_h = h * sb::PING_HEIGHT * n as f32 / most as f32;
        c.pic("white", [bar_x, y + h - bar_h, bar_w, bar_h], rgba);
        bar_x += bar_w + 1.0;
    }
}

/// The bars' colour: below half of `most` from the high ping's colour
/// toward the medium's, from half on from the medium's toward the low's.
fn bar_colour(g: &PingGraph, bars: i32, most: i32) -> [f32; 4] {
    let half = most / 2;
    let span = half.max(1) as f32;
    let (a, b, t) = if bars < half {
        (g.high, g.med, bars as f32 / span)
    } else {
        (g.med, g.low, (bars - half) as f32 / span)
    };
    std::array::from_fn(|k| a[k] + (b[k] - a[k]) * t)
}

/// `CG_DrawScrollbar` (0x4491d0, cod4-hud.md 7.1): an 8-wide track right
/// of the list from under the headers to the foot, a thumb over the lines
/// shown out of `total`, and the up and down arrows with their keys for
/// whichever way the board can scroll.
fn scroll_bar(c: &mut Canvas, frame: &Frame, scroll: &Scroll, total: i32) {
    let fade = c.i.fade;
    let tone = |a: f32| [1.0, 1.0, 1.0, fade * a];
    let x = frame.list_x() + Frame::list_width() * (sb::PING_WIDTH + 1.0) + 12.0;
    let track_y = frame.headers_y() + sb::BANNER_HEIGHT;
    let track_h = frame.foot() - track_y - 1.0;
    c.pic("black", [x, track_y, 8.0, track_h], tone(0.5));
    let inner = track_h - 2.0;
    let (thumb_y, thumb_h) = if total > 0 {
        let total = total as f32;
        let shown = (scroll.last() - scroll.first + 1) as f32;
        (
            track_y + 1.0 + (scroll.first - 1) as f32 / total * inner,
            inner * (shown / total),
        )
    } else {
        (track_y + 1.0, inner)
    };
    c.pic("white", [x + 1.0, thumb_y, 6.0, thumb_h], tone(0.25));
    let arrow_x = x + 14.0;
    let low = frame.foot() - 17.0;
    let arrows = [
        (scroll.first > 1, "hudscoreboardscroll_uparrow", track_y),
        (
            scroll.first > 1,
            "hudscoreboardscroll_upkey",
            track_y + 18.0,
        ),
        (scroll.overflow, "hudscoreboardscroll_downarrow", low),
        (scroll.overflow, "hudscoreboardscroll_downkey", low - 18.0),
    ];
    for (shown, material, y) in arrows {
        if shown {
            c.pic(material, [arrow_x, y, 16.0, 16.0], tone(1.0));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fmt::Write;
    use vcod_cod4::hud::ScoreRow;
    use vcod_cod4::net::delta::ClientState;
    use vcod_cod4::net::fields::CLIENT_STATE;

    /// [`crate::hud::hudelem::tests::test_font`] with advances varied by
    /// byte, so widths depend on the text.
    fn font() -> Font {
        let mut f = crate::hud::hudelem::tests::test_font();
        for (i, g) in f.glyphs.iter_mut().enumerate() {
            g.advance = 12.0 + (i % 7) as f32;
        }
        f
    }

    fn set(c: &mut ClientState, name: &str, v: u32) {
        let f = CLIENT_STATE.iter().find(|f| f.name == name).unwrap();
        c.0[f.word()] = v;
    }

    /// A client state: number, team, rank, prestige and a name.
    fn client(n: u32, team: u32, rank: u32, name: &str) -> ClientState {
        let mut c = ClientState::default();
        c.0[0] = n;
        set(&mut c, "team", team);
        set(&mut c, "rank", rank);
        set(&mut c, "prestige", rank % 3);
        let mut b = name.as_bytes().to_vec();
        b.resize(16, 0);
        for k in 0..4 {
            let w = u32::from_le_bytes(b[4 * k..4 * k + 4].try_into().unwrap());
            set(&mut c, &format!("name[{}]", 4 * k), w);
        }
        c
    }

    fn snapshot(clients: Vec<ClientState>) -> Snapshot {
        Snapshot {
            message_num: 0,
            server_time: 0,
            delta_num: 0,
            base: None,
            snap_flags: 0,
            ps: Default::default(),
            entities: Vec::new(),
            clients,
            wire: Default::default(),
        }
    }

    fn row(client: i32, score: i32, ping: i32, status_icon: i32) -> ScoreRow {
        ScoreRow {
            client,
            score,
            ping,
            deaths: client % 4,
            status_icon,
            kills: score / 10,
            assists: client % 3,
        }
    }

    /// Every piece on a line of its own, floats as `{:?}` prints them.
    fn render(b: &Board) -> String {
        let mut s = format!("bottom {} off {}\n", b.bottom, b.off_bottom);
        for p in &b.pieces {
            match p {
                Piece::Text {
                    text,
                    x,
                    y,
                    scale,
                    rgba,
                } => writeln!(s, "T {text:?} {x:?} {y:?} {scale:?} {rgba:?}"),
                Piece::Pic {
                    material,
                    x,
                    y,
                    w,
                    h,
                    rgba,
                } => writeln!(s, "P {material} {x:?} {y:?} {w:?} {h:?} {rgba:?}"),
            }
            .unwrap();
        }
        s
    }

    /// Lays out `scores` for clients `clients` with our own `own`, from
    /// line `top` (0: centred as the board opens), and renders it with
    /// `total_lines` and the top used.
    fn board(
        scores: &Scores,
        clients: Vec<ClientState>,
        own: u32,
        top: i32,
        graph: bool,
    ) -> String {
        let f = font();
        let snap = snapshot(clients);
        let configstring = |i: u16| match i - CS_STATUS_ICONS {
            1 => "hud_status_dead".to_string(),
            2 => "hud_status_connecting".to_string(),
            _ => String::new(),
        };
        let dvar = |name: &str| -> Option<String> {
            match name {
                "g_TeamIcon_Axis" => Some("faction_128_arab".into()),
                "g_TeamIcon_Allies" => Some("faction_128_usmc".into()),
                "g_TeamIcon_Spectator" => Some(String::new()),
                "g_TeamName_Axis" => Some("MPUI_OPFOR_SHORT".into()),
                "g_TeamName_Allies" => Some("MPUI_MARINES_SHORT".into()),
                "g_ScoresColor_Axis" => Some("0.65 0.57 0.41 0".into()),
                "g_ScoresColor_Allies" => Some("0.6 0.639216 0.690196 0".into()),
                "g_ScoresColor_Free" => Some("0.76 0.78 0.1".into()),
                _ => None,
            }
        };
        let localize = |k: &str| format!("<{k}>");
        let font_of = |_: f32| &f;
        let rank_icon =
            |rank: i32, prestige: i32| (rank > 0).then(|| format!("rank_{rank}_{prestige}"));
        let rank_level = |rank: i32| (rank != 2).then(|| (rank + 1).to_string());
        let talk_icon = |c: u32| match c % 3 {
            1 => Some("voice_on"),
            2 => Some("voice_off"),
            _ => None,
        };
        let ping_graph = PingGraph {
            enabled: graph,
            ..PingGraph::default()
        };
        let mut i = Inputs {
            scores,
            snap: &snap,
            own,
            grid_width: 853.333,
            fade: 0.75,
            host_name: "^1vcod ^7test server with a rather long host name here".into(),
            configstring: &configstring,
            dvar: &dvar,
            localize: &localize,
            font: &font_of,
            rank_icon: &rank_icon,
            rank_level: &rank_level,
            talk_icon: &talk_icon,
            top,
            ping_graph,
        };
        if top <= 0 {
            i.top = centred_top(&i);
        }
        format!(
            "top {} total {}\n{}",
            i.top,
            total_lines(&i),
            render(&layout(&i))
        )
    }

    fn short_scores() -> (Scores, Vec<ClientState>) {
        let scores = Scores {
            team_scores: [10, 0],
            score_limit: 750,
            rows: vec![
                row(0, 10, 18, 0),
                row(1, 0, 145, 1),
                row(2, 30, 999, 2),
                row(3, 5, -1, 9),
                // no client state: skipped
                row(9, 0, 50, 0),
            ],
        };
        let clients = vec![
            client(0, 2, 5, "^2Shooter"),
            client(1, 1, 0, "Target with a very long name"),
            client(2, 3, 2, "spec"),
            client(3, 0, 54, "free"),
        ];
        (scores, clients)
    }

    fn long_scores() -> (Scores, Vec<ClientState>) {
        let scores = Scores {
            team_scores: [120, 90],
            score_limit: 750,
            rows: (0..20).map(|n| row(n, 100 - 5 * n, 30 * n, 0)).collect(),
        };
        let clients = (0..20)
            .map(|n| client(n as u32, 1 + n as u32 % 2, n as u32 * 3, &format!("bot{n}")))
            .collect();
        (scores, clients)
    }

    /// Fails with the whole new text when `actual` moved off the pin.
    fn pin(tag: &str, actual: &str, expected: &str) {
        assert!(actual == expected, "{tag} moved:\n{actual}");
    }

    #[test]
    fn an_empty_board_pins() {
        let s = board(&Scores::default(), Vec::new(), 0, 1, false);
        pin(
            "empty",
            &s,
            r##"top 1 total 0
bottom 0 off false
T "^1vcod ^7test server with a rather long host name here" 185.6665 450.0 0.20000006 [1.0, 1.0, 1.0, 0.75]
"##,
        );
    }

    #[test]
    fn a_short_board_pins() {
        let (scores, clients) = short_scores();
        pin(
            "short_own",
            &board(&scores, clients.clone(), 0, 0, false),
            r##"top 1 total 9
bottom 8 off false
T "^1vcod ^7test server with a rather long host name here" 185.6665 450.0 0.20000006 [1.0, 1.0, 1.0, 0.75]
T "<CGAME_SB_SCORE>" 336.97278 124.5 0.2975 [1.0, 1.0, 1.0, 0.75]
T "<CGAME_SB_KILLS>" 383.83405 124.5 0.2975 [1.0, 1.0, 1.0, 0.75]
T "<CGAME_SB_ASSISTS>" 419.98526 124.5 0.2975 [1.0, 1.0, 1.0, 0.75]
T "<CGAME_SB_DEATHS>" 477.1103 124.5 0.2975 [1.0, 1.0, 1.0, 0.75]
T "<CGAME_SB_PING>" 541.82153 124.5 0.2975 [1.0, 1.0, 1.0, 0.75]
P faction_128_usmc 185.6665 89.5 35.0 35.0 [1.0, 1.0, 1.0, 0.75]
T "<MPUI_MARINES_SHORT>" 228.6665 124.5 0.35 [1.0, 1.0, 1.0, 0.75]
T "( 1 )" 554.81647 124.5 0.35 [1.0, 1.0, 1.0, 0.75]
P white 185.6665 128.5 482.0 18.0 [0.6, 0.639216, 0.690196, 0.375]
P rank_5_2 185.6665 128.5 18.0 18.0 [1.0, 1.0, 1.0, 0.75]
T "6" 203.6665 146.5 0.25 [1.0, 1.0, 1.0, 0.75]
T "^2Shooter" 233.86652 145.5 0.35 [1.0, 0.8, 0.4, 0.75]
T "10" 439.36655 145.5 0.35 [1.0, 0.8, 0.4, 0.75]
T "1" 506.46655 145.5 0.35 [1.0, 0.8, 0.4, 0.75]
T "0" 548.3665 145.5 0.35 [1.0, 0.8, 0.4, 0.75]
T "0" 596.5665 145.5 0.35 [1.0, 0.8, 0.4, 0.75]
T "18" 638.46655 145.5 0.35 [1.0, 0.8, 0.4, 0.75]
P faction_128_arab 185.6665 154.5 35.0 35.0 [1.0, 1.0, 1.0, 0.75]
T "<MPUI_OPFOR_SHORT>" 228.6665 189.5 0.35 [1.0, 1.0, 1.0, 0.75]
T "( 1 )" 518.06647 189.5 0.35 [1.0, 1.0, 1.0, 0.75]
P white 185.6665 193.5 482.0 18.0 [0.65, 0.57, 0.41, 0.375]
P hud_status_dead 215.86652 193.5 18.0 18.0 [1.0, 1.0, 1.0, 0.75]
T "Target with a ve" 233.86652 207.5 0.22499996 [1.0, 1.0, 1.0, 0.75]
P voice_on 402.56653 193.5 18.0 18.0 [1.0, 1.0, 1.0, 0.75]
T "0" 451.96652 210.5 0.35 [1.0, 1.0, 1.0, 0.75]
T "0" 500.16653 210.5 0.35 [1.0, 1.0, 1.0, 0.75]
T "1" 554.6665 210.5 0.35 [1.0, 1.0, 1.0, 0.75]
T "1" 602.8665 210.5 0.35 [1.0, 1.0, 1.0, 0.75]
T "145" 618.51654 210.5 0.35 [1.0, 1.0, 1.0, 0.75]
T "" 185.6665 254.5 0.35 [1.0, 1.0, 1.0, 0.75]
T "( 1 )" 193.6665 254.5 0.35 [1.0, 1.0, 1.0, 0.75]
P white 185.6665 258.5 482.0 18.0 [0.76, 0.78, 0.1, 0.375]
P rank_54_0 185.6665 258.5 18.0 18.0 [1.0, 1.0, 1.0, 0.75]
T "55" 203.6665 276.5 0.25 [1.0, 1.0, 1.0, 0.75]
T "free" 233.86652 275.5 0.35 [1.0, 1.0, 1.0, 0.75]
T "5" 454.06653 275.5 0.35 [1.0, 1.0, 1.0, 0.75]
T "0" 500.16653 275.5 0.35 [1.0, 1.0, 1.0, 0.75]
T "0" 548.3665 275.5 0.35 [1.0, 1.0, 1.0, 0.75]
T "3" 600.76654 275.5 0.35 [1.0, 1.0, 1.0, 0.75]
T "-1" 635.3165 275.5 0.35 [1.0, 1.0, 1.0, 0.75]
T "<CGAME_SPECTATORS>" 185.6665 319.5 0.35 [1.0, 1.0, 1.0, 0.75]
T "( 1 )" 484.51648 319.5 0.35 [1.0, 1.0, 1.0, 0.75]
P white 185.6665 323.5 482.0 18.0 [1.0, 1.0, 1.0, 0.375]
P rank_2_2 185.6665 323.5 18.0 18.0 [1.0, 1.0, 1.0, 0.75]
P hud_status_connecting 215.86652 323.5 18.0 18.0 [1.0, 1.0, 1.0, 0.75]
T "spec" 233.86652 340.5 0.35 [1.0, 1.0, 1.0, 0.75]
P voice_off 402.56653 323.5 18.0 18.0 [1.0, 1.0, 1.0, 0.75]
T "999" 622.71655 340.5 0.35 [1.0, 1.0, 1.0, 0.75]
"##,
        );
        pin(
            "short_graph",
            &board(&scores, clients, 1, 1, true),
            r##"top 1 total 9
bottom 8 off false
T "^1vcod ^7test server with a rather long host name here" 185.6665 450.0 0.20000006 [1.0, 1.0, 1.0, 0.75]
T "<CGAME_SB_SCORE>" 336.97278 124.5 0.2975 [1.0, 1.0, 1.0, 0.75]
T "<CGAME_SB_KILLS>" 383.83405 124.5 0.2975 [1.0, 1.0, 1.0, 0.75]
T "<CGAME_SB_ASSISTS>" 419.98526 124.5 0.2975 [1.0, 1.0, 1.0, 0.75]
T "<CGAME_SB_DEATHS>" 477.1103 124.5 0.2975 [1.0, 1.0, 1.0, 0.75]
T "<CGAME_SB_PING>" 541.82153 124.5 0.2975 [1.0, 1.0, 1.0, 0.75]
P faction_128_arab 185.6665 89.5 35.0 35.0 [1.0, 1.0, 1.0, 0.75]
T "<MPUI_OPFOR_SHORT>" 228.6665 124.5 0.35 [1.0, 1.0, 1.0, 0.75]
T "( 1 )" 518.06647 124.5 0.35 [1.0, 1.0, 1.0, 0.75]
P white 185.6665 128.5 482.0 18.0 [0.65, 0.57, 0.41, 0.375]
P hud_status_dead 215.86652 128.5 18.0 18.0 [1.0, 1.0, 1.0, 0.75]
T "Target with a ve" 233.86652 142.5 0.22499996 [1.0, 0.8, 0.4, 0.75]
P voice_on 402.56653 128.5 18.0 18.0 [1.0, 1.0, 1.0, 0.75]
T "0" 451.96652 145.5 0.35 [1.0, 0.8, 0.4, 0.75]
T "0" 500.16653 145.5 0.35 [1.0, 0.8, 0.4, 0.75]
T "1" 554.6665 145.5 0.35 [1.0, 0.8, 0.4, 0.75]
T "1" 602.8665 145.5 0.35 [1.0, 0.8, 0.4, 0.75]
T "145" 618.51654 145.5 0.35 [1.0, 0.8, 0.4, 0.75]
P white 674.66656 128.5 19.352 18.0 [0.25, 0.25, 0.25, 0.375]
P white 675.66656 143.35 3.3379998 3.1499999 [0.4, 0.775, 0.0, 0.75]
P white 680.0046 140.2 3.3379998 6.2999997 [0.4, 0.775, 0.0, 0.75]
P white 684.3426 137.05 3.3379998 9.45 [0.4, 0.775, 0.0, 0.75]
P faction_128_usmc 185.6665 154.5 35.0 35.0 [1.0, 1.0, 1.0, 0.75]
T "<MPUI_MARINES_SHORT>" 228.6665 189.5 0.35 [1.0, 1.0, 1.0, 0.75]
T "( 1 )" 554.81647 189.5 0.35 [1.0, 1.0, 1.0, 0.75]
P white 185.6665 193.5 482.0 18.0 [0.6, 0.639216, 0.690196, 0.375]
P rank_5_2 185.6665 193.5 18.0 18.0 [1.0, 1.0, 1.0, 0.75]
T "6" 203.6665 211.5 0.25 [1.0, 1.0, 1.0, 0.75]
T "^2Shooter" 233.86652 210.5 0.35 [1.0, 1.0, 1.0, 0.75]
T "10" 439.36655 210.5 0.35 [1.0, 1.0, 1.0, 0.75]
T "1" 506.46655 210.5 0.35 [1.0, 1.0, 1.0, 0.75]
T "0" 548.3665 210.5 0.35 [1.0, 1.0, 1.0, 0.75]
T "0" 596.5665 210.5 0.35 [1.0, 1.0, 1.0, 0.75]
T "18" 638.46655 210.5 0.35 [1.0, 1.0, 1.0, 0.75]
P white 674.66656 193.5 19.352 18.0 [0.25, 0.25, 0.25, 0.375]
P white 675.66656 208.35 3.3379998 3.1499999 [0.0, 0.75, 0.0, 0.75]
P white 680.0046 205.2 3.3379998 6.2999997 [0.0, 0.75, 0.0, 0.75]
P white 684.3426 202.05 3.3379998 9.45 [0.0, 0.75, 0.0, 0.75]
P white 688.6806 198.9 3.3379998 12.599999 [0.0, 0.75, 0.0, 0.75]
T "" 185.6665 254.5 0.35 [1.0, 1.0, 1.0, 0.75]
T "( 1 )" 193.6665 254.5 0.35 [1.0, 1.0, 1.0, 0.75]
P white 185.6665 258.5 482.0 18.0 [0.76, 0.78, 0.1, 0.375]
P rank_54_0 185.6665 258.5 18.0 18.0 [1.0, 1.0, 1.0, 0.75]
T "55" 203.6665 276.5 0.25 [1.0, 1.0, 1.0, 0.75]
T "free" 233.86652 275.5 0.35 [1.0, 1.0, 1.0, 0.75]
T "5" 454.06653 275.5 0.35 [1.0, 1.0, 1.0, 0.75]
T "0" 500.16653 275.5 0.35 [1.0, 1.0, 1.0, 0.75]
T "0" 548.3665 275.5 0.35 [1.0, 1.0, 1.0, 0.75]
T "3" 600.76654 275.5 0.35 [1.0, 1.0, 1.0, 0.75]
T "-1" 635.3165 275.5 0.35 [1.0, 1.0, 1.0, 0.75]
P white 674.66656 258.5 19.352 18.0 [0.25, 0.25, 0.25, 0.375]
P white 675.66656 273.35 3.3379998 3.1499999 [0.0, 0.75, 0.0, 0.75]
P white 680.0046 270.2 3.3379998 6.2999997 [0.0, 0.75, 0.0, 0.75]
P white 684.3426 267.05 3.3379998 9.45 [0.0, 0.75, 0.0, 0.75]
P white 688.6806 263.9 3.3379998 12.599999 [0.0, 0.75, 0.0, 0.75]
T "<CGAME_SPECTATORS>" 185.6665 319.5 0.35 [1.0, 1.0, 1.0, 0.75]
T "( 1 )" 484.51648 319.5 0.35 [1.0, 1.0, 1.0, 0.75]
P white 185.6665 323.5 482.0 18.0 [1.0, 1.0, 1.0, 0.375]
P rank_2_2 185.6665 323.5 18.0 18.0 [1.0, 1.0, 1.0, 0.75]
P hud_status_connecting 215.86652 323.5 18.0 18.0 [1.0, 1.0, 1.0, 0.75]
T "spec" 233.86652 340.5 0.35 [1.0, 1.0, 1.0, 0.75]
P voice_off 402.56653 323.5 18.0 18.0 [1.0, 1.0, 1.0, 0.75]
T "999" 622.71655 340.5 0.35 [1.0, 1.0, 1.0, 0.75]
P white 674.66656 323.5 19.352 18.0 [0.25, 0.25, 0.25, 0.375]
P white 675.66656 338.35 3.3379998 3.1499999 [0.8, 0.4, 0.0, 0.75]
"##,
        );
    }

    #[test]
    fn a_long_board_pins() {
        let (scores, clients) = long_scores();
        pin(
            "long_centred",
            &board(&scores, clients.clone(), 17, 0, false),
            r##"top 1 total 22
bottom 14 off true
T "^1vcod ^7test server with a rather long host name here" 185.6665 450.0 0.20000006 [1.0, 1.0, 1.0, 0.75]
T "<CGAME_SB_SCORE>" 336.97278 124.5 0.2975 [1.0, 1.0, 1.0, 0.75]
T "<CGAME_SB_KILLS>" 383.83405 124.5 0.2975 [1.0, 1.0, 1.0, 0.75]
T "<CGAME_SB_ASSISTS>" 419.98526 124.5 0.2975 [1.0, 1.0, 1.0, 0.75]
T "<CGAME_SB_DEATHS>" 477.1103 124.5 0.2975 [1.0, 1.0, 1.0, 0.75]
T "<CGAME_SB_PING>" 541.82153 124.5 0.2975 [1.0, 1.0, 1.0, 0.75]
P faction_128_usmc 185.6665 89.5 35.0 35.0 [1.0, 1.0, 1.0, 0.75]
T "<MPUI_MARINES_SHORT>" 228.6665 124.5 0.35 [1.0, 1.0, 1.0, 0.75]
T "( 10 )" 554.81647 124.5 0.35 [1.0, 1.0, 1.0, 0.75]
P white 185.6665 128.5 482.0 18.0 [0.6, 0.639216, 0.690196, 0.375]
P rank_3_0 185.6665 128.5 18.0 18.0 [1.0, 1.0, 1.0, 0.75]
T "4" 203.6665 146.5 0.25 [1.0, 1.0, 1.0, 0.75]
T "bot1" 233.86652 145.5 0.35 [1.0, 1.0, 1.0, 0.75]
P voice_on 402.56653 128.5 18.0 18.0 [1.0, 1.0, 1.0, 0.75]
T "95" 440.41653 145.5 0.35 [1.0, 1.0, 1.0, 0.75]
T "9" 505.41656 145.5 0.35 [1.0, 1.0, 1.0, 0.75]
T "1" 554.6665 145.5 0.35 [1.0, 1.0, 1.0, 0.75]
T "1" 602.8665 145.5 0.35 [1.0, 1.0, 1.0, 0.75]
T "30" 630.0665 145.5 0.35 [1.0, 1.0, 1.0, 0.75]
P white 185.6665 150.5 482.0 18.0 [0.6, 0.639216, 0.690196, 0.375]
P rank_9_0 185.6665 150.5 18.0 18.0 [1.0, 1.0, 1.0, 0.75]
T "10" 203.6665 168.5 0.25 [1.0, 1.0, 1.0, 0.75]
T "bot3" 233.86652 167.5 0.35 [1.0, 1.0, 1.0, 0.75]
T "85" 441.46655 167.5 0.35 [1.0, 1.0, 1.0, 0.75]
T "8" 506.46655 167.5 0.35 [1.0, 1.0, 1.0, 0.75]
T "0" 548.3665 167.5 0.35 [1.0, 1.0, 1.0, 0.75]
T "3" 600.76654 167.5 0.35 [1.0, 1.0, 1.0, 0.75]
T "90" 631.1166 167.5 0.35 [1.0, 1.0, 1.0, 0.75]
P white 185.6665 172.5 482.0 18.0 [0.6, 0.639216, 0.690196, 0.375]
P rank_15_0 185.6665 172.5 18.0 18.0 [1.0, 1.0, 1.0, 0.75]
T "16" 203.6665 190.5 0.25 [1.0, 1.0, 1.0, 0.75]
T "bot5" 233.86652 189.5 0.35 [1.0, 1.0, 1.0, 0.75]
P voice_off 402.56653 172.5 18.0 18.0 [1.0, 1.0, 1.0, 0.75]
T "75" 435.16653 189.5 0.35 [1.0, 1.0, 1.0, 0.75]
T "7" 500.16653 189.5 0.35 [1.0, 1.0, 1.0, 0.75]
T "2" 553.6165 189.5 0.35 [1.0, 1.0, 1.0, 0.75]
T "1" 602.8665 189.5 0.35 [1.0, 1.0, 1.0, 0.75]
T "150" 618.8165 189.0 0.325 [1.0, 1.0, 1.0, 0.75]
P white 185.6665 194.5 482.0 18.0 [0.6, 0.639216, 0.690196, 0.375]
P rank_21_0 185.6665 194.5 18.0 18.0 [1.0, 1.0, 1.0, 0.75]
T "22" 203.6665 212.5 0.25 [1.0, 1.0, 1.0, 0.75]
T "bot7" 233.86652 211.5 0.35 [1.0, 1.0, 1.0, 0.75]
P voice_on 402.56653 194.5 18.0 18.0 [1.0, 1.0, 1.0, 0.75]
T "65" 436.21655 211.5 0.35 [1.0, 1.0, 1.0, 0.75]
T "6" 501.21655 211.5 0.35 [1.0, 1.0, 1.0, 0.75]
T "1" 554.6665 211.5 0.35 [1.0, 1.0, 1.0, 0.75]
T "3" 600.76654 211.5 0.35 [1.0, 1.0, 1.0, 0.75]
T "210" 618.51654 211.5 0.35 [1.0, 1.0, 1.0, 0.75]
P white 185.6665 216.5 482.0 18.0 [0.6, 0.639216, 0.690196, 0.375]
P rank_27_0 185.6665 216.5 18.0 18.0 [1.0, 1.0, 1.0, 0.75]
T "28" 203.6665 234.5 0.25 [1.0, 1.0, 1.0, 0.75]
T "bot9" 233.86652 233.5 0.35 [1.0, 1.0, 1.0, 0.75]
T "55" 437.26654 233.5 0.35 [1.0, 1.0, 1.0, 0.75]
T "5" 502.26654 233.5 0.35 [1.0, 1.0, 1.0, 0.75]
T "0" 548.3665 233.5 0.35 [1.0, 1.0, 1.0, 0.75]
T "1" 602.8665 233.5 0.35 [1.0, 1.0, 1.0, 0.75]
T "270" 615.89154 233.0 0.325 [1.0, 1.0, 1.0, 0.75]
P white 185.6665 238.5 482.0 18.0 [0.6, 0.639216, 0.690196, 0.375]
P rank_33_0 185.6665 238.5 18.0 18.0 [1.0, 1.0, 1.0, 0.75]
T "34" 203.6665 256.5 0.25 [1.0, 1.0, 1.0, 0.75]
T "bot11" 233.86652 255.5 0.35 [1.0, 1.0, 1.0, 0.75]
P voice_off 402.56653 238.5 18.0 18.0 [1.0, 1.0, 1.0, 0.75]
T "45" 438.31653 255.5 0.35 [1.0, 1.0, 1.0, 0.75]
T "4" 503.31656 255.5 0.35 [1.0, 1.0, 1.0, 0.75]
T "2" 553.6165 255.5 0.35 [1.0, 1.0, 1.0, 0.75]
T "3" 600.76654 255.5 0.35 [1.0, 1.0, 1.0, 0.75]
T "330" 618.8165 255.0 0.325 [1.0, 1.0, 1.0, 0.75]
P white 185.6665 260.5 482.0 18.0 [0.6, 0.639216, 0.690196, 0.375]
P rank_39_0 185.6665 260.5 18.0 18.0 [1.0, 1.0, 1.0, 0.75]
T "40" 203.6665 277.5 0.225 [1.0, 1.0, 1.0, 0.75]
T "bot13" 233.86652 277.5 0.35 [1.0, 1.0, 1.0, 0.75]
P voice_on 402.56653 260.5 18.0 18.0 [1.0, 1.0, 1.0, 0.75]
T "35" 439.36655 277.5 0.35 [1.0, 1.0, 1.0, 0.75]
T "3" 504.36655 277.5 0.35 [1.0, 1.0, 1.0, 0.75]
T "1" 554.6665 277.5 0.35 [1.0, 1.0, 1.0, 0.75]
T "1" 602.8665 277.5 0.35 [1.0, 1.0, 1.0, 0.75]
T "390" 616.41656 277.5 0.35 [1.0, 1.0, 1.0, 0.75]
P white 185.6665 282.5 482.0 18.0 [0.6, 0.639216, 0.690196, 0.375]
P rank_45_0 185.6665 282.5 18.0 18.0 [1.0, 1.0, 1.0, 0.75]
T "46" 203.6665 300.5 0.25 [1.0, 1.0, 1.0, 0.75]
T "bot15" 233.86652 299.5 0.35 [1.0, 1.0, 1.0, 0.75]
T "25" 440.41653 299.5 0.35 [1.0, 1.0, 1.0, 0.75]
T "2" 505.41656 299.5 0.35 [1.0, 1.0, 1.0, 0.75]
T "0" 548.3665 299.5 0.35 [1.0, 1.0, 1.0, 0.75]
T "3" 600.76654 299.5 0.35 [1.0, 1.0, 1.0, 0.75]
T "450" 615.89154 299.0 0.325 [1.0, 1.0, 1.0, 0.75]
P white 185.6665 304.5 482.0 18.0 [0.6, 0.639216, 0.690196, 0.375]
P rank_51_0 185.6665 304.5 18.0 18.0 [1.0, 1.0, 1.0, 0.75]
T "52" 203.6665 322.5 0.25 [1.0, 1.0, 1.0, 0.75]
T "bot17" 233.86652 321.5 0.35 [1.0, 0.8, 0.4, 0.75]
P voice_off 402.56653 304.5 18.0 18.0 [1.0, 1.0, 1.0, 0.75]
T "15" 441.46655 321.5 0.35 [1.0, 0.8, 0.4, 0.75]
T "1" 506.46655 321.5 0.35 [1.0, 0.8, 0.4, 0.75]
T "2" 553.6165 321.5 0.35 [1.0, 0.8, 0.4, 0.75]
T "1" 602.8665 321.5 0.35 [1.0, 0.8, 0.4, 0.75]
T "510" 618.8165 321.0 0.325 [1.0, 0.8, 0.4, 0.75]
P white 185.6665 326.5 482.0 18.0 [0.6, 0.639216, 0.690196, 0.375]
P rank_57_0 185.6665 326.5 18.0 18.0 [1.0, 1.0, 1.0, 0.75]
T "58" 203.6665 344.5 0.25 [1.0, 1.0, 1.0, 0.75]
T "bot19" 233.86652 343.5 0.35 [1.0, 1.0, 1.0, 0.75]
P voice_on 402.56653 326.5 18.0 18.0 [1.0, 1.0, 1.0, 0.75]
T "5" 454.06653 343.5 0.35 [1.0, 1.0, 1.0, 0.75]
T "0" 500.16653 343.5 0.35 [1.0, 1.0, 1.0, 0.75]
T "1" 554.6665 343.5 0.35 [1.0, 1.0, 1.0, 0.75]
T "3" 600.76654 343.5 0.35 [1.0, 1.0, 1.0, 0.75]
T "570" 616.8666 342.5 0.29999998 [1.0, 1.0, 1.0, 0.75]
P faction_128_arab 185.6665 352.5 35.0 35.0 [1.0, 1.0, 1.0, 0.75]
T "<MPUI_OPFOR_SHORT>" 228.6665 387.5 0.35 [1.0, 1.0, 1.0, 0.75]
T "( 10 )" 518.06647 387.5 0.35 [1.0, 1.0, 1.0, 0.75]
P white 185.6665 391.5 482.0 18.0 [0.65, 0.57, 0.41, 0.375]
T "bot0" 233.86652 408.5 0.35 [1.0, 1.0, 1.0, 0.75]
T "100" 424.06653 408.0 0.325 [1.0, 1.0, 1.0, 0.75]
T "10" 487.56656 408.5 0.35 [1.0, 1.0, 1.0, 0.75]
T "0" 548.3665 408.5 0.35 [1.0, 1.0, 1.0, 0.75]
T "0" 596.5665 408.5 0.35 [1.0, 1.0, 1.0, 0.75]
T "0" 644.76654 408.5 0.35 [1.0, 1.0, 1.0, 0.75]
P white 185.6665 413.5 482.0 18.0 [0.65, 0.57, 0.41, 0.375]
P rank_6_0 185.6665 413.5 18.0 18.0 [1.0, 1.0, 1.0, 0.75]
T "7" 203.6665 431.5 0.25 [1.0, 1.0, 1.0, 0.75]
T "bot2" 233.86652 430.5 0.35 [1.0, 1.0, 1.0, 0.75]
P voice_off 402.56653 413.5 18.0 18.0 [1.0, 1.0, 1.0, 0.75]
T "90" 438.31653 430.5 0.35 [1.0, 1.0, 1.0, 0.75]
T "9" 505.41656 430.5 0.35 [1.0, 1.0, 1.0, 0.75]
T "2" 553.6165 430.5 0.35 [1.0, 1.0, 1.0, 0.75]
T "2" 601.8165 430.5 0.35 [1.0, 1.0, 1.0, 0.75]
T "60" 626.91656 430.5 0.35 [1.0, 1.0, 1.0, 0.75]
P black 697.01855 124.5 8.0 312.0 [1.0, 1.0, 1.0, 0.375]
P white 698.01855 125.5 6.0 197.27272 [1.0, 1.0, 1.0, 0.1875]
P hudscoreboardscroll_downarrow 711.01855 420.5 16.0 16.0 [1.0, 1.0, 1.0, 0.75]
P hudscoreboardscroll_downkey 711.01855 402.5 16.0 16.0 [1.0, 1.0, 1.0, 0.75]
"##,
        );
        pin(
            "long_scrolled",
            &board(&scores, clients.clone(), 2, 4, false),
            r##"top 4 total 22
bottom 16 off true
T "^1vcod ^7test server with a rather long host name here" 185.6665 450.0 0.20000006 [1.0, 1.0, 1.0, 0.75]
T "<CGAME_SB_SCORE>" 336.97278 124.5 0.2975 [1.0, 1.0, 1.0, 0.75]
T "<CGAME_SB_KILLS>" 383.83405 124.5 0.2975 [1.0, 1.0, 1.0, 0.75]
T "<CGAME_SB_ASSISTS>" 419.98526 124.5 0.2975 [1.0, 1.0, 1.0, 0.75]
T "<CGAME_SB_DEATHS>" 477.1103 124.5 0.2975 [1.0, 1.0, 1.0, 0.75]
T "<CGAME_SB_PING>" 541.82153 124.5 0.2975 [1.0, 1.0, 1.0, 0.75]
P white 185.6665 128.5 482.0 18.0 [0.65, 0.57, 0.41, 0.375]
P rank_12_0 185.6665 128.5 18.0 18.0 [1.0, 1.0, 1.0, 0.75]
T "13" 203.6665 146.5 0.25 [1.0, 1.0, 1.0, 0.75]
T "bot4" 233.86652 145.5 0.35 [1.0, 1.0, 1.0, 0.75]
P voice_on 402.56653 128.5 18.0 18.0 [1.0, 1.0, 1.0, 0.75]
T "80" 439.36655 145.5 0.35 [1.0, 1.0, 1.0, 0.75]
T "8" 506.46655 145.5 0.35 [1.0, 1.0, 1.0, 0.75]
T "1" 554.6665 145.5 0.35 [1.0, 1.0, 1.0, 0.75]
T "0" 596.5665 145.5 0.35 [1.0, 1.0, 1.0, 0.75]
T "120" 618.51654 145.5 0.35 [1.0, 1.0, 1.0, 0.75]
P white 185.6665 150.5 482.0 18.0 [0.65, 0.57, 0.41, 0.375]
P rank_18_0 185.6665 150.5 18.0 18.0 [1.0, 1.0, 1.0, 0.75]
T "19" 203.6665 168.5 0.25 [1.0, 1.0, 1.0, 0.75]
T "bot6" 233.86652 167.5 0.35 [1.0, 1.0, 1.0, 0.75]
T "70" 433.06653 167.5 0.35 [1.0, 1.0, 1.0, 0.75]
T "7" 500.16653 167.5 0.35 [1.0, 1.0, 1.0, 0.75]
T "0" 548.3665 167.5 0.35 [1.0, 1.0, 1.0, 0.75]
T "2" 601.8165 167.5 0.35 [1.0, 1.0, 1.0, 0.75]
T "180" 619.5665 167.5 0.35 [1.0, 1.0, 1.0, 0.75]
P white 185.6665 172.5 482.0 18.0 [0.65, 0.57, 0.41, 0.375]
P rank_24_0 185.6665 172.5 18.0 18.0 [1.0, 1.0, 1.0, 0.75]
T "25" 203.6665 190.5 0.25 [1.0, 1.0, 1.0, 0.75]
T "bot8" 233.86652 189.5 0.35 [1.0, 1.0, 1.0, 0.75]
P voice_off 402.56653 172.5 18.0 18.0 [1.0, 1.0, 1.0, 0.75]
T "60" 434.11655 189.5 0.35 [1.0, 1.0, 1.0, 0.75]
T "6" 501.21655 189.5 0.35 [1.0, 1.0, 1.0, 0.75]
T "2" 553.6165 189.5 0.35 [1.0, 1.0, 1.0, 0.75]
T "0" 596.5665 189.5 0.35 [1.0, 1.0, 1.0, 0.75]
T "240" 618.8165 189.0 0.325 [1.0, 1.0, 1.0, 0.75]
P white 185.6665 194.5 482.0 18.0 [0.65, 0.57, 0.41, 0.375]
P rank_30_0 185.6665 194.5 18.0 18.0 [1.0, 1.0, 1.0, 0.75]
T "31" 203.6665 212.5 0.25 [1.0, 1.0, 1.0, 0.75]
T "bot10" 233.86652 211.5 0.35 [1.0, 1.0, 1.0, 0.75]
P voice_on 402.56653 194.5 18.0 18.0 [1.0, 1.0, 1.0, 0.75]
T "50" 435.16653 211.5 0.35 [1.0, 1.0, 1.0, 0.75]
T "5" 502.26654 211.5 0.35 [1.0, 1.0, 1.0, 0.75]
T "1" 554.6665 211.5 0.35 [1.0, 1.0, 1.0, 0.75]
T "2" 601.8165 211.5 0.35 [1.0, 1.0, 1.0, 0.75]
T "300" 618.66656 210.5 0.29999998 [1.0, 1.0, 1.0, 0.75]
P white 185.6665 216.5 482.0 18.0 [0.65, 0.57, 0.41, 0.375]
P rank_36_0 185.6665 216.5 18.0 18.0 [1.0, 1.0, 1.0, 0.75]
T "37" 203.6665 234.5 0.25 [1.0, 1.0, 1.0, 0.75]
T "bot12" 233.86652 233.5 0.35 [1.0, 1.0, 1.0, 0.75]
T "40" 436.21655 233.5 0.35 [1.0, 1.0, 1.0, 0.75]
T "4" 503.31656 233.5 0.35 [1.0, 1.0, 1.0, 0.75]
T "0" 548.3665 233.5 0.35 [1.0, 1.0, 1.0, 0.75]
T "0" 596.5665 233.5 0.35 [1.0, 1.0, 1.0, 0.75]
T "360" 615.89154 233.0 0.325 [1.0, 1.0, 1.0, 0.75]
P white 185.6665 238.5 482.0 18.0 [0.65, 0.57, 0.41, 0.375]
P rank_42_0 185.6665 238.5 18.0 18.0 [1.0, 1.0, 1.0, 0.75]
T "43" 203.6665 256.5 0.25 [1.0, 1.0, 1.0, 0.75]
T "bot14" 233.86652 255.5 0.35 [1.0, 1.0, 1.0, 0.75]
P voice_off 402.56653 238.5 18.0 18.0 [1.0, 1.0, 1.0, 0.75]
T "30" 437.26654 255.5 0.35 [1.0, 1.0, 1.0, 0.75]
T "3" 504.36655 255.5 0.35 [1.0, 1.0, 1.0, 0.75]
T "2" 553.6165 255.5 0.35 [1.0, 1.0, 1.0, 0.75]
T "2" 601.8165 255.5 0.35 [1.0, 1.0, 1.0, 0.75]
T "420" 618.8165 255.0 0.325 [1.0, 1.0, 1.0, 0.75]
P white 185.6665 260.5 482.0 18.0 [0.65, 0.57, 0.41, 0.375]
P rank_48_0 185.6665 260.5 18.0 18.0 [1.0, 1.0, 1.0, 0.75]
T "49" 203.6665 278.5 0.25 [1.0, 1.0, 1.0, 0.75]
T "bot16" 233.86652 277.5 0.35 [1.0, 1.0, 1.0, 0.75]
P voice_on 402.56653 260.5 18.0 18.0 [1.0, 1.0, 1.0, 0.75]
T "20" 438.31653 277.5 0.35 [1.0, 1.0, 1.0, 0.75]
T "2" 505.41656 277.5 0.35 [1.0, 1.0, 1.0, 0.75]
T "1" 554.6665 277.5 0.35 [1.0, 1.0, 1.0, 0.75]
T "0" 596.5665 277.5 0.35 [1.0, 1.0, 1.0, 0.75]
T "480" 616.41656 277.5 0.35 [1.0, 1.0, 1.0, 0.75]
P white 185.6665 282.5 482.0 18.0 [0.65, 0.57, 0.41, 0.375]
P rank_54_0 185.6665 282.5 18.0 18.0 [1.0, 1.0, 1.0, 0.75]
T "55" 203.6665 300.5 0.25 [1.0, 1.0, 1.0, 0.75]
T "bot18" 233.86652 299.5 0.35 [1.0, 1.0, 1.0, 0.75]
T "10" 439.36655 299.5 0.35 [1.0, 1.0, 1.0, 0.75]
T "1" 506.46655 299.5 0.35 [1.0, 1.0, 1.0, 0.75]
T "0" 548.3665 299.5 0.35 [1.0, 1.0, 1.0, 0.75]
T "2" 601.8165 299.5 0.35 [1.0, 1.0, 1.0, 0.75]
T "540" 615.89154 299.0 0.325 [1.0, 1.0, 1.0, 0.75]
P faction_128_usmc 185.6665 308.5 35.0 35.0 [1.0, 1.0, 1.0, 0.75]
T "<MPUI_MARINES_SHORT>" 228.6665 343.5 0.35 [1.0, 1.0, 1.0, 0.75]
T "( 10 )" 554.81647 343.5 0.35 [1.0, 1.0, 1.0, 0.75]
P white 185.6665 347.5 482.0 18.0 [0.6, 0.639216, 0.690196, 0.375]
P rank_3_0 185.6665 347.5 18.0 18.0 [1.0, 1.0, 1.0, 0.75]
T "4" 203.6665 365.5 0.25 [1.0, 1.0, 1.0, 0.75]
T "bot1" 233.86652 364.5 0.35 [1.0, 1.0, 1.0, 0.75]
P voice_on 402.56653 347.5 18.0 18.0 [1.0, 1.0, 1.0, 0.75]
T "95" 440.41653 364.5 0.35 [1.0, 1.0, 1.0, 0.75]
T "9" 505.41656 364.5 0.35 [1.0, 1.0, 1.0, 0.75]
T "1" 554.6665 364.5 0.35 [1.0, 1.0, 1.0, 0.75]
T "1" 602.8665 364.5 0.35 [1.0, 1.0, 1.0, 0.75]
T "30" 630.0665 364.5 0.35 [1.0, 1.0, 1.0, 0.75]
P white 185.6665 369.5 482.0 18.0 [0.6, 0.639216, 0.690196, 0.375]
P rank_9_0 185.6665 369.5 18.0 18.0 [1.0, 1.0, 1.0, 0.75]
T "10" 203.6665 387.5 0.25 [1.0, 1.0, 1.0, 0.75]
T "bot3" 233.86652 386.5 0.35 [1.0, 1.0, 1.0, 0.75]
T "85" 441.46655 386.5 0.35 [1.0, 1.0, 1.0, 0.75]
T "8" 506.46655 386.5 0.35 [1.0, 1.0, 1.0, 0.75]
T "0" 548.3665 386.5 0.35 [1.0, 1.0, 1.0, 0.75]
T "3" 600.76654 386.5 0.35 [1.0, 1.0, 1.0, 0.75]
T "90" 631.1166 386.5 0.35 [1.0, 1.0, 1.0, 0.75]
P white 185.6665 391.5 482.0 18.0 [0.6, 0.639216, 0.690196, 0.375]
P rank_15_0 185.6665 391.5 18.0 18.0 [1.0, 1.0, 1.0, 0.75]
T "16" 203.6665 409.5 0.25 [1.0, 1.0, 1.0, 0.75]
T "bot5" 233.86652 408.5 0.35 [1.0, 1.0, 1.0, 0.75]
P voice_off 402.56653 391.5 18.0 18.0 [1.0, 1.0, 1.0, 0.75]
T "75" 435.16653 408.5 0.35 [1.0, 1.0, 1.0, 0.75]
T "7" 500.16653 408.5 0.35 [1.0, 1.0, 1.0, 0.75]
T "2" 553.6165 408.5 0.35 [1.0, 1.0, 1.0, 0.75]
T "1" 602.8665 408.5 0.35 [1.0, 1.0, 1.0, 0.75]
T "150" 618.8165 408.0 0.325 [1.0, 1.0, 1.0, 0.75]
P white 185.6665 413.5 482.0 18.0 [0.6, 0.639216, 0.690196, 0.375]
P rank_21_0 185.6665 413.5 18.0 18.0 [1.0, 1.0, 1.0, 0.75]
T "22" 203.6665 431.5 0.25 [1.0, 1.0, 1.0, 0.75]
T "bot7" 233.86652 430.5 0.35 [1.0, 1.0, 1.0, 0.75]
P voice_on 402.56653 413.5 18.0 18.0 [1.0, 1.0, 1.0, 0.75]
T "65" 436.21655 430.5 0.35 [1.0, 1.0, 1.0, 0.75]
T "6" 501.21655 430.5 0.35 [1.0, 1.0, 1.0, 0.75]
T "1" 554.6665 430.5 0.35 [1.0, 1.0, 1.0, 0.75]
T "3" 600.76654 430.5 0.35 [1.0, 1.0, 1.0, 0.75]
T "210" 618.51654 430.5 0.35 [1.0, 1.0, 1.0, 0.75]
P black 697.01855 124.5 8.0 312.0 [1.0, 1.0, 1.0, 0.375]
P white 698.01855 167.77274 6.0 183.18181 [1.0, 1.0, 1.0, 0.1875]
P hudscoreboardscroll_uparrow 711.01855 124.5 16.0 16.0 [1.0, 1.0, 1.0, 0.75]
P hudscoreboardscroll_upkey 711.01855 142.5 16.0 16.0 [1.0, 1.0, 1.0, 0.75]
P hudscoreboardscroll_downarrow 711.01855 420.5 16.0 16.0 [1.0, 1.0, 1.0, 0.75]
P hudscoreboardscroll_downkey 711.01855 402.5 16.0 16.0 [1.0, 1.0, 1.0, 0.75]
"##,
        );
        pin(
            "long_graph",
            &board(&scores, clients, 0, 30, true),
            r##"top 30 total 22
bottom 22 off false
T "^1vcod ^7test server with a rather long host name here" 185.6665 450.0 0.20000006 [1.0, 1.0, 1.0, 0.75]
T "<CGAME_SB_SCORE>" 336.97278 124.5 0.2975 [1.0, 1.0, 1.0, 0.75]
T "<CGAME_SB_KILLS>" 383.83405 124.5 0.2975 [1.0, 1.0, 1.0, 0.75]
T "<CGAME_SB_ASSISTS>" 419.98526 124.5 0.2975 [1.0, 1.0, 1.0, 0.75]
T "<CGAME_SB_DEATHS>" 477.1103 124.5 0.2975 [1.0, 1.0, 1.0, 0.75]
T "<CGAME_SB_PING>" 541.82153 124.5 0.2975 [1.0, 1.0, 1.0, 0.75]
P black 697.01855 124.5 8.0 312.0 [1.0, 1.0, 1.0, 0.375]
P white 698.01855 534.13635 6.0 -98.63636 [1.0, 1.0, 1.0, 0.1875]
P hudscoreboardscroll_uparrow 711.01855 124.5 16.0 16.0 [1.0, 1.0, 1.0, 0.75]
P hudscoreboardscroll_upkey 711.01855 142.5 16.0 16.0 [1.0, 1.0, 1.0, 0.75]
"##,
        );
    }
}
