//! Retail's ladder climb on mp_toujane (`vcod-cod2`'s `moves/ladder.txt`,
//! docs/research/cod2-movement.md 11) replayed through `Cod2Level`'s own
//! `client_think`: the body takes the ladder, climbs it, hangs, strafes,
//! climbs down, pushes off and climbs over the top where retail's did,
//! and the gun goes down on the rungs and up off them as retail's did
//! (docs/research/cod2-combat.md 1.7). Needs `COD2_DIR`; without it the
//! tests pass.

use std::rc::Rc;

use vcod_cod2::net::delta::field_index;
use vcod_cod2::net::fields::PLAYER_STATE_FIELDS;
use vcod_cod2::net::{PlayerState2, UserCmd2};
use vcod_cod2::pmove::weapon::{ev, state};
use vcod_cod2::pmove::{PlayerState, button, pmf};

use crate::game::cod2::Cod2Level;
use crate::game::script::MenuRef;

const CAPTURE: &str = include_str!("../../../cod2/tests/fixtures/moves/ladder.txt");

/// The capture's cmds and its snapshots' playerstates (each a full state:
/// the logged words are deltas).
fn parse() -> (Vec<UserCmd2>, Vec<PlayerState2>) {
    let mut cmds = Vec::new();
    let mut snaps = Vec::new();
    let mut ps = PlayerState2::default();
    for line in CAPTURE.lines() {
        let mut w = line.split_whitespace();
        match w.next() {
            Some("c") => {
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
                for kv in w.skip(2) {
                    let (k, v) = kv.split_once('=').unwrap();
                    ps.f[field_index(&PLAYER_STATE_FIELDS, k)] =
                        u32::from_str_radix(v, 16).unwrap();
                }
                snaps.push(ps.clone());
            }
            _ => {}
        }
    }
    (cmds, snaps)
}

/// A `dm` level on mp_toujane with one spawned client, his body set down
/// as retail's stood at the first snapshot that reached a logged cmd and
/// the clock caught up to it: (level, clock, that snapshot, its cmd).
fn spawned(cmds: &[UserCmd2], snaps: &[PlayerState2]) -> Option<(Cod2Level, i32, usize, usize)> {
    let fs = vcod_common::testing::cod2_fs()?;
    let mut level = Cod2Level::load(
        Rc::new(fs),
        "mp_toujane",
        "dm",
        crate::cvars::Cvars::new(),
        0,
    )
    .unwrap();
    let mut now = 0;
    let idle = |level: &mut Cod2Level, now: &mut i32| {
        *now += 50;
        let cmd = UserCmd2 {
            server_time: *now,
            ..Default::default()
        };
        level.client_think(0, &cmd);
        level.run_frame(*now);
    };
    level.connect(0, "climber");
    idle(&mut level, &mut now);
    level.begin(0);
    idle(&mut level, &mut now);
    level.menu_response(0, &MenuRef::Name("team_britishgerman".into()), "autoassign");
    for _ in 0..4 {
        idle(&mut level, &mut now);
    }
    let session = level.rt.client_field(0, "sessionteam").unwrap_or_default();
    let (menu, weapon) = if session == "axis" {
        ("weapon_german", "kar98k_mp")
    } else {
        ("weapon_british", "enfield_mp")
    };
    level.menu_response(0, &MenuRef::Name(menu.into()), weapon);
    for _ in 0..4 {
        idle(&mut level, &mut now);
    }
    assert_eq!(
        level.rt.client_field(0, "sessionstate").as_deref(),
        Some("playing")
    );

    let at = |t: i32| cmds.iter().position(|c| c.server_time == t);
    let (start, ci) = snaps
        .iter()
        .enumerate()
        .find_map(|(k, s)| Some((k, at(s.f[0] as i32).filter(|&i| i >= 1)?)))
        .unwrap();
    while now + 50 <= snaps[start].f[0] as i32 {
        idle(&mut level, &mut now);
    }
    let p = level.players[0].as_mut().unwrap();
    p.ps = PlayerState::from_wire(&snaps[start]);
    p.oldcmd = cmds[ci];
    Some((level, now, start, ci))
}

#[test]
fn retail_ladder_climb_replays_on_the_cod2_server() {
    let (cmds, snaps) = parse();
    let Some((mut level, mut now, start, mut ci)) = spawned(&cmds, &snaps) else {
        return;
    };
    let at = |t: i32| cmds.iter().position(|c| c.server_time == t);
    let mut climbed = false;
    let mut worst = 0.0f32;
    for snap in &snaps[start + 1..] {
        let j = at(snap.f[0] as i32).expect("a logged cmd");
        while ci < j {
            ci += 1;
            level.client_think(0, &cmds[ci]);
            if cmds[ci].server_time >= now + 50 {
                now += 50;
                level.run_frame(now);
            }
        }
        let ours = &level.players[0].as_ref().unwrap().ps;
        let retail = PlayerState::from_wire(snap);
        assert_eq!(
            ours.pm_flags & pmf::LADDER,
            retail.pm_flags & pmf::LADDER,
            "commandTime {}: ladder flag",
            snap.f[0]
        );
        worst = worst.max(ours.origin.distance(retail.origin));
        climbed |= ours.pm_flags & pmf::LADDER != 0 && ours.origin.z > 100.0;
    }
    assert!(climbed, "the body never rose up the ladder");
    let end = level.players[0].as_ref().unwrap().ps.origin;
    let retail_end = PlayerState::from_wire(snaps.last().unwrap()).origin;
    assert!(
        end.z > 110.0 && worst < 0.01,
        "ours ended at {end} (retail {retail_end}), at worst {worst} off retail"
    );
}

/// What [`climb_armed`] saw: the weapon events, whether a shot went off
/// on the rungs, the cmds on the rungs and those that ended empty-handed,
/// and the weapon and its state at the end.
struct Armed {
    events: Vec<i32>,
    fired_on_rungs: bool,
    rungs: usize,
    empty: usize,
    end: (u8, i32),
}

/// The climb with the cmds' weapon byte our spawn rifle's, the attack held
/// on every cmd that starts on the rungs when `attack`.
fn climb_armed(attack: bool) -> Option<Armed> {
    let (cmds, snaps) = parse();
    let (mut level, mut now, _, ci) = spawned(&cmds, &snaps)?;
    // The spawn's rifle: the held weapon that is not a grenade.
    let held = level.rt.host.client_weapons[0].held;
    let gun = (1..64u8)
        .find(|&i| {
            held >> i & 1 != 0
                && level.combat.weapons[usize::from(i)]
                    .as_ref()
                    .is_some_and(|w| w.offhand_class == 0)
        })
        .expect("spawned with no rifle");
    let mut out = Armed {
        events: Vec::new(),
        fired_on_rungs: false,
        rungs: 0,
        empty: 0,
        end: (0, 0),
    };
    for c in &cmds[ci + 1..] {
        let p = level.players[0].as_ref().unwrap();
        let on = p.ps.pm_flags & pmf::LADDER != 0;
        let seq = p.ps.event_sequence;
        let mut cmd = *c;
        cmd.weapon = gun;
        if on && attack {
            cmd.buttons |= button::ATTACK;
        }
        level.client_think(0, &cmd);
        if cmd.server_time >= now + 50 {
            now += 50;
            level.run_frame(now);
        }
        let p = level.players[0].as_ref().unwrap();
        for i in seq..p.ps.event_sequence {
            let e = p.ps.events[(i & 3) as usize];
            if (ev::NOAMMO..=ev::SWITCH_OFFHAND).contains(&e) {
                out.events.push(e);
            }
            out.fired_on_rungs |= on && matches!(e, ev::FIRE_WEAPON | ev::FIRE_WEAPON_LASTSHOT);
        }
        if on {
            out.rungs += 1;
            out.empty += usize::from(p.ws.weapon == 0);
        }
    }
    let ws = &level.players[0].as_ref().unwrap().ws;
    out.end = (ws.weapon, ws.state);
    assert_eq!(out.end.0, gun, "the rifle did not come back");
    Some(out)
}

/// The gun goes down as the body takes the ladder (`EV_PUTAWAY_WEAPON`,
/// then `EV_RAISE_WEAPON` onto the empty hands, as the `ladder` capture's
/// snapshots carry them), the hands stay empty on the rungs, and the rifle
/// comes back up, with no event, as the body leaves.
#[test]
fn the_ladder_holsters_the_gun_and_raises_it_after() {
    let Some(a) = climb_armed(false) else {
        return;
    };
    // The capture climbs twice.
    assert_eq!(
        a.events,
        [
            ev::PUTAWAY_WEAPON,
            ev::RAISE_WEAPON,
            ev::PUTAWAY_WEAPON,
            ev::RAISE_WEAPON
        ]
    );
    assert!(
        a.empty * 10 > a.rungs * 8,
        "the hands were empty on {} of {} cmds on the rungs",
        a.empty,
        a.rungs
    );
    assert_eq!(a.end.1, state::READY);
}

/// The attack held on the rungs fires nothing. A semi-automatic's held
/// trigger keeps the drop's `weaponTime` at 1 (cod2-combat.md 1.7), so the
/// hands never empty, and the drop ends back on the rifle once the body
/// leaves and the trigger is let go.
#[test]
fn nothing_fires_on_the_ladder() {
    let Some(a) = climb_armed(true) else {
        return;
    };
    assert!(!a.fired_on_rungs);
    assert_eq!(a.events, [ev::PUTAWAY_WEAPON, ev::PUTAWAY_WEAPON]);
    assert_eq!(a.end.1, state::READY);
}
