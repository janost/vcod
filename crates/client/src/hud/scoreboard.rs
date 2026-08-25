//! Tab-held scoreboard: the `b` server command, requested on demand
//! (docs/research/cod11-hud-protocol.md, sections 3-4).

use std::collections::HashMap;

use super::font::{self, Font};
use super::HudQuad;

/// The stock client's re-request cadence while the board is up (doc section 4).
pub const REQUEST_INTERVAL: f32 = 2.0;

const TOP_Y: f32 = 72.0;
/// Right edges of the right-aligned number columns, from the table's left edge.
const COL_SCORE: f32 = 300.0;
const COL_PING: f32 = 390.0;
const COL_TIME: f32 = 460.0;
/// Centering assumes content ends at `COL_TIME`; a wider value shifts the
/// table left of center.
const TABLE_W: f32 = COL_TIME;
const SECTION_GAP: f32 = 12.0;

/// `cl[+0x217c] == 3` (doc section 3). Team 0 is "unassigned", which every
/// row carries in `dm`.
const SPECTATOR_TEAM: i32 = 3;

/// Shared by CS 5/6 and the `b` totals; renders as `"-"` (doc section 5).
const NO_TEAM_SCORE: i32 = -9999;
/// Sent while the client is still connecting (doc section 3).
const CONNECTING_PING: i32 = -1;

/// `dm` is the only gametype confirmed live to carry team 0 on every row.
fn is_team_gametype(gametype: &str) -> bool {
    gametype != "dm"
}

fn gametype_label(gametype: &str) -> String {
    match gametype {
        "dm" => "Deathmatch".to_string(),
        other => other.to_string(),
    }
}

fn fmt_team_score(score: i32) -> String {
    if score == NO_TEAM_SCORE {
        "-".to_string()
    } else {
        score.to_string()
    }
}

fn fmt_ping(ping: i32) -> String {
    if ping == CONNECTING_PING {
        "-".to_string()
    } else {
        ping.to_string()
    }
}

/// One `b` row (doc section 3). `team` is not on the wire: `parse_scores`
/// leaves it 0 and [`Scoreboard::build`] fills it from `HudFrame.clients`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScoreEntry {
    pub client: u32,
    pub score: i32,
    pub ping: i32,
    pub time: i32,
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

/// `b <numRows> <axis> <allies> {<client> <score> <ping> <time> <statusIcon>}*`
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
    let num_rows = int(1, "numRows")?.max(0) as usize;
    let axis = int(2, "axisScore")?;
    let allies = int(3, "alliesScore")?;

    let mut entries = Vec::with_capacity(num_rows.min(64));
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
            client: client.max(0) as u32,
            score: field(1, "score")?,
            ping: field(2, "ping")?,
            time: field(3, "time")?,
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

/// The last parsed `b` reply, drawn while [`Self::visible`] is set. A
/// malformed reply keeps the previous table.
pub struct Scoreboard {
    entries: Vec<ScoreEntry>,
    pub axis_score: i32,
    pub allies_score: i32,
    pub visible: bool,
    /// Render-clock seconds of the next due `score` request; `f32::MIN` so
    /// the first check fires at once.
    next_request: f32,
    warned_malformed: bool,
    warned_truncated: bool,
    /// Gates [`Self::totals`] so an unrequested board does not override the
    /// header with `(0, 0)`.
    received: bool,
}

impl Scoreboard {
    pub fn new() -> Scoreboard {
        Scoreboard {
            entries: Vec::new(),
            axis_score: 0,
            allies_score: 0,
            visible: false,
            next_request: f32::MIN,
            warned_malformed: false,
            warned_truncated: false,
            received: false,
        }
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

    /// True when [`Self::visible`] and [`REQUEST_INTERVAL`] has elapsed. The
    /// caller sends `score` and then calls [`Self::mark_requested`].
    pub fn due(&self, now: f32) -> bool {
        self.visible && now >= self.next_request
    }

    pub fn mark_requested(&mut self, now: f32) {
        self.next_request = now + REQUEST_INTERVAL;
    }

    /// Drop the rows; the totals, visibility and request timer stay.
    #[allow(dead_code)] // called by the loading task
    pub fn clear(&mut self) {
        self.entries.clear();
    }

    /// Rows matching `is_member(team)`, score descending.
    fn sorted_rows(entries: &[ScoreEntry], is_member: impl Fn(i32) -> bool) -> Vec<&ScoreEntry> {
        let mut rows: Vec<&ScoreEntry> = entries.iter().filter(|e| is_member(e.team)).collect();
        rows.sort_by_key(|e| std::cmp::Reverse(e.score));
        rows
    }

    /// Sorted rows off the raw entries, whose `team` stays 0 until `build`
    /// resolves it.
    #[cfg_attr(not(test), allow(dead_code))] // only exercised by this module's own tests
    pub fn rows_for_team(&self, team: i32) -> Vec<&ScoreEntry> {
        Self::sorted_rows(&self.entries, |t| t == team)
    }

    /// Draw order. Team gametype: Axis (1), Allies (2), Spectators (3, plus
    /// team 0 for a player who has not picked a side). Non-team: one section
    /// named after the gametype, then Spectators.
    fn sections<'a>(
        gametype: &str,
        resolved: &'a [ScoreEntry],
        axis_score: i32,
        allies_score: i32,
    ) -> Vec<Section<'a>> {
        if is_team_gametype(gametype) {
            vec![
                Section {
                    label: "Axis".to_string(),
                    icon: Some("gfx/hud/axis_icon"),
                    total: Some(axis_score),
                    members: Self::sorted_rows(resolved, |t| t == 1),
                },
                Section {
                    label: "Allies".to_string(),
                    icon: Some("gfx/hud/allied_icon"),
                    total: Some(allies_score),
                    members: Self::sorted_rows(resolved, |t| t == 2),
                },
                Section {
                    label: "Spectators".to_string(),
                    icon: None,
                    total: None,
                    members: Self::sorted_rows(resolved, |t| t == 0 || t == SPECTATOR_TEAM),
                },
            ]
        } else {
            vec![
                Section {
                    label: gametype_label(gametype),
                    icon: None,
                    total: None,
                    members: Self::sorted_rows(resolved, |t| t != SPECTATOR_TEAM),
                },
                Section {
                    label: "Spectators".to_string(),
                    icon: None,
                    total: None,
                    members: Self::sorted_rows(resolved, |t| t == SPECTATOR_TEAM),
                },
            ]
        }
    }

    /// Centered table. `names` resolves a client's live name and team from
    /// `HudFrame.clients`; an unresolvable client keeps team 0 and still
    /// draws. `gametype` is CS 0's `g_gametype`. Status icons (CS 21-28) are
    /// not drawn; the doc does not confirm the string is a direct material path.
    pub fn build(
        &self,
        header: &Font,
        text: &Font,
        screen_w: f32,
        names: &dyn Fn(u32) -> Option<(String, i32)>,
        gametype: &str,
        out: &mut Vec<HudQuad>,
    ) {
        let scale = super::HUD_SCALE;
        let x0 = (screen_w - TABLE_W) / 2.0;
        let mut y = TOP_Y;

        // Wire entries carry `team: 0`; resolve before grouping.
        let mut resolved: Vec<ScoreEntry> = self.entries.clone();
        let mut display_name: HashMap<u32, String> = HashMap::with_capacity(resolved.len());
        for e in &mut resolved {
            let name = match names(e.client) {
                Some((name, team)) => {
                    e.team = team;
                    name
                }
                None => format!("client {}", e.client),
            };
            display_name.insert(e.client, name);
        }

        for section in Self::sections(gametype, &resolved, self.axis_score, self.allies_score) {
            Self::build_section(
                header,
                text,
                scale,
                x0,
                &mut y,
                &section,
                &display_name,
                out,
            );
        }
    }

    /// One header row plus one row per member. An empty section draws
    /// nothing, header included.
    #[allow(clippy::too_many_arguments)]
    fn build_section(
        header: &Font,
        text: &Font,
        scale: f32,
        x0: f32,
        y: &mut f32,
        section: &Section,
        display_name: &HashMap<u32, String>,
        out: &mut Vec<HudQuad>,
    ) {
        if section.members.is_empty() {
            return;
        }

        let color = font::COLORS[7];
        let icon_size = super::SIZE_HEADER as f32;
        let mut hx = x0;
        if let Some(icon) = section.icon {
            out.push(HudQuad {
                verts: [
                    [hx, *y],
                    [hx + icon_size, *y],
                    [hx + icon_size, *y + icon_size],
                    [hx, *y + icon_size],
                ],
                uvs: [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]],
                rgba: [1.0, 1.0, 1.0, 1.0],
                texture: icon.to_string(),
            });
            hx += icon_size + 8.0;
        }
        font::layout(header, &section.label, hx, *y, scale, color, out);
        if let Some(total) = section.total {
            let s = fmt_team_score(total);
            let w = font::measure(header, &s, scale);
            font::layout(header, &s, x0 + TABLE_W - w, *y, scale, color, out);
        }
        *y += header.line_height(scale);

        let line_h = text.line_height(scale);
        for e in &section.members {
            // `build` inserted every entry, fallback name included, before sectioning.
            let name = &display_name[&e.client];
            font::layout(text, name, x0, *y, scale, color, out);
            let score_s = e.score.to_string();
            let ping_s = fmt_ping(e.ping);
            let time_s = e.time.to_string();
            for (s, col_x) in [
                (&score_s, COL_SCORE),
                (&ping_s, COL_PING),
                (&time_s, COL_TIME),
            ] {
                let w = font::measure(text, s, scale);
                font::layout(text, s, x0 + col_x - w, *y, scale, color, out);
            }
            *y += line_h;
        }
        *y += SECTION_GAP;
    }
}

/// One drawn block; `members` are sorted by score descending.
struct Section<'a> {
    label: String,
    icon: Option<&'static str>,
    total: Option<i32>,
    members: Vec<&'a ScoreEntry>,
}

impl Default for Scoreboard {
    fn default() -> Self {
        Self::new()
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
        // Synthetic, from doc section 3's grammar; no raw `b` line was kept.
        let tokens = toks("b 3 5 8 0 12 45 100 0 1 9 60 250 3 2 3 80 400 0");
        let msg = parse_scores(&tokens).unwrap();
        assert!(!msg.truncated);
        assert_eq!(msg.axis, 5);
        assert_eq!(msg.allies, 8);
        assert_eq!(msg.entries.len(), 3);
        assert_eq!(msg.entries[0].client, 0);
        assert_eq!(msg.entries[0].score, 12);
        assert_eq!(msg.entries[0].ping, 45);
        assert_eq!(msg.entries[0].time, 100);
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
    fn table_sorts_by_score_within_team() {
        let mut b = Scoreboard::new();
        b.entries = vec![
            ScoreEntry {
                client: 1,
                score: 5,
                ping: 40,
                time: 1000,
                status_icon: 0,
                team: 1,
            },
            ScoreEntry {
                client: 2,
                score: 9,
                ping: 60,
                time: 2000,
                status_icon: 0,
                team: 1,
            },
        ];
        let rows = b.rows_for_team(1);
        assert_eq!(
            rows.iter().map(|e| e.client).collect::<Vec<_>>(),
            vec![2, 1]
        );
    }

    #[test]
    fn request_timer_fires_immediately_then_every_interval() {
        let mut b = Scoreboard::new();
        b.visible = true;
        assert!(b.due(0.0));
        b.mark_requested(0.0);
        assert!(!b.due(1.0));
        assert!(b.due(2.0));
    }

    #[test]
    fn hidden_board_never_requests() {
        let b = Scoreboard::new();
        assert!(!b.due(0.0));
        assert!(!b.due(1000.0));
    }

    #[test]
    fn build_groups_axis_allies_and_spectators() {
        let Some(fs) = vcod_common::testing::game_fs() else {
            return;
        };
        let header = font::load_font(&fs, 24).unwrap();
        let text = font::load_font(&fs, 16).unwrap();
        let mut b = Scoreboard::new();
        b.axis_score = 5;
        b.allies_score = 8;
        b.entries = vec![
            ScoreEntry {
                client: 1,
                score: 12,
                ping: 45,
                time: 100,
                status_icon: 0,
                team: 0,
            },
            ScoreEntry {
                client: 2,
                score: 9,
                ping: 60,
                time: 250,
                status_icon: 0,
                team: 0,
            },
            ScoreEntry {
                client: 3,
                score: 3,
                ping: 80,
                time: 400,
                status_icon: 0,
                team: 0,
            },
        ];
        let names = |c: u32| -> Option<(String, i32)> {
            match c {
                1 => Some(("Alice".into(), 1)),
                2 => Some(("Bob".into(), 2)),
                3 => Some(("Carol".into(), 3)),
                _ => None,
            }
        };
        let mut out = Vec::new();
        b.build(&header, &text, 1280.0, &names, "tdm", &mut out);
        assert!(!out.is_empty());
        let axis_y = out
            .iter()
            .find(|q| q.texture == "gfx/hud/axis_icon")
            .expect("axis header icon drawn")
            .verts[0][1];
        let allies_y = out
            .iter()
            .find(|q| q.texture == "gfx/hud/allied_icon")
            .expect("allies header icon drawn")
            .verts[0][1];
        assert!(axis_y < allies_y, "axis section must draw above allies");
    }

    #[test]
    fn table_is_centered_on_the_actual_content_width() {
        let Some(fs) = vcod_common::testing::game_fs() else {
            return;
        };
        let header = font::load_font(&fs, 24).unwrap();
        let text = font::load_font(&fs, 16).unwrap();
        let mut b = Scoreboard::new();
        b.axis_score = 5;
        b.allies_score = 8;
        b.entries = vec![ScoreEntry {
            client: 1,
            score: 12,
            ping: 45,
            time: 100,
            status_icon: 0,
            team: 0,
        }];
        let names = |_: u32| -> Option<(String, i32)> { Some(("Alice".into(), 1)) };
        let screen_w = 1280.0;
        let mut out = Vec::new();
        b.build(&header, &text, screen_w, &names, "tdm", &mut out);
        let x0 = (screen_w - TABLE_W) / 2.0;
        assert!(((x0 + TABLE_W / 2.0) - screen_w / 2.0).abs() < 0.01);
        // Nothing drawn may cross TABLE_W by more than a few px of bearing.
        let max_x = out
            .iter()
            .flat_map(|q| q.verts.iter().map(|v| v[0]))
            .fold(f32::MIN, f32::max);
        assert!(max_x <= x0 + TABLE_W + 4.0, "{max_x} vs {}", x0 + TABLE_W);
    }

    #[test]
    fn non_team_gametype_gets_one_section_not_spectators() {
        let resolved = vec![
            ScoreEntry {
                client: 1,
                score: 5,
                ping: 40,
                time: 100,
                status_icon: 0,
                team: 0,
            },
            ScoreEntry {
                client: 2,
                score: 9,
                ping: 60,
                time: 200,
                status_icon: 0,
                team: 0,
            },
        ];
        let sections = Scoreboard::sections("dm", &resolved, 0, 0);
        // `build_section` skips empty sections; only the non-empty ones reach the screen.
        let drawn: Vec<&Section> = sections.iter().filter(|s| !s.members.is_empty()).collect();
        assert_eq!(drawn.len(), 1);
        assert_eq!(drawn[0].label, "Deathmatch");
        assert!(drawn.iter().all(|s| s.label != "Spectators"));
        assert_eq!(
            drawn[0]
                .members
                .iter()
                .map(|e| e.client)
                .collect::<Vec<_>>(),
            vec![2, 1]
        );
    }

    #[test]
    fn non_team_gametype_still_separates_real_spectators() {
        let resolved = vec![
            ScoreEntry {
                client: 1,
                score: 5,
                ping: 40,
                time: 100,
                status_icon: 0,
                team: 0, // playing dm
            },
            ScoreEntry {
                client: 2,
                score: 0,
                ping: 20,
                time: 300,
                status_icon: 0,
                team: SPECTATOR_TEAM,
            },
        ];
        let sections = Scoreboard::sections("dm", &resolved, 0, 0);
        assert_eq!(sections.len(), 2);
        assert_eq!(sections[0].label, "Deathmatch");
        assert_eq!(sections[0].members.len(), 1);
        assert_eq!(sections[0].members[0].client, 1);
        assert_eq!(sections[1].label, "Spectators");
        assert_eq!(sections[1].members.len(), 1);
        assert_eq!(sections[1].members[0].client, 2);
    }

    #[test]
    fn team_gametype_keeps_axis_allies_and_spectator_sections() {
        let resolved = vec![
            ScoreEntry {
                client: 1,
                score: 5,
                ping: 40,
                time: 100,
                status_icon: 0,
                team: 1,
            },
            ScoreEntry {
                client: 2,
                score: 3,
                ping: 60,
                time: 200,
                status_icon: 0,
                team: 2,
            },
            ScoreEntry {
                client: 3,
                score: 0,
                ping: 20,
                time: 300,
                status_icon: 0,
                team: SPECTATOR_TEAM,
            },
        ];
        let sections = Scoreboard::sections("tdm", &resolved, 5, 8);
        let labels: Vec<&str> = sections.iter().map(|s| s.label.as_str()).collect();
        assert_eq!(labels, vec!["Axis", "Allies", "Spectators"]);
        assert_eq!(sections[0].total, Some(5));
        assert_eq!(sections[1].total, Some(8));
    }

    #[test]
    fn team_score_of_no_score_sentinel_renders_as_dash() {
        assert_eq!(fmt_team_score(-9999), "-");
        assert_eq!(fmt_team_score(7), "7");
    }

    #[test]
    fn connecting_ping_renders_as_dash() {
        assert_eq!(fmt_ping(-1), "-");
        assert_eq!(fmt_ping(52), "52");
    }
}
