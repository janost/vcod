//! Corpses and physics on a live server: ragdolls, car parts and the
//! map's clutter.

use std::collections::BTreeMap;

use vcod_cod2::player::{Character, eflags as cod2_eflags};
use vcod_cod4::net::conn::Cp;

use super::*;
use crate::play::cod4_menus::Join as MenuJoin;

/// vcod-server's `war` with eight shooting bots, watched as a
/// spectator and posed through `play::cod4_players` each frame: a
/// corpse keeps the models it was copied with while its client
/// respawns and changes its stowed weapons (`CG_ResetEntity`'s
/// `CG_CopyCorpseInfo`), and the server's `BodyEnd` takes its fresh
/// bit out, so nothing replays its death anim (cod4-player-anims.md
/// 9). `None` without an install.
#[test]
fn cod4_live_corpses_keep_their_models_and_go_stale() {
    use crate::play::cod4_players::Cod4Players;
    use vcod_server::cod4::{Cod4Config, Cod4Server};
    let Some(install) = vcod_cod4::testing::cod4_install() else {
        return;
    };
    let root = install.root().to_path_buf();
    let world = crash_world();
    let _live = live_slot();
    let mut sv = Cod4Server::new(
        Cod4Config {
            map: "mp_crash".into(),
            gametype: "war".into(),
            hostname: "vcod".into(),
            max_clients: 12,
        },
        7,
    );
    sv.set_cvar("scr_game_playerwaittime", "1");
    sv.set_cvar("scr_game_matchstarttime", "1");
    sv.load(Some(&install)).unwrap();
    sv.add_bots(8, true);
    let sock = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
    sock.set_nonblocking(true).unwrap();
    let addr = sock.local_addr().unwrap();
    let clock = sim::Clock::new();
    let client = sim::spawn(&clock, move || {
        let join = MenuJoin {
            team: Some("spectator".into()),
            class: Some("assault_mp,0".into()),
        };
        let mut s = Cod4Session::connect(addr, "watcher", Cp::Bare, join).unwrap();
        if let Some(w) = world {
            s.set_world(w);
        }
        let mut players = Cod4Players::default();
        let mut loaded = None;
        // Per corpse (number, client, teleport bit): the models it
        // was first posed with, and whether its fresh bit went out.
        let mut corpses: BTreeMap<(u32, i32, i32), (Character, bool)> = BTreeMap::new();
        let mut changed = Vec::new();
        let t0 = clk_now();
        while since(t0) < Duration::from_secs(45) {
            let now = since(t0).as_secs_f64() * 1000.0;
            s.step(now).unwrap();
            s.take_commands();
            clk_sleep(Duration::from_millis(16));
            let Some(scene) = s.scene() else { continue };
            if loaded.is_none() {
                loaded = Some(
                    crate::cod4_fly::load_bodies(&root, "mp_crash", &s.entity_assets()).unwrap(),
                );
            }
            let (_, weapons, src) = loaded.as_ref().unwrap();
            let views: Vec<(u32, &BodyView)> = scene
                .entities
                .iter()
                .filter_map(|d| Some((d.number, d.body.as_ref()?)))
                .collect();
            players.posed(
                src,
                weapons,
                &views,
                &[],
                scene.render_time,
                scene.server_time,
            );
            for (n, v) in views.iter().filter(|(_, v)| v.corpse) {
                let Some(c) = players.character(*n) else {
                    continue;
                };
                let key = (*n, v.frame.client_num, v.frame.eflags & 2);
                let stale = v.frame.eflags & cod2_eflags::FRESH_CORPSE == 0;
                let e = corpses.entry(key).or_insert((c.clone(), false));
                e.1 |= stale;
                if e.0 != *c {
                    changed.push(format!("{key:?}: {:?} -> {c:?}", e.0));
                    e.0 = c.clone();
                }
            }
        }
        s.disconnect();
        (corpses, changed)
    });
    serve(&mut sv, &sock, &clock, &client, |_, _, _| true);
    let (corpses, changed) = client.join().unwrap();
    println!("{} corpses", corpses.len());
    assert!(corpses.len() >= 3, "{corpses:?}");
    assert!(changed.is_empty(), "corpse models changed: {changed:#?}");
    assert!(
        corpses.values().filter(|(_, stale)| *stale).count() >= corpses.len() / 2,
        "fresh bits never went out: {corpses:?}"
    );
}

/// Up to `n` spots of open, level floor on the map, searched on a
/// 64-unit grid round each `mp_tdm_spawn` (in the map's order, nearest
/// first): floor flat 48 units every way, clear of the world 48 units
/// round and 72 up, 160 from another spot. Each is the floor point.
fn open_floor_spots(
    clip: &vcod_cod4::collision::ClipMap,
    map: &vcod_cod4::map::Map,
    n: usize,
) -> Vec<Vec3> {
    use vcod_cod4::ragdoll::CLIP_MASK;
    let z = Vec3::ZERO;
    let floor = |p: Vec3| {
        let t = clip.trace(p + Vec3::Z * 16.0, p - Vec3::Z * 64.0, z, z, CLIP_MASK);
        (t.fraction < 1.0 && !t.startsolid && t.normal.z > 0.99)
            .then(|| p + Vec3::Z * 16.0 - Vec3::Z * 80.0 * t.fraction)
    };
    let mut grid: Vec<Vec3> = (-4..=4)
        .flat_map(|i| (-4..=4).map(move |j| Vec3::new(i as f32, j as f32, 0.0) * 64.0))
        .collect();
    grid.sort_by(|a, b| a.length().total_cmp(&b.length()));
    let mut out: Vec<Vec3> = Vec::new();
    for e in &map.entities {
        let get = |key: &str| e.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str());
        if get("classname") != Some("mp_tdm_spawn") {
            continue;
        }
        let Some(o) = get("origin").and_then(vcod_common::bsp::parse_vec3) else {
            continue;
        };
        for g in &grid {
            let Some(f) = floor(Vec3::from(o) + *g) else {
                continue;
            };
            let level = [Vec3::X, -Vec3::X, Vec3::Y, -Vec3::Y]
                .iter()
                .all(|d| floor(f + *d * 48.0).is_some_and(|g| (g.z - f.z).abs() < 1.0));
            let room = Vec3::new(48.0, 48.0, 72.0);
            let clear = !clip
                .trace(
                    f + Vec3::Z * 2.0,
                    f + Vec3::Z * 2.0,
                    -room.with_z(0.0),
                    room,
                    CLIP_MASK,
                )
                .startsolid;
            if level && clear && out.iter().all(|s| s.distance(f) > 160.0) {
                out.push(f);
                break;
            }
        }
        if out.len() == n {
            break;
        }
    }
    out
}

/// vcod-server's `war` with eight bots held still, watched as a
/// spectator and posed through `play::cod4_players` with the level's
/// ragdoll: every 3 s one bot is put on a spot of open, level floor
/// ([`open_floor_spots`]) and dies there by `kill`, so where a corpse
/// falls is the test's, not a fight's. Each corpse whose entity takes
/// `startRagdoll`'s type runs its ragdoll, which goes to sleep before
/// `ragdoll_max_life` (or, for under half of them, freezes there),
/// every capsule clear of the world but for the layer its contacts sink
/// into, each that slept resting on what is under it, every def bone's
/// segment clear (cod4-player-anims.md 10, 10.7).
/// Returns at once without an install.
#[test]
fn cod4_live_corpses_ragdoll_onto_the_floor() {
    use crate::play::cod4_players::{Cod4Players, RagdollWorld};
    use vcod_cod4::ragdoll::{CLIP_MASK, Dvars, State};
    use vcod_server::cod4::{Cod4Config, Cod4Server};
    let Some(install) = vcod_cod4::testing::cod4_install() else {
        return;
    };
    let root = install.root().to_path_buf();
    let world = crash_world().expect("mp_crash's world");
    let def = world.ragdoll.clone().expect("ragdoll.cfg's def");
    let (map, _) = vcod_cod4::map::Map::load(&install, "english", "mp_crash").unwrap();
    let spots = open_floor_spots(&world.clip, &map, 8);
    assert!(spots.len() >= 6, "{spots:?}");
    let _live = live_slot();
    let mut sv = Cod4Server::new(
        Cod4Config {
            map: "mp_crash".into(),
            gametype: "war".into(),
            hostname: "vcod".into(),
            max_clients: 12,
        },
        11,
    );
    sv.set_cvar("scr_game_playerwaittime", "1");
    sv.set_cvar("scr_game_matchstarttime", "1");
    sv.set_cvar("sv_cheats", "1");
    sv.load(Some(&install)).unwrap();
    let bots: Vec<usize> = (0..spots.len())
        .map(|k| sv.add_held_bot_for_test(k % 2 == 1).unwrap())
        .collect();
    let sock = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
    sock.set_nonblocking(true).unwrap();
    let addr = sock.local_addr().unwrap();
    let clip = world.clip.clone();
    let clock = sim::Clock::new();
    let client = sim::spawn(&clock, move || {
        let join = MenuJoin {
            team: Some("spectator".into()),
            class: Some("assault_mp,0".into()),
        };
        let mut s = Cod4Session::connect(addr, "watcher", Cp::Bare, join).unwrap();
        s.set_world(world.clone());
        let mut players = Cod4Players::default();
        players.ragdoll = Some(RagdollWorld {
            def,
            clip: world.clip.clone(),
            dvars: Dvars::default(),
        });
        let mut loaded = None;
        // Per corpse (number, client, teleport bit): its ragdoll's state,
        // the ms it simulated and its capsules on its last step, and
        // whether its entity ever read a ragdoll type or was refused.
        type Seen = (Option<(State, i32, Vec<(Vec3, Vec3, f32)>)>, bool, bool);
        let mut corpses: BTreeMap<(u32, i32, i32), Seen> = BTreeMap::new();
        let t0 = clk_now();
        while since(t0) < Duration::from_secs(40) {
            let now = since(t0).as_secs_f64() * 1000.0;
            s.step(now).unwrap();
            s.take_commands();
            clk_sleep(Duration::from_millis(16));
            let Some(scene) = s.scene() else { continue };
            if loaded.is_none() {
                let assets = s.entity_assets();
                loaded = Some(crate::cod4_fly::load_bodies(&root, "mp_crash", &assets).unwrap());
            }
            let (_, weapons, src) = loaded.as_ref().unwrap();
            let views: Vec<(u32, &BodyView)> = scene
                .entities
                .iter()
                .filter_map(|d| Some((d.number, d.body.as_ref()?)))
                .collect();
            players.posed(
                src,
                weapons,
                &views,
                &[],
                scene.render_time,
                scene.server_time,
            );
            for (n, v) in views.iter().filter(|(_, v)| v.corpse) {
                let key = (*n, v.frame.client_num, v.frame.eflags & 2);
                let e = corpses.entry(key).or_insert((None, false, false));
                e.1 |= v.ragdoll;
                e.2 |= players.ragdoll_refused(*n);
                if let Some((r, id)) = players.ragdoll_of(*n)
                    && let Some(state) = r.state(id)
                {
                    let caps = r.capsules(id);
                    match &mut e.0 {
                        Some(seen) if caps.is_empty() => seen.0 = state,
                        seen if state == State::Simulating => {
                            *seen = Some((state, r.msec(id), caps))
                        }
                        _ => {}
                    }
                }
            }
        }
        s.disconnect();
        corpses
    });
    // Bot `k` goes onto spot `k` at 6 s + 3 s k and dies there two
    // frames later, once his feet are down.
    let mut placed: Vec<Option<i32>> = vec![None; bots.len()];
    let mut killed = 0;
    serve(&mut sv, &sock, &clock, &client, |sv, _, _| {
        let Some(l) = sv.game.level.as_mut() else {
            return true;
        };
        let now = l.level_time();
        for (k, &bot) in bots.iter().enumerate() {
            let in_play = l
                .playerstate(bot)
                .is_some_and(|p| p.get("pm_type") == Some(0));
            match placed[k] {
                None if in_play && now >= 6000 + 3000 * k as i32 => {
                    let at = spots[k] + Vec3::Z * 60.0;
                    l.set_view_pos(bot, at.to_array(), [0.0, 90.0 * k as f32, 0.0]);
                    placed[k] = Some(now);
                }
                Some(t) if in_play && now - t >= 100 => {
                    l.kill_command(bot);
                    placed[k] = Some(i32::MAX);
                    killed += 1;
                }
                _ => {}
            }
        }
        true
    });
    assert_eq!(killed, bots.len(), "every bot died on his spot");
    let corpses = client.join().unwrap();
    let flipped = corpses.values().filter(|c| c.1).count();
    let refused = corpses.values().filter(|c| c.2).count();
    let asleep: Vec<_> = corpses
        .iter()
        .filter_map(|(k, c)| Some((k, c.0.as_ref()?)))
        .filter(|(_, (state, ..))| *state == State::Idle)
        .collect();
    println!(
        "{} corpses, {flipped} took a ragdoll type, {refused} refused, {} asleep",
        corpses.len(),
        asleep.len()
    );
    assert!(asleep.len() >= 2, "{corpses:#?}");
    // Retail freezes a body still moving at `ragdoll_max_life` too:
    // counted, and held to under half.
    let timed_out = asleep.iter().filter(|(_, (_, ms, _))| *ms >= 4500).count();
    println!("{timed_out} frozen at their max life");
    assert!(
        timed_out * 2 <= asleep.len(),
        "{timed_out} of {} at their max life",
        asleep.len()
    );
    let mut problems = Vec::new();
    let z = Vec3::ZERO;
    for (key, (_, ms, caps)) in &asleep {
        // A capsule rests sunk into what holds it: `phys_csl`'s unit
        // surface layer and the soft contacts (cod4-player-anims.md 10.6;
        // retail's captured ends sit up to 1.8 under their floor, these
        // up to 2.6 where an end presses into a corner, 10.7), so each
        // end's box less 3 units stays clear.
        for &(a, b, r) in caps {
            for p in [a, b] {
                let e = Vec3::splat((r - 3.0).max(0.0));
                if clip.trace(p, p, -e, e, CLIP_MASK).startsolid {
                    let clear = (0..=40)
                        .map(|k| r * k as f32 / 40.0)
                        .take_while(|&c| {
                            !clip
                                .trace(p, p, -Vec3::splat(c), Vec3::splat(c), CLIP_MASK)
                                .startsolid
                        })
                        .last();
                    problems.push(format!(
                        "{key:?}: capsule end {p} (radius {r}, clear to {clear:?}) in the world"
                    ));
                }
            }
            let t = clip.trace(a, b, z, z, CLIP_MASK);
            if t.startsolid || t.fraction < 1.0 {
                problems.push(format!("{key:?}: bone {a} {b} through the world"));
            }
        }
        // One that went to sleep rests on what is under it: some
        // point whose sphere a 2-unit drop would put into the world.
        // One frozen at `ragdoll_max_life` holds wherever it was,
        // mid-fall too (`Ragdoll_CheckIdle`, cod4-player-anims.md
        // 10): a body that slid off a ledge late froze 9 units up.
        let rests = *ms >= 4500
            || caps
                .iter()
                .flat_map(|&(a, b, r)| [(a, r), (b, r)])
                .any(|(p, r)| {
                    let e = Vec3::splat(r);
                    clip.trace(p, p - Vec3::Z * 2.0, -e, e, CLIP_MASK).fraction < 1.0
                });
        if !rests {
            problems.push(format!("{key:?}: rests on nothing"));
        }
    }
    assert!(problems.is_empty(), "{problems:#?}");
}

/// vcod-server's `war` on mp_backlot, a held bot's frags bursting on a
/// destructible car until `_destructible.gsc` blows it up and
/// `physicsLaunch`es its parts, watched by a spectator: each part this
/// client sees launched flies off its spot (`play::cod4_physics`), and
/// goes to sleep clear of the world, resting on what is under it
/// (cod4-gsc.md 3.6); a round that meets a resting one pushes it on
/// and it sleeps again. Returns at once without an install.
#[test]
fn cod4_live_a_blown_up_cars_parts_fly_and_come_to_rest() {
    use vcod_cod4::ragdoll::CLIP_MASK;
    use vcod_server::cod4::{Cod4Config, Cod4Server};
    let Some(install) = vcod_cod4::testing::cod4_install() else {
        return;
    };
    let world = Arc::new(crate::cod4_fly::load_world(install.root(), "mp_backlot").unwrap());
    let _live = live_slot();
    let mut sv = Cod4Server::new(
        Cod4Config {
            map: "mp_backlot".into(),
            gametype: "war".into(),
            hostname: "vcod".into(),
            max_clients: 12,
        },
        11,
    );
    sv.set_cvar("scr_game_playerwaittime", "1");
    sv.set_cvar("scr_game_matchstarttime", "1");
    sv.load(Some(&install)).unwrap();
    let bot = sv.add_held_bot_for_test(false).unwrap();
    let sock = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
    sock.set_nonblocking(true).unwrap();
    let addr = sock.local_addr().unwrap();
    let clip = world.clip.clone();
    let clock = sim::Clock::new();
    // Per part: its model, launch spot, farthest drawn distance off
    // it, and its body's last state (asleep, ms simulated, points).
    type Seen = (String, Vec3, f32, Option<(bool, u32, Vec<Vec3>)>);
    let client = sim::spawn(&clock, move || {
        let join = MenuJoin {
            team: Some("spectator".into()),
            class: Some("assault_mp,0".into()),
        };
        let mut s = Cod4Session::connect(addr, "watcher", Cp::Bare, join).unwrap();
        s.set_world(world.clone());
        let mut parts: BTreeMap<u32, Seen> = BTreeMap::new();
        // At 22 s a round slanting down onto the first resting part
        // the shot's trace meets (`Phys_ObjBulletImpact` off 0x48df10):
        // the part, where it was hit, whether it woke, its last origin.
        let mut shot: Option<(u32, Vec3, bool, Vec3)> = None;
        let t0 = clk_now();
        while since(t0) < Duration::from_secs(30) {
            let now = since(t0).as_secs_f64() * 1000.0;
            s.step(now).unwrap();
            s.take_commands();
            clk_sleep(Duration::from_millis(16));
            let Some(scene) = s.scene() else { continue };
            let mut resting = Vec::new();
            for d in &scene.entities {
                let Some(b) = s.physics.body(d.number) else {
                    continue;
                };
                let Visual::Model(m) = &d.visual else {
                    continue;
                };
                let e = parts
                    .entry(d.number)
                    .or_insert_with(|| (m.clone(), d.origin, 0.0, None));
                e.2 = e.2.max(d.origin.distance(e.1));
                e.3 = Some((b.asleep, b.steps, b.world_points().collect()));
                if b.asleep {
                    resting.push((d.number, d.origin));
                }
                if let Some(sh) = shot.as_mut().filter(|sh| sh.0 == d.number) {
                    sh.2 |= !b.asleep;
                    sh.3 = d.origin;
                }
            }
            if shot.is_none() && since(t0) > Duration::from_secs(22) {
                let w = crate::cod4_fx::ShotWorld {
                    clip: &world.clip,
                    solids: &s.shot_solids,
                };
                let hit = resting.iter().find_map(|&(n, o)| {
                    let (start, end) = (
                        o + Vec3::new(-48.0, 0.0, 48.0),
                        o + Vec3::new(16.0, 0.0, -16.0),
                    );
                    let (at, met) = crate::cod4_fx::first_contact(w, start, end);
                    (met == Some(n)).then_some((n, o, at, (end - start).normalize()))
                });
                if let Some((n, o, at, dir)) = hit {
                    let msec0 = s.dynents.as_ref().map_or(17, |d| d.clock.step_ms);
                    assert!(s.physics.shoot(n, at, dir, 1000.0, msec0));
                    shot = Some((n, o, false, o));
                }
            }
        }
        s.disconnect();
        (parts, shot)
    });
    // From 4 s, a frag bursts on the car every 600 ms, 12 in all.
    let mut car = None;
    let mut bursts = 0;
    let mut last = 0;
    serve(&mut sv, &sock, &clock, &client, |sv, _, _| {
        let Some(l) = sv.game.level.as_mut() else {
            return true;
        };
        let now = l.level_time();
        if now < 4000 || bursts >= 12 || now - last < 600 {
            return true;
        }
        if car.is_none() {
            car = l
                .shot_models_for_test()
                .into_iter()
                .find(|m| m.2)
                .map(|m| m.1);
        }
        if let Some(at) = car {
            assert!(l.burst_for_test(bot, "frag_grenade_mp", at + Vec3::Z * 8.0));
            bursts += 1;
            last = now;
        }
        true
    });
    assert!(car.is_some(), "no destructible on mp_backlot");
    let (parts, shot) = client.join().unwrap();
    let (n, from, woke, to) = shot.expect("a round meets a resting part");
    println!("shot part {n}: {from} -> {to}");
    assert!(woke && to.distance(from) > 1.0, "{n}: {from} -> {to}");
    for (n, (m, at, far, b)) in &parts {
        let b = b.as_ref().unwrap();
        println!(
            "{n} {m}: from {at}, {far:.0} away at most, asleep {}, {} steps",
            b.0, b.1
        );
    }
    assert!(parts.len() >= 3, "{parts:#?}");
    let mut problems = Vec::new();
    let z = Vec3::ZERO;
    for (n, (m, _, far, b)) in &parts {
        let (asleep, _, points) = b.as_ref().unwrap();
        if *far < 8.0 {
            problems.push(format!("{n} {m}: never left its spot ({far})"));
        }
        if !asleep {
            problems.push(format!("{n} {m}: still moving"));
            continue;
        }
        for &p in points {
            if clip.trace(p, p, z, z, CLIP_MASK).startsolid {
                problems.push(format!("{n} {m}: point {p} in the world"));
            }
        }
        let rests = points
            .iter()
            .any(|&p| clip.trace(p, p - Vec3::Z * 2.0, z, z, CLIP_MASK).fraction < 1.0);
        if !rests {
            problems.push(format!("{n} {m}: rests on nothing"));
        }
    }
    assert!(problems.is_empty(), "{problems:#?}");
}

/// vcod-server's `war` on mp_crash, a held bot's frag bursting among
/// the map's clutter at 4 s and another on a destructible plate at
/// 6 s, watched by a spectator: the clutter the first reaches flies
/// and comes to rest again, the plate breaks and stops drawing, and
/// the scene draws every other dynamic entity at its def's pose
/// (cod4-world.md 13). Returns at once without an install.
#[test]
fn cod4_live_a_frag_flings_the_maps_clutter_and_breaks_a_plate() {
    use vcod_server::cod4::{Cod4Config, Cod4Server};
    let Some(install) = vcod_cod4::testing::cod4_install() else {
        return;
    };
    let world = Arc::new(crate::cod4_fly::load_world(install.root(), "mp_crash").unwrap());
    let defs = world.dynents.clone();
    assert_eq!(defs.len(), 516);
    // The densest clutter, and a plate well away from it.
    let near = |i: usize| {
        defs.iter()
            .filter(|d| d.origin.distance(defs[i].origin) < 128.0)
            .count()
    };
    let pile = (0..defs.len()).max_by_key(|&i| near(i)).unwrap();
    let plate = (0..defs.len())
        .filter(|&i| defs[i].kind == vcod_cod4::dynent::DESTRUCT)
        .max_by(|&a, &b| {
            let d = |i: usize| defs[i].origin.distance(defs[pile].origin);
            d(a).total_cmp(&d(b))
        })
        .unwrap();
    let (pile_at, plate_at) = (defs[pile].origin, defs[plate].origin);
    let _live = live_slot();
    let mut sv = Cod4Server::new(
        Cod4Config {
            map: "mp_crash".into(),
            gametype: "war".into(),
            hostname: "vcod".into(),
            max_clients: 12,
        },
        11,
    );
    sv.set_cvar("scr_game_playerwaittime", "1");
    sv.set_cvar("scr_game_matchstarttime", "1");
    sv.load(Some(&install)).unwrap();
    let bot = sv.add_held_bot_for_test(false).unwrap();
    let sock = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
    sock.set_nonblocking(true).unwrap();
    let addr = sock.local_addr().unwrap();
    let clock = sim::Clock::new();
    let client = sim::spawn(&clock, move || {
        let join = MenuJoin {
            team: Some("spectator".into()),
            class: Some("assault_mp,0".into()),
        };
        let mut s = Cod4Session::connect(addr, "watcher", Cp::Bare, join).unwrap();
        s.set_world(world.clone());
        let mut most_awake = 0;
        let mut drawn = Vec::new();
        let t0 = clk_now();
        while since(t0) < Duration::from_secs(14) {
            let now = since(t0).as_secs_f64() * 1000.0;
            s.step(now).unwrap();
            s.take_commands();
            clk_sleep(Duration::from_millis(16));
            let fired = s.take_fired();
            let Some(scene) = s.scene() else { continue };
            let weapons = s.gamestate().map(weapon_list).unwrap_or_default();
            if let Some(d) = s.dynents.as_mut() {
                crate::play::cod4_dynents::on_fired(
                    d,
                    &world,
                    &fired,
                    &weapons,
                    &[],
                    None,
                    scene.render_time,
                );
            }
            most_awake = most_awake.max(s.dynents.as_ref().map_or(0, |d| d.awake()));
            drawn = scene.dynents.clone();
        }
        s.disconnect();
        (s.dynents.take().unwrap(), most_awake, drawn)
    });
    let mut bursts = 0;
    serve(&mut sv, &sock, &clock, &client, |sv, _, _| {
        let Some(l) = sv.game.level.as_mut() else {
            return true;
        };
        let now = l.level_time();
        let at = match bursts {
            0 if now >= 4000 => pile_at + Vec3::Z * 8.0,
            1 if now >= 6000 => plate_at + Vec3::Z * 4.0,
            _ => return true,
        };
        assert!(l.burst_for_test(bot, "frag_grenade_mp", at));
        bursts += 1;
        true
    });
    assert_eq!(bursts, 2);
    let (d, most_awake, drawn) = client.join().unwrap();
    let moved: Vec<usize> = (0..defs.len())
        .filter(|&i| d.ents[i].origin.distance(defs[i].origin) > 1.0)
        .collect();
    println!(
        "pile {pile} at {pile_at}, plate {plate} at {plate_at}: {} moved, {most_awake} awake at most, {} drawn",
        moved.len(),
        drawn.len()
    );
    assert!((1..=40).contains(&most_awake), "{most_awake} awake");
    assert!(!moved.is_empty(), "nothing moved");
    assert_eq!(d.awake(), 0, "still moving at the end");
    assert!(
        !d.ents[plate].drawn && !d.ents[plate].live,
        "the plate stands"
    );
    assert!(drawn.iter().all(|&(i, ..)| i as usize != plate));
    assert_eq!(drawn.len(), d.ents.iter().filter(|e| e.drawn).count());
    // What the blasts did not reach stands where its def put it.
    for (i, e) in d.ents.iter().enumerate() {
        let far = e.origin.distance(pile_at) > 400.0 && e.origin.distance(plate_at) > 400.0;
        if far && e.drawn {
            assert_eq!(e.origin, defs[i].origin, "{} moved", defs[i].model);
        }
    }
}

/// One frame of a simulating ragdoll: the ms it has simulated and each
/// def bone's ends and body turn.
type RagStep = (i32, Vec<(Vec3, Vec3, glam::Quat)>);
/// One corpse's ragdoll: each frame it simulated, and its drawn pose on
/// every frame it held still (idle).
#[derive(Default)]
struct RagRun {
    pub(super) steps: Vec<RagStep>,
    held: Vec<Vec<(Vec3, glam::Quat)>>,
}
/// Per corpse (number, client, teleport bit), its ragdoll.
type RagFrames = BTreeMap<(u32, i32, i32), RagRun>;

/// [`cod4_live_corpses_ragdoll_onto_the_floor`]'s scene (six held bots
/// killed on open floor, 3 s apart, posed by a spectator through
/// `Cod4Players`) with the client's frame `frame_ms` long; every
/// ragdoll recorded on each frame its step moved it.
fn live_ragdoll_frames(frame_ms: u64) -> Option<RagFrames> {
    use crate::play::cod4_players::{Cod4Players, RagdollWorld};
    use vcod_cod4::ragdoll::{Dvars, State};
    use vcod_server::cod4::{Cod4Config, Cod4Server};
    let install = vcod_cod4::testing::cod4_install()?;
    let root = install.root().to_path_buf();
    let world = crash_world().expect("mp_crash's world");
    let def = world.ragdoll.clone().expect("ragdoll.cfg's def");
    let (map, _) = vcod_cod4::map::Map::load(&install, "english", "mp_crash").unwrap();
    let spots = open_floor_spots(&world.clip, &map, 6);
    let _live = live_slot();
    let mut sv = Cod4Server::new(
        Cod4Config {
            map: "mp_crash".into(),
            gametype: "war".into(),
            hostname: "vcod".into(),
            max_clients: 12,
        },
        11,
    );
    sv.set_cvar("scr_game_playerwaittime", "1");
    sv.set_cvar("scr_game_matchstarttime", "1");
    sv.set_cvar("sv_cheats", "1");
    sv.load(Some(&install)).unwrap();
    let bots: Vec<usize> = (0..spots.len())
        .map(|k| sv.add_held_bot_for_test(k % 2 == 1).unwrap())
        .collect();
    let sock = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
    sock.set_nonblocking(true).unwrap();
    let addr = sock.local_addr().unwrap();
    let clock = sim::Clock::new();
    let client = sim::spawn(&clock, move || {
        let join = MenuJoin {
            team: Some("spectator".into()),
            class: Some("assault_mp,0".into()),
        };
        let mut s = Cod4Session::connect(addr, "watcher", Cp::Bare, join).unwrap();
        s.set_world(world.clone());
        let mut players = Cod4Players::default();
        players.ragdoll = Some(RagdollWorld {
            def,
            clip: world.clip.clone(),
            dvars: Dvars::default(),
        });
        let mut loaded = None;
        let mut out: RagFrames = BTreeMap::new();
        let t0 = clk_now();
        while since(t0) < Duration::from_secs(32) {
            let now = since(t0).as_secs_f64() * 1000.0;
            s.step(now).unwrap();
            s.take_commands();
            clk_sleep(Duration::from_millis(frame_ms));
            let Some(scene) = s.scene() else { continue };
            if loaded.is_none() {
                let assets = s.entity_assets();
                loaded = Some(crate::cod4_fly::load_bodies(&root, "mp_crash", &assets).unwrap());
            }
            let (_, weapons, src) = loaded.as_ref().unwrap();
            let views: Vec<(u32, &BodyView)> = scene
                .entities
                .iter()
                .filter_map(|d| Some((d.number, d.body.as_ref()?)))
                .collect();
            players.posed(
                src,
                weapons,
                &views,
                &[],
                scene.render_time,
                scene.server_time,
            );
            for (n, v) in views.iter().filter(|(_, v)| v.corpse) {
                let key = (*n, v.frame.client_num, v.frame.eflags & 2);
                if let Some((r, id)) = players.ragdoll_of(*n) {
                    let e = out.entry(key).or_default();
                    match r.state(id) {
                        Some(State::Simulating) => e.steps.push((r.msec(id), r.frames(id))),
                        Some(State::Idle) => e.held.push(r.pose(id).unwrap_or_default().to_vec()),
                        _ => {}
                    }
                }
            }
        }
        s.disconnect();
        out
    });
    let mut placed: Vec<Option<i32>> = vec![None; bots.len()];
    serve(&mut sv, &sock, &clock, &client, |sv, _, _| {
        let Some(l) = sv.game.level.as_mut() else {
            return true;
        };
        let now = l.level_time();
        for (k, &bot) in bots.iter().enumerate() {
            let in_play = l
                .playerstate(bot)
                .is_some_and(|p| p.get("pm_type") == Some(0));
            match placed[k] {
                None if in_play && now >= 6000 + 3000 * k as i32 => {
                    let at = spots[k] + Vec3::Z * 60.0;
                    l.set_view_pos(bot, at.to_array(), [0.0, 90.0 * k as f32, 0.0]);
                    placed[k] = Some(now);
                }
                Some(t) if in_play && now - t >= 100 => {
                    l.kill_command(bot);
                    placed[k] = Some(i32::MAX);
                }
                _ => {}
            }
        }
        true
    });
    Some(client.join().unwrap())
}

/// The angle of `q`, degrees.
fn turn_deg(q: glam::Quat) -> f32 {
    2f32.to_degrees() * q.xyz().length().atan2(q.w.abs())
}

/// How far a corpse's neck twisted against its upper spine over the
/// shortest turn of its axis from its first frame, degrees.
fn ragdoll_twist(steps: &[RagStep]) -> f32 {
    // ragdoll.cfg's def 1: bones 0..2 the pelvis and spine, 3 the neck.
    let (spine, neck) = (2, 3);
    let in_spine = |f: &[(Vec3, Vec3, glam::Quat)]| {
        let r = f[spine].2.inverse();
        (
            (r * (f[neck].1 - f[neck].0)).normalize_or_zero(),
            r * f[neck].2,
        )
    };
    let (d0, rel0) = in_spine(&steps[0].1);
    steps
        .iter()
        .map(|s| {
            let (d, rel) = in_spine(&s.1);
            turn_deg((glam::Quat::from_rotation_arc(d0, d) * rel0).inverse() * rel)
        })
        .fold(0.0, f32::max)
}

/// The corpses of [`live_ragdoll_frames`] at 250 and at 10 frames a
/// second (cod4-player-anims.md 10.7): each goes idle, asleep or at
/// `ragdoll_max_life`, its pose then held frame after frame. Prints each
/// corpse's numbers, the neck's twist against the upper spine among them
/// (retail's max-life capture turns it 108 degrees by the same measure:
/// the joints' limits are the base pose's, not the death anim's).
/// Returns at once without an install.
#[test]
fn cod4_live_ragdolls_hold_still_once_down() {
    use vcod_cod4::ragdoll::Dvars;
    let d = Dvars::default();
    for frame_ms in [4, 100] {
        let Some(runs) = live_ragdoll_frames(frame_ms) else {
            return;
        };
        assert!(runs.len() >= 4, "{frame_ms} ms: {} ragdolls", runs.len());
        let (mut problems, mut frozen) = (Vec::new(), 0);
        for (key, run) in &runs {
            if run.steps.is_empty() {
                problems.push(format!("{frame_ms} ms {key:?}: never simulated"));
                continue;
            }
            let twist = ragdoll_twist(&run.steps);
            let ms = run.steps.last().unwrap().0;
            let moved = run.held.first().map(|first| {
                run.held
                    .iter()
                    .flat_map(|h| h.iter().zip(first).map(|(x, y)| x.0.distance(y.0)))
                    .fold(0.0f32, f32::max)
            });
            println!(
                "{frame_ms} ms {key:?}: idle after {ms} ms simulated; neck twist {twist:.1} deg; moved {moved:?} while held"
            );
            match moved {
                None => problems.push(format!("{frame_ms} ms {key:?}: never idle")),
                Some(m) if m > 0.0 => {
                    problems.push(format!("{frame_ms} ms {key:?}: moved {m} held"))
                }
                _ => {}
            }
            if ms >= d.max_life_ms {
                frozen += 1;
            }
        }
        println!("{frame_ms} ms: {frozen} of {} at max life", runs.len());
        assert!(problems.is_empty(), "{problems:#?}");
    }
}

/// vcod-server's `war` with eight shooting bots, watched as a
/// spectator posing every body with the level's ragdoll: each
/// grenade, rocket or script blast the server sends turns into the
/// ragdoll push of its weapon's `explosionRadius` (or the script's
/// radii) through [`cod4_players::explosion_of`], and pushes the
/// running and sleeping ragdolls around it (cod4-player-anims.md
/// 10.1). Prints how many blasts reached a body. Returns at once
/// without an install.
#[test]
fn cod4_live_blasts_push_the_ragdolls() {
    use crate::play::cod4_players::{Cod4Players, RagdollWorld, explosion_of};
    use vcod_cod4::ragdoll::Dvars;
    use vcod_server::cod4::{Cod4Config, Cod4Server};
    let Some(install) = vcod_cod4::testing::cod4_install() else {
        return;
    };
    let root = install.root().to_path_buf();
    let world = crash_world().expect("mp_crash's world");
    let def = world.ragdoll.clone().expect("ragdoll.cfg's def");
    let ((sounds, _), _) = crate::cod4_fly::load_av(&root, "mp_crash").unwrap();
    let defs = sounds.weapons.clone();
    let _live = live_slot();
    let mut sv = Cod4Server::new(
        Cod4Config {
            map: "mp_crash".into(),
            gametype: "war".into(),
            hostname: "vcod".into(),
            max_clients: 12,
        },
        13,
    );
    sv.set_cvar("scr_game_playerwaittime", "1");
    sv.set_cvar("scr_game_matchstarttime", "1");
    sv.load(Some(&install)).unwrap();
    sv.add_bots(8, true);
    let (events, built, reached, missing) = run_held(
        &mut sv,
        move |addr| {
            let join = MenuJoin {
                team: Some("spectator".into()),
                class: Some("assault_mp,0".into()),
            };
            let mut s = Cod4Session::connect(addr, "watcher", Cp::Bare, join).unwrap();
            s.set_world(world.clone());
            let mut players = Cod4Players::default();
            players.ragdoll = Some(RagdollWorld {
                def,
                clip: world.clip.clone(),
                dvars: Dvars::default(),
            });
            let mut loaded = None;
            // (explode events, pushes built, ragdolls reached, the
            // explodes whose weapon the client has no def of)
            let (mut events, mut built, mut reached) = (0, 0, 0);
            let mut missing = Vec::new();
            let t0 = clk_now();
            while since(t0) < Duration::from_secs(60) {
                let now = since(t0).as_secs_f64() * 1000.0;
                s.step(now).unwrap();
                s.take_commands();
                clk_sleep(Duration::from_millis(16));
                let fired = s.take_fired();
                let Some(gs) = s.gamestate() else { continue };
                let weapons = weapon_list(gs);
                for f in &fired {
                    let explode = f.name.contains("_EXPLODE") && f.name != "EV_FLASHBANG_EXPLODE";
                    events += usize::from(explode);
                    if let Some(e) = explosion_of(f, &weapons, &defs) {
                        built += 1;
                        reached += players.explode(&e);
                    } else if explode
                        && !weapon_name(&weapons, f.weapon).is_some_and(|w| defs.contains_key(w))
                    {
                        missing.push((f.name, f.weapon));
                    }
                }
                let Some(scene) = s.scene() else { continue };
                if loaded.is_none() {
                    loaded = Some(
                        crate::cod4_fly::load_bodies(&root, "mp_crash", &s.entity_assets())
                            .unwrap(),
                    );
                }
                let (_, weapons, src) = loaded.as_ref().unwrap();
                let views: Vec<(u32, &BodyView)> = scene
                    .entities
                    .iter()
                    .filter_map(|d| Some((d.number, d.body.as_ref()?)))
                    .collect();
                players.posed(
                    src,
                    weapons,
                    &views,
                    &[],
                    scene.render_time,
                    scene.server_time,
                );
            }
            s.disconnect();
            (events, built, reached, missing)
        },
        |_, _, _| true,
    );
    println!("{events} explodes, {built} pushes, {reached} ragdolls reached");
    assert!(events > 0, "no blast in a minute of bots");
    assert!(
        missing.is_empty(),
        "explodes of unknown weapons: {missing:?}"
    );
    assert!(built > 0, "no explode turned into a push");
}

/// The retail kill capture's corpse (64) as its shooter saw it: its
/// entity turns ragdoll at 49550 (cod4-combat.md 5.1), when the death
/// anim has put its hand into the wall it fell against, so the tunnel
/// test refuses the ragdoll at its build two frames on and the corpse
/// keeps its death anim's pose (cod4-player-anims.md 10, 10.7). Fixed
/// 60 Hz frames, so it repeats.
#[test]
fn the_kill_captures_corpse_fails_the_tunnel_test() {
    use crate::play::cod4_players::{Cod4Players, RagdollWorld};
    use vcod_cod4::ragdoll::Dvars;
    let Some(install) = vcod_cod4::testing::cod4_install() else {
        return;
    };
    let world = crash_world().expect("mp_crash's world");
    let text = std::fs::read_to_string(format!("{FIXTURES}/play-1.7-kill-shooter.txt")).unwrap();
    let mut s = Cod4Session::open_capture(&text).unwrap();
    let (_, weapons, src) =
        crate::cod4_fly::load_bodies(install.root(), "mp_crash", &s.entity_assets()).unwrap();
    let mut players = Cod4Players::default();
    players.ragdoll = Some(RagdollWorld {
        def: world.ragdoll.clone().expect("ragdoll.cfg's def"),
        clip: world.clip.clone(),
        dvars: Dvars::default(),
    });
    let (mut ms, mut flipped_at, mut refused_at) = (0.0, None, None);
    while !s.finished() {
        ms += 1000.0 / 60.0;
        s.step(ms).unwrap();
        let Some(scene) = s.scene() else { continue };
        let views: Vec<(u32, &BodyView)> = scene
            .entities
            .iter()
            .filter_map(|d| Some((d.number, d.body.as_ref()?)))
            .collect();
        players.posed(
            &src,
            &weapons,
            &views,
            &[],
            scene.render_time,
            scene.server_time,
        );
        if views.iter().any(|(n, v)| *n == 64 && v.ragdoll) {
            flipped_at.get_or_insert(scene.server_time);
        }
        if players.ragdoll_refused(64) {
            refused_at.get_or_insert(scene.server_time);
        }
        if let Some((r, id)) = players.ragdoll_of(64) {
            assert!(r.pose(id).is_none(), "a ragdoll ran");
        }
    }
    // The slot waits for the corpse's DObj, takes two snapshots a frame
    // apart and builds on the second: refused within those frames.
    assert_eq!(flipped_at, Some(49_550));
    assert!(
        refused_at.is_some_and(|t| (49_550..=49_650).contains(&t)),
        "{refused_at:?}"
    );
}
