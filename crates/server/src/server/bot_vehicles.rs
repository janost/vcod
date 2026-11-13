//! The server half of bots in United Offensive vehicles
//! (`crate::bots::vehicle`): each bot's seat, and the vehicles it could
//! board, as the use key would find them (`game::vehicle::seat::usable`).

use super::*;
use crate::bots::{ArmorView, Crowd, GunSight, SeatView, VehicleSpot};
use crate::game::vehicle::info::KIND_TANK;

/// `vc_bot_vehicles`: 0 stops bots boarding; one seated rides on.
pub(crate) const VEHICLES_CVAR: &str = "vc_bot_vehicles";

impl Server {
    /// Fills [`crate::bots::BotView::seat`], `vehicles` and `armor` for
    /// this tick's views. A level with no vehicle leaves them empty.
    pub(super) fn bot_vehicle_views(
        &mut self,
        views: &mut [(usize, crate::bots::BotView)],
        teams: &[i32],
    ) {
        let on = crate::game::builtins::cvar::atoi(&self.live_cvar(VEHICLES_CVAR, "1")) != 0;
        // Every body on foot, by slot and team: what a driver keeps off.
        let walkers: Vec<(usize, [f32; 3], i32)> = self
            .core
            .clients
            .iter()
            .enumerate()
            .filter_map(|(i, c)| {
                let s = c.as_ref()?.sim.as_ref()?;
                let up =
                    s.pm_type == crate::spectate::PmType::Normal && !s.dead && s.seat.is_none();
                up.then(|| (i, s.ps.origin.into(), teams.get(i).copied().unwrap_or(0)))
            })
            .collect();
        let crewed = self.crewed_vehicles(teams);
        let Some(rt) = self.game.module.script.as_mut() else {
            return;
        };
        if rt.host.vehicles.live.is_empty() {
            return;
        }
        let targets: Vec<(usize, u32)> = self
            .game
            .module
            .driver
            .bots
            .iter()
            .filter_map(|(s, b)| Some((*s, b.vehicle_target()?)))
            .collect();
        rt.with_host(|host, cx| {
            let order = host.vehicles.in_slot_order();
            for (slot, view) in views.iter_mut() {
                if !view.playing {
                    continue;
                }
                view.seat = seat_view(host, *slot, &targets);
                if let Some(seat) = view.seat.as_mut().filter(|s| s.seat == 1) {
                    let mine = teams.get(*slot).copied().unwrap_or(0);
                    seat.crowd = crowd(host, *slot, mine, &walkers);
                    if let Some((id, _)) = host.vehicles.seat_of(*slot) {
                        seat.capturing = crate::game::vehicle::think::capturing(host, cx, id);
                    }
                    if seat.tank
                        && seat.gun_speed > 0.0
                        && let Some((id, _)) = host.vehicles.seat_of(*slot)
                    {
                        seat.sight = crate::game::vehicle::fire::main_gun_sight(host, cx, id)
                            .map(|(muzzle, dir, eye)| GunSight { muzzle, dir, eye });
                    }
                }
                if view.seat.is_some() || !on {
                    continue;
                }
                let reach = crate::bots::VEHICLE_SEEK_RANGE;
                for &id in &order {
                    if host.ents.get(id).is_none_or(|e| e.unlinked) {
                        continue;
                    }
                    let Some(v) = host.vehicles.live.get(&id) else {
                        continue;
                    };
                    let info = host.vehicles.info_of(v);
                    let spot = VehicleSpot {
                        num: id.0,
                        origin: v.pose.origin,
                        yaw: v.pose.angles[1],
                        mins: info.mins,
                        maxs: info.maxs,
                        tank: info.kind == KIND_TANK,
                    };
                    if spot.gap(view.origin) < reach
                        && crate::game::vehicle::seat::usable(host, cx, id, *slot)
                    {
                        view.vehicles.push(spot);
                    }
                }
            }
        });
        if crewed.is_empty() {
            return;
        }
        // What a rocket meets by the muzzle besides the world: the script
        // models and the hulls.
        let models = if views
            .iter()
            .any(|(_, v)| v.seat.is_none() && v.rocket.is_some())
        {
            self.game
                .module
                .script
                .as_mut()
                .map(|rt| rt.shot_models())
                .unwrap_or_default()
        } else {
            Vec::new()
        };
        for (slot, view) in views.iter_mut() {
            view.armor = self.armor_view(*slot, view, &crewed, teams, &models);
        }
    }

    /// Each live vehicle with a rider: its box, its velocity, its riders as
    /// a slot mask and their teams.
    fn crewed_vehicles(&self, teams: &[i32]) -> Vec<Crewed> {
        let Some(rt) = self.game.module.script.as_ref() else {
            return Vec::new();
        };
        let host = &rt.host;
        host.vehicles
            .in_slot_order()
            .into_iter()
            .filter_map(|id| {
                let v = host.vehicles.live.get(&id)?;
                let riders: Vec<usize> = v.seats.iter().flatten().copied().collect();
                if riders.is_empty() || host.ents.get(id).is_none_or(|e| e.unlinked) {
                    return None;
                }
                let info = host.vehicles.info_of(v);
                let spot = VehicleSpot {
                    num: id.0,
                    origin: v.pose.origin,
                    yaw: v.pose.angles[1],
                    mins: info.mins,
                    maxs: info.maxs,
                    tank: info.kind == KIND_TANK,
                };
                let mask = riders
                    .iter()
                    .filter(|s| **s < 64)
                    .fold(0u64, |m, s| m | 1 << s);
                let sides = riders
                    .iter()
                    .map(|s| teams.get(*s).copied().unwrap_or(0))
                    .collect();
                Some(Crewed {
                    spot,
                    velocity: v.velocity,
                    crew: mask,
                    sides,
                })
            })
            .collect()
    }

    /// [`crate::bots::BotView::armor`]: the nearest vehicle an enemy rides
    /// in, in a clear line from the eye, for a bot on foot with a loaded
    /// launcher (past a rocket's splash, inside its range) or one driving a
    /// tank (inside the gun's range).
    fn armor_view(
        &self,
        slot: usize,
        view: &crate::bots::BotView,
        crewed: &[Crewed],
        teams: &[i32],
        models: &[crate::game::combat::PlacedModel],
    ) -> Option<ArmorView> {
        if !self.game.module.bot_admin.shoot || !view.playing || view.dead {
            return None;
        }
        let (min, max, speed, launcher) = match view.seat {
            Some(s) if s.seat == 1 && s.tank => {
                (0.0, crate::bots::VEHICLE_GUN_RANGE, s.gun_speed, None)
            }
            Some(_) => return None,
            None => {
                let r = view.rocket?;
                let def = self.game.module.weapon_table.get(r as usize)?;
                (
                    crate::bots::VEHICLE_ROCKET_MIN,
                    crate::bots::VEHICLE_ROCKET_RANGE,
                    def.projectile_speed,
                    Some(def),
                )
            }
        };
        let mine = teams.get(slot).copied().unwrap_or(0);
        let hostile = |their: i32| match mine {
            script::TEAM_AXIS => their != script::TEAM_AXIS,
            script::TEAM_ALLIES => their != script::TEAM_ALLIES,
            _ => true,
        };
        let me = self.core.clients[slot].as_ref()?.sim.as_ref()?;
        let eye = glam::Vec3::from(me.eye_origin());
        let world = self.game.world.as_ref().map(|w| &w.collision);
        // Along the view and the hip cone round it as a shot this cmd
        // would spread (`Bot::think_armor` fires from the hip), nothing
        // within the rocket's splash of the muzzle.
        let muzzle_clear = match (launcher, world) {
            (Some(def), Some(w)) => super::cone_clear(
                w,
                models,
                crate::game::combat::muzzle_point(&me.ps),
                crate::game::combat::aim_frame([view.view[0], view.view[1]])[0],
                crate::game::combat::hip_spread_now(def, me),
                def.explosion_radius + crate::bots::ROCKET_CLEAR_MARGIN,
            ),
            _ => true,
        };
        crewed
            .iter()
            .filter(|c| {
                (slot >= 64 || c.crew >> slot & 1 == 0) && c.sides.iter().any(|t| hostile(*t))
            })
            .map(|c| (crate::bots::dist_sq(view.origin, c.spot.centre()), c))
            .filter(|(d, _)| *d >= min * min && *d <= max * max)
            .filter(|(_, c)| {
                // The hull's own clip may stop the trace short of its
                // middle: a stop at its box is the vehicle.
                world.is_none_or(|w| {
                    let to = glam::Vec3::from(c.spot.centre());
                    let t = w.shot_trace(eye, to);
                    let end: [f32; 3] = (eye + (to - eye) * t.fraction).into();
                    let up = end[2] - c.spot.origin[2];
                    t.fraction >= 1.0
                        || c.spot.gap(end) < ARMOR_REACH
                            && up > c.spot.mins[2] - ARMOR_REACH
                            && up < c.spot.maxs[2] + ARMOR_REACH
                })
            })
            .min_by(|a, b| a.0.total_cmp(&b.0))
            .map(|(_, c)| ArmorView {
                num: c.spot.num,
                centre: c.spot.centre(),
                velocity: c.velocity,
                crew: c.crew,
                shot_speed: speed,
                friend_in_line: self
                    .bot_friend_in_line(slot, c.spot.centre(), teams, None)
                    .0,
                muzzle_clear,
            })
    }

    /// Test-facing: the kind of the live vehicle `num` (`info::KIND_*`).
    pub fn test_vehicle_kind(&self, num: u32) -> Option<i32> {
        let host = &self.game.module.script.as_ref()?.host;
        let (_, v) = host.vehicles.live.iter().find(|(id, _)| id.0 == num)?;
        Some(host.vehicles.info_of(v).kind)
    }

    /// Test-facing: vehicles the seven-second stuck kill destroyed this
    /// level.
    pub fn test_stuck_kills(&self) -> u32 {
        self.game
            .module
            .script
            .as_ref()
            .map_or(0, |rt| rt.host.vehicles.stuck_kills)
    }

    /// Test-facing: each live vehicle in pool order, as a bot would see it.
    pub fn test_vehicles(&self) -> Vec<VehicleSpot> {
        let Some(rt) = self.game.module.script.as_ref() else {
            return Vec::new();
        };
        let host = &rt.host;
        host.vehicles
            .in_slot_order()
            .into_iter()
            .filter_map(|id| {
                let v = host.vehicles.live.get(&id)?;
                let info = host.vehicles.info_of(v);
                Some(VehicleSpot {
                    num: id.0,
                    origin: v.pose.origin,
                    yaw: v.pose.angles[1],
                    mins: info.mins,
                    maxs: info.maxs,
                    tank: info.kind == KIND_TANK,
                })
            })
            .collect()
    }
}

/// The seat `slot` sits in, off the vehicle's record and wire fields.
fn seat_view(
    host: &crate::game::host::GameHost,
    slot: usize,
    targets: &[(usize, u32)],
) -> Option<SeatView> {
    let (id, seat) = host.vehicles.seat_of(slot)?;
    let v = host.vehicles.live.get(&id)?;
    let info = host.vehicles.info_of(v);
    let tank = info.kind == KIND_TANK;
    let yaw = v.pose.angles[1];
    // The turret's yaw off the hull, and the gunner's off the turret
    // (coduo-vehicles.md 2).
    let gun_yaw = match seat {
        1 if tank => Some(yaw + v.wire.angles2[1]),
        2 if info.has_gunner => Some(yaw + v.wire.angles2[1] + v.wire.origin2[1]),
        _ => None,
    };
    // The seat's gun's round: a tank's shell flies, the MGs' are bullets.
    let gun = match seat {
        1 => v.weapons[0],
        2 => v.weapons[2],
        _ => 0,
    };
    let gun_speed = host
        .weapons
        .get(gun)
        .filter(|d| gun != 0 && d.weapon_type == "projectile")
        .map_or(0.0, |d| d.projectile_speed);
    Some(SeatView {
        seat,
        tank,
        origin: v.pose.origin,
        yaw,
        speed: v.forward_speed,
        gun_yaw,
        driven: v.seats[1].is_some(),
        boarding: targets.iter().any(|&(s, n)| s != slot && n == id.0),
        gun_speed,
        crowd: Crowd::default(),
        contact_ms: v.phys.contact_ms(host.level_time_ms),
        level_ms: host.level_time_ms,
        sight: None,
        traverse: info.turret.horiz_left != 0.0 || info.turret.horiz_right != 0.0,
        capturing: false,
    })
}

/// A trace toward a vehicle's middle that stops this near its box met the
/// hull.
const ARMOR_REACH: f32 = 24.0;

/// A vehicle with a rider, for [`Server::armor_view`].
struct Crewed {
    spot: VehicleSpot,
    velocity: [f32; 3],
    /// The riders' slots as a mask, and each rider's team.
    crew: u64,
    sides: Vec<i32>,
}
/// How far past the hull's front or back a driver keeps teammates clear,
/// at rest and per unit/s of speed, and how wide of its sides.
const CROWD_REACH: f32 = 120.0;
const CROWD_REACH_PER_SPEED: f32 = 0.6;
const CROWD_MARGIN: f32 = 40.0;

/// [`Crowd`] round the vehicle `slot` drives, of the bodies on foot on
/// team `mine` (an axis or allies one; nobody is a teammate in `dm`).
fn crowd(
    host: &crate::game::host::GameHost,
    slot: usize,
    mine: i32,
    walkers: &[(usize, [f32; 3], i32)],
) -> Crowd {
    let mut c = Crowd::default();
    if mine != script::TEAM_AXIS && mine != script::TEAM_ALLIES {
        return c;
    }
    let Some((id, _)) = host.vehicles.seat_of(slot) else {
        return c;
    };
    let Some(v) = host.vehicles.live.get(&id) else {
        return c;
    };
    let info = host.vehicles.info_of(v);
    let (s, k) = v.pose.angles[1].to_radians().sin_cos();
    let reach = CROWD_REACH + v.forward_speed.abs() * CROWD_REACH_PER_SPEED;
    let corner = info.mins[0]
        .abs()
        .max(info.maxs[0])
        .hypot(info.mins[1].abs().max(info.maxs[1]));
    for &(_, p, _) in walkers.iter().filter(|w| w.0 != slot && w.2 == mine) {
        let (dx, dy) = (p[0] - v.pose.origin[0], p[1] - v.pose.origin[1]);
        if (p[2] - v.pose.origin[2]).abs() > info.maxs[2] + 64.0 {
            continue;
        }
        let (x, y) = (dx * k + dy * s, -dx * s + dy * k);
        let wide = y > info.mins[1] - CROWD_MARGIN && y < info.maxs[1] + CROWD_MARGIN;
        c.ahead |= wide && x > 0.0 && x < info.maxs[0] + reach;
        c.behind |= wide && x < 0.0 && x > info.mins[0] - reach;
        c.beside |= dx.hypot(dy) < corner + CROWD_MARGIN;
    }
    c
}

/// How far ahead along its route a driver steers: a hull turns wide, and
/// the graph's nodes stand a grid pitch apart.
const LOOK_AHEAD: f32 = 220.0;
/// A route node this near the hull is passed.
const PASS: f32 = 110.0;
/// A route goes again when its goal moves this far, when it makes no
/// headway on its next node for `HEADWAY_MS`, or after `REPLAN_MS`.
const RETARGET: f32 = 300.0;
const HEADWAY_MS: i32 = 5000;
const REPLAN_MS: i32 = 15_000;
/// A roam drives to a node at least this far off.
const ROAM_MIN: f32 = 1200.0;
/// The node expansions one plan may take.
const PLAN_EXPANSIONS: usize = 30_000;
/// Fit tests, about 3.5 us each on mp_foy, and cache lookups the warm
/// pass takes a tick ([`Server::warm_hull_fits`]).
const WARM_TESTS: usize = 600;
const WARM_LOOKUPS: usize = 8000;
/// A node a hull fits on: its box, this share of the vehicle's half
/// width, clear of the world and the vehicle clip from `FIT_LIFT` over the
/// feet up `FIT_HEIGHT`, and no edge on the way steeper than a step.
const FIT_SHARE: f32 = 0.75;
const FIT_LIFT: f32 = 18.0;
const FIT_HEIGHT: f32 = 48.0;
const VEHICLE_CLIP: u32 = 0x200;

/// One driver's route over the navigation graph.
pub(super) struct DriveRoute {
    path: Vec<u32>,
    next: usize,
    /// The goal point it was planned for, `None` for a roam.
    goal: Option<[f32; 3]>,
    planned_ms: i32,
    /// Nearest the hull has come to the next node, and when.
    best: f32,
    best_ms: i32,
}

/// The drivers' routes, for the graph they were found on, and where a hull
/// of a given half width fits.
#[derive(Default)]
pub(super) struct BotDrives {
    routes: BTreeMap<usize, DriveRoute>,
    fits: HullFits,
    graph: usize,
}

/// Fit tests by node point and box half width, kept across graph
/// rebuilds: a rebuild follows a script brush link change, and a fit
/// reads only what its box meets, so only the fits a toggled submodel's
/// bounds overlap are dropped. A `bas` restart relinks the walls, the
/// graph is rebuilt, and every driver plans again at once
/// (cod11-map-cycle.md 4.9).
#[derive(Default)]
struct HullFits {
    fit: HashMap<([u32; 3], u32), bool>,
    /// Each lump-27 model's link when the fits were taken.
    linked: Vec<bool>,
    /// The warm pass's next node, for the graph it walks; `usize::MAX`
    /// once it has passed every node.
    warm: usize,
    warm_graph: usize,
}

impl HullFits {
    /// Drops the fits a submodel linked or unlinked since the last call
    /// can change; all of them on a new map.
    fn relink(
        &mut self,
        collision: &vcod_common::collision::CollisionWorld,
        bounds: &[([f32; 3], [f32; 3])],
    ) {
        let linked: Vec<bool> = (0..collision.model_count())
            .map(|m| collision.model_linked(m))
            .collect();
        if linked.len() != self.linked.len() {
            self.fit.clear();
            self.warm = 0;
        } else if linked != self.linked {
            self.warm = 0;
            let toggled: Vec<(glam::Vec3, glam::Vec3)> = (0..linked.len())
                .filter(|&m| linked[m] != self.linked[m])
                .map(|m| {
                    // A model no bounds name drops every fit.
                    bounds.get(m).map_or(
                        (glam::Vec3::splat(f32::MIN), glam::Vec3::splat(f32::MAX)),
                        |(lo, hi)| (glam::Vec3::from(*lo) - 2.0, glam::Vec3::from(*hi) + 2.0),
                    )
                })
                .collect();
            self.fit.retain(|(p, key), _| {
                let (lo, hi) = fit_box(glam::Vec3::from(p.map(f32::from_bits)), *key as f32);
                !toggled
                    .iter()
                    .any(|(a, b)| lo.cmple(*b).all() && hi.cmpge(*a).all())
            });
        }
        self.linked = linked;
    }
}

/// Whether a hull of fit radius `r` fits on a node at `node`.
fn test_fit(collision: &vcod_common::collision::CollisionWorld, node: glam::Vec3, r: f32) -> bool {
    let p = node + glam::Vec3::Z * FIT_LIFT;
    !collision.box_position_test(
        p,
        glam::Vec3::new(-r, -r, 0.0),
        glam::Vec3::new(r, r, FIT_HEIGHT),
        vcod_common::collision::CONTENTS_SOLID | VEHICLE_CLIP,
    )
}

/// The box a hull of fit radius `r` is tested in on a node at `node`.
fn fit_box(node: glam::Vec3, r: f32) -> (glam::Vec3, glam::Vec3) {
    let p = node + glam::Vec3::Z * FIT_LIFT;
    (
        p + glam::Vec3::new(-r, -r, 0.0),
        p + glam::Vec3::new(r, r, FIT_HEIGHT),
    )
}

impl BotDrives {
    pub(super) fn forget(&mut self, slot: usize) {
        self.routes.remove(&slot);
    }
}

impl Server {
    /// The point a bot driving a vehicle steers at toward `goal`, along a
    /// route over the graph's nodes its hull fits on (no ladder, jump or
    /// step-high edge); `None` with nothing to drive to.
    pub(super) fn drive_waypoint(
        &mut self,
        slot: usize,
        goal: crate::bots::Goal,
        seat: &SeatView,
    ) -> Option<[f32; 3]> {
        use crate::bots::Goal;
        let g = self.game.module.driver.nav.clone()?;
        let graph = std::sync::Arc::as_ptr(&g) as usize;
        let drives = &mut self.game.module.bot_drives;
        if drives.graph != graph {
            drives.graph = graph;
            drives.routes.clear();
        }
        let half = self.seated_half_width(slot)?;
        let now = self.core.sv_time_ms;
        let at = glam::Vec3::from(seat.origin);
        let flat = |p: glam::Vec3| (p - at).truncate().length();
        let want = match goal {
            Goal::Hold => {
                self.game.module.bot_drives.routes.remove(&slot);
                return None;
            }
            Goal::To(p) => Some(p),
            Goal::Roam | Goal::Away(_) => None,
        };
        let stale = self
            .game
            .module
            .bot_drives
            .routes
            .get(&slot)
            .is_none_or(|r| {
                let moved = match (r.goal, want) {
                    (Some(a), Some(b)) => glam::Vec3::from(a).distance(b.into()) > RETARGET,
                    (None, None) => false,
                    _ => true,
                };
                moved
                    || r.next >= r.path.len() && want.is_none()
                    || now.wrapping_sub(r.best_ms) > HEADWAY_MS
                    || now.wrapping_sub(r.planned_ms) > REPLAN_MS
            });
        if stale {
            self.game.module.bot_drives.routes.remove(&slot);
            let mut rng = self.game.module.driver.bots.get(&slot).map_or(1, |b| b.rng);
            let path = self.plan_drive(&g, half, at, want, &mut rng);
            if let Some(b) = self.game.module.driver.bots.get_mut(&slot) {
                b.rng = rng;
            }
            let path = path?;
            self.game.module.bot_drives.routes.insert(
                slot,
                DriveRoute {
                    path,
                    next: 0,
                    goal: want,
                    planned_ms: now,
                    best: f32::INFINITY,
                    best_ms: now,
                },
            );
        }
        let r = self.game.module.bot_drives.routes.get_mut(&slot)?;
        while r.next < r.path.len() && flat(g.nodes[r.path[r.next] as usize]) < PASS {
            r.next += 1;
            r.best = f32::INFINITY;
            r.best_ms = now;
        }
        let Some(&n) = r.path.get(r.next) else {
            // Driven out: a goal is there, a roam plans again next tick.
            return None;
        };
        let d = flat(g.nodes[n as usize]);
        if d < r.best - 8.0 {
            r.best = d;
            r.best_ms = now;
        }
        let ahead = r.path[r.next..]
            .iter()
            .map(|&n| g.nodes[n as usize])
            .find(|p| flat(*p) >= LOOK_AHEAD)
            .unwrap_or(g.nodes[*r.path.last()? as usize]);
        Some(ahead.into())
    }

    /// Fills the hull fit cache a slice a tick, every node for every live
    /// vehicle's box, so a driver's plan finds its tests already run: a
    /// plan into fits never taken ran up to 21k tests, 96 ms, in one tick
    /// (cod11-map-cycle.md 4.9). A fit is the same whenever it is taken
    /// (`HullFits` drops the ones a link change moves), so the plans and
    /// the routes are those of a cold cache.
    pub(super) fn warm_hull_fits(&mut self) {
        let (Some(g), Some(world), Some(rt)) = (
            self.game.module.driver.nav.as_ref(),
            self.game.world.as_ref(),
            self.game.module.script.as_ref(),
        ) else {
            return;
        };
        let host = &rt.host;
        if host.vehicles.live.is_empty()
            || crate::game::builtins::cvar::atoi(&self.live_cvar(VEHICLES_CVAR, "1")) == 0
        {
            return;
        }
        let hull = &mut self.game.module.bot_drives.fits;
        hull.relink(&world.collision, &host.model_bounds);
        let graph = std::sync::Arc::as_ptr(g) as usize;
        if hull.warm_graph != graph {
            (hull.warm_graph, hull.warm) = (graph, 0);
        }
        if hull.warm >= g.nodes.len() {
            return;
        }
        // Each box's fit radius, as `plan_drive` keys it.
        let mut keys: Vec<(u32, f32)> = host
            .vehicles
            .live
            .values()
            .map(|v| {
                let info = host.vehicles.info_of(v);
                let half = (info.maxs[0] - info.mins[0]).min(info.maxs[1] - info.mins[1]) * 0.5;
                let r = half * FIT_SHARE;
                (r as u32, r)
            })
            .collect();
        keys.sort_by_key(|k| k.0);
        keys.dedup_by_key(|k| k.0);
        let (mut tests, mut lookups) = (0, 0);
        while hull.warm < g.nodes.len() && tests < WARM_TESTS && lookups < WARM_LOOKUPS {
            let node = g.nodes[hull.warm];
            let at = node.to_array().map(f32::to_bits);
            for &(key, r) in &keys {
                lookups += 1;
                hull.fit.entry((at, key)).or_insert_with(|| {
                    tests += 1;
                    test_fit(&world.collision, node, r)
                });
            }
            hull.warm += 1;
        }
    }

    /// Half the narrower side of the box of the vehicle `slot` sits in.
    fn seated_half_width(&self, slot: usize) -> Option<f32> {
        let host = &self.game.module.script.as_ref()?.host;
        let (id, _) = host.vehicles.seat_of(slot)?;
        let info = host.vehicles.info_of(host.vehicles.live.get(&id)?);
        Some(((info.maxs[0] - info.mins[0]).min(info.maxs[1] - info.mins[1])) * 0.5)
    }

    /// A* from the node nearest `at` to the node nearest `want` (a random
    /// far node for a roam) over the nodes a hull of `half` fits on. A goal
    /// out of reach routes to the reached node nearest it.
    fn plan_drive(
        &mut self,
        g: &crate::nav::NavGraph,
        half: f32,
        at: glam::Vec3,
        want: Option<[f32; 3]>,
        rng: &mut u64,
    ) -> Option<Vec<u32>> {
        let world = self.game.world.as_ref()?;
        let r = half * FIT_SHARE;
        let key = r as u32;
        let hull = &mut self.game.module.bot_drives.fits;
        let bounds = self
            .game
            .module
            .script
            .as_ref()
            .map_or(&[][..], |rt| &rt.host.model_bounds[..]);
        hull.relink(&world.collision, bounds);
        let fit = &mut hull.fit;
        let mut fits = |n: u32| {
            let node = g.nodes[n as usize];
            *fit.entry((node.to_array().map(f32::to_bits), key))
                .or_insert_with(|| test_fit(&world.collision, node, r))
        };
        let from = g.nearest(at.into())?;
        let to = match want {
            Some(p) => g.goal_node(p).or_else(|| g.nearest(p))?,
            None => {
                // A few draws for a far node a hull fits on.
                (0..16).find_map(|_| {
                    let n = (rand_from(rng) as usize % g.nodes.len()) as u32;
                    let p = g.nodes[n as usize];
                    ((p - at).truncate().length() > ROAM_MIN && fits(n)).then_some(n)
                })?
            }
        };
        let goal = g.nodes[to as usize];
        let h = |n: u32| g.nodes[n as usize].distance(goal);
        let mut open = std::collections::BinaryHeap::new();
        // Each node's best cost so far and the node it came from;
        // infinite while unreached.
        let mut cost = vec![(f32::INFINITY, u32::MAX); g.nodes.len()];
        cost[from as usize] = (0.0, from);
        open.push(std::cmp::Reverse((ordered(h(from)), from)));
        let (mut best, mut best_h) = (from, h(from));
        let mut expanded = 0;
        while let Some(std::cmp::Reverse((_, n))) = open.pop() {
            // A node is tested as it comes off the heap, not as it goes
            // on: the frontier left at the goal is never tested.
            if n != from && !fits(n) {
                continue;
            }
            if n == to {
                best = to;
                break;
            }
            expanded += 1;
            if expanded > PLAN_EXPANSIONS {
                break;
            }
            let here = cost[n as usize].0;
            let p = g.nodes[n as usize];
            if h(n) < best_h {
                (best, best_h) = (n, h(n));
            }
            for &m in &g.edges[n as usize] {
                let q = g.nodes[m as usize];
                let plain = !g.jumps.contains(&(n, m)) && !g.leaps.contains(&(n, m));
                if !plain || (q.z - p.z).abs() > vcod_common::pmove::STEPSIZE {
                    continue;
                }
                let c = here + p.distance(q);
                if c < cost[m as usize].0 {
                    cost[m as usize] = (c, n);
                    open.push(std::cmp::Reverse((ordered(c + h(m)), m)));
                }
            }
        }
        let mut path = vec![best];
        // `best` was expanded, so its chain runs back to `from`, its own
        // parent.
        loop {
            let last = path[path.len() - 1];
            let prev = cost[last as usize].1;
            if prev == last {
                break;
            }
            path.push(prev);
        }
        path.reverse();
        (path.len() > 1).then_some(path)
    }
}

/// An f32 cost as a heap key; costs are finite and non-negative.
fn ordered(c: f32) -> u32 {
    c.to_bits()
}

/// How long a vehicle holds its pose before its nodes are tested: a
/// second. A tank on the move is met by the walk's own stall handling.
const PARK_TICKS: u32 = 20;
/// How far, across and up or down, from a parked vehicle's origin its
/// collmap can cover a node: the tanks' (`script_vehicle_collmap` *28,
/// x -136..124, y -72..72) reach 154 at a corner, and a body 15 more.
const PARK_REACH: f32 = 192.0;
/// What a parked vehicle may drift off the pose it parked in, in units and
/// degrees, and still be parked.
const PARK_SLACK: f32 = 1.0;
/// How far a parked vehicle may move before the nodes it covered are let
/// go: a tank at mp_cassino's stair foot at (440, -1088, 13) parked again
/// a few units off three times, and each move let the stair go for a
/// second (seed 1, bot-navigation.md "Parked vehicles").
const PARK_SHOVE: f32 = 48.0;

/// One live vehicle's pose, the ticks it has held it within
/// [`PARK_SLACK`], and the graph's nodes its collmap covered where it last
/// parked.
struct Parked {
    pose: crate::game::vehicle::Pose,
    still: u32,
    at: [f32; 3],
    nodes: Vec<u32>,
}

/// Each live vehicle's [`Parked`]; the union of their nodes, sorted, is
/// what the followers plan round (`nav::Follower::blocked`,
/// bot-navigation.md "Parked vehicles").
#[derive(Default)]
pub(super) struct ParkedVehicles {
    graph: usize,
    each: HashMap<vcod_gsc::EntId, Parked>,
    pub(super) blocked: std::sync::Arc<[u32]>,
}

impl Server {
    /// Updates [`ParkedVehicles`] for this tick's vehicle poses. The graph
    /// was built round the vehicles where they stood then; one driven off
    /// and left over the graph's nodes covers them, and a bot walking that
    /// way wedged between mp_cassino's stair at (426, -958, 60) and a
    /// Panzer IV's rear (seed 23).
    pub(super) fn park_vehicles(&mut self) {
        let (Some(g), Some(world), Some(rt)) = (
            self.game.module.driver.nav.as_ref(),
            self.game.world.as_ref(),
            self.game.module.script.as_ref(),
        ) else {
            return;
        };
        let parked = &mut self.game.module.bot_parked;
        let graph = std::sync::Arc::as_ptr(g) as usize;
        if parked.graph != graph {
            *parked = ParkedVehicles {
                graph,
                ..Default::default()
            };
        }
        let live = &rt.host.vehicles.live;
        let mut changed = false;
        parked.each.retain(|id, p| {
            let keep = live.contains_key(id);
            changed |= !keep && !p.nodes.is_empty();
            keep
        });
        for (id, v) in live {
            let p = parked.each.entry(*id).or_insert(Parked {
                pose: v.pose,
                still: 0,
                at: v.pose.origin,
                nodes: Vec::new(),
            });
            let near = |a: [f32; 3], b: [f32; 3], d: f32| (0..3).all(|i| (a[i] - b[i]).abs() <= d);
            if !(near(p.pose.origin, v.pose.origin, PARK_SLACK)
                && near(p.pose.angles, v.pose.angles, PARK_SLACK))
            {
                (p.pose, p.still) = (v.pose, 0);
                if !p.nodes.is_empty() && !near(p.at, v.pose.origin, PARK_SHOVE) {
                    p.nodes.clear();
                    changed = true;
                }
                continue;
            }
            p.still = p.still.saturating_add(1);
            if p.still == PARK_TICKS {
                let nodes = g.blocked_near(&world.collision, v.pose.origin.into(), PARK_REACH);
                changed |= nodes != p.nodes;
                (p.at, p.nodes) = (v.pose.origin, nodes);
            }
        }
        if changed {
            let mut all: Vec<u32> = parked
                .each
                .values()
                .flat_map(|p| p.nodes.iter().copied())
                .collect();
            all.sort_unstable();
            all.dedup();
            parked.blocked = all.into();
        }
    }
}
