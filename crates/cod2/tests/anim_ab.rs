//! The CoD2 anim A/B: every movement capture in `tests/fixtures/moves/`
//! (moves_ab.rs's, a kar98k on retail `cod2_lnxded`) replayed through
//! `vcod_cod2::pmove` and its `PM_Weapon` with the anim script on (the
//! weapon's torso events too), resynced each snapshot as
//! moves_ab's second run is (the `MOVETYPE` and `STRAFING` conditions,
//! which are not on the wire, carry over), and the playerstate's
//! `legsAnim`, `torsoAnim`, `legsTimer` and `torsoTimer` compared with the
//! wire's. A field ours gets wrong must be named in [`GAPS`] with its row
//! count. A capture made under `RAND_TRACE=1` carries retail's `rand()`
//! draws, and its clause picks take `BG_AnimScriptEvent`'s in order (each
//! one consumed: the counts must agree); the others pick the first
//! command. Needs `COD2_DIR`. `ANIM_REPORT=1` prints every mismatching row.

use std::cell::Cell;

use std::collections::BTreeMap;

use vcod_cod2::net::delta::field_index;
use vcod_cod2::net::fields::PLAYER_STATE_FIELDS;
use vcod_cod2::pmove::weapon::{self, WeaponAnims, WeaponCmd};
use vcod_cod2::pmove::{self, AnimTable, PlayerState, Rules};
use vcod_common::collision::CollisionWorld;
use vcod_common::movetrace::MoveWorld;
use vcod_common::patch::Arith;
use vcod_common::testing;

use crate::moves_ab::{CAPTURES, parse, weapon_from_wire, weapons};

/// The captures only this gate replays: `move_probe`'s `fire` script, and
/// `hurt` under `probe_moves.gsc`'s `probe_hurt` (recipes in their headers).
const ANIM_CAPTURES: &[&str] = &["fire", "hurt"];

const COMPARED: &[&str] = &["legsAnim", "torsoAnim", "legsTimer", "torsoTimer"];

/// (capture, field, rows, why), each why a section of
/// docs/research/cod2-player-anims.md.
const GAPS: &[(&str, &str, usize, &str)] = &[];

/// `BG_AnimScriptEvent`'s `rand() % numCommands` (lnxded 0x80d977a, in
/// 0x80d96ee), the trace's site column.
const PICK_SITE: &str = "80d977a";

/// The trace's draws at [`PICK_SITE`] from `level.time` `from` on: the
/// frame that sent the replay's first snapshot ran its cmds' picks after it.
fn traced_picks(text: &str, from: i32) -> Vec<u32> {
    text.lines()
        .filter_map(|l| {
            let w: Vec<&str> = l.strip_prefix("# rand ")?.split(' ').collect();
            (w[1] == PICK_SITE && w[0].parse::<i32>().ok()? >= from).then(|| w[2].parse().unwrap())
        })
        .collect()
}

#[test]
fn retail_anims_replay_on_vcod_pmove() {
    let Some(fs) = testing::cod2_fs() else {
        return;
    };
    let report = std::env::var_os("ANIM_REPORT").is_some();
    let table = AnimTable::load(&fs).unwrap();
    let weapon = field_index(&PLAYER_STATE_FIELDS, "weapon");
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/moves");
    let mut worlds: BTreeMap<String, CollisionWorld> = BTreeMap::new();
    let mut unexplained = Vec::new();
    let rules = Rules {
        mantle_anims: pmove::mantle::anims(&fs),
        ..Rules::default()
    };
    for name in CAPTURES.iter().chain(ANIM_CAPTURES) {
        let text = std::fs::read_to_string(format!("{dir}/{name}.txt")).unwrap();
        let cap = parse(&text);
        // The probe's weapon at the index the map's registration gave it (3, or 4
        // on mp_farmhouse).
        let index = cap
            .snaps
            .iter()
            .map(|s| s.ps.f[weapon] as usize)
            .max()
            .unwrap_or(0);
        let mut defs = vec![None; index.max(1) + 1];
        if index != 0 {
            defs[index] = weapons(&fs, None, &cap.weapon).pop().flatten();
        }
        let world = worlds.entry(cap.map.clone()).or_insert_with(|| {
            let data = fs.read(&format!("maps/mp/{}.d3dbsp", cap.map)).unwrap();
            let b = vcod_cod2::d3dbsp::D3dBsp::parse(&data).unwrap();
            let w = vcod_cod2::world::from_bsp(&b);
            let cw = vcod_cod2::map::collision_world(&b, &w, &[]).unwrap();
            cw.set_arith(Arith::Stored);
            cw
        });
        let cmd_at = |t: i32| cap.cmds.iter().position(|c| c.server_time == t);
        let (start, mut ci) = cap
            .snaps
            .iter()
            .enumerate()
            .find_map(|(k, s)| Some((k, cmd_at(s.ps.f[0] as i32).filter(|&i| i >= 1)?)))
            .expect("no snapshot reached a logged cmd");
        let mut ps = PlayerState::from_wire(&cap.snaps[start].ps);
        let mut ws = weapon_from_wire(&cap.snaps[start].ps);
        let mut old = cap.cmds[ci];
        let traced = text.contains("\n# rand ");
        let draws = traced_picks(&text, cap.snaps[start].server_time);
        let drawn = Cell::new(0);
        let pick = |n: usize| {
            if !traced {
                return 0;
            }
            let k = drawn.get();
            drawn.set(k + 1);
            draws.get(k).map_or(0, |&v| v as usize % n)
        };
        let mut wrong: BTreeMap<&str, (usize, String)> = BTreeMap::new();
        let mut rows = 0;
        for (k, snap) in cap.snaps.iter().enumerate().skip(start + 1) {
            let j = cmd_at(snap.ps.f[0] as i32).expect("snapshot at a logged cmd");
            while ci < j {
                ci += 1;
                let cmd = cap.cmds[ci];
                let oldcmd = old;
                let msec = (cmd.server_time - ps.command_time).clamp(0, 200);
                if msec > 0 {
                    weapon::update_sight_flag(&mut ps, &ws, &cmd, &oldcmd, &defs);
                }
                let prone = ps.view_height_target == pmove::VIEW_PRONE;
                let input = pmove::AnimInput {
                    pick: Some(&pick),
                    ..weapon::anim_input(&table, &ps, &ws, &defs)
                };
                let scale = weapon::move_scale(&defs, ws.weapon);
                pmove::player_step_anim(
                    &mut ps,
                    &cmd,
                    &mut old,
                    MoveWorld::bare(world),
                    rules,
                    scale,
                    Some(input),
                );
                if msec > 0 {
                    let wc = WeaponCmd {
                        cmd: &cmd,
                        oldcmd: &oldcmd,
                        msec,
                        ads_exit_delay: 0,
                        prone_change: prone != (ps.view_height_target == pmove::VIEW_PRONE),
                        anim: Some(WeaponAnims {
                            table: &table,
                            pick: &pick,
                        }),
                    };
                    weapon::pm_weapon(&mut ps, &mut ws, &wc, &defs);
                }
            }
            rows += 1;
            let ours = ps.to_wire(&snap.ps);
            for &n in COMPARED {
                let i = field_index(&PLAYER_STATE_FIELDS, n);
                let bits = PLAYER_STATE_FIELDS[i].bits as u32;
                let mask = (1u32 << bits) - 1;
                let (a, b) = (ours.f[i] & mask, snap.ps.f[i] & mask);
                if a != b {
                    let row = format!(
                        "row {k} (serverTime {}): ours {a} retail {b}",
                        snap.server_time
                    );
                    if report {
                        let named = |v: u32| {
                            if n.ends_with("Anim") {
                                let a = table.anims.name((v & 0x1ff) as i32).unwrap_or("?");
                                format!(" ({a})")
                            } else {
                                String::new()
                            }
                        };
                        eprintln!("  {name} {n} {row}{} vs{}", named(a), named(b));
                    }
                    wrong.entry(n).or_insert((0, row)).0 += 1;
                }
            }
            let keep = (ps.old_velocity, ps.anim_movetype, ps.anim_strafing);
            ps = PlayerState::from_wire(&snap.ps);
            (ps.old_velocity, ps.anim_movetype, ps.anim_strafing) = keep;
            ws = weapon_from_wire(&snap.ps);
        }
        eprintln!("{name:<9} {rows:>4} rows, {} fields wrong", wrong.len());
        if traced && drawn.get() != draws.len() {
            unexplained.push(format!(
                "{name}: {} clause picks, retail drew {}",
                drawn.get(),
                draws.len()
            ));
        }
        for (field, (n, first)) in &wrong {
            eprintln!("    {field}: {n} rows, first {first}");
            match GAPS.iter().find(|g| g.0 == *name && g.1 == *field) {
                Some(g) if g.2 == *n => {}
                Some(g) => unexplained.push(format!("{name} {field}: {n} rows, GAPS says {}", g.2)),
                None => unexplained.push(format!("{name} {field}: {n} rows, first {first}")),
            }
        }
        for g in GAPS
            .iter()
            .filter(|g| g.0 == *name && !wrong.contains_key(g.1))
        {
            unexplained.push(format!(
                "{} {}: right in every row, GAPS says {}",
                g.0, g.1, g.2
            ));
        }
    }
    assert!(
        unexplained.is_empty(),
        "anim fields wrong outside GAPS:\n{}",
        unexplained.join("\n")
    );
}
