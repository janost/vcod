//! The killstreak hardpoints on ours over the kill pair's join
//! (`combat_ab::ours_hooked`, mp_crash `war`): the axis shooter calls in
//! the stock scripts' airstrike (`_hardpoints.gsc`'s `useAirstrike`, what
//! a confirmed location runs) and helicopter (`triggerHardpoint
//! ("helicopter_mp")`, what selecting the item runs) on the allied target,
//! and the target's snapshots carry the planes and the helicopter while
//! the damage lands (docs/research/cod4-gsc.md 3.2). Needs `COD4_DIR`.
//!
//! `HARDPOINT_REPORT=1` prints each second's vehicles and health.

use glam::Vec3;
use vcod_gsc::{EntId, Host, Value};

use super::combat_ab::kill;
use super::wire_ab::{Probe, side};
use super::*;
use crate::game::cod4::level::Cod4Level;
use crate::game::cod4::vehicle::{ET_HELICOPTER, ET_PLANE};

/// When the call goes in: both stand, past both `setviewpos` answers.
const CALL_MS: i32 = 48_000;
/// Where the target's eye goes before the call: open ground by the
/// shooter's spawn, no roof over it.
const TARGET_EYE: Vec3 = Vec3::new(-850.0, 2204.0, 314.0);

/// What one run saw: the target's snapshots, its lowest health after the
/// call, and the script errors.
struct Seen {
    snaps: Vec<vcod_cod4::net::parse::Snapshot>,
    low_health: i32,
    aborts: Vec<String>,
}

type Call<'a> = &'a dyn Fn(&mut Cod4Level, EntId, Vec3);
type Each<'a> = &'a mut dyn FnMut(&mut Cod4Level, &mut [Probe], i32);

/// The pair joined, the shooter held still, the target moved to
/// [`TARGET_EYE`], `cvars` set 2 s before [`CALL_MS`] (past a second of
/// `heli_update_global_dvars`' loop), `call` run on the level at
/// [`CALL_MS`] with the shooter's entity and the target's origin, `each`
/// after it every tick with the probes, and the server run to `until`.
fn run(until: i32, cvars: &[(&str, &str)], call: Call, each: Each) -> Option<Seen> {
    let classes = ["mr {sid} 15 assault_mp,0"; 2];
    run_classes(until, cvars, classes, &mut |_, _, _| {}, call, each)
}

/// [`run`] with each side's class (`mr` request) given, and `early` run
/// every tick before [`CALL_MS`].
fn run_classes(
    until: i32,
    cvars: &[(&str, &str)],
    classes: [&'static str; 2],
    early: Each,
    call: Call,
    each: Each,
) -> Option<Seen> {
    let install = vcod_cod4::testing::cod4_install()?;
    let rt = side(&kill("target"));
    let cs0 = String::from_utf8_lossy(rt.gs.configstring(0)).into_owned();
    let report = std::env::var_os("HARDPOINT_REPORT").is_some();
    let mut low = i32::MAX;
    let mut hook = |sv: &mut Cod4Server, probes: &mut [Probe], svs: i32| {
        let level = sv.game.level.as_mut().unwrap();
        if svs < CALL_MS {
            early(level, probes, svs);
        }
        if svs >= 46_900
            && let Some(ps) = level.pm_state(1)
        {
            // The shooter holds still: its own view, no keys.
            let short = |a: f32| ((a * 65536.0 / 360.0).round() as i32 & 0xffff) as u16;
            let c = UserCmd {
                server_time: svs + 10,
                weapon: 27,
                offhand: 40,
                angles: std::array::from_fn(|k| {
                    short(ps.view_angles[k]).wrapping_sub(short(ps.delta_angles[k]))
                }),
                ..Default::default()
            };
            probes[1].replay.clear();
            probes[1].replay.push_back((svs, vec![c]));
        }
        if svs == CALL_MS - 2000 {
            for (k, v) in cvars {
                level.rt.host.cvars.set(k, v);
            }
        }
        if svs == CALL_MS - 500 {
            level.set_view_pos(0, TARGET_EYE.to_array(), [0.0, 0.0, 0.0]);
        }
        if svs == CALL_MS {
            let shooter = level.rt.client_entity(1).unwrap();
            let at = level.pm_state(0).unwrap().origin;
            call(level, shooter, at);
        }
        if svs > CALL_MS {
            each(level, probes, svs);
            low = low.min(level.rt.client_vitals(0).health);
        }
        if report && svs % 1000 == 0 {
            let c4 = level.rt.host.cod4();
            let helis: Vec<_> = c4
                .helis
                .iter()
                .map(|h| (h.origin, h.angles.y, h.gun_yaw, h.gun_pitch))
                .collect();
            let planes = c4.planes.len();

            eprintln!(
                "{svs}: health {} target {:?} planes {planes} helis {helis:?} aborts {:?}",
                level.rt.client_vitals(0).health,
                level.pm_state(0).map(|p| p.origin),
                level.rt.aborts()
            );
        }
    };
    let late = [(until - 43_850, "score")];
    let (probes, aborts, _) =
        super::combat_ab::ours_set_up(&install, &cs0, &[], classes, None, &late, &mut hook);
    Some(Seen {
        snaps: side(&probes[0].lines).snaps,
        low_health: low,
        aborts,
    })
}

/// The entities of one type in a snapshot.
fn of_type(
    f: &vcod_cod4::net::parse::Snapshot,
    etype: u32,
) -> Vec<vcod_cod4::net::delta::EntityState> {
    f.entities
        .iter()
        .filter(|e| e.etype() == etype)
        .copied()
        .collect()
}

fn word(e: &vcod_cod4::net::delta::EntityState, name: &str) -> u32 {
    vcod_cod4::net::delta::EntityState::table(e.etype())
        .iter()
        .find(|f| f.name == name)
        .map_or(0, |f| e.0[f.word()])
}

/// The first `tag_origin` script model the strike spawns
/// (`callStrike_bombEffect`'s `newBomb`): its origin and angles.
fn new_bomb(level: &mut Cod4Level) -> Option<(Vec3, Vec3)> {
    level.rt.with_host(|h, cx| {
        let ids: Vec<EntId> = h.ents.iter_inuse().map(|(id, _)| id).collect();
        let (m, o, a) = (
            cx.intern_folded("model"),
            cx.intern_folded("origin"),
            cx.intern_folded("angles"),
        );
        ids.into_iter().find_map(|id| {
            let Value::String(s) = h.get_field(cx, id, m) else {
                return None;
            };
            if cx.resolve(s) != "tag_origin" {
                return None;
            }
            match (h.get_field(cx, id, o), h.get_field(cx, id, a)) {
                (Value::Vector(o), Value::Vector(a)) => Some((Vec3::from(o), Vec3::from(a))),
                _ => None,
            }
        })
    })
}

/// The airstrike: three `ET_PLANE`s with the shooter's team (axis, 1) and
/// slot (1) in `teamAndOwnerIndex` fly over, and the bombs'
/// `losRadiusDamage` hurts the target, stood where the first bomblet's
/// trace (55 down the bomb's heading) meets the ground as the bomb opens.
#[test]
fn an_airstrike_flies_three_planes_and_hurts_the_target() {
    let mut placed = false;
    let mut each = |level: &mut Cod4Level, _: &mut [Probe], _: i32| {
        if placed {
            return;
        }
        let Some((origin, angles)) = new_bomb(level) else {
            return;
        };
        placed = true;
        let [dir, _, _] = vcod_common::x87::angle_vectors(angles + Vec3::new(55.0, 0.0, 0.0));
        let end = origin + dir * 10_000.0;
        let hit = crate::game::builtins::cod4_trace::trace4(
            &level.rt.host,
            origin,
            end,
            Vec3::ZERO,
            Vec3::ZERO,
            1023,
            0x80_6831,
        )
        .map(|t| origin.lerp(end, t.fraction))
        .unwrap();
        level.set_view_pos(0, (hit + Vec3::Z * 61.0).to_array(), [0.0, 0.0, 0.0]);
    };
    let Some(seen) = run(
        62_000,
        &[],
        &|level, shooter, at| {
            level.rt.start_script_thread_for_test(
                "maps/mp/gametypes/_hardpoints",
                "useAirstrike",
                Some(shooter),
                |_| vec![Value::Vector(at.to_array())],
            );
        },
        &mut each,
    ) else {
        return;
    };
    assert!(placed, "no bomb opened");
    assert!(seen.aborts.is_empty(), "{:?}", seen.aborts);
    let mut planes = std::collections::BTreeSet::new();
    for f in &seen.snaps {
        for e in of_type(f, ET_PLANE) {
            assert_eq!(word(&e, "lerp.u.vehicle.teamAndOwnerIndex"), 1 | 1 << 2);
            assert_ne!(word(&e, "index"), 0, "a plane has its model");
            planes.insert(e.number());
        }
    }
    assert_eq!(planes.len(), 3, "{planes:?}");
    assert!(seen.low_health < 100, "health {}", seen.low_health);
}

/// Stands the target on open ground 600 ahead of the helicopter, clear
/// of its gun (the turret point `canTarget_turret` sights from, 160 under
/// and 144 ahead, to the body's middle), when there is such ground.
fn in_view(level: &mut Cod4Level) -> bool {
    let Some(h) = level.rt.host.cod4().helis.first().cloned() else {
        return false;
    };
    let host = &level.rt.host;
    let tr = |a: Vec3, b: Vec3, mask: u32| {
        crate::game::builtins::cod4_trace::trace4(host, a, b, Vec3::ZERO, Vec3::ZERO, h.id.0, mask)
            .map_or(1.0, |t| t.fraction)
    };
    let [f, _, _] = vcod_common::x87::angle_vectors(Vec3::new(0.0, h.angles.y, 0.0));
    let above = h.origin + f * 600.0;
    let down = above - Vec3::Z * 4000.0;
    let f_down = tr(above, down, 0x80_6831);
    if f_down >= 1.0 {
        return false;
    }
    let ground = above.lerp(down, f_down);
    let turret = h.origin - Vec3::Z * 160.0 + f * 144.0;
    let mid = ground + Vec3::Z * 36.0;
    let head = ground + Vec3::Z * 64.0;
    if ground.z >= h.origin.z - 200.0
        || tr(turret, mid, 0x80_6831) < 1.0
        || tr(turret, head, 0x80_6831) < 1.0
    {
        return false;
    }
    level.set_view_pos(0, (ground + Vec3::Z * 61.0).to_array(), [0.0, 0.0, 0.0]);
    true
}

/// The helicopter on its path: an `ET_HELICOPTER` with its owner and
/// model reaches the target's snapshots and flies (its origin moves a
/// long way), its gun fires on the target, stood in its view once it
/// spawns (`EV_FIRE_WEAPON` on its entity, the target hurt), it leaves,
/// and no thread dies.
#[test]
fn a_helicopter_flies_its_path_and_fires() {
    let mut placed = false;
    let mut last = 0;
    let Some(seen) = run(
        240_000,
        &[],
        &|level, shooter, _| {
            level.rt.start_script_thread_for_test(
                "maps/mp/gametypes/_hardpoints",
                "triggerHardpoint",
                Some(shooter),
                |cx| vec![Value::String(cx.intern_exact("helicopter_mp"))],
            );
        },
        &mut |level, _, svs| {
            // Again every 4 s until the gun has hurt it: the helicopter
            // flies on, and the target stands still.
            if level.rt.client_vitals(0).health == 100 && svs - last >= 4000 && in_view(level) {
                placed = true;
                last = svs;
            }
        },
    ) else {
        return;
    };
    assert!(placed, "the helicopter never had open ground in view");
    assert!(seen.aborts.is_empty(), "{:?}", seen.aborts);
    let helis: Vec<(i32, vcod_cod4::net::delta::EntityState)> = seen
        .snaps
        .iter()
        .flat_map(|f| {
            of_type(f, ET_HELICOPTER)
                .into_iter()
                .map(move |e| (f.server_time, e))
        })
        .collect();
    assert!(!helis.is_empty(), "no helicopter reached the target");
    let first = helis.first().unwrap().1;
    assert_eq!(word(&first, "lerp.u.vehicle.teamAndOwnerIndex"), 1 | 1 << 2);
    assert_ne!(word(&first, "un2"), 0);
    let o = |e: &vcod_cod4::net::delta::EntityState| Vec3::from(e.origin());
    let flown = helis
        .iter()
        .map(|(_, e)| o(e).distance(o(&first)))
        .fold(0.0f32, f32::max);
    assert!(flown > 1000.0, "flew {flown}");
    let fired = helis
        .iter()
        .any(|(_, e)| (0..4).any(|k| word(e, &format!("events[{k}]")) == 26));
    assert!(fired, "never fired");
    assert!(seen.low_health < 100, "health {}", seen.low_health);
    // Its loop done (`scr_heli_loopmax` 1), `heli_leave` flies it off and
    // deletes it: the last snapshots carry none.
    let last = helis.last().unwrap().0;
    let end = seen.snaps.last().unwrap().server_time;
    assert!(last + 10_000 < end, "still there at {last} of {end}");
}

/// The middle of the first helicopter's collision boxes in the world, and
/// its model; `None` before it spawns.
fn heli_mid(level: &Cod4Level) -> Option<(Vec3, String)> {
    let c4 = level.rt.host.cod4();
    let h = c4.helis.first()?;
    let surfs = c4.model_coll.get(&h.model.to_ascii_lowercase())?;
    let (lo, hi) = surfs.iter().fold(
        (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN)),
        |(lo, hi), s| (lo.min(Vec3::from(s.mins)), hi.max(Vec3::from(s.maxs))),
    );
    Some((h.origin + h.quat() * ((lo + hi) * 0.5), h.model.clone()))
}

/// Shot down: the allied target, held 400 under and 300 beside the axis
/// shooter's helicopter and kept alive, fires its rifle at the
/// helicopter's middle every other frame. The rounds meet the model's
/// collision, `heli_damage_monitor` adds them up, `heli_health` smokes it
/// (`setDamageStage` 3 down to 1 on the wire) and `heli_crash` flies the
/// crash path (stage 0) and blows it up: the entity goes long before its
/// loop would end. No thread dies. `scr_heli_maxhealth` and
/// `scr_heli_armor` are lowered so a clip or two does it.
#[test]
fn a_helicopter_shot_down_crashes() {
    let mut fired = 0;
    let mut had_coll = false;
    let Some(seen) = run(
        150_000,
        &[("scr_heli_maxhealth", "300"), ("scr_heli_armor", "100")],
        &|level, shooter, _| {
            level.rt.start_script_thread_for_test(
                "maps/mp/gametypes/_hardpoints",
                "triggerHardpoint",
                Some(shooter),
                |cx| vec![Value::String(cx.intern_exact("helicopter_mp"))],
            );
        },
        &mut |level, probes, svs| {
            let Some((mid, _)) = heli_mid(level) else {
                return;
            };
            had_coll = true;
            let h = level.rt.host.cod4().helis.first().cloned().unwrap();
            let [_, right, _] = vcod_common::x87::angle_vectors(Vec3::new(0.0, h.angles.y, 0.0));
            let eye = mid - Vec3::Z * 400.0 + right * 300.0;
            let a = crate::game::builtins::math::vectoangles((mid - eye).to_array());
            level.set_view_pos(0, eye.to_array(), [a[0], a[1], 0.0]);
            level.rt.host.client_vitals[0].health = 100;
            let Some(ps) = level.pm_state(0) else {
                return;
            };
            let short = |a: f32| ((a * 65536.0 / 360.0).round() as i32 & 0xffff) as u16;
            let attack = (svs / 50) % 2 == 0;
            if attack {
                fired += 1;
            }
            let c = UserCmd {
                server_time: svs + 10,
                weapon: ps.weapon as u8,
                offhand: ps.off_hand_index as u8,
                buttons: u32::from(attack),
                angles: std::array::from_fn(|k| {
                    short(ps.view_angles[k]).wrapping_sub(short(ps.delta_angles[k]))
                }),
                ..Default::default()
            };
            probes[0].replay.clear();
            probes[0].replay.push_back((svs, vec![c]));
        },
    ) else {
        return;
    };
    assert!(had_coll, "the helicopter's model has no collision");
    assert!(fired > 0);
    assert!(seen.aborts.is_empty(), "{:?}", seen.aborts);
    let stages: Vec<(i32, u32)> = seen
        .snaps
        .iter()
        .flat_map(|f| {
            of_type(f, ET_HELICOPTER)
                .into_iter()
                .map(move |e| (f.server_time, word(&e, "un1.helicopterStage")))
        })
        .collect();
    let mut order: Vec<u32> = Vec::new();
    for (_, st) in &stages {
        if order.last() != Some(st) {
            order.push(*st);
        }
    }
    assert!(order.contains(&3), "{order:?}");
    assert!(order.contains(&1), "{order:?}");
    assert_eq!(order.last(), Some(&0), "{order:?}");
    let last = stages.last().unwrap().0;
    let end = seen.snaps.last().unwrap().server_time;
    assert!(last + 10_000 < end, "still there at {last} of {end}");
}

/// `bulletTrace` scores the posed hit boxes: a segment through the middle
/// of the target's head box names the target (not when it passes him),
/// and his posed `j_head` sits by that box; a segment
/// across the target's capsule wide of the body (a plain trace would meet
/// it) passes; a segment through the helicopter's middle
/// names the helicopter.
#[test]
fn bullet_traces_meet_posed_bodies_and_the_helicopter() {
    let mut checked = (false, false);
    let Some(seen) = run(
        60_000,
        &[],
        &|level, shooter, _| {
            level.rt.start_script_thread_for_test(
                "maps/mp/gametypes/_hardpoints",
                "triggerHardpoint",
                Some(shooter),
                |cx| vec![Value::String(cx.intern_exact("helicopter_mp"))],
            );
        },
        &mut |level, _, svs| {
            use crate::game::builtins::cod4_trace::{locational4, trace4};
            let target = level.rt.client_entity(0).unwrap().0;
            if svs == CALL_MS + 1000 {
                let head = level.hit_box_center(0, 2).unwrap();
                let host = &level.rt.host;
                let (a, b) = (
                    head + Vec3::new(60.0, 0.0, 0.0),
                    head - Vec3::new(60.0, 0.0, 0.0),
                );
                let t = locational4(host, a, b, 1023, 0x280_6831).unwrap();
                assert_eq!(t.entity_num, target, "{t:?}");
                // `getTagOrigin("j_head")` on the posed body sits by the head
                // box, not at the bind pose's.
                let t_id = level.rt.client_entity(0).unwrap();
                let (tag, _) =
                    crate::game::builtins::cod4_script::posed_tag(host, t_id, "j_head").unwrap();
                assert!(tag.distance(head) < 12.0, "{tag:?} {head:?}");
                // Passing the target, the same segment misses it.
                let t = locational4(host, a, b, target, 0x280_6831).unwrap();
                assert_ne!(t.entity_num, target, "{t:?}");
                // Wide: 14 out at the capsule's straight part, across the
                // temp box model's capsule (cod4-pmove.md 15).
                let o = level.pm_state(0).unwrap().origin;
                let (a, b) = (
                    o + Vec3::new(14.0, -40.0, 20.0),
                    o + Vec3::new(14.0, 40.0, 20.0),
                );
                let boxed = trace4(host, a, b, Vec3::ZERO, Vec3::ZERO, 1023, 0x280_6831).unwrap();
                assert_eq!(boxed.entity_num, target, "{boxed:?}");
                let posed = locational4(host, a, b, 1023, 0x280_6831).unwrap();
                assert_ne!(posed.entity_num, target, "{posed:?}");
                checked.0 = true;
            }
            if !checked.1
                && let Some((mid, _)) = heli_mid(level)
            {
                let host = &level.rt.host;
                let (a, b) = (
                    mid + Vec3::new(0.0, 0.0, 400.0),
                    mid - Vec3::new(0.0, 0.0, 10.0),
                );
                let id = host.cod4().helis[0].id.0;
                let t = locational4(host, a, b, 1023, 0x280_6831).unwrap();
                assert_eq!(t.entity_num, id, "{t:?}");
                checked.1 = true;
            }
        },
    ) else {
        return;
    };
    assert!(checked.0 && checked.1, "{checked:?}");
    assert!(seen.aborts.is_empty(), "{:?}", seen.aborts);
}

/// One RPG flight at the helicopter: the allied target, on a custom class
/// with RPG x2 (`specialty_weapon_rpg`), held 1500 behind and 600 under
/// the axis shooter's helicopter, raises the RPG and fires once along a
/// view `off` degrees to the side of the body's middle. Each frame's
/// rocket (origin, velocity) and the helicopter's middle then. `cvars`
/// as [`run`]'s.
struct RpgFlight {
    rows: Vec<(Vec3, Vec3, Vec3)>,
    aborts: Vec<String>,
}

fn rpg_at_heli(cvars: &[(&str, &str)], off: f32) -> Option<RpgFlight> {
    let mut rows = Vec::new();
    let mut since = None;
    let mut held = (Vec3::ZERO, [0.0; 3]);
    let mut early = |level: &mut Cod4Level, _: &mut [Probe], svs: i32| {
        if !(44_000..45_850).contains(&svs) {
            return;
        }
        let row = |col, v: &str| super::perk_tests::stat_row(level, col, v);
        let vals = [
            (201, row(4, "m16")),
            (202, 0),
            (203, row(4, "beretta")),
            (204, 0),
            (205, row(6, "specialty_weapon_rpg")),
            (206, row(6, "specialty_armorvest")),
            (207, row(6, "specialty_longersprint")),
            (208, row(4, "flash_grenade")),
            (209, 0),
        ];
        let st = &mut level.rt.host.cod4_mut().stats[0];
        for (k, v) in vals {
            st.set(k, v);
        }
    };
    let mut each = |level: &mut Cod4Level, probes: &mut [Probe], svs: i32| {
        let Some((mid, _)) = heli_mid(level) else {
            return;
        };
        let h = level.rt.host.cod4().helis.first().cloned().unwrap();
        let first = *since.get_or_insert(svs);
        let rpg = level.rt.host.cod4().table.index("rpg_mp").unwrap() as u8;
        // Over the town by then, on its loop.
        let fire_at = first + 15_000;
        if svs <= fire_at {
            let [fwd, _, _] = vcod_common::x87::angle_vectors(Vec3::new(0.0, h.angles.y, 0.0));
            let eye = mid - fwd * 900.0 - Vec3::Z * 300.0;
            let a = crate::game::builtins::math::vectoangles((mid - eye).to_array());
            held = (eye, [a[0], a[1] + off, 0.0]);
        }
        // Held in the air where it fired from.
        level.set_view_pos(0, held.0.to_array(), held.1);
        level.rt.host.client_vitals[0].health = 100;
        let Some(ps) = level.pm_state(0) else {
            return;
        };
        if std::env::var_os("HARDPOINT_REPORT").is_some() && svs % 500 == 0 {
            eprintln!(
                "{svs} fire_at {fire_at} weapon {} state {} origin {:?} mid {mid:?}",
                ps.weapon, ps.weapon_state, ps.origin
            );
        }
        let short = |a: f32| ((a * 65536.0 / 360.0).round() as i32 & 0xffff) as u16;
        let c = UserCmd {
            server_time: svs + 10,
            weapon: rpg,
            offhand: ps.off_hand_index as u8,
            buttons: u32::from((fire_at..fire_at + 100).contains(&svs)),
            angles: std::array::from_fn(|k| {
                short(ps.view_angles[k]).wrapping_sub(short(ps.delta_angles[k]))
            }),
            ..Default::default()
        };
        probes[0].replay.clear();
        probes[0].replay.push_back((svs, vec![c]));
        let c4 = level.rt.host.cod4();
        if let Some(m) = c4.missiles.iter().find(|m| m.weapon == u32::from(rpg)) {
            rows.push((m.origin, m.pos.delta, mid));
            if std::env::var_os("HARDPOINT_REPORT").is_some() {
                eprintln!(
                    "{svs} rpg {:?} wait {} flags {:#x} curve {:?}",
                    m.pos.tr_type, m.wait, m.flags, m.curve
                );
            }
        }
    };
    let seen = run_classes(
        100_000,
        cvars,
        ["mr {sid} 15 custom1,0", "mr {sid} 15 assault_mp,0"],
        &mut early,
        &|level, shooter, _| {
            level.rt.start_script_thread_for_test(
                "maps/mp/gametypes/_hardpoints",
                "triggerHardpoint",
                Some(shooter),
                |cx| vec![Value::String(cx.intern_exact("helicopter_mp"))],
            );
        },
        &mut each,
    )?;
    Some(RpgFlight {
        rows,
        aborts: seen.aborts,
    })
}

/// The helicopter's attractor (`missile_createAttractorEnt(chopper,
/// scr_heli_attract_strength, scr_heli_attract_range)`) bends an RPG fired
/// 20 degrees wide of it toward the body: against the same flight with
/// the strength 0 (the same wobble draws, the level's one `rand()`), the
/// rocket flies the same until its first wobble (`destabilizeDistance`
/// 300 at 1500 a second), then passes nearer (`G_RunMissile`'s
/// attractors, cod4-gsc.md 3.2).
#[test]
fn the_helicopter_pulls_a_wide_rpg_in() {
    let Some(pulled) = rpg_at_heli(&[], 20.0) else {
        return;
    };
    let plain = rpg_at_heli(&[("scr_heli_attract_strength", "0")], 20.0).unwrap();
    assert!(pulled.aborts.is_empty(), "{:?}", pulled.aborts);
    let closest = |f: &RpgFlight| {
        f.rows
            .iter()
            .map(|(o, _, mid)| o.distance(*mid))
            .fold(f32::MAX, f32::min)
    };
    if std::env::var_os("HARDPOINT_REPORT").is_some() {
        for (o, v, mid) in &pulled.rows {
            eprintln!("pulled {o:?} v {v:?} to heli {}", o.distance(*mid));
        }
        for (o, v, mid) in &plain.rows {
            eprintln!("plain {o:?} v {v:?} to heli {}", o.distance(*mid));
        }
    }
    assert!(pulled.rows.len() > 10 && plain.rows.len() > 10);
    // Straight for 200 ms at 1500 (each component of the speed truncated).
    assert_eq!(pulled.rows[..4], plain.rows[..4]);
    assert_eq!(pulled.rows[1].1, pulled.rows[0].1);
    assert!((pulled.rows[0].1.length() - 1500.0).abs() < 2.0);
    // Wobbling from then on: the velocity turns.
    assert_ne!(plain.rows[6].1, plain.rows[4].1);
    let (near, far) = (closest(&pulled), closest(&plain));
    assert!(near + 20.0 < far, "{near} {far}");
}

/// One `hind_FFAR_mp` from the helicopter's `fire_missile("ffar", 1,
/// target)` once ground 4000 ahead of it is in its sight, the allied
/// target held there (`guided` false: no target given). Each frame's
/// rocket (origin, velocity) and the target's eye.
/// A flight's frames: the rocket's origin and velocity.
type Flight = Vec<(Vec3, Vec3)>;

fn ffar(guided: bool) -> Option<(Flight, Vec3, Vec<String>)> {
    let mut rows = Vec::new();
    let mut spot: Option<Vec3> = None;
    let mut ffar_id = None;
    let mut each = |level: &mut Cod4Level, _: &mut [Probe], svs: i32| {
        let Some(h) = level.rt.host.cod4().helis.first().cloned() else {
            return;
        };
        level.rt.host.client_vitals[0].health = 100;
        if let Some(eye) = spot {
            level.set_view_pos(0, eye.to_array(), [0.0, 0.0, 0.0]);
            let c4 = level.rt.host.cod4();
            // The axis shooter's helicopter fires `hind_FFAR_mp`.
            let ffar = c4.table.index("hind_ffar_mp").map(|w| w as u32);
            let m = match ffar_id {
                Some(id) => c4.missiles.iter().find(|m| m.id == id),
                None => c4.missiles.iter().find(|m| Some(m.weapon) == ffar),
            };
            if let Some(m) = m {
                ffar_id = Some(m.id);
                rows.push((m.origin, m.pos.delta));
            }
            return;
        }
        // Over the town by then.
        if svs < CALL_MS + 20_000 {
            return;
        }
        let host = &level.rt.host;
        let tr = |a: Vec3, b: Vec3| {
            crate::game::builtins::cod4_trace::trace4(
                host,
                a,
                b,
                Vec3::ZERO,
                Vec3::ZERO,
                h.id.0,
                0x80_6831,
            )
            .map_or(1.0, |t| t.fraction)
        };
        let [f, _, _] = vcod_common::x87::angle_vectors(Vec3::new(0.0, h.angles.y, 0.0));
        let above = h.origin + f * 4000.0;
        let down = above - Vec3::Z * 3000.0;
        let f_down = tr(above, down);
        if f_down >= 1.0 {
            return;
        }
        let eye = above.lerp(down, f_down) + Vec3::Z * 61.0;
        if tr(h.origin - Vec3::Z * 100.0, eye) < 1.0 {
            return;
        }
        spot = Some(eye);
        level.set_view_pos(0, eye.to_array(), [0.0, 0.0, 0.0]);
        let target = level.rt.client_entity(0).unwrap();
        level.rt.start_script_thread_for_test(
            "maps/mp/_helicopter",
            "fire_missile",
            Some(h.id),
            |cx| {
                let mut a = vec![Value::String(cx.intern_exact("ffar")), Value::Int(1)];
                if guided {
                    a.push(Value::Entity(target));
                }
                a
            },
        );
    };
    let seen = run(
        120_000,
        &[("scr_heli_missile_max", "0")],
        &|level, shooter, _| {
            level.rt.start_script_thread_for_test(
                "maps/mp/gametypes/_hardpoints",
                "triggerHardpoint",
                Some(shooter),
                |cx| vec![Value::String(cx.intern_exact("helicopter_mp"))],
            );
        },
        &mut each,
    )?;
    Some((rows, spot?, seen.aborts))
}

/// The helicopter's FFAR (`hind_FFAR_mp`: `Sidewinder`, `timeToAccelerate`
/// 1, `maxSteeringAccel` 2000, speed 6000, curvature 10) leaves at the
/// body's velocity and gains 300 a frame along its facing, curving; from
/// a second past the shot it steers at the target it was given and passes
/// nearer it than the same rocket with none (the missile trajectory step,
/// cod4-gsc.md 3.2).
#[test]
fn the_helicopter_guides_its_rocket() {
    let Some((guided, eye, aborts)) = ffar(true) else {
        return;
    };
    let (free, eye2, _) = ffar(false).unwrap();
    assert!(aborts.is_empty(), "{aborts:?}");
    assert_eq!(eye, eye2);
    let report = std::env::var_os("HARDPOINT_REPORT").is_some();
    for (name, rows) in [("guided", &guided), ("free", &free)] {
        for (o, v) in rows.iter() {
            if report {
                eprintln!(
                    "{name} {o:?} v {v:?} |v| {} to eye {}",
                    v.length(),
                    o.distance(eye)
                );
            }
        }
    }
    assert!(guided.len() > 22, "{}", guided.len());
    // The first second: the same flight, the speed growing.
    assert_eq!(guided[..20], free[..20]);
    let speeds: Vec<f32> = guided[..20].iter().map(|r| r.1.length()).collect();
    assert!(speeds.windows(2).all(|w| w[1] > w[0] + 200.0), "{speeds:?}");
    assert!(speeds[19] > 5000.0, "{speeds:?}");
    // Then the guided one steers.
    assert_ne!(guided[21], free[21]);
    let closest = |rows: &[(Vec3, Vec3)]| {
        rows.iter()
            .map(|(o, _)| o.distance(eye))
            .fold(f32::MAX, f32::min)
    };
    let (near, far) = (closest(&guided), closest(&free));
    assert!(near < far, "{near} {far}");
}

/// The 1.7 builtin `name` (`builtins::cod4_vehicle`) on the live host.
fn call_builtin(level: &mut Cod4Level, name: &str, recv: Option<EntId>, args: Vec<Value>) {
    let f = crate::game::builtins::cod4_vehicle::NAMES
        .iter()
        .find(|(n, _)| *n == name)
        .unwrap()
        .1;
    level.rt.with_host(|h, cx| {
        f(h, cx, recv.map(vcod_gsc::Target::Entity), &args).unwrap();
    });
}

/// `physicsLaunch`, `physicsExplosionSphere` and `..Cylinder` on the running level: a
/// solid, damageable script model of the map launched with a contact
/// point and a force goes out with both trajectories `TR_PHYSICS` (8),
/// the launch time, the contact in `pos.trDelta` and the force in
/// `apos.trDelta`, and stops clipping and taking damage; the sphere's
/// event entity (`EV_PHYS_EXPLOSION_SPHERE`, 57) carries the outer radius
/// as its parm and the inner radius and the magnitude in `lerp.u` words 0
/// and 1 (cod4-gsc.md 3.2).
#[test]
fn physics_launch_and_the_explosion_sphere_go_out() {
    let mut launched = None;
    let mut each = |level: &mut Cod4Level, _: &mut [Probe], svs: i32| {
        if launched.is_some() || svs < CALL_MS + 1000 {
            return;
        }
        let at = level.pm_state(0).unwrap().origin;
        let id = level.rt.with_host(|h, cx| {
            let id = h.ents.spawn(cx).unwrap();
            let fields = [
                ("classname", Value::String(cx.intern_exact("script_model"))),
                (
                    "origin",
                    Value::Vector((at + Vec3::new(64.0, 0.0, 16.0)).to_array()),
                ),
            ];
            for (k, v) in fields {
                let k = cx.intern_folded(k);
                h.set_field(cx, id, k, v).unwrap();
            }
            h.ents.get_mut(id).unwrap().solid = true;
            h.script_takedamage.insert(id);
            id
        });
        call_builtin(
            level,
            "physicslaunch",
            Some(id),
            vec![
                Value::Vector([1.0, 2.0, 3.0]),
                Value::Vector([40.0, 0.0, 300.0]),
            ],
        );
        let host = &level.rt.host;
        assert!(!host.ents.get(id).unwrap().solid);
        assert!(!host.script_takedamage.contains(&id));
        call_builtin(
            level,
            "physicsexplosionsphere",
            None,
            vec![
                Value::Vector(at.to_array()),
                Value::Int(200),
                Value::Int(100),
                Value::Float(2.5),
            ],
        );
        call_builtin(
            level,
            "physicsexplosioncylinder",
            None,
            vec![
                Value::Vector(at.to_array()),
                Value::Int(250),
                Value::Int(50),
                Value::Float(1.5),
            ],
        );
        launched = Some((id, level.level_time()));
    };
    let Some(seen) = run(CALL_MS + 4000, &[], &|_, _, _| {}, &mut each) else {
        return;
    };
    let (id, time) = launched.unwrap();
    let ent = seen
        .snaps
        .iter()
        .flat_map(|f| f.entities.iter())
        .find(|e| e.number() == id.0)
        .copied()
        .expect("the launched model reaches the target");
    assert_eq!(word(&ent, "lerp.pos.trType"), 8);
    assert_eq!(word(&ent, "lerp.apos.trType"), 8);
    assert_eq!(word(&ent, "lerp.pos.trTime"), time as u32);
    assert_eq!(f32::from_bits(word(&ent, "lerp.pos.trDelta[2]")), 3.0);
    assert_eq!(f32::from_bits(word(&ent, "lerp.apos.trDelta[2]")), 300.0);
    let sphere = seen
        .snaps
        .iter()
        .flat_map(|f| f.entities.iter())
        .find(|e| e.etype() == 17 + 57)
        .copied()
        .expect("the sphere's event reaches the target");
    assert_eq!(word(&sphere, "eventParm"), 200);
    assert_eq!(
        f32::from_bits(word(&sphere, "lerp.u.anonymous.data[0]")),
        100.0
    );
    assert_eq!(
        f32::from_bits(word(&sphere, "lerp.u.anonymous.data[1]")),
        2.5
    );
    let cylinder = seen
        .snaps
        .iter()
        .flat_map(|f| f.entities.iter())
        .find(|e| e.etype() == 17 + 58)
        .copied()
        .expect("the cylinder's event reaches the target");
    assert_eq!(word(&cylinder, "eventParm"), 250);
    assert_eq!(
        f32::from_bits(word(&cylinder, "lerp.u.anonymous.data[0]")),
        50.0
    );
    assert_eq!(
        f32::from_bits(word(&cylinder, "lerp.u.anonymous.data[1]")),
        1.5
    );
}
