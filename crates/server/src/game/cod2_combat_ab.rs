//! The CoD2 combat A/B: each two-client retail capture in
//! `crates/cod2/tests/fixtures/combat/` (a shooter's `<name>.txt` beside
//! its target's `<name>-b.txt`, made by `tools/run_probe_cod2.sh` with
//! `GT=combat SECOND=still`, the recipe in each file's header) replayed on
//! `Cod2Level` under the same `probe_combat.gsc` and the capture's cvars.
//!
//! Each client begins the frame before its first snapshot as a player and
//! takes that snapshot's playerstate, and again at the first snapshot the
//! two share; from its first on each frame runs, per client, the cmds
//! retail had run by that snapshot (its `commandTime`), then `G_RunFrame`.
//! Only rows from the shared snapshot on are compared. What is
//! compared: the gametype's `PROBE damage` and `PROBE killed` lines (the
//! callback's damage, flags, means of death, weapon, hit location and
//! health, the point and the direction within [`VEC_TOLERANCE`]), the
//! target's health and damage feedback words per snapshot, the shooter's
//! weapon words and rounds per snapshot, the event entities the shooter
//! saw (type, parm, the entities named, the origin), and every missile's
//! trajectory. A difference must be named in [`GAPS`] with its section
//! of docs/research/cod2-combat.md. Needs `COD2_DIR`.
//!
//! A capture made with `RAND_TRACE=1` carries retail's every `srand()` and
//! `rand()` (`tools/re/cod2_rand_trace.c`): the replay takes `G_InitGame`'s
//! seed and the draws before its start off it, and compares the level time
//! of each later draw with vcod's (the `rand draws` check).
//! `COMBAT_REPORT=1` prints every mismatching row; `COMBAT_MISSILES=<name>`
//! prints that capture's missiles frame by frame, ours over retail's.

use std::collections::BTreeMap;
use std::rc::Rc;

use glam::Vec3;
use vcod_cod2::net::delta::{CLIENT_FIELDS, ENTITY_FIELDS, field_index};
use vcod_cod2::net::fields::{CLIENT_STATE_FIELDS, ENTITY_STATE_FIELDS, PLAYER_STATE_FIELDS};
use vcod_cod2::net::{PlayerState2, UserCmd2};
use vcod_cod2::pmove::PlayerState;

use crate::game::cod2::Cod2Level;

const FIXTURES: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../cod2/tests/fixtures/combat");

/// The captures, each a shooter's `<name>.txt` and its target's
/// `<name>-b.txt`: kar98k rounds at the head, torso and legs at 220 units,
/// a sighted thompson burst there and at 1340, kar98k rounds at 1340 that
/// meet a static model, a killing head shot, a frag at the target's feet, a
/// turret's burst, and thompson bursts at the hip from 110 at a target no
/// round knocks (standing, after a jump, backing off, crouched; traced).
const CAPTURES: &[&str] = &[
    "hit-head",
    "hit-torso",
    "hit-legs",
    "hit-smg",
    "hit-smg-far",
    "hit-rifle-far",
    "hit-kill",
    "hit-throw",
    "hit-turret",
    "hit-spray",
];

/// What the replay still gets wrong: (capture, check, count, why). The
/// counts are exact. Each reason names its section of cod2-combat.md.
const GAPS: &[(&str, &str, usize, &str)] = &[
    (
        "hit-smg-far",
        "event eventParm",
        3,
        "9.6 a seed not recovered",
    ),
    (
        "hit-smg-far",
        "event pos.trBase[0]",
        4,
        "9.6 a seed not recovered",
    ),
    (
        "hit-smg-far",
        "event pos.trBase[1]",
        5,
        "9.6 a seed not recovered",
    ),
    (
        "hit-smg-far",
        "event pos.trBase[2]",
        3,
        "9.6 a seed not recovered",
    ),
    (
        "hit-smg-far",
        "event surfType",
        2,
        "9.6 a seed not recovered",
    ),
    ("hit-smg-far", "probe hitloc", 1, "9.6 a seed not recovered"),
    ("hit-smg-far", "probe point", 1, "9.6 a seed not recovered"),
    ("hit-smg-far", "probe time", 1, "9.6 a seed not recovered"),
    (
        "hit-smg-far",
        "target damageCount",
        2,
        "9.6 a seed not recovered",
    ),
    (
        "hit-smg-far",
        "target damageDuration",
        2,
        "9.6 a seed not recovered",
    ),
    (
        "hit-smg-far",
        "target damageEvent",
        2,
        "9.6 a seed not recovered",
    ),
    (
        "hit-smg-far",
        "target damageYaw",
        2,
        "9.6 a seed not recovered",
    ),
    (
        "hit-smg-far",
        "target flinchYaw",
        2,
        "9.6 a seed not recovered",
    ),
    (
        "hit-smg-far",
        "target health",
        2,
        "9.6 a seed not recovered",
    ),
    (
        "hit-smg-far",
        "target pm_flags",
        2,
        "9.6 a seed not recovered",
    ),
    (
        "hit-smg-far",
        "target pm_time",
        2,
        "9.6 a seed not recovered",
    ),
    (
        "hit-smg-far",
        "target velocity[0]",
        3,
        "9.6 a seed not recovered",
    ),
    (
        "hit-smg-far",
        "target velocity[1]",
        3,
        "9.6 a seed not recovered",
    ),
    (
        "hit-smg-far",
        "target velocity[2]",
        3,
        "9.6 a seed not recovered",
    ),
    (
        "hit-throw",
        "missile pos.trBase[1]",
        67,
        "9.4 a last bit at the first bounce",
    ),
    (
        "hit-throw",
        "missile pos.trBase[2]",
        3,
        "9.4 the bounce off the target's body: last bits",
    ),
    (
        "hit-throw",
        "missile pos.trDelta[0]",
        6,
        "9.4 the bounce off the target's body: last bits",
    ),
    (
        "hit-throw",
        "missile pos.trDelta[1]",
        6,
        "9.4 the bounce off the target's body: last bits",
    ),
    (
        "hit-throw",
        "missile pos.trDelta[2]",
        6,
        "9.4 the bounce off the target's body: last bits",
    ),
    (
        "hit-turret",
        "event eventParm",
        7,
        "cod2-turrets.md 11, a dead body left playing",
    ),
    (
        "hit-turret",
        "event otherEntityNum",
        7,
        "cod2-turrets.md 11, a dead body left playing",
    ),
    (
        "hit-turret",
        "event pos.trBase[0]",
        32,
        "9.6 a seed not recovered",
    ),
    (
        "hit-turret",
        "event pos.trBase[1]",
        31,
        "9.6 a seed not recovered",
    ),
    (
        "hit-turret",
        "event pos.trBase[2]",
        8,
        "9.6 a seed not recovered",
    ),
    (
        "hit-turret",
        "event surfType",
        7,
        "cod2-turrets.md 11, a dead body left playing",
    ),
    (
        "hit-turret",
        "event time",
        10,
        "cod2-turrets.md 11, a dead body left playing",
    ),
    (
        "hit-turret",
        "events",
        1,
        "cod2-turrets.md 11, a dead body left playing",
    ),
    (
        "hit-turret",
        "probe lines",
        1,
        "cod2-turrets.md 11, a dead body left playing",
    ),
    ("hit-turret", "probe point", 3, "9.6 a seed not recovered"),
    ("hit-turret", "probe time", 1, "9.6 a seed not recovered"),
    (
        "hit-turret",
        "target damageCount",
        10,
        "cod2-turrets.md 11, a dead body left playing",
    ),
    (
        "hit-turret",
        "target damageEvent",
        69,
        "cod2-turrets.md 11, a dead body left playing",
    ),
    (
        "hit-turret",
        "target health",
        70,
        "cod2-turrets.md 11, a dead body left playing",
    ),
    (
        "hit-turret",
        "target pm_flags",
        9,
        "cod2-turrets.md 11, a dead body left playing",
    ),
    (
        "hit-turret",
        "target pm_time",
        9,
        "cod2-turrets.md 11, a dead body left playing",
    ),
    (
        "hit-turret",
        "target velocity[0]",
        20,
        "cod2-turrets.md 11, a dead body left playing",
    ),
    (
        "hit-turret",
        "target velocity[1]",
        20,
        "cod2-turrets.md 11, a dead body left playing",
    ),
    (
        "hit-turret",
        "target velocity[2]",
        11,
        "cod2-turrets.md 11, a dead body left playing",
    ),
];

/// The glibc seed each capture's server drew from (`G_InitGame`'s
/// `srand(Sys_MillisecondsRaw())`) and the draws it made before the
/// replay's first: (capture, seed, skip). `recover_spread_seed` found them
/// (cod2-combat.md 9.6).
const SEEDS: &[(&str, u32, usize)] = &[("hit-smg", 1_679_727_541, 5)];

/// How far a logged point or direction may sit from retail's, in units:
/// the log prints six significant digits, two decimals on a map
/// coordinate past 1000.
const VEC_TOLERANCE: f32 = 0.02;

/// The target's words compared each snapshot.
const TARGET_WORDS: &[&str] = &[
    "pm_type",
    "damageEvent",
    "damageYaw",
    "damagePitch",
    "damageCount",
    "damageDuration",
    "flinchYaw",
    "pm_flags",
    "pm_time",
    "velocity[0]",
    "velocity[1]",
    "velocity[2]",
];

/// The shooter's words compared each snapshot.
const SHOOTER_WORDS: &[&str] = &[
    "weapon",
    "weaponstate",
    "weaponTime",
    "weaponDelay",
    "weapAnim",
    "fWeaponPosFrac",
    "eventSequence",
    "events[0]",
    "events[1]",
    "events[2]",
    "events[3]",
    "viewangles[0]",
    "viewangles[1]",
];

/// The entity words an event entity is compared on.
const EVENT_WORDS: &[&str] = &[
    "eType",
    "eventParm",
    "otherEntityNum",
    "attackerEntityNum",
    "surfType",
    "pos.trBase[0]",
    "pos.trBase[1]",
    "pos.trBase[2]",
];

/// The entity words a missile is compared on.
const MISSILE_WORDS: &[&str] = &[
    "eType",
    "eFlags",
    "pos.trType",
    "pos.trTime",
    "pos.trBase[0]",
    "pos.trBase[1]",
    "pos.trBase[2]",
    "pos.trDelta[0]",
    "pos.trDelta[1]",
    "pos.trDelta[2]",
    "groundEntityNum",
    "weapon",
    "eventSequence",
    "events[0]",
    "events[1]",
    "events[2]",
    "events[3]",
];

pub(super) type Ents = BTreeMap<u32, [u32; ENTITY_FIELDS]>;
/// The roster by client number (`r` lines; empty in captures made before
/// move_probe logged it).
pub(super) type Roster = BTreeMap<u32, [u32; CLIENT_FIELDS]>;

pub(super) struct Snap {
    pub(super) time: i32,
    pub(super) ps: PlayerState2,
    pub(super) ents: Ents,
    pub(super) clients: Roster,
}

#[derive(Default)]
pub(super) struct Side {
    pub(super) cmds: Vec<UserCmd2>,
    /// Each cmd's first packet, by that packet's newest cmd time: the
    /// order the server ran them in, as near as the log tells.
    pub(super) sent: Vec<i32>,
    pub(super) snaps: Vec<Snap>,
}

impl Side {
    pub(super) fn at(&self, time: i32) -> Option<&Snap> {
        self.snaps.iter().find(|s| s.time == time)
    }
}

pub(super) struct Capture {
    pub(super) map: String,
    pub(super) cvars: Vec<(String, String)>,
    pub(super) probes: Vec<String>,
    pub(super) side: Side,
    /// The server's `srand()` and `rand()` calls in order, when traced:
    /// (`srand`, `level.time`, the call site, the seed or the value).
    pub(super) rand: Vec<(bool, i32, u32, u32)>,
}

/// `G_InitGame`'s `srand` (lnxded 0x810911e).
const SRAND_INIT_GAME: u32 = 0x0810_911e;

impl Capture {
    /// Off a traced capture: `G_InitGame`'s last seed, how many draws it
    /// made before `start`, and the level time of each after, up to `end`.
    fn traced_rand(&self, start: i32, end: i32) -> Option<(u32, usize, Vec<i32>)> {
        let at = self
            .rand
            .iter()
            .rposition(|r| r.0 && r.2 == SRAND_INIT_GAME)?;
        let draws: Vec<i32> = self.rand[at + 1..]
            .iter()
            .filter(|r| !r.0)
            .map(|r| r.1)
            .collect();
        let skip = draws.iter().take_while(|&&t| t < start).count();
        let after = draws[skip..].iter().copied().filter(|&t| t < end).collect();
        Some((self.rand[at].3, skip, after))
    }
}

pub(super) fn parse(text: &str) -> Capture {
    let mut cap = Capture {
        map: String::new(),
        cvars: Vec::new(),
        probes: Vec::new(),
        side: Side::default(),
        rand: Vec::new(),
    };
    let mut ps = PlayerState2::default();
    let mut ents = Ents::new();
    let mut clients = Roster::new();
    for line in text.lines() {
        if let Some(p) = line.split_once("PROBE ").filter(|_| line.starts_with('#')) {
            cap.probes.push(p.1.to_string());
            continue;
        }
        let mut w = line.split_whitespace();
        match w.next() {
            Some("#") => match w.next() {
                Some("map") => cap.map = w.next().unwrap().to_string(),
                Some("made") => {
                    let rest = line.trim_end_matches(')');
                    let words: Vec<&str> = rest.split_whitespace().collect();
                    for k in 0..words.len() {
                        // The script's spot: `<script> <map> <x> <y> <z> <yaw>`.
                        if words[k].ends_with("run_probe_cod2.sh") && k + 6 < words.len() {
                            for (n, v) in ["probe_x", "probe_y", "probe_z", "probe_yaw"]
                                .iter()
                                .zip(&words[k + 3..k + 7])
                            {
                                cap.cvars.push((n.to_string(), v.to_string()));
                            }
                        }
                        if words[k] == "+set" && k + 2 < words.len() {
                            cap.cvars
                                .push((words[k + 1].to_string(), words[k + 2].to_string()));
                        }
                    }
                }
                Some(k @ ("srand" | "rand")) => {
                    let mut n = || w.next().unwrap();
                    let time = n().parse().unwrap();
                    let site = u32::from_str_radix(n(), 16).unwrap();
                    cap.rand
                        .push((k == "srand", time, site, n().parse().unwrap()));
                }
                _ => {}
            },
            Some("c") => {
                let n: Vec<i64> = w
                    .enumerate()
                    .map(|(k, t)| {
                        if k == 1 {
                            i64::from_str_radix(t, 16).unwrap()
                        } else {
                            t.parse().unwrap()
                        }
                    })
                    .collect();
                cap.side.cmds.push(UserCmd2 {
                    server_time: n[0] as i32,
                    buttons: n[1] as i32,
                    weapon: n[2] as u8,
                    off_hand_index: n[3] as u8,
                    angles: [n[4] as i32, n[5] as i32, n[6] as i32],
                    forward_move: n[7] as i8,
                    right_move: n[8] as i8,
                });
            }
            Some("p") => {
                let at = cap.side.cmds.last().map_or(0, |c| c.server_time);
                let n = cap.side.cmds.len();
                cap.side.sent.resize(n, at);
            }
            Some("s") => {
                let _msg = w.next();
                let time = w.next().unwrap().parse().unwrap();
                for kv in w {
                    let (k, v) = kv.split_once('=').unwrap();
                    let v = u32::from_str_radix(v, 16).unwrap();
                    // `S[i]`, `A[i]`, `C[i]`: the stats, reserve and clip.
                    let slot = |k: &str| k[2..k.len() - 1].parse::<usize>().unwrap();
                    match k.as_bytes()[0] {
                        b'S' if k.starts_with("S[") => ps.stats[slot(k)] = v as i32,
                        b'A' if k.starts_with("A[") => ps.ammo[slot(k)] = v as i32,
                        b'C' if k.starts_with("C[") => ps.ammoclip[slot(k)] = v as i32,
                        _ => ps.f[field_index(&PLAYER_STATE_FIELDS, k)] = v,
                    }
                }
                cap.side.snaps.push(Snap {
                    time,
                    ps: ps.clone(),
                    ents: ents.clone(),
                    clients: clients.clone(),
                });
            }
            Some("e") => {
                let n: u32 = w.next().unwrap().parse().unwrap();
                let e = ents.entry(n).or_insert([0; ENTITY_FIELDS]);
                for kv in w {
                    let (k, v) = kv.split_once('=').unwrap();
                    e[field_index(&ENTITY_STATE_FIELDS, k)] = u32::from_str_radix(v, 16).unwrap();
                }
                cap.side.snaps.last_mut().unwrap().ents = ents.clone();
            }
            Some("e-") => {
                let n: u32 = w.next().unwrap().parse().unwrap();
                ents.remove(&n);
                cap.side.snaps.last_mut().unwrap().ents = ents.clone();
            }
            Some("r") => {
                let n: u32 = w.next().unwrap().parse().unwrap();
                let c = clients.entry(n).or_insert([0; CLIENT_FIELDS]);
                for kv in w {
                    let (k, v) = kv.split_once('=').unwrap();
                    c[field_index(&CLIENT_STATE_FIELDS, k)] = u32::from_str_radix(v, 16).unwrap();
                }
                cap.side.snaps.last_mut().unwrap().clients = clients.clone();
            }
            Some("r-") => {
                let n: u32 = w.next().unwrap().parse().unwrap();
                clients.remove(&n);
                cap.side.snaps.last_mut().unwrap().clients = clients.clone();
            }
            _ => {}
        }
    }
    cap
}

pub(super) fn word(ps: &PlayerState2, name: &str) -> u32 {
    ps.f[field_index(&PLAYER_STATE_FIELDS, name)]
}

pub(super) fn eword(e: &[u32; ENTITY_FIELDS], name: &str) -> u32 {
    e[field_index(&ENTITY_STATE_FIELDS, name)]
}

/// The word as the wire carries it (see `moves_ab::on_wire`).
fn on_wire(bits: i32, v: u32) -> u32 {
    match bits {
        -100 => {
            ((f64::from(f32::from_bits(v)) * f64::from(65536.0f32 / 360.0)) as i64 as i16) as u16
                as u32
        }
        0 if v == 0x8000_0000 => 0,
        0 | 32 => v,
        b if b > 0 => v & ((1u32 << b) - 1),
        b => {
            let n = -b;
            (((v << (32 - n)) as i32) >> (32 - n)) as u32
        }
    }
}

fn ps_bits(name: &str) -> i32 {
    PLAYER_STATE_FIELDS[field_index(&PLAYER_STATE_FIELDS, name)].bits
}

fn es_bits(name: &str) -> i32 {
    ENTITY_STATE_FIELDS[field_index(&ENTITY_STATE_FIELDS, name)].bits
}

fn show(bits: i32, v: u32) -> String {
    if bits == 0 {
        format!("{}", f32::from_bits(v))
    } else {
        format!("{}", v as i32)
    }
}

/// What one capture's replay found wrong: check -> (count, first).
type Found = BTreeMap<String, (usize, String)>;

fn note(found: &mut Found, report: bool, check: &str, what: String) {
    if report {
        eprintln!("    {check}: {what}");
    }
    let e = found.entry(check.to_string()).or_insert((0, what));
    e.0 += 1;
}

/// A `PROBE damage` / `PROBE killed` line split into its words and its
/// vectors.
fn probe_parts(line: &str) -> (Vec<String>, Vec<Vec3>) {
    let mut words = Vec::new();
    let mut vecs = Vec::new();
    let mut rest = line;
    while !rest.is_empty() {
        if let Some(r) = rest.strip_prefix('(') {
            let (inner, after) = r.split_once(')').unwrap();
            let v: Vec<f32> = inner
                .split(',')
                .map(|t| t.trim().parse().unwrap())
                .collect();
            vecs.push(Vec3::new(v[0], v[1], v[2]));
            rest = after.trim_start();
        } else {
            let (w, after) = rest.split_once(' ').unwrap_or((rest, ""));
            words.push(w.to_string());
            rest = after.trim_start();
        }
    }
    (words, vecs)
}

/// The `PM_Weapon` timers retail's snapshot carries, onto `p`'s state.
pub(super) fn weapon_from_wire(p: &mut crate::game::cod2::Cod2Player, w: &PlayerState2) {
    let short = |n: &str| word(w, n) as u16 as i16 as i32;
    p.ws.state = word(w, "weaponstate") as i32;
    p.ws.weapon_time = short("weaponTime");
    p.ws.weapon_delay = short("weaponDelay");
    p.ws.grenade_time_left = short("grenadeTimeLeft");
    p.ws.weap_anim = word(w, "weapAnim") as i32;
    p.ws.pos_frac = f32::from_bits(word(w, "fWeaponPosFrac"));
    p.ps.weapon_state = p.ws.state;
}

/// The event entities and missiles of `level` this frame, as the wire
/// sends them (the shooter's view: a scope that hides one from slot 1
/// leaves it out).
fn our_entities(level: &Cod2Level) -> Ents {
    let mut out = Ents::new();
    for (n, te) in level.rt.temp_entities() {
        if !te.scope.admits(1) {
            continue;
        }
        let Some(e) = crate::game::cod2_wire::temp_entity(te, n) else {
            continue;
        };
        if let Some(c) = crate::game::cod2_wire::to_cod2_entity(&e) {
            out.insert(n, c.f);
        }
    }
    for (n, e) in level.missile_entities() {
        if let Some(c) = crate::game::cod2_wire::to_cod2_entity(&e) {
            out.insert(n, c.f);
        }
    }
    out
}

fn is_event(e: &[u32; ENTITY_FIELDS]) -> bool {
    eword(e, "eType") as i32 >= vcod_cod2::net::events::ET_EVENTS
}

/// A missile that went off: `ET_GENERAL` carrying an explode event.
fn is_spent(e: &[u32; ENTITY_FIELDS]) -> bool {
    use vcod_cod2::net::events as c2;
    let explode = [
        c2::EV_GRENADE_EXPLODE,
        c2::EV_ROCKET_EXPLODE,
        c2::EV_CUSTOM_EXPLODE,
        c2::EV_CUSTOM_EXPLODE_NOMARKS,
    ];
    eword(e, "eType") == 0
        && (0..4).any(|k| explode.contains(&(eword(e, &format!("events[{k}]")) as i32)))
}

fn is_missile(e: &[u32; ENTITY_FIELDS]) -> bool {
    eword(e, "eType") as i32 == vcod_cod2::net::events::ET_MISSILE
        || (eword(e, "eType") == 0 && eword(e, "pos.trType") == 5)
}

/// Puts retail's playerstate of `side`'s snapshot at `time` on `slot` and
/// returns its `commandTime`, the last cmd already run.
fn take_snapshot(level: &mut Cod2Level, slot: usize, side: &Side, time: i32) -> i32 {
    let snap = side.at(time).expect("a snapshot at the replay's start");
    let p = level.players[slot].as_mut().expect("both connected");
    p.ps = PlayerState::from_wire(&snap.ps);
    weapon_from_wire(p, &snap.ps);
    let done = p.ps.command_time;
    if let Some(c) = side.cmds.iter().rev().find(|c| c.server_time <= done) {
        p.oldcmd = *c;
    }
    done
}

/// Each client's cmds past `done` up to its snapshot's `commandTime`, the
/// two interleaved by send time: which packet came first is not logged. A
/// client without a snapshot runs none.
fn run_cmds(
    level: &mut Cod2Level,
    sides: [&Side; 2],
    snaps: [Option<&Snap>; 2],
    done: &mut [i32; 2],
) {
    let mut due: Vec<(i32, usize, UserCmd2)> = Vec::new();
    for (slot, snap) in snaps.iter().enumerate() {
        let Some(snap) = snap else { continue };
        let ct = word(&snap.ps, "commandTime") as i32;
        let side = sides[slot];
        for (k, c) in side.cmds.iter().enumerate() {
            if c.server_time > done[slot] && c.server_time <= ct {
                let sent = side.sent.get(k).copied().unwrap_or(c.server_time);
                due.push((sent, slot, *c));
            }
        }
        done[slot] = done[slot].max(ct);
    }
    due.sort_by_key(|&(sent, slot, c)| (sent, slot, c.server_time));
    let mut packet = None;
    for (sent, slot, c) in &due {
        // Each packet resets the skeleton cache.
        if packet != Some((*sent, *slot)) {
            packet = Some((*sent, *slot));
            level.packet();
        }
        level.client_think(*slot, c);
    }
}

fn replay(fs: &Rc<vcod_common::pk3::Pk3Fs>, name: &str, report: bool) -> Found {
    let read = |n: &str| std::fs::read_to_string(format!("{FIXTURES}/{n}.txt")).unwrap();
    let shooter = parse(&read(name));
    let target = parse(&read(&format!("{name}-b")));
    let gsc = std::fs::read_to_string(format!("{FIXTURES}/probe_combat.gsc")).unwrap();
    let mut cvars = crate::cvars::Cvars::new();
    cvars.set("developer", "1");
    for (k, v) in &shooter.cvars {
        cvars.set(k, v);
    }
    // The first snapshot time both clients share once both are playing.
    let playing = |s: &Snap| word(&s.ps, "pm_type") == 0 && word(&s.ps, "weapon") != 0;
    let start = shooter
        .side
        .snaps
        .iter()
        .filter(|s| playing(s))
        .find(|s| target.side.at(s.time).is_some_and(playing))
        .map(|s| s.time)
        .expect("no snapshot time both share");
    // Each client begins the frame before its first snapshot as a player,
    // takes that snapshot's playerstate, then runs its own cmds: the
    // target plays two seconds before the shooter spawns, and its legs
    // clip started then (cod2-combat.md 9.5, "The struck body").
    let sides = [&target.side, &shooter.side];
    let first = |side: &Side| {
        side.snaps
            .iter()
            .find(|s| word(&s.ps, "pm_type") == 0)
            .map(|s| s.time)
            .expect("a side never plays")
    };
    let firsts = [first(sides[0]), first(sides[1])];
    let t0 = (start - 2000).min(firsts[0] - 100).min(firsts[1] - 100);
    let mut level = Cod2Level::load_with_script(
        fs.clone(),
        &shooter.map,
        "probe_combat",
        (
            "maps/mp/gametypes/probe_combat".to_string(),
            gsc.to_string(),
        ),
        cvars,
        t0,
    )
    .expect("load probe_combat");
    level.connect(0, "target");
    level.connect(1, "vcodmoves");
    let mut done = [None::<i32>; 2];
    let mut t = t0;
    while t < start {
        t += 50;
        for (slot, &f) in firsts.iter().enumerate() {
            if t == f - 50 {
                level.begin(slot);
            }
        }
        let snaps = [0, 1].map(|s| done[s].and_then(|_| sides[s].at(t)));
        let mut d = done.map(|d| d.unwrap_or(0));
        run_cmds(&mut level, sides, snaps, &mut d);
        for s in 0..2 {
            if done[s].is_some() {
                done[s] = Some(d[s]);
            }
        }
        level.run_frame(t);
        for (slot, side) in sides.iter().enumerate() {
            if done[slot].is_none() && t == firsts[slot] && t < start {
                done[slot] = Some(take_snapshot(&mut level, slot, side, t));
            }
        }
    }
    // Both bodies as retail's snapshot has them.
    let mut done = [0, 1].map(|slot| take_snapshot(&mut level, slot, sides[slot], start));
    let end = shooter
        .side
        .snaps
        .last()
        .unwrap()
        .time
        .min(target.side.snaps.last().unwrap().time);
    // The capture's `rand()`, from where the replay starts: its trace, else
    // `SEEDS` (`COMBAT_SEED=<seed>,<skip>` tries another).
    let traced = shooter.traced_rand(start, end);
    let seed = std::env::var("COMBAT_SEED").ok().map(|v| {
        let (a, b) = v.split_once(',').unwrap();
        (a.parse().unwrap(), b.parse().unwrap())
    });
    if let Some((seed, skip)) = seed
        .or_else(|| traced.as_ref().map(|t| (t.0, t.1)))
        .or_else(|| SEEDS.iter().find(|s| s.0 == name).map(|s| (s.1, s.2)))
    {
        level.combat.pin_rand(seed, skip);
    }
    // Each of our draws from here by the level time retail's would log: a
    // cmd's at the frame before, the frame's at its own.
    let mut our_draws: Vec<i32> = Vec::new();
    let drawn0 = level.combat.rand.drawn();
    let tag = |draws: &mut Vec<i32>, level: &Cod2Level, time: i32| {
        let n = (level.combat.rand.drawn() - drawn0) as usize;
        draws.resize(n.max(draws.len()), time);
    };
    let mut found = Found::new();
    let trace_missiles = std::env::var("COMBAT_MISSILES").is_ok_and(|v| v == name);
    // Event entities each side saw, in order of first appearance: (time,
    // number, state).
    let mut retail_events: Vec<(i32, [u32; ENTITY_FIELDS])> = Vec::new();
    let mut our_events: Vec<(i32, [u32; ENTITY_FIELDS])> = Vec::new();
    // An event entity is new when its number was not one the frame
    // before, or held another type: numbers are reused.
    let mut seen_retail: BTreeMap<u32, u32> = BTreeMap::new();
    let mut seen_ours: BTreeMap<u32, u32> = BTreeMap::new();
    let mut t = start;
    while t < end {
        t += 50;
        let (Some(rs), Some(rt)) = (shooter.side.at(t), target.side.at(t)) else {
            continue;
        };
        run_cmds(&mut level, sides, [Some(rt), Some(rs)], &mut done);
        tag(&mut our_draws, &level, t - 50);
        level.run_frame(t);
        if t < end {
            tag(&mut our_draws, &level, t);
        }
        let row = format!("serverTime {t}");
        let team = 0;
        let ours_t = level.wire_playerstate(0, team).unwrap();
        let ours_s = level.wire_playerstate(1, team).unwrap();
        if ours_t.stats[0] != rt.ps.stats[0] {
            note(
                &mut found,
                report,
                "target health",
                format!("{row}: ours {} retail {}", ours_t.stats[0], rt.ps.stats[0]),
            );
        }
        for (who, ours, retail, words) in [
            ("target", &ours_t, &rt.ps, TARGET_WORDS),
            ("shooter", &ours_s, &rs.ps, SHOOTER_WORDS),
        ] {
            for &n in words {
                let b = ps_bits(n);
                let (a, r) = (on_wire(b, word(ours, n)), on_wire(b, word(retail, n)));
                if a != r {
                    note(
                        &mut found,
                        report,
                        &format!("{who} {n}"),
                        format!("{row}: ours {} retail {}", show(b, a), show(b, r)),
                    );
                }
            }
        }
        if let Some(k) = (0..64)
            .find(|&k| ours_s.ammoclip[k] != rs.ps.ammoclip[k] || ours_s.ammo[k] != rs.ps.ammo[k])
        {
            note(
                &mut found,
                report,
                "shooter rounds",
                format!(
                    "{row}: [{k}] ours {}+{} retail {}+{}",
                    ours_s.ammoclip[k], ours_s.ammo[k], rs.ps.ammoclip[k], rs.ps.ammo[k]
                ),
            );
        }
        // Missiles, frame by frame; events by first appearance.
        let ours_e = our_entities(&level);
        let retail_missiles: Vec<_> = rs.ents.values().filter(|e| is_missile(e)).collect();
        let our_missiles: Vec<_> = ours_e.values().filter(|e| is_missile(e)).collect();
        if retail_missiles.len() != our_missiles.len() {
            note(
                &mut found,
                report,
                "missiles",
                format!(
                    "{row}: ours {} retail {}",
                    our_missiles.len(),
                    retail_missiles.len()
                ),
            );
        }
        if trace_missiles {
            let row = |e: &[u32; ENTITY_FIELDS]| -> String {
                MISSILE_WORDS
                    .iter()
                    .map(|&n| show(es_bits(n), on_wire(es_bits(n), eword(e, n))))
                    .collect::<Vec<_>>()
                    .join(" ")
            };
            for e in &our_missiles {
                eprintln!("  {t} ours   {}", row(e));
            }
            for e in &retail_missiles {
                eprintln!("  {t} retail {}", row(e));
            }
        }
        for (a, r) in our_missiles.iter().zip(&retail_missiles) {
            for &n in MISSILE_WORDS {
                let b = es_bits(n);
                let (x, y) = (on_wire(b, eword(a, n)), on_wire(b, eword(r, n)));
                if x != y {
                    note(
                        &mut found,
                        report,
                        &format!("missile {n}"),
                        format!("{row}: ours {} retail {}", show(b, x), show(b, y)),
                    );
                }
            }
        }
        // A missile once it went off (`G_ExplodeMissile` 0x810d970): its
        // `eFlags` read whole, as the delta's change and zero bits carry
        // what sits above the field's 24 (cod2-protocol.md 5.2).
        let retail_spent: Vec<_> = rs.ents.values().filter(|e| is_spent(e)).collect();
        let our_spent: Vec<_> = ours_e.values().filter(|e| is_spent(e)).collect();
        if retail_spent.len() != our_spent.len() {
            note(
                &mut found,
                report,
                "spent missiles",
                format!(
                    "{row}: ours {} retail {}",
                    our_spent.len(),
                    retail_spent.len()
                ),
            );
        }
        let pairs = our_missiles.iter().zip(&retail_missiles);
        for (a, r) in pairs.chain(our_spent.iter().zip(&retail_spent)) {
            let (x, y) = (eword(a, "eFlags"), eword(r, "eFlags"));
            if x != y {
                note(
                    &mut found,
                    report,
                    "missile eFlags whole",
                    format!("{row}: ours {x:#x} retail {y:#x}"),
                );
            }
        }
        for (ents, seen, events) in [
            (&rs.ents, &mut seen_retail, &mut retail_events),
            (&ours_e, &mut seen_ours, &mut our_events),
        ] {
            for (n, e) in ents {
                if is_event(e) && seen.get(n) != Some(&eword(e, "eType")) {
                    events.push((t, *e));
                }
            }
            *seen = ents
                .iter()
                .filter(|(_, e)| is_event(e))
                .map(|(n, e)| (*n, eword(e, "eType")))
                .collect();
        }
    }
    // The event entities, in order.
    if retail_events.len() != our_events.len() {
        let list = |v: &[(i32, [u32; ENTITY_FIELDS])]| -> String {
            v.iter()
                .map(|(t, e)| format!("{t}:{}", eword(e, "eType") as i32 - 10))
                .collect::<Vec<_>>()
                .join(" ")
        };
        note(
            &mut found,
            report,
            "events",
            format!(
                "ours [{}] retail [{}]",
                list(&our_events),
                list(&retail_events)
            ),
        );
    }
    for ((ta, a), (tr, r)) in our_events.iter().zip(&retail_events) {
        if ta != tr {
            note(
                &mut found,
                report,
                "event time",
                format!(
                    "event {}: ours {ta} retail {tr}",
                    eword(r, "eType") as i32 - 10
                ),
            );
        }
        for &n in EVENT_WORDS {
            let b = es_bits(n);
            let (x, y) = (on_wire(b, eword(a, n)), on_wire(b, eword(r, n)));
            // A float origin goes out truncated to whole units both ways.
            let close = b == 0 && (f32::from_bits(x) - f32::from_bits(y)).abs() <= 1.0;
            if x != y && !close {
                note(
                    &mut found,
                    report,
                    &format!("event {n}"),
                    format!(
                        "event {} at {tr}: ours {} retail {}",
                        eword(r, "eType") as i32 - 10,
                        show(b, x),
                        show(b, y)
                    ),
                );
            }
        }
    }
    // The draws, one by one: a missing or extra one shifts the rest.
    if let Some((_, _, retail)) = &traced {
        let wrong: Vec<usize> = (0..our_draws.len().max(retail.len()))
            .filter(|&k| our_draws.get(k) != retail.get(k))
            .collect();
        if let Some(&k) = wrong.first() {
            let at = |v: &[i32]| v.get(k).map_or("none".to_string(), |t| t.to_string());
            let what = format!(
                "ours {} retail {}, draw {k}: ours at {} retail at {}",
                our_draws.len(),
                retail.len(),
                at(&our_draws),
                at(retail)
            );
            if report {
                eprintln!("    rand draws: {what}");
            }
            // Counted by draw, as the other checks by row.
            found.insert("rand draws".to_string(), (wrong.len(), what));
        }
    }
    // The callbacks' lines.
    let ours: Vec<String> = level
        .rt
        .host
        .script_log
        .iter()
        .filter_map(|l| l.split_once("PROBE ").map(|p| p.1.to_string()))
        .filter(|l| !l.starts_with("spawned"))
        .collect();
    let retail: Vec<&String> = shooter
        .probes
        .iter()
        .filter(|l| !l.starts_with("spawned"))
        .collect();
    if ours.len() != retail.len() {
        note(
            &mut found,
            report,
            "probe lines",
            format!("ours {ours:?} retail {retail:?}"),
        );
    }
    for (a, r) in ours.iter().zip(&retail) {
        let ((wa, va), (wr, vr)) = (probe_parts(a), probe_parts(r));
        let words = [
            "kind", "entity", "time", "damage", "flags", "mod", "weapon", "hitloc", "health",
        ];
        for (k, label) in words.iter().enumerate() {
            if wa.get(k) != wr.get(k) {
                note(
                    &mut found,
                    report,
                    &format!("probe {label}"),
                    format!("ours `{a}` retail `{r}`"),
                );
            }
        }
        for (k, label) in ["point", "dir"].iter().enumerate() {
            if let (Some(x), Some(y)) = (va.get(k), vr.get(k))
                && (*x - *y).abs().max_element() > VEC_TOLERANCE
            {
                note(
                    &mut found,
                    report,
                    &format!("probe {label}"),
                    format!("ours `{a}` retail `{r}`"),
                );
            }
        }
    }
    let aborts = level.rt.aborts();
    if !aborts.is_empty() {
        note(&mut found, report, "script errors", aborts.join("; "));
    }
    found
}

#[test]
fn retail_combat_replays_on_cod2_level() {
    let Some(fs) = vcod_common::testing::cod2_fs() else {
        return;
    };
    let fs = Rc::new(fs);
    let report = std::env::var_os("COMBAT_REPORT").is_some();
    let mut unexplained = Vec::new();
    for name in CAPTURES {
        if report {
            eprintln!("{name}:");
        }
        let found = replay(&fs, name, report);
        eprintln!("{name:<14} {} checks wrong", found.len());
        for (check, (n, first)) in &found {
            let gap = GAPS.iter().find(|g| g.0 == *name && g.1 == check);
            eprintln!(
                "    {check}: {n}{}, first {first}",
                gap.map_or(String::new(), |g| format!(" (gap: {})", g.3))
            );
            match gap {
                Some(g) if g.2 == *n => {}
                Some(g) => unexplained.push(format!("{name} {check}: {n}, GAPS says {}", g.2)),
                None => unexplained.push(format!("{name} {check}: {n}, first {first}")),
            }
        }
        for g in GAPS.iter().filter(|g| g.0 == *name) {
            if !found.contains_key(g.1) {
                unexplained.push(format!(
                    "{name} {}: right everywhere, GAPS says {}",
                    g.1, g.2
                ));
            }
        }
    }
    assert!(
        unexplained.is_empty(),
        "combat checks wrong outside GAPS:\n{}",
        unexplained.join("\n")
    );
}

/// Each round's two spread draws read back off its logged point: the
/// angle draw (`G_random()`, the angle over 360) and the fraction, from
/// the line between the shooter's eye and the point against the logged
/// forward, the view's roll 0, at `spread` degrees.
fn spread_draws(cap: &Capture, spread: f32) -> Vec<(f64, f64)> {
    let ps = PlayerState::from_wire(&cap.side.snaps[0].ps);
    let eye = (ps.origin + Vec3::Z * ps.view_height_current).as_dvec3();
    let k = f64::from(spread).to_radians().tan() * 8192.0;
    cap.probes
        .iter()
        .filter(|l| l.starts_with("damage"))
        .map(|l| {
            let (_, v) = probe_parts(l);
            let f = v[1].as_dvec3();
            let yaw = f.y.atan2(f.x).to_degrees() as f32;
            let pitch = (-f.z).asin().to_degrees() as f32;
            let [_, r, u] = vcod_common::x87::angle_vectors(Vec3::new(pitch, yaw, 0.0));
            let d = v[0].as_dvec3() - eye;
            let a = d.dot(f);
            let (x, y) = (
                8192.0 * d.dot(r.as_dvec3()) / a,
                8192.0 * d.dot(u.as_dvec3()) / a,
            );
            (
                y.atan2(x).to_degrees().rem_euclid(360.0) / 360.0,
                x.hypot(y) / k,
            )
        })
        .collect()
}

/// The glibc seed and the draws past it that put a capture's rounds where
/// retail's landed: each round four recoil draws (`BG_WeaponFireRecoil`)
/// then its angle and fraction (cod2-combat.md 9.6). Retail seeds
/// `G_InitGame`'s `srand` with the wall clock's milliseconds
/// (`Sys_MillisecondsRaw`), so the search walks the seconds
/// `COMBAT_SEED_FROM..COMBAT_SEED_TO` (Unix time) of the run. Run by hand
/// (`cargo test -p vcod-server --lib -- --ignored recover_spread_seed
/// --nocapture`); its finds are the seeds the replay pins.
#[test]
#[ignore]
fn recover_spread_seed() {
    let read = |n: &str| std::fs::read_to_string(format!("{FIXTURES}/{n}.txt")).unwrap();
    let secs = |k: &str| -> u64 { std::env::var(k).expect(k).parse().unwrap() };
    let (from, to) = (
        secs("COMBAT_SEED_FROM") * 1000,
        secs("COMBAT_SEED_TO") * 1000,
    );
    let name = std::env::var("COMBAT_SEED_CAPTURE").unwrap_or("hit-smg".into());
    let draws = spread_draws(&parse(&read(&name)), 0.5);
    let threads = 24u64;
    let chunk = (to - from).div_ceil(threads);
    std::thread::scope(|sc| {
        for t in 0..threads {
            let draws = &draws;
            let name = &name;
            sc.spawn(move || {
                let near = |a: f64, b: f64| {
                    let d = (a - b).abs();
                    d.min(1.0 - d) < 0.003
                };
                let mut r = vec![0.0f64; 1200];
                let lo = from + t * chunk;
                for ms in lo..(lo + chunk).min(to) {
                    let seed = ms as u32;
                    let mut g = vcod_common::rng::GlibcRand::new(seed);
                    for x in r.iter_mut() {
                        *x = f64::from(g.next_u31()) / 2_147_483_648.0;
                    }
                    let fit = |at: usize, (a, f): (f64, f64)| {
                        at + 1 < r.len() && near(r[at], a) && (r[at + 1] - f).abs() < 0.02
                    };
                    for j in 0..r.len() - 2 {
                        if !fit(j, draws[0]) {
                            continue;
                        }
                        // Each later round six or more draws on.
                        let mut at = j;
                        let mut gaps = Vec::new();
                        for &d in &draws[1..] {
                            match (6..30).find(|g| fit(at + g, d)) {
                                Some(g) => {
                                    at += g;
                                    gaps.push(g);
                                }
                                None => break,
                            }
                        }
                        if gaps.len() + 1 >= 5 {
                            eprintln!("{name}: seed {seed} (ms {ms}) from draw {j}, gaps {gaps:?}");
                        }
                    }
                }
            });
        }
    });
}
