//! Item pickup against the retail capture.
//!
//! Two fixtures from one retail run (their headers carry the recipe):
//! `items/mp_carentan-dm-pickup.txt` is the client's side, one `!cmd` per
//! usercmd with the absolute view it asked for and one `!trace` per snapshot
//! with the `!item` lines around it; `items/mp_carentan-dm-pickup-script.txt`
//! is the server's own log under `client-probes/probe_pickup.gsc`, which ours
//! runs as its gametype too. What they measured is in
//! `docs/research/cod11-items.md`, section 12.
//!
//! Ours runs the recipe's own cvars, so the gsc probe's two `setOrigin`s put
//! the client on the fg42s at the times they put retail's, and the replay is
//! one continuous run of the capture's cmds on retail's clock: our frame `t`
//! pairs with retail's snapshot at `t + 50`. The weapon, the cursor hint, the
//! ring and the origin are compared snapshot by snapshot; the inventory, the
//! pickup events, the commands and the items per phase. The gsc half is
//! compared per phase as a set: a frame's `Weapon:`, `trigger` and `touch`
//! lines reach the log in an order that is not the order they were raised
//! in (section 12.2).
//!
//! Needs `COD_DIR`; without the paks both tests return early.

mod common;

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::rc::Rc;
use std::time::{Duration, Instant};

use common::{holding, ClientEnd, Queues, CMD_MS, FRAME_MS};
use vcod_common::net::msg::{EntityState, UserCmd, NULL_USERCMD};
use vcod_common::net::protocol::PROTOCOL_V1;
use vcod_common::net::snapshot::Snapshot;
use vcod_common::net::trajectory::Trajectory;
use vcod_common::net::{NetClient, NetEvent};
use vcod_server::Server;

const MAP: &str = "mp_carentan";
const PROBE_PATH: &str = "maps/mp/gametypes/probe_pickup";
const PROBE_SRC: &str = "../gsc/tests/fixtures/semantics/client-probes/probe_pickup.gsc";
const CLIENT: &str = "tests/fixtures/items/mp_carentan-dm-pickup.txt";
const SCRIPT: &str = "tests/fixtures/items/mp_carentan-dm-pickup-script.txt";
const ET_ITEM: i32 = 3;
const ITEM_RADIUS: f32 = 256.0;
const EV_RAISE_WEAPON: i32 = 155;
/// `PM_UpdateViewAngles` caps the view here by pushing `delta_angles[0]`;
/// ours does not, so the replay caps the view it asks for
/// (`docs/research/cod11-gsc-object-model.md`, 23.1).
const PITCH_CLAMP_SHORT: i32 = 16000;
/// The origin a snapshot may differ by before it is a row.
const ORIGIN_EPS: f32 = 0.5;

/// Rows of either diff that are known divergences, each a substring of the
/// row it excuses and a ruling in the research doc's section 13.
const GAPS: &[&str] = &[
    // Every cmd's pmove runs before the deferred touch pass, so the swap's
    // disarm reaches the ring on the next tick's first cmd (13.2).
    "swap disarm one frame late",
];

fn report() -> bool {
    std::env::var("PICKUP_REPORT").is_ok_and(|v| v == "1")
}

fn read(path: &str) -> String {
    std::fs::read_to_string(path).unwrap_or_else(|e| panic!("read {path}: {e}"))
}

// Local copies of `crates/client/src/probe.rs`'s `escape_ctl` and
// `nonzero_pairs`/`pairs_str`: the client is a binary crate, out of reach.

fn escape_ctl(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c.is_control() {
                format!("\\x{:02x}", c as u32)
            } else {
                c.to_string()
            }
        })
        .collect()
}

fn pairs(a: &[i16]) -> String {
    let v: Vec<String> = a
        .iter()
        .enumerate()
        .filter(|(_, v)| **v != 0)
        .map(|(i, v)| format!("{i}:{v}"))
        .collect();
    if v.is_empty() {
        "-".to_string()
    } else {
        v.join(",")
    }
}

fn vec3(s: &str) -> [f32; 3] {
    let mut out = [0.0; 3];
    for (i, v) in s.split(',').take(3).enumerate() {
        out[i] = v.parse().unwrap_or_else(|_| panic!("a vector, got {s:?}"));
    }
    out
}

fn dist(a: [f32; 3], b: [f32; 3]) -> f32 {
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
}

fn kv(rest: &str) -> BTreeMap<&str, &str> {
    rest.split_whitespace()
        .filter_map(|t| t.split_once('='))
        .collect()
}

fn keeps_command(c: &str) -> bool {
    c.starts_with("a ") || c.starts_with("f GAME_")
}

// ------------------------------------------------------------ the samples

/// `(index, clientNum, groundEntityNum, x, y, z)`, the origin rounded.
type ItemKey = (i32, i32, i32, i32, i32, i32);

/// One snapshot, retail's off a `!trace` and ours off our client: the same
/// shape either side.
#[derive(Default, Debug, Clone)]
struct Sample {
    /// Retail's `serverTime`; ours carries the retail time it pairs with.
    t: i32,
    weapon: i32,
    /// `serverCursorHint:Val:String`.
    hint: String,
    /// The ring's new `(event, parm)` since the sample before.
    events: Vec<(i32, i32)>,
    /// `a ...` and `f "GAME_...` commands that arrived with the snapshot.
    commands: Vec<String>,
    origin: [f32; 3],
    /// Retail's view, which times the replay's; not compared.
    view: [f32; 3],
    weapons: String,
    slots: String,
    clip: String,
    ammo: String,
    health: i32,
    /// Items within 256 units of the origin.
    items: BTreeSet<ItemKey>,
}

/// The ring's new slots between two samples: `(prev..cur)`, the order
/// retail writes them (AGENTS.md, `eventSequence`).
fn drain(
    last: &mut Option<i32>,
    seq: i32,
    events: &[i32],
    parms: &[i32],
    out: &mut Vec<(i32, i32)>,
) {
    if let Some(prev) = *last {
        let mut s = prev;
        while s != seq {
            let k = (s & 3) as usize;
            out.push((events[k], parms[k]));
            s = (s + 1) & 0xff;
        }
    }
    *last = Some(seq);
}

fn item_key(index: i32, client: i32, ground: i32, at: [f32; 3]) -> ItemKey {
    (
        index,
        client,
        ground,
        at[0].round() as i32,
        at[1].round() as i32,
        at[2].round() as i32,
    )
}

// ------------------------------------------------------------ the fixture

struct Capture {
    /// `(name, first trace time, last trace time)` per phase past `wait`.
    phases: Vec<(String, i32, i32)>,
    /// Every cmd from `stand` on, as `(st, cmd)` with the absolute view.
    cmds: Vec<(i32, UserCmd)>,
    retail: Vec<Sample>,
}

impl Capture {
    fn phase_of(&self, t: i32) -> Option<&str> {
        self.phases
            .iter()
            .find(|(_, a, b)| (*a..=*b).contains(&t))
            .map(|(n, _, _)| n.as_str())
    }
}

fn parse(text: &str) -> Capture {
    let mut phase = String::new();
    let mut phases: Vec<(String, i32, i32)> = Vec::new();
    let mut cmds = Vec::new();
    let mut retail: Vec<Sample> = Vec::new();
    let mut last_seq = None;
    let mut last_trace_ms = None;
    let mut pending: Vec<String> = Vec::new();
    for line in text.lines() {
        if let Some(name) = line
            .strip_prefix("[phase ")
            .and_then(|l| l.strip_suffix(']'))
        {
            phase = name.to_string();
            continue;
        }
        if let Some(rest) = line.strip_prefix("!cmd ") {
            if phase == "wait" {
                continue;
            }
            let m = kv(rest);
            let i = |k: &str| m[k].parse::<i32>().unwrap();
            let view = vec3(m["view"]);
            let mut pitch = (view[0] as i32) & 0xffff;
            if pitch > 32767 {
                pitch -= 65536;
            }
            cmds.push((
                i("st"),
                UserCmd {
                    buttons: i("buttons") as u8,
                    wbuttons: i("wbuttons") as u8,
                    weapon: i("weapon") as u8,
                    up: i("up") as i8,
                    forward: i("forward") as i8,
                    right: i("right") as i8,
                    angles: [
                        pitch.clamp(-PITCH_CLAMP_SHORT, PITCH_CLAMP_SHORT),
                        view[1] as i32,
                        0,
                    ],
                    ..NULL_USERCMD
                },
            ));
        } else if let Some(rest) = line.strip_prefix("!server ") {
            let cmd = rest.split_once(' ').map_or("", |(_, c)| c);
            if keeps_command(cmd) {
                pending.push(cmd.to_string());
            }
        } else if let Some(rest) = line.strip_prefix("!trace ") {
            let m = kv(rest);
            let i = |k: &str| m[k].parse::<i32>().unwrap();
            let list =
                |k: &str| -> Vec<i32> { m[k].split(',').map(|v| v.parse().unwrap()).collect() };
            let mut s = Sample {
                t: i("serverTime"),
                weapon: i("weapon"),
                hint: m["hint"].to_string(),
                commands: std::mem::take(&mut pending),
                origin: vec3(m["origin"]),
                view: vec3(m["viewangles"]),
                weapons: m["weapons"].to_string(),
                slots: m["slots"].to_string(),
                clip: m["clip"].to_string(),
                ammo: m["ammo"].to_string(),
                health: i("health"),
                ..Sample::default()
            };
            drain(
                &mut last_seq,
                i("eventSequence"),
                &list("events"),
                &list("eventParms"),
                &mut s.events,
            );
            if phase != "wait" {
                match phases.last_mut() {
                    Some(p) if p.0 == phase => p.2 = s.t,
                    _ => phases.push((phase.clone(), s.t, s.t)),
                }
            }
            last_trace_ms = Some(i("ms"));
            retail.push(s);
        } else if let Some(rest) = line.strip_prefix("!item ") {
            let m = kv(rest);
            let s = retail.last_mut().expect("an !item after a !trace");
            if Some(m["ms"].parse::<i32>().unwrap()) != last_trace_ms {
                continue;
            }
            let pos: Vec<f32> = m["pos"].split(',').map(|v| v.parse().unwrap()).collect();
            let at = [pos[2], pos[3], pos[4]];
            if dist(at, s.origin) <= ITEM_RADIUS {
                let i = |k: &str| m[k].parse::<i32>().unwrap();
                s.items.insert(item_key(
                    i("index"),
                    i("clientNum"),
                    i("groundEntityNum"),
                    at,
                ));
            }
        }
    }
    retail.retain(|s| phases.iter().any(|(_, a, b)| (*a..=*b).contains(&s.t)));
    Capture {
        phases,
        cmds,
        retail,
    }
}

// --------------------------------------------------------------- our side

struct Rig {
    sv: Server,
    q: Rc<RefCell<Queues>>,
    cl: NetClient<ClientEnd>,
    now: Instant,
    last_seq: Option<i32>,
    log_seen: usize,
}

/// The probe as our gametype under the recipe's two cvars, one client
/// through the stock menus, and a second to let the first teleport settle.
fn rig() -> Option<Rig> {
    let fs = vcod_common::testing::game_fs()?;
    let bsp_path = fs.resolve_map(MAP).expect("map in the mounted paks");
    let bsp = vcod_common::bsp::parse(&fs.read(&bsp_path).unwrap()).unwrap();
    let fs = Rc::new(fs);
    let mut now = Instant::now();
    let mut sv = Server::new(common::cfg(MAP, "probe_pickup"), now);
    sv.overlay_script(PROBE_PATH, &read(PROBE_SRC));
    sv.set_cvar("probe_teleport", "1");
    sv.set_cvar("scr_allow_fg42", "1");
    sv.load_world(vcod_server::world::World::from_bsp(&bsp, Some(&fs)));
    sv.load_scripts(fs).expect("load the scripts");
    let q = Rc::new(RefCell::new(Queues::default()));
    let (cl, join) = common::join(&mut sv, &q, &mut now, "allies", "m1carbine_mp");
    assert!(join.findings().is_empty(), "{}", join.summary());
    let mut rig = Rig {
        sv,
        q,
        cl,
        now,
        last_seq: None,
        log_seen: 0,
    };
    for _ in 0..20 {
        let h = holding(&rig.cl);
        rig.frame([h, h], 0);
    }
    assert_eq!(rig.sv.script_aborts(), Vec::<String>::new());
    Some(rig)
}

impl Rig {
    /// One server frame with its two cmds on the 25 ms grid, and what the
    /// client saw of it, tagged `t`.
    fn frame(&mut self, cmds: [UserCmd; 2], t: i32) -> Sample {
        for cmd in cmds {
            self.now += Duration::from_millis(CMD_MS as u64);
            self.cl.pump_at(self.now);
            self.cl.send_frame(&cmd);
        }
        let mut out = Sample {
            t,
            ..Sample::default()
        };
        for e in common::step(&mut self.sv, &self.q, &mut self.cl, self.now) {
            if let NetEvent::ServerCommand(tokens) = e {
                let c = escape_ctl(&tokens.join(" "));
                if keeps_command(&c) {
                    out.commands.push(c);
                }
            }
        }
        let Some(s) = self.cl.snapshots().newest() else {
            return out;
        };
        let p = &PROTOCOL_V1;
        let f = |n: &str| s.ps.field_i32(p, n);
        let events: Vec<i32> = (0..4).map(|i| f(&format!("events[{i}]"))).collect();
        let parms: Vec<i32> = (0..4).map(|i| f(&format!("eventParms[{i}]"))).collect();
        drain(
            &mut self.last_seq,
            f("eventSequence"),
            &events,
            &parms,
            &mut out.events,
        );
        out.weapon = f("weapon");
        out.hint = format!(
            "{}:{}:{}",
            f("serverCursorHint"),
            f("serverCursorHintVal"),
            f("serverCursorHintString")
        );
        out.origin = s.ps.origin(p);
        out.weapons = format!("{},{}", f("weapons[0]"), f("weapons[1]"));
        out.slots = format!("{},{}", f("weaponslots[0]"), f("weaponslots[4]"));
        out.clip = pairs(&s.ps.arrays.ammoclip);
        out.ammo = pairs(&s.ps.arrays.ammo);
        out.health = s.ps.health();
        out.items = items_near(s, out.origin);
        out
    }

    /// Our log lines since the last call.
    fn new_log(&mut self) -> Vec<String> {
        let log = self.sv.script_log();
        let out = log[self.log_seen.min(log.len())..].to_vec();
        self.log_seen = log.len();
        out
    }
}

fn items_near(s: &Snapshot, me: [f32; 3]) -> BTreeSet<ItemKey> {
    let p = &PROTOCOL_V1;
    s.entities
        .values()
        .filter(|e: &&EntityState| e.field_i32(p, "eType") == ET_ITEM)
        .filter_map(|e| {
            let at: [f32; 3] = Trajectory::read(e, p, "pos").evaluate(s.server_time).into();
            (dist(at, me) <= ITEM_RADIUS).then(|| {
                item_key(
                    e.field_i32(p, "index"),
                    e.field_i32(p, "clientNum"),
                    e.field_i32(p, "groundEntityNum"),
                    at,
                )
            })
        })
        .collect()
}

/// The view each retail snapshot reports, as the capture cmd's exact wire
/// words that asked for it. A cmd's `st` is the client's clock, not the
/// server frame that ran it: the aim's first cmd, stamped 27983, is on
/// retail's 28050 snapshot, not its 28000 one. So the view goes by the
/// snapshot; the buttons and the weapon byte, which no such case moves, go
/// by `st`.
fn retail_views(cap: &Capture) -> BTreeMap<i32, [i32; 3]> {
    let deg = |s: i32| s as f32 * 360.0 / 65536.0;
    let near = |a: f32, b: f32| ((a - b + 180.0).rem_euclid(360.0) - 180.0).abs() < 0.1;
    let asked: BTreeSet<[i32; 3]> = cap.cmds.iter().map(|(_, c)| c.angles).collect();
    cap.retail
        .iter()
        .map(|s| {
            let words = asked
                .iter()
                .find(|a| near(deg(a[0]), s.view[0]) && near(deg(a[1]), s.view[1]))
                .copied()
                .unwrap_or_else(|| {
                    panic!("no cmd asked for retail's view {:?} at {}", s.view, s.t)
                });
            (s.t, words)
        })
        .collect()
}

/// Replays every cmd from `stand` on, on retail's clock: frame `t` takes the
/// cmds stamped in `(t, t + 50]`, each held until the next, and the view
/// retail's snapshot at `t + 50` reports. A slot takes the last cmd stamped
/// inside it with the buttons of every one ORed in, so a use tap two retail
/// cmds long survives the coarser grid. Returns one sample per frame, tagged
/// with the retail snapshot it pairs with, and every log line with that same
/// time.
fn replay(rig: &mut Rig, cap: &Capture) -> (Vec<Sample>, Vec<(i32, String)>) {
    rig.new_log();
    let cmds = &cap.cmds;
    let first = cmds.first().expect("cmds past wait").0;
    let end = cap.phases.last().unwrap().2;
    let mut t = first - first.rem_euclid(FRAME_MS as i32);
    let mut current = cmds[0].1;
    let mut i = 0;
    let views = retail_views(cap);
    let (mut samples, mut log) = (Vec::new(), Vec::new());
    while t + FRAME_MS as i32 <= end {
        let mut pair = [current; 2];
        for (half, slot) in pair.iter_mut().enumerate() {
            let slot_end = t + (half as i32 + 1) * CMD_MS as i32;
            let mut buttons = 0u8;
            while i < cmds.len() && cmds[i].0 <= slot_end {
                current = cmds[i].1;
                buttons |= current.buttons;
                i += 1;
            }
            *slot = UserCmd {
                buttons: current.buttons | buttons,
                ..current
            };
        }
        let at = t + FRAME_MS as i32;
        if let Some(view) = views.get(&at) {
            for c in &mut pair {
                c.angles = *view;
            }
        }
        samples.push(rig.frame(pair, at));
        log.extend(rig.new_log().into_iter().map(|l| (at, l)));
        t += FRAME_MS as i32;
    }
    (samples, log)
}

// ------------------------------------------------------------ the diff

/// What a phase drained and what it ends with, compared per phase: the
/// pickup events and the commands in order, the rest at its last sample.
#[derive(Default, Debug)]
struct Summary {
    events: Vec<(i32, i32)>,
    commands: Vec<String>,
    weapons: String,
    slots: String,
    clip: String,
    ammo: String,
    health: i32,
    items: BTreeSet<ItemKey>,
}

fn summarize<'a>(samples: impl Iterator<Item = &'a Sample>) -> Summary {
    let mut sum = Summary::default();
    for s in samples {
        sum.events.extend(
            s.events
                .iter()
                .filter(|(e, _)| (146..=148).contains(e))
                .copied(),
        );
        sum.commands.extend(s.commands.iter().cloned());
        sum.weapons = s.weapons.clone();
        sum.slots = s.slots.clone();
        sum.clip = s.clip.clone();
        sum.ammo = s.ammo.clone();
        sum.health = s.health;
        sum.items = s.items.clone();
    }
    sum
}

fn diff_phase(name: &str, retail: &Summary, ours: &Summary, rows: &mut Vec<String>) {
    let mut row = |field: &str, r: String, o: String| {
        if r != o {
            rows.push(format!("[{name}] {field}: retail {r} ours {o}"));
        }
    };
    row(
        "events",
        format!("{:?}", retail.events),
        format!("{:?}", ours.events),
    );
    row(
        "commands",
        format!("{:?}", retail.commands),
        format!("{:?}", ours.commands),
    );
    row("weapons", retail.weapons.clone(), ours.weapons.clone());
    row("slots", retail.slots.clone(), ours.slots.clone());
    row("clip", retail.clip.clone(), ours.clip.clone());
    row("ammo", retail.ammo.clone(), ours.ammo.clone());
    row("health", retail.health.to_string(), ours.health.to_string());
    let only = |a: &BTreeSet<ItemKey>, b: &BTreeSet<ItemKey>| -> Vec<String> {
        a.iter()
            .filter(|x| {
                !b.iter().any(|y| {
                    (x.0, x.1, x.2) == (y.0, y.1, y.2)
                        && (x.3 - y.3).abs() <= 1
                        && (x.4 - y.4).abs() <= 1
                        && (x.5 - y.5).abs() <= 1
                })
            })
            .map(|x| format!("{x:?}"))
            .collect()
    };
    row(
        "items retail only",
        format!("{:?}", only(&retail.items, &ours.items)),
        "[]".to_string(),
    );
    row(
        "items ours only",
        "[]".to_string(),
        format!("{:?}", only(&ours.items, &retail.items)),
    );
}

/// The ruled swap lag, taken out before the snapshot diff. Retail's swap
/// snapshot `T` reads `weapon` 0 with a 155 beside the 146, and `T + 50`
/// the new weapon. Ours runs that disarm on the next tick's first cmd, whose
/// later cmd already raises the new weapon, so ours reads the old weapon
/// with the 146 alone at `T` and the new weapon with the 155 at `T + 50`.
/// Exactly that shape is shifted back onto retail's and stands as one
/// `swap disarm one frame late` row; any other leaves ours as it was and
/// fails on the fields themselves.
fn unlag_swaps(retail: &[Sample], ours: &mut BTreeMap<i32, Sample>, rows: &mut Vec<String>) {
    let raise = |s: &Sample| s.events.iter().any(|(e, _)| *e == EV_RAISE_WEAPON);
    let step = FRAME_MS as i32;
    for k in 1..retail.len().saturating_sub(1) {
        let (before, r, after) = (&retail[k - 1], &retail[k], &retail[k + 1]);
        let swap = r.weapon == 0 && raise(r) && r.events.iter().any(|(e, _)| *e == 146);
        if !swap || after.t != r.t + step || raise(after) {
            continue;
        }
        let (Some(o), Some(next)) = (ours.get(&r.t), ours.get(&after.t)) else {
            continue;
        };
        let late =
            o.weapon == before.weapon && !raise(o) && next.weapon == after.weapon && raise(next);
        if !late {
            continue;
        }
        rows.push(format!(
            "[t={}] swap disarm one frame late: ours reads weapon {} there, the 155 a frame on",
            r.t, o.weapon
        ));
        let mut next = next.clone();
        let moved: Vec<(i32, i32)> = next
            .events
            .iter()
            .filter(|(e, _)| *e == EV_RAISE_WEAPON)
            .copied()
            .collect();
        next.events.retain(|(e, _)| *e != EV_RAISE_WEAPON);
        let o = ours.get_mut(&r.t).unwrap();
        o.weapon = 0;
        o.events.extend(moved);
        ours.insert(after.t, next);
    }
}

/// The ring's item and weapon events, `EV_ITEM_PICKUP` (146) up, with the
/// parm on the three pickup events only. The movement events below 146 and
/// the weapon events' parms are the motion and combat gates' business.
fn ring(events: &[(i32, i32)]) -> String {
    let kept: Vec<String> = events
        .iter()
        .filter(|(e, _)| *e >= 146)
        .map(|&(e, p)| match e {
            146..=148 => format!("{e}:{p}"),
            _ => e.to_string(),
        })
        .collect();
    format!("[{}]", kept.join(","))
}

/// Weapon, hint, the ring and the origin, snapshot by snapshot.
fn diff_snapshots(cap: &Capture, ours: &[Sample], rows: &mut Vec<String>) {
    let mut ours: BTreeMap<i32, Sample> = ours.iter().map(|s| (s.t, s.clone())).collect();
    unlag_swaps(&cap.retail, &mut ours, rows);
    for r in &cap.retail {
        let phase = cap.phase_of(r.t).unwrap_or("?");
        let Some(o) = ours.get(&r.t) else {
            rows.push(format!("[{phase}] t={} no sample of ours", r.t));
            continue;
        };
        let mut row = |field: &str, a: String, b: String| {
            if a != b {
                rows.push(format!("[{phase}] t={} {field}: retail {a} ours {b}", r.t));
            }
        };
        row("weapon", r.weapon.to_string(), o.weapon.to_string());
        row("hint", r.hint.clone(), o.hint.clone());
        row("ring", ring(&r.events), ring(&o.events));
        if dist(r.origin, o.origin) > ORIGIN_EPS {
            row(
                "origin",
                format!("{:?}", r.origin),
                format!("{:?}", o.origin),
            );
        }
    }
}

fn finish(rows: Vec<String>) {
    let (gapped, real): (Vec<String>, Vec<String>) = rows
        .into_iter()
        .partition(|r| GAPS.iter().any(|g| r.contains(g)));
    if report() {
        for r in &gapped {
            println!("gap: {r}");
        }
        for r in &real {
            println!("{r}");
        }
    }
    assert!(real.is_empty(), "{}", real.join("\n"));
}

#[test]
fn the_pickups_match_retail_on_mp_carentan() {
    let Some(mut rig) = rig() else { return };
    let text = read(CLIENT);
    assert!(
        !text.contains("# BROKEN"),
        "the committed capture is marked broken"
    );
    let cap = parse(&text);
    let (ours, _) = replay(&mut rig, &cap);
    let mut rows = Vec::new();
    for (name, a, b) in &cap.phases {
        let within = |s: &&Sample| (*a..=*b).contains(&s.t);
        let retail = summarize(cap.retail.iter().filter(within));
        let mine = summarize(ours.iter().filter(within));
        if report() {
            println!("[{name}] {a}..{b} retail {retail:?}");
            println!("[{name}] {a}..{b} ours   {mine:?}");
        }
        diff_phase(name, &retail, &mine, &mut rows);
    }
    diff_snapshots(&cap, &ours, &mut rows);
    finish(rows);
}

// ------------------------------------------------------------ the gsc half

/// Item number to name. The census (every `PROBE item` line sharing the
/// first one's `getTime`) is the map's placed items, whose numbers ours and
/// retail share (`entities_ab.rs` pins them), so they keep theirs; a drop is
/// numbered off each side's own free list, so it is named by classname.
fn names_of<'a>(lines: impl Iterator<Item = &'a str>) -> BTreeMap<String, String> {
    let items: Vec<Vec<&str>> = lines
        .map(|l| l.split_whitespace().collect::<Vec<&str>>())
        .filter(|t| t.len() > 4 && t[..2] == ["PROBE", "item"])
        .collect();
    let census = items.first().map(|t| t[4]);
    items
        .iter()
        .map(|t| {
            let name = if Some(t[4]) == census {
                format!("{}#{}", t[3], t[2])
            } else {
                format!("{}(drop)", t[3])
            };
            (t[2].to_string(), name)
        })
        .collect()
}

/// A log line's kind and the parts the diff keeps, with every item number
/// replaced by its name, and the `getTime` it carries.
fn gsc_key(line: &str, names: &BTreeMap<String, String>) -> Option<(Option<i32>, String)> {
    let t: Vec<&str> = line.split_whitespace().collect();
    let name = |n: &str| names.get(n).cloned().unwrap_or_else(|| n.to_string());
    let time = |i: usize| t.get(i).and_then(|v| v.parse().ok());
    match (t.first().copied()?, t.get(1).copied()?) {
        ("Weapon:" | "Item:", _) => Some((None, t.join(" "))),
        ("PROBE", "trigger") => {
            let swapped = t[5]
                .split_once(':')
                .map_or(t[5].to_string(), |(_, c)| format!("{c}(drop)"));
            Some((
                time(3),
                format!("trigger {} {} {swapped}", name(t[2]), t[4]),
            ))
        }
        ("PROBE", "touch") => Some((time(3), format!("touch {} {}", name(t[2]), t[4]))),
        ("PROBE", "ptouch") => Some((time(3), format!("ptouch {} {}", t[2], name(t[4])))),
        _ => None,
    }
}

/// Each phase's keys as a set. A line with no time of its own (`Weapon:`,
/// `Item:`) takes the next timed line's, which is its own frame's.
fn gsc_phases(
    cap: &Capture,
    lines: &[(Option<i32>, String)],
) -> BTreeMap<String, BTreeSet<String>> {
    let mut out: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    let mut untimed: Vec<&str> = Vec::new();
    for (time, key) in lines {
        let Some(t) = time else {
            untimed.push(key);
            continue;
        };
        let Some(phase) = cap.phase_of(*t) else {
            untimed.clear();
            continue;
        };
        let set = out.entry(phase.to_string()).or_default();
        set.extend(untimed.drain(..).map(str::to_string));
        set.insert(key.clone());
    }
    out
}

/// The census lines themselves, as `<num> <classname>`.
fn census<'a>(lines: impl Iterator<Item = &'a str>) -> Vec<String> {
    let items: Vec<Vec<&str>> = lines
        .map(|l| l.split_whitespace().collect::<Vec<&str>>())
        .filter(|t| t.len() > 4 && t[..2] == ["PROBE", "item"])
        .collect();
    let first = items.first().map(|t| t[4]);
    items
        .iter()
        .filter(|t| Some(t[4]) == first)
        .map(|t| format!("{} {}", t[2], t[3]))
        .collect()
}

#[test]
fn the_item_notifies_match_retail_on_mp_carentan() {
    let Some(mut rig) = rig() else { return };
    let cap = parse(&read(CLIENT));
    let retail_text = read(SCRIPT);
    let retail: Vec<&str> = retail_text
        .lines()
        .filter(|l| !l.starts_with('#'))
        .collect();
    let before: Vec<String> = rig.sv.script_log().to_vec();
    let (_, log) = replay(&mut rig, &cap);

    let mut rows = Vec::new();
    let (rc, oc) = (
        census(retail.iter().copied()),
        census(before.iter().map(String::as_str)),
    );
    if rc != oc {
        rows.push(format!("census: retail {rc:?} ours {oc:?}"));
    }
    let rn = names_of(retail.iter().copied());
    let on = names_of(
        before
            .iter()
            .map(String::as_str)
            .chain(log.iter().map(|(_, l)| l.as_str())),
    );
    let retail_keys: Vec<(Option<i32>, String)> =
        retail.iter().filter_map(|l| gsc_key(l, &rn)).collect();
    // Ours on retail's clock: a line logged in the frame paired with retail's
    // snapshot `t` is stamped `t`, whatever our own `getTime` read.
    let our_keys: Vec<(Option<i32>, String)> = log
        .iter()
        .filter_map(|(t, l)| gsc_key(l, &on).map(|(_, k)| (Some(*t), k)))
        .collect();
    let (r, o) = (gsc_phases(&cap, &retail_keys), gsc_phases(&cap, &our_keys));
    for (name, _, _) in &cap.phases {
        let empty = BTreeSet::new();
        let (a, b) = (r.get(name).unwrap_or(&empty), o.get(name).unwrap_or(&empty));
        if report() {
            println!("[{name}] retail {a:?}");
            println!("[{name}] ours   {b:?}");
        }
        for k in a.difference(b) {
            rows.push(format!("[{name}] gsc retail only: {k}"));
        }
        for k in b.difference(a) {
            rows.push(format!("[{name}] gsc ours only: {k}"));
        }
    }
    finish(rows);
}
