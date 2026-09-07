//! The grenade's flight against the retail capture: one `!missile` line per
//! snapshot in `mp_carentan-tdm-grenade.txt`, replayed through our server
//! from the spot the capture's own header says the probe threw from.
//!
//! What is compared is the flight, not the release: how many bounces the
//! contact loop produced, where the arc had reached at each of retail's
//! samples, when the fuse went off and where and how it came to rest. When
//! the throw itself happened is `playerstate_combat_ab`'s business, and the
//! two flights are aligned on the frame the missile first reached the wire
//! so a release a frame out does not read as a flight 48 units out.
//!
//! The event parms are compared too: a bounce off a prop carries the
//! surface type its xmodel collision surface names (21 on the two throws
//! that land on the same crate stack), and the explode's 16-unit downward
//! `trap_Trace` sees no static model, so a frag resting on a crate packs a
//! zero normal (parm 0) where one on world brush packs 5.
//!
//! Needs `COD_DIR`; without the paks it returns early.

mod common;

use common::{parse_fixture, replay, Sample};
use std::collections::{BTreeMap, BTreeSet};
use vcod_common::net::msg::EntityState;
use vcod_common::net::protocol::PROTOCOL_V1;
use vcod_common::net::trajectory::{Trajectory, TR_STATIONARY};

/// How far our arc may sit from retail's at one of its samples. A bounce
/// resolved a millisecond earlier or later moves the rest of the flight by
/// about this much, and the impact time retail interpolates is whole
/// milliseconds (`docs/research/cod11-combat.md` 12.4).
const POS_TOL: f32 = 8.0;
/// How far our explode may sit from retail's, in ms: one server frame.
const EXPLODE_TOL_MS: i32 = 50;
/// How far our resting height may sit from retail's.
const REST_TOL: f32 = 1.0;

const ET_MISSILE: i32 = 4;
const EV_GRENADE_BOUNCE: i32 = 177;
const EV_GRENADE_EXPLODE: i32 = 178;

/// One missile as a snapshot carries it, on either side of the comparison.
#[derive(Clone)]
struct MissileSample {
    /// ms from the first snapshot of the step.
    rel_ms: i32,
    /// The snapshot's own `serverTime`, which is what the trajectory is
    /// evaluated against.
    server_time: i32,
    e_type: i32,
    pos: Trajectory,
    apos: Trajectory,
    events: [i32; 4],
    event_parms: [i32; 4],
    event_sequence: i32,
}

impl MissileSample {
    fn at(&self, server_time: i32) -> glam::Vec3 {
        self.pos.evaluate(server_time)
    }
}

/// The `!missile` line writes each ring as one comma-separated field.
fn four(raw: &str) -> [i32; 4] {
    let mut out = [0; 4];
    for (i, v) in raw.split(',').enumerate().take(4) {
        out[i] = v.parse().expect("a number in an event ring");
    }
    out
}

/// The `!missile` lines of each step, keyed by the step label and then by the
/// entity number, with each line's snapshot `serverTime` taken from the
/// `!trace` line it follows.
fn retail_missiles(text: &str) -> Vec<(String, BTreeMap<u32, Vec<MissileSample>>)> {
    let mut out: Vec<(String, BTreeMap<u32, Vec<MissileSample>>)> = Vec::new();
    let (mut server_time, mut first) = (0, None);
    for line in text.lines() {
        if let Some(label) = line
            .strip_prefix("[step ")
            .and_then(|l| l.strip_suffix(']'))
        {
            out.push((label.to_string(), BTreeMap::new()));
            first = None;
            continue;
        }
        let kv = |rest: &str| -> BTreeMap<String, String> {
            rest.split_whitespace()
                .filter_map(|t| t.split_once('='))
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect()
        };
        if let Some(rest) = line.strip_prefix("!trace ") {
            server_time = kv(rest)["serverTime"].parse().unwrap();
            first.get_or_insert(server_time);
            continue;
        }
        let Some(rest) = line.strip_prefix("!missile ") else {
            continue;
        };
        let m = kv(rest);
        let i = |k: &str| m[k].parse::<i32>().unwrap();
        let traj = |k: &str| {
            let f: Vec<f32> = m[k].split(',').map(|v| v.parse().unwrap()).collect();
            Trajectory {
                tr_type: f[0] as i32,
                tr_time: f[1] as i32,
                tr_duration: 0,
                base: glam::Vec3::new(f[2], f[3], f[4]),
                delta: glam::Vec3::new(f[5], f[6], f[7]),
            }
        };
        let sample = MissileSample {
            rel_ms: server_time - first.unwrap_or(server_time),
            server_time,
            e_type: i("eType"),
            pos: traj("pos"),
            apos: traj("apos"),
            events: four(&m["events"]),
            event_parms: four(&m["eventParms"]),
            event_sequence: i("eventSequence"),
        };
        out.last_mut()
            .expect("a !missile line before any [step]")
            .1
            .entry(i("num") as u32)
            .or_default()
            .push(sample);
    }
    out
}

/// The same, off our own snapshots: an entity is watched from the first
/// frame it reads `eType` 4 until it leaves the wire, because the explode
/// flips that field to 0 (`docs/research/cod11-combat.md` section 13).
fn our_missiles(steps: &[Vec<Sample>]) -> Vec<BTreeMap<u32, Vec<MissileSample>>> {
    let p = &PROTOCOL_V1;
    let mut seen: BTreeSet<u32> = BTreeSet::new();
    let mut out = Vec::new();
    for step in steps {
        let mut per_step: BTreeMap<u32, Vec<MissileSample>> = BTreeMap::new();
        let first = step.first().map_or(0, |s| s.server_time);
        for snap in step {
            seen.retain(|n| snap.entities.contains_key(n));
            for (n, e) in &snap.entities {
                if e.field_i32(p, "eType") == ET_MISSILE {
                    seen.insert(*n);
                }
            }
            for num in &seen {
                let Some(e) = snap.entities.get(num) else {
                    continue;
                };
                per_step.entry(*num).or_default().push(ours_sample(
                    e,
                    snap.server_time - first,
                    snap.server_time,
                ));
            }
        }
        out.push(per_step);
    }
    out
}

fn ours_sample(e: &EntityState, rel_ms: i32, server_time: i32) -> MissileSample {
    let p = &PROTOCOL_V1;
    let ev = |n: &str, i: usize| e.field_i32(p, &format!("{n}[{i}]"));
    MissileSample {
        rel_ms,
        server_time,
        e_type: e.field_i32(p, "eType"),
        pos: Trajectory::read(e, p, "pos"),
        apos: Trajectory::read(e, p, "apos"),
        events: [
            ev("events", 0),
            ev("events", 1),
            ev("events", 2),
            ev("events", 3),
        ],
        event_parms: [
            ev("eventParms", 0),
            ev("eventParms", 1),
            ev("eventParms", 2),
            ev("eventParms", 3),
        ],
        event_sequence: e.field_i32(p, "eventSequence"),
    }
}

/// The new ring slots between consecutive samples that carry `event`. The
/// counter is bumped after the slot is written, so the new slots are
/// `prev ..= cur - 1` (`vcod_common::net::events::seq_diff`).
fn ring_events(samples: &[MissileSample], event: i32) -> usize {
    let first = samples.first().map_or(0, |s| {
        (0..s.event_sequence.min(4))
            .filter(|i| s.events[*i as usize] == event)
            .count()
    });
    first
        + samples
            .windows(2)
            .map(|w| {
                let diff = ((w[1].event_sequence - w[0].event_sequence) & 0xff).min(4);
                (0..diff)
                    .filter(|i| w[1].events[((w[0].event_sequence + i) & 3) as usize] == event)
                    .count()
            })
            .sum::<usize>()
}

/// When the explode event reached the ring, in ms from the step's first
/// snapshot, and the sample that carried it.
fn explode_at(samples: &[MissileSample]) -> Option<i32> {
    samples
        .iter()
        .find(|s| {
            s.e_type == 0 && s.events[((s.event_sequence - 1) & 3) as usize] == EV_GRENADE_EXPLODE
        })
        .map(|s| s.rel_ms)
}

#[test]
fn a_thrown_grenade_flies_bounces_and_explodes_like_retail() {
    let Some(fs) = vcod_common::testing::game_fs() else {
        return;
    };
    let (map, gametype) = ("mp_carentan", "tdm");
    let path = format!(
        "{}/tests/fixtures/playerstate/{map}-{gametype}-grenade.txt",
        env!("CARGO_MANIFEST_DIR")
    );
    let text = std::fs::read_to_string(&path).unwrap();
    let header = common::grenade_header(&text).expect("the capture's `# grenade` header line");
    let weapon = common::header_value(&text, "weapon", &path).to_string();
    let team = common::header_value(&text, "joined", &path).to_string();
    let held =
        vcod_server::configstrings::weapon_index(&weapon).expect("the joined weapon in CS 7");
    let steps = parse_fixture(&text, held as u8);

    let origin = common::header_vec3(&header, "origin");
    let yaw = common::header_vec3(&header, "viewangles")[1];
    let mine = replay(
        map,
        gametype,
        &steps,
        (&team, &weapon),
        fs,
        Some((origin, yaw)),
    );
    let ours = our_missiles(&mine);
    let retail = retail_missiles(&text);

    let mut bad = Vec::new();
    // A missile is compared in the step it first appeared in; the two later
    // steps that carry the tail of an earlier throw are the same flight.
    let mut done: BTreeSet<usize> = BTreeSet::new();
    for (i, (label, per_num)) in retail.iter().enumerate() {
        let Some((_, r)) = per_num.iter().next() else {
            continue;
        };
        // Only the step the throw happened in. A flight that started in an
        // earlier step is already on the wire at the step's first snapshot;
        // a fresh throw arrives partway in.
        if r.first().map(|s| s.rel_ms) == Some(0) {
            continue;
        }
        let Some(o) = ours[i].values().find(|o| o[0].rel_ms > 0) else {
            bad.push(format!(
                "{label}: retail put a grenade on the wire, ours none"
            ));
            continue;
        };
        done.insert(i);
        // The flights are aligned on the frame each first reached the wire.
        let shift = o[0].rel_ms - r[0].rel_ms;
        let (rb, ob) = (
            ring_events(r, EV_GRENADE_BOUNCE),
            ring_events(o, EV_GRENADE_BOUNCE),
        );
        if rb != ob {
            bad.push(format!("{label}: retail bounced {rb} times, ours {ob}"));
        }
        match (explode_at(r), explode_at(o)) {
            (Some(re), Some(oe)) if (re + shift - oe).abs() > EXPLODE_TOL_MS => bad.push(format!(
                "{label}: retail exploded {re} ms into the step, ours {oe} (shift {shift})"
            )),
            (Some(_), None) => bad.push(format!("{label}: ours never exploded")),
            (None, Some(_)) => bad.push(format!("{label}: ours exploded and retail did not")),
            _ => {}
        }
        // Per shared sample, where the arc had reached.
        let mut worst = (0.0f32, String::new());
        for rs in r {
            let want = rs.at(rs.server_time);
            let Some(os) = o.iter().rev().find(|os| os.rel_ms <= rs.rel_ms + shift) else {
                continue;
            };
            let got = os.at(os.server_time + (rs.rel_ms + shift - os.rel_ms));
            let d = (want - got).length();
            if d > worst.0 {
                worst = (
                    d,
                    format!(
                        "{label}: at {} ms retail {want:?} ours {got:?} ({d:.1} apart)",
                        rs.rel_ms
                    ),
                );
            }
        }
        // The ring the flight ended on, events and parms.
        let ring = |ss: &[MissileSample]| {
            ss.last()
                .map(|s| (s.events, s.event_sequence, s.event_parms))
                .unwrap_or_default()
        };
        let parms = |ss: &[MissileSample]| ss.last().map(|s| s.event_parms).unwrap_or_default();
        if ring(r) != ring(o) {
            bad.push(format!(
                "{label}: retail's ring ended {:?} ours {:?}",
                ring(r),
                ring(o)
            ));
        }
        eprintln!(
            "{label}: ring {:?} parms retail {:?} ours {:?}",
            ring(r).0,
            parms(r),
            parms(o)
        );
        eprintln!(
            "{label}: bounces retail {rb} ours {ob}, explode retail {:?} ours {:?} \
             (shift {shift} ms), worst position {:.1} units",
            explode_at(r),
            explode_at(o),
            worst.0
        );
        if worst.0 > POS_TOL {
            bad.push(worst.1);
        }
        // The blast the explode left for the radius damage pass, on the
        // frame the event reached the wire.
        if let Some(oe) = explode_at(o) {
            let blasts: usize = mine[i]
                .iter()
                .filter(|s| s.server_time - mine[i][0].server_time == oe)
                .map(|s| s.explosions)
                .sum();
            if blasts != 1 {
                bad.push(format!(
                    "{label}: the explode frame offered {blasts} blasts to the damage pass, not 1"
                ));
            }
        }
        // The rest: same trajectory type and height, at the last sample
        // before either exploded.
        let (rl, ol) = (
            r.iter().take_while(|s| s.e_type == ET_MISSILE).last(),
            o.iter().take_while(|s| s.e_type == ET_MISSILE).last(),
        );
        if let (Some(rl), Some(ol)) = (rl, ol) {
            if rl.pos.tr_type != ol.pos.tr_type {
                bad.push(format!(
                    "{label}: retail settled at trType {}, ours {}",
                    rl.pos.tr_type, ol.pos.tr_type
                ));
            }
            if rl.pos.tr_type == TR_STATIONARY && (rl.pos.base.z - ol.pos.base.z).abs() > REST_TOL {
                bad.push(format!(
                    "{label}: retail rested at z {}, ours {}",
                    rl.pos.base.z, ol.pos.base.z
                ));
            }
            eprintln!(
                "{label}: rest retail trType {} z {} ours trType {} z {}",
                rl.pos.tr_type, rl.pos.base.z, ol.pos.tr_type, ol.pos.base.z
            );
            if rl.apos.tr_type != ol.apos.tr_type {
                bad.push(format!(
                    "{label}: retail's angles settled at trType {}, ours {}",
                    rl.apos.tr_type, ol.apos.tr_type
                ));
            }
        }
    }
    assert!(
        done.len() >= 3,
        "the capture's three throws were not all found: {done:?}"
    );
    assert!(
        bad.is_empty(),
        "the grenade's flight differs from retail on {map}:\n  {}",
        bad.join("\n  ")
    );
}
