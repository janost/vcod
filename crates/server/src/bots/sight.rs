//! What a bot sees, hears and feels: the sight cone, the enemy pick, the aim point, friends in the
//! line, frags near, gunfire, the hits it takes, noises, and the spots it remembers.

use super::*;

/// A burning UO hull is kept this far off, across and up or down, until
/// its second blast: `tank_death`'s `radiusDamage` of 300 from 25 over its
/// origin meets a body's box 315 off, and 45 more is the walk's slack
/// (bot-objectives.md 10.10).
pub(crate) const WRECK_REACH: f32 = 360.0;

/// The nearest of `wrecks` ([`BotView::wrecks`]) with `p` inside its
/// [`WRECK_REACH`].
pub(crate) fn wreck_near(wrecks: &[[f32; 3]], p: [f32; 3]) -> Option<[f32; 3]> {
    let across = |w: &[f32; 3]| (w[0] - p[0]).hypot(w[1] - p[1]);
    wrecks
        .iter()
        .filter(|w| across(w) < WRECK_REACH && (w[2] - p[2]).abs() < WRECK_REACH)
        .min_by(|a, b| across(a).total_cmp(&across(b)))
        .copied()
}
/// Another's frag is run from only once noticed ([`notices_frag`]): in a
/// clear line and within this of the view's yaw either way, or this near.
const FRAG_SEEN_DEG: f32 = 60.0;
const FRAG_HEARD: f32 = 150.0;

/// Whether a bot whose eye is at `eye`, looking along `yaw` (degrees),
/// notices someone else's live frag at `frag`: in a clear line (`clear`)
/// and in front, or close enough to hear it land. Run from wherever it lay,
/// no frag ever went off near a bot (0 kills of 718 throws over the
/// `tdm` table's 16 matches, bot-combat.md 6.5); CoD 1 draws no grenade
/// indicator.
pub(crate) fn notices_frag(
    eye: glam::Vec3,
    yaw: f32,
    frag: glam::Vec3,
    clear: impl Fn(glam::Vec3, glam::Vec3) -> bool,
) -> bool {
    let to = frag - eye;
    let ahead = yaw_diff(to.y.atan2(to.x).to_degrees(), yaw).abs() < FRAG_SEEN_DEG;
    (ahead || to.length() < FRAG_HEARD) && clear(eye, frag)
}
/// How far a bot hears gunfire and a blast, units. The stock aliases carry
/// `dist_max` 7800 for every rifle and gun `_fire` and 6000 for
/// `grenade_explode_*` (`soundaliases/iw_sound.csv`), which covers most of a
/// map; these keep the same ratio at a range that leaves bots roaming.
pub(crate) const HEAR_GUNFIRE: f32 = 2000.0;
pub(crate) const HEAR_BLAST: f32 = 1500.0;

/// The nearest of `frags` (where it lies, and whether the bot knows of it
/// already, its own one) within [`FLEE_REACH`] across and [`FLEE_HEIGHT`]
/// up or down of `at`: one known, or one `notices` says the bot notices
/// ([`notices_frag`]).
pub(crate) fn frag_near(
    at: glam::Vec3,
    frags: impl IntoIterator<Item = (glam::Vec3, bool)>,
    notices: impl Fn(glam::Vec3) -> bool,
) -> Option<[f32; 3]> {
    frags
        .into_iter()
        .filter(|&(o, known)| known || notices(o))
        .map(|(o, _)| o)
        .filter(|o| (o.z - at.z).abs() < FLEE_HEIGHT && (*o - at).truncate().length() < FLEE_REACH)
        .min_by(|a, b| a.distance(at).total_cmp(&b.distance(at)))
        .map(Into::into)
}

/// Whether the client in `slot` fired since `heard`'s last read of its
/// event ring (`events` at `eventSequence` `seq`, a shot one of `fire`);
/// the read is kept.
pub(crate) fn fired<E: PartialEq>(
    heard: &mut std::collections::HashMap<usize, i32>,
    slot: usize,
    seq: i32,
    events: &[E; 4],
    fire: &[E],
) -> bool {
    let prev = heard.insert(slot, seq).unwrap_or(seq);
    (prev.max(seq - 4)..seq).any(|k| fire.contains(&events[(k & 3) as usize]))
}

/// The gunfire the client in `source` makes, chest high on its body at
/// `origin` with its eye `view_height` over it.
pub(crate) fn gunfire(source: usize, origin: [f32; 3], view_height: f32) -> Noise {
    Noise {
        at: chest(origin, view_height),
        source,
        radius: HEAR_GUNFIRE,
    }
}

/// The blast of `source`'s missile going off at `at`.
pub(crate) fn blast(source: usize, at: [f32; 3]) -> Noise {
    Noise {
        at,
        source,
        radius: HEAR_BLAST,
    }
}

/// A hit a bot felt: the attacker, the time it was read, the direction it
/// came along.
pub(crate) type HitFelt = (usize, i32, [f32; 3]);

/// A bot's hit off its `damageEvent` latch (the event last read and the
/// hit it gave): an `event` other than the last is a hit by `attacker`,
/// the client last named with the time he hit, if that was under a second
/// before `now`, along `dir` (zero for none).
pub(crate) fn feel_hit<E: PartialEq>(
    latch: &mut (E, Option<HitFelt>),
    event: E,
    attacker: Option<(usize, i32)>,
    dir: impl FnOnce() -> Option<glam::Vec3>,
    now: i32,
) -> Option<HitFelt> {
    if latch.0 != event {
        latch.0 = event;
        latch.1 = attacker
            .filter(|&(_, t)| (0..1000).contains(&now.wrapping_sub(t)))
            .map(|(a, _)| (a, now, dir().unwrap_or(glam::Vec3::ZERO).to_array()));
    }
    latch.1
}

/// [`BotView::hurt`] off the hit [`feel_hit`] gave: reported for
/// [`HURT_MS`] while `plays` says the attacker still plays.
pub(crate) fn hurt_of(
    hit: Option<HitFelt>,
    now: i32,
    plays: impl Fn(usize) -> bool,
) -> Option<Hurt> {
    let (a, at, dir) = hit?;
    let ago_ms = now.wrapping_sub(at);
    if !(0..HURT_MS).contains(&ago_ms) || !plays(a) {
        return None;
    }
    Some(Hurt {
        slot: a,
        ago_ms,
        dir,
    })
}
/// A lost enemy is remembered this far along its last velocity, seconds.
const MEMORY_LEAD_S: f32 = 0.5;

/// A tracked target in sight is kept over a new one unless the new one
/// stands nearer than this share of its range: picked nearest-first every
/// refresh, a bot between two enemies at like range swapped targets, and
/// paid a fresh reaction and full aim error, at each swap.
pub(crate) const RETARGET_RATIO: f32 = 0.6;

/// Whether `at` lies inside the sight cone `fov` degrees across about the
/// view `view` (wire degrees, pitch down) from `eye`.
pub(crate) fn in_sight_cone(eye: glam::Vec3, view: [f32; 3], at: glam::Vec3, fov: f32) -> bool {
    let [pitch, yaw, _] = view.map(f32::to_radians);
    let forward = glam::Vec3::new(
        pitch.cos() * yaw.cos(),
        pitch.cos() * yaw.sin(),
        -pitch.sin(),
    );
    fov >= 360.0 || (at - eye).normalize_or_zero().dot(forward) >= (fov * 0.5).to_radians().cos()
}

/// The enemy a bot fights from `order`, the candidates' (squared range,
/// slot) nearest first; `sighted(i)` traces the `i`th and gives its view
/// when the eye line reaches it. The `attacker` of a fresh hit in sight
/// comes first, then a new one nearer than [`RETARGET_RATIO`] of the
/// `tracked` target's range, then the tracked one, then the nearest.
pub(crate) fn pick_enemy(
    order: &[(f32, usize)],
    attacker: Option<usize>,
    tracked: Option<usize>,
    sighted: impl Fn(usize) -> Option<EnemyView>,
) -> Option<EnemyView> {
    let find = |slot: Option<usize>| {
        let i = order.iter().position(|c| Some(c.1) == slot)?;
        Some((order[i].0, sighted(i)?))
    };
    if let Some((_, e)) = find(attacker) {
        return Some(e);
    }
    let held = find(tracked);
    let reach = held.map_or(f32::INFINITY, |(d, _)| d * RETARGET_RATIO * RETARGET_RATIO);
    order
        .iter()
        .enumerate()
        .filter(|(_, c)| c.0 < reach && Some(c.1) != tracked && Some(c.1) != attacker)
        .find_map(|(i, _)| sighted(i))
        .or(held.map(|(_, e)| e))
}

/// The point a bot aims at on a body standing at `origin` with its eye
/// `eye_height` over it: two thirds of the way up to the eye, 40 on a
/// standing body, 26.7 crouched, 7.3 prone, so a crouched or prone enemy
/// is not shot over.
pub(crate) fn chest(origin: [f32; 3], eye_height: f32) -> [f32; 3] {
    [origin[0], origin[1], origin[2] + eye_height * (2.0 / 3.0)]
}

/// Where a bot aims at an enemy whose chest is `chest` and eye `eye`, given
/// whether the line from its own eye reaches a point: the chest, or over a
/// wall that covers the chest but not the eye, the head; `None` when the
/// line reaches neither. Aimed at a chest behind cover, every round goes
/// into the cover.
pub(crate) fn aim_point(
    chest: [f32; 3],
    eye: [f32; 3],
    clear: impl Fn([f32; 3]) -> bool,
) -> Option<[f32; 3]> {
    [chest, eye].into_iter().find(|p| clear(*p))
}

/// A client a bot may fight, as its game's playerstate places it.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Seen {
    pub(crate) slot: usize,
    pub(crate) origin: glam::Vec3,
    pub(crate) eye: glam::Vec3,
    /// The eye's height over `origin`, which places the chest ([`chest`]).
    pub(crate) view_height: f32,
    pub(crate) velocity: [f32; 3],
}

/// The bot looking for an enemy: where it stands, its eye and view (wire
/// degrees), its sight cone, and the target it tracks and the attacker of a
/// fresh hit, both seen wherever they stand.
pub(crate) struct Looker {
    pub(crate) origin: glam::Vec3,
    pub(crate) eye: glam::Vec3,
    pub(crate) view: [f32; 3],
    pub(crate) fov: f32,
    pub(crate) tracked: Option<usize>,
    pub(crate) attacker: Option<usize>,
}

/// The target `bot` tracks while aware, and its sight cone; a slot with no
/// brain sees with the default skill.
pub(crate) fn tracked_and_fov(bot: Option<&Bot>) -> (Option<usize>, f32) {
    let tracked = bot
        .and_then(|b| b.target_slot())
        .filter(|_| bot.is_none_or(|b| b.aware));
    let fov = bot.map_or(Skill::default().fov_deg, |b| b.skill.fov_deg);
    (tracked, fov)
}

/// Whether a client on `theirs` is a foe of one on `mine`: everyone in `dm`
/// and to a player on neither side, else only the other side.
pub(crate) fn foe(dm: bool, mine: i32, theirs: i32) -> bool {
    use crate::game::script::{TEAM_ALLIES, TEAM_AXIS};
    dm || !(mine == TEAM_AXIS || mine == TEAM_ALLIES)
        || (theirs != mine && (theirs == TEAM_AXIS || theirs == TEAM_ALLIES))
}

/// Whether `a` and `b` are on one side, axis or allies.
pub(crate) fn same_team(a: Option<i32>, b: Option<i32>) -> bool {
    use crate::game::script::{TEAM_ALLIES, TEAM_AXIS};
    a.is_some_and(|a| (a == TEAM_AXIS || a == TEAM_ALLIES) && Some(a) == b)
}

/// The enemy in sight `me` fights of `foes` ([`pick_enemy`]): those within
/// [`SHOOT_RANGE`], the tracked one and the attacker wherever they stand
/// and the rest inside the sight cone, nearest first and by slot on a tie,
/// each aimed at where `clear` says the eye line reaches ([`aim_point`]).
pub(crate) fn sight_enemy(
    me: &Looker,
    foes: impl IntoIterator<Item = Seen>,
    clear: impl Fn([f32; 3]) -> bool,
) -> Option<EnemyView> {
    let mut candidates: Vec<(f32, glam::Vec3, EnemyView)> = Vec::new();
    for s in foes {
        let d = dist_sq(me.origin.into(), s.origin.into());
        if d > SHOOT_RANGE * SHOOT_RANGE {
            continue;
        }
        let chest = chest(s.origin.into(), s.view_height);
        let known = Some(s.slot) == me.tracked || Some(s.slot) == me.attacker;
        if !known && !in_sight_cone(me.eye, me.view, chest.into(), me.fov) {
            continue;
        }
        candidates.push((
            d,
            s.eye,
            EnemyView {
                slot: s.slot,
                origin: chest,
                velocity: s.velocity,
            },
        ));
    }
    candidates.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.2.slot.cmp(&b.2.slot)));
    let sighted = |i: usize| {
        let (_, eye, e) = &candidates[i];
        let origin = aim_point(e.origin, (*eye).into(), &clear)?;
        Some(EnemyView { origin, ..*e })
    };
    let order: Vec<(f32, usize)> = candidates.iter().map(|c| (c.0, c.2.slot)).collect();
    pick_enemy(&order, me.attacker, me.tracked, sighted)
}

/// How long a bot keeps walking to one sidestep before it picks again.
const SIDESTEP_HOLD_MS: i32 = 3000;
/// A sidestep this near across is reached: the bot picks again if the line
/// is still blocked from there.
const SIDESTEP_REACHED: f32 = 16.0;

/// The sidestep a bot at `at` keeps walking to: `held`, the spot and the
/// sv_time it was picked, while it is not yet reached, not
/// [`SIDESTEP_HOLD_MS`] old and `clear` still passes it. Picked afresh
/// every tick, the nearest spot moves with the body and a moving
/// teammate's line, and the bot jitters between spots.
pub(crate) fn held_sidestep(
    held: Option<([f32; 3], i32)>,
    now_ms: i32,
    at: glam::Vec3,
    clear: impl Fn(glam::Vec3) -> bool,
) -> Option<[f32; 3]> {
    let (p, since) = held?;
    let v = glam::Vec3::from(p);
    (now_ms.wrapping_sub(since) < SIDESTEP_HOLD_MS
        && (v - at).truncate().length() > SIDESTEP_REACHED
        && clear(v))
    .then_some(p)
}

/// How far round a teammate's box a shot is held, units: aim error and
/// spread put a round a few units off the line.
const FRIEND_MARGIN: f32 = 8.0;

/// Whether the line from `eye` to `target` crosses any of `friends`' boxes
/// (`(mins, maxs)`, world), each grown by [`FRIEND_MARGIN`], short of the
/// target: a round fired at the target would strike a teammate first.
pub(crate) fn friend_in_line(
    eye: glam::Vec3,
    target: glam::Vec3,
    friends: &[(glam::Vec3, glam::Vec3)],
) -> bool {
    let m = glam::Vec3::splat(FRIEND_MARGIN);
    friends
        .iter()
        .any(|(lo, hi)| crate::game::combat::ray_box(eye, target, *lo - m, *hi + m).is_some())
}

/// The noise `listener` hears loudest from `at`: the one nearest relative
/// to its range scaled by `hearing` ([`Skill::hearing`]), first in recorded
/// order on a tie. Teammates' fights count too: a friend shooting means an
/// enemy close to him. The server leaves out those its teammates nearer the
/// noise answer instead (`team::answers`).
pub(crate) fn loudest(
    noises: &[Noise],
    listener: usize,
    at: [f32; 3],
    hearing: f32,
) -> Option<[f32; 3]> {
    noises
        .iter()
        .filter(|n| n.source != listener)
        .map(|n| (dist_sq(at, n.at).sqrt() / (n.radius * hearing), n.at))
        .filter(|(f, _)| *f < 1.0)
        .min_by(|a, b| a.0.total_cmp(&b.0))
        .map(|(_, p)| p)
}

/// A spot worth a look: where a seen enemy was lost, or a noise.
#[derive(Clone, Copy, Debug)]
pub(super) struct Recall {
    pub(super) at: [f32; 3],
    /// Ticks before it is dropped.
    pub(super) ticks: u32,
    /// A seen enemy's spot; a noise does not replace it.
    pub(super) seen: bool,
}

impl Bot {
    /// The remembered spot not yet reached, else this tick's noise. The same
    /// rule [`Bot::remember`] applies a tick later, so the two agree.
    pub(super) fn recall_goal(&self, view: &BotView) -> Option<[f32; 3]> {
        self.hunt_goal(view).or_else(|| {
            view.noise
                .filter(|n| !arrived(view.origin, *n) && !self.passes_over(*n))
        })
    }

    /// Gives up a remembered or heard spot the bot cannot close on: its
    /// follower walked out the plan to it ([`Bot::walked_to`]) and the body
    /// came no [`super::SHORT_GAIN`] nearer in [`super::SHORT_TICKS`]. The
    /// spot is passed over for [`super::SHUN_TICKS`], a noise heard there
    /// again included. A tank's shells struck a wall 272 units over the
    /// floor of mp_cassino's (2950, -2892), the plan to each blast ended on
    /// that floor, and a bot pressed the wall under it for 12 s, every shell
    /// renewing the spot (bot-objectives.md 14.14).
    pub(super) fn track_recall(&mut self, view: &BotView) {
        let spot = self.recall.map(|r| r.at).or(view.noise);
        let Some(p) = spot.filter(|p| self.walked_to == Some(*p)) else {
            self.recall_short = None;
            return;
        };
        let d = dist_sq(view.origin, p).sqrt();
        let (best, ticks) = match self.recall_short {
            Some((at, best, ticks)) if at == p && d > best - super::SHORT_GAIN => (best, ticks + 1),
            _ => (d, 0),
        };
        if ticks >= super::SHORT_TICKS {
            self.shunned.push((p, super::SHUN_TICKS));
            self.recall = None;
            self.recall_short = None;
        } else {
            self.recall_short = Some((p, best, ticks));
        }
    }

    /// Keeps [`Bot::recall`] current: a visible enemy is remembered where it
    /// is headed; out of sight the memory runs down and is dropped on
    /// arrival; a noise replaces anything but a seen enemy's spot.
    pub(super) fn remember(&mut self, view: &BotView) {
        if let Some(e) = view.enemy {
            let lead = |i: usize| e.origin[i] + e.velocity[i] * MEMORY_LEAD_S;
            self.recall = Some(Recall {
                at: [lead(0), lead(1), e.origin[2]],
                ticks: self.skill.memory_ticks,
                seen: true,
            });
            return;
        }
        if let Some(r) = self.recall.as_mut() {
            r.ticks = r.ticks.saturating_sub(1);
        }
        let hunted = self.hunt_arrived(view);
        self.hunts += u32::from(hunted);
        if hunted
            || self
                .recall
                .is_some_and(|r| r.ticks == 0 || arrived(view.origin, r.at))
        {
            self.recall = None;
        }
        if let Some(at) = view.noise
            && !self.recall.is_some_and(|r| r.seen)
            && !self.passes_over(at)
        {
            self.recall = Some(Recall {
                at,
                ticks: self.skill.noise_ticks,
                seen: false,
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bots::tests::*;

    #[test]
    fn a_lost_enemy_is_looked_for_where_it_was_headed_until_memory_runs_out() {
        let (mut bot, v) = lost([500.0, 0.0, 104.0], [200.0, 0.0, 0.0]);
        bot.wary = false;
        let spot = Goal::To([500.0 + 200.0 * MEMORY_LEAD_S, 0.0, 104.0]);
        assert_eq!(bot.goal(&v), spot);
        for _ in 1..bot.skill.memory_ticks {
            bot.think(&v);
        }
        assert_eq!(bot.goal(&v), spot, "forgotten early");
        bot.think(&v);
        assert_eq!(bot.goal(&v), Goal::Roam, "never forgotten");
    }

    /// A wary bot heads for the server's vantage over the spot, and gives
    /// the spot up before its memory of it runs out.
    #[test]
    fn a_wary_bot_hunts_from_the_vantage_and_gives_up_sooner() {
        let (mut bot, mut v) = lost([500.0, 0.0, 104.0], [0.0; 3]);
        assert_eq!(bot.hunt_query(), Some([500.0, 0.0, 104.0]));
        v.hunt = Some([400.0, 150.0, 64.0]);
        assert_eq!(bot.goal(&v), Goal::To([400.0, 150.0, 64.0]));
        let hunt = 70.min(bot.skill.memory_ticks);
        assert!(hunt < bot.skill.memory_ticks);
        for _ in 1..hunt {
            bot.think(&v);
        }
        assert_eq!(bot.goal(&v), Goal::To([400.0, 150.0, 64.0]));
        bot.think(&v);
        bot.think(&v);
        assert_eq!(bot.goal(&v), Goal::Roam, "hunted past its budget");
        assert_eq!(bot.hunt_query(), None);
        assert!(bot.recall.is_some(), "the spot is still remembered");
    }

    #[test]
    fn reaching_the_vantage_ends_the_hunt() {
        let (mut bot, mut v) = lost([500.0, 0.0, 104.0], [0.0; 3]);
        v.hunt = Some([400.0, 150.0, 64.0]);
        v.origin = [390.0, 140.0, 64.0];
        bot.think(&v);
        assert!(bot.recall.is_none());
        assert_eq!(bot.tactics()[2], 1);
    }

    #[test]
    fn reaching_the_remembered_spot_with_nothing_there_forgets_it() {
        let (mut bot, mut v) = lost([500.0, 0.0, 104.0], [0.0; 3]);
        v.origin = [470.0, 10.0, 64.0];
        assert_eq!(bot.goal(&v), Goal::Roam);
        bot.think(&v);
        v.origin = [0.0, 0.0, 64.0];
        assert_eq!(bot.goal(&v), Goal::Roam, "the reached spot came back");
    }

    #[test]
    fn a_heard_noise_is_a_goal_that_outlasts_the_tick() {
        let mut bot = Bot::new("allies", false, 1);
        let mut v = view();
        v.noise = Some([0.0, 900.0, 104.0]);
        assert_eq!(bot.goal(&v), Goal::To([0.0, 900.0, 104.0]));
        bot.think(&v);
        v.noise = None;
        assert_eq!(bot.goal(&v), Goal::To([0.0, 900.0, 104.0]));
    }

    /// bot-objectives.md 14.14: a heard spot whose plan the follower
    /// walked out, the body coming no nearer it, is given up after
    /// `SHORT_TICKS`, and the same noise heard again does not bring it
    /// back. One still walking its plan keeps it.
    #[test]
    fn a_heard_spot_no_body_reaches_is_given_up() {
        let mut bot = Bot::new("allies", false, 1);
        let mut v = view();
        let spot = [0.0, 900.0, 376.0];
        v.noise = Some(spot);
        for _ in 0..=super::super::SHORT_TICKS {
            bot.think(&v);
        }
        assert_eq!(bot.goal(&v), Goal::To(spot), "still walking the plan");
        for _ in 0..=super::super::SHORT_TICKS {
            bot.walked_to = Some(spot);
            bot.think(&v);
        }
        assert_eq!(bot.goal(&v), Goal::Roam, "the noise came back");
        v.noise = None;
        assert_eq!(bot.goal(&v), Goal::Roam, "the spot is still remembered");
    }

    #[test]
    fn a_seen_enemys_spot_beats_a_noise() {
        let (mut bot, mut v) = lost([500.0, 0.0, 104.0], [0.0; 3]);
        v.noise = Some([0.0, 900.0, 104.0]);
        assert_eq!(bot.goal(&v), Goal::To([500.0, 0.0, 104.0]));
        bot.think(&v);
        v.noise = None;
        assert_eq!(bot.goal(&v), Goal::To([500.0, 0.0, 104.0]));
        // Once the spot is reached, the noise is what is left to check.
        v.origin = [500.0, 0.0, 64.0];
        v.noise = Some([0.0, 900.0, 104.0]);
        bot.think(&v);
        v.noise = None;
        assert_eq!(bot.goal(&v), Goal::To([0.0, 900.0, 104.0]));
    }

    #[test]
    fn the_loudest_noise_is_the_nearest_for_its_range_and_never_ones_own() {
        let at = [0.0, 0.0, 0.0];
        let noise = |x: f32, source, radius| Noise {
            at: [x, 0.0, 0.0],
            source,
            radius,
        };
        let heard = |n: &[Noise]| loudest(n, 1, at, 1.0);
        assert_eq!(heard(&[noise(10.0, 1, HEAR_GUNFIRE)]), None, "own shot");
        assert_eq!(heard(&[noise(HEAR_BLAST + 1.0, 2, HEAR_BLAST)]), None);
        // 1800 of 2000 (0.90) is louder than 1400 of 1500 (0.93).
        let far_shot = noise(1800.0, 2, HEAR_GUNFIRE);
        let near_blast = noise(1400.0, 3, HEAR_BLAST);
        assert_eq!(heard(&[near_blast, far_shot]), Some(far_shot.at));
    }

    /// A sidestep is held until reached, blocked or stale, so a teammate
    /// moving across the line does not swing the bot between spots.
    #[test]
    fn a_sidestep_is_held_until_reached_or_blocked() {
        let spot = [64.0, 0.0, 0.0];
        let at = glam::Vec3::ZERO;
        let all = |_: glam::Vec3| true;
        assert_eq!(held_sidestep(Some((spot, 1000)), 1500, at, all), Some(spot));
        assert_eq!(held_sidestep(None, 1500, at, all), None);
        let near = glam::Vec3::new(56.0, 4.0, 18.0);
        assert_eq!(held_sidestep(Some((spot, 1000)), 1500, near, all), None);
        assert_eq!(held_sidestep(Some((spot, 1000)), 1500, at, |_| false), None);
        let stale = 1000 + SIDESTEP_HOLD_MS;
        assert_eq!(held_sidestep(Some((spot, 1000)), stale, at, all), None);
    }

    #[test]
    fn the_attacker_then_the_held_target_then_the_nearest() {
        let e = |slot| EnemyView {
            slot,
            origin: [0.0; 3],
            velocity: [0.0; 3],
        };
        // Slots 1, 2, 3 at 100, 200, 300 units; slot 3 out of sight.
        let order = [(1e4, 1), (4e4, 2), (9e4, 3)];
        let seen = |i: usize| (order[i].1 != 3).then(|| e(order[i].1));
        let pick = |a, t| pick_enemy(&order, a, t, seen).map(|e| e.slot);
        assert_eq!(pick(None, None), Some(1));
        assert_eq!(pick(Some(2), Some(1)), Some(2));
        // Unseen, the attacker gives way.
        assert_eq!(pick(Some(3), None), Some(1));
        // Slot 1 is under 0.6 of the held slot 2's range; nothing is
        // nearer than 0.6 of slot 1's.
        assert_eq!(pick(None, Some(2)), Some(1));
        let order = [(2e4, 1), (4e4, 2)];
        let seen = |i: usize| Some(e(order[i].1));
        assert_eq!(
            pick_enemy(&order, None, Some(2), seen).map(|e| e.slot),
            Some(2)
        );
    }

    /// Out of range or behind is not seen unless it just hit the bot; two
    /// at one range go by slot, whatever order the game lists them in.
    #[test]
    fn the_sight_pick_takes_the_nearest_in_the_cone_by_slot_on_a_tie() {
        let seen = |slot, x: f32, y: f32| Seen {
            slot,
            origin: glam::Vec3::new(x, y, 0.0),
            eye: glam::Vec3::new(x, y, 60.0),
            view_height: 60.0,
            velocity: [0.0; 3],
        };
        let me = |attacker| Looker {
            origin: glam::Vec3::ZERO,
            eye: glam::Vec3::new(0.0, 0.0, 60.0),
            view: [0.0; 3],
            fov: 90.0,
            tracked: None,
            attacker,
        };
        let pick = |me: &Looker, foes: Vec<Seen>| sight_enemy(me, foes, |_| true).map(|e| e.slot);
        let tie = vec![
            seen(5, 300.0, 0.0),
            seen(2, 0.0, 300.0),
            seen(3, 300.0, 0.0),
        ];
        assert_eq!(pick(&me(None), tie), Some(3));
        let behind = vec![seen(1, -200.0, 0.0), seen(4, SHOOT_RANGE + 10.0, 0.0)];
        assert_eq!(pick(&me(None), behind.clone()), None);
        assert_eq!(pick(&me(Some(1)), behind), Some(1));
        let e = sight_enemy(&me(None), [seen(6, 400.0, 0.0)], |p| p[2] > 50.0).unwrap();
        assert_eq!(
            e.origin,
            [400.0, 0.0, 60.0],
            "the chest is covered: the head"
        );
    }

    /// A shot is heard once, off the events past the last read; a ring
    /// that ran more than 4 past it is read from its oldest.
    #[test]
    fn gunfire_is_heard_once_off_the_ring_past_the_last_read() {
        let mut heard = std::collections::HashMap::new();
        let fire = [7, 8];
        assert!(
            !fired(&mut heard, 1, 10, &[7, 7, 7, 7], &fire),
            "the first read"
        );
        assert!(
            fired(&mut heard, 1, 11, &[0, 0, 7, 0], &fire),
            "event 10 sits at 10 & 3"
        );
        assert!(
            !fired(&mut heard, 1, 11, &[0, 0, 7, 0], &fire),
            "read twice"
        );
        assert!(fired(&mut heard, 1, 20, &[8, 0, 0, 0], &fire));
    }

    /// A new `damageEvent` is a hit by the attacker named under a second
    /// before; the same event read again keeps the hit until `HURT_MS`.
    #[test]
    fn a_hit_is_felt_off_a_new_damage_event_for_hurt_ms() {
        let dir = || Some(glam::Vec3::X);
        let mut latch = (0u32, None);
        assert_eq!(feel_hit(&mut latch, 0, Some((3, 900)), dir, 1000), None);
        let hit = feel_hit(&mut latch, 1, Some((3, 900)), dir, 1000);
        assert_eq!(hit, Some((3, 1000, [1.0, 0.0, 0.0])));
        assert_eq!(feel_hit(&mut latch, 1, None, dir, 1500), hit);
        assert_eq!(hurt_of(hit, 1500, |_| true).map(|h| h.ago_ms), Some(500));
        assert_eq!(hurt_of(hit, 1000 + HURT_MS, |_| true), None);
        assert_eq!(hurt_of(hit, 1500, |_| false), None, "the attacker left");
        assert_eq!(
            feel_hit(&mut latch, 2, Some((3, 0)), dir, 1500),
            None,
            "stale"
        );
    }

    #[test]
    fn the_sight_cone_is_about_the_view() {
        let eye = glam::Vec3::ZERO;
        let ahead = glam::Vec3::new(100.0, 0.0, 0.0);
        let side = glam::Vec3::new(0.0, 100.0, 0.0);
        assert!(in_sight_cone(eye, [0.0; 3], ahead, 120.0));
        assert!(!in_sight_cone(eye, [0.0; 3], side, 120.0));
        assert!(in_sight_cone(eye, [0.0, 90.0, 0.0], side, 120.0));
        assert!(in_sight_cone(eye, [0.0; 3], side, 181.0));
        assert!(in_sight_cone(eye, [0.0; 3], -ahead, 360.0));
        // Pitch is down positive: a view 80 down misses a level target.
        assert!(!in_sight_cone(eye, [80.0, 0.0, 0.0], ahead, 120.0));
    }

    /// A wall 50 high between a level bot and a standing enemy covers the
    /// chest (40) but not the eye (60): the bot aims at the head.
    #[test]
    fn a_chest_behind_cover_is_aimed_over() {
        let (chest, eye) = ([500.0, 0.0, 40.0], [500.0, 0.0, 60.0]);
        let over_wall = |p: [f32; 3]| p[2] > 50.0;
        assert_eq!(aim_point(chest, eye, over_wall), Some(eye));
        assert_eq!(aim_point(chest, eye, |_| true), Some(chest));
        assert_eq!(aim_point(chest, eye, |_| false), None);
    }

    /// Another's frag is noticed in front in a clear line, or close by;
    /// one behind or out of the line goes unnoticed.
    #[test]
    fn a_frag_is_noticed_in_front_or_close() {
        let eye = glam::Vec3::new(0.0, 0.0, 60.0);
        let open = |_, _| true;
        let notices = |at: [f32; 3], clear: &dyn Fn(glam::Vec3, glam::Vec3) -> bool| {
            notices_frag(eye, 0.0, at.into(), clear)
        };
        assert!(notices([300.0, 50.0, 0.0], &open));
        assert!(!notices([-300.0, 0.0, 0.0], &open), "seen behind");
        assert!(notices([-100.0, 0.0, 0.0], &open), "not heard close");
        assert!(
            !notices([300.0, 0.0, 0.0], &|_, _| false),
            "seen through a wall"
        );
    }
}
