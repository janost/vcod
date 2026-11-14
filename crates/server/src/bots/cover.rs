//! Fighting from cover (docs/research/bot-combat.md). In a fight the server
//! looks for a graph node near the bot that hides its body from the threat's
//! eye while a peek from it, standing up out of a crouch or leaning out with
//! retail's lean keys, still has a line to the threat ([`find`]). The brain
//! walks there, fights from the peek and drops back behind the cover while it
//! reloads or rechambers ([`Bot::cover_keys`]).

use super::{Bot, BotView, EnemyView, ObjTarget, aim_angles, yaw_diff};
use crate::nav::NavGraph;
use glam::Vec3;
use vcod_common::net::msg::{self, UserCmd};
use vcod_common::pmove::{self, aim};

/// How the body shows itself at a cover spot.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Peek {
    /// Crouched, the body is hidden; standing, the eye sees the threat.
    Stand,
    /// Standing, the body is hidden; leaning this way (-1 left, 1 right),
    /// the eye sees the threat.
    Lean(i8),
    /// Crouched, the body is hidden; leaning this way crouched, the eye
    /// sees the threat. Only ever a [`Spot::alt`].
    LowLean(i8),
}

/// A spot to fight from, as the server found it ([`BotView::cover`]).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Spot {
    /// A graph node: the feet.
    pub at: [f32; 3],
    pub peek: Peek,
    /// A second way to show from the same spot, other than [`Self::peek`]
    /// ([`Sight::alt`]).
    pub alt: Option<Peek>,
    /// The next nearest spot in reach at least [`SHIFT_MIN`] from this one,
    /// and its peek: where a wary bot shifts between peeks.
    pub next: Option<([f32; 3], Peek)>,
}

impl Spot {
    /// A spot with no second peek and no neighbour.
    pub fn new(at: [f32; 3], peek: Peek) -> Spot {
        Spot {
            at,
            peek,
            alt: None,
            next: None,
        }
    }
}

/// A cover spot is looked for this far along level edges from the body.
pub const REACH: f32 = 128.0;
/// Level edge rings the search walks out from the nearest node.
const RINGS: usize = 5;
/// Inside this range of the threat the bot fights where it stands: a run to
/// cover there is a run under fire.
pub const MIN_RANGE: f32 = 300.0;
/// The body's half-width across the line its points are tested at: a little
/// under the box's 15, since the eye sits mid-body and a lean only moves it
/// 20 (15 standing on 1.5 and UO).
const SHOULDER: f32 = 10.0;
/// A spot this near across is reached.
const ARRIVE: f32 = 12.0;
/// Past this far from the spot the sights come down for the walk.
const ADS_WALK: f32 = 64.0;
/// Inside this far from the spot the keys ease off, so the body comes to
/// rest on it rather than run past and back.
const SLOW: f32 = 48.0;
/// Weapon time left past which the body drops back behind the cover: a
/// reload or a bolt's rechamber, not a semi-automatic's tap spacing.
const HIDE_BUSY_MS: i32 = 300;
/// Ticks a held spot outlives the threat's last sighting: the bot peeks
/// for it again, then goes after it. A wary bot draws its own
/// ([`WARY`]).
const WAIT_TICKS: u32 = 60;
/// A neighbouring spot is at least this far from the one held.
const SHIFT_MIN: f32 = 48.0;
/// The crouched lean's limit on all three servers (`pmove::lean_scale`).
const LEAN_CROUCH: f32 = 0.5;

/// A wary bot's hold at a spot, per `vc_bot_skill` level: the ticks it
/// outlives the threat's last sighting as (least, spread), and the chance
/// in 8 that ducking back to reload moves it to the neighbouring spot
/// (bot-combat.md 10).
struct Wary {
    wait: (u32, u32),
    shift: u32,
}

const WARY: [Wary; 4] = [
    Wary {
        wait: (70, 20),
        shift: 0,
    },
    Wary {
        wait: (45, 30),
        shift: 3,
    },
    Wary {
        wait: (40, 30),
        shift: 4,
    },
    Wary {
        wait: (35, 30),
        shift: 4,
    },
];

/// What a cover search traces with.
pub struct Sight<'a> {
    /// The threat's eye, which the body hides from.
    pub threat_eye: Vec3,
    /// Where the bot aims on the threat ([`EnemyView::origin`]).
    pub target: Vec3,
    /// The standing lean's limit: 1 on 1.1, 0.5 on 1.5 and UO
    /// (`pmove::lean_scale`).
    pub lean: f32,
    /// A round's segment between two points meets nothing.
    pub clear: &'a dyn Fn(Vec3, Vec3) -> bool,
    /// The fraction of a lean's sweep from an eye to its full lean that the
    /// wall leaves (`PM_UpdateLean`'s sphere, `pmove::update_lean`).
    pub lean_room: &'a dyn Fn(Vec3, Vec3) -> f32,
}

impl Sight<'_> {
    /// Whether a body standing at `at` with its eye `eye` over the feet is
    /// out of the threat's sight: the eye, the chest and the chest either
    /// side across the line.
    fn hidden(&self, at: Vec3, eye: f32) -> bool {
        let line = (self.threat_eye - at).truncate().normalize_or_zero();
        let across = Vec3::new(-line.y, line.x, 0.0) * SHOULDER;
        let chest = at + Vec3::Z * eye * (2.0 / 3.0);
        [at + Vec3::Z * eye, chest, chest + across, chest - across]
            .into_iter()
            .all(|p| !(self.clear)(self.threat_eye, p))
    }

    /// Whether an eye leaning `side` up to `limit`, as far as the wall lets
    /// it, sees the target.
    fn leans_clear(&self, eye: Vec3, side: i8, limit: f32) -> bool {
        let to = self.target - eye;
        let yaw = to.y.atan2(to.x);
        let s = f32::from(side);
        let room = (self.lean_room)(eye, eye + aim::lean_offset(yaw, s));
        let lean = limit.min(aim::unget_lean_fraction(room));
        lean > 0.0 && (self.clear)(eye + aim::lean_offset(yaw, s * lean), self.target)
    }

    /// How a body at `at` fights from it, or `None` when it is no cover.
    pub fn peek(&self, at: Vec3) -> Option<Peek> {
        let eye = at + Vec3::Z * pmove::VIEW_STAND;
        if self.hidden(at, pmove::VIEW_STAND) {
            return [1i8, -1]
                .into_iter()
                .find(|&side| self.leans_clear(eye, side, self.lean))
                .map(Peek::Lean);
        }
        (self.hidden(at, pmove::VIEW_CROUCH) && (self.clear)(eye, self.target))
            .then_some(Peek::Stand)
    }

    /// A second way to show from `at` besides `peek`: the other lean side,
    /// else a crouched lean from a body hidden crouched.
    pub fn alt(&self, at: Vec3, peek: Peek) -> Option<Peek> {
        if let Peek::Lean(side) = peek
            && self.leans_clear(at + Vec3::Z * pmove::VIEW_STAND, -side, self.lean)
        {
            return Some(Peek::Lean(-side));
        }
        let eye = at + Vec3::Z * pmove::VIEW_CROUCH;
        if !self.hidden(at, pmove::VIEW_CROUCH) {
            return None;
        }
        [1i8, -1]
            .into_iter()
            .find(|&side| self.leans_clear(eye, side, LEAN_CROUCH))
            .map(Peek::LowLean)
    }

    /// `spot` checked again against this sight: its peek and second peek
    /// anew, its neighbour kept; `None` once it hides or peeks no more.
    pub fn recheck(&self, spot: Spot) -> Option<Spot> {
        let peek = self.peek(spot.at.into())?;
        Some(Spot {
            peek,
            alt: self.alt(spot.at.into(), peek),
            ..spot
        })
    }
}

/// The nearest spot to `from`, where the body stands or a node within
/// [`REACH`] along level edges (no jump, no risky edge, no step past
/// `STEPSIZE`), that [`Sight::peek`] passes: a [`Peek::Stand`] anywhere
/// in reach before a lean. In seeded `tdm` runs the crouch-and-stand
/// spots won fights and the lean spots only broke even (bot-combat.md 4.5).
pub fn find(g: &NavGraph, from: Vec3, sight: &Sight) -> Option<Spot> {
    let start = g.nearest(from.into())?;
    let level = |a: u32, b: u32| {
        (g.nodes[a as usize].z - g.nodes[b as usize].z).abs() <= pmove::STEPSIZE
            && !g.jumps.contains(&(a, b))
            && !g.risky.contains(&(a, b))
    };
    let mut seen = vec![start];
    let mut ring = vec![start];
    for _ in 0..RINGS {
        let mut next = Vec::new();
        for &a in &ring {
            for &b in &g.edges[a as usize] {
                if !seen.contains(&b)
                    && level(a, b)
                    && (g.nodes[b as usize] - from).truncate().length() <= REACH
                {
                    seen.push(b);
                    next.push(b);
                }
            }
        }
        ring = next;
    }
    let mut spots: Vec<(f32, Vec3)> = seen
        .iter()
        .map(|&n| g.nodes[n as usize])
        .map(|p| ((p - from).truncate().length(), p))
        .chain([(0.0, from)])
        .collect();
    spots.sort_by(|a, b| a.0.total_cmp(&b.0));
    let peeks: Vec<(Vec3, Option<Peek>)> = spots.iter().map(|&(_, p)| (p, sight.peek(p))).collect();
    let (at, peek) = peeks
        .iter()
        .find(|(_, k)| *k == Some(Peek::Stand))
        .or_else(|| peeks.iter().find(|(_, k)| k.is_some()))
        .map(|&(p, k)| (p, k.unwrap()))?;
    let next = peeks
        .iter()
        .find(|(p, k)| k.is_some() && (*p - at).truncate().length() >= SHIFT_MIN)
        .map(|&(p, k)| (p.into(), k.unwrap()));
    Some(Spot {
        at: at.into(),
        peek,
        alt: sight.alt(at, peek),
        next,
    })
}

/// The brain's side: the spot it holds and whom it hides from.
#[derive(Clone, Copy, Debug)]
pub(super) struct Hold {
    spot: Spot,
    /// Where the threat was last aimed at.
    threat: [f32; 3],
    /// Ticks since the threat was last in sight.
    unseen: u32,
    /// How the body shows itself: [`Spot::peek`] or [`Spot::alt`].
    pose: Peek,
    /// Ticks the hold outlives the threat's last sighting.
    wait: u32,
}

/// A bot's cover state: whether it takes cover at all, the spot held, and
/// its peeks this life.
#[derive(Clone, Copy, Debug)]
pub struct Cover {
    pub enabled: bool,
    pub(super) hold: Option<Hold>,
    /// The last peek's spot and pose, and whether the body showed itself
    /// last tick.
    last_peek: Option<([f32; 3], Peek)>,
    showing: bool,
    /// Peeks started, and those from the spot and pose of the one before
    /// (the `bot_match` re-peek column).
    pub(super) peeks: u32,
    pub(super) repeeks: u32,
}

impl Default for Cover {
    fn default() -> Self {
        Cover {
            enabled: true,
            hold: None,
            last_peek: None,
            showing: false,
            peeks: 0,
            repeeks: 0,
        }
    }
}

impl Cover {
    /// A new life: no peek to repeat.
    pub(super) fn end_life(&mut self) {
        self.last_peek = None;
        self.showing = false;
    }
}

/// Two spots this near across are one.
fn same_spot(a: [f32; 3], b: [f32; 3]) -> bool {
    (a[0] - b[0]).hypot(a[1] - b[1]) <= ARRIVE
}

impl Bot {
    /// The fight leaves the bot free to move: no plant, carry or zone to
    /// stand in, no bipod down, and fire on. A guard or a sweep fights from
    /// cover as well as anywhere.
    fn may_cover(&self, view: &BotView) -> bool {
        self.cover.enabled
            && self.shoot
            && !view.lmg
            && !view.linked
            && !self.objective_busy(view)
            && match self.objective_target(view) {
                None => true,
                Some(t) => matches!(
                    t,
                    ObjTarget::Guard { .. } | ObjTarget::Escort(_) | ObjTarget::Sweep(_)
                ),
            }
    }

    /// Whether a spot is held (the gates read it).
    pub fn holds_cover(&self) -> bool {
        self.cover.hold.is_some()
    }

    /// The spot held, which the server checks again before it looks for
    /// another: a wary bot may have shifted off the one it was given.
    pub fn held_spot(&self) -> Option<Spot> {
        self.cover.hold.map(|h| h.spot)
    }

    /// The bot holds a spot and still knows where the threat is: a target
    /// out of sight behind its own cover is not lost (`Bot::track`).
    pub(super) fn covering(&self) -> bool {
        self.cover.hold.is_some_and(|h| h.unseen <= h.wait)
    }

    /// A draw of `(least, spread)` ticks.
    fn draw(&mut self, (least, spread): (u32, u32)) -> u32 {
        least + self.rand_below(spread)
    }

    /// The fight's footwork from cover, with `e` in sight at `dist` and the
    /// view at `yaw`: takes up the server's spot, or drops the hold when
    /// there is none or the threat is too near.
    pub(super) fn cover_engage(
        &mut self,
        view: &BotView,
        e: EnemyView,
        dist: f32,
        yaw: f32,
        cmd: &mut UserCmd,
    ) {
        let spot = view
            .cover
            .filter(|_| dist >= MIN_RANGE && self.may_cover(view));
        let prior = self.cover.hold;
        self.cover.hold = spot.map(|spot| {
            // A wary bot keeps the pose it took while the spot still has it.
            let pose = prior
                .filter(|h| self.wary && same_spot(h.spot.at, spot.at))
                .map(|h| h.pose)
                .filter(|p| *p == spot.peek || Some(*p) == spot.alt)
                .unwrap_or(spot.peek);
            Hold {
                spot,
                threat: e.origin,
                unseen: 0,
                pose,
                wait: WAIT_TICKS,
            }
        });
        if self.cover.hold.is_none() {
            self.cover.showing = false;
        }
        self.cover_keys(view, yaw, cmd);
    }

    /// With nothing in sight, a held spot keeps the body there for
    /// [`WAIT_TICKS`] (a wary bot's own draw, [`WARY`]), peeking where the
    /// threat was. The view's (pitch, yaw), or `None` once the hold is let
    /// go.
    pub(super) fn cover_unseen(&mut self, view: &BotView, cmd: &mut UserCmd) -> Option<[f32; 2]> {
        let mut h = self.cover.hold?;
        h.unseen += 1;
        if self.wary && h.unseen == 1 {
            h.wait = self.draw(WARY[self.skill_level()].wait);
        }
        if h.unseen > h.wait || !self.may_cover(view) {
            self.cover.hold = None;
            self.cover.showing = false;
            return None;
        }
        self.cover.hold = Some(h);
        let (tp, ty) = aim_angles(view, h.threat);
        let look = self.turn_toward(view, [tp, ty]);
        self.cover_keys(view, look[1], cmd);
        Some(look)
    }

    /// The keys at a held spot: walk to it, then hold still, peeking while
    /// the weapon is ready and behind the cover while it is busy. A wary
    /// bot ducking back may move to the neighbouring spot ([`Wary::shift`]),
    /// and about to peek again from the spot and pose of its last peek
    /// takes the spot's other pose.
    fn cover_keys(&mut self, view: &BotView, yaw: f32, cmd: &mut UserCmd) {
        let Some(mut h) = self.cover.hold else {
            return;
        };
        let to = [h.spot.at[0] - view.origin[0], h.spot.at[1] - view.origin[1]];
        let flat = to[0].hypot(to[1]);
        let hide = view.clip == 0 || view.busy_ms > HIDE_BUSY_MS;
        cmd.up = 0;
        cmd.wbuttons &= !(msg::WBUTTON_CROUCH
            | msg::WBUTTON_PRONE
            | msg::WBUTTON_LEAN_LEFT
            | msg::WBUTTON_LEAN_RIGHT);
        if flat > ARRIVE {
            self.cover.showing = false;
            let d = yaw_diff(to[1].atan2(to[0]).to_degrees(), yaw).to_radians();
            let key = (flat / SLOW).min(1.0) * 127.0;
            cmd.forward = (d.cos() * key).round() as i8;
            cmd.right = (-d.sin() * key).round() as i8;
            if flat > ADS_WALK {
                cmd.buttons &= !msg::BUTTON_ADS;
            }
            return;
        }
        cmd.forward = 0;
        cmd.right = 0;
        if hide
            && self.cover.showing
            && self.wary
            && let Some((at, peek)) = h.spot.next
            && self.rand_below(8) < WARY[self.skill_level()].shift
        {
            h.spot = Spot {
                at,
                peek,
                alt: None,
                next: Some((h.spot.at, h.spot.peek)),
            };
            h.pose = peek;
            self.cover.hold = Some(h);
            self.cover.showing = false;
            return;
        }
        if !hide && !self.cover.showing {
            let repeats = |c: &Cover, h: &Hold| {
                c.last_peek
                    .is_some_and(|(at, p)| same_spot(at, h.spot.at) && p == h.pose)
            };
            if self.wary
                && repeats(&self.cover, &h)
                && let Some(alt) = h.spot.alt
            {
                h.pose = if h.pose == alt { h.spot.peek } else { alt };
                self.cover.hold = Some(h);
            }
            self.cover.peeks += 1;
            self.cover.repeeks += u32::from(repeats(&self.cover, &h));
            self.cover.last_peek = Some((h.spot.at, h.pose));
        }
        self.cover.showing = !hide;
        let (crouch, lean) = match h.pose {
            Peek::Stand => (hide, 0),
            Peek::Lean(side) => (false, if hide { 0 } else { side }),
            Peek::LowLean(side) => (true, if hide { 0 } else { side }),
        };
        if crouch {
            // The stance is a held level; retail sends `upmove` -127
            // with it (protocol doc, "Usercmd input bits").
            cmd.wbuttons |= msg::WBUTTON_CROUCH;
            cmd.up = -127;
        }
        cmd.wbuttons |= match lean {
            1 => msg::WBUTTON_LEAN_RIGHT,
            -1 => msg::WBUTTON_LEAN_LEFT,
            _ => 0,
        };
    }
}

#[cfg(test)]
mod tests {
    use super::super::tests::engaged;
    use super::*;
    use crate::game::combat::ray_box;

    /// A [`Sight`] from (500, 0, 60) at a chest at (500, 0, 40) past `walls`.
    fn sight(walls: &[(Vec3, Vec3)], test: impl FnOnce(&Sight)) {
        let clear = |a: Vec3, b: Vec3| {
            walls
                .iter()
                .all(|(lo, hi)| ray_box(a, b, *lo, *hi).is_none())
        };
        let room = |_: Vec3, _: Vec3| 1.0;
        test(&Sight {
            threat_eye: Vec3::new(500.0, 0.0, 60.0),
            target: Vec3::new(500.0, 0.0, 40.0),
            lean: 1.0,
            clear: &clear,
            lean_room: &room,
        });
    }

    #[test]
    fn a_wall_edge_on_the_left_is_leaned_round_to_the_left() {
        // Facing +x, the wall stops 9 units left (+y) of the line.
        let wall = (Vec3::new(90.0, -100.0, 0.0), Vec3::new(100.0, 9.0, 100.0));
        sight(&[wall], |s| {
            assert_eq!(s.peek(Vec3::ZERO), Some(Peek::Lean(-1)))
        });
        let wall = (Vec3::new(90.0, -9.0, 0.0), Vec3::new(100.0, 100.0, 100.0));
        sight(&[wall], |s| {
            assert_eq!(s.peek(Vec3::ZERO), Some(Peek::Lean(1)))
        });
    }

    #[test]
    fn a_wall_wider_than_a_lean_is_no_cover() {
        let wall = (Vec3::new(90.0, -100.0, 0.0), Vec3::new(100.0, 100.0, 100.0));
        sight(&[wall], |s| assert_eq!(s.peek(Vec3::ZERO), None));
    }

    #[test]
    fn a_wall_the_lean_sphere_meets_cuts_the_lean() {
        let wall = (Vec3::new(90.0, -100.0, 0.0), Vec3::new(100.0, 9.0, 100.0));
        let clear = |a: Vec3, b: Vec3| ray_box(a, b, wall.0, wall.1).is_none();
        let room = |_: Vec3, _: Vec3| 0.1;
        let s = Sight {
            threat_eye: Vec3::new(500.0, 0.0, 60.0),
            target: Vec3::new(500.0, 0.0, 40.0),
            lean: 1.0,
            clear: &clear,
            lean_room: &room,
        };
        assert_eq!(s.peek(Vec3::ZERO), None);
    }

    #[test]
    fn a_low_wall_is_fought_over_standing() {
        let wall = (Vec3::new(90.0, -100.0, 0.0), Vec3::new(100.0, 100.0, 48.0));
        sight(&[wall], |s| {
            assert_eq!(s.peek(Vec3::ZERO), Some(Peek::Stand))
        });
    }

    #[test]
    fn a_crouch_spot_is_taken_over_a_nearer_lean() {
        // A row of nodes along +y: a tall wall in front of the first two,
        // ending 5 units left of the second, and a low wall in front of
        // the fourth.
        let nodes: Vec<Vec3> = (0..5)
            .map(|i| Vec3::new(0.0, 40.0 * i as f32, 0.0))
            .collect();
        let n = nodes.len() as u32;
        let edges = (0..n)
            .map(|i| {
                [i.wrapping_sub(1), i + 1]
                    .into_iter()
                    .filter(|j| *j < n)
                    .collect()
            })
            .collect();
        let g = NavGraph::from_parts(nodes, edges);
        let tall = (Vec3::new(90.0, -100.0, 0.0), Vec3::new(100.0, 45.0, 100.0));
        let low = (Vec3::new(90.0, 60.0, 0.0), Vec3::new(100.0, 160.0, 48.0));
        sight(&[tall], |s| {
            let spot = find(&g, Vec3::ZERO, s).unwrap();
            assert_eq!((spot.at, spot.peek), ([0.0, 40.0, 0.0], Peek::Lean(-1)));
        });
        sight(&[tall, low], |s| {
            let spot = find(&g, Vec3::ZERO, s).unwrap();
            assert_eq!((spot.at, spot.peek), ([0.0, 120.0, 0.0], Peek::Stand));
        });
    }

    #[test]
    fn open_ground_is_no_cover() {
        sight(&[], |s| assert_eq!(s.peek(Vec3::ZERO), None));
    }

    /// An engaged bot 600 units from its enemy, holding `peek` at its feet.
    fn at_cover(peek: Peek) -> (Bot, BotView) {
        let (bot, mut v) = engaged(1);
        v.enemy.as_mut().unwrap().origin = [600.0, 0.0, 124.0];
        v.cover = Some(Spot::new(v.origin, peek));
        (bot, v)
    }

    fn leans(cmd: &UserCmd) -> u8 {
        cmd.wbuttons & (msg::WBUTTON_LEAN_LEFT | msg::WBUTTON_LEAN_RIGHT)
    }

    #[test]
    fn a_lean_spot_leans_out_while_ready_and_back_while_busy() {
        let (mut bot, mut v) = at_cover(Peek::Lean(-1));
        let cmd = bot.think(&v);
        assert_eq!(leans(&cmd), msg::WBUTTON_LEAN_LEFT);
        assert_eq!((cmd.forward, cmd.right), (0, 0));
        v.busy_ms = 1500;
        let cmd = bot.think(&v);
        assert_eq!(leans(&cmd), 0);
        assert_eq!(cmd.wbuttons & msg::WBUTTON_CROUCH, 0);
    }

    #[test]
    fn a_stand_spot_ducks_while_busy() {
        let (mut bot, mut v) = at_cover(Peek::Stand);
        v.clip = 0;
        let cmd = bot.think(&v);
        assert_ne!(cmd.wbuttons & msg::WBUTTON_CROUCH, 0);
        assert_eq!(cmd.up, -127);
        v.clip = 8;
        for _ in 0..40 {
            let cmd = bot.think(&v);
            assert_eq!(cmd.wbuttons & msg::WBUTTON_CROUCH, 0);
            assert_eq!(leans(&cmd), 0);
        }
    }

    #[test]
    fn a_spot_aside_is_walked_to_off_the_sights() {
        let (mut bot, mut v) = at_cover(Peek::Stand);
        // Left of the line to the enemy, which lies along +x.
        v.cover.as_mut().unwrap().at = [0.0, 150.0, 64.0];
        let mut cmd = bot.think(&v);
        for _ in 0..10 {
            cmd = bot.think(&v);
            v.view = [0.0, 0.0, 0.0];
        }
        assert!(cmd.right < -100, "{}", cmd.right);
        assert_eq!(cmd.buttons & msg::BUTTON_ADS, 0);
    }

    #[test]
    fn a_hidden_threat_is_peeked_for_then_let_go() {
        let (mut bot, mut v) = at_cover(Peek::Lean(1));
        bot.wary = false;
        bot.think(&v);
        v.enemy = None;
        v.cover = None;
        // Hidden past `forget_ticks`, the target is kept and the peek held.
        for _ in 0..WAIT_TICKS {
            let cmd = bot.think(&v);
            assert_eq!(leans(&cmd), msg::WBUTTON_LEAN_RIGHT);
        }
        assert!(bot.target.is_some());
        let cmd = bot.think(&v);
        assert_eq!(leans(&cmd), 0);
        assert!(!bot.holds_cover());
    }

    /// A wary bot's peeks alternate between the spot's two ways out, and
    /// it lets the spot go inside its drawn hold.
    #[test]
    fn a_wary_bot_peeks_each_way_by_turns() {
        let (mut bot, mut v) = at_cover(Peek::Lean(1));
        v.cover.as_mut().unwrap().alt = Some(Peek::Lean(-1));
        let mut peeks = Vec::new();
        for i in 0..6 {
            v.busy_ms = if i % 2 == 0 { 0 } else { 1500 };
            for _ in 0..3 {
                let l = leans(&bot.think(&v));
                if l != 0 && peeks.last() != Some(&l) {
                    peeks.push(l);
                }
            }
        }
        assert_eq!(peeks.len(), 3, "{peeks:?}");
        assert_eq!(bot.tactics()[1], 0);
        v.enemy = None;
        v.cover = None;
        v.busy_ms = 0;
        let mut held = 0;
        while bot.holds_cover() {
            bot.think(&v);
            held += 1;
            assert!(held < 200);
        }
        let w = &WARY[1];
        assert!(held <= w.wait.0 + w.wait.1 + 1, "{held}");
    }

    #[test]
    fn a_wary_bot_ducking_back_may_shift_to_the_next_spot() {
        let (mut bot, mut v) = at_cover(Peek::Stand);
        let next = [0.0, 60.0, 64.0];
        v.cover.as_mut().unwrap().next = Some((next, Peek::Lean(1)));
        let mut shifted = 0;
        for _ in 0..40 {
            v.busy_ms = 0;
            bot.think(&v);
            v.busy_ms = 1500;
            bot.think(&v);
            if bot.held_spot().is_some_and(|s| s.at == next) {
                shifted += 1;
            }
        }
        assert!(shifted > 0);
    }

    #[test]
    fn a_crouched_lean_is_the_second_way_over_a_low_wall_by_an_edge() {
        // A low wall in front, ending 9 units left of the line.
        let wall = (Vec3::new(90.0, -100.0, 0.0), Vec3::new(100.0, 9.0, 48.0));
        sight(&[wall], |s| {
            assert_eq!(s.peek(Vec3::ZERO), Some(Peek::Stand));
            assert_eq!(s.alt(Vec3::ZERO, Peek::Stand), Some(Peek::LowLean(-1)));
        });
    }

    #[test]
    fn a_near_enemy_is_fought_in_the_open() {
        let (mut bot, mut v) = at_cover(Peek::Lean(1));
        v.enemy.as_mut().unwrap().origin = [200.0, 0.0, 124.0];
        let cmd = bot.think(&v);
        assert_eq!(leans(&cmd), 0);
        assert!(!bot.holds_cover());
    }

    #[test]
    fn a_bot_with_cover_off_never_holds_a_spot() {
        let (mut bot, v) = at_cover(Peek::Lean(1));
        bot.cover.enabled = false;
        assert_eq!(leans(&bot.think(&v)), 0);
        assert!(!bot.holds_cover());
    }
}
