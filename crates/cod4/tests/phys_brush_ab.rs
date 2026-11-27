//! `phys_brush_ab`: the brush geom collider (`ode::collide::brushgeom`,
//! cod4-gsc.md 3.6.4) against retail's `dCollide` on every brush geom
//! step of the car take (`fixtures/phys/car-mp_backlot-1.7.txt`,
//! cod4-gsc.md 3.6.2): the sedan's hood and doors at the captured pose,
//! their raw contacts and 0x5a2320's cut. `COD4_DIR` unset passes
//! vacuously.

use std::collections::BTreeMap;

use vcod_cod4::assets::model::PhysBrush;
use vcod_cod4::collision::{ClipMap, Piece};
use vcod_cod4::map::Map;
use vcod_cod4::ode::collide::brushgeom::{self, BrushGeom, NARROW_MAX_LENGTH, collide_world};
use vcod_cod4::ode::collide::{ContactGeom, MAX_CONTACTS, cut};
use vcod_cod4::ode::rotation::q_to_r;
use vcod_cod4::ragdoll::CLIP_MASK;
use vcod_cod4::testing::cod4_install;
use vcod_cod4::walk::{WalkOptions, walk};
use vcod_cod4::zone::Zone;

static TAKE: vcod_common::manifest::ManifestPath =
    vcod_common::manifest_const!("/tests/fixtures/phys/car-mp_backlot-1.7.txt");
/// The take's brush parts by body id (phys_create's pins).
const PARTS: [(u32, &str); 3] = [
    (5, "vehicle_80s_sedan1_silv_door_rb"),
    (6, "vehicle_80s_sedan1_silv_door_rf"),
    (9, "vehicle_80s_sedan1_silv_hood"),
];
const TOL: f32 = 1e-4;

fn f(w: &str) -> f32 {
    f32::from_bits(u32::from_str_radix(w, 16).expect("hex word"))
}

fn contact(line: &str) -> ContactGeom {
    let w: Vec<&str> = line.split_whitespace().skip(1).collect();
    ContactGeom {
        pos: [f(w[0]), f(w[1]), f(w[2])],
        normal: [f(w[4]), f(w[5]), f(w[6])],
        depth: f(w[8]),
        surface: u32::from_str_radix(w[11], 16).unwrap(),
    }
}

/// One `collide` of a brush geom: the body's pose then, the raw contacts
/// and the contact pass's input.
struct Step {
    step: u32,
    body: u32,
    pos: [f32; 3],
    r: [[f32; 3]; 3],
    raw: Vec<ContactGeom>,
    cut: Vec<ContactGeom>,
}

/// The take's brush geom steps and each brush body's centre (geom +0x50).
fn steps(text: &str) -> (Vec<Step>, BTreeMap<u32, [f32; 3]>) {
    let mut pose: BTreeMap<u32, ([f32; 3], [[f32; 3]; 3])> = BTreeMap::new();
    let mut offsets = BTreeMap::new();
    let mut out: Vec<Step> = Vec::new();
    let mut lines = text.lines().peekable();
    while let Some(line) = lines.next() {
        let t: Vec<&str> = line.split_whitespace().collect();
        let parts: Vec<&str> = line.split('|').collect();
        match t.first().copied() {
            Some("new") => {
                let id = t[1].parse().unwrap();
                let field = |name: &str| -> Vec<f32> {
                    let p = parts[1..]
                        .iter()
                        .find(|p| p.trim().starts_with(name))
                        .unwrap();
                    p.split_whitespace().skip(1).map(f).collect()
                };
                let (p, r) = (field("pos "), field("R "));
                let r = std::array::from_fn(|i| std::array::from_fn(|j| r[4 * i + j]));
                pose.insert(id, ([p[0], p[1], p[2]], r));
            }
            Some("geom") => {
                let g: Vec<&str> = parts[1].split_whitespace().collect();
                if g[1] == "0000000c" {
                    let id = t[1].parse().unwrap();
                    offsets.insert(id, [f(g[20]), f(g[21]), f(g[22])]);
                }
            }
            Some("body") => {
                let id = t[1].parse().unwrap();
                let p: Vec<f32> = parts[1].split_whitespace().map(f).collect();
                let q: Vec<f32> = parts[2].split_whitespace().map(f).collect();
                pose.insert(id, ([p[0], p[1], p[2]], q_to_r([q[0], q[1], q[2], q[3]])));
            }
            Some("collide") if t[4] == "12" => {
                let (step, body) = (t[1].parse().unwrap(), t[2].parse().unwrap());
                let n: usize = t[6].parse().unwrap();
                let raw = (0..n).map(|_| contact(lines.next().unwrap())).collect();
                let mut cut = Vec::new();
                if lines.peek().is_some_and(|l| l.starts_with("cut ")) {
                    let c: Vec<&str> = lines.next().unwrap().split_whitespace().collect();
                    let n: usize = c[4].parse().unwrap();
                    cut = (0..n).map(|_| contact(lines.next().unwrap())).collect();
                }
                let (pos, r) = pose[&body];
                out.push(Step {
                    step,
                    body,
                    pos,
                    r,
                    raw,
                    cut,
                });
            }
            _ => {}
        }
    }
    (out, offsets)
}

/// The largest difference between two contact lists of one length, and
/// whether every word is equal; `None` when the counts or surfaces differ.
fn compare(ours: &[ContactGeom], theirs: &[ContactGeom]) -> Option<(f32, bool)> {
    if ours.len() != theirs.len() {
        return None;
    }
    let (mut worst, mut exact) = (0.0f32, true);
    for (a, b) in ours.iter().zip(theirs) {
        if a.surface != b.surface {
            return None;
        }
        for (x, y) in a
            .pos
            .iter()
            .chain(&a.normal)
            .chain([&a.depth])
            .zip(b.pos.iter().chain(&b.normal).chain([&b.depth]))
        {
            worst = worst.max((x - y).abs());
            exact &= x.to_bits() == y.to_bits() || (*x == 0.0 && *y == 0.0);
        }
    }
    Some((worst, exact))
}

fn brush_of(models: &[&vcod_cod4::assets::Assets], name: &str) -> PhysBrush {
    let m = models
        .iter()
        .find_map(|a| a.xmodels.iter().find(|m| m.name == name))
        .unwrap_or_else(|| panic!("{name}"));
    let list = m.phys_geoms.as_ref().expect("physGeoms");
    list.geoms
        .iter()
        .find_map(|g| g.brush.clone())
        .expect("a brush geom")
}

/// Every hood and door step of the car take: the port's raw contacts
/// equal the capture's `dCollide` output in count, order and surface
/// flags with position, normal and depth within 1e-4, and their cut
/// (0x5a2320 at five or more) the contact pass's input. All of them match
/// word for word.
#[test]
fn the_car_takes_brush_geoms_collide_as_retail() {
    let Some(install) = cod4_install() else {
        return;
    };
    let (_, walked) = Map::load(&install, "english", "mp_backlot").unwrap();
    let clip = ClipMap::from_assets(&walked.assets).unwrap();
    let common = walk(
        Zone::open_path(&install.zone("english", "common_mp")).unwrap(),
        WalkOptions { keep_bulk: false },
    )
    .unwrap()
    .assets;
    let zones = [&walked.assets, &common];
    let brushes: BTreeMap<u32, PhysBrush> = PARTS
        .iter()
        .map(|&(id, name)| (id, brush_of(&zones, name)))
        .collect();
    let (steps, offsets) = steps(&std::fs::read_to_string(&TAKE).unwrap());
    assert_eq!(offsets.keys().copied().collect::<Vec<_>>(), [5, 6, 9]);
    let (mut raw_ok, mut raw_exact, mut cut_ok, mut cut_exact, mut contacts) = (0, 0, 0, 0, 0);
    let (mut met_tris, mut tri_contacts) = (0, 0);
    let mut worst = 0.0f32;
    let mut bad = Vec::new();
    for s in &steps {
        let g = BrushGeom::new(
            &brushes[&s.body],
            offsets[&s.body],
            s.pos,
            &s.r,
            NARROW_MAX_LENGTH,
        );
        let ours = collide_world(&clip, &g, CLIP_MASK, MAX_CONTACTS);
        let (lo, hi) = g.bounds();
        let pieces = clip.world_pieces(glam::Vec3::from(g.pos), lo, hi, CLIP_MASK);
        for p in &pieces {
            if let Piece::Tris { tris: t, surface } = p {
                met_tris += 1;
                let mut scratch = Vec::new();
                brushgeom::tris(&mut scratch, MAX_CONTACTS, &g, t, *surface);
                tri_contacts += scratch.len();
            }
        }
        contacts += s.raw.len();
        match compare(&ours, &s.raw) {
            Some((d, exact)) if d <= TOL => {
                raw_ok += 1;
                raw_exact += usize::from(exact);
                worst = worst.max(d);
            }
            r => bad.push(format!(
                "step {} body {}: raw {} vs retail {} ({r:?})",
                s.step,
                s.body,
                ours.len(),
                s.raw.len()
            )),
        }
        let ours_cut = if ours.len() >= 5 { cut(&ours) } else { ours };
        if let Some((d, exact)) = compare(&ours_cut, &s.cut)
            && d <= TOL
        {
            cut_ok += 1;
            cut_exact += usize::from(exact);
        } else {
            bad.push(format!(
                "step {} body {}: cut {} vs retail {}",
                s.step,
                s.body,
                ours_cut.len(),
                s.cut.len()
            ));
        }
    }
    println!(
        "{} brush geom steps ({met_tris} partitions in reach, {tri_contacts} contacts off \
         them), {contacts} retail \
         contacts: raw {raw_ok} within {TOL} ({raw_exact} word for word, worst {worst:e}), \
         cut {cut_ok} ({cut_exact} word for word)",
        steps.len()
    );
    for b in &bad {
        println!("  {b}");
    }
    assert_eq!(steps.len(), 120);
    assert!(bad.is_empty(), "{} mismatches", bad.len());
    assert_eq!((raw_exact, cut_exact), (steps.len(), steps.len()));
}
