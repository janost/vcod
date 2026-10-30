//! The UO vehicle step (`game::vehicle::physics`) against two retail
//! captures of a driven vehicle: a Panzer IV on mp_cassino (entity 186)
//! and a Willys on mp_peaks (entity 110). Each snapshot carries the
//! vehicle's every non-zero entity field (`veh=`); one 50 ms server frame
//! is one step, driven by the last cmd the frame ran (`st == ct`) and the
//! snapshot's `ps.viewangles`.
//!
//! Two replays: a free run from the first snapshot, and anchored, which
//! steps each frame from retail's wire pose with the free run's state for
//! what the wire does not carry. `VEH_AB_REPORT=1` prints the per-field
//! tallies and every mismatch.

use std::collections::HashMap;

use glam::Vec3;
use vcod_common::collision::CollisionWorld;
use vcod_common::x87::angles_to_axis;
use vcod_server::game::vehicle::info::{VehicleInfo, VehicleInfos};
use vcod_server::game::vehicle::physics::{
    CollisionVehicleWorld, DriveInput, Physics, StepInput, StepOutput, VehicleModel, an180,
};

const TANK: &str = "mp_cassino-tdm-slope-8ms-tank-drive-uo.txt";
const JEEP: &str = "mp_peaks-tdm-slope-8ms-jeep-drive-uo.txt";

/// Wire fields the gate compares, with the tolerance a row passes within.
const FIELDS: [(&str, f32); 12] = [
    ("pos.trBase[0]", 0.01),
    ("pos.trBase[1]", 0.01),
    ("pos.trBase[2]", 0.01),
    ("apos.trBase[0]", 0.05),
    ("apos.trBase[1]", 0.01),
    ("apos.trBase[2]", 0.05),
    ("angles2[0]", 0.01),
    ("angles2[1]", 0.01),
    ("origin2[0]", 1.0),
    ("origin2[2]", 1.0),
    ("time", 1.0),
    ("leanf", 0.05),
];
const INT_FIELDS: [&str; 4] = ["origin2[0]", "origin2[2]", "time", "time2"];

#[derive(Clone, Debug)]
struct Snap {
    t: i32,
    ct: i32,
    view: [f32; 3],
    veh: HashMap<String, i64>,
}

#[derive(Clone, Copy, Debug)]
struct Cmd {
    st: i32,
    buttons: i32,
    forward: i8,
    right: i8,
    up: i8,
}

fn kv<'a>(line: &'a str, key: &str) -> Option<&'a str> {
    line.split_whitespace()
        .find_map(|w| w.strip_prefix(key)?.strip_prefix('='))
}

fn vec3(s: &str) -> [f32; 3] {
    let v: Vec<f32> = s.split(',').map(|x| x.parse().unwrap()).collect();
    [v[0], v[1], v[2]]
}

fn parse(text: &str) -> (Vec<Snap>, Vec<Cmd>) {
    let (mut snaps, mut cmds) = (Vec::new(), Vec::new());
    for line in text.lines() {
        if line.starts_with("!cmd") {
            let i = |k| kv(line, k).unwrap().parse::<i32>().unwrap();
            cmds.push(Cmd {
                st: i("st"),
                buttons: i("buttons"),
                forward: i("forward") as i8,
                right: i("right") as i8,
                up: i("up") as i8,
            });
        } else if line.starts_with("!snap") {
            let Some(veh) = kv(line, "veh") else {
                continue;
            };
            let veh = veh
                .split(',')
                .filter_map(|f| {
                    let (k, v) = f.rsplit_once(':')?;
                    Some((k.to_string(), v.parse().ok()?))
                })
                .collect();
            snaps.push(Snap {
                t: kv(line, "t").unwrap().parse().unwrap(),
                ct: kv(line, "ct").unwrap().parse().unwrap(),
                view: vec3(kv(line, "view").unwrap()),
                veh,
            });
        }
    }
    (snaps, cmds)
}

impl Snap {
    fn f(&self, k: &str) -> f32 {
        match self.veh.get(k) {
            // `leanf` of a centred wheel is -0.0 (`-steer`), which the
            // capture carries as bit pattern 1.
            Some(1) if k == "leanf" => -0.0,
            Some(&b) => f32::from_bits(b as u32),
            None => 0.0,
        }
    }
    fn i(&self, k: &str) -> i64 {
        self.veh.get(k).copied().unwrap_or(0)
    }
    fn origin(&self) -> Vec3 {
        Vec3::new(
            self.f("pos.trBase[0]"),
            self.f("pos.trBase[1]"),
            self.f("pos.trBase[2]"),
        )
    }
    fn angles(&self) -> Vec3 {
        Vec3::new(
            self.f("apos.trBase[0]"),
            self.f("apos.trBase[1]"),
            self.f("apos.trBase[2]"),
        )
    }
    /// A field as the gate compares it: floats as values, ints as ints.
    fn value(&self, k: &str) -> f32 {
        if INT_FIELDS.contains(&k) {
            self.i(k) as f32
        } else {
            self.f(k)
        }
    }
}

/// Our step's fields under the wire's names.
fn ours(p: &Physics, k: &str) -> f32 {
    match k {
        "pos.trBase[0]" => p.s.origin.x,
        "pos.trBase[1]" => p.s.origin.y,
        "pos.trBase[2]" => p.s.origin.z,
        "apos.trBase[0]" => p.s.angles.x,
        "apos.trBase[1]" => p.s.angles.y,
        "apos.trBase[2]" => p.s.angles.z,
        "angles2[0]" => p.angles2[0],
        "angles2[1]" => p.angles2[1],
        "origin2[0]" => p.sway_packed[0] as f32,
        "origin2[2]" => p.sway_packed[1] as f32,
        "time" => (p.time & 0xffff) as f32,
        "time2" => (p.time2 & 63) as f32,
        "leanf" => p.leanf,
        _ => unreachable!("{k}"),
    }
}

fn diff(k: &str, a: f32, b: f32) -> f32 {
    if k.starts_with("apos") || k.starts_with("angles2") {
        let mut d = a - b;
        while d > 180.0 {
            d -= 360.0;
        }
        while d < -180.0 {
            d += 360.0;
        }
        d.abs()
    } else if k == "time" {
        let d = (a as i32 - b as i32).rem_euclid(65536);
        d.min(65536 - d) as f32
    } else {
        (a - b).abs()
    }
}

struct Case {
    info: VehicleInfo,
    model: VehicleModel,
    world: CollisionWorld,
    bounds_maxs: Vec3,
    /// The BSP's spawn origin and angles, before the first think's settle.
    bsp_pose: (Vec3, Vec3),
    snaps: Vec<Snap>,
    cmds: Vec<Cmd>,
}

/// The map's collision world with every collmap's brush model unlinked:
/// retail links each on its vehicle, and the driven one is passed by its
/// own traces; the other vehicles stand off these routes.
fn load(fixture: &str, map: &str, vehicletype: &str, near: Vec3) -> Option<Case> {
    let fs = vcod_common::testing::uo_fs()?;
    let path = format!(
        "{}/tests/fixtures/playerstate/{fixture}",
        env!("CARGO_MANIFEST_DIR")
    );
    let text = std::fs::read_to_string(&path).unwrap();
    let (snaps, cmds) = parse(&text);
    let bsp_path = fs.resolve_map(map).expect("map in the paks");
    let bsp = vcod_common::bsp::parse(&fs.read(&bsp_path).unwrap()).unwrap();
    let world = vcod_server::world::World::from_bsp(&bsp, Some(&fs)).collision;
    world.unlink_script_brushes(&bsp.entities, "tdm");
    let blocks = vcod_common::bsp::entity_blocks(&bsp.entities);
    let model_num =
        |b: &HashMap<String, String>| b.get("model")?.strip_prefix('*')?.parse::<usize>().ok();
    let vehicle = blocks
        .iter()
        .filter(|b| {
            b.get("classname").is_some_and(|c| c == "script_vehicle")
                && b.get("vehicletype")
                    .is_some_and(|v| v.eq_ignore_ascii_case(vehicletype))
        })
        .min_by(|a, b| {
            let d = |e: &HashMap<String, String>| {
                let o = vcod_common::bsp::parse_vec3(e.get("origin").unwrap()).unwrap();
                Vec3::from(o).distance(near)
            };
            d(a).total_cmp(&d(b))
        })
        .expect("the vehicle in the ents");
    let xmodel = vehicle.get("model").unwrap().clone();
    let mut bounds_maxs = Vec3::ZERO;
    for b in &blocks {
        if b.get("classname")
            .is_some_and(|c| c == "script_vehicle_collmap")
            && let Some(n) = model_num(b)
        {
            world.set_model_linked(n, false);
            if b.get("targetname")
                .is_some_and(|t| t.eq_ignore_ascii_case(&xmodel))
            {
                bounds_maxs = Vec3::from(bsp.models[n].maxs);
            }
        }
    }
    let infos = VehicleInfos::load(&fs);
    let info = infos.list[infos.find(vehicletype).unwrap()].clone();
    let name = xmodel.strip_prefix("xmodel/").unwrap_or(&xmodel);
    let bones = vcod_common::model::load_bones(&fs, name).unwrap();
    let model = VehicleModel::from_bones(&bones);
    let pose = (
        Vec3::from(vcod_common::bsp::parse_vec3(vehicle.get("origin").unwrap()).unwrap()),
        Vec3::from(vcod_common::bsp::parse_vec3(vehicle.get("angles").unwrap()).unwrap()),
    );
    Some(Case {
        info,
        model,
        world,
        bounds_maxs,
        bsp_pose: pose,
        snaps,
        cmds,
    })
}

impl Case {
    fn vworld(&self) -> CollisionVehicleWorld<'_> {
        CollisionVehicleWorld {
            world: &self.world,
            skip: None,
        }
    }

    /// The step's input for the frame snapshot `k` closed: the cmd that
    /// frame ran last and the view the frame left.
    fn input(&self, k: usize) -> StepInput {
        let s = &self.snaps[k];
        // A cmd whose serverTime repeats the last one's never runs.
        let last = self.cmds.iter().rev().find(|c| c.st <= s.ct).map(|c| c.st);
        let cmd = self
            .cmds
            .iter()
            .find(|c| Some(c.st) == last)
            .copied()
            .unwrap_or(Cmd {
                st: 0,
                buttons: 0,
                forward: 0,
                right: 0,
                up: 0,
            });
        StepInput {
            now: s.t,
            driver: Some(DriveInput {
                forward: cmd.forward,
                right: cmd.right,
                up: cmd.up,
                buttons: cmd.buttons,
                view: s.view,
                ads: false,
                popped_out: false,
            }),
            driven: true,
            health: 1000,
            bounds_maxs: self.bounds_maxs,
            max_clients: 8,
            ..StepInput::default()
        }
    }

    /// The driven stretch: the snapshots that carry the seat's vehicle,
    /// which the probe prints only while seated.
    fn driven_len(&self) -> usize {
        self.snaps.len()
    }

    /// Retail's wire state put back on ours before the frame after
    /// snapshot `k`: the pose, the turret, the clock, the steer.
    /// The record at the first snapshot, parked.
    fn start(&self) -> Physics {
        let s0 = &self.snaps[0];
        let mut p = Physics::spawn(s0.origin(), s0.angles(), self.model);
        self.put_wire(&mut p, 0);
        p.committed = p.s;
        p
    }

    fn put_wire(&self, p: &mut Physics, k: usize) {
        let s = &self.snaps[k];
        p.s.origin = s.origin();
        p.s.angles = s.angles();
        p.angles2 = [s.f("angles2[0]"), s.f("angles2[1]"), s.f("angles2[2]")];
        p.gunner_yaw = s.f("origin2[1]");
        p.time = s.i("time") as i32;
        if self.info.steer_wheels {
            p.s.steer = -s.f("leanf");
        }
    }
}

/// Per field: rows compared, exact, within tolerance; and the misses.
#[derive(Default)]
struct Tally {
    rows: HashMap<&'static str, (usize, usize, usize)>,
    /// Every row off retail: time, field, ours, retail, how far.
    off: Vec<(i32, &'static str, f32, f32, f32)>,
}

impl Tally {
    fn add(&mut self, t: i32, k: &'static str, tol: f32, ours: f32, retail: f32) {
        let e = self.rows.entry(k).or_default();
        e.0 += 1;
        let d = diff(k, ours, retail);
        if d == 0.0 {
            e.1 += 1;
        } else {
            self.off.push((t, k, ours, retail, d));
        }
        if d <= tol {
            e.2 += 1;
        }
    }

    fn tol(k: &str) -> f32 {
        FIELDS
            .iter()
            .find(|(f, _)| *f == k)
            .map_or(0.0, |(_, t)| *t)
    }

    /// Rows past their tolerance.
    fn misses(&self) -> impl Iterator<Item = &(i32, &'static str, f32, f32, f32)> {
        self.off.iter().filter(|r| r.4 > Self::tol(r.1))
    }

    fn report(&self, name: &str) {
        if std::env::var_os("VEH_AB_REPORT").is_none() {
            return;
        }
        eprintln!("== {name}");
        for (k, _) in FIELDS {
            if let Some((n, exact, near)) = self.rows.get(k) {
                eprintln!("{k:16} {n:4} rows {exact:4} exact {near:4} within");
            }
        }
        for (t, k, ours, retail, d) in self.misses() {
            eprintln!("  t {t} {k}: ours {ours} retail {retail} (off {d})");
        }
    }

    /// Every miss outside `gaps`, and every inexact row of an `exact`
    /// field outside them.
    fn assert_gaps(&self, name: &str, gaps: &[(i32, i32, &str)], exact: &[&str]) {
        let gap = |t: i32| gaps.iter().any(|&(a, b, _)| (a..=b).contains(&t));
        let bad: Vec<String> = self
            .off
            .iter()
            .filter(|r| !gap(r.0) && (r.4 > Self::tol(r.1) || exact.contains(&r.1)))
            .map(|(t, k, ours, retail, d)| {
                format!("t {t} {k}: ours {ours} retail {retail} (off {d})")
            })
            .collect();
        assert!(
            bad.is_empty(),
            "{name}: {} rows off outside the gaps:\n{}",
            bad.len(),
            bad.join("\n")
        );
    }
}

/// Every snapshot after the first is one frame on.
fn assert_frames(case: &Case, n: usize) {
    for w in case.snaps[..n].windows(2) {
        assert_eq!(w[1].t - w[0].t, 50, "a frame missing before t {}", w[1].t);
    }
}

/// The vehicle's event ring took the main gun's fire event (163) in the
/// frame snapshot `k` closed.
fn fired(case: &Case, k: usize) -> bool {
    let (s, q) = (&case.snaps[k], &case.snaps[k - 1]);
    let seq = s.i("eventSequence");
    seq != q.i("eventSequence") && s.i(&format!("events[{}]", (seq - 1) & 3)) == 163
}

/// One think, with the main gun's jolt in the frame it fired: the
/// barrel's world forward reversed, 0.75 (weapons doc 1.2), between the
/// motion and the aim as retail's driver fire sits.
fn think(case: &Case, vw: &CollisionVehicleWorld<'_>, p: &mut Physics, k: usize) -> StepOutput {
    let input = case.input(k);
    let mut out = p.motion(&case.info, vw, &input);
    if fired(case, k) {
        let hull = angles_to_axis(p.s.angles + Vec3::new(p.s.sway_pitch, 0.0, p.s.sway_roll));
        let f = angles_to_axis(Vec3::new(p.angles2[0], p.angles2[1], 0.0))[0];
        let fwd = hull[0] * f.x + hull[1] * f.y + hull[2] * f.z;
        p.jolt(&case.info, -fwd, 0.75, 0.0, 0.0, input.now);
    }
    p.finish(&case.info, &input, &mut out);
    out
}

/// Each step from retail's wire pose, with the state the wire does not
/// carry taken from the free run at the same frame.
fn anchored(case: &Case) -> Tally {
    let n = case.driven_len();
    let vw = case.vworld();
    let mut f = case.start();
    let mut tally = Tally::default();
    for k in 1..n {
        let mut p = f.clone();
        case.put_wire(&mut p, k - 1);
        think(case, &vw, &mut p, k);
        think(case, &vw, &mut f, k);
        let s = &case.snaps[k];
        for (key, tol) in FIELDS {
            tally.add(s.t, key, tol, ours(&p, key), s.value(key));
        }
    }
    tally
}

fn free_run(case: &Case) -> Tally {
    let n = case.driven_len();
    let vw = case.vworld();
    let mut p = case.start();
    let mut tally = Tally::default();
    for k in 1..n {
        think(case, &vw, &mut p, k);
        let s = &case.snaps[k];
        for (key, tol) in FIELDS {
            tally.add(s.t, key, tol, ours(&p, key), s.value(key));
        }
    }
    tally
}

fn tank() -> Option<Case> {
    load(
        TANK,
        "mp_cassino",
        "panzerIV_mp",
        Vec3::new(4676.0, -1979.0, 0.0),
    )
}

fn jeep() -> Option<Case> {
    load(
        JEEP,
        "mp_peaks",
        "willyjeep_mp",
        Vec3::new(-4160.0, 162.0, -110.0),
    )
}

/// `Scr_Vehicle_Init`'s settle puts the Panzer from the BSP's z 4 on its
/// wheels at -7.875, level, as every snapshot of it carries; and the think
/// writes `time2` 7000, which the 6-bit wire field keeps as 24.
#[test]
fn the_panzer_settles_where_retail_parks_it() {
    let Some(case) = tank() else {
        return;
    };
    let (o, a) = case.bsp_pose;
    let mut p = Physics::spawn(o, a, case.model);
    p.init(&case.info, &case.vworld(), false);
    let s0 = &case.snaps[0];
    assert_eq!(p.s.origin, s0.origin(), "settled origin");
    assert_eq!(p.s.angles.x, 0.0);
    assert_eq!(p.s.angles.z, 0.0);
    // Retail parks it at -180, the same yaw.
    assert_eq!(
        diff("apos.trBase[1]", an180(p.s.angles.y), s0.angles().y),
        0.0
    );
    let mut out = p.motion(&case.info, &case.vworld(), &case.input(1));
    p.finish(&case.info, &case.input(1), &mut out);
    assert_eq!(p.time2 & 63, s0.i("time2") as i32);
}

/// Fields every row must match bit for bit: the hull yaw (16-bit
/// quantized, which the upmove turn lands on the view), the turret yaw
/// (1.5 a step), the sway bytes and the track clock.
const EXACT: [&str; 5] = [
    "apos.trBase[1]",
    "angles2[1]",
    "origin2[0]",
    "origin2[2]",
    "time",
];

/// Each frame stepped from retail's pose: no field misses. The wire lacks
/// the vertical velocity, the sway's state and a hit's lockout and bounce;
/// the free run's carry them, so a step that reads them (the crest, the
/// clip brush bounces, the probe hits) is measured too.
#[test]
fn tank_drive_anchored() {
    let Some(case) = tank() else {
        return;
    };
    assert_frames(&case, case.driven_len());
    let tally = anchored(&case);
    tally.report("tank anchored");
    tally.assert_gaps("tank anchored", &[], &EXACT);
}

/// The free run follows retail field for field through the whole drive:
/// the flat acceleration ramp, the turret's swing, the climb, the
/// step-ups, the drop, the vehicle clip brushes it bounces off, the hull
/// turned to the view, the fire jolt and the coast.
#[test]
fn tank_drive_free_run() {
    let Some(case) = tank() else {
        return;
    };
    let tally = free_run(&case);
    tally.report("tank free run");
    tally.assert_gaps("tank free run", &[], &EXACT);
}

#[test]
fn jeep_drive_anchored() {
    let Some(case) = jeep() else {
        return;
    };
    assert_frames(&case, case.driven_len());
    let tally = anchored(&case);
    tally.report("jeep anchored");
    tally.assert_gaps("jeep anchored", &[], &EXACT);
}

/// The Willys follows retail through the whole drive: its ramp at 17.6 a
/// step, the climb over the bumps, the suspension's pitch and roll, the
/// steer, the hits and the reverse into the wall.
#[test]
fn jeep_drive_free_run() {
    let Some(case) = jeep() else {
        return;
    };
    let tally = free_run(&case);
    tally.report("jeep free run");
    tally.assert_gaps("jeep free run", &[], &EXACT);
}
