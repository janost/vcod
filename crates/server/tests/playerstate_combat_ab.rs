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

/// The settled fields a step's end is compared on: which weapons the player
/// holds and which one is in hand.
const HELD_FIELDS: &[&str] = &["weapons[0]", "weapon"];

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

/// One channel of one step that the gate does not compare, keyed by map,
/// capture kind and step label, with the reason. The kind is in the key
/// because two captures share a map and a label: `ads` is the carbine's and
/// `ads-sniper` the scoped rifle's, and a gap one weapon needs must not
/// excuse the other.
///
/// Every entry is a known divergence from retail, not a fact about it, and
/// the list is self-cleaning: [`check`] asserts that a gapped channel still
/// differs, so an entry that starts matching fails the run rather than
/// quietly outliving the defect it names. Empty is the goal. It was empty
/// until the scoped-rifle capture landed; the four entries below are one
/// open defect, named in [`RECHAMBER_GAP`].
const KNOWN_GAPS: &[Gap] = &[
    Gap {
        map: "mp_carentan",
        kind: "ads-sniper",
        label: "ads_release",
        channel: "weaponstate",
        why: RECHAMBER_GAP,
    },
    Gap {
        map: "mp_carentan",
        kind: "ads-sniper",
        label: "ads_release",
        channel: "weaponDelay",
        why: RECHAMBER_GAP,
    },
    Gap {
        map: "mp_carentan",
        kind: "ads-sniper",
        label: "ads_release",
        channel: "weapAnim",
        why: RECHAMBER_GAP,
    },
    Gap {
        map: "mp_carentan",
        kind: "ads-sniper",
        label: "ads_release",
        channel: "torsoAnim",
        why: RECHAMBER_GAP,
    },
    Gap {
        map: "mp_carentan",
        kind: "ads-sniper",
        label: "ads_shot",
        channel: "torsoAnim",
        why: RECHAMBER_GAP,
    },
    Gap {
        map: "mp_carentan",
        kind: "ads-sniper",
        label: "ads_walk",
        channel: "transients",
        why: ADS_WALK_GAP,
    },
];

/// The second open defect, and the one worth chasing: walking away from the
/// capture's own spawn with the sight held, ours reads `groundEntityNum` 1023
/// for one sample where retail reads 1022 for all 31. `update_ads_flag`
/// clears the sight for an airborne frame, so `advance_ads` ramps *down* by
/// `msec / adsTransOutTime` for that cmd and back up for the next: at 25 ms
/// cmds that is -0.0625 then +0.083, and the capture reads the fraction
/// moving 0.333 to 0.354 across a 50 ms frame it should have moved 0.167.
/// On `kar98k_sniper_mp` (`adsZoomFov` 16) a client re-basing its zoom
/// prediction off that is the twitch this branch was opened for.
///
/// The step's spread counter is gapped with it and for a weaker reason: the
/// walk covers the map's own geometry, so when it stops climbing is a fact
/// about where the walk ended, which is why `walks` steps are excluded
/// wholesale elsewhere. Retail's counter starts decaying ~300 ms in and ours
/// does not.
///
/// Only the walking sight step is gapped. Every standing one is compared,
/// and the fraction matches there, which is what says the ramp itself is
/// right and the ground trace is not.
const ADS_WALK_GAP: &str = "ours goes airborne for one sample where retail never does, which reverses      the sight ramp for that cmd (see ADS_WALK_GAP, open)";

/// The one open defect [`KNOWN_GAPS`] names, measured off the retail
/// `mp_carentan-tdm-ads-sniper` capture: after a scoped `kar98k_sniper_mp`
/// shot, ours runs a rechamber retail does not. Retail's `ads_release` reads
/// `weaponstate` 0 and 9 with `weaponDelay` 0 throughout; ours holds
/// `weaponstate` 5 for 12 samples and a 175 ms `weaponDelay`, and writes the
/// rechamber's `weapAnim` 11 and 13 and a `torsoAnim` on the shot step as
/// well. The sight fraction is unaffected -- it reads 1.0 through every held
/// sample on both sides -- which is why the rest of this capture is compared
/// rather than skipped. The fix is in the weapon machine
/// (`vcod_common::pmove::weapon`); until it lands the channels are gapped and
/// the divergence is visible here rather than absent.
const RECHAMBER_GAP: &str = "ours runs a bolt-action rechamber after the scoped shot that retail does      not: retail holds `weaponDelay` 0 and never enters `weaponstate` 5, ours      holds 175 ms and 12 samples of it (see RECHAMBER_GAP, open)";

/// One [`KNOWN_GAPS`] entry.
struct Gap {
    map: &'static str,
    kind: &'static str,
    label: &'static str,
    /// `weapAnim`, `torsoAnim`, `weaponstate` or `weaponDelay`.
    channel: &'static str,
    why: &'static str,
}

/// The reason this map/kind/step/channel is not compared, if it is gapped.
fn gapped(map: &str, kind: &str, label: &str, channel: &str) -> Option<&'static str> {
    KNOWN_GAPS
        .iter()
        .find(|g| g.map == map && g.kind == kind && g.label == label && g.channel == channel)
        .map(|g| g.why)
}

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

/// The parms the fire events in a trace's ring carry. The grenade's fuse
/// rides `PmEvent.parm` inside vcod only: `eventParms[i]` is an 8-bit
/// netfield, so a 4000 ms fuse would reach a client as 160. Retail writes 0
/// on every throw frame it recorded (`mp_carentan-tdm-grenade-shooter.txt`
/// reads `eventParms=0,0,0,0` there, and so does every settled block of the
/// capture this gate replays); those `!trace` lines carry no `eventParms`
/// column of their own, so retail's side of the comparison is that constant.
fn fire_parms(trace: &[Trace]) -> BTreeSet<i32> {
    let mut out = BTreeSet::new();
    for w in trace.windows(2) {
        let Some(parms) = w[1].event_parms else {
            continue;
        };
        let diff = ((w[1].event_sequence - w[0].event_sequence) & 0xff).min(4);
        for i in 0..diff {
            let slot = ((w[0].event_sequence + i) & 3) as usize;
            if matches!(w[1].events[slot], EV_FIRE_WEAPON | EV_FIRE_WEAPON_LASTSHOT) {
                out.insert(parms[slot]);
            }
        }
    }
    out
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
        event_parms: Some([
            ps.field_i32(p, "eventParms[0]"),
            ps.field_i32(p, "eventParms[1]"),
            ps.field_i32(p, "eventParms[2]"),
            ps.field_i32(p, "eventParms[3]"),
        ]),
        pos_frac: Some(ps.field_f32(p, "fWeaponPosFrac")),
        spread: Some(ps.field_f32(p, "aimSpreadScale")),
        viewangles: Some([
            ps.field_f32(p, "viewangles[0]"),
            ps.field_f32(p, "viewangles[1]"),
            ps.field_f32(p, "viewangles[2]"),
        ]),
        pm_flags: Some(ps.field_i32(p, "pm_flags")),
        ground_entity: Some(ps.field_i32(p, "groundEntityNum")),
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
///
/// [`FRAC_TOL`] buys slack for a point on a ramp, where the two sample grids
/// sit half a frame apart. It buys none at the ends: 0.0 and 1.0 are where
/// the ramp clamps, so a sample retail reads saturated at is one ours has to
/// read saturated at too, exactly. That end is what the client's zoom sits
/// at while a sight is held, and a fraction that leaves it for a frame is
/// the twitch a scoped rifle shows -- `kar98k_sniper_mp` turns the same
/// error into a four times larger swing than `m1carbine_mp`, `adsZoomFov`
/// 16 against 65.
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
        let saturated = rf == 0.0 || rf == 1.0;
        let fracs: Vec<f32> = near.iter().filter_map(|o| o.pos_frac).collect();
        let frac_ok = if saturated {
            fracs.contains(&rf)
        } else {
            // On the slope, retail's sample can fall between two of ours,
            // which is a ramp passing through its value rather than a miss:
            // the two grids are 33-66 ms and 50 ms apart and the sniper's
            // `ads_walk` puts retail's 0.573 between our 0.500 and 0.667.
            // Bracketing says that directly; the tolerance is the fallback
            // for an end of the window with nothing on the far side.
            fracs.iter().any(|f| (f - rf).abs() <= FRAC_TOL)
                || fracs.windows(2).any(|w| (w[0] - rf) * (w[1] - rf) <= 0.0)
        };
        let spread_ok = near
            .iter()
            .any(|o| o.spread.is_some_and(|s| (s - rs).abs() <= SPREAD_TOL));
        // The view is not a transient and gets no tolerance: retail writes
        // `ps.viewangles` as `SHORT2ANGLE(cmd.angles + delta_angles)` and the
        // replay sends the capture's own cmd angles, so every axis has to
        // read the same float. `None` on a capture taken before the column
        // existed, which is every one but the scoped rifle's.
        if let Some(rv) = r.viewangles {
            if let Some(ov) = near.iter().find_map(|o| o.viewangles) {
                if ov != rv {
                    bad.push(format!(
                        "at {}ms retail viewangles {rv:?}, ours {ov:?}",
                        r.ms
                    ));
                }
            }
        }
        if !frac_ok || !spread_ok {
            let ours_at: Vec<String> = near
                .iter()
                .map(|o| {
                    format!(
                        "{}ms {:.3}/{:.1} pm=0x{:x} ground={}",
                        o.ms,
                        o.pos_frac.unwrap_or(f32::NAN),
                        o.spread.unwrap_or(f32::NAN),
                        o.pm_flags.unwrap_or(-1),
                        o.ground_entity.unwrap_or(-1),
                    )
                })
                .collect();
            bad.push(format!(
                "at {}ms retail fWeaponPosFrac {rf:.3} aimSpreadScale {rs:.1} \
                 pm=0x{:x} ground={}, ours {}",
                r.ms,
                r.pm_flags.unwrap_or(-1),
                r.ground_entity.unwrap_or(-1),
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
    let replayed = replay(map, gametype, &steps, (&team, &weapon), fs, place);
    let mine: Vec<Vec<Trace>> = replayed
        .iter()
        .map(|step| step.iter().map(|s| trace_of(s, s.ms)).collect())
        .collect();
    let mut bad = Vec::new();
    for ((step, ours), samples) in steps.iter().zip(&mine).zip(&replayed) {
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
        // A fire event's parm: 0 on retail, and compared to retail's own
        // column where a capture has one.
        let (rp, op) = (fire_parms(&step.trace), fire_parms(ours));
        if op.iter().any(|p| *p != 0) || (!rp.is_empty() && rp != op) {
            bad.push(format!(
                "{}: fire event parms retail {rp:?} ours {op:?}",
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
        let delay_same = !step.trace.iter().any(|t| t.weapon_delay.is_some())
            || (rd - od).abs() <= FRAME_MS as i32;
        match gapped(map, kind, &step.label, "weaponDelay") {
            Some(why) => assert!(
                !delay_same,
                "{map} {kind} {}: weaponDelay matches now; drop the KNOWN_GAPS \
                 entry ({why})",
                step.label
            ),
            None if !delay_same => bad.push(format!(
                "{}: the longest weaponDelay is {rd} on retail, {od} on ours",
                step.label
            )),
            None => {}
        }
        let mut state_bad = Vec::new();
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
                state_bad.push(format!(
                    "{}: weaponstate {state} holds {r} samples over {r_runs} runs on \
                     retail, {o} over {o_runs} on ours",
                    step.label
                ));
            }
        }
        match gapped(map, kind, &step.label, "weaponstate") {
            Some(why) => assert!(
                !state_bad.is_empty(),
                "{map} {kind} {}: every weaponstate matches now; drop the KNOWN_GAPS \
                 entry ({why})",
                step.label
            ),
            None => bad.append(&mut state_bad),
        }
        let same_anims = anims(&step.trace) == anims(ours);
        match gapped(map, kind, &step.label, "weapAnim") {
            Some(why) => assert!(
                !same_anims,
                "{map} {kind} {}: the weapAnim indices match now; drop the KNOWN_GAPS \
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
        match gapped(map, kind, &step.label, "torsoAnim") {
            Some(why) => assert!(
                !same_torsos,
                "{map} {kind} {}: the torsoAnim indices match now; drop the KNOWN_GAPS \
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
        // What the step left the player holding: retail's last throw spends
        // the frag's only clip and the weapon goes with it (combat doc, 1.5
        // step 9), so `throw_down` ends on `weapons[0]` 4112 and `weapon` 0.
        if let Some(last) = samples.last() {
            for field in HELD_FIELDS {
                let Some(r) = step.settled.get(*field) else {
                    continue;
                };
                let o = last.ps.field_i32(&PROTOCOL_V1, field);
                if o != *r {
                    bad.push(format!("{}: {field} retail {r}, ours {o}", step.label));
                }
            }
        }
        let misses = transient_misses(&step.trace, ours);
        match gapped(map, kind, &step.label, "transients") {
            Some(why) => assert!(
                !misses.is_empty(),
                "{map} {kind} {}: the sight and spread match now; drop the \
                 KNOWN_GAPS entry ({why})",
                step.label
            ),
            None if !misses.is_empty() => bad.push(format!(
                "{}: {} of {} samples off\n    {}",
                step.label,
                misses.len(),
                step.trace.len(),
                misses.join("\n    ")
            )),
            None => {}
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

/// The same sight script on the scoped rifle. `m1carbine_mp` is `adsZoomFov`
/// 65, `kar98k_sniper_mp` 16, so the sniper is where a sight fraction that is
/// merely close reads as the weapon twitching; this capture is what says the
/// fraction is not what twitches. It is also the capture that measured
/// `ps.viewangles` on the wire for a spawned player, which is compared here
/// exactly.
#[test]
fn the_scoped_sight_and_view_match_retail_on_mp_carentan() {
    check("mp_carentan", "tdm", "ads-sniper");
}

#[test]
fn the_grenade_and_melee_channels_match_retail_on_mp_carentan() {
    check("mp_carentan", "tdm", "grenade");
}
