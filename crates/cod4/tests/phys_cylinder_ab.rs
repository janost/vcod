//! The cylinder collider against retail (`ode::collide::cylinder` through
//! the class 6 transform, cod4-gsc.md 3.6.5): every cylinder collide of
//! the world 0 and 1 captures (`fixtures/phys/`, 3.6.2) fed its captured
//! pose gives the capture's raw contacts bit for bit and, through
//! 0x5a2320's cut, the contact pass's. `COD4_DIR` unset passes vacuously.

use std::collections::BTreeMap;

use vcod_cod4::collision::ClipMap;
use vcod_cod4::map::Map;
use vcod_cod4::ode::collide::brushgeom::NARROW_MAX_LENGTH;
use vcod_cod4::ode::collide::{ContactGeom, MAX_CONTACTS, cut, transform};
use vcod_cod4::ode::geom::{Geom, Transform};
use vcod_cod4::ode::math::{M3, V3};
use vcod_cod4::ode::rotation::q_to_r;
use vcod_cod4::ragdoll::CLIP_MASK;
use vcod_cod4::testing::cod4_install;

const FIXTURES: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/phys");

fn hex(w: &str) -> u32 {
    u32::from_str_radix(w, 16).expect("hex word")
}

fn f(w: u32) -> f32 {
    f32::from_bits(w)
}

/// The words of each `|` section after its label.
fn sections(line: &str) -> BTreeMap<String, Vec<u32>> {
    line.split('|')
        .skip(1)
        .map(|p| {
            let mut it = p.split_whitespace();
            let label = it.next().unwrap_or("").to_string();
            (label, it.map(hex).collect())
        })
        .collect()
}

fn rows(r: &[u32]) -> M3 {
    std::array::from_fn(|i| std::array::from_fn(|j| f(r[4 * i + j])))
}

/// A raw `c` line: the `dContactGeom`'s 12 words.
#[derive(Clone, Debug)]
struct Captured {
    words: Vec<u32>,
}

impl Captured {
    fn geom(&self) -> ContactGeom {
        let v = |k: usize| [f(self.words[k]), f(self.words[k + 1]), f(self.words[k + 2])];
        ContactGeom {
            pos: v(0),
            normal: v(4),
            depth: f(self.words[8]),
            surface: self.words[11],
        }
    }
}

/// One cylinder collide: the step, the body, the pose it ran at, and the
/// capture's raw contacts and the pass's.
struct Case {
    step: u32,
    body: u32,
    pose: (V3, M3),
    raw: Vec<Captured>,
    cut: Option<Vec<Captured>>,
}

struct Dump {
    transforms: BTreeMap<u32, (Transform, u32)>,
    cases: Vec<Case>,
}

fn parse(text: &str) -> Dump {
    let lines: Vec<&str> = text.lines().collect();
    let mut poses: BTreeMap<u32, (V3, M3)> = BTreeMap::new();
    let mut geoms: BTreeMap<u32, Vec<u32>> = BTreeMap::new();
    let mut inners: BTreeMap<u32, Vec<u32>> = BTreeMap::new();
    let mut cases = Vec::new();
    let mut i = 0;
    let take = |i: &mut usize, n: usize| -> Vec<Captured> {
        let out = (0..n)
            .map(|k| Captured {
                words: lines[*i + 1 + k]
                    .split_whitespace()
                    .skip(1)
                    .map(hex)
                    .collect(),
            })
            .collect();
        *i += n;
        out
    };
    while i < lines.len() {
        let line = lines[i];
        let w: Vec<&str> = line.split_whitespace().collect();
        match w.first().copied() {
            Some("new") => {
                let s = sections(line);
                let p = &s["pos"];
                poses.insert(
                    w[1].parse().unwrap(),
                    ([f(p[0]), f(p[1]), f(p[2])], rows(&s["R"])),
                );
            }
            Some("body") => {
                let parts: Vec<Vec<u32>> = line
                    .split('|')
                    .skip(1)
                    .map(|p| p.split_whitespace().map(hex).collect())
                    .collect();
                let q = [
                    f(parts[1][0]),
                    f(parts[1][1]),
                    f(parts[1][2]),
                    f(parts[1][3]),
                ];
                poses.insert(
                    w[1].parse().unwrap(),
                    ([f(parts[0][0]), f(parts[0][1]), f(parts[0][2])], q_to_r(q)),
                );
            }
            Some("geom") => {
                geoms.insert(w[1].parse().unwrap(), sections_raw(line));
            }
            Some("inner") => {
                inners.insert(hex(w[1]), sections_raw(line));
            }
            Some("collide") if w[4] == "6" => {
                let (step, body, n) = (
                    w[1].parse().unwrap(),
                    w[2].parse().unwrap(),
                    w[6].parse().unwrap(),
                );
                let pose = poses[&body];
                let raw = take(&mut i, n);
                cases.push(Case {
                    step,
                    body,
                    pose,
                    raw,
                    cut: None,
                });
            }
            Some("cut") => {
                let (step, body): (u32, u32) = (w[1].parse().unwrap(), w[2].parse().unwrap());
                let n = w[4].parse().unwrap();
                let c = take(&mut i, n);
                if let Some(case) = cases
                    .iter_mut()
                    .rev()
                    .find(|c| c.step == step && c.body == body)
                {
                    case.cut = Some(c);
                }
            }
            _ => {}
        }
        i += 1;
    }
    let transforms = geoms
        .iter()
        .filter(|(_, g)| g[1] == 6)
        .map(|(&id, g)| {
            let inner = &inners[&g[19]];
            let t = Transform {
                inner: Geom::Cylinder {
                    direction: inner[19] as usize,
                    radius: f(inner[20]),
                    half_height: f(inner[21]),
                },
                r: rows(&g[22..34]),
                pos: [f(g[34]), f(g[35]), f(g[36])],
            };
            (id, (t, g[19]))
        })
        .collect();
    Dump { transforms, cases }
}

/// A `geom` or `inner` line's words after its `|`.
fn sections_raw(line: &str) -> Vec<u32> {
    line.split('|')
        .nth(1)
        .unwrap()
        .split_whitespace()
        .map(hex)
        .collect()
}

fn close(a: f32, b: f32) -> bool {
    (a - b).abs() <= 1e-4
}

fn same(ours: &ContactGeom, theirs: &ContactGeom) -> (bool, bool) {
    let near = (0..3)
        .all(|k| close(ours.pos[k], theirs.pos[k]) && close(ours.normal[k], theirs.normal[k]))
        && close(ours.depth, theirs.depth)
        && ours.surface == theirs.surface;
    let exact = ours.pos.map(f32::to_bits) == theirs.pos.map(f32::to_bits)
        && ours.normal.map(f32::to_bits) == theirs.normal.map(f32::to_bits)
        && ours.depth.to_bits() == theirs.depth.to_bits()
        && ours.surface == theirs.surface;
    (near, exact)
}

/// Each list against the capture's: (within 1e-4, bit for bit).
fn compare(ours: &[ContactGeom], theirs: &[Captured]) -> (bool, bool) {
    if ours.len() != theirs.len() {
        return (false, false);
    }
    ours.iter()
        .zip(theirs)
        .fold((true, true), |(n, e), (o, t)| {
            let (a, b) = same(o, &t.geom());
            (n && a, e && b)
        })
}

#[derive(Default)]
struct Tally {
    cases: usize,
    contacts: usize,
    raw_near: usize,
    raw_exact: usize,
    cuts: usize,
    cut_exact: usize,
    misses: Vec<String>,
}

fn run(clip: &ClipMap, name: &str) -> Tally {
    let d = parse(&std::fs::read_to_string(format!("{FIXTURES}/{name}")).unwrap());
    let mut t = Tally::default();
    for c in &d.cases {
        let (tr, inner) = &d.transforms[&c.body];
        assert!(
            c.raw.iter().all(|x| x.words[9] == *inner),
            "{name}: contacts keep the inner geom"
        );
        let ours = transform::collide_geom(
            clip,
            &Geom::Transform(Box::new(tr.clone())),
            c.pose.0,
            &c.pose.1,
            c.pose.0,
            NARROW_MAX_LENGTH,
            CLIP_MASK,
            MAX_CONTACTS,
        )
        .unwrap();
        t.cases += 1;
        t.contacts += c.raw.len();
        let (near, exact) = compare(&ours, &c.raw);
        t.raw_near += usize::from(near);
        t.raw_exact += usize::from(exact);
        if !near {
            t.misses.push(format!(
                "{name} step {} body {}: ours {} theirs {}\n  ours   {:?}\n  theirs {:?}",
                c.step,
                c.body,
                ours.len(),
                c.raw.len(),
                ours.iter().take(4).collect::<Vec<_>>(),
                c.raw.iter().take(4).map(Captured::geom).collect::<Vec<_>>()
            ));
        }
        if near && !exact {
            for (o, x) in ours.iter().zip(&c.raw) {
                let x = x.geom();
                if same(o, &x).1 {
                    continue;
                }
                println!(
                    "inexact {name} step {} body {}: ours {:?} theirs {:?}",
                    c.step, c.body, o, x
                );
            }
        }
        if let Some(theirs) = &c.cut {
            let pass = if ours.len() >= 5 {
                cut(&ours)
            } else {
                ours.clone()
            };
            t.cuts += 1;
            let ok = compare(&pass, theirs).1;
            t.cut_exact += usize::from(ok);
            if !ok {
                println!(
                    "cut {name} step {} body {}: raw {}",
                    c.step,
                    c.body,
                    ours.len()
                );
                for o in &ours {
                    println!("  raw   {:?}", o);
                }
                for o in &pass {
                    println!("  ours  {:?}", o);
                }
                for x in theirs {
                    println!("  their {:?}", x.geom());
                }
            }
        }
    }
    t
}

/// Every cylinder geom step of the frag (a bottle, a can, two tyres on
/// mp_crash) and the car (its two tyres on mp_backlot): 240 collides, 666
/// contacts.
#[test]
fn cylinder_contacts_match_the_captures() {
    let Some(install) = cod4_install() else {
        return;
    };
    let mut all = Tally::default();
    for (map, name) in [
        ("mp_crash", "frag-mp_crash-1.7.txt"),
        ("mp_backlot", "car-mp_backlot-1.7.txt"),
    ] {
        let (_, walked) = Map::load(&install, "english", map).unwrap();
        let clip = ClipMap::from_assets(&walked.assets).unwrap();
        let t = run(&clip, name);
        println!(
            "{name}: {} collides ({} contacts): raw {} within 1e-4, {} exact; cut {} of {} exact",
            t.cases, t.contacts, t.raw_near, t.raw_exact, t.cut_exact, t.cuts
        );
        for m in t.misses.iter().take(6) {
            println!("{m}");
        }
        all.cases += t.cases;
        all.raw_exact += t.raw_exact;
        all.cuts += t.cuts;
        all.cut_exact += t.cut_exact;
        all.misses.extend(t.misses);
    }
    assert_eq!(all.cases, 240);
    assert_eq!(
        (all.raw_exact, all.cut_exact),
        (all.cases, all.cuts),
        "{} outside 1e-4",
        all.misses.len()
    );
}
