//! Our ladder mover against retail's. The fixture is `--probe-ladder 90
//! --save-ladder` against `client-probes/probe_ladder` under
//! `tools/run_probe.sh mp_ship` (1.1d; the `-1.5` and `-uo` captures under
//! `COD_VERSION=1.5` and `uo`, replayed under their own `MoveRules`): every
//! usercmd and every snapshot while the player climbs the hold ladder at several pitches, backs down, strafes,
//! lets go of each key and climbs off the top onto the deck. The replay runs
//! the same cmds through `pmove` on our collision world, the `bump_ab.rs`
//! shape: free-running per phase and rebased on retail's playerstate at
//! every snapshot, diffing the origin and velocity at every snapshot's
//! `commandTime`.
//!
//! The older captures log no `vLadderVec`, so a rebased run on the rungs takes the
//! normal the probe faced, square into the rungs. Set
//! `LADDER_REPORT=1` to print every snapshot's delta. Needs `COD_DIR`;
//! without the paks the replay returns early, save the taped one at the
//! end (docs/dev/testing.md, "Trace tapes").

use glam::Vec3;
use std::collections::BTreeMap;
use vcod_common::collision::CollisionWorld;
use vcod_common::movetrace::MoveWorld;
use vcod_common::net::msg::{NULL_USERCMD, UserCmd};
use vcod_common::net::protocol::ENTITYNUM_NONE;
use vcod_common::patch::Arith;
use vcod_common::pmove::cmd;
use vcod_common::pmove::{MoveRules, PlayerState, pmove};
use vcod_common::tracetape::Tape;
use vcod_common::weapon::WeaponDef;

use crate::common::tapes;
use vcod_common::net::flags::{PMF_JUMP_HELD, PMF_LADDER};

/// Each capture, the world yaw it faced the rungs at and the patch whose
/// server it ran on.
const FIXTURES: &[(&str, f32, MoveRules)] = &[
    ("mp_ship-dm-ladder.txt", 90.0, MoveRules::V1_1),
    (
        "mp_ship-dm-ladder-crowsnest-climb.txt",
        0.0,
        MoveRules::V1_1,
    ),
    ("mp_ship-dm-ladder-crowsnest.txt", 0.0, MoveRules::V1_1),
    ("mp_ship-dm-ladder-1.5.txt", 90.0, MoveRules::V1_5),
    ("mp_ship-dm-ladder-uo.txt", 90.0, MoveRules::Uo),
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
    /// `vLadderVec`, in the captures that log it.
    lv: Option<Vec3>,
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
                lv: kv.get("lv").map(|v| parse_vec3(v)),
            })),
            _ => {}
        }
    }
    lines
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
    cmd::apply_view(ps, cmd.angles, da);
    cmd::chop(*last_st, cmd, |step, dt| {
        pmove(ps, &cmd::pm_input(step), world, dt, weapons);
        ps.jump_latched
    });
    *last_st = cmd.server_time;
}

/// Retail's playerstate at a snapshot: origin, velocity, ground, the ladder
/// flag and the held weapon.
fn state_from(snap: &Snap, climb_weapon: u8, yaw: f32, rules: MoveRules) -> PlayerState {
    let mut ps = PlayerState::spawn(snap.origin, yaw);
    ps.rules = rules;
    ps.velocity = snap.velocity;
    ps.on_ground = snap.ground != ENTITYNUM_NONE;
    ps.ground_entity = snap.ground;
    ps.jump_latched = snap.pm_flags & PMF_JUMP_HELD != 0;
    ps.on_ladder = snap.pm_flags & PMF_LADDER != 0;
    if let Some(lv) = snap.lv {
        ps.ladder_normal = lv;
    } else if ps.on_ladder {
        let yaw = yaw.to_radians();
        ps.ladder_normal = -Vec3::new(yaw.cos(), yaw.sin(), 0.0);
    }
    ps.weapon = snap.weapon;
    ps.weapons_held = 1 << climb_weapon;
    ps
}

#[derive(Clone, Copy, Debug, Default)]
struct Delta {
    dz: f32,
    dxy: f32,
    dv: f32,
    ladder_disagrees: bool,
}

fn delta(ours: &PlayerState, retail: &Snap) -> Delta {
    let d = ours.origin - retail.origin;
    Delta {
        dz: d.z,
        dxy: d.truncate().length(),
        dv: (ours.velocity - retail.velocity).length(),
        ladder_disagrees: ours.on_ladder != (retail.pm_flags & PMF_LADDER != 0),
    }
}

struct Row {
    phase: String,
    retail: Snap,
    free: Delta,
    rebased: Delta,
    ours_free: PlayerState,
    ours_rebased: PlayerState,
}

fn replay(
    lines: &[Line],
    yaw: f32,
    rules: MoveRules,
    world: &CollisionWorld,
    weapons: &[Option<WeaponDef>],
) -> Vec<Row> {
    replay_taped(lines, yaw, rules, world, None, weapons)
}

/// [`replay`], its traces recorded on or answered from `tape`.
fn replay_taped(
    lines: &[Line],
    yaw: f32,
    rules: MoveRules,
    world: &CollisionWorld,
    tape: Option<&Tape>,
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
    let mw = tape.map_or(mw, |t| mw.with_tape(t));
    let mut next_cmd = 0usize;
    let mut rows = Vec::new();
    let mut phase = String::new();
    let mut fresh_phase = false;
    let mut prev: Option<Snap> = None;
    let mut free = PlayerState::spawn(Vec3::ZERO, 0.0);
    let mut free_st = 0;
    let mut rebased = free;
    let mut rebased_st = 0;
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
        // The capture records cmds from the script's first on, and the
        // snapshot before it may predate a cmd it never wrote.
        if prev.is_none() && s.ct < cmds[0].server_time {
            continue;
        }
        let Some(p) = prev.as_ref() else {
            rebased = state_from(s, climb_weapon, yaw, rules);
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
            free = state_from(p, climb_weapon, yaw, rules);
            free_st = p.ct;
            fresh_phase = false;
        }
        while next_cmd < cmds.len() && cmds[next_cmd].server_time <= s.ct {
            let c = &cmds[next_cmd];
            run_cmd(&mut free, c, &mut free_st, p.da, &mw, weapons);
            run_cmd(&mut rebased, c, &mut rebased_st, p.da, &mw, weapons);
            next_cmd += 1;
        }
        rows.push(Row {
            phase: phase.clone(),
            retail: s.clone(),
            free: delta(&free, s),
            rebased: delta(&rebased, s),
            ours_free: free,
            ours_rebased: rebased,
        });
        rebased = state_from(s, climb_weapon, yaw, rules);
        rebased_st = s.ct;
        prev = Some(s.clone());
    }
    rows
}

fn size(d: Delta) -> f32 {
    d.dz.abs().max(d.dxy)
}

fn report(rows: &[Row]) {
    if std::env::var_os("LADDER_REPORT").is_none() {
        return;
    }
    for r in rows {
        let (o, v) = (r.retail.origin, r.retail.velocity);
        let (f, b) = (&r.ours_free, &r.ours_rebased);
        println!(
            "{:<15} ct={} retail [{},{},{}] v [{:.1},{:.1},{:.1}] fl={:#x} | free dz={:e} dxy={:e} v [{:.1},{:.1},{:.1}] | rebased dz={:e} dxy={:e} v [{:.1},{:.1},{:.1}]{}",
            r.phase,
            r.retail.ct,
            o.x,
            o.y,
            o.z,
            v.x,
            v.y,
            v.z,
            r.retail.pm_flags,
            r.free.dz,
            r.free.dxy,
            f.velocity.x,
            f.velocity.y,
            f.velocity.z,
            r.rebased.dz,
            r.rebased.dxy,
            b.velocity.x,
            b.velocity.y,
            b.velocity.z,
            if r.rebased.ladder_disagrees {
                " ladder!"
            } else {
                ""
            },
        );
    }
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

fn fixture(name: &str) -> Vec<Line> {
    let path = format!(
        "{}/tests/fixtures/playerstate/{name}",
        env!("CARGO_MANIFEST_DIR")
    );
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{path}: {e}"));
    parse_fixture(&text)
}

#[test]
fn the_ladder_mover_climbs_where_retail_does() {
    let Some((world, weapons)) = load() else {
        eprintln!("COD_DIR unset or has no main/: skipping");
        return;
    };
    // A 1.1 capture replays on 1.1's paks only (a UO install's `main/` is
    // 1.5's).
    let install_11 = vcod_common::testing::game_fs_11().is_some();
    // UO numbers its weapons its own way (`defs::UO`): the capture's climb
    // weapon 12 is the enfield, and its `moveSpeedScale` sets the deck walk.
    let uo_weapons = vcod_common::testing::uo_fs()
        .map(|fs| vcod_server::weapons::WeaponTable::load(&fs, &vcod_server::defs::UO));
    for (name, yaw, rules) in FIXTURES {
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
        // 1.5b's and UO's servers store their floats (docs/dev/traps.md,
        // "Movement and collision").
        world.set_arith(if rules.v15() {
            Arith::Stored
        } else {
            Arith::Kept
        });
        check(
            name,
            &replay(&fixture(name), *yaw, *rules, &world, weapons.defs()),
        );
    }
}

fn check(name: &str, rows: &[Row]) {
    assert!(rows.len() > 150, "only {} snapshots", rows.len());
    report(rows);
    let mut worst: BTreeMap<&str, (f32, f32, f32)> = BTreeMap::new();
    for r in rows {
        let w = worst.entry(&r.phase).or_default();
        w.0 = w.0.max(size(r.free));
        w.1 = w.1.max(size(r.rebased));
        w.2 = w.2.max(r.rebased.dv);
    }
    for (p, (free, rebased, dv)) in &worst {
        println!("{p:<15} free {free:e} rebased {rebased:e} dv {dv:e}");
    }
    // Every row matches to the bit, origin and velocity, on all three
    // servers (docs/research/cod11-mantle.md, "Ladders").
    for r in rows {
        assert!(
            !r.rebased.ladder_disagrees,
            "{name} {} ct={}: ours on the ladder {}, retail pm_flags {:#x}",
            r.phase, r.retail.ct, r.ours_rebased.on_ladder, r.retail.pm_flags
        );
        assert!(
            size(r.rebased) == 0.0 && r.rebased.dv == 0.0,
            "{name} {} ct={}: rebased {:?}",
            r.phase,
            r.retail.ct,
            r.rebased
        );
        assert!(
            size(r.free) == 0.0,
            "{name} {} ct={}: free {:?}",
            r.phase,
            r.retail.ct,
            r.free
        );
    }
}

/// Runs without the paks: each patch's hold ladder capture climbs at 59.375
/// units/s with a level view, `pm_ladderScale` 0.5 x upscale 0.625 x 190,
/// and at the clamp's 95 looking up and down (docs/research/cod11-mantle.md,
/// "Ladders"). The wire velocity is the frame's end `SnapVector`, so 59.375
/// reads 59.
#[test]
fn retail_climbs_at_the_level_and_clamped_rates() {
    for name in [FIXTURES[0].0, FIXTURES[3].0, FIXTURES[4].0] {
        let lines = fixture(name);
        let mut phase = "";
        let mut seen: BTreeMap<&str, Vec<f32>> = BTreeMap::new();
        for l in &lines {
            match l {
                Line::Phase(p) => phase = p,
                Line::Snap(s) if s.pm_flags & PMF_LADDER != 0 => {
                    seen.entry(phase).or_default().push(s.velocity.z)
                }
                _ => {}
            }
        }
        let settled = |p: &str| -> f32 {
            let v = &seen[p];
            v[v.len() / 2]
        };
        assert_eq!(settled("up-level"), 59.0, "{name}");
        assert_eq!(settled("up-look-up"), 95.0, "{name}");
        assert_eq!(settled("down-look-down"), -95.0, "{name}");
        assert_eq!(settled("down-back"), -59.0, "{name}");
    }
}

/// The `--save-ladder grab` captures (`mp_ship-dm-ladder-grab-*.txt`): set
/// down 23 units left of the hold ladder's foot at (4308, -94, 57) facing
/// away, turned to yaw 60 and walked into the rungs. The view's 8-unit trace
/// meets them a cmd or two before a trace along their normal does, so 1.5b
/// and UO send `vLadderVec` without the ladder flag for those snapshots
/// (14082, 13982), where 1.1d grabs on the first walking cmd
/// (cod15-movement.md 2). Every snapshot replays from retail's the one
/// before, `vLadderVec` included, and matches to the bit.
#[test]
fn a_15_grab_waits_for_the_trace_along_the_normal() {
    let Some((world, weapons)) = load() else {
        eprintln!("COD_DIR unset or has no main/: skipping");
        return;
    };
    let install_11 = vcod_common::testing::game_fs_11().is_some();
    let uo_weapons = vcod_common::testing::uo_fs()
        .map(|fs| vcod_server::weapons::WeaponTable::load(&fs, &vcod_server::defs::UO));
    let grabs = [
        ("mp_ship-dm-ladder-grab-11.txt", MoveRules::V1_1),
        ("mp_ship-dm-ladder-grab-15.txt", MoveRules::V1_5),
        ("mp_ship-dm-ladder-grab-uo.txt", MoveRules::Uo),
    ];
    let mut ran = 0;
    for (name, rules) in grabs {
        if rules == MoveRules::V1_1 && !install_11 {
            continue;
        }
        let weapons = match rules {
            MoveRules::Uo => match &uo_weapons {
                Some(w) => w,
                None => continue,
            },
            _ => &weapons,
        };
        world.set_arith(if rules.v15() {
            Arith::Stored
        } else {
            Arith::Kept
        });
        let lines = fixture(name);
        let rows = replay(&lines, 240.0, rules, &world, weapons.defs());
        report(&rows);
        // The snapshots past the turn, when the walk starts.
        let walk: Vec<&Row> = rows
            .iter()
            .filter(|r| matches!(r.phase.as_str(), "walk" | "hold"))
            .collect();
        assert!(walk.len() > 40, "{name}: {} walk snapshots", walk.len());
        for r in &walk {
            assert!(
                !r.rebased.ladder_disagrees,
                "{name} {} ct={}: ours on the ladder {}, retail pm_flags {:#x}",
                r.phase, r.retail.ct, r.ours_rebased.on_ladder, r.retail.pm_flags
            );
            assert_eq!(
                Some(r.ours_rebased.ladder_normal),
                r.retail.lv,
                "{name} {} ct={}: vLadderVec",
                r.phase,
                r.retail.ct
            );
            assert!(
                size(r.rebased) == 0.0 && r.rebased.dv == 0.0,
                "{name} {} ct={}: rebased {:?}",
                r.phase,
                r.retail.ct,
                r.rebased
            );
        }
        ran += 1;
    }
    println!("{ran} grab captures");
}

const TAPED_WEAPONS: &str = "weapons-ladder-1.1.txt";

/// Rows as the tape's header keeps them: the replay must give them back.
fn summary(rows: &[Row]) -> Vec<String> {
    rows.iter()
        .map(|r| {
            format!(
                "row {} {} free {:?} rebased {:?}",
                r.phase, r.retail.ct, r.free, r.rebased
            )
        })
        .collect()
}

/// The 1.1 hold ladder capture replayed off its trace tape with no install
/// (docs/dev/testing.md, "Trace tapes"): every row must be the one the live
/// gate gave when the tape was recorded, and pass its check.
/// `RECORD_TAPES=1` with `COD_DIR` at the 1.1 install records it first.
#[test]
fn the_taped_ladder_climbs_without_the_paks() {
    let (name, yaw, rules) = FIXTURES[0];
    let lines = fixture(name);
    if tapes::recording() {
        let fs = vcod_common::testing::game_fs_11().expect("RECORD_TAPES needs COD_DIR at 1.1");
        let (world, weapons) = load().unwrap();
        world.set_arith(Arith::Kept);
        let tape = Tape::record();
        let rows = replay_taped(&lines, yaw, rules, &world, Some(&tape), weapons.defs());
        let mut header = vec![
            format!("{name} replayed under V1_1 on mp_ship by ladder_ab.rs"),
            "RECORD_TAPES=1 rewrites it (docs/dev/testing.md, \"Trace tapes\")".to_owned(),
        ];
        header.extend(summary(&rows));
        std::fs::write(tapes::tape_path(name), tape.text(&header)).unwrap();
        let carried = lines
            .iter()
            .filter_map(|l| match l {
                Line::Cmd(c) => Some(usize::from(c.weapon)),
                Line::Snap(s) => Some(usize::from(s.weapon)),
                Line::Phase(_) => None,
            })
            .collect();
        tapes::write_weapons(&fs, vcod_server::defs::V1.weapons, &carried, TAPED_WEAPONS);
        println!("{name}: {} queries", tape.len());
    }
    let weapons = tapes::read_weapons(TAPED_WEAPONS);
    let world = vcod_common::collision::test_world(&[]);
    world.set_arith(Arith::Kept);
    let text = std::fs::read_to_string(tapes::tape_path(name)).unwrap();
    let tape = Tape::replay(&text);
    let rows = replay_taped(&lines, yaw, rules, &world, Some(&tape), &weapons);
    assert_eq!(tape.remaining(), 0, "{name}: queries left on the tape");
    assert_eq!(
        summary(&rows),
        tapes::recorded(&text, &["row "]),
        "{name}: not the recorded rows"
    );
    check(name, &rows);
}
