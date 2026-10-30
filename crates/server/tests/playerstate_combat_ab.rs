//! The weapon channel under each combat input, diffed per snapshot against
//! the retail combat captures. A shot is a transient the event ring
//! overwrites within four slots, so both sides carry a `!trace` line per
//! snapshot and the comparison is over those, never over a settled sample.
//!
//! The `--save-ads` captures ride the same replay: the sight fraction and
//! the spread counter are transients too.
//!
//! Every capture logs each cmd the probe sent and each snapshot's
//! `commandTime`, and the replay runs retail's cmds frame for frame
//! (`common::CmdLog`), so ours samples the same frames retail did and every
//! channel is compared sample for sample, with no slack.
//!
//! The 1.1 captures need `COD_DIR` at a 1.1 install, the 1.5 one at a 1.5
//! install (a UO one's `main/` is 1.5's), the UO ones at a UO install;
//! without the paks each returns early.

use crate::common;

use common::{Sample, Trace, parse_fixture};
use std::collections::BTreeSet;
use vcod_common::net::protocol::{PROTOCOL_V1, PROTOCOL_V6, PROTOCOL_V22, Protocol};

const EV_FIRE_WEAPON: i32 = 159;
const EV_FIRE_WEAPON_LASTSHOT: i32 = 161;
const EV_MELEE_SWIPE: i32 = 164;

/// The settled fields a step's end is compared on: which weapons the player
/// holds and which one is in hand.
const HELD_FIELDS: &[&str] = &["weapons[0]", "weapon"];

/// One channel of one step that the gate does not compare, keyed by map,
/// capture kind and step label, with the reason. The kind is in the key
/// because two captures share a map and a label: `ads` is the carbine's and
/// `ads-sniper` the scoped rifle's, and a gap one weapon needs must not
/// excuse the other.
///
/// Every entry is a known divergence from retail, not a fact about it, and
/// the list is self-cleaning: [`check`] asserts that a gapped channel still
/// differs, so an entry that starts matching fails the run rather than
/// quietly outliving the defect it names. Empty is the goal. The scoped
/// rifle's segmented reload was closed 2026-09-07 (cod11-combat.md, 9.2).
///
/// The tap phase rows and the scoped reload's `ads_release` boundary closed
/// 2026-10-18, once the captures logged every cmd and each snapshot's
/// `commandTime` and the replay ran retail's cmds frame for frame
/// (`common::CmdLog`).
const KNOWN_GAPS: &[Gap] = &[];

/// One [`KNOWN_GAPS`] entry.
struct Gap {
    map: &'static str,
    kind: &'static str,
    label: &'static str,
    /// `weapAnim`, `torsoAnim`, `weaponstate`, `weaponDelay`, `transients`
    /// (the sight fraction and the spread counter together) or `viewangles`.
    /// The last two are separate channels so a gap on one cannot hide a
    /// regression in the other behind a self-clean assert that only knows
    /// something still differs.
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
/// reads `eventParms=0,0,0,0` there, and so does this gate's own capture,
/// whose `!trace` lines carry the column).
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

/// The `grenadeTimeLeft` values a trace took.
fn fuses(trace: &[Trace]) -> BTreeSet<i32> {
    trace.iter().filter_map(|t| t.grenade_time_left).collect()
}

/// The longest `weaponDelay` a trace sampled: what is left of
/// `holdFireTime` on a pullback, of `meleeDelay` on a swing, 0 anywhere else.
fn peak_delay(trace: &[Trace]) -> i32 {
    trace
        .iter()
        .filter_map(|t| t.weapon_delay)
        .max()
        .unwrap_or(0)
}

/// How many samples a trace spent in `state`, and how many separate runs
/// they fell into. Both are compared exactly: the replay samples retail's
/// frames, so a run's length is the state's duration on both sides.
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
/// The lone state-0 sample between a shot's 3 and its rechamber's 4 is kept:
/// it is the one cmd between the release check and the rechamber
/// (cod11-combat.md 1.4), and the replay runs retail's cmds.
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

/// One of our samples as a trace, its events in 1.1's numbering as the
/// probe prints a capture's.
fn trace_of(p: &Protocol, s: &Sample, ms: i64) -> Trace {
    let ps = &s.ps;
    let ev = |i: usize| (p.event_to_v1)(ps.field_i32(p, &format!("events[{i}]")));
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
        server_time: Some(s.server_time),
        command_time: Some(ps.field_i32(p, "commandTime")),
    }
}

/// Every retail sample's sight fraction and spread counter against ours of
/// the same frame, to the capture's print (4 and 2 decimals). That end of
/// the ramp is what the client's zoom sits at while a sight is held, and a
/// fraction that leaves it for a frame is the twitch a scoped rifle shows:
/// `kar98k_sniper_mp` turns the same error into a four times larger swing
/// than `m1carbine_mp`, `adsZoomFov` 16 against 65.
fn transient_misses(retail: &[Trace], ours: &[Trace]) -> Vec<String> {
    let mut bad = Vec::new();
    for r in retail {
        let (Some(rf), Some(rs)) = (r.pos_frac, r.spread) else {
            continue;
        };
        let Some(o) = ours.iter().find(|o| o.ms == r.ms) else {
            bad.push(format!("at {}ms ours has no sample", r.ms));
            continue;
        };
        let (of, os) = (o.pos_frac.unwrap_or(f32::NAN), o.spread.unwrap_or(f32::NAN));
        if !((rf - of).abs() <= 0.00005 && (rs - os).abs() <= 0.005) {
            bad.push(format!(
                "at {}ms retail fWeaponPosFrac {rf:.4} aimSpreadScale {rs:.2} pm=0x{:x} \
                 ground={}, ours {of:.4} {os:.2} pm=0x{:x} ground={}",
                r.ms,
                r.pm_flags.unwrap_or(-1),
                r.ground_entity.unwrap_or(-1),
                o.pm_flags.unwrap_or(-1),
                o.ground_entity.unwrap_or(-1),
            ));
        }
    }
    bad
}

/// Every retail sample's `viewangles` against ours of the same frame, with
/// no tolerance. The view is not a transient: retail writes it as
/// `SHORT2ANGLE(cmd.angles + delta_angles)` and the replay sends the
/// capture's own cmd angles, so every axis has to read the same float.
///
/// It is its own channel and not part of [`transient_misses`] on purpose. A
/// gap that suppressed both would let a view regression hide behind a sight
/// one, and the self-clean assert -- which only knows that *something* still
/// differs -- would keep passing while it did.
///
fn view_misses(retail: &[Trace], ours: &[Trace]) -> Vec<String> {
    let mut bad = Vec::new();
    for r in retail {
        let Some(rv) = r.viewangles else { continue };
        let Some(ov) = ours
            .iter()
            .find(|o| o.ms == r.ms)
            .and_then(|o| o.viewangles)
        else {
            bad.push(format!("at {}ms ours has no sample", r.ms));
            continue;
        };
        if ov != rv {
            bad.push(format!(
                "at {}ms retail viewangles {rv:?}, ours {ov:?}",
                r.ms
            ));
        }
    }
    bad
}

fn check(map: &str, gametype: &str, kind: &str) {
    let Some(fs) = vcod_common::testing::game_fs_11() else {
        return;
    };
    check_on(&PROTOCOL_V1, fs, map, gametype, kind);
}

/// [`check`] on `proto`'s server, the capture taken on that protocol's
/// retail server.
fn check_on(
    proto: &'static Protocol,
    fs: vcod_common::pk3::Pk3Fs,
    map: &str,
    gametype: &str,
    kind: &str,
) {
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
    let held = vcod_server::defs::Defs::of(proto)
        .weapon_index(&weapon)
        .expect("the joined weapon in CS 7");
    let mut steps = parse_fixture(&text, held as u8);
    // The combat script's `advance` ends wherever its stall turns steered
    // it, and the still `idle` after it turns the view back to the spawn's
    // facing. The step after such a turn starts at retail's settled spot and
    // facing, since what the prone check finds behind the body depends on
    // both (player-model-anim-system.md, "Prone fire").
    for i in 2..steps.len() {
        let prev = &steps[i - 1];
        let still = !prev.walks && prev.base.forward == 0 && prev.base.right == 0;
        let (before, after) = (steps[i - 2].settled_place(), prev.settled_place());
        let turned = matches!((before, after), (Some(b), Some(a)) if b.1 != a.1);
        if still && turned {
            steps[i].place = after;
        }
    }
    // A grenade capture replays from the spot its own script threw from:
    // where a blast lands, and so who it hurts, is the map's business and
    // not the input's (`# grenade` header, AGENTS.md).
    let place = common::captured_place(&text);
    let replayed = common::replay_as(proto, map, gametype, &steps, (&team, &weapon), fs, place);
    let mine: Vec<Vec<Trace>> = replayed
        .iter()
        .map(|step| step.iter().map(|s| trace_of(proto, s, s.ms)).collect())
        .collect();
    // `COMBAT_REPORT=<label>` prints retail's and our trace of that step.
    if let Ok(label) = std::env::var("COMBAT_REPORT") {
        for (step, ours) in steps.iter().zip(&mine) {
            if step.label != label {
                continue;
            }
            for (side, trace) in [("retail", &step.trace), ("ours", ours)] {
                for t in trace {
                    println!(
                        "{side} {kind} {label} {}ms st={:?} ct={:?} state={} anim={} torso={} seq={} frac={:?} spread={:?} delay={:?} view={:?}",
                        t.ms,
                        t.server_time,
                        t.command_time,
                        t.weaponstate,
                        t.weap_anim,
                        t.torso_anim,
                        t.event_sequence,
                        t.pos_frac,
                        t.spread,
                        t.weapon_delay,
                        t.viewangles,
                    );
                }
            }
        }
    }
    let mut bad = Vec::new();
    for ((step, ours), samples) in steps.iter().zip(&mine).zip(&replayed) {
        assert!(
            step.cmds.is_some(),
            "{path}: step {} has no !cmds line; recapture it",
            step.label
        );
        // The probe's stall turns are only in the log, so a walk that logs
        // no view cannot be replayed.
        assert!(
            !step.walks || step.cmds.as_ref().is_some_and(|c| c.views.is_some()),
            "{path}: walk {} logs no a= view; recapture it",
            step.label
        );
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
        let delay_same = rd == od;
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
            let differs = r != o || r_runs != o_runs;
            if differs {
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
        let (r_flips, o_flips) = (torso_flips(&step.trace), torso_flips(ours));
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
                let o = last.ps.field_i32(proto, field);
                if o != *r {
                    bad.push(format!("{}: {field} retail {r}, ours {o}", step.label));
                }
            }
        }
        let view_bad = view_misses(&step.trace, ours);
        match gapped(map, kind, &step.label, "viewangles") {
            Some(why) => assert!(
                !view_bad.is_empty(),
                "{map} {kind} {}: the view matches now; drop the KNOWN_GAPS \
                 entry ({why})",
                step.label
            ),
            None if !view_bad.is_empty() => bad.push(format!(
                "{}: {} of {} samples off\n    {}",
                step.label,
                view_bad.len(),
                step.trace.len(),
                view_bad.join("\n    ")
            )),
            None => {}
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

/// 1.5b's carbine under the same combat script (`--save-combat
/// --capture-tag 15` beside `COD_VERSION=1.5 tools/run_server.sh
/// mp_carentan`, cod15-protocol-diff.md), replayed on a protocol 6 server.
/// Needs `COD_DIR` at a 1.5 install (a UO one's `main/` is 1.5's).
#[test]
fn the_weapon_channel_matches_retail_on_15_mp_carentan() {
    let Some(fs) = vcod_common::testing::game_fs().filter(|fs| {
        vcod_server::server::install_protocol(fs).transport.version == PROTOCOL_V6.transport.version
    }) else {
        return;
    };
    check_on(&PROTOCOL_V6, fs, "mp_carentan", "dm", "combat-15");
}

/// A second 1.5b take whose `advance` walk stalled twice: the probe turned
/// the walk 45 degrees, then 90, and the `!cmds` line's `a=` names the cmd
/// of each turn, so the walk is compared too.
#[test]
fn the_weapon_channel_matches_retail_on_15_mp_carentan_after_a_stall_turn() {
    let Some(fs) = vcod_common::testing::game_fs().filter(|fs| {
        vcod_server::server::install_protocol(fs).transport.version == PROTOCOL_V6.transport.version
    }) else {
        return;
    };
    check_on(&PROTOCOL_V6, fs, "mp_carentan", "dm", "combat-15-stall");
}

/// United Offensive 1.51b's kar98k under the same combat script
/// (`--save-combat --capture-tag uo --probe-team axis` beside
/// `COD_VERSION=uo tools/run_server.sh`, docs/research/coduo-diff.md 20),
/// replayed on a protocol 22 server. Needs `COD_DIR` at a UO install.
#[test]
fn the_weapon_channel_matches_retail_on_uo_mp_cassino() {
    let Some(fs) = vcod_common::testing::uo_fs() else {
        return;
    };
    check_on(&PROTOCOL_V22, fs, "mp_cassino", "dm", "combat-uo");
}

/// A second UO take whose `advance` walk stalled: the probe turned the walk
/// 45 degrees, so `idle` opens turned and its first cmd turns back.
#[test]
fn the_weapon_channel_matches_retail_on_uo_mp_cassino_after_a_stall_turn() {
    let Some(fs) = vcod_common::testing::uo_fs() else {
        return;
    };
    check_on(&PROTOCOL_V22, fs, "mp_cassino", "dm", "combat-uo-stall");
}
