//! `phys_ab`: retail 1.7's worlds 0 and 1 against `vcod_cod4::ode::phys`
//! on the four takes of `fixtures/phys/` (cod4-gsc.md 3.6.2): each take's
//! bodies built from their models (3.6.3) and set to the capture's start
//! words, the world's seed and clock the capture's, stepped frame by frame
//! as retail stepped them over the map's clip map, and compared step by
//! step (3.6.7). A `body` line is dumped before the frame's sweep
//! (0x5a6930), so each step is read where the port's step returns, and the
//! sweep then runs as on retail. `COD4_DIR` unset passes vacuously.

use std::collections::{BTreeMap, HashMap, HashSet};

use glam::Vec3;
use vcod_cod4::assets::Assets;
use vcod_cod4::collision::ClipMap;
use vcod_cod4::ode::Body;
use vcod_cod4::ode::collide;
use vcod_cod4::ode::math::M3;
use vcod_cod4::ode::obj::{ObjPreset, impact_aliases, impact_sound_rows};
use vcod_cod4::ode::phys::{PhysWorld, WorldId};
use vcod_cod4::ode::world::{BodyId, PhysDvars};
use vcod_cod4::testing::cod4_install;
use vcod_cod4::walk::{WalkOptions, walk};
use vcod_cod4::zone::Zone;

static FIXTURES: vcod_common::manifest::ManifestPath =
    vcod_common::manifest_const!("/tests/fixtures/phys");

fn hex(w: &str) -> u32 {
    u32::from_str_radix(w, 16).expect("hex word")
}

fn f(w: &str) -> f32 {
    f32::from_bits(hex(w))
}

/// A body after a step, as the dump has it.
#[derive(Clone, Copy, Debug)]
struct Rec {
    flags: u32,
    pos: [f32; 3],
    q: [f32; 4],
}

/// One step: its length, the clock after it and the time its frame runs
/// to, the seed after it, the next step's length, its bodies by id and the
/// impact sounds the step's collides played (handle, point).
#[derive(Clone, Debug, Default)]
struct Step {
    msec: i32,
    sim: i32,
    to: i32,
    seed: u32,
    next: i32,
    bodies: BTreeMap<u32, Rec>,
    sounds: Vec<(u32, [f32; 3])>,
}

/// A body's start: the `new` line's parts by label and the `user` words.
#[derive(Clone, Debug, Default)]
struct New {
    id: u32,
    creator: String,
    flags: u32,
    parts: HashMap<String, Vec<u32>>,
    user: Vec<u32>,
}

#[derive(Debug, Default)]
struct Take {
    world: u32,
    seed: u32,
    msec_step: i32,
    news: Vec<New>,
    steps: Vec<Step>,
}

fn floats<const N: usize>(part: &str) -> [f32; N] {
    let v: Vec<f32> = part.split_whitespace().map(f).collect();
    v.try_into().expect("field width")
}

/// The take's lines, as the README beside the fixtures gives them.
fn parse(text: &str) -> Take {
    let mut t = Take::default();
    let mut sounds: BTreeMap<usize, Vec<(u32, [f32; 3])>> = BTreeMap::new();
    for line in text.lines() {
        let w: Vec<&str> = line.split_whitespace().collect();
        match w.first().copied() {
            Some("world") if t.news.is_empty() => {
                t.world = w[1].parse().unwrap();
                t.seed = hex(w[4]);
                t.msec_step = w[6].parse().unwrap();
            }
            Some("new") => {
                let parts: Vec<&str> = line.split('|').collect();
                let mut n = New {
                    id: w[1].parse().unwrap(),
                    creator: w[3].to_string(),
                    flags: hex(w[7]),
                    ..New::default()
                };
                for p in &parts[1..] {
                    let (label, rest) = p.trim().split_once(' ').unwrap();
                    n.parts.insert(
                        label.to_string(),
                        rest.split_whitespace().map(hex).collect(),
                    );
                }
                t.news.push(n);
            }
            Some("user") => {
                t.news.last_mut().unwrap().user = w[2..].iter().map(|x| hex(x)).collect()
            }
            Some("step") => t.steps.push(Step {
                msec: w[3].parse().unwrap(),
                sim: w[5].parse().unwrap(),
                to: w[7].parse().unwrap(),
                seed: hex(w[9]),
                next: w[11].parse().unwrap(),
                ..Step::default()
            }),
            Some("body") => {
                let parts: Vec<&str> = line.split('|').collect();
                t.steps.last_mut().unwrap().bodies.insert(
                    w[1].parse().unwrap(),
                    Rec {
                        flags: hex(w[2]),
                        pos: floats(parts[1]),
                        q: floats(parts[2]),
                    },
                );
            }
            Some("sound") => sounds
                .entry(w[1].parse().unwrap())
                .or_default()
                .push((hex(w[2]), [f(w[3]), f(w[4]), f(w[5])])),
            _ => {}
        }
    }
    for (k, s) in sounds {
        t.steps[k - 1].sounds = s;
    }
    // listed newest first; made, and so built, in id order
    t.news.sort_by_key(|n| n.id);
    t
}

fn walked(install: &vcod_cod4::install::Install, name: &str) -> Assets {
    let z = Zone::open_path(&install.zone("english", name)).unwrap();
    walk(z, WalkOptions { keep_bulk: false }).unwrap().assets
}

/// The startup zones the cgame registers its presets off before the map's.
const STARTUP: [&str; 4] = [
    "code_post_gfx_mp",
    "localized_code_post_gfx_mp",
    "common_mp",
    "localized_common_mp",
];

/// A take, its map, and each body's model in id order: the ones
/// `phys_create::fixture_bodies_build_word_for_word` pins.
struct Case {
    file: &'static str,
    map: &'static str,
    models: Vec<String>,
}

fn cases() -> Vec<Case> {
    let sedan = |p: &str| format!("vehicle_80s_sedan1_silv_{p}");
    let chunk = |n: u32| format!("fx_watermelonchunk_0{n}");
    vec![
        Case {
            file: "round-mp_crash-1.7.txt",
            map: "mp_crash",
            models: vec!["bog_b_cinderblock_clutter".into()],
        },
        Case {
            file: "frag-mp_crash-1.7.txt",
            map: "mp_crash",
            models: [
                "me_plastic_crate1",
                "com_bottle1",
                "com_soup_can",
                "com_barrel_shard4",
                "com_barrel_shard3",
                "me_plastic_crate2",
                "com_junktire",
                "com_junktire2",
            ]
            .map(String::from)
            .to_vec(),
        },
        Case {
            file: "car-mp_backlot-1.7.txt",
            map: "mp_backlot",
            models: [
                "mirror_r", "mirror_l", "bumper_b", "bumper_f", "door_rb", "door_rf", "wheel_lf",
                "wheel_lf", "hood",
            ]
            .map(sedan)
            .to_vec(),
        },
        Case {
            file: "watermelon-mp_bog-1.7.txt",
            map: "mp_bog",
            models: [4, 1, 4, 4, 1, 6].map(chunk).to_vec(),
        },
    ]
}

/// The take's world built: each body through `Phys_ObjCreate` and its
/// collision off its model, with the capture's mass, friction, bounce and
/// sound row (3.6.3 matched them word for word), then set to the `new`
/// line's flags, idle counters, position, rotation, velocities and the
/// push already in its accumulators; the seed and clock the capture's.
fn build(t: &Take, models: &[&vcod_cod4::assets::model::XModel]) -> (PhysWorld, Vec<BodyId>) {
    let world = if t.world == 0 {
        WorldId::Dynent
    } else {
        WorldId::Fx
    };
    let dv = PhysDvars::default();
    let mut pw = PhysWorld::new(world);
    let mut ids = Vec::new();
    for (n, m) in t.news.iter().zip(models) {
        let fl = |label: &str| -> Vec<f32> {
            n.parts[label].iter().map(|&x| f32::from_bits(x)).collect()
        };
        let preset = ObjPreset {
            sound: n.user[16] as i32,
            mass: fl("mass")[0],
            bounce: f32::from_bits(n.user[18]),
            friction: f32::from_bits(n.user[17]),
            cylinder: false,
        };
        let pos: [f32; 3] = fl("pos").try_into().unwrap();
        let r = fl("R");
        let rows: M3 = std::array::from_fn(|i| std::array::from_fn(|j| r[4 * i + j]));
        let axes: M3 = std::array::from_fn(|i| std::array::from_fn(|j| rows[j][i]));
        let id = pw
            .obj_create(pos, &axes, [0.0; 3], &preset, &dv)
            .expect("room");
        if n.creator == "dynent" {
            pw.dynent_collision(id, m, false);
        } else {
            pw.set_collision_from_xmodel(id, m);
        }
        let idle = &n.parts["idle"];
        let b = pw.world.body_mut(id);
        b.flags = n.flags;
        b.adis_timeleft = f32::from_bits(idle[2]);
        b.adis_stepsleft = idle[3] as i32;
        b.pos = pos;
        b.q = fl("q").try_into().unwrap();
        b.r = rows;
        b.lvel = fl("lvel").try_into().unwrap();
        b.avel = fl("avel").try_into().unwrap();
        b.facc = fl("force").try_into().unwrap();
        b.tacc = fl("torque").try_into().unwrap();
        ids.push(id);
    }
    pw.world.seed = t.seed;
    let first = &t.steps[0];
    pw.clock.now = first.sim - first.msec;
    pw.clock.last = pw.clock.now;
    pw.clock.msec_step = t.msec_step;
    (pw, ids)
}

/// One step's worst difference from the capture over its bodies: the
/// distance, the turn, the bodies whose flags part and whether every
/// position and quaternion word is retail's.
#[derive(Clone, Copy, Debug, Default)]
struct Diff {
    pos: f32,
    turn_deg: f64,
    flags: usize,
    exact: bool,
}

/// What a run shows against its take.
#[derive(Debug, Default)]
struct Run {
    diffs: Vec<Diff>,
    /// The first step whose seed after it parts from retail's.
    seed_parts: Option<usize>,
    /// Steps whose next `msecStep` parts from retail's.
    msec_parts: Vec<usize>,
    /// The step each body first reads asleep, ours and retail's, by id.
    asleep: BTreeMap<u32, (Option<usize>, Option<usize>)>,
    /// Steps whose sounds part in count (step, ours, retail's).
    sounds_part: Vec<(usize, usize, usize)>,
    /// The port's alias names met under each retail handle, on one step.
    handles: BTreeMap<u32, HashSet<Option<String>>>,
    sounds: usize,
    /// The farthest a sound's mean point lay from retail's.
    sound_pos: f32,
}

/// The angle between two ODE quaternions (`w` first), in `f64` and each
/// normalized: `f32`'s `acos` reads 0.05 degrees between equal ones.
fn turn_deg(a: [f32; 4], b: [f32; 4]) -> f64 {
    let dot = |a: [f32; 4], b: [f32; 4]| (0..4).map(|k| a[k] as f64 * b[k] as f64).sum::<f64>();
    let d = dot(a, b).abs() / (dot(a, a) * dot(b, b)).sqrt();
    (2.0 * d.min(1.0).acos()).to_degrees()
}

/// Each body at a step's end.
fn snap(pw: &PhysWorld, ids: &[BodyId]) -> Vec<Body> {
    ids.iter().map(|&id| pw.world.body(id).clone()).collect()
}

/// The take run frame by frame to its last step.
fn run(
    t: &Take,
    pw: &mut PhysWorld,
    ids: &[BodyId],
    clip: &ClipMap,
    aliases: &[Option<String>],
) -> Run {
    let dv = PhysDvars::default();
    let contact = collide::Dvars::default();
    let mut out = Run::default();
    for n in &t.news {
        out.asleep.insert(n.id, (None, None));
    }
    for (k, step) in t.steps.iter().enumerate() {
        let mut seen = Vec::new();
        pw.run_to_time_with(step.to, &dv, &contact, clip, &mut |_| {}, &mut |pw| {
            seen.push((snap(pw, ids), pw.world.seed, pw.clock.msec_step));
        });
        assert_eq!(seen.len(), 1, "step {}: one step a frame", k + 1);
        let (bodies, seed, next) = &seen[0];
        if *seed != step.seed {
            out.seed_parts.get_or_insert(k + 1);
        }
        if *next != step.next {
            out.msec_parts.push(k + 1);
        }
        let mut d = Diff {
            exact: true,
            ..Diff::default()
        };
        for (&id, r) in &step.bodies {
            let o = &bodies[id as usize - 1];
            d.pos = d.pos.max(Vec3::from(o.pos).distance(Vec3::from(r.pos)));
            d.turn_deg = d.turn_deg.max(turn_deg(o.q, r.q));
            d.flags += usize::from(o.flags != r.flags);
            d.exact &= o.pos.map(f32::to_bits) == r.pos.map(f32::to_bits)
                && o.q.map(f32::to_bits) == r.q.map(f32::to_bits);
            if r.flags & 4 != 0 {
                out.asleep.get_mut(&id).unwrap().1.get_or_insert(k + 1);
            }
        }
        for (i, o) in bodies.iter().enumerate() {
            if o.flags & 4 != 0 {
                out.asleep
                    .get_mut(&(i as u32 + 1))
                    .unwrap()
                    .0
                    .get_or_insert(k + 1);
            }
        }
        out.diffs.push(d);
        if pw.impacts.len() != step.sounds.len() {
            out.sounds_part
                .push((k + 1, pw.impacts.len(), step.sounds.len()));
            continue;
        }
        for (i, (h, at)) in pw.impacts.iter().zip(&step.sounds) {
            let alias = aliases.get(i.slot).cloned().flatten();
            out.handles.entry(*h).or_default().insert(alias);
            out.sounds += 1;
            out.sound_pos = out
                .sound_pos
                .max(Vec3::from(i.pos).distance(Vec3::from(*at)));
        }
    }
    out
}

fn worst(diffs: &[Diff]) -> Diff {
    diffs.iter().fold(
        Diff {
            exact: true,
            ..Diff::default()
        },
        |a, d| Diff {
            pos: a.pos.max(d.pos),
            turn_deg: a.turn_deg.max(d.turn_deg),
            flags: a.flags + d.flags,
            exact: a.exact && d.exact,
        },
    )
}

/// Each take against the port, every step to its last (each body's sleep
/// or the cut): no body more than 0.25 units or 5 degrees from retail's,
/// every body's flags retail's on every step (so each sleeps on retail's
/// step), every `msecStep` and every seed after a step retail's, and the
/// impact sounds retail's in count on every step, under one alias per
/// retail handle and one handle per alias, within 0.01 of retail's point.
/// Measured: the round, frag and car takes equal to the bit on every step;
/// the watermelon's to step 27, then within 2.5e-4 (its body 4's sweep on
/// step 27, cod4-gsc.md 3.6.5 and 3.6.7).
#[test]
fn the_phys_takes_against_the_port() {
    let Some(install) = cod4_install() else {
        return;
    };
    let startup: Vec<Assets> = STARTUP.iter().map(|n| walked(&install, n)).collect();
    let mut maps: HashMap<&str, (Assets, ClipMap)> = HashMap::new();
    for case in cases() {
        let (map, clip) = maps.entry(case.map).or_insert_with(|| {
            let a = walked(&install, case.map);
            let (_, w) = vcod_cod4::map::Map::load(&install, "english", case.map).unwrap();
            (a, ClipMap::from_assets(&w.assets).unwrap())
        });
        let zones: Vec<&Assets> = std::iter::once(&*map).chain(startup.iter()).collect();
        let presets = startup.iter().chain(std::iter::once(&*map)).flat_map(|z| {
            z.phys_presets
                .iter()
                .filter(|p| !p.name.starts_with(','))
                .map(|p| (p.name.as_str(), p.snd_alias_prefix.as_deref()))
        });
        let rows = impact_sound_rows(presets);
        let sounds: HashSet<&str> = zones
            .iter()
            .flat_map(|z| z.sounds.iter())
            .filter(|s| !s.name.starts_with(','))
            .map(|s| s.name.as_str())
            .collect();
        let aliases = impact_aliases(&rows.prefixes, |n| sounds.contains(n));
        let models: Vec<_> = case
            .models
            .iter()
            .map(|n| {
                zones
                    .iter()
                    .find_map(|z| z.xmodels.iter().find(|m| &m.name == n))
                    .expect("model")
            })
            .collect();
        let text = std::fs::read_to_string(format!("{FIXTURES}/{}", case.file)).unwrap();
        let t = parse(&text);
        let (mut pw, ids) = build(&t, &models);
        let r = run(&t, &mut pw, &ids, clip, &aliases);
        let w = worst(&r.diffs);
        let first_inexact = r.diffs.iter().position(|d| !d.exact).map(|k| k + 1);
        eprintln!(
            "{}: {} steps, worst {w:?}, first inexact step {first_inexact:?}; seed parts after {:?}; \
             asleep (ours, retail's) {:?}; {} sounds within {}; handles {:?}",
            case.file,
            r.diffs.len(),
            r.seed_parts,
            r.asleep,
            r.sounds,
            r.sound_pos,
            r.handles
        );
        for (k, d) in r.diffs.iter().enumerate() {
            if d.pos > 0.25 || d.turn_deg > 5.0 || d.flags > 0 {
                eprintln!("  step {}: {d:?}", k + 1);
            }
        }
        let f = case.file;
        assert!(w.pos <= 0.25 && w.turn_deg <= 5.0, "{f}: {w:?}");
        assert_eq!(w.flags, 0, "{f}: steps whose flags part");
        assert!(
            r.asleep.values().all(|(a, b)| a == b),
            "{f}: {:?}",
            r.asleep
        );
        assert_eq!(r.seed_parts, None, "{f}: the seed");
        assert!(r.msec_parts.is_empty(), "{f}: msecStep {:?}", r.msec_parts);
        assert!(r.sounds_part.is_empty(), "{f}: sounds {:?}", r.sounds_part);
        let names: Vec<_> = r.handles.values().collect();
        assert!(
            names.iter().all(|n| n.len() == 1 && !n.contains(&None)),
            "{f}: one alias a handle {:?}",
            r.handles
        );
        let distinct: HashSet<_> = names.iter().flat_map(|n| n.iter()).collect();
        assert_eq!(distinct.len(), names.len(), "{f}: one handle an alias");
        assert!(r.sound_pos <= 0.01, "{f}: sound point {}", r.sound_pos);
    }
}
