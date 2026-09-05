//! The weapon channel under each combat input, diffed per snapshot against
//! the retail combat captures. A shot is a transient the event ring
//! overwrites within four slots, so both sides carry a `!trace` line per
//! snapshot and the comparison is over those, never over a settled sample.
//!
//! The `--save-ads` captures ride the same replay: the sight fraction and
//! the spread counter are transients too, and every retail sample of them
//! has to be met by one of ours within a snapshot's worth of time.
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

fn cfg(map: &str, gametype: &str) -> vcod_server::ServerConfig {
    vcod_server::ServerConfig {
        map: map.into(),
        hostname: "vcod test".into(),
        max_clients: 8,
        gametype: gametype.into(),
        test_entities: 0,
        trace: false,
    }
}

const EV_FIRE_WEAPON: i32 = 159;
const EV_FIRE_WEAPON_LASTSHOT: i32 = 161;
const EV_MELEE_SWIPE: i32 = 164;
const EV_PAIN: i32 = 187;

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

/// Steps whose `torsoAnim` is not compared, by map, with the reason. An entry
/// suppresses the restart-toggle flip count with the indices, since a channel
/// nobody writes cannot flip. Same self-cleaning guard as [`ANIM_GAPS`]: an
/// entry that starts matching fails.
const TORSO_GAPS: &[(&str, &str, &str)] = &[(
    "mp_carentan",
    "melee_tap",
    "the swing's torso anim is the animscript's `meleeattack` clause and \
     `spectate.rs::weapon_anim_event` maps no event to it yet",
)];

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
    /// Bits held down for the first `press_ms` of the step rather than
    /// tapped: a grenade is cooked by holding the trigger and thrown by the
    /// release, which the tap machinery above cannot express.
    press_buttons: u8,
    press_ms: i64,
    /// A `cmd.weapon` the step asks for `switch_ms` into itself, held on
    /// every cmd after that: retail's pickup half reads the byte again on the
    /// frame the putaway ends, so a byte sent once leaves the old weapon in
    /// hand (`cod11-combat.md` section 1.8).
    switch_weapon: u8,
    switch_ms: i64,
    /// Retail's per-snapshot trace: (weaponstate, weapAnim, torsoAnim,
    /// eventSequence, events).
    trace: Vec<Trace>,
}

#[derive(Clone, Copy)]
struct Trace {
    /// ms into the step; retail's is the probe's clock, ours the frame grid.
    ms: i64,
    weaponstate: i32,
    weap_anim: i32,
    torso_anim: i32,
    event_sequence: i32,
    events: [i32; 4],
    /// `fWeaponPosFrac` and `aimSpreadScale`; `None` on a capture taken
    /// before the trace carried them.
    pos_frac: Option<f32>,
    spread: Option<f32>,
    /// `grenadeTimeLeft` and `weaponDelay`; `None` on a capture taken before
    /// the trace carried them.
    grenade_time_left: Option<i32>,
    weapon_delay: Option<i32>,
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
                press_buttons: 0,
                press_ms: 0,
                switch_weapon: 0,
                switch_ms: 0,
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
            // The `throw_down` step aims 45 degrees below the horizon; every
            // capture taken before the column existed left it level.
            if let Some(p) = m.get("pitch") {
                step.base.angles[0] = p.parse::<i64>().unwrap() as i32;
            }
            for (key, into) in [
                ("press_buttons", &mut step.press_buttons),
                ("switch_weapon", &mut step.switch_weapon),
            ] {
                if let Some(v) = m.get(key) {
                    *into = v.parse::<u8>().unwrap();
                }
            }
            for (key, into) in [
                ("press_ms", &mut step.press_ms),
                ("switch_ms", &mut step.switch_ms),
            ] {
                if let Some(v) = m.get(key) {
                    *into = v.parse::<i64>().unwrap();
                }
            }
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
            let f = |k: &str| m.get(k).map(|v| v.parse::<f32>().unwrap());
            step.trace.push(Trace {
                ms: m["ms"].parse::<i64>().unwrap(),
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
                pos_frac: f("fWeaponPosFrac"),
                spread: f("aimSpreadScale"),
                grenade_time_left: m.get("grenadeTimeLeft").map(|v| v.parse().unwrap()),
                weapon_delay: m.get("weaponDelay").map(|v| v.parse().unwrap()),
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
    ring_events(trace, |ev| {
        ev == EV_FIRE_WEAPON || ev == EV_FIRE_WEAPON_LASTSHOT
    })
}

/// Melee swings a trace holds, the same ring walk over `EV_MELEE_SWIPE`. A
/// swing is one event per press, so this compares exactly.
fn swings(trace: &[Trace]) -> usize {
    ring_events(trace, |ev| ev == EV_MELEE_SWIPE)
}

/// The new ring slots between consecutive samples that `want` accepts.
fn ring_events(trace: &[Trace], want: impl Fn(i32) -> bool) -> usize {
    trace
        .windows(2)
        .map(|w| {
            let diff = ((w[1].event_sequence - w[0].event_sequence) & 0xff).min(4);
            (0..diff)
                .filter(|i| want(w[1].events[((w[0].event_sequence + i) & 3) as usize]))
                .count()
        })
        .sum()
}

/// The `grenadeTimeLeft` values a trace took, bucketed to the frame: the fuse
/// is armed and cleared inside a frame, so the exact ms a sample catches it at
/// is the sampling grid's and not the machine's.
fn fuses(trace: &[Trace]) -> BTreeSet<i32> {
    trace
        .iter()
        .filter_map(|t| t.grenade_time_left)
        .map(|v| (v + 25) / 50 * 50)
        .collect()
}

/// The longest `weaponDelay` a trace sampled: `holdFireTime` on a pullback,
/// `meleeDelay` on a swing, 0 anywhere else. Compared to one frame, since
/// where the first sample after the write falls is the grid's.
fn peak_delay(trace: &[Trace]) -> i32 {
    trace
        .iter()
        .filter_map(|t| t.weapon_delay)
        .max()
        .unwrap_or(0)
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

/// ms into the step retail first took a hit at, if it did. From there on the
/// cone is `P_DamageFeedback`'s and not the weapon machine's, and vcod hurts
/// nobody with a grenade yet: no missile is spawned. Only the grenade
/// capture's `throw_down` reaches it, where the third frag goes off at the
/// thrower's own feet.
fn hurt_at_ms(trace: &[Trace]) -> Option<i64> {
    trace.windows(2).find_map(|w| {
        let diff = ((w[1].event_sequence - w[0].event_sequence) & 0xff).min(4);
        (0..diff)
            .any(|i| w[1].events[((w[0].event_sequence + i) & 3) as usize] == EV_PAIN)
            .then_some(w[1].ms)
    })
}

fn trace_of(ps: &vcod_common::net::msg::PlayerState, ms: i64) -> Trace {
    let p = &PROTOCOL_V1;
    let ev = |i: usize| ps.field_i32(p, &format!("events[{i}]"));
    Trace {
        ms,
        weaponstate: ps.field_i32(p, "weaponstate"),
        weap_anim: ps.field_i32(p, "weapAnim"),
        torso_anim: ps.field_i32(p, "torsoAnim"),
        event_sequence: ps.field_i32(p, "eventSequence"),
        events: [ev(0), ev(1), ev(2), ev(3)],
        pos_frac: Some(ps.field_f32(p, "fWeaponPosFrac")),
        spread: Some(ps.field_f32(p, "aimSpreadScale")),
        grenade_time_left: Some(ps.field_i32(p, "grenadeTimeLeft")),
        weapon_delay: Some(ps.field_i32(p, "weaponDelay")),
    }
}

/// A snapshot's worth of slack when a retail sample is looked for among
/// ours: retail's arrive 33 to 66 ms apart, ours every 50.
const SAMPLE_SLACK_MS: i64 = 60;
/// How far a matched sample may sit from retail's: half a frame of the
/// fastest slope either transient has. The ramp covers a sixth of the way
/// per frame at its fastest (300 ms up), the counter 51 of 255 (the
/// carbine's decay), and the probe steps pmove every 16 ms where the replay
/// steps it every 25, so the two grids sit up to half a frame apart on a
/// slope. Carentan's `ads_walk` decay reads 25.5 off at every sample for
/// exactly that reason.
const FRAC_TOL: f32 = 0.09;
const SPREAD_TOL: f32 = 26.0;

/// Every retail sample of the sight fraction and the spread counter has one
/// of ours within [`SAMPLE_SLACK_MS`] that reads the same to tolerance.
/// Returns the misses, worst first.
fn transient_misses(retail: &[Trace], ours: &[Trace]) -> Vec<String> {
    let mut bad = Vec::new();
    let hurt = hurt_at_ms(retail).unwrap_or(i64::MAX);
    // The same self-cleaning guard [`TORSO_GAPS`] carries: the skip is only
    // honest for as long as nothing on our side hurts the player either. The
    // moment the missile pass spawns a grenade and the blast does radius
    // damage, this fails and the skip has to go.
    assert!(
        hurt == i64::MAX || hurt_at_ms(ours).is_none(),
        "ours raises EV_PAIN too now; drop the skip in transient_misses -- it \
         exists only because vcod spawns no missile and does no radius damage"
    );
    for r in retail {
        if r.ms >= hurt {
            continue;
        }
        let (Some(rf), Some(rs)) = (r.pos_frac, r.spread) else {
            continue;
        };
        let near: Vec<&Trace> = ours
            .iter()
            .filter(|o| (o.ms - r.ms).abs() <= SAMPLE_SLACK_MS)
            .collect();
        if near.is_empty() {
            continue;
        }
        let frac_ok = near
            .iter()
            .any(|o| o.pos_frac.is_some_and(|f| (f - rf).abs() <= FRAC_TOL));
        let spread_ok = near
            .iter()
            .any(|o| o.spread.is_some_and(|s| (s - rs).abs() <= SPREAD_TOL));
        if !frac_ok || !spread_ok {
            let ours_at: Vec<String> = near
                .iter()
                .map(|o| {
                    format!(
                        "{}ms {:.3}/{:.1}",
                        o.ms,
                        o.pos_frac.unwrap_or(f32::NAN),
                        o.spread.unwrap_or(f32::NAN)
                    )
                })
                .collect();
            bad.push(format!(
                "at {}ms retail fWeaponPosFrac {rf:.3} aimSpreadScale {rs:.1}, ours {}",
                r.ms,
                ours_at.join(", ")
            ));
        }
    }
    bad
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
/// The weapon byte a cmd carries: the step's switch once it has been asked
/// for, the step's own byte otherwise, and our own `ps.weapon` where the
/// capture recorded a 0. A 0 is not neutral -- retail reads a `cmd.weapon`
/// differing from `ps.weapon` as a request to holster -- and the capture's 0
/// means the probe was holding nothing, which is a fact about retail's
/// playerstate and not an input to replay.
fn weapon_byte(step: &Step, t: i64, ours_now: u8) -> u8 {
    if step.switch_weapon != 0 && t >= step.switch_ms {
        step.switch_weapon
    } else if step.weapon != 0 {
        step.weapon
    } else {
        ours_now
    }
}

fn ours(
    map: &str,
    gametype: &str,
    steps: &[Step],
    join: (&str, &str),
    fs: vcod_common::pk3::Pk3Fs,
) -> Vec<Vec<Trace>> {
    let bsp_path = fs.resolve_map(map).expect("map in the mounted paks");
    let bsp = vcod_common::bsp::parse(&fs.read(&bsp_path).unwrap()).unwrap();
    let fs = Rc::new(fs);
    let mut now = Instant::now();
    let mut sv = vcod_server::Server::new(cfg(map, gametype), now);
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
        let held = |cl: &vcod_common::net::NetClient<common::ClientEnd>| -> u8 {
            cl.snapshots()
                .newest()
                .map_or(0, |s| s.ps.field_i32(p, "weapon") as u8)
        };
        if step.weapon == 0 {
            base.weapon = held(&cl);
        }
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
            trace.push(trace_of(&s.ps, 0));
        }
        let frames = (step.hold_ms / FRAME_MS).max(1);
        let taps = taps(step);
        for i in 0..frames {
            let ours_now = held(&cl);
            for half in 0..2 {
                let t = i * FRAME_MS + half * CMD_MS;
                let mut cmd = base;
                cmd.weapon = weapon_byte(step, t, ours_now);
                if t < step.press_ms {
                    cmd.buttons |= step.press_buttons;
                }
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
                trace.push(trace_of(&s.ps, (i + 1) * FRAME_MS));
            }
        }
        out.push(trace);
    }
    out
}

fn check(map: &str, gametype: &str, kind: &str) {
    let Some(fs) = vcod_common::testing::game_fs() else {
        return;
    };
    let path = format!(
        "{}/tests/fixtures/playerstate/{map}-{gametype}-{kind}.txt",
        env!("CARGO_MANIFEST_DIR")
    );
    let text = std::fs::read_to_string(&path).unwrap();
    // The gametype is the fixture's, not a default: the grenade capture was
    // taken under `tdm` and the two the bullet ones under `dm`, and the
    // gametype picks the loadout and the spawn.
    assert_eq!(
        common::header_value(&text, "g_gametype", &path),
        gametype,
        "{path}: the header's gametype is not the one the test asks for"
    );
    let team = common::header_value(&text, "joined", &path).to_string();
    let weapon = common::header_value(&text, "weapon", &path).to_string();
    let held =
        vcod_server::configstrings::weapon_index(&weapon).expect("the joined weapon in CS 7");
    let steps = parse_fixture(&text, held as u8);
    let mine = ours(map, gametype, &steps, (&team, &weapon), fs);
    let mut bad = Vec::new();
    for (step, ours) in steps.iter().zip(&mine) {
        if step.walks || SKIPPED.iter().any(|(l, _)| *l == step.label) {
            continue;
        }
        let (rs, os) = (shots(&step.trace), shots(ours));
        if rs != os {
            bad.push(format!("{}: retail {rs} shots, ours {os}", step.label));
        }
        let (rw, ow) = (swings(&step.trace), swings(ours));
        if rw != ow {
            bad.push(format!(
                "{}: retail {rw} melee swings, ours {ow}",
                step.label
            ));
        }
        let (rf, of) = (fuses(&step.trace), fuses(ours));
        if !rf.is_empty() && rf != of {
            bad.push(format!(
                "{}: grenadeTimeLeft retail {rf:?} ours {of:?}",
                step.label
            ));
        }
        let (rd, od) = (peak_delay(&step.trace), peak_delay(ours));
        if step.trace.iter().any(|t| t.weapon_delay.is_some()) && (rd - od).abs() > FRAME_MS as i32
        {
            bad.push(format!(
                "{}: the longest weaponDelay is {rd} on retail, {od} on ours",
                step.label
            ));
        }
        for state in [1, 2, 3, 4, 5, 10, 11] {
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
        let same_torsos = torsos(&step.trace) == torsos(ours);
        // Ours opens every step with the snapshot the step before it ended
        // on; retail's own first sample is whenever its next snapshot
        // happened to arrive. Where that is more than a cmd in, retail missed
        // the write on the step boundary and ours has to drop the sample that
        // holds it or it counts a flip retail could not have seen.
        let aligned = usize::from(step.trace.first().is_some_and(|t| t.ms >= CMD_MS));
        let (r_flips, o_flips) = (
            torso_flips(&step.trace),
            torso_flips(&ours[aligned.min(ours.len())..]),
        );
        match TORSO_GAPS
            .iter()
            .find(|(m, l, _)| *m == map && *l == step.label)
        {
            Some((.., why)) => assert!(
                !same_torsos,
                "{map} {}: the torsoAnim indices match now; drop the TORSO_GAPS \
                 entry ({why})",
                step.label
            ),
            None => {
                if !same_torsos {
                    bad.push(format!(
                        "{}: torsoAnim indices retail {:?} ours {:?}",
                        step.label,
                        torsos(&step.trace),
                        torsos(ours)
                    ));
                }
                if r_flips != o_flips {
                    bad.push(format!(
                        "{}: the torso toggle flipped {r_flips} times on retail, {o_flips} on \
                         ours ({rs} shots)",
                        step.label
                    ));
                }
            }
        }
        let misses = transient_misses(&step.trace, ours);
        if !misses.is_empty() {
            bad.push(format!(
                "{}: {} of {} samples off\n    {}",
                step.label,
                misses.len(),
                step.trace.len(),
                misses.join("\n    ")
            ));
        }
    }
    assert!(
        bad.is_empty(),
        "the weapon channel differs from retail on {map} ({kind}):\n  {}",
        bad.join("\n  ")
    );
}

#[test]
fn the_weapon_channel_matches_retail_on_mp_carentan() {
    check("mp_carentan", "dm", "combat");
}

#[test]
fn the_weapon_channel_matches_retail_on_mp_pavlov() {
    check("mp_pavlov", "dm", "combat");
}

#[test]
fn the_sight_and_spread_match_retail_on_mp_carentan() {
    check("mp_carentan", "dm", "ads");
}

#[test]
fn the_sight_and_spread_match_retail_on_mp_pavlov() {
    check("mp_pavlov", "dm", "ads");
}

#[test]
fn the_grenade_and_melee_channels_match_retail_on_mp_carentan() {
    check("mp_carentan", "tdm", "grenade");
}
