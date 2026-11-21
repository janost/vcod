//! A `ctf` carrier's run home (bot-objectives.md 14.12), experiments off
//! by default: the escorts the driver deals round him ([`deal_escort`],
//! `Bot::ctf_escort`) and the edges near enemies his side saw of late and
//! near the outposts by the flag he took, which his plan pays more on
//! ([`danger`], `Bot::ctf_route`). Once a side's flag is taken, its
//! guards near home hold the carrier's way out or close on him along it
//! ([`deal_take`], `Bot::ctf_watch`, `Bot::ctf_close`, 14.13).

use glam::Vec3;

use super::super::{BotView, GuardPost, UoView, dist_sq};
use super::BotWorld;
use crate::game::script::{TEAM_ALLIES, TEAM_AXIS};
use crate::nav::NavGraph;

/// Teammates within this of a carrier escort him.
const ESCORT_NEAR: f32 = 1500.0;
/// At most this many of them.
const ESCORTS: usize = 3;
/// The lead walks for the point this far along the carrier's path ahead.
const LEAD_AHEAD: f32 = 700.0;
/// The others keep this far to his left and right, [`FLANK_BEHIND`] back
/// from him against his way: most carriers fell to fire from behind or
/// the side (bot-objectives.md 14.12).
const FLANK_SIDE: f32 = 200.0;
const FLANK_BEHIND: f32 = 200.0;
/// His way: toward the point this far along his path.
const HEADING_AHEAD: f32 = 200.0;
/// A flank point snaps to the graph node nearest it within this, else
/// falls back on the carrier.
const FLANK_SNAP: f32 = 128.0;
/// Within this of his goal the carrier needs no escort: he waits in
/// cover there, or captures (14.11).
const ESCORT_HOME: f32 = 800.0;

/// A side's sighting of an enemy marks the graph round it this long, ms.
pub(crate) const DANGER_MS: i32 = 8000;
/// The edges out of nodes within this of a sighting, across and up or
/// down [`DANGER_RISE`].
const DANGER_NEAR: f32 = 600.0;
const DANGER_RISE: f32 = 200.0;
/// The edges out of nodes within this of an outpost the enemy holds
/// from, across.
const OUTPOST_NEAR: f32 = 300.0;
/// Sightings this near one already kept refresh it.
const DANGER_SAME: f32 = 256.0;

/// [`super::super::CtfView::lead`] for each side's bots near a `ctf`
/// carrier of theirs, `Bot::ctf_escort` and team play on, with no flag of
/// theirs lying out: of the attackers not hunting, the [`ESCORTS`] nearest
/// within [`ESCORT_NEAR`], and with their flag home one of its holders
/// when there are two or more, the nearest; the one nearest the
/// carrier's path [`LEAD_AHEAD`] on walks for that point, the rest
/// behind him to his left and right in turn. None once he stands within
/// [`ESCORT_HOME`] of his goal.
pub(super) fn deal_escort<W: BotWorld>(
    w: &W,
    views: &mut [(usize, BotView)],
    teams: &[i32],
    on_side: &[(usize, i32, [f32; 3])],
) {
    let Some(g) = w.driver().nav.clone() else {
        return;
    };
    for team in [TEAM_AXIS, TEAM_ALLIES] {
        let mine = |s: usize| teams.get(s) == Some(&team);
        let ctf = views
            .iter()
            .filter(|(s, _)| mine(*s))
            .find_map(|(_, v)| match v.uo.as_ref() {
                Some(UoView::Ctf(c)) => Some((c.theirs.carrier, c.theirs.goal.stand, c.ours)),
                _ => None,
            });
        let Some((Some(k), goal, ours)) = ctf else {
            continue;
        };
        if ours.dropped.is_some() {
            continue;
        }
        let Some(&(_, _, at)) = on_side.iter().find(|c| c.0 == k) else {
            continue;
        };
        if dist_sq(at, goal) < ESCORT_HOME * ESCORT_HOME {
            continue;
        }
        let path: Vec<Vec3> = w
            .driver()
            .paths
            .get(&k)
            .map(|f| {
                f.ahead()
                    .map(|(_, b)| g.nodes[b as usize])
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let lead = along(Vec3::from(at), &path, LEAD_AHEAD);
        let ahead = along(Vec3::from(at), &path, HEADING_AHEAD);
        let heading = (ahead - Vec3::from(at))
            .truncate()
            .try_normalize()
            .or_else(|| {
                (Vec3::from(goal) - Vec3::from(at))
                    .truncate()
                    .try_normalize()
            })
            .unwrap_or(glam::Vec2::X);
        let right = Vec3::new(heading.y, -heading.x, 0.0);
        let back = Vec3::from(at) - heading.extend(0.0) * FLANK_BEHIND;
        let flanks = [back + right * FLANK_SIDE, back - right * FLANK_SIDE]
            .map(|p| snap(&g, p).unwrap_or(Vec3::from(at)));
        let on = |s: usize| {
            w.driver()
                .bots
                .get(&s)
                .is_some_and(|b| b.ctf_escort && b.team_play)
        };
        let mut near: Vec<(f32, usize)> = views
            .iter()
            .filter(|(s, v)| *s != k && mine(*s) && v.playing && !v.dead && on(*s))
            .filter(|(_, v)| {
                matches!(v.uo.as_ref(), Some(UoView::Ctf(c))
                    if !c.carrying && c.hunt.is_none() && c.rank % 2 == 0)
            })
            .map(|(s, v)| (dist_sq(v.origin, at), *s))
            .filter(|(d, _)| *d < ESCORT_NEAR * ESCORT_NEAR)
            .collect();
        near.sort_by(|x, y| x.0.total_cmp(&y.0).then(x.1.cmp(&y.1)));
        near.truncate(ESCORTS);
        // With our flag home and two or more holding it, the one of them
        // nearest the carrier walks out to meet him: half the carries had
        // no teammate within 500 units (bot-objectives.md 14.12).
        let mut holders: Vec<(f32, usize)> = views
            .iter()
            .filter(|(s, v)| *s != k && mine(*s) && v.playing && !v.dead && on(*s))
            .filter(|(_, v)| {
                matches!(v.uo.as_ref(), Some(UoView::Ctf(c))
                    if !c.carrying && c.hunt.is_none() && c.rank % 2 == 1)
            })
            .map(|(s, v)| (dist_sq(v.origin, at), *s))
            .collect();
        if ours.home.is_some() && holders.len() >= 2 {
            holders.sort_by(|x, y| x.0.total_cmp(&y.0).then(x.1.cmp(&y.1)));
            near.push(holders[0]);
        }
        let first = near
            .iter()
            .filter_map(|(_, s)| views.iter().find(|(x, _)| x == s))
            .min_by(|a, b| {
                dist_sq(a.1.origin, lead.into())
                    .total_cmp(&dist_sq(b.1.origin, lead.into()))
                    .then(a.0.cmp(&b.0))
            })
            .map(|(s, _)| *s);
        let mut flank = 0;
        for (_, s) in near {
            let to = if Some(s) == first {
                lead
            } else {
                flank += 1;
                flanks[(flank - 1) % 2]
            };
            if let Some((_, v)) = views.iter_mut().find(|(x, _)| *x == s)
                && let Some(UoView::Ctf(c)) = v.uo.as_mut()
            {
                c.lead = Some(to.into());
            }
        }
    }
}

/// A side's `ctf` flag taken from home and carried (bot-objectives.md
/// 14.13): when, where it stood, the graph's way from there to the
/// carrier's goal, and the guards [`deal_take`] keeps on it.
#[derive(Clone, Debug, Default)]
pub(crate) struct Take {
    since: i32,
    home: Vec3,
    exit: Vec<Vec3>,
    /// The guards kept on an outpost looking at the way out
    /// (`Bot::ctf_watch`), and their posts.
    watch: Vec<(usize, GuardPost)>,
    /// The guards closing on the carrier along it (`Bot::ctf_close`).
    close: Vec<usize>,
}

/// Guards within this of their flag's home when it is taken stay on its
/// way out ([`deal_take`]).
const TAKE_NEAR: f32 = 2000.0;
/// A watcher takes an outpost within this of where he stands.
const WATCH_REACH: f32 = 800.0;
/// An outpost sees the way out at a point this near and no nearer than
/// [`WATCH_MIN`].
const WATCH_SEE: f32 = 1500.0;
const WATCH_MIN: f32 = 200.0;
/// Two watchers' outposts stand at least this far apart.
const WATCH_APART: f32 = 200.0;
/// A watcher holds this long past the take, ms; a closer closes this long.
const WATCH_MS: i32 = 15_000;
const CLOSE_MS: i32 = 15_000;
/// The pace a closer reckons the carrier makes along his way, units a
/// second (run speed 190, his way not straight).
const CARRIER_PACE: f32 = 170.0;
/// A closer's point moves on at most this often, ms.
const CLOSE_STEP_MS: i32 = 1000;

/// Keeps per side its flag's home while it stands there (`base`) and,
/// once it is carried, a [`Take`]; while one runs, the guards of the side
/// that stood within [`TAKE_NEAR`] of the home at the take, a guard
/// being a bot ranked to defend with team play on, with `Bot::ctf_watch`
/// hold the outpost round the home (`posts`' `outers`) within
/// [`WATCH_REACH`] that sees the carrier's way out, looking at it toward
/// the home ([`super::super::CtfView::watch`]), for [`WATCH_MS`]; with
/// `Bot::ctf_close` walk for the point on his way nearest them that he
/// has not passed by the pace he makes ([`CARRIER_PACE`]), as a hunt
/// ([`super::super::CtfView::hunt`]), for [`CLOSE_MS`]. A watcher or
/// closer that dies leaves it; a sighting of the carrier sends the
/// nearest two after him as before (`deal_hunt`, which runs after).
pub(super) fn deal_take<W: BotWorld>(w: &mut W, views: &mut [(usize, BotView)], teams: &[i32]) {
    let Some(g) = w.driver().nav.clone() else {
        return;
    };
    let now = w.now();
    let mut takes = std::mem::take(&mut w.driver_mut().ctf_take);
    let mut base = w.driver().ctf_base;
    for (i, team) in [TEAM_AXIS, TEAM_ALLIES].into_iter().enumerate() {
        let mine = |s: usize| teams.get(s) == Some(&team);
        let ours = views
            .iter()
            .filter(|(s, _)| mine(*s))
            .find_map(|(_, v)| match v.uo.as_ref() {
                Some(UoView::Ctf(c)) => Some(c.ours),
                _ => None,
            });
        let Some(ours) = ours else {
            takes[i] = None;
            continue;
        };
        if let Some(h) = ours.home {
            base[i] = Some(h.stand);
            takes[i] = None;
            continue;
        }
        if !ours.carried {
            takes[i] = None;
            continue;
        }
        let Some(home) = base[i] else {
            continue;
        };
        let guard = |s: usize, v: &BotView, on: fn(&super::super::Bot) -> bool| {
            mine(s)
                && v.playing
                && !v.dead
                && v.seat.is_none()
                && matches!(v.uo.as_ref(), Some(UoView::Ctf(c)) if !c.carrying && c.rank % 2 == 1)
                && dist_sq(v.origin, home) < TAKE_NEAR * TAKE_NEAR
                && w.driver()
                    .bots
                    .get(&s)
                    .is_some_and(|b| b.team_play && b.tactics && on(b))
        };
        if takes[i].is_none() {
            let exit: Vec<Vec3> = match (g.nearest(home), g.nearest(ours.goal.stand)) {
                (Some(a), Some(b)) => g
                    .path_toward(a, b)
                    .unwrap_or_default()
                    .into_iter()
                    .map(|n| g.nodes[n as usize])
                    .collect(),
                _ => Vec::new(),
            };
            let mut t = Take {
                since: now,
                home: Vec3::from(home),
                exit,
                ..Default::default()
            };
            let outs = outposts(&w.driver().posts, home);
            if let Some(world) = w.world() {
                let clear = |a: Vec3, b: Vec3| world.collision.shot_trace(a, b).fraction >= 1.0;
                for (s, v) in views.iter() {
                    if !guard(*s, v, |b| b.ctf_watch) {
                        continue;
                    }
                    let feet = Vec3::from(v.origin);
                    let pick = outs
                        .iter()
                        .filter(|p| p.distance(feet) < WATCH_REACH)
                        .filter(|p| {
                            t.watch
                                .iter()
                                .all(|(_, q)| Vec3::from(q.at).distance(**p) >= WATCH_APART)
                        })
                        .filter_map(|p| watch_look(&t.exit, *p, &clear).map(|l| (*p, l)))
                        .min_by(|x, y| x.0.distance(feet).total_cmp(&y.0.distance(feet)));
                    if let Some((at, look)) = pick {
                        t.watch.push((
                            *s,
                            GuardPost {
                                at: at.into(),
                                crouch: false,
                                look: look.into(),
                                careful: None,
                            },
                        ));
                    }
                }
            }
            t.close = views
                .iter()
                .filter(|(s, v)| guard(*s, v, |b| b.ctf_close))
                .map(|(s, _)| *s)
                .collect();
            takes[i] = Some(t);
        }
        let Some(t) = takes[i].as_mut() else {
            continue;
        };
        let alive = |s: usize, views: &[(usize, BotView)]| {
            views
                .iter()
                .any(|(x, v)| *x == s && v.playing && !v.dead && v.seat.is_none())
        };
        t.watch.retain(|(s, _)| alive(*s, views));
        t.close.retain(|s| alive(*s, views));
        let age = now.wrapping_sub(t.since);
        if age < WATCH_MS {
            for (s, p) in &t.watch {
                if let Some((_, v)) = views.iter_mut().find(|(x, _)| x == s)
                    && let Some(UoView::Ctf(c)) = v.uo.as_mut()
                {
                    c.watch = Some(*p);
                    v.guard_post = Some(*p);
                }
            }
        }
        if age < CLOSE_MS && !t.exit.is_empty() {
            // Where he is reckoned to be along his way, by the step.
            let ran = (age / CLOSE_STEP_MS * CLOSE_STEP_MS) as f32 / 1000.0 * CARRIER_PACE;
            let reckoned = index_along(t.home, &t.exit, ran);
            for s in &t.close {
                if let Some((_, v)) = views.iter_mut().find(|(x, _)| x == s)
                    && let Some(UoView::Ctf(c)) = v.uo.as_mut()
                {
                    let feet = Vec3::from(v.origin);
                    let nearest = (0..t.exit.len())
                        .min_by(|a, b| {
                            t.exit[*a]
                                .distance(feet)
                                .total_cmp(&t.exit[*b].distance(feet))
                        })
                        .unwrap_or(0);
                    c.hunt = Some(t.exit[nearest.max(reckoned)].into());
                }
            }
        }
    }
    w.driver_mut().ctf_take = takes;
    w.driver_mut().ctf_base = base;
}

/// Where an outpost at `p` looks along the way out `exit`: of the way's
/// nodes from [`WATCH_MIN`] to [`WATCH_SEE`] off whose chest its standing
/// eye sees, the nearest it of those on the home side of the way's node
/// nearest it, else the nearest it.
fn watch_look(exit: &[Vec3], p: Vec3, clear: &dyn Fn(Vec3, Vec3) -> bool) -> Option<Vec3> {
    const EYE: f32 = vcod_common::pmove::VIEW_STAND;
    let at =
        (0..exit.len()).min_by(|a, b| exit[*a].distance(p).total_cmp(&exit[*b].distance(p)))?;
    let seen: Vec<usize> = (0..exit.len())
        .filter(|&j| {
            let d = exit[j].distance(p);
            (WATCH_MIN..WATCH_SEE).contains(&d)
                && clear(p + Vec3::Z * EYE, exit[j] + Vec3::Z * (EYE * 2.0 / 3.0))
        })
        .collect();
    let near = |js: &mut dyn Iterator<Item = usize>| {
        js.min_by(|a, b| exit[*a].distance(p).total_cmp(&exit[*b].distance(p)))
    };
    near(&mut seen.iter().copied().filter(|j| *j < at))
        .or_else(|| near(&mut seen.iter().copied()))
        .map(|j| exit[j])
}

/// The index of the node of `path` (walked from `from`) `ahead` units
/// along it, its last past its end.
fn index_along(from: Vec3, path: &[Vec3], ahead: f32) -> usize {
    let mut left = ahead;
    let mut at = from;
    for (i, &p) in path.iter().enumerate() {
        let d = at.distance(p);
        if d >= left {
            return i;
        }
        left -= d;
        at = p;
    }
    path.len().saturating_sub(1)
}

/// The point `ahead` units along `path` from `from`, else its last node,
/// else `from`.
fn along(from: Vec3, path: &[Vec3], ahead: f32) -> Vec3 {
    let mut left = ahead;
    let mut at = from;
    for &n in path {
        let d = at.distance(n);
        if d >= left {
            return at + (n - at) * (left / d);
        }
        left -= d;
        at = n;
    }
    at
}

/// The graph node nearest `p` within [`FLANK_SNAP`].
fn snap(g: &NavGraph, p: Vec3) -> Option<Vec3> {
    let n = g.nodes[g.nearest(p.into())? as usize];
    (n.distance(p) < FLANK_SNAP).then_some(n)
}

/// Keeps each side's sightings of enemies (`kept`: team, where, when),
/// adding this tick's from the bots' views and dropping those older than
/// [`DANGER_MS`].
pub(super) fn sightings(
    kept: &mut Vec<(i32, [f32; 3], i32)>,
    views: &[(usize, BotView)],
    teams: &[i32],
    now: i32,
) {
    kept.retain(|(_, _, t)| now.wrapping_sub(*t) < DANGER_MS);
    for (s, v) in views {
        let (Some(&team), Some(e)) = (teams.get(*s), v.enemy) else {
            continue;
        };
        if !v.playing || v.dead {
            continue;
        }
        match kept
            .iter_mut()
            .find(|(t, p, _)| *t == team && dist_sq(*p, e.origin) < DANGER_SAME * DANGER_SAME)
        {
            Some(k) => *k = (team, e.origin, now),
            None => kept.push((team, e.origin, now)),
        }
    }
}

/// Keeps, per side, the stand of the other side's flag while it stands at
/// home: a carrier's danger reads the outposts round it after the take.
pub(super) fn homes(home: &mut [Option<[f32; 3]>; 2], views: &[(usize, BotView)], teams: &[i32]) {
    for (i, team) in [TEAM_AXIS, TEAM_ALLIES].into_iter().enumerate() {
        let at = views
            .iter()
            .filter(|(s, _)| teams.get(*s) == Some(&team))
            .find_map(|(_, v)| match v.uo.as_ref() {
                Some(UoView::Ctf(c)) => c.theirs.home.map(|h| h.stand),
                _ => None,
            });
        if at.is_some() {
            home[i] = at;
        }
    }
}

/// The outposts the guards of a flag at home `at` hold from
/// ([`super::super::posts::outer`], made for a `GuardHold::Far` guard
/// round it).
fn outposts(posts: &super::super::posts::Posts, at: [f32; 3]) -> Vec<Vec3> {
    posts
        .ring_outposts(at)
        .flat_map(|o| o.posts.iter().map(|p| p.at))
        .collect()
}

/// Per side, the edges (`(low, high)`, sorted) out of nodes within
/// [`DANGER_NEAR`] of where it saw enemies of late, and within
/// [`OUTPOST_NEAR`] of the outposts round the other side's flag's home
/// (`home`, [`homes`]): what a carrier's plan pays
/// [`crate::nav::DANGER_COST`] more on.
pub(super) fn danger(
    g: &NavGraph,
    kept: &[(i32, [f32; 3], i32)],
    posts: &super::super::posts::Posts,
    home: [Option<[f32; 3]>; 2],
) -> Vec<(i32, Vec<(u32, u32)>)> {
    let mut out: Vec<(i32, Vec<(u32, u32)>)> = Vec::new();
    for (i, team) in [TEAM_AXIS, TEAM_ALLIES].into_iter().enumerate() {
        let outs = home[i].map(|h| outposts(posts, h)).unwrap_or_default();
        let mut edges: Vec<(u32, u32)> = kept
            .iter()
            .filter(|k| k.0 == team)
            .flat_map(|k| g.nodes_in_box(Vec3::from(k.1), DANGER_NEAR, DANGER_RISE))
            .chain(
                outs.iter()
                    .flat_map(|p| g.nodes_in_box(*p, OUTPOST_NEAR, DANGER_RISE)),
            )
            .flat_map(|a| g.next(a).map(move |b| (a.min(b), a.max(b))))
            .collect();
        edges.sort_unstable();
        edges.dedup();
        if !edges.is_empty() {
            out.push((team, edges));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The point a length along a path: on an edge, at its last node past
    /// its end, at the start with none.
    #[test]
    fn a_point_along_the_path() {
        let path = [Vec3::new(100.0, 0.0, 0.0), Vec3::new(100.0, 300.0, 0.0)];
        assert_eq!(along(Vec3::ZERO, &path, 50.0), Vec3::new(50.0, 0.0, 0.0));
        assert_eq!(
            along(Vec3::ZERO, &path, 200.0),
            Vec3::new(100.0, 100.0, 0.0)
        );
        assert_eq!(along(Vec3::ZERO, &path, 900.0), path[1]);
        assert_eq!(along(Vec3::ONE, &[], 900.0), Vec3::ONE);
    }

    /// A watcher looks at the way out where it sees it, home side of the
    /// way's node nearest it first; a node it cannot see is passed over.
    #[test]
    fn a_watcher_looks_back_along_the_way_out() {
        let exit: Vec<Vec3> = (0..10)
            .map(|i| Vec3::new(i as f32 * 200.0, 0.0, 0.0))
            .collect();
        let p = Vec3::new(1000.0, 300.0, 0.0);
        let all = |_: Vec3, _: Vec3| true;
        assert_eq!(watch_look(&exit, p, &all), Some(Vec3::new(800.0, 0.0, 0.0)));
        let past_home = |_: Vec3, b: Vec3| b.x >= 1000.0;
        assert_eq!(
            watch_look(&exit, p, &past_home),
            Some(Vec3::new(1000.0, 0.0, 0.0))
        );
        assert_eq!(watch_look(&exit, p, &|_, _| false), None);
        assert_eq!(index_along(Vec3::ZERO, &exit, 450.0), 3);
        assert_eq!(index_along(Vec3::ZERO, &exit, 9000.0), 9);
    }
}
