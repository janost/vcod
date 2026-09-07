//! Server-side bots: synthetic clients in `Server::clients` whose brains
//! live here. A bot joins through the stock menus like any client (the
//! driver in `server.rs` feeds this module the reliable server commands and
//! sends back the `mr` replies), then sends real usercmds through pmove.
//!
//! The brain is pure: [`Bot::observe`] consumes server commands and returns
//! `mr` replies, [`Bot::think`] returns the tick's usercmd from a read-only
//! [`BotView`] the server fills. Everything wire- and VM-shaped stays in
//! `server.rs`.

use vcod_common::net::msg::{self, UserCmd, NULL_USERCMD};

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
    pub dead: bool,
    /// `pm_type` 0, a spawned player rather than a spectator or camera.
    pub playing: bool,
    /// Nearest live enemy with a clear sightline, as the server traced it.
    pub enemy: Option<EnemyView>,
    /// A held grenade's configstring index, when one is in the kit.
    pub grenade: Option<u8>,
}

#[derive(Clone, Copy)]
pub struct EnemyView {
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
    fire_cooldown: u32,
    grenade_cooldown: u32,
    respawn_ticks: u32,
    stage: Stage,
    /// The weapon a grenade throw hands back to.
    rifle: u8,
    /// The last reliable server command the bot consumed.
    pub(crate) last_seen_seq: i32,
    /// The client-command sequence the bot's own replies use.
    pub(crate) next_command_seq: i32,
}

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
            main_menu: String::new(),
            answered: Vec::new(),
            heading: 0.0,
            heading_ticks: 0,
            stall_origin: [0.0; 3],
            stall_ticks: 0,
            fire_cooldown: 0,
            grenade_cooldown: 200,
            respawn_ticks: 0,
            stage: Stage::Wander,
            rifle: 0,
            last_seen_seq: 0,
            next_command_seq: 1,
            team: team.to_string(),
            rng: seed | 1,
        }
    }

    /// xorshift64* masked to 31 bits, the server's own generator.
    fn rand(&mut self) -> i32 {
        (vcod_common::rng::xorshift(&mut self.rng) >> 33) as i32 & 0x7fff_ffff
    }

    /// The reliable server commands the bot received this tick, as
    /// `(seq, text)`; returns the `mr` replies to send, in order. `server_id`
    /// is what the reply must name.
    pub(crate) fn observe(&mut self, server_id: i32, cmds: &[(i32, &str)]) -> Vec<String> {
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
                    let Some(reply) = menu_reply(&self.main_menu, &self.team) else {
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

    /// A new gamestate reruns `ClientConnect` and reopens the menus under the
    /// indices the last one used; without the clear the bot never spawns
    /// again (the same rejoin the probes learned).
    pub fn rearm(&mut self) {
        self.main_menu.clear();
        self.answered.clear();
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
            return cmd;
        }
        self.respawn_ticks = 0;
        match self.stage {
            Stage::ToGrenade => return self.think_grenade(view, cmd),
            Stage::Cook { left } => return self.think_cook(view, cmd, left),
            Stage::BackToRifle { weapon } => return self.think_back(view, cmd, weapon),
            Stage::Wander => {}
        }
        // A frag goes at a close enemy, occasionally, once the cooldown is
        // spent; the switch itself is the stage machine below.
        self.grenade_cooldown = self.grenade_cooldown.saturating_sub(1);
        if self.shoot && self.grenade_cooldown == 0 {
            if let (Some(g), Some(e)) = (view.grenade, view.enemy) {
                let close = dist_sq(view.origin, e.origin) < GRENADE_RANGE * GRENADE_RANGE;
                if close && self.rand() % 4 == 0 {
                    self.grenade_cooldown = GRENADE_COOLDOWN_TICKS;
                    self.rifle = view.weapon;
                    self.stage = Stage::ToGrenade;
                    cmd.weapon = g;
                    return cmd;
                }
            }
        }

        self.stall_ticks += 1;
        if self.heading_ticks == 0
            || (self.stall_ticks >= 10 && dist_sq(view.origin, self.stall_origin) < 15.0 * 15.0)
        {
            self.pick_heading(view);
        } else {
            self.heading_ticks -= 1;
        }

        let mut yaw = self.heading;
        let mut pitch = 0.0;
        if self.shoot {
            if let Some(e) = view.enemy {
                (pitch, yaw) = aim_angles(view, e.origin);
                if self.fire_cooldown == 0
                    && view.busy_ms == 0
                    && view.clip != 0
                    && yaw_diff(yaw, view.view[1]).abs() < 6.0
                {
                    cmd.buttons |= msg::BUTTON_ATTACK;
                    let fire = if view.fire_time_ms > 0 {
                        view.fire_time_ms
                    } else {
                        200
                    };
                    self.fire_cooldown = (fire / TICK_MS).max(2) as u32;
                }
            }
        }
        // A dry clip reloads; the tap is suppressed while the machine is
        // busy so a held bit cannot restart the reload it started.
        if view.clip == 0 && view.busy_ms == 0 {
            cmd.wbuttons |= msg::WBUTTON_RELOAD;
        }
        cmd.forward = 127;
        cmd.angles = [
            deg_short(pitch) - view.delta_angles[0],
            deg_short(yaw) - view.delta_angles[1],
            -view.delta_angles[2],
        ];
        cmd
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

/// The shorter signed way round, degrees.
fn yaw_diff(to: f32, from: f32) -> f32 {
    (to - from + 180.0).rem_euclid(360.0) - 180.0
}

fn deg_short(deg: f32) -> i32 {
    (deg * ANGLE2SHORT) as i32
}

/// The team menu takes a team; the weapon menu takes a weapon the menu's
/// nationality allows, and the stock defaults allow exactly one rifle each.
fn menu_reply<'a>(menu: &str, team: &'a str) -> Option<&'a str> {
    if menu.starts_with("team_") {
        return Some(team);
    }
    match menu.strip_prefix("weapon_")? {
        "american" => Some("m1carbine_mp"),
        "british" => Some("enfield_mp"),
        "russian" => Some("mosin_nagant_mp"),
        "german" => Some("kar98k_mp"),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use vcod_common::net::msg::{BUTTON_ATTACK, BUTTON_USE, WBUTTON_RELOAD};

    const SID: i32 = 0x10;

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
            dead: false,
            playing: true,
            enemy: None,
            grenade: Some(6),
        }
    }

    #[test]
    fn an_open_team_menu_is_answered_with_the_bots_team_once() {
        let mut bot = Bot::new("allies", false, 1);
        assert_eq!(
            bot.observe(SID, &[(1, "v g_scriptMainMenu \"team_allies\"")]),
            Vec::<String>::new()
        );
        assert_eq!(
            bot.observe(SID, &[(2, "t 0")]),
            vec![format!("mr {SID} 0 allies")]
        );
        assert_eq!(bot.observe(SID, &[(3, "t 0")]), Vec::<String>::new());
    }

    #[test]
    fn an_open_weapon_menu_is_answered_with_the_nationalitys_rifle() {
        let mut bot = Bot::new("axis", false, 1);
        assert_eq!(
            bot.observe(SID, &[(1, "v g_scriptMainMenu weapon_german")]),
            Vec::<String>::new()
        );
        assert_eq!(
            bot.observe(SID, &[(2, "t 1")]),
            vec![format!("mr {SID} 1 kar98k_mp")]
        );
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
    fn a_shooting_bot_aims_at_the_enemy_and_taps_fire() {
        let mut bot = Bot::new("allies", true, 1);
        let mut v = view();
        // The view already points at the enemy (yaw 0 deg is +x).
        v.view = [0.0, 0.0, 0.0];
        v.enemy = Some(EnemyView {
            origin: [100.0, 0.0, 40.0],
        });
        let cmd = bot.think(&v);
        assert!(
            cmd.buttons & BUTTON_ATTACK != 0,
            "the first aimed tick fires"
        );
        assert_eq!(cmd.weapon, 10);
        // The tap is one tick long, then the fireTime paces the next one.
        let next = bot.think(&view());
        assert_eq!(
            next.buttons & BUTTON_ATTACK,
            0,
            "a semi-auto tap is not held"
        );
    }

    #[test]
    fn an_aimed_bot_keeps_walking() {
        let mut bot = Bot::new("allies", true, 1);
        let mut v = view();
        v.enemy = Some(EnemyView {
            origin: [100.0, 0.0, 40.0],
        });
        let cmd = bot.think(&v);
        assert_eq!(cmd.forward, 127, "the bot walks while it aims");
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
