//! The posed body against `cod2_lnxded`'s own, read under gdb
//! (`fixtures/skel/`, the header the recipe): `DObjCalcAnim`'s locals off
//! the dumped tree's clips, node times and weights, and `DObjCalcSkel`'s
//! bones off retail's locals, word for word (cod2-player-anims.md 11).
//! Needs `COD2_DIR`.

use std::collections::HashMap;
use std::rc::Rc;

use vcod_cod2::player::dobj::{self, Mat, Tree};
use vcod_common::pk3::Pk3Fs;
use vcod_common::skeleton::Skeleton;
use vcod_common::xanim::{self, XAnim};

const DUMP: &str = include_str!("fixtures/skel/hitpose-gdb.txt");

/// The target's and the shooter's DObj parts, each grafted by name.
const MODELS: [&str; 3] = [
    "playerbody_german_africa01",
    "head_german_afrca_christoph",
    "helmet_german_africa",
];

fn bits(s: &str) -> f32 {
    f32::from_bits(u32::from_str_radix(s, 16).unwrap())
}

/// A walked node: its index, its children's index range, its info's time
/// and weight, its clip.
struct Node {
    index: usize,
    kids: std::ops::Range<usize>,
    time: f32,
    weight: f32,
    clip: Option<String>,
    children: Vec<Node>,
}

/// One build: the tree the walk visited, the control mask (skeleton
/// +0x10), and each bone's `DObjAnimMat` before and after `DObjCalcSkel`.
struct Call {
    tree: Option<Node>,
    ctrl: Vec<u8>,
    pre: Vec<[f32; 8]>,
    post: Vec<[f32; 8]>,
}

impl Call {
    fn control(&self, i: usize) -> bool {
        self.ctrl[i / 8] >> (i % 8) & 1 != 0
    }
}

/// Hangs `n` under the deepest node on `stack` whose children it is one of.
fn attach(stack: &mut Vec<Node>, n: Node) {
    while let Some(top) = stack.last() {
        if top.kids.contains(&n.index) {
            break;
        }
        let done = stack.pop().unwrap();
        stack.last_mut().unwrap().children.push(done);
    }
    stack.push(n);
}

fn calls(dump: &str) -> Vec<Call> {
    let mut out: Vec<Call> = Vec::new();
    let mut stack: Vec<Node> = Vec::new();
    let close = |stack: &mut Vec<Node>| {
        while stack.len() > 1 {
            let done = stack.pop().unwrap();
            stack.last_mut().unwrap().children.push(done);
        }
        stack.pop()
    };
    for line in dump.lines().filter(|l| !l.starts_with('#')) {
        let t: Vec<&str> = line.split_whitespace().collect();
        match t[0] {
            "calc" => out.push(Call {
                tree: None,
                ctrl: Vec::new(),
                pre: Vec::new(),
                post: Vec::new(),
            }),
            // walk NODE WEIGHT CLEAR NESTED info INFO TIME IWEIGHT (leaf
            // CLIP | kids N first F)
            "walk" => {
                let index: usize = t[1].parse().unwrap();
                let (kids, clip) = match t[9] {
                    "leaf" => (0..0, Some(t[10].to_string())),
                    _ => {
                        let (n, f): (usize, usize) =
                            (t[10].parse().unwrap(), t[12].parse().unwrap());
                        (f..f + n, None)
                    }
                };
                let n = Node {
                    index,
                    kids,
                    time: bits(t[7]),
                    weight: bits(t[8]),
                    clip,
                    children: Vec::new(),
                };
                attach(&mut stack, n);
            }
            "leaf" => {}
            "ctrl" => {
                let c = out.last_mut().unwrap();
                c.tree = close(&mut stack);
                c.ctrl = (0..16)
                    .map(|i| u8::from_str_radix(&t[1][2 * i..2 * i + 2], 16).unwrap())
                    .collect();
            }
            "pre" => out
                .last_mut()
                .unwrap()
                .pre
                .push(std::array::from_fn(|i| bits(t[2 + i]))),
            "post" => out
                .last_mut()
                .unwrap()
                .post
                .push(std::array::from_fn(|i| bits(t[2 + i]))),
            _ => panic!("unknown line {line}"),
        }
    }
    out
}

/// The rig as the dump's DObj lays it out: retail's bone `i` (the parts'
/// bones end to end) is our `map[i]`; and each of our bones' base quat.
struct Rig {
    skel: Skeleton,
    map: Vec<usize>,
    base: Vec<Option<[f32; 4]>>,
}

fn rig(fs: &Pk3Fs) -> Rig {
    vcod_cod2::model::register();
    let models: Vec<_> = MODELS
        .iter()
        .map(|m| vcod_common::model::load(fs, m).unwrap())
        .collect();
    let refs: Vec<_> = models.iter().map(|m| (m, None)).collect();
    let skel = Skeleton::build_grafted(&refs);
    // A later model's bone that shares an earlier bone's name is that bone
    // in retail's DObj (`DObjCreateDuplicateParts`): its build copies it.
    let map: Vec<usize> = (0..models.len())
        .flat_map(|m| skel.model_map(m).to_vec())
        .map(|si| skel.bone_index(&skel.bones()[si].name).unwrap())
        .collect();
    let mut base = vec![None; skel.bones().len()];
    let mut seen = vec![false; skel.bones().len()];
    let mut i = 0;
    for m in MODELS {
        for b in vcod_cod2::model::parts(fs, m).unwrap().bones {
            let si = map[i];
            if !seen[si] {
                seen[si] = true;
                base[si] = b.parent.map(|_| b.quat);
            }
            i += 1;
        }
    }
    Rig { skel, map, base }
}

fn words(m: &Mat) -> [f32; 8] {
    [
        m.quat[0], m.quat[1], m.quat[2], m.quat[3], m.trans[0], m.trans[1], m.trans[2], m.weight,
    ]
}

fn off(rig: &Rig, i: usize, ours: &Mat, retail: &[f32; 8], out: &mut Vec<String>) {
    for (w, (a, b)) in words(ours).iter().zip(retail).enumerate() {
        if a.to_bits() != b.to_bits() {
            out.push(format!(
                "bone {i} ({}) word {w}: ours {a:e} retail {b:e}",
                rig.skel.bones()[rig.map[i]].name
            ));
        }
    }
}

struct Clips<'a> {
    fs: &'a Pk3Fs,
    skel: &'a Skeleton,
    held: HashMap<String, (Rc<XAnim>, Vec<Option<usize>>)>,
}

impl Clips<'_> {
    fn load(&mut self, name: &str) {
        if !self.held.contains_key(name) {
            let a = xanim::load(self.fs, name).unwrap();
            let remap = a
                .tracks
                .iter()
                .map(|t| self.skel.bone_index(&t.bone))
                .collect();
            self.held.insert(name.into(), (Rc::new(a), remap));
        }
    }

    fn tree<'b>(&'b self, n: &Node) -> Tree<'b> {
        match &n.clip {
            Some(c) => {
                let (clip, remap) = &self.held[c];
                Tree::Leaf {
                    clip,
                    remap,
                    time: n.time,
                    weight: n.weight,
                }
            }
            None => Tree::Node {
                weight: n.weight,
                children: n.children.iter().map(|c| self.tree(c)).collect(),
            },
        }
    }
}

fn clip_names(n: &Node, out: &mut Vec<String>) {
    out.extend(n.clip.clone());
    for c in &n.children {
        clip_names(c, out);
    }
}

/// Our walk's locals for every bone retail's walk built (the controls'
/// aside, which the controller sets after), against retail's.
fn locals_off(rig: &Rig, clips: &mut Clips, call: &Call) -> (usize, Vec<String>) {
    let tree = call.tree.as_ref().unwrap();
    let mut names = Vec::new();
    clip_names(tree, &mut names);
    for n in &names {
        clips.load(n);
    }
    let (mats, reached) = dobj::calc(&clips.tree(tree), rig.skel.bones().len(), None);
    let mut seen = vec![false; rig.skel.bones().len()];
    let (mut n, mut out) = (0, Vec::new());
    for (i, &si) in rig.map.iter().enumerate() {
        // `tag_origin` takes the controller's local tag after the walk.
        if std::mem::replace(&mut seen[si], true) || call.control(i) || si == 0 {
            continue;
        }
        let ours = match reached[si] {
            true => mats[si],
            false => Mat::base(rig.base[si]),
        };
        n += 8;
        off(rig, i, &ours, &call.pre[i], &mut out);
    }
    (n, out)
}

/// Our build over retail's locals against the bones retail's left.
fn build_off(rig: &Rig, call: &Call) -> (usize, Vec<String>) {
    let n = rig.skel.bones().len();
    let mut locals = vec![Mat::ZERO; n];
    let mut control = vec![false; n];
    let mut seen = vec![false; n];
    for (i, &si) in rig.map.iter().enumerate() {
        if !std::mem::replace(&mut seen[si], true) {
            let p = call.pre[i];
            locals[si] = Mat {
                quat: [p[0], p[1], p[2], p[3]],
                trans: [p[4], p[5], p[6]],
                weight: p[7],
            };
            control[si] = call.control(i);
        }
    }
    let built = dobj::build(&rig.skel, &locals, &control, None);
    let mut out = Vec::new();
    for (i, &si) in rig.map.iter().enumerate() {
        off(rig, i, &built[si], &call.post[i], &mut out);
    }
    (rig.map.len() * 8, out)
}

fn report(what: &str, n: usize, off: &[String]) {
    assert!(
        off.is_empty(),
        "{what}: {} of {n} words off:\n{}",
        off.len(),
        off.iter().take(40).cloned().collect::<Vec<_>>().join("\n")
    );
}

fn dump() -> String {
    match std::env::var("SKEL_DUMP") {
        Ok(p) => std::fs::read_to_string(p).unwrap(),
        Err(_) => DUMP.to_string(),
    }
}

#[test]
fn the_walks_locals_match_retail_word_for_word() {
    let Some(fs) = vcod_common::testing::cod2_fs() else {
        return;
    };
    let rig = rig(&fs);
    let mut clips = Clips {
        fs: &fs,
        skel: &rig.skel,
        held: HashMap::new(),
    };
    let (mut n, mut all) = (0, Vec::new());
    let calls = calls(&dump());
    assert!(calls.len() >= 25);
    for (ci, call) in calls.iter().enumerate() {
        let (w, off) = locals_off(&rig, &mut clips, call);
        n += w;
        all.extend(off.into_iter().map(|o| format!("call {ci}: {o}")));
    }
    report("locals", n, &all);
}

#[test]
fn the_skeleton_matches_retail_word_for_word() {
    let Some(fs) = vcod_common::testing::cod2_fs() else {
        return;
    };
    let rig = rig(&fs);
    let (mut n, mut all) = (0, Vec::new());
    let calls = calls(&dump());
    assert!(calls.len() >= 25);
    for (ci, call) in calls.iter().enumerate() {
        let (w, off) = build_off(&rig, call);
        n += w;
        all.extend(off.into_iter().map(|o| format!("call {ci}: {o}")));
    }
    report("skeleton", n, &all);
}

/// `SKEL_BENCH=1`: the walk and the build's time a call over the dump.
#[test]
#[ignore = "tool: set SKEL_BENCH=1"]
fn bench_the_pose() {
    assert!(std::env::var_os("SKEL_BENCH").is_some(), "set SKEL_BENCH=1");
    let Some(fs) = vcod_common::testing::cod2_fs() else {
        return;
    };
    let rig = rig(&fs);
    let mut clips = Clips {
        fs: &fs,
        skel: &rig.skel,
        held: HashMap::new(),
    };
    let calls = calls(&dump());
    for c in &calls {
        locals_off(&rig, &mut clips, c);
    }
    let n = rig.skel.bones().len();
    let reps = 20;
    let (mut walk, mut skel) = (0.0, 0.0);
    for _ in 0..reps {
        for c in &calls {
            let t = std::time::Instant::now();
            let (m, _) = dobj::calc(&clips.tree(c.tree.as_ref().unwrap()), n, None);
            walk += t.elapsed().as_secs_f64();
            let t = std::time::Instant::now();
            std::hint::black_box(dobj::build(&rig.skel, &m, &vec![false; n], None));
            skel += t.elapsed().as_secs_f64();
        }
    }
    let per = |s: f64| s * 1e6 / (reps * calls.len()) as f64;
    eprintln!(
        "pose: the walk {:.1} us, the build {:.1} us a call, {} reruns",
        per(walk),
        per(skel),
        dobj::reruns()
    );
}

/// A pose built for the box bones alone leaves each of them, and its
/// parents, the words the whole build gives.
#[test]
fn a_masked_pose_keeps_the_words_of_the_bones_it_builds() {
    let Some(fs) = vcod_common::testing::cod2_fs() else {
        return;
    };
    let rig = rig(&fs);
    let mut clips = Clips {
        fs: &fs,
        skel: &rig.skel,
        held: HashMap::new(),
    };
    let bones = rig.skel.bones();
    let mut need = vec![false; bones.len()];
    for (i, b) in bones.iter().enumerate() {
        let mut j = i as i32;
        while b.has_hit_box() && j >= 0 {
            need[j as usize] = true;
            j = bones[j as usize].parent;
        }
    }
    assert!(need.iter().any(|n| !n));
    let control = vec![false; bones.len()];
    for c in &calls(&dump()) {
        locals_off(&rig, &mut clips, c);
        let tree = clips.tree(c.tree.as_ref().unwrap());
        let (all, _) = dobj::calc(&tree, bones.len(), None);
        let (some, _) = dobj::calc(&tree, bones.len(), Some(&need));
        let (all, some) = (
            dobj::build(&rig.skel, &all, &control, None),
            dobj::build(&rig.skel, &some, &control, Some(&need)),
        );
        for (i, &n) in need.iter().enumerate() {
            if n {
                assert_eq!(
                    words(&all[i]).map(f32::to_bits),
                    words(&some[i]).map(f32::to_bits)
                );
            }
        }
    }
}
