//! `phys_ab`: retail 1.7's worlds 0 and 1 against `vcod_cod4::ode::phys`
//! on the takes of `fixtures/phys/` (cod4-gsc.md 3.6.2, 3.6.9): each take's
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
use vcod_cod4::fx_phys::{self, EffectFrame, PhysEffect};
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

/// An `fxbody` line: 0x4a1ff0's element index and seed, and the effect's
/// first 0x60 bytes as words.
#[derive(Clone, Debug, Default)]
struct FxLine {
    elem: usize,
    seed: u16,
    effect: Vec<u32>,
}

#[derive(Debug, Default)]
struct Take {
    world: u32,
    seed: u32,
    msec_step: i32,
    news: Vec<New>,
    steps: Vec<Step>,
    fx: Vec<FxLine>,
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
            Some("fxbody") => t.fx.push(FxLine {
                elem: w[2].parse().unwrap(),
                seed: w[4].parse().unwrap(),
                effect: line.split('|').nth(1).unwrap()[" effect ".len()..]
                    .split_whitespace()
                    .map(hex)
                    .collect(),
            }),
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
    let frag: Vec<String> = [
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
    .to_vec();
    vec![
        Case {
            file: "round-mp_crash-1.7.txt",
            map: "mp_crash",
            models: vec!["bog_b_cinderblock_clutter".into()],
        },
        Case {
            file: "frag-mp_crash-1.7.txt",
            map: "mp_crash",
            models: frag.clone(),
        },
        Case {
            file: "shards-mp_crash-1.7.txt",
            map: "mp_crash",
            models: frag.clone(),
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
        Case {
            file: "copypaper-mp_broadcast-1.7.txt",
            map: "mp_broadcast",
            models: vec![
                "com_copypaper_box_lid".into(),
                "com_copypaper_box_open".into(),
            ],
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
/// or the cut): every body's position and quaternion retail's to the bit
/// and its flags retail's on every step (so each sleeps on retail's step),
/// every `msecStep` and every seed after a step retail's, and the impact
/// sounds retail's in count on every step, under one alias per retail
/// handle and one handle per alias, each at retail's point.
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
        check(case.file, &r);
    }
}

/// The gate's rule over a run of take `f` (see the test above).
fn check(f: &str, r: &Run) {
    let w = worst(&r.diffs);
    for (k, d) in r.diffs.iter().enumerate() {
        if !d.exact || d.flags > 0 {
            eprintln!("  step {}: {d:?}", k + 1);
        }
    }
    assert!(w.exact, "{f}: {w:?}");
    assert_eq!(w.flags, 0, "{f}: steps whose flags part");
    assert!(
        r.asleep.values().all(|(a, b)| a == b),
        "{f}: {:?}",
        r.asleep
    );
    assert_eq!(r.seed_parts, None, "{f}: the seed parts after a step");
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
    assert_eq!(r.sound_pos, 0.0, "{f}: sound point");
}

/// The ODE words a body starts with, as a `new` line gives them.
fn start_words(b: &Body) -> Vec<u32> {
    let mut v: Vec<u32> = b.pos.iter().chain(&b.q).map(|x| x.to_bits()).collect();
    v.extend(b.lvel.iter().chain(&b.avel).map(|x| x.to_bits()));
    v
}

/// An effect's take spawned through the FX path (cod4-effects.md 9.1.1):
/// `effect` played at the pose of the map's dynamic entity at `at`, as its
/// `destroyFx` (0x48deb1..0x48decf), its one-shot physics elements'
/// instances seeded off the one effect seed (searched over all 479: the
/// take's seed and start time enter only as their sum), each spawned and
/// built by `fx_phys` with its visual's own model and preset. A take with
/// `fxbody` lines gives the effect's frame, seed and start instead, and
/// each element's seed is checked against them. Every body's position,
/// quaternion and velocities equal the `new` line's to the bit, each picks
/// the model `pins` names, and the world they make then passes
/// [`the_phys_takes_against_the_port`]'s rule to the take's end.
fn fx_take(map_name: &str, effect: &str, at: Vec3, file: &str, pins: &[String]) {
    let Some(install) = cod4_install() else {
        return;
    };
    let startup: Vec<Assets> = STARTUP.iter().map(|n| walked(&install, n)).collect();
    let z = Zone::open_path(&install.zone("english", map_name)).unwrap();
    let map = walk(z, WalkOptions { keep_bulk: true }).unwrap().assets;
    let (_, mw) = vcod_cod4::map::Map::load(&install, "english", map_name).unwrap();
    let clip = ClipMap::from_assets(&mw.assets).unwrap();
    let zones: Vec<&Assets> = std::iter::once(&map).chain(startup.iter()).collect();
    let rows = impact_sound_rows(startup.iter().chain(std::iter::once(&map)).flat_map(|z| {
        z.phys_presets
            .iter()
            .filter(|p| !p.name.starts_with(','))
            .map(|p| (p.name.as_str(), p.snd_alias_prefix.as_deref()))
    }));
    let sounds: HashSet<&str> = zones
        .iter()
        .flat_map(|z| z.sounds.iter())
        .filter(|s| !s.name.starts_with(','))
        .map(|s| s.name.as_str())
        .collect();
    let aliases = impact_aliases(&rows.prefixes, |n| sounds.contains(n));
    let model = |name: &str| {
        zones.iter().find_map(|z| {
            let m = z.xmodels.iter().find(|m| m.name == name)?;
            let p = &z.phys_presets[m.phys_preset?.index()].name;
            let p = zones
                .iter()
                .find_map(|z| z.phys_presets.iter().find(|q| q.name == *p))?;
            let row = rows.by_preset.get(&p.name.to_ascii_lowercase()).copied();
            Some((m, ObjPreset::from_raw(&p.raw, row.unwrap_or(-1))?))
        })
    };
    let fx = map.fx.iter().find(|f| f.name == effect).unwrap();
    let find = |n: &str| map.fx.iter().find(|f| f.name.eq_ignore_ascii_case(n));
    let effect = PhysEffect::of(&map, fx, &find).expect("a physics element");
    assert!(!effect.timed_seed, "a rand() seed");
    assert!(effect.elems.iter().all(|e| !e.looping), "one-shot elements");
    let word = |d: &vcod_cod4::assets::world::DynEntityDef, o: usize| {
        f32::from_le_bytes(d.raw[o..o + 4].try_into().unwrap())
    };
    let clip_assets = map.clip_maps_pvs.first().unwrap();
    let dynent = clip_assets.dyn_ents[0]
        .iter()
        .find(|d| {
            d.destroy_fx
                .is_some_and(|i| map.fx[i.index()].name == fx.name)
                && Vec3::new(word(d, 20), word(d, 24), word(d, 28)).distance(at) < 0.1
        })
        .unwrap();
    let word = |o: usize| word(dynent, o);
    let text = std::fs::read_to_string(format!("{FIXTURES}/{file}")).unwrap();
    let t = parse(&text);
    let (frame, seeds, start) = match t.fx.first() {
        // the effect's own +0x28 quaternion and +0x38 origin (0x4a13d9..0x4a13ef),
        // read through 0x561100; its +0x12 seed and +0x20 start
        Some(l) => {
            let e = |k: usize| f32::from_bits(l.effect[k]);
            let frame = EffectFrame {
                origin: [e(14), e(15), e(16)],
                axes: vcod_cod4::ragdoll::iw::unit_axes([e(10), e(11), e(12), e(13)]),
            };
            let seed = (l.effect[4] >> 16) as u16;
            eprintln!(
                "{file}: the effect at {:?}, the dynamic entity's def at {:?}",
                frame.origin,
                [word(20), word(24), word(28)]
            );
            (frame, vec![seed], l.effect[8] as i32)
        }
        None => {
            // 0x48deb1..0x48decf: the pose's axes off 0x561100 into the effect
            let axis = vcod_cod4::ragdoll::iw::unit_axes([word(4), word(8), word(12), word(16)]);
            let frame = EffectFrame::of_axis([word(20), word(24), word(28)], &axis);
            (frame, (0..479u16).collect(), 0)
        }
    };
    let captured: Vec<Vec<u32>> = t
        .news
        .iter()
        .map(|n| {
            ["pos", "q", "lvel", "avel"]
                .iter()
                .flat_map(|l| n.parts[*l].clone())
                .collect()
        })
        .collect();
    // 0x4a3220 for each one-shot element in turn, the bodies in that order
    let spawns = |seed: u16| -> Vec<(usize, u16, fx_phys::Spawn)> {
        let mut out = Vec::new();
        for (i, e) in effect.elems.iter().enumerate() {
            for k in 0..fx_phys::one_shot_count(&e.def, seed) {
                let (_, s) = fx_phys::elem_seed(&e.def, seed, start, k as u32);
                out.push((i, s, fx_phys::spawn(&e.def, e.models.len(), &frame, s)));
            }
        }
        out
    };
    // Each seed's farthest spawn from its body's start, the body sitting
    // at its mass centre a hair off the spawn; a take's own seed is taken
    // as it stands (the copy paper's centres sit 3.9 units off).
    let off = |k: u16| -> f32 {
        let s = spawns(k);
        if s.len() != captured.len() {
            return f32::INFINITY;
        }
        s.iter()
            .zip(&captured)
            .map(|((_, _, s), c)| {
                Vec3::from(s.pos).distance(Vec3::new(
                    f32::from_bits(c[0]),
                    f32::from_bits(c[1]),
                    f32::from_bits(c[2]),
                ))
            })
            .fold(0.0, f32::max)
    };
    let found: Vec<u16> = match t.fx.is_empty() {
        true => seeds.iter().copied().filter(|&k| off(k) < 1e-3).collect(),
        false => seeds.clone(),
    };
    let best = seeds
        .iter()
        .copied()
        .min_by(|&a, &b| off(a).total_cmp(&off(b)))
        .unwrap();
    eprintln!("{file}: best seed {best}: {}", off(best));
    eprintln!("{file}: effect seed plus start time {found:?}");
    assert_eq!(found.len(), 1, "one seed spawns the take's positions");
    let mut pw = PhysWorld::new(WorldId::Fx);
    let dv = PhysDvars::default();
    let mut ids = Vec::new();
    let mut mismatched = 0;
    for (i, (e, seed, s)) in spawns(found[0]).iter().enumerate() {
        if let Some(l) = t.fx.get(i) {
            assert_eq!(
                (effect.elems[*e].index, *seed),
                (l.elem, l.seed),
                "body {}'s element and seed",
                i + 1
            );
        }
        let name = &effect.elems[*e].models[s.visual];
        assert_eq!(name, &pins[i], "body {}'s visual", i + 1);
        let (m, preset) = model(name).expect("a preset");
        let id = pw.fx_body(s, m, &preset, &dv).expect("room");
        let words = start_words(pw.world.body(id));
        if words != captured[i] {
            let fl = |v: &[u32]| v.iter().map(|&x| f32::from_bits(x)).collect::<Vec<_>>();
            eprintln!(
                "body {}: ours {:?}\n  retail {:?}\n  spawn {s:?}",
                i + 1,
                fl(&words),
                fl(&captured[i])
            );
        }
        mismatched += usize::from(words != captured[i]);
        ids.push(id);
    }
    assert_eq!(mismatched, 0, "bodies whose start words part");
    pw.world.seed = t.seed;
    let first = &t.steps[0];
    pw.clock.now = first.sim - first.msec;
    pw.clock.last = pw.clock.now;
    pw.clock.msec_step = t.msec_step;
    let r = run(&t, &mut pw, &ids, &clip, &aliases);
    let first_inexact = r.diffs.iter().position(|d| !d.exact).map(|k| k + 1);
    eprintln!(
        "{file} through the FX path: {} steps, worst {:?}, first inexact step {first_inexact:?}",
        r.diffs.len(),
        worst(&r.diffs)
    );
    check(&format!("{file} (FX path)"), &r);
}

/// Scenario 4 through the FX path: `props/watermelon`'s element 3, six
/// instances under run bits 0xc0 off a sphere offset.
#[test]
fn the_watermelon_spawned_through_the_fx_path() {
    let at = Vec3::new(3672.6, -32.2, 43.5);
    let pins = &cases()
        .into_iter()
        .find(|c| c.file.starts_with("watermelon"))
        .unwrap()
        .models;
    fx_take(
        "mp_bog",
        "props/watermelon",
        at,
        "watermelon-mp_bog-1.7.txt",
        pins,
    );
}

/// Scenario 5 through the FX path: `props/copypaper_box_exp`'s elements 4
/// and 5, one instance each under run bits 0x40, their origins taken into
/// and out of the frame of a box turned 32 degrees (0x4a2220, 0x4a2021).
#[test]
fn the_copy_paper_box_spawned_through_the_fx_path() {
    let at = Vec3::new(-201.184, 300.911, 0.0);
    let pins = &cases()
        .into_iter()
        .find(|c| c.file.starts_with("copypaper"))
        .unwrap()
        .models;
    fx_take(
        "mp_broadcast",
        "props/copypaper_box_exp",
        at,
        "copypaper-mp_broadcast-1.7.txt",
        pins,
    );
}

/// Each body's model in a world 0 take: the dynamic entity whose origin is
/// the body's position plus its offset (userdata +0) turned by its
/// rotation, within 0.05.
fn dynent_models(t: &Take, defs: &[vcod_cod4::dynent::DynDef]) -> Vec<String> {
    t.news
        .iter()
        .map(|n| {
            let fl = |label: &str| -> Vec<f32> {
                n.parts[label].iter().map(|&x| f32::from_bits(x)).collect()
            };
            let (pos, r) = (fl("pos"), fl("R"));
            let off: Vec<f32> = n.user[..3].iter().map(|&x| f32::from_bits(x)).collect();
            let origin = Vec3::from(std::array::from_fn::<f32, 3, _>(|i| {
                pos[i] + (0..3).map(|j| r[4 * i + j] * off[j]).sum::<f32>()
            }));
            let d = defs
                .iter()
                .filter(|d| d.body.is_some())
                .min_by(|a, b| {
                    a.origin
                        .distance(origin)
                        .total_cmp(&b.origin.distance(origin))
                })
                .unwrap();
            assert!(
                d.origin.distance(origin) < 0.05,
                "body {}: {origin} {}",
                n.id,
                d.origin
            );
            d.model.clone()
        })
        .collect()
}

/// The mp_shipment frag (`fixtures/phys/`, cod4-gsc.md 3.6.9): eleven
/// bottles and fruit flung off the floor by one blast, the last asleep on
/// retail's step 1212 (20.6 s), held to [`check`]'s rule on the steps the
/// take keeps (every tenth, and each body's first and first asleep).
#[test]
fn the_shipment_frag_against_the_port() {
    let Some(install) = cod4_install() else {
        return;
    };
    let startup: Vec<Assets> = STARTUP.iter().map(|n| walked(&install, n)).collect();
    let z = Zone::open_path(&install.zone("english", "mp_shipment")).unwrap();
    let map = walk(z, WalkOptions { keep_bulk: true }).unwrap().assets;
    let (_, mw) = vcod_cod4::map::Map::load(&install, "english", "mp_shipment").unwrap();
    let clip = ClipMap::from_assets(&mw.assets).unwrap();
    let refs: Vec<&Assets> = startup.iter().collect();
    let defs = vcod_cod4::dynent::defs(&map, &refs);
    let text = std::fs::read_to_string(format!("{FIXTURES}/frag-mp_shipment-1.7.txt")).unwrap();
    let t = parse(&text);
    let names = dynent_models(&t, &defs);
    eprintln!("shipment models {names:?}");
    let zones: Vec<&Assets> = std::iter::once(&map).chain(startup.iter()).collect();
    let models: Vec<_> = names
        .iter()
        .map(|n| {
            zones
                .iter()
                .find_map(|z| z.xmodels.iter().find(|m| &m.name == n))
                .expect("model")
        })
        .collect();
    let (mut pw, ids) = build(&t, &models);
    let r = run(&t, &mut pw, &ids, &clip, &[]);
    eprintln!(
        "shipment: {} steps, worst {:?}; asleep (ours, retail's) {:?}",
        r.diffs.len(),
        worst(&r.diffs),
        r.asleep
    );
    check("frag-mp_shipment-1.7.txt", &r);
}
