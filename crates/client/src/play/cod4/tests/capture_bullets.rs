//! Committed captures' rounds: client-side and server-side bullets, their
//! marks, footstep surfaces and the shooter's aim.

use std::collections::BTreeMap;

use super::*;

/// Each capture's client-side bullets (cod4-effects.md 12): the other
/// players' shots traced from their posed `tag_flash` along their
/// view, the world impacts' effects and sounds, and the `whizby`s the
/// drawn eye hears. Returns per shot the hit's distance and surface,
/// the impact names and the whizbys.
fn replay_bullets(name: &str) -> Option<Bullets> {
    use crate::play::cod4_players::Cod4Players;
    let install = vcod_cod4::testing::cod4_install()?;
    let world = crash_world()?;
    let text = std::fs::read_to_string(format!("{FIXTURES}/{name}")).unwrap();
    let mut s = Cod4Session::open_capture(&text).unwrap();
    let (_, models, src) =
        crate::cod4_fly::load_bodies(install.root(), "mp_crash", &s.entity_assets()).unwrap();
    let ((mut sounds, mut effects), _) =
        crate::cod4_fly::load_av(install.root(), "mp_crash").unwrap();
    let mut players = Cod4Players::default();
    let (mut shots, mut names, mut whizby) = (Vec::new(), BTreeMap::new(), 0);
    let mut ms = 0.0;
    let mut eye = None;
    while !s.finished() {
        ms += 1000.0 / 60.0;
        s.step(ms).unwrap();
        let fired = s.take_fired();
        let Some(scene) = s.scene() else { continue };
        let weapons = weapon_list(s.gamestate().unwrap());
        for f in &fired {
            let defs = sounds.weapons.clone();
            let Some(hit) = first_bullet(
                f,
                &weapons,
                &defs,
                &players.tags,
                None,
                &world.clip,
                &s.shot_solids,
            ) else {
                continue;
            };
            assert!(hit.hit, "{hit:?}");
            shots.push(((hit.end - hit.start).length(), hit.surf));
            for p in effects.bullet_plays(f, &weapons, &hit) {
                *names.entry(p.key).or_default() += 1;
            }
            for c in sounds.bullet_cues(f, &weapons, &hit, eye) {
                if c.alias == "whizby" {
                    whizby += 1;
                } else {
                    *names.entry(c.alias).or_default() += 1;
                }
            }
        }
        let views: Vec<(u32, &BodyView)> = scene
            .entities
            .iter()
            .filter_map(|d| Some((d.number, d.body.as_ref()?)))
            .collect();
        players.posed(
            &src,
            &models,
            &views,
            &[],
            scene.render_time,
            scene.server_time,
        );
        eye = Some(scene.eye);
    }
    assert!(sounds.missing.is_empty(), "{:?}", sounds.missing);
    assert!(effects.missing.is_empty(), "{:?}", effects.missing);
    println!("{name}: shots {shots:?}, {names:?}, {whizby} whizbys");
    Some((shots, names, whizby))
}

/// The spectator capture's 18 M16 shots each strike the world, the
/// table's impact and the surface's `bullet_large_*` playing at each.
/// The kill target hears none of the shooter's rounds go by: they
/// strike the world under 400 units out, most under 90, and 1.7 plays a
/// whizby only to an eye 64 clear of both ends (cod4-effects.md 12).
#[test]
fn client_side_bullets_strike_the_world() {
    let Some((shots, names, _)) = replay_bullets("play-1.7-spec.txt") else {
        return;
    };
    assert_eq!(shots.len(), 18);
    let effects: usize = names
        .iter()
        .filter(|(k, _)| k.ends_with(".efx"))
        .map(|(_, n)| n)
        .sum();
    let large: usize = names
        .iter()
        .filter(|(k, _)| k.starts_with("bullet_large_"))
        .map(|(_, n)| n)
        .sum();
    // One round passes a plaster wall (the penetration walk): its far
    // side plays the exit effect and sound, and the wood it meets next
    // its own, two more of each (cod4-effects.md 12.1).
    assert_eq!((effects, large), (20, 20), "{names:?}");
    for k in [
        "impacts/large_plaster_1_exit.efx",
        "bullet_large_exit_plaster",
        "impacts/large_woodhit.efx",
        "bullet_large_wood",
    ] {
        assert_eq!(names.get(k), Some(&1), "{k} in {names:?}");
    }
    assert!(
        shots.iter().all(|&(d, _)| d > 16.0 && d < 4000.0),
        "{shots:?}"
    );
    let (target, _, whizby) = replay_bullets("play-1.7-kill-target.txt").unwrap();
    assert!(!target.is_empty());
    assert_eq!(whizby, 0);
}

/// What a capture's frames give [`crate::cod4_fx::frame_rounds`], the
/// rounds the client's frame plays.
struct Replayed {
    /// `sv_clientSideBullets` off the systeminfo.
    client_side: bool,
    /// The shots that ran client-side rounds.
    pub(super) shots: usize,
    /// Each server bullet hit event's round, whether the drawn eye
    /// hears its whizby and whether its shooter is the drawn view's
    /// client.
    pub(super) server: Vec<(crate::cod4_fx::ServerRound, bool, bool)>,
    /// The dynamic entities the server rounds' legs struck: the leg
    /// and where.
    struck: Vec<((Vec3, Vec3), Vec3)>,
}

/// [`Replayed`] of a capture, the shooters posed as the client poses
/// them.
fn replay_rounds(name: &str) -> Option<Replayed> {
    use crate::play::cod4_players::Cod4Players;
    let install = vcod_cod4::testing::cod4_install()?;
    let world = crash_world()?;
    let text = std::fs::read_to_string(format!("{FIXTURES}/{name}")).unwrap();
    let mut s = Cod4Session::open_capture(&text).unwrap();
    s.set_world(world.clone());
    let si = s
        .gamestate()
        .and_then(|g| g.systeminfo("sv_clientSideBullets"));
    let client_side = crate::cod4_fx::client_side_bullets(si.as_deref());
    let (_, models, src) =
        crate::cod4_fly::load_bodies(install.root(), "mp_crash", &s.entity_assets()).unwrap();
    let ((sounds, _), _) = crate::cod4_fly::load_av(install.root(), "mp_crash").unwrap();
    let mut players = Cod4Players::default();
    let (mut shots, mut server, mut struck) = (0, Vec::new(), Vec::new());
    let mut ms = 0.0;
    while !s.finished() {
        ms += 1000.0 / 60.0;
        s.step(ms).unwrap();
        let fired = s.take_fired();
        let Some(scene) = s.scene() else { continue };
        let weapons = weapon_list(s.gamestate().unwrap());
        let own = crate::cod4_fx::OwnView {
            client: Some(scene.view.client_num),
            eye: scene.eye,
            angles: scene.angles,
            flash: None,
            third_person: false,
        };
        let w = crate::cod4_fx::ShotWorld {
            clip: &world.clip,
            solids: &s.shot_solids,
        };
        let (hits, rounds) = crate::cod4_fx::frame_rounds(
            &fired,
            &weapons,
            &sounds.weapons,
            &players.tags,
            Some(&own),
            w,
            client_side,
            &s.client().snapshots.last().unwrap().entities,
        );
        shots += hits.iter().filter(|h| !h.is_empty()).count();
        // The legs on the map's dynamic entities, as the frame hands
        // them on.
        let legs: Vec<Vec<_>> = rounds
            .iter()
            .map(|r| r.and_then(|r| r.leg).into_iter().collect())
            .collect();
        if let Some(d) = s.dynents.as_mut() {
            let now = scene.render_time;
            let done =
                crate::play::cod4_dynents::on_fired(d, &world, &fired, &weapons, &legs, None, now);
            struck.extend(done.struck.iter().map(|(i, st)| (legs[*i][0], st.at)));
        }
        for r in rounds.into_iter().flatten() {
            let heard = crate::cod4_audio::whizby_cue(r.muzzle, r.end, scene.eye).is_some();
            server.push((r, heard, scene.view.client_num == r.shooter));
        }
        let views: Vec<(u32, &BodyView)> = scene
            .entities
            .iter()
            .filter_map(|d| Some((d.number, d.body.as_ref()?)))
            .collect();
        players.posed(
            &src,
            &models,
            &views,
            &[],
            scene.render_time,
            scene.server_time,
        );
    }
    Some(Replayed {
        client_side,
        shots,
        server,
        struck,
    })
}

/// Retail under `+set sv_clientSideBullets 0` (`play-1.7-csb0-*`, the
/// wall shot of cod4-combat.md 3.1: a target outside mp_crash's metal
/// plate, a shooter inside aiming over his head): no client runs a
/// round of its own, and the server's events carry each, the plate's
/// entry, its exit (`un1` 4) and the cloth far beyond. Each entry's leg
/// runs from the world's origin, the server leaving the events'
/// `lerp.u` at 0; an exit's pushes nothing. The target hears every
/// round that went on past him, none that stopped short; the shooter,
/// his own rounds leaving from the drawn eye, hears none
/// (cod4-effects.md 12.4).
#[test]
fn server_side_bullets_carry_every_round() {
    for name in ["play-1.7-csb0-target.txt", "play-1.7-csb0-shooter.txt"] {
        let Some(Replayed {
            client_side,
            shots,
            server,
            struck,
        }) = replay_rounds(name)
        else {
            return;
        };
        assert!(!client_side, "{name}");
        assert_eq!(shots, 0, "{name}: client-side shots");
        let n = server.len();
        let exits = server.iter().filter(|(r, ..)| r.leg.is_none()).count();
        assert!(
            n > 30 && exits > 0 && exits < n,
            "{name}: {exits} exits of {n}"
        );
        for (r, ..) in &server {
            assert_eq!(r.shooter, 1, "{name}");
            if let Some(leg) = r.leg {
                assert_eq!(leg, (Vec3::ZERO, r.end), "{name}");
            }
        }
        // The rounds that went on past the target to the cloth, 300
        // units and more past him (y 2380).
        let past = |own: bool| {
            let p = server
                .iter()
                .filter(|(r, _, o)| r.end.y > 2600.0 && *o == own);
            p.fold((0, 0), |(n, h), (_, w, _)| (n + 1, h + *w as usize))
        };
        let ((theirs, heard), (own, own_heard)) = (past(false), past(true));
        let short = server
            .iter()
            .filter(|(r, w, _)| r.end.y < 2600.0 && *w)
            .count();
        println!(
            "{name}: {n} rounds, {exits} exits; past: {heard} of {theirs} heard, \
                 {own_heard} of {own} with the shooter's view"
        );

        assert_eq!((short, own_heard), (0, 0), "{name}");
        if name.contains("target") {
            assert!(theirs > 0 && heard == theirs, "{name}");
        }
        // No prop lies on the lines from the world's origin to these
        // contacts.
        assert!(struck.is_empty(), "{name}: {struck:?}");
    }
    // Under 1 the shots run here and the events push nothing.
    let r = replay_rounds("play-1.7-kill-target.txt").unwrap();
    assert!(r.client_side && r.shots > 0);
    assert!(!r.server.is_empty() && r.server.iter().all(|(r, ..)| r.leg.is_none()));
    assert!(r.struck.is_empty());
}

/// Retail's footstep and landing parms against the ground our clip map
/// gives under the stepping body (cod4-sound.md 4): every step of
/// each capture names the surface type a trace down from its origin
/// reports.
#[test]
fn footstep_surfaces_match_the_clip_map() {
    use vcod_cod4::collision::MASK_PLAYERSOLID;
    let Some(world) = crash_world() else { return };
    let mut by: BTreeMap<(u32, u32), usize> = BTreeMap::new();
    for name in [
        "play-1.7-walk.txt",
        "play-1.7-spec.txt",
        "play-1.7-kill-target.txt",
    ] {
        let text = std::fs::read_to_string(format!("{FIXTURES}/{name}")).unwrap();
        let mut s = Cod4Session::open_capture(&text).unwrap();
        let mut ms = 0.0;
        while !s.finished() {
            ms += 1000.0 / 60.0;
            s.step(ms).unwrap();
            for f in s.take_fired() {
                if !f.name.starts_with("EV_FOOTSTEP") {
                    continue;
                }
                let o = f.origin;
                let t = world.clip.trace(
                    o + glam::Vec3::Z,
                    o - glam::Vec3::Z * 16.0,
                    glam::Vec3::new(-15.0, -15.0, 0.0),
                    glam::Vec3::new(15.0, 15.0, 0.0),
                    MASK_PLAYERSOLID,
                );
                let ours = if t.surface_flags & 0x2000 != 0 {
                    0
                } else {
                    (t.surface_flags & 0x1f0_0000) >> 20
                };
                *by.entry((f.parm, ours)).or_default() += 1;
            }
        }
    }
    println!("(retail parm, our ground) {by:?}");
    // One asphalt step reads the caulk under a curb's edge: the event
    // carries the snapshot's origin, not the cmd's.
    let (right, wrong): (Vec<_>, Vec<_>) = by.iter().partition(|((a, b), _)| a == b);
    let n = |v: &[(&(u32, u32), &usize)]| v.iter().map(|(_, n)| **n).sum::<usize>();
    assert!(n(&right) >= 19 && n(&wrong) <= 1, "{by:?}");
    assert!(right.iter().any(|((a, _), _)| *a == 17) && right.iter().any(|((a, _), _)| *a == 22));
}

/// The kill shooter's own shots as a live client plays them
/// (cod4-effects.md 12): each traced from the drawn eye along the
/// view, its impact effect and sound off the table, the decal its
/// effect places landing on the map's mark world in a material the
/// renderer holds, and about half drawing a `gfx_tracer` beam from
/// the view weapon's muzzle.
#[test]
fn own_shots_strike_mark_and_trace() {
    use crate::cod4_fx::OwnView;
    use crate::fx::sim::FxSystem;
    let Some(install) = vcod_cod4::testing::cod4_install() else {
        return;
    };
    let text = std::fs::read_to_string(format!("{FIXTURES}/play-1.7-kill-shooter.txt")).unwrap();
    let mut s = Cod4Session::open_capture(&text).unwrap();
    let mut map =
        crate::cod4_fly::load(install.root(), None, "mp_crash", &s.entity_assets()).unwrap();
    let (mut sounds, mut effects) = map.av.take().unwrap();
    let world = map.world.clone().unwrap();
    let fs = vcod_common::pk3::Pk3Fs::empty();
    let mut fx = FxSystem::new();
    for (k, v) in effects.take_effects().unwrap() {
        fx.insert(&k, v);
    }
    let marks_world = crate::marks::MarkWorld::build(&map.bsp, |_| true);
    let (mut shots, mut impacts, mut cues, mut marks, mut placed) = (0, 0, 0, Vec::new(), 0);
    let mut pierced: BTreeMap<(bool, bool), usize> = BTreeMap::new();
    let mut bodies = 0;
    let mut now = 0.0f32;
    let mut ms = 0.0;
    while !s.finished() {
        ms += 1000.0 / 60.0;
        now += 1.0 / 60.0;
        s.step(ms).unwrap();
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
        for f in fired.iter().filter(|f| f.entity.is_none()) {
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
            assert!(hit.own && hit.hit, "{hit:?}");
            // In third person the same round's tracer starts 30 along
            // its way from the eye (cod4-effects.md 12.3).
            let third = OwnView {
                third_person: true,
                ..own
            };
            let t = first_bullet(
                f,
                &weapons,
                &defs,
                &HashMap::new(),
                Some(&third),
                &world.clip,
                &s.shot_solids,
            )
            .unwrap();
            assert_eq!((t.start, t.end), (hit.start, hit.end));
            assert_eq!(hit.muzzle, hit.start);
            let lead = t.muzzle - t.start;
            assert!((lead.length() - 30.0).abs() < 1e-3, "{lead}");
            assert!(lead.normalize().dot((t.end - t.start).normalize()) > 0.97);
            shots += 1;
            // A round into the target's box plays nothing here.
            bodies += usize::from(hit.body.is_some());
            // A penetrating round's further contacts play too; one
            // into a caulk floor (surface 0) only its sound.
            for p in hit.pierced.iter().flatten() {
                *pierced.entry((p.exit, p.surf == 0)).or_default() += 1;
            }
            effects.tracers(&mut fx, std::slice::from_ref(&hit), now, 16);
            for p in effects.bullet_plays(f, &weapons, &hit) {
                impacts += 1;
                effects.start(&mut fx, &fs, &p, f64::from(now), None);
            }
            cues += sounds.bullet_cues(f, &weapons, &hit, Some(scene.eye)).len();
        }
        fx.step(1.0 / 60.0, now, None);
        fx.spawn_chained(&fs, now);
        for m in fx.take_marks(now) {
            placed += usize::from(
                !crate::marks::place(&marks_world, &m, (now * 1000.0) as i32).is_empty(),
            );
            marks.push(m.shader);
        }
    }
    println!(
        "{shots} own shots ({bodies} into a body), {impacts} impacts, {cues} cues, {} tracers, marks {marks:?}, {placed} placed, pierced {pierced:?}",
        effects.tracers
    );
    assert!(shots >= 10, "{shots}");
    let further: usize = pierced.values().sum();
    let bare = pierced
        .iter()
        .filter(|((_, b), _)| *b)
        .map(|(_, n)| n)
        .sum::<usize>();
    assert_eq!(
        (impacts, cues),
        (shots - bodies + further - bare, shots - bodies + further)
    );
    assert_eq!(marks.len(), impacts, "{marks:?}");
    assert_eq!(placed, impacts);
    assert!(
        marks.iter().all(|m| map.materials.get(m).is_some()),
        "{marks:?}"
    );
    // `cg_firstPersonTracerChance` 0.5.
    assert!(
        effects.tracers * 4 > shots && effects.tracers * 4 < shots * 3,
        "{}",
        effects.tracers
    );
    assert!(
        map.materials
            .get(crate::fx::cod2_tracer::COD4.material)
            .is_some()
    );
}

/// The kill captures' shots, hits and the killcam's, from both ends.
#[test]
fn kill_captures_play_their_hits() {
    let Some(shooter) = replay_av("play-1.7-kill-shooter.txt") else {
        return;
    };
    let target = replay_av("play-1.7-kill-target.txt").unwrap();
    for n in [&shooter, &target] {
        assert!(n.sounds_missing.is_empty(), "{:?}", n.sounds_missing);
        assert!(n.effects_missing.is_empty(), "{:?}", n.effects_missing);
    }
    // The shooter hears its own shots as the player.
    assert!(
        shooter.sounds.keys().any(|k| k.ends_with("_plr")),
        "{:?}",
        shooter.sounds
    );
    assert!(
        target
            .sounds
            .keys()
            .any(|k| k.starts_with("bullet_large_flesh")),
        "{:?}",
        target.sounds
    );
}

/// The shooter's own view is its cmds' (snapshots leave it out): while
/// it aims down its sights at the target, the drawn view points at the
/// target's body.
#[test]
fn shooter_view_is_its_cmds_aim() {
    let text = std::fs::read_to_string(format!("{FIXTURES}/play-1.7-kill-shooter.txt")).unwrap();
    let mut s = Cod4Session::open_capture(&text).unwrap();
    let (mut on, mut off) = (0, 0);
    let mut ms = 0.0;
    while !s.finished() {
        ms += 1000.0 / 60.0;
        s.step(ms).unwrap();
        let Some(scene) = s.scene() else { continue };
        let own = s.gamestate().unwrap().client_num as u32;
        let Some(target) = scene
            .entities
            .iter()
            .find(|d| d.etype == ET_PLAYER && d.number != own)
        else {
            continue;
        };
        let d = target.origin + Vec3::Z * 40.0 - scene.eye;
        let yaw = d.y.atan2(d.x).to_degrees();
        let off_by = ((yaw - scene.angles.y + 540.0) % 360.0 - 180.0).abs();
        if off_by < 5.0 {
            on += 1;
        } else {
            off += 1;
        }
    }
    assert!(on > 3 * off, "on target {on} frames, off {off}");
}
