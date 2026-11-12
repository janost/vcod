//! Grenade arms the A/B gates never reach, pinned on ours: a launcher
//! dud coming to rest (`EV_CHANGE_TO_DUD`), a frag bouncing off a body
//! as off flesh, a smoke grenade whose fuse runs out in the air, a
//! claymore's damage cone, a flashbang's shares, and a frag's blast on
//! mp_backlot's destructible sedans (`CanDamage` on an entity). The kill
//! pair's join
//! (`combat_ab::ours_hooked`) with the shooter held still; needs
//! `COD4_DIR` (docs/research/cod4-combat.md 10.2, 10.3, 13).

use std::time::{Duration, Instant};

use glam::Vec3;
use vcod_gsc::{EntId, Value};

use super::combat_ab::{kill, ours_hooked};
use super::wire_ab::{Probe, side};
use super::*;
use crate::game::cod4::level::Cod4Level;
use crate::game::cod4::missile::ev;

/// When the act starts, past both spawns and the prematch.
const ACT_MS: i32 = 47_450;

/// Runs the kill pair with the shooter (slot 1) holding still from its
/// spawn and `act` called on every tick from [`ACT_MS`] on.
fn run(act: &mut dyn FnMut(&mut Cod4Level, i32)) -> Option<()> {
    let install = vcod_cod4::testing::cod4_install()?;
    let rt = side(&kill("target"));
    let cs0 = String::from_utf8_lossy(rt.gs.configstring(0)).into_owned();
    let mut hook = |sv: &mut Cod4Server, probes: &mut [Probe], svs: i32| {
        let level = sv.game.level.as_mut().unwrap();
        if svs >= 46_900
            && let Some(ps) = level.pm_state(1)
        {
            let short = |a: f32| ((a * 65536.0 / 360.0).round() as i32 & 0xffff) as u16;
            let c = UserCmd {
                server_time: svs + 10,
                weapon: ps.weapon as u8,
                offhand: ps.off_hand_index as u8,
                angles: std::array::from_fn(|k| {
                    short(ps.view_angles[k]).wrapping_sub(short(ps.delta_angles[k]))
                }),
                ..Default::default()
            };
            probes[1].replay.clear();
            probes[1].replay.push_back((svs, vec![c]));
        }
        if svs >= ACT_MS {
            act(level, svs);
        }
    };
    let (_, aborts, _) = ours_hooked(&install, &cs0, None, &[], &mut hook);
    assert!(aborts.is_empty(), "{aborts:?}");
    Some(())
}

/// One missile's state on a tick: `(time, eventSequence, events, parms,
/// trType, ground, origin)`.
type Seen = (i32, u32, [u32; 4], [u32; 4], u32, u32, [i32; 3]);

fn seen(level: &Cod4Level, id: EntId, svs: i32) -> Option<Seen> {
    let m = level.rt.host.cod4().missiles.iter().find(|m| m.id == id)?;
    Some((
        svs,
        m.event_seq,
        m.events,
        m.parms,
        m.pos.tr_type,
        m.ground,
        m.origin.to_array().map(|v| (v * 8.0).round() as i32),
    ))
}

/// Fires `weapon` from the shooter at `ACT_MS`, `from` the target's
/// origin moving at `vel`, and logs the missile every tick until it is
/// gone, with the target's health.
fn flight(weapon: &str, from: Vec3, vel: Vec3, fuse_ms: i32) -> Option<(Vec<Seen>, Vec<i32>)> {
    let mut id = None;
    let mut log = Vec::new();
    let mut health = Vec::new();
    run(&mut |level, svs| {
        if svs == ACT_MS {
            let ps = level.pm_state(0).unwrap();
            id = level.fire_grenade_for_test(1, weapon, ps.origin + from, vel, fuse_ms);
            assert!(id.is_some(), "{weapon}");
        }
        if let Some(s) = id.and_then(|id| seen(level, id, svs)) {
            log.push(s);
            health.push(level.rt.client_vitals(0).health);
        }
    })?;
    Some((log, health))
}

/// The distinct event rings a log went through.
fn rings(log: &[Seen]) -> Vec<(u32, [u32; 4], [u32; 4])> {
    let mut out: Vec<(u32, [u32; 4], [u32; 4])> = Vec::new();
    for s in log {
        let r = (s.1, s.2, s.3);
        if out.last() != Some(&r) {
            out.push(r);
        }
    }
    out
}

/// The M16's launcher grenade dropped onto the floor inside its arming
/// distance bounces as a dud, comes to rest and goes as `EV_CHANGE_TO_DUD`
/// (51) with the floor's normal, then is freed 300 ms on.
#[test]
fn a_dud_at_rest_changes_to_a_dud() {
    let Some((log, _)) = flight(
        "gl_m16_mp",
        Vec3::new(80.0, 0.0, 60.0),
        Vec3::new(0.0, 0.0, -300.0),
        0,
    ) else {
        return;
    };
    let rings = rings(&log);
    let last = rings.last().unwrap();
    assert_eq!(
        last.1[(last.0 as usize + 3) % 4],
        ev::CHANGE_TO_DUD,
        "{rings:?}"
    );
    assert_eq!(format!("{rings:?}"), DUD_RINGS, "{log:?}");
}

const DUD_RINGS: &str = "[(0, [0, 0, 0, 0], [0, 0, 0, 0]), (1, [51, 0, 0, 0], [5, 0, 0, 0])]";

/// A frag thrown into the target's chest bounces off him as off flesh:
/// `EV_GRENADE_BOUNCE` with surface type 7.
#[test]
fn a_frag_bounces_off_a_body_as_off_flesh() {
    let Some((log, health)) = flight(
        "frag_grenade_mp",
        Vec3::new(-60.0, 0.0, 40.0),
        Vec3::new(600.0, 0.0, 0.0),
        3_000,
    ) else {
        return;
    };
    let rings = rings(&log);
    assert!(
        rings
            .iter()
            .any(|r| (0..4).any(|k| r.1[k] == ev::GRENADE_BOUNCE && r.2[k] == 7)),
        "{rings:?}"
    );
    let first: Vec<_> = log.iter().take(12).collect();
    assert_eq!(
        format!("{first:?} {:?}", &health[..12.min(health.len())]),
        FLESH_LOG,
        "{log:?}"
    );
}

const FLESH_LOG: &str = "[(47450, 0, [0, 0, 0, 0], [0, 0, 0, 0], 5, 1023, [1176, 18328, 2209]), (47500, 0, [0, 0, 0, 0], [0, 0, 0, 0], 5, 1023, [1416, 18328, 2201]), (47550, 1, [44, 0, 0, 0], [7, 0, 0, 0], 5, 1023, [1574, 18328, 2185]), (47600, 1, [44, 0, 0, 0], [7, 0, 0, 0], 5, 1023, [1545, 18366, 2170]), (47650, 1, [44, 0, 0, 0], [7, 0, 0, 0], 5, 1023, [1516, 18404, 2139]), (47700, 1, [44, 0, 0, 0], [7, 0, 0, 0], 5, 1023, [1487, 18442, 2092]), (47750, 1, [44, 0, 0, 0], [7, 0, 0, 0], 5, 1023, [1458, 18480, 2029]), (47800, 1, [44, 0, 0, 0], [7, 0, 0, 0], 5, 1023, [1428, 18518, 1950]), (47850, 2, [44, 44, 0, 0], [7, 5, 0, 0], 5, 1022, [1410, 18542, 1901]), (47900, 2, [44, 44, 0, 0], [7, 5, 0, 0], 5, 1023, [1401, 18553, 1919]), (47950, 2, [44, 44, 0, 0], [7, 5, 0, 0], 5, 1023, [1393, 18564, 1921]), (48000, 2, [44, 44, 0, 0], [7, 5, 0, 0], 5, 1023, [1384, 18575, 1906])] [100, 100, 85, 85, 85, 85, 85, 85, 85, 85, 85, 85]";

/// A smoke grenade whose fuse runs out in the air waits a frame at a time
/// (`nextthink` 50) and bursts only once it lies on something: its cloud
/// carries `eFlags` 0x90020 and lives 61 s.
#[test]
fn a_smoke_grenade_bursts_only_on_the_ground() {
    let mut id = None;
    let mut grounds = Vec::new();
    let mut clouds = Vec::new();
    let ran = run(&mut |level, svs| {
        if svs == ACT_MS {
            let ps = level.pm_state(0).unwrap();
            id = level.fire_grenade_for_test(
                1,
                "smoke_grenade_mp",
                ps.origin + Vec3::new(-80.0, 0.0, 20.0),
                Vec3::new(-100.0, 0.0, 150.0),
                1,
            );
            assert!(id.is_some());
        }
        if let Some(s) = id.and_then(|id| seen(level, id, svs)) {
            grounds.push((svs, s.5, s.6));
        }
        for b in &level.rt.host.cod4().bursts {
            if !clouds
                .iter()
                .any(|c: &(i32, u32, u32, Option<i32>)| c.0 == b.time)
            {
                clouds.push((b.time, b.event, b.e_flags, b.smoke_until));
            }
        }
    });
    if ran.is_none() {
        return;
    }
    assert_eq!(clouds.len(), 1, "{clouds:?}");
    let cloud = clouds[0];
    assert_eq!(cloud.2, 0x9_0020, "{clouds:?}");
    assert_eq!(cloud.3, Some(cloud.0 + 61_000), "{clouds:?}");
    // In the air (ground 1023) on every tick before the burst.
    assert!(
        grounds
            .iter()
            .filter(|g| g.0 < cloud.0)
            .all(|g| g.1 == 1023),
        "{grounds:?}"
    );
    assert_eq!(format!("{cloud:?} {grounds:?}"), SMOKE_CLOUD);
}

const SMOKE_CLOUD: &str = "(47950, 49, 589856, Some(108950)) [(47450, 1023, [1016, 18328, 2049]), (47500, 1023, [976, 18328, 2101]), (47550, 1023, [936, 18328, 2137]), (47600, 1023, [896, 18328, 2157]), (47650, 1023, [856, 18328, 2161]), (47700, 1023, [816, 18328, 2149]), (47750, 1023, [776, 18328, 2121]), (47800, 1023, [736, 18328, 2077]), (47850, 1023, [696, 18328, 2017]), (47900, 1023, [656, 18328, 1941])]";

/// The splash a claymore planted at `off` from the target's origin hands
/// each player when it bursts a frame later, `(victim, damage)`, and the
/// target's health after. Planted with no move, it faces straight down
/// (`vectoangles` of a zero vector reads pitch 90). [`run`] asserts no
/// script aborted.
fn claymore(off: Vec3) -> Option<(Vec<(usize, i32)>, i32)> {
    let mut got = Vec::new();
    let mut id = None;
    let mut health = 0;
    run(&mut |level, svs| {
        if svs == ACT_MS {
            let ps = level.pm_state(0).unwrap();
            id = level.rest_grenade_for_test(1, "claymore_mp", ps.origin + off, 0);
            assert!(id.is_some());
        }
        if svs == ACT_MS + 50 {
            let queued = level.combat_splashes().len();
            assert!(level.explode_for_test(id.unwrap()));
            got = level.combat_splashes()[queued..].to_vec();
            health = level.rt.client_vitals(0).health;
        }
    })?;
    Some((got, health))
}

/// A claymore's blast reaches only inside its `damageConeAngle` (60):
/// the target under it takes the falloff times how many of the five
/// points fall inside the cone, one beside it none.
#[test]
fn a_claymore_blasts_only_inside_its_cone() {
    let offsets = [
        Vec3::new(0.0, 0.0, 120.0),
        Vec3::new(0.0, 60.0, 120.0),
        Vec3::new(0.0, 150.0, 100.0),
        Vec3::new(0.0, 30.0, 120.0),
        Vec3::new(0.0, -40.0, 120.0),
        Vec3::new(40.0, 0.0, 120.0),
        Vec3::new(-40.0, 0.0, 120.0),
        Vec3::new(0.0, 0.0, 70.0),
        Vec3::new(20.0, 20.0, 60.0),
        Vec3::new(-225.0, 0.0, 10.0),
    ];
    let mut got = Vec::new();
    for off in offsets {
        let Some((splash, _)) = claymore(off) else {
            return;
        };
        got.push(splash);
    }
    assert_eq!(format!("{got:?}"), CLAYMORE);
}

const CLAYMORE: &str = "[[(0, 129), (1, 42)], [], [], [(0, 127)], [(0, 125), (1, 53)], [(0, 125)], [(0, 125), (1, 46)], [(0, 158)], [(0, 161)], []]";

/// The `flashbang` notifies a flashbang bursting at each offset from the
/// target's origin raises, `(player entity, distance share, view share,
/// thrower)`: inside `explosionRadiusMin` (200) the share is 1, past it
/// it falls toward the outer 800, and the view share is how much the eye
/// faces the burst.
#[test]
fn a_flashbang_shares_out_by_distance_and_view() {
    let offsets = [
        Vec3::new(-100.0, 0.0, 50.0),
        Vec3::new(-350.0, 0.0, 50.0),
        Vec3::new(-600.0, 20.0, 60.0),
        Vec3::new(0.0, -120.0, 40.0),
        Vec3::new(60.0, 0.0, 20.0),
    ];
    let mut got = Vec::new();
    let ran = run(&mut |level, svs| {
        let k = (svs - ACT_MS) / 1_000;
        if (svs - ACT_MS) % 1_000 != 0 || k as usize >= offsets.len() {
            return;
        }
        let ps = level.pm_state(0).unwrap();
        let queued = level.rt.host.engine_notifies.len();
        assert!(level.burst_for_test(1, "flash_grenade_mp", ps.origin + offsets[k as usize]));
        for (id, name, args) in &level.rt.host.engine_notifies[queued..] {
            if *name != "flashbang" {
                continue;
            }
            let f = |v: &Value| match v {
                Value::Float(f) => format!("{f}"),
                Value::Entity(e) => format!("e{}", e.0),
                other => format!("{other:?}"),
            };
            got.push((k, id.0, f(&args[0]), f(&args[1]), f(&args[2])));
        }
    });
    if ran.is_none() {
        return;
    }
    assert_eq!(format!("{got:?}"), FLASH);
}

const FLASH: &str = "[(0, 0, \"1\", \"0.5\", \"e1\"), (0, 1, \"1\", \"0.4560393\", \"e1\"), (1, 0, \"0.7440777\", \"0.5\", \"e1\"), (2, 0, \"0.32779318\", \"0.5166574\", \"e1\"), (3, 0, \"1\", \"0.49990538\", \"e1\"), (3, 1, \"1\", \"0.89727086\", \"e1\")]";

/// mp_backlot's sedans: (entity, origin).
fn sedans(level: &mut Cod4Level) -> Vec<(EntId, Vec3)> {
    level.rt.with_host(|h, cx| {
        let ids: Vec<EntId> = h.ents.iter_inuse().map(|(id, _)| id).collect();
        let (m, o) = (cx.intern_folded("model"), cx.intern_folded("origin"));
        ids.into_iter()
            .filter_map(|id| {
                let Value::String(s) = vcod_gsc::Host::get_field(h, cx, id, m) else {
                    return None;
                };
                let name = cx.resolve(s);
                if !(name.contains("sedan1") && name.ends_with("_destructible_mp")) {
                    return None;
                }
                match vcod_gsc::Host::get_field(h, cx, id, o) {
                    Value::Vector(o) => Some((id, Vec3::from(o))),
                    _ => None,
                }
            })
            .collect()
    })
}

/// Frags bursting around one of mp_backlot's sedans, one a second: the
/// `damage` each raises on a script entity, `(burst, entity, damage)`.
/// `CanDamage` on the car's bounds scales each by the share of its five
/// traces through.
#[test]
fn frags_blast_a_sedan_by_its_bounds() {
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
        "bomber",
        0,
        28971,
        5_600,
        &[
            (1_000, "mr {sid} 8 allies"),
            (2_500, "mr {sid} 15 assault_mp,0"),
        ],
    );
    let (_, car) = sedans(sv.game.level.as_mut().unwrap())[0];
    let offsets = [
        Vec3::new(-120.0, 0.0, 30.0),
        Vec3::new(0.0, 130.0, 20.0),
        Vec3::new(70.0, -70.0, 90.0),
        Vec3::new(0.0, 0.0, 160.0),
        Vec3::new(30.0, 220.0, 10.0),
    ];
    let mut hits = Vec::new();
    let mut now = Instant::now();
    const FROM: i32 = 15_000;
    while sv.core.sv_time_ms < FROM + 1_000 * offsets.len() as i32 + 500 {
        let svs = sv.core.sv_time_ms;
        if !probe.joined && svs >= probe.connect_at {
            probe.join(&mut sv, now);
        }
        if probe.joined {
            let pkt = probe.packet(svs);
            sv.handle_packet(probe.addr, &pkt, now);
        }
        if svs >= FROM && (svs - FROM) % 1_000 == 0 {
            let k = ((svs - FROM) / 1_000) as usize;
            if let Some(&off) = offsets.get(k) {
                let level = sv.game.level.as_mut().unwrap();
                let queued = level.rt.host.engine_notifies.len();
                assert!(level.burst_for_test(0, "frag_grenade_mp", car + off));
                for (id, name, args) in &level.rt.host.engine_notifies[queued..] {
                    if *name == "damage"
                        && let Some(Value::Int(n)) = args.first()
                    {
                        hits.push((k, id.0, *n));
                    }
                }
            }
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
    assert!(!hits.is_empty());
    assert_eq!(format!("{hits:?}"), SEDAN_HITS);
}

const SEDAN_HITS: &str = "[(0, 125, 191), (1, 125, 184), (2, 125, 182)]";

/// A claymore kill at full health runs `Callback_PlayerKilled` clean: the
/// planted claymore's `owner` (`_weapons.gsc` `watchClaymores`) lets
/// `Callback_PlayerDamage` fill `explosiveInfo["bulletPenetrationKill"]`,
/// which `_missions.gsc` `ch_kills` reads (cod4-combat.md 13).
#[test]
fn a_claymore_kill_runs_the_kill_scripts_clean() {
    let Some((splash, health)) = claymore(Vec3::new(0.0, 0.0, 70.0)) else {
        return;
    };
    assert!(
        splash.iter().any(|&(v, d)| v == 0 && d >= 100),
        "{splash:?}"
    );
    assert!(health <= 0, "{health}");
}
