//! Fall damage against two retail runs of `client-probes/probe_fall`: one at
//! the stock bounds, one under `+set bg_fallDamageMinHeight 200 +set
//! bg_fallDamageMaxHeight 1000`. The probe drops a standing allied player
//! six times and wraps the damage and killed callbacks to log what the
//! engine hands them; each fixture holds those `PROBE` lines and the client's
//! `FALL` and `CMDS` lines (docs/research/cod11-player-clip.md 8.9, 8.10).
//!
//! A fall's impact speed moves with the cmd lengths, since the velocity snap
//! keeps a different share of each cmd's gravity, so ours replays the cmd
//! timeline retail ran (the `CMDS` lines, shifted by the gap between the two
//! runs' first drops) on the real server, each cmd delivered ahead of the
//! frame whose snapshot first reported it. Ours is held to:
//!
//! - every retail `FALL` line from the second drop on, `t` and `ct` shifted:
//!   origin, velocity, ground, `pm_flags`, `pm_time`, health and the event
//!   ring, so each landing's parm, stun and damage, exactly. The first drop
//!   runs on our own cadence until its line names the shift;
//! - the probe's lines, timestamps aside: the damage callback's arguments
//!   (undefined entities and vectors, `MOD_FALLING`, weapon and hit location
//!   `none`, the damage and health), the killed callback's (the world twice,
//!   a zero direction), and the session state and health each hit leaves;
//! - each landing's damage is `ClientEvents`' share of `maxhealth` for the
//!   landing pain's parm, on both sides;
//! - no landing raises `EV_PAIN`, on either side: the fall's
//!   `pain_debounce_time` holds it off.
//!
//! `FALL_REPORT=1` prints both sides' parms. Needs `COD_DIR`; without the
//! paks it returns early.

mod common;

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;
use std::time::{Duration, Instant};
use vcod_common::net::msg::{NULL_USERCMD, UserCmd};
use vcod_common::net::protocol::PROTOCOL_V1;
use vcod_common::net::snapshot::Snapshot;

const PROBE_PATH: &str = "maps/mp/gametypes/probe_fall";
const PROBE_SRC: &str = "../gsc/tests/fixtures/semantics/client-probes/probe_fall.gsc";
const MAP: &str = "mp_carentan";
const STOCK: &str = "tests/fixtures/playerstate/mp_carentan-dm-fall-damage.txt";
const CVARS: &str = "tests/fixtures/playerstate/mp_carentan-dm-fall-damage-cvars.txt";
const WALK: &str = "tests/fixtures/playerstate/mp_carentan-dm-fall-walk.txt";
const CORPSE: &str = "tests/fixtures/playerstate/mp_carentan-dm-fall-corpse.txt";
/// `--probe-fall-walk`'s yaw in the walk fixture.
const WALK_YAW: f32 = 315.0;

const EV_LANDING_PAIN: std::ops::RangeInclusive<i32> = 116..=138;
const EV_PAIN: i32 = 187;
const FRAME_MS: i32 = 50;

/// `ClientEvents`' damage for a landing pain's parm, transcribed from the
/// research doc (8.8) and held to retail's numbers by the gate itself.
fn expected_damage(percent: i32, max_health: i32) -> i32 {
    let share = if percent > 99 {
        f64::from(1.1f32)
    } else {
        f64::from(percent) * f64::from(0.01f32)
    };
    (f64::from(max_health) * share) as i32
}

/// The landing pains and `EV_PAIN`s a run raised, in order.
#[derive(Default, Debug)]
struct Events {
    pains: Vec<i32>,
    ev_pain: usize,
}

impl Events {
    fn take(&mut self, event: i32, parm: i32) {
        if EV_LANDING_PAIN.contains(&event) {
            self.pains.push(parm);
        } else if event == EV_PAIN {
            self.ev_pain += 1;
        }
    }
}

/// One `FALL` line: the snapshot's server time, its `commandTime`, and
/// everything after the `ct=` field.
#[derive(Clone, Debug, PartialEq)]
struct FallLine {
    t: i32,
    ct: i32,
    rest: String,
    seq: i32,
    events: [i32; 4],
    parms: [i32; 4],
}

fn parse_fall(l: &str) -> FallLine {
    let field = |k: &str| {
        l.split_whitespace()
            .find_map(|t| t.strip_prefix(k))
            .unwrap_or_else(|| panic!("no {k} in {l}"))
    };
    let list = |k: &str| -> [i32; 4] {
        let v: Vec<i32> = field(k)
            .trim_matches(['[', ']'])
            .split(',')
            .map(|n| n.parse().unwrap())
            .collect();
        v.try_into().unwrap()
    };
    let rest = l.split_once(" origin=").expect("an origin field").1;
    FallLine {
        t: field("t=").parse().unwrap(),
        ct: field("ct=").parse().unwrap(),
        rest: format!("origin={rest}"),
        seq: field("seq=").parse().unwrap(),
        events: list("events="),
        parms: list("parms="),
    }
}

/// The probe client's `FALL` line for a snapshot (`crates/client/src/probe.rs`,
/// `FallProbe::observe`).
fn fall_line(s: &Snapshot) -> FallLine {
    let p = &PROTOCOL_V1;
    let i = |n: &str| s.ps.field_i32(p, n);
    let f = |n: &str| f32::from_bits(s.ps.field_i32(p, n) as u32);
    let events: [i32; 4] = std::array::from_fn(|k| i(&format!("events[{k}]")));
    let parms: [i32; 4] = std::array::from_fn(|k| i(&format!("eventParms[{k}]")));
    let line = format!(
        "FALL t={} ct={} origin={:.3},{:.3},{:.3} vel={},{},{} ground={} pm_type={} pm_flags=0x{:x} pm_time={} \
health={} seq={} events=[{},{},{},{}] parms=[{},{},{},{}]",
        s.server_time,
        i("commandTime"),
        f("origin[0]"),
        f("origin[1]"),
        f("origin[2]"),
        f("velocity[0]"),
        f("velocity[1]"),
        f("velocity[2]"),
        i("groundEntityNum"),
        i("pm_type"),
        i("pm_flags"),
        i("pm_time"),
        s.ps.health(),
        i("eventSequence"),
        events[0],
        events[1],
        events[2],
        events[3],
        parms[0],
        parms[1],
        parms[2],
        parms[3],
    );
    parse_fall(&line)
}

/// The events each new `seq` of a run's `FALL` lines brought, off its ring.
fn ring_events<'a>(lines: impl Iterator<Item = &'a FallLine>) -> Events {
    let mut out = Events::default();
    let mut last_seq = 0;
    for l in lines {
        for i in last_seq.max(l.seq - 4)..l.seq {
            out.take(l.events[(i & 3) as usize], l.parms[(i & 3) as usize]);
        }
        last_seq = l.seq;
    }
    out
}

/// Two `FALL` lines' tails equal but for each origin component, which may
/// read one unit apart in the last printed decimal: a fall of a few hundred
/// units leaves a one-ulp difference against retail's x87 now and then
/// (docs/research/cod11-player-clip.md 8.9).
fn same_but_origin_ulp(a: &str, b: &str) -> bool {
    let split = |s: &str| -> ([f32; 3], String) {
        let (o, rest) = s
            .strip_prefix("origin=")
            .and_then(|s| s.split_once(' '))
            .expect("an origin first");
        let v: Vec<f32> = o.split(',').map(|n| n.parse().unwrap()).collect();
        ([v[0], v[1], v[2]], rest.to_string())
    };
    let ((oa, ra), (ob, rb)) = (split(a), split(b));
    ra == rb && oa.iter().zip(ob).all(|(x, y)| (x - y).abs() < 0.0015)
}

/// What a retail fixture carries.
struct Retail {
    probe: Vec<String>,
    falls: Vec<FallLine>,
    /// Every cmd the probe client sent, in the order it sent them: its
    /// `serverTime` and,
    /// under `--probe-fall-walk`, the yaw word it carried with forward held.
    cmds: Vec<(i32, Option<i32>)>,
}

fn parse_retail(text: &str) -> Retail {
    let probe = text
        .lines()
        .filter_map(|l| l.strip_prefix("# PROBE "))
        .map(|l| format!("PROBE {l}"))
        .collect();
    let falls = text
        .lines()
        .filter(|l| l.starts_with("FALL "))
        .map(parse_fall)
        .collect();
    let mut cmds = Vec::new();
    for l in text.lines().filter_map(|l| l.strip_prefix("CMDS st=")) {
        let (st, rest) = l.split_once(" d=").expect("a CMDS line");
        let (steps, yaws) = match rest.split_once(" yaw=") {
            Some((steps, yaws)) => (steps, Some(yaws)),
            None => (rest, None),
        };
        let mut t: i32 = st.parse().unwrap();
        let mut times = vec![t];
        for d in steps.split(',') {
            t += d.parse::<i32>().unwrap();
            times.push(t);
        }
        let yaws: Vec<Option<i32>> = match yaws {
            Some(y) => y.split(',').map(|w| Some(w.parse().unwrap())).collect(),
            None => vec![None; times.len()],
        };
        assert_eq!(yaws.len(), times.len(), "{l}");
        cmds.extend(times.into_iter().zip(yaws));
    }
    Retail { probe, falls, cmds }
}

/// The time on the n-th `PROBE drop` line.
fn drop_time(probe: &[String], n: usize) -> Option<i32> {
    probe
        .iter()
        .filter(|l| l.starts_with("PROBE drop "))
        .nth(n)
        .map(|l| l.split_whitespace().nth(2).unwrap().parse().unwrap())
}

/// A probe line with its timestamp masked, and, on an `after` line, where
/// the player came to rest when that is not a pmove question: the first drop
/// ran on our own cadence.
fn shape(line: &str, first_after: bool) -> String {
    let mut out: Vec<&str> = line.split_whitespace().collect();
    if out.len() > 2 {
        out[2] = "<t>";
    }
    if out[1] == "after" && first_after {
        out.truncate(6);
    }
    out.join(" ")
}

/// `all_after` masks every `after` line's origin: a walking player ends each
/// drop in a corner whose rest is not a pmove-under-a-timer question.
fn shapes(lines: &[String], all_after: bool) -> Vec<String> {
    let first_after = lines.iter().position(|l| l.starts_with("PROBE after "));
    lines
        .iter()
        .enumerate()
        .map(|(i, l)| shape(l, all_after || Some(i) == first_after))
        .collect()
}

/// The number after `key` in a probe line.
fn num_after(line: &str, key: &str) -> i32 {
    let t: Vec<&str> = line.split_whitespace().collect();
    // `damage` is also the line's own kind, at index 1.
    let i = (2..t.len())
        .find(|&i| t[i] == key)
        .unwrap_or_else(|| panic!("no {key} in {line}"));
    t[i + 1].parse().unwrap()
}

/// Each damage line's figure against the parm of the landing that caused
/// it, and the health the hit left against it.
fn check_damage(side: &str, probe: &[String], pains: &[i32]) -> Vec<String> {
    let mut diffs = Vec::new();
    let hits: Vec<(&String, &String)> = probe
        .iter()
        .enumerate()
        .filter(|(_, l)| l.starts_with("PROBE damage "))
        .map(|(i, l)| {
            let after = probe[i + 1..]
                .iter()
                .find(|n| n.starts_with("PROBE damaged "))
                .expect("a damaged line after every damage line");
            (l, after)
        })
        .collect();
    if hits.len() != pains.len() {
        diffs.push(format!(
            "{side}: {} damage callbacks for {} landing pains {pains:?}",
            hits.len(),
            pains.len()
        ));
        return diffs;
    }
    for ((hit, after), &parm) in hits.iter().zip(pains) {
        let (damage, health, max) = (
            num_after(hit, "damage"),
            num_after(hit, "health"),
            num_after(hit, "maxhealth"),
        );
        let want = expected_damage(parm, max);
        if damage != want {
            diffs.push(format!(
                "{side}: parm {parm} at maxhealth {max} did {damage}, the share is {want}"
            ));
        }
        let left = num_after(after, "health");
        if left != (health - damage).max(0) {
            diffs.push(format!("{side}: {health} less {damage} left {left}"));
        }
    }
    diffs
}

struct Ours {
    probe: Vec<String>,
    /// Every snapshot's `FALL` line, keyed by server time.
    falls: BTreeMap<i32, FallLine>,
    /// Our first drop's time less retail's.
    shift: i32,
    /// Configstring 1 as the client holds it at the end.
    systeminfo: String,
}

/// The probe on our server, `sets` as `+set`s, until it logs `done`. Until
/// the first drop the client sends 16 and 17 ms cmds of its own, walking at
/// `walk` if given; from then on, retail's, shifted onto our clock.
fn run_ours(
    fs: vcod_common::pk3::Pk3Fs,
    sets: &[(&str, &str)],
    walk: Option<f32>,
    retail: &Retail,
) -> Ours {
    let probe = std::fs::read_to_string(PROBE_SRC).expect("read the probe");
    let bsp_path = fs.resolve_map(MAP).expect("the map in the mounted paks");
    let bsp = vcod_common::bsp::parse(&fs.read(&bsp_path).expect("read the bsp")).expect("bsp");
    let mut now = Instant::now();
    let mut sv = vcod_server::Server::new(common::cfg(MAP, "probe_fall"), now);
    sv.overlay_script(PROBE_PATH, &probe);
    sv.set_cvar("probe_teleport", "1");
    for (k, v) in sets {
        sv.set_cvar(k, v);
    }
    sv.load_world(vcod_server::world::World::from_bsp(&bsp, Some(&fs)));
    sv.load_scripts(Rc::new(fs)).expect("load the scripts");
    let q = Rc::new(RefCell::new(common::Queues::default()));
    let (mut cl, _join) = common::join(&mut sv, &q, &mut now, "allies", "m1carbine_mp");

    let p = &PROTOCOL_V1;
    let retail_first = drop_time(&retail.probe, 0).expect("a retail drop");
    // Retail's snapshot times and what each had run, so each cmd reaches
    // ours ahead of the same frame it reached retail's.
    let retail_ct: BTreeMap<i32, i32> = retail.falls.iter().map(|l| (l.t, l.ct)).collect();
    let mut shift = None;
    let mut last_sent = i32::MIN;
    // The next of retail's cmds to send, in the order the client sent them:
    // a client whose clock stepped back sent a stale cmd, which the server
    // drops, between two live ones.
    let mut next_cmd = 0;
    let mut falls = BTreeMap::new();
    // 100 s of server time at most; the probe is done in about 60.
    for _ in 0..2000 {
        let Some(t) = cl.snapshots().newest().map(|s| s.server_time) else {
            panic!("no snapshot after the join");
        };
        let next = t + FRAME_MS;
        let weapon = cl
            .snapshots()
            .newest()
            .map_or(0, |s| s.ps.field_i32(p, "weapon") as u8);
        match shift {
            None => {
                for ms in [16, 17, 17] {
                    now += Duration::from_millis(ms);
                    cl.pump_at(now);
                    let mut cmd = UserCmd {
                        weapon,
                        ..NULL_USERCMD
                    };
                    if let Some(yaw) = walk {
                        cmd.forward = 127;
                        cmd.angles[1] = (yaw * 65536.0 / 360.0).round() as i32;
                    }
                    if let Some(c) = cl.send_frame(&cmd) {
                        last_sent = c.server_time;
                    }
                }
            }
            Some(shift) => {
                now += Duration::from_millis(FRAME_MS as u64);
                cl.pump_at(now);
                // Where retail's snapshot is not in the fixture, a cmd
                // stamped on the frame's own time reached it after the frame,
                // as it did on almost every snapshot that is.
                let upto = retail_ct
                    .get(&(next - shift))
                    .map_or(next - 1, |ct| ct + shift);
                // Verbatim: the yaw word already had retail's `delta_angles`
                // taken off, which the server adds back.
                let mut cmds = Vec::new();
                while let Some(&(st, yaw)) = retail.cmds.get(next_cmd) {
                    let server_time = st + shift;
                    if server_time > upto && server_time > last_sent {
                        break;
                    }
                    next_cmd += 1;
                    let mut cmd = UserCmd {
                        server_time,
                        weapon,
                        ..NULL_USERCMD
                    };
                    if let Some(yaw) = yaw {
                        cmd.forward = 127;
                        cmd.angles[1] = yaw;
                    }
                    cmds.push(cmd);
                    last_sent = last_sent.max(server_time);
                }
                cl.send_cmds(&cmds);
            }
        }
        common::step(&mut sv, &q, &mut cl, now);
        if let Some(s) = cl.snapshots().newest() {
            falls.insert(s.server_time, fall_line(s));
        }
        if shift.is_none() {
            let log: Vec<String> = sv.script_log().iter().map(|l| l.to_string()).collect();
            if let Some(ours) = drop_time(&log, 0) {
                let s = ours - retail_first;
                shift = Some(s);
                next_cmd = retail
                    .cmds
                    .iter()
                    .position(|&(st, _)| st + s > last_sent)
                    .unwrap_or(retail.cmds.len());
            }
        }
        // Past the probe's end too, as far as retail's client logged.
        let logged = shift.is_some_and(|sh| {
            retail
                .falls
                .last()
                .is_none_or(|l| cl.snapshots().newest().map(|s| s.server_time) >= Some(l.t + sh))
        });
        if logged && sv.script_log().iter().any(|l| l.starts_with("PROBE done")) {
            break;
        }
    }
    let probe = sv
        .script_log()
        .iter()
        .filter(|l| l.starts_with("PROBE "))
        .map(|l| l.trim_end().to_string())
        .collect();
    let systeminfo = cl.configstring(1).to_string();
    Ours {
        probe,
        falls,
        shift: shift.expect("our probe dropped the player"),
        systeminfo,
    }
}

/// Every retail `FALL` line from the second drop on against ours at the
/// same time shifted onto our clock: the origin to a unit in its last
/// printed decimal, the rest exactly.
fn compare_rows(retail: &Retail, ours: &Ours, second: i32) -> Vec<String> {
    let mut diffs = Vec::new();
    let mut lines = 0;
    for r in retail.falls.iter().filter(|l| l.t > second) {
        lines += 1;
        let o = ours.falls.get(&(r.t + ours.shift));
        let close =
            o.is_some_and(|o| o.ct == r.ct + ours.shift && same_but_origin_ulp(&o.rest, &r.rest));
        if !close {
            diffs.push(format!(
                "retail t={}: ct={} {}\n  ours: {}",
                r.t,
                r.ct + ours.shift,
                r.rest,
                o.map_or("no snapshot".into(), |o| format!("ct={} {}", o.ct, o.rest))
            ));
        }
    }
    assert!(lines > 100, "only {lines} FALL lines past the second drop");
    diffs
}

/// The value of `key` in a `FALL` line's tail.
fn tail_field<'a>(rest: &'a str, key: &str) -> &'a str {
    rest.split_whitespace()
        .find_map(|t| t.strip_prefix(key))
        .unwrap_or_else(|| panic!("no {key} in {rest}"))
}

/// The walk capture's rows under a landing stun (`pm_flags` 0x100) from the
/// second drop on: ours at the same shifted time holds the same `commandTime`,
/// velocity, ground, `pm_flags`, `pm_time` and health, and an origin within a
/// unit. Each walk ends jittering against a pillar (cod11-player-clip.md
/// 12), and `setorigin` keeps the velocity, so a drop starts with whatever
/// the jitter's phase left: one whose teleport row's velocity differs
/// between the two sides lands on another cmd and is left out, at most two
/// of the five from the second on (the last, fatal one has no stun rows).
/// The rest of the start moves the origin by up to 0.93 and nothing else.
/// Retail must also have pressed into the wall: rows on its plane with a
/// velocity into it.
fn compare_stun_rows(retail: &Retail, ours: &Ours, second: i32) -> Vec<String> {
    const WALL_Y: f32 = 1815.128;
    let drops: Vec<i32> = (0..).map_while(|n| drop_time(&retail.probe, n)).collect();
    let skipped: Vec<(i32, i32)> = drops
        .iter()
        .enumerate()
        .filter(|&(_, &d)| {
            if d < second {
                return false;
            }
            let r = retail.falls.iter().find(|l| l.t >= d);
            let o = r.and_then(|r| ours.falls.get(&(r.t + ours.shift)));
            r.zip(o)
                .is_some_and(|(r, o)| tail_field(&r.rest, "vel=") != tail_field(&o.rest, "vel="))
        })
        .map(|(i, &d)| (d, drops.get(i + 1).copied().unwrap_or(i32::MAX)))
        .collect();
    assert!(skipped.len() <= 2, "drops off another start: {skipped:?}");
    let mut diffs = Vec::new();
    let (mut lines, mut pressed) = (0, 0);
    for r in retail.falls.iter().filter(|l| l.t > second) {
        let flags = i32::from_str_radix(tail_field(&r.rest, "pm_flags=0x"), 16).unwrap();
        if flags & 0x100 == 0 || skipped.iter().any(|&(a, b)| (a..b).contains(&r.t)) {
            continue;
        }
        lines += 1;
        let origin = |rest: &str| -> Vec<f32> {
            tail_field(rest, "origin=")
                .split(',')
                .map(|n| n.parse().unwrap())
                .collect()
        };
        let vel: Vec<i32> = tail_field(&r.rest, "vel=")
            .split(',')
            .map(|n| n.parse().unwrap())
            .collect();
        if (origin(&r.rest)[1] - WALL_Y).abs() < 0.01 && vel[1] < 0 {
            pressed += 1;
        }
        let same = |o: &FallLine| {
            o.ct == r.ct + ours.shift
                && ["vel=", "ground=", "pm_flags=", "pm_time=", "health="]
                    .iter()
                    .all(|k| tail_field(&o.rest, k) == tail_field(&r.rest, k))
                && origin(&o.rest)
                    .iter()
                    .zip(origin(&r.rest))
                    .all(|(a, b)| (a - b).abs() < 1.0)
        };
        let o = ours.falls.get(&(r.t + ours.shift));
        if !o.is_some_and(same) {
            diffs.push(format!(
                "retail t={}: ct={} {}\n  ours: {}",
                r.t,
                r.ct + ours.shift,
                r.rest,
                o.map_or("no snapshot".into(), |o| format!("ct={} {}", o.ct, o.rest))
            ));
        }
    }
    assert!(
        lines > 50,
        "only {lines} stunned FALL lines past the second drop"
    );
    assert!(
        pressed > 20,
        "retail pressed into the wall on only {pressed} rows"
    );
    diffs
}

/// `sets` are the two bounds as retail's systeminfo carried them, `walk` the
/// capture's `--probe-fall-walk` yaw.
fn gate(fixture: &str, sets: &[(&str, &str)], walk: Option<f32>) {
    let Some(fs) = vcod_common::testing::game_fs() else {
        return;
    };
    let text = std::fs::read_to_string(fixture).unwrap_or_else(|e| panic!("read {fixture}: {e}"));
    let retail = parse_retail(&text);
    assert!(
        retail.probe.iter().any(|l| l.starts_with("PROBE done")),
        "{fixture} has no finished run"
    );
    let retail_events = ring_events(retail.falls.iter());
    let ours = run_ours(fs, sets, walk, &retail);
    let ours_events = ring_events(ours.falls.values());
    let mut diffs = Vec::new();

    for (key, stock) in [
        ("bg_fallDamageMaxHeight", "480"),
        ("bg_fallDamageMinHeight", "256"),
    ] {
        let want = sets
            .iter()
            .find(|(k, _)| *k == key)
            .map_or(stock, |(_, v)| v);
        let got = vcod_common::net::info_value_for_key(&ours.systeminfo, key);
        if got != Some(want) {
            diffs.push(format!("systeminfo {key} is {got:?}, retail's {want}"));
        }
    }

    let second = drop_time(&retail.probe, 1).expect("a second retail drop");
    if walk.is_some() {
        diffs.extend(compare_stun_rows(&retail, &ours, second));
    } else {
        diffs.extend(compare_rows(&retail, &ours, second));
    }

    let (rs, os) = (
        shapes(&retail.probe, walk.is_some()),
        shapes(&ours.probe, walk.is_some()),
    );
    if rs != os {
        diffs.push(format!(
            "the probe lines differ\nretail:\n  {}\nours:\n  {}",
            rs.join("\n  "),
            os.join("\n  ")
        ));
    }
    if ours_events.pains != retail_events.pains {
        diffs.push(format!(
            "landing pain parms: retail {:?}, ours {:?}",
            retail_events.pains, ours_events.pains
        ));
    }
    diffs.extend(check_damage("retail", &retail.probe, &retail_events.pains));
    diffs.extend(check_damage("ours", &ours.probe, &ours_events.pains));
    for (side, ev) in [("retail", &retail_events), ("ours", &ours_events)] {
        if ev.ev_pain != 0 {
            diffs.push(format!("{side}: {} EV_PAIN on a fall", ev.ev_pain));
        }
    }
    if std::env::var_os("FALL_REPORT").is_some() {
        eprintln!(
            "{fixture}: retail parms {:?}, ours {:?}",
            retail_events.pains, ours_events.pains
        );
    }
    assert!(diffs.is_empty(), "{fixture}:\n{}", diffs.join("\n"));
}

#[test]
fn fall_damage_matches_retail_at_the_stock_bounds() {
    gate(STOCK, &[], None);
}

#[test]
fn fall_damage_follows_the_bound_cvars_as_retail_does() {
    gate(
        CVARS,
        &[
            ("bg_fallDamageMinHeight", "200"),
            ("bg_fallDamageMaxHeight", "1000"),
        ],
        None,
    );
}

/// The stock-bounds run again, its client printing every snapshot that moved
/// the corpse: the fatal drop's body creeps down the street's grade for the
/// rest of the run, a fall and a landing on every frame (8.11).
#[test]
fn a_corpse_slides_down_the_grade_as_retail_does() {
    gate(CORPSE, &[], None);
}

/// Each stun walks the player obliquely into the street's south wall, where
/// retail's slide hands back the velocity it started with while `pm_time`
/// runs: the rows read a velocity into the wall at a standstill across it
/// (8.5).
#[test]
fn a_stunned_walk_into_a_wall_keeps_its_velocity_as_retail_does() {
    gate(WALK, &[], Some(WALK_YAW));
}
