//! The struck target's pose against the engines' own, read under gdb
//! (`fixtures/skel11/`, each header the recipe): the anim calc's records
//! off the clips' node times and weights, and the skeleton build off
//! retail's records, word for word (cod11-combat.md 3.1.3). Needs a 1.1 `COD_DIR`, and a 1.5 one for the 1.5b dump.

use vcod_common::model;
use vcod_common::net::protocol::{PROTOCOL_V6, install_protocol};
use vcod_common::patch::Arith;
use vcod_common::pk3::Pk3Fs;
use vcod_common::skeleton::{
    Blend, PoseBuffer, Record, Skeleton, StoredBone, build_kept, build_stored,
};
use vcod_common::xanim::{self, XAnim};

const DUMP_11: &str = include_str!("fixtures/skel11/mp_carentan-hitbody2-gdb.txt");
const DUMP_15: &str = include_str!("fixtures/skel11/mp_carentan-hitbody15-kill-gdb.txt");

/// The target's DObj parts in order, each grafted by name; the head is the
/// dump's own.
fn models(head: &str) -> [&str; 5] {
    [
        "playerbody_german_fallschirmjagercamo",
        head,
        "gear_german_helmet_falshrm_blk",
        "gear_german_load1_falshrm",
        "gear_german_bandolier",
    ]
}

fn bits(s: &str) -> f32 {
    f32::from_bits(u32::from_str_radix(s, 16).unwrap())
}

/// One traced call: the control mask, the legs node's clips in the walk's
/// order (name, node time, weight), the records at the build's entry, and
/// per bone the matrix (rows, origin) and quaternion the build left.
struct Call {
    ctrl: Vec<u8>,
    clips: Vec<(String, f32, f32)>,
    pre: Vec<[f32; 8]>,
    post: Vec<[f32; 16]>,
}

impl Call {
    fn control(&self, i: usize) -> bool {
        self.ctrl[i / 8] >> (i % 8) & 1 != 0
    }
}

fn calls(dump: &str) -> Vec<Call> {
    let mut out: Vec<Call> = Vec::new();
    for line in dump.lines().filter(|l| !l.starts_with('#')) {
        let t: Vec<&str> = line.split_whitespace().collect();
        if t[0] == "call" {
            out.push(Call {
                ctrl: Vec::new(),
                clips: Vec::new(),
                pre: Vec::new(),
                post: Vec::new(),
            });
            continue;
        }
        let c = out.last_mut().unwrap();
        match t[0] {
            "ctrl" => {
                c.ctrl = (0..16)
                    .map(|i| u8::from_str_radix(&t[1][2 * i..2 * i + 2], 16).unwrap())
                    .collect();
            }
            "clip" => c.clips.push((t[1].into(), bits(t[2]), bits(t[3]))),
            "pre" => c.pre.push(std::array::from_fn(|i| bits(t[2 + i]))),
            "post" => c.post.push(std::array::from_fn(|i| bits(t[2 + i]))),
            _ => panic!("unknown line {line}"),
        }
    }
    out
}

/// The rig as the dump's DObj lays it out: retail's bone `i` (the parts'
/// bones end to end) is our `map[i]`.
fn rig(fs: &Pk3Fs, head: &str) -> (Skeleton, Vec<usize>) {
    let models: Vec<model::Model> = models(head)
        .iter()
        .map(|m| model::load(fs, m).unwrap())
        .collect();
    let refs: Vec<(&model::Model, Option<&str>)> = models.iter().map(|m| (m, None)).collect();
    let skel = Skeleton::build_grafted(&refs);
    let map = (0..models.len())
        .flat_map(|m| skel.model_map(m).to_vec())
        .collect();
    (skel, map)
}

/// Every bone retail's anim calc keyed off `call.clips` under the legs
/// node (the controllers' bones aside, which the game module sets after),
/// against our walk's records under `arith`: the words that differ.
fn records_off(
    skel: &Skeleton,
    map: &[usize],
    call: &Call,
    fs: &Pk3Fs,
    arith: Arith,
) -> Vec<String> {
    let anims: Vec<XAnim> = call
        .clips
        .iter()
        .map(|(n, ..)| xanim::load(fs, n).unwrap())
        .collect();
    let binds: Vec<_> = anims.iter().map(|a| skel.bind(a)).collect();
    let legs = anims
        .iter()
        .zip(&binds)
        .zip(&call.clips)
        .map(|((a, b), (_, t, w))| {
            let frame = f64::from(*t) * f64::from(a.frame_count.saturating_sub(1));
            Blend::Clip((a, b, frame, *w))
        })
        .collect();
    let mut pose = PoseBuffer::new_in(skel, arith);
    pose.blend_tree(&[Blend::Node(1.0, legs), Blend::Node(0.0, Vec::new())]);
    let ours = pose.records().unwrap();
    let mut off = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for (i, &si) in map.iter().enumerate() {
        if call.control(i) || !seen.insert(si) {
            continue;
        }
        let r = ours[si];
        let o = [
            r.q[0], r.q[1], r.q[2], r.q[3], r.weight, r.t[0], r.t[1], r.t[2],
        ];
        for (w, (a, b)) in o.iter().zip(call.pre[i]).enumerate() {
            if a.to_bits() != b.to_bits() {
                off.push(format!(
                    "bone {i} ({}) word {w}: ours {a:e} retail {b:e}",
                    skel.bones()[si].name
                ));
            }
        }
    }
    off
}

/// `build` over retail's records against the matrices, origins and
/// quaternions retail's build left: the words compared and those that
/// differ.
fn build_off(
    skel: &Skeleton,
    map: &[usize],
    call: &Call,
    build: fn(&Skeleton, &[Record]) -> Vec<StoredBone>,
) -> (usize, Vec<String>) {
    // Each of our bones takes the record of the first retail bone that is
    // it; a later part's copy is the copy retail's build makes. A second
    // box our rig hangs off its bone (`extra_box`) rides it at identity.
    let mut records = vec![
        Record {
            q: [0.0, 0.0, 0.0, 1.0],
            weight: 0.0,
            t: [0.0; 3],
            control: false,
        };
        skel.bone_count()
    ];
    let mut seen = std::collections::HashSet::new();
    for (i, &si) in map.iter().enumerate() {
        if seen.insert(si) {
            let p = call.pre[i];
            records[si] = Record {
                q: [p[0], p[1], p[2], p[3]],
                weight: p[4],
                t: [p[5], p[6], p[7]],
                control: call.control(i),
            };
        }
    }
    let built = build(skel, &records);
    let (mut words, mut off) = (0, Vec::new());
    for (i, &si) in map.iter().enumerate() {
        let b = &built[si];
        let ours = b
            .axes
            .iter()
            .flat_map(|a| a.to_array())
            .chain(b.origin.to_array())
            .chain(b.q);
        for (w, (o, r)) in ours.zip(call.post[i]).enumerate() {
            words += 1;
            if o.to_bits() != r.to_bits() {
                off.push(format!(
                    "bone {i} ({}) word {w}: ours {o:e} retail {r:e}",
                    skel.bones()[si].name
                ));
            }
        }
    }
    (words, off)
}

fn none_off(what: &str, off: &[String]) {
    assert!(
        off.is_empty(),
        "{what}: {} words off:\n{}",
        off.len(),
        off.iter().take(40).cloned().collect::<Vec<_>>().join("\n")
    );
}

/// 1.1d's four idle frames and the knockback kill, whose legs blend the
/// run (anim 94) in at 0.001 before the idle (anim 122): the walk visits
/// them in anim index order, the order the pose sorts its layers into.
#[test]
fn the_targets_records_match_11ds_word_for_word() {
    let Some(fs) = vcod_common::testing::game_fs_11() else {
        return;
    };
    let (skel, map) = rig(&fs, "basehead_axis_b_02");
    let calls = calls(DUMP_11);
    assert_eq!(calls.len(), 5);
    for (ci, call) in calls.iter().enumerate() {
        none_off(
            &format!("call {ci}"),
            &records_off(&skel, &map, call, &fs, Arith::Kept),
        );
    }
}

#[test]
fn the_targets_skeleton_matches_11ds_word_for_word() {
    let Some(fs) = vcod_common::testing::game_fs_11() else {
        return;
    };
    let (skel, map) = rig(&fs, "basehead_axis_b_02");
    let calls = calls(DUMP_11);
    let mut words = 0;
    for (ci, call) in calls.iter().enumerate() {
        let (w, off) = build_off(&skel, &map, call, build_kept);
        words += w;
        none_off(&format!("call {ci}"), &off);
    }
    assert_eq!(words, 5 * 113 * 16);
}

/// 1.5b's knockback kill: the stored walk over the two clips in the order
/// retail's sampler took them (the run first, `0x80c47de`'s call log), and
/// its stored build (cod11-combat.md 3.1.2).
#[test]
fn the_knockback_kill_matches_15bs_word_for_word() {
    let Some(fs) = vcod_common::testing::game_fs()
        .filter(|fs| install_protocol(fs).transport.version == PROTOCOL_V6.transport.version)
    else {
        return;
    };
    let (skel, map) = rig(&fs, "basehead_axis_a_04");
    let calls = calls(DUMP_15);
    assert_eq!(calls.len(), 1);
    none_off(
        "records",
        &records_off(&skel, &map, &calls[0], &fs, Arith::Stored),
    );
    let (words, off) = build_off(&skel, &map, &calls[0], build_stored);
    none_off("build", &off);
    assert_eq!(words, 113 * 16);
}
