//! mp_backlot `sab`'s destructible sedans (`_destructible.gsc`) blown up
//! by `radiusDamage` blasts beside a joined player, in process: each
//! part's break plays its sound through `_utility.gsc`'s `play_sound_on_tag`, which `linkTo`s a
//! script origin to the car's `tag_glass_*_d` and `tag_light_*` bones, and
//! the scripts run on without an error (cod4-gsc.md 9.4). Needs `COD4_DIR`.

use std::time::{Duration, Instant};

use glam::Vec3;
use vcod_gsc::{EntId, Host, Value};

use super::wire_ab::Probe;
use super::*;
use crate::game::cod4::level::Cod4Level;

/// The destructible sedans on the level: (entity, origin).
fn sedans(level: &mut Cod4Level) -> Vec<(EntId, Vec3)> {
    level.rt.with_host(|h, cx| {
        let ids: Vec<EntId> = h.ents.iter_inuse().map(|(id, _)| id).collect();
        let (m, o) = (cx.intern_folded("model"), cx.intern_folded("origin"));
        ids.into_iter()
            .filter_map(|id| {
                let Value::String(s) = h.get_field(cx, id, m) else {
                    return None;
                };
                let name = cx.resolve(s);
                if !(name.contains("sedan1") && name.ends_with("_destructible_mp")) {
                    return None;
                }
                match h.get_field(cx, id, o) {
                    Value::Vector(o) => Some((id, Vec3::from(o))),
                    _ => None,
                }
            })
            .collect()
    })
}

/// The stock scripts' own blast, `_destructible.gsc`'s explosion and
/// `_minefields.gsc`'s: `radiusDamage` notifies `damage` on each
/// destructible in reach with the blast's point.
const BLAST: &str = "vc_blast() { radiusDamage(level.vc_blast_at, 256, 300, 150); }";

#[test]
fn backlot_sedans_break_without_tag_errors() {
    let Some(install) = vcod_cod4::testing::cod4_install() else {
        return;
    };
    let mut sv = Cod4Server::new(
        Cod4Config {
            map: "mp_backlot".into(),
            gametype: "sab".into(),
            hostname: "vcod".into(),
            max_clients: 4,
        },
        7,
    );
    sv.set_cvar("sv_pure", "0");
    sv.load(Some(&install)).unwrap();
    let mut probe = Probe::new(
        "bomber",
        0,
        28971,
        5_600,
        &[
            (1_000, "mr {sid} 8 allies"),
            (2_500, "mr {sid} 15 assault_mp,0"),
        ],
    );
    let cars = sedans(sv.game.level.as_mut().unwrap());
    assert!(cars.len() >= 2, "mp_backlot's sedans: {cars:?}");
    sv.game.level.as_mut().unwrap().rt.install_for_test(BLAST);
    // A blast on the next car every second from 15 s on, each car twice.
    const BLAST_EVERY: i32 = 1_000;
    let (from, until) = (15_000, 15_000 + 2 * BLAST_EVERY * cars.len() as i32);
    let mut now = Instant::now();
    while sv.core.sv_time_ms < until + 6_000 {
        let svs = sv.core.sv_time_ms;
        if !probe.joined && svs >= probe.connect_at {
            probe.join(&mut sv, now);
        }
        if probe.joined {
            let pkt = probe.packet(svs);
            sv.handle_packet(probe.addr, &pkt, now);
        }
        if (from..until).contains(&svs) && (svs - from) % BLAST_EVERY == 0 {
            let level = sv.game.level.as_mut().unwrap();
            let at = cars[((svs - from) / BLAST_EVERY) as usize % cars.len()].1;
            let at = at + Vec3::Z * 40.0;
            level
                .rt
                .set_level_field_for_test("vc_blast_at", Value::Vector(at.to_array()));
            let shooter = level.rt.client_entity(0).expect("the probe is in");
            level.rt.start_thread_for_test(shooter, "vc_blast");
        }
        now += Duration::from_millis(FRAME_MS as u64);
        sv.tick_frames(now, 1);
        let svs = sv.core.sv_time_ms;
        for (to, pkt) in sv.take_outgoing() {
            if to == probe.addr {
                probe.receive(svs, &pkt);
            }
        }
    }
    let level = sv.game.level.as_mut().unwrap();
    let aborts = level.rt.aborts();
    assert!(aborts.is_empty(), "script errors:\n{}", aborts.join("\n"));
    // The cars did break: the script swaps each one's model when it dies.
    let left = sedans(level);
    assert!(left.len() < cars.len(), "no sedan broke: {left:?}");
}

/// A joined player stood beside one of mp_backlot's sedans (`war`) holds
/// the trigger on it, the view on the middle of its body: each round
/// takes `G_Damage`'s entity arm, the script's `damage` notify, and the
/// car burns and blows (its model swapped for the wreck) with no script
/// error (cod4-combat.md 13). Needs `COD4_DIR`.
#[test]
fn rounds_shoot_a_backlot_sedan_until_it_blows() {
    let Some(install) = vcod_cod4::testing::cod4_install() else {
        return;
    };
    let mut sv = Cod4Server::new(
        Cod4Config {
            map: "mp_backlot".into(),
            gametype: "war".into(),
            hostname: "vcod".into(),
            max_clients: 4,
        },
        7,
    );
    sv.set_cvar("sv_pure", "0");
    sv.load(Some(&install)).unwrap();
    let mut probe = Probe::new(
        "shooter",
        0,
        28971,
        5_600,
        &[
            (1_000, "mr {sid} 8 allies"),
            (2_500, "mr {sid} 15 assault_mp,0"),
        ],
    );
    let (car, at) = sedans(sv.game.level.as_mut().unwrap())[0];
    let target = at + Vec3::Z * 30.0;
    let mut now = Instant::now();
    let mut stood: Option<i32> = None;
    let mut blown = None;
    while sv.core.sv_time_ms < 120_000 && blown.is_none() {
        let svs = sv.core.sv_time_ms;
        if !probe.joined && svs >= probe.connect_at {
            probe.join(&mut sv, now);
        }
        let level = sv.game.level.as_mut().unwrap();
        let live = level.pm_state(0).is_some_and(|ps| {
            ps.pm_type == 0 && ps.pm_flags & vcod_cod4::pmove::pmf::FROZEN == 0 && ps.weapon != 0
        });
        if live && stood.is_none() {
            // A spot 160 off the car with a clear line to its middle and
            // a floor under it.
            let eye = (0..16)
                .map(|i| (i as f32 * 22.5).to_radians())
                .map(|a| target + Vec3::new(a.cos(), a.sin(), 0.0) * 160.0 + Vec3::Z * 30.0)
                .find(|&e| {
                    level.shot_trace_for_test(e, target).fraction >= 1.0
                        && level.shot_trace_for_test(e, e - Vec3::Z * 140.0).fraction < 1.0
                })
                .expect("a clear spot beside the car");
            level.set_view_pos(0, eye.to_array(), [0.0; 3]);
            stood = Some(svs);
        }
        if let Some(t) = stood
            && svs - t > 1_000
        {
            // Pulsed: the class's rifle may be semi-automatic.
            let fire = if (svs / 50) % 2 == 0 { 1 } else { 0 };
            if let Some(c) = super::damage_soak::aim_cmd(level, 0, Some(target), fire, svs, 0) {
                probe.replay.clear();
                probe.replay.push_back((svs, vec![c]));
            }
        }
        if probe.joined {
            let pkt = probe.packet(svs);
            sv.handle_packet(probe.addr, &pkt, now);
        }
        now += Duration::from_millis(FRAME_MS as u64);
        sv.tick_frames(now, 1);
        let svs = sv.core.sv_time_ms;
        for (to, pkt) in sv.take_outgoing() {
            if to == probe.addr {
                probe.receive(svs, &pkt);
            }
        }
        let level = sv.game.level.as_mut().unwrap();
        if !sedans(level).iter().any(|&(id, _)| id == car) {
            blown = Some(svs);
        }
    }
    let level = sv.game.level.as_mut().unwrap();
    let aborts = level.rt.aborts();
    assert!(aborts.is_empty(), "script errors:\n{}", aborts.join("\n"));
    let stood = stood.expect("the shooter spawned");
    let blown = blown.expect("the sedan blew");
    assert!(blown - stood < 60_000, "blew {} ms on", blown - stood);
}
