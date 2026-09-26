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
//! with the surface normal under them and what our world puts in the way of
//! the cmd's wish direction there. Needs `COD_DIR`; without the paks it
//! returns early.

mod common;

use glam::Vec3;
use std::collections::BTreeMap;
use vcod_common::collision::CollisionWorld;
use vcod_common::movetrace::MoveWorld;
use vcod_common::net::msg;
use vcod_common::net::msg::{
    UserCmd, BUTTON_ADS, BUTTON_ATTACK, BUTTON_MELEE, BUTTON_USE, NULL_USERCMD, WBUTTON_CROUCH,
    WBUTTON_LEAN_LEFT, WBUTTON_LEAN_RIGHT, WBUTTON_PRONE, WBUTTON_RELOAD,
};
use vcod_common::net::protocol::{ENTITYNUM_NONE, ENTITYNUM_WORLD, PROTOCOL_V1};
use vcod_common::pmove::{pmove, predict, PlayerState, PmInput};
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
    /// The stance and prone fields, which captures from before the prone
    /// crawl do not carry.
    prone: Option<ProneFields>,
}

/// What a `--probe-prone` capture adds to each snapshot line.
#[derive(Clone, Debug)]
struct ProneFields {
    eflags: i32,
    pm_flags: i32,
    view_height: f32,
    direction: f32,
    direction_pitch: f32,
    torso_pitch: f32,
    events: Vec<(i32, i32)>,
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
                    prone: kv.get("eflags").map(|_| {
                        let float = |k: &str| -> f32 { kv[k].parse().unwrap() };
                        ProneFields {
                            eflags: int("eflags"),
                            pm_flags: int("pm_flags"),
                            view_height: float("vh"),
                            direction: float("pdir"),
                            direction_pitch: float("pdirpitch"),
                            torso_pitch: float("ptorso"),
                            events: kv["ev"]
                                .split(',')
                                .filter(|e| !e.is_empty())
                                .map(|e| {
                                    let (a, b) = e.split_once(':').unwrap();
                                    (a.parse().unwrap(), b.parse().unwrap())
                                })
                                .collect(),
                        }
                    }),
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
/// at most `MAX_FRAME_MS` from the last processed clock up to the cmd's, with
/// the prone caps' corrections pushed into `da` the way `ClientSim::step`
/// pushes them into `delta_angles`.
fn run_cmd(
    ps: &mut PlayerState,
    cmd: &UserCmd,
    last_st: &mut i32,
    da: &mut [i32; 3],
    world: &MoveWorld,
    weapons: &[Option<WeaponDef>],
) {
    let dt_ms = cmd.server_time.wrapping_sub(*last_st);
    if dt_ms <= 0 {
        return;
    }
    let mut base = *last_st;
    while base != cmd.server_time {
        let msec = (cmd.server_time - base).min(vcod_common::pmove::MAX_FRAME_MS as i32);
        base += msec;
        ps.yaw = short_deg(cmd.angles[1] + da[1]).to_radians();
        ps.pitch = -short_deg(cmd.angles[0] + da[0]).to_radians();
        pmove(ps, &pm_input(cmd), world, msec as f32 / 1000.0, weapons);
        let corrections = [ps.view_pitch_correction, ps.view_yaw_correction];
        for (i, c) in corrections.into_iter().enumerate() {
            if c != 0.0 {
                da[i] = (da[i] + (c * ANGLE2SHORT) as i32) & 0xffff;
            }
        }
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
/// the cmds carry. A capture that carries the stance fields is rebuilt the
/// way a predicting client rebuilds it, through `predict::from_wire`.
fn state_from(snap: &Snap, weapon: u8) -> PlayerState {
    if let Some(pf) = &snap.prone {
        let p = &PROTOCOL_V1;
        let mut w = msg::PlayerState::null(p);
        let mut set = |name: &str, v: i32| {
            w.fields[msg::PlayerState::field_index(p, name).unwrap()] = v;
        };
        for i in 0..3 {
            set(&format!("origin[{i}]"), snap.origin[i].to_bits() as i32);
            set(&format!("velocity[{i}]"), snap.velocity[i].to_bits() as i32);
            set(&format!("viewangles[{i}]"), snap.view[i].to_bits() as i32);
            set(&format!("delta_angles[{i}]"), snap.delta_angles[i]);
        }
        set("eFlags", pf.eflags);
        set("pm_flags", pf.pm_flags);
        set(
            "groundEntityNum",
            if snap.on_ground {
                ENTITYNUM_WORLD as i32
            } else {
                ENTITYNUM_NONE as i32
            },
        );
        set("viewHeightCurrent", pf.view_height.to_bits() as i32);
        set("proneDirection", pf.direction.to_bits() as i32);
        set("proneDirectionPitch", pf.direction_pitch.to_bits() as i32);
        set("proneTorsoPitch", pf.torso_pitch.to_bits() as i32);
        set("weapon", i32::from(weapon));
        set("weapons[0]", 1 << weapon);
        set("fWeaponPosFrac", snap.frac.to_bits() as i32);
        return predict::from_wire(p, &w, None).ps;
    }
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
    /// Our state at the snapshot's clock in the rebased run.
    ours: PlayerState,
    /// Our `delta_angles` there, raw 16-bit.
    ours_da: [i32; 3],
    normal: Vec3,
    /// The last cmd's wish direction on the ground plane, unit or zero.
    wish: Vec3,
}

/// Where the cmd asked to go, in the world: forward and right against the
/// cmd's yaw, the way `PM_CmdScale`'s caller builds the wish.
fn wish_dir(cmd: &UserCmd, da: [i32; 3]) -> Vec3 {
    let yaw = short_deg(cmd.angles[1] + da[1]).to_radians();
    let forward = Vec3::new(yaw.cos(), yaw.sin(), 0.0);
    let right = Vec3::new(yaw.sin(), -yaw.cos(), 0.0);
    (forward * f32::from(cmd.forward) + right * f32::from(cmd.right)).normalize_or_zero()
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
    let mut free_da = first.delta_angles;
    let mut rebased_da = first.delta_angles;
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
    let mut wish = Vec3::ZERO;
    let mw = MoveWorld::bare(world);
    for l in lines {
        let Line::Snap(s) = l else { continue };
        if s.ct <= first.ct {
            continue;
        }
        while next_cmd < cmds.len() && cmds[next_cmd].server_time <= s.ct {
            let c = &cmds[next_cmd];
            wish = wish_dir(c, rebased_da);
            run_cmd(&mut free, c, &mut free_st, &mut free_da, &mw, weapons);
            run_cmd(
                &mut rebased,
                c,
                &mut rebased_st,
                &mut rebased_da,
                &mw,
                weapons,
            );
            next_cmd += 1;
        }
        rows.push(Row {
            retail: s.clone(),
            free: delta(&free, s),
            rebased: delta(&rebased, s),
            ours_free: free.origin,
            ours: rebased,
            ours_da: rebased_da,
            normal: ground_normal(world, s.origin),
            wish,
        });
        rebased = state_from(s, weapon);
        rebased_st = s.ct;
        // Retail's cmds were rebased on retail's `delta_angles`, so the free
        // run takes them too.
        free_da = s.delta_angles;
        rebased_da = s.delta_angles;
    }
    rows
}

struct Stats {
    max_dz: f32,
    max_dxy: f32,
    /// Median over the rows.
    typ_dz: f32,
    typ_dxy: f32,
    /// 95th and 99th percentiles over the rows.
    p95_dz: f32,
    p95_dxy: f32,
    p99_dz: f32,
    p99_dxy: f32,
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
        p99_dz: at(&dz, 0.99),
        p99_dxy: at(&dxy, 0.99),
        outliers: rows
            .iter()
            .filter(|r| pick(r).dz.abs() > 1.0 || pick(r).dxy > 1.0)
            .count(),
        ground_disagreements: rows.iter().filter(|r| pick(r).ground_disagrees).count(),
    }
}

/// What our world puts in the way at a retail spot: the player capsule
/// swept 8 units along the wish and 18 down, each named by `describe`.
fn obstacle(world: &CollisionWorld, at: Vec3, wish: Vec3) -> String {
    let mins = Vec3::new(-15.0, -15.0, 0.0);
    let maxs = Vec3::new(15.0, 15.0, 70.0);
    let name = |t: &vcod_common::collision::Trace| match t.hit {
        Some(p) if t.fraction < 1.0 => format!(
            "{} f={:.3}{}",
            world.describe(p),
            t.fraction,
            if t.startsolid { " startsolid" } else { "" }
        ),
        _ => "clear".into(),
    };
    let ahead = world.box_trace(at, at + wish * 8.0, mins, maxs);
    let down = world.box_trace(at, at - Vec3::Z * 18.0, mins, maxs);
    format!("ahead: {} | down: {}", name(&ahead), name(&down))
}

/// Degrees between two angles, folded to 0..180.
fn angle_off(a: f32, b: f32) -> f32 {
    (a - b + 180.0).rem_euclid(360.0) - 180.0
}

/// Where ours and retail part on the view a prone player's client predicts,
/// all in degrees: the body's yaw and both prone pitches, the view angles the
/// snapshot carries, and the `delta_angles` the caps push.
#[derive(Clone, Copy, Debug, Default)]
struct ViewDelta {
    direction: f32,
    direction_pitch: f32,
    torso_pitch: f32,
    view: [f32; 2],
    delta_angles: [f32; 2],
}

impl ViewDelta {
    fn of(r: &Row) -> Option<Self> {
        let pf = r.retail.prone.as_ref()?;
        let da = |i: usize| {
            let d = (r.ours_da[i] - r.retail.delta_angles[i]) as i16;
            (f32::from(d) / ANGLE2SHORT).abs()
        };
        Some(Self {
            direction: angle_off(r.ours.prone_direction, pf.direction).abs(),
            direction_pitch: angle_off(r.ours.prone_direction_pitch, pf.direction_pitch).abs(),
            torso_pitch: angle_off(r.ours.prone_torso_pitch, pf.torso_pitch).abs(),
            view: [
                angle_off(-r.ours.pitch.to_degrees(), r.retail.view[0]).abs(),
                angle_off(r.ours.yaw.to_degrees(), r.retail.view[1]).abs(),
            ],
            delta_angles: [da(0), da(1)],
        })
    }

    fn max(self, o: Self) -> Self {
        Self {
            direction: self.direction.max(o.direction),
            direction_pitch: self.direction_pitch.max(o.direction_pitch),
            torso_pitch: self.torso_pitch.max(o.torso_pitch),
            view: [self.view[0].max(o.view[0]), self.view[1].max(o.view[1])],
            delta_angles: [
                self.delta_angles[0].max(o.delta_angles[0]),
                self.delta_angles[1].max(o.delta_angles[1]),
            ],
        }
    }

    fn worst(self) -> f32 {
        [
            self.direction,
            self.direction_pitch,
            self.torso_pitch,
            self.view[0],
            self.view[1],
            self.delta_angles[0],
            self.delta_angles[1],
        ]
        .into_iter()
        .fold(0.0, f32::max)
    }
}

/// The prone half of a row: retail's stance fields against ours.
fn prone_line(r: &Row) -> String {
    let Some(pf) = &r.retail.prone else {
        return String::new();
    };
    format!(
        " | eflags={} pmf={:#x} vh={:.2}/{:.2} pdir={:.2}/{:.2} dpitch={:.2}/{:.2} tpitch={:.2}/{:.2} pitch={:.2}/{:.2} yaw={:.2}/{:.2} da={},{}/{},{} ev={:?}",
        pf.eflags,
        pf.pm_flags,
        pf.view_height,
        r.ours.view_height(),
        pf.direction,
        r.ours.prone_direction,
        pf.direction_pitch,
        r.ours.prone_direction_pitch,
        pf.torso_pitch,
        r.ours.prone_torso_pitch,
        r.retail.view[0],
        -r.ours.pitch.to_degrees(),
        r.retail.view[1],
        r.ours.yaw.to_degrees(),
        r.retail.delta_angles[0],
        r.retail.delta_angles[1],
        r.ours_da[0],
        r.ours_da[1],
        pf.events,
    )
}

fn report(path: &str, rows: &[Row], world: &CollisionWorld) {
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
             | rebased dz={:.3} dxy={:.3} runmax={:.3} ground={}{} n=[{:.2},{:.2},{:.2}] frac={:.2} step={:?}{}",
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
            prone_line(r),
        );
    }
    let mut worst: Vec<&Row> = rows.iter().collect();
    worst.sort_by(|a, b| b.rebased.dz.abs().total_cmp(&a.rebased.dz.abs()));
    println!("-- worst rebased dz");
    for r in worst.iter().take(8) {
        let o = r.retail.origin;
        println!(
            "  ct={} dz={:.3} dxy={:.3} at [{:.1},{:.1},{:.1}] normal=[{:.3},{:.3},{:.3}] ground={} vel=[{:.1},{:.1},{:.1}] wish=[{:.2},{:.2}] {}",
            r.retail.ct,
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
            r.wish.x,
            r.wish.y,
            obstacle(world, o, r.wish),
        );
    }
    worst.sort_by(|a, b| b.rebased.dxy.total_cmp(&a.rebased.dxy));
    println!("-- worst rebased dxy");
    for r in worst.iter().take(8) {
        let o = r.retail.origin;
        println!(
            "  ct={} dxy={:.3} dz={:.3} at [{:.1},{:.1},{:.1}] normal=[{:.3},{:.3},{:.3}] ground={} vel=[{:.1},{:.1},{:.1}] wish=[{:.2},{:.2}] {}",
            r.retail.ct,
            r.rebased.dxy,
            r.rebased.dz,
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
            r.wish.x,
            r.wish.y,
            obstacle(world, o, r.wish),
        );
    }
    let mut worst: Vec<(&Row, ViewDelta)> = rows
        .iter()
        .filter_map(|r| ViewDelta::of(r).map(|d| (r, d)))
        .collect();
    if !worst.is_empty() {
        worst.sort_by(|a, b| b.1.worst().total_cmp(&a.1.worst()));
        println!("-- worst prone view");
        for (r, d) in worst.iter().take(12) {
            println!("  ct={} {d:?}{}", r.retail.ct, prone_line(r));
        }
    }
    println!("-- ground disagreements");
    for r in rows.iter().filter(|r| r.rebased.ground_disagrees).take(8) {
        let o = r.retail.origin;
        println!(
            "  ct={} at [{:.1},{:.1},{:.1}] retail ground={} {}",
            r.retail.ct,
            o.x,
            o.y,
            o.z,
            r.retail.on_ground,
            obstacle(world, o, r.wish),
        );
    }
}

/// What the rebased run may read, from the measurement of 2026-09-23, with
/// the velocity snap and the walk's accel floor (docs/research/cod11-mantle.md,
/// "The tail of the default arm" and "The walk's accel floor"): the 8 ms
/// capture reads |dz| p95 0.000, p99 0.017, max 0.125 and dxy p95 0.000,
/// p99 0.001, max 2.9, the 25 ms one |dz| 0 throughout and dxy p95 0.004,
/// p99 0.430, max 4.5, with 2 and 12 rows past a unit and no ground
/// disagreement. Without the two the dxy p95 read 0.130 and 0.146. A box
/// mover read 1.08 on every snapshot of the 3.3-degree street, and the
/// facet polyhedron on terrain read 0.96 at a kerb-ramp foot.
/// The outlier share: the two captures sit at 0.14% and 0.8%.
const SLOPE: Tolerance = Tolerance {
    p95_z: 0.02,
    p95_xy: 0.05,
    p99_z: 0.05,
    p99_xy: 0.6,
    max_z: 0.5,
    outlier_share: 0.015,
    view: None,
};

/// What the prone crawls may read, from the measurement of 2026-09-27 with
/// the dive, the landing damp, the body swing while moving, the pitch cap and
/// the fit trace at 24 (docs/research/cod11-mantle.md, "Prone"). The street
/// reads |dz| max 0.125 and dxy p99 0.233, max 2.7 on the one row past a
/// unit, a sideways prone press whose speed retail blends across the eye's
/// drop; the mound to [`MOUND_UNTIL`] reads |dz| p99 0.128, max 0.173 and
/// dxy max 0.52. The view: the body's yaw within 0.5 degrees, both prone
/// pitches within 1.42, where our terrain trace names the other of two
/// facets under a crawl, and the view angles and `delta_angles` within one
/// 16-bit step. Before those the dives read 10.7 units off, the body stood
/// still for 99 snapshots of retail's swing and the pitch cap was missing
/// outright, 38.8 degrees on the street and 48.8 on the mound.
const PRONE: Tolerance = Tolerance {
    p95_z: 0.02,
    p95_xy: 0.05,
    p99_z: 0.2,
    p99_xy: 0.6,
    max_z: 0.5,
    outlier_share: 0.015,
    view: Some(ViewTolerance {
        direction: 1.0,
        pitches: 2.0,
        view: 0.1,
    }),
};

/// Where the mound crawl leaves prone movement: from here the strafe and the
/// last two dives wedge the player against a `clip_nosight` brush and a 50
/// degree plane, airborne and sliding, which is the collider's corner and not
/// the prone path.
const MOUND_UNTIL: i32 = 84000;

/// What a replay may read, origin in units and the prone view in degrees.
struct Tolerance {
    p95_z: f32,
    p95_xy: f32,
    p99_z: f32,
    p99_xy: f32,
    max_z: f32,
    outlier_share: f32,
    /// Bounds on `ViewDelta`: the body's yaw, both prone pitches, and the
    /// view angles and `delta_angles` the caps push.
    view: Option<ViewTolerance>,
}

struct ViewTolerance {
    direction: f32,
    pitches: f32,
    view: f32,
}

fn check(map: &str, gametype: &str, cmd_ms: u32) {
    let path = format!(
        "{}/tests/fixtures/playerstate/{map}-{gametype}-slope-{cmd_ms}ms.txt",
        env!("CARGO_MANIFEST_DIR")
    );
    check_path(map, gametype, &path, &SLOPE, None);
}

/// What the stock gametype script does to the map's brush models before a
/// client walks: `_gameobjects::main` `delete()`s every entity carrying a
/// `script_gameobjectname` the gametype did not list (`dm.gsc:78`,
/// `tdm.gsc:78`: their own name; `sd.gsc:123`: `sd`, `bombzone`,
/// `blocker`), and a deleted `script_brushmodel` takes its brushes out of
/// the clip. The server's `delete` builtin does this at run time; the
/// replay has no script, so it applies the rule itself.
fn unlink_gameobjects(world: &CollisionWorld, entities: &str, gametype: &str) {
    let allowed: &[&str] = match gametype {
        "sd" => &["sd", "bombzone", "blocker"],
        g => &[g][..],
    };
    for block in vcod_common::bsp::entity_blocks(entities) {
        let Some(name) = block.get("script_gameobjectname") else {
            continue;
        };
        if allowed.contains(&name.as_str()) {
            continue;
        }
        if let Some(n) = block
            .get("model")
            .and_then(|m| m.strip_prefix('*'))
            .and_then(|n| n.parse::<usize>().ok())
        {
            world.set_model_linked(n, false);
        }
    }
}

/// `until` drops the rows from that `commandTime` on.
fn check_path(map: &str, gametype: &str, path: &str, tol: &Tolerance, until: Option<i32>) {
    let Some(fs) = vcod_common::testing::game_fs() else {
        return;
    };
    let text = std::fs::read_to_string(path).unwrap_or_else(|e| panic!("{path}: {e}"));
    let lines = parse_fixture(&text);
    let bsp_path = fs.resolve_map(map).expect("map in the mounted paks");
    let bsp = vcod_common::bsp::parse(&fs.read(&bsp_path).expect("read the bsp")).expect("parse");
    let world = vcod_server::world::World::from_bsp(&bsp, Some(&fs)).collision;
    unlink_gameobjects(&world, &bsp.entities, gametype);
    let weapons = vcod_server::weapons::WeaponTable::load(&fs);
    let mut rows = replay(&lines, &world, weapons.defs());
    if let Some(ct) = until {
        rows.retain(|r| r.retail.ct < ct);
    }
    assert!(rows.len() > 100, "{path}: only {} snapshots", rows.len());
    report(path, &rows, &world);
    let free = stats(&rows, |r| r.free);
    let rebased = stats(&rows, |r| r.rebased);
    println!(
        "{path}: free run max dz {:.3} dxy {:.3} median dz {:.3} dxy {:.3}, ground disagreements {}; \
         rebased max dz {:.3} dxy {:.3} median dz {:.3} dxy {:.3} p95 dz {:.3} dxy {:.3} \
         p99 dz {:.3} dxy {:.3}, {} of {} rows past a unit, ground disagreements {}",
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
        rebased.p99_dz,
        rebased.p99_dxy,
        rebased.outliers,
        rows.len(),
        rebased.ground_disagreements,
    );
    let view = rows.iter().filter_map(ViewDelta::of).reduce(ViewDelta::max);
    if let Some(v) = view {
        println!("{path}: prone view, rebased, max {v:?}");
    }
    if let (Some(v), Some(t)) = (view, &tol.view) {
        assert!(
            v.direction <= t.direction
                && v.direction_pitch <= t.pitches
                && v.torso_pitch <= t.pitches
                && v.view.iter().chain(&v.delta_angles).all(|&d| d <= t.view),
            "{path}: the prone view parts from retail's: {v:?}"
        );
    }
    assert!(
        rebased.p95_dz <= tol.p95_z && rebased.p95_dxy <= tol.p95_xy,
        "{path}: rebased p95 dz {:.3} dxy {:.3} past the tolerance",
        rebased.p95_dz,
        rebased.p95_dxy
    );
    assert!(
        rebased.p99_dz <= tol.p99_z && rebased.p99_dxy <= tol.p99_xy,
        "{path}: rebased p99 dz {:.3} dxy {:.3} past the tolerance",
        rebased.p99_dz,
        rebased.p99_dxy
    );
    assert!(
        rebased.max_dz <= tol.max_z,
        "{path}: rebased max dz {:.3} past the tolerance",
        rebased.max_dz
    );
    assert!(
        (rebased.outliers as f32) <= tol.outlier_share * rows.len() as f32,
        "{path}: {} of {} snapshots land more than a unit from retail",
        rebased.outliers,
        rows.len()
    );
}

/// A capture that is not committed: `SLOPE_FIXTURE=<path> SLOPE_MAP=<map>
/// SLOPE_GAMETYPE=<gt> cargo test ... -- --ignored`, for a run kept in `tmp/`.
#[test]
#[ignore]
fn the_mover_replays_the_fixture_named_by_slope_fixture() {
    let path = std::env::var("SLOPE_FIXTURE").expect("SLOPE_FIXTURE");
    let map = std::env::var("SLOPE_MAP").unwrap_or_else(|_| "mp_carentan".into());
    let gametype = std::env::var("SLOPE_GAMETYPE").unwrap_or_else(|_| "dm".into());
    let until = std::env::var("SLOPE_UNTIL")
        .ok()
        .and_then(|v| v.parse().ok());
    check_path(&map, &gametype, &path, &SLOPE, until);
}

#[test]
fn the_mover_lands_where_retail_does_on_mp_carentan_grades_at_8_ms() {
    check("mp_carentan", "dm", 8);
}

#[test]
fn the_mover_lands_where_retail_does_on_mp_carentan_grades_at_25_ms() {
    check("mp_carentan", "dm", 25);
}

fn check_prone(spot: &str, until: Option<i32>) {
    let path = format!(
        "{}/tests/fixtures/playerstate/mp_carentan-dm-slope-8ms-prone-{spot}.txt",
        env!("CARGO_MANIFEST_DIR")
    );
    check_path("mp_carentan", "dm", &path, &PRONE, until);
}

#[test]
fn a_prone_crawl_and_dive_match_retail_on_the_mp_carentan_street() {
    check_prone("street", None);
}

#[test]
fn a_prone_crawl_and_dive_match_retail_on_the_mp_carentan_mound() {
    check_prone("mound", Some(MOUND_UNTIL));
}
