//! `vcod_cod2::player::dobj::traceline` against `cod2_lnxded`'s
//! `DObjTraceline` (0x80bb27e), read under gdb (`fixtures/trace/`, the
//! header the recipe): each dumped trace's line, priority map, limit and
//! posed bones in, its fraction, hit location and normal out, word for word;
//! and `SV_PointTraceToEntity`'s move of the world line into the body's
//! frame (cod2-player-anims.md 12). Needs `COD2_DIR`. `TRACE_DUMP=<path>` reads
//! another dump.

use glam::Vec3;
use vcod_cod2::player::dobj::{self, Mat};
use vcod_common::pk3::Pk3Fs;
use vcod_common::skeleton::Skeleton;

const DUMP: &str = include_str!("fixtures/trace/antilag-gdb.txt");

/// The DObj's parts, grafted by name; the head and helmet keep no boxes
/// (`ignoreCollision`, the dump's mask 0x06).
const MODELS: [&str; 3] = [
    "playerbody_german_africa01",
    "head_german_afrca_christoph",
    "helmet_german_africa",
];

/// `bulletPriorityMap` and `riflePriorityMap` by their lnxded addresses.
const BULLET: (u32, [u8; 19]) = (
    0x0818_7c0c,
    [1, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 0],
);
const RIFLE: (u32, [u8; 19]) = (
    0x0818_7c1f,
    [1, 9, 9, 9, 8, 7, 6, 6, 6, 6, 5, 5, 4, 4, 4, 4, 3, 3, 0],
);

fn word(s: &str) -> u32 {
    u32::from_str_radix(s, 16).unwrap()
}

fn bits(s: &str) -> f32 {
    f32::from_bits(word(s))
}

fn vec3(t: &[&str]) -> [f32; 3] {
    [bits(t[0]), bits(t[1]), bits(t[2])]
}

/// One dumped trace: the caller's world line and the body's axis and
/// origin when `SV_PointTraceToEntity` ran it, the line in the body's
/// frame, the map, the limit, retail's bones (its DObj order) and result
/// words (fraction, +4, normal, part name and location).
struct Trace {
    time: i32,
    point: Option<([f32; 6], [[f32; 3]; 4])>,
    start: [f32; 3],
    end: [f32; 3],
    map: u32,
    limit: f32,
    /// By retail's bone index.
    mats: Vec<(usize, [f32; 8])>,
    result: [u32; 6],
}

fn traces(dump: &str) -> Vec<Trace> {
    let mut out: Vec<Trace> = Vec::new();
    let mut point = None;
    for line in dump.lines().filter(|l| !l.starts_with('#')) {
        let t: Vec<&str> = line.split_whitespace().collect();
        match t[0] {
            "point" => {
                let l: [f32; 6] = std::array::from_fn(|i| bits(t[5 + i]));
                let a: [[f32; 3]; 4] = std::array::from_fn(|r| vec3(&t[12 + 3 * r..]));
                point = Some((l, a));
            }
            "trace" => out.push(Trace {
                time: t[3].parse().unwrap(),
                point: point.take(),
                start: vec3(&t[15..]),
                end: vec3(&t[19..]),
                map: word(t[9]),
                limit: bits(t[23]),
                mats: Vec::new(),
                result: [0; 6],
            }),
            "mat" => {
                let m: [f32; 8] = std::array::from_fn(|i| bits(t[2 + i]));
                out.last_mut()
                    .unwrap()
                    .mats
                    .push((t[1].parse().unwrap(), m));
            }
            "result" => out.last_mut().unwrap().result = std::array::from_fn(|i| word(t[2 + i])),
            _ => {}
        }
    }
    out
}

/// The skeleton as the trace reads it, and retail's bone `i` as our
/// `map[i]` (a later part's namesake is the earlier bone's copy).
fn rig(fs: &Pk3Fs) -> (Skeleton, Vec<usize>) {
    vcod_cod2::model::register();
    let mut models: Vec<_> = MODELS
        .iter()
        .map(|m| vcod_common::model::load(fs, m).unwrap())
        .collect();
    for m in &mut models[1..] {
        for b in &mut m.bones {
            b.hit_mins = glam::Vec3::ZERO;
            b.hit_maxs = glam::Vec3::ZERO;
        }
    }
    let refs: Vec<_> = models.iter().map(|m| (m, None)).collect();
    let skel = Skeleton::build_grafted(&refs);
    let map = (0..models.len())
        .flat_map(|m| skel.model_map(m).to_vec())
        .map(|si| skel.bone_index(&skel.bones()[si].name).unwrap())
        .collect();
    (skel, map)
}

fn dump() -> String {
    match std::env::var("TRACE_DUMP") {
        Ok(p) => std::fs::read_to_string(p).unwrap(),
        Err(_) => DUMP.to_string(),
    }
}

/// What a result's words say: (fraction, hit location, normal), or `None`
/// for a miss (the fraction left at the limit, no part name).
fn retail_hit(t: &Trace) -> Option<(u32, u8, [u32; 3])> {
    let r = t.result;
    (r[5] & 0xffff != 0).then_some((r[0], (r[5] >> 16) as u8, [r[2], r[3], r[4]]))
}

#[test]
fn the_walk_matches_retail_word_for_word() {
    let Some(fs) = vcod_common::testing::cod2_fs() else {
        return;
    };
    let (skel, map) = rig(&fs);
    let traces = traces(&dump());
    assert!(traces.len() >= 20, "{} traces", traces.len());
    let (mut hits, mut off) = (0, Vec::new());
    for (i, t) in traces.iter().enumerate() {
        let mut mats = vec![Mat::ZERO; skel.bones().len()];
        for &(ri, m) in &t.mats {
            mats[map[ri]] = Mat {
                quat: [m[0], m[1], m[2], m[3]],
                trans: [m[4], m[5], m[6]],
                weight: m[7],
            };
        }
        let prio = match t.map {
            m if m == BULLET.0 => &BULLET.1,
            m if m == RIFLE.0 => &RIFLE.1,
            m => panic!("map {m:08x}"),
        };
        let ours = dobj::traceline(&skel, &mats, t.start, t.end, prio, t.limit).map(|h| {
            (
                h.fraction.to_bits(),
                h.hit_location,
                h.normal.map(f32::to_bits),
                skel.bones()[h.bone].name.clone(),
            )
        });
        let retail = retail_hit(t);
        hits += usize::from(retail.is_some());
        let same = match (&ours, &retail) {
            (None, None) => true,
            (Some((f, l, n, _)), Some((rf, rl, rn))) => f == rf && l == rl && n == rn,
            _ => false,
        };
        if !same {
            off.push(format!(
                "trace {i} at {}: ours {ours:x?} retail {retail:x?}",
                t.time
            ));
        }
    }
    for o in &off {
        eprintln!("{o}");
    }
    assert!(hits >= 20, "{hits} hits");
    assert!(
        off.is_empty(),
        "{} of {} traces off",
        off.len(),
        traces.len()
    );
}

/// `SV_PointTraceToEntity`'s line in the body's frame (lnxded 0x80a74f8):
/// each component `p - origin` stored, then its dot with each axis row on
/// the stack, stored; the axis `AnglesToAxis(r.currentAngles)`
/// (0x80a94aa).
#[test]
fn the_line_moves_into_the_body_as_retails() {
    let traces = traces(&dump());
    let mut n = 0;
    for t in &traces {
        let Some((line, axis)) = t.point else {
            continue;
        };
        n += 1;
        let rows = [axis[0], axis[1], axis[2]].map(Vec3::from);
        let origin = Vec3::from(axis[3]);
        let ours =
            |p: [f32; 3]| vcod_common::x87::into_frame_x87(Vec3::from(p), &rows, origin).to_array();
        assert_eq!(
            ours([line[0], line[1], line[2]]).map(f32::to_bits),
            t.start.map(f32::to_bits),
            "start at {}",
            t.time
        );
        assert_eq!(
            ours([line[3], line[4], line[5]]).map(f32::to_bits),
            t.end.map(f32::to_bits),
            "end at {}",
            t.time
        );
    }
    assert!(n >= 20);
}
