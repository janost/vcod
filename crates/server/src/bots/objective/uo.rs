//! United Offensive's objectives: `ctf`, `dom`, `hq` and `bas`.

use super::*;

impl Bot {
    /// United Offensive's objective gametypes. Odd ranks keep their own
    /// side's objective and even ranks go for the other side's, so a lone
    /// bot attacks.
    pub(super) fn uo_target(&self, view: &BotView, uo: &UoView) -> Option<ObjTarget> {
        let rank = uo.rank();
        let defends = rank % 2 == 1;
        let guard = |at: [f32; 3]| ObjTarget::Guard {
            at,
            inner: 0.0,
            hold: GuardHold::Near,
        };
        // Held from the ways in (bot-objectives.md 14).
        let far = |at: [f32; 3]| ObjTarget::Guard {
            at,
            inner: 0.0,
            hold: GuardHold::Far,
        };
        match uo {
            UoView::Ctf(c) => {
                let far = |at: [f32; 3]| ObjTarget::Guard {
                    at,
                    inner: 0.0,
                    hold: if self.ctf_near {
                        GuardHold::FarNear
                    } else {
                        GuardHold::Far
                    },
                };
                if c.carrying {
                    // The capture wants our flag home.
                    if c.ours.home.is_some() {
                        return Some(ObjTarget::Deliver(c.theirs.goal.stand));
                    }
                    if !self.ctf_carry {
                        return Some(ObjTarget::Occupy(c.theirs.goal));
                    }
                    // Our flag lying near: the carrier's touch returns it
                    // too. Else wait on a post near the goal, in cover
                    // from its ways in, while the rest bring it back.
                    if let Some(d) = c.ours.dropped
                        && dist_sq(view.origin, d.stand) < CTF_RETURN_NEAR * CTF_RETURN_NEAR
                    {
                        return Some(ObjTarget::Deliver(d.stand));
                    }
                    return Some(guard(c.theirs.goal.stand));
                }
                // Our flag lying out: a touch returns it, and their carrier
                // cannot score until it is home.
                if let Some(d) = c.ours.dropped
                    && (defends
                        || c.theirs.carried
                        || dist_sq(view.origin, d.stand) < CTF_RETURN_NEAR * CTF_RETURN_NEAR)
                {
                    return Some(ObjTarget::Deliver(d.stand));
                }
                // After the enemy carrying our flag, where a teammate last
                // saw him.
                if let Some(at) = c.hunt {
                    return Some(ObjTarget::Chase(at));
                }
                // Ahead of our carrier or at his side ([`Bot::ctf_escort`]).
                if let Some(at) = c.lead {
                    return Some(ObjTarget::Escort(at));
                }
                // On the carrier's way out ([`Bot::ctf_watch`]).
                if let Some(p) = c.watch {
                    return Some(ObjTarget::Guard {
                        at: p.at,
                        inner: 0.0,
                        hold: GuardHold::Watch,
                    });
                }
                if defends {
                    return Some(match c.ours.home {
                        Some(h) => far(h.stand),
                        // Carried off: hold the goal their carrier needs.
                        None => ObjTarget::Guard {
                            at: c.ours.goal.stand,
                            inner: 0.0,
                            hold: GuardHold::Near,
                        },
                    });
                }
                if let Some(at) = c.theirs.home.or(c.theirs.dropped) {
                    return Some(ObjTarget::Deliver(at.stand));
                }
                c.escort.map(ObjTarget::Escort)
            }
            UoView::Dom(d) => {
                if let Some(at) = self.obj.sweep {
                    return Some(ObjTarget::Sweep(at));
                }
                let open: Vec<usize> = (0..d.flags.len()).filter(|i| !d.flags[*i].ours).collect();
                if open.is_empty() {
                    let n = d.flags.len();
                    return (n > 0).then(|| far(d.flags[rank % n].site.stand));
                }
                let pick = self
                    .obj
                    .zone
                    .filter(|z| open.contains(z))
                    .unwrap_or_else(|| self.dom_pick(view, d, &open, rank));
                Some(ObjTarget::Occupy(d.flags[pick].site))
            }
            UoView::Hq(h) => match h.radio {
                Some(r) if r.ours => Some(far(r.at)),
                Some(r) => Some(ObjTarget::Radio {
                    at: r.at,
                    radius: r.radius,
                    height: r.height,
                    stand: r.stand,
                }),
                None => h.next.map(guard),
            },
            UoView::Bas(b) => {
                let side = |ours: bool| -> Vec<&BasBaseView> {
                    b.bases.iter().filter(|x| x.ours == ours).collect()
                };
                let (mine, theirs) = (side(true), side(false));
                // A tank's driver breaches with its gun: the other side's
                // standing base it has a shot at, else the nearest, driven
                // to (bot-objectives.md 7.2, "Tanks").
                if view.seat.is_some_and(|s| s.sight.is_some()) {
                    let standing = || theirs.iter().filter(|x| !x.breached);
                    if let Some(t) = standing().find(|x| x.shot.is_some()).or_else(|| {
                        standing().min_by(|x, y| {
                            dist_sq(view.origin, x.at).total_cmp(&dist_sq(view.origin, y.at))
                        })
                    }) {
                        return Some(ObjTarget::Breach {
                            base: t.at,
                            stand: None,
                            aim: t.shot,
                        });
                    }
                }
                // A charge on one of ours: every defender goes for it, and
                // an attacker near it. One that does not shoot only from
                // near: the planter cannot stop it, and a side's defuser
                // back from across the map in time for every plant left
                // two lone bases standing for good (seeds 17 and 19).
                // Not one that goes off before a defuse could finish, nor
                // one whose hold under way cannot: the defuser held use
                // inside the blast to the end (seed 5). A second to settle
                // the view on the trigger.
                let fits = |x: &BasBaseView, c: BombView| {
                    let need = if self.obj.held > 0 {
                        BAS_DEFUSE_TICKS.saturating_sub(self.obj.held) as f32 / 20.0
                    } else {
                        let trip = dist_sq(view.origin, c.origin).sqrt() / BAS_RUN_SPEED;
                        BAS_DEFUSE_TICKS as f32 / 20.0 + trip + 1.0
                    };
                    x.blows_in.is_none_or(|s| s > need)
                };
                let ticking = mine
                    .iter()
                    .filter(|x| x.planted)
                    .find_map(|x| x.charge.filter(|c| fits(x, *c)));
                if let Some(c) = ticking
                    && ((defends && self.shoot)
                        || theirs.is_empty()
                        || dist_sq(view.origin, c.origin) < BAS_DEFUSE_NEAR * BAS_DEFUSE_NEAR)
                {
                    return Some(ObjTarget::Defuse {
                        bomb: c,
                        reach: self.press_reach(BAS_REACH),
                        ticks: BAS_DEFUSE_TICKS,
                    });
                }
                // Nothing left to fire: the nearest weapon lying within
                // `DRY_FETCH`, before the `kill` (`DRY_TICKS`). A planter
                // that ran dry in its charge's room killed itself, and the
                // charge was defused (seeds 1 and 7).
                // With a rocket left, a gun only, and from nearer.
                let dry = Self::dry(view);
                let reach = if dry { DRY_FETCH } else { ARMS_FETCH };
                if (dry || self.unarmed(view))
                    && let Some(&w) =
                        b.guns
                            .iter()
                            .chain(b.launchers.iter().filter(|l| {
                                dry && b.bases.iter().all(|x| {
                                    x.breached || dist_sq(**l, x.at) > BAS_BUNKER * BAS_BUNKER
                                })
                            }))
                            .filter(|l| dist_sq(view.origin, **l) < reach * reach)
                            .filter(|l| !self.shunned.iter().any(|s| arrived(s.0, **l)))
                            .min_by(|x, y| {
                                dist_sq(view.origin, **x).total_cmp(&dist_sq(view.origin, **y))
                            })
                {
                    return Some(ObjTarget::Pickup { aim: w, from: w });
                }
                // A defender keeps the charge room of a breached base of
                // ours, the one way a plant comes; with none breached there
                // is nothing to defend and it attacks with the rest. One
                // that does not shoot stops no plant there, and defused
                // each charge the moment it was down for good: it attacks
                // too, and defuses what it passes near.
                let open: Vec<&&BasBaseView> = mine.iter().filter(|x| x.breached).collect();
                // A charge of ours ticking: a defender that shoots joins
                // its planter in holding its room. One on one, the
                // defender in the room won most fights and defused. So does
                // one whose side has breached the other side's last base,
                // charged or not: it came from its own base after the
                // planter had died and the charge was defused (seed 23).
                let backing = theirs
                    .iter()
                    .find(|x| x.planted && x.charge.is_some())
                    .or_else(|| theirs.first().filter(|x| theirs.len() == 1 && x.breached))
                    .filter(|_| self.shoot);
                if theirs.is_empty()
                    || (defends && self.shoot && !open.is_empty() && backing.is_none())
                {
                    let n = open.len();
                    return if n > 0 {
                        let x = open[(rank / 2) % n];
                        Some(guard(x.charge.map_or(x.at, |c| c.origin)))
                    } else {
                        (!mine.is_empty()).then(|| guard(mine[(rank / 2) % mine.len()].at))
                    };
                }
                // Without shooting, a side's two bots split over the other
                // side's bases: on one, its lone defender ran back and
                // defused every plant (seed 7's 67 defuses).
                let pick = if self.shoot { rank / 2 } else { rank };
                let t = match backing {
                    Some(x) if defends => x,
                    _ => theirs[(pick + self.deaths as usize) % theirs.len()],
                };
                Some(match t.charge {
                    // Our charge: hold its room against the defuse, then
                    // clear the blast before it goes.
                    Some(c) if t.planted => ObjTarget::Guard {
                        hold: GuardHold::Near,
                        at: c.origin,
                        inner: if t.blows_in.is_some_and(|s| s > BAS_CLEAR_S) {
                            0.0
                        } else {
                            BAS_BLAST
                        },
                    },
                    Some(c) if t.breached => ObjTarget::Defuse {
                        bomb: c,
                        reach: self.press_reach(BAS_REACH),
                        ticks: BAS_PLANT_TICKS,
                    },
                    _ if t.breached => guard(t.at),
                    // Standing: rockets breach it. Without one, the nearest
                    // launcher lying about first.
                    _ if view.rocket.is_some() => ObjTarget::Breach {
                        base: t.at,
                        stand: self.breach_stand(&mine, t),
                        aim: t.shot,
                    },
                    // Those in a standing base, ours or theirs, lie behind
                    // its walls.
                    _ => match b
                        .launchers
                        .iter()
                        .filter(|l| {
                            b.bases
                                .iter()
                                .all(|x| x.breached || dist_sq(**l, x.at) > BAS_BUNKER * BAS_BUNKER)
                        })
                        .filter(|l| !self.shunned.iter().any(|s| arrived(s.0, **l)))
                        .min_by(|x, y| {
                            dist_sq(view.origin, **x).total_cmp(&dist_sq(view.origin, **y))
                        }) {
                        Some(&l) => ObjTarget::Pickup { aim: l, from: l },
                        None => guard(t.at),
                    },
                })
            }
        }
    }

    /// The breach stand a bot with a rocket walks to: the stands nearest
    /// its own side's nearest base first, one given up on (`pick`) for the
    /// next.
    fn breach_stand(&self, mine: &[&BasBaseView], t: &BasBaseView) -> Option<[f32; 3]> {
        let home = mine
            .iter()
            .map(|m| m.at)
            .min_by(|a, b| dist_sq(*a, t.at).total_cmp(&dist_sq(*b, t.at)))
            .unwrap_or(t.at);
        let mut stands = t.stands.clone();
        stands.sort_by(|a, b| dist_sq(*a, home).total_cmp(&dist_sq(*b, home)));
        (!stands.is_empty()).then(|| stands[self.obj.pick as usize % stands.len()])
    }

    /// The domination flag a bot sets out for: among those its team does
    /// not hold, nearest first, dealt out by rank.
    fn dom_pick(&self, view: &BotView, d: &DomView, open: &[usize], rank: usize) -> usize {
        let mut by_dist = open.to_vec();
        by_dist.sort_by(|a, b| {
            let da = dist_sq(view.origin, d.flags[*a].site.stand);
            let db = dist_sq(view.origin, d.flags[*b].site.stand);
            da.total_cmp(&db).then(a.cmp(b))
        });
        by_dist[rank % by_dist.len()]
    }
}
