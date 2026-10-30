//! Team play: how a team's bots share out an objective round and the ways
//! to it (docs/research/bot-objectives.md section 9), and which of a
//! side's bots go to a noise (bot-combat.md section 7). The server calls
//! these once a frame with the whole team in hand and puts the answers in
//! each bot's view; the brain only reads them.

use super::dist_sq;

/// A teammate planting within this covers the planter instead of going on
/// to its own site.
pub const COVER_RANGE: f32 = 1500.0;
/// Covering bots keep this far off the planter, as off a planted bomb: a
/// body on the zone stands in a defender's line to him.
pub const COVER_INNER: f32 = 96.0;
/// The attackers nearest a retrieval carrier who escort him; the rest
/// hold the ring round the goal he is making for.
pub const ESCORTS: usize = 1;
/// Teammates who go to a noise: those nearer it than this many others
/// (its shooter not counted). The rest of the team ignores it and keeps to
/// its own way, or the whole side converged on every shot
/// (bot-combat.md 7).
pub const ANSWERS: usize = 2;

impl super::Bot {
    /// The bot carries a retrieval objective or CoD4's bomb: it keeps running for the
    /// goal, the keys on the path, and fights on the way (as a `bas` bot
    /// clearing a charge's blast does) rather than close on what it sees.
    pub(super) fn carrying(&self, view: &super::BotView) -> bool {
        self.team_play
            && (view
                .re
                .as_ref()
                .is_some_and(|r| r.objectives.iter().any(|m| m.mine))
                || view
                    .sd
                    .as_ref()
                    .and_then(|sd| sd.key)
                    .is_some_and(|k| k.mine))
    }
}

/// splitmix64 over the round, the team and the slot.
fn mix(round: u32, team: i32, slot: usize) -> u64 {
    let mut z = (u64::from(round) << 40) ^ ((team as u64) << 32) ^ slot as u64;
    z = z.wrapping_add(0x9e37_79b9_7f4a_7c15);
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^ (z >> 31)
}

/// A bot's place among its team's bots (`mates`, itself included) this
/// round: the team shuffled by `round` (the level's `sv_serverid`, which
/// every `map_restart` moves on), so the sites and objectives dealt out
/// by rank go to other bots each round.
pub fn round_rank(slot: usize, mates: &[usize], team: i32, round: u32) -> usize {
    let key = |s: usize| (mix(round, team, s), s);
    mates.iter().filter(|&&m| key(m) < key(slot)).count()
}

/// Where a team's dealing starts this round: 0 or 1, so a lone bot, or the
/// odd one out, goes to either site.
pub fn round_shift(team: i32, round: u32) -> usize {
    (mix(round, team, usize::MAX) & 1) as usize
}

/// A playing bot's slot, team and the edges of its path ahead.
pub type TeamPath = (usize, i32, Vec<(u32, u32)>);

/// An S&D bot's site rank (`SdView::rank`) this round. The attackers all
/// go for one site, picked per round by [`round_shift`]; the defenders
/// spread over both by [`round_rank`] (bot-objectives.md section 9).
pub fn site_rank(attack: bool, slot: usize, mates: &[usize], team: i32, round: u32) -> usize {
    let shift = round_shift(team, round);
    if attack {
        shift
    } else {
        round_rank(slot, mates, team, round) + shift
    }
}

/// The edges every other teammate's path still runs along, `(low, high)`,
/// sorted.
pub fn crowd(paths: &[TeamPath], slot: usize, team: i32) -> Vec<(u32, u32)> {
    let mut out: Vec<(u32, u32)> = paths
        .iter()
        .filter(|(s, t, _)| *s != slot && *t == team)
        .flat_map(|(_, _, p)| p.iter().map(|&(a, b)| (a.min(b), a.max(b))))
        .collect();
    out.sort_unstable();
    out.dedup();
    out
}

/// Whether a bot at `at` goes to `noise`: fewer than [`ANSWERS`] of its
/// teammates (`mates`, each slot and feet, the bot itself left out) stand
/// nearer it, the one who made it aside.
pub fn answers(at: [f32; 3], noise: &super::Noise, mates: &[(usize, [f32; 3])]) -> bool {
    let d = dist_sq(at, noise.at);
    mates
        .iter()
        .filter(|(s, p)| *s != noise.source && dist_sq(*p, noise.at) < d)
        .count()
        < ANSWERS
}

/// The nearest teammate within [`COVER_RANGE`] the script holds at a plant
/// (`linkTo`): `mates` holds each one's slot, feet and whether he is held.
pub fn cover(slot: usize, at: [f32; 3], mates: &[(usize, [f32; 3], bool)]) -> Option<[f32; 3]> {
    mates
        .iter()
        .filter(|(s, p, held)| *s != slot && *held && dist_sq(at, *p) < COVER_RANGE * COVER_RANGE)
        .map(|(_, p, _)| *p)
        .min_by(|a, b| dist_sq(at, *a).total_cmp(&dist_sq(at, *b)))
}

/// The [`ESCORTS`] teammates nearest the carrier at `carrier`, lower slot
/// first on a tie: `mates` holds each playing one's slot and feet, the
/// carrier left out.
pub fn escorts(carrier: [f32; 3], mates: &[(usize, [f32; 3])]) -> Vec<usize> {
    let mut by: Vec<(f32, usize)> = mates
        .iter()
        .map(|(s, p)| (dist_sq(carrier, *p), *s))
        .collect();
    by.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
    by.into_iter().take(ESCORTS).map(|(_, s)| s).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ranks_are_a_permutation_that_changes_with_the_round() {
        let mates = [2, 3, 5, 7];
        let ranks = |round| -> Vec<usize> {
            mates
                .iter()
                .map(|&s| round_rank(s, &mates, 1, round))
                .collect()
        };
        for round in 0..8 {
            let mut r = ranks(round);
            r.sort_unstable();
            assert_eq!(r, [0, 1, 2, 3]);
        }
        assert!((1..8).any(|round| ranks(round) != ranks(0)));
        // And the shift takes both values.
        let shifts: Vec<usize> = (0..16).map(|r| round_shift(2, r)).collect();
        assert!(shifts.contains(&0) && shifts.contains(&1));
    }

    #[test]
    fn attackers_share_a_site_and_defenders_split() {
        let mates = [1, 4, 6];
        for round in 0..8 {
            let att: Vec<usize> = mates
                .iter()
                .map(|&s| site_rank(true, s, &mates, 1, round) % 2)
                .collect();
            assert!(att.iter().all(|&r| r == att[0]));
            let def: Vec<usize> = mates
                .iter()
                .map(|&s| site_rank(false, s, &mates, 2, round) % 2)
                .collect();
            assert!(def.contains(&0) && def.contains(&1));
        }
    }

    #[test]
    fn the_crowd_is_the_other_teammates_edges_either_way_round() {
        let paths = vec![
            (1, 1, vec![(4, 3), (3, 9)]),
            (2, 1, vec![(9, 3)]),
            (3, 2, vec![(7, 8)]),
            (4, 1, vec![(5, 6)]),
        ];
        assert_eq!(crowd(&paths, 4, 1), vec![(3, 4), (3, 9)]);
        assert_eq!(crowd(&paths, 3, 2), vec![]);
    }

    #[test]
    fn the_teammates_nearest_a_noise_answer_it() {
        let noise = super::super::Noise {
            at: [0.0; 3],
            source: 9,
            radius: 2000.0,
        };
        let mates = [(9, [10.0, 0.0, 0.0]), (2, [100.0, 0.0, 0.0])];
        // The shooter does not count: one nearer, so the second answers.
        assert!(answers([200.0, 0.0, 0.0], &noise, &mates));
        let mates = [(3, [50.0, 0.0, 0.0]), (2, [100.0, 0.0, 0.0])];
        assert!(!answers([200.0, 0.0, 0.0], &noise, &mates));
        assert!(answers([75.0, 0.0, 0.0], &noise, &mates));
    }

    #[test]
    fn a_held_teammate_in_range_is_covered() {
        let mates = [
            (1, [0.0, 0.0, 0.0], false),
            (2, [1000.0, 0.0, 0.0], true),
            (3, [5000.0, 0.0, 0.0], true),
            (4, [10.0, 0.0, 0.0], true),
        ];
        assert_eq!(cover(1, [0.0; 3], &mates), Some([10.0, 0.0, 0.0]));
        assert_eq!(cover(4, [0.0; 3], &mates), Some([1000.0, 0.0, 0.0]));
        assert_eq!(cover(3, [5000.0, 0.0, 0.0], &mates[..2]), None);
    }

    #[test]
    fn the_nearest_teammates_escort() {
        let mates = [(5, [300.0, 0.0, 0.0]), (2, [100.0, 0.0, 0.0])];
        assert_eq!(escorts([0.0; 3], &mates), vec![2]);
    }
}
