//! `--probe-triggers`: steers at each of the map's trigger brushes in turn.

use super::hit::{TARGET_USE_AFTER_DEATH, TARGET_USE_RETRY};
use super::pvs::{PVS_STUCK_UNITS, PVS_STUCK_WINDOW, entity_origin, offset_by};
use super::*;

/// A trigger brush the walk aims at, in world space: the submodel's own box
/// out of lump 27, moved by the entity's `origin` key.
pub(super) struct TriggerBox {
    /// The `*N` the entity's `model` key named, for the log line.
    pub(super) model: usize,
    pub(super) mins: [f32; 3],
    pub(super) maxs: [f32; 3],
}

impl TriggerBox {
    pub(super) fn centre(&self) -> [f32; 3] {
        [
            (self.mins[0] + self.maxs[0]) * 0.5,
            (self.mins[1] + self.maxs[1]) * 0.5,
            (self.mins[2] + self.maxs[2]) * 0.5,
        ]
    }

    /// Only the horizontal box: a map's trigger brushes span 1400 units of z
    /// so that a player standing on any floor under them touches, and the
    /// walker's own z is whatever ground it found.
    pub(super) fn holds(&self, p: [f32; 3]) -> bool {
        (0..2).all(|i| p[i] >= self.mins[i] && p[i] <= self.maxs[i])
    }

    /// Horizontal distance to the centre, for the same reason `holds` ignores
    /// z: the centre's z is the midpoint of a brush that spans the map's
    /// whole height, so a 3D distance to it never falls below ~560 units and
    /// the near test can never pass.
    fn away_from(&self, p: [f32; 3]) -> f32 {
        let c = self.centre();
        ((c[0] - p[0]).powi(2) + (c[1] - p[1]).powi(2)).sqrt()
    }
}

/// How close to a target box the walk counts as having reached it when it
/// never gets inside: a brush behind a wall is not worth the rest of the run.
const TRIGGER_NEAR: f32 = 200.0;

/// A leg's budget is its distance at this speed plus [`TRIGGER_LEG_SLACK`].
/// Well under retail's ~190 u/s run: the walk detours around geometry and a
/// budget set at the straight-line speed gives up on every target that is
/// not in the open. A flat budget does not work either: one long enough for
/// a far target spends the run on a near one behind a wall.
const TRIGGER_LEG_SPEED: f32 = 70.0;

const TRIGGER_LEG_SLACK: Duration = Duration::from_secs(6);

/// A leg that has turned through half a circle looking for a way in has
/// tried every heading the stall rule offers; the target is walled off. This,
/// not the clock, is what ends most legs on a stock map: several of a belt's
/// brushes sit behind solid geometry and cannot be walked into at all.
const TRIGGER_GIVE_UP_DEG: i32 = 180;

/// How far ahead the steer traces, and how far off the bearing it will look
/// for clear ground. 15-degree steps out to 165 covers everything short of
/// walking back the way it came.
const TRIGGER_PROBE_AHEAD: f32 = 320.0;

const TRIGGER_STEER_ARC: i32 = 165;

/// The trace's floor, lifted by retail's step height so a kerb is not a wall
/// (the movement constants table in docs/research/cod11-mantle.md).
const TRIGGER_STEP_HEIGHT: f32 = 18.0;

/// A walker that has not left this box in [`TRIGGER_WEDGE`] is wedged
/// somewhere the steer cannot get it out of (a pit, a doorway it rebounds
/// in) and kills itself to respawn somewhere else.
const TRIGGER_WEDGE_UNITS: f32 = 80.0;

const TRIGGER_WEDGE: Duration = Duration::from_secs(12);

/// `--probe-triggers`: steers at each of the map's trigger brushes in turn,
/// nearest unvisited first, and presses use after every death so a minefield
/// does not end the run.
///
/// A wander does not do: mp_pavlov is dense enough that a blind heading spends
/// a whole run inside the town. The walk is not the measurement (the retail server's own
/// log is), so steering it at the answer costs nothing and buys the coverage.
#[derive(Default)]
pub(super) struct TriggerProbe {
    boxes: Vec<TriggerBox>,
    /// The map's collision, for the steer's look-ahead trace. `None` on a
    /// host with no paks, where the walk turns blind.
    world: Option<Box<vcod_common::collision::CollisionWorld>>,
    /// Which boxes the walk is done with, whether it got inside or gave up.
    visited: Vec<bool>,
    /// The box being walked at, when that leg started, and how long it gets.
    target: Option<usize>,
    started: Option<Instant>,
    budget: Duration,
    /// The bearing the last observed snapshot put the target at, in shorts.
    yaw: Option<i32>,
    /// Added to the bearing once a walk stalls against geometry, cleared with
    /// the target: the next one is in a different direction.
    detour_deg: i32,
    last_progress: Option<(Instant, [f32; 3])>,
    press_use: bool,
    died_at: Option<Instant>,
    used_at: Option<Instant>,
    deaths: u32,
    /// Boxes the walk actually stood inside, which is the coverage a capture
    /// can hope to have.
    entered: usize,
    /// Reliable commands the loop sends once its snapshot borrow is done.
    pub(super) pending: Vec<String>,
    /// When the walker was last somewhere else, for the wedge escape.
    moved_at: Option<(Instant, [f32; 3])>,
}

impl TriggerProbe {
    /// Reads the map's trigger brushes out of the BSP. Their bounds are the
    /// submodel's; a trigger the gametype deletes is still aimed at, and
    /// walking through where it was costs one leg.
    pub(super) fn load_map(&mut self, fs: Option<&vcod_common::pk3::Pk3Fs>, map: &str) {
        let Some(fs) = fs else {
            println!("TRIGGERS: no game data, the walk has nothing to steer at");
            return;
        };
        let Some(bsp) = vcod_common::mapworld::find(fs, map).ok() else {
            println!("TRIGGERS: cannot load {map}, the walk has nothing to steer at");
            return;
        };
        for block in bsp.entities.split('}') {
            let key = |k: &str| {
                block
                    .split_once(&format!("\"{k}\""))
                    .and_then(|(_, rest)| rest.split('"').nth(1))
            };
            if !key("classname").is_some_and(|c| c.starts_with("trigger_")) {
                continue;
            }
            let Some(model) = key("model")
                .and_then(|m| m.strip_prefix('*'))
                .and_then(|n| n.parse::<usize>().ok())
            else {
                continue;
            };
            let Some(m) = bsp.models.get(model) else {
                continue;
            };
            let at = entity_origin(key("origin"));
            self.boxes.push(TriggerBox {
                model,
                mins: offset_by(m.mins, at),
                maxs: offset_by(m.maxs, at),
            });
        }
        self.visited = vec![false; self.boxes.len()];
        let statics = vcod_common::props::static_models(fs, &bsp.entities);
        self.world = Some(Box::new(
            vcod_common::collision::CollisionWorld::with_statics(&bsp, &statics),
        ));
        println!("TRIGGERS: {} trigger brushes to walk", self.boxes.len());
    }

    pub(super) fn running(&self) -> bool {
        !self.boxes.is_empty() && self.visited.iter().any(|v| !v)
    }

    pub(super) fn cmd(&self) -> net::msg::UserCmd {
        net::msg::UserCmd {
            angles: [0, self.yaw.unwrap_or(0), 0],
            forward: if self.yaw.is_some() && self.died_at.is_none() {
                127
            } else {
                0
            },
            buttons: if self.press_use { BUTTON_USE } else { 0 },
            ..net::msg::NULL_USERCMD
        }
    }

    pub(super) fn status(&self, snap: &net::snapshot::Snapshot) -> String {
        let at = match self.target {
            Some(i) => format!("*{}", self.boxes[i].model),
            None => "-".to_string(),
        };
        format!(
            "triggers: target {at} {}/{} done, {} entered, health {}, {} deaths",
            self.visited.iter().filter(|v| **v).count(),
            self.boxes.len(),
            self.entered,
            snap.ps.health(),
            self.deaths,
        )
    }

    /// Every iteration, not only the ones a live playerstate reaches: a dead
    /// walker has to press use to get up, and the leg clock has to stop while
    /// it is down or a death spends the leg it fell in.
    pub(super) fn observe(&mut self, now: Instant, snap: &net::snapshot::Snapshot) {
        if self.boxes.is_empty() {
            return;
        }
        let p = wire();
        let me = snap.ps.origin(p);

        if snap.ps.field_i32(p, "pm_type") != PM_NORMAL {
            if self.died_at.is_none() {
                self.deaths += 1;
                println!(
                    "TRIGGERS: death {} at [{:.0},{:.0}]",
                    self.deaths, me[0], me[1]
                );
                self.died_at = Some(now);
                self.used_at = None;
            }
            self.press_use = self.use_press_due(now);
            return;
        }
        if let Some(dead) = self.died_at.take() {
            println!(
                "TRIGGERS: alive again after {} ms",
                now.duration_since(dead).as_millis()
            );
            // The respawn is somewhere else entirely; the nearest unvisited
            // box from there is not the one the walk was on.
            self.target = None;
            self.moved_at = None;
        }
        self.press_use = false;

        let (t, o) = *self.moved_at.get_or_insert((now, me));
        if dist(o, me) >= TRIGGER_WEDGE_UNITS {
            self.moved_at = Some((now, me));
        } else if now.duration_since(t) >= TRIGGER_WEDGE {
            println!(
                "TRIGGERS: wedged at [{:.0},{:.0},{:.0}], killing to respawn",
                me[0], me[1], me[2]
            );
            self.pending.push("kill".to_string());
            self.moved_at = Some((now, me));
            return;
        }

        let Some(idx) = self.target.or_else(|| self.pick(me, now)) else {
            return;
        };
        let elapsed = self
            .started
            .map_or(Duration::ZERO, |t| now.duration_since(t));
        if self.boxes[idx].holds(me) || self.boxes[idx].away_from(me) < TRIGGER_NEAR {
            if self.boxes[idx].holds(me) {
                self.entered += 1;
                println!(
                    "TRIGGERS: inside *{} at [{:.0},{:.0},{:.0}]",
                    self.boxes[idx].model, me[0], me[1], me[2]
                );
            } else {
                println!(
                    "TRIGGERS: within {TRIGGER_NEAR:.0}u of *{}",
                    self.boxes[idx].model
                );
            }
            self.finish(idx);
            return;
        }
        if elapsed >= self.budget || self.detour_deg >= TRIGGER_GIVE_UP_DEG {
            println!(
                "TRIGGERS: gave up on *{} after {}ms and {} deg of detour, at \
                 [{:.0},{:.0},{:.0}], {:.0}u short",
                self.boxes[idx].model,
                elapsed.as_millis(),
                self.detour_deg,
                me[0],
                me[1],
                me[2],
                self.boxes[idx].away_from(me),
            );
            self.finish(idx);
            return;
        }

        // A walk that has stopped covering ground for a whole window is
        // walled in whichever way the steer turns; that, not the clock, is
        // what ends most legs on a stock map.
        let (t0, o0) = *self.last_progress.get_or_insert((now, me));
        if now.duration_since(t0) >= PVS_STUCK_WINDOW {
            if dist(o0, me) < PVS_STUCK_UNITS {
                self.detour_deg += 45;
            }
            self.last_progress = Some((now, me));
        }
        let (bearing, _) = aim_at(me, self.boxes[idx].centre());
        self.yaw = Some(self.steer(me, bearing));
    }

    /// The heading closest to `bearing` with clear ground ahead of it. A
    /// blind turn-when-stuck is not enough on a stock map: a walker that
    /// meets a wall at an angle slides along it and never covers the window's
    /// worth of ground, so every leg dies against the first building. The
    /// trace is a player box lifted off the floor by the step height, so a
    /// kerb does not read as a wall.
    fn steer(&self, me: [f32; 3], bearing: i32) -> i32 {
        let Some(world) = self.world.as_deref() else {
            return (bearing + self.detour_deg * 65536 / 360) & 0xffff;
        };
        let mins = [-15.0, -15.0, TRIGGER_STEP_HEIGHT];
        let maxs = [15.0, 15.0, vcod_common::pmove::HEIGHT_STAND];
        let ahead = |yaw: i32| {
            let rad = yaw as f32 * std::f32::consts::TAU / 65536.0;
            let end = [
                me[0] + rad.cos() * TRIGGER_PROBE_AHEAD,
                me[1] + rad.sin() * TRIGGER_PROBE_AHEAD,
                me[2],
            ];
            world
                .box_trace(me.into(), end.into(), mins.into(), maxs.into())
                .fraction
        };
        if ahead(bearing) >= 1.0 {
            return bearing;
        }
        // Hysteresis, and it is what makes the steer work at all: without it
        // the scan below picks a different clear heading every frame as the
        // walker moves, and the walk oscillates on the spot instead of
        // getting round the building in front of it.
        if let Some(prev) = self.yaw
            && ahead(prev) >= 1.0
        {
            return prev;
        }
        let mut best = (0.0f32, bearing);
        for k in (15..=TRIGGER_STEER_ARC).step_by(15) {
            for sign in [1, -1] {
                let yaw = (bearing + sign * k * 65536 / 360) & 0xffff;
                let f = ahead(yaw);
                if f >= 1.0 {
                    return yaw;
                }
                if f > best.0 {
                    best = (f, yaw);
                }
            }
        }
        best.1
    }

    fn use_press_due(&mut self, now: Instant) -> bool {
        let Some(dead) = self.died_at else {
            return false;
        };
        let due = now.duration_since(dead) >= TARGET_USE_AFTER_DEATH;
        let again = self
            .used_at
            .is_none_or(|t| now.duration_since(t) >= TARGET_USE_RETRY);
        if due && again {
            self.used_at = Some(now);
            return true;
        }
        false
    }

    fn finish(&mut self, idx: usize) {
        self.visited[idx] = true;
        self.target = None;
        self.started = None;
        self.detour_deg = 0;
        self.last_progress = None;
        self.yaw = None;
    }

    /// The nearest box the walk has not finished with. Greedy rather than a
    /// fixed order: a route that crosses the map between neighbours spends
    /// the run walking, and every box entered is a measurement.
    fn pick(&mut self, me: [f32; 3], now: Instant) -> Option<usize> {
        let next = (0..self.boxes.len())
            .filter(|i| !self.visited[*i])
            .min_by(|a, b| {
                self.boxes[*a]
                    .away_from(me)
                    .total_cmp(&self.boxes[*b].away_from(me))
            })?;
        let away = self.boxes[next].away_from(me);
        println!(
            "TRIGGERS: heading for *{} at {away:.0}u",
            self.boxes[next].model
        );
        self.target = Some(next);
        self.started = Some(now);
        self.budget = TRIGGER_LEG_SLACK + Duration::from_secs_f32(away / TRIGGER_LEG_SPEED);
        self.detour_deg = 0;
        self.last_progress = Some((now, me));
        Some(next)
    }

    pub(super) fn report(&self) {
        println!(
            "\nTRIGGERS report: {} brushes, {} walked to, {} stood inside, {} deaths",
            self.boxes.len(),
            self.visited.iter().filter(|v| **v).count(),
            self.entered,
            self.deaths,
        );
    }
}
