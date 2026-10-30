//! `cod4_moves_ab`: retail 1.7's movement against `vcod_cod4::pmove`. Each
//! capture's own usercmds (the client's `clc_move` ops) are replayed cmd
//! for cmd from the snapshot of its last spawn on the capture's map, and
//! every later snapshot's pmove fields are compared with the wire's words
//! as the field's width carries them (an angle as its short).
//!
//! Two replays per capture. The free run never looks back at retail: its
//! first divergence is the report. The resynced run takes each snapshot's
//! state as the next row's start (keeping only `oldVelocity`, which is not
//! on the wire), so each row tests one snapshot's worth of cmds; a field it
//! finds wrong must be named in [`GAPS`]. Needs `COD4_DIR`.
//!
//! The kill capture's shooter replays armed: the weapon half runs on the
//! level's weapon defs and its words ([`COMPARED_WEAPON`], the clip and the
//! event ring) are compared too, from the first snapshot after its
//! `setviewpos` (cod4-pmove.md 16).
//!
//! `MOVES_REPORT=1` prints every mismatching row; `MOVES_TRACE=walk:resync:120`
//! prints that row's cmds and the state ours reached.
//! Recipes: `play-1.7-walk.txt` is docs/research/cod4-netfields.md 8,
//! `play-1.7-kill-shooter.txt` section 10 there, `play-1.7-fly.txt` and
//! `play-1.7-mantle.txt` (two mantles and the leans, cod4-pmove.md 17)
//! and `play-1.7-firerate.txt` / `-firerate-rof.txt` (an AK-47 held on
//! the trigger, cod4-combat.md 2.1) and `play-1.7-lunge.txt` (a melee
//! charge, cod4-pmove.md 19) carry their own in their headers. The mantle clips come out of
//! `common_mp` (`pmove::mantle::install`).

use std::collections::BTreeMap;

use vcod_cod4::collision::ClipMap;
use vcod_cod4::map::Map;
use vcod_cod4::net::capture::{self, Item, Session};
use vcod_cod4::net::clmsg::ClOp;
use vcod_cod4::net::delta;
use vcod_cod4::net::fields::PLAYER_STATE;
use vcod_cod4::net::parse::Op;
use vcod_cod4::pmove::weapon::WeaponDef;
use vcod_cod4::pmove::{self, Cmd, PlayerState, Rules, WeaponMove};
use vcod_cod4::testing::cod4_install;
use vcod_cod4::walk::{WalkOptions, walk};
use vcod_cod4::zone::Zone;

const FIXTURES: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/net");

/// The captures: (name, file, map, from this server time, armed).
const CAPTURES: &[(&str, &str, &str, Option<i32>, bool)] = &[
    ("walk", "play-1.7-walk.txt", "mp_crash", None, false),
    ("fly", "play-1.7-fly.txt", "mp_crash", None, false),
    // The walker armed from just before its frag key (64900).
    ("frag", "play-1.7-walk.txt", "mp_crash", Some(64500), true),
    (
        "shooter",
        "play-1.7-kill-shooter.txt",
        "mp_crash",
        Some(47550),
        true,
    ),
    // An AK-47 held on the trigger, plain and under Double Tap, from the
    // snapshot after the probe gsc armed it (cod4-combat.md 2.1).
    (
        "autofire",
        "play-1.7-firerate.txt",
        "mp_crash",
        Some(AUTOFIRE_FROM),
        true,
    ),
    (
        "doubletap",
        "play-1.7-firerate-rof.txt",
        "mp_crash",
        Some(DOUBLETAP_FROM),
        true,
    ),
    // From the snapshot after the `setviewpos` landed.
    (
        "mantle",
        "play-1.7-mantle.txt",
        "mp_crash",
        Some(47100),
        true,
    ),
    // From a second before the melee press (52250): the charge pick's
    // lunge (cod4-pmove.md 19).
    ("lunge", "play-1.7-lunge.txt", "mp_crash", Some(51250), true),
];

/// The fire rate captures' first rows: the snapshot after the probe gsc's
/// `armed` line (25100 and 30250 in its log) settled.
const DOUBLETAP_FROM: i32 = 25200;
const AUTOFIRE_FROM: i32 = 30350;

/// `CS_WEAPONS`: the level's weapon names, weapon 1 first.
const CS_WEAPONS: u16 = 2258;

/// What the resynced replay still gets wrong: (capture, field, rows, why).
/// The rows are exact: a fix that rights one fails the gate until the
/// entry shrinks. Each reason names its section of
/// docs/research/cod4-pmove.md.
const GAPS: &[(&str, &str, usize, &str)] = &[
    // A last bit of z on mp_crash's terrain and the sign of a velocity's
    // zero after it, pressed into a wall on a slope: the float model of
    // the trace kernels (NEEDS-M6).
    ("walk", "origin[2]", 26, "13.2 the float model"),
    ("walk", "velocity[2]", 11, "13.2 the float model"),
    // Rows 499 and 505..508: the step up and back down a 0.78 brush slope
    // lands 0.01 off retail's, once.
    ("walk", "origin[0]", 3, "13.2 the steep step"),
    ("walk", "origin[1]", 1, "13.2 the steep step"),
    // The armed walk's tail shares the walk's steep step and terrain bits.
    ("frag", "origin[0]", 3, "13.2 the steep step"),
    ("frag", "origin[1]", 1, "13.2 the steep step"),
    ("frag", "origin[2]", 4, "13.2 the float model"),
    // The throw's cmd: the server's grenade timer init zeroes the fuse
    // inside `ClientEvents` (cod4-combat.md 10.1), which pmove alone cannot.
    (
        "frag",
        "grenadeTimeLeft",
        1,
        "cod4-combat.md 10.1, the server's fuse",
    ),
    // The free spectator's unsnapped velocity a bit off twice: the
    // friction's leftover at a stop (row 132) and a turn (row 244).
    ("fly", "velocity[0]", 2, "13.2 the float model"),
    ("fly", "velocity[1]", 2, "13.2 the float model"),
    // The walk off over the terrain: a bit of the height.
    ("mantle", "origin[2]", 7, "13.2 the float model"),
    // The charge swing's hit: the server's `EV_MELEE_BLOOD` (37) on the
    // attacker, and the lunge carries him over the victim's dropped rifle,
    // whose ammo the server's touch hands him (`EV_AMMO_PICKUP`, 9).
    (
        "lunge",
        "eventSequence",
        1,
        "19 the server's melee and pickup",
    ),
    ("lunge", "events[0]", 1, "19 the server's melee and pickup"),
    ("lunge", "events[3]", 1, "19 the server's melee and pickup"),
    (
        "lunge",
        "eventParms[0]",
        1,
        "19 the server's melee and pickup",
    ),
    ("lunge", "ammo", 1, "19 the server's melee and pickup"),
];

/// The playerstate words pmove writes, as the wire names them.
const COMPARED: &[&str] = &[
    "commandTime",
    "pm_type",
    "pm_flags",
    "pm_time",
    "origin[0]",
    "origin[1]",
    "origin[2]",
    "velocity[0]",
    "velocity[1]",
    "velocity[2]",
    "delta_angles[0]",
    "delta_angles[1]",
    "delta_angles[2]",
    "groundEntityNum",
    "jumpTime",
    "jumpOriginZ",
    "movementDir",
    "leanf",
    "viewHeightTarget",
    "viewHeightCurrent",
    "viewHeightLerpTime",
    "viewHeightLerpTarget",
    "viewHeightLerpDown",
    "bobCycle",
    "proneDirection",
    "sprintState.lastSprintStart",
    "sprintState.lastSprintEnd",
    "sprintState.sprintStartMaxLength",
    "sprintState.sprintButtonUpRequired",
    "sprintState.sprintDelay",
    "holdBreathScale",
    "mantleState.yaw",
    "mantleState.timer",
    "mantleState.transIndex",
    "mantleState.flags",
];

/// The weapon half's words, compared on an armed capture.
const COMPARED_WEAPON: &[&str] = &[
    "weapon",
    "weaponstate",
    "weaponTime",
    "weaponDelay",
    "weaponShotCount",
    "weapAnim",
    "weapFlags",
    "fWeaponPosFrac",
    "aimSpreadScale",
    "weaponRestrictKickTime",
    "offHandIndex",
    "grenadeTimeLeft",
    "throwBackGrenadeOwner",
    "eventSequence",
    "events[0]",
    "events[1]",
    "events[2]",
    "events[3]",
    "eventParms[0]",
    "eventParms[1]",
    "eventParms[2]",
    "eventParms[3]",
];

/// `eFlags` bits pmove owns (crouch, prone, talk); the firing bit rides
/// the weapon half's ammo test and the rest are the server's.
const EFLAGS_PMOVE: u32 = 0x20_000c;
/// With the weapon half, the firing bit too.
const EFLAGS_ARMED: u32 = 0x20_004c;

struct Capture {
    weapons: Vec<String>,
    cmds: Vec<Cmd>,
    snaps: Vec<(i32, delta::PlayerState)>,
}

fn read(file: &str) -> Capture {
    let text = std::fs::read_to_string(format!("{FIXTURES}/{file}")).unwrap();
    let lines = capture::parse(&text).unwrap();
    let mut s = Session::new();
    let (mut cmds, mut snaps) = (Vec::new(), Vec::new());
    for l in &lines {
        match s.feed(l).unwrap() {
            Some(Item::Client { msg, .. }) => {
                for op in &msg.ops {
                    if let ClOp::Move { cmds: c, .. } = op {
                        cmds.extend(c.iter().map(|(c, _)| Cmd::from(c)));
                    }
                }
            }
            Some(Item::Server { parsed, .. }) => {
                for op in &parsed.ops {
                    if let Op::Snapshot(i) = op {
                        let sn = &s.client.snapshots[*i];
                        snaps.push((sn.server_time, sn.ps.clone()));
                    }
                }
            }
            None => {}
        }
    }
    let gs = s.client.gamestate.as_ref().expect("a gamestate");
    let weapons = String::from_utf8_lossy(gs.configstring(CS_WEAPONS))
        .split_whitespace()
        .map(str::to_string)
        .collect();
    Capture {
        weapons,
        cmds,
        snaps,
    }
}

fn field(name: &str) -> &'static vcod_cod4::net::delta::Field {
    PLAYER_STATE.iter().find(|f| f.name == name).unwrap()
}

/// The 16-bit angle a float goes out as: rounded, not truncated (a spawn
/// delta of 65.0 reads 11833 on the wire; cod4-pmove.md 13).
fn angle_short(bits: u32) -> u16 {
    (f64::from(f32::from_bits(bits)) * (65536.0 / 360.0)).round() as i64 as u16
}

/// The short a server's float goes out as (`delta::angle16_sent`).
fn sent_short(bits: u32) -> u16 {
    angle_short(delta::angle16_sent(bits))
}

/// A float the server could hold behind a decoded angle: the decoded one
/// where it goes out as itself, else (a negative short, which the writer's
/// half rounds a short nearer zero) the one a short further from zero.
fn sent_float(a: f32) -> f32 {
    let u = angle_short(a.to_bits());
    if sent_short(a.to_bits()) == u {
        return a;
    }
    ((f64::from(u as i16) - 1.0) * (360.0 / 65536.0)) as f32
}

/// The word as the field's code carries it.
fn on_wire(name: &str, v: u32, armed: bool) -> u32 {
    if name == "eFlags" {
        return v & if armed { EFLAGS_ARMED } else { EFLAGS_PMOVE };
    }
    match field(name).bits {
        -100 | -87 => u32::from(angle_short(v)),
        b if b > 0 && b < 32 => v & ((1u32 << b) - 1),
        _ => v,
    }
}

fn word(ps: &delta::PlayerState, name: &str) -> u32 {
    ps.words[field(name).word()]
}

fn diff(
    ours: &delta::PlayerState,
    retail: &delta::PlayerState,
    armed: bool,
) -> Vec<(&'static str, u32, u32)> {
    let weapon: &[&'static str] = if armed { COMPARED_WEAPON } else { &[] };
    let mut d: Vec<_> = COMPARED
        .iter()
        .chain(weapon)
        .chain(&["eFlags"])
        .filter_map(|&n| {
            let a = match field(n).bits {
                -100 | -87 => u32::from(sent_short(word(ours, n))),
                _ => on_wire(n, word(ours, n), armed),
            };
            let b = on_wire(n, word(retail, n), armed);
            (a != b).then_some((n, a, b))
        })
        .collect();
    if armed {
        if ours.ammoclip != retail.ammoclip {
            let k = (0..128)
                .find(|&k| ours.ammoclip[k] != retail.ammoclip[k])
                .unwrap();
            d.push((
                "ammoclip",
                ours.ammoclip[k] as u32,
                retail.ammoclip[k] as u32,
            ));
        }
        if ours.ammo != retail.ammo {
            let k = (0..64).find(|&k| ours.ammo[k] != retail.ammo[k]).unwrap();
            d.push(("ammo", ours.ammo[k] as u32, retail.ammo[k] as u32));
        }
    }
    d
}

fn show(name: &str, v: u32) -> String {
    if name.starts_with("ammo") {
        return format!("{}", v as i32);
    }
    match field(name).bits {
        0 | -88 | -89 | -99 => format!("{} ({v:08x})", f32::from_bits(v)),
        _ => format!("{} ({v:x})", v as i32),
    }
}

#[derive(Default)]
struct Outcome {
    rows: usize,
    cmds: usize,
    free_matching: usize,
    free_first: Option<String>,
    /// The free run's largest origin error, in units, and its row.
    free_max_err: (f32, usize),
    resync_matching: usize,
    resync_fields: BTreeMap<&'static str, (usize, String)>,
}

struct Replay<'a> {
    cap: &'a Capture,
    world: &'a ClipMap,
    weapons: &'a [WeaponMove],
    rules: Rules,
    /// The level's weapon defs by number, for an armed capture.
    defs: Option<&'a Vec<WeaponDef>>,
    /// The first row's server time, where the capture names one.
    from: Option<i32>,
}

impl Replay<'_> {
    fn weapon(&self, ps: &PlayerState) -> WeaponMove {
        (ps.weapon as usize)
            .checked_sub(1)
            .and_then(|i| self.weapons.get(i).copied())
            .unwrap_or(WeaponMove::NONE)
    }

    /// The snapshot of the last spawn: the last one whose `commandTime`
    /// no cmd carries. A spectator that never spawned starts at its first
    /// snapshot free to move (`speed` set).
    fn start(&self) -> usize {
        if let Some(t) = self.from {
            return (0..self.cap.snaps.len())
                .find(|&k| self.cap.snaps[k].0 >= t)
                .expect("a row from the start time");
        }
        let cmd_at = |t: i32| self.cap.cmds.iter().any(|c| c.server_time == t);
        let snap = |k: usize| &self.cap.snaps[k].1;
        let alive = |k: &usize| matches!(word(snap(*k), "pm_type"), 0 | 4);
        (0..self.cap.snaps.len())
            .rev()
            .filter(alive)
            .filter(|&k| word(snap(k), "speed") != 0)
            .find(|&k| !cmd_at(word(snap(k), "commandTime") as i32))
            .or_else(|| {
                (0..self.cap.snaps.len())
                    .find(|&k| word(snap(k), "pm_type") == 4 && word(snap(k), "speed") != 0)
            })
            .expect("a spawn")
    }

    /// The free run's first `rows` rows from `start` under `delta`:
    /// how many match retail before the first that does not.
    fn exact_prefix(&self, start: usize, delta: [f32; 3], rows: usize) -> usize {
        let mut ps = PlayerState::from_wire(&self.cap.snaps[start].1);
        ps.delta_angles = delta;
        let mut old = self.old_cmd(ps.command_time);
        for (n, (_, retail)) in self.cap.snaps.iter().skip(start + 1).take(rows).enumerate() {
            let ct = word(retail, "commandTime") as i32;
            let from = ps.command_time;
            for c in self
                .cap
                .cmds
                .iter()
                .filter(|c| c.server_time > from && c.server_time <= ct)
            {
                self.step(&mut ps, c, &mut old);
            }
            if !diff(&ps.to_wire(retail), retail, self.defs.is_some()).is_empty() {
                return n;
            }
        }
        rows
    }

    /// `delta_angles` as the server holds it. The wire carries a rounded
    /// short (`-100`), the server a float within half a short's step, which
    /// turns every view the cmds build: the yaw is the float in that step
    /// whose free run matches retail longest, the wire's own on a tie
    /// (docs/research/cod4-pmove.md 13).
    fn fit_delta(&self, start: usize) -> [f32; 3] {
        const ROWS: usize = 60;
        let ps = PlayerState::from_wire(&self.cap.snaps[start].1);
        let base = ps.delta_angles.map(sent_float);
        let mut best = (self.exact_prefix(start, base, ROWS), base);
        let wire = angle_short(ps.delta_angles[1].to_bits());
        for dir in [1i64, -1] {
            for n in 1i64.. {
                if best.0 == ROWS {
                    return best.1;
                }
                let mut d = base;
                d[1] = f32::from_bits((i64::from(d[1].to_bits()) + dir * n) as u32);
                if sent_short(d[1].to_bits()) != wire {
                    break;
                }
                let m = self.exact_prefix(start, d, ROWS);
                if m > best.0 {
                    best = (m, d);
                }
            }
        }
        best.1
    }

    /// One cmd, armed when the capture is.
    fn step(&self, ps: &mut PlayerState, c: &Cmd, old: &mut Cmd) {
        let w = self.weapon(ps);
        match self.defs {
            Some(defs) => {
                pmove::player_step_armed(ps, c, old, self.world, self.rules, w, defs);
            }
            None => {
                pmove::player_step(ps, c, old, self.world, self.rules, w);
            }
        }
    }

    /// The cmd `ClientThink` holds as `oldcmd` at `time`.
    fn old_cmd(&self, time: i32) -> Cmd {
        *self
            .cap
            .cmds
            .iter()
            .rev()
            .find(|c| c.server_time <= time)
            .unwrap_or(&Cmd::default())
    }

    fn run(&self, resync: bool, out: &mut Outcome, report: bool, trace: Option<usize>) {
        let cap = self.cap;
        let start = self.start();
        let mut ps = PlayerState::from_wire(&cap.snaps[start].1);
        ps.delta_angles = self.fit_delta(start);
        let mut old = self.old_cmd(ps.command_time);
        let mut free_ok = true;
        let mut ran = 0;
        for (k, (server_time, retail)) in cap.snaps.iter().enumerate().skip(start + 1) {
            let ct = word(retail, "commandTime") as i32;
            let from = ps.command_time;
            for c in cap
                .cmds
                .iter()
                .filter(|c| c.server_time > from && c.server_time <= ct)
            {
                if trace == Some(k) {
                    eprintln!(
                        "  cmd {c:?}\n    from origin {:?} velocity {:?} ground {} flags {:x} delta {:?} view {:?}",
                        ps.origin,
                        ps.velocity,
                        ps.ground_entity_num,
                        ps.pm_flags,
                        ps.delta_angles,
                        ps.view_angles
                    );
                }
                self.step(&mut ps, c, &mut old);
                ran += 1;
            }
            if trace == Some(k) {
                eprintln!("  ours origin {:?} velocity {:?}", ps.origin, ps.velocity);
            }
            let ours = ps.to_wire(retail);
            let d = diff(&ours, retail, self.defs.is_some());
            let row = format!("row {k} (serverTime {server_time}, commandTime {ct})");
            if !resync {
                out.rows += 1;
                out.cmds = ran;
                if free_ok && d.is_empty() {
                    out.free_matching += 1;
                } else if free_ok {
                    free_ok = false;
                    let (n, a, b) = d[0];
                    out.free_first = Some(format!(
                        "{row}: {n} ours {} retail {}{}",
                        show(n, a),
                        show(n, b),
                        if d.len() > 1 {
                            format!(" (+{} more fields)", d.len() - 1)
                        } else {
                            String::new()
                        }
                    ));
                }
                let r = PlayerState::from_wire(retail);
                let err = (ps.origin - r.origin).length();
                if err > out.free_max_err.0 {
                    out.free_max_err = (err, k);
                }
                continue;
            }
            if d.is_empty() {
                out.resync_matching += 1;
            } else if report {
                let fields: Vec<String> = d
                    .iter()
                    .map(|&(n, a, b)| format!("{n} {} vs {}", show(n, a), show(n, b)))
                    .collect();
                eprintln!("  {row}: {}", fields.join(", "));
            }
            for &(n, a, b) in &d {
                let e = out.resync_fields.entry(n).or_insert_with(|| {
                    (
                        0,
                        format!("{row}: ours {} retail {}", show(n, a), show(n, b)),
                    )
                });
                e.0 += 1;
            }
            let keep = (ps.old_velocity, ps.delta_angles);
            ps = PlayerState::from_wire(retail);
            ps.old_velocity = keep.0;
            for i in 0..3 {
                // The float behind the wire's short, while it still is.
                if sent_short(keep.1[i].to_bits()) == angle_short(ps.delta_angles[i].to_bits()) {
                    ps.delta_angles[i] = keep.1[i];
                }
            }
        }
    }
}

/// The level's weapon defs by number (0 the empty hand), each ammo and
/// clip slot the first earlier weapon's of its name, as the server's
/// registration gives them (`BG_SetupAmmoIndexes`,
/// `game::cod4::weapons`).
fn weapon_defs(
    install: &vcod_cod4::install::Install,
    map: &str,
    names: &[String],
) -> Vec<WeaponDef> {
    let mut found: BTreeMap<String, (WeaponDef, String, String)> = BTreeMap::new();
    for zone in [map, "common_mp"] {
        let path = install.zone("english", zone);
        let z = Zone::open_path(&path).unwrap();
        let w = walk(z, WalkOptions { keep_bulk: false }).unwrap();
        for d in &w.assets.weapons {
            if let Some(def) = WeaponDef::of(d) {
                let s = |k: &str| d.strings.get(k).cloned().unwrap_or_default();
                found.entry(d.name.to_ascii_lowercase()).or_insert((
                    def,
                    s("ammoName"),
                    s("clipName"),
                ));
            }
        }
    }
    let (mut ammo, mut clips) = (vec![String::new()], vec![String::new()]);
    let slot = |v: &mut Vec<String>, n: &str| {
        v.iter().position(|x| x == n).unwrap_or_else(|| {
            v.push(n.to_string());
            v.len() - 1
        })
    };
    let mut defs = vec![WeaponDef::default()];
    for n in names {
        let (mut d, a, c) = found
            .get(&n.to_ascii_lowercase())
            .cloned()
            .unwrap_or_default();
        d.ammo_index = slot(&mut ammo, &a);
        d.clip_index = slot(&mut clips, &c);
        defs.push(d);
    }
    defs
}

/// Each weapon's move scales by the level's weapon list, off the zones
/// the map and `common_mp` hold.
fn weapon_moves(
    install: &vcod_cod4::install::Install,
    map: &str,
    names: &[String],
) -> Vec<WeaponMove> {
    let mut found: BTreeMap<String, WeaponMove> = BTreeMap::new();
    for zone in [map, "common_mp"] {
        let path = install.zone("english", zone);
        let z = Zone::open_path(&path).unwrap();
        let w = walk(z, WalkOptions { keep_bulk: false }).unwrap();
        for d in &w.assets.weapons {
            if let Some(m) = WeaponMove::from_raw(&d.raw) {
                found.entry(d.name.clone()).or_insert(m);
            }
        }
    }
    names
        .iter()
        .map(|n| found.get(n).copied().unwrap_or(WeaponMove::NONE))
        .collect()
}

#[test]
fn cod4_moves_ab() {
    let Some(install) = cod4_install() else {
        return;
    };
    let report = std::env::var_os("MOVES_REPORT").is_some();
    let trace = std::env::var("MOVES_TRACE").ok();
    let trace_row = |name: &str, resync: bool| -> Option<usize> {
        let t = trace.as_deref()?;
        let mut p = t.split(':');
        let (c, mode, row) = (p.next()?, p.next()?, p.next()?.parse().ok()?);
        (c == name && (mode == "resync") == resync).then_some(row)
    };
    // `Mantle_CreateAnims`: the clips out of `common_mp`.
    let common = walk(
        Zone::open_path(&install.zone("english", "common_mp")).unwrap(),
        WalkOptions { keep_bulk: false },
    )
    .unwrap()
    .assets;
    pmove::mantle::install(&[&common]).expect("the mantle clips");
    let mut unexplained = Vec::new();
    eprintln!("capture  rows  cmds  free-run  resynced  max free error  first free-run divergence");
    for &(name, file, map, from, armed) in CAPTURES {
        let cap = read(file);
        let (_, walked) = Map::load(&install, "english", map).unwrap();
        let world = ClipMap::from_assets(&walked.assets).unwrap();
        let weapons = weapon_moves(&install, map, &cap.weapons);
        let defs = armed.then(|| weapon_defs(&install, map, &cap.weapons));
        let r = Replay {
            cap: &cap,
            world: &world,
            weapons: &weapons,
            rules: Rules::default(),
            defs: defs.as_ref(),
            from,
        };
        let mut out = Outcome::default();
        let start = r.start();
        let wire = PlayerState::from_wire(&cap.snaps[start].1).delta_angles[1];
        eprintln!(
            "{name}: start row {start}, delta_angles[1] {} on the wire, {} fitted",
            wire,
            r.fit_delta(start)[1]
        );
        r.run(false, &mut out, false, trace_row(name, false));
        if report {
            eprintln!("{name}:");
        }
        r.run(true, &mut out, report, trace_row(name, true));
        eprintln!(
            "{name:<8} {:>4}  {:>4}  {:>8}  {:>8}  {:>8.3} (row {})  {}",
            out.rows,
            out.cmds,
            out.free_matching,
            out.resync_matching,
            out.free_max_err.0,
            out.free_max_err.1,
            out.free_first.as_deref().unwrap_or("-")
        );
        for (f, (n, first)) in &out.resync_fields {
            let gap = GAPS.iter().find(|&&(c, g, _, _)| c == name && g == *f);
            eprintln!(
                "    {f}: {n} rows{}, first {first}",
                gap.map_or(String::new(), |g| format!(" (gap: {})", g.3))
            );
            match gap {
                Some(&(_, _, rows, _)) if rows == *n => {}
                Some(&(_, _, rows, _)) => {
                    unexplained.push(format!("{name} {f}: {n} rows, GAPS says {rows}"))
                }
                None => unexplained.push(format!("{name} {f}: {n} rows, first {first}")),
            }
        }
        for &(c, f, rows, _) in GAPS {
            if c == name && !out.resync_fields.contains_key(f) {
                unexplained.push(format!("{c} {f}: right in every row, GAPS says {rows}"));
            }
        }
    }
    assert!(
        unexplained.is_empty(),
        "fields wrong outside GAPS:\n{}",
        unexplained.join("\n")
    );
}

/// The trigger held through a mantle fires nothing: the mantle capture's
/// own cmds, armed, with attack added to every cmd from its first mantle
/// on. While `PMF_MANTLE` is up no round goes (no fire event, the clip
/// untouched) and the weapon is put away to 0, as `Mantle_IsWeaponInactive`
/// puts it (cod4-pmove.md 16).
#[test]
fn the_trigger_fires_nothing_through_a_mantle() {
    let Some(install) = cod4_install() else {
        return;
    };
    let common = walk(
        Zone::open_path(&install.zone("english", "common_mp")).unwrap(),
        WalkOptions { keep_bulk: false },
    )
    .unwrap()
    .assets;
    pmove::mantle::install(&[&common]).expect("the mantle clips");
    let cap = read("play-1.7-mantle.txt");
    let (_, walked) = Map::load(&install, "english", "mp_crash").unwrap();
    let world = ClipMap::from_assets(&walked.assets).unwrap();
    let weapons = weapon_moves(&install, "mp_crash", &cap.weapons);
    let defs = weapon_defs(&install, "mp_crash", &cap.weapons);
    let r = Replay {
        cap: &cap,
        world: &world,
        weapons: &weapons,
        rules: Rules::default(),
        defs: Some(&defs),
        from: Some(47100),
    };
    let start = r.start();
    let mut ps = PlayerState::from_wire(&cap.snaps[start].1);
    ps.delta_angles = r.fit_delta(start);
    let mut old = r.old_cmd(ps.command_time);
    let (mut mantled, mut fired, mut put_away) = (0, Vec::new(), false);
    let mut armed_from = None;
    let from = ps.command_time;
    for c in cap.cmds.iter().filter(|c| c.server_time > from) {
        let mut c = *c;
        if armed_from.is_some() {
            c.buttons |= 1;
        }
        let seq = ps.event_sequence;
        let clip = ps.ammoclip;
        r.step(&mut ps, &c, &mut old);
        if ps.pm_flags & pmove::pmf::MANTLE != 0 {
            armed_from.get_or_insert(c.server_time);
            mantled += 1;
            let shot = (seq.max(ps.event_sequence - 4)..ps.event_sequence)
                .any(|k| matches!(ps.events[(k & 3) as usize], 26 | 27));
            if shot || ps.ammoclip != clip {
                fired.push(c.server_time);
            }
            put_away |= ps.weapon == 0;
        }
    }
    assert!(mantled > 20, "{mantled} mantling cmds");
    assert!(fired.is_empty(), "fired while mantling at {fired:?}");
    assert!(put_away, "the weapon stayed out through the mantle");
}

/// `EV_FIRE_WEAPON` and `EV_FIRE_WEAPON_LASTSHOT`.
fn fire_event(e: u32) -> bool {
    matches!(e, 26 | 27)
}

/// The rounds of each burst (shots more than half a second apart start a
/// new one) at the times `shots` lists.
fn bursts(shots: &[i32]) -> Vec<usize> {
    let mut out: Vec<usize> = Vec::new();
    for (k, &t) in shots.iter().enumerate() {
        if k == 0 || t - shots[k - 1] > 500 {
            out.push(0);
        }
        *out.last_mut().unwrap() += 1;
    }
    out
}

/// The AK-47 on the trigger, plain and under Double Tap: each burst fires
/// as many rounds on ours, the capture's cmds replayed free from its armed
/// snapshot, as on retail, counted off the fire events each snapshot's
/// ring adds. Retail's clock drops what a step overshoots and Double Tap
/// rounds each scaled step to the millisecond (cod4-combat.md 2.1), so at
/// 16 ms cmds the 85 ms round takes six cmds plain and four with the perk:
/// half as many rounds again, not the multiplier's third.
#[test]
fn cod4_fire_rate_ab() {
    let Some(install) = cod4_install() else {
        return;
    };
    let (_, walked) = Map::load(&install, "english", "mp_crash").unwrap();
    let world = ClipMap::from_assets(&walked.assets).unwrap();
    let mut seen = Vec::new();
    for (file, from) in [
        ("play-1.7-firerate.txt", AUTOFIRE_FROM),
        ("play-1.7-firerate-rof.txt", DOUBLETAP_FROM),
    ] {
        let cap = read(file);
        let weapons = weapon_moves(&install, "mp_crash", &cap.weapons);
        let defs = weapon_defs(&install, "mp_crash", &cap.weapons);
        let r = Replay {
            cap: &cap,
            world: &world,
            weapons: &weapons,
            rules: Rules::default(),
            defs: Some(&defs),
            from: Some(from),
        };
        let start = r.start();
        let mut retail = Vec::new();
        for w in cap.snaps[start..].windows(2) {
            let (a, b) = (&w[0].1, &w[1].1);
            let (from, to) = (word(a, "eventSequence"), word(b, "eventSequence"));
            for k in from.max(to.saturating_sub(4))..to {
                if fire_event(word(b, &format!("events[{}]", k & 3))) {
                    retail.push(w[1].0);
                }
            }
        }
        let mut ps = PlayerState::from_wire(&cap.snaps[start].1);
        ps.delta_angles = r.fit_delta(start);
        let mut old = r.old_cmd(ps.command_time);
        let mut ours = Vec::new();
        let begin = ps.command_time;
        for c in cap.cmds.iter().filter(|c| c.server_time > begin) {
            let seq = ps.event_sequence;
            r.step(&mut ps, c, &mut old);
            for k in seq.max(ps.event_sequence - 4)..ps.event_sequence {
                if fire_event(ps.events[(k & 3) as usize]) {
                    ours.push(c.server_time);
                }
            }
        }
        let (retail, ours) = (bursts(&retail), bursts(&ours));
        eprintln!("{file}: rounds a burst, retail {retail:?}, ours {ours:?}");
        assert_eq!(ours, retail, "{file}");
        seen.push(retail);
    }
    let [plain, rof] = [&seen[0], &seen[1]];
    assert_eq!(plain.len(), 2, "{plain:?}");
    assert_eq!(rof.len(), 2, "{rof:?}");
}
