//! What a bot sees and hears: the sight cone, the enemy pick, the aim point, friends in the line,
//! noises, and the spots it remembers.

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
/// A lost enemy is remembered this far along its last velocity, seconds.
pub(super) const MEMORY_LEAD_S: f32 = 0.5;

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

/// How long a bot keeps walking to one sidestep before it picks again.
pub(super) const SIDESTEP_HOLD_MS: i32 = 3000;
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
        self.hunt_goal(view)
            .or_else(|| view.noise.filter(|n| !arrived(view.origin, *n)))
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
        {
            self.recall = Some(Recall {
                at,
                ticks: self.skill.noise_ticks,
                seen: false,
            });
        }
    }
}
