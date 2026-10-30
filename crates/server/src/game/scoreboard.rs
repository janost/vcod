//! `DeathmatchScoreboardMessage` and `player_die`'s follower walk, which
//! sends it. Both run inside the VM: the walk right after the killed callback
//! returns, so its `b` lands where retail's does in the reliable stream.

use crate::game::host::GameHost;
use vcod_gsc::{Cx, Host, Value};

/// What `Server` mirrors of one connected client slot before any script
/// entry that can kill (`GameHost::client_roster`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RosterSlot {
    /// `CS_CONNECTED`, no gamestate sent yet.
    pub connecting: bool,
    /// In the world: the game's `CON_CONNECTED`, which `ClientBegin` sets
    /// and which a chat line's recipient needs.
    pub active: bool,
    /// The slot this client's follow is on, the `spectatorClient` the walk
    /// compares.
    pub following: Option<usize>,
    /// `cl->ping`, which `SV_CalcPings` copies into `ps.ping`.
    pub ping: i32,
    /// A vcod bot (`--bots`), which never votes.
    pub bot: bool,
}

/// One client's row before `SortRanks`.
pub struct Row {
    pub slot: usize,
    pub connecting: bool,
    pub spectator: bool,
    pub score: i64,
    pub deaths: i64,
    pub ping: i32,
    /// The 1-based `CsRange::StatusIcon` index, 0 for none.
    pub icon: usize,
}

/// `b <numRows> <axis> <allies>{ <client> <score> <ping> <time> <icon>}*`,
/// one row per online client (`docs/research/cod11-hud-protocol.md` section
/// 3). `ping` is `-1` for a client still connecting, else capped at 999.
pub fn text(rows: Vec<Row>, scores: [i32; 2]) -> String {
    text_capped(rows, scores, None)
}

/// [`text`] with the rows' text cut at `cap` bytes: CoD2's
/// `DeathmatchScoreboardMessage` (Mac 0x1a4f98) stops at the first row that
/// would take it past 1024 and counts only the rows written
/// (cod2-protocol.md 3.4). 1.1 passes `None`.
pub fn text_capped(mut rows: Vec<Row>, [axis, allies]: [i32; 2], cap: Option<usize>) -> String {
    // `level.sortedClients[]`'s order, `SortRanks` (.so 0x50090): a
    // connecting client last, then spectators last among themselves by
    // slot, then score descending, then deaths ascending; ties keep the
    // slot order (hud protocol doc, section 3).
    rows.sort_by_key(|r| (r.connecting, r.spectator, -r.score, r.deaths, r.slot));
    let mut body = String::new();
    let mut count = 0;
    for r in rows {
        let ping = if r.connecting { -1 } else { r.ping.min(999) };
        let row = format!(" {} {} {ping} {} {}", r.slot, r.score, r.deaths, r.icon);
        if cap.is_some_and(|cap| body.len() + row.len() > cap) {
            break;
        }
        body.push_str(&row);
        count += 1;
    }
    format!("b {count} {axis} {allies}{body}")
}

impl GameHost {
    /// `DeathmatchScoreboardMessage` (`.so` 0x459c0) over the mirrored
    /// roster. The score, the deaths, the team and the status icon come from
    /// the script's own client fields, which is where every gametype writes
    /// them; tokens 2 and 3 are the two team scores, axis before allies
    /// (map-cycle doc, 6.3).
    pub fn scoreboard(&mut self, cx: &mut Cx) -> String {
        let roster: Vec<(usize, bool, i32)> = self
            .client_roster
            .iter()
            .enumerate()
            .filter_map(|(slot, r)| r.map(|r| (slot, r.connecting, r.ping)))
            .collect();
        let mut rows = Vec::with_capacity(roster.len());
        for (slot, connecting, ping) in roster {
            let mut field = |name: &str| self.client_field(cx, slot, name);
            let num = |v: Option<String>| v.and_then(|s| s.parse::<i64>().ok()).unwrap_or(0);
            let score = num(field("score"));
            let deaths = num(field("deaths"));
            let spectator = field("sessionteam").is_some_and(|t| t == "spectator");
            let icon = field("statusicon").filter(|n| !n.is_empty());
            rows.push(Row {
                slot,
                connecting,
                spectator,
                score,
                deaths,
                ping,
                icon: status_icon_index(
                    &self.configstrings,
                    self.allocators.layout(),
                    icon.as_deref(),
                ),
            });
        }
        let cap = self.profile.scoreboard_cap;
        text_capped(rows, self.team_scores, cap)
    }

    /// `player_die`'s walk (combat doc 5.1 step 9), run once the victim's
    /// `CodeCallback_PlayerKilled` has returned: every spectator following
    /// the victim is sent the scoreboard, behind what that callback queued.
    pub fn player_die_walk(&mut self, cx: &mut Cx, victim: usize) {
        for slot in 0..self.client_roster.len() {
            let Some(r) = self.client_roster[slot] else {
                continue;
            };
            if r.following == Some(victim)
                && self.client_field(cx, slot, "sessionstate").as_deref() == Some("spectator")
            {
                let text = self.scoreboard(cx);
                self.client_commands.push((slot, text));
            }
        }
    }

    /// One field off a client's entity, rendered the way string
    /// concatenation renders it. `None` when the slot holds no client entity.
    pub(crate) fn client_field(&mut self, cx: &mut Cx, slot: usize, name: &str) -> Option<String> {
        let ent = self.ents.handle(u32::try_from(slot).ok()?)?;
        self.ents.get(ent)?.client.as_ref()?;
        let atom = cx.intern_folded(name);
        match self.get_field(cx, ent, atom) {
            Value::Undefined => Some(String::new()),
            v => Some(cx.format_number(v).unwrap_or_default()),
        }
    }
}

/// A client's `.statusicon` as the 1-based index into `CsRange::StatusIcon`
/// the scoreboard row carries, or 0 when there is none or it names an icon
/// nothing precached. The client resolves `20 + n` for an `n` in `1..8`
/// (hud protocol doc, section 3).
pub(crate) fn status_icon_index(
    configstrings: &[String],
    layout: &vcod_common::net::protocol::CsLayout,
    name: Option<&str>,
) -> usize {
    let Some(name) = name else { return 0 };
    let (first, last) = layout.bounds(crate::configstrings::CsRange::StatusIcon);
    configstrings
        .get(first..=last)
        .and_then(|range| range.iter().position(|cs| cs == name))
        .map_or(0, |i| i + 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(slot: usize, score: i64) -> Row {
        Row {
            slot,
            connecting: false,
            spectator: false,
            score,
            deaths: 0,
            ping: 50,
            icon: 0,
        }
    }

    #[test]
    fn a_cod2_board_stops_at_the_row_past_1024_bytes() {
        // Each row " <slot> 1000000000000 50 0 0" is 26 or 27 bytes.
        let rows = || {
            (0..64)
                .map(|s| row(s, 1_000_000_000_000))
                .collect::<Vec<_>>()
        };
        let full = text(rows(), [0, 0]);
        assert!(full.starts_with("b 64 0 0 "));
        let cut = text_capped(rows(), [0, 0], Some(1024));
        let body = cut.splitn(5, ' ').nth(4).unwrap();
        let n: usize = cut.split(' ').nth(1).unwrap().parse().unwrap();
        assert!(n < 64);
        assert!(body.len() < 1024);
        assert_eq!(body.split(' ').count(), n * 5);
        // The rows kept are the full board's first `n`.
        assert!(full.splitn(5, ' ').nth(4).unwrap().starts_with(body));
    }
}
