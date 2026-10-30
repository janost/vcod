//! Bot management for both servers (`crate::botmgmt` holds the rules):
//! the `addbot`, `removebot` and `listbots` console commands, and the pass
//! that keeps the bot count, the names and the teams where the
//! `vc_bot_*` cvars say. The pass and the commands are written once over
//! [`AdminHost`]; CoD1's `Server` implements it here, `Cod2Server` in
//! `cod2/bots.rs`. docs/research/bot-objectives.md, "Bot management".

use super::*;
use crate::botmgmt::{self, Action, Member, Settings, Side, cvar};

/// How often the pass reads the cvars and the roster when nothing asked
/// for one sooner.
const PASS_MS: i32 = 500;

/// The admin state both servers keep between passes (CoD1's [`BotAdmin`],
/// CoD2's `Cod2Bots::admin`).
#[derive(Debug, Default)]
pub struct Ledger {
    /// The bots asked for by count: `--bots`, `addbot`, `removebot`.
    pub manual: usize,
    /// Bots `addbot` put on a team by name; balance leaves them there.
    pub pinned: std::collections::BTreeSet<usize>,
    /// Bots `addbot ... skill N` set a level for; the cvar leaves them be.
    pub skills: BTreeMap<usize, u8>,
    /// Levels for the bots an `addbot auto ... skill N` asked the next
    /// pass to add.
    pub pending_skills: std::collections::VecDeque<u8>,
    pub last_pass: Option<i32>,
    /// A console command changed something; plan on the next tick.
    pub dirty: bool,
}

impl Ledger {
    pub fn new(manual: usize) -> Self {
        Ledger {
            manual,
            ..Default::default()
        }
    }
}

pub(super) struct BotAdmin {
    pub(super) ledger: Ledger,
    /// `vc_bot_shoot` as last applied.
    pub(super) shoot: bool,
    /// `vc_bot_skill` as last applied.
    skill: u8,
    /// The name draw's own generator, so naming a bot never moves the
    /// server's `rand()` sequence the seeded tests replay.
    name_rng: u64,
    /// The bots' own seeds and challenges: retail's `SV_AddTestClient`
    /// draws no `rand()`, so adding a bot leaves the shared sequence the
    /// spread and the recoil take where it was.
    pub(super) seed_rng: u64,
    /// Team menu answers for moved bots, sent with the bots' own replies.
    pub(super) orders: Vec<(usize, String)>,
    /// Each bot's `pers["team"]` as last logged: the script, not the menu
    /// answer, decides it (`bel` deals out the allied places).
    landed: BTreeMap<usize, String>,
}

impl BotAdmin {
    pub(super) fn new(manual: usize, shoot: bool, seed: u64) -> Self {
        BotAdmin {
            ledger: Ledger::new(manual),
            shoot,
            skill: crate::bots::DEFAULT_LEVEL,
            name_rng: seed ^ 0x9e37_79b9_7f4a_7c15,
            seed_rng: seed_stream(seed),
            orders: Vec::new(),
            landed: BTreeMap::new(),
        }
    }
}

/// The bot seed stream's start off a server seed.
pub(super) fn seed_stream(seed: u64) -> u64 {
    (seed ^ 0xd1b5_4a32_d192_ed03) | 1
}

/// What the shared pass and commands ask of a server. Retail ships no bots
/// on either game, so every hook is vcod's; they differ where the engines
/// under them do (cvar registry, client table, configstring layout) and
/// where each server's bots were built differently (seeds, naming, team
/// replies), which this stage keeps as it was.
pub(crate) trait AdminHost {
    /// Where a command's reply goes: CoD1's `Com_Printf` needs nothing,
    /// CoD2's takes the rcon `Redirect`.
    type Out;
    fn say(&mut self, out: &mut Self::Out, msg: &str);
    /// A level with a script is running.
    fn level_running(&self) -> bool;
    /// The pass may run: a level is up and the server keeps admin state
    /// (CoD2 makes its state here once a floor asks for bots).
    fn admin_ready(&mut self) -> bool;
    /// The level clock the pass is timed on.
    fn admin_now(&self) -> i32;
    fn ledger(&mut self) -> Option<&mut Ledger>;
    /// The ledger, made if the server had none yet.
    fn ledger_or_new(&mut self) -> &mut Ledger;
    fn bot_settings(&self) -> Settings;
    /// Applies a changed `vc_bot_shoot` and `vc_bot_skill` to the bots.
    fn apply_settings(&mut self, s: Settings);
    /// Whether the running gametype has two sides to balance.
    fn bot_teamed(&self) -> bool;
    /// The free public slots plus those bots hold: what bots may fill.
    fn bot_room(&self) -> usize;
    fn bot_count(&self) -> usize;
    /// Every connected client as the planner sees it.
    fn bot_roster(&mut self) -> Vec<Member>;
    /// A bot's name, `""` for no bot.
    fn bot_name(&self, slot: usize) -> String;
    /// A bot's skill level, `None` for no bot or a hand-set `Skill`.
    fn bot_level(&self, slot: usize) -> Option<u8>;
    /// Connects one bot asking for `side`, at `level` (pinned) or the
    /// cvar's; the slot taken.
    fn admit_bot(&mut self, side: Side, s: Settings, level: Option<u8>) -> Option<usize>;
    /// Drops a bot through `SV_DropClient`, as a kick would.
    fn remove_bot(&mut self, slot: usize);
    /// Answers the team menu for `side` on a bot's behalf.
    fn move_bot(&mut self, slot: usize, side: Side);
}

/// The planner's side for a roster row: `pers["team"]` once it names one,
/// else for a bot still joining (`asked`, its menu answer) that side.
pub(crate) fn roster_side(team: &str, asked: Option<&str>, joining: bool) -> Side {
    match Side::parse(team) {
        Some(s @ (Side::Allies | Side::Axis)) => s,
        _ => match asked.and_then(Side::parse) {
            Some(s) if joining => s,
            _ => Side::None,
        },
    }
}

/// The bot pass, at the top of each server's bot frame: applies the cvars,
/// then joins, drops and moves bots toward what the manual count, the
/// floor and balance ask for. Runs every [`PASS_MS`], or on the next tick
/// after a console command.
pub(crate) fn manage<H: AdminHost>(h: &mut H) {
    if !h.admin_ready() {
        return;
    }
    let now = h.admin_now();
    let Some(l) = h.ledger() else {
        return;
    };
    let due = l.last_pass.is_none_or(|t| now.wrapping_sub(t) >= PASS_MS);
    if !due && !l.dirty {
        return;
    }
    l.last_pass = Some(now);
    l.dirty = false;
    let s = h.bot_settings();
    h.apply_settings(s);
    let manual = h.ledger().map_or(0, |l| l.manual);
    let roster = h.bot_roster();
    // A human counts once he has picked a side; one still in the menus
    // or spectating takes no bot's place.
    let humans = roster
        .iter()
        .filter(|m| !m.bot && m.side != Side::None)
        .count();
    let want = botmgmt::target(manual, s.min_players, humans, h.bot_room());
    let teamed = h.bot_teamed();
    for action in botmgmt::plan(&roster, want, teamed, s.autobalance) {
        match action {
            Action::Add(side) => {
                let level = h.ledger().and_then(|l| l.pending_skills.pop_front());
                if h.admit_bot(side, s, level).is_none() {
                    break;
                }
            }
            Action::Remove(slot) => h.remove_bot(slot),
            Action::Move(slot, side) => h.move_bot(slot, side),
        }
    }
    if let Some(l) = h.ledger() {
        l.pending_skills.clear();
    }
}

/// `addbot [allies|axis|auto] [count] [skill N]`, the words in any
/// order. A named side pins the bots there; `auto` (the default) raises
/// the count the pass deals out by balance. `skill N` sets the bots'
/// level, which `vc_bot_skill` then leaves alone.
pub(crate) fn add_bot_command<H: AdminHost>(h: &mut H, args: &[String], out: &mut H::Out) {
    const USAGE: &str = "usage: addbot [allies|axis|auto] [count] [skill 0-3]\n";
    let mut side = Side::None;
    let mut count = 1usize;
    let mut level = None;
    let mut words = args.iter();
    while let Some(a) = words.next() {
        if let Some(s) = Side::parse(a) {
            side = s;
        } else if a.eq_ignore_ascii_case("skill") {
            match words.next().and_then(|n| n.parse::<u8>().ok()) {
                Some(n) => level = Some(n.min(crate::bots::MAX_LEVEL)),
                None => return h.say(out, USAGE),
            }
        } else if let Ok(n) = a.parse::<usize>() {
            count = n;
        } else {
            return h.say(out, USAGE);
        }
    }
    if !h.level_running() {
        return h.say(out, "addbot: no level is running\n");
    }
    let room = h.bot_room().saturating_sub(h.bot_count());
    let count = count.min(room);
    if count == 0 {
        return h.say(out, "addbot: no free public slot\n");
    }
    if side == Side::None {
        let have = h.bot_count();
        let l = h.ledger_or_new();
        if let Some(lv) = level {
            l.pending_skills.extend(std::iter::repeat_n(lv, count));
        }
        l.manual = have.max(l.manual) + count;
        l.dirty = true;
        return h.say(out, &format!("adding {count} bot(s)\n"));
    }
    h.ledger_or_new();
    let s = h.bot_settings();
    let mut added = 0;
    for _ in 0..count {
        let Some(slot) = h.admit_bot(side, s, level) else {
            break;
        };
        if let Some(l) = h.ledger() {
            l.pinned.insert(slot);
        }
        added += 1;
    }
    let have = h.bot_count();
    if let Some(l) = h.ledger() {
        l.manual = have.max(l.manual + added);
    }
    h.say(
        out,
        &format!("added {added} bot(s) to {}\n", side.response()),
    );
}

/// `removebot [name|all|count]`: by name (the `[BOT] ` prefix and
/// colours optional), every bot, or that many, lowest score first.
pub(crate) fn remove_bot_command<H: AdminHost>(h: &mut H, arg: Option<&str>, out: &mut H::Out) {
    let arg = arg.unwrap_or("1").trim();
    let roster = h.bot_roster();
    let mut bots: Vec<&Member> = roster.iter().filter(|m| m.bot).collect();
    bots.sort_by(|a, b| a.score.cmp(&b.score).then(b.slot.cmp(&a.slot)));
    let picked: Vec<usize> = if arg.eq_ignore_ascii_case("all") {
        bots.iter().map(|m| m.slot).collect()
    } else if let Ok(n) = arg.parse::<usize>() {
        bots.iter().take(n).map(|m| m.slot).collect()
    } else {
        let want = crate::game::say::clean_name(arg);
        let found = bots.iter().find(|m| {
            let clean = crate::game::say::clean_name(&h.bot_name(m.slot));
            clean.eq_ignore_ascii_case(&want)
                || clean
                    .strip_prefix(botmgmt::PREFIX)
                    .is_some_and(|n| n.eq_ignore_ascii_case(&want))
        });
        match found {
            Some(m) => vec![m.slot],
            None => return h.say(out, &format!("removebot: no bot named {arg}\n")),
        }
    };
    let names: Vec<String> = picked.iter().map(|&s| h.bot_name(s)).collect();
    for &slot in &picked {
        h.remove_bot(slot);
    }
    if let Some(l) = h.ledger() {
        l.manual = l.manual.saturating_sub(picked.len());
        l.dirty = true;
    }
    for n in names {
        h.say(out, &format!("removed {n}\n"));
    }
    let floor = h.bot_settings().min_players;
    if floor > 0 {
        h.say(
            out,
            &format!(
                "{} {floor} tops the bots back up; set it to 0 to keep them out\n",
                cvar::MIN_PLAYERS
            ),
        );
    }
}

/// `listbots`: slot, team, score, pin and skill of every bot, then the
/// count the server keeps and the cvars.
pub(crate) fn list_bots<H: AdminHost>(h: &mut H, out: &mut H::Out) {
    let roster = h.bot_roster();
    let mut text = String::from("num team    score pinned skill name\n");
    for m in roster.iter().filter(|m| m.bot) {
        let name = h.bot_name(m.slot);
        let team = match m.side {
            Side::Allies => "allies",
            Side::Axis => "axis",
            Side::None => "-",
        };
        let pinned = if m.pinned { "yes" } else { "no" };
        let skill = h.bot_level(m.slot).map_or("-".into(), |l| l.to_string());
        text.push_str(&format!(
            "{:3} {team:<7} {:5} {pinned:<6} {skill:<5} {name}\n",
            m.slot, m.score
        ));
    }
    let s = h.bot_settings();
    let humans = roster
        .iter()
        .filter(|m| !m.bot && m.side != Side::None)
        .count();
    let manual = h.ledger().map_or(0, |l| l.manual);
    text.push_str(&format!(
        "{} bot(s), {humans} human player(s); manual {manual}, {} {}, {} {}, {} {}, {} {}\n",
        roster.iter().filter(|m| m.bot).count(),
        cvar::MIN_PLAYERS,
        s.min_players,
        cvar::AUTOBALANCE,
        u8::from(s.autobalance),
        cvar::SHOOT,
        u8::from(s.shoot),
        cvar::SKILL,
        s.skill,
    ));
    h.say(out, &text);
}

impl Server {
    /// A bot's skill level, or `None` for no bot or values no level has
    /// (a test's hand-set `Skill`).
    pub fn bot_skill(&self, slot: usize) -> Option<u8> {
        self.game.module.driver.bots.get(&slot)?.skill.level_of()
    }

    /// Test-facing, for the bot harness: the team scores, [axis, allies].
    pub fn team_scores(&mut self) -> [i32; 2] {
        self.game
            .module
            .script
            .as_mut()
            .map_or([0, 0], |rt| rt.with_host(|h, _| h.team_scores))
    }

    /// Connects one bot in the first free public slot, asking for `side`
    /// (allies on `Side::None`), at skill `level` (pinned) or the cvar's.
    /// Its two draws come off the bots' own seed stream, never the
    /// server's `rand()`.
    fn add_bot(&mut self, side: Side, s: Settings, level: Option<u8>) -> Option<usize> {
        let private = self.private_clients().max(0) as usize;
        let Some(slot) = self.free_slot_from(private) else {
            log::warn!("no free slot for a bot");
            return None;
        };
        let team = match side {
            Side::Axis => "axis",
            _ => "allies",
        };
        let mut bot = crate::bots::Bot::new(
            team,
            self.game.module.bot_admin.shoot,
            rand_from(&mut self.game.module.bot_admin.seed_rng) as u64,
        );
        if self.move_rules() == vcod_common::pmove::MoveRules::Uo {
            bot.menus = &crate::bots::WEAPON_MENUS_UO;
        }
        let name = if s.names {
            let taken: Vec<String> = self
                .game
                .module
                .driver
                .bots
                .values()
                .map(|b| b.name.clone())
                .collect();
            let rng = &mut self.game.module.bot_admin.name_rng;
            botmgmt::draw_name(&taken, &mut |n| rand_from(rng) as usize % n)
        } else {
            format!("bot{}", slot + 1)
        };
        bot.name = name.clone();
        bot.skill = crate::bots::Skill::level(level.unwrap_or(s.skill));
        if let Some(l) = level {
            self.game.module.bot_admin.ledger.skills.insert(slot, l);
        }
        let userinfo = format!("\\name\\{name}\\snaps\\20\\rate\\25000\\cl_anonymous\\0");
        // A loopback address nothing listens on; every transmit to a bot
        // is skipped, the addr only has to be unique.
        let addr = SocketAddr::from(([127, 0, 0, 1], 65535 - slot as u16));
        let mut client = Client::new(
            addr,
            0x7000 + slot as u16,
            rand_from(&mut self.game.module.bot_admin.seed_rng),
            userinfo,
            Instant::now(),
        );
        client.is_bot = true;
        // `status` prints it as a bot address, as retail's test clients.
        client.bot_port = Some(slot as u16 + 1);
        self.core.clients[slot] = Some(client);
        self.game.module.driver.bots.insert(slot, bot);
        if let Some(rt) = self.game.module.script.as_mut() {
            rt.push_client_event(ClientEvent::Connect {
                slot,
                name: name.clone(),
            });
        }
        // The menu answer only: `bel`'s script picks the side, which the
        // pass logs once `pers["team"]` holds it.
        log::info!("client {slot} {name:?} connected (bot, asks for {team})");
        Some(slot)
    }
}

impl AdminHost for Server {
    type Out = ();

    fn say(&mut self, _: &mut (), msg: &str) {
        self.print(msg);
    }

    fn level_running(&self) -> bool {
        self.game.module.script.is_some()
    }

    fn admin_ready(&mut self) -> bool {
        self.game.module.script.is_some()
    }

    fn admin_now(&self) -> i32 {
        self.core.sv_time_ms
    }

    fn ledger(&mut self) -> Option<&mut Ledger> {
        Some(&mut self.game.module.bot_admin.ledger)
    }

    fn ledger_or_new(&mut self) -> &mut Ledger {
        &mut self.game.module.bot_admin.ledger
    }

    fn bot_settings(&self) -> Settings {
        let on = |name: &str, default: &str| {
            crate::game::builtins::cvar::atoi(&self.live_cvar(name, default)) != 0
        };
        Settings {
            min_players: self.live_int_cvar(cvar::MIN_PLAYERS).max(0) as usize,
            autobalance: on(cvar::AUTOBALANCE, "1"),
            shoot: on(cvar::SHOOT, "1"),
            names: on(cvar::NAMES, "1"),
            skill: self
                .live_int_cvar(cvar::SKILL)
                .clamp(0, i32::from(crate::bots::MAX_LEVEL)) as u8,
        }
    }

    /// Starts from `--bots-shoot` and the default level, and forgets the
    /// pins, levels and landings of bots gone since the last pass.
    fn apply_settings(&mut self, s: Settings) {
        if s.shoot != self.game.module.bot_admin.shoot {
            self.game.module.bot_admin.shoot = s.shoot;
            for b in self.game.module.driver.bots.values_mut() {
                b.set_shoot(s.shoot);
            }
        }
        let admin = &mut self.game.module.bot_admin;
        let bots = &mut self.game.module.driver.bots;
        admin
            .ledger
            .skills
            .retain(|slot, _| bots.contains_key(slot));
        if s.skill != admin.skill {
            admin.skill = s.skill;
            for (slot, b) in bots.iter_mut() {
                if !admin.ledger.skills.contains_key(slot) {
                    b.skill = crate::bots::Skill::level(s.skill);
                }
            }
        }
        admin.ledger.pinned.retain(|slot| bots.contains_key(slot));
        admin.landed.retain(|slot, _| bots.contains_key(slot));
    }

    /// Everything but `dm`, and `bel`, whose script deals the allied
    /// places itself.
    fn bot_teamed(&self) -> bool {
        !matches!(
            self.live_gametype().to_ascii_lowercase().as_str(),
            "dm" | "bel"
        )
    }

    /// A zombie holds its slot until the zombie pass frees it.
    fn bot_room(&self) -> usize {
        let private = self.private_clients().max(0) as usize;
        let free = (private..self.core.clients.len())
            .filter(|&i| self.core.clients[i].is_none() && self.game.zombies[i].is_none())
            .count();
        free + self.game.module.driver.bots.len()
    }

    fn bot_count(&self) -> usize {
        self.game.module.driver.bots.len()
    }

    /// Logs each bot's `pers["team"]` once it lands: on `bel` the script,
    /// not the menu answer, picks it.
    fn bot_roster(&mut self) -> Vec<Member> {
        let mut out = Vec::new();
        for slot in 0..self.core.clients.len() {
            let Some(c) = self.core.clients[slot].as_ref() else {
                continue;
            };
            let bot = self.game.module.driver.bots.get(&slot);
            // `pers["team"]`, which the team menu sets at once; `sessionteam`
            // waits for the spawn.
            let (team, state, score) = match self.game.module.script.as_mut() {
                Some(rt) => (
                    rt.client_pers(slot, "team").unwrap_or_default(),
                    rt.client_field(slot, "sessionstate").unwrap_or_default(),
                    rt.client_field(slot, "score").unwrap_or_default(),
                ),
                None => Default::default(),
            };
            if let Some(b) = bot
                && !team.is_empty()
                && self.game.module.bot_admin.landed.get(&slot) != Some(&team)
            {
                log::info!("bot {slot} {:?} is on team {team}", b.name);
                self.game.module.bot_admin.landed.insert(slot, team.clone());
            }
            let asked = bot.filter(|_| c.is_bot).map(|b| b.team());
            let side = roster_side(&team, asked, !matches!(state.as_str(), "playing" | "dead"));
            out.push(Member {
                slot,
                bot: c.is_bot && bot.is_some(),
                side,
                score: score.parse().unwrap_or(0),
                // Dead or spectating is out of the fight; an intermission
                // is not a time to move anyone.
                playing: !matches!(state.as_str(), "dead" | "spectator"),
                pinned: self.game.module.bot_admin.ledger.pinned.contains(&slot),
            });
        }
        out
    }

    fn bot_name(&self, slot: usize) -> String {
        self.game
            .module
            .driver
            .bots
            .get(&slot)
            .map_or(String::new(), |b| b.name.clone())
    }

    fn bot_level(&self, slot: usize) -> Option<u8> {
        self.bot_skill(slot)
    }

    fn admit_bot(&mut self, side: Side, s: Settings, level: Option<u8>) -> Option<usize> {
        self.add_bot(side, s, level)
    }

    fn remove_bot(&mut self, slot: usize) {
        self.game.module.bot_admin.ledger.pinned.remove(&slot);
        self.game.module.bot_admin.landed.remove(&slot);
        self.drop_client(slot, "EXE_PLAYERKICKED");
        self.game.module.driver.bots.remove(&slot);
    }

    /// The stock handlers switch a dead or spectating player and reopen the
    /// weapon menu, which the bot answers again. The reply goes out with
    /// the bots' own replies.
    fn move_bot(&mut self, slot: usize, side: Side) {
        let Some(bot) = self.game.module.driver.bots.get_mut(&slot) else {
            return;
        };
        bot.set_team(side.response());
        log::info!("bot {slot} {:?} moves to {}", bot.name, side.response());
        let (first, last) = self.game.proto.cs.bounds(configstrings::CsRange::Menu);
        let menu = (first..last).find(|&i| {
            self.core
                .configstrings
                .get(i)
                .is_some_and(|m| m.starts_with("team_") && m != "team_germanonly")
        });
        if let Some(i) = menu {
            let sid = i32::from(self.game.server_id);
            let reply = format!("mr {sid} {} {}", i - first, side.response());
            self.game.module.bot_admin.orders.push((slot, reply));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Adding bots leaves the server's shared `rand()` where it was, and
    /// each bot draws off a stream of its own.
    #[test]
    fn adding_a_bot_draws_nothing_off_the_shared_rand() {
        let cfg = ServerConfig {
            map: "mp_carentan".into(),
            hostname: "vcod test".into(),
            max_clients: 4,
            gametype: "dm".into(),
            test_entities: 0,
            trace: false,
            bots: 0,
            bots_shoot: false,
        };
        let mut sv = Server::new(cfg, Instant::now());
        sv.test_seed_rng(5);
        let before = sv.core.rng;
        let s = sv.bot_settings();
        let a = sv.add_bot(Side::Axis, s, None).expect("a slot");
        let b = sv.add_bot(Side::Allies, s, None).expect("a slot");
        assert_eq!(sv.core.rng, before);
        assert_ne!(
            sv.game.module.driver.bots[&a].rng,
            sv.game.module.driver.bots[&b].rng
        );
    }

    /// On `bel` every bot answers the team menu with axis and the script
    /// deals out the allied places: the side logged is `pers["team"]`,
    /// never the side the bot was added for.
    #[test]
    fn a_bel_bot_lands_where_the_script_puts_it() {
        let Some(fs) = vcod_common::testing::game_fs_11() else {
            eprintln!("COD_DIR unset or has no main/: skipping");
            return;
        };
        let cfg = ServerConfig {
            map: "mp_brecourt".into(),
            hostname: "vcod test".into(),
            max_clients: 8,
            gametype: "bel".into(),
            test_entities: 0,
            trace: false,
            bots: 4,
            bots_shoot: false,
        };
        let now = Instant::now();
        let mut sv = Server::new(cfg, now);
        sv.load_scripts(std::rc::Rc::new(fs))
            .expect("load the scripts");
        for _ in 0..200 {
            sv.tick(now);
        }
        let asked = sv
            .game
            .module
            .driver
            .bots
            .values()
            .filter(|b| b.team() == "allies")
            .count();
        let landed = sv.game.module.bot_admin.landed.clone();
        assert_eq!(landed.len(), 4, "{landed:?}");
        for (slot, team) in &landed {
            assert_eq!(
                sv.game
                    .module
                    .script
                    .as_mut()
                    .unwrap()
                    .client_pers(*slot, "team")
                    .as_deref(),
                Some(team.as_str())
            );
        }
        // Half asked for allies; bel's script made one of the four allied.
        assert_eq!(asked, 2);
        assert_eq!(landed.values().filter(|t| *t == "allies").count(), 1);
    }
}
