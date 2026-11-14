//! Bots in United Offensive vehicles: walking up to one and boarding it
//! with the use key, driving it along the navigation graph's waypoints,
//! laying its guns, and getting out. Every input is a key a client sends
//! from a seat (docs/research/coduo-vehicles.md 1, 2): the use press
//! boards and exits, `forwardmove` and `rightmove` drive, the view lays the
//! seat's gun and `buttons` 0x1 fires it. The server half, what the seat
//! and the vehicles near the bot look like, is
//! `server/bot_vehicles.rs`. docs/research/bot-objectives.md section 10.

use super::{Bot, BotView, Goal, aim_angles, cmd_angles, yaw_diff};
use vcod_common::net::msg::{self, UserCmd};

/// A vehicle this near across is worth walking to.
pub(crate) const SEEK_RANGE: f32 = 900.0;
/// With an objective, a vehicle is boarded only this near, on the way to
/// one this far off.
const DETOUR_RANGE: f32 = 400.0;
const OBJECTIVE_FAR: f32 = 1500.0;
/// The use press goes in this near the hull, and the walk stops this near.
const PRESS_REACH: f32 = 90.0;
const STAND_OFF: f32 = 30.0;
/// Ticks a walk to one vehicle may take, and the presses at its side,
/// before the bot gives it up.
const SEEK_TICKS: u32 = 400;
const PRESS_TICKS: u32 = 40;
/// Ticks after a give-up before the bot looks for one again.
const REST_TICKS: u32 = 600;
/// Ticks after an exit: a ride that ended short of its goal is not taken
/// again at once from where it left the bot.
const RIDDEN_TICKS: u32 = 1200;
/// A ride ends after this long, so the bot gets back to the objective.
const RIDE_TICKS: u32 = 1800;
/// Ticks a seat with nothing to drive to, or a gunner's seat with nobody
/// driving, is held before the bot gets out.
const IDLE_TICKS: u32 = 100;
/// A leaving rider's use press lands on one frame in this many, its
/// seat's: no two seats of a vehicle exit on one frame.
const EXIT_FRAMES: i32 = 4;
/// A rider with nobody to shoot gets out of a vehicle nobody drives this
/// soon: the driver gave it up.
const UNDRIVEN_TICKS: u32 = 40;
/// A driven vehicle the open throttle took under `STALL_DIST` in
/// `STALL_TICKS` is stalling (30 units/s): the bot backs off for
/// `REVERSE_TICKS`, and past `STALLS` back-offs with no ground made it gets
/// out. Ground made over the window, not the speed: a hull bouncing off
/// what it is wedged on reads 40 units/s one tick and -30 the next.
const STALL_SPEED: f32 = 30.0;
const STALL_TICKS: u32 = 30;
const STALL_DIST: f32 = 45.0;
/// A tank turning in place stalls when the window turned it less than this,
/// degrees: wedged between walls, its hull shrugs a few degrees each way.
const STALL_TURN: f32 = 15.0;
/// The hull's unbroken contact (`SeatView::contact_ms`) past which the
/// driver backs off, and past which every rider gets out: seven seconds of
/// it destroy the vehicle, and its riders with it.
const BACK_OFF_CONTACT_MS: i32 = 2500;
const LEAVE_CONTACT_MS: i32 = 5000;
const REVERSE_TICKS: u32 = 30;
const STALLS: u32 = 4;
/// Ground a ride has to make for its back-offs to start counting afresh,
/// and the driving ticks it may take to make it before the bot gets out.
const PROGRESS: f32 = 300.0;
const PROGRESS_TICKS: u32 = 160;
/// A route's last point this near the hull is reached.
const PASSED: f32 = 48.0;
/// The steer holds off inside this much heading error; a tank turns in
/// place past `TURN_IN_PLACE` before it drives on.
const STEER_SLACK: f32 = 6.0;
const TURN_IN_PLACE: f32 = 50.0;
/// A driver waits this long at most for a teammate walking up to board.
const CREW_WAIT_TICKS: u32 = 200;
/// A seat's gun fires once laid this near the enemy in yaw, within range.
const LAID_DEG: f32 = 4.0;
/// A tank shelling a `bas` base whose gun has not lain on it this long
/// gets out: its line will not come.
const SHELL_WAIT_TICKS: u32 = 200;
pub(crate) const GUN_RANGE: f32 = 3000.0;
/// A driver held by a teammate in its way this long backs off, when the
/// way back is clear.
const BLOCKED_TICKS: u32 = 40;
/// A rocket on foot goes at a vehicle once the view rests this near the
/// lead, and from this far at least: past the launchers' 256-unit splash
/// with a margin (`BREACH_CLEAR`).
const ROCKET_LAID_DEG: f32 = 2.0;
pub(crate) const ROCKET_MIN: f32 = super::BREACH_CLEAR;
pub(crate) const ROCKET_RANGE: f32 = super::BREACH_RANGE;
/// How far past its own splash the muzzle's cone has to be clear
/// ([`ArmorView::muzzle_clear`]): the splash is measured to the shooter's
/// box, which reaches a half width (15) round the muzzle, and this is
/// twice that, rounded up.
pub(crate) const ROCKET_CLEAR_MARGIN: f32 = 32.0;

/// A usable vehicle near the bot, as the server sees it this tick.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct VehicleSpot {
    pub num: u32,
    pub origin: [f32; 3],
    /// Hull yaw, degrees.
    pub yaw: f32,
    /// The vehicle file's box, in the hull's frame.
    pub mins: [f32; 3],
    pub maxs: [f32; 3],
    pub tank: bool,
}

impl VehicleSpot {
    /// The box's centre in the world.
    pub fn centre(&self) -> [f32; 3] {
        let c: [f32; 3] = std::array::from_fn(|i| (self.mins[i] + self.maxs[i]) * 0.5);
        let (s, k) = self.yaw.to_radians().sin_cos();
        [
            self.origin[0] + c[0] * k - c[1] * s,
            self.origin[1] + c[0] * s + c[1] * k,
            self.origin[2] + c[2],
        ]
    }

    /// How far across `p` stands off the box, 0 inside it.
    pub fn gap(&self, p: [f32; 3]) -> f32 {
        let (s, k) = self.yaw.to_radians().sin_cos();
        let (dx, dy) = (p[0] - self.origin[0], p[1] - self.origin[1]);
        let local = [dx * k + dy * s, -dx * s + dy * k];
        let out = |i: usize| {
            (self.mins[i] - local[i])
                .max(local[i] - self.maxs[i])
                .max(0.0)
        };
        out(0).hypot(out(1))
    }
}

/// The seat the bot sits in.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SeatView {
    /// 1 drives, 2 is a gunner's, 3 up ride.
    pub seat: usize,
    pub tank: bool,
    /// The vehicle's origin, the hull yaw (degrees) and its forward speed,
    /// units/s.
    pub origin: [f32; 3],
    pub yaw: f32,
    pub speed: f32,
    /// The world yaw the seat's gun points along: a tank's turret for its
    /// driver, the MG for a gunner. `None` for a seat with no gun.
    pub gun_yaw: Option<f32>,
    /// Somebody sits in the driver's seat.
    pub driven: bool,
    /// Another bot is walking up to board it.
    pub boarding: bool,
    /// The seat's gun's round speed, units/s; 0 for a bullet gun.
    pub gun_speed: f32,
    /// A teammate on foot in the hull's way (driver's seat only).
    pub crowd: Crowd,
    /// How long the hull has been striking something without a second's
    /// break, ms; seven seconds of it destroy the vehicle
    /// (coduo-vehicles.md 12, "Self-damage").
    pub contact_ms: i32,
    /// `level.time`, ms: which frames this seat's exit press may land on.
    pub level_ms: i32,
    /// A tank driver's main gun, when it fires a shell.
    pub sight: Option<GunSight>,
    /// The turret turns; the SU-152's and the Elefant's gun points the
    /// hull's way, and the hull turns to the view for it.
    pub traverse: bool,
    /// The driver is capturing the other side's tank: the stock script
    /// holds the hull and turret 15 s first (`_tankdrive_gmi.gsc`
    /// `tank_capture_think`, `level.tank_capture_time`).
    pub capturing: bool,
}

/// A tank's main gun as it stands this tick (`vehicle::fire::main_gun_sight`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GunSight {
    /// Where a shell fired now leaves from, along and how fast (units/s).
    pub muzzle: [f32; 3],
    pub dir: [f32; 3],
    pub speed: f32,
    /// The chase camera the driver's view is cast from: the shell bends
    /// toward where that view meets the world (coduo-vehicles.md 2, 13).
    pub eye: [f32; 3],
}

/// Teammates on foot round a driven hull: a body the hull rolls into is
/// crushed (`MOD_CRUSH_TANK`, `MOD_CRUSH_JEEP`), whichever side it is on.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Crowd {
    /// In the strip the hull drives into, forward or reversing.
    pub ahead: bool,
    pub behind: bool,
    /// Within reach of a corner turning in place.
    pub beside: bool,
}

/// An enemy-crewed vehicle the bot has a clear line to, for a rocket on
/// foot or a tank's gun.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ArmorView {
    pub num: u32,
    /// The hull box's middle in the world, and the vehicle's velocity.
    pub centre: [f32; 3],
    pub velocity: [f32; 3],
    /// The riders' slots, as a mask: an enemy among them is the vehicle.
    pub crew: u64,
    /// The speed of the round the bot would fire at it, units/s.
    pub shot_speed: f32,
    /// A teammate stands across the line to `centre`.
    pub friend_in_line: bool,
    /// For a launcher on foot: nothing within its splash and
    /// [`ROCKET_CLEAR_MARGIN`] of the muzzle along the view or the hip
    /// cone round it (`Server::armor_view`). Set for a tank's gun.
    pub muzzle_clear: bool,
}

impl ArmorView {
    pub(crate) fn carries(&self, slot: usize) -> bool {
        slot < 64 && self.crew >> slot & 1 == 1
    }
}

/// Where to aim at a body at `at` moving at `vel` (its horizontal part)
/// from `from`, for a round at `speed` units/s: ahead by the round's
/// flight. A bullet (`speed` 0) aims at the body. Neither the view's lag
/// nor a shell's fall is led: neither scored better on mp_cassino, and
/// the fall's lift scored worse (bot-objectives.md 10.6).
pub(crate) fn lead(from: [f32; 3], at: [f32; 3], vel: [f32; 3], speed: f32) -> [f32; 3] {
    if speed <= 0.0 {
        return at;
    }
    let ahead = |p: [f32; 3]| {
        let t = super::dist_sq(from, p).sqrt() / speed;
        [at[0] + vel[0] * t, at[1] + vel[1] * t, at[2]]
    };
    // The flight to the led point, twice over, settles well inside a unit
    // for anything slower than the round.
    ahead(ahead(ahead(at)))
}

/// The bot's own vehicle state, carried across ticks.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct Rider {
    /// The vehicle walked to, and the ticks spent on it.
    target: Option<VehicleSpot>,
    seek_ticks: u32,
    press_ticks: u32,
    /// Ticks before another vehicle is looked for.
    rest: u32,
    /// Ticks in the seat, and the ticks with nothing to do there.
    seated: u32,
    idle: u32,
    /// A use press goes in on every other tick: the server boards and exits
    /// on the press's edge.
    pressed: bool,
    /// The seat is being left.
    leaving: bool,
    /// An enemy in the seat's gun range this tick.
    fighting: bool,
    /// The driver holds this tick for a teammate walking up to board.
    crew_wait: bool,
    /// Ticks shelling a `bas` base since the gun last lay on it.
    shell_wait: u32,
    /// Stall bookkeeping: ticks stalling, back-off ticks left and the way
    /// it steers, back-offs since the ride last made [`PROGRESS`], from
    /// where.
    stall: u32,
    /// Where the hull stood, and its yaw, as the stall window opened.
    stall_from: [f32; 3],
    stall_yaw: f32,
    /// Ticks a driver has been held by a teammate in its way.
    blocked: u32,
    reverse: u32,
    reverse_right: i8,
    /// The throttle a back-off holds: back, or ahead for a hull hemmed in
    /// behind.
    reverse_forward: i8,
    stalls: u32,
    progress_from: [f32; 3],
    /// Driving ticks (a rider's: ticks with nobody to shoot) since the
    /// ride last made [`PROGRESS`].
    unmade: u32,
    /// Boardings and exits, for the gates.
    pub(crate) boarded: u32,
    /// Rockets fired on foot at an enemy vehicle, for the gates; and the
    /// last tick's trigger, which a launcher wants released between shots.
    pub(crate) rockets: u32,
    rocket_tap: bool,
    /// This tick's fight is a vehicle: the launcher stays out.
    pub(crate) at_armor: bool,
}

impl Rider {
    /// The vehicle the bot walks to, for [`Bot::goal`].
    pub(crate) fn heading_to(&self) -> Option<[f32; 3]> {
        self.target.map(|t| t.origin)
    }
}

impl Bot {
    /// The entity number of the vehicle the bot walks to board.
    pub(crate) fn vehicle_target(&self) -> Option<u32> {
        self.rider.target.map(|t| t.num)
    }
}

impl Bot {
    /// Where the bot walks to board a vehicle, ahead of its objective.
    pub(crate) fn vehicle_goal(&self, view: &BotView) -> Option<Goal> {
        if view.seat.is_some() {
            return None;
        }
        self.rider.heading_to().map(Goal::To)
    }

    /// The tick's cmd in or around a vehicle; `None` to play on foot as
    /// usual.
    pub(crate) fn think_vehicle(&mut self, view: &BotView, cmd: UserCmd) -> Option<UserCmd> {
        self.rider.rest = self.rider.rest.saturating_sub(1);
        if let Some(seat) = view.seat {
            if self.rider.seated == 0 {
                self.rider.boarded += 1;
                self.rider.target = None;
                self.rider.progress_from = seat.origin;
            }
            self.rider.seated += 1;
            // Whatever the walk was doing starts afresh once out.
            self.disengage();
            self.restart_stall(view.origin);
            return Some(self.think_seat(view, seat, cmd));
        }
        if self.rider.seated > 0 {
            // Out, by choice or by the vehicle's death.
            self.rider = Rider {
                rest: RIDDEN_TICKS,
                boarded: self.rider.boarded,
                ..Rider::default()
            };
        }
        self.think_boarding(view, cmd)
    }

    /// A death drops the walk and the seat.
    pub(crate) fn vehicle_life_over(&mut self) {
        self.rider = Rider {
            boarded: self.rider.boarded,
            rockets: self.rider.rockets,
            ..Rider::default()
        };
    }

    /// Rockets fired on foot at enemy vehicles.
    pub fn rockets_at_armor(&self) -> u32 {
        self.rider.rockets
    }

    /// On foot with a loaded launcher and an enemy-crewed vehicle in its
    /// line, nearer than any other enemy (or carrying him): the launcher
    /// comes out, the feet stop, the view lays on the hull's lead and a
    /// rocket goes once it rests there. The view to hold, `None` with no
    /// such fight. `path_yaw` is the view the walk's keys in `cmd` were
    /// set for.
    pub(crate) fn think_armor(
        &mut self,
        view: &BotView,
        cmd: &mut UserCmd,
        path_yaw: f32,
    ) -> Option<[f32; 2]> {
        self.rider.at_armor = false;
        let rocket = view.rocket?;
        let a = view.armor?;
        if view.seat.is_some() || view.deployed {
            return None;
        }
        let d = super::dist_sq(view.origin, a.centre);
        if view
            .enemy
            .is_some_and(|e| !a.carries(e.slot) && super::dist_sq(view.origin, e.origin) < d)
        {
            return None;
        }
        self.rider.at_armor = true;
        if view.weapon != rocket && self.switch_to.is_none() {
            self.switch_to = Some(rocket);
        }
        let eye = [
            view.origin[0],
            view.origin[1],
            view.origin[2] + view.eye_height,
        ];
        let (pitch, yaw) = aim_angles(view, lead(eye, a.centre, a.velocity, a.shot_speed));
        let aim = self.turn_toward(view, [pitch, yaw]);
        // A wall, a prop or a hull in the cone by the muzzle takes the
        // rocket's splash back on the shooter: on mp_foy a bot fired at an
        // enemy vehicle 1300 off and died of its own rocket (seed 7,
        // bot-objectives.md 10.6). No round then, and the feet keep the
        // walk, which may carry the line clear.
        if a.muzzle_clear {
            cmd.forward = 0;
            cmd.right = 0;
        } else {
            (cmd.forward, cmd.right) = super::rekey(cmd.forward, cmd.right, path_yaw, aim[1]);
        }
        let off = yaw_diff(yaw, view.view[1])
            .abs()
            .max(yaw_diff(pitch, view.view[0]).abs());
        let fire = view.weapon == rocket
            && view.busy_ms == 0
            && view.clip > 0
            && !a.friend_in_line
            && a.muzzle_clear
            && !self.rider.rocket_tap
            && off < ROCKET_LAID_DEG;
        self.rider.rocket_tap = fire;
        if fire {
            cmd.buttons |= msg::BUTTON_ATTACK;
            self.rider.rockets += 1;
        }
        Some(aim)
    }

    /// In a seat with an enemy in its gun's range: a tank holds still
    /// for the shot on purpose.
    pub(crate) fn seated_fight(&self) -> bool {
        self.rider.seated > 0 && self.rider.fighting
    }

    /// In the driver's seat holding for a teammate on his way in: the hull
    /// stands still on purpose.
    pub(crate) fn waiting_for_crew(&self) -> bool {
        self.rider.seated > 0 && self.rider.crew_wait
    }

    /// Times the bot got into a seat.
    pub fn boardings(&self) -> u32 {
        self.rider.boarded
    }

    /// Picks a vehicle to walk to, walks there and presses use at its side.
    fn think_boarding(&mut self, view: &BotView, mut cmd: UserCmd) -> Option<UserCmd> {
        let free = view.enemy.is_none()
            && matches!(self.stage, super::Stage::Wander)
            && view.frag_near.is_none()
            && !self.objective_busy(view)
            && self.forced.is_none();
        // A vehicle is a ride to an objective far off, near the way; with
        // the objective close the bot walks on.
        let reach = match self.objective_goal(view) {
            None | Some(Goal::Roam) => SEEK_RANGE,
            Some(Goal::To(p)) if super::dist_sq(view.origin, p) > OBJECTIVE_FAR * OBJECTIVE_FAR => {
                DETOUR_RANGE
            }
            _ => 0.0,
        };
        // In `bas` a bot fetching a launcher, or walking to a stand to fire
        // one, takes a tank near instead: its gun breaches a base too.
        let breaching = matches!(view.uo, Some(super::UoView::Bas(_)))
            && matches!(
                self.objective_target(view),
                Some(super::ObjTarget::Pickup { .. } | super::ObjTarget::Breach { aim: None, .. })
            );
        let reach_of = |s: &VehicleSpot| {
            if breaching && s.tank {
                SEEK_RANGE
            } else {
                reach
            }
        };
        if !free || reach == 0.0 && !breaching {
            self.rider.target = None;
            return None;
        }
        // The pick is refreshed off this tick's list: a vehicle somebody
        // else took, or one driven off, drops out of it.
        let near = |s: &VehicleSpot| s.gap(view.origin) < reach_of(s);
        let target = match self.rider.target {
            Some(t) => view.vehicles.iter().find(|s| s.num == t.num).copied(),
            None if self.rider.rest == 0 => view
                .vehicles
                .iter()
                .filter(|s| near(s))
                .min_by(|a, b| a.gap(view.origin).total_cmp(&b.gap(view.origin)))
                .copied(),
            None => None,
        };
        let Some(t) = target else {
            self.rider.target = None;
            self.rider.seek_ticks = 0;
            self.rider.press_ticks = 0;
            return None;
        };
        self.rider.target = Some(t);
        self.rider.seek_ticks += 1;
        let gap = t.gap(view.origin);
        if gap > PRESS_REACH {
            if self.rider.seek_ticks > SEEK_TICKS {
                self.give_up_vehicle();
            }
            // The graph's waypoint takes it there (`Bot::goal`).
            return None;
        }
        self.rider.press_ticks += 1;
        if self.rider.press_ticks > PRESS_TICKS {
            self.give_up_vehicle();
            return None;
        }
        let (pitch, yaw) = aim_angles(view, t.centre());
        cmd.angles = cmd_angles(view, [pitch, yaw]);
        cmd.forward = if gap > STAND_OFF { 127 } else { 0 };
        self.rider.pressed = !self.rider.pressed;
        if self.rider.pressed {
            cmd.buttons |= msg::BUTTON_USE;
        }
        self.restart_stall(view.origin);
        Some(cmd)
    }

    fn give_up_vehicle(&mut self) {
        self.rider.target = None;
        self.rider.seek_ticks = 0;
        self.rider.press_ticks = 0;
        self.rider.rest = REST_TICKS;
    }

    /// One tick in a seat: drive, lay the gun, or get out.
    fn think_seat(&mut self, view: &BotView, seat: SeatView, mut cmd: UserCmd) -> UserCmd {
        // A capture under way: nothing the keys do counts until it ends.
        // Read as a stall, it got the driver out before the hull was his.
        if seat.capturing && !self.rider.leaving {
            let r = &mut self.rider;
            (r.idle, r.stall, r.unmade, r.shell_wait) = (0, 0, 0, 0);
            r.progress_from = seat.origin;
            (cmd.forward, cmd.right, cmd.up) = (0, 0, 0);
            cmd.angles = cmd_angles(view, [view.view[0], view.view[1]]);
            return cmd;
        }
        // The view stays where it is unless a gun or the walk-in turns it.
        let mut aim = [view.view[0], view.view[1]];
        let in_range = |p: [f32; 3]| super::dist_sq(seat.origin, p) < GUN_RANGE * GUN_RANGE;
        // A tank's driver lays its gun on an enemy vehicle with nobody
        // else to shoot.
        let armor = view
            .armor
            .filter(|a| seat.seat == 1 && seat.tank && in_range(a.centre));
        let enemy = view
            .enemy
            .filter(|e| in_range(e.origin))
            .map(|e| (e.origin, e.velocity))
            .or(armor.map(|a| (a.centre, a.velocity)));
        let friend_in_line = match view.enemy.filter(|e| in_range(e.origin)) {
            Some(_) => view.friend_in_line,
            None => armor.is_some_and(|a| a.friend_in_line),
        };
        // With nobody to shoot, a `bas` base the gun has a shot at.
        let shell = seat
            .sight
            .filter(|_| seat.seat == 1 && enemy.is_none())
            .and_then(|g| Some((g, bas_shot(view)?)));
        self.rider.shell_wait = match shell {
            Some((_, (_, true))) | None => 0,
            Some(_) => self.rider.shell_wait + 1,
        };
        self.rider.fighting = enemy.is_some() || shell.is_some();
        if seat.seat == 1 {
            self.drive(view, seat, &mut cmd);
        } else if enemy.is_some() || seat.driven && seat.speed.abs() >= STALL_SPEED {
            // A rider with nobody to shoot is idle while the vehicle
            // stands, whoever drives it.
            self.rider.idle = 0;
        } else {
            self.rider.idle += 1;
        }
        // A rider gives up a ride that makes no headway as its driver does:
        // a gunner who waited for the driver to leave first, then out the
        // undriven seat's wait, sat 10 s in a hull shuffling round one spot
        // and walked off it past the 12 s stall gate (seed 45).
        if seat.seat > 1 {
            let r = &mut self.rider;
            if super::dist_sq(seat.origin, r.progress_from) > PROGRESS * PROGRESS {
                r.progress_from = seat.origin;
                r.unmade = 0;
            }
            if enemy.is_none() {
                r.unmade += 1;
            }
        }
        let headless = seat.seat > 1 && self.rider.unmade > PROGRESS_TICKS;
        if let Some((at, vel)) = enemy {
            // A seat's gun leads a moving target by its round's flight; a
            // passenger's own weapon aims as on foot.
            let at = if seat.gun_yaw.is_some() {
                let eye = [
                    view.origin[0],
                    view.origin[1],
                    view.origin[2] + view.eye_height,
                ];
                lead(eye, at, vel, seat.gun_speed)
            } else {
                at
            };
            let (pitch, yaw) = aim_angles(view, at);
            aim = [pitch, yaw];
            // A four-wheel's driver has no gun (0x1 sounds the horn); a
            // passenger fires his own weapon, a tap at a time.
            let laid = match seat.gun_yaw {
                Some(g) => yaw_diff(yaw, g).abs() < LAID_DEG,
                None => seat.seat > 2 && yaw_diff(yaw, view.view[1]).abs() < LAID_DEG,
            };
            let gun = seat.gun_yaw.is_some() || seat.seat > 2;
            if self.shoot && gun && laid && !friend_in_line {
                let tap =
                    seat.gun_yaw.is_some() || view.automatic || self.rider.seated.is_multiple_of(2);
                if tap {
                    cmd.buttons |= msg::BUTTON_ATTACK;
                }
            }
        } else if let Some((g, (spot, laid))) = shell {
            // The view from the chase camera onto the base: the shell
            // bends toward where it meets the world (coduo-vehicles.md 13),
            // and goes when the server finds its line on the base.
            aim = angles_from(g.eye, spot);
            // A base is no player: the shell goes with shooting off, as
            // a breach rocket does.
            if laid {
                cmd.buttons |= msg::BUTTON_ATTACK;
            }
            // The SU-152's and the Elefant's hull turns to the view.
            if !seat.traverse {
                cmd.up = 127;
            }
        } else if seat.gun_yaw.is_some() && seat.seat == 1 {
            // A tank's turret looks the way it drives.
            aim = [0.0, seat.yaw];
        }
        cmd.angles = cmd_angles(view, aim);
        // A hull that has struck something for seconds on end, driven or
        // sliding, is about to destroy itself and everyone in it.
        let wedged = seat.contact_ms >= LEAVE_CONTACT_MS;
        let abandoned = seat.seat > 1 && !seat.driven && self.rider.idle > UNDRIVEN_TICKS;
        // A ride ends on its clock, but not while the gun breaches a base:
        // 35 shells take one, three seconds apart.
        let done = self.rider.seated > RIDE_TICKS && shell.is_none()
            || self.rider.shell_wait > SHELL_WAIT_TICKS
            || self.rider.idle > IDLE_TICKS
            || wedged
            || abandoned
            || headless;
        if done || self.rider.leaving {
            self.rider.leaving = true;
            cmd.forward = 0;
            cmd.right = 0;
            cmd.buttons &= !msg::BUTTON_ATTACK;
            // Each seat presses on its own frames, one in `EXIT_FRAMES`:
            // two riders out of one tank on one frame lose the second's
            // `deactivated` to stock `_tankdrive_gmi::deactivated`'s loop,
            // whose old HUD thread then reads the destroyed fire icons
            // (bot-objectives.md 10.6, seed 15).
            let frame = seat.level_ms / super::FRAME_MS;
            self.rider.pressed =
                frame.rem_euclid(EXIT_FRAMES) == (seat.seat as i32 - 1) % EXIT_FRAMES;
            if self.rider.pressed {
                cmd.buttons |= msg::BUTTON_USE;
            }
        }
        cmd
    }

    /// The driver's keys toward the waypoint: `rightmove` steers both
    /// kinds, a tank turns in place, a four-wheel only rolling
    /// (coduo-vehicles.md 2, 12).
    fn drive(&mut self, view: &BotView, seat: SeatView, cmd: &mut UserCmd) {
        let r = &mut self.rider;
        let to = view
            .waypoint
            .filter(|w| (w[0] - seat.origin[0]).hypot(w[1] - seat.origin[1]) > PASSED);
        // A fight holds a tank still for the gun; a four-wheel drives on.
        // Both wait for a teammate on his way in.
        let crew = seat.boarding && r.seated < CREW_WAIT_TICKS;
        r.crew_wait = crew;
        let hold = seat.tank && r.fighting || crew;
        let Some(to) = to.filter(|_| !hold) else {
            r.stall = 0;
            r.idle = if hold { 0 } else { r.idle + 1 };
            // Ground made counts from where a hold left the hull: from an
            // older spot, the way back across it after a fight read as
            // headway and a tank dithered 12 s round one place (seed 25).
            if hold {
                r.progress_from = seat.origin;
            }
            return;
        };
        r.idle = 0;
        if super::dist_sq(seat.origin, r.progress_from) > PROGRESS * PROGRESS {
            r.progress_from = seat.origin;
            r.stalls = 0;
            r.unmade = 0;
        }
        // A hull that backs, turns and creeps round one tight spot without
        // ever stalling outright is let go of too.
        r.unmade += 1;
        if r.unmade > PROGRESS_TICKS {
            r.leaving = true;
        }
        if r.reverse > 0 {
            r.reverse -= 1;
            // Nobody is run over: the back-off waits on him.
            let clear = if r.reverse_forward < 0 {
                !seat.crowd.behind
            } else {
                !seat.crowd.ahead
            };
            if clear {
                cmd.forward = r.reverse_forward;
                cmd.right = r.reverse_right;
            }
            return;
        }
        let want = (to[1] - seat.origin[1])
            .atan2(to[0] - seat.origin[0])
            .to_degrees();
        let off = yaw_diff(want, seat.yaw);
        // Positive `rightmove` turns the hull clockwise, the yaw down.
        cmd.right = if off > STEER_SLACK {
            -127
        } else if off < -STEER_SLACK {
            127
        } else {
            0
        };
        cmd.forward = if seat.tank && off.abs() > TURN_IN_PLACE {
            0
        } else {
            127
        };
        // A teammate in the way: the hull holds rather than run him over,
        // and backs off when he stays (a bot walking the same route).
        if cmd.forward > 0 && seat.crowd.ahead || cmd.forward == 0 && seat.crowd.beside {
            cmd.forward = 0;
            cmd.right = 0;
            r.stall = 0;
            r.blocked += 1;
            // Backed off straight, or rolled on when somebody stands behind
            // too: a turn in place held by a teammate at its back corner.
            let way = if !seat.crowd.behind {
                Some(-127)
            } else if !seat.crowd.ahead {
                Some(127)
            } else {
                None
            };
            if let Some(way) = way.filter(|_| r.blocked >= BLOCKED_TICKS) {
                r.blocked = 0;
                r.reverse = REVERSE_TICKS;
                r.reverse_right = 0;
                r.reverse_forward = way;
            }
            return;
        }
        // A teammate milling round the hull clears it for a tick at a time:
        // the hold's count runs down rather than starting over.
        r.blocked = r.blocked.saturating_sub(1);
        let keyed = cmd.forward != 0 || cmd.right != 0;
        if !keyed || r.stall == 0 {
            r.stall_from = seat.origin;
            r.stall_yaw = seat.yaw;
        }
        r.stall = if keyed { r.stall + 1 } else { 0 };
        let stalled = r.stall >= STALL_TICKS && {
            r.stall = 0;
            if cmd.forward != 0 {
                super::dist_sq(seat.origin, r.stall_from) < STALL_DIST * STALL_DIST
            } else {
                yaw_diff(seat.yaw, r.stall_yaw).abs() < STALL_TURN
            }
        };
        if stalled || seat.contact_ms >= BACK_OFF_CONTACT_MS {
            r.stall = 0;
            r.stalls += 1;
            r.reverse = REVERSE_TICKS;
            r.reverse_forward = -127;
            // Backing out steers the other way round the obstacle.
            r.reverse_right = if cmd.right == 0 { 127 } else { cmd.right };
            if r.stalls >= STALLS {
                r.leaving = true;
            }
        }
    }
}

/// The other side's standing `bas` base nearest the bot that its tank's
/// gun has a shot at: the aim point, and whether a shell fired now bursts
/// on it.
fn bas_shot(view: &BotView) -> Option<([f32; 3], bool)> {
    let Some(super::UoView::Bas(b)) = view.uo.as_ref() else {
        return None;
    };
    b.bases
        .iter()
        .filter(|x| !x.ours && !x.breached)
        .filter_map(|x| Some((x.shot?, x.laid)))
        .min_by(|a, b| {
            super::dist_sq(view.origin, a.0).total_cmp(&super::dist_sq(view.origin, b.0))
        })
}

/// The view (pitch, yaw) from `from` onto `at`.
pub(crate) fn angles_from(from: [f32; 3], at: [f32; 3]) -> [f32; 2] {
    let d: [f32; 3] = std::array::from_fn(|i| at[i] - from[i]);
    let pitch = -d[2].atan2(d[0].hypot(d[1]).max(1.0)).to_degrees();
    [pitch.clamp(-80.0, 80.0), d[1].atan2(d[0]).to_degrees()]
}
