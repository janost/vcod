//! The box collider (`ode::collide::boxes`, cod4-gsc.md 3.6.4) against
//! retail 1.7's `dCollide` in the world 0 and 1 captures
//! (`fixtures/phys/`, 3.6.2): every class 1 geom's collide fed the body's
//! captured pose gives the capture's contacts, and 0x5a2320's cut of them
//! the contact pass's input. `COD4_DIR` unset passes vacuously.

use std::collections::HashMap;

use vcod_cod4::collision::ClipMap;
use vcod_cod4::map::Map;
use vcod_cod4::ode::collide::boxes::{BoxGeom, NARROW_MAX_LENGTH, collide_world};
use vcod_cod4::ode::collide::{ContactGeom, MAX_CONTACTS, cut};
use vcod_cod4::ode::math::M3;
use vcod_cod4::ode::rotation::q_to_r;
use vcod_cod4::ragdoll::CLIP_MASK;
use vcod_cod4::testing::cod4_install;

const FIXTURES: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/phys");

fn hex(w: &str) -> u32 {
    u32::from_str_radix(w, 16).expect("hex word")
}

fn f(w: &str) -> f32 {
    f32::from_bits(hex(w))
}

/// A collide of a box geom: the body's pose before the step, the box's
/// sides, the capture's raw contacts and the pass's input.
struct Collide {
    step: u32,
    body: u32,
    pos: [f32; 3],
    r: M3,
    sides: [f32; 3],
    raw: Vec<[u32; 12]>,
    cut: Option<Vec<[u32; 12]>>,
}

/// The box collides of one capture, in its order (README's line format),
/// each body's pose its last `body` line moved by the frame's sweep.
fn box_collides(text: &str, clip: &ClipMap) -> Vec<Collide> {
    let mut pose: HashMap<u32, ([f32; 3], M3)> = HashMap::new();
    let mut sides: HashMap<u32, [f32; 3]> = HashMap::new();
    let mut out: Vec<Collide> = Vec::new();
    let mut lines = text.lines().peekable();
    let words = |l: &str| -> Vec<[u32; 12]> {
        let w: Vec<&str> = l.split_whitespace().collect();
        assert_eq!(w[0], "c");
        vec![std::array::from_fn(|i| hex(w[i + 1]))]
    };
    while let Some(line) = lines.next() {
        let w: Vec<&str> = line.split_whitespace().collect();
        match w.first().copied() {
            Some("new") => {
                let parts: Vec<Vec<&str>> = line
                    .split('|')
                    .map(|p| p.split_whitespace().collect())
                    .collect();
                // mass, inv, force, torque, idle, pos, q, R, lvel, avel
                let p = &parts[6];
                let r = &parts[8];
                let pos = [f(p[1]), f(p[2]), f(p[3])];
                let r: M3 = std::array::from_fn(|i| std::array::from_fn(|j| f(r[1 + 4 * i + j])));
                pose.insert(w[1].parse().unwrap(), (pos, r));
            }
            Some("geom") => {
                let g: Vec<&str> = line.split('|').nth(1).unwrap().split_whitespace().collect();
                if hex(g[1]) == 1 {
                    sides.insert(w[1].parse().unwrap(), [f(g[19]), f(g[20]), f(g[21])]);
                }
            }
            Some("body") => {
                let parts: Vec<Vec<&str>> = line
                    .split('|')
                    .map(|p| p.split_whitespace().collect())
                    .collect();
                let p = &parts[1];
                let q = &parts[2];
                let mut pos = [f(p[0]), f(p[1]), f(p[2])];
                let q = [f(q[0]), f(q[1]), f(q[2]), f(q[3])];
                let id: u32 = w[1].parse().unwrap();
                if let (Some(s), Some(&(from, _))) = (sides.get(&id), pose.get(&id)) {
                    pos = swept(clip, from, pos, *s);
                }
                pose.insert(id, (pos, q_to_r(q)));
            }
            Some("collide") if w[4] == "1" => {
                let body: u32 = w[2].parse().unwrap();
                let n: usize = w[6].parse().unwrap();
                let raw = (0..n).flat_map(|_| words(lines.next().unwrap())).collect();
                let (pos, r) = pose[&body];
                out.push(Collide {
                    step: w[1].parse().unwrap(),
                    body,
                    pos,
                    r,
                    sides: sides[&body],
                    raw,
                    cut: None,
                });
            }
            Some("cut") => {
                let (step, body): (u32, u32) = (w[1].parse().unwrap(), w[2].parse().unwrap());
                let n: usize = w[4].parse().unwrap();
                let c: Vec<[u32; 12]> = (0..n).flat_map(|_| words(lines.next().unwrap())).collect();
                if let Some(last) = out.last_mut()
                    && last.step == step
                    && last.body == body
                {
                    last.cut = Some(c);
                }
            }
            _ => {}
        }
    }
    out
}

/// Where 0x5a6930's sweep after the frame puts a box body that moved
/// `from -> to` (cod4-gsc.md 3.6.1): the body line is dumped before it. A
/// box's size is half its least side; under twice `phys_csl` (1) a point
/// backed off by half the size, else a box of a fifth of it.
fn swept(clip: &ClipMap, from: [f32; 3], to: [f32; 3], sides: [f32; 3]) -> [f32; 3] {
    if from == to {
        return to;
    }
    let w = |x: f32| x as f64;
    let m = (w(sides.iter().copied().fold(f32::MAX, f32::min)) * 0.5) as f32;
    let thin = 2.0 > m;
    let e = if thin {
        0.001
    } else if ((w(m) * 0.800_000_011_920_929) as f32) < 2.0 {
        ((w(m) - 2.0) as f32).max(0.001)
    } else {
        (w(m) * 0.200_000_002_980_232_24) as f32
    };
    let tr = clip.trace(
        glam::Vec3::from(from),
        glam::Vec3::from(to),
        glam::Vec3::splat(-e),
        glam::Vec3::splat(e),
        CLIP_MASK,
    );
    if tr.fraction >= 1.0 || tr.startsolid {
        return to;
    }
    let d: [f32; 3] = std::array::from_fn(|k| (w(to[k]) - w(from[k])) as f32);
    let mut frac = tr.fraction;
    if thin {
        // 0x403a90's fast inverse root: one Newton step off 0x5f3759df.
        let l2 = ((w(d[0]) * w(d[0]) + w(d[1]) * w(d[1])) + w(d[2]) * w(d[2])) as f32;
        let y = f32::from_bits(0x5f37_59df - (l2.to_bits() >> 1));
        let inv = (w(y) * (1.5 - w((w(l2) * 0.5) as f32) * w(y) * w(y))) as f32;
        frac = ((w(frac) - 0.5 * w(m) * w(inv)) as f32).max(0.0);
    }
    std::array::from_fn(|k| (w(from[k]) + w(d[k]) * w(frac)) as f32)
}

/// How far a port contact is from a captured one: the largest difference
/// over pos, normal and depth, and whether every word is equal.
fn diff(p: &ContactGeom, c: &[u32; 12]) -> (f32, bool) {
    let cap = [c[0], c[1], c[2], c[4], c[5], c[6], c[8]].map(f32::from_bits);
    let port = [
        p.pos[0],
        p.pos[1],
        p.pos[2],
        p.normal[0],
        p.normal[1],
        p.normal[2],
        p.depth,
    ];
    let d = (0..7).map(|i| (port[i] - cap[i]).abs()).fold(0.0, f32::max);
    let exact = (0..7).all(|i| port[i].to_bits() == cap[i].to_bits());
    (d, exact)
}

/// The largest difference over two lists of equal length, `None` when the
/// counts or a surface differ.
fn list_diff(port: &[ContactGeom], cap: &[[u32; 12]]) -> Option<(f32, bool)> {
    if port.len() != cap.len() {
        return None;
    }
    let mut worst = (0.0f32, true);
    for (p, c) in port.iter().zip(cap) {
        if p.surface != c[11] {
            return None;
        }
        let (d, e) = diff(p, c);
        worst = (worst.0.max(d), worst.1 && e);
    }
    Some(worst)
}

const TOL: f32 = 1e-4;

/// Collides whose pose the capture does not hold to the bit: the sweep
/// after the frame (0x5a6930) put the body where vcod's trace lands an ulp
/// off retail's (3.6.4).
const POSE_ULP: &[(&str, u32, u32)] = &[("watermelon-mp_bog-1.7.txt", 28, 4)];

/// Cuts `cut` gives in another order than 0x5a2320 on contacts equal to
/// the bit: its f32 dots tie the third seed differently (3.6.4).
const CUT_ORDER: &[(&str, u32, u32)] = &[("watermelon-mp_bog-1.7.txt", 13, 5)];

#[derive(Default, Debug)]
struct Tally {
    collides: usize,
    raw_ok: usize,
    raw_exact: usize,
    cut_seen: usize,
    cut_ok: usize,
}

fn print(tag: &str, port: &[ContactGeom], cap: &[[u32; 12]]) {
    for p in port {
        println!(
            "  {tag} port {:?} {:?} {} {:x}",
            p.pos, p.normal, p.depth, p.surface
        );
    }
    for w in cap {
        let v = w.map(f32::from_bits);
        println!(
            "  {tag} cap  {:?} {:?} {} {:x}",
            &v[0..3],
            &v[4..7],
            v[8],
            w[11]
        );
    }
}

/// Every box collide of capture `name` against the port: the raw contacts
/// within [`TOL`] and in order, and the cut, but for the known cases.
fn run(name: &str, clip: &ClipMap) -> Tally {
    let text = std::fs::read_to_string(format!("{FIXTURES}/{name}")).unwrap();
    let mut t = Tally::default();
    let mut bad = Vec::new();
    for c in box_collides(&text, clip) {
        let key = (name, c.step, c.body);
        t.collides += 1;
        let b = BoxGeom::new(c.pos, &c.r, c.sides, c.pos, NARROW_MAX_LENGTH);
        let raw = collide_world(clip, &b, CLIP_MASK, MAX_CONTACTS);
        let ok = match list_diff(&raw, &c.raw) {
            Some((d, exact)) if d <= TOL => {
                t.raw_ok += 1;
                t.raw_exact += usize::from(exact);
                true
            }
            r => {
                println!(
                    "{name} step {} body {}: raw {} vs {} {r:?}",
                    c.step,
                    c.body,
                    raw.len(),
                    c.raw.len()
                );
                print("raw", &raw, &c.raw);
                if !POSE_ULP.contains(&key) {
                    bad.push(key);
                }
                false
            }
        };
        if let Some(cap_cut) = &c.cut {
            t.cut_seen += 1;
            let port_cut = if raw.len() >= 5 {
                cut(&raw)
            } else {
                raw.clone()
            };
            if list_diff(&port_cut, cap_cut).is_some_and(|(d, _)| d <= TOL) {
                t.cut_ok += 1;
            } else {
                println!("{name} step {} body {}: cut", c.step, c.body);
                print("cut", &port_cut, cap_cut);
                if ok && !CUT_ORDER.contains(&key) {
                    bad.push(key);
                }
            }
        }
    }
    println!("{name}: {t:?}");
    assert!(t.collides > 0 && bad.is_empty(), "{name}: {bad:?}");
    t
}

/// Each capture's box collides, fed the captured pose (and the frame's
/// sweep), give retail's raw contacts and its cut.
#[test]
fn box_contacts_match_retails_collides() {
    let Some(install) = cod4_install() else {
        return;
    };
    let mut maps: HashMap<&str, ClipMap> = HashMap::new();
    let mut exact = (0, 0);
    for (file, map) in [
        ("round-mp_crash-1.7.txt", "mp_crash"),
        ("frag-mp_crash-1.7.txt", "mp_crash"),
        ("car-mp_backlot-1.7.txt", "mp_backlot"),
        ("watermelon-mp_bog-1.7.txt", "mp_bog"),
    ] {
        let clip = maps.entry(map).or_insert_with(|| {
            let (_, walked) = Map::load(&install, "english", map).unwrap();
            ClipMap::from_assets(&walked.assets).unwrap()
        });
        let t = run(file, clip);
        exact = (exact.0 + t.raw_exact, exact.1 + t.collides);
    }
    println!("{} of {} collides equal to the bit", exact.0, exact.1);
    assert!(exact.0 + 2 >= exact.1, "{exact:?}");
}
