//! The capture pairs of cod4-combat.md 15 on ours. Each is a retail
//! `cod4_lnxded` 1.7 run (`+devmap mp_crash`, `war`, `sv_pure 0`) of two
//! `cod4_probe`s: `target` (allies, `--goto "207 2291 296"`) and, a second
//! later, `shooter` (axis, `--goto "123 2110 300"`), each logged into
//! `play-1.7-<name>-target.txt` and `-shooter.txt` (recipes on each
//! [`Pair`]). Both join [`Cod4Server`] in process on retail's level clock
//! and seed, each reading its first snapshot when its twin did, sending
//! its twin's client commands as far past it as its twin did, spawning on
//! its twin's spot and from there sending its twin's own cmds, a frame's
//! two packets in the order retail received them. Each gate
//! then compares a window of both clients' snapshots with retail's: the
//! listed playerstate words and every entity, by number, word for word.
//! What still differs is the gate's `GAPS`. Needs `COD4_DIR`.
//!
//! `CAPTURE_AB_REPORT=1` prints every differing row.

use std::collections::{BTreeMap, VecDeque};
use std::time::{Duration, Instant};

use vcod_cod4::net::capture::{Item, Line, Session};
use vcod_cod4::net::clmsg::ClOp;
use vcod_cod4::net::delta::EntityState;
use vcod_cod4::net::parse::{Op, Snapshot};

use super::wire_ab::{Probe, Side, replay_schedule_timed, retail, side};
use super::*;

/// One retail run of the two probes.
struct Pair {
    /// The captures' prefix: `play-1.7-<name>-target.txt`, `-shooter.txt`.
    name: &'static str,
    /// The run's `srand` seed (`G_InitGame`), recovered off the two
    /// challenges by `wire_ab::recover_game_seed` (cod4-combat.md 10.9).
    seed: u32,
    /// The probe gsc's directory under `crates/cod4/tests/fixtures/gsc`,
    /// which retail ran as a mod's (`+set fs_game`).
    gsc: Option<&'static str>,
    /// Where the comparison stops short of the captures' end.
    until: Option<i32>,
}

/// What a probe's capture fixes for its twin on ours.
struct Twin {
    slot: u32,
    /// The server times of its first snapshot, its spawn and its last.
    first: i32,
    spawn: i32,
    last: i32,
    /// The spawn's spot and yaw (the script's pick is a `randomInt` off a
    /// clock-seeded stream).
    pin: crate::game::host::SpawnPin,
    /// Each client command but `cp` (which `Probe` sends itself) by how
    /// far past the first snapshot its newest snapshot was when it went,
    /// an `mr`'s server id as `{sid}`.
    commands: Vec<(i32, &'static str)>,
}

fn twin(lines: &[Line]) -> Twin {
    let mut s = Session::new();
    let (mut newest, mut first) = (0, None);
    let mut commands: BTreeMap<i32, (i32, &'static str)> = BTreeMap::new();
    for l in lines.iter().filter(|l| l.dir != "auth") {
        match s.feed(l) {
            Ok(Some(Item::Client { msg, .. })) => {
                for op in &msg.ops {
                    if let ClOp::Command { sequence, text } = op
                        && !text.starts_with(b"cp ")
                    {
                        let mut t = String::from_utf8_lossy(text).into_owned();
                        if let Some(rest) = t.strip_prefix("mr ")
                            && let Some((_, menu)) = rest.split_once(' ')
                        {
                            t = format!("mr {{sid}} {menu}");
                        }
                        let at = newest - first.unwrap_or(newest);
                        commands
                            .entry(*sequence)
                            .or_insert_with(|| (at, Box::leak(t.into_boxed_str())));
                    }
                }
            }
            Ok(Some(Item::Server { parsed, .. })) => {
                for op in &parsed.ops {
                    if let Op::Snapshot(i) = op {
                        newest = s.client.snapshots[*i].server_time;
                        first.get_or_insert(newest);
                    }
                }
            }
            _ => {}
        }
    }
    let snaps = &s.client.snapshots;
    let slot = snaps[0].ps.get("clientNum").unwrap();
    let spawn = snaps
        .iter()
        .find(|f| f.ps.get("pm_type") == Some(0) && f.ps.get("clientNum") == Some(slot))
        .expect("a spawn");
    let v = |n: &str| [0, 1, 2].map(|k| spawn.ps.get_f32(&format!("{n}[{k}]")).unwrap_or(0.0));
    Twin {
        slot,
        first: snaps[0].server_time,
        spawn: spawn.server_time,
        last: snaps.last().unwrap().server_time,
        pin: Some((v("origin"), [0.0, v("viewangles")[1], 0.0])),
        commands: commands.into_values().collect(),
    }
}

/// The probe clock's time each snapshot first arrived at, by server time.
fn snapshot_arrivals(lines: &[Line]) -> BTreeMap<i32, u64> {
    let mut s = Session::new();
    let mut out = BTreeMap::new();
    for l in lines.iter().filter(|l| l.dir != "auth") {
        if let Ok(Some(Item::Server { parsed, .. })) = s.feed(l) {
            for op in &parsed.ops {
                if let Op::Snapshot(i) = op {
                    out.entry(s.client.snapshots[*i].server_time)
                        .or_insert(l.ms);
                }
            }
        }
    }
    out
}

/// What to add to `b`'s clock to read it on `a`'s: the median gap between
/// the two probes' arrivals of the same snapshot (one frame's messages
/// leave together, cod4-server-frame.md 4).
fn clock_offset(a: &BTreeMap<i32, u64>, b: &BTreeMap<i32, u64>) -> i64 {
    let mut gaps: Vec<i64> = a
        .iter()
        .filter_map(|(t, ma)| Some(*ma as i64 - *b.get(t)? as i64))
        .collect();
    gaps.sort_unstable();
    gaps[gaps.len() / 2]
}

/// A hook run before each tick's packets: the server and the time.
type Hook<'a> = dyn FnMut(&mut Cod4Server, i32) + 'a;

/// What one run of a [`Pair`] gives: retail's two sides, ours, and the
/// script errors of ours.
struct Run {
    retail: [Side; 2],
    ours: [Side; 2],
    aborts: Vec<String>,
}

/// The pair's two probes against `Cod4Server` to the captures' last
/// snapshot.
fn run(install: &vcod_cod4::install::Install, pair: &Pair, hook: &mut Hook) -> Run {
    let lines = ["target", "shooter"].map(|w| retail(&format!("{}-{w}", pair.name)));
    let twins = [0, 1].map(|k| twin(&lines[k]));
    let retail_sides = [0, 1].map(|k| side(&lines[k]));
    let cs0 = String::from_utf8_lossy(retail_sides[0].gs.configstring(0)).into_owned();
    let mut sv = Cod4Server::new(
        Cod4Config {
            map: "mp_crash".into(),
            gametype: "war".into(),
            hostname: "CoD4Host".into(),
            max_clients: 8,
        },
        7,
    );
    for (k, v) in vcod_common::net::connectionless::info_pairs(&cs0) {
        if !matches!(k, "protocol" | "shortversion" | "fs_game") {
            sv.set_cvar(k, v);
        }
    }
    // `+devmap`: cheats on, so `setviewpos` runs.
    sv.set_cvar("sv_cheats", "1");
    sv.pin_game_seed(pair.seed);
    if let Some(gsc) = pair.gsc {
        sv.set_script_overrides(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../cod4/tests/fixtures/gsc")
                .join(gsc),
        );
    }
    sv.load(Some(install)).unwrap();
    let mut pins = vec![VecDeque::new(); crate::server::MAX_CLIENTS];
    for t in &twins {
        pins[t.slot as usize].push_back(t.pin);
    }
    sv.game.level.as_mut().unwrap().rt.host.spawn_pins =
        Some(std::rc::Rc::new(std::cell::RefCell::new(pins)));
    let arrivals = [0, 1].map(|k| snapshot_arrivals(&lines[k]));
    let offsets = [0, clock_offset(&arrivals[0], &arrivals[1])];
    // Each probe's replayed packets' send times on the target's clock, by
    // the tick they go at: within a tick the probes send in retail's
    // arrival order, as each cmd is thought on arrival (cod4-server-frame.md
    // 2) and a hit reads whatever cmds the victim has run.
    let mut sends: Vec<VecDeque<(i32, i64)>> = Vec::new();
    let mut probes: Vec<Probe> = twins
        .iter()
        .zip(["target", "shooter"])
        .zip(&lines)
        .zip(offsets)
        .map(|(((t, name), l), offset)| {
            let mut p = Probe::new(name, t.slot, 28961 + t.slot as u16, t.first, &t.commands);
            let timed = replay_schedule_timed(l, t.spawn);
            sends.push(
                timed
                    .iter()
                    .map(|(at, ms, _)| (*at, *ms as i64 + offset))
                    .collect(),
            );
            p.replay = timed.into_iter().map(|(at, _, c)| (at, c)).collect();
            p.replay_from = t.spawn;
            p
        })
        .collect();
    let run_ms = twins.iter().map(|t| t.last).max().unwrap() + FRAME_MS;
    let mut now = Instant::now();
    while sv.core.sv_time_ms < run_ms {
        let svs = sv.core.sv_time_ms;
        for p in probes.iter_mut() {
            if !p.joined && svs >= p.connect_at {
                p.join(&mut sv, now);
            }
        }
        hook(&mut sv, svs);
        let mut order: Vec<(i64, usize)> = sends
            .iter_mut()
            .enumerate()
            .map(|(k, q)| {
                let mut first = i64::MAX;
                while let Some(&(at, ms)) = q.front()
                    && at <= svs
                {
                    first = first.min(ms);
                    q.pop_front();
                }
                (first, k)
            })
            .collect();
        order.sort();
        for k in order.into_iter().map(|(_, k)| k) {
            let p = &mut probes[k];
            if !p.joined {
                continue;
            }
            let pkt = p.packet(svs);
            p.log("c2s", svs, &pkt);
            sv.handle_packet(p.addr, &pkt, now);
        }
        now += Duration::from_millis(FRAME_MS as u64);
        sv.tick_frames(now, 1);
        let svs = sv.core.sv_time_ms;
        for (to, pkt) in sv.take_outgoing() {
            if let Some(p) = probes.iter_mut().find(|p| p.addr == to) {
                p.receive(svs, &pkt);
            }
        }
    }
    let aborts = sv.game.level.as_ref().unwrap().rt.aborts();
    let [a, b] = [&probes[0], &probes[1]].map(|p| side(&p.lines));
    Run {
        retail: retail_sides,
        ours: [a, b],
        aborts,
    }
}

/// Every difference of `o` from `r` in the snapshots `window` holds, one
/// line each: the playerstate's `fields`, then each entity by number,
/// word for word. Each line reads `<tag> <what> <field> <time>: ours ..
/// retail ..`.
fn diffs(
    tag: &str,
    r: &Side,
    o: &Side,
    window: std::ops::RangeInclusive<i32>,
    fields: &[&str],
) -> Vec<String> {
    let mut out = Vec::new();
    for a in r.snaps.iter().filter(|f| window.contains(&f.server_time)) {
        let t = a.server_time;
        let Some(b) = o.snaps.iter().find(|f| f.server_time == t) else {
            out.push(format!("{tag} snapshot - {t}: ours none"));
            continue;
        };
        for n in fields {
            let (x, y) = (b.ps.get(n).unwrap_or(0), a.ps.get(n).unwrap_or(0));
            if x != y {
                out.push(format!("{tag} ps {n} {t}: ours {x:#x} retail {y:#x}"));
            }
        }
        out.extend(entity_diffs(tag, a, b));
    }
    out
}

fn entity_diffs(tag: &str, a: &Snapshot, b: &Snapshot) -> Vec<String> {
    let t = a.server_time;
    let mut out = Vec::new();
    let ours: BTreeMap<u32, &EntityState> = b.entities.iter().map(|e| (e.number(), e)).collect();
    let theirs: BTreeMap<u32, &EntityState> = a.entities.iter().map(|e| (e.number(), e)).collect();
    for (n, x) in &theirs {
        let Some(y) = ours.get(n) else {
            out.push(format!(
                "{tag} entity {n} {t}: ours none retail type {}",
                x.etype()
            ));
            continue;
        };
        if x.etype() != y.etype() {
            out.push(format!(
                "{tag} entity {n} {t}: ours type {} retail type {}",
                y.etype(),
                x.etype()
            ));
            continue;
        }
        for f in EntityState::table(x.etype()) {
            let (v, w) = (y.0[f.word()], x.0[f.word()]);
            if v != w {
                out.push(format!(
                    "{tag} entity {n}:{} {} {t}: ours {v:#x} retail {w:#x}",
                    x.etype(),
                    f.name
                ));
            }
        }
    }
    for (n, y) in &ours {
        if !theirs.contains_key(n) {
            out.push(format!(
                "{tag} entity {n} {t}: ours type {} retail none",
                y.etype()
            ));
        }
    }
    out
}

/// Whether `row` is the gap `pattern`: word by word, `*` any word, `*x` a
/// word ending in `x`, else a prefix of the word.
fn gap_matches(pattern: &str, row: &str) -> bool {
    let mut words = row.split_whitespace();
    pattern.split_whitespace().all(|p| {
        words.next().is_some_and(|w| match p.strip_prefix('*') {
            Some(end) => w.ends_with(end),
            None => w.starts_with(p),
        })
    })
}

/// Fails on a script error, a row outside `gaps` ([`gap_matches`]
/// patterns, each with why) or a gap no row matches;
/// `CAPTURE_AB_REPORT=1` prints every row.
fn check(gate: &str, all: &[String], aborts: &[String], gaps: &[(&str, &str)]) {
    let report = std::env::var_os("CAPTURE_AB_REPORT").is_some();
    let gap = |d: &str| gaps.iter().find(|(g, _)| gap_matches(g, d));
    let mut unexplained = Vec::new();
    for d in all {
        match gap(d) {
            Some((_, why)) if report => eprintln!("  {d} (gap: {why})"),
            Some(_) => {}
            None => {
                if report {
                    eprintln!("  {d}");
                }
                unexplained.push(d.clone());
            }
        }
    }
    eprintln!(
        "{gate}: {} rows differ, {} outside GAPS",
        all.len(),
        unexplained.len()
    );
    assert!(aborts.is_empty(), "script errors:\n{}", aborts.join("\n"));
    assert!(
        unexplained.is_empty(),
        "rows outside GAPS (the first 60):\n{}",
        unexplained[..unexplained.len().min(60)].join("\n")
    );
    let unused: Vec<&str> = gaps
        .iter()
        .filter(|(g, _)| !all.iter().any(|d| gap_matches(g, d)))
        .map(|(g, _)| *g)
        .collect();
    assert!(unused.is_empty(), "GAPS rows matching nothing: {unused:?}");
}

/// Every playerstate word the wire carries.
fn all_fields() -> Vec<&'static str> {
    vcod_cod4::net::fields::PLAYER_STATE
        .iter()
        .map(|f| f.name)
        .collect()
}

/// Both clients from the later spawn to the end of the shorter capture (or
/// `until`), every playerstate word, health and ammo, and every entity.
fn pair_diffs(run: &Run, until: Option<i32>) -> Vec<String> {
    let fields = all_fields();
    let mut out = Vec::new();
    let spawns = run.retail.each_ref().map(|s| {
        s.snaps
            .iter()
            .find(|f| f.ps.get("pm_type") == Some(0))
            .map_or(0, |f| f.server_time)
    });
    let end = run
        .retail
        .iter()
        .map(|s| s.snaps.last().unwrap().server_time)
        .min()
        .unwrap()
        .min(until.unwrap_or(i32::MAX));
    let window = spawns[0].max(spawns[1])..=end;
    for (k, tag) in ["target", "shooter"].into_iter().enumerate() {
        let (r, o) = (&run.retail[k], &run.ours[k]);
        out.extend(diffs(tag, r, o, window.clone(), &fields));
        for a in r.snaps.iter().filter(|f| window.contains(&f.server_time)) {
            let Some(b) = o.snaps.iter().find(|f| f.server_time == a.server_time) else {
                continue;
            };
            let t = a.server_time;
            if a.ps.stats != b.ps.stats {
                out.push(format!(
                    "{tag} ps stats {t}: ours {:?} retail {:?}",
                    b.ps.stats, a.ps.stats
                ));
            }
            if a.ps.ammo != b.ps.ammo || a.ps.ammoclip != b.ps.ammoclip {
                out.push(format!("{tag} ps ammo {t}: differs"));
            }
        }
    }
    out
}

/// The pairs. Each probe ran `cod4_probe --name <target|shooter> --qport
/// <4021|4022> --secs 25`, the target `--team allies --goto "207 2291
/// 296"`, the shooter `--team axis --goto "123 2110 300"`, `--script idle`
/// unless the pair says otherwise; the server `+set g_gametype war +set
/// sv_pure 0 +devmap mp_crash` (cod4-handshake.md 7's namespace).
const ROCKET: Pair = Pair {
    // Shooter `--class demolitions_mp,0 --script rocket --goto "697 -2038
    // 200" --weapon 29 --face 105 --pitch -10`: the RPG up, a rocket
    // along a clear line for 3.5 s, a second at the target 4300 units off.
    name: "rocket",
    seed: 3_486_109_213,
    gsc: None,
    until: None,
};

const THROWBACK: Pair = Pair {
    // Target `--script throwback`, shooter `--script frag --aim-z -60`
    // (cod4-gsc.md 9.3): the frag lands short of the target, which takes
    // it and throws it back. Compared to the thrown grenade's strike on
    // the shooter at 50100, which ours misses ([`THROWBACK_GAPS`]).
    name: "throwback",
    seed: 3_487_136_041,
    gsc: None,
    until: Some(50_050),
};

const PRONE: Pair = Pair {
    // `+set scr_player_maxhealth 30`; target `--script prone`, shooter
    // `--script shoot --fire-after 3`: the target lies down and dies prone.
    name: "prone",
    seed: 3_485_363_811,
    gsc: None,
    until: None,
};

const KCENT: Pair = Pair {
    // `+set fs_game mods/vcodkc`, the `killcam_entity` gsc in the mod's
    // iwd.
    name: "kcent",
    seed: 3_485_193_718,
    gsc: Some("killcam_entity"),
    until: None,
};

fn pair_ab(gate: &str, pair: &Pair, gaps: &[(&str, &str)]) {
    let Some(install) = vcod_cod4::testing::cod4_install() else {
        return;
    };
    let run = run(&install, pair, &mut |_, _| {});
    for p in &run.ours {
        assert!(!p.snaps.is_empty());
    }
    check(gate, &pair_diffs(&run, pair.until), &run.aborts, gaps);
}

/// Why a dropped weapon differs: it leaves from the body's middle, not
/// `tag_weapon_right`, and `G_RunItem`'s fall (`trType` 5, then 3 for 50
/// ms) and its rest on its side (roll 90, ground 1022) are left out
/// (cod4-combat.md 6).
const ITEM: &str = "the dropped weapon's start and fall (cod4-combat.md 6)";

/// What the rocket run still differs in.
const ROCKET_GAPS: &[(&str, &str)] = &[
    (
        "* entity *:4 lerp.pos.trDelta",
        "the wobble's velocity, the facing times the speed, a unit in the last \
         place off; the path's points match (cod4-gsc.md 3.2)",
    ),
    (
        "shooter ps velocity",
        "the RPG's fire frame's velocity, a unit in the last place off (cod4-gsc.md 3.2)",
    ),
    (
        "* entity 316",
        "the second rocket takes 316 on ours, 317 on retail; nothing either client \
         saw took 316 (cod4-gsc.md 3.2)",
    ),
    ("* entity 317", "the second rocket's number, as above"),
    (
        "shooter ps viewangles[1]",
        "`setviewpos` 697 -2038 200's `delta_angles` for its first 0.55 s (cod4-gsc.md 3.2)",
    ),
    ("shooter ps delta_angles", "as above"),
    (
        "target entity 1:1 lerp.apos.trBase",
        "the same view on the shooter's body",
    ),
];

/// The RPG's two rockets fly, wobble and burst as retail's (cod4-gsc.md
/// 3.2, "Missiles under 1.7").
#[test]
fn cod4_rocket_ab() {
    pair_ab("cod4_rocket_ab", &ROCKET, ROCKET_GAPS);
}

/// Nothing differs up to the strike.
const THROWBACK_GAPS: &[(&str, &str)] = &[];

/// The live frag's flight, the hint, the pickup and the throw back
/// (cod4-gsc.md 9.3).
#[test]
fn cod4_throwback_ab() {
    pair_ab("cod4_throwback_ab", &THROWBACK, THROWBACK_GAPS);
}

/// Why the prone run's killcam differs: retail trims its look back to
/// 4200, ours to 4250 (a frame older), and ends it a frame apart
/// (cod4-combat.md 7). Its rows but the clocks are held to the killcam's
/// frames from 50000 on.
const TRIM: &str = "the killcam's trim, 4200 against ours 4250 (cod4-combat.md 7)";

const PRONE_GAPS: &[(&str, &str)] = &[
    ("* entity *:3", ITEM),
    ("target ps commandTime", TRIM),
    ("target ps deltaTime", TRIM),
    ("target ps eventSequence 5", TRIM),
    ("target ps events 5", TRIM),
    ("target ps weap 5", TRIM),
    ("target ps torsoAnim 5", TRIM),
    ("target ps torsoTimer 5", TRIM),
    ("target ps pm_flags 5", TRIM),
    ("target ps legsAnim 5", TRIM),
    ("target ps eFlags 5", TRIM),
    ("target ps viewangles 5", TRIM),
    ("target ps aimSpreadScale 5", TRIM),
    ("target ps fWeaponPosFrac 5", TRIM),
    ("target ps ammo 5", TRIM),
];

/// A prone body's death and its corpse's torso (cod4-combat.md 5).
#[test]
fn cod4_prone_death_ab() {
    pair_ab("cod4_prone_death_ab", &PRONE, PRONE_GAPS);
}

const KCENT_GAPS: &[(&str, &str)] = &[
    ("* entity *:3", ITEM),
    (
        "target ps viewangles[1]",
        "the killcam's archived view: ours keeps the shooter's spawn yaw 90 where \
         its live view after `setviewpos` reads 0 (cod4-combat.md 7)",
    ),
    (
        "shooter ps origin[2]",
        "`placeSpawnPoint`'s drop onto the terrain (cod4-pmove.md 13.2)",
    ),
];

/// A killcam with a `killcamentity`: the follower's playerstate names the
/// script's inflictor, and that entity rides the archived frames with its
/// trajectory's time moved up by the look back (cod4-combat.md 7).
#[test]
fn cod4_killcam_entity_ab() {
    pair_ab("cod4_killcam_entity_ab", &KCENT, KCENT_GAPS);
}
