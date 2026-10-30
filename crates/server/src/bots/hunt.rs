//! Going after a lost enemy (docs/research/bot-combat.md 10). A wary bot
//! does not walk onto the spot a seen enemy was lost at: the server picks a
//! graph node near it whose eye sees the spot ([`vantage`]), the bot walks
//! there with its view on the spot, and gives up on the spot sooner the
//! better it plays ([`HUNT_TICKS`]).

use super::{Bot, BotView, arrived};
use crate::nav::NavGraph;
use glam::Vec3;
use vcod_common::pmove;

/// A vantage is at least this far across from the spot: past
/// `ARRIVE_RADIUS`, so standing on it is not standing on the spot.
const VANTAGE_MIN: f32 = 160.0;
/// And no further than this across, nor `STEPSIZE` times this many rings of
/// climb.
const VANTAGE_REACH: f32 = 480.0;
/// Edge rings walked out from the spot's node.
const RINGS: usize = 14;
/// Candidates nearest the bot whose sight of the spot is traced.
const TRIES: usize = 12;
/// Ticks after the enemy was lost past which a wary bot stops walking to
/// the spot, per `vc_bot_skill` level, never past `Skill::memory_ticks`:
/// a better player reads a stale spot as a trap sooner.
const HUNT_TICKS: [u32; 4] = [70, 70, 60, 50];

/// The node within [`VANTAGE_REACH`] of `spot` (a chest-high point) but not
/// within [`VANTAGE_MIN`], walked to along edges with no jump or risky
/// step, nearest `from`, from whose feet `sees` reports a line to the
/// spot. `None` where no node near it sees it.
pub fn vantage(g: &NavGraph, from: Vec3, spot: Vec3, sees: &dyn Fn(Vec3) -> bool) -> Option<Vec3> {
    let start = g.nearest(spot.into())?;
    let near = |p: Vec3| (p - spot).truncate().length();
    // The nodes reached, in the order reached; `marked` holds them too, so
    // an edge back into the rings costs no hash.
    let mut marked = vec![false; g.nodes.len()];
    marked[start as usize] = true;
    let mut seen = vec![start];
    let mut ring = vec![start];
    for _ in 0..RINGS {
        let mut next = Vec::new();
        for &a in &ring {
            for &b in &g.edges[a as usize] {
                if !marked[b as usize]
                    && near(g.nodes[b as usize]) <= VANTAGE_REACH
                    && !g.jumps.contains(&(a, b))
                    && !g.risky.contains(&(a, b))
                {
                    marked[b as usize] = true;
                    seen.push(b);
                    next.push(b);
                }
            }
        }
        ring = next;
    }
    let mut spots: Vec<Vec3> = seen
        .into_iter()
        .map(|n| g.nodes[n as usize])
        .filter(|&p| near(p) >= VANTAGE_MIN)
        .collect();
    spots.sort_by(|a, b| {
        a.distance_squared(from)
            .total_cmp(&b.distance_squared(from))
    });
    spots.into_iter().take(TRIES).find(|&p| sees(p))
}

/// The eye over a vantage's feet that [`vantage`]'s `sees` traces from.
pub fn eye(feet: Vec3) -> Vec3 {
    feet + Vec3::Z * pmove::VIEW_STAND
}

impl Bot {
    /// The `vc_bot_skill` level these values are the preset of, else the
    /// default level's.
    pub(super) fn skill_level(&self) -> usize {
        usize::from(self.skill.level_of().unwrap_or(super::DEFAULT_LEVEL)).min(3)
    }

    /// Ticks a lost enemy's spot is walked to: [`HUNT_TICKS`] for a wary
    /// bot, else as long as it is remembered.
    fn hunt_ticks(&self) -> u32 {
        let memory = self.skill.memory_ticks;
        if self.wary {
            HUNT_TICKS[self.skill_level()].min(memory)
        } else {
            memory
        }
    }

    /// The spot of a seen enemy a wary bot still goes after, for the
    /// server to find its vantage ([`BotView::hunt`]).
    pub fn hunt_query(&self) -> Option<[f32; 3]> {
        let r = self.recall.filter(|r| r.seen && self.wary)?;
        (self.skill.memory_ticks.saturating_sub(r.ticks) <= self.hunt_ticks()).then_some(r.at)
    }

    /// Where the remembered spot is walked to: a seen enemy's spot past
    /// [`Bot::hunt_ticks`] is not, and a wary bot heads for the vantage
    /// over it when the server found one.
    pub(super) fn hunt_goal(&self, view: &BotView) -> Option<[f32; 3]> {
        let r = self.recall?;
        if r.seen && self.skill.memory_ticks.saturating_sub(r.ticks) > self.hunt_ticks() {
            return None;
        }
        let to = view.hunt.filter(|_| r.seen && self.wary).unwrap_or(r.at);
        (!arrived(view.origin, to)).then_some(to)
    }

    /// The hunt ended where it was headed this tick: on a seen enemy's
    /// spot, or on the vantage over it.
    pub(super) fn hunt_arrived(&self, view: &BotView) -> bool {
        self.recall.is_some_and(|r| {
            r.seen
                && (arrived(view.origin, r.at)
                    || view
                        .hunt
                        .is_some_and(|v| self.wary && arrived(view.origin, v)))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A row of nodes along +x, 40 apart, a wall at x 200..210 hiding the
    /// spot past it from all but the nodes at y 120 and over.
    fn graph() -> NavGraph {
        let mut nodes = Vec::new();
        for y in 0..5 {
            for x in 0..10 {
                nodes.push(Vec3::new(40.0 * x as f32, 40.0 * y as f32, 0.0));
            }
        }
        let n = nodes.len();
        let edges = (0..n)
            .map(|i| {
                let (x, y) = (i % 10, i / 10);
                let mut e = Vec::new();
                if x > 0 {
                    e.push(i as u32 - 1);
                }
                if x < 9 {
                    e.push(i as u32 + 1);
                }
                if y > 0 {
                    e.push(i as u32 - 10);
                }
                if y < 4 {
                    e.push(i as u32 + 10);
                }
                e
            })
            .collect();
        NavGraph::from_parts(nodes, edges)
    }

    #[test]
    fn the_vantage_sees_the_spot_off_the_spot_itself() {
        let g = graph();
        let spot = Vec3::new(360.0, 0.0, 40.0);
        let sees = |p: Vec3| p.y >= 120.0;
        let v = vantage(&g, Vec3::ZERO, spot, &sees).unwrap();
        assert!(v.y >= 120.0, "{v}");
        assert!((v - spot).truncate().length() >= VANTAGE_MIN, "{v}");
        // Nearest the bot of those that see it.
        assert_eq!(v, Vec3::new(0.0, 120.0, 0.0));
    }

    #[test]
    fn no_node_that_sees_the_spot_is_no_vantage() {
        let g = graph();
        let sees = |_: Vec3| false;
        assert_eq!(
            vantage(&g, Vec3::ZERO, Vec3::new(360.0, 0.0, 40.0), &sees),
            None
        );
    }
}
