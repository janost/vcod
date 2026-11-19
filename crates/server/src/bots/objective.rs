//! The gametype objectives: what each wants of a bot ([`ObjTarget`]), the guard posts, and the
//! plant, defuse, carry and breach.

use super::*;

mod uo;

/// The S&D objective bookkeeping; one life is one round, so a death
/// starts it over (docs/research/bot-objectives.md).
#[derive(Default)]
pub(super) struct Objective {
    /// Sites given up on this life; the site is `(rank + pick) % sites`.
    pub(super) pick: u32,
    /// Ticks the use key has been held at the objective; nonzero means the
    /// plant or defuse is in progress and nothing else may break it off.
    held: u32,
    /// Ticks the view has rested on the bomb.
    settled: u32,
    /// Ticks before another try after one that ran out.
    rest: u32,
    /// Set while the bot plays, so the end of a life redraws `pick` once.
    pub(super) live: bool,
    /// A defender has walked to where the carried objective was taken
    /// from; cleared once nothing is carried.
    swept: bool,
    /// The domination flag this life is set on, by its index, until its
    /// team holds it.
    zone: Option<usize>,
    /// Ticks stood at a breach stand with no shot from it; past
    /// [`BREACH_IDLE_TICKS`] the next stand is tried (`pick`).
    idle: u32,
    /// Ticks since an S&D defuser's, or a `bas` planter's or defuser's,
    /// settled view picked a weapon lying by the charge instead of its
    /// trigger; while set it closes in to [`DEFUSE_CLOSE`].
    close_in: u32,
    /// The far side of a contested zone the bot stands in, walked to
    /// before it settles again ([`Bot::sweep_contested`]).
    sweep: Option<[f32; 3]>,
    /// The objective's point the follower walked out its plan to with the
    /// objective not reached, the nearest the body has come to it since
    /// and the ticks since it last came [`SHORT_GAIN`] nearer
    /// ([`Bot::track_reach`]).
    short: Option<([f32; 3], f32, u32)>,
    /// An objective's point given up on and the ticks left: the objective
    /// there reads as a guard round it ([`Bot::objective_target`]).
    shy: Option<([f32; 3], u32)>,
}

/// Where a guard's posts stand (bot-objectives.md 14).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum GuardHold {
    /// Round the objective, as section 13 deals them.
    Near,
    /// Out on the ways in, with at most one teammate near the objective;
    /// cues pull a share back ([`Bot::far`], [`Bot::cues`]).
    Far,
    /// Round the objective, walked in to with care: S&D defenders making
    /// for a planted bomb ([`Bot::cues`]).
    Back,
    /// Out on the ways in from a band nearer the objective: a `ctf` flag
    /// at home with [`Bot::ctf_near`] ([`posts::CTF_NEAR_BAND`]).
    FarNear,
    /// The one post the driver dealt ([`CtfView::watch`]): none dealt
    /// round the point.
    Watch,
}

/// What the objective wants of the bot right now.
#[derive(Clone, Copy, Debug)]
pub(super) enum ObjTarget {
    Plant(SiteView),
    /// Stand within [`GUARD_RADIUS`] of a point and fight from there, but
    /// no nearer than `inner`; `hold` says where its posts stand.
    Guard {
        at: [f32; 3],
        inner: f32,
        hold: GuardHold,
    },
    /// Walk within `reach` of the bomb, rest the view on it and hold use
    /// `ticks`: S&D's defuse, and base assault's plant and defuse.
    Defuse {
        bomb: BombView,
        reach: f32,
        ticks: u32,
    },
    /// Stand inside a zone until the script hands it over: a domination
    /// flag, or a capture goal waiting on the carrier's own flag.
    Occupy(SiteView),
    /// Stand within `radius` across and `height` up or down of a
    /// headquarters radio, walking to `stand`.
    Radio {
        at: [f32; 3],
        radius: f32,
        height: f32,
        stand: [f32; 3],
    },
    /// Aim at a retrieval objective's pickup trigger, `aim`, and tap use,
    /// standing within reach of `from`.
    Pickup {
        aim: [f32; 3],
        from: [f32; 3],
    },
    /// Carry a retrieval objective into its goal; touching it delivers.
    Deliver([f32; 3]),
    /// Run at a moving point: a carrier the compass shows.
    Chase([f32; 3]),
    /// Keep within [`GUARD_RADIUS`] of a teammate carrying: a guard round
    /// a point that moves, so no post.
    Escort([f32; 3]),
    /// Walk the carrier's likely way backwards, from the goal side to where
    /// the objective was taken, once per carry.
    Sweep([f32; 3]),
    /// Fire rockets at a `bas` base: walk toward it, and from where a shot
    /// bursts at it (`aim`) stand and fire (bot-objectives.md 7.2).
    Breach {
        base: [f32; 3],
        stand: Option<[f32; 3]>,
        aim: Option<[f32; 3]>,
    },
}

/// `level.planttime` is 5 s (`sd.gsc` `bombzones`); a hold this long that
/// planted nothing has failed.
const PLANT_TICKS: u32 = 120;
/// `level.defusetime` is 10 s.
const DEFUSE_TICKS: u32 = 210;
/// The defuse wants `distance(origin, trigger.origin) < 64`.
const DEFUSE_REACH: f32 = 48.0;
/// A defuser whose press would take a weapon lying by the bomb stands this
/// near the bomb instead: `G_GetActivateEnt` scores an entity dead on the
/// view at its distance from the eye, so with the eye over the bomb the
/// trigger's middle is nearer than anything lying on the floor.
const DEFUSE_CLOSE: f32 = 16.0;
/// Past this long closing in (the bomb's own body in the way), the press
/// goes ahead whatever it takes.
pub(super) const DEFUSE_CLOSE_TICKS: u32 = 100;
const GUARD_RADIUS: f32 = 250.0;
/// An escort this near the carrier it keeps by steps off: standing at his
/// next node on a stair it held mp_carentan's carrier upstairs for 75 s
/// (bot-objectives.md 4.2).
const ESCORT_CLEAR: f32 = 80.0;
/// Ticks a bot that walked out its plan to an objective's point may go
/// without coming [`SHORT_GAIN`] nearer it before it gives the point up
/// ([`Bot::track_reach`]).
pub(super) const SHORT_TICKS: u32 = 60;
pub(super) const SHORT_GAIN: f32 = 16.0;
/// Ticks a point given up on reads as a guard round it.
pub(super) const SHY_TICKS: u32 = 300;
/// How near a target's point must stand to the one given up on to be it.
const SHY_SAME: f32 = 32.0;
/// A guard ring is at least this wide.
const GUARD_BAND: f32 = 150.0;
/// A guard's posts stand at least this far off its point (a site's
/// middle, a radio), and this far past the ring it holds anywhere in
/// without one: a post has a line to the point, so it may stand off it.
const POST_INNER: f32 = 48.0;
const POST_REACH: f32 = 150.0;
/// A guard pulled back to its objective walks the last this far in and
/// crouches the last this far ([`GuardPost::careful`]).
const CAREFUL_NEAR: f32 = 700.0;
pub(super) const CAREFUL_CROUCH: f32 = 300.0;
/// An attacker steps off the bomb it planted: a body on the line from a
/// defender's eye to the trigger blocks the defuse's `isLookingAt`, which
/// is the defender's problem, not the bot's to make.
const BOMB_GUARD_INNER: f32 = 96.0;
/// An attacker this near its own dropped flag returns it on the way, and
/// a carrier waiting on it ([`Bot::ctf_carry`]).
const CTF_RETURN_NEAR: f32 = 1000.0;
/// A pickup trigger is aimed at and used from within this, horizontally:
/// `G_GetActivateEnt` reaches 128 units from the eye.
const PICKUP_REACH: f32 = 48.0;
/// How far round the pickup a bot steps for another try.
const PICKUP_RING: f32 = 40.0;
/// Use held this long with no link means the plant or defuse never
/// started (not touching, not on the ground, the aim off the trigger).
pub(super) const START_TICKS: u32 = 20;
/// Release after a failed try, so the next press is a fresh one.
const RETRY_TICKS: u32 = 10;
/// The aim trace is read a frame late (object-model doc 23.1): the view
/// rests on the bomb this many ticks before the press.
const SETTLE_TICKS: u32 = 2;
pub(super) const SETTLE_DEG: f32 = 1.0;
/// The planter's view pitch, wire convention (up). An item on the floor
/// inside 128 units sits more than 30 degrees below level, and
/// `G_GetActivateEnt`'s cone is 40 either side of the view.
pub(super) const PLANT_PITCH: f32 = -60.0;
/// `level.zradioradius` (`hq.gsc` 186): how far up or down of a radio a
/// body still counts; the bot stands this far inside both reaches.
pub(crate) const RADIO_HEIGHT: f32 = 50.0;
/// CoD2's `level.zradioradius` (`hq.gsc` 214, iw_15.iwd): 72, and
/// `level.radioradius` 120 is a distance in three dimensions (1255).
pub(crate) const RADIO_HEIGHT_COD2: f32 = 72.0;
pub(crate) const RADIO_MARGIN: f32 = 16.0;
/// `scr_bas_planttime` 15 s and `scr_bas_defusetime` 10 s, a half second
/// over; the hold aborts past 155 units from the objective model.
const BAS_PLANT_TICKS: u32 = 310;
const BAS_DEFUSE_TICKS: u32 = 210;
const BAS_REACH: f32 = 112.0;
/// A base's charge goes off with a 1000-damage `radiusDamage` on every
/// player within 500 units of it (`BaseAssault_bomb_countdown`).
const BAS_BLAST: f32 = 520.0;
/// An attacker holds the room of its own charge until this many seconds
/// are left on it, then gets out of the blast: with 8 s a planter climbing
/// out of the room was still inside it when it went.
pub(super) const BAS_CLEAR_S: f32 = 20.0;
/// An attacker this near a charge on its own base turns back to defuse it.
const BAS_DEFUSE_NEAR: f32 = 2000.0;
/// The graph's run, units a second: a bot this far from a charge in
/// straight line needs at least that long to reach it.
const BAS_RUN_SPEED: f32 = 190.0;
/// A rocket's burst counts at a base this near its origin: the base links
/// as a point with the turret's box round it (`GameHost::blast_entities`),
/// and the rocket's 256-unit splash falls off from 100 to 10.
pub const BREACH_NEAR: f32 = 64.0;
/// A rocket is fired at a base from this far at least, clear of its own
/// 256-unit splash, and at most this far.
pub const BREACH_MIN: f32 = 400.0;
/// A rocket's burst on the way to a base is this far from the muzzle at
/// least: the launchers' 256-unit splash, and a margin.
pub const BREACH_CLEAR: f32 = 320.0;
pub const BREACH_RANGE: f32 = 1500.0;
/// A base's own launchers (`mpweapon_*` with `script_gameobjectname bas`)
/// lie in its bunker, this near its origin, behind the walls a breach
/// deletes (about 145 units on mp_foy).
const BAS_BUNKER: f32 = 300.0;
/// Dry this long, a bot uses `kill` (`Bot::think`).
pub(super) const DRY_TICKS: u32 = 200;
/// A dry `bas` bot fetches a weapon lying this near instead.
const DRY_FETCH: f32 = 1500.0;
/// A `bas` bot left with only a rocket ([`Bot::unarmed`]) fetches a gun
/// this near: from 1000 and 1500 units off, the walk cost mp_foy's seeds 7
/// and 11 a minute and more.
const ARMS_FETCH: f32 = 800.0;
/// Ticks a fetch may go without coming [`FETCH_CLOSING`] nearer its
/// launcher (the reach included: one stood at counts down too).
pub(super) const FETCH_STALL_TICKS: u32 = 400;
const FETCH_CLOSING: f32 = 32.0;
/// Taps at one launcher before it is passed over, and for how long.
pub(super) const LAUNCHER_TAPS: u32 = 3;
pub(super) const SHUN_TICKS: u32 = 1200;
/// Stood this long at a breach stand with no shot, the bot tries another.
const BREACH_IDLE_TICKS: u32 = 40;
/// Inset from a bombzone's edges before the bot counts as inside it.
pub(super) const SITE_MARGIN: f32 = 8.0;
/// The standing player's box height, for the zone's z overlap.
pub(super) const PLAYER_HEIGHT: f32 = 70.0;

impl Bot {
    /// A `bas` charge about to go off with the bot inside its blast, fight
    /// or no fight: one of its own side's in the last [`BAS_CLEAR_S`], one
    /// on its own base once a defuse could no longer finish.
    pub(super) fn blast_near(view: &BotView) -> Option<[f32; 3]> {
        let Some(UoView::Bas(b)) = view.uo.as_ref() else {
            return None;
        };
        b.bases.iter().filter(|x| x.planted).find_map(|x| {
            let (left, c) = (x.blows_in?, x.charge?);
            let late = if x.ours {
                left < BAS_DEFUSE_TICKS as f32 / 20.0
            } else {
                left <= BAS_CLEAR_S
            };
            (late && dist_sq(view.origin, c.origin) < BAS_BLAST * BAS_BLAST).then_some(c.origin)
        })
    }

    /// The hunted run from the last enemy they saw or heard, else from
    /// their own marker, where the hunters are headed; the hunters go after
    /// what they saw or heard, else the nearest compass marker they have
    /// not yet stood at.
    pub(super) fn bel_goal(&self, view: &BotView, bel: &BelView) -> Goal {
        if bel.hunted {
            return match self.recall.map(|r| r.at).or(view.noise).or(bel.trail) {
                Some(threat) => Goal::Away(threat),
                None => Goal::Roam,
            };
        }
        if let Some(p) = self.recall_goal(view) {
            return Goal::To(p);
        }
        match self.next_marker(view, bel) {
            Some(m) => Goal::To(m),
            None => Goal::Roam,
        }
    }

    /// A marker no body reaches counts as stood at: the follower walked
    /// out the plan to it (`walked`) and the body came no [`SHORT_GAIN`]
    /// nearer in [`SHORT_TICKS`]. The marker is the mean of where it was
    /// and where its player stands (`bel.gsc` 1588-1603), so it can hang
    /// over a drop or sit in a wall, off the graph; the hunter walked at
    /// it until it moved again (bot-objectives.md 5.2).
    pub(super) fn marker_short(
        &mut self,
        view: &BotView,
        m: [f32; 3],
        walked: Option<[f32; 3]>,
    ) -> bool {
        if walked != Some(m) {
            self.marker_near = None;
            return false;
        }
        let d = dist_sq(view.origin, m).sqrt();
        let (best, ticks) = match self.marker_near {
            Some((at, best, t)) if at == m && d > best - SHORT_GAIN => (best, t + 1),
            _ => (d, 0),
        };
        self.marker_near = Some((m, best, ticks));
        ticks >= SHORT_TICKS
    }

    /// The nearest marker other than the one last stood at.
    pub(super) fn next_marker(&self, view: &BotView, bel: &BelView) -> Option<[f32; 3]> {
        bel.markers
            .iter()
            .filter(|m| self.marker_reached.is_none_or(|r| dist_sq(r, **m) > 1.0))
            .min_by(|a, b| dist_sq(view.origin, **a).total_cmp(&dist_sq(view.origin, **b)))
            .copied()
    }

    /// A plant or defuse is under way: use is held, or the script holds
    /// the body.
    pub(super) fn objective_busy(&self, view: &BotView) -> bool {
        self.obj.held > 0 || view.linked
    }

    /// The objective this tick, from the bot's role and the round's state:
    /// [`Self::objective_wanted`], or a guard round its point while that
    /// point is given up on ([`Bot::track_reach`]).
    pub(super) fn objective_target(&self, view: &BotView) -> Option<ObjTarget> {
        let t = self.objective_wanted(view)?;
        match (self.obj.shy, Self::target_point(&t)) {
            (Some((at, _)), Some(p)) if dist_sq(at, p) < SHY_SAME * SHY_SAME => {
                Some(ObjTarget::Guard {
                    at: p,
                    inner: 0.0,
                    hold: GuardHold::Near,
                })
            }
            _ => Some(t),
        }
    }

    /// The point a target's walk heads for when the bot may give it up
    /// ([`Bot::track_reach`]): not a guard ring, which holds anywhere in
    /// it, nor a breach stand, which waits there for its shot.
    fn target_point(t: &ObjTarget) -> Option<[f32; 3]> {
        match *t {
            ObjTarget::Plant(s) | ObjTarget::Occupy(s) => Some(s.stand),
            ObjTarget::Defuse { bomb, .. } => Some(bomb.origin),
            ObjTarget::Radio { stand, .. } => Some(stand),
            ObjTarget::Pickup { from, .. } => Some(from),
            ObjTarget::Deliver(at) | ObjTarget::Chase(at) | ObjTarget::Sweep(at) => Some(at),
            ObjTarget::Guard { .. } | ObjTarget::Escort(_) | ObjTarget::Breach { .. } => None,
        }
    }

    /// Gives up an objective's point the bot cannot close on: its follower
    /// walked out the plan to it ([`Self::arrived_goal`]), the objective is
    /// not reached, and the body came no [`SHORT_GAIN`] nearer the point
    /// in [`SHORT_TICKS`]. The point then reads as a guard round it for
    /// [`SHY_TICKS`]. A plan's end short of a point no node reaches is the
    /// node nearest it, and from there the bot walked at the point into a
    /// wall or a teammate standing on it for the rest of the round
    /// (bot-objectives.md 12.19).
    pub(super) fn track_reach(&mut self, view: &BotView) {
        if let Some((at, left)) = self.obj.shy {
            self.obj.shy = (left > 1).then_some((at, left - 1));
        }
        let Some(p) = self.arrived_goal.take() else {
            self.obj.short = None;
            return;
        };
        let short = self.objective_wanted(view).filter(|t| {
            self.obj.shy.is_none()
                && Self::target_point(t) == Some(p)
                && !Self::at_objective(view, t)
                && !self.objective_busy(view)
        });
        if short.is_none() {
            self.obj.short = None;
            return;
        }
        let d = dist_sq(view.origin, p).sqrt();
        let (best, ticks) = match self.obj.short {
            Some((at, best, ticks)) if at == p && d > best - SHORT_GAIN => (best, ticks + 1),
            _ => (d, 0),
        };
        if ticks >= SHORT_TICKS {
            self.obj.shy = Some((p, SHY_TICKS));
            self.obj.short = None;
        } else {
            self.obj.short = Some((p, best, ticks));
        }
    }

    /// [`Self::objective_target`] before a given-up point turns it to a
    /// guard.
    fn objective_wanted(&self, view: &BotView) -> Option<ObjTarget> {
        if let Some(uo) = view.uo.as_ref() {
            return self.uo_target(view, uo);
        }
        if let Some(re) = view.re.as_ref() {
            return self.retrieval_target(view, re);
        }
        let sd = view.sd.as_ref()?;
        let site = || {
            let n = sd.sites.len();
            (n > 0).then(|| sd.sites[(sd.rank + self.obj.pick as usize) % n])
        };
        match (sd.role, sd.bomb) {
            (ObjRole::Attack, Some(b)) => Some(ObjTarget::Guard {
                at: b.origin,
                inner: BOMB_GUARD_INNER,
                hold: GuardHold::Far,
            }),
            (ObjRole::Attack, None) => match (sd.key, sd.cover) {
                (
                    Some(KeyBomb {
                        fetch: Some(at), ..
                    }),
                    _,
                ) => Some(ObjTarget::Deliver(at)),
                (_, Some(at)) => Some(ObjTarget::Guard {
                    at,
                    inner: team::COVER_INNER,
                    hold: GuardHold::Near,
                }),
                (Some(k), None) if !k.mine => site().map(|s| ObjTarget::Guard {
                    at: s.stand,
                    inner: 0.0,
                    hold: GuardHold::Near,
                }),
                _ => site().map(ObjTarget::Plant),
            },
            (ObjRole::Defend, Some(b)) if sd.lead => Some(ObjTarget::Defuse {
                bomb: b,
                reach: self.press_reach(DEFUSE_REACH),
                ticks: sd.key.map_or(DEFUSE_TICKS, |k| k.defuse_ticks),
            }),
            // Everyone else keeps off the defuser's line to the bomb.
            (ObjRole::Defend, Some(b)) => Some(ObjTarget::Guard {
                at: b.origin,
                inner: BOMB_GUARD_INNER,
                hold: GuardHold::Back,
            }),
            (ObjRole::Defend, None) => sd
                .hot
                .filter(|_| self.converge && self.team_play)
                .and_then(|h| sd.sites.get(h).copied())
                .or_else(site)
                .map(|s| ObjTarget::Guard {
                    at: s.stand,
                    inner: 0.0,
                    hold: GuardHold::Far,
                }),
        }
    }

    /// Attackers: deliver what they carry, else go for the nearest objective
    /// lying there, else escort a teammate carrying one, else stand by the
    /// goal it goes to. Defenders: each guards an objective lying there by
    /// rank. Once one is carried the first by rank holds its goal and the
    /// rest go after the carrier: at him while the compass shows him, else
    /// back along his likely way once, then to the goal.
    fn retrieval_target(&self, view: &BotView, re: &ReView) -> Option<ObjTarget> {
        let o = &re.objectives;
        let nearest = |at: fn(&ReObjView) -> Option<[f32; 3]>| {
            o.iter()
                .filter_map(at)
                .min_by(|a, b| dist_sq(view.origin, *a).total_cmp(&dist_sq(view.origin, *b)))
        };
        match re.role {
            ObjRole::Attack => {
                if let Some(m) = o.iter().find(|m| m.mine) {
                    return Some(ObjTarget::Deliver(m.goal));
                }
                // A dropped one the run cannot reach before it goes back:
                // wait where it comes back.
                if let Some(at) = Self::returns_first(view, o) {
                    return Some(ObjTarget::Guard {
                        at,
                        inner: 0.0,
                        hold: GuardHold::Near,
                    });
                }
                if let Some(aim) = nearest(|m| m.pickup) {
                    // Each tap that took nothing tries the next side: the
                    // use key's pick traces from the eye to the trigger's
                    // middle, and a table or crate can stand in the way.
                    let from = match self.obj.pick % 9 {
                        0 => aim,
                        k => {
                            let a = (k as f32 - 1.0) * 45f32.to_radians();
                            [
                                aim[0] + PICKUP_RING * a.cos(),
                                aim[1] + PICKUP_RING * a.sin(),
                                aim[2],
                            ]
                        }
                    };
                    return Some(ObjTarget::Pickup { aim, from });
                }
                if let Some(at) = Self::carrier(view, o).filter(|_| re.escort) {
                    return Some(ObjTarget::Escort(at));
                }
                // The rest go ahead and hold the goal he makes for.
                Self::goal_guard(view, o)
            }
            ObjRole::Defend => {
                if let Some(g) = Self::goal_guard(view, o) {
                    if re.rank == 0 {
                        return Some(g);
                    }
                    if let Some(at) = Self::carrier(view, o) {
                        return Some(ObjTarget::Chase(at));
                    }
                    let laid = o.iter().filter(|m| m.carried).find_map(|m| m.laid);
                    return Some(match laid {
                        Some(at) if !self.obj.swept => ObjTarget::Sweep(at),
                        _ => g,
                    });
                }
                // A dropped one guarded where it comes back once the walk
                // to it would outlast its clock.
                let lying: Vec<[f32; 3]> = o
                    .iter()
                    .filter_map(|m| match m.returns {
                        Some((home, left)) if Self::outruns(view, m.pickup?, left) => Some(home),
                        _ => m.pickup,
                    })
                    .collect();
                (!lying.is_empty()).then(|| ObjTarget::Guard {
                    at: lying[re.rank % lying.len()],
                    inner: 0.0,
                    hold: GuardHold::Far,
                })
            }
        }
    }

    /// The nearest carrier the compass shows.
    pub(super) fn carrier(view: &BotView, o: &[ReObjView]) -> Option<[f32; 3]> {
        o.iter()
            .filter(|m| !m.mine)
            .filter_map(|m| m.carrier_at)
            .min_by(|a, b| dist_sq(view.origin, *a).total_cmp(&dist_sq(view.origin, *b)))
    }

    /// The spot a dropped objective goes back to, when no other lies
    /// there to take and the run to where it lies would outlast its clock.
    fn returns_first(view: &BotView, o: &[ReObjView]) -> Option<[f32; 3]> {
        let mut lying = o.iter().filter(|m| m.pickup.is_some());
        let m = lying.next()?;
        if lying.next().is_some() {
            return None;
        }
        let (home, left) = m.returns?;
        Self::outruns(view, m.pickup?, left).then_some(home)
    }

    /// A straight run at [`RUN_SPEED`] to `at` takes longer than `left`
    /// seconds.
    fn outruns(view: &BotView, at: [f32; 3], left: f32) -> bool {
        dist_sq(view.origin, at).sqrt() > left * RUN_SPEED
    }

    /// A ring round the nearest goal an objective is being carried to, just
    /// outside its trigger.
    fn goal_guard(view: &BotView, o: &[ReObjView]) -> Option<ObjTarget> {
        o.iter()
            .filter(|m| m.carried)
            .min_by(|a, b| dist_sq(view.origin, a.goal).total_cmp(&dist_sq(view.origin, b.goal)))
            .map(|m| ObjTarget::Guard {
                at: m.goal,
                inner: m.goal_clear,
                hold: GuardHold::Near,
            })
    }

    pub(super) fn at_objective(view: &BotView, t: &ObjTarget) -> bool {
        let o = view.origin;
        match t {
            ObjTarget::Plant(s) => inside_site(o, s),
            ObjTarget::Guard { at, inner, .. } => {
                let d = dist_sq(o, *at);
                let outer = GUARD_RADIUS.max(inner + GUARD_BAND);
                d < outer * outer && d >= inner * inner
            }
            ObjTarget::Escort(at) => (ESCORT_CLEAR * ESCORT_CLEAR..GUARD_RADIUS * GUARD_RADIUS)
                .contains(&dist_sq(o, *at)),
            ObjTarget::Defuse { bomb, reach, .. } => dist_sq(o, bomb.origin) < reach * reach,
            ObjTarget::Occupy(s) => inside_site(o, s),
            ObjTarget::Radio {
                at, radius, height, ..
            } => {
                (at[0] - o[0]).hypot(at[1] - o[1]) < radius - RADIO_MARGIN
                    && (at[2] - o[2]).abs() < height - RADIO_MARGIN
            }
            ObjTarget::Pickup { aim, from } => {
                let reach = if from == aim { PICKUP_REACH } else { 16.0 };
                (from[0] - o[0]).hypot(from[1] - o[1]) < reach && (aim[2] - o[2]).abs() < 64.0
            }
            // The goal trigger's touch delivers; there is nothing to stand
            // at.
            ObjTarget::Deliver(_) | ObjTarget::Chase(_) => false,
            ObjTarget::Sweep(at) => arrived(o, *at),
            ObjTarget::Breach { aim, .. } => aim.is_some(),
        }
    }

    /// Where the objective sends a bot that is not there yet; `Hold` once
    /// it is, and `None` when the round has no objective for it.
    pub(super) fn objective_goal(&self, view: &BotView) -> Option<Goal> {
        let t = self.objective_target(view)?;
        if let Some(l) = view
            .lane
            .filter(|_| self.lanes && self.team_play && Self::laned(&t))
        {
            return Some(Goal::To(l));
        }
        if let Some(p) = self.post(view, &t) {
            return Some(if Self::on_post(view, &p) {
                Goal::Hold
            } else {
                Goal::To(p.at)
            });
        }
        if Self::at_objective(view, &t) {
            return Some(Goal::Hold);
        }
        Some(match t {
            ObjTarget::Plant(s) => Goal::To(s.stand),
            // A charge about to blow: away from it, the way out of its room.
            ObjTarget::Guard { at, inner, .. }
                if inner >= BAS_BLAST && dist_sq(view.origin, at) < inner * inner =>
            {
                Goal::Away(at)
            }
            // Too close: any way out will do, and the roam leaves the ring
            // well before it gets anywhere.
            ObjTarget::Guard { at, inner, .. } if dist_sq(view.origin, at) < inner * inner => {
                Goal::Roam
            }
            ObjTarget::Escort(at) if dist_sq(view.origin, at) < ESCORT_CLEAR * ESCORT_CLEAR => {
                Goal::Away(at)
            }
            ObjTarget::Guard { at, .. } | ObjTarget::Escort(at) => Goal::To(at),
            ObjTarget::Defuse { bomb, .. } => Goal::To(bomb.origin),
            ObjTarget::Occupy(s) => Goal::To(s.stand),
            ObjTarget::Radio { stand, .. } => Goal::To(stand),
            ObjTarget::Breach {
                stand: Some(at), ..
            } => Goal::To(at),
            // Inside its own splash of the base: off, then back.
            ObjTarget::Breach { base, .. }
                if dist_sq(view.origin, base) < BREACH_MIN * BREACH_MIN =>
            {
                Goal::Away(base)
            }
            ObjTarget::Breach { base: at, .. }
            | ObjTarget::Pickup { from: at, .. }
            | ObjTarget::Deliver(at)
            | ObjTarget::Chase(at)
            | ObjTarget::Sweep(at) => Goal::To(at),
        })
    }

    /// The bot stands at its objective without acting on it: it holds its
    /// ground instead of following a waypoint, and is not stuck.
    pub(super) fn objective_standing(&self, view: &BotView) -> bool {
        view.linked
            || (view.give_way.is_none()
                && self
                    .objective_target(view)
                    .is_some_and(|t| self.at_post(view, &t)))
    }

    /// The post the driver dealt for this target: only a guard, a zone
    /// held or a radio taken, and only with [`Self::tactics`] on.
    pub(super) fn post(&self, view: &BotView, t: &ObjTarget) -> Option<GuardPost> {
        let posted = matches!(
            t,
            ObjTarget::Guard { .. } | ObjTarget::Occupy(_) | ObjTarget::Radio { .. }
        );
        view.guard_post
            .filter(|_| posted && self.tactics && self.team_play)
    }

    /// A target walked to by a way in ([`BotView::lane`]): a site to
    /// plant at, a zone or radio to take.
    fn laned(t: &ObjTarget) -> bool {
        matches!(
            t,
            ObjTarget::Plant(_) | ObjTarget::Occupy(_) | ObjTarget::Radio { .. }
        )
    }

    /// The target the driver deals a lane toward: [`Self::laned`]'s, with
    /// [`Self::lanes`] and team play on.
    pub(crate) fn lane_target(&self, view: &BotView) -> bool {
        self.lanes && self.team_play && self.objective_target(view).is_some_and(|t| Self::laned(&t))
    }

    /// The objective a guard pulled back to it walks in to with care, while
    /// within [`CAREFUL_NEAR`] of it and off its post
    /// ([`GuardPost::careful`]).
    pub(super) fn careful(&self, view: &BotView) -> Option<[f32; 3]> {
        let p = self
            .objective_target(view)
            .and_then(|t| self.post(view, &t))?;
        let at = p.careful?;
        (!Self::on_post(view, &p) && dist_sq(view.origin, at) < CAREFUL_NEAR * CAREFUL_NEAR)
            .then_some(at)
    }

    /// Whether the bot passes over the health item at `p` (`Bot::track_heal`).
    pub(crate) fn passes_over(&self, p: [f32; 3]) -> bool {
        self.shunned.iter().any(|s| arrived(s.0, p))
    }

    pub(super) fn on_post(view: &BotView, p: &GuardPost) -> bool {
        let o = view.origin;
        (p.at[0] - o[0]).hypot(p.at[1] - o[1]) < posts::ARRIVE
            && (p.at[2] - o[2]).abs() < ARRIVE_HEIGHT
    }

    /// Standing where the target wants it: on its post when it has one,
    /// else anywhere [`Self::at_objective`] takes.
    pub(super) fn at_post(&self, view: &BotView, t: &ObjTarget) -> bool {
        match self.post(view, t) {
            Some(p) => Self::on_post(view, &p),
            None => Self::at_objective(view, t),
        }
    }

    /// What a bot guards, holds or takes, as the driver deals posts round
    /// it ([`posts::assign`]): a guard round a point that holds still (not
    /// one clearing a charge's blast), a zone, a radio.
    pub(crate) fn guard_anchor(&self, view: &BotView) -> Option<posts::Anchor> {
        match self.objective_target(view)? {
            ObjTarget::Guard {
                hold: GuardHold::Watch,
                ..
            } => None,
            ObjTarget::Guard { at, inner, .. } if inner < BAS_BLAST => Some(posts::Anchor::Ring {
                at,
                inner: inner.max(POST_INNER),
                outer: GUARD_RADIUS.max(inner + GUARD_BAND) + POST_REACH,
            }),
            ObjTarget::Occupy(s) => Some(posts::Anchor::Zone {
                mins: s.mins,
                maxs: s.maxs,
                stand: s.stand,
            }),
            ObjTarget::Radio {
                at,
                radius,
                height,
                stand,
            } => Some(posts::Anchor::Radio {
                at,
                radius: radius - RADIO_MARGIN,
                height: height - RADIO_MARGIN,
                stand,
            }),
            _ => None,
        }
    }

    /// What a bot not on guard is counted against for exposure
    /// ([`BotView::exposure`]): a ring round the point of an objective
    /// that holds still (a site to plant, a bomb to defuse, a flag to take
    /// or a goal to carry to).
    pub(crate) fn counted_anchor(&self, view: &BotView) -> Option<posts::Anchor> {
        let t = self.objective_target(view)?;
        matches!(
            t,
            ObjTarget::Plant(_) | ObjTarget::Defuse { .. } | ObjTarget::Deliver(_)
        )
        .then(|| posts::Anchor::Ring {
            at: Self::anchor_point(&t),
            inner: POST_INNER,
            outer: GUARD_RADIUS + POST_REACH,
        })
    }

    /// The point a target centres on: `bot_match` counts the bot-ticks
    /// near it.
    pub(super) fn anchor_point(t: &ObjTarget) -> [f32; 3] {
        match *t {
            ObjTarget::Plant(s) | ObjTarget::Occupy(s) => s.stand,
            ObjTarget::Guard { at, .. }
            | ObjTarget::Radio { at, .. }
            | ObjTarget::Pickup { aim: at, .. }
            | ObjTarget::Deliver(at)
            | ObjTarget::Chase(at)
            | ObjTarget::Escort(at)
            | ObjTarget::Sweep(at)
            | ObjTarget::Breach { base: at, .. } => at,
            ObjTarget::Defuse { bomb, .. } => bomb.origin,
        }
    }

    /// The point its objective centred on at the last think, if any.
    pub fn objective_point(&self) -> Option<[f32; 3]> {
        self.anchor
    }

    /// The point it guarded at the last think, standing guard on its post
    /// or in its ring: a site, a bomb, a flag, a radio (`bot_match`'s guard
    /// distance).
    pub fn guard_point(&self) -> Option<[f32; 3]> {
        self.guarded
    }

    /// Where the posts of what the bot guards stand ([`GuardHold`]), by
    /// its switches: a far guard with [`Self::far`] off holds near, and so
    /// does one walking back with [`Self::cues`] off.
    pub(crate) fn guard_hold(&self, view: &BotView) -> GuardHold {
        match self.objective_target(view) {
            Some(ObjTarget::Guard { hold, .. }) => match hold {
                GuardHold::Far if self.far => GuardHold::Far,
                GuardHold::FarNear if self.far => GuardHold::FarNear,
                GuardHold::Back if self.far && self.cues => GuardHold::Back,
                GuardHold::Watch => GuardHold::Watch,
                _ => GuardHold::Near,
            },
            _ => GuardHold::Near,
        }
    }

    /// The share of its objective's ways in that saw it at the last
    /// think ([`BotView::exposure`]).
    pub fn exposure(&self) -> Option<f32> {
        self.exposure
    }

    /// In a contested zone (an enemy stands in it too, out of sight, and
    /// the capture is held) the bot walks to the zone's far side, and back
    /// once there, until the contest ends or it leaves the zone. On
    /// mp_backlot's L-shaped `koth` room an axis and an allies bot each
    /// held a leg of it for the rest of the match, the axis one in the
    /// door its teammates queued behind (bot-objectives.md 12.6).
    fn sweep_contested(&mut self, view: &BotView) {
        let Some(UoView::Dom(d)) = view.uo.as_ref() else {
            self.obj.sweep = None;
            return;
        };
        let Some(f) = d
            .flags
            .iter()
            .find(|f| f.contested && inside_site(view.origin, &f.site))
        else {
            self.obj.sweep = None;
            return;
        };
        if self.obj.sweep.is_some_and(|at| !arrived(view.origin, at)) {
            return;
        }
        let (lo, hi) = (f.site.mins, f.site.maxs);
        let o = view.origin;
        self.obj.sweep = Some([lo[0] + hi[0] - o[0], lo[1] + hi[1] - o[1], f.site.stand[2]]);
    }

    /// The plant or defuse itself: the cmd for this tick while one is under
    /// way or starting, `None` to play on as usual.
    pub(super) fn think_objective(&mut self, view: &BotView, cmd: UserCmd) -> Option<UserCmd> {
        self.obj.rest = self.obj.rest.saturating_sub(1);
        let t = self.objective_target(view);
        if Self::press_swaps_guarded(view) && matches!(t, Some(ObjTarget::Defuse { .. })) {
            if self.obj.close_in > 0 {
                self.obj.close_in += 1;
            }
        } else {
            self.obj.close_in = 0;
        }
        let at = t.is_some_and(|t| Self::at_objective(view, &t));
        if let Some(re) = view.re.as_ref() {
            if matches!(t, Some(ObjTarget::Sweep(_))) && at {
                self.obj.swept = true;
            }
            if !re.objectives.iter().any(|m| m.carried) {
                self.obj.swept = false;
            }
        }
        // The domination flag picked stays the bot's until its team holds it.
        if let (Some(UoView::Dom(d)), Some(ObjTarget::Occupy(site))) = (view.uo.as_ref(), t) {
            self.obj.zone = d.flags.iter().position(|f| f.site.stand == site.stand);
        }
        self.sweep_contested(view);
        let ready = self.obj.rest == 0 && (at || (view.linked && self.obj.held > 0));
        self.fetching(view, t);
        let cur = [yaw_diff(view.view[0], 0.0), view.view[1]];
        match t {
            Some(ObjTarget::Plant(_)) if ready => {
                let plant_ticks =
                    (view.sd.as_ref().and_then(|sd| sd.key)).map_or(PLANT_TICKS, |k| k.plant_ticks);
                if self.obj.held >= plant_ticks || (self.obj.held >= START_TICKS && !view.linked) {
                    // Nothing planted: another zone's plant blocks this one,
                    // or the body never touched. Try the other site.
                    self.obj.held = 0;
                    self.obj.rest = RETRY_TICKS;
                    self.obj.pick = self.obj.pick.wrapping_add(1);
                    return None;
                }
                self.obj.held += 1;
                // Looking up: the press's rising edge also takes the item
                // or turret the view picks (`Cmd_Activate_f`), and a dropped
                // weapon in the zone sits on the floor.
                Some(self.hold_use(view, cmd, [PLANT_PITCH, cur[1]]))
            }
            Some(ObjTarget::Defuse { bomb: b, ticks, .. }) if ready => {
                if self.obj.held >= ticks || (self.obj.held >= START_TICKS && !view.linked) {
                    // The aim trace never reached the trigger (a body in
                    // the way); release and settle again.
                    self.obj.held = 0;
                    self.obj.settled = 0;
                    self.obj.rest = RETRY_TICKS;
                    return None;
                }
                let (tp, ty) = aim_angles(view, b.aim);
                let aim = self.turn_toward(view, [tp, ty]);
                if self.obj.held > 0 {
                    self.obj.held += 1;
                    return Some(self.hold_use(view, cmd, aim));
                }
                // Not started yet: a visible enemy comes first.
                if self.shoot && view.enemy.is_some() {
                    self.obj.settled = 0;
                    return None;
                }
                let off = yaw_diff(tp, cur[0]).hypot(yaw_diff(ty, cur[1]));
                self.obj.settled = if off < SETTLE_DEG {
                    self.obj.settled + 1
                } else {
                    0
                };
                let mut cmd = self.hold_use(view, cmd, aim);
                let swaps = Self::press_swaps_guarded(view)
                    && view.use_takes_weapon
                    && self.obj.close_in < DEFUSE_CLOSE_TICKS;
                if self.obj.settled >= SETTLE_TICKS && swaps {
                    // A weapon lying by the bomb outscores the trigger
                    // from here: close in over the bomb.
                    self.obj.close_in = self.obj.close_in.max(1);
                    self.obj.settled = 0;
                    cmd.buttons = 0;
                } else if self.obj.settled >= SETTLE_TICKS {
                    self.obj.held = 1;
                } else {
                    cmd.buttons = 0;
                }
                Some(cmd)
            }
            Some(ObjTarget::Pickup { aim: spot, .. }) if ready => {
                // Not with an enemy in sight: the pickup is a tap, and can
                // wait.
                if self.shoot && view.enemy.is_some() {
                    self.obj.settled = 0;
                    return None;
                }
                let (tp, ty) = aim_angles(view, spot);
                // The view's pitch stops at 80: one right underfoot settles
                // there.
                let tp = tp.clamp(-80.0, 80.0);
                let aim = self.turn_toward(view, [tp, ty]);
                let off = yaw_diff(tp, cur[0]).hypot(yaw_diff(ty, cur[1]));
                self.obj.settled = if off < SETTLE_DEG {
                    self.obj.settled + 1
                } else {
                    0
                };
                let mut cmd = self.hold_use(view, cmd, aim);
                // The primary it would rather lose in hand first: the swap
                // drops what is held, and a bot that tapped with its gun
                // out traded it.
                let swap = match view.uo.as_ref() {
                    Some(UoView::Bas(b)) => Self::swap_out(view, b.launchers.contains(&spot)),
                    _ => None,
                }
                .filter(|&w| w != view.weapon);
                if let Some(r) = swap {
                    cmd.weapon = r;
                    cmd.buttons = 0;
                } else if self.obj.settled < SETTLE_TICKS {
                    cmd.buttons = 0;
                } else {
                    // A tap: two ticks down, then up for a while. Held, the
                    // use key drops what the carrier holds (`re.gsc`
                    // `holduse`).
                    self.obj.held += 1;
                    if self.obj.held >= 2 {
                        if matches!(view.uo, Some(UoView::Bas(_))) {
                            self.tapped_launcher(spot);
                        }
                        self.obj.held = 0;
                        self.obj.settled = 0;
                        self.obj.rest = RETRY_TICKS;
                        self.obj.pick = self.obj.pick.wrapping_add(1);
                    }
                }
                Some(cmd)
            }
            Some(ObjTarget::Breach {
                stand: Some(st),
                aim: None,
                ..
            }) if arrived(view.origin, st) => {
                self.obj.idle += 1;
                if self.obj.idle >= BREACH_IDLE_TICKS {
                    self.obj.idle = 0;
                    self.obj.pick = self.obj.pick.wrapping_add(1);
                }
                None
            }
            Some(ObjTarget::Breach {
                aim: Some(spot), ..
            }) if at => {
                let rocket = view.rocket?;
                // A visible enemy comes first.
                if self.shoot && view.enemy.is_some() {
                    self.obj.settled = 0;
                    return None;
                }
                Some(self.breach(view, cmd, rocket, spot))
            }
            _ => {
                self.obj.held = 0;
                self.obj.settled = 0;
                self.obj.idle = 0;
                None
            }
        }
    }

    /// A `bas` launcher the body has come no nearer to in
    /// [`FETCH_STALL_TICKS`] is passed over: one lay where the graph's
    /// nearest node was a floor over it, and the bot walked into the wall
    /// for good; at another the use tap took nothing, and the bot stood
    /// there for twenty minutes.
    pub(super) fn fetching(&mut self, view: &BotView, t: Option<ObjTarget>) {
        let Some(ObjTarget::Pickup { aim, .. }) =
            t.filter(|_| matches!(view.uo, Some(UoView::Bas(_))))
        else {
            self.fetch = None;
            return;
        };
        let d = dist_sq(view.origin, aim).sqrt();
        let (best, ticks) = match self.fetch {
            Some((p, best, ticks)) if p == aim && d > best - FETCH_CLOSING => (best, ticks + 1),
            _ => (d, 0),
        };
        if ticks >= FETCH_STALL_TICKS {
            self.shunned.push((aim, SHUN_TICKS));
            self.fetch = None;
        } else {
            self.fetch = Some((aim, best, ticks));
        }
    }

    /// The S&D defuse and the `bas` plant and defuse hold use off the
    /// press a weapon lying in reach would answer ([`DEFUSE_CLOSE`]).
    fn press_swaps_guarded(view: &BotView) -> bool {
        view.sd.is_some() || matches!(view.uo, Some(UoView::Bas(_)))
    }

    /// `far`, or [`DEFUSE_CLOSE`] while closing in over a charge whose
    /// press would take a weapon lying by it.
    fn press_reach(&self, far: f32) -> f32 {
        if (1..DEFUSE_CLOSE_TICKS).contains(&self.obj.close_in) {
            DEFUSE_CLOSE
        } else {
            far
        }
    }

    /// The primary a `bas` bot holds when it taps for a weapon lying about,
    /// a launcher with `launcher`: with a slot free nothing drops (`None`);
    /// else an empty one before a loaded one, then one of the kind being
    /// fetched, then not the gun it fights with. A launcher fetch with a
    /// spent launcher and a gun drops the launcher; a gun fetch with a
    /// loaded launcher and a dry gun drops the gun.
    pub(super) fn swap_out(view: &BotView, launcher: bool) -> Option<u8> {
        let [a, b] = view.primaries;
        if a == 0 || b == 0 {
            return None;
        }
        let cost = |w: u8| {
            let bit = |m: u64| w < 64 && m >> w & 1 == 1;
            (
                bit(view.loaded),
                bit(view.launchers) != launcher,
                view.gun == Some(w),
                w != view.weapon,
            )
        };
        Some(if cost(b) < cost(a) { b } else { a })
    }

    /// A tap at a `bas` launcher: [`LAUNCHER_TAPS`] at one with no rocket
    /// in hand after passes it over for a while. The use pickup refused
    /// some for good (a bot stood tapping at one for 20 minutes).
    pub(super) fn tapped_launcher(&mut self, at: [f32; 3]) {
        let taps = match self.tapped {
            Some((p, n)) if arrived(p, at) => n + 1,
            _ => 1,
        };
        self.tapped = Some((at, taps));
        if taps >= LAUNCHER_TAPS {
            self.tapped = None;
            self.shunned.push((at, SHUN_TICKS));
        }
    }

    /// One tick of a stand at a base: the launcher out, the view on `spot`,
    /// a round fired once the view has rested on it, a reload after.
    fn breach(&mut self, view: &BotView, mut cmd: UserCmd, rocket: u8, spot: [f32; 3]) -> UserCmd {
        self.disengage();
        self.restart_stall(view.origin);
        let (tp, ty) = aim_angles(view, spot);
        let aim = self.turn_toward(view, [tp, ty]);
        let cur = [yaw_diff(view.view[0], 0.0), view.view[1]];
        let off = yaw_diff(tp, cur[0]).hypot(yaw_diff(ty, cur[1]));
        self.obj.settled = if off < SETTLE_DEG {
            self.obj.settled + 1
        } else {
            0
        };
        cmd.angles = cmd_angles(view, aim);
        if view.weapon != rocket {
            self.switch_to = Some(rocket);
            cmd.weapon = rocket;
            return cmd;
        }
        self.switch_to = None;
        cmd.weapon = rocket;
        if view.busy_ms != 0 {
            return cmd;
        }
        // A rocket leaves along the spread aim (`Weapon_RocketLauncher_Fire`):
        // up to 12 degrees off from the hip, 4 sighted. From the hip a
        // breach rocket struck the stand's own walls, and so did one fired
        // sighted with the cone a turn opened on the way up still wide:
        // the stand's checks clear the sighted floor only.
        let sighted = (!view.has_ads || view.ads_frac >= 1.0) && view.spread_scale == 0.0;
        if view.clip == 0 {
            cmd.wbuttons |= msg::WBUTTON_RELOAD;
        } else if view.has_ads {
            cmd.buttons |= msg::BUTTON_ADS;
        }
        if view.clip != 0 && sighted && self.obj.settled >= SETTLE_TICKS && self.fire_cooldown == 0
        {
            cmd.buttons |= msg::BUTTON_ATTACK;
            // The bit comes up between rounds.
            self.fire_cooldown = 2;
        }
        cmd
    }

    /// Use down, no move keys, the view at `aim`. Nothing else runs: the
    /// stall watch restarts from here and the fight is dropped.
    fn hold_use(&mut self, view: &BotView, mut cmd: UserCmd, aim: [f32; 2]) -> UserCmd {
        self.disengage();
        self.restart_stall(view.origin);
        cmd.buttons = msg::BUTTON_USE;
        cmd.angles = cmd_angles(view, aim);
        cmd
    }

    /// No round left in any gun, launcher or pistol.
    pub(super) fn dry(view: &BotView) -> bool {
        let spent = |w: u8| w == 0 || w >= 64 || view.loaded >> w & 1 == 0;
        view.gun.is_none()
            && view.rocket.is_none()
            && view.pistol.is_none_or(spent)
            && spent(view.weapon)
    }

    /// Shooting, only a rocket left (none goes at a player) and an enemy
    /// given up on ([`Self::count_futile`]): the bot fetches a gun lying
    /// near. Two such bots on mp_foy, seed 23, stood side by side in a
    /// charge room while one planted and the other defused, eight times
    /// over 5 min.
    fn unarmed(&self, view: &BotView) -> bool {
        let spent = |w: u8| w == 0 || w >= 64 || view.loaded >> w & 1 == 0;
        self.shoot
            && self.ignored.is_some()
            && view.rocket.is_some()
            && view.gun.is_none()
            && view.pistol.is_none_or(spent)
    }
}

/// The feet stand inside a zone, [`SITE_MARGIN`] in from its sides, or on
/// its stand when the zone is narrower than the margin.
pub(crate) fn inside_site(o: [f32; 3], s: &SiteView) -> bool {
    let inside = (0..2).all(|i| o[i] >= s.mins[i] + SITE_MARGIN && o[i] <= s.maxs[i] - SITE_MARGIN)
        && o[2] <= s.maxs[2]
        && o[2] + PLAYER_HEIGHT >= s.mins[2];
    let flat = (o[0] - s.stand[0]).hypot(o[1] - s.stand[1]);
    inside || (flat < 16.0 && (o[2] - s.stand[2]).abs() < 32.0)
}
