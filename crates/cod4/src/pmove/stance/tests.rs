//! Pins for the prone fits, the prone view, the lean and the gunner's
//! clamp, which no 1.7 capture covers yet (cod4-pmove.md 8, 18): each
//! records what the model does on a box world, so a rewrite that moves a
//! result shows here. The world is axis-aligned boxes and tilted floors,
//! not the capsule clip, so the numbers are the model's, not retail's.

use glam::Vec3;

use super::super::*;
use super::{ProneCheck, check_prone, gunner_arcs};

/// A solid shape: the half-space under a plane (`n . p < dist`) or a box.
#[derive(Clone, Copy)]
enum Solid {
    Below(Vec3, f32),
    Block(Vec3, Vec3),
}

/// The surface clip epsilon the flat floor of `pmove/tests.rs` keeps.
const EPS: f32 = 0.125;

struct World(Vec<Solid>);

impl World {
    fn sweep(s: Solid, start: Vec3, end: Vec3, mins: Vec3, maxs: Vec3) -> Tr {
        let mut t = Tr::default();
        match s {
            Solid::Below(n, dist) => {
                let off = n.x.min(0.0) * (maxs.x - mins.x)
                    + n.y.min(0.0) * (maxs.y - mins.y)
                    + n.z.min(0.0) * (maxs.z - mins.z)
                    + n.dot(mins);
                let (a, b) = (n.dot(start) + off - dist, n.dot(end) + off - dist);
                if a < 0.0 {
                    t.fraction = 0.0;
                    t.startsolid = true;
                    t.allsolid = b < 0.0;
                } else if b < EPS && b < a {
                    t.fraction = ((a - EPS) / (a - b)).max(0.0);
                } else {
                    return t;
                }
                t.normal = n;
            }
            Solid::Block(lo, hi) => {
                let (lo, hi) = (lo - maxs, hi - mins);
                let inside = |p: Vec3| p.cmpgt(lo).all() && p.cmplt(hi).all();
                if inside(start) {
                    t.fraction = 0.0;
                    t.startsolid = true;
                    t.allsolid = inside(end);
                    t.normal = Vec3::Z;
                } else {
                    let dir = end - start;
                    let (mut enter, mut exit, mut axis) = (0.0f32, 1.0f32, None);
                    for i in 0..3 {
                        if dir[i] == 0.0 {
                            if start[i] <= lo[i] || start[i] >= hi[i] {
                                return t;
                            }
                            continue;
                        }
                        let (a, b) = ((lo[i] - start[i]) / dir[i], (hi[i] - start[i]) / dir[i]);
                        let (near, far) = if a < b { (a, b) } else { (b, a) };
                        if near > enter {
                            enter = near;
                            axis = Some(i);
                        }
                        exit = exit.min(far);
                    }
                    let Some(i) = axis else { return t };
                    if enter >= exit || enter >= 1.0 {
                        return t;
                    }
                    t.fraction = (enter - EPS / dir.length()).max(0.0);
                    let mut n = Vec3::ZERO;
                    n[i] = -dir[i].signum();
                    t.normal = n;
                }
            }
        }
        t.walkable = t.normal.z >= 0.7;
        t.entity_num = ENTITYNUM_WORLD as u32;
        t.contents = 1;
        t
    }
}

impl PmWorld for World {
    fn trace(&self, start: Vec3, end: Vec3, mins: Vec3, maxs: Vec3, _pass: u32, _mask: u32) -> Tr {
        let mut best = Tr::default();
        for &s in &self.0 {
            let t = World::sweep(s, start, end, mins, maxs);
            if t.allsolid || (t.startsolid && !best.startsolid) || t.fraction < best.fraction {
                best = t;
            }
        }
        best
    }
}

/// `actual` against its pinned value; an empty pin prints the value to
/// fill it with.
fn pin(actual: &str, expected: &str) {
    if expected.is_empty() {
        println!("PIN<<{actual}>>PIN");
    } else {
        assert_eq!(actual, expected);
    }
}

fn flat() -> World {
    World(vec![Solid::Below(Vec3::Z, 0.0)])
}

/// A floor rising `deg` toward +x through the origin.
fn slope(deg: f32) -> World {
    let r = deg.to_radians();
    World(vec![Solid::Below(Vec3::new(-r.sin(), 0.0, r.cos()), 0.0)])
}

/// The flat floor with a box `x0..x1` across y, up to `top`.
fn flat_with(x0: f32, x1: f32, top: f32) -> World {
    World(vec![
        Solid::Below(Vec3::Z, 0.0),
        Solid::Block(Vec3::new(x0, -500.0, -10.0), Vec3::new(x1, 500.0, top)),
    ])
}

fn yaw_short(yaw: f32) -> u16 {
    (yaw * 65536.0 / 360.0).round() as i32 as u16
}

/// A body standing on `world` at `z` facing `yaw` (through `delta_angles`).
fn standing(z: f32, yaw: f32) -> PlayerState {
    let mut ps = PlayerState::spawn(Vec3::new(0.0, 0.0, z), 0.0, 1000);
    ps.ground_entity_num = ENTITYNUM_WORLD;
    ps.delta_angles[1] = yaw;
    ps
}

/// `n` 50 ms cmds holding `buttons`, the view at `yaw` shorts over the
/// delta, `forward` and `right` keys.
fn run(world: &World, ps: &mut PlayerState, n: usize, buttons: u32, yaw: f32, keys: (i8, i8)) {
    let mut old = Cmd {
        server_time: ps.command_time,
        ..Default::default()
    };
    for _ in 0..n {
        let mut c = Cmd {
            server_time: ps.command_time + 50,
            buttons,
            forward_move: keys.0,
            right_move: keys.1,
            ..Default::default()
        };
        c.angles[1] = yaw_short(yaw);
        player_step(ps, &c, &mut old, world, Rules::default(), WeaponMove::NONE);
    }
}

/// What the stance code leaves on a playerstate, every float exact.
fn snap(ps: &PlayerState) -> String {
    format!(
        "pmf {:#x} ef {:#x} eye {:?}/{} o {:?} view {:?} delta {:?} dir {:?} dirpitch {:?} \
         torsopitch {:?} pitches {:?} {:?} lean {:?} ev {} {:?} {:?}",
        ps.pm_flags & (pmf::PRONE | pmf::DUCKED | pmf::PRONE_REFUSED),
        ps.e_flags & (ef::PRONE | ef::CROUCH),
        ps.view_height_current,
        ps.view_height_target,
        ps.origin,
        ps.view_angles,
        ps.delta_angles,
        ps.prone_direction,
        ps.prone_direction_pitch,
        ps.prone_torso_pitch,
        ps.torso_pitch,
        ps.waist_pitch,
        ps.leanf,
        ps.event_sequence,
        ps.events,
        ps.event_parms,
    )
}

#[test]
fn prone_down_and_up_on_the_flat() {
    let w = flat();
    let mut ps = standing(0.125, 30.0);
    run(&w, &mut ps, 12, button::PRONE, 0.0, (0, 0));
    pin(
        &snap(&ps),
        "pmf 0x1 ef 0x8 eye 17.75/11 o Vec3(0.0, 0.0, 0.125) view [0.0, 30.000002, 0.0] delta [0.0, 30.0, 0.0] dir 30.000002 dirpitch -0.0 torsopitch -0.0 pitches 3.0356396e-6 8.717179e-6 lean 0.0 ev 0 [0, 0, 0, 0] [0, 0, 0, 0]",
    );
    run(&w, &mut ps, 16, 0, 0.0, (0, 0));
    pin(
        &snap(&ps),
        "pmf 0x0 ef 0x0 eye 60.0/60 o Vec3(0.0, 0.0, 0.125) view [0.0, 30.000002, 0.0] delta [0.0, 30.0, 0.0] dir 30.000002 dirpitch -0.0 torsopitch -0.0 pitches 3.0356396e-6 8.717179e-6 lean 0.0 ev 0 [0, 0, 0, 0] [0, 0, 0, 0]",
    );
}

#[test]
fn prone_on_a_slope() {
    let w = slope(15.0);
    let mut ps = standing(4.3, 0.0);
    run(&w, &mut ps, 12, button::PRONE, 0.0, (0, 0));
    pin(
        &snap(&ps),
        "pmf 0x1 ef 0x8 eye 17.75/11 o Vec3(0.0, 0.0, 4.3) view [0.0, 0.0, 0.0] delta [0.0, 0.0, 0.0] dir 0.0 dirpitch 345.0 torsopitch 345.0 pitches -22.309929 -14.999991 lean 0.0 ev 0 [0, 0, 0, 0] [0, 0, 0, 0]",
    );
    let mut ps = standing(4.3, 180.0);
    run(&w, &mut ps, 12, button::PRONE, 0.0, (0, 0));
    pin(
        &snap(&ps),
        "pmf 0x1 ef 0x8 eye 17.75/11 o Vec3(0.0, 0.0, 4.3) view [0.0, -180.0, 0.0] delta [0.0, 180.0, 0.0] dir -180.0 dirpitch 15.000004 torsopitch 15.000004 pitches 7.1569567 14.999999 lean 0.0 ev 0 [0, 0, 0, 0] [0, 0, 0, 0]",
    );
}

#[test]
fn prone_refused_on_a_steep_slope() {
    let w = slope(50.0);
    let mut ps = standing(0.25, 90.0);
    run(&w, &mut ps, 4, button::PRONE, 0.0, (0, 0));
    pin(
        &snap(&ps),
        "pmf 0x0 ef 0x0 eye 60.0/60 o Vec3(0.0, 0.0, 0.25) view [0.0, 90.00001, 0.0] delta [0.0, 90.0, 0.0] dir 0.0 dirpitch 0.0 torsopitch 0.0 pitches 0.0 0.0 lean 0.0 ev 1 [6, 0, 0, 0] [0, 0, 0, 0]",
    );
}

/// A wall behind: the body faces +x, its feet lie toward -x.
#[test]
fn prone_against_a_wall_behind() {
    let mut out = Vec::new();
    for x in [-20.0, -30.0, -45.0, -60.0] {
        let w = flat_with(x - 50.0, x, 200.0);
        let mut ps = standing(0.125, 0.0);
        run(&w, &mut ps, 12, button::PRONE, 0.0, (0, 0));
        out.push(format!("wall at {x}: {}", snap(&ps)));
    }
    pin(
        &out.join("\n"),
        "wall at -20: pmf 0x1000 ef 0x0 eye 60.0/60 o Vec3(0.0, 0.0, 0.125) view [0.0, 0.0, 0.0] delta [0.0, 0.0, 0.0] dir 0.0 dirpitch 0.0 torsopitch 0.0 pitches 0.0 0.0 lean 0.0 ev 12 [6, 6, 6, 6] [0, 0, 0, 0]\nwall at -30: pmf 0x1000 ef 0x0 eye 60.0/60 o Vec3(0.0, 0.0, 0.125) view [0.0, 0.0, 0.0] delta [0.0, 0.0, 0.0] dir 0.0 dirpitch 0.0 torsopitch 0.0 pitches 0.0 0.0 lean 0.0 ev 12 [6, 6, 6, 6] [0, 0, 0, 0]\nwall at -45: pmf 0x1000 ef 0x0 eye 60.0/60 o Vec3(0.0, 0.0, 0.125) view [0.0, 0.0, 0.0] delta [0.0, 0.0, 0.0] dir 0.0 dirpitch 0.0 torsopitch 0.0 pitches 0.0 0.0 lean 0.0 ev 12 [6, 6, 6, 6] [0, 0, 0, 0]\nwall at -60: pmf 0x1 ef 0x8 eye 17.75/11 o Vec3(0.0, 0.0, 0.125) view [0.0, 0.0, 0.0] delta [0.0, 0.0, 0.0] dir 0.0 dirpitch -0.0 torsopitch -0.0 pitches 3.0356396e-6 8.717179e-6 lean 0.0 ev 0 [0, 0, 0, 0] [0, 0, 0, 0]",
    );
}

/// A sill behind at a few heights: the sweep retried 22 higher.
#[test]
fn prone_over_a_sill_behind() {
    let mut out = Vec::new();
    for top in [8.0, 16.0, 24.0, 30.0, 40.0] {
        for x in [-25.0, -35.0] {
            let w = flat_with(x - 80.0, x, top);
            let mut ps = standing(0.125, 0.0);
            run(&w, &mut ps, 12, button::PRONE, 0.0, (0, 0));
            out.push(format!("sill {top} at {x}: {}", snap(&ps)));
        }
    }
    pin(
        &out.join("\n"),
        "sill 8 at -25: pmf 0x1000 ef 0x0 eye 60.0/60 o Vec3(0.0, 0.0, 0.125) view [0.0, 0.0, 0.0] delta [0.0, 0.0, 0.0] dir 0.0 dirpitch 0.0 torsopitch 0.0 pitches 3.0356396e-6 17.10273 lean 0.0 ev 12 [6, 6, 6, 6] [0, 0, 0, 0]\nsill 8 at -35: pmf 0x1 ef 0x8 eye 17.75/11 o Vec3(0.0, 0.0, 0.125) view [0.0, 0.0, 0.0] delta [0.0, 0.0, 0.0] dir 0.0 dirpitch -0.0 torsopitch -0.0 pitches 3.0356396e-6 17.10273 lean 0.0 ev 0 [0, 0, 0, 0] [0, 0, 0, 0]\nsill 16 at -25: pmf 0x1000 ef 0x0 eye 60.0/60 o Vec3(0.0, 0.0, 0.125) view [0.0, 0.0, 0.0] delta [0.0, 0.0, 0.0] dir 0.0 dirpitch 0.0 torsopitch 0.0 pitches 3.0356396e-6 31.6075 lean 0.0 ev 12 [6, 6, 6, 6] [0, 0, 0, 0]\nsill 16 at -35: pmf 0x1000 ef 0x0 eye 60.0/60 o Vec3(0.0, 0.0, 0.125) view [0.0, 0.0, 0.0] delta [0.0, 0.0, 0.0] dir 0.0 dirpitch 0.0 torsopitch 0.0 pitches 3.0356396e-6 31.6075 lean 0.0 ev 12 [6, 6, 6, 6] [0, 0, 0, 0]\nsill 24 at -25: pmf 0x1000 ef 0x0 eye 60.0/60 o Vec3(0.0, 0.0, 0.125) view [0.0, 0.0, 0.0] delta [0.0, 0.0, 0.0] dir 0.0 dirpitch 0.0 torsopitch 0.0 pitches -0.0 40.702297 lean 0.0 ev 12 [6, 6, 6, 6] [0, 0, 0, 0]\nsill 24 at -35: pmf 0x1000 ef 0x0 eye 60.0/60 o Vec3(0.0, 0.0, 0.125) view [0.0, 0.0, 0.0] delta [0.0, 0.0, 0.0] dir 0.0 dirpitch 0.0 torsopitch 0.0 pitches 3.0356396e-6 51.08866 lean 0.0 ev 12 [6, 6, 6, 6] [0, 0, 0, 0]\nsill 30 at -25: pmf 0x1000 ef 0x0 eye 60.0/60 o Vec3(0.0, 0.0, 0.125) view [0.0, 0.0, 0.0] delta [0.0, 0.0, 0.0] dir 0.0 dirpitch 0.0 torsopitch 0.0 pitches 0.0 0.0 lean 0.0 ev 12 [6, 6, 6, 6] [0, 0, 0, 0]\nsill 30 at -35: pmf 0x1000 ef 0x0 eye 60.0/60 o Vec3(0.0, 0.0, 0.125) view [0.0, 0.0, 0.0] delta [0.0, 0.0, 0.0] dir 0.0 dirpitch 0.0 torsopitch 0.0 pitches 3.0356396e-6 57.14644 lean 0.0 ev 12 [6, 6, 6, 6] [0, 0, 0, 0]\nsill 40 at -25: pmf 0x1000 ef 0x0 eye 60.0/60 o Vec3(0.0, 0.0, 0.125) view [0.0, 0.0, 0.0] delta [0.0, 0.0, 0.0] dir 0.0 dirpitch 0.0 torsopitch 0.0 pitches 0.0 0.0 lean 0.0 ev 12 [6, 6, 6, 6] [0, 0, 0, 0]\nsill 40 at -35: pmf 0x1000 ef 0x0 eye 60.0/60 o Vec3(0.0, 0.0, 0.125) view [0.0, 0.0, 0.0] delta [0.0, 0.0, 0.0] dir 0.0 dirpitch 0.0 torsopitch 0.0 pitches 3.0356396e-6 64.15747 lean 0.0 ev 12 [6, 6, 6, 6] [0, 0, 0, 0]",
    );
}

/// A fit's name, world, origin, yaw, already prone, on the ground,
/// walkable and feet distance.
type Case = (&'static str, World, Vec3, f32, bool, bool, bool, f32);

/// The fit itself, each argument arm, its pitches.
#[test]
fn check_prone_arms() {
    let cases: [Case; 8] = [
        (
            "flat",
            flat(),
            Vec3::new(0.0, 0.0, 0.125),
            0.0,
            false,
            true,
            true,
            50.0,
        ),
        (
            "flat prone 45",
            flat(),
            Vec3::new(0.0, 0.0, 0.125),
            90.0,
            true,
            true,
            true,
            45.0,
        ),
        (
            "slope",
            slope(20.0),
            Vec3::new(0.0, 0.0, 0.25),
            0.0,
            true,
            true,
            true,
            50.0,
        ),
        (
            "slope back",
            slope(20.0),
            Vec3::new(0.0, 0.0, 0.25),
            180.0,
            true,
            true,
            true,
            50.0,
        ),
        (
            "air",
            flat(),
            Vec3::new(0.0, 0.0, 200.0),
            0.0,
            true,
            false,
            true,
            50.0,
        ),
        (
            "unwalkable",
            flat(),
            Vec3::new(0.0, 0.0, 0.125),
            0.0,
            false,
            true,
            false,
            50.0,
        ),
        (
            "wall",
            flat_with(-80.0, -30.0, 200.0),
            Vec3::new(0.0, 0.0, 0.125),
            0.0,
            true,
            true,
            true,
            50.0,
        ),
        (
            "wall air",
            flat_with(-80.0, -30.0, 200.0),
            Vec3::new(0.0, 0.0, 20.0),
            0.0,
            true,
            false,
            true,
            50.0,
        ),
    ];
    let mut out = Vec::new();
    for (name, w, origin, yaw, already, on_ground, walkable, feet) in cases {
        let mut ps = standing(origin.z, yaw);
        ps.origin = origin;
        ps.torso_pitch = 99.0;
        ps.waist_pitch = 99.0;
        let mut pm = Pmove::new(
            &mut ps,
            Cmd::default(),
            Cmd::default(),
            &w,
            Rules::default(),
        );
        let ok = check_prone(
            &mut pm,
            ProneCheck {
                origin,
                size: 15.0,
                height: 30.0,
                yaw,
                already_prone: already,
                on_ground,
                walkable,
                feet_dist: feet,
                outputs: true,
            },
        );
        out.push(format!(
            "{name}: {ok} {:?} {:?}",
            ps.torso_pitch, ps.waist_pitch
        ));
    }
    pin(
        &out.join("\n"),
        "flat: true 3.0356396e-6 8.717179e-6\nflat prone 45: true 3.0356396e-6 8.717179e-6\nslope: true -13.990165 -19.999992\nslope back: true 25.584862 20.0\nair: true 0.0 0.0\nunwalkable: false 99.0 99.0\nwall: false 99.0 99.0\nwall air: false 99.0 99.0",
    );
}

/// Prone on the flat, the view swung past the cap and the body after it.
#[test]
fn turning_while_prone() {
    let w = flat();
    let mut ps = standing(0.125, 0.0);
    run(&w, &mut ps, 12, button::PRONE, 0.0, (0, 0));
    let mut out = Vec::new();
    for yaw in [30.0, 60.0, 100.0, 140.0, -120.0] {
        run(&w, &mut ps, 3, button::PRONE, yaw, (0, 0));
        out.push(snap(&ps));
    }
    // Crawling forward swings the body onto the view.
    run(&w, &mut ps, 6, button::PRONE, -120.0, (127, 0));
    out.push(snap(&ps));
    pin(
        &out.join("\n"),
        "pmf 0x1 ef 0x8 eye 11.0/11 o Vec3(0.0, 0.0, 0.125) view [0.0, 29.99817, 0.0] delta [0.0, 0.0, 0.0] dir 0.0 dirpitch -0.0 torsopitch -0.0 pitches 3.0356396e-6 8.717179e-6 lean 0.0 ev 0 [0, 0, 0, 0] [0, 0, 0, 0]\npmf 0x1 ef 0x8 eye 11.0/11 o Vec3(0.0, 0.0, 0.125) view [0.0, 60.001835, 0.0] delta [0.0, 0.0, 0.0] dir 0.0 dirpitch -0.0 torsopitch -0.0 pitches 3.0356396e-6 8.717179e-6 lean 0.0 ev 0 [0, 0, 0, 0] [0, 0, 0, 0]\npmf 0x1 ef 0x8 eye 11.0/11 o Vec3(0.0, 0.0, 0.125) view [0.0, 87.75, 0.0] delta [0.0, -12.247566, 0.0] dir 8.250001 dirpitch -0.0 torsopitch -0.0 pitches 3.0356396e-6 8.717179e-6 lean 0.0 ev 0 [0, 0, 0, 0] [0, 0, 0, 0]\npmf 0x1 ef 0x8 eye 11.0/11 o Vec3(0.0, 0.0, 0.125) view [0.0, 95.99999, 0.0] delta [0.0, -43.998795, 0.0] dir 16.500002 dirpitch -0.0 torsopitch -0.0 pitches 3.0356396e-6 8.717179e-6 lean 0.0 ev 0 [0, 0, 0, 0] [0, 0, 0, 0]\npmf 0x1 ef 0x8 eye 11.0/11 o Vec3(0.0, 0.0, 0.125) view [0.0, 104.25002, 0.0] delta [0.0, -135.75182, 0.0] dir 24.750002 dirpitch -0.0 torsopitch -0.0 pitches 3.0356396e-6 8.717179e-6 lean 0.0 ev 0 [0, 0, 0, 0] [0, 0, 0, 0]\npmf 0x1 ef 0x8 eye 11.0/11 o Vec3(-2.1046143, 8.286924, 0.125) view [0.0, 104.25002, 0.0] delta [0.0, -135.75182, 0.0] dir 41.250004 dirpitch -0.0 torsopitch -0.0 pitches 3.0356398e-6 8.717179e-6 lean 0.0 ev 0 [0, 0, 0, 0] [0, 0, 0, 0]",
    );
}

/// Prone beside a wall on the right (-y): a swing toward it is refused.
#[test]
fn turning_while_prone_into_a_wall() {
    let w = World(vec![
        Solid::Below(Vec3::Z, 0.0),
        Solid::Block(
            Vec3::new(-500.0, -60.0, -10.0),
            Vec3::new(500.0, -25.0, 200.0),
        ),
    ]);
    let mut ps = standing(0.125, 0.0);
    run(&w, &mut ps, 12, button::PRONE, 0.0, (0, 0));
    let mut out = vec![snap(&ps)];
    for yaw in [20.0, 45.0, 80.0, 80.0, 80.0] {
        run(&w, &mut ps, 2, button::PRONE, yaw, (0, 0));
        out.push(snap(&ps));
    }
    pin(
        &out.join("\n"),
        "pmf 0x1 ef 0x8 eye 17.75/11 o Vec3(0.0, 0.0, 0.125) view [0.0, 0.0, 0.0] delta [0.0, 0.0, 0.0] dir 0.0 dirpitch -0.0 torsopitch -0.0 pitches 3.0356396e-6 8.717179e-6 lean 0.0 ev 0 [0, 0, 0, 0] [0, 0, 0, 0]\npmf 0x1 ef 0x8 eye 11.0/11 o Vec3(0.0, 0.0, 0.125) view [0.0, 20.000612, 0.0] delta [0.0, 0.0, 0.0] dir 0.0 dirpitch -0.0 torsopitch -0.0 pitches 3.0356396e-6 8.717179e-6 lean 0.0 ev 0 [0, 0, 0, 0] [0, 0, 0, 0]\npmf 0x1 ef 0x8 eye 11.0/11 o Vec3(0.0, 0.0, 0.125) view [0.0, 33.0, 0.0] delta [0.0, -12.0, 0.0] dir 0.0 dirpitch -0.0 torsopitch -0.0 pitches 3.0356396e-6 8.717179e-6 lean 0.0 ev 0 [0, 0, 0, 0] [0, 0, 0, 0]\npmf 0x1 ef 0x8 eye 11.0/11 o Vec3(0.0, 0.0, 0.125) view [0.0, 33.00244, 0.0] delta [0.0, -47.0, 0.0] dir 0.0 dirpitch -0.0 torsopitch -0.0 pitches 3.0356396e-6 8.717179e-6 lean 0.0 ev 0 [0, 0, 0, 0] [0, 0, 0, 0]\npmf 0x1 ef 0x8 eye 11.0/11 o Vec3(0.0, 0.0, 0.125) view [0.0, 33.00244, 0.0] delta [0.0, -47.0, 0.0] dir 0.0 dirpitch -0.0 torsopitch -0.0 pitches 3.0356396e-6 8.717179e-6 lean 0.0 ev 0 [0, 0, 0, 0] [0, 0, 0, 0]\npmf 0x1 ef 0x8 eye 11.0/11 o Vec3(0.0, 0.0, 0.125) view [0.0, 33.00244, 0.0] delta [0.0, -47.0, 0.0] dir 0.0 dirpitch -0.0 torsopitch -0.0 pitches 3.0356396e-6 8.717179e-6 lean 0.0 ev 0 [0, 0, 0, 0] [0, 0, 0, 0]",
    );
}

/// Lean out and back in the open, then into a wall on the right.
#[test]
fn lean_in_the_open_and_against_a_wall() {
    let w = flat();
    let mut ps = standing(0.125, 0.0);
    let mut out = Vec::new();
    for (n, b) in [
        (3, button::LEAN_LEFT),
        (6, button::LEAN_LEFT),
        (2, 0),
        (6, 0),
        (4, button::LEAN_RIGHT),
    ] {
        run(&w, &mut ps, n, b, 0.0, (0, 0));
        out.push(format!("{:?}", ps.leanf));
    }
    let w = World(vec![
        Solid::Below(Vec3::Z, 0.0),
        Solid::Block(
            Vec3::new(-500.0, -40.0, -10.0),
            Vec3::new(500.0, -20.0, 200.0),
        ),
    ]);
    let mut ps = standing(0.125, 0.0);
    for (n, b) in [(3, button::LEAN_RIGHT), (6, button::LEAN_RIGHT), (8, 0)] {
        run(&w, &mut ps, n, b, 0.0, (0, 0));
        out.push(format!("{:?}", ps.leanf));
    }
    // Prone leans a quarter.
    let w = flat();
    let mut ps = standing(0.125, 0.0);
    run(&w, &mut ps, 12, button::PRONE, 0.0, (0, 0));
    run(
        &w,
        &mut ps,
        8,
        button::PRONE | button::LEAN_RIGHT,
        0.0,
        (0, 0),
    );
    out.push(format!("{:?}", ps.leanf));
    pin(
        &out.join(" "),
        "-0.21428572 -0.5 -0.3214286 0.0 0.2857143 0.21428572 0.3818816 0.0 0.25",
    );
}

/// The gunner's arcs: inside, past each edge, wrapped, and an open arc.
#[test]
fn gunner_arcs_at_each_edge() {
    let mut out = Vec::new();
    for (base, range, view) in [
        ([10.0, 90.0], [30.0, 45.0], [5.0, 100.0]),
        ([10.0, 90.0], [30.0, 45.0], [50.0, 100.0]),
        ([10.0, 90.0], [30.0, 45.0], [-30.0, 100.0]),
        ([10.0, 90.0], [30.0, 45.0], [5.0, 150.0]),
        ([10.0, 90.0], [30.0, 45.0], [5.0, 30.0]),
        ([0.0, 170.0], [20.0, 30.0], [0.0, -150.0]),
        ([0.0, -170.0], [20.0, 30.0], [0.0, 150.0]),
        ([0.0, 0.0], [180.0, 180.0], [80.0, 170.0]),
    ] {
        for old_yaw in [90.0, 130.0, view[1]] {
            let mut ps = PlayerState {
                view_angle_clamp_base: base,
                view_angle_clamp_range: range,
                view_angles: [view[0], view[1], 0.0],
                delta_angles: [1.0, 2.0, 0.0],
                ..Default::default()
            };
            gunner_arcs(&mut ps, old_yaw);
            out.push(format!("{:?} {:?}", ps.view_angles, ps.delta_angles));
        }
    }
    pin(
        &out.join("\n"),
        "[5.0, 100.0, 0.0] [1.0, 2.0, 0.0]\n[5.0, 100.0, 0.0] [1.0, 2.0, 0.0]\n[5.0, 100.0, 0.0] [1.0, 2.0, 0.0]\n[40.0, 100.0, 0.0] [-9.0, 2.0, 0.0]\n[40.0, 100.0, 0.0] [-9.0, 2.0, 0.0]\n[40.0, 100.0, 0.0] [-9.0, 2.0, 0.0]\n[340.0, 100.0, 0.0] [11.0, 2.0, 0.0]\n[340.0, 100.0, 0.0] [11.0, 2.0, 0.0]\n[340.0, 100.0, 0.0] [11.0, 2.0, 0.0]\n[5.0, 135.0, 0.0] [1.0, -13.0, 0.0]\n[5.0, 135.0, 0.0] [1.0, -13.0, 0.0]\n[5.0, 135.0, 0.0] [1.0, -13.0, 0.0]\n[5.0, 45.0, 0.0] [1.0, 17.0, 0.0]\n[5.0, 45.0, 0.0] [1.0, 17.000008, 0.0]\n[5.0, 45.0, 0.0] [1.0, 17.0, 0.0]\n[0.0, 200.00002, 0.0] [1.0, -7.9999924, 0.0]\n[0.0, 200.00002, 0.0] [1.0, -7.9999924, 0.0]\n[0.0, 200.00002, 0.0] [1.0, -7.999996, 0.0]\n[0.0, 159.99998, 0.0] [1.0, 11.999985, 0.0]\n[0.0, 159.99998, 0.0] [1.0, 11.999985, 0.0]\n[0.0, 159.99998, 0.0] [1.0, 11.999996, 0.0]\n[80.0, 170.0, 0.0] [1.0, 2.0, 0.0]\n[80.0, 170.0, 0.0] [1.0, 2.0, 0.0]\n[80.0, 170.0, 0.0] [1.0, 2.0, 0.0]",
    );
}
