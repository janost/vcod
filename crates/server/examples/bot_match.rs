//! Seeded bot-vs-bot matches: the one measuring stick for bot changes
//! (docs/research/bot-combat.md section 1). Each map and seed runs one
//! match with the bots' fire on until the map ends (`exitLevel`) or
//! `TICKS` run out, and reads the script's `logPrint` lines: kills,
//! deaths, damage, the time from a bot's first hit on a victim to the
//! kill, objective actions (`A;`: plants, defuses, pickups, captures),
//! wins (`W;`: a round's in `sd` and `re`, the match's elsewhere) and the
//! team scores. Each bot-tick adds its life, its wounds (health first
//! under 50 in a fight this life) and those it died of within 5 s, the
//! retreats it starts (hurt, dry), and the ticks it holds a cover spot and
//! leans, the grenades it throws and the clip it starts each fight on
//! (grenade kills come off the `K;` lines), and its aim: rounds fired (a
//! bullet gun's clip drops), bullet hits off the `D;`/`K;` lines, and the
//! time from acquiring a target with none tracked to the first round at
//! it. It also counts the bot-ticks 12 s within 150 units, as
//! `ship_stalls` does, split into `held` (holding still on purpose,
//! `BotBody::holds`, on any tick of the 12 s) and `stalled` (the rest),
//! and the edges the followers gave up on, with those by another body
//! (`edges`, `by a body`: `Server::bot_edge_stalls`), and the mean time
//! from a round's start (the match's, or the last `W;` line) to its first
//! `A;` line (`to objective`),
//! how close
//! a team keeps: each playing bot-tick's distance to its nearest teammate
//! and whether two or more stand within 400 units, the hits bots answer
//! with fire at the attacker within 1 s and the deaths to a killer the
//! bot never targeted. It counts peeks from cover, those that repeat the
//! spot and pose of the one before, hunts that reach a lost enemy's spot
//! (or the vantage over it), and the deaths within 2 s of each. A fight
//! is a bot's ticks on one target with gaps under 1 s; it counts
//! how fights end (target down, the bot down, broken off) and the
//! re-engagements: the same bot on the same target again within 10 s of
//! a fight broken off with both alive.
//! Run: COD_DIR=<install> cargo run --release -p vcod-server --example bot_match [seed...]
//! `MAPS=` a comma list (`MAP=` alias; mp_carentan), `GAMETYPE=` (tdm),
//! `BOTS=` (6), `TICKS=` (12000, ten minutes), `JOBS=` (4, matches run in
//! parallel), `SET=name=value,...` cvars, `UO=1` a United Offensive
//! server. `AXIS_SKILL=` / `ALLIES_SKILL=` add the bots per side at those
//! `vc_bot_skill` levels, half each (dm: `SKILLS=a,b,...`, one bot per
//! entry); without them `BOTS` join by balance at `vc_bot_skill`.
//! One-sided feature arms: `COVER=`, `RETREAT=`, `TEAMPLAY=`, `AWARE=`,
//! `FOOTWORK=`, `WARY=`, `SQUAD=` and `AIM=` (cover and lean, `bots/cover.rs`; the
//! retreat, `bots/retreat.rs`; team play, `bots/team.rs`;
//! `Bot::aware`; moving while fighting, `Bot::footwork`; varied peeks
//! and hunts, `Bot::wary`; spacing out, `Bot::squad`; the hand's aim,
//! `Bot::human_aim`) each take `all` (the default), `none`, `axis`,
//! `allies` or `alt` (axis on odd seeds, allies on even, so the map's own
//! asymmetry cancels). `dm` has no sides: there an odd slot takes axis's
//! arm and an even one allies'.
//! With an arm one-sided the totals add a row per arm, named by the
//! one-sided features on for it. `VERBOSE=1` prints every kill. Seeds
//! default to 1..=8.
//!
//! `DIGEST=1` (any game) prints a line per match, `digest <map>
//! <gametype> seed <s> ticks <n> <hex>`: a running FNV-1a 64 over each
//! bot's last run cmd and its origin, velocity, health and weapon, folded
//! after every tick in slot order. `DIGEST=<N>` adds `digest@<tick>` lines
//! every N ticks, so two runs' first differing line names the window where
//! a bot first did something else. It only reads: the other lines match a
//! run without it.
//!
//! `NETDIGEST=<N>` joins one observer client (a spectator that never picks
//! a team, at a non-LAN address so rates and `snaps` pace it) before the
//! first tick and folds every datagram the server sends each tick, to
//! anyone, in send order (address, length, bytes), into a second FNV-1a
//! 64 printed as `net@<tick>` every N ticks and `net <map> <gametype> seed
//! <s> ticks <n> <hex>` per match. The observer is a client, so it changes
//! the match: compare `NETDIGEST` runs only with `NETDIGEST` runs. CoD2's
//! `svs.time` reads the wall clock, which keys the challenge and so every
//! netchan byte; under `NETDIGEST` the CoD2 server runs it on the frame
//! clock (`Cod2Server::test_frame_clock`). `NETDUMP=<dir>` writes the
//! datagrams themselves, a line each (`<tick> <to> <len> <hex>`), to
//! `<dir>/<game>_<map>_<gametype>_s<seed>.net`, to find the bytes behind a
//! `net@` window that parted.
//!
//! `OBSERVERS=<N>` (CoD1 and CoD2) joins N more such spectators, each at
//! its own address, whose datagrams no digest folds: a server with N
//! human clients to build snapshots for. They change the match as the
//! `NETDIGEST` one does.
//!
//! `VCOD_TICK_TRACE=<dir>` writes each match's tick step log
//! (`vcod_server::tick_trace`) to `<dir>/<game>_<map>_<gametype>_s<seed>.trace`,
//! `<game>` `11`, `uo` or `cod2`.
//!
//! `GAME=cod2` runs CoD2 matches instead (`COD2_DIR=<install>`, maps
//! `mp_toujane` by default, `GAMETYPE=` dm or tdm): the bots of
//! `vcod-server --game cod2 --bots N --bots-shoot` (`bots::cod2`), with
//! kills, deaths, suicides, hits, rounds fired and the 12 s stalls split
//! as above (held: `BotBody::holds`' engaged, cover, retreat or objective,
//! or frozen by the script, on any tick of the window; `STALLS=1` prints
//! every 20th stalled bot-tick to stderr as CoD4's does; for CoD1 where
//! each stalled window begins), plus the cover-hold and lean ticks, the
//! retreats, the smoke grenades thrown and the hits answered within 1 s.
//! The mounted guns' mounts, seconds manned and kills (`GUNS=0`:
//! bots leave the guns alone).
//! `SKILLS=` (one level a bot, in join order) or `AXIS_SKILL=` /
//! `ALLIES_SKILL=` set the levels; the totals split kills, hits, rounds,
//! sighted rounds and time-to-kill by weapon class and by skill, and count
//! the guns bots picked up. The feature arms and `UO` are CoD1's only.
//! `SPIKE_MS=<ms>` prints every tick past it with its phases, as CoD4's;
//! `RESTART=<n>` runs a console `map_restart` every `n` ticks, so every
//! bot spawns in one frame (the spawn wave, cod4-server-frame.md 9).
//!
//! `GAME=cod4` runs CoD4 1.7 matches (`COD4_DIR=<install>`, maps
//! `mp_crash` by default, `GAMETYPE=` any stock one, `war` by default,
//! `SET=` cvars before the load): the bots of `vcod-server --game cod4
//! --bots N --bots-shoot` (`bots::cod4`), with kills, deaths, suicides,
//! hits, rounds fired, the 12 s stalls split as CoD2's, ladder ticks, the
//! frag throws and mantles, the levels run past the first (`restarts`, a
//! round's `fast_restart`; `spawns`, a map loaded by the match's end and
//! rotation; the maps in order), the objectives' actions read off the
//! scripts each tick (bomb `pickup`, `plant`, `defuse`, `explode`, zone
//! `capture`), and every distinct script error the levels raised
//! (`ScriptRuntime::aborts`) with its count, per match and summed.
//! `LEAVE=<tick>,...` drops the lowest scoring bot at each of those ticks
//! (`removebot 1`), a client leaving mid-match; `OBJ=0` keeps the bots off
//! the objectives (`Cod4Bots::objectives`). `SPIKE_MS=<ms>` prints every
//! tick past it with its phases (`vcod_server::tick_profile`).

use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::net::SocketAddr;
use std::rc::Rc;
use std::time::{Duration, Instant};

const TICK_MS: u64 = 50;
/// A carrier's death this near a goal (or his pickup) counts as there.
const NEAR_GOAL: f32 = 600.0;
const WINDOW: usize = 240;
const RADIUS: f32 = 150.0;
/// A bot-tick is packed with this many teammates or more within
/// `PACK_RANGE` of it.
const PACK: usize = 2;
const PACK_RANGE: f32 = 400.0;

/// Which side a feature is on for (`COVER=`, `RETREAT=`, `TEAMPLAY=`,
/// `AWARE=`, `FOOTWORK=`, `WARY=`, `AIM=`).
#[derive(Clone, Copy, PartialEq)]
enum Arm {
    All,
    None,
    Axis,
    Allies,
    Alt,
}

impl Arm {
    fn parse(k: &str) -> Arm {
        match std::env::var(k).as_deref().unwrap_or("all") {
            "all" => Arm::All,
            "none" => Arm::None,
            "axis" => Arm::Axis,
            "allies" => Arm::Allies,
            "alt" => Arm::Alt,
            v => panic!("{k}={v}: all, none, axis, allies or alt"),
        }
    }

    /// Whether the feature is on for `team` ("axis", "allies", "none") in
    /// `seed`'s match.
    fn on(self, team: &str, seed: u64) -> bool {
        match self {
            Arm::All => true,
            Arm::None => false,
            Arm::Axis => team == "axis",
            Arm::Allies => team == "allies",
            Arm::Alt => team == if seed % 2 == 1 { "axis" } else { "allies" },
        }
    }

    fn one_sided(self) -> bool {
        matches!(self, Arm::Axis | Arm::Allies | Arm::Alt)
    }
}

#[derive(Clone)]
struct Opts {
    maps: Vec<String>,
    gametype: String,
    bots: usize,
    ticks: usize,
    uo: bool,
    set: Vec<(String, String)>,
    /// Console lines run once the level is up (the per-side `addbot`s).
    console: Vec<String>,
    verbose: bool,
    /// cover, retreat, team play, awareness, footwork, wariness, squad,
    /// aim, tactics, converge, lanes, far, cues, travel.
    arms: [Arm; 14],
}

const FEATURES: [&str; 14] = [
    "cover", "retreat", "teamplay", "aware", "footwork", "wary", "squad", "aim", "tactics",
    "converge", "lanes", "far", "cues", "travel",
];
const ARMS: [&str; 14] = [
    "COVER", "RETREAT", "TEAMPLAY", "AWARE", "FOOTWORK", "WARY", "SQUAD", "AIM", "TACTICS",
    "CONVERGE", "LANES", "FAR", "CUES", "TRAVEL",
];

/// The side a client's arms follow: its team, or in `dm` its slot's
/// parity (odd plays axis's arm).
fn arm_side(num: &str, team: &str, dm: bool) -> &'static str {
    match (dm, team) {
        (true, _) => match num.parse::<usize>() {
            Ok(s) if s % 2 == 1 => "axis",
            Ok(_) => "allies",
            Err(_) => "none",
        },
        (false, "axis") => "axis",
        (false, "allies") => "allies",
        _ => "none",
    }
}

impl Opts {
    /// The one-sided features on for `team`, the arm its tallies land in.
    fn arm(&self, team: &str, seed: u64) -> String {
        let on: Vec<&str> = FEATURES
            .iter()
            .zip(&self.arms)
            .filter(|(_, a)| a.one_sided() && a.on(team, seed))
            .map(|(f, _)| *f)
            .collect();
        if on.is_empty() {
            "control".into()
        } else {
            on.join("+")
        }
    }
}

/// One side of the tally: a team, or a skill level.
#[derive(Default, Clone)]
struct Tally {
    kills: u32,
    deaths: u32,
    suicides: u32,
    team_kills: u32,
    damage: u64,
    ttk_ms: Vec<u64>,
    /// Bot-ticks played and the lives they made up.
    life_ticks: u64,
    lives: u32,
    /// Wounds, and those died of within 5 s.
    wounds: u32,
    wound_deaths: u32,
    /// Retreats started hurt and dry.
    hurt: u32,
    dry: u32,
    /// Bot-ticks holding a cover spot, and leaning.
    hold: u64,
    lean: u64,
    /// Hits a bot took from an enemy (one per attacker until answered or
    /// 1 s on), those whose attacker it targeted within 1 s, and those it
    /// answered within 1 s with its trigger down and that attacker its
    /// target.
    hits_taken: u32,
    turned: u32,
    answered: u32,
    /// A bot's deaths to an enemy, and those whose killer it never
    /// targeted this life.
    killed: u32,
    blindsided: u32,
    /// Bot-ticks playing with a teammate playing, the sum of their
    /// distances to the nearest one, and those with [`PACK`] or more
    /// teammates inside [`PACK_RANGE`].
    mate_ticks: u64,
    mate_dist: f64,
    packed: u64,
    aim: Aim,
    kit: Kit,
    peeks: Peeks,
    /// Walking bot-ticks (`BotBody::walking` this tick and the last), the
    /// sum of their view turns in degrees, and those that turned more than
    /// [`SNAP_DEG`].
    walk_ticks: u64,
    walk_turn: f64,
    snaps: u64,
    fights: Fights,
}

/// A walking view turned more than this in one tick has snapped.
const SNAP_DEG: f32 = 30.0;

/// Grenades and the clip a fight starts on: throws, the hits and kills
/// their blasts made on others (`MOD_GRENADE_SPLASH`), the throwers they
/// killed (`own`) and the teammates (`team`), engagements started with a
/// bullet weapon in hand and the clip fractions they started on, summed.
#[derive(Default, Clone)]
struct Kit {
    throws: u32,
    grenade_hits: u32,
    grenade_kills: u32,
    own_frags: u32,
    team_frags: u32,
    engages: u32,
    clip_sum: f64,
}

impl Kit {
    fn add(&mut self, o: &Kit) {
        self.throws += o.throws;
        self.grenade_hits += o.grenade_hits;
        self.grenade_kills += o.grenade_kills;
        self.own_frags += o.own_frags;
        self.team_frags += o.team_frags;
        self.engages += o.engages;
        self.clip_sum += o.clip_sum;
    }

    fn line(&self) -> String {
        format!(
            "grenades thrown {:4} hits {:3} kills {:3} own {:2} team {:2} engagements {:5} clip at start {:3.0}%",
            self.throws,
            self.grenade_hits,
            self.grenade_kills,
            self.own_frags,
            self.team_frags,
            self.engages,
            100.0 * self.clip_sum / f64::from(self.engages.max(1)),
        )
    }

    /// One bot-tick: the throws since `life`'s last count, and an
    /// engagement on the tick tracking starts.
    fn step(life: &mut Life, body: Option<&vcod_server::bots::BotBody>) -> Kit {
        let mut k = Kit::default();
        let throws = body.map_or(life.throws, |b| b.throws);
        k.throws = throws.saturating_sub(life.throws);
        life.throws = throws;
        let engaged = body.is_some_and(|b| b.playing && b.engaged);
        if engaged
            && !life.engaged
            && let Some(b) = body.filter(|b| b.clip_size > 0 && b.clip >= 0)
        {
            k.engages = 1;
            k.clip_sum = f64::from(b.clip) / f64::from(b.clip_size);
        }
        life.engaged = engaged;
        k
    }
}

/// The aim columns (bot-combat.md section 1): rounds fired, bullet hits,
/// targets acquired with none tracked, and the ms from each to its first
/// round and its first bullet hit at that target.
#[derive(Default, Clone)]
struct Aim {
    shots: u64,
    hits: u64,
    acquired: u32,
    first_shot_ms: Vec<u64>,
    first_hit_ms: Vec<u64>,
}

impl Aim {
    fn add(&mut self, o: &Aim) {
        self.shots += o.shots;
        self.hits += o.hits;
        self.acquired += o.acquired;
        self.first_shot_ms.extend(&o.first_shot_ms);
        self.first_hit_ms.extend(&o.first_hit_ms);
    }

    fn line(&self) -> String {
        let spread = |v: &[u64]| {
            let mut t = v.to_vec();
            t.sort_unstable();
            let mean = t.iter().sum::<u64>() / t.len().max(1) as u64;
            let pct = 100.0 * t.len() as f32 / self.acquired.max(1) as f32;
            (mean, t.get(t.len() / 2).copied().unwrap_or(0), pct)
        };
        let (shot, shot_mid, shot_pct) = spread(&self.first_shot_ms);
        let (hit, hit_mid, hit_pct) = spread(&self.first_hit_ms);
        format!(
            "shots {:6} hits {:5} accuracy {:4.1}% sight to shot mean {:4} median {:4} ms, {:4.1}% of {} acquired fired on; sight to hit mean {:4} median {:4} ms, {:4.1}% hit",
            self.shots,
            self.hits,
            100.0 * self.hits as f32 / self.shots.max(1) as f32,
            shot,
            shot_mid,
            shot_pct,
            self.acquired,
            hit,
            hit_mid,
            hit_pct,
        )
    }
}

/// A means of death a bullet deals: what a hit counts.
fn bullet(mod_: &str) -> bool {
    matches!(
        mod_,
        "MOD_RIFLE_BULLET" | "MOD_PISTOL_BULLET" | "MOD_HEAD_SHOT"
    )
}

impl Tally {
    fn add(&mut self, o: &Tally) {
        self.kills += o.kills;
        self.deaths += o.deaths;
        self.suicides += o.suicides;
        self.team_kills += o.team_kills;
        self.damage += o.damage;
        self.ttk_ms.extend(&o.ttk_ms);
        self.life_ticks += o.life_ticks;
        self.lives += o.lives;
        self.wounds += o.wounds;
        self.wound_deaths += o.wound_deaths;
        self.hurt += o.hurt;
        self.dry += o.dry;
        self.hold += o.hold;
        self.lean += o.lean;
        self.aim.add(&o.aim);
        self.hits_taken += o.hits_taken;
        self.answered += o.answered;
        self.turned += o.turned;
        self.killed += o.killed;
        self.blindsided += o.blindsided;
        self.mate_ticks += o.mate_ticks;
        self.mate_dist += o.mate_dist;
        self.packed += o.packed;
        self.kit.add(&o.kit);
        self.peeks.add(&o.peeks);
        self.walk_ticks += o.walk_ticks;
        self.walk_turn += o.walk_turn;
        self.snaps += o.snaps;
        self.fights.add(&o.fights);
    }

    /// The walking view's line (bot-combat.md section 1).
    fn walk(&self) -> String {
        format!(
            "walking ticks {:7} turn mean {:5.2} deg snaps over {SNAP_DEG:.0} deg {:6} ({:5.2}%)",
            self.walk_ticks,
            self.walk_turn / self.walk_ticks.max(1) as f64,
            self.snaps,
            100.0 * self.snaps as f64 / self.walk_ticks.max(1) as f64,
        )
    }

    /// The per-bot-tick line under [`Tally::line`].
    fn survive(&self) -> String {
        format!(
            "life {:5.1} s wounds {:4} dead in 5 s {:3.0}% retreats hurt {:4} dry {:4} cover hold {:6} lean {:6} mate {:4.0} packed {:4.1}% turned {:3.0}% answered {:3.0}% of {:5} blindsided {:3.0}% of {:4}",
            self.life_ticks as f32 * TICK_MS as f32 / 1000.0 / self.lives.max(1) as f32,
            self.wounds,
            100.0 * self.wound_deaths as f32 / self.wounds.max(1) as f32,
            self.hurt,
            self.dry,
            self.hold,
            self.lean,
            self.mate_dist / self.mate_ticks.max(1) as f64,
            100.0 * self.packed as f64 / self.mate_ticks.max(1) as f64,
            100.0 * self.turned as f32 / self.hits_taken.max(1) as f32,
            100.0 * self.answered as f32 / self.hits_taken.max(1) as f32,
            self.hits_taken,
            100.0 * self.blindsided as f32 / self.killed.max(1) as f32,
            self.killed,
        )
    }

    fn line(&self) -> String {
        let mut t = self.ttk_ms.clone();
        t.sort_unstable();
        let mean = if t.is_empty() {
            0
        } else {
            t.iter().sum::<u64>() / t.len() as u64
        };
        let median = t.get(t.len() / 2).copied().unwrap_or(0);
        format!(
            "kills {:4} deaths {:4} k/d {:5.2} suicides {:3} teamkills {:3} damage {:6} ttk mean {:5} median {:5} ms",
            self.kills,
            self.deaths,
            self.kills as f32 / self.deaths.max(1) as f32,
            self.suicides,
            self.team_kills,
            self.damage,
            mean,
            median,
        )
    }
}

/// `TICKSTATS=1` (any game): each tick's wall time and its
/// `vcod_server::tick_profile` phases, printed per match to stderr as
/// `tickstats <map> <gametype> seed <s>: ticks N p50 p90 p99 max` and a
/// line per phase (mean, p50, p99 and max ms a tick, longest mean first). Tick 0,
/// which waits on the nav graph, is left out. Run with `JOBS=1`: matches
/// beside each other share the CPUs.
#[derive(Default)]
struct TickStats {
    ms: Vec<f32>,
    /// Each phase's time per counted tick, 0 where it did not run.
    phases: BTreeMap<&'static str, Vec<f32>>,
    /// The slowest tick: ms, tick, its phases.
    worst: (f32, usize, String),
}

impl TickStats {
    fn from_env() -> Option<TickStats> {
        (std::env::var("TICKSTATS").as_deref() == Ok("1")).then(|| {
            vcod_server::tick_profile::enable();
            TickStats::default()
        })
    }

    fn add(&mut self, tick: usize, ms: f32, phases: &[vcod_server::tick_profile::PhaseTime]) {
        if tick == 0 {
            return;
        }
        let n = self.ms.len();
        self.ms.push(ms);
        for (name, d, _) in phases {
            let v = self.phases.entry(name).or_default();
            v.resize(n, 0.0);
            v.push(d.as_secs_f32() * 1000.0);
        }
        if ms > self.worst.0 {
            self.worst = (ms, tick, vcod_server::tick_profile::format(phases, 1.0));
        }
    }

    fn pct(v: &[f32], p: f32) -> f32 {
        let mut s = v.to_vec();
        s.sort_by(f32::total_cmp);
        s.get(((s.len() as f32 - 1.0) * p).round() as usize)
            .copied()
            .unwrap_or(0.0)
    }

    fn print(&self, map: &str, gametype: &str, seed: u64) {
        let n = self.ms.len();
        eprintln!(
            "tickstats {map} {gametype} seed {seed}: ticks {n} p50 {:.2} p90 {:.2} p99 {:.2} max {:.1} ms (tick {}: {})",
            Self::pct(&self.ms, 0.5),
            Self::pct(&self.ms, 0.9),
            Self::pct(&self.ms, 0.99),
            self.worst.0,
            self.worst.1,
            self.worst.2,
        );
        let mut rows: Vec<(&str, f32, [f32; 3])> = self
            .phases
            .iter()
            .map(|(k, v)| {
                let mut v = v.clone();
                v.resize(n, 0.0);
                (
                    *k,
                    v.iter().sum::<f32>() / n.max(1) as f32,
                    [0.5, 0.99, 1.0].map(|p| Self::pct(&v, p)),
                )
            })
            .collect();
        rows.sort_by(|a, b| b.1.total_cmp(&a.1));
        for (k, mean, [p50, p99, max]) in rows {
            if mean >= 0.01 || p99 >= 0.1 {
                eprintln!(
                    "tickstats   {k:<28} mean {mean:6.3} p50 {p50:6.2} p99 {p99:6.2} max {max:6.1} ms"
                );
            }
        }
    }
}

/// `DIGEST=`: a running FNV-1a 64 over every bot's last run cmd and body,
/// folded after each tick in slot order (bot-combat.md section 1). `1`
/// prints the match's final value; `N` above 1 also prints it every `N`
/// ticks, so a diff of two runs names the first window that parted.
#[derive(Default, Clone)]
struct Digest {
    /// `digest` or `net`: the lines' first word.
    label: &'static str,
    hash: u64,
    every: usize,
    /// (tick, running hash) every `every` ticks.
    windows: Vec<(usize, u64)>,
}

impl Digest {
    fn from_env() -> Option<Digest> {
        Self::from_var("DIGEST", "digest")
    }

    /// `NETDIGEST=`'s: the datagrams sent.
    fn net_from_env() -> Option<Digest> {
        Self::from_var("NETDIGEST", "net")
    }

    fn from_var(var: &str, label: &'static str) -> Option<Digest> {
        let every: usize = std::env::var(var).ok()?.parse().expect(var);
        (every > 0).then_some(Digest {
            label,
            hash: 0xcbf2_9ce4_8422_2325,
            every,
            windows: Vec::new(),
        })
    }

    fn bytes(&mut self, b: &[u8]) {
        for &x in b {
            self.hash ^= u64::from(x);
            self.hash = self.hash.wrapping_mul(0x0000_0100_0000_01b3);
        }
    }

    fn i32s(&mut self, v: &[i32]) {
        for x in v {
            self.bytes(&x.to_le_bytes());
        }
    }

    /// Floats by their bits: a last-bit change shows.
    fn f32s(&mut self, v: &[f32]) {
        for x in v {
            self.bytes(&x.to_bits().to_le_bytes());
        }
    }

    /// Closes tick `n` (1-based).
    fn end_tick(&mut self, n: usize) {
        if self.every > 1 && n.is_multiple_of(self.every) {
            self.windows.push((n, self.hash));
        }
    }

    fn print(&self, map: &str, gametype: &str, seed: u64, ticks: usize) {
        let label = self.label;
        for (t, h) in &self.windows {
            println!("{label}@{t} {map} {gametype} seed {seed} {h:016x}");
        }
        println!(
            "{label} {map} {gametype} seed {seed} ticks {ticks} {:016x}",
            self.hash
        );
    }

    /// One tick's datagrams, in send order, then the tick closed.
    fn datagrams(&mut self, sent: &[(SocketAddr, Vec<u8>)], tick: usize) {
        for (to, p) in sent {
            self.bytes(to.to_string().as_bytes());
            self.bytes(&(p.len() as u32).to_le_bytes());
            self.bytes(p);
        }
        self.end_tick(tick + 1);
    }

    /// One CoD1 tick: each bot slot, then its last run cmd, origin,
    /// velocity, health and weapon (or a 0 byte with no body).
    fn cod1(&mut self, sv: &vcod_server::Server, tick: usize) {
        for slot in sv.bot_slots() {
            self.bytes(&(slot as u32).to_le_bytes());
            let Some((c, ps, health)) = sv.bot_digest_state(slot) else {
                self.bytes(&[0]);
                continue;
            };
            self.bytes(&[1]);
            self.i32s(&[c.server_time]);
            self.bytes(&[c.buttons, c.wbuttons, c.weapon, c.flags]);
            self.i32s(&c.angles);
            self.bytes(&[c.forward as u8, c.right as u8, c.up as u8]);
            self.f32s(&ps.origin.to_array());
            self.f32s(&ps.velocity.to_array());
            self.i32s(&[health]);
            self.bytes(&[ps.weapon]);
        }
        self.end_tick(tick + 1);
    }

    /// One CoD2 tick, as [`Digest::cod1`] over `slots` sorted: the
    /// player's `oldcmd` (the last cmd that ran) and playerstate.
    fn cod2(&mut self, sv: &vcod_server::cod2::Cod2Server, slots: &[usize], tick: usize) {
        let mut slots = slots.to_vec();
        slots.sort_unstable();
        for slot in slots {
            self.bytes(&(slot as u32).to_le_bytes());
            let level = sv.level();
            let Some(p) = level.and_then(|l| l.players.get(slot)?.as_ref()) else {
                self.bytes(&[0]);
                continue;
            };
            let c = p.oldcmd;
            self.bytes(&[1]);
            self.i32s(&[c.server_time, c.buttons]);
            self.bytes(&[c.weapon, c.off_hand_index]);
            self.i32s(&c.angles);
            self.bytes(&[c.forward_move as u8, c.right_move as u8]);
            self.f32s(&p.ps.origin.to_array());
            self.f32s(&p.ps.velocity.to_array());
            let health = level.map_or(0, |l| l.rt.client_vitals(slot).health);
            self.i32s(&[health]);
            self.bytes(&[p.ws.weapon]);
        }
        self.end_tick(tick + 1);
    }

    /// One CoD4 tick, as [`Digest::cod2`]: the player's `oldcmd` and
    /// playerstate.
    fn cod4(
        &mut self,
        level: &vcod_server::game::cod4::level::Cod4Level,
        slots: &[usize],
        tick: usize,
    ) {
        let mut slots = slots.to_vec();
        slots.sort_unstable();
        for slot in slots {
            self.bytes(&(slot as u32).to_le_bytes());
            let (Some(p), Some(ps)) = (
                level.players.get(slot).and_then(Option::as_ref),
                level.pm_state(slot),
            ) else {
                self.bytes(&[0]);
                continue;
            };
            let c = p.oldcmd;
            self.bytes(&[1]);
            self.i32s(&[c.server_time, c.buttons as i32]);
            self.bytes(&[c.weapon, c.offhand]);
            self.i32s(&c.angles.map(i32::from));
            self.bytes(&[c.forward_move as u8, c.right_move as u8]);
            self.f32s(&ps.origin.to_array());
            self.f32s(&ps.velocity.to_array());
            self.i32s(&[level.rt.client_vitals(slot).health]);
            self.i32s(&[ps.weapon as i32]);
        }
        self.end_tick(tick + 1);
    }
}

/// `NETDIGEST`'s observer: a non-LAN address, so the server paces it on
/// its rate and `snaps` as it would a remote client.
const OBSERVER: SocketAddr = SocketAddr::new(
    std::net::IpAddr::V4(std::net::Ipv4Addr::new(198, 51, 100, 7)),
    28961,
);

/// The two queues between [`Observer`]'s client and the server.
#[derive(Default)]
struct Pipe {
    to_server: VecDeque<Vec<u8>>,
    to_client: VecDeque<Vec<u8>>,
}

struct PipeEnd(Rc<std::cell::RefCell<Pipe>>);

impl vcod_common::net::Transport for PipeEnd {
    fn try_recv(&mut self, buf: &mut [u8]) -> Option<usize> {
        let p = self.0.borrow_mut().to_client.pop_front()?;
        buf[..p.len()].copy_from_slice(&p);
        Some(p.len())
    }
    fn send(&mut self, data: &[u8]) {
        self.0.borrow_mut().to_server.push_back(data.to_vec());
    }
}

/// A CoD1 spectator client that joins and stays in the team menu,
/// acknowledging every snapshot.
struct Observer {
    pipe: Rc<std::cell::RefCell<Pipe>>,
    cl: vcod_common::net::NetClient<PipeEnd>,
    addr: SocketAddr,
}

/// `OBSERVERS=`' count, 0 without it.
fn observers_from_env() -> usize {
    std::env::var("OBSERVERS").map_or(0, |v| v.parse().expect("OBSERVERS"))
}

/// The `i`th observer's address and qport: [`OBSERVER`] and 4660 for the
/// first, the next host and qport for each after it.
fn observer_addr(i: usize) -> (SocketAddr, u16) {
    let mut a = OBSERVER;
    a.set_ip(std::net::IpAddr::V4(std::net::Ipv4Addr::new(
        198,
        51,
        100,
        7 + i as u8,
    )));
    (a, 4660 + i as u16)
}

impl Observer {
    fn new(proto: &'static vcod_common::net::protocol::Protocol, now: Instant, i: usize) -> Self {
        let (addr, qport) = observer_addr(i);
        let pipe = Rc::new(std::cell::RefCell::new(Pipe::default()));
        let cl = vcod_common::net::NetClient::start_with_protocol(
            PipeEnd(pipe.clone()),
            now,
            qport,
            proto,
        );
        Observer { pipe, cl, addr }
    }

    /// The client's packets since the last tick, into the server.
    fn deliver(&mut self, sv: &mut vcod_server::Server, now: Instant) {
        let pending: Vec<Vec<u8>> = self.pipe.borrow_mut().to_server.drain(..).collect();
        for p in pending {
            sv.handle_packet(self.addr, &p, now);
        }
    }

    /// The tick's datagrams to the client, then its frame.
    fn receive(&mut self, sent: &[(SocketAddr, Vec<u8>)], now: Instant) {
        for (to, p) in sent {
            if *to == self.addr {
                self.pipe.borrow_mut().to_client.push_back(p.clone());
            }
        }
        self.cl.pump_at(now);
        self.cl
            .send_frame(&vcod_common::net::msg::UserCmd::default());
    }
}

/// `NETDUMP=<dir>`: this match's datagrams file.
fn net_dump(
    game: &str,
    map: &str,
    gametype: &str,
    seed: u64,
) -> Option<std::io::BufWriter<std::fs::File>> {
    let dir = std::env::var_os("NETDUMP")?;
    let path = std::path::Path::new(&dir).join(format!("{game}_{map}_{gametype}_s{seed}.net"));
    Some(std::io::BufWriter::new(
        std::fs::File::create(path).expect("NETDUMP"),
    ))
}

/// One tick's datagrams into a [`net_dump`] file.
fn dump_datagrams(
    w: &mut Option<std::io::BufWriter<std::fs::File>>,
    sent: &[(SocketAddr, Vec<u8>)],
    tick: usize,
) {
    use std::io::Write;
    let Some(w) = w.as_mut() else { return };
    for (to, p) in sent {
        let hex: String = p.iter().map(|b| format!("{b:02x}")).collect();
        writeln!(w, "{} {to} {} {hex}", tick + 1, p.len()).expect("NETDUMP");
    }
}

/// `VCOD_TICK_TRACE=<dir>`: this match's step log, on this thread.
fn install_trace(game: &str, map: &str, gametype: &str, seed: u64) {
    if let Some(dir) = std::env::var_os("VCOD_TICK_TRACE") {
        let path =
            std::path::Path::new(&dir).join(format!("{game}_{map}_{gametype}_s{seed}.trace"));
        vcod_server::tick_trace::install(&path).expect("VCOD_TICK_TRACE");
    }
}

/// A bot-tick is near its objective within this of the point the
/// objective centres on (`Bot::objective_point`).
const OBJ_NEAR: f32 = 800.0;
/// A body within this of where it stood `STILL_TICKS` before stands still.
const STILL_TICKS: usize = 40;
const STILL_RADIUS: f32 = 32.0;

/// A bot's objective point, exposure, cover hold and guarded point, as
/// [`ObjSample`] takes them.
type ObjInfo = (Option<[f32; 3]>, Option<f32>, bool, Option<[f32; 3]>);

/// One playing bot's tick for [`ObjSpread`]: slot, team, feet, its
/// objective's point, the share of that objective's ways in that see it,
/// on a held cover spot.
struct ObjSample {
    slot: usize,
    team: String,
    at: [f32; 3],
    point: Option<[f32; 3]>,
    exposure: Option<f32>,
    cover: bool,
    /// The point it guards, a guard only (`Bot::guard_point`).
    guard: Option<[f32; 3]>,
}

/// The playing bot-ticks near their objective (bot-objectives.md 13):
/// with a teammate playing, the sum of the nearest one's distance and
/// those with [`PACK`] or more inside [`PACK_RANGE`]; those standing still
/// ([`STILL_RADIUS`] over [`STILL_TICKS`]); the share of the objective's
/// ways in that see the body (`BotView::exposure`), over the ticks with a
/// sample; those on a cover spot.
#[derive(Default, Clone)]
struct ObjSpread {
    ticks: u64,
    mate_ticks: u64,
    mate: f64,
    packed: u64,
    still: u64,
    exposed_ticks: u64,
    exposure: f64,
    cover: u64,
    /// Guards' bot-ticks anywhere and their summed distance from the point
    /// they guard (bot-objectives.md 14).
    guard_ticks: u64,
    guard_dist: f64,
    /// Cues that pulled guards back, those a guard came back from, and the
    /// summed ms from cue to the first back (`Server::bot_cue_tally`).
    cues: u64,
    backs: u64,
    back_ms: u64,
}

impl ObjSpread {
    fn add(&mut self, o: &ObjSpread) {
        self.ticks += o.ticks;
        self.mate_ticks += o.mate_ticks;
        self.mate += o.mate;
        self.packed += o.packed;
        self.still += o.still;
        self.exposed_ticks += o.exposed_ticks;
        self.exposure += o.exposure;
        self.cover += o.cover;
        self.guard_ticks += o.guard_ticks;
        self.guard_dist += o.guard_dist;
        self.cues += o.cues;
        self.backs += o.backs;
        self.back_ms += o.back_ms;
    }

    /// A match's cue counts for one side, off the driver at its end.
    fn add_cues(out: &mut BTreeMap<String, ObjSpread>, tally: [(u32, u32, u64); 2]) {
        for (team, (cues, back, ms)) in ["axis", "allies"].into_iter().zip(tally) {
            if cues > 0 {
                let o = out.entry(team.to_string()).or_default();
                o.cues += u64::from(cues);
                o.backs += u64::from(back);
                o.back_ms += ms;
            }
        }
    }

    /// One tick of every playing bot into `out` by team; `trail` keeps
    /// each one's feet over the last [`STILL_TICKS`].
    fn feed(
        out: &mut BTreeMap<String, ObjSpread>,
        samples: &[ObjSample],
        trail: &mut HashMap<usize, VecDeque<[f32; 3]>>,
    ) {
        trail.retain(|s, _| samples.iter().any(|x| x.slot == *s));
        for x in samples {
            let t = trail.entry(x.slot).or_default();
            t.push_back(x.at);
            if t.len() > STILL_TICKS + 1 {
                t.pop_front();
            }
        }
        let dist = |a: [f32; 3], b: [f32; 3]| glam::Vec3::from(a).distance(glam::Vec3::from(b));
        for x in samples {
            if let Some(g) = x.guard {
                let o = out.entry(x.team.clone()).or_default();
                o.guard_ticks += 1;
                o.guard_dist += f64::from(dist(x.at, g));
            }
            if !x.point.is_some_and(|p| dist(x.at, p) < OBJ_NEAR) {
                continue;
            }
            let o = out.entry(x.team.clone()).or_default();
            o.ticks += 1;
            let d: Vec<f32> = samples
                .iter()
                .filter(|o| o.slot != x.slot && o.team == x.team)
                .map(|o| dist(o.at, x.at))
                .collect();
            if let Some(near) = d.iter().copied().reduce(f32::min) {
                o.mate_ticks += 1;
                o.mate += f64::from(near);
                o.packed += u64::from(d.iter().filter(|&&v| v < PACK_RANGE).count() >= PACK);
            }
            let t = &trail[&x.slot];
            o.still += u64::from(t.len() == STILL_TICKS + 1 && dist(t[0], x.at) < STILL_RADIUS);
            if let Some(e) = x.exposure {
                o.exposed_ticks += 1;
                o.exposure += f64::from(e);
            }
            o.cover += u64::from(x.cover);
        }
    }

    fn line(&self) -> String {
        let share = |n: u64, of: u64| 100.0 * n as f64 / of.max(1) as f64;
        format!(
            "near objective {:8} bot-ticks mate {:4.0} packed {:4.1}% still {:4.1}% exposed {:4.1}% cover hold {:4.1}% guard dist {:4.0} cues {:4} back {:4} in {:4.1} s",
            self.ticks,
            self.mate / self.mate_ticks.max(1) as f64,
            share(self.packed, self.mate_ticks),
            share(self.still, self.ticks),
            100.0 * self.exposure / self.exposed_ticks.max(1) as f64,
            share(self.cover, self.ticks),
            self.guard_dist / self.guard_ticks.max(1) as f64,
            self.cues,
            self.backs,
            self.back_ms as f64 / 1000.0 / self.backs.max(1) as f64,
        )
    }
}

#[derive(Default, Clone)]
struct Match {
    net: Option<Digest>,
    map: String,
    seed: u64,
    ticks: usize,
    /// Whether the map ended inside `TICKS`.
    ended: bool,
    /// `W;` lines by winning team (or, in `dm`, player name).
    wins: BTreeMap<String, u32>,
    /// The tick of each `W;` line: when each round ended (`bas` logs it
    /// after `scr_bas_endrounddelay`).
    win_ticks: Vec<usize>,
    /// [axis, allies] when the match ended or ran out.
    scores: [i32; 2],
    by_team: BTreeMap<String, Tally>,
    by_skill: BTreeMap<String, Tally>,
    by_arm: BTreeMap<String, Tally>,
    /// Wins and actions by arm, keyed as `wins` and `actions` are.
    arm_wins: BTreeMap<String, u32>,
    arm_actions: BTreeMap<String, u32>,
    actions: BTreeMap<String, u32>,
    /// 12 s stall ticks with no hold, and those holding on purpose.
    stalled: u32,
    held: u32,
    /// Edges the followers gave up on, and those by a body
    /// (`Server::bot_edge_stalls`).
    edges: [u32; 2],
    /// Per round with an objective action, the ticks from the round's
    /// start (the match's, or the last `W;` line) to its first `A;` line.
    to_objective: Vec<usize>,
    /// Near the objective, by team ([`ObjSpread`]).
    obj: BTreeMap<String, ObjSpread>,
    log: Vec<String>,
    dm: bool,
    digest: Option<Digest>,
}

fn main() {
    let env = |k: &str, d: &str| std::env::var(k).unwrap_or_else(|_| d.into());
    if env("GAME", "cod1") == "cod2" {
        return cod2::main();
    }
    if env("GAME", "cod1") == "cod4" {
        return cod4::main();
    }
    let uo = env("UO", "0") == "1";
    let gametype = env("GAMETYPE", "tdm");
    let mut bots: usize = env("BOTS", "6").parse().expect("BOTS");
    let mut console = Vec::new();
    let side_skill = (std::env::var("AXIS_SKILL"), std::env::var("ALLIES_SKILL"));
    if let Ok(list) = std::env::var("SKILLS") {
        let list: Vec<&str> = list.split(',').collect();
        for s in &list {
            console.push(format!("addbot 1 skill {s}"));
        }
        bots = list.len();
    } else if side_skill.0.is_ok() || side_skill.1.is_ok() {
        let axis = side_skill.0.unwrap_or_else(|_| "1".into());
        let allies = side_skill.1.unwrap_or_else(|_| "1".into());
        console.push(format!("addbot axis {} skill {axis}", bots - bots / 2));
        console.push(format!("addbot allies {} skill {allies}", bots / 2));
    }
    let opts = Opts {
        maps: env("MAPS", &env("MAP", "mp_carentan"))
            .split(',')
            .map(str::to_string)
            .collect(),
        gametype,
        bots,
        ticks: env("TICKS", "12000").parse().expect("TICKS"),
        uo,
        set: env("SET", "")
            .split(',')
            .filter(|s| !s.is_empty())
            .map(|kv| {
                let (k, v) = kv.split_once('=').expect("SET name=value");
                (k.to_string(), v.to_string())
            })
            .collect(),
        console,
        verbose: env("VERBOSE", "0") == "1",
        arms: ARMS.map(Arm::parse),
    };
    let jobs: usize = env("JOBS", "4").parse().expect("JOBS");
    let mut seeds: Vec<u64> = std::env::args()
        .skip(1)
        .map(|s| s.parse().expect("seed"))
        .collect();
    if seeds.is_empty() {
        seeds = (1..=8).collect();
    }
    println!(
        "maps {} gametype {} bots {} ticks {} uo {} set {:?} console {:?} cover/retreat/teamplay/aware/footwork/wary/squad/aim/tactics/converge/lanes {:?}",
        opts.maps.join(","),
        opts.gametype,
        opts.bots,
        opts.ticks,
        opts.uo,
        opts.set,
        opts.console,
        ARMS.map(|k| std::env::var(k).unwrap_or_else(|_| "all".into())),
    );
    let started = Instant::now();
    let jobs_list: Vec<(String, u64)> = opts
        .maps
        .iter()
        .rev()
        .flat_map(|m| seeds.iter().rev().map(move |s| (m.clone(), *s)))
        .collect();
    let queue = std::sync::Mutex::new(jobs_list);
    let results = std::sync::Mutex::new(Vec::new());
    std::thread::scope(|s| {
        for _ in 0..jobs.max(1) {
            s.spawn(|| {
                loop {
                    let Some((map, seed)) = queue.lock().unwrap().pop() else {
                        break;
                    };
                    let m = run(&opts, &map, seed);
                    vcod_server::tick_trace::finish().expect("VCOD_TICK_TRACE");
                    let Some(m) = m else {
                        return;
                    };
                    eprintln!(
                        "{map} seed {seed} done ({:.0} s)",
                        started.elapsed().as_secs_f32()
                    );
                    results.lock().unwrap().push(m);
                }
            });
        }
    });
    let mut results = results.into_inner().unwrap();
    if results.is_empty() {
        eprintln!("no game fs (set COD_DIR)");
        return;
    }
    results.sort_by(|a, b| (&a.map, a.seed).cmp(&(&b.map, b.seed)));
    for m in &results {
        for l in &m.log {
            println!("  {} seed {} {l}", m.map, m.seed);
        }
        let all = m.by_team.values().fold(Tally::default(), |mut a, t| {
            a.add(t);
            a
        });
        println!(
            "{} seed {}: ticks {} ended {} wins {:?} at {:?} score axis {} allies {} stalled {} held {} edges {} by a body {} {} actions {:?} to objective {}",
            m.map,
            m.seed,
            m.ticks,
            m.ended,
            m.wins,
            m.win_ticks,
            m.scores[0],
            m.scores[1],
            m.stalled,
            m.held,
            m.edges[0],
            m.edges[1],
            all.line(),
            m.actions,
            to_objective(&m.to_objective),
        );
        for (k, o) in &m.obj {
            println!("{} seed {}: obj {k} {}", m.map, m.seed, o.line());
        }
        if m.by_arm.len() > 1 {
            let k: BTreeMap<&str, u32> = m
                .by_arm
                .iter()
                .map(|(k, t)| (k.as_str(), t.kills))
                .collect();
            println!("{} seed {}: arm kills {k:?}", m.map, m.seed);
        }
        if let Some(d) = &m.digest {
            d.print(&m.map, &opts.gametype, m.seed, m.ticks);
        }
        if let Some(d) = &m.net {
            d.print(&m.map, &opts.gametype, m.seed, m.ticks);
        }
    }
    if opts.maps.len() > 1 {
        for map in &opts.maps {
            let of: Vec<&Match> = results.iter().filter(|m| &m.map == map).collect();
            summarize(map, &of);
        }
    }
    summarize("total", &results.iter().collect::<Vec<_>>());
}

/// Totals over `results`: per team, skill and arm, wins, scores, actions.
fn summarize(what: &str, results: &[&Match]) {
    let mut by_team: BTreeMap<String, Tally> = BTreeMap::new();
    let mut by_skill: BTreeMap<String, Tally> = BTreeMap::new();
    let mut by_arm: BTreeMap<String, Tally> = BTreeMap::new();
    let mut actions: BTreeMap<String, u32> = BTreeMap::new();
    let mut wins: BTreeMap<String, u32> = BTreeMap::new();
    let mut arm_wins: BTreeMap<String, u32> = BTreeMap::new();
    let mut arm_actions: BTreeMap<String, u32> = BTreeMap::new();
    let (mut stalled, mut held, mut ended, mut scores) = (0, 0, 0, [0i64; 2]);
    let mut edges = [0u32; 2];
    let mut to_obj: Vec<usize> = Vec::new();
    let mut obj: BTreeMap<String, ObjSpread> = BTreeMap::new();
    for m in results {
        for (k, t) in &m.obj {
            obj.entry(k.clone()).or_default().add(t);
            obj.entry("all".into()).or_default().add(t);
        }
        for (k, t) in &m.by_team {
            by_team.entry(k.clone()).or_default().add(t);
        }
        for (k, t) in &m.by_skill {
            by_skill.entry(k.clone()).or_default().add(t);
        }
        for (k, t) in &m.by_arm {
            by_arm.entry(k.clone()).or_default().add(t);
        }
        for (k, n) in &m.actions {
            *actions.entry(k.clone()).or_default() += n;
        }
        for (k, n) in &m.wins {
            *wins.entry(k.clone()).or_default() += n;
        }
        for (k, n) in &m.arm_wins {
            *arm_wins.entry(k.clone()).or_default() += n;
        }
        for (k, n) in &m.arm_actions {
            *arm_actions.entry(k.clone()).or_default() += n;
        }
        ended += u32::from(m.ended);
        stalled += m.stalled;
        held += m.held;
        edges[0] += m.edges[0];
        edges[1] += m.edges[1];
        to_obj.extend(&m.to_objective);
        scores[0] += i64::from(m.scores[0]);
        scores[1] += i64::from(m.scores[1]);
    }
    println!("{what} over {} matches:", results.len());
    let rows = |kind: &str, map: &BTreeMap<String, Tally>| {
        for (k, t) in map {
            println!("  {kind} {k:<13} {}", t.line());
            println!("  {kind} {k:<13} {}", t.survive());
            println!("  {kind} {k:<13} {}", t.aim.line());
            println!("  {kind} {k:<13} {}", t.kit.line());
            println!("  {kind} {k:<13} {}", t.peeks.line());
            println!("  {kind} {k:<13} {}", t.walk());
            println!("  {kind} {k:<13} {}", t.fights.line());
        }
    };
    rows("team ", &by_team);
    rows("skill", &by_skill);
    if by_arm.len() > 1 {
        rows("arm  ", &by_arm);
        println!("  arm wins {arm_wins:?}  arm actions {arm_actions:?}");
    }
    for (k, t) in &obj {
        println!("  obj   {k:<13} {}", t.line());
    }
    println!("  ended {ended}  wins {wins:?}");
    println!(
        "  score axis {} allies {}  actions {actions:?}  stalled {stalled} held {held}  edges {} by a body {}  to objective {}",
        scores[0],
        scores[1],
        edges[0],
        edges[1],
        to_objective(&to_obj)
    );
}

/// The mean seconds to a round's first objective action, and the rounds.
fn to_objective(ticks: &[usize]) -> String {
    let mean = ticks.iter().sum::<usize>() as f64 / ticks.len().max(1) as f64;
    format!(
        "{:.1} s ({} rounds)",
        mean * TICK_MS as f64 / 1000.0,
        ticks.len()
    )
}

/// One seeded match; `None` without game data.
fn run(o: &Opts, map: &str, seed: u64) -> Option<Match> {
    let fs = if o.uo {
        vcod_common::testing::uo_fs()
    } else {
        vcod_common::testing::game_fs()
    }?;
    let fs = Rc::new(fs);
    let bsp = vcod_common::mapworld::find(&*fs, map).unwrap();
    let mut now = Instant::now();
    let manual = o.console.is_empty();
    let cfg = vcod_server::ServerConfig {
        map: map.to_string(),
        hostname: "match".into(),
        max_clients: (o.bots + 2 + observers_from_env()).max(8),
        gametype: o.gametype.clone(),
        test_entities: 0,
        trace: false,
        bots: if manual { o.bots } else { 0 },
        bots_shoot: true,
    };
    let mut sv = vcod_server::Server::new(cfg, now);
    if o.uo {
        sv.set_protocol(&vcod_common::net::protocol::PROTOCOL_V22);
    }
    // `test_seed_rng` sets the low bit: 2 and 3 would replay one match.
    sv.test_seed_rng(seed * 2 + 1);
    for (k, v) in &o.set {
        sv.set_cvar(k, v);
    }
    sv.load_world(vcod_server::world::World::from_bsp(&bsp, Some(&fs)));
    sv.load_scripts(fs.clone()).expect("load the scripts");
    for line in &o.console {
        sv.push_console(line);
    }

    let mut m = Match {
        map: map.to_string(),
        seed,
        dm: o.gametype == "dm",
        digest: Digest::from_env(),
        net: Digest::net_from_env(),
        ..Default::default()
    };
    let game = if o.uo { "uo" } else { "11" };
    install_trace(game, map, &o.gametype, seed);
    let mut observer = m
        .net
        .is_some()
        .then(|| Observer::new(sv.protocol(), now, 0));
    let mut crowd: Vec<Observer> = (1..=observers_from_env())
        .map(|i| Observer::new(sv.protocol(), now, i))
        .collect();
    let mut dump = net_dump(game, map, &o.gametype, seed);
    let mut seen = 0;
    let mut level = sv.server_id();
    // The tick the round began and whether it has had an `A;` line yet.
    let (mut round_start, mut acted) = (0, false);
    // (attacker, victim) -> tick of the attacker's first hit this life.
    let mut first_hit: HashMap<(String, String), usize> = HashMap::new();
    // `re` carriers by client num, from their `re_pickup` to their capture,
    // death or the round's end, and the tick of the pickup: a death counts
    // `<team>:re_carrier_down` and a `re_down_<s>` bucket by seconds carried.
    // `re_died_near_goal` counts deaths within `NEAR_GOAL` of a goal,
    // `re_died_near_pickup` within it of where he took it.
    let mut carriers: HashMap<String, (usize, [f32; 3], String)> = HashMap::new();
    // Between a round's `W;` and the restart: a pickup then, after the
    // round, counts as `re_late_pickup` and starts no carry.
    let mut round_over = false;
    // Each bot's last 12 s: where it stood and whether it held still on
    // purpose there.
    let mut hist: HashMap<usize, VecDeque<([f32; 3], bool)>> = HashMap::new();
    let mut trail: HashMap<usize, VecDeque<[f32; 3]>> = HashMap::new();
    // `STALLS=1`: where each stalled window begins, to stderr.
    let stall_log = std::env::var("STALLS").as_deref() == Ok("1");
    let mut stalled_now: HashSet<usize> = HashSet::new();
    let mut bots: HashMap<usize, Life> = HashMap::new();
    let mut react = React::default();
    let mut stats = TickStats::from_env();
    for tick in 0..o.ticks {
        now += Duration::from_millis(TICK_MS);
        // A default arm never calls its hook, so an all-on run is the
        // stock game.
        for slot in sv.bot_slots() {
            let team = arm_side(&slot.to_string(), team_name(&mut sv, slot, m.dm), m.dm);
            let [
                cover,
                retreat,
                team_play,
                aware,
                footwork,
                wary,
                squad,
                aim,
                tactics,
                converge,
                lanes,
                far,
                cues,
                travel,
            ] = o.arms;
            if cover != Arm::All {
                sv.test_bot_cover(slot, cover.on(team, seed));
            }
            if retreat != Arm::All {
                sv.test_bot_retreat(slot, retreat.on(team, seed));
            }
            if team_play != Arm::All {
                sv.test_bot_team_play(slot, team_play.on(team, seed));
            }
            if aware != Arm::All {
                sv.test_bot_aware(slot, aware.on(team, seed));
            }
            if footwork != Arm::All {
                sv.test_bot_footwork(slot, footwork.on(team, seed));
            }
            if wary != Arm::All {
                sv.test_bot_wary(slot, wary.on(team, seed));
            }
            if squad != Arm::All {
                sv.test_bot_squad(slot, squad.on(team, seed));
            }
            if aim != Arm::All {
                sv.test_bot_human_aim(slot, aim.on(team, seed));
            }
            if tactics != Arm::All {
                sv.test_bot_tactics(slot, tactics.on(team, seed));
            }
            if converge != Arm::All {
                sv.test_bot_converge(slot, converge.on(team, seed));
            }
            if lanes != Arm::All {
                sv.test_bot_lanes(slot, lanes.on(team, seed));
            }
            if far != Arm::All {
                sv.test_bot_far(slot, far.on(team, seed));
            }
            if cues != Arm::All {
                sv.test_bot_cues(slot, cues.on(team, seed));
            }
            if travel != Arm::All {
                sv.test_bot_travel(slot, travel.on(team, seed));
            }
        }
        if let Some(ob) = observer.as_mut() {
            ob.deliver(&mut sv, now);
        }
        for ob in &mut crowd {
            ob.deliver(&mut sv, now);
        }
        let t0 = Instant::now();
        sv.tick(now);
        if let Some(st) = stats.as_mut() {
            let ms = t0.elapsed().as_secs_f32() * 1000.0;
            st.add(tick, ms, &vcod_server::tick_profile::take());
        }
        m.ticks = tick + 1;
        if let Some(d) = m.digest.as_mut() {
            d.cod1(&sv, tick);
        }
        if let (Some(ob), Some(d)) = (observer.as_mut(), m.net.as_mut()) {
            let sent = sv.take_outgoing();
            d.datagrams(&sent, tick);
            dump_datagrams(&mut dump, &sent, tick);
            ob.receive(&sent, now);
            for ob in &mut crowd {
                ob.receive(&sent, now);
            }
        } else if !crowd.is_empty() {
            let sent = sv.take_outgoing();
            for ob in &mut crowd {
                ob.receive(&sent, now);
            }
        }
        if sv.server_id() != level {
            // A restart (sd's rounds) keeps the log; every body respawns.
            level = sv.server_id();
            if o.verbose {
                m.log.push(format!("t{tick} server id {level}"));
            }
            first_hit.clear();
            carriers.clear();
            round_over = false;
        }
        seen = seen.min(sv.script_log().len());
        let skill: HashMap<String, String> = sv
            .bot_slots()
            .into_iter()
            .map(|s| (s.to_string(), skill_of(&sv, s)))
            .collect();
        let lines: Vec<String> = sv.script_log().get(seen..).unwrap_or_default().to_vec();
        seen += lines.len();
        react.watch(&mut sv, m.dm, tick, |num, team, f| {
            let t = tally(&mut m, o, num, team, &skill);
            f(t.0);
            f(t.1);
            f(t.2);
        });
        for l in &lines {
            let f: Vec<&str> = l.trim_end().split(';').collect();
            if o.verbose && (f[0] == "A" || f[0] == "W") {
                m.log.push(format!("t{tick} {}", l.trim_end()));
            }
            match f[0] {
                // victim num, team, name, attacker num, team, name, weapon,
                // damage, means of death, hit location; UO adds a guid
                // before each num.
                "D" if f.len() >= 11 => {
                    let g = usize::from(f.len() >= 13);
                    let (vnum, anum, ateam) = (f[1 + g], f[4 + 2 * g], f[5 + 2 * g]);
                    if anum == "-1" {
                        continue;
                    }
                    let att = tally(&mut m, o, anum, ateam, &skill);
                    let dmg: u64 = f[8 + 2 * g].parse().unwrap_or(0);
                    if anum != vnum {
                        let hit = u64::from(bullet(f[9 + 2 * g]));
                        att.0.damage += dmg;
                        att.1.damage += dmg;
                        att.2.damage += dmg;
                        att.0.aim.hits += hit;
                        att.1.aim.hits += hit;
                        att.2.aim.hits += hit;
                        if let Some(ms) = sight_to_hit(&mut bots, anum, vnum, hit) {
                            att.0.aim.first_hit_ms.push(ms);
                            att.1.aim.first_hit_ms.push(ms);
                            att.2.aim.first_hit_ms.push(ms);
                        }
                        first_hit
                            .entry((anum.to_string(), vnum.to_string()))
                            .or_insert(tick);
                        if f[9 + 2 * g] == "MOD_GRENADE_SPLASH" {
                            att.0.kit.grenade_hits += 1;
                            att.1.kit.grenade_hits += 1;
                            att.2.kit.grenade_hits += 1;
                        }
                        let vteam = f[2 + g];
                        if m.dm || ateam.is_empty() || ateam != vteam {
                            react.hit(vnum, anum, tick, |t| {
                                let v = tally(&mut m, o, vnum, vteam, &skill);
                                v.0.hits_taken += t;
                                v.1.hits_taken += t;
                                v.2.hits_taken += t;
                            });
                        }
                    }
                }
                "K" if f.len() >= 11 => {
                    let g = usize::from(f.len() >= 13);
                    let (vnum, vteam) = (f[1 + g], f[2 + g]);
                    let (anum, ateam) = (f[4 + 2 * g], f[5 + 2 * g]);
                    if o.verbose {
                        m.log.push(format!("t{tick} kill {}", f[1..].join(";")));
                    }
                    if let Some((t, took, _)) = carriers.remove(vnum) {
                        let s = (tick - t) as u64 * TICK_MS / 1000;
                        let at = vnum
                            .parse()
                            .ok()
                            .and_then(|n| sv.bot_body(n))
                            .map(|b| b.origin);
                        let near = |p: [f32; 3]| {
                            at.is_some_and(|a| glam::Vec3::from(a).distance(p.into()) < NEAR_GOAL)
                        };
                        let place = if sv.re_goals().into_iter().any(near) {
                            Some("goal")
                        } else if near(took) {
                            Some("pickup")
                        } else {
                            None
                        };
                        if let Some(place) = place {
                            *m.actions
                                .entry(format!("{vteam}:re_died_near_{place}"))
                                .or_default() += 1;
                        }
                        let bucket = match s {
                            0..5 => "0-5s",
                            5..15 => "5-15s",
                            15..30 => "15-30s",
                            _ => "30s+",
                        };
                        *m.actions
                            .entry(format!("{vteam}:re_carrier_down"))
                            .or_default() += 1;
                        *m.actions
                            .entry(format!("{vteam}:re_down_{bucket}"))
                            .or_default() += 1;
                    }
                    let v = tally(&mut m, o, vnum, vteam, &skill);
                    v.0.deaths += 1;
                    v.1.deaths += 1;
                    v.2.deaths += 1;
                    // The lethal hit logs `K` alone: a one-hit kill takes 0.
                    let ttk = first_hit
                        .remove(&(anum.to_string(), vnum.to_string()))
                        .map_or(0, |t| (tick - t) as u64 * TICK_MS);
                    let dmg: u64 = f[8 + 2 * g].parse().unwrap_or(0);
                    first_hit.retain(|(_, victim), _| victim != vnum);
                    if anum == vnum || anum == "-1" {
                        let v = tally(&mut m, o, vnum, vteam, &skill);
                        v.0.suicides += 1;
                        v.1.suicides += 1;
                        v.2.suicides += 1;
                        if anum == vnum && f[9 + 2 * g] == "MOD_GRENADE_SPLASH" {
                            v.0.kit.own_frags += 1;
                            v.1.kit.own_frags += 1;
                            v.2.kit.own_frags += 1;
                        }
                        continue;
                    }
                    let a = tally(&mut m, o, anum, ateam, &skill);
                    let hit = u64::from(bullet(f[9 + 2 * g]));
                    a.0.aim.hits += hit;
                    a.1.aim.hits += hit;
                    a.2.aim.hits += hit;
                    if let Some(ms) = sight_to_hit(&mut bots, anum, vnum, hit) {
                        a.0.aim.first_hit_ms.push(ms);
                        a.1.aim.first_hit_ms.push(ms);
                        a.2.aim.first_hit_ms.push(ms);
                    }
                    if !ateam.is_empty() && ateam == vteam && o.gametype != "dm" {
                        a.0.team_kills += 1;
                        a.1.team_kills += 1;
                        a.2.team_kills += 1;
                        if f[9 + 2 * g] == "MOD_GRENADE_SPLASH" {
                            a.0.kit.team_frags += 1;
                            a.1.kit.team_frags += 1;
                            a.2.kit.team_frags += 1;
                        }
                    } else {
                        let blind = react.killed(vnum, anum);
                        if let Some(blind) = blind {
                            let v = tally(&mut m, o, vnum, vteam, &skill);
                            for t in [v.0, v.1, v.2] {
                                t.killed += 1;
                                t.blindsided += u32::from(blind);
                            }
                        }
                        let a = tally(&mut m, o, anum, ateam, &skill);
                        a.0.kills += 1;
                        a.1.kills += 1;
                        a.2.kills += 1;
                        a.0.damage += dmg;
                        a.1.damage += dmg;
                        a.2.damage += dmg;
                        a.0.ttk_ms.push(ttk);
                        a.1.ttk_ms.push(ttk);
                        a.2.ttk_ms.push(ttk);
                        if f[9 + 2 * g] == "MOD_GRENADE_SPLASH" {
                            a.0.kit.grenade_hits += 1;
                            a.1.kit.grenade_hits += 1;
                            a.2.kit.grenade_hits += 1;
                            a.0.kit.grenade_kills += 1;
                            a.1.kit.grenade_kills += 1;
                            a.2.kit.grenade_kills += 1;
                        }
                    }
                }
                // [guid,] num, team, name, action
                "A" if f.len() >= 5 => {
                    let n = f.len();
                    if !std::mem::replace(&mut acted, true) {
                        m.to_objective.push(tick - round_start);
                    }
                    match f[n - 1] {
                        "re_pickup" if round_over => {
                            *m.actions
                                .entry(format!("{}:re_late_pickup", f[n - 3]))
                                .or_default() += 1;
                        }
                        "re_pickup" => {
                            let at = f[n - 4]
                                .parse()
                                .ok()
                                .and_then(|n| sv.bot_body(n))
                                .map_or([0.0; 3], |b| b.origin);
                            carriers.insert(f[n - 4].to_string(), (tick, at, f[n - 3].to_string()));
                        }
                        "re_capture" => {
                            carriers.remove(f[n - 4]);
                        }
                        _ => {}
                    }
                    *m.actions
                        .entry(format!("{}:{}", f[n - 3], f[n - 1]))
                        .or_default() += 1;
                    let arm = o.arm(f[n - 3], seed);
                    *m.arm_actions
                        .entry(format!("{arm}:{}", f[n - 1]))
                        .or_default() += 1;
                }
                "W" => {
                    round_over = true;
                    // Carries the round ended: `re_carry_at_end`, and
                    // `re_end_near_goal` those within `NEAR_GOAL` of one.
                    let goals = if carriers.is_empty() {
                        Vec::new()
                    } else {
                        sv.re_goals()
                    };
                    for (num, (_, _, team)) in carriers.drain() {
                        let at = num.parse().ok().and_then(|n| sv.bot_body(n));
                        *m.actions
                            .entry(format!("{team}:re_carry_at_end"))
                            .or_default() += 1;
                        if at.is_some_and(|b| {
                            goals.iter().any(|g| {
                                glam::Vec3::from(b.origin).distance((*g).into()) < NEAR_GOAL
                            })
                        }) {
                            *m.actions
                                .entry(format!("{team}:re_end_near_goal"))
                                .or_default() += 1;
                        }
                    }
                    let who = match f.get(1) {
                        Some(t) if !t.is_empty() => t.to_string(),
                        _ => f.last().unwrap_or(&"").to_string(),
                    };
                    *m.arm_wins.entry(o.arm(&who, seed)).or_default() += 1;
                    *m.wins.entry(who).or_default() += 1;
                    m.win_ticks.push(tick);
                    (round_start, acted) = (tick, false);
                }
                // `exitLevel`'s own line; it has zeroed the team scores.
                "ExitLevel: executed" => m.ended = true,
                _ => {}
            }
        }
        if m.ended {
            break;
        }
        m.scores = sv.team_scores();
        // Bots dead or off the field this tick: a fight on one ends with
        // its target down.
        let down: HashSet<usize> = sv
            .bot_slots()
            .into_iter()
            .filter(|&s| sv.bot_body(s).is_none_or(|b| !b.playing || b.dead))
            .collect();
        for slot in sv.bot_slots() {
            let team = team_name(&mut sv, slot, m.dm);
            // Still picking a team.
            if team == "none" && !m.dm {
                continue;
            }
            let num = slot.to_string();
            let body = sv.bot_body(slot);
            let cover = sv.bot_cover_state(slot);
            let tactics = sv.bot_tactics(slot);
            let life = bots.entry(slot).or_default();
            let t = tally(&mut m, o, &num, team, &skill);
            life.step(body.as_ref(), cover, tactics, tick, &down, t);
        }
        // Each playing bot's distance to its nearest playing teammate, and
        // how many stand inside PACK_RANGE.
        let playing: Vec<(usize, [f32; 3])> = sv
            .bot_slots()
            .into_iter()
            .filter_map(|s| Some((s, sv.bot_body(s).filter(|b| b.playing && !b.dead)?.origin)))
            .collect();
        let playing: Vec<(usize, &'static str, [f32; 3])> = playing
            .into_iter()
            .map(|(s, p)| (s, team_name(&mut sv, s, m.dm), p))
            .filter(|(_, t, _)| *t != "none")
            .collect();
        for &(slot, team, at) in &playing {
            let d: Vec<f32> = playing
                .iter()
                .filter(|(s, t, _)| *s != slot && *t == team)
                .map(|(_, _, p)| (0..3).map(|i| (p[i] - at[i]).powi(2)).sum::<f32>().sqrt())
                .collect();
            let Some(near) = d.iter().copied().reduce(f32::min) else {
                continue;
            };
            let close = d.iter().filter(|&&x| x < PACK_RANGE).count();
            let t = tally(&mut m, o, &slot.to_string(), team, &skill);
            for t in [t.0, t.1, t.2] {
                t.mate_ticks += 1;
                t.mate_dist += f64::from(near);
                t.packed += u64::from(close >= PACK);
            }
        }
        let samples: Vec<ObjSample> = sv
            .bot_slots()
            .into_iter()
            .filter_map(|s| {
                let b = sv.bot_body(s).filter(|b| b.playing && !b.dead)?;
                let team = team_name(&mut sv, s, m.dm);
                (team != "none").then(|| ObjSample {
                    slot: s,
                    team: team.to_string(),
                    at: b.origin,
                    point: b.objective,
                    exposure: b.exposure,
                    cover: b.holds_cover,
                    guard: b.guard,
                })
            })
            .collect();
        ObjSpread::feed(&mut m.obj, &samples, &mut trail);
        for slot in sv.bot_slots() {
            let Some(b) = sv.bot_body(slot).filter(|b| b.playing && !b.deployed) else {
                hist.remove(&slot);
                continue;
            };
            let h = hist.entry(slot).or_default();
            h.push_back((b.origin, b.holds()));
            if h.len() > WINDOW {
                h.pop_front();
            }
            let first = h[0].0;
            if h.len() == WINDOW
                && h.iter().all(|(p, _)| {
                    (0..3).map(|i| (p[i] - first[i]).powi(2)).sum::<f32>() < RADIUS * RADIUS
                })
            {
                // A window a hold left behind is the hold's: a fight's
                // spot is still within 150 units for up to 12 s after it.
                if h.iter().any(|(_, held)| *held) {
                    m.held += 1;
                    stalled_now.remove(&slot);
                } else {
                    m.stalled += 1;
                    if stall_log && stalled_now.insert(slot) {
                        let (wp, next) = sv.bot_waypoints(slot);
                        eprintln!(
                            "{map} seed {seed} t{tick} bot {slot} stalled at {:?} (12 s at {:?}) lad {} wp {:?} next {:?}",
                            b.origin.map(|x| x.round()),
                            first.map(|x| x.round()),
                            b.on_ladder as u8,
                            wp.map(|p| p.map(|x| x.round())),
                            next.map(|p| p.map(|x| x.round())),
                        );
                    }
                }
            } else {
                stalled_now.remove(&slot);
            }
        }
    }
    m.edges = sv.bot_edge_stalls();
    ObjSpread::add_cues(&mut m.obj, sv.bot_cue_tally());
    if let Some(st) = &stats {
        st.print(map, &o.gametype, seed);
    }
    Some(m)
}

/// The tallies a client's line lands in: its team's, its skill's and its
/// arm's.
fn tally<'a>(
    m: &'a mut Match,
    o: &Opts,
    num: &str,
    team: &str,
    skill: &HashMap<String, String>,
) -> (&'a mut Tally, &'a mut Tally, &'a mut Tally) {
    // `dm`'s `D;` lines carry the menu's team, its `K;` lines none.
    let team = if team.is_empty() || m.dm {
        "none"
    } else {
        team
    };
    let skill = skill.get(num).cloned().unwrap_or_else(|| "-".into());
    let arm = o.arm(arm_side(num, team, m.dm), m.seed);
    (
        m.by_team.entry(team.to_string()).or_default(),
        m.by_skill.entry(skill).or_default(),
        m.by_arm.entry(arm).or_default(),
    )
}

/// A bot's team as the log names it ("none" in `dm` or off a team).
fn team_name(sv: &mut vcod_server::Server, slot: usize, dm: bool) -> &'static str {
    match sv.bot_team(slot) {
        _ if dm => "none",
        1 => "axis",
        2 => "allies",
        _ => "none",
    }
}

/// One bot's running life, wound and retreat state between ticks.
#[derive(Default)]
struct Life {
    alive: u64,
    /// Ticks since this life's wound, if any.
    wound: Option<u64>,
    retreating: u8,
    /// The held bullet gun and its clip last tick.
    gun: Option<(u8, i16)>,
    /// The target acquired with none tracked, and the ticks since, until
    /// its first round.
    acquired: Option<(usize, Option<u64>)>,
    /// The same, until its first bullet hit lands (read off the log).
    hit_wait: Option<(usize, u64)>,
    /// [`Kit::step`]'s last throw count and tracking.
    throws: u32,
    engaged: bool,
    peeks: PeekLife,
    /// The view's yaw last tick when the bot walked then.
    walk_yaw: Option<f32>,
    fight: FightLife,
}

impl Life {
    /// Adds one tick of `body` (and its cover hold and lean) to `t`.
    fn step(
        &mut self,
        body: Option<&vcod_server::bots::BotBody>,
        cover: Option<(bool, f32)>,
        tactics: Option<[u32; 3]>,
        tick: usize,
        down: &HashSet<usize>,
        t: (&mut Tally, &mut Tally, &mut Tally),
    ) {
        let (mut d, playing, dead) = (
            Tally::default(),
            body.is_some_and(|b| b.playing),
            body.is_some_and(|b| b.dead),
        );
        let r = body.map_or(0, |b| b.retreating);
        match (self.retreating, r) {
            (0, 1) => d.hurt += 1,
            (0, 2) => d.dry += 1,
            _ => {}
        }
        self.retreating = r;
        let fired = match (body.filter(|b| b.playing && b.gun), self.gun) {
            (Some(b), Some((w, clip))) if b.weapon == w && b.clip < clip => (clip - b.clip) as u64,
            _ => 0,
        };
        d.aim.shots += fired;
        self.gun = body
            .filter(|b| b.playing && b.gun)
            .map(|b| (b.weapon, b.clip));
        let target = body.filter(|b| b.playing).and_then(|b| b.target);
        let fresh = target.filter(|_| self.acquired.is_none());
        // Ticks since the fresh target came, `None` once it was fired on.
        self.acquired = match (target, self.acquired) {
            (Some(t), Some((a, pending))) if t == a => Some((
                a,
                pending.and_then(|n| {
                    if fired > 0 {
                        d.aim.first_shot_ms.push(n * TICK_MS);
                        None
                    } else {
                        Some(n + 1)
                    }
                }),
            )),
            // A switch from one target to another is not a fresh sight.
            (Some(t), Some(_)) => Some((t, None)),
            (Some(t), None) => {
                d.aim.acquired += 1;
                if fired > 0 {
                    d.aim.first_shot_ms.push(0);
                    Some((t, None))
                } else {
                    Some((t, Some(1)))
                }
            }
            (None, _) => None,
        };
        self.hit_wait = match (fresh, target, self.hit_wait) {
            (Some(t), _, _) => Some((t, 1)),
            (None, Some(t), Some((a, n))) if t == a => Some((a, n + 1)),
            _ => None,
        };
        d.kit = Kit::step(self, body);
        d.peeks = Peeks::step(&mut self.peeks, tactics, dead);
        d.fights = Fights::step(&mut self.fight, body, tick, down);
        let walking = body.filter(|b| b.playing && b.walking);
        if let (Some(b), Some(y)) = (walking, self.walk_yaw) {
            let turn = (b.yaw - y + 180.0).rem_euclid(360.0) - 180.0;
            d.walk_ticks += 1;
            d.walk_turn += f64::from(turn.abs());
            d.snaps += u64::from(turn.abs() > SNAP_DEG);
        }
        self.walk_yaw = walking.map(|b| b.yaw);
        if let Some(b) = body.filter(|b| b.playing) {
            if self.wound.is_none() && b.engaged && b.health < 50 {
                self.wound = Some(0);
                d.wounds += 1;
            } else if let Some(w) = self.wound.as_mut() {
                *w += 1;
            }
        } else if dead {
            if self.wound.is_some_and(|w| w < 5000 / TICK_MS) {
                d.wound_deaths += 1;
            }
            self.wound = None;
        }
        if playing {
            self.alive += 1;
            if let Some((hold, lean)) = cover {
                d.hold += u64::from(hold);
                d.lean += u64::from(lean != 0.0);
            }
        } else if self.alive > 0 {
            d.life_ticks += self.alive;
            d.lives += 1;
            self.alive = 0;
        }
        t.0.add(&d);
        t.1.add(&d);
        t.2.add(&d);
    }
}

/// A bullet hit (`hit` 1) by bot `anum` on `vnum`: the ms since it
/// acquired `vnum`, if this is the first since.
fn sight_to_hit(bots: &mut HashMap<usize, Life>, anum: &str, vnum: &str, hit: u64) -> Option<u64> {
    let (a, v) = (anum.parse::<usize>().ok()?, vnum.parse::<usize>().ok()?);
    let life = bots.get_mut(&a).filter(|_| hit > 0)?;
    let (_, n) = life.hit_wait.filter(|w| w.0 == v)?;
    life.hit_wait = None;
    Some(n * TICK_MS)
}

/// The `vc_bot_skill` level a bot plays at, as text.
fn skill_of(sv: &vcod_server::Server, slot: usize) -> String {
    sv.bot_skill(slot).map_or("-".into(), |l| l.to_string())
}

/// Ticks a hit's answer is waited for: 1 s.
const ANSWER_TICKS: usize = (1000 / TICK_MS) as usize;

/// How bots answer hits and whom they had in their sights when killed
/// (`turned`, `answered`, `blindsided`; bot-combat.md section 1).
#[derive(Default)]
struct React {
    /// (victim, attacker) -> tick of the hit still waiting on an answer,
    /// and whether the victim has targeted the attacker since.
    pending: HashMap<(usize, usize), (usize, bool)>,
    /// Per bot: the slots it targeted this life, and whether it played
    /// last tick.
    targeted: HashMap<usize, (std::collections::HashSet<usize>, bool)>,
}

impl React {
    /// One tick's bodies, read before its log lines: on a tick after the
    /// hit's, the attacker as target turns, the trigger down besides
    /// answers. `add` credits a slot's tallies.
    fn watch(
        &mut self,
        sv: &mut vcod_server::Server,
        dm: bool,
        tick: usize,
        mut add: impl FnMut(&str, &str, &mut dyn FnMut(&mut Tally)),
    ) {
        for slot in sv.bot_slots() {
            let Some(b) = sv.bot_body(slot) else {
                continue;
            };
            let (seen, was) = self.targeted.entry(slot).or_default();
            if b.playing && !*was {
                seen.clear();
            }
            *was = b.playing;
            if !b.playing {
                continue;
            }
            seen.extend(b.target);
            let team = team_name(sv, slot, dm);
            let num = slot.to_string();
            self.pending.retain(|(v, a), (t, turned)| {
                if *v != slot || *t >= tick || b.target != Some(*a) {
                    return true;
                }
                if !*turned {
                    *turned = true;
                    add(&num, team, &mut |t| t.turned += 1);
                }
                if b.firing {
                    add(&num, team, &mut |t| t.answered += 1);
                }
                !b.firing
            });
        }
        self.pending.retain(|_, (t, _)| tick - *t < ANSWER_TICKS);
    }

    /// A hit on `victim` by `attacker` at `tick`; `count(1)` when it opens
    /// a window (the victim is a bot with none open on that attacker).
    fn hit(&mut self, victim: &str, attacker: &str, tick: usize, count: impl FnOnce(u32)) {
        let (Ok(v), Ok(a)) = (victim.parse(), attacker.parse()) else {
            return;
        };
        if !self.targeted.contains_key(&v) || self.pending.contains_key(&(v, a)) {
            return;
        }
        self.pending.insert((v, a), (tick, false));
        count(1);
    }

    /// A bot `victim` killed by enemy `killer`: whether it never targeted
    /// him this life; `None` when the victim is no bot.
    fn killed(&mut self, victim: &str, killer: &str) -> Option<bool> {
        let (Ok(v), Ok(k)) = (victim.parse::<usize>(), killer.parse::<usize>()) else {
            return None;
        };
        self.pending.retain(|(pv, _), _| *pv != v);
        let (seen, _) = self.targeted.get(&v)?;
        Some(!seen.contains(&k))
    }
}

/// Ticks a death counts against the peek or hunt arrival before it: 2 s.
const EXPOSED_TICKS: u64 = 2000 / TICK_MS;

/// Peeks from cover and hunt arrivals (`Bot::tactics`) and the deaths
/// within [`EXPOSED_TICKS`] of each: of any peek, of a re-peek (the spot
/// and pose of the peek before), of a hunt reaching the lost enemy's spot
/// or its vantage (bot-combat.md section 1).
#[derive(Default, Clone)]
struct Peeks {
    peeks: u32,
    peek_deaths: u32,
    repeeks: u32,
    repeek_deaths: u32,
    hunts: u32,
    hunt_deaths: u32,
}

/// One bot's last counts and the ticks since its last peek, re-peek and
/// hunt arrival this life.
#[derive(Default)]
struct PeekLife {
    last: [u32; 3],
    since: [Option<u64>; 3],
    dead: bool,
}

impl Peeks {
    fn add(&mut self, o: &Peeks) {
        self.peeks += o.peeks;
        self.peek_deaths += o.peek_deaths;
        self.repeeks += o.repeeks;
        self.repeek_deaths += o.repeek_deaths;
        self.hunts += o.hunts;
        self.hunt_deaths += o.hunt_deaths;
    }

    fn line(&self) -> String {
        let pct = |d: u32, n: u32| 100.0 * d as f32 / n.max(1) as f32;
        format!(
            "peeks {:6} dead in 2 s {:4.1}% re-peeks {:6} dead in 2 s {:4.1}% hunts {:5} dead in 2 s {:4.1}%",
            self.peeks,
            pct(self.peek_deaths, self.peeks),
            self.repeeks,
            pct(self.repeek_deaths, self.repeeks),
            self.hunts,
            pct(self.hunt_deaths, self.hunts),
        )
    }

    /// One bot-tick: the counts' rise since the last, and on the tick the
    /// body dies, a death against each still inside its window.
    fn step(life: &mut PeekLife, tactics: Option<[u32; 3]>, dead: bool) -> Peeks {
        let mut p = Peeks::default();
        let now = tactics.unwrap_or(life.last);
        let rose: [u32; 3] = std::array::from_fn(|i| now[i].saturating_sub(life.last[i]));
        [p.peeks, p.repeeks, p.hunts] = rose;
        for (since, rose) in life.since.iter_mut().zip(rose) {
            *since = if rose > 0 {
                Some(0)
            } else {
                since.map(|t| t + 1)
            };
        }
        life.last = now;
        if dead && !life.dead {
            let inside = |s: Option<u64>| u32::from(s.is_some_and(|t| t < EXPOSED_TICKS));
            p.peek_deaths = inside(life.since[0]);
            p.repeek_deaths = inside(life.since[1]);
            p.hunt_deaths = inside(life.since[2]);
            life.since = [None; 3];
        }
        life.dead = dead;
        p
    }
}

/// Gap in a fight's tracking that ends it: 1 s.
const FIGHT_GAP: usize = (1000 / TICK_MS) as usize;
/// A fight on the same target this soon after one broken off is a
/// re-engagement: 10 s.
const REENGAGE_TICKS: usize = (10_000 / TICK_MS) as usize;
/// A fight this long or longer is long: 10 s.
const LONG_FIGHT: usize = (10_000 / TICK_MS) as usize;

/// Fights (bot-combat.md section 1): a bot's ticks with `BotBody::target`
/// on one slot, gaps under [`FIGHT_GAP`] (another target in between
/// included). Each ends with the target down (dead or off the field), the
/// bot down, or broken off (the gap) with both alive. A re-engagement is a
/// fight on the target of one broken off within [`REENGAGE_TICKS`].
#[derive(Default, Clone)]
struct Fights {
    fights: u32,
    downed: u32,
    died: u32,
    broken: u32,
    long: u32,
    reengaged: u32,
}

/// One bot's fights under way by target (first and last tracked tick),
/// and its fights broken off by target (last tracked tick).
#[derive(Default)]
struct FightLife {
    on: HashMap<usize, (usize, usize)>,
    broke: HashMap<usize, usize>,
}

impl Fights {
    fn add(&mut self, o: &Fights) {
        self.fights += o.fights;
        self.downed += o.downed;
        self.died += o.died;
        self.broken += o.broken;
        self.long += o.long;
        self.reengaged += o.reengaged;
    }

    fn line(&self) -> String {
        let pct = |d: u32| 100.0 * d as f32 / self.fights.max(1) as f32;
        format!(
            "fights {:6} target down {:4.1}% bot down {:4.1}% broken {:4.1}% over 10 s {:4} re-engaged in 10 s {:5} ({:4.1}% of broken)",
            self.fights,
            pct(self.downed),
            pct(self.died),
            pct(self.broken),
            self.long,
            self.reengaged,
            100.0 * self.reengaged as f32 / self.broken.max(1) as f32,
        )
    }

    /// One bot-tick: the fights that ended or started on it.
    fn step(
        life: &mut FightLife,
        body: Option<&vcod_server::bots::BotBody>,
        tick: usize,
        down: &HashSet<usize>,
    ) -> Fights {
        let mut f = Fights::default();
        let alive = body.is_some_and(|b| b.playing && !b.dead);
        let target = body.filter(|_| alive).and_then(|b| b.target);
        life.broke
            .retain(|t, end| !down.contains(t) && tick - *end <= REENGAGE_TICKS);
        let ended = |f: &mut Fights, (start, last): (usize, usize)| {
            f.fights += 1;
            f.long += u32::from(last - start >= LONG_FIGHT);
        };
        for (t, (start, last)) in std::mem::take(&mut life.on) {
            if !alive {
                f.died += 1;
            } else if down.contains(&t) {
                f.downed += 1;
            } else if target != Some(t) && tick - last >= FIGHT_GAP {
                f.broken += 1;
                life.broke.insert(t, last);
            } else {
                let last = if target == Some(t) { tick } else { last };
                life.on.insert(t, (start, last));
                continue;
            }
            ended(&mut f, (start, last));
        }
        if !alive {
            life.broke.clear();
        }
        if let Some(t) = target.filter(|t| !down.contains(t))
            && !life.on.contains_key(&t)
        {
            f.reengaged += u32::from(life.broke.remove(&t).is_some());
            life.on.insert(t, (tick, tick));
        }
        f
    }
}

/// `GAME=cod2`: CoD2 matches through `vcod_server::cod2::Cod2Server`.
mod cod2 {
    use std::collections::{BTreeMap, HashMap, VecDeque};
    use std::rc::Rc;
    use std::time::{Duration, Instant};

    use vcod_server::cod2::{Cod2Config, Cod2Server};

    use super::{Digest, RADIUS, TICK_MS, WINDOW, install_trace};

    /// `NETDIGEST`'s CoD2 observer ([`super::Observer`]): challenge,
    /// connect, then a packet a tick, its `cp` once the gamestate is in.
    #[derive(Default)]
    enum Observer2 {
        #[default]
        Start,
        Challenging,
        Connecting(i32),
        Joined {
            cl: Box<vcod_cod2::net::Client>,
            out: Vec<Vec<u8>>,
        },
    }

    /// An [`Observer2`] at the `i`th observer's address
    /// ([`super::observer_addr`]).
    struct Spectator2 {
        state: Observer2,
        addr: std::net::SocketAddr,
        qport: u16,
    }

    impl Spectator2 {
        fn new(i: usize) -> Self {
            let (addr, qport) = super::observer_addr(i);
            Spectator2 {
                state: Observer2::Start,
                addr,
                qport,
            }
        }

        fn deliver(&mut self, sv: &mut Cod2Server, now: Instant) {
            for p in self.state.packets(self.qport) {
                sv.handle_packet(self.addr, &p, now);
            }
        }

        fn receive(
            &mut self,
            sent: &[(std::net::SocketAddr, Vec<u8>)],
            map: &str,
            fs: &vcod_common::pk3::Pk3Fs,
        ) {
            self.state.receive(sent, self.addr, self.qport, map, fs);
        }
    }

    impl Observer2 {
        /// The packets due to the server.
        fn packets(&mut self, qport: u16) -> Vec<Vec<u8>> {
            use vcod_cod2::net::oob;
            match self {
                Observer2::Start => {
                    *self = Observer2::Challenging;
                    vec![oob::getchallenge(None)]
                }
                Observer2::Challenging => Vec::new(),
                Observer2::Connecting(c) => {
                    let ui = "\\name\\observer\\rate\\25000\\snaps\\20\\cl_punkbuster\\0";
                    vec![oob::connect(ui, *c, qport)]
                }
                Observer2::Joined { out, .. } => std::mem::take(out),
            }
        }

        fn receive(
            &mut self,
            sent: &[(std::net::SocketAddr, Vec<u8>)],
            addr: std::net::SocketAddr,
            qport: u16,
            map: &str,
            fs: &vcod_common::pk3::Pk3Fs,
        ) {
            use vcod_cod2::net::oob::{self, Reply};
            for (to, p) in sent {
                if *to != addr {
                    continue;
                }
                if let Some(payload) = oob::payload(p) {
                    match (oob::parse_reply(payload), &*self) {
                        (Reply::Challenge(c), Observer2::Challenging) => {
                            *self = Observer2::Connecting(c);
                        }
                        (Reply::Connect, Observer2::Connecting(c)) => {
                            let cl = vcod_cod2::net::Client::new(qport, *c);
                            *self = Observer2::Joined {
                                cl: Box::new(cl),
                                out: Vec::new(),
                            };
                        }
                        _ => {}
                    }
                    continue;
                }
                let Observer2::Joined { cl, .. } = self else {
                    continue;
                };
                if let Ok(Some(parsed)) = cl.receive(p)
                    && parsed.gamestate
                {
                    let gs = cl.parser.gamestate.as_ref().unwrap();
                    let si = gs.configstring(1).to_string();
                    let feed = gs.checksum_feed;
                    fs.clear_references();
                    fs.touch(&format!("maps/mp/{map}.d3dbsp"));
                    let cp = vcod_cod2::net::pure::pure_command(fs, feed, &si);
                    cl.add_reliable(&cp);
                }
            }
            if let Observer2::Joined { cl, out } = self {
                let pkt = if cl.parser.gamestate.is_some() {
                    let base = cl.parser.snapshot().map_or(0, |s| s.server_time);
                    let cmd = cl.idle_cmd(base + super::TICK_MS as i32);
                    cl.build_packet(&[cmd])
                } else {
                    cl.build_packet(&[])
                };
                out.extend(pkt.ok());
            }
        }
    }

    #[derive(Default)]
    struct Cod2Match {
        net: Option<Digest>,
        map: String,
        seed: u64,
        ticks: usize,
        /// By the killer's team (`dm`: "none"), kills of another player.
        kills: BTreeMap<String, u32>,
        deaths: u32,
        /// Deaths with no other player the killer: suicides and falls.
        suicides: u32,
        hits: u32,
        rounds: u32,
        playing: u64,
        stalled: u64,
        held: u64,
        ladder: u64,
        /// Units walked by every bot, over every tick it played.
        travelled: f64,
        /// The script's `A;` actions (`bomb_plant`, `bomb_defuse`).
        actions: BTreeMap<String, u32>,
        /// The team scores at the end, [axis, allies].
        scores: [i32; 2],
        /// Offhand grenades the bots threw.
        throws: u32,
        /// Mantles the bots started.
        mantles: u32,
        /// Mounts of a gun, the ticks spent on one, and kills off one.
        mounts: u32,
        gun_ticks: u64,
        gun_kills: u32,
        /// Bot-ticks playing on a held cover spot, and with `leanf` off 0.
        cover: u64,
        lean: u64,
        /// Retreats started (`Bot::retreating` from 0 to 1 or 2).
        retreats: u32,
        /// Smoke grenades the bots threw.
        smokes: u32,
        /// Hits on a bot by an enemy (one per attacker until answered or
        /// 1 s passes), and those the bot fired back at him within 1 s.
        hurt: u32,
        answered: u32,
        /// By weapon class ([`class_of`]) and by skill level.
        classes: BTreeMap<&'static str, Arms>,
        skills: BTreeMap<u8, Arms>,
        /// By weapon name.
        weapons: BTreeMap<String, Arms>,
        /// Each bot's spawn kit chosen, by primary name.
        picks: BTreeMap<String, u32>,
        /// Weapons a living bot came to hold that it did not spawn with.
        pickups: u32,
        /// Near the objective, by team ([`super::ObjSpread`]).
        obj: BTreeMap<String, super::ObjSpread>,
        log: Vec<String>,
        digest: Option<Digest>,
    }

    /// One row of the weapon class or skill split: kills, deaths, hits,
    /// rounds fired and those fired with the sight fully up, and each
    /// kill's time from the killer's first hit on the victim, ms.
    #[derive(Default, Clone)]
    struct Arms {
        kills: u32,
        deaths: u32,
        hits: u32,
        rounds: u32,
        sighted: u32,
        ttk_ms: Vec<u64>,
    }

    impl Arms {
        fn add(&mut self, o: &Arms) {
            self.kills += o.kills;
            self.deaths += o.deaths;
            self.hits += o.hits;
            self.rounds += o.rounds;
            self.sighted += o.sighted;
            self.ttk_ms.extend(&o.ttk_ms);
        }

        fn line(&self) -> String {
            let mut t = self.ttk_ms.clone();
            t.sort_unstable();
            let mean = t.iter().sum::<u64>() / (t.len().max(1) as u64);
            let median = t.get(t.len() / 2).copied().unwrap_or(0);
            format!(
                "kills {:4} deaths {:4} hits {:5} rounds {:6} acc {:.3} sighted {:3.0}% ttk mean {:5} median {:5} ms",
                self.kills,
                self.deaths,
                self.hits,
                self.rounds,
                f64::from(self.hits) / f64::from(self.rounds.max(1)),
                100.0 * f64::from(self.sighted) / f64::from(self.rounds.max(1)),
                mean,
                median
            )
        }
    }

    /// The class a weapon's kills and rounds count under: CoD2's
    /// `weaponClass`, its rifles split into bolt, semi-automatic and scoped.
    fn class_of(d: Option<&vcod_cod2::pmove::weapon::WeaponParams>) -> &'static str {
        let Some(d) = d else {
            return "other";
        };
        match d.weap_class {
            0 if d.name.contains("sniper") || d.name.contains("scope") => "sniper",
            0 if d.bolt_action => "bolt",
            0 if d.semi_auto => "semi",
            0 => "auto rifle",
            1 => "mg",
            2 => "smg",
            3 => "shotgun",
            4 => "pistol",
            5 => "grenade",
            _ => "other",
        }
    }

    pub(super) fn main() {
        let env = |k: &str, d: &str| std::env::var(k).unwrap_or_else(|_| d.into());
        let maps: Vec<String> = env("MAPS", &env("MAP", "mp_toujane"))
            .split(',')
            .map(str::to_string)
            .collect();
        let gametype = env("GAMETYPE", "dm");
        let bots: usize = env("BOTS", "6").parse().expect("BOTS");
        let ticks: usize = env("TICKS", "6000").parse().expect("TICKS");
        let jobs: usize = env("JOBS", "4").parse().expect("JOBS");
        let verbose = env("VERBOSE", "0") == "1";
        let mut seeds: Vec<u64> = std::env::args()
            .skip(1)
            .map(|s| s.parse().expect("seed"))
            .collect();
        if seeds.is_empty() {
            seeds = (1..=4).collect();
        }
        println!(
            "cod2 maps {} gametype {gametype} bots {bots} ticks {ticks}",
            maps.join(",")
        );
        let started = Instant::now();
        let queue = std::sync::Mutex::new(
            maps.iter()
                .rev()
                .flat_map(|m| seeds.iter().rev().map(move |s| (m.clone(), *s)))
                .collect::<Vec<_>>(),
        );
        let results = std::sync::Mutex::new(Vec::new());
        std::thread::scope(|s| {
            for _ in 0..jobs.max(1) {
                s.spawn(|| {
                    while let Some((map, seed)) = queue.lock().unwrap().pop() {
                        let m = run(&map, &gametype, bots, ticks, seed, verbose);
                        vcod_server::tick_trace::finish().expect("VCOD_TICK_TRACE");
                        let Some(m) = m else {
                            return;
                        };
                        eprintln!(
                            "{map} seed {seed} done ({:.0} s)",
                            started.elapsed().as_secs_f32()
                        );
                        results.lock().unwrap().push(m);
                    }
                });
            }
        });
        let mut results = results.into_inner().unwrap();
        if results.is_empty() {
            eprintln!("no CoD2 fs (set COD2_DIR)");
            return;
        }
        results.sort_by(|a, b| (&a.map, a.seed).cmp(&(&b.map, b.seed)));
        let line = |m: &Cod2Match| {
            let kills: u32 = m.kills.values().sum();
            format!(
                "kills {kills} {:?} deaths {} suicides {} hits {} rounds {} acc {:.2} stalled {} held {} of {} ({:.1}%) ladder {} walked {:.0} u/bot-s actions {:?} score axis {} allies {} throws {} mantles {} cover {} lean {} retreats {} smokes {} answered {} of {} ({:.0}%) mounts {} gun-s {:.0} gun kills {}",
                m.kills,
                m.deaths,
                m.suicides,
                m.hits,
                m.rounds,
                f64::from(m.hits) / f64::from(m.rounds.max(1)),
                m.stalled,
                m.held,
                m.playing,
                100.0 * m.stalled as f64 / m.playing.max(1) as f64,
                m.ladder,
                m.travelled / (m.playing.max(1) as f64 * TICK_MS as f64 / 1000.0),
                m.actions,
                m.scores[0],
                m.scores[1],
                m.throws,
                m.mantles,
                m.cover,
                m.lean,
                m.retreats,
                m.smokes,
                m.answered,
                m.hurt,
                100.0 * f64::from(m.answered) / f64::from(m.hurt.max(1)),
                m.mounts,
                m.gun_ticks as f64 * TICK_MS as f64 / 1000.0,
                m.gun_kills,
            )
        };
        for m in &results {
            for l in &m.log {
                println!("  {} seed {} {l}", m.map, m.seed);
            }
            println!("{} seed {}: ticks {} {}", m.map, m.seed, m.ticks, line(m));
            for (k, t) in &m.obj {
                println!("{} seed {}: obj {k} {}", m.map, m.seed, t.line());
            }
            if let Some(d) = &m.digest {
                d.print(&m.map, &gametype, m.seed, m.ticks);
            }
            if let Some(d) = &m.net {
                d.print(&m.map, &gametype, m.seed, m.ticks);
            }
        }
        let mut classes: BTreeMap<&str, Arms> = BTreeMap::new();
        let mut skills: BTreeMap<u8, Arms> = BTreeMap::new();
        let mut pickups = 0;
        let mut weapons: BTreeMap<&str, Arms> = BTreeMap::new();
        let mut picks: BTreeMap<&str, u32> = BTreeMap::new();
        for m in &results {
            for (k, a) in &m.weapons {
                weapons.entry(k).or_default().add(a);
            }
            for (k, n) in &m.picks {
                *picks.entry(k).or_default() += n;
            }
            for (k, a) in &m.classes {
                classes.entry(k).or_default().add(a);
            }
            for (k, a) in &m.skills {
                skills.entry(*k).or_default().add(a);
            }
            pickups += m.pickups;
        }
        for (k, a) in &classes {
            println!("class {k:8} {}", a.line());
        }
        for (k, a) in &weapons {
            println!("weapon {k:22} {}", a.line());
        }
        for (k, a) in &skills {
            println!("skill {k}        {}", a.line());
        }
        println!("spawn picks {picks:?}");
        println!("pickups {pickups}");
        let mut all_obj: BTreeMap<String, super::ObjSpread> = BTreeMap::new();
        for map in &maps {
            let mut t = Cod2Match {
                map: map.clone(),
                ..Default::default()
            };
            for m in results.iter().filter(|m| &m.map == map) {
                for (k, v) in &m.kills {
                    *t.kills.entry(k.clone()).or_default() += v;
                }
                t.deaths += m.deaths;
                t.suicides += m.suicides;
                t.hits += m.hits;
                t.rounds += m.rounds;
                t.playing += m.playing;
                t.stalled += m.stalled;
                t.held += m.held;
                t.ladder += m.ladder;
                t.travelled += m.travelled;
                for (k, v) in &m.actions {
                    *t.actions.entry(k.clone()).or_default() += v;
                }
                t.scores[0] += m.scores[0];
                t.scores[1] += m.scores[1];
                t.throws += m.throws;
                t.mantles += m.mantles;
                t.mounts += m.mounts;
                t.gun_ticks += m.gun_ticks;
                t.gun_kills += m.gun_kills;
                t.cover += m.cover;
                t.lean += m.lean;
                t.retreats += m.retreats;
                t.smokes += m.smokes;
                t.hurt += m.hurt;
                t.answered += m.answered;
                for (k, o) in &m.obj {
                    t.obj.entry(k.clone()).or_default().add(o);
                    all_obj.entry(k.clone()).or_default().add(o);
                }
            }
            println!("{map} total: {}", line(&t));
            for (k, o) in &t.obj {
                println!("{map} total: obj {k} {}", o.line());
            }
        }
        for (k, o) in &all_obj {
            println!("all maps: obj {k} {}", o.line());
        }
    }

    fn run(
        map: &str,
        gametype: &str,
        bots: usize,
        ticks: usize,
        seed: u64,
        verbose: bool,
    ) -> Option<Cod2Match> {
        let fs = Rc::new(vcod_common::testing::cod2_fs()?);
        let mut now = Instant::now();
        let mut sv = Cod2Server::new(
            Cod2Config {
                map: map.to_string(),
                gametype: gametype.to_string(),
                hostname: "match".into(),
                max_clients: (bots + 2 + super::observers_from_env()).max(8),
                gametype_script: None,
            },
            seed * 2 + 1,
            now,
        );
        sv.load(Some(fs.clone())).expect("load the level");
        let slots = sv.add_bots(bots, true);
        // `SKILLS=a,b,...` one level a bot in join order, else
        // `AXIS_SKILL=` / `ALLIES_SKILL=` by team (`add_bots` alternates
        // allies and axis), else `vc_bot_skill`'s.
        let list: Vec<u8> = std::env::var("SKILLS")
            .map(|l| l.split(',').filter_map(|s| s.trim().parse().ok()).collect())
            .unwrap_or_default();
        let side = |k: &str| std::env::var(k).ok().and_then(|v| v.parse::<u8>().ok());
        let (axis, allies) = (side("AXIS_SKILL"), side("ALLIES_SKILL"));
        let mut level_of: HashMap<usize, u8> = HashMap::new();
        if let Some(b) = sv.bots_mut() {
            b.wait_for_graph();
            b.man_guns = std::env::var("GUNS").map_or(true, |v| v != "0");
            for (i, &slot) in slots.iter().enumerate() {
                let l = if !list.is_empty() {
                    Some(list[i % list.len()])
                } else if gametype == "dm" {
                    None
                } else if i % 2 == 0 {
                    allies
                } else {
                    axis
                };
                if let Some(l) = l {
                    b.admin.skills.insert(slot, l);
                    if let Some(bot) = b.bots_mut().get_mut(&slot) {
                        bot.skill = vcod_server::bots::Skill::level(l);
                    }
                }
                let l = b
                    .bots()
                    .get(&slot)
                    .and_then(|bot| bot.skill.level_of())
                    .unwrap_or(1);
                level_of.insert(slot, l);
            }
        }
        let mut first_hit: HashMap<(usize, usize), usize> = HashMap::new();
        let mut kit: HashMap<usize, u64> = HashMap::new();
        let mut lives: HashMap<usize, u32> = HashMap::new();
        let mut m = Cod2Match {
            map: map.to_string(),
            seed,
            digest: Digest::from_env(),
            net: Digest::net_from_env(),
            ..Default::default()
        };
        install_trace("cod2", map, gametype, seed);
        let mut observer = m.net.is_some().then(|| Spectator2::new(0));
        let mut crowd: Vec<Spectator2> = (1..=super::observers_from_env())
            .map(Spectator2::new)
            .collect();
        let mut dump = super::net_dump("cod2", map, gametype, seed);
        if observer.is_some() {
            sv.test_frame_clock();
        }
        // `STALLS=1`: every 20th tick of a stalled sample to stderr.
        let stall_log = std::env::var("STALLS").as_deref() == Ok("1");
        let mut seen = 0;
        let mut hist: HashMap<usize, VecDeque<([f32; 3], bool)>> = HashMap::new();
        let mut trail: HashMap<usize, VecDeque<[f32; 3]>> = HashMap::new();
        let mut last: HashMap<usize, ([f32; 3], u8, i32)> = HashMap::new();
        let mut mantling: HashMap<usize, bool> = HashMap::new();
        let mut gunning: HashMap<usize, bool> = HashMap::new();
        let mut retreating: HashMap<usize, u8> = HashMap::new();
        // (victim, attacker) -> the tick of the hit still to answer.
        let mut pending: HashMap<(usize, usize), usize> = HashMap::new();
        let mut stats = super::TickStats::from_env();
        // `SPIKE_MS=<ms>`: each tick past it to stderr with its phases.
        let spike_ms: Option<f32> = std::env::var("SPIKE_MS").ok().and_then(|v| v.parse().ok());
        if spike_ms.is_some() {
            vcod_server::tick_profile::enable();
        }
        // `RESTART=<n>`: a console `map_restart` every `n` ticks, each a
        // spawn wave of every bot at once.
        let restart: usize = std::env::var("RESTART")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(0);
        for tick in 0..ticks {
            now += Duration::from_millis(TICK_MS);
            // `TACTICS=0` (or `none`): every bot guards from the first spot in reach
            // (`Bot::tactics`); `CONVERGE=0`: no S&D defender leaves its
            // site for one under attack (`Bot::converge`); `LANES=0`: every
            // bot walks straight to its objective (`Bot::lanes`); `FAR=0`,
            // `CUES=0`, `TRAVEL=0` turn off bot-objectives.md 14's rules.
            if let Some(b) = sv.bots_mut() {
                let off = |k: &str| matches!(std::env::var(k).as_deref(), Ok("0" | "none"));
                let arms = [
                    !off("TACTICS"),
                    !off("CONVERGE"),
                    !off("LANES"),
                    !off("FAR"),
                    !off("CUES"),
                    !off("TRAVEL"),
                ];
                for bot in b.bots_mut().values_mut() {
                    bot.tactics &= arms[0];
                    bot.converge &= arms[1];
                    bot.lanes &= arms[2];
                    bot.far &= arms[3];
                    bot.cues &= arms[4];
                    bot.travel &= arms[5];
                }
            }
            if restart > 0 && tick > 0 && tick % restart == 0 {
                sv.core.console.push_back("map_restart".into());
            }
            // The script's `map_restart` and `exitLevel` run off the
            // console, as `main.rs`'s loop drains it ahead of the frame.
            if let Some(ob) = observer.as_mut() {
                ob.deliver(&mut sv, now);
            }
            for ob in &mut crowd {
                ob.deliver(&mut sv, now);
            }
            let sched = spike_ms.and_then(|_| vcod_server::tick_profile::sched());
            let t0 = Instant::now();
            sv.drain_console(now);
            sv.tick_frames(now, 1);
            let tick_ms = t0.elapsed().as_secs_f32() * 1000.0;
            let phases = vcod_server::tick_profile::take();
            if let Some(st) = stats.as_mut() {
                st.add(tick, tick_ms, &phases);
            }
            if spike_ms.is_some_and(|s| tick_ms > s) {
                eprintln!(
                    "spike {map} {gametype} seed {seed} tick {tick} {tick_ms:.1} ms ({}): {}",
                    vcod_server::tick_profile::split(sched, tick_ms),
                    vcod_server::tick_profile::format(&phases, 1.0)
                );
            }
            m.ticks = tick + 1;
            if let Some(d) = m.digest.as_mut() {
                d.cod2(&sv, &slots, tick);
            }
            if let (Some(ob), Some(d)) = (observer.as_mut(), m.net.as_mut()) {
                let sent = sv.take_outgoing();
                d.datagrams(&sent, tick);
                super::dump_datagrams(&mut dump, &sent, tick);
                ob.receive(&sent, map, &fs);
                for ob in &mut crowd {
                    ob.receive(&sent, map, &fs);
                }
            } else if !crowd.is_empty() {
                let sent = sv.take_outgoing();
                for ob in &mut crowd {
                    ob.receive(&sent, map, &fs);
                }
            }
            if tick == 0
                && let Some(g) = sv.bots_mut().and_then(|b| b.graph())
            {
                eprintln!(
                    "{map} seed {seed}: graph {} nodes, {} edges, {} mantles",
                    g.len(),
                    g.edge_count(),
                    g.mantles.len()
                );
            }
            // Per bot: holding still on purpose (`BotBody::holds` less the
            // seats and bipods CoD2 has none of), on a cover spot, its
            // retreat, and the target it fired at this tick.
            let state: HashMap<usize, (bool, bool, u8, Option<usize>)> = sv
                .bots_mut()
                .map(|b| {
                    b.bots()
                        .iter()
                        .map(|(s, b)| {
                            let holds = b.engaged()
                                || b.holds_cover()
                                || b.retreating() != 0
                                || b.on_objective();
                            let fired = b.target_slot().filter(|_| b.fired());
                            (*s, (holds, b.holds_cover(), b.retreating(), fired))
                        })
                        .collect()
                })
                .unwrap_or_default();
            let teams: HashMap<usize, String> = sv
                .bots_mut()
                .map(|b| {
                    b.bots()
                        .iter()
                        .map(|(s, b)| (*s, b.team().to_string()))
                        .collect()
                })
                .unwrap_or_default();
            pending.retain(|(victim, attacker), at| {
                if state.get(victim).and_then(|s| s.3) == Some(*attacker) {
                    m.answered += 1;
                    return false;
                }
                tick - *at < 20
            });
            // Per bot: its objective's point, the share of its ways in that see it, on
            // a cover spot (`ObjSpread`).
            let objinfo: HashMap<usize, super::ObjInfo> = sv
                .bots_mut()
                .map(|b| {
                    b.bots()
                        .iter()
                        .map(|(s, b)| {
                            let info = (
                                b.objective_point(),
                                b.exposure(),
                                b.holds_cover(),
                                b.guard_point(),
                            );
                            (*s, info)
                        })
                        .collect()
                })
                .unwrap_or_default();
            let level = sv.level()?;
            let samples: Vec<super::ObjSample> = slots
                .iter()
                .filter_map(|&s| {
                    let p = level.players.get(s)?.as_ref()?;
                    let (point, exposure, cover, guard) = *objinfo.get(&s)?;
                    let team = teams.get(&s)?.clone();
                    (level.playing(s) && p.ps.pm_type == 0 && (team == "axis" || team == "allies"))
                        .then(|| super::ObjSample {
                            slot: s,
                            team,
                            at: p.ps.origin.into(),
                            point,
                            exposure,
                            cover,
                            guard,
                        })
                })
                .collect();
            super::ObjSpread::feed(&mut m.obj, &samples, &mut trail);
            for &slot in &slots {
                let Some(p) = level.players.get(slot).and_then(Option::as_ref) else {
                    continue;
                };
                let playing = level.playing(slot) && p.ps.pm_type == 0;
                let at: [f32; 3] = p.ps.origin.into();
                let held_w = level
                    .combat
                    .weapons
                    .get(usize::from(p.ws.weapon))
                    .and_then(Option::as_ref);
                let clip = held_w.map_or(0, |d| p.ws.clip[d.clip_index]);
                if let Some((was, w, c)) = last.get(&slot)
                    && playing
                {
                    if *w == p.ws.weapon && clip < *c && held_w.is_some_and(|d| d.weap_type == 0) {
                        let n = (c - clip) as u32;
                        m.rounds += n;
                        let sighted = if p.ws.pos_frac >= 1.0 { n } else { 0 };
                        let name = held_w.map_or("?", |d| d.name.as_str()).to_string();
                        for a in [
                            m.classes.entry(class_of(held_w)).or_default(),
                            m.skills.entry(level_of[&slot]).or_default(),
                            m.weapons.entry(name).or_default(),
                        ] {
                            a.rounds += n;
                            a.sighted += sighted;
                        }
                    }
                    let d = (glam::Vec3::from(at) - glam::Vec3::from(*was))
                        .truncate()
                        .length();
                    if d < 200.0 {
                        m.travelled += f64::from(d);
                    }
                }
                last.insert(slot, (at, p.ws.weapon, clip));
                let h = hist.entry(slot).or_default();
                if !playing {
                    h.clear();
                    kit.remove(&slot);
                    lives.remove(&slot);
                    continue;
                }
                let life = lives.entry(slot).or_default();
                *life += 1;
                // A gun come to hand past the spawn's kit.
                let guns = (1..64u32)
                    .filter(|&w| p.ws.held >> w & 1 == 1)
                    .filter(|&w| {
                        level
                            .combat
                            .weapons
                            .get(w as usize)
                            .and_then(Option::as_ref)
                            .is_some_and(|d| d.offhand_class == 0 && d.weap_type == 0)
                    })
                    .fold(0u64, |m, w| m | 1 << w);
                // The spawn's kit is what it holds over its first second;
                // a gun past that was picked up.
                let had = kit.get(&slot).copied().unwrap_or(0);
                if *life > 20 {
                    m.pickups += (guns & !had).count_ones();
                } else if *life == 20 {
                    let first = (1..64u32).find(|&w| {
                        guns >> w & 1 == 1
                            && level
                                .combat
                                .weapons
                                .get(w as usize)
                                .and_then(Option::as_ref)
                                .is_some_and(|d| d.weap_class != 4)
                    });
                    if let Some(d) = first.and_then(|w| {
                        level
                            .combat
                            .weapons
                            .get(w as usize)
                            .and_then(Option::as_ref)
                    }) {
                        *m.picks.entry(d.name.clone()).or_default() += 1;
                    }
                }
                kit.insert(slot, had | guns);
                m.playing += 1;
                let (holds, cover, retreat, _) = state.get(&slot).copied().unwrap_or_default();
                m.cover += u64::from(cover);
                m.lean += u64::from(p.ps.leanf != 0.0);
                if retreat != 0 && retreating.get(&slot).copied().unwrap_or(0) == 0 {
                    m.retreats += 1;
                }
                retreating.insert(slot, retreat);
                let mantle = p.ps.pm_flags & vcod_cod2::pmove::pmf::MANTLE != 0;
                if mantle && !mantling.get(&slot).copied().unwrap_or(false) {
                    m.mantles += 1;
                }
                mantling.insert(slot, mantle);
                let on_gun = p.ps.e_flags & vcod_cod2::pmove::ef::TURRET != 0;
                if on_gun && !gunning.get(&slot).copied().unwrap_or(false) {
                    m.mounts += 1;
                }
                gunning.insert(slot, on_gun);
                if on_gun {
                    m.gun_ticks += 1;
                }
                if p.ps.pm_flags & vcod_cod2::pmove::pmf::LADDER != 0 {
                    m.ladder += 1;
                }
                // A body the script froze (`freezeControls`, a round's end)
                // holds still by the rules, not by a stall.
                let frozen = p.ps.pm_flags & vcod_cod2::pmove::pmf::FROZEN != 0;
                h.push_back((at, holds || frozen));
                if h.len() > WINDOW {
                    h.pop_front();
                }
                if h.len() == WINDOW {
                    let here = glam::Vec3::from(at);
                    let still = h
                        .iter()
                        .all(|(o, _)| glam::Vec3::from(*o).distance(here) < RADIUS);
                    if still {
                        if h.iter().any(|(_, e)| *e) {
                            m.held += 1;
                        } else {
                            m.stalled += 1;
                            if stall_log && tick % 20 == 0 {
                                // The nearest teammate: where it stands and
                                // how fast (a queue behind a still one).
                                let team = teams.get(&slot);
                                let mate = slots
                                    .iter()
                                    .filter(|&&o| o != slot && teams.get(&o) == team)
                                    .filter_map(|&o| {
                                        let q = level.players.get(o)?.as_ref()?;
                                        level.playing(o).then_some((o, q))
                                    })
                                    .map(|(o, q)| {
                                        let o_at = glam::Vec3::from(<[f32; 3]>::from(q.ps.origin));
                                        let v = glam::Vec3::from(<[f32; 3]>::from(q.ps.velocity));
                                        (o, o_at.distance(here), o_at, v.length())
                                    })
                                    .min_by(|a, b| a.1.total_cmp(&b.1));
                                eprintln!(
                                    "STALL {map} seed {seed} tick {tick} slot {slot} team {} at {:.0?} pmf {:#x} gnd {} vel {:.0?} mate {:.0?}",
                                    team.map_or("?", String::as_str),
                                    at,
                                    p.ps.pm_flags,
                                    p.ps.ground_entity_num,
                                    <[f32; 3]>::from(p.ps.velocity),
                                    mate
                                );
                            }
                        }
                    }
                }
            }
            let log = level.rt.script_log();
            for l in log.get(seen..).unwrap_or_default() {
                let f: Vec<&str> = l.trim_end().split(';').collect();
                // guid, num, team, name for the victim and the attacker,
                // then weapon, damage, means of death, location.
                if f[0] == "A" && f.len() >= 6 {
                    *m.actions.entry(f[5].to_string()).or_default() += 1;
                    if verbose {
                        m.log.push(format!("tick {tick} {}", l.trim_end()));
                    }
                    continue;
                }
                if f.len() < 13 {
                    continue;
                }
                let (vnum, anum, ateam) = (f[2], f[6], f[7]);
                let def = level
                    .combat
                    .weapons
                    .iter()
                    .flatten()
                    .find(|d| d.name == f[9]);
                let class = if f[11] == "MOD_MELEE" {
                    "melee"
                } else {
                    class_of(def)
                };
                let num = |s: &str| s.parse::<usize>().ok();
                let attacker = num(anum).filter(|a| Some(*a) != num(vnum));
                let alevel = attacker.and_then(|a| level_of.get(&a).copied());
                match f[0] {
                    "D" if anum != "-1" && anum != vnum => {
                        m.hits += 1;
                        m.classes.entry(class).or_default().hits += 1;
                        m.weapons.entry(f[9].to_string()).or_default().hits += 1;
                        if let Some(l) = alevel {
                            m.skills.entry(l).or_default().hits += 1;
                        }
                        if let (Some(v), Some(a)) = (num(vnum), attacker) {
                            first_hit.entry((a, v)).or_insert(tick);
                        }
                        let (Ok(v), Ok(a)) = (vnum.parse::<usize>(), anum.parse::<usize>()) else {
                            continue;
                        };
                        let enemy = gametype == "dm" || f[3] != ateam;
                        if enemy && slots.contains(&v) && !pending.contains_key(&(v, a)) {
                            m.hurt += 1;
                            pending.insert((v, a), tick);
                        }
                    }
                    "K" => {
                        m.deaths += 1;
                        let v = num(vnum);
                        if let Some(l) = v.and_then(|v| level_of.get(&v)) {
                            m.skills.entry(*l).or_default().deaths += 1;
                        }
                        if let (Some(v), Some(a)) = (v, attacker) {
                            let ttk = first_hit
                                .remove(&(a, v))
                                .map_or(0, |t| (tick - t) as u64 * TICK_MS);
                            // The lethal hit logs `K` alone.
                            for c in [
                                m.classes.entry(class).or_default(),
                                m.weapons.entry(f[9].to_string()).or_default(),
                            ] {
                                c.kills += 1;
                                c.hits += 1;
                                c.ttk_ms.push(ttk);
                            }
                            if let Some(l) = alevel {
                                let s = m.skills.entry(l).or_default();
                                s.kills += 1;
                                s.hits += 1;
                                s.ttk_ms.push(ttk);
                            }
                        }
                        if let Some(v) = v {
                            first_hit.retain(|(_, victim), _| *victim != v);
                        }
                        if anum == "-1" || anum == vnum {
                            m.suicides += 1;
                        } else {
                            let team = if ateam.is_empty() { "none" } else { ateam };
                            *m.kills.entry(team.to_string()).or_default() += 1;
                            if f[9].starts_with("mg42_bipod") || f[9].starts_with("30cal") {
                                m.gun_kills += 1;
                            }
                        }
                        if verbose {
                            m.log.push(format!("tick {tick} {}", l.trim_end()));
                        }
                    }
                    _ => {}
                }
            }
            seen = log.len();
        }
        if verbose && let Some(l) = sv.level() {
            for e in l.rt.aborts() {
                m.log.push(format!("script error: {e}"));
            }
        }
        m.scores = sv
            .level_mut()
            .map_or([0, 0], |l| l.rt.with_host(|h, _| h.team_scores));
        m.throws = sv
            .bots_mut()
            .map_or(0, |b| b.bots().values().map(|b| b.throws()).sum());
        m.smokes = sv.bots_mut().map_or(0, |b| b.smokes());
        if let Some(b) = sv.bots_mut() {
            super::ObjSpread::add_cues(&mut m.obj, b.cue_tally());
        }
        if let Some(st) = &stats {
            st.print(map, gametype, seed);
        }
        Some(m)
    }
}

/// `GAME=cod4`: CoD4 1.7 matches through `vcod_server::cod4::Cod4Server`.
mod cod4 {
    use std::collections::{BTreeMap, HashMap, VecDeque};
    use std::time::{Duration, Instant};

    use vcod_server::cod4::{Cod4Config, Cod4Server};
    use vcod_server::engine::GameModule;
    use vcod_server::game::cod4::bot_feed::Cod4Objectives;

    use super::{Digest, RADIUS, TICK_MS, WINDOW, install_trace};

    #[derive(Default)]
    struct Cod4Match {
        map: String,
        seed: u64,
        ticks: usize,
        /// By the killer's team (`dm`: "none"), kills of another player.
        kills: BTreeMap<String, u32>,
        deaths: u32,
        /// Deaths with no other player the killer: suicides and falls.
        suicides: u32,
        hits: u32,
        /// Bullet rounds fired: a held bullet gun's clip drops.
        rounds: u32,
        playing: u64,
        stalled: u64,
        held: u64,
        ladder: u64,
        travelled: f64,
        /// Frag throws let go, mantles started.
        throws: u32,
        mantles: u32,
        /// Guns manned and live frags thrown back (`GUNS=0` and
        /// `THROWBACK=0` keep the bots off both).
        mounts: u32,
        throw_backs: u32,
        /// Levels run past the first: `fast_restart`s (a round's end, the
        /// low nibble of `sv_serverid` stepped) and spawns (a map loaded:
        /// the match ended and rotated), and the maps in run order.
        restarts: u32,
        spawns: u32,
        maps: Vec<String>,
        /// The script's `A;` actions and `W;` wins; on CoD4, whose
        /// `logString` writes nothing, the objectives' changes read off the
        /// scripts each tick ([`objective_actions`]).
        actions: BTreeMap<String, u32>,
        wins: BTreeMap<String, u32>,
        /// Every distinct script error (`ScriptRuntime::aborts`) and how
        /// often it came.
        errors: BTreeMap<String, u32>,
        /// Levels whose `game["state"]` reached `postgame` (`endGame`).
        ended: u32,
        /// A line per level run: how it began, its ticks, the bot-ticks
        /// playing, the kills, whether its match ended.
        levels: Vec<String>,
        /// Near the objective, by team ([`super::ObjSpread`]).
        obj: BTreeMap<String, super::ObjSpread>,
        log: Vec<String>,
        digest: Option<Digest>,
    }

    pub(super) fn main() {
        // The server's warnings at `RUST_LOG`'s level, none without it.
        let _ = env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("off"))
            .try_init();
        let env = |k: &str, d: &str| std::env::var(k).unwrap_or_else(|_| d.into());
        let maps: Vec<String> = env("MAPS", &env("MAP", "mp_crash"))
            .split(',')
            .map(str::to_string)
            .collect();
        let gametype = env("GAMETYPE", "war");
        let bots: usize = env("BOTS", "6").parse().expect("BOTS");
        let ticks: usize = env("TICKS", "6000").parse().expect("TICKS");
        let jobs: usize = env("JOBS", "4").parse().expect("JOBS");
        let verbose = env("VERBOSE", "0") == "1";
        let sets: Vec<(String, String)> = env("SET", "")
            .split(',')
            .filter_map(|p| p.split_once('='))
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        let mut seeds: Vec<u64> = std::env::args()
            .skip(1)
            .map(|s| s.parse().expect("seed"))
            .collect();
        if seeds.is_empty() {
            seeds = (1..=4).collect();
        }
        println!(
            "cod4 maps {} gametype {gametype} bots {bots} ticks {ticks}",
            maps.join(",")
        );
        let started = Instant::now();
        let queue = std::sync::Mutex::new(
            maps.iter()
                .rev()
                .flat_map(|m| seeds.iter().rev().map(move |s| (m.clone(), *s)))
                .collect::<Vec<_>>(),
        );
        let results = std::sync::Mutex::new(Vec::new());
        std::thread::scope(|s| {
            for _ in 0..jobs.max(1) {
                s.spawn(|| {
                    while let Some((map, seed)) = queue.lock().unwrap().pop() {
                        let m = run(&map, &gametype, bots, ticks, seed, verbose, &sets);
                        vcod_server::tick_trace::finish().expect("VCOD_TICK_TRACE");
                        let Some(m) = m else {
                            return;
                        };
                        eprintln!(
                            "{map} seed {seed} done ({:.0} s)",
                            started.elapsed().as_secs_f32()
                        );
                        results.lock().unwrap().push(m);
                    }
                });
            }
        });
        let mut results = results.into_inner().unwrap();
        if results.is_empty() {
            eprintln!("no CoD4 install (set COD4_DIR)");
            return;
        }
        results.sort_by(|a, b| (&a.map, a.seed).cmp(&(&b.map, b.seed)));
        let line = |m: &Cod4Match| {
            let kills: u32 = m.kills.values().sum();
            let errors: u32 = m.errors.values().sum();
            format!(
                "kills {kills} {:?} deaths {} suicides {} hits {} rounds {} acc {:.2} stalled {} held {} of {} ({:.1}%) ladder {} walked {:.0} u/bot-s throws {} mantles {} mounts {} throwbacks {} restarts {} spawns {} ended {} actions {:?} wins {:?} script errors {errors} ({} distinct)",
                m.kills,
                m.deaths,
                m.suicides,
                m.hits,
                m.rounds,
                f64::from(m.hits) / f64::from(m.rounds.max(1)),
                m.stalled,
                m.held,
                m.playing,
                100.0 * m.stalled as f64 / m.playing.max(1) as f64,
                m.ladder,
                m.travelled / (m.playing.max(1) as f64 * TICK_MS as f64 / 1000.0),
                m.throws,
                m.mantles,
                m.mounts,
                m.throw_backs,
                m.restarts,
                m.spawns,
                m.ended,
                m.actions,
                m.wins,
                m.errors.len(),
            )
        };
        let mut errors: BTreeMap<&str, u32> = BTreeMap::new();
        let mut all_obj: BTreeMap<String, super::ObjSpread> = BTreeMap::new();
        for m in &results {
            for (k, o) in &m.obj {
                println!("{} seed {}: obj {k} {}", m.map, m.seed, o.line());
                all_obj.entry(k.clone()).or_default().add(o);
            }
            for l in m.log.iter().chain(&m.levels) {
                println!("  {} seed {} {l}", m.map, m.seed);
            }
            println!(
                "{} seed {}: ticks {} maps {} {}",
                m.map,
                m.seed,
                m.ticks,
                m.maps.join(">"),
                line(m)
            );
            if let Some(d) = &m.digest {
                d.print(&m.map, &gametype, m.seed, m.ticks);
            }
            for (e, n) in &m.errors {
                *errors.entry(e).or_default() += n;
            }
        }
        for (k, o) in &all_obj {
            println!("all runs: obj {k} {}", o.line());
        }
        println!("{gametype} script errors: {} distinct", errors.len());
        for (e, n) in &errors {
            println!("  {n:6} {e}");
        }
    }

    /// What changed between two ticks' objective states, counted into
    /// `actions`: a bomb taken (`pickup`), planted, defused or blown up
    /// (`explode`, the bomb's timer run out), a zone taken by a team
    /// (`capture`).
    fn objective_actions(
        was: Option<&Cod4Objectives>,
        now: Option<&Cod4Objectives>,
        actions: &mut BTreeMap<String, u32>,
    ) {
        let mut count = |k: &str| *actions.entry(k.to_string()).or_default() += 1;
        match (was, now) {
            (Some(Cod4Objectives::Bomb(a)), Some(Cod4Objectives::Bomb(b))) => {
                if a.carrier.is_none() && b.carrier.is_some() && b.planted.is_none() {
                    count("pickup");
                }
                match (a.planted.is_some(), b.planted.is_some()) {
                    (false, true) => count("plant"),
                    (true, false) if b.exploded => count("explode"),
                    (true, false) => count("defuse"),
                    _ => {}
                }
            }
            (Some(Cod4Objectives::Zones(a)), Some(Cod4Objectives::Zones(b))) => {
                for z in b {
                    let before = a.iter().find(|y| y.bounds == z.bounds);
                    if z.team != 0 && before.is_some_and(|y| y.team != z.team) {
                        count("capture");
                    }
                }
            }
            _ => {}
        }
    }

    /// The weapon in hand, its clip, and whether it fires bullets.
    fn clip_of(
        level: &vcod_server::game::cod4::level::Cod4Level,
        slot: usize,
    ) -> Option<(u32, i32, bool)> {
        let ps = level.pm_state(slot)?;
        let d = level.weapon_defs().get(ps.weapon as usize)?;
        Some((
            ps.weapon,
            ps.ammoclip.get(d.clip_index).copied().unwrap_or(0),
            d.weap_type == 0 && d.offhand_class == 0,
        ))
    }

    fn run(
        map: &str,
        gametype: &str,
        bots: usize,
        ticks: usize,
        seed: u64,
        verbose: bool,
        sets: &[(String, String)],
    ) -> Option<Cod4Match> {
        let install = vcod_cod4::testing::cod4_install()?;
        let leave: Vec<usize> = std::env::var("LEAVE")
            .unwrap_or_default()
            .split(',')
            .filter_map(|t| t.trim().parse().ok())
            .collect();
        let mut now = Instant::now();
        let mut sv = Cod4Server::new(
            Cod4Config {
                map: map.to_string(),
                gametype: gametype.to_string(),
                hostname: "match".into(),
                max_clients: (bots + 2).max(8),
            },
            seed * 2 + 1,
        );
        for (k, v) in sets {
            sv.set_cvar(k, v);
        }
        sv.load(Some(&install)).expect("load the level");
        let slots = sv.add_bots(bots, true);
        if let Some(b) = sv.bots_mut() {
            b.wait_for_graph();
            b.objectives = std::env::var("OBJ").as_deref() != Ok("0");
            b.man_guns = std::env::var("GUNS").as_deref() != Ok("0");
            b.throw_back = std::env::var("THROWBACK").as_deref() != Ok("0");
        }
        let mut last_obj = None;
        // `STALLS=1`: every 20th tick of a stalled sample to stderr.
        let stall_log = std::env::var("STALLS").as_deref() == Ok("1");
        let mut m = Cod4Match {
            map: map.to_string(),
            seed,
            digest: Digest::from_env(),
            maps: vec![map.to_string()],
            ..Default::default()
        };
        install_trace("cod4", map, gametype, seed);
        // `SPIKE_MS=<ms>`: each tick past it to stderr with its phases
        // (`vcod_server::tick_profile`).
        let spike_ms: Option<f32> = std::env::var("SPIKE_MS").ok().and_then(|v| v.parse().ok());
        if spike_ms.is_some() {
            vcod_server::tick_profile::enable();
        }
        let mut stats = super::TickStats::from_env();
        let mut seen = 0;
        let mut errors_seen = 0;
        let mut level_id = sv.server_id();
        let mut hist: HashMap<usize, VecDeque<([f32; 3], bool)>> = HashMap::new();
        let mut trail: HashMap<usize, VecDeque<[f32; 3]>> = HashMap::new();
        let mut last: HashMap<usize, ([f32; 3], u32, i32)> = HashMap::new();
        let mut mantling: HashMap<usize, bool> = HashMap::new();
        // The level under way: how it began, its first tick, its playing
        // bot-ticks and kills, whether its match ended.
        let mut lv = ("start", 0usize, 0u64, 0u32, None::<usize>);
        let close = |m: &mut Cod4Match, lv: (&str, usize, u64, u32, Option<usize>), tick: usize| {
            m.ended += u32::from(lv.4.is_some());
            m.levels.push(format!(
                "level {} {} ({}) ticks {}..{tick}: bot-ticks playing {} kills {} match ended {}",
                m.levels.len(),
                m.maps.last().map_or("", String::as_str),
                lv.0,
                lv.1,
                lv.2,
                lv.3,
                lv.4.map_or("no".to_string(), |t| format!("at tick {t}")),
            ));
        };
        for tick in 0..ticks {
            now += Duration::from_millis(TICK_MS);
            // `TACTICS=0` (or `none`): every bot guards from the first spot in reach
            // (`Bot::tactics`); `CONVERGE=0`: no S&D defender leaves its
            // site for one under attack (`Bot::converge`); `LANES=0`: every
            // bot walks straight to its objective (`Bot::lanes`); `FAR=0`,
            // `CUES=0`, `TRAVEL=0` turn off bot-objectives.md 14's rules.
            if let Some(b) = sv.bots_mut() {
                let off = |k: &str| matches!(std::env::var(k).as_deref(), Ok("0" | "none"));
                let arms = [
                    !off("TACTICS"),
                    !off("CONVERGE"),
                    !off("LANES"),
                    !off("FAR"),
                    !off("CUES"),
                    !off("TRAVEL"),
                ];
                for bot in b.bots_mut().values_mut() {
                    bot.tactics &= arms[0];
                    bot.converge &= arms[1];
                    bot.lanes &= arms[2];
                    bot.far &= arms[3];
                    bot.cues &= arms[4];
                    bot.travel &= arms[5];
                }
            }
            // The script's `map_restart` and `exitLevel` run off the
            // console, as `main.rs`'s loop drains it ahead of the frame.
            if leave.contains(&tick) {
                sv.core.console.push_back("removebot 1".into());
            }
            let sched = spike_ms.and_then(|_| vcod_server::tick_profile::sched());
            let t0 = Instant::now();
            <vcod_server::cod4::Cod4Game as GameModule>::drain_console(&mut sv, now);
            sv.tick_frames(now, 1);
            let tick_ms = t0.elapsed().as_secs_f32() * 1000.0;
            let phases = vcod_server::tick_profile::take();
            if let Some(st) = stats.as_mut() {
                st.add(tick, tick_ms, &phases);
            }
            if spike_ms.is_some_and(|s| tick_ms > s) {
                eprintln!(
                    "spike {map} {gametype} seed {seed} tick {tick} {tick_ms:.1} ms ({}): {}",
                    vcod_server::tick_profile::split(sched, tick_ms),
                    vcod_server::tick_profile::format(&phases, 1.0)
                );
            }
            sv.take_outgoing();
            m.ticks = tick + 1;
            if tick == 0
                && let Some(g) = sv.bots_mut().and_then(|b| b.graph())
            {
                eprintln!(
                    "{map} seed {seed}: graph {} nodes, {} edges, {} mantles",
                    g.len(),
                    g.edge_count(),
                    g.mantles.len()
                );
            }
            let id = sv.server_id();
            if id != level_id {
                close(&mut m, lv, tick);
                // A restart keeps the high nibble (`restart_server_id`).
                if id & 0xf0 == level_id & 0xf0 {
                    m.restarts += 1;
                    lv = ("fast_restart", tick, 0, 0, None);
                } else {
                    m.spawns += 1;
                    m.maps.push(sv.cvar("mapname").to_string());
                    lv = ("map loaded", tick, 0, 0, None);
                }
                level_id = id;
                seen = 0;
                errors_seen = 0;
                last.clear();
                hist.clear();
                last_obj = None;
            }
            let obj = sv.level_mut()?.objectives();
            objective_actions(last_obj.as_ref(), obj.as_ref(), &mut m.actions);
            last_obj = obj;
            // Per bot: holding still on purpose (a fight, cover, a retreat,
            // its objective, as CoD2's), and the team it asked for (the
            // `K;` lines leave the teams empty, `_globallogic.gsc`).
            // Per bot: its objective's point, the share of its ways in that see it, on
            // a cover spot (`ObjSpread`).
            let objinfo: HashMap<usize, super::ObjInfo> = sv
                .bots_mut()
                .map(|b| {
                    b.bots()
                        .iter()
                        .map(|(s, b)| {
                            let info = (
                                b.objective_point(),
                                b.exposure(),
                                b.holds_cover(),
                                b.guard_point(),
                            );
                            (*s, info)
                        })
                        .collect()
                })
                .unwrap_or_default();
            let state: HashMap<usize, (bool, String)> = sv
                .bots_mut()
                .map(|b| {
                    b.bots()
                        .iter()
                        .map(|(s, b)| {
                            let holds = b.engaged()
                                || b.holds_cover()
                                || b.retreating() != 0
                                || b.on_objective();
                            let team = match b.team() {
                                "allies" | "axis" if gametype != "dm" => b.team(),
                                _ => "none",
                            };
                            (*s, (holds, team.to_string()))
                        })
                        .collect()
                })
                .unwrap_or_default();
            if (tick % 20 == 19 || tick + 1 == ticks)
                && sv.level_mut()?.game_value("state") == "postgame"
            {
                lv.4.get_or_insert(tick);
            }
            let level = sv.level()?;
            if tick % 20 == 19 || tick + 1 == ticks {
                let errs = level.rt.aborts();
                for e in errs.get(errors_seen..).unwrap_or_default() {
                    // The thread's own numbers vary; the place does not.
                    *m.errors.entry(e.clone()).or_default() += 1;
                }
                errors_seen = errs.len().max(errors_seen);
            }
            let samples: Vec<super::ObjSample> = slots
                .iter()
                .filter_map(|&s| {
                    let ps = level.pm_state(s)?;
                    let (point, exposure, cover, guard) = *objinfo.get(&s)?;
                    let team = state.get(&s)?.1.clone();
                    (ps.pm_type == 0
                        && ps.other_flags & vcod_cod4::pmove::POF_PLAYER != 0
                        && !level.rt.client_vitals(s).dead
                        && team != "none")
                        .then(|| super::ObjSample {
                            slot: s,
                            team,
                            at: ps.origin.into(),
                            point,
                            exposure,
                            cover,
                            guard,
                        })
                })
                .collect();
            super::ObjSpread::feed(&mut m.obj, &samples, &mut trail);
            for &slot in &slots {
                let Some(ps) = level.pm_state(slot) else {
                    continue;
                };
                // A follower is sent the followed client's playerstate.
                let playing = ps.pm_type == 0
                    && ps.other_flags & vcod_cod4::pmove::POF_PLAYER != 0
                    && !level.rt.client_vitals(slot).dead;
                let at: [f32; 3] = ps.origin.into();
                let clip = clip_of(level, slot);
                if let (Some((was, w, c)), Some((w2, c2, bullet))) = (last.get(&slot), clip)
                    && playing
                {
                    if *w == w2 && c2 < *c && bullet {
                        m.rounds += (c - c2) as u32;
                    }
                    let d = (glam::Vec3::from(at) - glam::Vec3::from(*was))
                        .truncate()
                        .length();
                    if d < 200.0 {
                        m.travelled += f64::from(d);
                    }
                }
                if let Some((w, c, _)) = clip {
                    last.insert(slot, (at, w, c));
                }
                let h = hist.entry(slot).or_default();
                if !playing {
                    h.clear();
                    continue;
                }
                m.playing += 1;
                lv.2 += 1;
                let mantle = ps.pm_flags & vcod_cod4::pmove::pmf::MANTLE != 0;
                if mantle && !mantling.get(&slot).copied().unwrap_or(false) {
                    m.mantles += 1;
                }
                mantling.insert(slot, mantle);
                if ps.pm_flags & vcod_cod4::pmove::pmf::LADDER != 0 {
                    m.ladder += 1;
                }
                // A body the script froze (`freezeControls`, a round's end)
                // holds still by the rules, not by a stall.
                let frozen = ps.pm_flags & vcod_cod4::pmove::pmf::FROZEN != 0;
                h.push_back((at, frozen || state.get(&slot).is_some_and(|s| s.0)));
                if h.len() > WINDOW {
                    h.pop_front();
                }
                if h.len() == WINDOW {
                    let here = glam::Vec3::from(at);
                    let still = h
                        .iter()
                        .all(|(o, _)| glam::Vec3::from(*o).distance(here) < RADIUS);
                    if still {
                        if h.iter().any(|(_, e)| *e) {
                            m.held += 1;
                        } else {
                            m.stalled += 1;
                            if stall_log && tick % 20 == 0 {
                                // The nearest teammate: where it stands, how
                                // far and how fast (a queue behind a guard).
                                let team = state.get(&slot).map(|s| s.1.clone());
                                let mate = slots
                                    .iter()
                                    .filter(|&&o| {
                                        o != slot && state.get(&o).map(|s| &s.1) == team.as_ref()
                                    })
                                    .filter_map(|&o| Some((o, level.pm_state(o)?)))
                                    .map(|(o, p)| {
                                        (
                                            o,
                                            p.origin.distance(ps.origin),
                                            p.origin,
                                            p.velocity.length(),
                                        )
                                    })
                                    .min_by(|a, b| a.1.total_cmp(&b.1));
                                eprintln!(
                                    "STALL {map} seed {seed} tick {tick} slot {slot} team {} at {:.0?} pmf {:#x} gnd {} vel {:.0?} mantle {:?} mate {:.0?}",
                                    team.as_deref().unwrap_or("?"),
                                    at,
                                    ps.pm_flags,
                                    ps.ground_entity_num,
                                    ps.velocity,
                                    ps.mantle,
                                    mate
                                );
                            }
                        }
                    }
                }
            }
            if let Some(d) = m.digest.as_mut() {
                d.cod4(level, &slots, tick);
            }
            let log = level.rt.script_log();
            for l in log.get(seen..).unwrap_or_default() {
                let f: Vec<&str> = l.trim_end().split(';').collect();
                match f[0] {
                    "A" if f.len() >= 6 => {
                        *m.actions.entry(f[5].to_string()).or_default() += 1;
                    }
                    "W" | "L" if f.len() >= 2 => {
                        if f[0] == "W" {
                            *m.wins.entry(f[1].to_string()).or_default() += 1;
                        }
                    }
                    "D" if f.len() >= 13 && f[6] != "-1" && f[6] != f[2] => m.hits += 1,
                    "K" if f.len() >= 13 => {
                        m.deaths += 1;
                        if f[6] == "-1" || f[6] == f[2] {
                            m.suicides += 1;
                        } else {
                            let team = f[6]
                                .parse::<usize>()
                                .ok()
                                .and_then(|a| state.get(&a))
                                .map_or("none", |s| s.1.as_str());
                            *m.kills.entry(team.to_string()).or_default() += 1;
                            m.hits += 1;
                            lv.3 += 1;
                        }
                        if verbose {
                            m.log.push(format!("tick {tick} {}", l.trim_end()));
                        }
                    }
                    _ => {}
                }
            }
            seen = log.len();
        }
        close(&mut m, lv, ticks);
        m.throws = sv
            .bots_mut()
            .map_or(0, |b| b.bots().values().map(|b| b.throws()).sum());
        (m.mounts, m.throw_backs) = sv.bots_mut().map_or((0, 0), |b| b.gun_counts());
        if let Some(b) = sv.bots_mut() {
            super::ObjSpread::add_cues(&mut m.obj, b.cue_tally());
        }
        if let Some(st) = &stats {
            st.print(map, gametype, seed);
        }
        Some(m)
    }
}
