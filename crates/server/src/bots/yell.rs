//! Combat yells. A bot picks a stock quick message as a player does from
//! the `V` menu: an `mr` on the `quickcommands` or `quickstatements` script
//! menu. The gametype script plays the team's voice line, sets the head
//! icon and prints the team chat line (cod11-quick-chat.md, "The quick
//! message menu"; docs/bots.md, "Yells").

use std::collections::BTreeMap;

use crate::game::script::{TEAM_ALLIES, TEAM_AXIS};

/// Ms a bot keeps quiet after its own yell.
const BOT_QUIET_MS: i32 = 8000;
/// Ms a team keeps quiet after any of its bots yelled, so a squad that sees
/// the same enemy sends one voice, not a chorus.
const TEAM_QUIET_MS: i32 = 2500;

/// What a bot has to yell about this tick.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Yell {
    /// A one-life round (`sd`, `re`) starts: "Follow me!" or "Move in!".
    Advance,
    /// A new target in sight, not one it lost a moment ago: "Enemy spotted!".
    Spotted,
    /// A frag left the hand: "Grenade!".
    Grenade,
    /// The bot's hit was the last on an enemy who just died: "Enemy down!".
    Down,
}

impl Yell {
    /// The script menu the row is picked from.
    pub(crate) fn menu(self) -> &'static str {
        match self {
            Yell::Advance => "quickcommands",
            Yell::Spotted | Yell::Grenade | Yell::Down => "quickstatements",
        }
    }

    /// The rows `_teams.gsc` (1.1, 1.5, UO) and CoD2's `_quickmessages.gsc`
    /// number alike: `quickcommands` 1 "Follow me", 2 "Move in";
    /// `quickstatements` 1 "Enemy spotted", 2 "Enemy down", 5 "Grenade".
    fn rows(self) -> &'static [u8] {
        match self {
            Yell::Advance => &[1, 2],
            Yell::Spotted => &[1],
            Yell::Down => &[2],
            Yell::Grenade => &[5],
        }
    }

    /// Chance in 100 that the yell goes out at all.
    fn chance(self) -> u32 {
        match self {
            Yell::Advance => 40,
            Yell::Spotted => 30,
            Yell::Grenade => 70,
            Yell::Down => 40,
        }
    }
}

/// The driver's yell bookkeeping: who may yell now, and who was alive last
/// tick, for the deaths [`Yell::Down`] is credited on.
#[derive(Default)]
pub(crate) struct Gate {
    /// The gate's own generator, so a yell's roll moves no brain's draws.
    rng: u64,
    /// Per bot slot and per team, the time of its last yell.
    bot_last: BTreeMap<usize, i32>,
    team_last: BTreeMap<i32, i32>,
    /// Per slot, whether the body played last tick.
    alive: Vec<bool>,
}

impl Gate {
    /// The row of `yell`'s menu the bot in `slot` sends at `now` (level
    /// ms), or `None`: not on a side (DM's `none`), quiet after its own or
    /// its team's last yell, or the roll said no. A clock that went back
    /// (a restart) ends any quiet.
    pub(crate) fn pass(&mut self, slot: usize, team: i32, yell: Yell, now: i32) -> Option<u8> {
        if team != TEAM_AXIS && team != TEAM_ALLIES {
            return None;
        }
        let quiet = |last: Option<&i32>, ms: i32| {
            last.is_some_and(|&t| (0..ms).contains(&now.wrapping_sub(t)))
        };
        if quiet(self.bot_last.get(&slot), BOT_QUIET_MS)
            || quiet(self.team_last.get(&team), TEAM_QUIET_MS)
        {
            return None;
        }
        if self.rng == 0 {
            self.rng = 0x9e37_79b9_7f4a_7c15;
        }
        let mut roll = || crate::game::host::rand_int(&mut self.rng) as u32;
        if roll() % 100 >= yell.chance() {
            return None;
        }
        let rows = yell.rows();
        let row = rows[roll() as usize % rows.len()];
        self.bot_last.insert(slot, now);
        self.team_last.insert(team, now);
        Some(row)
    }

    /// The slots whose body played last tick and does not now, given each
    /// slot's `playing` this tick; remembers `playing` for the next.
    pub(crate) fn deaths(&mut self, playing: Vec<bool>) -> Vec<usize> {
        let died = playing
            .iter()
            .enumerate()
            .filter(|&(s, &now)| !now && self.alive.get(s).copied().unwrap_or(false))
            .map(|(s, _)| s)
            .collect();
        self.alive = playing;
        died
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game::script::{TEAM_NONE, TEAM_SPECTATOR};

    /// Every row `yell` sent over many tries, each past every quiet.
    fn rows_sent(yell: Yell) -> std::collections::BTreeSet<u8> {
        let mut g = Gate::default();
        (0..400)
            .filter_map(|t| g.pass(t, TEAM_ALLIES, yell, t as i32 * BOT_QUIET_MS))
            .collect()
    }

    /// A gate whose bot 7 on the axis side just yelled, and when.
    fn after_a_yell() -> (Gate, i32) {
        let mut g = Gate::default();
        let now = (0..1000)
            .map(|t| t * BOT_QUIET_MS)
            .find(|&now| g.pass(7, TEAM_AXIS, Yell::Grenade, now).is_some())
            .expect("a grenade yell passes some roll");
        (g, now)
    }

    #[test]
    fn rows_are_the_stock_menus_rows() {
        assert_eq!(
            rows_sent(Yell::Advance).into_iter().collect::<Vec<_>>(),
            [1, 2]
        );
        assert_eq!(
            rows_sent(Yell::Spotted).into_iter().collect::<Vec<_>>(),
            [1]
        );
        assert_eq!(rows_sent(Yell::Down).into_iter().collect::<Vec<_>>(), [2]);
        assert_eq!(
            rows_sent(Yell::Grenade).into_iter().collect::<Vec<_>>(),
            [5]
        );
    }

    #[test]
    fn a_bot_and_its_team_keep_quiet_after_a_yell() {
        let (mut g, now) = after_a_yell();
        for s in 0..50 {
            assert_eq!(
                g.pass(s, TEAM_AXIS, Yell::Grenade, now + TEAM_QUIET_MS - 1),
                None
            );
        }
        // The other side is not held.
        assert!((0..50).any(|s| g.pass(s, TEAM_ALLIES, Yell::Grenade, now + 1).is_some()));
        // Past the team's quiet the bot itself still holds off for its own.
        for t in TEAM_QUIET_MS..BOT_QUIET_MS {
            assert_eq!(g.pass(7, TEAM_AXIS, Yell::Grenade, now + t), None);
        }
    }

    #[test]
    fn a_restart_clock_ends_the_quiet() {
        let (mut g, now) = after_a_yell();
        // Any time before the yell reads as a clock gone back.
        assert!((1..200).any(|t| g.pass(7, TEAM_AXIS, Yell::Grenade, now - t).is_some()));
    }

    #[test]
    fn no_yell_off_the_two_sides() {
        let mut g = Gate::default();
        for t in 0..200 {
            let now = t * BOT_QUIET_MS;
            assert_eq!(g.pass(1, TEAM_NONE, Yell::Grenade, now), None);
            assert_eq!(g.pass(1, TEAM_SPECTATOR, Yell::Grenade, now), None);
        }
    }

    #[test]
    fn a_death_is_a_body_that_played_last_tick() {
        let mut g = Gate::default();
        // The first tick knows no one alive: no death off an empty table.
        assert!(g.deaths(vec![false, true, true]).is_empty());
        assert_eq!(g.deaths(vec![false, false, true]), [1]);
        assert!(g.deaths(vec![true, false, true]).is_empty());
    }
}
