//! The server half of bots in United Offensive vehicles
//! (`crate::bots::vehicle`): each bot's seat, and the vehicles it could
//! board, as the use key would find them (`game::vehicle::seat::usable`).

use super::bot_world::team_of;
use super::*;
use crate::bots::{ArmorView, Crowd, GunSight, SeatView, VehicleSpot};
use crate::game::vehicle::info::KIND_TANK;

/// `vc_bot_vehicles`: 0 keeps bots out of vehicles: none boards, and one
/// seated gets out.
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
                let up = s.playing() && s.seat.is_none();
                up.then(|| (i, s.ps.origin.into(), team_of(teams, i)))
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
                if let Some(seat) = view.seat.as_mut() {
                    seat.off = !on;
                }
                if let Some(seat) = view.seat.as_mut().filter(|s| s.seat == 1) {
                    let mine = team_of(teams, *slot);
                    seat.crowd = crowd(host, *slot, mine, &walkers);
                    if let Some((id, _)) = host.vehicles.seat_of(*slot) {
                        seat.capturing = crate::game::vehicle::think::capturing(host, cx, id);
                    }
                    if seat.tank
                        && seat.gun_speed > 0.0
                        && let Some((id, _)) = host.vehicles.seat_of(*slot)
                    {
                        let speed = seat.gun_speed;
                        seat.sight = crate::game::vehicle::fire::main_gun_sight(host, cx, id).map(
                            |(muzzle, dir, eye)| GunSight {
                                muzzle,
                                dir,
                                eye,
                                speed,
                            },
                        );
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
                    let spot = spot_of(id, v, info);
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
        // What a rocket meets by the muzzle, or a tank's shell on its way,
        // besides the world: the script models and the hulls.
        let models = if views.iter().any(|(_, v)| match v.seat {
            None => v.rocket.is_some(),
            Some(s) => s.sight.is_some(),
        }) {
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
                let spot = spot_of(id, v, info);
                let mask = riders
                    .iter()
                    .filter(|s| **s < 64)
                    .fold(0u64, |m, s| m | 1 << s);
                let sides = riders.iter().map(|s| team_of(teams, *s)).collect();
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
    /// tank (inside the gun's range), and whether the tank's shell fired
    /// now meets it ([`shell_on_hull`]).
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
        let mine = team_of(teams, slot);
        let hostile = |their: i32| match mine {
            script::TEAM_AXIS => their != script::TEAM_AXIS,
            script::TEAM_ALLIES => their != script::TEAM_ALLIES,
            _ => true,
        };
        let me = self.core.clients[slot].as_ref()?.sim.as_ref()?;
        let eye = glam::Vec3::from(me.eye_origin());
        let world = self.game.world.as_ref().map(|w| &w.collision);
        let (_, c) = crewed
            .iter()
            .filter(|c| {
                (slot >= 64 || c.crew >> slot & 1 == 0) && c.sides.iter().any(|t| hostile(*t))
            })
            .map(|c| (crate::bots::dist_sq(view.origin, c.spot.centre()), c))
            .filter(|(d, _)| *d >= min * min && *d <= max * max)
            .filter(|(_, c)| {
                world.is_none_or(|w| {
                    let to = glam::Vec3::from(c.spot.centre());
                    let t = w.shot_trace(eye, to);
                    let end: [f32; 3] = (eye + (to - eye) * t.fraction).into();
                    hull_met(&c.spot, t.fraction, end)
                })
            })
            .min_by(|a, b| a.0.total_cmp(&b.0))?;
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
        let shell_on = match (view.seat.and_then(|s| s.sight), world) {
            (Some(g), Some(w)) => {
                let rt = self.game.module.script.as_ref()?;
                let own = rt.host.vehicles.seat_of(slot)?.0;
                shell_on_hull(w, models, own, g, &c.spot)
            }
            _ => true,
        };
        Some(ArmorView {
            num: c.spot.num,
            centre: c.spot.centre(),
            velocity: c.velocity,
            crew: c.crew,
            shot_speed: speed,
            friend_in_line: self
                .bot_friend_in_line(slot, c.spot.centre(), teams, None, false)
                .0,
            muzzle_clear,
            shell_on,
        })
    }

    /// Test-facing: the kind of the live vehicle `num` (`info::KIND_*`).
    #[cfg(any(test, feature = "testing"))]
    pub fn test_vehicle_kind(&self, num: u32) -> Option<i32> {
        let host = &self.game.module.script.as_ref()?.host;
        let (_, v) = host.vehicles.live.iter().find(|(id, _)| id.0 == num)?;
        Some(host.vehicles.info_of(v).kind)
    }

    /// Test-facing: vehicles the seven-second stuck kill destroyed this
    /// level.
    #[cfg(any(test, feature = "testing"))]
    pub fn test_stuck_kills(&self) -> u32 {
        self.game
            .module
            .script
            .as_ref()
            .map_or(0, |rt| rt.host.vehicles.stuck_kills)
    }

    /// Test-facing: a blast of `damage` on vehicle `num` from itself, as
    /// the stock self-destruct's `doDamage` (`_tankdrive_gmi.gsc`); false
    /// when `num` is no live vehicle.
    #[cfg(any(test, feature = "testing"))]
    pub fn test_damage_vehicle(&mut self, num: u32, damage: i32) -> bool {
        let Some(rt) = self.game.module.script.as_mut() else {
            return false;
        };
        let Some((&id, v)) = rt.host.vehicles.live.iter().find(|(id, _)| id.0 == num) else {
            return false;
        };
        let o = v.pose.origin;
        rt.damage_vehicle(
            id,
            damage,
            crate::game::combat::Attacker::Entity(id),
            id,
            [o[0], o[1], o[2] + 1.0],
            "MOD_EXPLOSIVE",
        );
        true
    }

    /// Test-facing: from now on, keep each vehicle step's record.
    #[cfg(any(test, feature = "testing"))]
    pub fn test_log_vehicle_thinks(&mut self) {
        if let Some(rt) = self.game.module.script.as_mut() {
            rt.host.vehicles.think_log = Some(Vec::new());
        }
    }

    /// Test-facing: the steps logged since [`Self::test_log_vehicle_thinks`].
    #[cfg(any(test, feature = "testing"))]
    pub fn test_vehicle_thinks(&self) -> Vec<crate::game::vehicle::ThinkLog> {
        self.game
            .module
            .script
            .as_ref()
            .and_then(|rt| rt.host.vehicles.think_log.clone())
            .unwrap_or_default()
    }

    /// Test-facing: where the bots read a burning hull, by entity number.
    pub fn test_wrecks(&self) -> Vec<[f32; 3]> {
        self.game.module.bot_parked.wrecks()
    }

    /// Test-facing: each live vehicle in pool order, as a bot would see it.
    #[cfg(any(test, feature = "testing"))]
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
                Some(spot_of(id, v, info))
            })
            .collect()
    }
}

/// Half the narrower side of a vehicle's box: what its fits are keyed by.
fn half_width(info: &crate::game::vehicle::info::VehicleInfo) -> f32 {
    (info.maxs[0] - info.mins[0]).min(info.maxs[1] - info.mins[1]) * 0.5
}

/// Vehicle `id` as a bot sees it: where it stands, its box, whether a tank.
fn spot_of(
    id: vcod_gsc::EntId,
    v: &crate::game::vehicle::Vehicle,
    info: &crate::game::vehicle::info::VehicleInfo,
) -> VehicleSpot {
    VehicleSpot {
        num: id.0,
        origin: v.pose.origin,
        yaw: v.pose.angles[1],
        mins: info.mins,
        maxs: info.maxs,
        tank: info.kind == KIND_TANK,
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
        short: false,
        capturing: false,
        off: false,
    })
}

/// A trace toward a vehicle's middle that stops this near its box met the
/// hull.
const ARMOR_REACH: f32 = 24.0;

/// Whether a trace toward `spot`'s middle that stopped at `fraction`, at
/// `end`, met the hull: clear through, or stopped within [`ARMOR_REACH`] of
/// its box, the hull's own clip being able to stop it short of the middle.
fn hull_met(spot: &VehicleSpot, fraction: f32, end: [f32; 3]) -> bool {
    let up = end[2] - spot.origin[2];
    fraction >= 1.0
        || spot.gap(end) < ARMOR_REACH
            && up > spot.mins[2] - ARMOR_REACH
            && up < spot.maxs[2] + ARMOR_REACH
}

/// Whether a shell from the tank gun `gun` (its own hull `own`) strikes
/// the hull `spot` before anything else, or bursts on the world within
/// [`ARMOR_REACH`] of its box. A line the driver's eye had over a wall
/// flew the shell over the hull it aimed at, into a wall 800 units past
/// it, on mp_cassino (bot-objectives.md 10.11).
fn shell_on_hull(
    world: &vcod_common::collision::CollisionWorld,
    models: &[crate::game::combat::PlacedModel],
    own: vcod_gsc::EntId,
    gun: GunSight,
    spot: &VehicleSpot,
) -> bool {
    let muzzle = glam::Vec3::from(gun.muzzle);
    let half = spot
        .mins
        .iter()
        .chain(&spot.maxs)
        .fold(0.0f32, |m, v| m.max(v.abs()));
    let reach = muzzle.distance(spot.centre().into()) + half + ARMOR_REACH;
    let strike =
        super::breach::shell_strike(world, models, own, muzzle, gun.dir.into(), gun.speed, reach);
    match strike {
        Some((_, Some(m))) => m.id.0 == spot.num,
        Some((at, None)) => hull_met(spot, 0.0, at.into()),
        None => false,
    }
}

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
    let Some((id, _)) = host.vehicles.seat_of(slot) else {
        return Crowd::default();
    };
    let Some(v) = host.vehicles.live.get(&id) else {
        return Crowd::default();
    };
    let info = host.vehicles.info_of(v);
    let hull = spot_of(id, v, info);
    crowd_round(&hull, v.forward_speed, slot, mine, walkers)
}

/// [`crowd`]'s strips round `hull` moving at `speed`: the teammates of
/// `mine` (an axis or allies team) on foot other than `slot` in the strip ahead or behind, out to
/// [`CROWD_REACH`] and more with speed, or within a corner's turn.
fn crowd_round(
    hull: &VehicleSpot,
    speed: f32,
    slot: usize,
    mine: i32,
    walkers: &[(usize, [f32; 3], i32)],
) -> Crowd {
    let mut c = Crowd::default();
    if mine != script::TEAM_AXIS && mine != script::TEAM_ALLIES {
        return c;
    }
    let (s, k) = hull.yaw.to_radians().sin_cos();
    let reach = CROWD_REACH + speed.abs() * CROWD_REACH_PER_SPEED;
    let (mins, maxs) = (hull.mins, hull.maxs);
    let corner = mins[0].abs().max(maxs[0]).hypot(mins[1].abs().max(maxs[1]));
    for &(_, p, _) in walkers.iter().filter(|w| w.0 != slot && w.2 == mine) {
        let (dx, dy) = (p[0] - hull.origin[0], p[1] - hull.origin[1]);
        if (p[2] - hull.origin[2]).abs() > maxs[2] + 64.0 {
            continue;
        }
        let (x, y) = (dx * k + dy * s, -dx * s + dy * k);
        let wide = y > mins[1] - CROWD_MARGIN && y < maxs[1] + CROWD_MARGIN;
        c.ahead |= wide && x > 0.0 && x < maxs[0] + reach;
        c.behind |= wide && x < 0.0 && x > mins[0] - reach;
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

/// One driver's route over the navigation graph.
pub(super) struct DriveRoute {
    path: Vec<u32>,
    next: usize,
    /// The goal point it was planned for, `None` for a roam.
    goal: Option<[f32; 3]>,
    /// Its last node stands more than [`PASS`] off the goal: the hull
    /// cannot reach it.
    short: bool,
    planned_ms: i32,
    /// Nearest the hull has come to the next node, and when.
    best: f32,
    best_ms: i32,
    /// For a goal the plan found no route to, what it failed on.
    failed: Option<PlanInputs>,
}

/// What a plan toward a goal answers from: its start and goal nodes, the
/// fit key and the fits' link epoch. The same inputs replay the same A*
/// over the same cached fits, so a failure stands until one moves.
#[derive(Clone, Copy, Debug, PartialEq)]
struct PlanInputs {
    from: u32,
    to: u32,
    key: u32,
    links: u64,
}

impl PlanInputs {
    fn of(
        g: &crate::nav::NavGraph,
        half: f32,
        at: glam::Vec3,
        want: [f32; 3],
        links: u64,
    ) -> Option<Self> {
        Some(PlanInputs {
            from: g.nearest(at.into())?,
            to: g.goal_node(want).or_else(|| g.nearest(want))?,
            key: (half * FIT_SHARE) as u32,
            links,
        })
    }
}

/// The drivers' routes, for the graph they were found on, and where a hull
/// of a given half width fits.
#[derive(Default)]
pub(super) struct BotDrives {
    routes: BTreeMap<usize, DriveRoute>,
    fits: HullFits,
    /// The graph generation the routes were found on (`BotDriver::graph`).
    graph: u64,
}

impl BotDrives {
    /// A restart of the level: the routes ran over the old level's
    /// vehicles; the fits stay, the map being the same.
    pub(super) fn restart(&mut self) {
        self.routes.clear();
    }
}

/// Fit tests by node point and box half width, kept across graph
/// rebuilds: a rebuild follows a script brush link change, and a fit
/// reads only what its box meets, so only the fits a toggled submodel's
/// bounds overlap are dropped. A `bas` restart relinks the walls, the
/// graph is rebuilt, and every driver plans again at once
/// (cod11-map-cycle.md 4.9).
#[derive(Default)]
struct HullFits {
    fit: FitCache,
    /// Each lump-27 model's link when the fits were taken.
    linked: Vec<bool>,
    /// The warm pass's next node, for the graph generation `warm_graph`; `usize::MAX`
    /// once it has passed every node.
    warm: usize,
    warm_graph: u64,
    /// Counts the relinks that dropped fits.
    links: u64,
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
            self.links += 1;
        } else if linked != self.linked {
            self.warm = 0;
            self.links += 1;
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
        vcod_common::collision::CONTENTS_SOLID | vcod_common::collision::CONTENTS_VEHICLE_CLIP,
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

    /// Whether `slot`'s route toward `want` (`None` a roam) is to be
    /// planned again at `now`: none kept, the goal moved, a roam driven
    /// out, no headway, or old.
    fn stale(&self, slot: usize, want: Option<[f32; 3]>, now: i32) -> bool {
        self.routes.get(&slot).is_none_or(|r| {
            let moved = match (r.goal, want) {
                (Some(a), Some(b)) => glam::Vec3::from(a).distance(b.into()) > RETARGET,
                (None, None) => false,
                _ => true,
            };
            moved
                || r.next >= r.path.len() && want.is_none()
                || now.wrapping_sub(r.best_ms) > HEADWAY_MS
                || now.wrapping_sub(r.planned_ms) > REPLAN_MS
        })
    }

    /// Keeps a plan's route, or for a goal no route reached an empty one
    /// with the inputs it `failed` on.
    fn keep(
        &mut self,
        slot: usize,
        path: Option<Vec<u32>>,
        want: Option<[f32; 3]>,
        short: bool,
        failed: Option<PlanInputs>,
        now: i32,
    ) {
        let failed = if path.is_some() { None } else { failed };
        if path.is_none() && failed.is_none() {
            return;
        }
        let route = DriveRoute {
            path: path.unwrap_or_default(),
            next: 0,
            goal: want,
            short,
            planned_ms: now,
            best: f32::INFINITY,
            best_ms: now,
            failed,
        };
        self.routes.insert(slot, route);
    }
}

impl Server {
    /// The point a bot driving a vehicle steers at toward `goal`, along a
    /// route over the graph's nodes its hull fits on (no ladder, jump or
    /// step-high edge); `None` with nothing to drive to, `seat.short` set
    /// when that is a route run out short of the goal.
    pub(super) fn drive_waypoint(
        &mut self,
        slot: usize,
        goal: crate::bots::Goal,
        seat: &mut SeatView,
    ) -> Option<[f32; 3]> {
        use crate::bots::Goal;
        let g = self.game.module.driver.nav.clone()?;
        let graph = self.game.module.driver.graph;
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
        let mut stale = self.game.module.bot_drives.stale(slot, want, now);
        // A failure stands while its inputs do: the same A* would fail again.
        if !stale
            && let Some(failed) = self
                .game
                .module
                .bot_drives
                .routes
                .get(&slot)
                .and_then(|r| r.failed)
        {
            let links = self.fit_links();
            stale = want.and_then(|p| PlanInputs::of(&g, half, at, p, links)) != Some(failed);
        }
        if stale {
            self.game.module.bot_drives.routes.remove(&slot);
            let mut rng = self.game.module.driver.bots.get(&slot).map_or(1, |b| b.rng);
            let path = self.plan_drive(&g, half, at, want, &mut rng);
            if let Some(b) = self.game.module.driver.bots.get_mut(&slot) {
                b.rng = rng;
            }
            let short = want
                .zip(path.as_ref().and_then(|p| p.last()))
                .is_some_and(|(p, &n)| g.nodes[n as usize].distance(glam::Vec3::from(p)) > PASS);
            let links = self.game.module.bot_drives.fits.links;
            let failed = want.and_then(|p| PlanInputs::of(&g, half, at, p, links));
            self.game
                .module
                .bot_drives
                .keep(slot, path, want, short, failed, now);
        }
        let r = self.game.module.bot_drives.routes.get_mut(&slot)?;
        while r.next < r.path.len() && flat(g.nodes[r.path[r.next] as usize]) < PASS {
            r.next += 1;
            r.best = f32::INFINITY;
            r.best_ms = now;
        }
        let Some(&n) = r.path.get(r.next) else {
            // Driven out: a goal is there, or as near as the hull goes; a
            // roam plans again next tick.
            seat.short = r.short;
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
        let graph = self.game.module.driver.graph;
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
                let r = half_width(host.vehicles.info_of(v)) * FIT_SHARE;
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
        Some(half_width(info))
    }

    /// The fits' link epoch, once they have dropped what the brush links
    /// since moved.
    fn fit_links(&mut self) -> u64 {
        let hull = &mut self.game.module.bot_drives.fits;
        if let Some(world) = self.game.world.as_ref() {
            let bounds = self
                .game
                .module
                .script
                .as_ref()
                .map_or(&[][..], |rt| &rt.host.model_bounds[..]);
            hull.relink(&world.collision, bounds);
        }
        hull.links
    }

    /// [`plan_route`] for the vehicle's hull, on the fits cache once it has
    /// dropped what the brush links since moved.
    fn plan_drive(
        &mut self,
        g: &crate::nav::NavGraph,
        half: f32,
        at: glam::Vec3,
        want: Option<[f32; 3]>,
        rng: &mut u64,
    ) -> Option<Vec<u32>> {
        let world = self.game.world.as_ref()?;
        let hull = &mut self.game.module.bot_drives.fits;
        let bounds = self
            .game
            .module
            .script
            .as_ref()
            .map_or(&[][..], |rt| &rt.host.model_bounds[..]);
        hull.relink(&world.collision, bounds);
        plan_route(g, &world.collision, &mut hull.fit, half, at, want, rng)
    }
}

/// The fit cache: whether a hull fits, by node point bits and fit radius.
type FitCache = HashMap<([u32; 3], u32), bool>;

/// A* from the node nearest `at` to the node nearest `want` (a random far
/// node for a roam) over the nodes a hull of `half` fits on, through `fit`.
/// A goal out of reach routes to the reached node nearest it.
fn plan_route(
    g: &crate::nav::NavGraph,
    collision: &vcod_common::collision::CollisionWorld,
    fit: &mut FitCache,
    half: f32,
    at: glam::Vec3,
    want: Option<[f32; 3]>,
    rng: &mut u64,
) -> Option<Vec<u32>> {
    let r = half * FIT_SHARE;
    let key = r as u32;
    let mut fits = |n: u32| {
        let node = g.nodes[n as usize];
        *fit.entry((node.to_array().map(f32::to_bits), key))
            .or_insert_with(|| test_fit(collision, node, r))
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
        for m in g.next(n) {
            let q = g.nodes[m as usize];
            let plain = !g.has(n, m, |k| k.jump) && !g.has(n, m, |k| k.leap);
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
    /// Once it burns ([`crate::game::vehicle::Vehicle::dead`]).
    wreck: Option<Wreck>,
}

/// A burning hull where it died, and the graph's nodes inside
/// [`crate::bots::WRECK_REACH`] of it with their distance across.
struct Wreck {
    at: [f32; 3],
    nodes: Vec<(u32, f32)>,
}

/// Each live vehicle's [`Parked`]; the union of their nodes, sorted, is
/// what the followers plan round (`nav::Follower::blocked`,
/// bot-navigation.md "Parked vehicles"), with the burning hulls' added
/// ([`Self::blocked_at`], bot-objectives.md 10.10).
#[derive(Default)]
pub(super) struct ParkedVehicles {
    /// The graph generation the nodes are on (`BotDriver::graph`).
    graph: u64,
    each: HashMap<vcod_gsc::EntId, Parked>,
    pub(super) blocked: std::sync::Arc<[u32]>,
    /// [`Self::blocked`] and every burning hull's nodes, sorted.
    round_wrecks: std::sync::Arc<[u32]>,
}

/// A bot inside a burning hull's reach plans round the hull's nodes this
/// much nearer it than his own feet, one grid pitch at its coarsest: the
/// way out never turns in past the hull.
const WRECK_DEEPER: f32 = 48.0;

impl ParkedVehicles {
    /// The burning hulls, in entity order (`bots::BotView::wrecks`).
    pub(super) fn wrecks(&self) -> Vec<[f32; 3]> {
        let mut w: Vec<_> = (self.each.iter())
            .filter_map(|(id, p)| Some((id.0, p.wreck.as_ref()?.at)))
            .collect();
        w.sort_unstable_by_key(|w| w.0);
        w.into_iter().map(|w| w.1).collect()
    }

    /// What the follower of a bot at `at` plans round: the parked
    /// vehicles' nodes and the burning hulls'; of a hull whose reach he
    /// stands in, only those [`WRECK_DEEPER`] nearer it than he is.
    pub(super) fn blocked_at(&self, at: Option<[f32; 3]>) -> std::sync::Arc<[u32]> {
        let reach = crate::bots::WRECK_REACH;
        let inside = |w: &[f32; 3], a: [f32; 3]| {
            let d = (w[0] - a[0]).hypot(w[1] - a[1]);
            (d < reach && (w[2] - a[2]).abs() < reach).then_some(d)
        };
        let Some(a) = at else {
            return self.blocked.clone();
        };
        let wrecks = || self.each.values().filter_map(|p| p.wreck.as_ref());
        if wrecks().all(|w| inside(&w.at, a).is_none()) {
            return self.round_wrecks.clone();
        }
        let mut all = self.blocked.to_vec();
        for w in wrecks() {
            let keep = inside(&w.at, a).map_or(f32::MAX, |d| d - WRECK_DEEPER);
            all.extend(w.nodes.iter().filter(|n| n.1 < keep).map(|n| n.0));
        }
        all.sort_unstable();
        all.dedup();
        all.into()
    }
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
        let host = &rt.host;
        let live: Vec<LiveHull> = (host.vehicles.live.iter())
            .map(|(id, v)| LiveHull {
                id: *id,
                pose: v.pose,
                dead: v.dead,
                linked: host.ents.get(*id).is_some_and(|e| !e.unlinked),
            })
            .collect();
        let graph = self.game.module.driver.graph;
        self.game
            .module
            .bot_parked
            .update(g, graph, &world.collision, &live);
    }
}

/// What [`ParkedVehicles::update`] reads of one live vehicle.
struct LiveHull {
    id: vcod_gsc::EntId,
    pose: crate::game::vehicle::Pose,
    dead: bool,
    /// Its entity is linked: a dead one burns until its second blast
    /// unlinks it.
    linked: bool,
}

impl ParkedVehicles {
    /// One tick of the live vehicles on graph `g` (`graph` its generation): a
    /// pose held [`PARK_TICKS`] covers the nodes its collmap blocks, a move
    /// past [`PARK_SHOVE`] lets them go, and a dead linked hull burns where
    /// it died.
    fn update(
        &mut self,
        g: &crate::nav::NavGraph,
        graph: u64,
        collision: &vcod_common::collision::CollisionWorld,
        live: &[LiveHull],
    ) {
        if self.graph != graph {
            *self = ParkedVehicles {
                graph,
                ..Default::default()
            };
        }
        let mut changed = false;
        self.each.retain(|id, p| {
            let keep = live.iter().any(|v| v.id == *id);
            changed |= !keep && (!p.nodes.is_empty() || p.wreck.is_some());
            keep
        });
        for v in live {
            let p = self.each.entry(v.id).or_insert(Parked {
                pose: v.pose,
                still: 0,
                at: v.pose.origin,
                nodes: Vec::new(),
                wreck: None,
            });
            // A live hull is no wreck, whatever a record of its number left
            // (a restart reissues map vehicles under the same handles).
            if !v.dead && p.wreck.take().is_some() {
                changed = true;
            }
            // A hull burns where it died until its second blast deletes it.
            if p.wreck.is_none() && v.dead && v.linked {
                let at = v.pose.origin;
                let reach = crate::bots::WRECK_REACH;
                let nodes = g
                    .nodes_in_box(at.into(), reach, reach)
                    .into_iter()
                    .map(|n| {
                        let d = g.nodes[n as usize] - glam::Vec3::from(at);
                        (n, d.truncate().length())
                    })
                    .filter(|n| n.1 < reach)
                    .collect();
                p.wreck = Some(Wreck { at, nodes });
                changed = true;
            }
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
                let nodes = g.blocked_near(collision, v.pose.origin.into(), PARK_REACH);
                changed |= nodes != p.nodes;
                (p.at, p.nodes) = (v.pose.origin, nodes);
            }
        }
        if changed {
            let mut all: Vec<u32> = (self.each.values())
                .flat_map(|p| p.nodes.iter().copied())
                .collect();
            all.sort_unstable();
            all.dedup();
            let mut round = all.clone();
            for w in self.each.values().filter_map(|p| p.wreck.as_ref()) {
                round.extend(w.nodes.iter().map(|n| n.0));
            }
            round.sort_unstable();
            round.dedup();
            self.blocked = all.into();
            self.round_wrecks = round.into();
        }
    }
}

#[cfg(test)]
mod tests {
    use glam::Vec3;
    use vcod_common::collision::{submodel_test_world, test_world};

    use super::*;
    use crate::nav::NavGraph;

    /// A tank's shell meets an enemy hull (here a world box in its place)
    /// along a level line; pitched 4 degrees up it flies over it into the
    /// wall behind, and a high wall between takes a level one.
    #[test]
    fn a_shell_over_the_hull_or_into_a_wall_is_not_on_it() {
        let spot = VehicleSpot {
            num: 7,
            origin: [800.0, 0.0, 0.0],
            yaw: 0.0,
            mins: [-100.0, -50.0, 0.0],
            maxs: [100.0, 50.0, 80.0],
            tank: true,
        };
        let hull = (Vec3::new(700.0, -50.0, 0.0), Vec3::new(900.0, 50.0, 80.0));
        let behind = (
            Vec3::new(1000.0, -512.0, 0.0),
            Vec3::new(1016.0, 512.0, 400.0),
        );
        let between = (
            Vec3::new(400.0, -512.0, 0.0),
            Vec3::new(416.0, 512.0, 300.0),
        );
        let gun = |pitch: f32| GunSight {
            muzzle: [0.0, 0.0, 60.0],
            dir: [pitch.to_radians().cos(), 0.0, pitch.to_radians().sin()],
            speed: 10000.0,
            eye: [-100.0, 0.0, 160.0],
        };
        let own = vcod_gsc::EntId(1, 0);
        let open = test_world(&[hull, behind]);
        assert!(shell_on_hull(&open, &[], own, gun(0.0), &spot));
        assert!(!shell_on_hull(&open, &[], own, gun(4.0), &spot));
        let walled = test_world(&[hull, behind, between]);
        assert!(!shell_on_hull(&walled, &[], own, gun(0.0), &spot));
    }

    /// A `nx` by `ny` grid of nodes `pitch` apart from the origin, each
    /// joined both ways to its four neighbours.
    fn grid(nx: usize, ny: usize, pitch: f32) -> NavGraph {
        let id = |x: usize, y: usize| (y * nx + x) as u32;
        let mut nodes = Vec::new();
        let mut edges = Vec::new();
        for y in 0..ny {
            for x in 0..nx {
                nodes.push(Vec3::new(x as f32 * pitch, y as f32 * pitch, 0.0));
                let mut e = Vec::new();
                if x > 0 {
                    e.push(id(x - 1, y));
                }
                if x + 1 < nx {
                    e.push(id(x + 1, y));
                }
                if y > 0 {
                    e.push(id(x, y - 1));
                }
                if y + 1 < ny {
                    e.push(id(x, y + 1));
                }
                edges.push(e);
            }
        }
        NavGraph::from_parts(nodes, edges)
    }

    fn plan(
        g: &NavGraph,
        world: &vcod_common::collision::CollisionWorld,
        half: f32,
        from: u32,
        to: u32,
    ) -> Vec<u32> {
        let goal = g.nodes[to as usize].to_array();
        let at = g.nodes[from as usize];
        let path = plan_route(g, world, &mut FitCache::new(), half, at, Some(goal), &mut 1);
        let path = path.expect("no route");
        // A chain of the graph's own edges from the start.
        assert_eq!(path[0], from);
        for w in path.windows(2) {
            assert!(g.linked(w[0], w[1]), "{w:?} is no edge");
        }
        path
    }

    /// A goal no route reaches is planned for again only when the plan's
    /// inputs move or the headway runs out; a roam that found none draws
    /// again the next tick.
    #[test]
    fn a_failed_plan_stands_while_its_inputs_do() {
        let failed = PlanInputs {
            from: 1,
            to: 9,
            key: 40,
            links: 0,
        };
        let plans = |want: Option<[f32; 3]>, moves_at: Option<i32>| {
            let mut drives = BotDrives::default();
            (0..=100)
                .filter(|tick| {
                    let now = tick * 50;
                    let inputs = PlanInputs {
                        from: if moves_at.is_some_and(|t| *tick >= t) {
                            2
                        } else {
                            1
                        },
                        ..failed
                    };
                    let stale = drives.stale(3, want, now)
                        || drives.routes[&3].failed.is_some_and(|f| f != inputs);
                    if stale {
                        drives.keep(3, None, want, false, want.and(Some(inputs)), now);
                    }
                    stale
                })
                .count()
        };
        let goal = Some([1000.0, 0.0, 0.0]);
        assert_eq!(plans(goal, None), 1);
        assert_eq!(plans(goal, Some(40)), 2);
        assert_eq!(plans(None, None), 101);
    }

    /// A pillar 40 wide on the middle of a 5 x 3 grid's first row, 48
    /// apart: a hull whose fit radius clears the gaps beside it drives
    /// past it, a wider one goes round by the next row, and a goal on the
    /// far side of a wall no hull fits round is driven to the node nearest
    /// it.
    #[test]
    fn a_route_keeps_to_the_nodes_its_hull_fits_on() {
        let g = grid(5, 3, 48.0);
        let pillar = (Vec3::new(76.0, -20.0, 0.0), Vec3::new(116.0, 20.0, 200.0));
        let world = test_world(&[pillar]);
        // Fit radius 0.75 x half: 7.5 stands beside the pillar on node 2's
        // neighbours; 30 meets it from nodes 1 and 3 too.
        let narrow = plan(&g, &world, 10.0, 0, 4);
        assert!(!narrow.contains(&2), "{narrow:?}");
        assert_eq!(narrow.len(), 7, "{narrow:?}");
        let wide = plan(&g, &world, 40.0, 0, 4);
        assert!(wide.iter().all(|&n| n >= 5 || n == 0 || n == 4), "{wide:?}");

        // A wall across x 96 on every row: the plan stops at the node of
        // the near side nearest the goal.
        let wall = (Vec3::new(86.0, -100.0, 0.0), Vec3::new(106.0, 200.0, 200.0));
        let world = test_world(&[wall]);
        let short = plan(&g, &world, 10.0, 0, 4);
        assert_eq!(short.last(), Some(&1), "{short:?}");
    }

    /// The plan takes plain edges within a step: a jump edge, a leap edge
    /// and one up to a node more than a step high are no road for a hull.
    #[test]
    fn a_route_takes_no_jump_leap_or_step_high_edge() {
        let world = test_world(&[]);
        let mut g = grid(4, 2, 64.0);
        // The direct row 0 -> 1 -> 2 -> 3 and its detour by row 1.
        assert_eq!(plan(&g, &world, 10.0, 0, 3), [0, 1, 2, 3]);
        g.edge_mut(1, 2).unwrap().jump = true;
        let p = plan(&g, &world, 10.0, 0, 3);
        assert!(!p.windows(2).any(|w| w == [1, 2]), "{p:?}");
        g.edges.iter_mut().flatten().for_each(|k| k.jump = false);
        g.edge_mut(1, 2).unwrap().leap = true;
        let p = plan(&g, &world, 10.0, 0, 3);
        assert!(!p.windows(2).any(|w| w == [1, 2]), "{p:?}");
        g.edges.iter_mut().flatten().for_each(|k| k.leap = false);
        g.nodes[2].z = vcod_common::pmove::STEPSIZE + 1.0;
        let p = plan(&g, &world, 10.0, 0, 3);
        assert!(!p.contains(&2), "{p:?}");
        g.nodes[2].z = vcod_common::pmove::STEPSIZE;
        assert_eq!(plan(&g, &world, 10.0, 0, 3), [0, 1, 2, 3]);
    }

    /// A link change drops only the fits a toggled submodel's bounds (2
    /// wider) overlap; a model the bounds do not name drops them all, as
    /// does a different model count.
    #[test]
    fn a_relink_drops_the_fits_a_toggled_model_can_change() {
        let door = ([200.0, -8.0, 0.0], [216.0, 8.0, 96.0]);
        let world = submodel_test_world("", &[door]);
        let bounds = vec![([-1024.0, -1024.0, -16.0], [1024.0, 1024.0, 0.0]), door];
        let mut fits = HullFits::default();
        let key = |x: f32| ([x, 0.0, 0.0].map(f32::to_bits), 15u32);
        let fill = |fits: &mut HullFits| {
            for x in [0.0f32, 190.0, 230.0, 260.0] {
                fits.fit.insert(key(x), true);
            }
        };
        // The first call is a new map: nothing kept.
        fill(&mut fits);
        fits.relink(&world, &bounds);
        assert!(fits.fit.is_empty());
        fill(&mut fits);
        fits.relink(&world, &bounds);
        assert_eq!(fits.fit.len(), 4, "nothing toggled");

        // The door unlinked: a radius 15 box on x 190 or 230 reaches it.
        world.set_model_linked(1, false);
        fits.relink(&world, &bounds);
        let mut left: Vec<f32> = fits.fit.keys().map(|k| f32::from_bits(k.0[0])).collect();
        left.sort_by(f32::total_cmp);
        assert_eq!(left, [0.0, 260.0]);

        // Toggled with no bounds for it: every fit goes.
        world.set_model_linked(1, true);
        fits.relink(&world, &bounds[..1]);
        assert!(fits.fit.is_empty());
    }

    fn pose(x: f32, y: f32, yaw: f32) -> crate::game::vehicle::Pose {
        crate::game::vehicle::Pose {
            origin: [x, y, 0.0],
            angles: [0.0, yaw, 0.0],
        }
    }

    fn hull(id: u32, pose: crate::game::vehicle::Pose, dead: bool) -> LiveHull {
        LiveHull {
            id: vcod_gsc::EntId(id, 0),
            pose,
            dead,
            linked: true,
        }
    }

    /// A hull covers the nodes its collmap blocks once it has held its
    /// pose a second, keeps them through a shove of less than 48 and lets
    /// them go past it or when it leaves the level.
    #[test]
    fn a_parked_hull_blocks_its_nodes_until_it_moves_off() {
        let g = grid(9, 3, 32.0);
        // The collmap, as the world clips it, over nodes x 96..160 of row 0.
        let body = (Vec3::new(90.0, -10.0, 0.0), Vec3::new(166.0, 10.0, 60.0));
        let world = test_world(&[body]);
        let mut parked = ParkedVehicles::default();
        let tick = |p: &mut ParkedVehicles, live: &[LiveHull]| p.update(&g, 1, &world, live);
        let at = pose(128.0, 0.0, 0.0);
        for _ in 0..PARK_TICKS - 1 {
            tick(&mut parked, &[hull(5, at, false)]);
        }
        assert!(parked.blocked.is_empty(), "blocked before a second");
        tick(&mut parked, &[hull(5, at, false)]);
        let covered: Vec<u32> = (0..g.nodes.len() as u32)
            .filter(|&n| {
                let p = g.nodes[n as usize];
                p.y == 0.0 && (96.0..=160.0).contains(&p.x)
            })
            .collect();
        assert_eq!(*parked.blocked, covered[..]);

        // Shoved 30: still its nodes, even once it parks again there.
        let shoved = pose(158.0, 0.0, 0.0);
        tick(&mut parked, &[hull(5, shoved, false)]);
        assert_eq!(*parked.blocked, covered[..]);
        // Driven 100 off: let go.
        tick(&mut parked, &[hull(5, pose(228.0, 0.0, 0.0), false)]);
        assert!(parked.blocked.is_empty());
        // Parked again (the first tick back is the move), then gone from
        // the level.
        for _ in 0..=PARK_TICKS {
            tick(&mut parked, &[hull(5, at, false)]);
        }
        assert!(!parked.blocked.is_empty());
        tick(&mut parked, &[]);
        assert!(parked.blocked.is_empty() && parked.each.is_empty());
    }

    /// A burning hull's nodes: all of them for a bot outside its reach,
    /// for one inside only those [`WRECK_DEEPER`] nearer it than he is, and
    /// none once the hull lives again under its handle.
    #[test]
    fn a_wreck_is_planned_round_by_where_the_bot_stands() {
        let g = grid(30, 1, 32.0);
        let world = test_world(&[]);
        let mut parked = ParkedVehicles::default();
        let wreck = pose(0.0, 0.0, 0.0);
        parked.update(&g, 1, &world, &[hull(7, wreck, true)]);
        assert_eq!(parked.wrecks(), [[0.0, 0.0, 0.0]]);
        let reach = crate::bots::WRECK_REACH;
        let all: Vec<u32> = (0..g.nodes.len() as u32)
            .filter(|&n| g.nodes[n as usize].x < reach)
            .collect();
        assert!(parked.blocked_at(None).is_empty(), "no parked hull");
        assert_eq!(*parked.blocked_at(Some([reach + 10.0, 0.0, 0.0])), all[..]);
        // 200 off: the nodes nearer the hull than 152.
        let near: Vec<u32> = all
            .iter()
            .copied()
            .filter(|&n| g.nodes[n as usize].x < 200.0 - WRECK_DEEPER)
            .collect();
        assert_eq!(*parked.blocked_at(Some([200.0, 0.0, 0.0])), near[..]);
        // Under it in height reach and over it: out of reach above.
        assert_eq!(*parked.blocked_at(Some([200.0, 0.0, reach + 1.0])), all[..]);
        // The level restarted under the same handle: no wreck.
        parked.update(&g, 1, &world, &[hull(7, wreck, false)]);
        assert!(parked.wrecks().is_empty());
        assert!(parked.blocked_at(Some([200.0, 0.0, 0.0])).is_empty());
    }

    fn tank_at(origin: [f32; 3], yaw: f32) -> VehicleSpot {
        VehicleSpot {
            num: 1,
            origin,
            yaw,
            mins: [-130.0, -70.0, 0.0],
            maxs: [120.0, 70.0, 90.0],
            tank: true,
        }
    }

    /// The driver's crowd strips: a teammate ahead within the reach (more
    /// with speed), behind, within a corner's turn; never himself, an
    /// enemy, one far above, or anyone in a team-less game.
    #[test]
    fn the_crowd_reads_teammates_ahead_behind_and_beside() {
        let (axis, allies) = (script::TEAM_AXIS, script::TEAM_ALLIES);
        // Turned to face +y: ahead is +y.
        let t = tank_at([0.0, 0.0, 0.0], 90.0);
        let c =
            |p: [f32; 3], team: i32, speed: f32| crowd_round(&t, speed, 3, axis, &[(5, p, team)]);
        let ahead = c([0.0, 200.0, 0.0], axis, 0.0);
        assert!(ahead.ahead && !ahead.behind && !ahead.beside, "{ahead:?}");
        // Past 120 + 120 at rest, inside it at 300 units/s.
        assert!(!c([0.0, 280.0, 0.0], axis, 0.0).ahead);
        assert!(c([0.0, 280.0, 0.0], axis, 300.0).ahead);
        let behind = c([0.0, -200.0, 0.0], axis, 0.0);
        assert!(behind.behind && !behind.ahead, "{behind:?}");
        // Off to the side: past the strip's 40 margin, inside the corner's.
        let side = c([150.0, 0.0, 0.0], axis, 0.0);
        assert!(side.beside && !side.ahead && !side.behind, "{side:?}");
        assert_eq!(c([0.0, 200.0, 0.0], allies, 0.0), Crowd::default());
        assert_eq!(c([0.0, 200.0, 200.0], axis, 0.0), Crowd::default());
        let me = crowd_round(&t, 0.0, 3, axis, &[(3, [0.0, 200.0, 0.0], axis)]);
        assert_eq!(me, Crowd::default());
        let dm = crowd_round(&t, 0.0, 3, 0, &[(5, [0.0, 200.0, 0.0], 0)]);
        assert_eq!(dm, Crowd::default());
    }

    /// A shot line toward the hull's middle meets it clear through, or
    /// stopped within 24 of its box and its height; short of that it met
    /// the world.
    #[test]
    fn a_shot_line_meets_the_hull_at_its_box() {
        let t = tank_at([0.0, 0.0, 0.0], 0.0);
        assert!(hull_met(&t, 1.0, [0.0, 0.0, 45.0]));
        assert!(hull_met(&t, 0.5, [140.0, 0.0, 45.0]));
        assert!(!hull_met(&t, 0.5, [150.0, 0.0, 45.0]));
        assert!(hull_met(&t, 0.5, [0.0, 90.0, 45.0]));
        assert!(!hull_met(&t, 0.5, [0.0, 0.0, 120.0]));
        assert!(!hull_met(&t, 0.5, [0.0, 0.0, -30.0]));
    }
}
