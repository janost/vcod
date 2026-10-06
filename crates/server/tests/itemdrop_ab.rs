//! Item flight, landing and respawn against one retail run of
//! `client-probes/probe_itemdrop` on mp_carentan
//! (`items/mp_carentan-dm-itemdrop.txt`, header for the recipe; read in
//! `docs/research/cod11-items.md` section 14). The fixture holds the probe's
//! `PROBE` lines and every `ITEM`/`ITEM_GONE` line the `--probe-items`
//! client printed.
//!
//! Two gates:
//!
//! - every flight retail sent is replayed through our `G_RunItem` from the
//!   trajectory retail put on the wire, on the map's own collision: each
//!   later trajectory retail sent (a wall's nudge) on the same frame and to
//!   the hundredth, and the landing on the same frame, at the same x and y,
//!   at most the half unit of random lift above our sweep's end, aligned to
//!   the same angles and on the same ground;
//! - the probe runs on our server with a client holding its view the way the
//!   probe client did, and its items are held to retail's frame by frame,
//!   timed from `PROBE start`: what the client sees and when it sees it,
//!   with the launch's random climb, spin and lift held to their ranges
//!   rather than to retail's draws.
//!
//! `ITEM_REPORT=1` prints both sides' item lines.
//!
//! Needs `COD_DIR`; without the paks both tests return early.

mod common;

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;
use std::time::{Duration, Instant};

use glam::Vec3;
use vcod_common::net::msg::{NULL_USERCMD, UserCmd};
use vcod_common::net::protocol::PROTOCOL_V1;
use vcod_common::net::trajectory::{TR_GRAVITY, TR_LINEAR, TR_STATIONARY, Trajectory};
use vcod_server::game::item::{LAUNCH_CLIPMASK, RUN_CLIPMASK, Ran, bounds, run_flight};

const MAP: &str = "mp_carentan";
const PROBE_PATH: &str = "maps/mp/gametypes/probe_itemdrop";
const PROBE_SRC: &str = "../gsc/tests/fixtures/semantics/client-probes/probe_itemdrop.gsc";
const FIXTURE: &str = "tests/fixtures/items/mp_carentan-dm-itemdrop.txt";
const ET_ITEM: i32 = 3;
const FRAME_MS: i32 = 50;
/// The most a landing sits above the sweep's end: `G_BounceItem`'s lift is
/// `0.5 + 0.5 * neg_unit()`, in (0, 0.5].
const LIFT_MAX: f32 = 0.5;
/// Where the two sides' start positions may differ: a player resting a fifth
/// of a unit off retail's (items doc 13.4).
const START_TOL: f32 = 0.5;
/// Where a drop may start off retail's: the tag sways with the idle
/// animation, whose phase is wherever each server last restarted it
/// (items doc 14).
const TAG_TOL: f32 = 4.0;
/// Angles the same surface aligns to.
const ANGLE_TOL: f32 = 0.05;
/// The drops, whose climb, spin and so landing frame are random draws.
const DROPS: [&str; 4] = ["flat", "wall", "slope", "health"];
/// The probe's respawn half, which ours does not model yet: both items stay
/// taken (items doc 11).
const GAPS: [&str; 2] = ["randomhealth", "respawncolt"];

fn report() -> bool {
    std::env::var("ITEM_REPORT").is_ok_and(|v| v == "1")
}

/// One `ITEM` line, either side's.
#[derive(Clone, Debug, PartialEq)]
struct Item {
    t: i32,
    num: u32,
    index: i32,
    client_num: i32,
    eflags: i32,
    ground: i32,
    pos: Trajectory,
    apos: Trajectory,
    seq: i32,
    events: [i32; 4],
}

#[derive(Clone, Debug)]
enum Line {
    Item(Item),
    Gone { t: i32, num: u32 },
}

impl Line {
    fn t(&self) -> i32 {
        match self {
            Line::Item(i) => i.t,
            Line::Gone { t, .. } => *t,
        }
    }

    fn num(&self) -> u32 {
        match self {
            Line::Item(i) => i.num,
            Line::Gone { num, .. } => *num,
        }
    }
}

fn traj(s: &str) -> Trajectory {
    let v: Vec<f32> = s.split(',').map(|x| x.parse().expect("a number")).collect();
    Trajectory {
        tr_type: v[0] as i32,
        tr_time: v[1] as i32,
        tr_duration: 0,
        base: Vec3::new(v[2], v[3], v[4]),
        delta: Vec3::new(v[5], v[6], v[7]),
    }
}

fn parse_line(line: &str) -> Option<Line> {
    let mut kv = BTreeMap::new();
    let mut words = line.split_whitespace();
    let head = words.next()?;
    for w in words {
        if let Some((k, v)) = w.split_once('=') {
            kv.insert(k, v);
        }
    }
    let int = |k: &str| kv[k].parse::<i32>().expect("an int");
    match head {
        "ITEM_GONE" => Some(Line::Gone {
            t: int("t"),
            num: int("num") as u32,
        }),
        "ITEM" => {
            let ev: Vec<i32> = kv["events"]
                .split(',')
                .map(|x| x.parse().unwrap())
                .collect();
            Some(Line::Item(Item {
                t: int("t"),
                num: int("num") as u32,
                index: int("index"),
                client_num: int("clientNum"),
                eflags: int("eFlags"),
                ground: int("ground"),
                pos: traj(kv["pos"]),
                apos: traj(kv["apos"]),
                seq: int("seq"),
                events: [ev[0], ev[1], ev[2], ev[3]],
            }))
        }
        _ => None,
    }
}

/// The fixture or our run: the probe's lines, its items, and the entity each
/// tag named.
struct Run {
    probe: Vec<String>,
    lines: Vec<Line>,
    start: i32,
    tags: BTreeMap<String, u32>,
}

impl Run {
    fn new(probe: Vec<String>, lines: Vec<Line>) -> Run {
        let start = probe
            .iter()
            .find_map(|l| l.strip_prefix("PROBE start "))
            .and_then(|t| t.trim().parse().ok())
            .expect("a PROBE start line");
        let mut tags = BTreeMap::new();
        for l in &probe {
            let w: Vec<&str> = l.split_whitespace().collect();
            if matches!(w.get(1), Some(&"drop") | Some(&"spawned")) && w.len() > 4 {
                tags.insert(w[2].to_string(), w[4].parse().expect("an entity number"));
            }
        }
        Run {
            probe,
            lines,
            start,
            tags,
        }
    }

    /// One tag's lines, in order, while its entity number was its own: a
    /// later item that reuses a freed number is someone else's.
    fn of(&self, tag: &str) -> Vec<Line> {
        let num = self.tags[tag];
        let from = self
            .probe
            .iter()
            .find_map(|l| {
                let w: Vec<&str> = l.split_whitespace().collect();
                (w.get(2) == Some(&tag) && matches!(w[1], "drop" | "spawned"))
                    .then(|| w[3].parse::<i32>().unwrap())
            })
            .expect("the tag's own line");
        self.lines
            .iter()
            .filter(|l| l.num() == num && l.t() >= from)
            .cloned()
            .collect()
    }
}

fn retail() -> Run {
    let text = std::fs::read_to_string(FIXTURE).unwrap_or_else(|e| panic!("read {FIXTURE}: {e}"));
    let probe = text
        .lines()
        .filter(|l| l.starts_with("PROBE "))
        .map(str::to_string)
        .collect();
    let lines = text.lines().filter_map(parse_line).collect();
    Run::new(probe, lines)
}

fn world(fs: &vcod_common::pk3::Pk3Fs) -> vcod_server::world::World {
    let bsp_path = fs.resolve_map(MAP).expect("the map in the mounted paks");
    let bsp = vcod_common::bsp::parse(&fs.read(&bsp_path).expect("read the bsp")).expect("bsp");
    vcod_server::world::World::from_bsp(&bsp, Some(fs))
}

/// Two angles apart, wrapped.
fn angle_off(a: f32, b: f32) -> f32 {
    let d = (a - b).rem_euclid(360.0);
    d.min(360.0 - d)
}

fn angles_off(a: Vec3, b: Vec3) -> f32 {
    angle_off(a.x, b.x)
        .max(angle_off(a.y, b.y))
        .max(angle_off(a.z, b.z))
}

#[test]
fn the_flights_land_where_retail_lands() {
    let Some(fs) = vcod_common::testing::game_fs() else {
        return;
    };
    let retail = retail();
    let world = world(&fs);
    let mut rows = Vec::new();
    for tag in retail.tags.keys() {
        let lines: Vec<Item> = retail
            .of(tag)
            .into_iter()
            .filter_map(|l| match l {
                Line::Item(i) => Some(i),
                Line::Gone { .. } => None,
            })
            .collect();
        let Some(first) = lines.iter().position(|i| i.pos.tr_type == TR_GRAVITY) else {
            continue;
        };
        let launch = &lines[first];
        // A script spawn is still `ENTITYNUM_NONE` and has no clipmask.
        let mask = if launch.ground == 1023 {
            RUN_CLIPMASK
        } else {
            LAUNCH_CLIPMASK
        };
        let weapon = launch.index <= 64;
        let mut pos = launch.pos;
        let mut origin = pos.evaluate(pos.tr_time);
        let mut now = pos.tr_time;
        let mut nudges = Vec::new();
        let landed = loop {
            let ran = run_flight(
                &world.collision,
                &mut pos,
                &mut origin,
                bounds(weapon),
                mask,
                now,
                &mut || 0.0,
            );
            match ran {
                Ran::Flew => {}
                Ran::Nudged => nudges.push((now, pos)),
                Ran::Landed { ground, normal } => break Some((now, ground, normal)),
            }
            now += FRAME_MS;
            if now > launch.t + 20_000 {
                break None;
            }
        };
        let Some((at, ground, normal)) = landed else {
            rows.push(format!("{tag}: never landed"));
            continue;
        };
        // Retail's later trajectories while it flew: each a nudge of ours.
        let flying: Vec<&Item> = lines[first + 1..]
            .iter()
            .take_while(|i| i.pos.tr_type == TR_GRAVITY)
            .filter(|i| i.pos != launch.pos)
            .collect();
        if flying.len() != nudges.len() {
            rows.push(format!(
                "{tag}: retail nudged {} times, ours {}",
                flying.len(),
                nudges.len()
            ));
        }
        for (r, (t, p)) in flying.iter().zip(&nudges) {
            if r.t != *t
                || r.pos.tr_time != p.tr_time
                || (r.pos.base - p.base).abs().max_element() > 0.01
                || r.pos.delta != p.delta
            {
                rows.push(format!(
                    "{tag}: retail nudged at {} to {:?}, ours at {t} to {:?}",
                    r.t, r.pos, p
                ));
            }
        }
        let Some(rest) = lines[first + 1..]
            .iter()
            .find(|i| i.pos.tr_type == TR_STATIONARY)
        else {
            rows.push(format!("{tag}: retail never landed"));
            continue;
        };
        // A taken item falls while hidden, so its landing is only seen on
        // its respawn.
        let hidden = retail
            .of(tag)
            .iter()
            .any(|l| matches!(l, Line::Gone { t, .. } if *t <= rest.t));
        if (!hidden && rest.t != at) || (hidden && at > rest.t) {
            rows.push(format!("{tag}: retail landed at {}, ours at {at}", rest.t));
        }
        let lift = rest.pos.base.z - origin.z;
        if (rest.pos.base.truncate() - origin.truncate())
            .abs()
            .max_element()
            > 0.01
            || !(-0.01..=LIFT_MAX + 0.01).contains(&lift)
        {
            rows.push(format!(
                "{tag}: retail rests at {}, ours at {origin} before the lift",
                rest.pos.base
            ));
        }
        if rest.ground != ground as i32 {
            rows.push(format!(
                "{tag}: retail's ground {}, ours {ground}",
                rest.ground
            ));
        }
        let current = launch.apos.base.to_array();
        let aligned = Vec3::from(vcod_server::game::spawn::align_to_surface(
            current,
            normal.into(),
            weapon,
        ));
        if angles_off(aligned, rest.apos.base) > ANGLE_TOL {
            rows.push(format!(
                "{tag}: retail lies at {}, ours at {aligned}",
                rest.apos.base
            ));
        }
        if report() {
            eprintln!(
                "{tag}: landed {at} at {origin} lift {lift:.3} angles {aligned} nudges {}",
                nudges.len()
            );
        }
    }
    assert!(rows.is_empty(), "{}", rows.join("\n"));
}

/// Our server under the probe, with a client sending the probe client's
/// cadence and a zero view, until `PROBE done`.
fn run_ours(fs: vcod_common::pk3::Pk3Fs) -> Run {
    let probe = std::fs::read_to_string(PROBE_SRC).expect("read the probe");
    let world = world(&fs);
    let mut now = Instant::now();
    let mut sv = vcod_server::Server::new(common::cfg(MAP, "probe_itemdrop"), now);
    sv.overlay_script(PROBE_PATH, &probe);
    sv.load_world(world);
    sv.load_scripts(Rc::new(fs)).expect("load the scripts");
    let q = Rc::new(RefCell::new(common::Queues::default()));
    let (mut cl, _join) = common::join(&mut sv, &q, &mut now, "allies", "m1carbine_mp");
    let p = &PROTOCOL_V1;
    let carbine = vcod_server::configstrings::weapon_index("m1carbine_mp").unwrap() as u8;
    let mut raised = false;
    let mut last: BTreeMap<u32, Item> = BTreeMap::new();
    let mut lines = Vec::new();
    let mut seen_time = i32::MIN;
    for _ in 0..1600 {
        // Every cmd carries the weapon the playerstate holds, as the probe
        // client's do; a 0 would read as a holster and pose another torso.
        // The join's null cmds holstered the spawn's carbine, so it is
        // asked for again until the playerstate holds it.
        let held = cl
            .snapshots()
            .newest()
            .map_or(0, |s| s.ps.field_i32(p, "weapon") as u8);
        raised |= held == carbine;
        let weapon = if raised { held } else { carbine };
        for ms in [16, 17, 17] {
            now += Duration::from_millis(ms);
            cl.pump_at(now);
            cl.send_frame(&UserCmd {
                weapon,
                ..NULL_USERCMD
            });
        }
        common::step(&mut sv, &q, &mut cl, now);
        if let Some(s) = cl.snapshots().newest()
            && s.server_time != seen_time
        {
            seen_time = s.server_time;
            let mut here = BTreeMap::new();
            for (&num, e) in &s.entities {
                if e.field_i32(p, "eType") != ET_ITEM {
                    continue;
                }
                let i = |n: &str| e.field_i32(p, n);
                let item = Item {
                    t: s.server_time,
                    num,
                    index: i("index"),
                    client_num: i("clientNum"),
                    eflags: i("eFlags"),
                    ground: i("groundEntityNum"),
                    pos: Trajectory::read(e, p, "pos"),
                    apos: Trajectory::read(e, p, "apos"),
                    seq: i("eventSequence"),
                    events: [
                        i("events[0]"),
                        i("events[1]"),
                        i("events[2]"),
                        i("events[3]"),
                    ],
                };
                // A line per change, the way the probe client prints them.
                let mut key = item.clone();
                key.t = 0;
                if last.get(&num) != Some(&key) {
                    lines.push(Line::Item(item));
                }
                here.insert(num, key);
            }
            for &num in last.keys().filter(|n| !here.contains_key(n)) {
                lines.push(Line::Gone {
                    t: s.server_time,
                    num,
                });
            }
            last = here;
        }
        if sv.script_log().iter().any(|l| l.starts_with("PROBE done")) {
            break;
        }
    }
    let probe = sv
        .script_log()
        .iter()
        .filter(|l| l.starts_with("PROBE "))
        .map(|l| l.trim_end().to_string())
        .collect();
    Run::new(probe, lines)
}

fn vec3(s: &str) -> Vec3 {
    let v: Vec<f32> = s
        .trim_matches(|c| c == '(' || c == ')')
        .split(',')
        .map(|x| x.trim().parse().expect("a vector component"))
        .collect();
    Vec3::new(v[0], v[1], v[2])
}

/// A `PROBE` line as the two sides must agree on it: its words with the
/// times made relative, and the vectors pulled out to compare apart.
fn shape(line: &str, start: i32) -> (String, Vec<Vec3>) {
    let mut words = Vec::new();
    let mut vectors = Vec::new();
    let mut rest = line;
    while let Some(open) = rest.find('(') {
        words.extend(rest[..open].split_whitespace().map(str::to_string));
        let close = rest[open..].find(')').expect("a closed vector") + open;
        vectors.push(vec3(&rest[open..=close]));
        rest = &rest[close + 1..];
    }
    words.extend(rest.split_whitespace().map(str::to_string));
    if let Some(t) = words.get(3).and_then(|w| w.parse::<i32>().ok())
        && words[1] != "start"
    {
        words[3] = (t - start).to_string();
    }
    if words[1] == "start" || words[1] == "done" {
        words[2] = (words[2].parse::<i32>().unwrap() - start).to_string();
    }
    (words.join(" "), vectors)
}

#[test]
fn the_probe_runs_as_retail_ran() {
    let Some(fs) = vcod_common::testing::game_fs() else {
        return;
    };
    let retail = retail();
    let ours = run_ours(fs);
    let mut rows = Vec::new();

    // The probe's own lines, the flights' positions aside: a drop's climb is
    // a random draw, so its `at` and `rest` lines are the item gate's.
    let keep = |l: &&String| {
        let w: Vec<&str> = l.split_whitespace().collect();
        !(matches!(w[1], "at" | "rest") && DROPS.contains(&w[2]))
            && !GAPS.contains(&w.get(2).copied().unwrap_or(""))
    };
    let r: Vec<_> = retail
        .probe
        .iter()
        .filter(keep)
        .map(|l| shape(l, retail.start))
        .collect();
    let o: Vec<_> = ours
        .probe
        .iter()
        .filter(keep)
        .map(|l| shape(l, ours.start))
        .collect();
    if r.len() != o.len() {
        rows.push(format!(
            "retail logged {} probe lines, ours {}:\n  retail {:?}\n  ours {:?}",
            r.len(),
            o.len(),
            r.iter().map(|x| &x.0).collect::<Vec<_>>(),
            o.iter().map(|x| &x.0).collect::<Vec<_>>()
        ));
    }
    for ((rw, rv), (ow, ov)) in r.iter().zip(&o) {
        if rw != ow {
            rows.push(format!("probe line\n  retail {rw}\n  ours   {ow}"));
            continue;
        }
        // A weapon drop's third vector is where the tag put it.
        let thrown = rw.starts_with("PROBE drop") && !rw.starts_with("PROBE drop health");
        for (k, (a, b)) in rv.iter().zip(ov).enumerate() {
            let tol = if thrown && k == 2 { TAG_TOL } else { START_TOL };
            if (*a - *b).abs().max_element() > tol {
                rows.push(format!("{rw}: retail {a}, ours {b}"));
            }
        }
    }

    for tag in retail.tags.keys().filter(|t| !GAPS.contains(&t.as_str())) {
        let (r, o) = (retail.of(tag), ours.of(tag));
        if report() {
            eprintln!("{tag} retail:");
            r.iter().for_each(|l| eprintln!("  {l:?}"));
            eprintln!("{tag} ours:");
            o.iter().for_each(|l| eprintln!("  {l:?}"));
        }
        compare_item(tag, &r, retail.start, &o, ours.start, &mut rows);
    }
    assert!(rows.is_empty(), "{}", rows.join("\n"));
}

/// One item's lines, both sides, timed from each side's `PROBE start`.
fn compare_item(tag: &str, r: &[Line], rs: i32, o: &[Line], os: i32, rows: &mut Vec<String>) {
    let drop = DROPS.contains(&tag);
    // A drop's landing frame and a landing's lift are random draws; the
    // lines either side of them still have to come in retail's order.
    let kind = |l: &Line| match l {
        Line::Gone { .. } => "gone",
        Line::Item(i) if i.pos.tr_type == TR_GRAVITY => "flying",
        Line::Item(_) => "still",
    };
    let rk: Vec<_> = r.iter().map(kind).collect();
    let ok: Vec<_> = o.iter().map(kind).collect();
    if rk != ok {
        rows.push(format!("{tag}: retail's lines go {rk:?}, ours {ok:?}"));
        return;
    }
    for (a, b) in r.iter().zip(o) {
        let (rt, ot) = (a.t() - rs, b.t() - os);
        let landing = matches!(a, Line::Item(i) if i.pos.tr_type == TR_STATIONARY && i.ground == 1022 && i.client_num != 254);
        let nudge = matches!(a, Line::Item(i) if i.pos.tr_type == TR_GRAVITY && i.pos.delta == Vec3::ZERO && i.ground == 0);
        if rt != ot && !(drop && (landing || nudge)) {
            rows.push(format!("{tag}: a line at {rt} on retail, {ot} on ours"));
        }
        let (Line::Item(a), Line::Item(b)) = (a, b) else {
            continue;
        };
        let fields = |i: &Item| {
            (
                i.index,
                i.client_num,
                i.eflags,
                i.ground,
                i.pos.tr_type,
                i.apos.tr_type,
                i.seq,
                i.events,
            )
        };
        if fields(a) != fields(b) {
            rows.push(format!(
                "{tag} at {rt}: retail {:?}, ours {:?}",
                fields(a),
                fields(b)
            ));
        }
        // The launch: retail's frame, along +x at 150, climbing and
        // spinning inside the draw's range.
        if a.pos.tr_type == TR_GRAVITY && a.ground == 0 && a.pos.delta != Vec3::ZERO {
            let d = b.pos.delta;
            if d.x != a.pos.delta.x || d.y != a.pos.delta.y || !(d.z > 50.0 && d.z <= 150.0) {
                rows.push(format!("{tag}: launched at {d}, retail at {}", a.pos.delta));
            }
            if b.pos.tr_time - os != a.pos.tr_time - rs {
                rows.push(format!("{tag}: the launch's trTime is not retail's"));
            }
        }
        if a.apos.tr_type == TR_LINEAR {
            let s = b.apos.delta;
            let inside = |v: f32, k: f32| v > -3.0 * k && v <= -k;
            if !(inside(s.x, 50.0) && inside(s.y, 40.0) && inside(s.z, 60.0))
                || b.apos.base != a.apos.base
            {
                rows.push(format!("{tag}: spins {:?}, retail {:?}", b.apos, a.apos));
            }
        }
        let start = |i: &Item| i.pos.base;
        let tol = if drop && tag != "health" {
            TAG_TOL
        } else {
            START_TOL
        };
        let near = if a.pos.tr_type == TR_STATIONARY {
            // Where it rests: the same spot up to the two draws of lift,
            // except a drop, whose climb moved the spot.
            drop || ((start(a) - start(b)).abs().max_element() <= LIFT_MAX + 0.01)
        } else {
            drop && a.pos.delta == Vec3::ZERO || (start(a) - start(b)).abs().max_element() <= tol
        };
        if !near {
            rows.push(format!(
                "{tag} at {rt}: retail at {}, ours at {}",
                start(a),
                start(b)
            ));
        }
        if a.apos.tr_type == TR_STATIONARY
            && !drop
            && angles_off(a.apos.base, b.apos.base) > ANGLE_TOL
        {
            rows.push(format!(
                "{tag} at {rt}: retail lies at {}, ours at {}",
                a.apos.base, b.apos.base
            ));
        }
    }
}
