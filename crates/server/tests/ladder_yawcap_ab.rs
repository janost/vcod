//! `bg_ladder_yawcap` against retail. The fixture is `--probe-ladder 90
//! --save-ladder turn --capture-tag turn` against `client-probes/probe_ladder`
//! under `tools/run_probe.sh mp_ship +set probe_teleport 1` (1.1d): up the
//! hold ladder, then the view swept past the cap both ways while hanging, a
//! climb and a flick while turned, the climb down and the same sweep at the
//! foot on the ground, every cmd on the wire as a mouse sends it. The
//! `-1.5` and `-uo` captures are the same run under `COD_VERSION=1.5` and
//! `uo`, replayed under their own `MoveRules`.
//!
//! The replay runs the cmds through `cmd::player_step`, the server's step,
//! which pushes `delta_angles` as retail's cap does, and diffs
//! `delta_angles[1]`, the view yaw and the origin at every snapshot: rebased
//! on retail's playerstate (its `vLadderVec` included) at each one, and
//! free-running per phase. `LADDER_REPORT=1` prints every row. Needs
//! `COD_DIR`; without the paks the replay returns early, and the 1.1
//! capture replays on a 1.1 install only (docs/research/cod11-mantle.md,
//! "The ladder yaw cap").

use glam::Vec3;
use std::collections::BTreeMap;
use vcod_common::collision::CollisionWorld;
use vcod_common::movetrace::MoveWorld;
use vcod_common::net::flags::{PMF_JUMP_HELD, PMF_LADDER};
use vcod_common::net::msg::{NULL_USERCMD, UserCmd};
use vcod_common::net::protocol::ENTITYNUM_NONE;
use vcod_common::patch::Arith;
use vcod_common::pmove::cmd::{self, EventRing};
use vcod_common::pmove::{MoveRules, PlayerState};
use vcod_common::weapon::WeaponDef;

/// Each capture and the patch whose server it ran on.
const FIXTURES: &[(&str, MoveRules)] = &[
    ("mp_ship-dm-ladder-turn.txt", MoveRules::V1_1),
    ("mp_ship-dm-ladder-turn-1.5.txt", MoveRules::V1_5),
    ("mp_ship-dm-ladder-turn-uo.txt", MoveRules::Uo),
];

#[derive(Clone, Debug)]
struct Snap {
    ct: i32,
    origin: Vec3,
    velocity: Vec3,
    ground: u32,
    pm_flags: i32,
    da: [i32; 3],
    weapon: u8,
    view: [f32; 3],
    ladder_vec: Vec3,
}

enum Line {
    Phase(String),
    Cmd(UserCmd),
    Snap(Snap),
}

fn parse_vec3(s: &str) -> Vec3 {
    let v: Vec<f32> = s.split(',').map(|x| x.parse().expect("a float")).collect();
    Vec3::new(v[0], v[1], v[2])
}

fn parse_fixture(text: &str) -> Vec<Line> {
    let mut lines = Vec::new();
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
        let ints = |k: &str| -> [i32; 3] {
            let v: Vec<i32> = kv[k].split(',').map(|x| x.parse().unwrap()).collect();
            [v[0], v[1], v[2]]
        };
        match kind {
            "!cmd" => lines.push(Line::Cmd(UserCmd {
                server_time: int("st"),
                buttons: int("buttons") as u8,
                wbuttons: int("wbuttons") as u8,
                weapon: int("weapon") as u8,
                angles: ints("angles"),
                forward: int("forward") as i8,
                right: int("right") as i8,
                up: int("up") as i8,
                ..NULL_USERCMD
            })),
            "!snap" => lines.push(Line::Snap(Snap {
                ct: int("ct"),
                origin: parse_vec3(kv["origin"]),
                velocity: parse_vec3(kv["vel"]),
                ground: int("ground") as u32,
                pm_flags: int("pm_flags"),
                da: ints("da"),
                weapon: int("weapon") as u8,
                view: parse_vec3(kv["view"]).to_array(),
                ladder_vec: parse_vec3(kv["lv"]),
            })),
            _ => {}
        }
    }
    lines
}

/// Retail's playerstate at a snapshot.
fn state_from(snap: &Snap, climb_weapon: u8, rules: MoveRules) -> PlayerState {
    let mut ps = PlayerState::spawn(snap.origin, snap.view[1]);
    ps.rules = rules;
    ps.velocity = snap.velocity;
    ps.on_ground = snap.ground != ENTITYNUM_NONE;
    ps.ground_entity = snap.ground;
    ps.jump_latched = snap.pm_flags & PMF_JUMP_HELD != 0;
    ps.on_ladder = snap.pm_flags & PMF_LADDER != 0;
    ps.ladder_normal = snap.ladder_vec;
    ps.weapon = snap.weapon;
    ps.weapons_held = 1 << climb_weapon;
    ps
}

/// One body and its `delta_angles` through the server's step.
struct Body {
    ps: PlayerState,
    da: [i32; 3],
    view: [f32; 3],
    st: i32,
}

impl Body {
    fn from(snap: &Snap, climb_weapon: u8, rules: MoveRules) -> Self {
        Body {
            ps: state_from(snap, climb_weapon, rules),
            da: snap.da.map(|d| d & 0xffff),
            view: snap.view,
            st: snap.ct,
        }
    }

    fn run(&mut self, c: &UserCmd, world: &MoveWorld, weapons: &[Option<WeaponDef>]) {
        if c.server_time.wrapping_sub(self.st) <= 0 {
            return;
        }
        let mut ring = EventRing::default();
        let (ps, da, view) = (&mut self.ps, &mut self.da, &mut self.view);
        cmd::chop(self.st, c, |step, dt| {
            *view = cmd::player_step(ps, da, &mut ring, step, dt, world, weapons).view;
            ps.jump_latched
        });
        self.st = c.server_time;
    }
}

/// How far our view yaw lands from retail's as the wire carries it, in
/// shorts: 1.1 and 1.5 send the float, which has to match to the bit (a
/// capped view is 0..360, an uncapped one -180..180), UO a short truncated
/// toward zero (coduo-diff.md 3).
fn view_miss(ours: f32, retail: f32, rules: MoveRules) -> f32 {
    let unit = f64::from(65536.0f32 / 360.0);
    let shorts = |deg: f64| (deg * unit).round() as i64;
    let wrap = |d: i64| ((d.rem_euclid(65536) + 32768) % 65536 - 32768).abs() as f32;
    if rules == MoveRules::Uo {
        return wrap((f64::from(ours) * unit) as i64 - shorts(f64::from(retail)));
    }
    if ours == retail || (ours - retail).abs() == 360.0 {
        0.0
    } else {
        wrap(shorts(f64::from(ours)) - shorts(f64::from(retail))).max(0.5)
    }
}

#[derive(Debug)]
struct Row {
    phase: String,
    ct: i32,
    capped: bool,
    rebased: [f32; 3],
    free: [f32; 3],
}

/// (delta_angles[1] miss in shorts, view yaw miss in shorts, origin miss).
fn miss(b: &Body, s: &Snap) -> [f32; 3] {
    let wrap = |d: i32| (d.rem_euclid(65536) + 32768) % 65536 - 32768;
    [
        wrap(b.da[1] - (s.da[1] & 0xffff)).abs() as f32,
        view_miss(b.view[1], s.view[1], b.ps.rules),
        (b.ps.origin - s.origin).length(),
    ]
}

fn replay(
    lines: &[Line],
    rules: MoveRules,
    world: &CollisionWorld,
    weapons: &[Option<WeaponDef>],
) -> Vec<Row> {
    let cmds: Vec<UserCmd> = lines
        .iter()
        .filter_map(|l| match l {
            Line::Cmd(c) => Some(*c),
            _ => None,
        })
        .collect();
    let climb_weapon = cmds.iter().map(|c| c.weapon).find(|&w| w != 0).unwrap_or(0);
    let mw = MoveWorld::new(world, &[], u32::MAX);
    let mut rows = Vec::new();
    let mut phase = String::new();
    let mut fresh_phase = false;
    let mut prev: Option<Snap> = None;
    let mut free: Option<Body> = None;
    let mut next_cmd = 0;
    for l in lines {
        let s = match l {
            Line::Phase(p) => {
                phase = p.clone();
                fresh_phase = true;
                continue;
            }
            Line::Cmd(_) => continue,
            Line::Snap(s) => s,
        };
        if prev.is_none() && s.ct < cmds[0].server_time {
            continue;
        }
        let Some(p) = prev.as_ref() else {
            prev = Some(s.clone());
            next_cmd = cmds.partition_point(|c| c.server_time <= s.ct);
            continue;
        };
        if s.ct <= p.ct {
            continue;
        }
        if fresh_phase || free.is_none() {
            free = Some(Body::from(p, climb_weapon, rules));
            fresh_phase = false;
        }
        let free = free.as_mut().expect("set above");
        let mut rebased = Body::from(p, climb_weapon, rules);
        while next_cmd < cmds.len() && cmds[next_cmd].server_time <= s.ct {
            rebased.run(&cmds[next_cmd], &mw, weapons);
            free.run(&cmds[next_cmd], &mw, weapons);
            next_cmd += 1;
        }
        rows.push(Row {
            phase: phase.clone(),
            ct: s.ct,
            capped: s.da[1] & 0xffff != p.da[1] & 0xffff,
            rebased: miss(&rebased, s),
            free: miss(free, s),
        });
        prev = Some(s.clone());
    }
    rows
}

fn load() -> Option<(CollisionWorld, vcod_server::weapons::WeaponTable)> {
    let fs = vcod_common::testing::game_fs()?;
    let bsp_path = fs.resolve_map("mp_ship").expect("map in the mounted paks");
    let bsp = vcod_common::bsp::parse(&fs.read(&bsp_path).expect("read the bsp")).expect("parse");
    let world = vcod_server::world::World::from_bsp(&bsp, Some(&fs)).collision;
    world.unlink_script_brushes(&bsp.entities, "dm");
    Some((
        world,
        vcod_server::weapons::WeaponTable::load(&fs, &vcod_server::defs::V1),
    ))
}

#[test]
fn a_climber_looking_away_is_capped_where_retail_caps_him() {
    let Some((world, weapons)) = load() else {
        eprintln!("COD_DIR unset or has no main/: skipping");
        return;
    };
    // A 1.1 capture replays on 1.1's paks only (a UO install's `main/` is
    // 1.5's).
    let install_11 = vcod_common::testing::game_fs_11().is_some();
    // UO numbers its weapons its own way (`defs::UO`).
    let uo_weapons = vcod_common::testing::uo_fs()
        .map(|fs| vcod_server::weapons::WeaponTable::load(&fs, &vcod_server::defs::UO));
    for (name, rules) in FIXTURES {
        if *rules == MoveRules::V1_1 && !install_11 {
            continue;
        }
        let weapons = match rules {
            MoveRules::Uo => match &uo_weapons {
                Some(w) => w,
                None => continue,
            },
            _ => &weapons,
        };
        println!("{name}");
        world.set_arith(if rules.v15() {
            Arith::Stored
        } else {
            Arith::Kept
        });
        let path = format!(
            "{}/tests/fixtures/playerstate/{name}",
            vcod_common::manifest_dir!()
        );
        let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{path}: {e}"));
        check(&replay(
            &parse_fixture(&text),
            *rules,
            &world,
            weapons.defs(),
        ));
    }
}

fn check(rows: &[Row]) {
    let report = std::env::var_os("LADDER_REPORT").is_some();
    let mut worst: BTreeMap<&str, ([f32; 3], [f32; 3])> = BTreeMap::new();
    for r in rows {
        if report {
            println!("{r:?}");
        }
        let w = worst.entry(&r.phase).or_default();
        for i in 0..3 {
            w.0[i] = w.0[i].max(r.rebased[i]);
            w.1[i] = w.1[i].max(r.free[i]);
        }
    }
    for (p, (b, f)) in &worst {
        println!(
            "{p:<12} rebased da {} view {} origin {:e} | free da {} view {} origin {:e}",
            b[0], b[1], b[2], f[0], f[1], f[2]
        );
    }
    assert!(rows.len() > 300, "only {} snapshots", rows.len());
    let pushes = rows.iter().filter(|r| r.capped).count();
    assert!(pushes > 40, "the capture caps on only {pushes} snapshots");
    for r in rows {
        assert!(
            r.rebased == [0.0; 3] && r.free == [0.0; 3],
            "{} ct {}: rebased misses {:?}, free {:?} (da shorts, view shorts, origin)",
            r.phase,
            r.ct,
            r.rebased,
            r.free
        );
    }
}
