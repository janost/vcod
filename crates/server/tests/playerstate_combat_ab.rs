//! The weapon channel under each combat input, diffed per snapshot against
//! the retail combat captures. A shot is a transient the event ring
//! overwrites within four slots, so both sides carry a `!trace` line per
//! snapshot and the comparison is over those, never over a settled sample.
//!
//! Needs `COD_DIR`; without the paks it returns early.

mod common;

use common::Queues;
use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::rc::Rc;
use std::time::{Duration, Instant};
use vcod_common::net::msg::{UserCmd, NULL_USERCMD};
use vcod_common::net::protocol::PROTOCOL_V1;

fn cfg(map: &str) -> vcod_server::ServerConfig {
    vcod_server::ServerConfig {
        map: map.into(),
        hostname: "vcod test".into(),
        max_clients: 8,
        gametype: "dm".into(),
        test_entities: 0,
        trace: false,
    }
}

const EV_FIRE_WEAPON: i32 = 159;
const EV_FIRE_WEAPON_LASTSHOT: i32 = 161;

/// One server frame, and one usercmd, in ms. A client sends cmds faster than
/// the server ticks; the replay sends two per frame because a tap one frame
/// long can land on the very frame a semi-automatic weapon's `weaponTime`
/// expires, where the latch swallows it (combat doc, section 1.4) and the
/// capture's own tap, 32 ms out of every 183, does not.
const FRAME_MS: i64 = 50;
const CMD_MS: i64 = 25;

/// How long a `wait_ready` step holds its input before it starts looking for
/// a ready weapon. The capture's own floor: every `wait_ready` step of both
/// fixtures reports `waited_ready_ms` around 505, even the ones that had
/// nothing to wait for. Without it the wait ends on the snapshot that arrived
/// before the step's first cmd was even simulated, and a step opens with the
/// weapon still busy from the one before it.
const WAIT_FLOOR_MS: i64 = 500;

/// Steps the gate does not compare, with the reason. `walks` is the capture's
/// own exclusion -- the stall response steers it, so where it ends up is not
/// reproducible.
const SKIPPED: &[(&str, &str)] = &[(
    "prone_fire",
    "the prone is taken on one map and refused on the other -- pavlov's \
     capture reads the standing idle and `viewHeightTarget` 60 throughout it \
     -- and the replay spawns somewhere else again, so what the step recorded \
     is not what the replay does (player-model-anim-system.md, \"Prone fire \
     is unmeasured\")",
)];

/// Steps whose `weapAnim` indices are not compared, by map, with the reason.
/// The states and the shot count still are. Empty is the goal and it is
/// empty: pavlov's `idle_after` was the entry, and it earned itself back when
/// the captures were retaken with the usercmd carrying the held weapon. The
/// guard below fails on an entry that starts matching, so the list cannot rot
/// into a lie.
const ANIM_GAPS: &[(&str, &str, &str)] = &[];

struct Step {
    label: String,
    base: UserCmd,
    pulse_buttons: u8,
    pulse_wbuttons: u8,
    pulses: u32,
    pulse_period_ms: i64,
    hold_ms: i64,
    walks: bool,
    wait_ready: bool,
    /// The weapon byte the probe sent through the step, `cmd.weapon`: retail
    /// reads a byte that differs from `ps.weapon` as a request to holster
    /// (`cod11-combat.md` section 1.8), so the replay has to send the same
    /// one. Defaults to the joined weapon's CS 7 index for a capture taken
    /// before the key existed.
    weapon: u8,
    /// Retail's per-snapshot trace: (weaponstate, weapAnim, torsoAnim,
    /// eventSequence, events).
    trace: Vec<Trace>,
}

#[derive(Clone, Copy)]
struct Trace {
    weaponstate: i32,
    weap_anim: i32,
    torso_anim: i32,
    event_sequence: i32,
    events: [i32; 4],
}

fn parse_fixture(text: &str, default_weapon: u8) -> Vec<Step> {
    let mut steps: Vec<Step> = Vec::new();
    for line in text.lines() {
        let line = line.trim_end();
        if line.starts_with('#') || line.is_empty() {
            continue;
        }
        if let Some(label) = line
            .strip_prefix("[step ")
            .and_then(|l| l.strip_suffix(']'))
        {
            steps.push(Step {
                label: label.into(),
                base: NULL_USERCMD,
                pulse_buttons: 0,
                pulse_wbuttons: 0,
                pulses: 0,
                pulse_period_ms: 0,
                hold_ms: 0,
                walks: false,
                wait_ready: false,
                weapon: default_weapon,
                trace: Vec::new(),
            });
            continue;
        }
        let step = steps.last_mut().expect("a line before any [step]");
        let kv = |rest: &str| -> BTreeMap<String, String> {
            rest.split_whitespace()
                .filter_map(|t| t.split_once('='))
                .map(|(k, v)| (k.into(), v.into()))
                .collect()
        };
        if let Some(rest) = line.strip_prefix("!input ") {
            let m = kv(rest);
            let i = |k: &str| m[k].parse::<i64>().unwrap();
            step.base.buttons = i("buttons") as u8;
            step.base.wbuttons = i("wbuttons") as u8;
            step.base.up = i("up") as i8;
            step.base.forward = i("forward") as i8;
            step.base.right = i("right") as i8;
            step.base.angles[1] = i("yaw") as i32;
            step.pulse_buttons = i("pulse_buttons") as u8;
            step.pulse_wbuttons = i("pulse_wbuttons") as u8;
            step.pulses = i("pulses") as u32;
            step.pulse_period_ms = i("pulse_period_ms");
            step.hold_ms = i("hold_ms");
            step.walks = i("walks") != 0;
            step.wait_ready = i("wait_ready") != 0;
            if let Some(w) = m.get("weapon") {
                step.weapon = w.parse::<u8>().unwrap();
            }
        } else if let Some(rest) = line.strip_prefix("!trace ") {
            let m = kv(rest);
            let i = |k: &str| m[k].parse::<i32>().unwrap();
            step.trace.push(Trace {
                weaponstate: i("weaponstate"),
                weap_anim: i("weapAnim"),
                torso_anim: i("torsoAnim"),
                event_sequence: i("eventSequence"),
                events: [
                    i("events[0]"),
                    i("events[1]"),
                    i("events[2]"),
                    i("events[3]"),
                ],
            });
        }
        // `!observed` and the settled field lines are not compared here.
    }
    steps
}

/// Shots a trace holds: the new ring slots between consecutive samples that
/// read 159 or 161 (player-model-anim-system.md, "Neither counter in
/// `!observed` counts shots").
///
/// The counter is bumped after the write, so the slots new in a sample are
/// `prev ..= cur - 1` and not `prev + 1 ..= cur`; reading the latter is one
/// slot high and both drops the newest event and counts a stale one
/// (`vcod_common::net::events::seq_diff`, cod11-combat.md section 7). On
/// retail's own carentan `single_shot` the high reading counts no shot at all.
fn shots(trace: &[Trace]) -> usize {
    trace
        .windows(2)
        .map(|w| {
            let diff = ((w[1].event_sequence - w[0].event_sequence) & 0xff).min(4);
            (0..diff)
                .filter(|i| {
                    let ev = w[1].events[((w[0].event_sequence + i) & 3) as usize];
                    ev == EV_FIRE_WEAPON || ev == EV_FIRE_WEAPON_LASTSHOT
                })
                .count()
        })
        .sum()
}

/// How many samples a trace spent in `state`, and how many separate runs
/// they fell into.
///
/// Both are compared: the total with one sample of slack per run, the run
/// count to one run. A run bounds the state's length to `((N-1)*50, (N+1)*50)` ms and no
/// tighter (player-model-anim-system.md, "The weapon channel"), so a single
/// run's length is the state's duration plus where its edges happened to
/// fall in the sample grid. Retail's snapshots arrive 33 to 66 ms apart and
/// the replay's exactly 50, so that placement is not a fact about the
/// weapon: on carentan's `sustained_fire` two of retail's six shots share a
/// sample with the shot before them and read as one run of 6, and which of
/// ours do is a coincidence of the tap grid. The totals are the durations,
/// and they compare.
fn state_samples(trace: &[Trace], state: i32) -> (usize, usize) {
    let (mut total, mut runs, mut inside) = (0, 0, false);
    for t in trace {
        if t.weaponstate == state {
            total += 1;
            runs += usize::from(!inside);
        }
        inside = t.weaponstate == state;
    }
    (total, runs)
}

/// The `weapAnim` indices a trace took, without the 512 restart toggle: every
/// write flips it, so the raw word is not comparable (combat doc, section 1.2).
fn anims(trace: &[Trace]) -> BTreeSet<i32> {
    trace.iter().map(|t| t.weap_anim & 511).collect()
}

/// The `torsoAnim` indices a trace took, toggle masked off. The weapon channel
/// writes these off the animscript's `EVENTS` blocks
/// (player-model-anim-system.md, "The weapon channel").
fn torsos(trace: &[Trace]) -> BTreeSet<i32> {
    trace.iter().map(|t| t.torso_anim & 511).collect()
}

/// How often the torso's 512 restart toggle flipped between consecutive
/// samples. A shot restarts the same index and is visible only here, and the
/// channel clearing at the end of the anim flips it once more, so a fire step
/// reads one flip per shot plus one: carentan's `sustained_fire` reads 765,
/// 253, 765, 253, 765, 253, 512.
fn torso_flips(trace: &[Trace]) -> usize {
    trace
        .windows(2)
        .filter(|w| (w[0].torso_anim ^ w[1].torso_anim) & 512 != 0)
        .count()
}

fn trace_of(ps: &vcod_common::net::msg::PlayerState) -> Trace {
    let p = &PROTOCOL_V1;
    let ev = |i: usize| ps.field_i32(p, &format!("events[{i}]"));
    Trace {
        weaponstate: ps.field_i32(p, "weaponstate"),
        weap_anim: ps.field_i32(p, "weapAnim"),
        torso_anim: ps.field_i32(p, "torsoAnim"),
        event_sequence: ps.field_i32(p, "eventSequence"),
        events: [ev(0), ev(1), ev(2), ev(3)],
    }
}

/// When each of a step's taps goes down, in ms from the step's start: one
/// usercmd long, at the first cmd at or after the capture's own tap time. The
/// capture holds the bit 32 ms out of every `pulse_period_ms`, which no cmd
/// grid divides evenly, so the tap is placed rather than sampled -- sampling
/// it drops the taps that fall between two cmds.
fn taps(step: &Step) -> Vec<i64> {
    (0..i64::from(step.pulses))
        .map(|k| (k * step.pulse_period_ms + CMD_MS - 1) / CMD_MS * CMD_MS)
        .collect()
}

/// Replays the capture's taps against our server: one trace per snapshot
/// per step, the way the probe traced retail's.
fn ours(
    map: &str,
    steps: &[Step],
    join: (&str, &str),
    fs: vcod_common::pk3::Pk3Fs,
) -> Vec<Vec<Trace>> {
    let bsp_path = fs.resolve_map(map).expect("map in the mounted paks");
    let bsp = vcod_common::bsp::parse(&fs.read(&bsp_path).unwrap()).unwrap();
    let fs = Rc::new(fs);
    let mut now = Instant::now();
    let mut sv = vcod_server::Server::new(cfg(map), now);
    sv.load_world(vcod_server::world::World::from_bsp(&bsp));
    sv.load_scripts(fs).expect("load the scripts");
    let q = Rc::new(RefCell::new(Queues::default()));
    let (mut cl, _join) = common::join(&mut sv, &q, &mut now, join.0, join.1);
    let p = &PROTOCOL_V1;
    let mut out = Vec::new();
    for step in steps {
        let mut trace = Vec::new();
        // The held input, carrying the weapon byte the probe sent: a byte that
        // differs from `ps.weapon` is a holster request, so a replay that left
        // it 0 would not be running the capture's input.
        let mut base = step.base;
        base.weapon = step.weapon;
        if step.wait_ready {
            for i in 0..100 {
                now += Duration::from_millis(FRAME_MS as u64);
                cl.send_frame(&base);
                common::step(&mut sv, &q, &mut cl, now);
                let ready = cl
                    .snapshots()
                    .newest()
                    .is_some_and(|s| s.ps.field_i32(p, "weaponstate") == 0);
                if i * FRAME_MS >= WAIT_FLOOR_MS && ready {
                    break;
                }
            }
        }
        // The state the step opens in, before its first tap: retail's own
        // first sample is the one that arrived with the tap still in flight,
        // so without it a shot on the first frame falls outside every
        // window `shots` looks at.
        if let Some(s) = cl.snapshots().newest() {
            trace.push(trace_of(&s.ps));
        }
        let frames = (step.hold_ms / FRAME_MS).max(1);
        let taps = taps(step);
        for i in 0..frames {
            for half in 0..2 {
                let t = i * FRAME_MS + half * CMD_MS;
                let mut cmd = base;
                if taps.contains(&t) {
                    cmd.buttons |= step.pulse_buttons;
                    cmd.wbuttons |= step.pulse_wbuttons;
                }
                now += Duration::from_millis(CMD_MS as u64);
                cl.pump_at(now);
                cl.send_frame(&cmd);
            }
            common::step(&mut sv, &q, &mut cl, now);
            if let Some(s) = cl.snapshots().newest() {
                trace.push(trace_of(&s.ps));
            }
        }
        out.push(trace);
    }
    out
}

fn check(map: &str) {
    let Some(fs) = vcod_common::testing::game_fs() else {
        return;
    };
    let path = format!(
        "{}/tests/fixtures/playerstate/{map}-{}-combat.txt",
        env!("CARGO_MANIFEST_DIR"),
        cfg(map).gametype
    );
    let text = std::fs::read_to_string(&path).unwrap();
    let team = common::header_value(&text, "joined", &path).to_string();
    let weapon = common::header_value(&text, "weapon", &path).to_string();
    let held =
        vcod_server::configstrings::weapon_index(&weapon).expect("the joined weapon in CS 7");
    let steps = parse_fixture(&text, held as u8);
    let mine = ours(map, &steps, (&team, &weapon), fs);
    let mut bad = Vec::new();
    for (step, ours) in steps.iter().zip(&mine) {
        if step.walks || SKIPPED.iter().any(|(l, _)| *l == step.label) {
            continue;
        }
        let (rs, os) = (shots(&step.trace), shots(ours));
        if rs != os {
            bad.push(format!("{}: retail {rs} shots, ours {os}", step.label));
        }
        for state in [2, 3, 4, 5] {
            let ((r, r_runs), (o, o_runs)) = (
                state_samples(&step.trace, state),
                state_samples(ours, state),
            );
            if r == 0 && o == 0 {
                continue;
            }
            // How often the state was entered is a fact about the machine
            // and not about the sampling, so it is compared on its own, to
            // one run: a state ours flickers into once per shot where retail
            // enters it once would otherwise buy itself a sample of slack per
            // flicker and pass. Retail's carentan `crouch_fire` enters
            // `weaponstate` 3 three times where ours enters it twice, two
            // shots having shared a sample, which is where the one comes
            // from.
            let slack = r_runs.max(o_runs).max(1);
            if r.abs_diff(o) > slack || r_runs.abs_diff(o_runs) > 1 {
                bad.push(format!(
                    "{}: weaponstate {state} holds {r} samples over {r_runs} runs on \
                     retail, {o} over {o_runs} on ours",
                    step.label
                ));
            }
        }
        let same_anims = anims(&step.trace) == anims(ours);
        match ANIM_GAPS
            .iter()
            .find(|(m, l, _)| *m == map && *l == step.label)
        {
            Some((.., why)) => assert!(
                !same_anims,
                "{map} {}: the weapAnim indices match now; drop the ANIM_GAPS \
                 entry ({why})",
                step.label
            ),
            None if !same_anims => bad.push(format!(
                "{}: weapAnim indices retail {:?} ours {:?}",
                step.label,
                anims(&step.trace),
                anims(ours)
            )),
            None => {}
        }
        if torsos(&step.trace) != torsos(ours) {
            bad.push(format!(
                "{}: torsoAnim indices retail {:?} ours {:?}",
                step.label,
                torsos(&step.trace),
                torsos(ours)
            ));
        }
        let (r_flips, o_flips) = (torso_flips(&step.trace), torso_flips(ours));
        if r_flips != o_flips {
            bad.push(format!(
                "{}: the torso toggle flipped {r_flips} times on retail, {o_flips} on ours \
                 ({rs} shots)",
                step.label
            ));
        }
    }
    assert!(
        bad.is_empty(),
        "the weapon channel differs from retail on {map}:\n  {}",
        bad.join("\n  ")
    );
}

#[test]
fn the_weapon_channel_matches_retail_on_mp_carentan() {
    check("mp_carentan");
}

#[test]
fn the_weapon_channel_matches_retail_on_mp_pavlov() {
    check("mp_pavlov");
}
