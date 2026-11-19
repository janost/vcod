//! `vcod_cod4::skel` against 1.7's own skeleton: the cobra's DObj as
//! `cod4_lnxded` built it on each of 159 helicopter thinks, read under gdb
//! (`fixtures/skel/cobra-idle-gdb.txt`, its header the recipe), rebuilt
//! here from the zone's model and the think's two gun angles: the base
//! locals, the controller's three tags and `DObjCalcSkel`'s composition,
//! word for word (docs/research/cod4-gsc.md 3.2, "The skeleton"). Needs
//! `COD4_DIR`.

use vcod_cod4::assets::model::XModel;
use vcod_cod4::skel::{
    AnimMat, SkelModel, angles_to_quat, angles_to_quat_in, compose_controlled,
    compose_controlled_in,
};
use vcod_cod4::testing::cod4_install;
use vcod_cod4::walk::{WalkOptions, walk};
use vcod_cod4::zone::Zone;
use vcod_common::playerpose::Cod4Arith;

const DUMP: &str = include_str!("fixtures/skel/cobra-idle-gdb.txt");

/// The cobra's bones the controller sets: `tag_body`, `tag_turret`,
/// `tag_barrel` (vehicle +0x318, +0x31c, +0x324 in the dump's run).
const BODY: usize = 2;
const TURRET: usize = 41;
const BARREL: usize = 48;

fn cobra() -> Option<XModel> {
    let install = cod4_install()?;
    let z = Zone::open_path(&install.zone("english", "mp_crash")).unwrap();
    walk(z, WalkOptions::default())
        .unwrap()
        .assets
        .xmodels
        .into_iter()
        .find(|m| m.name == "vehicle_cobra_helicopter_fly")
}

fn bits(s: &str) -> f32 {
    f32::from_bits(u32::from_str_radix(s, 16).unwrap())
}

#[test]
fn the_cobras_skeleton_matches_retails_word_for_word() {
    let Some(x) = cobra() else {
        return;
    };
    let skel = SkelModel::of(&x).expect("the cobra's arrays");
    assert_eq!(skel.bones(), 54);
    let set = |angles: [f32; 3]| AnimMat {
        quat: angles_to_quat(angles),
        trans: [0.0; 3],
        trans_weight: 0.0,
    };
    let (mut hits, mut words) = (0, 0);
    let mut mats = Vec::new();
    let mut off = Vec::new();
    for line in DUMP.lines().filter(|l| !l.starts_with('#')) {
        let t: Vec<&str> = line.split_whitespace().collect();
        if t[0] == "hit" {
            let (pitch, yaw) = (bits(t[1]), bits(t[2]));
            mats = skel.base_locals();
            mats[BODY] = set([0.0; 3]);
            mats[TURRET] = set([0.0, yaw, 0.0]);
            mats[BARREL] = set([pitch, 0.0, 0.0]);
            skel.calc_skel(&mut mats);
            hits += 1;
            continue;
        }
        let b: usize = t[0].parse().unwrap();
        let m = mats[b];
        let ours = [
            m.quat[0],
            m.quat[1],
            m.quat[2],
            m.quat[3],
            m.trans[0],
            m.trans[1],
            m.trans[2],
            m.trans_weight,
        ];
        for (k, w) in t[1..].iter().enumerate() {
            words += 1;
            if ours[k].to_bits() != bits(w).to_bits() {
                off.push(format!(
                    "hit {hits} bone {b} word {k}: retail {w} ours {:08x}",
                    ours[k].to_bits()
                ));
            }
        }
    }
    assert_eq!(hits, 159);
    assert_eq!(words, 159 * 9 * 8);
    assert!(
        off.is_empty(),
        "{} words off: {:?}",
        off.len(),
        &off[..off.len().min(10)]
    );
}

/// A minute of two moving players' skeletons, controls and swings as
/// `cod4_lnxded` built them under gdb (its header the recipe).
const PLAYER_DUMP: &str = include_str!("fixtures/skel/player-mp_crash-gdb.txt");

/// A player body's control bones as 1.7 composed them: each one the dump
/// shows its controller set rebuilt off retail's own bone 0, parent and
/// angles through [`compose_controlled`], word for word; `iw3mp.exe`'s
/// arithmetic off the same inputs within 1e-7.
#[test]
fn a_players_control_bones_match_retails_word_for_word() {
    use vcod_common::playerpose::Cod4Arith;
    let Some(install) = cod4_install() else {
        return;
    };
    let z = Zone::open_path(&install.zone("english", "mp_crash")).unwrap();
    let models = walk(z, WalkOptions::default()).unwrap().assets.xmodels;
    let mut off = Vec::new();
    let (mut calcs, mut checked) = (0, 0);
    let mut client_off = 0.0f32;
    let mut client_words = 0;
    let mut lines = PLAYER_DUMP
        .lines()
        .filter(|l| !l.starts_with('#'))
        .peekable();
    while let Some(line) = lines.next() {
        let t: Vec<&str> = line.split_whitespace().collect();
        if t[0] != "calc" {
            continue;
        }
        let x = models.iter().find(|m| m.name == t[2]).expect(t[2]);
        let skel = SkelModel::of(x).unwrap();
        let mut ctls = Vec::new();
        let mut mats = std::collections::HashMap::new();
        while let Some(l) = lines.next_if(|l| !l.starts_with("calc")) {
            let w: Vec<&str> = l.split_whitespace().collect();
            match w[0] {
                "ctlin" | "ctlout" | "swing" | "tag" => {}
                "ctl" => ctls.push((
                    w[1].parse::<usize>().unwrap(),
                    [bits(w[2]), bits(w[3]), bits(w[4])],
                )),
                b => {
                    let v: Vec<f32> = w[1..].iter().map(|s| bits(s)).collect();
                    let m = AnimMat {
                        quat: [v[0], v[1], v[2], v[3]],
                        trans: [v[4], v[5], v[6]],
                        trans_weight: v[7],
                    };
                    mats.insert(b.parse::<usize>().unwrap(), m);
                }
            }
        }
        calcs += 1;
        for (b, angles) in ctls {
            let p = skel.parents[b - skel.roots];
            let local = AnimMat {
                quat: angles_to_quat(angles),
                trans: [0.0; 3],
                trans_weight: 0.0,
            };
            let parent = mats
                .get(&p)
                .unwrap_or_else(|| panic!("bone {b}'s parent {p} not dumped"));
            let m = compose_controlled(&mats[&0], parent, &local, skel.trans[b - skel.roots]);
            checked += 1;
            // `iw3mp.exe`'s arm at double precision with its own stores, off
            // the same inputs.
            let client = compose_controlled_in(
                &mats[&0],
                parent,
                &AnimMat {
                    quat: angles_to_quat_in(angles, Cod4Arith::Client),
                    ..local
                },
                skel.trans[b - skel.roots],
                Cod4Arith::Client,
            );
            let q = (0..4).map(|k| (client.quat[k] - m.quat[k]).abs());
            let t = (0..3).map(|k| (client.trans[k] - m.trans[k]).abs());
            client_words += (0..4).filter(|&k| client.quat[k] != m.quat[k]).count()
                + (0..3).filter(|&k| client.trans[k] != m.trans[k]).count();
            client_off = q.chain(t).fold(client_off, f32::max);
            if m != mats[&b] {
                off.push(format!(
                    "calc {calcs} bone {b} {angles:?}: ours {m:?} retail {:?}",
                    mats[&b]
                ));
            }
        }
    }
    assert_eq!((calcs, checked), (200, 804));
    println!("the client's arm off by {client_off:e}, {client_words} words");
    assert!(client_off < 1e-7, "the client's arm off by {client_off}");
    assert!(
        off.is_empty(),
        "{} of {checked} off:\n{}",
        off.len(),
        off.join("\n")
    );
}

/// 1.7's controller (`BG_Player_DoControllersInternal`, 0x804cf90) on each
/// body the dump caught: its `clientInfo_t` inputs through
/// `Controllers::cod4` and one ease from the controls it held, word for word
/// with the controls it left.
#[test]
fn a_players_controls_match_retails_word_for_word() {
    use vcod_common::net::cod1::flags::{EF_CROUCH, EF_DEAD, EF_PRONE};
    use vcod_common::playerpose::{
        BodyAngles, BodyInput, BodySlope, Cod4Arith, Controllers, LeanDvars,
    };
    let words = |s: &str| -> Vec<f32> { s.split_whitespace().map(bits).collect() };
    let controls = |w: &[f32]| Controllers {
        back_low: [w[0], w[1], w[2]],
        back_mid: [w[3], w[4], w[5]],
        back_up: [w[6], w[7], w[8]],
        neck: [w[9], w[10], w[11]],
        head: [w[12], w[13], w[14]],
        pelvis: [w[15], w[16], w[17]],
        origin: [w[18], w[19], w[20]],
        origin_trans: glam::Vec3::new(w[21], w[22], w[23]),
    };
    let mut lines = PLAYER_DUMP.lines();
    let (mut checked, mut off) = (0, Vec::new());
    while let Some(line) = lines.next() {
        let Some(rest) = line.strip_prefix("ctlin ") else {
            continue;
        };
        let (head, held) = rest.split_once(" | ").unwrap();
        let h: Vec<&str> = head.split_whitespace().collect();
        let ft: i32 = h[1].parse().unwrap();
        let e = u32::from_str_radix(h[2], 16).unwrap() as i32;
        let v: Vec<f32> = h[3..].iter().map(|s| bits(s)).collect();
        let raw = |k: usize| u32::from_str_radix(h[3 + k], 16).unwrap();
        let mut eflags = 0;
        for (c4, v1) in [(0x4, EF_CROUCH), (0x8, EF_PRONE), (0x20000, EF_DEAD)] {
            if e & c4 != 0 {
                eflags |= v1;
            }
        }
        let angles = BodyAngles {
            legs_yaw: v[2],
            torso_yaw: v[3],
            pitch: v[4],
            movetype: raw(9),
            ..Default::default()
        };
        let input = BodyInput {
            view: [v[6], v[7], v[8]],
            eflags,
            ..Default::default()
        };
        let slope = BodySlope {
            lean: v[5],
            torso_pitch: v[0],
            waist_pitch: v[1],
            ..Default::default()
        };
        let mut c = controls(&words(held));
        let target = Controllers::cod4(
            &angles,
            &input,
            &slope,
            &LeanDvars::default(),
            Cod4Arith::Server,
        );
        c.ease_toward_cod4(&target, ft);
        let out = lines.next().unwrap().strip_prefix("ctlout ").unwrap();
        let want = controls(&words(out));
        checked += 1;
        if c != want && off.len() < 20 {
            off.push(format!("{line}\n  ours {c:?}\n  retail {want:?}"));
        }
    }
    assert_eq!(checked, 255);
    assert!(off.is_empty(), "{checked} checked:\n{}", off.join("\n"));
}

/// Player bodies' anim trees as `cod4_lnxded` walked them under gdb, and
/// the locals `DObjCalcAnim` left (its header the recipe).
const ANIM_DUMP: &str = include_str!("fixtures/skel/player-anim-gdb.txt");

/// 1.7's sampler (`vcod_cod4::xanimcalc`, cod4-player-anims.md 11) on each
/// tree the dump caught: the leaves' clips at their node times and weights,
/// the folds of two or more live children, every needed bone's local word
/// for word.
#[test]
fn a_players_anim_locals_match_retails_word_for_word() {
    let Some((calcs, words, off)) = anim_locals(Cod4Arith::Server) else {
        return;
    };
    assert_eq!((calcs, words), (280, 17392));
    assert!(
        off.is_empty(),
        "{calcs} calcs, {} of {words} words off: {:#?}",
        off.len(),
        &off[..off.len().min(30)]
    );
}

/// The same trees in `iw3mp.exe`'s arithmetic, what the drawn bodies
/// take: the same sums, its x87 at double precision and storing a few
/// values the server keeps (cod4-player-anims.md 11). No client dump
/// exists, so this bounds how far the two binaries' words part: a shift
/// whose `mins` and `size` nearly cancel parts most.
#[test]
fn a_players_drawn_anim_locals_part_from_retails_by_their_stores() {
    let Some((calcs, words, off)) = anim_locals(Cod4Arith::Client) else {
        return;
    };
    assert_eq!((calcs, words), (280, 17392));
    let worst = off.iter().map(|o| o.1).fold(0.0f32, f32::max);
    println!("{} of {words} words off, worst {worst:e}", off.len());
    assert!(!off.is_empty() && worst < 4e-6, "{worst}");
}

/// The calcs sampled, the words checked, and each word off retail's with
/// how far.
type Sampled = (usize, usize, Vec<(String, f32)>);

/// Each tree of `ANIM_DUMP` rebuilt and sampled in `a`'s arithmetic: the
/// calcs, the words checked, and each word off retail's with how far.
fn anim_locals(a: Cod4Arith) -> Option<Sampled> {
    anim_locals_in(a, false)
}

/// Each tree of `ANIM_DUMP` as `main`'s nodes and their leaves, through
/// `xanimcalc::player_locals` over a skeleton of its models, what the
/// server's hit poses and the drawn bodies sample by: every needed bone's
/// local word for word, but a head's copies of body bones, which the
/// skeleton merges into the body's.
#[test]
fn a_players_anim_locals_through_the_shared_sampler_match_retails() {
    let Some((calcs, words, off)) = anim_locals_in(Cod4Arith::Server, true) else {
        return;
    };
    assert_eq!((calcs, words), (280, 17392));
    let bones: std::collections::BTreeSet<String> = off
        .iter()
        .map(|o| {
            o.0.split(" word")
                .next()
                .unwrap()
                .split_once(' ')
                .unwrap()
                .1
                .to_string()
        })
        .collect();
    assert!(
        off.is_empty(),
        "{} of {words} words off: {bones:#?}",
        off.len()
    );
}

/// [`anim_locals`]; `shared` samples each tree through
/// `xanimcalc::player_locals`, a dumped bone found by name.
fn anim_locals_in(a: Cod4Arith, shared: bool) -> Option<Sampled> {
    use std::collections::HashMap;
    use vcod_cod4::anim::BodySource;
    use vcod_cod4::anim::script_string;
    use vcod_cod4::xanimcalc::{Parts, Tree, calc, player_locals};
    use vcod_common::playerpose::Leaf;
    use vcod_common::skeleton::Skeleton;
    let install = cod4_install()?;
    let zone = |name: &str| {
        let z = Zone::open_path(&install.zone("english", name)).unwrap();
        walk(z, WalkOptions { keep_bulk: true }).unwrap().assets
    };
    let zones = [zone("mp_crash"), zone("common_mp")];
    let mut parts: HashMap<String, Parts> = HashMap::new();
    let mut part = |name: &str| -> Parts {
        parts
            .entry(name.to_string())
            .or_insert_with(|| {
                zones
                    .iter()
                    .find_map(|a| {
                        let x = a.xanims.iter().find(|x| x.name == name)?;
                        Some(Parts::of(a, x).unwrap())
                    })
                    .expect(name)
            })
            .clone()
    };
    let bone_names = |models: &str| -> Vec<String> {
        models
            .split(',')
            .flat_map(|m| {
                let (a, x) = zones
                    .iter()
                    .find_map(|a| Some((a, a.xmodels.iter().find(|x| x.name == m)?)))
                    .expect(m);
                x.bone_names
                    .iter()
                    .map(|&i| script_string(a, i).to_string())
                    .collect::<Vec<_>>()
            })
            .collect()
    };
    // Each model's own locals, the DObj's bones in model order: what a bone
    // no leaf reached keeps (`SkelModel::base_locals`).
    let bases = |models: &str| -> Vec<AnimMat> {
        models
            .split(',')
            .flat_map(|m| {
                let x = zones
                    .iter()
                    .find_map(|a| a.xmodels.iter().find(|x| x.name == m))
                    .expect(m);
                SkelModel::of(x).unwrap().base_locals()
            })
            .collect()
    };
    let remap = |p: &Parts, names: &[String]| -> Vec<Option<usize>> {
        p.bones
            .iter()
            .map(|b| names.iter().position(|n| n == b))
            .collect()
    };
    struct Walk<'a> {
        time: f32,
        leaf: Option<&'a str>,
        kids: Vec<(usize, f32)>,
    }
    // Per model set: its skeleton, its models' names and a source holding
    // their own locals.
    let mut sources: HashMap<String, (Skeleton, Vec<String>, BodySource)> = HashMap::new();
    let (mut calcs, mut words, mut off, mut skipped) = (0, 0, Vec::new(), 0);
    let mut lines = ANIM_DUMP.lines().filter(|l| !l.starts_with('#')).peekable();
    while let Some(line) = lines.next() {
        let t: Vec<&str> = line.split_whitespace().collect();
        if t[0] != "calc" {
            continue;
        }
        let names = bone_names(t[1]);
        assert_eq!(names.len(), t[3].parse::<usize>().unwrap(), "{line}");
        let mut walks: Vec<(usize, Walk)> = Vec::new();
        let mut copies: Vec<usize> = Vec::new();
        let mut want = Vec::new();
        while let Some(l) = lines.next_if(|l| !l.starts_with("calc")) {
            let w: Vec<&str> = l.split_whitespace().collect();
            if w[0] == "walk" {
                let kids = match w[6] {
                    "kids" if w[7] != "-" => w[7]
                        .split(',')
                        .map(|k| {
                            let (i, wt) = k.split_once(':').unwrap();
                            (i.parse().unwrap(), bits(wt))
                        })
                        .collect(),
                    _ => Vec::new(),
                };
                walks.push((
                    w[1].parse().unwrap(),
                    Walk {
                        time: bits(w[2]),
                        leaf: (w[6] == "leaf").then_some(w[7]),
                        kids,
                    },
                ));
            } else {
                want.push(l);
            }
        }
        let leaves: HashMap<&str, (Parts, Vec<Option<usize>>)> = walks
            .iter()
            .filter_map(|(_, w)| w.leaf)
            .map(|n| {
                let p = part(n);
                let r = remap(&p, &names);
                (n, (p, r))
            })
            .collect();
        fn tree<'a>(
            walks: &[(usize, Walk)],
            leaves: &'a HashMap<&str, (Parts, Vec<Option<usize>>)>,
            idx: usize,
            weight: f32,
        ) -> Tree<'a> {
            let w = &walks.iter().find(|(i, _)| *i == idx).expect("walked").1;
            match w.leaf {
                Some(n) => {
                    let (parts, remap) = &leaves[n];
                    Tree::Leaf {
                        clip: parts,
                        remap,
                        time: w.time,
                        weight,
                    }
                }
                None => Tree::Node {
                    weight,
                    children: w
                        .kids
                        .iter()
                        .map(|&(k, kw)| tree(walks, leaves, k, kw))
                        .collect(),
                },
            }
        }
        let root = tree(&walks, &leaves, walks[0].0, 1.0);
        let (mut mats, reached) = calc(&root, names.len(), a);
        for ((m, r), base) in mats.iter_mut().zip(&reached).zip(bases(t[1])) {
            if !r {
                *m = base;
            }
        }
        if shared {
            let (skel, models, src) = sources.entry(t[1].to_string()).or_insert_with(|| {
                let mut built = HashMap::new();
                let list: Vec<String> = t[1].split(',').map(str::to_string).collect();
                for m in &list {
                    let (a, x) = zones
                        .iter()
                        .find_map(|a| Some((a, a.xmodels.iter().find(|x| &x.name == m)?)))
                        .expect(m);
                    let model =
                        vcod_cod4::anim::model(a, x, |id| a.materials[id.index()].name.clone())
                            .unwrap()
                            .expect(m);
                    built.insert(m.clone(), model);
                }
                let skel = Skeleton::build(&list.iter().map(|m| &built[m]).collect::<Vec<_>>());
                let src = BodySource::from_zones(&[&zones[0], &zones[1]], built).unwrap();
                (skel, list, src)
            });
            let walk = |i: usize| &walks.iter().find(|(k, _)| *k == i).expect("walked").1;
            let main = walk(walks[0].0).kids.as_slice();
            assert_eq!(main.len(), 1, "{line}");
            let channels: Vec<(f32, Vec<Leaf>)> = walk(main[0].0)
                .kids
                .iter()
                .map(|&(node, weight)| {
                    let leaves = walk(node)
                        .kids
                        .iter()
                        .map(|&(k, w)| Leaf {
                            clip: walk(k).leaf.expect("a leaf under a node").to_string(),
                            time: walk(k).time,
                            weight: w,
                        })
                        .collect();
                    (weight, leaves)
                })
                .collect();
            let models: Vec<&str> = models.iter().map(String::as_str).collect();
            let locals = player_locals(src, skel, &models, &channels, a).expect("every clip held");
            // The DObj's bones are each model's in turn; the skeleton keeps
            // the body's and merges the head's into them by name.
            let dobj: Vec<usize> = (0..models.len())
                .flat_map(|i| skel.model_map(i).iter().copied())
                .collect();
            assert_eq!(dobj.len(), names.len(), "{line}");
            for (b, m) in mats.iter_mut().enumerate() {
                assert_eq!(skel.bones()[dobj[b]].name, names[b]);
                *m = locals[dobj[b]].unwrap_or_else(|| panic!("{} has no local", names[b]));
            }
            // A head's own copy of a body bone (its spine, neck and wrists)
            // is that body bone in the skeleton: its drawn place is the
            // body's, its own unkeyed local not one vcod keeps.
            copies = (0..names.len())
                .filter(|&b| dobj[..b].contains(&dobj[b]))
                .collect();
        }
        calcs += 1;
        for l in want {
            let w: Vec<&str> = l.split_whitespace().collect();
            let b: usize = w[0].parse().unwrap();
            if copies.contains(&b) {
                skipped += w.len() - 1;
                continue;
            }
            let m = mats[b];
            let ours = [
                m.quat[0],
                m.quat[1],
                m.quat[2],
                m.quat[3],
                m.trans[0],
                m.trans[1],
                m.trans[2],
                m.trans_weight,
            ];
            for (k, s) in w[1..].iter().enumerate() {
                words += 1;
                let theirs = bits(s);
                if ours[k].to_bits() != theirs.to_bits() {
                    off.push((
                        format!(
                            "{} bone {b} ({}) word {k}: retail {s} ours {:08x}",
                            t[2],
                            names[b],
                            ours[k].to_bits()
                        ),
                        (ours[k] - theirs).abs(),
                    ));
                }
            }
        }
    }
    Some((calcs, words + skipped, off))
}
