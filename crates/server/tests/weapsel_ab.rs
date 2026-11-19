//! The weapon machine under the client's weapon selection, against the
//! retail `--probe-weapsel` capture (its header carries the recipe):
//! `client-probes/probe_weapsel.gsc` switches by script, empties the rifle
//! and hands over a thompson, and the client answers with its weapon byte,
//! dry pulls and two `weapalt`s. What it measured is in
//! `docs/research/cod11-combat.md`, 9.8 and 9.9.
//!
//! Ours runs the same gsc probe as its gametype and replays the capture's
//! cmds from the first `a` on, verbatim with retail's stamps, retail's clock
//! shifted onto ours at that command: everything after it is the script's
//! own fixed waits and the client's answers. Each retail snapshot is a frame
//! of ours, run after the cmds up to that snapshot's `commandTime`. Each
//! side is reduced to its transitions (`weapon`, `weaponstate`, the
//! `weapAnim` index, the slot words, and the ring's events with their
//! parms) and the two lists are matched in order on the same snapshot. A
//! state's entry also carries its `weaponTime`, which must match too.
//!
//! Needs `COD_DIR`; without the paks it returns early.

use crate::common;

use std::cell::RefCell;
use std::rc::Rc;
use std::time::{Duration, Instant};

use common::{ADDR, ClientEnd, FRAME_MS, Queues, holding};
use vcod_common::net::msg::{NULL_USERCMD, UserCmd};
use vcod_common::net::protocol::PROTOCOL_V1;
use vcod_common::net::{NetClient, NetEvent};
use vcod_server::Server;

const MAP: &str = "mp_carentan";
const PROBE_SRC: &str = "../gsc/tests/fixtures/semantics/client-probes/probe_weapsel.gsc";
const FIXTURE: &str = "tests/fixtures/playerstate/mp_carentan-dm-weapsel.txt";
/// The same recipe with the probe's last two commands: the thompson back,
/// then the pistol asked for inside its raise (combat doc 1.8, "A switch
/// inside a raise").
const RAISE_FIXTURE: &str = "tests/fixtures/playerstate/mp_carentan-dm-weapsel-raise.txt";

/// Rows that are known divergences, each a substring of the row it excuses
/// and a ruling in the research doc.
const GAPS: &[&str] = &[];

fn report() -> bool {
    std::env::var("WEAPSEL_REPORT").is_ok_and(|v| v == "1")
}

fn kv(rest: &str) -> std::collections::BTreeMap<&str, &str> {
    rest.split_whitespace()
        .filter_map(|t| t.split_once('='))
        .collect()
}

/// One snapshot, either side, in retail's time.
#[derive(Default, Clone, Debug)]
struct Sample {
    t: i32,
    weapon: i32,
    state: i32,
    time: i32,
    anim: i32,
    slots: String,
    /// The ring's new `(event, parm)` since the sample before.
    events: Vec<(i32, i32)>,
}

/// The ring's new slots between two samples: `(prev..cur)`.
fn drain(last: &mut Option<i32>, seq: i32, events: &[i32], parms: &[i32]) -> Vec<(i32, i32)> {
    let mut out = Vec::new();
    if let Some(prev) = *last {
        let mut s = prev;
        while s != seq {
            let k = (s & 3) as usize;
            out.push((events[k], parms[k]));
            s = (s + 1) & 0xff;
        }
    }
    *last = Some(seq);
    out
}

struct Capture {
    /// Every cmd past the first `a`, as `(ms, cmd)` with retail's stamp.
    cmds: Vec<(i32, UserCmd)>,
    /// The snapshot time the first `a` rode.
    anchor_t: i32,
    /// Each snapshot from there on as `(ms it arrived at, serverTime,
    /// commandTime)`.
    frames: Vec<(i32, i32, i32)>,
    retail: Vec<Sample>,
}

fn parse(text: &str) -> Capture {
    let mut cmds = Vec::new();
    let mut anchor_ms = None;
    let mut anchor_t = None;
    let mut retail = Vec::new();
    let mut frames = Vec::new();
    let mut seq = None;
    for line in text.lines() {
        let Some((kind, rest)) = line.split_once(' ') else {
            continue;
        };
        let m = kv(rest);
        let i = |k: &str| m[k].parse::<i32>().unwrap();
        match kind {
            "!server"
                if anchor_ms.is_none()
                    && rest
                        .split_once(' ')
                        .is_some_and(|(_, c)| c.starts_with("a ")) =>
            {
                anchor_ms = Some(i("ms"))
            }
            "!cmd" if anchor_ms.is_some() => cmds.push((
                i("ms"),
                UserCmd {
                    server_time: i("st"),
                    buttons: i("buttons") as u8,
                    wbuttons: i("wbuttons") as u8,
                    weapon: i("weapon") as u8,
                    up: i("up") as i8,
                    forward: i("forward") as i8,
                    right: i("right") as i8,
                    ..NULL_USERCMD
                },
            )),
            "!trace" => {
                let list =
                    |k: &str| -> Vec<i32> { m[k].split(',').map(|v| v.parse().unwrap()).collect() };
                let events = drain(
                    &mut seq,
                    i("eventSequence"),
                    &list("events"),
                    &list("eventParms"),
                );
                let t = i("serverTime");
                if anchor_ms.is_some() && anchor_t.is_none() {
                    anchor_t = Some(t);
                }
                if anchor_t.is_some() {
                    frames.push((i("ms"), t, i("commandTime")));
                    retail.push(Sample {
                        t,
                        weapon: i("weapon"),
                        state: i("weaponstate"),
                        time: i("weaponTime"),
                        anim: i("weapAnim"),
                        slots: m["slots"].to_string(),
                        events,
                    });
                }
            }
            _ => {}
        }
    }
    assert!(anchor_ms.is_some(), "an `a` command in the capture");
    Capture {
        cmds,
        anchor_t: anchor_t.expect("a snapshot after the `a`"),
        frames,
        retail,
    }
}

struct Rig {
    sv: Server,
    q: Rc<RefCell<Queues>>,
    cl: NetClient<ClientEnd>,
    now: Instant,
    seq: Option<i32>,
}

fn rig() -> Option<Rig> {
    let fs = vcod_common::testing::game_fs_11()?;
    let fs = Rc::new(fs);
    let src = std::fs::read_to_string(PROBE_SRC).expect("the gsc probe");
    let (mut sv, mut now) = common::Boot {
        script: Some(&src),
        ..common::Boot::new(MAP, "probe_weapsel")
    }
    .start(fs);
    let q = Rc::new(RefCell::new(Queues::default()));
    let (cl, join) = common::join(&mut sv, &q, &mut now, "allies", "m1carbine_mp");
    assert!(join.findings().is_empty(), "{}", join.summary());
    Some(Rig {
        sv,
        q,
        cl,
        now,
        seq: None,
    })
}

impl Rig {
    /// Retail's cmds verbatim, stamps and all, handled on arrival as
    /// retail's `SV_PacketEvent` runs them between frames.
    fn send(&mut self, cmds: &[UserCmd], at: Instant) {
        self.now = at;
        self.cl.pump_at(at);
        self.cl.send_cmds(cmds);
        let pending: Vec<Vec<u8>> = self.q.borrow_mut().to_server.drain(..).collect();
        for p in pending {
            self.sv.handle_packet(ADDR, &p, at);
        }
    }

    /// One server frame at `at`, the snapshot it sent and whether an `a`
    /// command came with it.
    fn tick(&mut self, at: Instant) -> (Option<Sample>, bool) {
        self.now = at;
        self.sv.tick(at);
        for (_, p) in self.sv.take_outgoing() {
            self.q.borrow_mut().to_client.push_back(p);
        }
        let mut asked = false;
        for e in self.cl.pump_at(at) {
            if let NetEvent::ServerCommand(tokens) = e {
                asked |= tokens.first().is_some_and(|t| t == "a");
            }
        }
        let Some(s) = self.cl.snapshots().newest() else {
            return (None, asked);
        };
        let p = &PROTOCOL_V1;
        let f = |n: &str| s.ps.field_i32(p, n);
        let events: Vec<i32> = (0..4).map(|i| f(&format!("events[{i}]"))).collect();
        let parms: Vec<i32> = (0..4).map(|i| f(&format!("eventParms[{i}]"))).collect();
        let events = drain(&mut self.seq, f("eventSequence"), &events, &parms);
        let sample = Sample {
            t: s.server_time.raw(),
            weapon: f("weapon"),
            state: f("weaponstate"),
            time: f("weaponTime"),
            anim: f("weapAnim"),
            slots: format!("{},{}", f("weaponslots[0]"), f("weaponslots[4]")),
            events,
        };
        (Some(sample), asked)
    }
}

/// Idles until our first `a`, then replays the capture on retail's clock
/// moved onto ours there: each retail snapshot is one frame of ours, run
/// after every cmd up to the `commandTime` retail's snapshot of that frame
/// reports, each cmd with retail's own stamp moved by the same shift.
/// Returns our samples in retail's time, the one that carried the `a`
/// first.
fn replay(rig: &mut Rig, cap: &Capture) -> Vec<Sample> {
    let frame = Duration::from_millis(FRAME_MS as u64);
    let mut first = None;
    for _ in 0..1200 {
        let h = holding(&rig.cl);
        let at = rig.now + frame;
        rig.cl.pump_at(at - frame / 2);
        rig.cl.send_frame(&h);
        let pending: Vec<Vec<u8>> = rig.q.borrow_mut().to_server.drain(..).collect();
        for p in pending {
            rig.sv.handle_packet(ADDR, &p, at - frame / 2);
        }
        if let (Some(s), true) = rig.tick(at) {
            first = Some(s);
            break;
        }
    }
    let mut first = first.expect("our probe never sent `a`");
    let shift = cap.anchor_t - first.t;
    first.t += shift;
    let base = rig.now;
    let mut out = vec![first];
    let mut i = 0;
    for &(_, t, ct) in &cap.frames[1..] {
        let at = base + Duration::from_millis((t - cap.anchor_t) as u64);
        let mut batch = Vec::new();
        while i < cap.cmds.len() && cap.cmds[i].1.server_time <= ct {
            let mut c = cap.cmds[i].1;
            c.server_time -= shift;
            batch.push(c);
            i += 1;
        }
        if !batch.is_empty() {
            rig.send(&batch, at - Duration::from_millis(1));
        }
        if let (Some(mut s), _) = rig.tick(at) {
            s.t += shift;
            out.push(s);
        }
    }
    out
}

/// A side's transitions: `(t, what)`, plus the entry `weaponTime` of each
/// new `weaponstate`.
fn transitions(samples: &[Sample]) -> Vec<(i32, String, Option<i32>)> {
    let mut out = Vec::new();
    let mut last: Option<&Sample> = None;
    for s in samples {
        for (e, p) in &s.events {
            out.push((s.t, format!("event {e}:{p}"), None));
        }
        if let Some(l) = last {
            if l.weapon != s.weapon {
                out.push((s.t, format!("weapon {}", s.weapon), None));
            }
            if l.state != s.state {
                out.push((s.t, format!("weaponstate {}", s.state), Some(s.time)));
            }
            if l.anim & !0x200 != s.anim & !0x200 {
                out.push((s.t, format!("weapAnim {}", s.anim & !0x200), None));
            }
            if l.slots != s.slots {
                out.push((s.t, format!("slots {}", s.slots), None));
            }
        }
        last = Some(s);
    }
    out
}

fn diff(retail: &[Sample], ours: &[Sample]) -> Vec<String> {
    let end = retail
        .last()
        .map_or(0, |s| s.t)
        .min(ours.last().map_or(0, |s| s.t));
    let r = transitions(retail);
    let o = transitions(ours);
    let mut used = vec![false; o.len()];
    let mut rows = Vec::new();
    for (t, what, time) in r.iter().filter(|x| x.0 <= end) {
        let hit = o
            .iter()
            .enumerate()
            .find(|(k, x)| !used[*k] && x.1 == *what && x.0 == *t);
        match hit {
            Some((k, x)) => {
                used[k] = true;
                if let (Some(a), Some(b)) = (time, x.2)
                    && *a != b
                {
                    rows.push(format!("t={t} {what}: weaponTime retail {a} ours {b}"));
                }
            }
            None => rows.push(format!("t={t} {what}: retail only")),
        }
    }
    for (k, (t, what, _)) in o.iter().enumerate() {
        if !used[k] && *t <= end && *t > retail[0].t {
            rows.push(format!("t={t} {what}: ours only"));
        }
    }
    rows
}

#[test]
fn the_weapon_selection_matches_retail_on_mp_carentan() {
    for fixture in [FIXTURE, RAISE_FIXTURE] {
        selection_matches(fixture);
    }
}

fn selection_matches(fixture: &str) {
    let Some(mut rig) = rig() else { return };
    let text = std::fs::read_to_string(fixture).expect("the capture");
    let cap = parse(&text);
    let ours = replay(&mut rig, &cap);
    assert_eq!(rig.sv.script_aborts(), Vec::<String>::new());
    if report() {
        for (side, s) in [("retail", &cap.retail), ("ours", &ours)] {
            for (t, what, time) in transitions(s) {
                println!("{side} t={t} {what} {time:?}");
            }
        }
    }
    let rows = diff(&cap.retail, &ours);
    let (gapped, real): (Vec<String>, Vec<String>) = rows
        .into_iter()
        .partition(|r| GAPS.iter().any(|g| r.contains(g)));
    if report() {
        for r in &gapped {
            println!("gap: {r}");
        }
    }
    assert!(real.is_empty(), "{fixture}:\n{}", real.join("\n"));
}

/// The dry pull, which the transition diff only sees as its event and its
/// state: the first sample past `EV_NOAMMO` reads `weaponstate` 3 for
/// `fireTime` plus 500 ms with the idle anim unwritten, on both sides
/// (combat doc, 1.5 step 2).
#[test]
fn the_dry_pull_holds_the_firing_state() {
    let Some(mut rig) = rig() else { return };
    let text = std::fs::read_to_string(FIXTURE).expect("the capture");
    let cap = parse(&text);
    let ours = replay(&mut rig, &cap);
    let first_dry = |s: &[Sample]| {
        s.iter()
            .find(|x| x.events.iter().any(|e| e.0 == 149))
            .cloned()
            .expect("a dry pull")
    };
    let (r, o) = (first_dry(&cap.retail), first_dry(&ours));
    assert_eq!((r.state, r.anim), (3, 0), "retail's own reading");
    assert_eq!((o.state, o.anim), (r.state, r.anim));
    assert!(
        o.time == r.time,
        "weaponTime retail {} ours {}",
        r.time,
        o.time
    );
}
