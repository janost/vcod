//! Our mover against retail's with another player in the way. The fixture is
//! `--save-bump` against `client-probes/probe_bump` under `tools/run_probe.sh`:
//! a walker's every usercmd and every snapshot, with the target player's
//! entity (origin, `solid`) beside it, while the walker runs into, round and
//! onto a standing, crouched and prone target. The replay runs the same cmds
//! through `pmove` on our collision world with the target as a [`Body`], the
//! `playerstate_slope_ab.rs` shape: free-running per phase and rebased on
//! retail's playerstate at every snapshot, diffing the origin at every
//! snapshot's `commandTime`.
//!
//! The target is the body the latest snapshot showed, applied to the cmds
//! that follow it: a snapshot is the end of a server frame, and the next
//! frame's cmds run against whatever the target was then. Its `solid` is a
//! frame late by that rule (a stuck mark reaches the wire at the target's
//! next link); taking the next snapshot's instead changes no row of this
//! capture. `target=` is the entity's `pos.trBase`, truncated toward zero on
//! the wire; the target never leaves the header's `spot`, so the body stands
//! at the spot's exact origin and a test checks every `target=` is its
//! truncation.
//!
//! `StuckInClient`'s push is an end-frame server effect pmove cannot make.
//! It writes velocity and the knockback timer after the frame's last move,
//! so a push snapshot's origin still compares; the rebased run then starts
//! the next interval from the pushed state like any other, and the free run
//! adopts retail's velocity and timer at the snapshots `PUSHES` names.
//!
//! Rows that miss for a pmove jump difference rather than a clipping one are
//! named in `GAPS`. Set `BUMP_REPORT=1` to print every snapshot's delta, and
//! `BUMP_TRACE=<ct>` to print ours cmd by cmd into that clock. Needs
//! `COD_DIR`; without the paks the replay returns early.

use glam::Vec3;
use std::collections::BTreeMap;
use vcod_common::collision::CollisionWorld;
use vcod_common::movetrace::{Body, MoveWorld, CONTENTS_BODY};
use vcod_common::net::msg::{
    UserCmd, BUTTON_ADS, BUTTON_ATTACK, BUTTON_MELEE, BUTTON_USE, NULL_USERCMD, WBUTTON_CROUCH,
    WBUTTON_LEAN_LEFT, WBUTTON_LEAN_RIGHT, WBUTTON_PRONE, WBUTTON_RELOAD,
};
use vcod_common::net::protocol::ENTITYNUM_NONE;
use vcod_common::pmove::{pmove, PlayerState, PmInput, PMF_TIME_KNOCKBACK};
use vcod_common::weapon::WeaponDef;

const FIXTURE: &str = "mp_carentan-dm-bump-walker.txt";

/// Each `StuckInClient` push in the fixture, as (phase, snapshot
/// `commandTime`): the snapshots whose `pm_time` reads a fresh 300. Checked
/// against the fixture both ways, so a stale or missing entry fails.
const PUSHES: &[(&str, i32)] = &[("crouch/jump", 70633), ("prone/jump", 97483)];

/// `pm_flags` 0x8, retail's held-jump latch.
const PMF_JUMP_HELD: i32 = 0x8;

#[derive(Clone, Debug)]
struct Snap {
    ct: i32,
    origin: Vec3,
    velocity: Vec3,
    ground: u32,
    pm_flags: i32,
    pm_time: i32,
    target: Vec3,
    target_solid: i32,
    target_num: u32,
}

enum Line {
    Phase(String),
    Cmd(UserCmd),
    Snap(Snap),
}

struct Fixture {
    spot: Vec3,
    yaw_deg: f32,
    /// The cmds' constant yaw rebased onto the header's world yaw, in shorts.
    delta_angles: [i32; 3],
    lines: Vec<Line>,
}

fn parse_vec3(s: &str) -> Vec3 {
    let v: Vec<f32> = s.split(',').map(|x| x.parse().expect("a float")).collect();
    Vec3::new(v[0], v[1], v[2])
}

const ANGLE2SHORT: f32 = 65536.0 / 360.0;

fn parse_fixture(text: &str) -> Fixture {
    let header = text
        .lines()
        .find_map(|l| l.strip_prefix("# bump "))
        .expect("a `# bump spot=.. yaw=..` header");
    let hv: BTreeMap<&str, &str> = header
        .split_whitespace()
        .filter_map(|kv| kv.split_once('='))
        .collect();
    let spot = parse_vec3(hv["spot"]);
    let yaw_deg: f32 = hv["yaw"].parse().unwrap();
    let mut lines = Vec::new();
    let mut cmd_angles: Option<[i32; 3]> = None;
    for line in text.lines() {
        if let Some(p) = line
            .strip_prefix("[phase ")
            .and_then(|p| p.strip_suffix(']'))
        {
            lines.push(Line::Phase(p.to_string()));
            continue;
        }
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
                let a = [a[0], a[1], a[2]];
                // The fixture carries no `da=`: the probe never turns, so the
                // header's world yaw minus the cmd's is the whole rebase.
                match cmd_angles {
                    None => cmd_angles = Some(a),
                    Some(first) => assert_eq!(
                        first, a,
                        "the cmd angles changed within the fixture ({line}): it needs a `da=` field"
                    ),
                }
                lines.push(Line::Cmd(UserCmd {
                    server_time: int("st"),
                    buttons: int("buttons") as u8,
                    wbuttons: int("wbuttons") as u8,
                    weapon: int("weapon") as u8,
                    angles: a,
                    forward: int("forward") as i8,
                    right: int("right") as i8,
                    up: int("up") as i8,
                    ..NULL_USERCMD
                }));
            }
            "!snap" => {
                assert_eq!(kv["stance"], "stand", "the walker always stands: {line}");
                lines.push(Line::Snap(Snap {
                    ct: int("ct"),
                    origin: parse_vec3(kv["origin"]),
                    velocity: parse_vec3(kv["vel"]),
                    ground: int("ground") as u32,
                    pm_flags: int("pm_flags"),
                    pm_time: int("pm_time"),
                    target: parse_vec3(kv["target"]),
                    target_solid: int("target_solid"),
                    target_num: int("target_num") as u32,
                }));
            }
            _ => {}
        }
    }
    let a = cmd_angles.expect("at least one cmd");
    let yaw_short = (yaw_deg * ANGLE2SHORT).round() as i32;
    Fixture {
        spot,
        yaw_deg,
        delta_angles: [-a[0], yaw_short - a[1], -a[2]].map(|v| v.rem_euclid(65536)),
        lines,
    }
}

/// `spectate::pm_input`, which is private to the server crate.
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

fn short_deg(v: i32) -> f32 {
    let deg = v as f32 / ANGLE2SHORT;
    (deg + 180.0).rem_euclid(360.0) - 180.0
}

/// The server's `replay_moves` clocking for one cmd.
fn run_cmd(
    ps: &mut PlayerState,
    cmd: &UserCmd,
    last_st: &mut i32,
    da: [i32; 3],
    world: &MoveWorld,
    weapons: &[Option<WeaponDef>],
) {
    if cmd.server_time.wrapping_sub(*last_st) <= 0 {
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
    // `BUMP_TRACE=<ct>`: print every cmd within 60 ms before that clock.
    if let Some(at) = std::env::var("BUMP_TRACE")
        .ok()
        .and_then(|v| v.parse::<i32>().ok())
    {
        if (at - 60..=at).contains(&cmd.server_time) {
            println!(
                "  cmd st={} up={} fwd={} origin=[{:.3},{:.3},{:.3}] vel=[{:.2},{:.2},{:.2}] ground={} kb={}",
                cmd.server_time,
                cmd.up,
                cmd.forward,
                ps.origin.x,
                ps.origin.y,
                ps.origin.z,
                ps.velocity.x,
                ps.velocity.y,
                ps.velocity.z,
                ps.ground_entity_num(),
                ps.knockback_ms,
            );
        }
    }
}

/// Retail's playerstate at a snapshot: origin, velocity, ground, the held
/// jump latch and the knockback timer.
fn state_from(snap: &Snap, yaw_deg: f32, weapon: u8) -> PlayerState {
    let mut ps = PlayerState::spawn(snap.origin, yaw_deg);
    ps.velocity = snap.velocity;
    ps.on_ground = snap.ground != ENTITYNUM_NONE;
    ps.ground_entity = snap.ground;
    ps.jump_latched = snap.pm_flags & PMF_JUMP_HELD != 0;
    adopt_push(&mut ps, snap);
    ps.weapon = weapon;
    ps.weapons_held = 1 << weapon;
    ps
}

fn adopt_push(ps: &mut PlayerState, snap: &Snap) {
    ps.knockback_ms = if snap.pm_flags & PMF_TIME_KNOCKBACK != 0 {
        snap.pm_time as f32
    } else {
        0.0
    };
}

/// The target as the snapshot shows it: `None` while its `solid` reads 0.
/// `from_solid` decodes cgame's box, one unit below the feet; the server's
/// box starts at them.
fn target_body(snap: &Snap, spot: Vec3) -> Option<Body> {
    if snap.target_solid == 0 {
        return None;
    }
    let mut b = Body::from_solid(snap.target_num, spot, snap.target_solid, CONTENTS_BODY);
    b.mins.z = 0.0;
    Some(b)
}

#[derive(Clone, Copy, Debug, Default)]
struct Delta {
    dz: f32,
    dxy: f32,
    ground_disagrees: bool,
}

fn delta(ours: &PlayerState, retail: &Snap) -> Delta {
    let d = ours.origin - retail.origin;
    Delta {
        dz: d.z,
        dxy: d.truncate().length(),
        ground_disagrees: ours.on_ground != (retail.ground != ENTITYNUM_NONE),
    }
}

struct Row {
    phase: String,
    retail: Snap,
    free: Delta,
    rebased: Delta,
    ours_free: Vec3,
    ours_rebased: Vec3,
}

fn is_push(s: &Snap) -> bool {
    s.pm_flags & PMF_TIME_KNOCKBACK != 0 && s.pm_time == 300
}

fn replay(fx: &Fixture, world: &CollisionWorld, weapons: &[Option<WeaponDef>]) -> Vec<Row> {
    let yaw_deg = fx.yaw_deg;
    let weapon = fx
        .lines
        .iter()
        .find_map(|l| match l {
            Line::Cmd(c) => Some(c.weapon),
            _ => None,
        })
        .unwrap_or(0);
    let cmds: Vec<UserCmd> = fx
        .lines
        .iter()
        .filter_map(|l| match l {
            Line::Cmd(c) => Some(*c),
            _ => None,
        })
        .collect();
    let mut next_cmd = 0usize;
    let mut rows = Vec::new();
    let mut phase = String::new();
    let mut fresh_phase = false;
    let mut prev: Option<Snap> = None;
    let mut free = PlayerState::spawn(Vec3::ZERO, 0.0);
    let mut free_st = 0;
    let mut rebased = free;
    let mut rebased_st = 0;
    let mut pushes_seen = Vec::new();
    for l in &fx.lines {
        let s = match l {
            Line::Phase(p) => {
                phase = p.clone();
                fresh_phase = true;
                continue;
            }
            Line::Cmd(_) => continue,
            Line::Snap(s) => s,
        };
        let Some(p) = prev.as_ref() else {
            rebased = state_from(s, yaw_deg, weapon);
            rebased_st = s.ct;
            prev = Some(s.clone());
            next_cmd = cmds.partition_point(|c| c.server_time <= s.ct);
            continue;
        };
        if s.ct <= p.ct {
            continue;
        }
        if fresh_phase {
            // The free run starts over at each phase, from the snapshot
            // before the phase's first.
            free = state_from(p, yaw_deg, weapon);
            free_st = p.ct;
            fresh_phase = false;
        }
        let bodies: Vec<Body> = target_body(p, fx.spot).into_iter().collect();
        let mw = MoveWorld::new(world, &bodies, u32::MAX);
        while next_cmd < cmds.len() && cmds[next_cmd].server_time <= s.ct {
            let c = &cmds[next_cmd];
            run_cmd(&mut free, c, &mut free_st, fx.delta_angles, &mw, weapons);
            run_cmd(
                &mut rebased,
                c,
                &mut rebased_st,
                fx.delta_angles,
                &mw,
                weapons,
            );
            next_cmd += 1;
        }
        rows.push(Row {
            phase: phase.clone(),
            retail: s.clone(),
            free: delta(&free, s),
            rebased: delta(&rebased, s),
            ours_free: free.origin,
            ours_rebased: rebased.origin,
        });
        if is_push(s) {
            pushes_seen.push((phase.clone(), s.ct));
            free.velocity = s.velocity;
            adopt_push(&mut free, s);
        }
        rebased = state_from(s, yaw_deg, weapon);
        rebased_st = s.ct;
        prev = Some(s.clone());
    }
    let listed: Vec<(String, i32)> = PUSHES.iter().map(|(p, c)| (p.to_string(), *c)).collect();
    assert_eq!(
        pushes_seen, listed,
        "the fixture's push snapshots and PUSHES disagree"
    );
    rows
}

struct Stats {
    max_dz: f32,
    max_dxy: f32,
    p95_dz: f32,
    p95_dxy: f32,
    p99_dz: f32,
    p99_dxy: f32,
    outliers: usize,
    ground_disagreements: usize,
    n: usize,
}

fn stats<'a>(rows: impl Iterator<Item = &'a Row>, pick: impl Fn(&Row) -> Delta) -> Stats {
    let d: Vec<Delta> = rows.map(pick).collect();
    let mut dz: Vec<f32> = d.iter().map(|d| d.dz.abs()).collect();
    let mut dxy: Vec<f32> = d.iter().map(|d| d.dxy).collect();
    dz.sort_by(f32::total_cmp);
    dxy.sort_by(f32::total_cmp);
    let at = |v: &[f32], q: f32| v.get((q * v.len() as f32) as usize).copied().unwrap_or(0.0);
    Stats {
        max_dz: dz.last().copied().unwrap_or(0.0),
        max_dxy: dxy.last().copied().unwrap_or(0.0),
        p95_dz: at(&dz, 0.95),
        p95_dxy: at(&dxy, 0.95),
        p99_dz: at(&dz, 0.99),
        p99_dxy: at(&dxy, 0.99),
        outliers: d.iter().filter(|d| d.dz.abs() > 1.0 || d.dxy > 1.0).count(),
        ground_disagreements: d.iter().filter(|d| d.ground_disagrees).count(),
        n: d.len(),
    }
}

impl std::fmt::Display for Stats {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(
            f,
            "n={} max dz {:.3} dxy {:.3} p95 dz {:.3} dxy {:.3} p99 dz {:.3} dxy {:.3} past-a-unit {} ground {}",
            self.n,
            self.max_dz,
            self.max_dxy,
            self.p95_dz,
            self.p95_dxy,
            self.p99_dz,
            self.p99_dxy,
            self.outliers,
            self.ground_disagreements
        )
    }
}

fn phases(rows: &[Row]) -> Vec<&str> {
    let mut out: Vec<&str> = Vec::new();
    for r in rows {
        if out.last() != Some(&r.phase.as_str()) {
            out.push(&r.phase);
        }
    }
    out
}

fn report(rows: &[Row], spot: Vec3) {
    if std::env::var_os("BUMP_REPORT").is_none() {
        return;
    }
    for (i, r) in rows.iter().enumerate() {
        let o = r.retail.origin;
        println!(
            "{i:>4} {:<18} ct={} retail=[{:.3},{:.3},{:.3}] d={:.4} vel=[{:.1},{:.1},{:.1}] pm_time={} solid={} \
             | free [{:.3},{:.3},{:.3}] dz={:.3} dxy={:.3} | rebased [{:.3},{:.3},{:.3}] dz={:.3} dxy={:.3}{}",
            r.phase,
            r.retail.ct,
            o.x,
            o.y,
            o.z,
            (o - spot).truncate().length(),
            r.retail.velocity.x,
            r.retail.velocity.y,
            r.retail.velocity.z,
            r.retail.pm_time,
            r.retail.target_solid,
            r.ours_free.x,
            r.ours_free.y,
            r.ours_free.z,
            r.free.dz,
            r.free.dxy,
            r.ours_rebased.x,
            r.ours_rebased.y,
            r.ours_rebased.z,
            r.rebased.dz,
            r.rebased.dxy,
            if r.rebased.ground_disagrees {
                " ground!"
            } else {
                ""
            },
        );
    }
}

/// The slope gate's rebased bounds (`playerstate_slope_ab.rs`).
const P95_TOLERANCE_Z: f32 = 0.02;
const P95_TOLERANCE_XY: f32 = 0.05;
const P99_TOLERANCE_Z: f32 = 0.05;
const P99_TOLERANCE_XY: f32 = 0.6;
const MAX_TOLERANCE_Z: f32 = 0.5;

/// Loads the map the way the slope gate does, with `dm`'s gameobject rule.
fn load() -> Option<(CollisionWorld, vcod_server::weapons::WeaponTable)> {
    let fs = vcod_common::testing::game_fs()?;
    let bsp_path = fs
        .resolve_map("mp_carentan")
        .expect("map in the mounted paks");
    let bsp = vcod_common::bsp::parse(&fs.read(&bsp_path).expect("read the bsp")).expect("parse");
    let world = vcod_server::world::World::from_bsp(&bsp, Some(&fs)).collision;
    unlink_gameobjects(&world, &bsp.entities, "dm");
    Some((world, vcod_server::weapons::WeaponTable::load(&fs)))
}

/// `playerstate_slope_ab.rs`'s: what `_gameobjects::main` deletes before a
/// client walks.
fn unlink_gameobjects(world: &CollisionWorld, entities: &str, gametype: &str) {
    for block in vcod_common::bsp::entity_blocks(entities) {
        let Some(name) = block.get("script_gameobjectname") else {
            continue;
        };
        if name == gametype {
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

fn fixture() -> Fixture {
    let path = format!(
        "{}/tests/fixtures/playerstate/{FIXTURE}",
        env!("CARGO_MANIFEST_DIR")
    );
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{path}: {e}"));
    parse_fixture(&text)
}

fn snaps(fx: &Fixture) -> impl Iterator<Item = &Snap> {
    fx.lines.iter().filter_map(|l| match l {
        Line::Snap(s) => Some(s),
        _ => None,
    })
}

/// Runs without the paks: the fixture's own shape.
#[test]
fn captured_solid_values_are_the_three_stance_packs_at_a_fixed_spot() {
    let fx = fixture();
    let stand = Vec3::new(15.0, 15.0, 70.0);
    let pack = |z: f32| Body::pack_solid(Vec3::new(-15.0, -15.0, 0.0), stand.with_z(z));
    let expected: std::collections::BTreeSet<i32> =
        [pack(70.0), pack(50.0), pack(30.0)].into_iter().collect();
    let captured: std::collections::BTreeSet<i32> = snaps(&fx)
        .map(|s| s.target_solid)
        .filter(|&v| v != 0)
        .collect();
    assert_eq!(captured, expected);
    for s in snaps(&fx) {
        assert_eq!(s.target, fx.spot.trunc(), "the target moved at ct={}", s.ct);
    }
    assert_eq!(fx.delta_angles, [0, 16384, 0]);
}

/// Rebased rows the mover misses for reasons outside player clipping: the
/// phase, the snapshot `commandTime` and which of the two pmove jump
/// differences the walker's jump exposed (docs/research/cod11-player-clip.md,
/// "What the bump capture measured"). Kept out of the stats and checked to
/// still miss, so an entry that starts matching fails.
const GAPS: &[(&str, i32, &str)] = &[
    ("stand/jump", 39083, TAKEOFF),
    ("stand/jump", 39682, REJUMP),
    ("stand/land", 39732, REJUMP),
    ("crouch/jump", 70232, TAKEOFF),
    ("crouch/land", 70882, REJUMP),
    ("prone/jump", 97033, TAKEOFF),
    ("prone/land", 97666, REJUMP),
];
/// Retail's forward jump leaves at 249.8 (`PM_Jump`'s `sqrt(g * 78)`), ours
/// at 233.2.
const TAKEOFF: &str = "takeoff speed";
/// Ours jumps again off the landing with up still held; retail's held-jump
/// latch (`pm_flags` 0x8) refuses.
const REJUMP: &str = "held-jump latch";

fn is_gap(r: &Row) -> bool {
    GAPS.iter()
        .any(|(p, ct, _)| *p == r.phase && *ct == r.retail.ct)
}

fn misses(d: Delta) -> bool {
    d.dz.abs() > MAX_TOLERANCE_Z || d.dxy > P99_TOLERANCE_XY || d.ground_disagrees
}

/// The walker's centre distance from the target's, on the floor plane.
fn dist(p: Vec3, spot: Vec3) -> f32 {
    (p - spot).truncate().length()
}

#[test]
fn the_mover_bumps_where_retail_does() {
    let Some((world, weapons)) = load() else {
        eprintln!("COD_DIR unset or has no main/: skipping");
        return;
    };
    let fx = fixture();
    let rows = replay(&fx, &world, weapons.defs());
    assert!(rows.len() > 1000, "only {} snapshots", rows.len());
    report(&rows, fx.spot);
    for p in phases(&rows) {
        let of = || rows.iter().filter(move |r| r.phase == p);
        println!("{p:<18} free    {}", stats(of(), |r| r.free));
        println!("{p:<18} rebased {}", stats(of(), |r| r.rebased));
    }
    // The head-on stop and the glance's closest pass, ours from the phase's
    // free run: the stop is what pins `BODY_RADIUS_EPS`.
    for stance in ["stand", "crouch", "prone"] {
        let headon = format!("{stance}/headon");
        let last = rows.iter().rfind(|r| r.phase == headon).unwrap();
        let (retail, ours) = (
            dist(last.retail.origin, fx.spot),
            dist(last.ours_free, fx.spot),
        );
        println!("{headon}: stop retail {retail:.4} ours {ours:.4}");
        assert!(
            (retail - 30.125).abs() < 0.01 && (ours - retail).abs() < 0.01,
            "{headon}: stop retail {retail:.4} ours {ours:.4}"
        );
        let glance = format!("{stance}/glance");
        let min = |f: fn(&Row) -> Vec3| {
            rows.iter()
                .filter(|r| r.phase == glance)
                .map(|r| dist(f(r), fx.spot))
                .fold(f32::MAX, f32::min)
        };
        let (retail, ours) = (min(|r| r.retail.origin), min(|r| r.ours_free));
        println!("{glance}: closest retail {retail:.4} ours {ours:.4}");
        // Inside the 0.125: retail backs off only a hit on the bare radius.
        assert!(
            retail < 30.1 && (ours - retail).abs() < 0.01,
            "{glance}: closest retail {retail:.4} ours {ours:.4}"
        );
    }
    for (phase, ct, why) in GAPS {
        let row = rows
            .iter()
            .find(|r| r.phase == *phase && r.retail.ct == *ct)
            .unwrap_or_else(|| panic!("GAPS names {phase} ct={ct}, which the fixture lacks"));
        assert!(
            misses(row.rebased),
            "GAPS entry {phase} ct={ct} ({why}) now matches: {:?}",
            row.rebased
        );
    }
    let kept = || rows.iter().filter(|r| !is_gap(r));
    let rebased = stats(kept(), |r| r.rebased);
    println!("rebased outside GAPS {rebased}");
    assert!(
        rebased.p95_dz <= P95_TOLERANCE_Z && rebased.p95_dxy <= P95_TOLERANCE_XY,
        "rebased p95 past the tolerance: {rebased}"
    );
    assert!(
        rebased.p99_dz <= P99_TOLERANCE_Z && rebased.p99_dxy <= P99_TOLERANCE_XY,
        "rebased p99 past the tolerance: {rebased}"
    );
    if let Some(r) = kept().find(|r| misses(r.rebased)) {
        panic!(
            "{} ct={} misses outside GAPS: {:?}",
            r.phase, r.retail.ct, r.rebased
        );
    }
}
