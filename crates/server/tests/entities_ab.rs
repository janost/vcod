//! The entity list, diffed against a retail trace. Stage 5's content gate:
//! every entity the trace shows retail sending has to exist in ours with the
//! same fields.
//!
//! The fixtures are traces, not settled captures: `--net-probe
//! --save-entities` walks a route and records `(origin, entity set)` per
//! station, because a capture cannot be told where to stand (`setviewpos` is
//! refused on a dedicated server) and the entity list depends on position
//! (`docs/protocol-1.1.md`, "Which entities a client is sent"). This gate uses
//! the union of the samples; the per-sample sets are the visibility gate's
//! subject, and that one needs our cull to exist first.
//!
//! Directional on purpose: a trace cannot prove its union is the whole map, so
//! an entity of ours the trace never saw reads as an extra rather than as a
//! gap in the capture. Extras are still asserted against an empty allowance,
//! so a failure forces the question to be answered rather than assumed.
//!
//! Needs `COD_DIR`; without the paks it returns early.

mod common;

use common::{header_value, Queues};
use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::rc::Rc;
use std::time::Instant;
use vcod_common::net::msg::EntityState;
use vcod_common::net::protocol::{Protocol, PROTOCOL_V1};

/// Entities stage 5 knowingly does not reproduce, each with the reason. Empty
/// is the goal, and a listed gap that starts matching fails the guard below,
/// so the list cannot rot into a lie.
const GAPS: &[(&str, &str)] = &[];

/// Fields stage 5 knowingly does not reproduce, as `(map, field, reason)`.
/// Same rule as `GAPS`: a listed field that stops differing on its own map
/// fails the guard, so this cannot rot either. Scoped to a map because a gap
/// only exists where the entity that carries it does -- pavlov places no
/// turret, so a turret's field matches there by having nothing to differ on.
const FIELD_GAPS: &[(&str, &str, &str)] = &[(
    "mp_carentan",
    "angles2[0]",
    "a turret's, -63 on one carentan turret and -72 on the other. It is not \
     spawn state: the helper after `turret_think_client` (0x524cc) does \
     `angles2[0] += angles2[2]` every think, so it accumulates. What sets the \
     value it accumulates from is not found -- not the Radiant yaw (295 and \
     229), not the weapon file's arcs (45/45/40/40), not any constant in \
     `G_SpawnTurret` (-90, -32, 32, 56). It reads the same in every sample of \
     four independent server runs at different uptimes, so whatever writes it \
     settles",
)];

/// How far a float field may sit from the capture's. Every entity field with
/// `bits == 0` is a float, and two of ours differ from retail's by a bounded
/// amount with one cause: retail drops an item with `trap_TraceCapsule` where
/// we box-trace, so its resting z and the surface normal it aligns to are a
/// little different. Measured maxima across both maps are 0.024 units of z and
/// 0.035 degrees of angle; this bound is twice that, and it is far under the
/// errors the gate exists to catch -- an item floating at the mapper's z is
/// five units out and an unwrapped roll is 360 degrees out.
///
/// It also covers `+0.0` against `-0.0`, which is what a flat floor's pitch
/// comes to on either side.
const FLOAT_EPS: f32 = 0.05;

/// Entities we send that no sample in the trace shows retail sending. Empty,
/// and a new one is a question to answer, not a line to add: either our server
/// is sending something retail does not, or the trace never walked the corner
/// that entity stands in, and only one of those is fine.
const EXTRAS_ALLOWED: &[(&str, &str)] = &[];

fn cfg(map: &str) -> vcod_server::ServerConfig {
    vcod_server::ServerConfig {
        map: map.into(),
        hostname: "vcod test".into(),
        // The captures were taken at 8, and each fixture header is checked
        // against this.
        max_clients: 8,
        gametype: "dm".into(),
        test_entities: 0,
        trace: false,
    }
}

// ---------------------------------------------------------------- the fixture

/// One station: where the probe stood, and every entity the server had sent
/// it as of that frame.
struct Sample {
    label: String,
    origin: [f32; 3],
    ents: BTreeMap<u32, EntityState>,
}

/// A retail trace, and the join it was taken with.
struct Trace {
    team: String,
    weapon: String,
    samples: Vec<Sample>,
}

impl Trace {
    /// Every entity any sample carried, by entity number. An entity in two
    /// samples is the same entity, so the first one wins and a disagreement
    /// between samples is a defect in the capture worth failing on.
    fn union(&self, path: &str) -> BTreeMap<u32, EntityState> {
        let mut out: BTreeMap<u32, EntityState> = BTreeMap::new();
        for s in &self.samples {
            for (num, e) in &s.ents {
                match out.get(num) {
                    Some(prev) => assert_eq!(
                        prev.fields, e.fields,
                        "{path}: entity {num} differs between samples, so the trace \
                         disagrees with itself (sample {:?} is the second one)",
                        s.label
                    ),
                    None => {
                        out.insert(*num, e.clone());
                    }
                }
            }
        }
        out
    }

    /// The distinct entity sets across the samples. Fewer than two means the
    /// trace cannot tell a server that culls from one that sends everything.
    fn distinct_sets(&self) -> usize {
        self.samples
            .iter()
            .map(|s| s.ents.keys().copied().collect::<Vec<_>>())
            .collect::<BTreeSet<_>>()
            .len()
    }
}

/// The committed retail trace. The fixture name carries the gametype, so it
/// comes from `cfg` rather than a literal: a changed `cfg().gametype` has to
/// move the fixture too, not silently keep reading the `dm` capture.
fn retail(map: &str) -> (Trace, String) {
    let path = format!("tests/fixtures/entities/{map}-{}.txt", cfg(map).gametype);
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {path}: {e}"));
    let head = |k: &str| header_value(&text, k, &path);
    assert_eq!(head("map"), map, "{path}: header names another map");
    assert_eq!(
        head("g_gametype"),
        cfg(map).gametype,
        "{path}: header names another gametype"
    );
    assert_eq!(
        head("sv_maxclients"),
        cfg(map).max_clients.to_string(),
        "{path}: header names another sv_maxclients, which moves script state"
    );

    let p = &PROTOCOL_V1;
    let mut samples: Vec<Sample> = Vec::new();
    let mut ent: Option<u32> = None;
    for line in text.lines() {
        if line.starts_with('#') || line.is_empty() {
            continue;
        }
        if let Some(rest) = line.strip_prefix("[sample ") {
            let label = rest.split_once("] ").map(|(_, l)| l).unwrap_or("");
            samples.push(Sample {
                label: label.to_string(),
                origin: [0.0; 3],
                ents: BTreeMap::new(),
            });
            ent = None;
            continue;
        }
        let s = samples
            .last_mut()
            .unwrap_or_else(|| panic!("{path}: a line before the first [sample]: {line}"));
        if let Some(num) = line
            .strip_prefix("[ent ")
            .and_then(|r| r.strip_suffix(']'))
            .and_then(|n| n.parse::<u32>().ok())
        {
            let mut e = EntityState::null(p);
            e.number = num;
            s.ents.insert(num, e);
            ent = Some(num);
            continue;
        }
        let (name, v) = line
            .split_once(' ')
            .unwrap_or_else(|| panic!("{path}: bad line: {line}"));
        let v: i32 = v
            .parse()
            .unwrap_or_else(|e| panic!("{path}: {name} is not an i32: {e}"));
        match ent {
            // A field inside an [ent] block.
            Some(num) => {
                let idx = EntityState::field_index(p, name)
                    .unwrap_or_else(|| panic!("{path}: {name} is not an entity field"));
                s.ents.get_mut(&num).expect("the block's entity").fields[idx] = v;
            }
            // A sample-level field: the origin the gate replays, and the
            // serverTime it was read at.
            None => match name {
                "origin[0]" => s.origin[0] = f32::from_bits(v as u32),
                "origin[1]" => s.origin[1] = f32::from_bits(v as u32),
                "origin[2]" => s.origin[2] = f32::from_bits(v as u32),
                "serverTime" => {}
                _ => panic!("{path}: {name} is not a sample field"),
            },
        }
    }

    let trace = Trace {
        team: head("joined").to_string(),
        weapon: head("weapon").to_string(),
        samples,
    };
    assert!(
        trace.samples.len() >= 2,
        "{path}: {} samples; a trace needs several positions to say anything",
        trace.samples.len()
    );
    // The property the capture exists for. A fixture whose samples all carry
    // one set is passed by a server that sends every entity to everyone, which
    // is the bug the visibility gate is here to catch.
    assert!(
        trace.distinct_sets() >= 2,
        "{path}: every sample carries the same entity set, so this trace cannot \
         fail a server that sends everything; walk it again for a route that \
         crosses a boundary"
    );
    (trace, path)
}

/// Our entity list at the same shape the trace's was read: scripts loaded,
/// one client joined the way the capture joined, and the list it was sent.
fn ours(
    map: &str,
    trace: &Trace,
    fs: vcod_common::pk3::Pk3Fs,
    bsp: &vcod_common::bsp::Bsp,
) -> BTreeMap<u32, EntityState> {
    let fs = Rc::new(fs);
    let mut now = Instant::now();
    let mut sv = vcod_server::Server::new(cfg(map), now);
    sv.load_world(vcod_server::world::World::from_bsp(bsp));
    sv.load_scripts(fs).expect("load the scripts");

    let q = Rc::new(RefCell::new(Queues::default()));
    let (cl, _join) = common::join(&mut sv, &q, &mut now, &trace.team, &trace.weapon);
    assert!(
        cl.snapshots().newest().is_some(),
        "the server sent no snapshot at all"
    );
    // Not the snapshot's list: that one is already culled to where this client
    // happens to stand.
    sv.all_entities()
}

// ------------------------------------------------------------------- the diff

/// What matches an entity across two servers. Never the slot: the number comes
/// out of each server's own spawn order. Stage 2 pinned ours against a retail
/// capture and they agreed, so the numbers are checked separately, as a
/// finding rather than as the key.
#[derive(PartialEq, Eq, PartialOrd, Ord, Debug)]
struct Key {
    etype: i32,
    index: i32,
    /// Quantized to 1 unit: a float that survived a wire round trip is not
    /// bit-identical to the one that produced it.
    origin: [i32; 3],
}

fn key(p: &Protocol, e: &EntityState) -> Key {
    let at = |axis: usize| e.field_f32(p, &format!("pos.trBase[{axis}]")).round() as i32;
    Key {
        etype: e.field_i32(p, "eType"),
        index: e.field_i32(p, "index"),
        origin: [at(0), at(1), at(2)],
    }
}

fn describe(p: &Protocol, num: u32, e: &EntityState) -> String {
    let k = key(p, e);
    format!(
        "ent {num} eType {} index {} at [{}, {}, {}]",
        k.etype, k.index, k.origin[0], k.origin[1], k.origin[2]
    )
}

fn compare(map: &str, retail: &BTreeMap<u32, EntityState>, ours: &BTreeMap<u32, EntityState>) {
    let p = &PROTOCOL_V1;
    let mut findings: Vec<String> = Vec::new();
    let mut matched: BTreeSet<u32> = BTreeSet::new();
    let mut gaps_hit: BTreeSet<&str> = BTreeSet::new();
    let mut field_gaps_hit: BTreeSet<&str> = BTreeSet::new();

    for (num, want) in retail {
        let found = ours.iter().find(|(_, got)| key(p, got) == key(p, want));
        let Some((got_num, got)) = found else {
            let d = describe(p, *num, want);
            match GAPS.iter().find(|(g, _)| d.starts_with(g)) {
                Some((g, _)) => {
                    gaps_hit.insert(g);
                }
                None => findings.push(format!("missing: {d}")),
            }
            continue;
        };
        matched.insert(*got_num);
        if got_num != num {
            findings.push(format!(
                "number: {} is entity {got_num} here",
                describe(p, *num, want)
            ));
        }
        for (i, f) in p.entity_fields.iter().enumerate() {
            let (a, b) = (got.fields[i], want.fields[i]);
            if a == b {
                continue;
            }
            if FIELD_GAPS.iter().any(|(m, g, _)| *m == map && *g == f.name) {
                field_gaps_hit.insert(f.name);
                continue;
            }
            // `bits == 0` is this codec's float (docs/protocol-1.1.md, "Delta
            // field encoding"), and a float gets the bounded tolerance above.
            if f.bits == 0 {
                let (x, y) = (f32::from_bits(a as u32), f32::from_bits(b as u32));
                if (x - y).abs() <= FLOAT_EPS {
                    continue;
                }
                findings.push(format!(
                    "field: {} {} is {x} here, {y} in the capture",
                    describe(p, *num, want),
                    f.name,
                ));
                continue;
            }
            findings.push(format!(
                "field: {} {} is {a} here, {b} in the capture",
                describe(p, *num, want),
                f.name,
            ));
        }
    }

    for (num, got) in ours {
        if matched.contains(num) {
            continue;
        }
        let d = describe(p, *num, got);
        if EXTRAS_ALLOWED.iter().any(|(a, _)| d.starts_with(a)) {
            continue;
        }
        findings.push(format!("extra: {d}"));
    }

    // A gap that starts matching is a lie the list can no longer tell.
    for (g, why) in GAPS {
        assert!(
            gaps_hit.contains(g),
            "{map}: GAPS lists {g:?} ({why}) but it is not missing any more; drop it"
        );
    }
    for (m, g, why) in FIELD_GAPS {
        if *m != map {
            continue;
        }
        assert!(
            field_gaps_hit.contains(g),
            "{map}: FIELD_GAPS lists {g:?} ({why}) but it matches everywhere now; drop it"
        );
    }

    assert!(
        findings.is_empty(),
        "{map}: {} findings against the retail trace ({} entities in the capture, {} in ours)\n{}",
        findings.len(),
        retail.len(),
        ours.len(),
        findings.join("\n"),
    );
}

fn check(map: &str) {
    let Some(fs) = vcod_common::testing::game_fs() else {
        eprintln!("COD_DIR unset or has no main/: skipping {map}");
        return;
    };
    let bsp_path = fs.resolve_map(map).expect("map in the mounted paks");
    let bsp_bytes = fs.read(&bsp_path).expect("read the bsp");
    let bsp = vcod_common::bsp::parse(&bsp_bytes).expect("parse the bsp");
    let (trace, path) = retail(map);
    let want = trace.union(&path);
    let got = ours(map, &trace, fs, &bsp);
    compare(map, &want, &got);
}

#[test]
fn carentan_entities_match_retail() {
    check("mp_carentan");
}

#[test]
fn pavlov_entities_match_retail() {
    check("mp_pavlov");
}
