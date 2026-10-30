//! Retail's mount of mp_carentan's mg42 (`vcod-cod2`'s
//! `turret/carentan-mg42.txt`, docs/research/cod2-turrets.md 8) replayed
//! through `Cod2Level`: the use tap mounts, the view clamps to the arcs,
//! the barrel follows it, the held trigger and the tap fire a round a
//! frame with the loop and the stop sound, the use tap lets go, and the
//! barrel walks home. Each snapshot's cmds run before its frame, as the
//! snapshot's `commandTime` says retail's did; with a second client
//! (`<name>-b.txt`, slot 0) both clients' packets run in the order they
//! were sent and both playerstates are compared. Needs `COD2_DIR`; without
//! it the test passes. `TURRET_REPORT=1` prints every snapshot's diff.

use std::rc::Rc;

use glam::Vec3;
use vcod_cod2::net::delta::field_index;
use vcod_cod2::net::fields::PLAYER_STATE_FIELDS;
use vcod_cod2::net::{EntityState2, PlayerState2, UserCmd2};
use vcod_cod2::pmove::PlayerState;
use vcod_gsc::EntId;

use crate::game::cod2::Cod2Level;
use crate::game::cod2_combat_ab::{Capture, Side, Snap, parse, weapon_from_wire};
use crate::game::script::MenuRef;

/// One retail capture beside `probe_turret.gsc` (or the stock `dm` for
/// the first): the gun it mans, retail's number and where it stands. The
/// probe gametype's cvars come off the capture's header.
struct Fixture {
    name: &'static str,
    gun: u32,
    gun_at: Vec3,
    /// The stock `dm` and its menus, not the probe gametype.
    dm: bool,
    /// A second client's `<name>-b.txt` beside it, joined first (slot 0;
    /// the capture's own client is slot 1).
    second: bool,
}

const FIXTURES: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../cod2/tests/fixtures/turret");

const MG42: Fixture = Fixture {
    name: "carentan-mg42",
    gun: 295,
    gun_at: Vec3::new(1718.0, 1824.0, 5.0),
    dm: true,
    second: false,
};

/// A `spawnTurret` prone and duck gun on mp_toujane's open ground
/// (cod2-turrets.md 13).
const PRONE: Fixture = Fixture {
    name: "toujane-prone",
    gun: 619,
    gun_at: Vec3::new(-299.0, 1080.0, 28.125),
    dm: false,
    second: false,
};
const DUCK: Fixture = Fixture {
    name: "toujane-duck",
    ..PRONE
};
/// The prone 30cal there (cod2-turrets.md 13).
const CAL30: Fixture = Fixture {
    name: "toujane-30cal",
    ..PRONE
};

/// mp_carentan's mg42 with its gunner killed on it (cod2-turrets.md 14).
const DIE: Fixture = Fixture {
    name: "carentan-die",
    dm: false,
    ..MG42
};
/// Its gunner shot dead off it while he fires (cod2-turrets.md 14.1).
const SHOT: Fixture = Fixture {
    name: "carentan-shot",
    second: true,
    ..DIE
};
/// Two players at it: the second's use refused while the first mans it,
/// then taken once he lets go (cod2-turrets.md 14.2).
const SHARE: Fixture = Fixture {
    name: "carentan-share",
    second: true,
    ..DIE
};

/// One client's capture with the gun's entity at each snapshot.
struct Run {
    slot: usize,
    side: Side,
    guns: Vec<EntityState2>,
}

impl Run {
    fn at(&self, time: i32) -> Option<(&Snap, &EntityState2)> {
        let k = self.side.snaps.iter().position(|s| s.time == time)?;
        Some((&self.side.snaps[k], &self.guns[k]))
    }
}

fn run_of(fx: &Fixture, slot: usize, cap: Capture) -> Run {
    let mut gun = EntityState2::null(fx.gun);
    let guns = cap
        .side
        .snaps
        .iter()
        .map(|s| {
            if let Some(e) = s.ents.get(&fx.gun) {
                gun.f = *e;
            }
            gun
        })
        .collect();
    Run {
        slot,
        side: cap.side,
        guns,
    }
}

/// The level with every client spawned, each body as retail's stood at
/// the main capture's first snapshot, the clock there: (level, the gun,
/// the runs, each run's cmds already done).
fn spawned(fx: &Fixture) -> Option<(Cod2Level, EntId, Vec<Run>, Vec<i32>)> {
    let fs = Rc::new(vcod_common::testing::cod2_fs()?);
    let read = |n: &str| std::fs::read_to_string(format!("{FIXTURES}/{n}.txt")).unwrap();
    let main = parse(&read(fx.name));
    let map = main.map.clone();
    let cvar_sets = main.cvars.clone();
    let mut runs = Vec::new();
    if fx.second {
        runs.push(run_of(fx, 0, parse(&read(&format!("{}-b", fx.name)))));
    }
    runs.push(run_of(fx, runs.len(), main));
    let mut level = if fx.dm {
        Cod2Level::load(fs, &map, "dm", crate::cvars::Cvars::new(), 0).unwrap()
    } else {
        let gsc = std::fs::read_to_string(format!("{FIXTURES}/probe_turret.gsc")).unwrap();
        let mut cvars = crate::cvars::Cvars::new();
        cvars.set("developer", "1");
        for (k, v) in &cvar_sets {
            cvars.set(k, v);
        }
        Cod2Level::load_with_script(
            fs,
            &map,
            "probe_turret",
            ("maps/mp/gametypes/probe_turret".to_string(), gsc),
            cvars,
            0,
        )
        .unwrap()
    };
    let slots: Vec<usize> = runs.iter().map(|r| r.slot).collect();
    let mut now = 0;
    let idle = |level: &mut Cod2Level, now: &mut i32| {
        *now += 50;
        let cmd = UserCmd2 {
            server_time: *now,
            ..Default::default()
        };
        for &slot in &slots {
            level.client_think(slot, &cmd);
        }
        level.run_frame(*now);
    };
    for &slot in &slots {
        level.connect(slot, "gunner");
    }
    idle(&mut level, &mut now);
    for &slot in &slots {
        level.begin(slot);
    }
    idle(&mut level, &mut now);
    if fx.dm {
        // mp_carentan's allies are American.
        level.menu_response(
            0,
            &MenuRef::Name("team_americangerman".into()),
            "autoassign",
        );
        for _ in 0..4 {
            idle(&mut level, &mut now);
        }
        let session = level.rt.client_field(0, "sessionteam").unwrap_or_default();
        let (menu, weapon) = if session == "axis" {
            ("weapon_german", "kar98k_mp")
        } else {
            ("weapon_american", "m1garand_mp")
        };
        level.menu_response(0, &MenuRef::Name(menu.into()), weapon);
    }
    let first = runs.last().unwrap().side.snaps[0].time;
    while now + 50 < first {
        idle(&mut level, &mut now);
    }
    let guns: Vec<EntId> = level.rt.host.turrets.keys().copied().collect();
    let gun = guns
        .into_iter()
        .min_by_key(|id| {
            let o = Vec3::from(level.rt.entity_origin_of(*id).unwrap());
            (o.distance(fx.gun_at) * 16.0) as i32
        })
        .unwrap();
    let mut done = Vec::new();
    for run in &runs {
        assert_eq!(
            level.rt.client_field(run.slot, "sessionstate").as_deref(),
            Some("playing")
        );
        let snap = run
            .side
            .snaps
            .iter()
            .rev()
            .find(|s| s.time <= first)
            .unwrap();
        let ct = snap.ps.f[0] as i32;
        let cmds = &run.side.cmds;
        // The first snapshot comes before the first logged cmd: the cmd
        // before it held the same view.
        let ci = cmds.iter().take_while(|c| c.server_time <= ct).count();
        let p = level.players[run.slot].as_mut().unwrap();
        p.ps = PlayerState::from_wire(&snap.ps);
        if fx.second {
            weapon_from_wire(p, &snap.ps);
        }
        p.oldcmd = UserCmd2 {
            server_time: ct,
            buttons: 0,
            ..cmds[ci.min(cmds.len() - 1)]
        };
        done.push(ct);
    }
    level.run_frame(first);
    Some((level, gun, runs, done))
}

fn f(ps: &PlayerState2, n: &str) -> f32 {
    f32::from_bits(ps.f[field_index(&PLAYER_STATE_FIELDS, n)])
}
fn i(ps: &PlayerState2, n: &str) -> i32 {
    ps.f[field_index(&PLAYER_STATE_FIELDS, n)] as i32
}

/// `a - b` folded into (-180, 180].
fn adiff(a: f32, b: f32) -> f32 {
    let d = (a - b).rem_euclid(360.0);
    if d > 180.0 { d - 360.0 } else { d }
}

/// What a replay measured against the capture.
#[derive(Default, Debug)]
struct Tally {
    mounted: usize,
    fired: usize,
    worst_view: f32,
    worst_origin: f32,
    worst_barrel: f32,
    mismatches: Vec<String>,
}

/// The playerstate checks of one client against its capture, into `bad`;
/// `gun` is ours's number and `retail_gun` retail's.
fn check_player(
    t: &mut Tally,
    bad: &mut Vec<String>,
    ours: &PlayerState2,
    retail: &PlayerState2,
    gun: u32,
    retail_gun: u32,
) {
    let mut check = |what: &str, ok: bool, detail: String| {
        if !ok {
            bad.push(format!("{what}: {detail}"));
        }
    };
    let mounted = i(retail, "eFlags") & 0x300;
    for bit in [0x300, 0x40] {
        check(
            "eFlags",
            i(ours, "eFlags") & bit == i(retail, "eFlags") & bit,
            format!(
                "{bit:x}: {:x} vs {:x}",
                i(ours, "eFlags"),
                i(retail, "eFlags")
            ),
        );
    }
    // The gunner's anim group (the toggle aside) while on the gun.
    if mounted != 0 {
        let legs = |ps: &PlayerState2| i(ps, "legsAnim") & !0x200;
        check(
            "legsAnim",
            legs(ours) == legs(retail),
            format!("{:x} vs {:x}", i(ours, "legsAnim"), i(retail, "legsAnim")),
        );
    }
    // The release's stance event.
    let stance_events = |ps: &PlayerState2| -> Vec<i32> {
        (0..4)
            .map(|k| i(ps, &format!("events[{k}]")))
            .filter(|e| (140..=142).contains(e))
            .collect()
    };
    check(
        "stance event",
        stance_events(ours).is_empty() == stance_events(retail).is_empty(),
        format!("{:?} vs {:?}", stance_events(ours), stance_events(retail)),
    );
    check(
        "viewlocked",
        i(ours, "viewlocked") == i(retail, "viewlocked"),
        format!("{} vs {}", i(ours, "viewlocked"), i(retail, "viewlocked")),
    );
    let locked = |ps: &PlayerState2, n| i(ps, "viewlocked_entNum") == n;
    check(
        "viewlocked_entNum",
        locked(retail, 1023) == locked(ours, 1023)
            && locked(retail, retail_gun as i32) == locked(ours, gun as i32),
        format!("{}", i(ours, "viewlocked_entNum")),
    );
    for k in 0..2 {
        let view = adiff(
            f(ours, &format!("viewangles[{k}]")),
            f(retail, &format!("viewangles[{k}]")),
        );
        t.worst_view = t.worst_view.max(view.abs());
        check("view", view.abs() < 0.01, format!("[{k}] off {view}"));
    }
    if mounted != 0 {
        t.mounted += 1;
        for n in [
            "viewAngleClampBase[0]",
            "viewAngleClampBase[1]",
            "viewAngleClampRange[0]",
            "viewAngleClampRange[1]",
        ] {
            check(
                n,
                adiff(f(ours, n), f(retail, n)).abs() < 1e-3,
                format!("{} vs {}", f(ours, n), f(retail, n)),
            );
        }
    }
    let o =
        |ps: &PlayerState2| Vec3::new(f(ps, "origin[0]"), f(ps, "origin[1]"), f(ps, "origin[2]"));
    let off = o(ours).distance(o(retail));
    t.worst_origin = t.worst_origin.max(off);
    check(
        "origin",
        off < 0.01,
        format!("{} vs {}", o(ours), o(retail)),
    );
    // The hint names the gun's weapon (`weapon + 4`) in retail's
    // registration order.
    check(
        "cursorHint",
        i(ours, "cursorHint") == i(retail, "cursorHint")
            && i(ours, "cursorHintString") == i(retail, "cursorHintString"),
        format!(
            "{}/{} vs {}/{}",
            i(ours, "cursorHint"),
            i(ours, "cursorHintString"),
            i(retail, "cursorHint"),
            i(retail, "cursorHintString")
        ),
    );
}

fn replay(fx: &Fixture) -> Option<Tally> {
    let (mut level, gun, runs, mut done) = spawned(fx)?;
    let report = std::env::var_os("TURRET_REPORT").is_some();
    let mut t = Tally::default();
    let main = runs.last().unwrap();
    let mut now = main.side.snaps[0].time;
    let mut ours_seq = None;
    let mut retail_seq = main.guns[0].i32("eventSequence");
    for (k, snap) in main.side.snaps.iter().enumerate().skip(1) {
        while now + 50 < snap.time {
            now += 50;
            level.run_frame(now);
        }
        // Each client's cmds up to its snapshot's `commandTime`, the
        // clients' packets in the order they were sent.
        let mut due: Vec<(i32, usize, UserCmd2)> = Vec::new();
        for (r, run) in runs.iter().enumerate() {
            let Some((rs, _)) = run.at(snap.time) else {
                continue;
            };
            let ct = rs.ps.f[0] as i32;
            for (c, cmd) in run.side.cmds.iter().enumerate() {
                if cmd.server_time > done[r] && cmd.server_time <= ct {
                    let sent = run.side.sent.get(c).copied().unwrap_or(cmd.server_time);
                    due.push((sent, run.slot, *cmd));
                }
            }
            done[r] = done[r].max(ct);
        }
        due.sort_by_key(|&(sent, slot, c)| (sent, slot, c.server_time));
        let mut packet = None;
        for (sent, slot, cmd) in &due {
            if runs.len() > 1 && packet != Some((*sent, *slot)) {
                packet = Some((*sent, *slot));
                level.packet();
            }
            // The capture's weapon byte as sent: the level registers its
            // weapons in retail's order (cod2-protocol.md 15.3).
            level.client_think(*slot, cmd);
        }
        now = snap.time;
        level.run_frame(now);
        let mut bad = Vec::new();
        for run in &runs {
            let Some((rs, _)) = run.at(snap.time) else {
                continue;
            };
            let ours = level.wire_playerstate(run.slot, 0).unwrap();
            let mut mine = Vec::new();
            check_player(&mut t, &mut mine, &ours, &rs.ps, gun.0, fx.gun);
            bad.extend(mine.into_iter().map(|b| format!("slot {}: {b}", run.slot)));
        }
        let ent = level
            .rt
            .packet_entities(vcod_cod2::net::PROTOCOL_118.as_cod1_shim());
        let ours_gun = crate::game::cod2_wire::to_cod2_entity(&ent[&gun.0]).unwrap();
        let retail_gun = &main.guns[k];
        let mut check = |what: &str, ok: bool, detail: String| {
            if !ok {
                bad.push(format!("{what}: {detail}"));
            }
        };
        for k in 0..2 {
            let n = format!("angles2[{k}]");
            let d = adiff(ours_gun.f32(&n), retail_gun.f32(&n));
            t.worst_barrel = t.worst_barrel.max(d.abs());
            check(
                "barrel",
                d.abs() < 1e-2,
                format!("{n} {} vs {}", ours_gun.f32(&n), retail_gun.f32(&n)),
            );
        }
        check(
            "gun weapon",
            ours_gun.i32("weapon") == retail_gun.i32("weapon"),
            format!("{} vs {}", ours_gun.i32("weapon"), retail_gun.i32("weapon")),
        );
        check(
            "gun eFlags",
            ours_gun.i32("eFlags") & 0x40 == retail_gun.i32("eFlags") & 0x40,
            format!(
                "{:x} vs {:x}",
                ours_gun.i32("eFlags"),
                retail_gun.i32("eFlags")
            ),
        );
        check(
            "loopSound",
            ours_gun.i32("loopSound") == retail_gun.i32("loopSound"),
            format!(
                "{} vs {}",
                ours_gun.i32("loopSound"),
                retail_gun.i32("loopSound")
            ),
        );
        // The gun's new events since the last snapshot, by id.
        let events = |e: &EntityState2, from: i32| -> Vec<i32> {
            (from..e.i32("eventSequence"))
                .map(|s| e.i32(&format!("events[{}]", s & 3)))
                .collect()
        };
        let ours_from = ours_seq.unwrap_or(ours_gun.i32("eventSequence"));
        let (a, b) = (events(&ours_gun, ours_from), events(retail_gun, retail_seq));
        t.fired += b.iter().filter(|&&e| e == 175).count();
        check("gun events", a == b, format!("{a:?} vs {b:?}"));
        ours_seq = Some(ours_gun.i32("eventSequence"));
        retail_seq = retail_gun.i32("eventSequence");
        if report {
            eprintln!("{}: {}", snap.time, bad.join("; "));
        }
        t.mismatches
            .extend(bad.into_iter().map(|b| format!("{}: {b}", snap.time)));
    }
    Some(t)
}

fn gate(fx: &Fixture, fired: usize) {
    let Some(t) = replay(fx) else {
        return;
    };
    eprintln!(
        "mounted {} fired {} worst view {} origin {} barrel {}",
        t.mounted, t.fired, t.worst_view, t.worst_origin, t.worst_barrel
    );
    assert!(t.mounted > 20, "{t:?}");
    assert!(t.fired >= fired, "{t:?}");
    assert!(
        t.mismatches.is_empty(),
        "{} mismatches, first: {:#?}",
        t.mismatches.len(),
        &t.mismatches[..t.mismatches.len().min(12)]
    );
}

#[test]
fn retail_mg42_mount_fire_and_release_replay_on_the_cod2_server() {
    gate(&MG42, 20);
}

#[test]
fn retail_prone_gun_mount_fire_and_release_replay_on_the_cod2_server() {
    gate(&PRONE, 20);
}

#[test]
fn retail_duck_gun_mount_fire_and_release_replay_on_the_cod2_server() {
    gate(&DUCK, 20);
}

#[test]
fn retail_prone_30cal_mount_fire_and_release_replay_on_the_cod2_server() {
    gate(&CAL30, 15);
}

#[test]
fn retail_gunner_shot_off_the_gun_while_firing_replays_on_the_cod2_server() {
    gate(&SHOT, 20);
}

#[test]
fn retail_second_player_at_a_manned_gun_replays_on_the_cod2_server() {
    gate(&SHARE, 10);
}

#[test]
fn retail_gunner_killed_on_the_gun_replays_on_the_cod2_server() {
    // The trigger goes down on the frame the gunner dies: no round.
    gate(&DIE, 0);
}
