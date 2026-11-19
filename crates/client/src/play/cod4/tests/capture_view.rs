//! Committed captures' own view: smooth turns, third person, prediction
//! and the view weapon.

use std::collections::BTreeMap;

use vcod_cod4::net::events::event_name;

use super::*;
use crate::play::cod4::scene::eased_cmd;

/// The view of a capture played at `hz`, drawn off the snapshots: per
/// frame the drawn time, the eye's step from the last frame and whether
/// the view started over.
fn view_steps(name: &str, hz: f64) -> Vec<(i32, f32, bool)> {
    let (steps, _) = view_steps_in(name, hz, None);
    steps.iter().map(|&(t, d, r)| (t, d.length(), r)).collect()
}

/// A weapon looks up by its name in any case in all four of the
/// world's maps (BUG-121).
#[test]
fn a_weapon_name_looks_up_in_any_case() {
    let Some(w) = crash_world() else {
        return;
    };
    for n in ["m16_mp", "M16_MP"] {
        assert!(w.weapon_move(n).is_some(), "{n}");
        assert!(w.def(n).is_some(), "{n}");
        assert!(w.kick(n).is_some(), "{n}");
        assert_eq!(w.is_primary(n), w.is_primary("m16_mp"), "{n}");
    }
}

/// [`view_steps`] as the eye's moves, predicting through `world` when
/// given; the predictor's counts at the end.
fn view_steps_in(
    name: &str,
    hz: f64,
    world: Option<Arc<Cod4World>>,
) -> (Vec<(i32, Vec3, bool)>, cod4_predict::PredictStats) {
    let text = std::fs::read_to_string(format!("{FIXTURES}/{name}")).unwrap();
    let mut s = Cod4Session::open_capture(&text).unwrap();
    if let Some(w) = world {
        s.set_world(w);
    }
    let (mut ms, mut last, mut out) = (0.0, None::<(Vec3, bool)>, Vec::new());
    while !s.finished() {
        ms += 1000.0 / hz;
        s.step(ms).unwrap();
        let Some(sc) = s.scene() else { continue };
        if let Some((eye, reset)) = last {
            // A reset counts on the frame the drawn pair turns to it.
            let new_reset = sc.view_reset && !reset;
            out.push((sc.render_time, sc.eye - eye, new_reset));
        }
        last = Some((sc.eye, sc.view_reset));
    }
    (out, s.predictor.stats)
}

/// The walk capture drawn at 60 and 144 frames a second: once the
/// clock has settled the drawn time steps by the frame's length (the
/// adjust moves it a millisecond or two a snapshot), the eye never
/// steps further than a run's pace, and it jumps only where the view
/// starts over (the two spawns), as retail cuts (cod4-client-view.md).
#[test]
fn walk_capture_view_runs_smooth() {
    for hz in [60.0, 144.0] {
        let frame = (1000.0 / hz) as i32;
        let steps = view_steps("play-1.7-walk.txt", hz);
        let settled = &steps[(2.0 * hz) as usize..];
        let times: Vec<i32> = settled.windows(2).map(|w| w[1].0 - w[0].0).collect();
        let (lo, hi) = (times.iter().min().unwrap(), times.iter().max().unwrap());
        let most = steps
            .iter()
            .filter(|s| !s.2)
            .map(|s| s.1)
            .fold(0.0, f32::max);
        let jumps = steps.iter().filter(|s| s.1 > 20.0).count();
        let resets = steps.iter().filter(|s| s.2 && s.1 > 20.0).count();
        println!(
            "{hz} Hz: drawn time steps {lo}..{hi}, max eye step {most:.2}, jumps {jumps}, at resets {resets}"
        );
        assert!(
            *lo >= frame - 3 && *hi <= frame + 4,
            "{hz} Hz: drawn time steps {lo}..{hi}"
        );
        assert!(most < 8.0, "{hz} Hz: eye stepped {most}");
        assert_eq!((jumps, resets), (2, 2), "{hz} Hz");
    }
    // The kill pair: the spawns, the killcam's start and end and the
    // respawn, each a reset.
    for (name, n) in [
        ("play-1.7-kill-shooter.txt", 2),
        ("play-1.7-kill-target.txt", 5),
    ] {
        let steps = view_steps(name, 60.0);
        let jumps = steps.iter().filter(|s| s.1 > 20.0).count();
        let resets = steps.iter().filter(|s| s.2 && s.1 > 20.0).count();
        assert_eq!((jumps, resets), (n, n), "{name}");
        // Predicted, the same cuts and nothing else.
        if let Some(w) = crash_world() {
            let (steps, st) = view_steps_in(name, 60.0, Some(w));
            let jumps = steps.iter().filter(|s| s.1.length() > 20.0).count();
            let resets = steps.iter().filter(|s| s.2 && s.1.length() > 20.0).count();
            println!("{name} predicted: jumps {jumps}, at resets {resets}; {st:?}");
            assert_eq!((jumps, resets), (n, n), "{name} predicted");
        }
    }
}

/// A capture's own view turns every frame, not in its cmds' 100 ms
/// steps: the walk capture predicted at 60 Hz turns on about twice the
/// frames it did unsmoothed (92), its largest step a frame outside the
/// cuts a fifth of the unsmoothed 1.29 degrees (cod4-client-view.md 6).
#[test]
fn capture_view_turns_every_frame() {
    let Some(world) = crash_world() else { return };
    let text = std::fs::read_to_string(format!("{FIXTURES}/play-1.7-walk.txt")).unwrap();
    let mut s = Cod4Session::open_capture(&text).unwrap();
    s.set_world(world);
    let (mut ms, mut last) = (0.0, None::<f32>);
    let (mut most, mut turning) = (0.0f32, 0);
    while !s.finished() {
        ms += 1000.0 / 60.0;
        s.step(ms).unwrap();
        let Some(sc) = s.scene() else { continue };
        if let Some(y) = last.filter(|_| !sc.following && !sc.view_reset) {
            let d = ((sc.angles.y - y + 540.0) % 360.0 - 180.0).abs();
            most = most.max(d);
            turning += usize::from(d > 0.01);
        }
        last = Some(sc.angles.y);
    }
    println!("max yaw step {most:.2}, {turning} turning frames");
    assert!(most < 0.4, "{most}");
    assert!(turning > 150, "{turning}");
}

#[test]
fn an_eased_cmd_turns_the_short_way() {
    let cmd = |t: i32, yaw: u16| Cmd {
        server_time: t,
        angles: [0, yaw, 0],
        ..Cmd::default()
    };
    // 350 degrees to 10 crosses 0, not 180.
    let (a, b) = (cmd(0, 63716), cmd(100, 1820));
    assert_eq!(eased_cmd(&a, &b, 50).angles[1], 0);
    assert_eq!(eased_cmd(&a, &b, 100).angles[1], 1820);
    assert_eq!(eased_cmd(&a, &b, 0).angles[1], 63716);
    assert_eq!(eased_cmd(&b, &b, 50).angles, b.angles);
    // A snap (65 degrees in 19 ms) and a long gap are taken at once.
    let snap = cmd(119, 1820 + 11833);
    assert_eq!(eased_cmd(&b, &snap, 110).angles, snap.angles);
    let late = cmd(400, 3640);
    assert_eq!(eased_cmd(&b, &late, 250).angles, late.angles);
}

/// The walk capture under `cg_thirdPerson` 1 on mp_crash
/// (cod4-client-view.md 16): the first-person offsets drop (the eye is
/// the drawn origin plus the view height), the drawn playerstate's
/// body (ours, or the one followed; none for a spectator) stands at
/// that origin, and the camera backs off it as
/// `CG_OffsetThirdPersonView` does: never past the range from 8 over
/// the eye, at the full range where nothing is behind, and nearer
/// where a wall stops the trace (the walker idles 4 off one). Off
/// again, the scene is first person with no own body.
#[test]
fn walk_capture_draws_its_own_body_in_third_person() {
    let Some(world) = crash_world() else { return };
    let text = std::fs::read_to_string(format!("{FIXTURES}/play-1.7-walk.txt")).unwrap();
    let mut s = Cod4Session::open_capture(&text).unwrap();
    s.set_world(world);
    s.third_person.on = true;
    let (mut ms, mut frames, mut full, mut pulled) = (0.0, 0, 0, 0);
    while !s.finished() {
        ms += 1000.0 / 60.0;
        s.step(ms).unwrap();
        let Some(sc) = s.scene() else { continue };
        // Our own body, or the one we follow (`otherFlags` 2).
        let own = sc.view.client_num;
        let t = sc.third_person.expect("third person");
        assert_eq!(t.dead_yaw, None);
        let body = sc.entities.iter().find(|d| d.number == own);
        // A spectator (`otherFlags` 0) has no body to draw.
        if sc.view.other_flags & 6 == 0 {
            assert!(body.is_none());
            continue;
        }
        let body = body.expect("own body");
        let frame = &body.body.as_ref().expect("posed").frame;
        assert_eq!(sc.eye, body.origin + Vec3::Z * t.view_height);
        assert_eq!(frame.angles, sc.angles.to_array());
        assert_eq!(t.angles.y, sc.angles.y);
        let back = (t.origin - (sc.eye + Vec3::Z * 8.0)).length();
        assert!(back < 120.0 + 0.01, "{back}");
        if (back - 120.0).abs() < 0.01 {
            full += 1;
        } else {
            pulled += 1;
        }
        frames += 1;
    }
    println!("{frames} frames: {full} at the full range, {pulled} pulled in");
    assert!(frames > 100 && full > 0 && pulled > 0);
    s.third_person.on = false;
    let sc = s.scene().unwrap();
    assert!(sc.third_person.is_none());
    assert!(sc.entities.iter().all(|d| d.number != sc.view.client_num));
}

/// The kill capture's target dies into the third-person view with
/// `cg_thirdPerson` 0 (`pm_type` 7, 0x451d80): the camera faces its
/// `stats[1]`, 245, and its own body is not drawn (`eFlags` 0x20000,
/// the snapshot's corpse is); the killcam that follows is first person.
#[test]
fn kill_capture_dies_into_the_third_person_view() {
    let Some(world) = crash_world() else { return };
    let text = std::fs::read_to_string(format!("{FIXTURES}/play-1.7-kill-target.txt")).unwrap();
    let mut s = Cod4Session::open_capture(&text).unwrap();
    s.set_world(world);
    let (mut ms, mut dead, mut alive) = (0.0, 0, 0);
    while !s.finished() {
        ms += 1000.0 / 60.0;
        s.step(ms).unwrap();
        let Some(sc) = s.scene() else { continue };
        // Neither our body nor, in the killcam, the killer's.
        let me = sc.view.client_num;
        assert!(
            sc.entities
                .iter()
                .all(|d| d.number != me || d.etype != ET_PLAYER)
        );
        match sc.view.pm_type >= 7 {
            true => {
                let t = sc.third_person.expect("dead view");
                assert_eq!(t.dead_yaw, Some(245.0));
                assert_eq!(t.angles.y, 245.0);
                dead += 1;
            }
            false => {
                assert!(sc.third_person.is_none());
                alive += 1;
            }
        }
    }
    println!("{dead} dead frames, {alive} alive");
    assert!(dead > 60 && alive > 0);
}

/// The walk capture predicted: its own cmds replayed over mp_crash's
/// clip map from each snapshot, the frame's own cmd on top
/// (cod4-client-view.md 5). The eye moves every frame by the frame's
/// share of the run (no 100 ms beats), level within a run's pace at
/// 190 units a second, up and down within a jump's takeoff; every
/// snapshot's playerstate lands within the last bits of where the
/// replay put it.
#[test]
fn walk_capture_predicts_its_own_body() {
    let Some(world) = crash_world() else { return };
    for hz in [60.0, 144.0] {
        let dt = 1.0 / hz as f32;
        let flat = |v: &[(i32, Vec3, bool)]| {
            v.iter()
                .filter(|s| !s.2)
                .map(|s| s.1.truncate().length())
                .fold(0.0, f32::max)
        };
        let whole = |v: &[(i32, Vec3, bool)]| {
            v.iter()
                .filter(|s| !s.2)
                .map(|s| s.1.length())
                .fold(0.0, f32::max)
        };
        let (before, _) = view_steps_in("play-1.7-walk.txt", hz, None);
        let (steps, st) = view_steps_in("play-1.7-walk.txt", hz, Some(world.clone()));
        let jumps = steps.iter().filter(|s| s.1.length() > 20.0).count();
        let resets = steps.iter().filter(|s| s.2 && s.1.length() > 20.0).count();
        println!(
            "{hz} Hz: max eye step level {:.2} -> {:.2}, whole {:.2} -> {:.2}; \
                 jumps {jumps}, at resets {resets}; {} snapshots corrected (max {:.4}), {} past 0.1, {} cmds run",
            flat(&before),
            flat(&steps),
            whole(&before),
            whole(&steps),
            st.corrected,
            st.max_correction,
            st.missed,
            st.cmds_run,
        );
        // The frame's cmd is stamped in whole ms, a ms of slew on top.
        let dt = dt + 0.002;
        assert!(
            flat(&steps) <= 190.0 * dt,
            "{hz} Hz: level step {}",
            flat(&steps)
        );
        // A jump's takeoff: sqrt(2 * 39 * 800) units a second up.
        assert!(
            whole(&steps) <= 260.0 * dt,
            "{hz} Hz: step {}",
            whole(&steps)
        );
        assert_eq!((jumps, resets), (2, 2), "{hz} Hz");
        assert_eq!(st.missed, 0, "{hz} Hz");
        assert!(st.max_correction < 0.05, "{hz} Hz: {}", st.max_correction);
    }
}

/// The mantle capture predicted: both mantles and the leans replay
/// in the client's own cmds within the last bits of the server's
/// (cod4-pmove.md 17, cod4-client-view.md 7).
#[test]
fn mantle_capture_predicts_without_a_correction() {
    let Some(world) = crash_world() else { return };
    let (_, st) = view_steps_in("play-1.7-mantle.txt", 60.0, Some(world));
    println!("{st:?}");
    assert_eq!(st.missed, 0);
    assert!(st.max_correction < 0.05, "{}", st.max_correction);
}

/// The touch capture's walker predicted against retail's snapshots: it
/// walks into the target, strafes along it and walks in again 26 to
/// the side (cod4-pmove.md 15.3). Clipped as boxes the prediction
/// missed 26 snapshots by up to 20 units; as capsules what is left is
/// the unit the client's capsule sits inside the server's
/// (cod4-client-view.md 5.1).
#[test]
fn touch_capture_predicts_the_walker_against_the_target() {
    let Some(world) = crash_world() else { return };
    for hz in [60.0, 144.0] {
        let (_, st) = view_steps_in("play-1.7-touch-walker.txt", hz, Some(world.clone()));
        println!("{hz} Hz: {st:?}");
        assert!(st.cmds_run > 5000, "{hz} Hz: {st:?}");
        assert!(st.missed <= 8, "{hz} Hz: {st:?}");
        assert!(st.max_correction < 2.1, "{hz} Hz: {st:?}");
    }
}

/// A walk into another player on mp_crash, the touch capture's second
/// approach (26 to the side of the target), stepped cmd by cmd through
/// the prediction's world and vcod-server's (`Cod4Clip`). A body packed
/// to read as the server's box gives the server's origin every cmd: the
/// same capsule in the same arithmetic. The real packing, a unit
/// inside, leaves the walker within 1.1 of the server's path
/// (cod4-client-view.md 5.1).
#[test]
fn predicted_walk_into_a_body_follows_the_server() {
    use vcod_cod4::pmove::{self, Cmd, PlayerState, PmWorld, Rules};
    use vcod_server::game::cod4::clip::{Body4, Cod4Clip};
    let Some(w) = crash_world() else { return };
    let (mins, maxs) = (pmove::PLAYER_MINS, pmove::PLAYER_MAXS);
    let floor = |x: f32, y: f32| {
        let p = Vec3::new(x, y, 296.0);
        let t = w
            .clip
            .trace(p, p - Vec3::Z * 256.0, mins, maxs, pmove::MASK_PLAYERSOLID);
        p.z - 256.0 * t.fraction
    };
    let target = Vec3::new(207.0, 2291.0, floor(207.0, 2291.0));
    let start = Vec3::new(181.0, 2211.0, target.z);
    let weapon = w.weapons["m16_mp"];
    let walk = |world: &dyn PmWorld| -> Vec<Vec3> {
        let mut ps = PlayerState::spawn(start, 90.0, 1000);
        ps.ground_entity_num = pmove::ENTITYNUM_WORLD;
        ps.weapon = 1;
        let mut old = Cmd::default();
        (1..=60)
            .map(|k| {
                let cmd = Cmd {
                    server_time: 1000 + 25 * k,
                    angles: [0, 0x4000, 0],
                    forward_move: 127,
                    ..Cmd::default()
                };
                pmove::player_step(&mut ps, &cmd, &mut old, world, Rules::default(), weapon);
                ps.origin
            })
            .collect()
    };
    let server = [Body4 {
        entity: 1,
        origin: target,
        mins,
        maxs,
        contents: pmove::CONTENTS_BODY,
        owner: None,
    }];
    let ours = walk(&Cod4Clip {
        clip: &w.clip,
        bodies: &server,
        brushes: &[],
    });
    let predicted = |solid: u32| {
        let solids = [Solid::of(1, true, solid, 0, 0, target, Vec3::ZERO).unwrap()];
        walk(&cod4_predict::Solids {
            clip: &w.clip,
            solids: &solids,
        })
    };
    // The server's box, (-15,-15,0)..(15,15,70), packed a unit out.
    assert_eq!(predicted(16 | (2 << 8) | ((70 + 33) << 16)), ours);
    // `SV_LinkEntity`'s packing (`game::cod4::level`).
    let real = predicted(15 | (1 << 8) | ((70 + 32) << 16));
    let last = *ours.last().unwrap();
    let met = ours
        .iter()
        .position(|p| p.x != start.x)
        .expect("met the body");
    println!(
        "server from {start} to {last}, met at cmd {met}; predicted to {}",
        real.last().unwrap()
    );
    assert!(last.y - start.y > 40.0, "walked {}", last.y - start.y);
    assert_eq!(real[..met - 1], ours[..met - 1]);
    let apart = real
        .iter()
        .zip(&ours)
        .map(|(a, b)| a.distance(*b))
        .fold(0.0, f32::max);
    println!("predicted at most {apart} from the server");
    assert!(apart < 1.5, "{apart}");
}

/// The own events the prediction fires on top of the snapshots', by
/// capture. Empty since the weapon half models `Mantle_IsWeaponInactive`
/// (cod4-pmove.md 17.4): the mantle's putaway is predicted as retail
/// sends it.
const OWN_EVENT_GAPS: &[(&str, &str, usize)] = &[];

/// Our own ring's events as the window fires them off the prediction,
/// against the snapshots' ring read straight: each capture's own
/// events (shots, reloads, switches, footsteps, the jumps) fire once,
/// none a second time when the snapshot carrying it arrives.
#[test]
fn predicted_own_events_fire_once() {
    let Some(world) = crash_world() else {
        return;
    };
    for name in [
        "play-1.7-kill-shooter.txt",
        "play-1.7-walk.txt",
        "play-1.7-mantle.txt",
    ] {
        let text = std::fs::read_to_string(format!("{FIXTURES}/{name}")).unwrap();
        let mut s = Cod4Session::open_capture(&text).unwrap();
        s.set_world(world.clone());
        let mut ours: BTreeMap<&str, usize> = BTreeMap::new();
        let mut ms = 0.0;
        while !s.finished() {
            ms += 1000.0 / 60.0;
            s.step(ms).unwrap();
            s.scene();
            for f in s.take_fired().iter().filter(|f| f.entity.is_none()) {
                *ours.entry(f.name).or_default() += 1;
            }
        }
        // The snapshots' ring, each entry once, up to the last
        // snapshot the scene predicted from.
        let c = s.client();
        let mut theirs: BTreeMap<&str, usize> = BTreeMap::new();
        let mut seen: Option<u32> = None;
        for x in &c.snapshots {
            let seq = x.ps.get("eventSequence").unwrap_or(0) & 0xff;
            if let Some(prev) = seen {
                let to = if seq < prev { seq + 256 } else { seq };
                for q in prev.max(to.saturating_sub(4))..to {
                    let id = x.ps.get(&format!("events[{}]", q & 3)).unwrap_or(0);
                    if let Some(n) = event_name(id).filter(|_| id != 0) {
                        *theirs.entry(n).or_default() += 1;
                    }
                }
            }
            seen = Some(seq);
        }
        println!("{name}: predicted {ours:?}\n  snapshots {theirs:?}");
        for &(gap, event, n) in OWN_EVENT_GAPS {
            if gap == name {
                *theirs.entry(event).or_default() += n;
            }
        }
        assert_eq!(ours, theirs, "{name}");
    }
}

/// What the view weapon drew over a capture (cod4-client-view.md 4).
#[derive(Default, Debug)]
struct ViewCounts {
    pub(super) frames: usize,
    /// Frames drawn with a view weapon, by weapon.
    pub(super) drawn: BTreeMap<String, usize>,
    /// The own view's shots, and the fire nodes started.
    pub(super) shots: usize,
    fire_starts: usize,
    /// Each own shot's view flash: its place off the eye along the
    /// view (forward, right, up) and its forward against the view's.
    pub(super) flashes: Vec<(Vec3, f32)>,
    pub(super) ejects: usize,
}

/// `name` through the frame path with the view weapon as `cod4_client`
/// draws it at 60 frames a second: the scene (its prediction firing our
/// own events), this frame's pose, then the shots' effects off its
/// view tags.
fn replay_view(name: &str) -> Option<ViewCounts> {
    use crate::cod4_viewmodel::{Cod4View, tag_to_world};
    use crate::fx::sim::SpawnAt;
    let install = vcod_cod4::testing::cod4_install()?;
    let text = std::fs::read_to_string(format!("{FIXTURES}/{name}")).unwrap();
    let mut s = Cod4Session::open_capture(&text).unwrap();
    // Predicted, as the window plays it.
    s.set_world(crash_world()?);
    let (models, weapons, _) =
        crate::cod4_fly::load_bodies(install.root(), "mp_crash", &s.entity_assets()).unwrap();
    let ((_, mut effects), _) = crate::cod4_fly::load_av(install.root(), "mp_crash").unwrap();
    let mut view = Cod4View::default();
    let mut n = ViewCounts::default();
    let (mut starts, mut ms) = (0, 0.0);
    while !s.finished() {
        ms += 1000.0 / 60.0;
        s.step(ms).unwrap();
        let Some(scene) = s.scene() else {
            s.take_fired();
            continue;
        };
        let fired = s.take_fired();
        let gs = s.gamestate().unwrap();
        let list = weapon_list(gs);
        let cs = |k: u16| String::from_utf8_lossy(gs.configstring(k)).into_owned();
        let (yaw, pitch) = (scene.angles.y.to_radians(), -scene.angles.x.to_radians());
        let basis = crate::camera::basis(yaw, pitch);
        n.frames += 1;
        let out = view.frame(
            &scene.view,
            &list,
            scene.hands.as_deref(),
            &weapons,
            &models,
            scene.render_time,
        );
        let mut tags = None;
        if let (Some(frame), Some((w, anim))) = (&out.frame, view.anim()) {
            assert!(frame.draw.bone_sets.iter().flatten().all(|m| m.is_finite()));
            *n.drawn.entry(w.to_string()).or_default() += 1;
            if anim.starts > starts && [3, 5, 28, 29].contains(&anim.node) {
                n.fire_starts += 1;
            }
            starts = anim.starts;
            let world =
                |t: Option<(Vec3, glam::Quat)>| t.map(|t| tag_to_world(t, scene.eye, basis));
            tags = Some((
                scene.view.client_num,
                crate::play::cod4_players::WeaponTags {
                    flash: world(out.tags.flash),
                    brass: world(out.tags.brass),
                },
            ));
        } else {
            starts = 0;
        }
        for f in fired
            .iter()
            .filter(|f| f.entity.is_none() && f.name.starts_with("EV_FIRE_WEAPON"))
        {
            n.shots += 1;
            for p in effects.plays(f, &list, &cs, &HashMap::new(), tags.as_ref()) {
                let k = p.key;
                match p.at {
                    SpawnAt::Oriented { pos, axis } if k.starts_with("muzzleflashes/") => {
                        let d = pos - scene.eye;
                        let (fwd, right, up) = basis;
                        n.flashes.push((
                            Vec3::new(d.dot(fwd), d.dot(right), d.dot(up)),
                            axis[0].dot(fwd),
                        ))
                    }
                    SpawnAt::Oriented { .. } if k.starts_with("shellejects/") => n.ejects += 1,
                    _ => {}
                }
            }
        }
    }
    Some(n)
}

/// The shooter draws its M16 in first person, plays the fire clip on
/// every own shot and flashes on the view model's `tag_flash`, at the
/// muzzle ahead of the eye, pointing down the view.
#[test]
fn shooter_draws_its_view_weapon_and_flashes_on_its_tag() {
    let Some(n) = replay_view("play-1.7-kill-shooter.txt") else {
        return;
    };
    println!("{n:?}");
    assert!(
        n.drawn.keys().all(|w| w.starts_with("m16")),
        "{:?}",
        n.drawn
    );
    assert!(n.drawn.values().sum::<usize>() > n.frames / 2, "{n:?}");
    assert!(n.shots > 0);
    assert_eq!(n.fire_starts, n.shots, "{n:?}");
    assert_eq!(n.flashes.len(), n.shots);
    assert_eq!(n.ejects, n.shots);
    for (at, along) in &n.flashes {
        assert!((10.0..40.0).contains(&at.x), "{at}");
        assert!(at.y.abs() < 10.0 && at.z.abs() < 10.0, "{at}");
        assert!(*along > 0.9, "{along}");
    }
}

/// The walker draws its rifle in first person once it spawns (a frag
/// for the throws), the idle teammate's while its view follows him,
/// and each of its shots flashes and ejects on the view tags.
#[test]
fn walker_draws_its_view_weapon() {
    let Some(n) = replay_view("play-1.7-walk.txt") else {
        return;
    };
    println!("{n:?}");
    assert!(n.drawn.contains_key("m16_gl_mp"), "{n:?}");
    assert!(n.drawn.values().sum::<usize>() > n.frames / 2, "{n:?}");
    assert!(n.fire_starts >= n.shots, "{n:?}");
    assert_eq!((n.flashes.len(), n.ejects), (n.shots, n.shots));
}
