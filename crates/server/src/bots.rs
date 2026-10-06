//! Server-side bots: synthetic clients in `Server::clients` whose brains
//! live here. A bot joins through the stock menus like any client (the
//! driver in `server.rs` feeds this module the reliable server commands and
//! sends back the `mr` replies), then sends real usercmds through pmove.
//!
//! The brain is pure: [`Bot::observe`] consumes server commands and returns
//! `mr` replies, [`Bot::think`] returns the tick's usercmd from a read-only
//! [`BotView`] the server fills. Everything wire- and VM-shaped stays in
//! `server.rs`.

use vcod_common::net::msg::{self, NULL_USERCMD, UserCmd};

/// Hip-shot range; beyond it a tap is noise anyway.
pub(crate) const SHOOT_RANGE: f32 = 1500.0;
/// A frag goes at a target this close and no further.
const GRENADE_RANGE: f32 = 600.0;
/// Ticks a dead bot waits before its first use press.
const RESPAWN_DELAY: u32 = 30;
/// Ticks between use-press retries, and the press length: two ticks down,
/// then 1 s of release, repeated until the script's poll catches one.
const RESPAWN_RETRY: u32 = 20;
/// The bot sends one cmd per tick; `sv_fps 20`.
const TICK_MS: i32 = 50;
/// Half the width of what a shot has to land in (chest or head), units; the
/// fire cone is the angle it subtends at the target's range.
const HIT_RADIUS: f32 = 10.0;
/// A bot moving faster than this aims with [`Skill::error_moving`].
const MOVING_SPEED: f32 = 20.0;

/// How well a bot fights. Ticks are 50 ms; the aim error is a miss distance
/// at the target, in units, so its angle shrinks with range on its own.
#[derive(Clone, Copy, Debug)]
pub struct Skill {
    /// Ticks from first sight of a target to the first turn toward it:
    /// `reaction_min + rand(reaction_spread)`.
    pub reaction_min: u32,
    pub reaction_spread: u32,
    /// Ticks out of sight after which a returning target counts as new.
    pub forget_ticks: u32,
    /// The most the view turns in one tick, degrees.
    pub turn_deg: f32,
    /// Aim error on acquisition; each tick on the target multiplies it by
    /// `error_decay`, down to `error_floor`.
    pub error_start: f32,
    pub error_floor: f32,
    pub error_decay: f32,
    /// The range at which the error doubles.
    pub error_range: f32,
    /// Error multiplier while the bot itself moves.
    pub error_moving: f32,
    /// Past this range a weapon with sights aims down them.
    pub ads_range: f32,
}

impl Default for Skill {
    fn default() -> Self {
        Skill {
            reaction_min: 5,
            reaction_spread: 6,
            forget_ticks: 20,
            turn_deg: 15.0,
            error_start: 48.0,
            error_floor: 4.0,
            error_decay: 0.88,
            error_range: 1000.0,
            error_moving: 1.75,
            ads_range: 400.0,
        }
    }
}

/// What the brain needs to know about its own body this tick. The server
/// fills it from the sim, the weapon table and the script's team table.
pub struct BotView {
    pub origin: [f32; 3],
    /// `ps.delta_angles`, so the bot's absolute aim lands right after a spawn.
    pub delta_angles: [i32; 3],
    /// `ps.viewangles`, degrees, wire convention (pitch positive down).
    pub view: [f32; 3],
    /// `ps.weapon`, a 1-based index into configstring 7.
    pub weapon: u8,
    pub weapons_held: u64,
    /// The held weapon's loaded magazine; -1 when the weapon file is unknown.
    pub clip: i16,
    /// The held weapon's `fireTime`, in ms; 0 when unknown.
    pub fire_time_ms: i32,
    /// `ps.weaponTime`: the machine is busy and a tap would be latched away.
    pub busy_ms: i32,
    /// The held weapon fires while the trigger is held (`semiAuto 0`).
    pub automatic: bool,
    /// The held weapon has sights (`aimDownSight`).
    pub has_ads: bool,
    /// The held weapon's sights are a scope (`adsOverlayShader`).
    pub sniper: bool,
    /// `ps.fWeaponPosFrac`: 1 once the sights are fully up.
    pub ads_frac: f32,
    /// Horizontal speed, units/s.
    pub speed: f32,
    pub dead: bool,
    /// `pm_type` 0, a spawned player rather than a spectator or camera.
    pub playing: bool,
    /// Nearest live enemy with a clear sightline, as the server traced it.
    pub enemy: Option<EnemyView>,
    /// A held grenade's configstring index, when one is in the kit.
    pub grenade: Option<u8>,
    /// The next point on the server's path toward [`Bot::goal`]; `None`
    /// while there is no path, and the bot wanders.
    pub waypoint: Option<[f32; 3]>,
    /// `linkTo` holds the body (a plant or defuse in progress); it cannot
    /// move, so it is not stuck.
    pub linked: bool,
    /// The S&D objectives, on an `sd` level only.
    pub sd: Option<SdView>,
}

/// Which side of the S&D objective the bot's team plays this map.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ObjRole {
    Attack,
    Defend,
}

/// The stock `sd.gsc` objectives as the server read them this frame
/// (docs/research/bot-objectives.md).
#[derive(Clone, Debug)]
pub struct SdView {
    pub role: ObjRole,
    /// The bombzones still standing, A before B; empty once a bomb is down.
    pub sites: Vec<SiteView>,
    /// The planted bomb, until it is defused or explodes.
    pub bomb: Option<BombView>,
}

#[derive(Clone, Copy, Debug)]
pub struct SiteView {
    /// The zone's absolute bounds.
    pub mins: [f32; 3],
    pub maxs: [f32; 3],
    /// A feet origin inside the bounds the navigation graph reaches.
    pub stand: [f32; 3],
}

#[derive(Clone, Copy, Debug)]
pub struct BombView {
    /// The defuse trigger's origin, which the script's 64-unit range reads.
    pub origin: [f32; 3],
    /// The point the view has to rest on for `isLookingAt`: the middle of
    /// the trigger's bounds.
    pub aim: [f32; 3],
}

/// Where a bot wants to go. The brain names it from its view; the server
/// plans the way there over the map's navigation graph (`crate::nav`) and
/// hands back the next [`BotView::waypoint`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Goal {
    /// Nowhere: dead, spectating or mid-throw.
    Hold,
    /// Anywhere far: the server picks a spot and keeps it until reached.
    Roam,
    /// A point, feet or chest high.
    To([f32; 3]),
}

#[derive(Clone, Copy)]
pub struct EnemyView {
    /// The enemy's client slot, so a switch of target reads as one.
    pub slot: usize,
    /// The chest, the point the bot aims at.
    pub origin: [f32; 3],
}

/// What a bot's body is doing, as the gates read it.
pub struct BotBody {
    pub origin: [f32; 3],
    pub playing: bool,
    pub dead: bool,
    pub health: i32,
    pub clip: i16,
}

pub struct Bot {
    pub name: String,
    shoot: bool,
    pub skill: Skill,
    /// The team the team menu is answered with.
    team: String,
    /// The menu retail last named in `v g_scriptMainMenu`; the `t` that
    /// follows opens it.
    main_menu: String,
    /// Menu indices already answered, so a reopened menu is not answered
    /// twice (the same bookkeeping `JoinProbe` keeps).
    answered: Vec<i32>,
    /// The tick's usercmd is stamped with the server's own clock; the driver
    /// stamps it, so the brain carries only the input state below.
    rng: u64,
    /// Wander heading, degrees, wire convention.
    heading: f32,
    /// Ticks left on the current heading.
    heading_ticks: u32,
    /// Where the bot was when it picked its heading; a heading that barely
    /// moves the origin is swapped for a new one.
    stall_origin: [f32; 3],
    stall_ticks: u32,
    /// Ticks left on a wander heading taken to get unstuck, waypoint or not.
    unstick_ticks: u32,
    /// Ticks until the trigger may go down again: a semi-auto's tap spacing,
    /// an automatic's pause between bursts.
    fire_cooldown: u32,
    /// Ticks an automatic's trigger stays held.
    burst_ticks: u32,
    target: Option<Target>,
    /// Engagement footwork: the strafe direction (`right`), the ticks left
    /// on it, and the ticks left on a crouch.
    strafe: i8,
    strafe_ticks: u32,
    crouch_ticks: u32,
    grenade_cooldown: u32,
    respawn_ticks: u32,
    stage: Stage,
    /// The weapon a grenade throw hands back to.
    rifle: u8,
    obj: Objective,
    /// The last reliable server command the bot consumed.
    pub(crate) last_seen_seq: i32,
    /// The client-command sequence the bot's own replies use.
    pub(crate) next_command_seq: i32,
}

/// The enemy the bot is engaging and how settled its aim on it is.
struct Target {
    slot: usize,
    /// Ticks before the bot turns toward it at all.
    react: u32,
    /// Aim error, units at the target.
    error: f32,
    /// The error's direction, (pitch, yaw), unit length; resampled every
    /// `wobble` ticks so the aim drifts rather than sitting off by a constant.
    dir: [f32; 2],
    wobble: u32,
    /// Ticks out of sight.
    lost: u32,
}

/// The S&D objective bookkeeping; one life is one round, so a death
/// starts it over (docs/research/bot-objectives.md).
#[derive(Default)]
struct Objective {
    /// Which site this life goes to: `pick % sites.len()`.
    pick: u32,
    /// Ticks the use key has been held at the objective; nonzero means the
    /// plant or defuse is in progress and nothing else may break it off.
    held: u32,
    /// Ticks the view has rested on the bomb.
    settled: u32,
    /// Ticks before another try after one that ran out.
    rest: u32,
    /// Set while the bot plays, so the end of a life redraws `pick` once.
    live: bool,
}

/// What the objective wants of the bot right now.
#[derive(Clone, Copy, Debug)]
enum ObjTarget {
    Plant(SiteView),
    /// Stand within [`GUARD_RADIUS`] of a point and fight from there, but
    /// no nearer than `inner`.
    Guard {
        at: [f32; 3],
        inner: f32,
    },
    Defuse(BombView),
}

/// `level.planttime` is 5 s (`sd.gsc` `bombzones`); a hold this long that
/// planted nothing has failed.
const PLANT_TICKS: u32 = 120;
/// `level.defusetime` is 10 s.
const DEFUSE_TICKS: u32 = 210;
/// The defuse wants `distance(origin, trigger.origin) < 64`.
const DEFUSE_REACH: f32 = 48.0;
const GUARD_RADIUS: f32 = 250.0;
/// An attacker steps off the bomb it planted: a body on the line from a
/// defender's eye to the trigger blocks the defuse's `isLookingAt`, which
/// is the defender's problem, not the bot's to make.
const BOMB_GUARD_INNER: f32 = 96.0;
/// Use held this long with no link means the plant or defuse never
/// started (not touching, not on the ground, the aim off the trigger).
const START_TICKS: u32 = 20;
/// Release after a failed try, so the next press is a fresh one.
const RETRY_TICKS: u32 = 10;
/// The aim trace is read a frame late (object-model doc 23.1): the view
/// rests on the bomb this many ticks before the press.
const SETTLE_TICKS: u32 = 2;
const SETTLE_DEG: f32 = 1.0;
/// Inset from a bombzone's edges before the bot counts as inside it.
const SITE_MARGIN: f32 = 8.0;
/// The standing player's box height, for the zone's z overlap.
const PLAYER_HEIGHT: f32 = 70.0;

enum Stage {
    Wander,
    ToGrenade,
    Cook {
        left: u32,
    },
    /// The throw is out; hold the rifle byte until `ps.weapon` follows.
    BackToRifle {
        weapon: u8,
    },
}

impl Bot {
    pub fn new(team: &str, shoot: bool, seed: u64) -> Self {
        Bot {
            name: String::new(),
            shoot,
            skill: Skill::default(),
            main_menu: String::new(),
            answered: Vec::new(),
            heading: 0.0,
            heading_ticks: 0,
            stall_origin: [0.0; 3],
            stall_ticks: 0,
            unstick_ticks: 0,
            fire_cooldown: 0,
            burst_ticks: 0,
            target: None,
            strafe: 0,
            strafe_ticks: 0,
            crouch_ticks: 0,
            grenade_cooldown: 200,
            respawn_ticks: 0,
            stage: Stage::Wander,
            rifle: 0,
            // Off the seed rather than the generator, so the stream every
            // other draw reads is the one it was before objectives.
            obj: Objective {
                pick: (seed >> 32) as u32,
                ..Objective::default()
            },
            last_seen_seq: 0,
            next_command_seq: 1,
            team: team.to_string(),
            rng: seed | 1,
        }
    }

    /// xorshift64* masked to 31 bits, the server's own generator.
    fn rand(&mut self) -> i32 {
        crate::game::host::rand_int(&mut self.rng)
    }

    /// The reliable server commands the bot received this tick, as
    /// `(seq, text)`; returns the `mr` replies to send, in order. `server_id`
    /// is what the reply must name; `allowed` answers whether a `scr_allow_*`
    /// cvar lets its weapon through.
    pub(crate) fn observe(
        &mut self,
        server_id: i32,
        cmds: &[(i32, &str)],
        allowed: &dyn Fn(&str) -> bool,
    ) -> Vec<String> {
        let mut replies = Vec::new();
        for (_, text) in cmds {
            let tokens: Vec<&str> = text.split_whitespace().collect();
            match tokens.as_slice() {
                ["v", "g_scriptMainMenu", menu] => {
                    // `set_client_cvar` quotes the value (hud protocol doc 0.1).
                    self.main_menu = menu.trim_matches('"').to_string();
                }
                // A restart reruns `ClientConnect` without a gamestate and
                // reopens the menus under the indices the last level used
                // (the `n` is what the probes forget them on too).
                ["n"] => self.rearm(),
                ["t", idx] => {
                    let Ok(idx) = idx.parse::<i32>() else {
                        continue;
                    };
                    if self.answered.contains(&idx) {
                        continue;
                    }
                    let Some(reply) = self.menu_reply(allowed) else {
                        continue;
                    };
                    self.answered.push(idx);
                    replies.push(format!("mr {server_id} {idx} {reply}"));
                }
                _ => {}
            }
        }
        replies
    }

    /// The team menu takes the bot's team; the weapon menu takes a weapon
    /// its nationality offers and the cvars allow, picked at random.
    fn menu_reply(&mut self, allowed: &dyn Fn(&str) -> bool) -> Option<String> {
        if self.main_menu.starts_with("team_") {
            return Some(self.team.clone());
        }
        let menu = self.main_menu.strip_prefix("weapon_")?;
        let open: Vec<&str> = WEAPON_MENUS
            .iter()
            .find(|(n, _)| *n == menu)?
            .1
            .iter()
            .filter(|(_, cvar)| allowed(cvar))
            .map(|(w, _)| *w)
            .collect();
        if open.is_empty() {
            return None;
        }
        let pick = self.rand() as usize % open.len();
        Some(open[pick].to_string())
    }

    /// A new gamestate reruns `ClientConnect` and reopens the menus under the
    /// indices the last one used; without the clear the bot never spawns
    /// again (the same rejoin the probes learned).
    pub fn rearm(&mut self) {
        self.main_menu.clear();
        self.answered.clear();
    }

    /// Where the bot wants to be this tick: the enemy it sees, else
    /// anywhere far. Asked before [`Bot::think`], which then follows the
    /// waypoint the server planned toward it.
    pub fn goal(&self, view: &BotView) -> Goal {
        if view.dead || !view.playing || !matches!(self.stage, Stage::Wander) {
            return Goal::Hold;
        }
        // A plant or defuse aborts on release, so it outranks a fight.
        if self.objective_busy(view) {
            return Goal::Hold;
        }
        if let Some(e) = view.enemy {
            return Goal::To(e.origin);
        }
        if let Some(g) = self.objective_goal(view) {
            return g;
        }
        Goal::Roam
    }

    /// A plant or defuse is under way: use is held, or the script holds
    /// the body.
    fn objective_busy(&self, view: &BotView) -> bool {
        self.obj.held > 0 || view.linked
    }

    /// The objective this tick, from the bot's role and the round's state.
    fn objective_target(&self, view: &BotView) -> Option<ObjTarget> {
        let sd = view.sd.as_ref()?;
        let site = || {
            let n = sd.sites.len();
            (n > 0).then(|| sd.sites[self.obj.pick as usize % n])
        };
        match (sd.role, sd.bomb) {
            (ObjRole::Attack, Some(b)) => Some(ObjTarget::Guard {
                at: b.origin,
                inner: BOMB_GUARD_INNER,
            }),
            (ObjRole::Attack, None) => site().map(ObjTarget::Plant),
            (ObjRole::Defend, Some(b)) => Some(ObjTarget::Defuse(b)),
            (ObjRole::Defend, None) => site().map(|s| ObjTarget::Guard {
                at: s.stand,
                inner: 0.0,
            }),
        }
    }

    fn at_objective(view: &BotView, t: &ObjTarget) -> bool {
        let o = view.origin;
        match t {
            ObjTarget::Plant(s) => {
                let inside = (0..2)
                    .all(|i| o[i] >= s.mins[i] + SITE_MARGIN && o[i] <= s.maxs[i] - SITE_MARGIN)
                    && o[2] <= s.maxs[2]
                    && o[2] + PLAYER_HEIGHT >= s.mins[2];
                // A zone narrower than the margin still has its stand.
                let flat = (o[0] - s.stand[0]).hypot(o[1] - s.stand[1]);
                inside || (flat < 16.0 && (o[2] - s.stand[2]).abs() < 32.0)
            }
            ObjTarget::Guard { at, inner } => {
                let d = dist_sq(o, *at);
                d < GUARD_RADIUS * GUARD_RADIUS && d >= inner * inner
            }
            ObjTarget::Defuse(b) => dist_sq(o, b.origin) < DEFUSE_REACH * DEFUSE_REACH,
        }
    }

    /// Where the objective sends a bot that is not there yet; `Hold` once
    /// it is, and `None` when the round has no objective for it.
    fn objective_goal(&self, view: &BotView) -> Option<Goal> {
        let t = self.objective_target(view)?;
        if Self::at_objective(view, &t) {
            return Some(Goal::Hold);
        }
        Some(match t {
            ObjTarget::Plant(s) => Goal::To(s.stand),
            // Too close: any way out will do, and the roam leaves the ring
            // well before it gets anywhere.
            ObjTarget::Guard { at, inner } if dist_sq(view.origin, at) < inner * inner => {
                Goal::Roam
            }
            ObjTarget::Guard { at, .. } => Goal::To(at),
            ObjTarget::Defuse(b) => Goal::To(b.origin),
        })
    }

    /// The bot stands at its objective without acting on it: it holds its
    /// ground instead of following a waypoint, and is not stuck.
    fn objective_standing(&self, view: &BotView) -> bool {
        view.linked
            || self
                .objective_target(view)
                .is_some_and(|t| Self::at_objective(view, &t))
    }

    /// The plant or defuse itself: the cmd for this tick while one is under
    /// way or starting, `None` to play on as usual.
    fn think_objective(&mut self, view: &BotView, cmd: UserCmd) -> Option<UserCmd> {
        self.obj.rest = self.obj.rest.saturating_sub(1);
        let t = self.objective_target(view);
        let at = t.is_some_and(|t| Self::at_objective(view, &t));
        let ready = self.obj.rest == 0 && (at || (view.linked && self.obj.held > 0));
        let cur = [yaw_diff(view.view[0], 0.0), view.view[1]];
        match t {
            Some(ObjTarget::Plant(_)) if ready => {
                if self.obj.held >= PLANT_TICKS || (self.obj.held >= START_TICKS && !view.linked) {
                    // Nothing planted: another zone's plant blocks this one,
                    // or the body never touched. Try the other site.
                    self.obj.held = 0;
                    self.obj.rest = RETRY_TICKS;
                    self.obj.pick = self.obj.pick.wrapping_add(1);
                    return None;
                }
                self.obj.held += 1;
                Some(self.hold_use(view, cmd, cur))
            }
            Some(ObjTarget::Defuse(b)) if ready => {
                if self.obj.held >= DEFUSE_TICKS || (self.obj.held >= START_TICKS && !view.linked) {
                    // The aim trace never reached the trigger (a body in
                    // the way); release and settle again.
                    self.obj.held = 0;
                    self.obj.settled = 0;
                    self.obj.rest = RETRY_TICKS;
                    return None;
                }
                let (tp, ty) = aim_angles(view, b.aim);
                let aim = turn(cur, [tp, ty], self.skill.turn_deg);
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
                if self.obj.settled >= SETTLE_TICKS {
                    self.obj.held = 1;
                } else {
                    cmd.buttons = 0;
                }
                Some(cmd)
            }
            _ => {
                self.obj.held = 0;
                self.obj.settled = 0;
                None
            }
        }
    }

    /// Use down, no move keys, the view at `aim`. Nothing else runs: the
    /// stall watch restarts from here and the fight is dropped.
    fn hold_use(&mut self, view: &BotView, mut cmd: UserCmd, aim: [f32; 2]) -> UserCmd {
        self.disengage();
        self.stall_origin = view.origin;
        self.stall_ticks = 0;
        cmd.buttons = msg::BUTTON_USE;
        cmd.angles = cmd_angles(view, aim);
        cmd
    }

    /// One life over: the next is a new round, with a new site to go to.
    fn end_life(&mut self) {
        if self.obj.live {
            self.obj = Objective {
                pick: self.rand() as u32,
                ..Objective::default()
            };
        }
    }

    /// The tick's usercmd. The driver stamps `server_time` and pushes the
    /// cmd into the client's queue.
    pub(crate) fn think(&mut self, view: &BotView) -> UserCmd {
        self.fire_cooldown = self.fire_cooldown.saturating_sub(1);
        let mut cmd = NULL_USERCMD;
        cmd.weapon = view.weapon;
        // Death and spectatorship outrank the stage machine: a corpse still
        // in a grenade phase must reach the respawn press below, not sit in
        // `think_cook` forever.
        if view.dead {
            self.respawn_ticks += 1;
            // Whatever the death interrupted, the new life starts clean: no
            // throw resumes on respawn.
            self.stage = Stage::Wander;
            self.disengage();
            self.end_life();
            // The stock death flow polls the use key only after its own
            // `wait 2` (dm.gsc, `waitRespawnButton`), so a one-shot press
            // lands before any poll reads it; retry every second, the way
            // the probe's target does.
            let since_death = self.respawn_ticks.saturating_sub(RESPAWN_DELAY);
            if self.respawn_ticks >= RESPAWN_DELAY && since_death % RESPAWN_RETRY < 2 {
                cmd.buttons = msg::BUTTON_USE;
            }
            return cmd;
        }
        if !view.playing {
            // Spectator or intermission camera: nothing to press.
            self.respawn_ticks = 0;
            self.disengage();
            self.end_life();
            return cmd;
        }
        self.respawn_ticks = 0;
        self.obj.live = true;
        match self.stage {
            Stage::ToGrenade => return self.think_grenade(view, cmd),
            Stage::Cook { left } => return self.think_cook(view, cmd, left),
            Stage::BackToRifle { weapon } => return self.think_back(view, cmd, weapon),
            Stage::Wander => {}
        }
        if let Some(cmd) = self.think_objective(view, cmd) {
            return cmd;
        }
        // A frag goes at a close enemy, occasionally, once the cooldown is
        // spent; the switch itself is the stage machine below.
        self.grenade_cooldown = self.grenade_cooldown.saturating_sub(1);
        if self.shoot
            && self.grenade_cooldown == 0
            && let (Some(g), Some(e)) = (view.grenade, view.enemy)
        {
            let close = dist_sq(view.origin, e.origin) < GRENADE_RANGE * GRENADE_RANGE;
            if close && self.rand() % 4 == 0 {
                self.grenade_cooldown = GRENADE_COOLDOWN_TICKS;
                self.rifle = view.weapon;
                self.stage = Stage::ToGrenade;
                cmd.weapon = g;
                return cmd;
            }
        }

        // A body that has not left a 15-unit circle in ten ticks is stuck,
        // path or not, and takes a random heading for a spell. One standing
        // at its objective, or held by a link, means to stay put.
        let standing = self.objective_standing(view);
        self.stall_ticks += 1;
        if standing {
            self.stall_origin = view.origin;
            self.stall_ticks = 0;
            self.unstick_ticks = 0;
        } else if self.stall_ticks >= 10 {
            if dist_sq(view.origin, self.stall_origin) < 15.0 * 15.0 {
                self.pick_heading(view);
                self.unstick_ticks = UNSTICK_TICKS;
            } else {
                self.stall_origin = view.origin;
                self.stall_ticks = 0;
            }
        }
        let (mut pitch, mut yaw, forward) = match view.waypoint {
            // On guard: look about, feet still.
            _ if standing => {
                if self.heading_ticks == 0 {
                    self.pick_heading(view);
                } else {
                    self.heading_ticks -= 1;
                }
                (0.0, self.heading, 0)
            }
            Some(w) if self.unstick_ticks == 0 => steer(view.origin, w),
            _ => {
                self.unstick_ticks = self.unstick_ticks.saturating_sub(1);
                if self.heading_ticks == 0 {
                    self.pick_heading(view);
                } else {
                    self.heading_ticks -= 1;
                }
                (0.0, self.heading, 127)
            }
        };
        // Engaging overrides all of this: `footwork` owns the move keys and
        // the aim owns the view.
        cmd.forward = forward;
        if self.shoot {
            self.track(view.enemy);
            if let Some(e) = view.enemy
                && let Some(aim) = self.engage(view, e, &mut cmd)
            {
                [pitch, yaw] = aim;
            }
        }
        // A dry clip reloads; the tap is suppressed while the machine is
        // busy so a held bit cannot restart the reload it started.
        if view.clip == 0 && view.busy_ms == 0 {
            cmd.wbuttons |= msg::WBUTTON_RELOAD;
        }
        cmd.angles = cmd_angles(view, [pitch, yaw]);
        cmd
    }

    /// Keeps [`Bot::target`] in step with the server's enemy: a new slot is
    /// a new target with a fresh reaction and full error; one out of sight
    /// for `forget_ticks` is dropped.
    fn track(&mut self, enemy: Option<EnemyView>) {
        let Some(e) = enemy else {
            if let Some(t) = self.target.as_mut() {
                t.lost += 1;
                if t.lost > self.skill.forget_ticks {
                    self.disengage();
                }
            }
            return;
        };
        if let Some(t) = self.target.as_mut().filter(|t| t.slot == e.slot) {
            t.lost = 0;
            return;
        }
        let react = self.skill.reaction_min + self.rand_below(self.skill.reaction_spread);
        self.burst_ticks = 0;
        self.target = Some(Target {
            slot: e.slot,
            react,
            error: self.skill.error_start,
            dir: [0.0, 1.0],
            wobble: 0,
            lost: 0,
        });
    }

    fn disengage(&mut self) {
        self.target = None;
        self.burst_ticks = 0;
        self.crouch_ticks = 0;
    }

    /// One tick against the tracked target: turn toward it at a bounded
    /// rate, sights, trigger and footwork into `cmd`. Returns the commanded
    /// (pitch, yaw), or `None` while the reaction delay still runs.
    fn engage(&mut self, view: &BotView, e: EnemyView, cmd: &mut UserCmd) -> Option<[f32; 2]> {
        let mut t = self.target.take()?;
        if t.react > 0 {
            t.react -= 1;
            self.target = Some(t);
            return None;
        }
        t.error = (t.error * self.skill.error_decay).max(self.skill.error_floor);
        if t.wobble == 0 {
            let a = (self.rand() % 360) as f32;
            t.dir = [a.to_radians().sin(), a.to_radians().cos()];
            t.wobble = 4 + self.rand_below(5);
        }
        t.wobble -= 1;
        let dist = dist_sq(view.origin, e.origin).sqrt().max(1.0);
        let moving = if view.speed > MOVING_SPEED {
            self.skill.error_moving
        } else {
            1.0
        };
        let miss = t.error * (1.0 + dist / self.skill.error_range) * moving;
        let err_deg = (miss / dist).atan().to_degrees();
        let dir = t.dir;
        self.target = Some(t);

        let (tp, ty) = aim_angles(view, e.origin);
        let want = [tp + dir[0] * err_deg, ty + dir[1] * err_deg];
        let cur = [yaw_diff(view.view[0], 0.0), view.view[1]];
        let aim = turn(cur, want, self.skill.turn_deg);
        // The view's miss of the true chest point after this cmd's turn.
        let off = yaw_diff(aim[0], tp).hypot(yaw_diff(aim[1], ty));
        let cone = (HIT_RADIUS / dist).atan().to_degrees();

        let ads = view.has_ads && (view.sniper || dist > self.skill.ads_range);
        if ads {
            cmd.buttons |= msg::BUTTON_ADS;
        }
        // A scope or sights still coming up would waste the shot on hip spread.
        let ready = !ads || view.ads_frac >= 1.0;
        self.trigger(view, dist, off, cone, ready, cmd);
        self.footwork(view, dist, ads, cmd);
        Some(aim)
    }

    /// Semi-autos tap, at least `fireTime` apart (AGENTS.md, "Fire is
    /// tapped"); automatics hold the bit for a burst, shorter at range.
    fn trigger(
        &mut self,
        view: &BotView,
        dist: f32,
        off: f32,
        cone: f32,
        ready: bool,
        cmd: &mut UserCmd,
    ) {
        if view.clip == 0 {
            self.burst_ticks = 0;
            return;
        }
        if view.automatic && self.burst_ticks > 0 {
            // A burst rides out recoil and wobble, but not a lost line.
            if off <= cone * 2.0 {
                cmd.buttons |= msg::BUTTON_ATTACK;
                self.burst_ticks -= 1;
            } else {
                self.burst_ticks = 0;
            }
            if self.burst_ticks == 0 {
                self.fire_cooldown = 3 + self.rand_below(4);
            }
            return;
        }
        if self.fire_cooldown > 0 || view.busy_ms != 0 || !ready || off > cone {
            return;
        }
        cmd.buttons |= msg::BUTTON_ATTACK;
        if view.automatic {
            let long = dist > 800.0;
            self.burst_ticks = if long { 1 } else { 2 } + self.rand_below(if long { 3 } else { 6 });
        } else {
            let fire = if view.fire_time_ms > 0 {
                view.fire_time_ms
            } else {
                200
            };
            // Rounded up, and never under two ticks: the bit has to come up
            // between taps for the next to read as a press.
            let ticks = ((fire + TICK_MS - 1) / TICK_MS).max(2) as u32;
            self.fire_cooldown = ticks + self.rand_below(3);
        }
    }

    /// Strafe in spells and crouch now and then while engaging; close in
    /// only past the weapon's working range. Scoped and sighted shots at
    /// range stand still, since moving widens the error.
    fn footwork(&mut self, view: &BotView, dist: f32, ads: bool, cmd: &mut UserCmd) {
        if self.strafe_ticks == 0 {
            self.strafe = if self.rand() % 2 == 0 { 127 } else { -127 };
            self.strafe_ticks = 8 + self.rand_below(16);
            if self.crouch_ticks == 0 && self.rand_below(if view.sniper { 2 } else { 4 }) == 0 {
                self.crouch_ticks = 20 + self.rand_below(30);
            }
        } else {
            self.strafe_ticks -= 1;
        }
        let range = if view.sniper {
            f32::INFINITY
        } else if view.automatic {
            350.0
        } else {
            700.0
        };
        cmd.forward = if dist > range { 127 } else { 0 };
        cmd.right = if ads { 0 } else { self.strafe };
        if self.crouch_ticks > 0 {
            self.crouch_ticks -= 1;
            // The stance is a held level; retail sends `upmove` -127 with it
            // (protocol doc, "Usercmd input bits").
            cmd.wbuttons |= msg::WBUTTON_CROUCH;
            cmd.up = -127;
        }
    }

    /// `0..n`, 0 when `n` is 0.
    fn rand_below(&mut self, n: u32) -> u32 {
        if n == 0 { 0 } else { self.rand() as u32 % n }
    }

    /// A fresh wander heading, and a new stall baseline to measure it by.
    fn pick_heading(&mut self, view: &BotView) {
        self.heading = (self.rand() % 360) as f32;
        self.heading_ticks = 40 + (self.rand() % 40) as u32;
        self.stall_origin = view.origin;
        self.stall_ticks = 0;
    }

    fn think_grenade(&mut self, view: &BotView, mut cmd: UserCmd) -> UserCmd {
        if let Some(g) = view.grenade {
            cmd.weapon = g;
            if view.weapon == g {
                // No cook exists in 1.1 (combat doc 1.11): the release throws,
                // so the hold is only long enough to look like a throw, not a
                // cook.
                let left = 6 + (self.rand() % 8) as u32;
                self.stage = Stage::Cook { left };
            }
            return cmd;
        }
        self.stage = Stage::Wander;
        cmd
    }

    fn think_cook(&mut self, view: &BotView, mut cmd: UserCmd, left: u32) -> UserCmd {
        if let Some(g) = view.grenade {
            cmd.weapon = g;
        }
        if view.weapon != cmd.weapon || !view.playing {
            return cmd;
        }
        if left > 1 {
            cmd.buttons = msg::BUTTON_ATTACK;
            self.stage = Stage::Cook { left: left - 1 };
        } else {
            // The release; the next stage walks the byte back to the rifle.
            self.stage = Stage::BackToRifle { weapon: self.rifle };
        }
        cmd
    }

    fn think_back(&mut self, view: &BotView, mut cmd: UserCmd, weapon: u8) -> UserCmd {
        cmd.weapon = weapon;
        if view.weapon == weapon {
            self.stage = Stage::Wander;
        }
        cmd
    }
}

/// Ticks a stuck bot spends on a random heading before its waypoint again.
const UNSTICK_TICKS: u32 = 15;
/// A waypoint this far above the feet is up a ladder: look up, where
/// `ladder_move` climbs at full rate.
const CLIMB_HEIGHT: f32 = 48.0;
/// A waypoint this far below is down a ladder or off a ledge: back onto it
/// facing the way it came, which is the side a ladder's grab traces toward
/// (the graph proved the edge the same way, `crate::nav`).
const BACK_DOWN_HEIGHT: f32 = 64.0;

/// (pitch, yaw, forward) that take a body at `from` toward `to`.
fn steer(from: [f32; 3], to: [f32; 3]) -> (f32, f32, i8) {
    let yaw = (to[1] - from[1]).atan2(to[0] - from[0]).to_degrees();
    let dz = to[2] - from[2];
    if dz < -BACK_DOWN_HEIGHT {
        (-crate::nav::LADDER_PITCH, yaw_diff(yaw + 180.0, 0.0), -127)
    } else if dz > CLIMB_HEIGHT {
        (-crate::nav::LADDER_PITCH, yaw, 127)
    } else {
        (0.0, yaw, 127)
    }
}

/// Grenade reuses are minutes apart, not seconds.
const GRENADE_COOLDOWN_TICKS: u32 = 400;
const BOT_EYE_HEIGHT: f32 = 60.0;
const ANGLE2SHORT: f32 = 65536.0 / 360.0;

pub(crate) fn dist_sq(a: [f32; 3], b: [f32; 3]) -> f32 {
    let d = [a[0] - b[0], a[1] - b[1], a[2] - b[2]];
    d[0] * d[0] + d[1] * d[1] + d[2] * d[2]
}

/// Wire pitch is positive down (protocol doc, "View angles"), so a target
/// below the eye aims positive.
fn aim_angles(view: &BotView, at: [f32; 3]) -> (f32, f32) {
    let dx = at[0] - view.origin[0];
    let dy = at[1] - view.origin[1];
    let dz = at[2] - (view.origin[2] + BOT_EYE_HEIGHT);
    let horiz = (dx * dx + dy * dy).sqrt().max(1.0);
    let pitch = (-dz.atan2(horiz).to_degrees()).clamp(-80.0, 80.0);
    let yaw = dy.atan2(dx).to_degrees();
    (pitch, yaw)
}

/// (pitch, yaw) one tick's turn from `cur` toward `want`, at most `max`
/// degrees along the straight line between them.
fn turn(cur: [f32; 2], want: [f32; 2], max: f32) -> [f32; 2] {
    let d = [yaw_diff(want[0], cur[0]), yaw_diff(want[1], cur[1])];
    let len = d[0].hypot(d[1]);
    let k = if len > max { max / len } else { 1.0 };
    [(cur[0] + d[0] * k).clamp(-80.0, 80.0), cur[1] + d[1] * k]
}

/// An absolute (pitch, yaw) as the cmd carries it, net of `delta_angles`.
fn cmd_angles(view: &BotView, aim: [f32; 2]) -> [i32; 3] {
    [
        deg_short(aim[0]) - view.delta_angles[0],
        deg_short(aim[1]) - view.delta_angles[1],
        -view.delta_angles[2],
    ]
}

/// The shorter signed way round, degrees.
fn yaw_diff(to: f32, from: f32) -> f32 {
    (to - from + 180.0).rem_euclid(360.0) - 180.0
}

fn deg_short(deg: f32) -> i32 {
    (deg * ANGLE2SHORT) as i32
}

/// The stock weapon menus (`ui_mp/scriptmenus/weapon_<nationality>.menu`),
/// each weapon with the cvar `_teams::restrict` checks it against. Any other
/// response is "restricted" and reopens the menu.
const WEAPON_MENUS: [(&str, &[(&str, &str)]); 4] = [
    (
        "american",
        &[
            ("m1carbine_mp", "scr_allow_m1carbine"),
            ("m1garand_mp", "scr_allow_m1garand"),
            ("thompson_mp", "scr_allow_thompson"),
            ("bar_mp", "scr_allow_bar"),
            ("springfield_mp", "scr_allow_springfield"),
        ],
    ),
    (
        "british",
        &[
            ("enfield_mp", "scr_allow_enfield"),
            ("sten_mp", "scr_allow_sten"),
            ("bren_mp", "scr_allow_bren"),
            ("springfield_mp", "scr_allow_springfield"),
        ],
    ),
    (
        "russian",
        &[
            ("mosin_nagant_mp", "scr_allow_nagant"),
            ("ppsh_mp", "scr_allow_ppsh"),
            ("mosin_nagant_sniper_mp", "scr_allow_nagantsniper"),
        ],
    ),
    (
        "german",
        &[
            ("kar98k_mp", "scr_allow_kar98k"),
            ("mp40_mp", "scr_allow_mp40"),
            ("mp44_mp", "scr_allow_mp44"),
            ("kar98k_sniper_mp", "scr_allow_kar98ksniper"),
        ],
    ),
];

#[cfg(test)]
mod tests {
    use super::*;
    use vcod_common::net::msg::{BUTTON_ADS, BUTTON_ATTACK, BUTTON_USE, WBUTTON_RELOAD};

    const SID: i32 = 0x10;

    fn all(_: &str) -> bool {
        true
    }

    /// A shooting bot with no reaction delay, already looking at an enemy
    /// 100 units down +x.
    fn engaged(seed: u64) -> (Bot, BotView) {
        let mut bot = Bot::new("allies", true, seed);
        bot.skill.reaction_min = 0;
        bot.skill.reaction_spread = 0;
        let mut v = view();
        v.view = [0.0, 0.0, 0.0];
        v.enemy = Some(EnemyView {
            slot: 3,
            origin: [100.0, 0.0, 124.0],
        });
        (bot, v)
    }

    fn view() -> BotView {
        BotView {
            origin: [0.0, 0.0, 64.0],
            delta_angles: [0; 3],
            view: [0.0, 90.0, 0.0],
            weapon: 10,
            weapons_held: (1 << 10) | (1 << 6),
            clip: 8,
            fire_time_ms: 300,
            busy_ms: 0,
            automatic: false,
            has_ads: true,
            sniper: false,
            ads_frac: 0.0,
            speed: 0.0,
            dead: false,
            playing: true,
            enemy: None,
            grenade: Some(6),
            waypoint: None,
            linked: false,
            sd: None,
        }
    }

    /// A view standing in the middle of one bombzone, with a second far off.
    fn sd_view(role: ObjRole) -> BotView {
        let mut v = view();
        v.sd = Some(SdView {
            role,
            sites: vec![
                SiteView {
                    mins: [-100.0, -100.0, 0.0],
                    maxs: [100.0, 100.0, 128.0],
                    stand: [0.0, 0.0, 64.0],
                },
                SiteView {
                    mins: [1900.0, -100.0, 0.0],
                    maxs: [2100.0, 100.0, 128.0],
                    stand: [2000.0, 0.0, 64.0],
                },
            ],
            bomb: None,
        });
        v
    }

    /// An attacker that always goes to the first site.
    fn attacker() -> Bot {
        let mut bot = Bot::new("allies", true, 1);
        bot.obj.pick = 0;
        bot
    }

    fn plant_bomb(v: &mut BotView, at: [f32; 3]) {
        let sd = v.sd.as_mut().unwrap();
        sd.sites.clear();
        sd.bomb = Some(BombView {
            origin: at,
            aim: [at[0], at[1], at[2] + 8.0],
        });
    }

    #[test]
    fn an_attacker_at_a_site_holds_use_still_until_the_zones_go() {
        let mut bot = attacker();
        let mut v = sd_view(ObjRole::Attack);
        for tick in 0..100 {
            let cmd = bot.think(&v);
            assert_eq!(cmd.buttons, BUTTON_USE, "tick {tick}");
            assert_eq!((cmd.forward, cmd.right, cmd.up), (0, 0, 0), "tick {tick}");
            assert_eq!(cmd.weapon, 10, "the cmd carries the held weapon byte");
            assert_eq!(bot.goal(&v), Goal::Hold);
            // The script links the planter once its loop reads the press.
            v.linked = tick >= 2;
        }
        // Planted: the zones are gone and the bomb sits at the feet.
        plant_bomb(&mut v, [0.0, 0.0, 64.0]);
        v.linked = false;
        let cmd = bot.think(&v);
        assert_eq!(cmd.buttons & BUTTON_USE, 0, "use held past the plant");
        assert_eq!(bot.goal(&v), Goal::Roam, "the planter stays on its bomb");
    }

    #[test]
    fn a_planting_bot_holds_use_through_an_enemy_and_a_stall() {
        let mut bot = attacker();
        let mut v = sd_view(ObjRole::Attack);
        bot.think(&v);
        v.linked = true;
        v.enemy = Some(EnemyView {
            slot: 3,
            origin: [300.0, 0.0, 104.0],
        });
        for tick in 0..90 {
            let cmd = bot.think(&v);
            assert_eq!(cmd.buttons, BUTTON_USE, "tick {tick}: let go to fight");
            assert_eq!((cmd.forward, cmd.right), (0, 0), "tick {tick}");
            assert_eq!(bot.goal(&v), Goal::Hold);
        }
    }

    #[test]
    fn a_plant_that_never_links_is_let_go_for_the_other_site() {
        let mut bot = attacker();
        let v = sd_view(ObjRole::Attack);
        let held = (0..60)
            .take_while(|_| bot.think(&v).buttons == BUTTON_USE)
            .count();
        assert_eq!(held as u32, START_TICKS);
        assert_eq!(bot.goal(&v), Goal::To([2000.0, 0.0, 64.0]));
    }

    #[test]
    fn a_defender_turns_onto_the_bomb_then_holds_use() {
        let mut bot = Bot::new("axis", true, 1);
        let mut v = sd_view(ObjRole::Defend);
        let bomb = [30.0, 0.0, 64.0];
        plant_bomb(&mut v, bomb);
        // Facing away from it, down +y.
        v.view = [0.0, 90.0, 0.0];
        assert_eq!(bot.goal(&v), Goal::Hold, "within reach already");
        let (tp, ty) = aim_angles(&v, [30.0, 0.0, 72.0]);
        let mut pressed = None;
        for tick in 0..40 {
            let cmd = bot.think(&v);
            let aim = [
                cmd.angles[0] as f32 / ANGLE2SHORT,
                cmd.angles[1] as f32 / ANGLE2SHORT,
            ];
            assert_eq!((cmd.forward, cmd.right), (0, 0));
            if cmd.buttons & BUTTON_USE != 0 {
                // Pressed only with the view, not just the cmd, on the bomb.
                assert!(yaw_diff(v.view[0], tp).hypot(yaw_diff(v.view[1], ty)) < SETTLE_DEG);
                pressed = Some(tick);
                break;
            }
            v.view = [aim[0], aim[1], 0.0];
        }
        let pressed = pressed.expect("never pressed use on the bomb");
        assert!(pressed >= 6, "90 degrees at 15 a tick takes six ticks");
        v.linked = true;
        for _ in 0..180 {
            assert_eq!(bot.think(&v).buttons, BUTTON_USE);
        }
        // Defused: the trigger is gone, so is the press.
        v.sd.as_mut().unwrap().bomb = None;
        v.linked = false;
        assert_eq!(bot.think(&v).buttons & BUTTON_USE, 0);
    }

    #[test]
    fn use_is_pressed_only_at_the_objective() {
        for role in [ObjRole::Attack, ObjRole::Defend] {
            let mut bot = attacker();
            let mut v = sd_view(role);
            v.origin = [800.0, 0.0, 64.0];
            v.waypoint = Some([700.0, 0.0, 64.0]);
            assert_eq!(bot.goal(&v), Goal::To([0.0, 0.0, 64.0]), "{role:?}");
            for _ in 0..100 {
                let cmd = bot.think(&v);
                assert_eq!(cmd.buttons & BUTTON_USE, 0, "{role:?}");
                assert_eq!(cmd.forward, 127, "{role:?}");
                v.origin[0] -= 1.0;
            }
            // A guarding defender stands in its zone and presses nothing.
            if role == ObjRole::Defend {
                v.origin = [0.0, 0.0, 64.0];
                assert_eq!(bot.goal(&v), Goal::Hold);
                for _ in 0..100 {
                    let cmd = bot.think(&v);
                    assert_eq!((cmd.buttons, cmd.forward), (0, 0));
                }
            }
        }
        // A defender short of the bomb walks to it without a press.
        let mut bot = Bot::new("axis", true, 1);
        let mut v = sd_view(ObjRole::Defend);
        plant_bomb(&mut v, [500.0, 0.0, 64.0]);
        assert_eq!(bot.goal(&v), Goal::To([500.0, 0.0, 64.0]));
        assert_eq!(bot.think(&v).buttons & BUTTON_USE, 0);
    }

    #[test]
    fn an_open_team_menu_is_answered_with_the_bots_team_once() {
        let mut bot = Bot::new("allies", false, 1);
        assert_eq!(
            bot.observe(SID, &[(1, "v g_scriptMainMenu \"team_allies\"")], &all),
            Vec::<String>::new()
        );
        assert_eq!(
            bot.observe(SID, &[(2, "t 0")], &all),
            vec![format!("mr {SID} 0 allies")]
        );
        assert_eq!(bot.observe(SID, &[(3, "t 0")], &all), Vec::<String>::new());
    }

    #[test]
    fn a_wandering_bot_runs_forward_carrying_the_held_weapon() {
        let mut bot = Bot::new("allies", false, 1);
        let cmd = bot.think(&view());
        assert_eq!(cmd.forward, 127, "the wander holds forward");
        assert_eq!(cmd.weapon, 10, "the cmd carries the held weapon byte");
        assert_eq!(cmd.buttons, 0, "nothing is pressed without an enemy");
        assert_eq!(cmd.right, 0);
    }

    #[test]
    fn a_stalled_heading_is_swapped_for_a_new_one() {
        let mut bot = Bot::new("allies", false, 1);
        let v = view();
        let first = bot.think(&v).angles[1];
        // Grinding into a wall: ten ticks without moving off the stall spot.
        for _ in 0..20 {
            bot.think(&v);
        }
        let after = bot.think(&v).angles[1];
        assert_ne!(first, after, "a bot that cannot move keeps one heading");
        // And the new heading eventually turns again too.
        for _ in 0..40 {
            bot.think(&v);
        }
        assert_ne!(after, bot.think(&v).angles[1], "the stall swap is not once");
    }

    #[test]
    fn the_goal_is_the_seen_enemy_else_anywhere() {
        let bot = Bot::new("allies", true, 1);
        let mut v = view();
        assert_eq!(bot.goal(&v), Goal::Roam);
        v.enemy = Some(EnemyView {
            slot: 3,
            origin: [500.0, 0.0, 40.0],
        });
        assert_eq!(bot.goal(&v), Goal::To([500.0, 0.0, 40.0]));
        v.dead = true;
        assert_eq!(bot.goal(&v), Goal::Hold);
    }

    #[test]
    fn a_bot_runs_at_its_waypoint() {
        let mut bot = Bot::new("allies", false, 1);
        let mut v = view();
        v.waypoint = Some([0.0, 100.0, 64.0]);
        let cmd = bot.think(&v);
        assert_eq!(cmd.forward, 127);
        assert_eq!(cmd.angles, [0, deg_short(90.0), 0], "level, facing +y");
    }

    #[test]
    fn a_waypoint_up_a_ladder_looks_up_and_one_below_is_backed_onto() {
        let mut bot = Bot::new("allies", false, 1);
        let mut v = view();
        v.waypoint = Some([32.0, 0.0, 200.0]);
        let up = bot.think(&v);
        assert_eq!(
            (up.forward, up.angles[0]),
            (127, deg_short(-crate::nav::LADDER_PITCH))
        );
        v.waypoint = Some([32.0, 0.0, -100.0]);
        let down = bot.think(&v);
        assert_eq!(down.forward, -127, "backs off the ledge");
        assert_eq!(
            down.angles[1],
            deg_short(-180.0),
            "facing away from the waypoint"
        );
    }

    #[test]
    fn a_stuck_bot_leaves_its_waypoint_for_a_spell() {
        let mut bot = Bot::new("allies", false, 1);
        let mut v = view();
        v.waypoint = Some([0.0, 100.0, 64.0]);
        let toward = bot.think(&v).angles[1];
        // Pinned: the origin never moves, and the stall reads within two
        // ten-tick windows.
        let turned = (0..25).any(|_| bot.think(&v).angles[1] != toward);
        assert!(turned, "a pinned bot kept pushing at its waypoint");
        // The random heading gets it moving again.
        for _ in 0..UNSTICK_TICKS {
            v.origin[0] += 5.0;
            v.waypoint = Some([v.origin[0], 100.0, 64.0]);
            bot.think(&v);
        }
        assert_eq!(bot.think(&v).angles[1], toward, "and goes back to it after");
    }

    #[test]
    fn a_weapon_menu_reply_is_one_the_nationality_offers_and_the_cvars_allow() {
        for (nation, weapons) in WEAPON_MENUS {
            let mut seen = Vec::new();
            for seed in 1..64 {
                let mut bot = Bot::new("axis", false, seed);
                let menu = format!("v g_scriptMainMenu weapon_{nation}");
                bot.observe(SID, &[(1, menu.as_str())], &all);
                let reply = bot.observe(SID, &[(2, "t 1")], &all);
                let w = reply[0]
                    .strip_prefix(&format!("mr {SID} 1 "))
                    .unwrap()
                    .to_string();
                assert!(weapons.iter().any(|(n, _)| *n == w), "{nation}: {w}");
                seen.push(w);
            }
            for (w, _) in weapons {
                assert!(seen.iter().any(|s| s == w), "{nation}: {w} never picked");
            }
        }
        // A restricted weapon is never picked; nothing left means no reply.
        let only_kar = |c: &str| c == "scr_allow_kar98k";
        for seed in 1..32 {
            let mut bot = Bot::new("axis", false, seed);
            bot.observe(SID, &[(1, "v g_scriptMainMenu weapon_german")], &all);
            assert_eq!(
                bot.observe(SID, &[(2, "t 1")], &only_kar),
                vec![format!("mr {SID} 1 kar98k_mp")]
            );
        }
        let mut bot = Bot::new("allies", false, 1);
        bot.observe(SID, &[(1, "v g_scriptMainMenu weapon_russian")], &all);
        assert!(bot.observe(SID, &[(2, "t 1")], &|_: &str| false).is_empty());
    }

    #[test]
    fn a_new_target_holds_fire_through_the_reaction_delay() {
        let (_, v) = engaged(7);
        let mut bot = Bot::new("allies", true, 7);
        // Settled aim from the start: without the delay the first tick fires.
        bot.skill.error_start = bot.skill.error_floor;
        let react = bot.skill.reaction_min as usize;
        for tick in 0..react {
            let cmd = bot.think(&v);
            assert_eq!(cmd.buttons & BUTTON_ATTACK, 0, "fired on tick {tick}");
        }
        let fired = (0..40).any(|_| bot.think(&v).buttons & BUTTON_ATTACK != 0);
        assert!(fired, "never fired once the delay ran out");
        // A switch to another enemy restarts the delay.
        let e = v.enemy.unwrap();
        let mut other = v;
        other.enemy = Some(EnemyView { slot: 4, ..e });
        bot.fire_cooldown = 0;
        bot.burst_ticks = 0;
        for tick in 0..react {
            let cmd = bot.think(&other);
            assert_eq!(cmd.buttons & BUTTON_ATTACK, 0, "fired on tick {tick}");
        }
    }

    #[test]
    fn the_view_turns_a_bounded_step_per_tick() {
        let (mut bot, mut v) = engaged(3);
        // The enemy is behind: yaw 180 from a view at 0.
        v.enemy = Some(EnemyView {
            slot: 3,
            origin: [-500.0, 0.0, 124.0],
        });
        let step = bot.skill.turn_deg;
        let mut yaw = 0.0f32;
        for _ in 0..30 {
            v.view = [0.0, yaw, 0.0];
            let cmd = bot.think(&v);
            let next = cmd.angles[1] as f32 / ANGLE2SHORT;
            assert!(
                yaw_diff(next, yaw).abs() <= step + 0.01,
                "turned {} in one tick",
                yaw_diff(next, yaw)
            );
            yaw = next;
        }
        assert!(yaw_diff(180.0, yaw).abs() < 5.0, "never came round: {yaw}");
    }

    #[test]
    fn the_aim_error_shrinks_on_a_held_target() {
        let (mut bot, v) = engaged(5);
        bot.think(&v);
        let first = bot.target.as_ref().unwrap().error;
        for _ in 0..40 {
            bot.think(&v);
        }
        let later = bot.target.as_ref().unwrap().error;
        assert!(later < first / 4.0, "{first} -> {later}");
        assert_eq!(later, bot.skill.error_floor);
    }

    #[test]
    fn an_automatic_holds_the_trigger_and_a_semi_auto_taps() {
        let (mut bot, mut v) = engaged(9);
        v.automatic = true;
        let held: Vec<bool> = (0..40)
            .map(|_| bot.think(&v).buttons & BUTTON_ATTACK != 0)
            .collect();
        assert!(
            held.windows(2).any(|w| w[0] && w[1]),
            "an automatic never held across two ticks"
        );

        let (mut bot, v) = engaged(9);
        let mut last = None;
        let mut count = 0;
        let gap = (v.fire_time_ms / TICK_MS).max(2);
        for tick in 0..60 {
            if bot.think(&v).buttons & BUTTON_ATTACK != 0 {
                if let Some(l) = last {
                    assert!(tick - l >= gap, "taps {l} and {tick} too close");
                }
                last = Some(tick);
                count += 1;
            }
        }
        assert!(count >= 3, "a semi-auto tapped {count} times in 3 s");
    }

    #[test]
    fn sights_go_up_at_range_and_hold_fire_until_they_are() {
        let (mut bot, mut v) = engaged(11);
        v.enemy = Some(EnemyView {
            slot: 3,
            origin: [900.0, 0.0, 124.0],
        });
        for _ in 0..40 {
            let cmd = bot.think(&v);
            assert!(cmd.buttons & BUTTON_ADS != 0, "no sights at 900 units");
            assert_eq!(cmd.buttons & BUTTON_ATTACK, 0, "fired from the hip");
            assert_eq!(cmd.right, 0, "strafed while sighted");
        }
        v.ads_frac = 1.0;
        assert!((0..40).any(|_| bot.think(&v).buttons & BUTTON_ATTACK != 0));
    }

    #[test]
    fn an_engaging_bot_strafes() {
        let (mut bot, v) = engaged(13);
        let rights: Vec<i8> = (0..80).map(|_| bot.think(&v).right).collect();
        assert!(
            rights.contains(&127) && rights.contains(&-127),
            "{rights:?}"
        );
    }

    #[test]
    fn an_empty_clip_is_reloaded() {
        let mut bot = Bot::new("allies", true, 1);
        let mut v = view();
        v.clip = 0;
        let cmd = bot.think(&v);
        assert!(cmd.wbuttons & WBUTTON_RELOAD != 0, "a dry clip reloads");
    }

    #[test]
    fn a_dead_bot_presses_use_after_a_beat() {
        let mut bot = Bot::new("allies", false, 1);
        let mut v = view();
        v.dead = true;
        v.playing = false;
        for _ in 0..RESPAWN_DELAY + 2 {
            let cmd = bot.think(&v);
            if cmd.buttons & BUTTON_USE != 0 {
                return;
            }
        }
        panic!("a dead bot never pressed use");
    }

    #[test]
    fn a_bot_throw_switches_to_the_frag_cooks_and_throws() {
        let mut bot = Bot::new("allies", true, 0x1234_5678);
        let mut v = view();
        v.enemy = Some(EnemyView {
            slot: 3,
            origin: [300.0, 0.0, 40.0],
        });
        // Grind until the grenade machine starts; the cooldown is short with
        // a live enemy this close.
        let mut switched = None;
        let mut held = false;
        let mut released = false;
        for _ in 0..600 {
            let cmd = bot.think(&v);
            // The real sim follows the byte; the fake view does the same.
            v.weapon = cmd.weapon;
            if cmd.weapon == 6 {
                switched = Some(cmd.weapon);
                held |= cmd.buttons & BUTTON_ATTACK != 0;
            }
            if held && cmd.weapon == 6 && cmd.buttons & BUTTON_ATTACK == 0 {
                released = true;
            }
        }
        assert!(switched.is_some(), "the bot never switched to the frag");
        assert!(held, "the pullback never held the trigger");
        assert!(released, "the cooked throw never released");
    }

    /// A death mid-grenade-phase must not strand the corpse: the stage
    /// machine must not swallow the respawn press.
    #[test]
    fn a_bot_killed_mid_grenade_phase_still_presses_use() {
        for start_stage in 0..3 {
            let mut bot = Bot::new("allies", true, 1);
            // Walk the machine into each non-wander stage by force.
            match start_stage {
                0 => {
                    bot.rifle = 10;
                    bot.stage = Stage::ToGrenade;
                }
                1 => {
                    bot.rifle = 10;
                    bot.stage = Stage::Cook { left: 5 };
                }
                _ => {
                    bot.stage = Stage::BackToRifle { weapon: 10 };
                }
            }
            let mut v = view();
            v.dead = true;
            v.playing = false;
            let mut pressed = false;
            for _ in 0..RESPAWN_DELAY + RESPAWN_RETRY * 3 {
                if bot.think(&v).buttons & BUTTON_USE != 0 {
                    pressed = true;
                    break;
                }
            }
            assert!(
                pressed,
                "stage {start_stage}: a bot killed mid-grenade-phase never pressed use"
            );
        }
    }
}
