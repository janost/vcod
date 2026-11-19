//! A live session's lifetime: lerps, silences, drops, shed snapshots,
//! pitch limits and the team pick.

use super::*;
use crate::play::cod4::scene::lerp_wrapped;

/// A watched player's lean and move direction step between snapshots
/// as its angles do, the short way round, rather than jumping at
/// each snapshot (a choppy lean seen in play).
#[test]
fn lean_and_move_direction_lerp_between_snapshots() {
    let near = |a: f32, b: f32| (a - b).abs() < 1e-4;
    assert!(near(lerp_wrapped(0.25, 0.75, 0.5), 0.5));
    assert!(near(lerp_wrapped(-0.5, 0.0, 0.25), -0.375));
    assert!(near(lerp_wrapped(170.0, -170.0, 0.5), 180.0));
    assert!(near(lerp_wrapped(-45.0, 45.0, 1.0), 45.0));
}

/// The server gone quiet (Ctrl+C with no `w` sent): the cmds the
/// server has not run pile up past the 128 kept and `Connection
/// Interrupted` draws, then `cl_timeout` (cut to 6 s here) of silence
/// drops the session with `EXE_ERR_SERVER_TIMEOUT`. The interrupt
/// counts cmds (one a frame) and the timeout wall time, so the first
/// 128 frames run 2 ms apart, as a fast client's would, then a 16 ms
/// grid: a loaded machine's slow frames cannot push the 128th past
/// the timeout.
#[test]
fn cod4_live_silence_interrupts_then_times_out() {
    const TIMEOUT: Duration = Duration::from_secs(6);
    let Some((interrupted, dropped)) = live_join_ctl(|s, spawned, t0, tx| {
        spawned.as_ref()?;
        s.timeout = TIMEOUT;
        tx.send(Ctl::Silence).ok()?;
        let quiet = clk_now();
        let mut interrupted = None;
        let mut grid = FrameClock::new();
        let mut frame = 0u32;
        while since(quiet) < TIMEOUT * 5 {
            s.step(since(t0).as_secs_f64() * 1000.0).unwrap();
            s.take_commands();
            s.scene();
            s.take_fired();
            frame += 1;
            if interrupted.is_none() && s.interrupted() {
                interrupted = Some((frame, since(quiet)));
            }
            if let Some(why) = s.take_dropped() {
                return Some((interrupted, (why, frame, since(quiet))));
            }
            if frame < 128 {
                clk_sleep(Duration::from_millis(2));
                grid = FrameClock::new();
            } else {
                grid.wait();
            }
        }
        Some((interrupted, (String::new(), frame, since(quiet))))
    })
    .flatten() else {
        return;
    };
    println!("interrupted {interrupted:?}, dropped {dropped:?}");
    let (at, when) = interrupted.expect("Connection Interrupted drew");
    assert_eq!(dropped.0, "EXE_ERR_SERVER_TIMEOUT");
    assert!(dropped.1 > at && when < dropped.2, "{at} {dropped:?}");
    assert!(dropped.2 >= TIMEOUT, "{:?}", dropped.2);
}

/// A `w` from the server (its `quit`, a kick) drops the session with
/// its reason, as `CL_DisconnectError` words it.
#[test]
fn cod4_live_w_drops_with_its_reason() {
    let Some(why) = live_join_ctl(|s, spawned, t0, tx| {
        spawned.as_ref()?;
        tx.send(Ctl::Command("w \"EXE_SERVERQUIT\"".into())).ok()?;
        let sent = clk_now();
        while since(sent) < Duration::from_secs(3) {
            s.step(since(t0).as_secs_f64() * 1000.0).unwrap();
            s.take_commands();
            if let Some(why) = s.take_dropped() {
                return Some(why);
            }
            clk_sleep(Duration::from_millis(16));
        }
        Some(String::new())
    })
    .flatten() else {
        return;
    };
    assert_eq!(why, "EXE_SERVERDISCONNECTREASON\u{15}EXE_SERVERQUIT");
    let Some(install) = vcod_cod4::testing::cod4_install() else {
        return;
    };
    let text = localize_drop(&why, install.root());
    assert!(!text.contains("EXE_") && !text.contains("&&1"), "{text}");
}

/// A live session sheds its old snapshots (the soak's leak: every
/// snapshot was kept, 100 MB a match) and plays on: the clock reads
/// each new snapshot, the scene draws and our own shots still fire
/// their events after the sheds.
#[test]
fn cod4_live_session_sheds_old_snapshots_and_plays_on() {
    use crate::play::input::Action;
    let Some(out) = live_join(|s, spawned, t0| {
        spawned.as_ref()?;
        // The prematch freezes the weapon (`PMF_FROZEN` 0x800).
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
        s.keep_snapshots = 40;
        let (mut fired_late, mut scenes, mut max_len) = (0, 0, 0);
        let (mut last_time, mut stuck, mut held) = (0, 0, false);
        let start = s.client().total_snapshots();
        let t1 = clk_now();
        while since(t1) < Duration::from_secs(30) && s.client().total_snapshots() < start + 240 {
            let want = since(t1).as_millis() % 1000 < 300;
            if want != held {
                s.input.key(Action::Attack, want);
                held = want;
            }
            s.step(since(t0).as_secs_f64() * 1000.0).unwrap();
            s.take_commands();
            let shed = s.client().shed_count > 0;
            fired_late += s
                .take_fired()
                .iter()
                .filter(|f| shed && f.name.starts_with("EV_FIRE_WEAPON"))
                .count();
            max_len = max_len.max(s.client().snapshots.len());
            let scene = s.scene();
            if shed && let Some(scene) = scene {
                scenes += 1;
                // The drawn time never runs back or sticks across a
                // shed: the clock still reads each new snapshot.
                assert!(scene.server_time >= last_time, "time ran back");
                stuck = if scene.server_time == last_time {
                    stuck + 1
                } else {
                    0
                };
                assert!(stuck < 60, "the drawn time stuck");
                last_time = scene.server_time;
            }
            clk_sleep(Duration::from_millis(16));
        }
        let c = s.client();
        Some((
            c.total_snapshots() - start,
            c.shed_count,
            max_len,
            fired_late,
            scenes,
        ))
    }) else {
        return;
    };
    let (got, shed, max_len, fired_late, scenes) = out.expect("spawned");
    assert!(got >= 200, "{got} snapshots");
    assert!(shed + 80 >= got, "{shed} of {got} shed");
    assert!(max_len < 80, "{max_len} snapshots held");
    assert!(fired_late > 0, "no shot fired after a shed");
    assert!(scenes > 100, "{scenes} scenes");
}

/// Holding the mouse past `player_view_pitch_down` (85) or `_up` keeps
/// the drawn view still at the limit: the mouse turns the cmd's view
/// ([`Cod4Session::turn`], `cl.viewangles`), the camera draws the
/// predicted view, and the server's `delta_angles` pushes take the
/// excess back. In play the view jumped between 85 and the
/// camera's own 89 at each push. Turning back moves the view at once
/// and only one way.
#[test]
fn cod4_live_pitch_held_past_the_limit_stays_at_the_limit() {
    let Some(out) = live_join(|s, spawned, t0| {
        spawned.as_ref()?;
        // (mouse degrees a frame, frames) phases, about 2, 0.5 and
        // 2.5 s at 60 Hz: counted in frames, as a loaded machine
        // stretches each; the drawn pitch of each.
        let phases = [(3.0f32, 125u32), (-0.5, 31), (-3.0, 156)];
        let mut drawn = vec![Vec::new(); phases.len()];
        let mut held = Vec::new();
        let mut jumps = 0;
        for (k, &(mouse, frames)) in phases.iter().enumerate() {
            if k == 1 {
                // Still until the server has run every held cmd, so
                // its last push has taken all the excess back: the
                // excess a round trip leaves grows with the lag, and a
                // starved server's would outlast the slow turn back.
                let last = s.sent_cmds.back().map_or(0, |c| c.server_time);
                let run = |s: &Cod4Session| {
                    s.client()
                        .snapshots
                        .last()
                        .is_some_and(|x| x.ps.get("commandTime").unwrap_or(0) as i32 >= last)
                };
                let wait = clk_now();
                while !run(s) && since(wait) < Duration::from_secs(30) {
                    s.turn([0.0, 0.0]);
                    s.step(since(t0).as_secs_f64() * 1000.0).unwrap();
                    s.take_commands();
                    jumps += usize::from(s.take_view_jump());
                    if let Some(scene) = s.scene() {
                        held.push(scene.angles.x);
                    }
                    clk_sleep(Duration::from_millis(16));
                }
            }
            for _ in 0..frames {
                s.turn([mouse, 0.0]);
                s.step(since(t0).as_secs_f64() * 1000.0).unwrap();
                s.take_commands();
                jumps += usize::from(s.take_view_jump());
                if let Some(scene) = s.scene() {
                    drawn[k].push(scene.angles.x);
                }
                clk_sleep(Duration::from_millis(16));
            }
        }
        Some((drawn, held, jumps))
    })
    .flatten() else {
        return;
    };
    let (drawn, held, jumps) = out;
    println!(
        "pitch: {:?} frames, {jumps} jumps",
        drawn.iter().map(Vec::len).collect::<Vec<_>>()
    );
    assert!(jumps > 0, "the server never pushed the pitch");
    // Held down: once at the limit the view never leaves it.
    let at = drawn[0]
        .iter()
        .position(|&p| p >= 84.99)
        .expect("never reached 85");
    assert!(drawn[0][at..].len() > 60, "{}", drawn[0].len());
    assert!(
        drawn[0][at..].iter().all(|&p| (84.99..=85.01).contains(&p)),
        "{:?}",
        drawn[0]
    );
    assert!(
        held.iter().all(|&p| (84.99..=85.01).contains(&p)),
        "{held:?}"
    );
    // Turned back slowly: the view comes off the limit and never
    // steps back toward it.
    let back = &drawn[1];
    assert!(back.last().is_some_and(|&p| p < 84.0), "{back:?}");
    assert!(back.windows(2).all(|w| w[1] <= w[0] + 0.01), "{back:?}");
    // Held up: the same at -85.
    let up = &drawn[2];
    let at = up
        .iter()
        .position(|&p| p <= -84.99)
        .expect("never reached -85");
    assert!(up[at..].len() > 60, "{}", up.len());
    assert!(
        up[at..].iter().all(|&p| (-85.01..=-84.99).contains(&p)),
        "{up:?}"
    );
}

/// The live join answers the team menu and the class menu
/// `changeclass_marines` opens, and spawns playing with the class's
/// weapon, its cmds carrying that weapon as its own pick.
#[test]
fn cod4_live_join_picks_a_team_and_a_class_and_spawns() {
    let Some((spawned, opened, ps, sent)) = live_join(|s, spawned, t0| {
        // A second of play: the cmds go on carrying the pick.
        let t1 = clk_now();
        while spawned.is_some() && since(t1) < Duration::from_secs(1) {
            s.step(since(t0).as_secs_f64() * 1000.0).unwrap();
            clk_sleep(Duration::from_millis(16));
        }
        let ps = s.client().snapshots.last().map(|x| x.ps.clone());
        let sent = s.sent_cmds.back().copied();
        (spawned, s.menus.opened.clone(), ps, sent)
    }) else {
        return;
    };
    assert_eq!(opened[..2], ["team_marinesopfor", "changeclass_marines"]);
    let spawned = spawned.expect("no spawn");
    assert!(spawned.contains("m16_gl_mp"), "{spawned}");
    let ps = ps.unwrap();
    let weapon = ps.get("weapon").unwrap();
    assert_eq!(sent.map(|c| u32::from(c.weapon)), Some(weapon));
}
