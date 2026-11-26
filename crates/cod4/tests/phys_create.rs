//! Worlds 0 and 1's bodies as `Phys_ObjCreate` and its collision build
//! them (`vcod_cod4::ode::obj`, cod4-gsc.md 3.6.3), and what the stock
//! zones hand that build. `COD4_DIR` unset passes vacuously.

use std::collections::BTreeMap;
use std::sync::Mutex;
use vcod_cod4::assets::Assets;
use vcod_cod4::assets::fx::FxVisual;
use vcod_cod4::assets::model::XModel;
use vcod_cod4::ode::geom::Geom;
use vcod_cod4::ode::obj::{ObjPreset, impact_sound_rows};
use vcod_cod4::ode::phys::{PhysWorld, WorldId};
use vcod_cod4::ode::world::{BodyId, PhysDvars};
use vcod_cod4::testing::cod4_install;
use vcod_cod4::walk::{WalkOptions, walk};
use vcod_cod4::zone::Zone;

/// Every `physGeoms` entry of every stock zone, by what 0x5a5710 makes of
/// it: a brush (its `PhysGeomInfo.type` alongside), or its type; and the
/// lists by length. No type but 1 and 4 outside a brush, so nothing takes
/// 0x5a55f0's cylinder on another type.
#[test]
fn the_stock_zones_phys_geoms_are_brushes_boxes_and_cylinders() {
    let Some(install) = cod4_install() else {
        return;
    };
    let paths = install.zones("english").unwrap();
    let next = Mutex::new(paths.iter());
    // By model name: one zone's copy stands for every zone's.
    let models: Mutex<BTreeMap<String, Vec<String>>> = Mutex::default();
    let threads = std::thread::available_parallelism()
        .map_or(1, |n| n.get())
        .min(4);
    std::thread::scope(|s| {
        for _ in 0..threads {
            s.spawn(|| {
                loop {
                    let Some(path) = next.lock().unwrap().next() else {
                        break;
                    };
                    let z = Zone::open_path(path).unwrap();
                    let a = walk(z, WalkOptions { keep_bulk: false }).unwrap().assets;
                    for m in a.xmodels.iter().filter(|m| !m.name.starts_with(',')) {
                        let Some(list) = &m.phys_geoms else { continue };
                        let k = list.geoms.iter().map(|g| match g.brush {
                            Some(_) => format!("brush type {}", g.kind),
                            None => format!("type {}", g.kind),
                        });
                        models.lock().unwrap().insert(m.name.clone(), k.collect());
                    }
                }
            });
        }
    });
    let models = models.into_inner().unwrap();
    let (mut kinds, mut lens) = (
        BTreeMap::<&str, usize>::new(),
        BTreeMap::<usize, usize>::new(),
    );
    for k in models.values() {
        *lens.entry(k.len()).or_default() += 1;
        for k in k {
            *kinds.entry(k).or_default() += 1;
        }
    }
    println!(
        "{} zones, {} models with physGeoms: geoms {kinds:?}, lists by length {lens:?}",
        paths.len(),
        models.len()
    );
    let want: BTreeMap<&str, usize> = [("brush type 0", 95), ("type 1", 72), ("type 4", 82)].into();
    assert_eq!(kinds, want);
    assert_eq!(
        lens,
        [(1, 175), (2, 10), (3, 10), (4, 2), (5, 2), (6, 1)].into()
    );
}

/// One body's creation lines in a `fixtures/phys/` take.
#[derive(Debug, Default)]
struct Captured {
    id: u32,
    creator: String,
    mass: Vec<u32>,
    inv: Vec<u32>,
    pos: Vec<u32>,
    r: Vec<u32>,
    user: Vec<u32>,
    geom: Vec<u32>,
    inner: Vec<u32>,
    brush: Vec<u32>,
}

fn hex(w: &str) -> u32 {
    u32::from_str_radix(w, 16).expect("hex word")
}

fn words(part: &str) -> Vec<u32> {
    part.split_whitespace().map(hex).collect()
}

/// The world and each body's `new`, `user`, `geom`, `inner` and `brush`
/// lines (README.md beside the takes).
fn captured(text: &str) -> (u32, Vec<Captured>) {
    let (mut world, mut out) = (0, Vec::<Captured>::new());
    for line in text.lines() {
        let t: Vec<&str> = line.split_whitespace().collect();
        let parts: Vec<&str> = line.split('|').collect();
        match t.first().copied() {
            Some("world") if out.is_empty() => world = t[1].parse().unwrap(),
            Some("new") => {
                let mut c = Captured {
                    id: t[1].parse().unwrap(),
                    creator: t[3].to_string(),
                    ..Default::default()
                };
                for p in &parts[1..] {
                    let (label, rest) = p.trim().split_once(' ').unwrap();
                    let v = words(rest);
                    match label {
                        "mass" => c.mass = v,
                        "inv" => c.inv = v,
                        "pos" => c.pos = v,
                        "R" => c.r = v,
                        _ => {}
                    }
                }
                out.push(c);
            }
            Some("user") => {
                out.last_mut().unwrap().user = words(&line[line.find(' ').unwrap()..])[1..].to_vec()
            }
            Some("geom") => out.last_mut().unwrap().geom = words(parts[1]),
            Some("inner") => out.last_mut().unwrap().inner = words(parts[1]),
            Some("brush") => out.last_mut().unwrap().brush = words(&line[5..]),
            Some("step") => break,
            _ => {}
        }
    }
    (world, out)
}

fn walked(install: &vcod_cod4::install::Install, name: &str) -> Assets {
    let z = Zone::open_path(&install.zone("english", name)).unwrap();
    walk(z, WalkOptions { keep_bulk: false }).unwrap().assets
}

fn f(w: u32) -> f32 {
    f32::from_bits(w)
}

/// The words a geom holds from +4 as 0x5a9790 and its class's creator
/// leave them, by byte offset; the pointers (+0x10 the body's, the inner
/// geom's, the brush's) are checked apart.
fn geom_words(g: &Geom, on_body: bool) -> Vec<(usize, u32)> {
    let mut v = vec![(4, g.class() as u32), (8, 0xf), (0x14, 0)];
    v.extend((0x2c..0x44).step_by(4).map(|o| (o, 0)));
    v.extend([(0x44, u32::MAX), (0x48, u32::MAX)]);
    if !on_body {
        v.push((0x10, 0));
    }
    fn fl(o: usize, x: &[f32]) -> impl Iterator<Item = (usize, u32)> + '_ {
        x.iter()
            .enumerate()
            .map(move |(k, x)| (o + 4 * k, x.to_bits()))
    }
    match g {
        Geom::Box { sides } => v.extend(fl(0x4c, sides)),
        Geom::Brush { offset, .. } => v.extend(fl(0x50, offset)),
        Geom::Cylinder {
            direction,
            radius,
            half_height,
        } => {
            v.push((0x4c, *direction as u32));
            v.extend(fl(0x50, &[*radius, *half_height]));
        }
        Geom::Transform(t) => {
            v.extend([(0x50, 1), (0x54, 0)]);
            for (i, row) in t.r.iter().enumerate() {
                v.extend(fl(0x58 + 0x10 * i, row));
                v.push((0x64 + 0x10 * i, 0));
            }
            v.extend(fl(0x88, &t.pos));
            // the final rotation the ctor sets to the identity, its first
            // word cleared by the frame's position written as four words
            let id = [0.0f32, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0];
            v.extend(fl(0x94, &id));
        }
        Geom::Capsule { .. } => unreachable!("worlds 0 and 1 make no capsule"),
    }
    v
}

/// Where `got` differs from the capture, as readable lines.
fn diffs(c: &Captured, pw: &PhysWorld, id: BodyId) -> Vec<String> {
    let mut out = Vec::new();
    let mut cmp = |what: &str, k: usize, want: u32, got: u32| {
        if want != got {
            out.push(format!(
                "{what}[{k:#x}] retail {want:08x} ({}) port {got:08x} ({})",
                f(want),
                f(got)
            ));
        }
    };
    let b = pw.world.body(id);
    for (k, (&want, got)) in c.mass.iter().zip(b.mass.words()).enumerate() {
        cmp("mass", k, want, got);
    }
    let mut inv: Vec<u32> = Vec::new();
    for row in &b.inv_i {
        inv.extend(row.iter().map(|x| x.to_bits()));
        inv.push(0);
    }
    inv.push(b.inv_mass.to_bits());
    for (k, (&want, got)) in c.inv.iter().zip(inv).enumerate() {
        cmp("inv", k, want, got);
    }
    let o = pw.obj(id).unwrap();
    for k in 0..3 {
        cmp("user", 4 * k, c.user[k], o.offset[k].to_bits());
    }
    cmp("user", 0x40, c.user[16], o.sound as u32);
    cmp("user", 0x44, c.user[17], o.obj.friction.to_bits());
    cmp("user", 0x48, c.user[18], o.obj.bounce.to_bits());
    cmp("user", 0x4c, c.user[19], o.obj.state as u32);
    cmp("geoms", 0, 1, o.geoms.len() as u32);
    let Some(g) = o.geom() else { return out };
    // a class the capture has no inner or brush line for reads as zeros
    let word = |v: &[u32], k: usize| v.get(k).copied().unwrap_or(0);
    for (off, want) in geom_words(g, true) {
        cmp("geom", off, c.geom[off / 4], want);
    }
    let no_body = c.geom[4] == 0;
    match g {
        Geom::Transform(t) => {
            for (off, want) in geom_words(&t.inner, false) {
                cmp("inner", off, word(&c.inner, off / 4), want);
            }
            for off in [0x18, 0x1c] {
                cmp("inner", off, c.geom[off / 4], word(&c.inner, off / 4));
            }
        }
        Geom::Brush { brush, .. } => {
            for k in 0..3 {
                cmp("brush", 4 * k, word(&c.brush, k), brush.mins[k].to_bits());
                cmp(
                    "brush",
                    0x10 + 4 * k,
                    word(&c.brush, 4 + k),
                    brush.maxs[k].to_bits(),
                );
            }
        }
        _ => {}
    }
    if no_body {
        out.push("geom +0x10: no body".into());
    }
    out
}

/// Each fixture body built through the port from its model and preset at
/// the captured pose: its 17 `dMass` words (and the 13 of the inverse
/// `dBodySetMass` keeps), the phys object's offset, sound row, friction,
/// bounce and state, and its geom's words, inner geom's and brush's
/// bounds equal the capture's. Which model: a dynamic entity's is the def
/// at the origin the capture's pose and offset give; a launched part's is
/// searched among the sedans' parts (`vehicle_80s_sedan1*`) and an FX
/// chunk's among the watermelon effect's model visuals, and the one
/// pinned must be among those that build it.
#[test]
fn fixture_bodies_build_word_for_word() {
    let Some(install) = cod4_install() else {
        return;
    };
    let startup: Vec<Assets> = [
        "code_post_gfx_mp",
        "localized_code_post_gfx_mp",
        "common_mp",
        "localized_common_mp",
    ]
    .iter()
    .map(|n| walked(&install, n))
    .collect();
    // Each body and the model the search below finds for it. The car is
    // the silver sedan; its colours' parts build alike. Both tyres are the
    // left front wheel's model. The FX system picks a chunk's visual at
    // random: the one listed is the only visual of the effect that builds
    // it.
    let sedan = |p: &str| format!("vehicle_80s_sedan1_silv_{p}");
    let chunk = |n: u32| format!("fx_watermelonchunk_0{n}");
    let takes: [(&str, &str, Vec<String>); 4] = [
        (
            "round-mp_crash-1.7.txt",
            "mp_crash",
            vec!["bog_b_cinderblock_clutter".into()],
        ),
        (
            "frag-mp_crash-1.7.txt",
            "mp_crash",
            [
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
        ),
        (
            "car-mp_backlot-1.7.txt",
            "mp_backlot",
            [
                "mirror_r", "mirror_l", "bumper_b", "bumper_f", "door_rb", "door_rf", "wheel_lf",
                "wheel_lf", "hood",
            ]
            .map(sedan)
            .to_vec(),
        ),
        (
            "watermelon-mp_bog-1.7.txt",
            "mp_bog",
            [4, 1, 4, 4, 1, 6].map(chunk).to_vec(),
        ),
    ];
    let mut failed = Vec::new();
    for (file, map_name, pins) in takes {
        let text = std::fs::read_to_string(format!(
            "{}/tests/fixtures/phys/{file}",
            env!("CARGO_MANIFEST_DIR")
        ))
        .unwrap();
        let (world, bodies) = captured(&text);
        let map = walked(&install, map_name);
        let zones: Vec<&Assets> = std::iter::once(&map).chain(startup.iter()).collect();
        let load_order = startup.iter().chain(std::iter::once(&map));
        let rows = impact_sound_rows(load_order.flat_map(|z| {
            z.phys_presets
                .iter()
                .filter(|p| !p.name.starts_with(','))
                .map(|p| (p.name.as_str(), p.snd_alias_prefix.as_deref()))
        }));
        let model = |name: &str| {
            zones
                .iter()
                .find_map(|z| z.xmodels.iter().find(|m| m.name == name))
        };
        let preset = |name: &str| -> Option<ObjPreset> {
            let n = name.trim_start_matches(',');
            let p = zones
                .iter()
                .find_map(|z| z.phys_presets.iter().find(|p| p.name == n))?;
            ObjPreset::from_raw(
                &p.raw,
                rows.get(&n.to_ascii_lowercase()).copied().unwrap_or(-1),
            )
        };
        let model_preset = |m: &XModel| {
            let z = zones
                .iter()
                .find(|z| z.xmodels.iter().any(|x| std::ptr::eq(x, m)))?;
            preset(&z.phys_presets[m.phys_preset?.index()].name)
        };
        // The candidates by name: (model, preset).
        let candidates = |c: &Captured| -> Vec<(String, &XModel, ObjPreset)> {
            match c.creator.as_str() {
                "dynent" => {
                    let r: Vec<f64> = c.r.iter().map(|&x| f(x) as f64).collect();
                    let o: Vec<f64> = (0..3)
                        .map(|i| {
                            f(c.pos[i]) as f64
                                + (0..3)
                                    .map(|j| r[4 * i + j] * f(c.user[j]) as f64)
                                    .sum::<f64>()
                        })
                        .collect();
                    let clip = map.clip_maps_pvs.first().unwrap();
                    clip.dyn_ents[0]
                        .iter()
                        .filter(|d| {
                            (0..3).all(|k| {
                                let x = f32::from_le_bytes(
                                    d.raw[20 + 4 * k..24 + 4 * k].try_into().unwrap(),
                                );
                                (x as f64 - o[k]).abs() < 0.5
                            })
                        })
                        .filter_map(|d| {
                            let name = map.xmodels[d.model?.index()].name.trim_start_matches(',');
                            let p = preset(&map.phys_presets[d.preset?.index()].name)?;
                            Some((name.to_string(), model(name)?, p))
                        })
                        .collect()
                }
                creator => {
                    let names: Vec<String> = if creator == "launch" {
                        zones
                            .iter()
                            .flat_map(|z| z.xmodels.iter())
                            .filter(|m| m.name.starts_with("vehicle_80s_sedan1"))
                            .map(|m| m.name.clone())
                            .collect()
                    } else {
                        map.fx
                            .iter()
                            .filter(|x| x.name.contains("watermelon"))
                            .flat_map(|x| x.elems.iter())
                            .flat_map(|e| e.visuals.iter())
                            .filter_map(|v| match v {
                                FxVisual::Model(Some(m)) => {
                                    Some(map.xmodels[m.index()].name.clone())
                                }
                                _ => None,
                            })
                            .collect()
                    };
                    let mut out: Vec<(String, &XModel, ObjPreset)> = Vec::new();
                    for n in names {
                        let n = n.trim_start_matches(',');
                        if out.iter().any(|(x, ..)| x == n) {
                            continue;
                        }
                        if let Some(m) = model(n)
                            && let Some(p) = model_preset(m)
                        {
                            out.push((n.to_string(), m, p));
                        }
                    }
                    out
                }
            }
        };
        let world = if world == 0 {
            WorldId::Dynent
        } else {
            WorldId::Fx
        };
        for c in &bodies {
            let pos: [f32; 3] = std::array::from_fn(|k| f(c.pos[k]));
            let axes: [[f32; 3]; 3] =
                std::array::from_fn(|i| std::array::from_fn(|j| f(c.r[4 * j + i])));
            let mut best: Option<(String, Vec<String>)> = None;
            let mut matched = Vec::new();
            for (name, m, p) in candidates(c) {
                let mut pw = PhysWorld::new(world);
                let id = pw.obj_create(pos, &axes, [0.0; 3], &p, &PhysDvars::default());
                if c.creator == "dynent" {
                    pw.dynent_collision(id, m, p.cylinder);
                } else {
                    pw.set_collision_from_xmodel(id, m);
                }
                let d = diffs(c, &pw, id);
                if d.is_empty() {
                    matched.push(name);
                } else if best.as_ref().is_none_or(|(_, b)| d.len() < b.len()) {
                    best = Some((name, d));
                }
            }
            println!("{file} body {} ({}): {matched:?}", c.id, c.creator);
            let pin = &pins[c.id as usize - 1];
            assert!(
                matched.is_empty() || matched.contains(pin),
                "{file} body {}: {pin} not among {matched:?}",
                c.id
            );
            if matched.is_empty() {
                let (name, d) = best.unwrap_or_default();
                println!("  closest {name}:\n    {}", d.join("\n    "));
                failed.push(format!("{file} body {}", c.id));
            }
        }
    }
    assert!(failed.is_empty(), "no model builds {failed:?}");
}
