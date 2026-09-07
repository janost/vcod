//! Our mover against retail's, cmd for cmd, on sloped ground. The fixture is
//! `--net-probe --save-slope --probe-cmd-ms N` against `tools/run_server.sh`:
//! every usercmd the probe sent and every snapshot's movement fields, in
//! order. The replay runs the same cmds through `pmove` on our collision
//! world, starting from the first snapshot's origin, and diffs the origin at
//! every snapshot's `commandTime`.
//!
//! Two replays, because they answer different questions. The free run starts
//! once and accumulates, so it reads how far the two movers drift apart over a
//! route. The rebased run resets to retail's playerstate at every snapshot
//! and replays only the cmds up to the next one, which is what a predicting
//! client does with our snapshots: its error per snapshot is the correction
//! the client's view would show.
//!
//! Set `SLOPE_REPORT=1` to print every snapshot's delta and the worst spots
//! with the surface normal under them. Needs `COD_DIR`; without the paks it
//! returns early.

mod common;

use glam::Vec3;
use std::collections::BTreeMap;
use vcod_common::collision::CollisionWorld;
use vcod_common::net::msg::{
    UserCmd, BUTTON_ADS, BUTTON_ATTACK, BUTTON_MELEE, BUTTON_USE, NULL_USERCMD, WBUTTON_CROUCH,
    WBUTTON_LEAN_LEFT, WBUTTON_LEAN_RIGHT, WBUTTON_PRONE, WBUTTON_RELOAD,
};
use vcod_common::net::protocol::ENTITYNUM_WORLD;
use vcod_common::pmove::{pmove, PlayerState, PmInput};
use vcod_common::weapon::WeaponDef;

/// What the retail server said the player was after some cmd.
#[derive(Clone, Debug)]
struct Snap {
    /// `ps.commandTime`: the serverTime of the last cmd this state is after.
    ct: i32,
    origin: Vec3,
    velocity: Vec3,
    on_ground: bool,
    /// Wire order: pitch, yaw, roll, in degrees.
    view: [f32; 3],
    delta_angles: [i32; 3],
    frac: f32,
    /// `EV_STEP_VIEW` parms the snapshot brought, minus the bias.
    steps: Vec<i32>,
}

enum Line {
    Cmd(UserCmd),
    Snap(Snap),
}

fn parse_vec3(s: &str) -> Vec3 {
    let v: Vec<f32> = s.split(',').map(|x| x.parse().expect("a float")).collect();
    Vec3::new(v[0], v[1], v[2])
}

fn parse_fixture(text: &str) -> Vec<Line> {
    let mut out = Vec::new();
    for line in text.lines() {
        let Some((kind, rest)) = line.split_once(' ') else {
            continue;
        };
        let kv: BTreeMap<&str, &str> = rest
            .split_whitespace()
            .filter_map(|kv| kv.split_once('='))
            .collect();
        let int = |k: &str| -> i32 { kv[k].parse().unwrap_or_else(|_| panic!("{k} in {line}")) };
        match kind {
            "!cmd" => {
                let a: Vec<i32> = kv["angles"]
                    .split(',')
                    .map(|x| x.parse().unwrap())
                    .collect();
                out.push(Line::Cmd(UserCmd {
                    server_time: int("st"),
                    buttons: int("buttons") as u8,
                    wbuttons: int("wbuttons") as u8,
                    weapon: int("weapon") as u8,
                    angles: [a[0], a[1], a[2]],
                    forward: int("forward") as i8,
                    right: int("right") as i8,
                    up: int("up") as i8,
                    ..NULL_USERCMD
                }));
            }
            "!snap" => {
                let view: Vec<f32> = kv["view"].split(',').map(|x| x.parse().unwrap()).collect();
                let da: Vec<i32> = kv["da"].split(',').map(|x| x.parse().unwrap()).collect();
                let steps = kv["step"]
                    .split(',')
                    .filter(|s| !s.is_empty())
                    .map(|s| s.parse().unwrap())
                    .collect();
                out.push(Line::Snap(Snap {
                    ct: int("ct"),
                    origin: parse_vec3(kv["origin"]),
                    velocity: parse_vec3(kv["vel"]),
                    on_ground: int("ground") == ENTITYNUM_WORLD as i32,
                    view: [view[0], view[1], view[2]],
                    delta_angles: [da[0], da[1], da[2]],
                    frac: kv["frac"].parse().unwrap(),
                    steps,
                }));
            }
            _ => {}
        }
    }
    out
}

/// `spectate::pm_input`, which is private to the server crate; the same
/// mapping so the replay reads the cmd the way the server does.
fn pm_input(cmd: &UserCmd) -> PmInput {
    PmInput {
        forward: f32::from(cmd.forward) / 127.0,
        right: f32::from(cmd.right) / 127.0,
        jump: cmd.up > 0,
        crouch: cmd.wbuttons & WBUTTON_CROUCH != 0,
        prone: cmd.wbuttons & WBUTTON_PRONE != 0,
        walk_slow: false,
        lean_left: cmd.wbuttons & WBUTTON_LEAN_LEFT != 0,
        lean_right: cmd.wbuttons & WBUTTON_LEAN_RIGHT != 0,
        attack: cmd.buttons & BUTTON_ATTACK != 0,
        melee: cmd.buttons & BUTTON_MELEE != 0,
        reload: cmd.wbuttons & WBUTTON_RELOAD != 0,
        ads: cmd.buttons & BUTTON_ADS != 0,
        use_button: cmd.buttons & BUTTON_USE != 0,
        weapon: cmd.weapon,
        angles: [cmd.angles[0], cmd.angles[1]],
    }
}

const ANGLE2SHORT: f32 = 65536.0 / 360.0;

fn short_deg(v: i32) -> f32 {
    let deg = v as f32 / ANGLE2SHORT;
    (deg + 180.0).rem_euclid(360.0) - 180.0
}

/// The server's `replay_moves` clocking for one cmd: `PmoveSingle` steps of
/// at most `MAX_FRAME_MS` from the last processed clock up to the cmd's.
fn run_cmd(
    ps: &mut PlayerState,
    cmd: &UserCmd,
    last_st: &mut i32,
    da: [i32; 3],
    world: &CollisionWorld,
    weapons: &[Option<WeaponDef>],
) {
    let dt_ms = cmd.server_time.wrapping_sub(*last_st);
    if dt_ms <= 0 {
        return;
    }
    ps.yaw = short_deg(cmd.angles[1] + da[1]).to_radians();
    ps.pitch = -short_deg(cmd.angles[0] + da[0]).to_radians();
    let mut base = *last_st;
    while base != cmd.server_time {
        let msec = (cmd.server_time - base).min(vcod_common::pmove::MAX_FRAME_MS as i32);
        base += msec;
        pmove(ps, &pm_input(cmd), world, msec as f32 / 1000.0, weapons);
    }
    *last_st = cmd.server_time;
    // `SLOPE_TRACE=<ct>`: print every cmd within 60 ms before that clock.
    if let Some(at) = std::env::var("SLOPE_TRACE")
        .ok()
        .and_then(|v| v.parse::<i32>().ok())
    {
        if (at - 60..=at).contains(&cmd.server_time) {
            println!(
                "  cmd st={} dt={dt_ms} origin=[{:.3},{:.3},{:.3}] vel=[{:.2},{:.2},{:.2}] ground={} n=[{:.3},{:.3},{:.3}] walking={}",
                cmd.server_time,
                ps.origin.x,
                ps.origin.y,
                ps.origin.z,
                ps.velocity.x,
                ps.velocity.y,
                ps.velocity.z,
                ps.on_ground,
                ps.ground_normal.x,
                ps.ground_normal.y,
                ps.ground_normal.z,
                ps.walking,
            );
        }
    }
}

/// A `PlayerState` standing where retail's snapshot says, holding the weapon
/// the cmds carry.
fn state_from(snap: &Snap, weapon: u8) -> PlayerState {
    let mut ps = PlayerState::spawn(snap.origin, snap.view[1]);
    ps.velocity = snap.velocity;
    ps.on_ground = snap.on_ground;
    ps.weapon = weapon;
    ps.weapons_held = 1 << weapon;
    ps.weapon_pos_frac = snap.frac;
    ps.ads_active = snap.frac > 0.0;
    ps
}

#[derive(Clone, Copy, Debug, Default)]
struct Delta {
    dz: f32,
    dxy: f32,
    ground_disagrees: bool,
}

/// One row of the comparison: retail's snapshot, ours at the same clock.
struct Row {
    retail: Snap,
    free: Delta,
    rebased: Delta,
    /// Our origin at the snapshot's clock in the free run.
    ours_free: Vec3,
    normal: Vec3,
}

/// `dz` is signed, ours minus retail; the stats take its magnitude.
fn delta(ours: &PlayerState, retail: &Snap) -> Delta {
    let d = ours.origin - retail.origin;
    Delta {
        dz: d.z,
        dxy: d.truncate().length(),
        ground_disagrees: ours.on_ground != retail.on_ground,
    }
}

/// The surface normal under a point, by the player box's own ground trace.
fn ground_normal(world: &CollisionWorld, at: Vec3) -> Vec3 {
    let mins = Vec3::new(-15.0, -15.0, 0.0);
    let maxs = Vec3::new(15.0, 15.0, 70.0);
    let t = world.box_trace(at, at - Vec3::Z * 18.0, mins, maxs);
    if t.fraction < 1.0 {
        t.normal
    } else {
        Vec3::ZERO
    }
}

fn replay(lines: &[Line], world: &CollisionWorld, weapons: &[Option<WeaponDef>]) -> Vec<Row> {
    let first = lines
        .iter()
        .find_map(|l| match l {
            Line::Snap(s) => Some(s.clone()),
            _ => None,
        })
        .expect("a snapshot before the first cmd");
    let weapon = lines
        .iter()
        .find_map(|l| match l {
            Line::Cmd(c) => Some(c.weapon),
            _ => None,
        })
        .unwrap_or(0);
    let mut free = state_from(&first, weapon);
    let mut free_st = first.ct;
    let mut rebased = free;
    let mut rebased_st = first.ct;
    let mut da = first.delta_angles;
    let mut rows = Vec::new();
    // Cmds whose clock is past the newest snapshot's `commandTime` were sent
    // but not yet applied when that snapshot went out; they belong to the
    // next interval. The interleaving in the file is arrival order at the
    // probe, so the cmds are walked by clock against each snapshot.
    let cmds: Vec<UserCmd> = lines
        .iter()
        .filter_map(|l| match l {
            Line::Cmd(c) => Some(*c),
            _ => None,
        })
        .collect();
    let mut next_cmd = 0usize;
    for l in lines {
        let Line::Snap(s) = l else { continue };
        if s.ct <= first.ct {
            continue;
        }
        while next_cmd < cmds.len() && cmds[next_cmd].server_time <= s.ct {
            let c = &cmds[next_cmd];
            run_cmd(&mut free, c, &mut free_st, da, world, weapons);
            run_cmd(&mut rebased, c, &mut rebased_st, da, world, weapons);
            next_cmd += 1;
        }
        rows.push(Row {
            retail: s.clone(),
            free: delta(&free, s),
            rebased: delta(&rebased, s),
            ours_free: free.origin,
            normal: ground_normal(world, s.origin),
        });
        rebased = state_from(s, weapon);
        rebased_st = s.ct;
        da = s.delta_angles;
    }
    rows
}

struct Stats {
    max_dz: f32,
    max_dxy: f32,
    /// Median over the rows.
    typ_dz: f32,
    typ_dxy: f32,
    /// 95th percentile over the rows.
    p95_dz: f32,
    p95_dxy: f32,
    /// Rows off by more than a unit on either axis.
    outliers: usize,
    ground_disagreements: usize,
}

fn stats(rows: &[Row], pick: impl Fn(&Row) -> Delta) -> Stats {
    let mut dz: Vec<f32> = rows.iter().map(|r| pick(r).dz.abs()).collect();
    let mut dxy: Vec<f32> = rows.iter().map(|r| pick(r).dxy).collect();
    dz.sort_by(f32::total_cmp);
    dxy.sort_by(f32::total_cmp);
    let at = |v: &[f32], q: f32| v.get((q * v.len() as f32) as usize).copied().unwrap_or(0.0);
    Stats {
        max_dz: dz.last().copied().unwrap_or(0.0),
        max_dxy: dxy.last().copied().unwrap_or(0.0),
        typ_dz: at(&dz, 0.5),
        typ_dxy: at(&dxy, 0.5),
        p95_dz: at(&dz, 0.95),
        p95_dxy: at(&dxy, 0.95),
        outliers: rows
            .iter()
            .filter(|r| pick(r).dz.abs() > 1.0 || pick(r).dxy > 1.0)
            .count(),
        ground_disagreements: rows.iter().filter(|r| pick(r).ground_disagrees).count(),
    }
}

fn report(path: &str, rows: &[Row]) {
    let verbose = std::env::var_os("SLOPE_REPORT").is_some();
    if !verbose {
        return;
    }
    println!("== {path}: {} snapshots", rows.len());
    let mut run_max = 0.0f32;
    for (i, r) in rows.iter().enumerate() {
        run_max = run_max.max(r.rebased.dz.abs().max(r.rebased.dxy));
        let o = r.retail.origin;
        println!(
            "{i:>4} ct={} retail=[{:.2},{:.2},{:.2}] ours=[{:.2},{:.2},{:.2}] free dz={:.3} dxy={:.3} \
             | rebased dz={:.3} dxy={:.3} runmax={:.3} ground={}{} n=[{:.2},{:.2},{:.2}] frac={:.2} step={:?}",
            r.retail.ct,
            o.x,
            o.y,
            o.z,
            r.ours_free.x,
            r.ours_free.y,
            r.ours_free.z,
            r.free.dz,
            r.free.dxy,
            r.rebased.dz,
            r.rebased.dxy,
            run_max,
            r.retail.on_ground,
            if r.rebased.ground_disagrees { "!" } else { "" },
            r.normal.x,
            r.normal.y,
            r.normal.z,
            r.retail.frac,
            r.retail.steps,
        );
    }
    let mut worst: Vec<&Row> = rows.iter().collect();
    worst.sort_by(|a, b| b.rebased.dz.abs().total_cmp(&a.rebased.dz.abs()));
    println!("-- worst rebased dz");
    for r in worst.iter().take(8) {
        let o = r.retail.origin;
        println!(
            "  dz={:.3} dxy={:.3} at [{:.1},{:.1},{:.1}] normal=[{:.3},{:.3},{:.3}] ground={} vel=[{:.1},{:.1},{:.1}]",
            r.rebased.dz,
            r.rebased.dxy,
            o.x,
            o.y,
            o.z,
            r.normal.x,
            r.normal.y,
            r.normal.z,
            r.retail.on_ground,
            r.retail.velocity.x,
            r.retail.velocity.y,
            r.retail.velocity.z,
        );
    }
    worst.sort_by(|a, b| b.rebased.dxy.total_cmp(&a.rebased.dxy));
    println!("-- worst rebased dxy");
    for r in worst.iter().take(8) {
        let o = r.retail.origin;
        println!(
            "  dxy={:.3} dz={:.3} at [{:.1},{:.1},{:.1}] normal=[{:.3},{:.3},{:.3}] ground={}",
            r.rebased.dxy,
            r.rebased.dz,
            o.x,
            o.y,
            o.z,
            r.normal.x,
            r.normal.y,
            r.normal.z,
            r.retail.on_ground,
        );
    }
}

/// What the rebased run may read, from the measurement of 2026-09-07
/// (docs/research/cod11-mantle.md, "The player is a capsule"): on the 8 ms
/// capture 95% of snapshots land within 0.006 vertically and 0.135
/// horizontally of retail, on the 25 ms one within 0 and 0.30, and retail's
/// own noise floor is the integer truncation of the velocity it sends, under
/// 0.05 per interval. The share of snapshots off by more than a unit is 1.6%
/// and 2.9%: wall corners and slides, one prop mesh retail's player does
/// not collide, and the `clip_*` brushes the mantle doc lists as open. A
/// box mover read 1.08 on every snapshot of the 3.3-degree street.
const P95_TOLERANCE_Z: f32 = 0.1;
const P95_TOLERANCE_XY: f32 = 0.5;
/// The outlier share: the two captures sit at 1.6% and 2.9%.
const OUTLIER_SHARE: f32 = 0.04;

fn check(map: &str, gametype: &str, cmd_ms: u32) {
    let path = format!(
        "{}/tests/fixtures/playerstate/{map}-{gametype}-slope-{cmd_ms}ms.txt",
        env!("CARGO_MANIFEST_DIR")
    );
    check_path(map, &path);
}

fn check_path(map: &str, path: &str) {
    let Some(fs) = vcod_common::testing::game_fs() else {
        return;
    };
    let text = std::fs::read_to_string(path).unwrap_or_else(|e| panic!("{path}: {e}"));
    let lines = parse_fixture(&text);
    let bsp_path = fs.resolve_map(map).expect("map in the mounted paks");
    let bsp = vcod_common::bsp::parse(&fs.read(&bsp_path).expect("read the bsp")).expect("parse");
    let world = vcod_server::world::World::from_bsp(&bsp, Some(&fs)).collision;
    let weapons = vcod_server::weapons::WeaponTable::load(&fs);
    let rows = replay(&lines, &world, weapons.defs());
    assert!(rows.len() > 100, "{path}: only {} snapshots", rows.len());
    report(path, &rows);
    let free = stats(&rows, |r| r.free);
    let rebased = stats(&rows, |r| r.rebased);
    println!(
        "{path}: free run max dz {:.3} dxy {:.3} median dz {:.3} dxy {:.3}, ground disagreements {}; \
         rebased max dz {:.3} dxy {:.3} median dz {:.3} dxy {:.3} p95 dz {:.3} dxy {:.3}, \
         {} of {} rows past a unit, ground disagreements {}",
        free.max_dz,
        free.max_dxy,
        free.typ_dz,
        free.typ_dxy,
        free.ground_disagreements,
        rebased.max_dz,
        rebased.max_dxy,
        rebased.typ_dz,
        rebased.typ_dxy,
        rebased.p95_dz,
        rebased.p95_dxy,
        rebased.outliers,
        rows.len(),
        rebased.ground_disagreements,
    );
    assert!(
        rebased.p95_dz <= P95_TOLERANCE_Z && rebased.p95_dxy <= P95_TOLERANCE_XY,
        "{path}: rebased p95 dz {:.3} dxy {:.3} past the tolerance",
        rebased.p95_dz,
        rebased.p95_dxy
    );
    assert!(
        (rebased.outliers as f32) <= OUTLIER_SHARE * rows.len() as f32,
        "{path}: {} of {} snapshots land more than a unit from retail",
        rebased.outliers,
        rows.len()
    );
}

/// A capture that is not committed: `SLOPE_FIXTURE=<path> SLOPE_MAP=<map>
/// cargo test ... -- --ignored`, for a run kept in `tmp/`.
#[test]
#[ignore]
fn the_mover_replays_the_fixture_named_by_slope_fixture() {
    let path = std::env::var("SLOPE_FIXTURE").expect("SLOPE_FIXTURE");
    let map = std::env::var("SLOPE_MAP").unwrap_or_else(|_| "mp_carentan".into());
    check_path(&map, &path);
}

#[test]
fn the_mover_lands_where_retail_does_on_mp_carentan_grades_at_8_ms() {
    check("mp_carentan", "dm", 8);
}

#[test]
fn the_mover_lands_where_retail_does_on_mp_carentan_grades_at_25_ms() {
    check("mp_carentan", "dm", 25);
}
