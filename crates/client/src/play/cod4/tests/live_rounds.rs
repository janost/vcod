//! Our rounds and the bots' on a live server, against what the server
//! traces and bleeds.

use vcod_cod4::net::events::event_name;

use super::*;

/// A live session on vcod-server fires its own rounds into mp_crash's
/// ground (cod4-effects.md 12): `+attack` held, each fire event the server
/// sends back is traced client-side from the drawn eye, strikes the
/// world and resolves an impact effect and an impact sound; the
/// tracer rolls run on the same hits. Waits out the prematch freeze
/// first.
#[test]
fn cod4_live_own_shots_strike_the_world() {
    use crate::cod4_fx::OwnView;
    use crate::play::input::Action;
    let Some(out) = live_join(|s, spawned, t0| {
        spawned.as_ref()?;
        let install = vcod_cod4::testing::cod4_install()?;
        let ((mut sounds, mut effects), _) =
            crate::cod4_fly::load_av(install.root(), "mp_crash").ok()?;
        let world = s.world()?.clone();
        let mut fx = crate::fx::sim::FxSystem::new();
        let (mut fires, mut hits, mut impacts, mut cues) = (0, Vec::new(), 0, 0);
        let mut further = Vec::new();
        // The prematch freezes the player and his weapon
        // (`PMF_FROZEN` 0x800) for its first seconds.
        let frozen = |s: &Cod4Session| {
            s.client()
                .snapshots
                .last()
                .and_then(|x| x.ps.get("pm_flags"))
                .is_some_and(|f| f & 0x800 != 0)
        };
        let wait = clk_now();
        while frozen(s) && since(wait) < Duration::from_secs(30) {
            s.step(since(t0).as_secs_f64() * 1000.0).unwrap();
            s.take_commands();
            s.take_fired();
            clk_sleep(Duration::from_millis(16));
        }
        // Down at the ground ahead: the spawn faces the map's caulk
        // edge, whose surface (0) the impact table leaves empty.
        s.view[0] = 45.0;
        let t1 = clk_now();
        let mut held = false;
        while since(t1) < Duration::from_millis(2500) {
            // Two bursts: held 0.3 s to 1 s and 1.4 s to 2 s.
            let ms = since(t1).as_millis();
            let want = (300..1000).contains(&ms) || (1400..2000).contains(&ms);
            if want != held {
                s.input.key(Action::Attack, want);
                held = want;
            }
            s.step(since(t0).as_secs_f64() * 1000.0).unwrap();
            s.take_commands();
            let fired = s.take_fired();
            let Some(scene) = s.scene() else { continue };
            let weapons = weapon_list(s.gamestate().unwrap());
            let own = OwnView {
                client: Some(scene.view.client_num),
                eye: scene.eye,
                angles: scene.angles,
                flash: None,
                third_person: false,
            };
            for f in fired
                .iter()
                .filter(|f| f.name.starts_with("EV_FIRE_WEAPON"))
            {
                fires += 1;
                let defs = sounds.weapons.clone();
                let Some(hit) = first_bullet(
                    f,
                    &weapons,
                    &defs,
                    &HashMap::new(),
                    Some(&own),
                    &world.clip,
                    &s.shot_solids,
                ) else {
                    continue;
                };
                effects.tracers(&mut fx, std::slice::from_ref(&hit), 0.0, 16);
                impacts += effects.bullet_plays(f, &weapons, &hit).len();
                cues += sounds.bullet_cues(f, &weapons, &hit, None).len();
                hits.push(((hit.end - hit.start).length(), hit.surf, hit.own));
                further.extend(hit.pierced.iter().flatten().map(|p| p.surf));
            }
            clk_sleep(Duration::from_millis(16));
        }
        s.input.key(Action::Attack, false);
        Some((
            fires,
            hits,
            impacts,
            cues,
            effects.tracers,
            effects.missing.clone(),
            further,
        ))
    })
    .flatten() else {
        return;
    };
    let (fires, hits, impacts, cues, tracers, missing, further) = out;
    println!(
        "live: {fires} fire events, hits {hits:?}, {impacts} impacts, {cues} cues, {tracers} tracers, further {further:?}"
    );
    assert!(fires >= 3, "{fires}");
    assert_eq!(hits.len(), fires);
    assert!(
        hits.iter().all(|h| h.2 && h.0 < 400.0 && h.1 != 0),
        "{hits:?}"
    );
    // A round passes the rock's one-sided terrain (cod4-collision.md 3)
    // and meets the caulk floor under it: a sound, no effect.
    let bare = further.iter().filter(|&&s| s == 0).count();
    assert_eq!(
        (impacts, cues),
        (fires + further.len() - bare, fires + further.len()),
        "{missing:?}"
    );
}

/// Our own M16 rounds on vcod-server through the spawn's wall
/// (the penetration walk, cod4-effects.md 12.1): turned east, each
/// round's first contact is the 8-unit concrete wall ahead, its far
/// side plays the large exit effect and sound, and the round goes on
/// to the next surface, which plays its own impact.
#[test]
fn cod4_live_own_rounds_pierce_the_wall_and_play_its_far_side() {
    use crate::cod4_fx::OwnView;
    use crate::play::input::Action;
    let Some(out) = live_join(|s, spawned, t0| {
        spawned.as_ref()?;
        let install = vcod_cod4::testing::cod4_install()?;
        let ((mut sounds, mut effects), _) =
            crate::cod4_fly::load_av(install.root(), "mp_crash").ok()?;
        let world = s.world()?.clone();
        let frozen = |s: &Cod4Session| {
            s.client()
                .snapshots
                .last()
                .and_then(|x| x.ps.get("pm_flags"))
                .is_some_and(|f| f & 0x800 != 0)
        };
        let wait = clk_now();
        while frozen(s) && since(wait) < Duration::from_secs(30) {
            s.step(since(t0).as_secs_f64() * 1000.0).unwrap();
            s.take_commands();
            s.take_fired();
            clk_sleep(Duration::from_millis(16));
        }
        s.view = [0.0, 0.0];
        let (mut rounds, mut exits, mut names) = (0, 0, HashSet::new());
        let t1 = clk_now();
        while since(t1) < Duration::from_millis(1500) {
            let ms = since(t1).as_millis();
            s.input.key(Action::Attack, (300..900).contains(&ms));
            s.step(since(t0).as_secs_f64() * 1000.0).unwrap();
            s.take_commands();
            let fired = s.take_fired();
            let Some(scene) = s.scene() else { continue };
            let weapons = weapon_list(s.gamestate().unwrap());
            let own = OwnView {
                client: Some(scene.view.client_num),
                eye: scene.eye,
                angles: scene.angles,
                flash: None,
                third_person: false,
            };
            let defs = sounds.weapons.clone();
            for f in fired
                .iter()
                .filter(|f| f.name.starts_with("EV_FIRE_WEAPON"))
            {
                let Some(hit) = first_bullet(
                    f,
                    &weapons,
                    &defs,
                    &HashMap::new(),
                    Some(&own),
                    &world.clip,
                    &s.shot_solids,
                ) else {
                    continue;
                };
                rounds += 1;
                exits += hit.pierced.iter().flatten().filter(|p| p.exit).count();
                names.extend(
                    effects
                        .bullet_plays(f, &weapons, &hit)
                        .into_iter()
                        .map(|p| p.key),
                );
                names.extend(
                    sounds
                        .bullet_cues(f, &weapons, &hit, None)
                        .into_iter()
                        .map(|c| c.alias),
                );
            }
            clk_sleep(Duration::from_millis(16));
        }
        s.input.key(Action::Attack, false);
        Some((
            rounds,
            exits,
            names,
            effects.missing.clone(),
            sounds.missing.clone(),
        ))
    })
    .flatten() else {
        return;
    };
    let (rounds, exits, names, fx_missing, snd_missing) = out;
    println!("pierce: {rounds} rounds, {exits} exits, {names:?}");
    assert!(rounds >= 3, "{rounds}");
    assert_eq!(exits, rounds, "{names:?}");
    assert!(names.iter().any(|n| n.ends_with("_exit.efx")), "{names:?}");
    assert!(
        names.iter().any(|n| n.starts_with("bullet_large_exit")),
        "{names:?}"
    );
    assert!(
        fx_missing.is_empty() && snd_missing.is_empty(),
        "{fx_missing:?} {snd_missing:?}"
    );
}

/// Our own M16 rounds on vcod-server at an axis bot kept standing
/// 200 units ahead (cod4-effects.md 12.2): each round leaves along the
/// direction the server's `Bullet_Fire` drew for it (the spread off
/// the same `commandTime` seed), stops on the bot's box and plays no
/// impact of its own; the server's `EV_BULLET_HIT` flesh events land
/// on the bot where our rounds stopped, and they play the flesh hit.
#[test]
fn cod4_live_rounds_stop_on_the_bot_where_the_server_bleeds() {
    rounds_on_the_bot(false, false);
}

/// [`cod4_live_rounds_stop_on_the_bot_where_the_server_bleeds`] up the
/// sight: the rounds leave along the gun's angles, the gun spring's
/// kick in them (cod4-combat.md 3.2).
#[test]
fn cod4_live_sighted_rounds_stop_on_the_bot_where_the_server_bleeds() {
    rounds_on_the_bot(true, false);
}

/// [`cod4_live_rounds_stop_on_the_bot_where_the_server_bleeds`] with
/// us hurt 280 ms before each burst, in turn from ahead, from the right
/// and from no direction: the rounds leave along the damage kick the
/// server's aim block eases in, which the predictor works out off the
/// snapshot's `damageEvent` (cod4-combat.md 3.2).
#[test]
fn cod4_live_hurt_rounds_take_the_servers_damage_kick() {
    rounds_on_the_bot(false, true);
}

/// The melee key with an axis bot standing 100 units ahead, past
/// `player_meleeRange` and inside `aim_automelee_range`: our cmd asks
/// for a 100-unit lunge along the yaw to him, vcod-server's pmove runs
/// it (`PMF_MELEE_CHARGE`) and throws us at him, the charge swing kills
/// him and we run on past where he stood, as the retail capture's
/// attacker does (`play-1.7-lunge.txt`), and our prediction lands where
/// the server put us (cod4-pmove.md 19).
#[test]
fn cod4_live_the_melee_key_lunges_at_a_bot_in_reach() {
    use crate::play::input::Action;
    let sets = [
        ("sv_cheats", "1"),
        ("scr_game_playerwaittime", "1"),
        ("scr_game_matchstarttime", "1"),
    ];
    let Some(out) = live_join_on(
        "mp_crash",
        crash_world(),
        &sets,
        move |s, spawned, t0, tx| {
            spawned.as_ref().expect("spawned");
            let world = s.world().expect("world").clone();
            let step = |s: &mut Cod4Session, ms: u64| {
                let end = clk_now() + Duration::from_millis(ms);
                while clk_now() < end {
                    s.step(since(t0).as_secs_f64() * 1000.0).unwrap();
                    s.take_commands();
                    // The frame's prediction, which the pick reads.
                    s.scene();
                    clk_sleep(Duration::from_millis(16));
                }
            };
            let flags = |s: &Cod4Session| {
                s.client()
                    .snapshots
                    .last()
                    .and_then(|x| x.ps.get("pm_flags"))
                    .unwrap_or(0)
            };
            let wait = clk_now();
            while flags(s) & 0x800 != 0 && since(wait) < Duration::from_secs(30) {
                step(s, 16);
            }
            let feet = |s: &Cod4Session| {
                let ps = &s.client().snapshots.last().unwrap().ps;
                vec3(|k| ps_f32(ps, &format!("origin[{k}]")))
            };
            let start = feet(s);
            let eye = start + Vec3::Z * 60.0;
            // A yaw along which a standing body's box crosses 150 units
            // of level floor (the bot stands at 100).
            let (lo, hi) = (Vec3::new(-16.0, -16.0, 0.0), Vec3::new(16.0, 16.0, 70.0));
            let yaw = (0..24).map(|k| k as f32 * 15.0).find(|y| {
                let d = Vec3::new(y.to_radians().cos(), y.to_radians().sin(), 0.0);
                let lift = Vec3::Z * 1.0;
                let t =
                    world
                        .clip
                        .trace(start + lift, start + lift + d * 150.0, lo, hi, 0x281_0011);
                let level = [50.0, 100.0, 150.0].iter().all(|&r| {
                    let p = start + d * r;
                    let down = world.clip.trace(
                        p + Vec3::Z * 2.0,
                        p - Vec3::Z * 2.0,
                        Vec3::ZERO,
                        Vec3::ZERO,
                        1,
                    );
                    down.fraction < 1.0 && !down.startsolid
                });
                let sight =
                    world
                        .clip
                        .trace(eye, eye + d * 200.0, Vec3::ZERO, Vec3::ZERO, 0x280_6831);
                t.fraction == 1.0 && !t.startsolid && level && sight.fraction == 1.0
            });
            let yaw = yaw.expect("an open yaw");
            let d = Vec3::new(yaw.to_radians().cos(), yaw.to_radians().sin(), 0.0);
            let at = start + d * 100.0;
            // Mortal, as retail's target is: the swing kills him and the
            // lunge runs on through where he stood.
            tx.send(Ctl::PinVictim(at.to_array())).unwrap();
            s.view = [0.0, yaw];
            step(s, 6000);
            let before = feet(s);
            s.input.key(Action::Melee, true);
            let (mut charged, mut path) = (false, Vec::new());
            let t1 = clk_now();
            while since(t1) < Duration::from_millis(1200) {
                step(s, 16);
                if since(t1) > Duration::from_millis(100) {
                    s.input.key(Action::Melee, false);
                }
                charged |= flags(s) & vcod_cod4::pmove::pmf::MELEE_CHARGE != 0;
                path.push(feet(s));
            }
            let predicted = s.last_predicted.as_ref().expect("predicted").origin;
            (before, at, charged, path, predicted, feet(s))
        },
    ) else {
        return;
    };
    let (before, bot, charged, path, predicted, server) = out;
    println!("from {before} bot {bot} path {path:?}");
    assert!(charged, "the server never ran the charge");
    // At least the lunge's 100 units toward him (the forced forward
    // key walks on while the charge lasts).
    let toward = (server - before)
        .truncate()
        .dot((bot - before).truncate().normalize());
    assert!(toward > 100.0, "moved {toward} toward him");
    // The lunge's first snapshot moves at its speed, not the walk's.
    let first = path
        .windows(2)
        .map(|w| (w[1] - w[0]).truncate().length())
        .fold(0.0, f32::max);
    assert!(first > 15.0, "fastest snapshot step {first}");
    assert!(predicted.distance(server) < 0.5, "{predicted} {server}");
}

/// Our own M16 rounds on vcod-server at an mp_crash destructible car
/// (cod4-combat.md 13.1): a script mover with no `solid`, which the
/// client traces by its model's collision (`CG_LocationalTraceDObj`)
/// as the server's `Bullet_Trace` does, so each round stops on the car
/// where the server's trace of the same line stops.
#[test]
fn cod4_live_own_rounds_stop_on_a_car_where_the_servers_do() {
    use crate::cod4_fx::OwnView;
    use crate::play::input::Action;
    let sets = [
        ("sv_cheats", "1"),
        ("scr_game_playerwaittime", "1"),
        ("scr_game_matchstarttime", "1"),
    ];
    let Some(out) = live_join_on(
        "mp_crash",
        crash_world(),
        &sets,
        move |s, spawned, t0, tx| {
            spawned.as_ref()?;
            let install = vcod_cod4::testing::cod4_install()?;
            let ((sounds, _), _) = crate::cod4_fly::load_av(install.root(), "mp_crash").ok()?;
            let world = s.world()?.clone();
            let step = |s: &mut Cod4Session, ms: u64| {
                let end = clk_now() + Duration::from_millis(ms);
                let mut fired = Vec::new();
                while clk_now() < end {
                    s.step(since(t0).as_secs_f64() * 1000.0).unwrap();
                    s.take_commands();
                    fired.extend(s.take_fired());
                    clk_sleep(Duration::from_millis(16));
                }
                fired
            };
            let frozen = |s: &Cod4Session| {
                s.client()
                    .snapshots
                    .last()
                    .and_then(|x| x.ps.get("pm_flags"))
                    .is_some_and(|f| f & 0x800 != 0)
            };
            let wait = clk_now();
            while frozen(s) && since(wait) < Duration::from_secs(30) {
                step(s, 16);
            }
            let (mtx, mrx) = std::sync::mpsc::channel();
            tx.send(Ctl::Models(mtx)).ok()?;
            let models: ModelList = loop {
                step(s, 16);
                if let Ok(v) = mrx.try_recv() {
                    break v;
                }
            };
            let ps = &s.client().snapshots.last()?.ps;
            let feet = vec3(|k| ps_f32(ps, &format!("origin[{k}]")));
            let mut cars: Vec<_> = models.iter().filter(|m| m.2).collect();
            cars.sort_by(|a, b| a.1.distance(feet).total_cmp(&b.1.distance(feet)));
            // A spot 160 units off a car, on the floor, whose eye sees
            // the car's doors over open air.
            let (car, spot, aim) = cars.iter().find_map(|c| {
                (0..16).find_map(|k| {
                    let y = (k as f32 * 22.5).to_radians();
                    let d = Vec3::new(y.cos(), y.sin(), 0.0);
                    let spot = c.1 + d * 160.0;
                    let (top, bottom) = (spot + Vec3::Z * 64.0, spot - Vec3::Z * 128.0);
                    let down = world
                        .clip
                        .trace(top, bottom, Vec3::ZERO, Vec3::ZERO, 0x281_0011);
                    if down.startsolid || down.fraction >= 1.0 {
                        return None;
                    }
                    let floor = top.lerp(bottom, down.fraction) + Vec3::Z * 0.125;
                    if (floor.z - c.1.z).abs() > 32.0 {
                        return None;
                    }
                    let (lo, hi) = (Vec3::new(-16.0, -16.0, 0.0), Vec3::new(16.0, 16.0, 70.0));
                    let room = world.clip.trace(floor, floor, lo, hi, 0x281_0011);
                    if room.startsolid {
                        return None;
                    }
                    let aim = c.1 + Vec3::Z * 22.0;
                    let eye = floor + Vec3::Z * 60.0;
                    let t = world
                        .clip
                        .trace(eye, aim, Vec3::ZERO, Vec3::ZERO, 0x280_6831);
                    (t.fraction == 1.0).then_some((c.0, floor, aim))
                })
            })?;
            let to = aim - (spot + Vec3::Z * 60.0);
            let yaw = to.y.atan2(to.x).to_degrees();
            // `setviewpos` places the eye.
            s.command(&format!(
                "setviewpos {} {} {} {yaw}",
                spot.x,
                spot.y,
                spot.z + 60.0
            ));
            step(s, 1500);
            let ps = &s.client().snapshots.last()?.ps;
            let eye = vec3(|k| ps_f32(ps, &format!("origin[{k}]"))) + Vec3::Z * 60.0;
            // On the spot, not knocked off it.
            if eye.truncate().distance(spot.truncate()) > 1.0 {
                return None;
            }
            let to = aim - eye;
            s.view = [
                -to.z.atan2(to.truncate().length()).to_degrees(),
                to.y.atan2(to.x).to_degrees(),
            ];
            step(s, 500);
            let weapons = weapon_list(s.gamestate()?);
            let defs = sounds.weapons.clone();
            let mut rounds = Vec::new();
            let t1 = clk_now();
            while since(t1) < Duration::from_millis(1600) {
                // Two short bursts: the car must not burn out.
                let ms = since(t1).as_millis() % 800;
                s.input.key(Action::Attack, ms < 100);
                s.step(since(t0).as_secs_f64() * 1000.0).unwrap();
                s.take_commands();
                let scene = s.scene();
                let fired = s.take_fired();
                let Some(scene) = scene else {
                    clk_sleep(Duration::from_millis(16));
                    continue;
                };
                let own = OwnView {
                    client: Some(scene.view.client_num),
                    eye: scene.eye,
                    angles: scene.angles,
                    flash: None,
                    third_person: false,
                };
                for f in fired
                    .iter()
                    .filter(|f| f.name.starts_with("EV_FIRE_WEAPON"))
                {
                    if let Some(h) = first_bullet(
                        f,
                        &weapons,
                        &defs,
                        &HashMap::new(),
                        Some(&own),
                        &world.clip,
                        &s.shot_solids,
                    ) {
                        rounds.push((h.start, h.end));
                    }
                }
                clk_sleep(Duration::from_millis(16));
            }
            s.input.key(Action::Attack, false);
            let lines = rounds
                .iter()
                .map(|&(a, b)| (a, a + (b - a).normalize_or_zero() * 8192.0))
                .collect();
            let (htx, hrx) = std::sync::mpsc::channel();
            tx.send(Ctl::ModelTrace(lines, htx)).ok()?;
            let server: ModelHits = loop {
                step(s, 16);
                if let Ok(v) = hrx.try_recv() {
                    break v;
                }
            };
            Some((car, rounds, server))
        },
    )
    .flatten() else {
        return;
    };
    let (car, rounds, server) = out;
    println!("car {car} rounds {rounds:?}\nserver {server:?}");
    assert!(rounds.len() >= 4, "{rounds:?}");
    // The wire carries the car's origin in whole units, as retail's
    // (car 153 reads 996 -1408 64 in `play-1.7-spec.txt`, where the
    // server's stands at 995.9 -1407.9), so the client's car is a
    // tenth of a unit off the server's on each axis.
    for ((_, end), (sv_end, sv_car)) in rounds.iter().zip(&server) {
        assert_eq!(*sv_car, Some(car), "the server's round missed the car");
        assert!(
            end.distance(*sv_end) < 0.5,
            "ours {end} the server's {sv_end}"
        );
    }
}

fn rounds_on_the_bot(sighted: bool, hurt: bool) {
    use crate::cod4_fx::OwnView;
    use crate::play::input::Action;
    let sets = [
        ("sv_cheats", "1"),
        ("scr_game_playerwaittime", "1"),
        ("scr_game_matchstarttime", "1"),
    ];
    let Some(out) = live_join_on(
        "mp_crash",
        crash_world(),
        &sets,
        move |s, spawned, t0, tx| {
            spawned.as_ref()?;
            let install = vcod_cod4::testing::cod4_install()?;
            let ((sounds, mut effects), _) =
                crate::cod4_fly::load_av(install.root(), "mp_crash").ok()?;
            let world = s.world()?.clone();
            // A frame as the client's: the scene too, whose prediction
            // runs the aim block on every cmd from the spawn on, as
            // the server's think does; one first drawn at the burst
            // started the idle drift's clock there, the server's
            // seconds ahead, and a sighted round left up to 0.05
            // degrees off by the join's length.
            let step = |s: &mut Cod4Session, ms: u64| {
                let end = clk_now() + Duration::from_millis(ms);
                let mut fired = Vec::new();
                while clk_now() < end {
                    s.step(since(t0).as_secs_f64() * 1000.0).unwrap();
                    s.take_commands();
                    s.scene();
                    fired.extend(s.take_fired());
                    clk_sleep(Duration::from_millis(16));
                }
                fired
            };
            let frozen = |s: &Cod4Session| {
                s.client()
                    .snapshots
                    .last()
                    .and_then(|x| x.ps.get("pm_flags"))
                    .is_some_and(|f| f & 0x800 != 0)
            };
            let wait = clk_now();
            while frozen(s) && since(wait) < Duration::from_secs(30) {
                step(s, 16);
            }
            // A yaw along which the eye sees 400 units of open floor.
            let ps = &s.client().snapshots.last()?.ps;
            let feet = vec3(|k| ps_f32(ps, &format!("origin[{k}]")));
            let eye = feet + Vec3::Z * 60.0;
            let yaw = (0..24).map(|k| k as f32 * 15.0).find(|y| {
                let d = Vec3::new(y.to_radians().cos(), y.to_radians().sin(), 0.0);
                let t = world
                    .clip
                    .trace(eye, eye + d * 400.0, Vec3::ZERO, Vec3::ZERO, 0x280_6831);
                let down = world.clip.trace(
                    feet + d * 200.0 + Vec3::Z * 8.0,
                    feet + d * 200.0 - Vec3::Z * 8.0,
                    Vec3::ZERO,
                    Vec3::ZERO,
                    1,
                );
                t.fraction == 1.0 && down.fraction < 1.0
            })?;
            let d = Vec3::new(yaw.to_radians().cos(), yaw.to_radians().sin(), 0.0);
            let at = feet + d * 200.0;
            tx.send(Ctl::PinBot(at.to_array())).ok()?;
            // At his chest, 15 under the eye: level, the hip spread put
            // a burst's rounds over his head by its seeds, the cmds'
            // times, and the server's bones met none of them.
            s.view = [15f32.atan2(200.0).to_degrees(), yaw];
            s.input.key(Action::Ads, sighted);
            step(s, 6000);
            let weapons = weapon_list(s.gamestate()?);
            let defs = sounds.weapons.clone();
            let (mut rounds, mut bleeds, mut own_plays) = (Vec::new(), Vec::new(), 0);
            let mut flesh = Vec::new();
            let mut body = None;
            let mut hurts = 0;
            let t1 = clk_now();
            while since(t1) < Duration::from_millis(4000) {
                // Three bursts a second and more apart, the spread settled
                // between them.
                let ms = since(t1).as_millis() % 1300;
                let burst = since(t1).as_millis() / 1300;
                if hurt && hurts <= burst && ms >= 20 {
                    let view = Vec3::new(0.0, yaw, 0.0);
                    let [f, r, _] = vcod_common::x87::angle_vectors(view);
                    let dir = [Some(f.to_array()), Some(r.to_array()), None];
                    tx.send(Ctl::Hurt(40, dir[hurts as usize % 3])).ok()?;
                    hurts += 1;
                }
                s.input.key(Action::Ads, sighted);
                // Hurt, the burst waits 280 ms: the snapshot with
                // the hit reaches the predictor first, and the kick
                // (out by 500 ms) still turns the rounds. A round
                // fired before that snapshot leaves without it, on
                // retail too.
                let burst_at = if hurt { 300..450 } else { 0..150 };
                s.input.key(Action::Attack, burst_at.contains(&ms));
                s.step(since(t0).as_secs_f64() * 1000.0).unwrap();
                s.take_commands();
                // The scene first, as the client's frame takes it: its
                // prediction fires our own shots this frame, which leave
                // along this frame's view (the next frame's has moved on
                // by the recoil).
                let scene = s.scene();
                let fired = s.take_fired();
                let Some(scene) = scene else {
                    clk_sleep(Duration::from_millis(16));
                    continue;
                };
                let own = OwnView {
                    client: Some(scene.view.client_num),
                    eye: scene.eye,
                    angles: scene.angles,
                    flash: None,
                    third_person: false,
                };
                for f in &fired {
                    if f.name == "EV_BULLET_HIT" {
                        let surf = f.state.as_ref().map_or(0, |e| ent_u32(e, "surfType"));
                        if surf == 7 {
                            bleeds.push(f.origin);
                            flesh.extend(
                                effects
                                    .plays(f, &weapons, &|_| String::new(), &HashMap::new(), None)
                                    .into_iter()
                                    .map(|p| p.key),
                            );
                        }
                        continue;
                    }
                    if !f.name.starts_with("EV_FIRE_WEAPON") {
                        continue;
                    }
                    let world = crate::cod4_fx::ShotWorld {
                        clip: &world.clip,
                        solids: &s.shot_solids,
                    };
                    for hit in crate::cod4_fx::client_bullets(
                        f,
                        &weapons,
                        &defs,
                        &HashMap::new(),
                        Some(&own),
                        world,
                    ) {
                        body = body.or(hit.body);
                        // Past the body the round goes on, and what it
                        // meets beyond plays; the body itself never.
                        if hit.body.is_some() {
                            let plays = effects.bullet_plays(f, &weapons, &hit).len();
                            own_plays += plays.saturating_sub(hit.pierced.iter().flatten().count());
                        }
                        let (seed, spread) = f.aim.map_or((-1, 0.0), |a| (a.seed, a.spread));
                        rounds.push((seed, hit.start, hit.end, hit.body, spread));
                    }
                }
                clk_sleep(Duration::from_millis(16));
            }
            s.input.key(Action::Attack, false);
            step(s, 300);
            let (stx, srx) = std::sync::mpsc::channel();
            tx.send(Ctl::Shots(stx)).ok()?;
            let mut shots = None;
            while shots.is_none() {
                step(s, 16);
                shots = srx.try_recv().ok();
            }
            Some((rounds, bleeds, flesh, own_plays, body, at, shots?))
        },
    )
    .flatten() else {
        return;
    };
    let (rounds, bleeds, flesh, own_plays, body, at, shots) = out;
    println!(
        "rounds {rounds:?}\nbleeds {bleeds:?}\nflesh {flesh:?}\nbody {body:?} at {at}\nserver {shots:?}"
    );
    assert!(rounds.len() >= 6, "{rounds:?}");
    // Most rounds stop on the bot's box and play nothing of their own
    // (hurt, the kick throws the rounds about).
    let on_body = rounds.iter().filter(|r| r.3.is_some()).count();
    assert!(body.is_some(), "{rounds:?}");
    assert!(hurt || on_body * 2 >= rounds.len(), "{rounds:?}");
    assert_eq!(own_plays, 0, "a round into a bot played an impact on him");
    // Each round is seeded by the time of the cmd that fired, the
    // server's round of that cmd: the same disc point along the same
    // view. The spread's scale is the client's last frame's and the
    // server's last end frame's, which part during a burst by where
    // the server's frames fall between the cmds' packets (round two of
    // a burst read 3 against the client's 4.6 degrees in one phase and
    // agreed in another), so a round leaves along the server's disc
    // point scaled to the client's own spread, and exactly along the
    // server's where the two scales agree.
    let dir = |a: Vec3, b: Vec3| (b - a).normalize();
    let tan = |deg: f32| (deg * 0.017_453_292).tan();
    // `Vec3::angle_between`'s f32 `acos` reads in steps near 0 (0.0396,
    // 0.0442, 0.0484, 0.0523 degrees), so one step past a true 0.049
    // read over the bound; the angle off the cross and dot in f64.
    let degrees = |a: Vec3, b: Vec3| {
        let (a, b) = (a.as_dvec3(), b.as_dvec3());
        a.cross(b).length().atan2(a.dot(b)).to_degrees()
    };
    let mut exact = 0;
    for (t, start, end, _, spread) in &rounds {
        let sv = shots.iter().find(|s| s.0 == *t);
        let sv = sv.unwrap_or_else(|| panic!("no server round at {t}: {shots:?}"));
        let (aimed, sv_end) = (sv.2, sv.3);
        // The disc point off the seed (`Bullet_RandomDir`), at the
        // client's spread, along the server's offset's direction.
        let (x, y) = vcod_cod4::bullet::random_dir(*t);
        let range = aimed.distance(sv.1);
        let off = (sv_end - aimed).normalize_or_zero() * x.hypot(y) * range * tan(*spread);
        let deg = degrees(dir(*start, *end), dir(sv.1, aimed + off));
        // Hurt, a round's kick is the server's to a frame: which frame
        // runs a cmd depends on when its packet goes out (every 33 ms
        // of the client's clock), up to 1.3 degrees in the kick's
        // return (cod4-combat.md 3.2); 3.9 without the predictor's.
        let bound = if hurt { 1.5 } else { 0.05 };
        assert!(
            deg < bound,
            "round at {t}: {deg} degrees off the server's disc point"
        );
        exact += usize::from(degrees(dir(*start, *end), dir(sv.1, sv_end)) < 0.05);
    }
    assert!(exact >= 2, "{exact} of {} exact", rounds.len());
    // The first burst's first round plays too, though the loop's first
    // frame is the session's first scene since the spawn.
    assert_eq!(rounds[0].0, shots[0].0, "{rounds:?}");
    // The server bled the axis bot where our rounds stopped on him.
    assert!(!bleeds.is_empty());
    for b in &bleeds {
        let near = rounds
            .iter()
            .filter(|r| r.3.is_some())
            .map(|r| r.2.distance(*b))
            .fold(f32::MAX, f32::min);
        assert!(near < 24.0, "bleed {b} {near} from our nearest round");
    }
    assert!(flesh.iter().any(|k| k.contains("flesh")), "{flesh:?}");
}

/// Our demolitions shotgun on vcod-server (cod4-effects.md 12.2): each
/// shot's pellets leave along the server's pellets (the seed the cmd's
/// time plus the pellet, the disc point scaled to our own spread), and
/// the shot rolls its tracer once (`DrawBulletImpacts`, iw3mp.exe
/// 0x4567f2): every pellet draws one or none does.
#[test]
fn cod4_live_own_shotgun_pellets_share_one_tracer_roll() {
    use crate::cod4_fx::OwnView;
    use crate::play::input::Action;
    let sets = [
        ("scr_game_playerwaittime", "1"),
        ("scr_game_matchstarttime", "1"),
    ];
    let Some(out) = live_join_as(
        "mp_crash",
        crash_world(),
        &sets,
        "demolitions_mp,0",
        |s, spawned, t0, tx| {
            spawned.as_ref()?;
            let install = vcod_cod4::testing::cod4_install()?;
            let ((sounds, mut effects), _) =
                crate::cod4_fly::load_av(install.root(), "mp_crash").ok()?;
            let world = s.world()?.clone();
            let mut fx = crate::fx::sim::FxSystem::new();
            let step = |s: &mut Cod4Session, ms: u64| {
                let end = clk_now() + Duration::from_millis(ms);
                while clk_now() < end {
                    s.step(since(t0).as_secs_f64() * 1000.0).unwrap();
                    s.take_commands();
                    s.scene();
                    s.take_fired();
                    clk_sleep(Duration::from_millis(16));
                }
            };
            step(s, 3000);
            let weapons = weapon_list(s.gamestate()?);
            let defs = sounds.weapons.clone();
            // (seed, spread, pellets' start and end, tracers drawn).
            let mut shots = Vec::new();
            for press in 0..8 {
                // The pump's rechamber done between two presses.
                let t1 = clk_now();
                while since(t1) < Duration::from_millis(1600) {
                    s.input
                        .key(Action::Attack, since(t1) < Duration::from_millis(100));
                    s.step(since(t0).as_secs_f64() * 1000.0).unwrap();
                    s.take_commands();
                    let scene = s.scene();
                    let fired = s.take_fired();
                    let Some(scene) = scene else {
                        clk_sleep(Duration::from_millis(16));
                        continue;
                    };
                    let own = OwnView {
                        client: Some(scene.view.client_num),
                        eye: scene.eye,
                        angles: scene.angles,
                        flash: None,
                        third_person: false,
                    };
                    for f in fired
                        .iter()
                        .filter(|f| f.entity.is_none() && f.name.starts_with("EV_FIRE_WEAPON"))
                    {
                        let shot_world = crate::cod4_fx::ShotWorld {
                            clip: &world.clip,
                            solids: &s.shot_solids,
                        };
                        let hits = crate::cod4_fx::client_bullets(
                            f,
                            &weapons,
                            &defs,
                            &HashMap::new(),
                            Some(&own),
                            shot_world,
                        );
                        let drawn = effects.tracers(&mut fx, &hits, press as f32, 16);
                        let aim = f.aim?;
                        let rounds: Vec<_> = hits.iter().map(|h| (h.start, h.end)).collect();
                        shots.push((aim.seed, aim.spread, rounds, drawn));
                    }
                    clk_sleep(Duration::from_millis(16));
                }
            }
            s.input.key(Action::Attack, false);
            step(s, 300);
            let (stx, srx) = std::sync::mpsc::channel();
            tx.send(Ctl::Shots(stx)).ok()?;
            let mut server = None;
            while server.is_none() {
                step(s, 16);
                server = srx.try_recv().ok();
            }
            Some((shots, server?))
        },
    )
    .flatten() else {
        return;
    };
    let (shots, server) = out;
    println!("client {shots:?}\nserver {server:?}");
    assert!(shots.len() >= 6, "{} shots", shots.len());
    let tan = |deg: f32| (deg * 0.017_453_292).tan();
    let dir = |a: Vec3, b: Vec3| (b - a).normalize();
    let mut rolled = [0; 2];
    for (seed, spread, rounds, drawn) in &shots {
        assert!(rounds.len() > 1, "not a shotgun: {} rounds", rounds.len());
        assert!(
            *drawn == 0 || *drawn == rounds.len(),
            "{drawn} tracers for {} pellets",
            rounds.len()
        );
        rolled[usize::from(*drawn > 0)] += 1;
        let pellets: Vec<_> = server.iter().filter(|r| r.0 == *seed).collect();
        assert_eq!(pellets.len(), rounds.len(), "server pellets at {seed}");
        for (k, ((start, end), sv)) in rounds.iter().zip(&pellets).enumerate() {
            let (x, y) = vcod_cod4::bullet::random_dir(seed + k as i32);
            let (aimed, sv_end) = (sv.2, sv.3);
            let range = aimed.distance(sv.1);
            let off = (sv_end - aimed).normalize_or_zero() * x.hypot(y) * range * tan(*spread);
            let deg = dir(*start, *end)
                .angle_between(dir(sv.1, aimed + off))
                .to_degrees();
            assert!(deg < 0.05, "pellet {k} at {seed}: {deg} degrees off");
        }
    }
    assert!(rolled[0] > 0 && rolled[1] > 0, "rolls {rolled:?}");
}

/// An axis bot pinned 200 units ahead on vcod-server fires at us
/// (cod4-effects.md 12.2): each of his fire events takes the spread
/// of the stance and sight his entity's legs anim reads, as 1.7's
/// `CG_GuessSpreadForWeapon` and 0x456200 read it, through the anim
/// set's movetype bits; every legs anim he shoots in names a movetype
/// but a `fireweapon` clause's (a throw's, which names none and reads
/// standing) on a standing or crouched body, a standing body's never a
/// crouched or prone one, a crouched one only on a crouched body (a
/// crouched run reads standing).
#[test]
fn cod4_live_a_bots_rounds_spread_by_his_legs_anim() {
    use crate::play::cod4_players::Cod4Players;
    use vcod_cod4::pmove::anim::{ANIM_TOGGLEBIT, LegsPose};
    let sets = [
        ("scr_game_playerwaittime", "1"),
        ("scr_game_matchstarttime", "1"),
    ];
    let Some(out) = live_join_on("mp_crash", crash_world(), &sets, |s, spawned, t0, tx| {
        spawned.as_ref()?;
        let install = vcod_cod4::testing::cod4_install()?;
        let (_, _, src) =
            crate::cod4_fly::load_bodies(install.root(), "mp_crash", &s.entity_assets()).ok()?;
        let mut players = Cod4Players::default();
        let table = players.legs_movetypes(&src)?;
        let throws = players.fire_event_legs(&src)?;
        s.legs_movetypes = Some(table.clone());
        let world = s.world()?.clone();
        // Past the prematch's freeze.
        let wait = clk_now();
        while since(wait) < Duration::from_secs(30)
            && s.client()
                .snapshots
                .last()
                .and_then(|x| x.ps.get("pm_flags"))
                .is_some_and(|f| f & 0x800 != 0)
        {
            s.step(since(t0).as_secs_f64() * 1000.0).unwrap();
            s.take_commands();
            s.take_fired();
            clk_sleep(Duration::from_millis(16));
        }
        let ps = &s.client().snapshots.last()?.ps;
        let feet = vec3(|k| ps_f32(ps, &format!("origin[{k}]")));
        let eye = feet + Vec3::Z * 60.0;
        let yaw = (0..24).map(|k| k as f32 * 15.0).find(|y| {
            let d = Vec3::new(y.to_radians().cos(), y.to_radians().sin(), 0.0);
            let t = world
                .clip
                .trace(eye, eye + d * 400.0, Vec3::ZERO, Vec3::ZERO, 0x280_6831);
            let down = world.clip.trace(
                feet + d * 200.0 + Vec3::Z * 8.0,
                feet + d * 200.0 - Vec3::Z * 8.0,
                Vec3::ZERO,
                Vec3::ZERO,
                1,
            );
            t.fraction == 1.0 && down.fraction < 1.0
        })?;
        let d = Vec3::new(yaw.to_radians().cos(), yaw.to_radians().sin(), 0.0);
        tx.send(Ctl::PinShooter((feet + d * 200.0).to_array()))
            .ok()?;
        s.view = [0.0, yaw];
        let mut shots = Vec::new();
        let t1 = clk_now();
        while since(t1) < Duration::from_secs(20) && shots.len() < 12 {
            s.step(since(t0).as_secs_f64() * 1000.0).unwrap();
            s.take_commands();
            // The window's frame: its scene loads the level's defs.
            s.scene();
            for f in s.take_fired() {
                // Another's shot: our own carry no entity.
                let (Some(_), Some(e), Some(aim)) = (f.entity, f.state.as_ref(), f.aim) else {
                    continue;
                };
                if !f.name.starts_with("EV_FIRE_WEAPON") {
                    continue;
                }
                let wire = ent_u32(e, "legsAnim") & !ANIM_TOGGLEBIT;
                let def = s.level_def(f.weapon)?.clone();
                shots.push((wire, ent_u32(e, "lerp.eFlags"), aim, def));
            }
            clk_sleep(Duration::from_millis(16));
        }
        let names: Vec<_> = shots
            .iter()
            .map(|x| players.anim_name(&src, x.0 as i32).unwrap_or_default())
            .collect();
        Some((shots, table, throws, names))
    }) else {
        return;
    };
    let Some((shots, table, throws, names)) = out else {
        return;
    };
    println!(
        "{:?}",
        shots
            .iter()
            .zip(&names)
            .map(|((w, f, a, _), n)| (w, n, f, table[*w as usize], a.spread))
            .collect::<Vec<_>>()
    );
    assert!(shots.len() >= 3, "the bot fired {} rounds", shots.len());
    for ((wire, flags, aim, def), name) in shots.iter().zip(&names) {
        let mask = table[*wire as usize];
        // A `fireweapon` clause's legs anim (`pb_stand_grenade_throw`
        // on `movetype idle`, `pb_crouch_grenade_throw` on `idlecr`,
        // which the next rounds still fire in) is in no state's block:
        // movetype 0, read as standing (cod4-effects.md 12.2). Its
        // timer holds it through a crouch or a stand: the script has no
        // event between the two (cod4-player-anims.md 7). Lying down
        // has one (`crouch_to_prone`'s `both pb_crouch2prone`).
        if mask == 0 {
            assert!(
                throws.contains(wire),
                "legs anim {wire} {name} names no movetype and no fire event's"
            );
            assert!(flags & 8 == 0, "{wire} {name} on a prone body");
        }
        let legs = LegsPose::of(mask);
        if flags & 0xc == 0 {
            assert!(!legs.crouched && !legs.prone, "{wire} standing: {legs:?}");
        }
        if legs.crouched {
            assert!(flags & 4 != 0, "{wire} crouched on a body standing");
        }
        // The speed scale is the snapshots', not ours: the spread
        // lies between its stance's near end and its far end.
        let near = crate::cod4_fx::Aim::other(def, legs, 0.0, aim.seed).spread;
        let far = crate::cod4_fx::Aim::other(def, legs, 255.0, aim.seed).spread;
        assert!(
            aim.spread >= near.min(far) - 1e-4 && aim.spread <= near.max(far) + 1e-4,
            "{wire} {legs:?}: spread {} outside {near}..{far}",
            aim.spread
        );
    }
}

/// `class`'s primary on vcod-server, from the hip and then sighted,
/// two presses each (held `hold` frames), in the window's frame order
/// (step, scene, events): every predicted own `EV_FIRE_WEAPON`'s seed
/// (the time of the cmd that fired it) with whether it was sighted, and
/// the times of the rounds the server's `Bullet_Fire` fired for us.
#[allow(clippy::type_complexity)]
fn own_rounds(class: &'static str, hold: usize) -> Option<(Vec<(i32, bool)>, Vec<i32>)> {
    use crate::play::input::Action;
    let sets = [
        ("scr_game_playerwaittime", "1"),
        ("scr_game_matchstarttime", "1"),
    ];
    live_join_as(
        "mp_crash",
        crash_world(),
        &sets,
        class,
        move |s, spawned, t0, tx| {
            spawned.as_ref()?;
            let mut seeds = Vec::new();
            let frame = |s: &mut Cod4Session, ads: bool, seeds: &mut Vec<(i32, bool)>| {
                s.step(since(t0).as_secs_f64() * 1000.0).unwrap();
                s.take_commands();
                s.scene();
                for f in s.take_fired() {
                    if f.entity.is_none() && f.name.starts_with("EV_FIRE_WEAPON") {
                        seeds.push((f.aim.map_or(-1, |a| a.seed), ads));
                    }
                }
                clk_sleep(Duration::from_millis(16));
            };
            let snap = |s: &Cod4Session, n: &str| {
                let ps = &s.client().snapshots.last().unwrap().ps;
                (ps.get(n).unwrap_or(0), ps_f32(ps, n))
            };
            // Not frozen, and the weapon ready on the server's clock.
            let ready = |s: &Cod4Session| {
                snap(s, "pm_flags").0 & 0x800 == 0
                    && snap(s, "weaponstate").0 == 0
                    && snap(s, "weaponTime").0 == 0
            };
            for ads in [false, true] {
                s.input.key(Action::Ads, ads);
                for _ in 0..2 {
                    let wait = clk_now();
                    while since(wait) < Duration::from_secs(30)
                        && !(ready(s) && (!ads || snap(s, "fWeaponPosFrac").1 >= 1.0))
                    {
                        frame(s, ads, &mut seeds);
                    }
                    s.input.key(Action::Attack, true);
                    for _ in 0..hold {
                        frame(s, ads, &mut seeds);
                    }
                    s.input.key(Action::Attack, false);
                    for _ in 0..30 {
                        frame(s, ads, &mut seeds);
                    }
                }
            }
            s.input.key(Action::Ads, false);
            // The server's rounds once the last snapshot carrying ours is in.
            for _ in 0..30 {
                frame(s, false, &mut seeds);
            }
            let (stx, srx) = std::sync::mpsc::channel();
            tx.send(Ctl::Shots(stx)).ok()?;
            let mut shots = None;
            while shots.is_none() {
                frame(s, false, &mut seeds);
                shots = srx.try_recv().ok();
            }
            let mut times: Vec<i32> = shots?.iter().map(|r| r.0).collect();
            times.dedup();
            Some((seeds, times))
        },
    )
    .flatten()
}

/// Every round the server fires for our presses plays once on the
/// client, the first of each burst included (cod4-effects.md 12.2):
/// the M16's three-round bursts, the MP5 held, the M40A3's single
/// shots, from the hip and sighted.
#[test]
fn cod4_live_every_server_round_fires_once() {
    for (class, hold) in [
        ("assault_mp,0", 8),
        ("specops_mp,0", 20),
        ("sniper_mp,0", 4),
    ] {
        let Some((seeds, times)) = own_rounds(class, hold) else {
            return;
        };
        println!("{class}: client {seeds:?}\nserver {times:?}");
        for ads in [false, true] {
            let n = seeds.iter().filter(|s| s.1 == ads).count();
            assert!(n >= 2, "{class} ads {ads}: {seeds:?}");
        }
        let ours: Vec<i32> = seeds.iter().map(|s| s.0).collect();
        assert_eq!(ours, times, "{class}");
    }
}

/// Our own shots on vcod-server fire off the prediction
/// (`CG_CheckPlayerstateEvents` on the predicted state, cod4-effects.md
/// 12): in the window's frame order (step, scene, events) each burst's
/// first `EV_FIRE_WEAPON` fires on the frame `+attack` goes down, with
/// its flash and its client-side impact, and every round fires once:
/// as many own fire events as the server's snapshots carry in our ring,
/// none of those again when they arrive a round trip later.
#[test]
fn cod4_live_own_shots_fire_the_frame_they_press() {
    use crate::cod4_fx::OwnView;
    use crate::play::input::Action;
    let Some(out) = live_join(|s, spawned, t0| {
        spawned.as_ref()?;
        let install = vcod_cod4::testing::cod4_install()?;
        let ((sounds, mut effects), _) =
            crate::cod4_fly::load_av(install.root(), "mp_crash").ok()?;
        let world = s.world()?.clone();
        let frozen = |s: &Cod4Session| {
            s.client()
                .snapshots
                .last()
                .and_then(|x| x.ps.get("pm_flags"))
                .is_some_and(|f| f & 0x800 != 0)
        };
        // The weapon ready on the server's clock (`weaponstate` 0, no
        // `weaponTime` left): a loaded machine spawns us late, so the
        // spawn's raise can still run where a press would land.
        let ready = |s: &Cod4Session| {
            s.client().snapshots.last().is_some_and(|x| {
                let u = |n: &str| x.ps.get(n).unwrap_or(0);
                u("weaponstate") == 0 && u("weaponTime") == 0
            })
        };
        let wait = clk_now();
        while (frozen(s) || !ready(s)) && since(wait) < Duration::from_secs(60) {
            s.step(since(t0).as_secs_f64() * 1000.0).unwrap();
            s.take_commands();
            s.scene();
            s.take_fired();
            clk_sleep(Duration::from_millis(16));
        }
        s.view[0] = 45.0;
        // The server's own count: fire events in our snapshot ring, by
        // the frame they arrived.
        let mut seen = s
            .client()
            .snapshots
            .last()
            .map(|x| x.ps.get("eventSequence").unwrap_or(0) & 0xff);
        let mut walked = s.client().snapshots.len();
        let (mut frame, mut held) = (0usize, false);
        let mut presses = Vec::new();
        let (mut ours, mut theirs) = (Vec::new(), Vec::new());
        let (mut flashes, mut impacts) = (0, 0);
        let t1 = clk_now();
        // Two bursts of 30 frames (about 300..800 and 1300..1800 ms at
        // 60 Hz), counted in frames as a loaded machine stretches
        // them; the second waits for the weapon to settle after the
        // first. 72 frames past the second, and on until the server's
        // ring has every predicted round: a starved server sends the
        // last bursts' late.
        let mut bursts = [(18usize, 48usize), (78, 108)];
        while frame < bursts[1].1 + 72
            || (theirs.len() < ours.len() && since(t1) < Duration::from_secs(30))
        {
            if !held && bursts[1].0 == frame && presses.len() == 1 && !ready(s) {
                bursts[1].0 += 1;
                bursts[1].1 += 1;
            }
            let want = bursts.iter().any(|&(a, b)| (a..b).contains(&frame));
            if want != held {
                s.input.key(Action::Attack, want);
                held = want;
                if want {
                    presses.push((frame, since(t1)));
                }
            }
            s.step(since(t0).as_secs_f64() * 1000.0).unwrap();
            s.take_commands();
            let scene = s.scene();
            let fired = s.take_fired();
            let snaps = &s.client().snapshots;
            for x in &snaps[walked..] {
                let seq = x.ps.get("eventSequence").unwrap_or(0) & 0xff;
                if let Some(prev) = seen {
                    let to = if seq < prev { seq + 256 } else { seq };
                    for q in prev.max(to.saturating_sub(4))..to {
                        let id = x.ps.get(&format!("events[{}]", q & 3)).unwrap_or(0);
                        if event_name(id).is_some_and(|n| n.starts_with("EV_FIRE_WEAPON")) {
                            theirs.push((frame, since(t1)));
                        }
                    }
                }
                seen = Some(seq);
            }
            walked = snaps.len();
            let weapons = weapon_list(s.gamestate().unwrap());
            let cs = |k: u16| {
                String::from_utf8_lossy(s.gamestate().unwrap().configstring(k)).into_owned()
            };
            for f in fired
                .iter()
                .filter(|f| f.name.starts_with("EV_FIRE_WEAPON"))
            {
                assert!(f.entity.is_none(), "{f:?}");
                ours.push((frame, since(t1)));
                flashes += effects
                    .plays(f, &weapons, &cs, &HashMap::new(), None)
                    .iter()
                    .filter(|p| p.key.starts_with("muzzleflashes/"))
                    .count();
                let Some(scene) = &scene else { continue };
                let own = OwnView {
                    client: Some(scene.view.client_num),
                    eye: scene.eye,
                    angles: scene.angles,
                    flash: None,
                    third_person: false,
                };
                let defs = sounds.weapons.clone();
                if let Some(hit) = first_bullet(
                    f,
                    &weapons,
                    &defs,
                    &HashMap::new(),
                    Some(&own),
                    &world.clip,
                    &s.shot_solids,
                ) {
                    impacts += effects.bullet_plays(f, &weapons, &hit).len();
                }
            }
            frame += 1;
            clk_sleep(Duration::from_millis(16));
        }
        s.input.key(Action::Attack, false);
        Some((presses, ours, theirs, flashes, impacts))
    })
    .flatten() else {
        return;
    };
    let (presses, ours, theirs, flashes, impacts) = out;
    let first = |v: &[(usize, Duration)], p: &(usize, Duration)| {
        v.iter().find(|x| x.0 >= p.0).copied().unwrap()
    };
    for p in &presses {
        let (o, t) = (first(&ours, p), first(&theirs, p));
        println!(
            "press at frame {}: predicted fire {} frames / {} ms later, the server's {} frames / {} ms",
            p.0,
            o.0 - p.0,
            (o.1 - p.1).as_millis(),
            t.0 - p.0,
            (t.1 - p.1).as_millis()
        );
        assert!(o.0 - p.0 <= 1, "{p:?} {o:?}");
    }
    println!(
        "{} predicted fire events, {} in the server's ring, {flashes} flashes, {impacts} impacts",
        ours.len(),
        theirs.len()
    );
    assert_eq!(presses.len(), 2);
    assert!(ours.len() >= 4, "{ours:?}");
    assert_eq!(ours.len(), theirs.len(), "{ours:?} {theirs:?}");
    assert_eq!((flashes, impacts), (ours.len(), ours.len()));
}
