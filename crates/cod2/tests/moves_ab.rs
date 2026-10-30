//! The CoD2 movement A/B: each retail capture in `tests/fixtures/moves/`
//! (made by `examples/move_probe.rs` through `tools/run_probe_cod2.sh`, the
//! recipe in each file's header) replayed through `vcod_cod2::pmove`, cmd
//! for cmd, on the same map, and every snapshot's pmove fields compared with
//! the wire's words bit for bit (an angle as the short the wire carries).
//!
//! Two replays per capture. The free run starts from the first snapshot
//! after the spawn and never looks back at retail: its first divergence is
//! the report. The resynced run takes each snapshot's state as the next
//! row's start (keeping only `oldVelocity`, which is not on the wire), so
//! each row tests one snapshot's worth of cmds; a field it finds wrong must
//! be named in [`GAPS`] for that script. Needs `COD2_DIR`.
//! Beside the move each cmd runs the weapon half as vcod-server does
//! (`update_sight_flag`, `adjust_aim_spread`, then `pm_weapon` after the
//! move), on the weapon table the capture's `# weapons` line names
//! (`probe_moves.gsc`'s kar98k at retail's index 3 without one), with
//! `ammo[]` and `ammoclip[]` compared too.
//! `MOVES_REPORT=1` prints every mismatching row; `MOVES_TRACE=walk:resync:120`
//! prints that row's cmds and the state ours reached.

use std::collections::BTreeMap;

use vcod_cod2::net::delta::field_index;
use vcod_cod2::net::fields::PLAYER_STATE_FIELDS;
use vcod_cod2::net::{PlayerState2, UserCmd2};
use vcod_cod2::pmove::weapon::{self, WeaponParams, WeaponState};
use vcod_cod2::pmove::{self, PlayerState, Rules};
use vcod_common::collision::CollisionWorld;
use vcod_common::movetrace::MoveWorld;
use vcod_common::patch::Arith;
use vcod_common::testing;

/// The captures, each `<name>.txt` beside `probe_moves.gsc`.
pub(crate) const CAPTURES: &[&str] = &[
    "walk",
    "jump",
    "stance",
    "reverse",
    "lean",
    "wall",
    "slope",
    "steps",
    "ledge",
    "mantle",
    "mantle_turn",
    "mantle_crouch",
    "ladder",
    // `moveSpeedScale` 0.9 and 1.1 (probe_weapon bar_mp, colt_mp).
    "walk_bar",
    "stance_colt",
];

/// The weapon captures, each `<name>.txt` beside
/// `fixtures/combat/probe_combat.gsc`: a bolt rifle, an smg, the BAR's
/// bursts (the recoil's: cod2-weapons.md, "The recoil"), a pistol, the
/// primary slots' switches and the frag key, each with its weapon table in
/// the `# weapons` line.
const WEAPON_CAPTURES: &[&str] = &["rifle", "smg", "bar", "pistol", "switch", "frag", "breath"];

/// What the resynced replay still gets wrong: (capture, field, rows, why).
/// The rows are exact: a fix that rights one fails the gate until the
/// entry shrinks. Each reason names its section of
/// docs/research/cod2-movement.md, or of cod2-combat.md where it says so.
const GAPS: &[(&str, &str, usize, &str)] = &[
    // `EV_RESET_ADS` (149) off the weapon code as the body rises from prone.
    ("stance", "eventSequence", 1, "17.3 weapon events"),
    ("stance", "events[1]", 1, "17.3 weapon events"),
    ("stance_colt", "eventSequence", 1, "17.3 weapon events"),
    ("stance_colt", "events[1]", 1, "17.3 weapon events"),
    // The thrower's own frag lands beside him: its blast's knockback is
    // `G_Damage`'s, server-side (cod2-combat.md 9.1).
    ("frag", "pm_flags", 3, "cod2-combat.md 9.1"),
    ("frag", "pm_time", 3, "cod2-combat.md 9.1"),
    ("frag", "velocity[1]", 3, "cod2-combat.md 9.1"),
    ("frag", "velocity[2]", 3, "cod2-combat.md 9.1"),
];

/// The playerstate words pmove writes.
const COMPARED: &[&str] = &[
    "commandTime",
    "pm_type",
    "pm_flags",
    "pm_time",
    "origin[0]",
    "origin[1]",
    "origin[2]",
    "velocity[0]",
    "velocity[1]",
    "velocity[2]",
    "viewangles[0]",
    "viewangles[1]",
    "viewangles[2]",
    "delta_angles[0]",
    "delta_angles[1]",
    "delta_angles[2]",
    "groundEntityNum",
    "vLadderVec[0]",
    "vLadderVec[1]",
    "vLadderVec[2]",
    "jumpTime",
    "jumpOriginZ",
    "movementDir",
    "eFlags",
    "leanf",
    "viewHeightTarget",
    "viewHeightCurrent",
    "viewHeightLerpTime",
    "viewHeightLerpTarget",
    "viewHeightLerpDown",
    "mins[0]",
    "mins[1]",
    "mins[2]",
    "maxs[0]",
    "maxs[1]",
    "maxs[2]",
    "proneDirection",
    "proneDirectionPitch",
    "proneTorsoPitch",
    "fTorsoPitch",
    "fWaistPitch",
    "bobCycle",
    "eventSequence",
    "events[0]",
    "events[1]",
    "events[2]",
    "events[3]",
    "eventParms[0]",
    "eventParms[1]",
    "eventParms[2]",
    "eventParms[3]",
    "weapon",
    "weaponstate",
    "weaponTime",
    "weaponDelay",
    "weapAnim",
    "fWeaponPosFrac",
    "holdBreathScale",
    "holdBreathTimer",
    "offHandIndex",
    "weapons[0]",
    "weaponrechamber[0]",
    "grenadeTimeLeft",
    "mantleState.yaw",
    "mantleState.timer",
    "mantleState.transIndex",
    "mantleState.flags",
];

/// The array blocks compared beside [`COMPARED`], by name.
const COMPARED_ARRAYS: &[&str] = &["ammo", "ammoclip"];

pub(crate) struct Snap {
    pub(crate) server_time: i32,
    pub(crate) ps: PlayerState2,
}

pub(crate) struct Capture {
    pub(crate) map: String,
    /// `CS_WEAPONS`, when the capture logged it.
    pub(crate) weapons: Option<Vec<String>>,
    /// The weapon `probe_moves.gsc` handed out: the header's
    /// `probe_weapon`, else the kar98k.
    pub(crate) weapon: String,
    pub(crate) cmds: Vec<UserCmd2>,
    pub(crate) snaps: Vec<Snap>,
}

pub(crate) fn parse(text: &str) -> Capture {
    let mut map = None;
    let mut weapons = None;
    let mut weapon = "kar98k_mp".to_string();
    let mut cmds = Vec::new();
    let mut snaps = Vec::new();
    let mut ps = PlayerState2::default();
    for line in text.lines() {
        let mut w = line.split_whitespace();
        match w.next() {
            Some("#") if line.contains("probe_weapon ") => {
                let (_, rest) = line.split_once("probe_weapon ").unwrap();
                weapon = rest
                    .split(|c: char| c == ',' || c == ')' || c.is_whitespace())
                    .next()
                    .unwrap()
                    .to_string();
            }
            Some("#") => match w.next() {
                Some("map") => map = w.next().map(str::to_string),
                Some("weapons") => weapons = Some(w.map(str::to_string).collect()),
                _ => {}
            },
            Some("c") => {
                // Decimal but for the buttons, which are hex.
                let n: Vec<i64> = w
                    .enumerate()
                    .map(|(k, t)| {
                        if k == 1 {
                            i64::from_str_radix(t, 16).unwrap()
                        } else {
                            t.parse().unwrap()
                        }
                    })
                    .collect();
                cmds.push(UserCmd2 {
                    server_time: n[0] as i32,
                    buttons: n[1] as i32,
                    weapon: n[2] as u8,
                    off_hand_index: n[3] as u8,
                    angles: [n[4] as i32, n[5] as i32, n[6] as i32],
                    forward_move: n[7] as i8,
                    right_move: n[8] as i8,
                });
            }
            Some("s") => {
                let _msg = w.next();
                let server_time = w.next().unwrap().parse().unwrap();
                for kv in w {
                    let (k, v) = kv.split_once('=').unwrap();
                    let v = u32::from_str_radix(v, 16).unwrap();
                    // `S[i]`, `A[i]` and `C[i]`: the stats, reserve and
                    // clip arrays.
                    let slot = |k: &str| k[2..k.len() - 1].parse::<usize>().unwrap();
                    match k.as_bytes()[0] {
                        b'S' if k.starts_with("S[") => ps.stats[slot(k)] = v as i32,
                        b'A' if k.starts_with("A[") => ps.ammo[slot(k)] = v as i32,
                        b'C' if k.starts_with("C[") => ps.ammoclip[slot(k)] = v as i32,
                        _ => ps.f[field_index(&PLAYER_STATE_FIELDS, k)] = v,
                    }
                }
                snaps.push(Snap {
                    server_time,
                    ps: ps.clone(),
                });
            }
            _ => {}
        }
    }
    Capture {
        map: map.expect("# map line"),
        weapons,
        weapon,
        cmds,
        snaps,
    }
}

/// The level's weapon table off `CS_WEAPONS` (1-based), each weapon's ammo
/// and clip index by `vcod_cod2::weapon::clip_indices` (an empty name
/// shares the default def's 0, cod2-combat.md 9.1).
/// `probe_moves.gsc`'s captures carry no table: their one weapon,
/// `probe` (the kar98k, or the bar or colt of `walk_bar` and
/// `stance_colt`), is weapon 3 (`weapon=3`, `weapons[0]=8`), its ammo and
/// clip index 2 (the `fire` capture's `A[2]`, `C[2]`).
pub(crate) fn weapons(
    fs: &vcod_common::pk3::Pk3Fs,
    names: Option<&[String]>,
    probe: &str,
) -> Vec<Option<WeaponParams>> {
    let Some(names) = names else {
        let def = vcod_cod2::weapon::load(fs, probe).unwrap().def;
        return vec![
            None,
            None,
            None,
            Some(WeaponParams::from_def(&def, 2, 2, 0)),
        ];
    };
    let names: Vec<&str> = names.iter().map(String::as_str).collect();
    let table = weapon::table(fs, &names);
    assert!(table[1..].iter().all(Option::is_some), "{names:?} load");
    table
}

/// `PM_Weapon`'s half of the playerstate off the wire.
pub(crate) fn weapon_from_wire(w: &PlayerState2) -> WeaponState {
    WeaponState::from_wire(w)
}

/// The 16-bit angle a wire float stands for.
fn angle_short(bits: u32) -> i16 {
    (f64::from(f32::from_bits(bits)) * f64::from(65536.0f32 / 360.0)) as i64 as i16
}

/// The word as the field's width carries it.
fn on_wire(name: &str, v: u32) -> u32 {
    let bits = PLAYER_STATE_FIELDS[field_index(&PLAYER_STATE_FIELDS, name)].bits;
    match bits {
        -100 => angle_short(v) as u16 as u32,
        // A float goes integral or raw with no zero flag (`read_ps_float`),
        // so -0 reads back as +0 unless the base was +0: the sign of a zero
        // is not something the wire reliably carries.
        0 if v == 0x8000_0000 => 0,
        0 | 32 => v,
        b if b > 0 => v & ((1u32 << b) - 1),
        b => {
            let n = -b;
            (((v << (32 - n)) as i32) >> (32 - n)) as u32
        }
    }
}

/// The compared fields that differ: (name, ours, retail). An array block
/// that differs counts once, under its name, with its first wrong word.
fn diff(ours: &PlayerState2, retail: &PlayerState2) -> Vec<(&'static str, u32, u32)> {
    let mut d: Vec<_> = COMPARED
        .iter()
        .filter_map(|&n| {
            let i = field_index(&PLAYER_STATE_FIELDS, n);
            let (a, b) = (on_wire(n, ours.f[i]), on_wire(n, retail.f[i]));
            (a != b).then_some((n, a, b))
        })
        .collect();
    for &n in COMPARED_ARRAYS {
        let (a, b) = match n {
            "ammo" => (&ours.ammo, &retail.ammo),
            _ => (&ours.ammoclip, &retail.ammoclip),
        };
        if let Some(k) = (0..64).find(|&k| a[k] != b[k]) {
            d.push((n, a[k] as u32, b[k] as u32));
        }
    }
    d
}

fn show(name: &str, v: u32) -> String {
    if COMPARED_ARRAYS.contains(&name) {
        return format!("{}", v as i32);
    }
    let bits = PLAYER_STATE_FIELDS[field_index(&PLAYER_STATE_FIELDS, name)].bits;
    if bits == 0 {
        format!("{} ({v:08x})", f32::from_bits(v))
    } else {
        format!("{} ({v:x})", v as i32)
    }
}

#[derive(Default)]
struct Outcome {
    rows: usize,
    free_matching: usize,
    free_first: Option<String>,
    resync_matching: usize,
    /// field -> rows it was wrong in, with the first.
    resync_fields: BTreeMap<&'static str, (usize, String)>,
}

/// `trace_row`: (resynced, row) whose cmds to print.
#[allow(clippy::too_many_arguments)]
fn replay(
    cap: &Capture,
    world: &CollisionWorld,
    defs: &Vec<Option<WeaponParams>>,
    rules: Rules,
    resync: bool,
    out: &mut Outcome,
    report: bool,
    trace_row: Option<(bool, usize)>,
) {
    let cmd_at = |t: i32| cap.cmds.iter().position(|c| c.server_time == t);
    // The first snapshot whose commandTime is a logged cmd with one before it.
    let (start, mut ci) = cap
        .snaps
        .iter()
        .enumerate()
        .find_map(|(k, s)| {
            let i = cmd_at(s.ps.f[0] as i32)?;
            (i >= 1).then_some((k, i))
        })
        .expect("no snapshot reached a logged cmd");
    let mut ps = PlayerState::from_wire(&cap.snaps[start].ps);
    let mut ws = weapon_from_wire(&cap.snaps[start].ps);
    let mut old = cap.cmds[ci];
    let mut free_ok = true;
    for (k, snap) in cap.snaps.iter().enumerate().skip(start + 1) {
        let ct = snap.ps.f[0] as i32;
        let Some(j) = cmd_at(ct) else {
            panic!(
                "snapshot {} commandTime {ct}: no such cmd",
                snap.server_time
            );
        };
        let trace = trace_row == Some((resync, k));
        while ci < j {
            ci += 1;
            if trace {
                eprintln!(
                    "  cmd {:?}\n    from origin {:?} velocity {:?} ground {}",
                    cap.cmds[ci], ps.origin, ps.velocity, ps.ground_entity_num
                );
            }
            let cmd = cap.cmds[ci];
            weapon::armed_step(
                &mut ps,
                &mut ws,
                &cmd,
                &mut old,
                MoveWorld::bare(world),
                rules,
                defs,
            );
        }
        if trace {
            eprintln!("  ours origin {:?} velocity {:?}", ps.origin, ps.velocity);
        }
        let mut ours = ps.to_wire(&snap.ps);
        ws.write_wire(&mut ours);
        let d = diff(&ours, &snap.ps);
        let row = format!("row {k} (serverTime {}, cmd {ci})", snap.server_time);
        if !resync {
            out.rows += 1;
            if free_ok && d.is_empty() {
                out.free_matching += 1;
            } else if free_ok {
                free_ok = false;
                let (n, a, b) = d[0];
                out.free_first = Some(format!(
                    "{row}: {n} ours {} retail {}{}",
                    show(n, a),
                    show(n, b),
                    if d.len() > 1 {
                        format!(" (+{} more fields)", d.len() - 1)
                    } else {
                        String::new()
                    }
                ));
            }
            continue;
        }
        if d.is_empty() {
            out.resync_matching += 1;
        } else if report {
            let fields: Vec<String> = d
                .iter()
                .map(|&(n, a, b)| format!("{n} {} vs {}", show(n, a), show(n, b)))
                .collect();
            eprintln!("  {row}: {}", fields.join(", "));
        }
        for &(n, a, b) in &d {
            let e = out.resync_fields.entry(n).or_insert_with(|| {
                (
                    0,
                    format!("{row}: ours {} retail {}", show(n, a), show(n, b)),
                )
            });
            e.0 += 1;
        }
        let keep = ps.old_velocity;
        ps = PlayerState::from_wire(&snap.ps);
        ps.old_velocity = keep;
        // What the wire does not carry of the weapon half stays ours.
        ws = WeaponState {
            restrict_kick_time: ws.restrict_kick_time,
            ads_delay_time: ws.ads_delay_time,
            ..weapon_from_wire(&snap.ps)
        };
    }
}

#[test]
fn retail_moves_replay_on_vcod_pmove() {
    let Some(fs) = testing::cod2_fs() else {
        return;
    };
    let report = std::env::var_os("MOVES_REPORT").is_some();
    // MOVES_TRACE=<capture>:<free|resync>:<row> prints that row's cmds.
    let trace = std::env::var("MOVES_TRACE").ok();
    let trace_row = |name: &str, resync: bool| {
        let t = trace.as_deref()?;
        let mut p = t.split(':');
        let (c, mode, row) = (p.next()?, p.next()?, p.next()?.parse().ok()?);
        (c == name && (mode == "resync") == resync).then_some((resync, row))
    };
    let root = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures");
    let mut worlds: BTreeMap<String, CollisionWorld> = BTreeMap::new();
    let rules = Rules {
        mantle_anims: pmove::mantle::anims(&fs),
        ..Rules::default()
    };
    assert!(rules.mantle_anims.is_some(), "the mantle anims load");
    let mut unexplained = Vec::new();
    eprintln!("capture   rows  free-run  resynced  first free-run divergence");
    let all = CAPTURES
        .iter()
        .map(|n| ("moves", n))
        .chain(WEAPON_CAPTURES.iter().map(|n| ("combat", n)));
    for (dir, name) in all {
        let text = std::fs::read_to_string(format!("{root}/{dir}/{name}.txt")).unwrap();
        let cap = parse(&text);
        let defs = weapons(&fs, cap.weapons.as_deref(), &cap.weapon);
        let world = worlds.entry(cap.map.clone()).or_insert_with(|| {
            let data = fs.read(&format!("maps/mp/{}.d3dbsp", cap.map)).unwrap();
            let b = vcod_cod2::d3dbsp::D3dBsp::parse(&data).unwrap();
            let w = vcod_cod2::world::from_bsp(&b);
            let cw = vcod_cod2::map::collision_world(&b, &w, &[]).unwrap();
            cw.set_arith(Arith::Stored);
            cw
        });
        let mut out = Outcome::default();
        replay(
            &cap,
            world,
            &defs,
            rules,
            false,
            &mut out,
            false,
            trace_row(name, false),
        );
        if report {
            eprintln!("{name}:");
        }
        replay(
            &cap,
            world,
            &defs,
            rules,
            true,
            &mut out,
            report,
            trace_row(name, true),
        );
        eprintln!(
            "{name:<9} {:>4}  {:>8}  {:>8}  {}",
            out.rows,
            out.free_matching,
            out.resync_matching,
            out.free_first.as_deref().unwrap_or("-")
        );
        for (field, (n, first)) in &out.resync_fields {
            let gap = GAPS.iter().find(|&&(c, f, _, _)| c == *name && f == *field);
            eprintln!(
                "    {field}: {n} rows{}, first {first}",
                gap.map_or(String::new(), |g| format!(" (gap: {})", g.3))
            );
            match gap {
                Some(&(_, _, rows, _)) if rows == *n => {}
                Some(&(_, _, rows, _)) => {
                    unexplained.push(format!("{name} {field}: {n} rows, GAPS says {rows}"))
                }
                None => unexplained.push(format!("{name} {field}: {n} rows, first {first}")),
            }
        }
        for &(c, f, rows, _) in GAPS {
            if c == *name && !out.resync_fields.contains_key(f) {
                unexplained.push(format!("{c} {f}: right in every row, GAPS says {rows}"));
            }
        }
    }
    assert!(
        unexplained.is_empty(),
        "fields wrong outside GAPS:\n{}",
        unexplained.join("\n")
    );
}
