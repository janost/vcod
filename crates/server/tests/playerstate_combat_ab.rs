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

use common::{parse_fixture, replay, Sample, Trace, CMD_MS, FRAME_MS};
use std::collections::BTreeSet;
use vcod_common::net::protocol::PROTOCOL_V1;

const EV_FIRE_WEAPON: i32 = 159;
const EV_FIRE_WEAPON_LASTSHOT: i32 = 161;
const EV_MELEE_SWIPE: i32 = 164;

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
const TORSO_GAPS: &[(&str, &str, &str)] = &[];

/// Steps whose `torsoAnim` index is drawn rather than fixed, with the reason.
/// The `meleeattack` clause lists several anims per channel and retail draws
/// among them, so the index a capture happened to record is not something a
/// replay can reproduce: only whether the channel was written, and how often
/// the restart toggle flipped, are comparable. Both are still checked.
const TORSO_DRAWN: &[(&str, &str, &str)] = &[(
    "mp_carentan",
    "melee_tap",
    "the swing's torso anim is drawn among the `meleeattack` clause's five \
     lines, on retail as on ours",
)];

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

fn trace_of(s: &Sample, ms: i64) -> Trace {
    let (p, ps) = (&PROTOCOL_V1, &s.ps);
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
    for r in retail {
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
    // A grenade capture replays from the spot its own script threw from:
    // where a blast lands, and so who it hurts, is the map's business and
    // not the input's (`# grenade` header, AGENTS.md).
    let place = common::captured_place(&text);
    let mine: Vec<Vec<Trace>> = replay(map, gametype, &steps, (&team, &weapon), fs, place)
        .iter()
        .map(|step| step.iter().map(|s| trace_of(s, s.ms)).collect())
        .collect();
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
        // A drawn index is not reproducible, so the comparison drops to
        // whether the channel was written at all.
        let drawn = TORSO_DRAWN
            .iter()
            .any(|(m, l, _)| *m == map && *l == step.label);
        let written =
            |t: &[Trace]| -> BTreeSet<bool> { torsos(t).iter().map(|i| *i != 0).collect() };
        let same_torsos = if drawn {
            written(&step.trace) == written(ours)
        } else {
            torsos(&step.trace) == torsos(ours)
        };
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
