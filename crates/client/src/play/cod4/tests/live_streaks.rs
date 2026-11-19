//! Kill streaks on a live server: the hardpoints earned and called in,
//! the killcam and the progress bar.

use super::*;

/// [`live_war_pair`]: the stock `giveHardpointItem("airstrike_mp")`,
/// its action slot pressed, and the stock `useAirstrike`'s
/// `beginLocationSelection` sets `ps.locationSelectionInfo`: the HUD
/// draws `fullscreenmap` (the minimap, its border, the selector's
/// material under the cursor), the mouse moves the cursor and not the
/// view, and the `+attack` press's 0x10000 confirms it, which ends the
/// selection (cod4-hud.md 15.8).
/// [`live_war_pair`]: the assault class's M16 grenade launcher, its
/// action slot pressed and its trigger pulled, puts a `gl_m16_mp`
/// grenade in our snapshots on a gravity arc along the view, which
/// bursts (`EV_GRENADE_EXPLODE`) past its 375-unit arming distance. A
/// grenade that wobbled like a rocket hung 120 ahead of the eye and
/// never burst (cod4-combat.md 10.3).
#[test]
fn cod4_live_grenade_launcher_fires_along_the_view() {
    use crate::play::input::Action;
    #[derive(Default, Debug)]
    struct Seen {
        pressed_at: Option<i32>,
        fired_at: Option<i32>,
        released: bool,
        forward: Vec3,
        eye: Vec3,
        /// The first state: `trType`, `trBase`, `trDelta`.
        launch: Option<(u32, Vec3, Vec3)>,
        /// Where the burst's event rode.
        burst: Option<Vec3>,
        /// The yaw the shot goes along.
        yaw: f32,
        /// The snapshots read so far.
        read: usize,
        /// Each grenade state's server time, `trType`, `trBase` and
        /// events, for a failure's message.
        states: Vec<(i32, u32, Vec3, [u32; 4])>,
    }
    let mut seen = Seen::default();
    let Some(out) = live_war_pair(move |f| {
        let snap = f.s.client().snapshots.last().cloned()?;
        let u = |n: &str| snap.ps.get(n).unwrap_or(0);
        if !f.both || f.level < 9000 {
            return None;
        }
        if seen.pressed_at.is_none() {
            // Level along a yaw with 600 units open under the grenade's
            // mask (`G_InitGrenadeEntity`'s 0x2806891, missile clip
            // too), the static models and the solid entities, so it
            // arms before it meets anything: one that met something
            // inside its 375 went dud, whichever way the spawn faced.
            let clip = &crash_world()?.clip;
            // The server's eye: the scene's may still be the spawn's
            // camera move.
            let eye = vec3(|k| ps_f32(&snap.ps, &format!("origin[{k}]"))) + Vec3::Z * 60.0;
            let yaw = (0..24).map(|k| k as f32 * 15.0).find(|y| {
                let d = Vec3::new(y.to_radians().cos(), y.to_radians().sin(), 0.0);
                let (e, end) = (Vec3::splat(4.0), eye + d * 600.0);
                let mut t = clip.trace(eye, end, -e, e, 0x280_6891);
                clip.trace_static_models(eye, end, 0x280_6891, &mut t);
                let solids = crate::play::cod4_predict::Solids {
                    clip,
                    solids: &f.s.shot_solids,
                };
                let me = f.s.client().gamestate.as_ref().map_or(0, |g| g.client_num);
                let ents = vcod_cod4::pmove::PmWorld::trace(
                    &solids, eye, end, -e, e, me as u32, 0x280_6891,
                );
                t.fraction == 1.0 && !t.startsolid && ents.fraction == 1.0
            })?;
            seen.yaw = yaw;
            f.s.view = [0.0, yaw];
            let slot = (0..4).find(|k| u(&format!("actionSlotType[{k}]")) == 2)?;
            if f.s.action_slot(slot + 1) {
                seen.pressed_at = Some(f.level);
            }
            return None;
        }
        let Some(fired) = seen.fired_at else {
            let weapon = u("weapon") as usize;
            let on_alt =
                f.s.level_defs
                    .as_ref()
                    .is_some_and(|(_, d)| d.get(weapon).is_some_and(|d| d.name == "gl_m16_mp"));
            // The view on the open yaw first.
            let turned = (f.angles.y - seen.yaw)
                .rem_euclid(360.0)
                .min((seen.yaw - f.angles.y).rem_euclid(360.0))
                < 0.5;
            if on_alt && u("weaponstate") == 0 && turned {
                f.s.input.key(Action::Attack, true);
                seen.fired_at = Some(f.level);
                seen.eye = f.eye;
                seen.forward = vcod_common::x87::angle_vectors(f.angles)[0];
            }
            assert!(
                f.level - seen.pressed_at.unwrap() < 5000,
                "never on the launcher"
            );
            return None;
        };
        // Held until the grenade shows: a loaded machine may run few
        // frames in 200 ms.
        if !seen.released && seen.launch.is_some() {
            f.s.input.key(Action::Attack, false);
            seen.released = true;
        }
        // Every snapshot since the last frame, not the newest alone: the
        // burst rides its entity 300 ms, and one frame may read more.
        let snaps = &f.s.client().snapshots;
        let from = seen.read.max(snaps.len().saturating_sub(64));
        seen.read = snaps.len();
        for snap in &snaps[from..] {
            for e in snap.entities.iter().filter(|e| e.etype() == 4) {
                let v = |n: &str| vec3(|k| ent_f32(e, &format!("lerp.pos.{n}[{k}]")));
                let events = std::array::from_fn(|k| ent_u32(e, &format!("events[{k}]")));
                let tr = ent_u32(e, "lerp.pos.trType");
                seen.states
                    .push((snap.server_time, tr, v("trBase"), events));
                if seen.launch.is_none() {
                    seen.launch = Some((tr, v("trBase"), v("trDelta")));
                }
                if events.contains(&45) {
                    seen.burst = Some(v("trBase"));
                    return Some(std::mem::take(&mut seen));
                }
            }
        }
        if f.level - fired > 8000 {
            return Some(std::mem::take(&mut seen));
        }
        None
    }) else {
        return;
    };
    let out = out.expect("no launcher shot in 120 s");
    let (tr, base, delta) = out.launch.expect("no launcher grenade in the snapshots");
    assert_eq!(tr, 5, "TR_GRAVITY: {out:?}");
    assert!(base.distance(out.eye) < 2.0, "{out:?}");
    assert!((delta.dot(out.forward) - 2400.0).abs() < 20.0, "{out:?}");
    let burst = out
        .burst
        .unwrap_or_else(|| panic!("the grenade never burst: {out:?}"));
    assert!((burst - base).dot(out.forward) > 375.0, "{out:?}");
}

#[test]
fn cod4_live_airstrike_picks_its_spot_on_the_full_map() {
    use crate::play::input::Action;
    #[derive(Default)]
    struct Seen {
        given: bool,
        pressed: bool,
        frames: u32,
        drawn: Option<(bool, bool, bool)>,
        moved: Option<[f32; 2]>,
        fired: bool,
        ended: bool,
        /// The menu's texts as drawn.
        texts: Vec<String>,
    }
    let mut seen = Seen::default();
    let Some(out) = live_war_pair(move |f| {
        if f.both && !seen.given && f.level > 9000 {
            seen.given = true;
            f.tx.send((
                0,
                "maps/mp/gametypes/_hardpoints",
                "giveHardpointItem",
                "airstrike_mp",
            ))
            .ok();
        }
        if seen.given && !seen.pressed && f.s.action_slot(4) {
            seen.pressed = true;
        }
        let info = f.s.selecting_location();
        if info == 0 {
            if seen.fired {
                seen.ended = true;
                return Some(std::mem::take(&mut seen));
            }
            return None;
        }
        seen.frames += 1;
        let tex = |m: &str| f.hud.texture_of(m);
        let cs = |i: u16| f.hud.configstring(f.s, i);
        let map = vcod_cod4::hud::tokenize(&cs(vcod_cod4::hud::CS_MINIMAP))
            .first()
            .and_then(|m| tex(m));
        let selector = tex(&cs(827 + (info & 3) as u16 - 1));
        let border = tex("map_border");
        let has = |t: &Option<String>| {
            t.as_ref()
                .is_some_and(|t| f.quads.iter().any(|q| &q.texture == t))
        };
        seen.drawn = Some((has(&map), has(&selector), has(&border)));
        seen.texts = f.hud.fullscreen_texts(f.shell);
        if seen.frames == 5 {
            f.s.move_selector([10.0, -20.0]);
            seen.moved = Some(f.s.selector.at);
        }
        if seen.frames == 10 {
            f.s.input.key(Action::Attack, true);
            seen.fired = true;
        }
        if seen.frames == 14 {
            f.s.input.key(Action::Attack, false);
        }
        None
    }) else {
        return;
    };
    let out = out.expect("the airstrike's selection never came and went in 120 s");
    assert!(out.pressed, "action slot 4 selected nothing");
    assert_eq!(
        out.drawn,
        Some((true, true, true)),
        "map, selector, border drawn"
    );
    let at = out.moved.unwrap();
    assert!(
        (at[0] - 0.62).abs() < 1e-4 && (at[1] - 0.56).abs() < 1e-4,
        "{at:?}"
    );
    assert!(out.ended, "the confirm never ended the selection");
    // `@PLATFORM_PRESS_TO_SET_AIRSTRIKE` localized, its key named.
    assert!(
        !out.texts.is_empty() && out.texts.iter().all(|t| !t.contains('@')),
        "{:?}",
        out.texts
    );
}

/// What [`streak_run`] saw: the HUD's texts, and when each step came.
#[derive(Default)]
struct Streak {
    kills: u32,
    pub(super) last: Option<Duration>,
    pub(super) texts: std::collections::BTreeSet<String>,
    /// The `dpad` drew the hardpoint's icon.
    pub(super) icon: bool,
    /// Action slot 4 selected it.
    pub(super) pressed: bool,
    /// The location selector came up; frames since.
    selecting: u32,
    /// The selector came down after the confirm.
    confirmed: bool,
    /// Our compass drew the vehicle as ours.
    pub(super) called: bool,
}

/// The killcam on vcod-server, live: the enemy kills us through the
/// stock damage callback (`gsc/killcam`), the stock killcam follows
/// him. `CG_DrawDisconnect` never fires on it, as the server moves the
/// archived `commandTime` up to the frame's (cod4-combat.md 7), and
/// the use key skips it (`waitSkipKillcamButton` polls
/// `useButtonPressed`).
#[test]
fn cod4_live_killcam_draws_no_interrupt_and_the_use_key_skips_it() {
    use crate::play::input::Action;
    const GSC: &str = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../cod4/tests/fixtures/gsc/killcam"
    );
    #[derive(Debug, Default)]
    struct Kc {
        sent: bool,
        start: Option<i32>,
        frames: u32,
        interrupted: u32,
        pressed: Option<i32>,
        end: Option<i32>,
    }
    let mut kc = Kc::default();
    let out = live_war_pair_in(["assault_mp,0", "assault_mp,0"], Some(GSC), move |f| {
        if f.both && f.level > 9000 && !kc.sent {
            f.tx.send((0, "maps/mp/mp_crash_fx", "killedBy", "enemy"))
                .ok();
            kc.sent = true;
        }
        let enemy =
            f.e.client()
                .snapshots
                .last()
                .and_then(|x| x.ps.get("clientNum"));
        let ps = &f.s.client().snapshots.last()?.ps;
        let watching =
            kc.sent && ps.get("otherFlags").unwrap_or(0) & 2 != 0 && ps.get("clientNum") == enemy;
        if watching {
            let start = *kc.start.get_or_insert(f.level);
            kc.frames += 1;
            kc.interrupted += u32::from(f.s.interrupted());
            if kc.pressed.is_none() && f.level - start >= 500 {
                f.s.input.key(Action::Use, true);
                kc.pressed = Some(f.level);
            }
        } else if kc.start.is_some() {
            f.s.input.key(Action::Use, false);
            kc.end = Some(f.level);
            return Some(std::mem::take(&mut kc));
        }
        None
    });
    let Some(out) = out else {
        return;
    };
    let kc = out.expect("the killcam started and ended");
    eprintln!("{kc:?}");
    assert!(kc.frames > 10, "{kc:?}");
    assert_eq!(kc.interrupted, 0, "{kc:?}");
    let (pressed, end) = (kc.pressed.expect("pressed"), kc.end.unwrap());
    assert!(end - pressed <= 500, "the use key skipped it: {kc:?}");
}

/// The stock primary progress bar, live: `updateBar( 0, 0.5 )` sets
/// the fill a unit wide and `scaleOverTime`s it to full in 2 s (the HQ
/// capture's bar). Our fill grows with it, never full from the start.
#[test]
fn cod4_live_progress_bar_fills_over_its_scale_time() {
    const GSC: &str = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../cod4/tests/fixtures/gsc/killcam"
    );
    let mut sent: Option<i32> = None;
    let mut widths: Vec<(i32, f32)> = Vec::new();
    let out = live_war_pair_in(["assault_mp,0", "assault_mp,0"], Some(GSC), move |f| {
        if f.both && f.level > 9000 && sent.is_none() {
            f.tx.send((0, "maps/mp/mp_crash_fx", "progressBar", ""))
                .ok();
            sent = Some(f.level);
        }
        let start = sent?;
        let fill = f.hud.texture_of("progress_bar_fill").expect("the fill");
        // The fill is white; the backing under it shares its image, black.
        if let Some(q) = f
            .quads
            .iter()
            .find(|q| q.texture == fill && q.rgba[0] > 0.5)
        {
            widths.push((f.level - start, q.verts[1][0] - q.verts[0][0]));
        }
        (f.level - start > 3000).then(|| std::mem::take(&mut widths))
    });
    let Some(out) = out else {
        return;
    };
    let w = out.expect("the bar drew");
    let full = w.last().expect("a bar").1;
    let at = |ms: i32| w.iter().find(|(t, _)| *t >= ms).map(|x| x.1);
    let early = w.first().unwrap().1;
    assert!(early < 0.25 * full, "{w:?}");
    let mid = at(1200).expect("a frame at 1.2 s");
    assert!(mid > 0.25 * full && mid < 0.9 * full, "{w:?}");
}

/// [`live_war_pair_in`] under `streak_kill`'s probe gsc (no killcam,
/// `scr_game_forceuav` 1): we kill the enemy `kills` times in a row
/// through the stock damage callback, so the stock
/// `Callback_PlayerKilled` counts the streak and
/// `giveHardpointItemForStreak` hands out `weapon`. Then the `dpad`'s
/// icon, its key, the airstrike's selector and its confirm, until our
/// compass draws the `kind` vehicle as ours (cod4-hud.md 15).
fn streak_run(kills: u32, weapon: &'static str, kind: &'static str) -> Option<Streak> {
    use crate::play::input::Action;
    const GSC: &str = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../cod4/tests/fixtures/gsc/streak_kill"
    );
    let t0 = clk_now();
    let mut seen = Streak::default();
    let out = live_war_pair_in(["assault_mp,0", "assault_mp,0"], Some(GSC), move |f| {
        let icon = f
            .hud
            .dpad_icon(weapon)
            .and_then(|m| f.hud.texture_of(&m))
            .unwrap_or_else(|| panic!("{weapon}'s dpad icon"));
        let ours = f
            .hud
            .texture_of(&format!("compass_objpoint_{kind}_friendly"))
            .unwrap_or_else(|| panic!("{kind}'s compass icon"));
        let t = since(t0);
        if f.both
            && f.level > 9000
            && seen.kills < kills
            && f.e.spawn().is_some()
            && seen
                .last
                .is_none_or(|l| t - l > Duration::from_millis(1500))
        {
            f.tx.send((1, "maps/mp/mp_crash_fx", "killedBy", "joiner"))
                .ok();
            seen.kills += 1;
            seen.last = Some(t);
        }
        if let Some(snap) = f.s.client().snapshots.last() {
            for e in vcod_cod4::hud::hud_elems(&snap.ps) {
                if let Some(t) = f.hud.elem_text(f.s, &e, f.level, f.shell)
                    && !t.is_empty()
                {
                    seen.texts.insert(t);
                }
            }
        }
        seen.icon |= seen.kills == kills && f.quads.iter().any(|q| q.texture == icon);
        if seen.icon && !seen.pressed && f.s.action_slot(4) {
            seen.pressed = true;
        }
        if f.s.selecting_location() != 0 {
            seen.selecting += 1;
            // The confirm: `+attack`'s 0x10000 while the map is up.
            f.s.input.key(Action::Attack, seen.selecting >= 10);
        } else if seen.selecting > 0 && !seen.confirmed {
            seen.confirmed = true;
            f.s.input.key(Action::Attack, false);
        }
        seen.called |= f.quads.iter().any(|q| q.texture == ours);
        // The streak's splash waits its turn behind the promotion's.
        let splash = seen.texts.contains(&format!("{kills} Kill Streak!"));
        (seen.called && splash).then(|| std::mem::take(&mut seen))
    })?;
    Some(out.unwrap_or_else(|| panic!("no {kind} of ours on the compass in 40 s")))
}

/// One earn of [`airstrike_twice`].
#[derive(Debug, Default)]
struct Earn {
    /// The `dpad` drew the airstrike, action slot 4 selected it, the
    /// selector came up and went down on the confirm.
    pub(super) icon: bool,
    pub(super) pressed: bool,
    confirmed: bool,
    /// The enemy, set down where the strike aims, died under it.
    enemy_killed: bool,
    /// The loop sounds our snapshots carried during the strike, and
    /// those still on 20 s after the confirm.
    loops: std::collections::BTreeSet<String>,
    loops_after: Vec<String>,
}

/// [`live_war_pair_in`] under `streak_kill`'s probe gsc: twice, our
/// own M16 kills the enemy three times in a row (each time set down
/// before us, the kill landing inside one of our cmds as a real one
/// does), the `dpad` draws the airstrike, its key opens the selector,
/// the confirm sends the planes, the enemy stands where they aim, and
/// 20 s on the strike's loop sounds are gone. Between the two the
/// enemy kills us, which ends our streak. The HUD's texts and the
/// local sounds come back beside.
fn airstrike_twice() -> Option<(Vec<Earn>, std::collections::BTreeSet<String>, Vec<String>)> {
    use crate::play::input::Action;
    const GSC: &str = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../cod4/tests/fixtures/gsc/streak_kill"
    );
    #[derive(Clone, Copy, Debug, PartialEq)]
    enum Phase {
        Kill,
        Earned(Duration),
        Selecting(u32),
        Strike(Duration),
        Reset(Duration),
    }
    let t0 = clk_now();
    let mut earns: Vec<Earn> = vec![Earn::default()];
    let mut texts = std::collections::BTreeSet::new();
    let mut sounds = Vec::new();
    let mut phase = Phase::Kill;
    let mut kills = 0;
    let mut placed: Option<Duration> = None;
    let mut ready_at = Duration::ZERO;
    let mut under = false;
    // A seed whose strike lands on him both times.
    const SRAND: &str = "1";
    let out = live_war_pair_in(["assault_mp,0", "assault_mp,0"], Some(GSC), move |f| {
        let t = since(t0);
        let icon = f
            .hud
            .dpad_icon("airstrike_mp")
            .and_then(|m| f.hud.texture_of(&m))
            .expect("the airstrike's dpad icon");
        sounds.extend(f.local_sounds.iter().filter(|s| s.1).map(|s| s.0.clone()));
        if let Some(snap) = f.s.client().snapshots.last() {
            for e in vcod_cod4::hud::hud_elems(&snap.ps) {
                if let Some(t) = f.hud.elem_text(f.s, &e, f.level, f.shell)
                    && !t.is_empty()
                {
                    texts.insert(t);
                }
            }
        }
        let loops: Vec<String> = f.s.client().snapshots.last().map_or_else(Vec::new, |x| {
            let cs = |i: u16| f.hud.configstring(f.s, i);
            crate::cod4_audio::loop_sounds(&x.entities, &cs, &[], &origin_of)
                .into_values()
                .filter_map(|(_, a, _)| a)
                .filter(|a| a.starts_with("veh_mig29"))
                .collect()
        });
        let enemy_up = f.e.spawn().is_some();
        if phase == Phase::Kill
            && let Some(at) = placed
            && t - at > Duration::from_millis(300)
            && !enemy_up
        {
            f.s.input.key(Action::Attack, false);
            placed = None;
            kills += 1;
            ready_at = t + Duration::from_millis(1500);
            if kills == 3 {
                phase = Phase::Earned(t);
            }
        }
        let earn = earns.last_mut().unwrap();
        match phase {
            Phase::Kill => {
                if f.level < 9000 || !f.both || t < ready_at {
                    return None;
                }
                let Some(at) = placed else {
                    f.tx.send((1, "maps/mp/mp_crash_fx", "standBefore", "joiner"))
                        .ok();
                    placed = Some(t);
                    return None;
                };
                let since = t - at;
                if since < Duration::from_millis(300) {
                    return None;
                }
                if since > Duration::from_secs(6) {
                    // A wall in the way: set him down again.
                    placed = None;
                    return None;
                }
                // Aim at his chest and fire the burst over and over.
                let ps = &f.e.client().snapshots.last()?.ps;
                let target = vec3(|k| ps_f32(ps, &format!("origin[{k}]"))) + Vec3::Z * 40.0;
                let d = target - f.eye;
                let yaw = d.y.atan2(d.x).to_degrees();
                let pitch = -d.z.atan2(d.truncate().length()).to_degrees();
                let wrap = |a: f32| (a + 540.0).rem_euclid(360.0) - 180.0;
                f.s.turn([wrap(pitch - f.angles.x), wrap(yaw - f.angles.y)]);
                f.s.input
                    .key(Action::Attack, (since.as_millis() / 120).is_multiple_of(2));
            }
            Phase::Earned(at) => {
                earn.icon |= f.quads.iter().any(|q| q.texture == icon);
                // The third kill's respawn first (about 2 s), so he
                // stands where the strike aims before its bomb.
                let respawned = t - at > Duration::from_secs(3) && enemy_up;
                if earn.icon && respawned && !earn.pressed && f.s.action_slot(4) {
                    earn.pressed = true;
                    // The strike's path and spray are the stock
                    // script's draws: the same ones on every run.
                    f.tx.send((0, "", "srand", SRAND)).ok();
                }
                if f.s.selecting_location() != 0 {
                    phase = Phase::Selecting(0);
                } else if t - at > Duration::from_secs(8) {
                    // Never came: report it.
                    return Some((
                        std::mem::take(&mut earns),
                        std::mem::take(&mut texts),
                        std::mem::take(&mut sounds),
                    ));
                }
            }
            Phase::Selecting(n) => {
                if f.s.selecting_location() != 0 {
                    f.s.input.key(Action::Attack, n >= 10);
                    phase = Phase::Selecting(n + 1);
                } else {
                    f.s.input.key(Action::Attack, false);
                    earn.confirmed = true;
                    f.tx.send((1, "maps/mp/mp_crash_fx", "standAtStrike", ""))
                        .ok();
                    phase = Phase::Strike(t);
                }
            }
            Phase::Strike(at) => {
                earn.loops.extend(loops.iter().cloned());
                // Up under the strike once `standAtStrike` set him
                // down, then dead.
                if enemy_up && t - at > Duration::from_secs(3) {
                    under = true;
                }
                earn.enemy_killed |= under && !enemy_up;
                if t - at > Duration::from_secs(20) {
                    earn.loops_after = loops;
                    if earns.len() == 2 {
                        return Some((
                            std::mem::take(&mut earns),
                            std::mem::take(&mut texts),
                            std::mem::take(&mut sounds),
                        ));
                    }
                    // The strike may have spared us: the enemy ends
                    // our streak.
                    f.tx.send((0, "maps/mp/mp_crash_fx", "killedBy", "enemy"))
                        .ok();
                    earns.push(Earn::default());
                    under = false;
                    phase = Phase::Reset(t);
                }
            }
            Phase::Reset(at) => {
                if t - at > Duration::from_secs(1) {
                    phase = Phase::Kill;
                    kills = 0;
                    ready_at = t;
                }
            }
        }
        None
    })?;
    Some(out.expect("the two airstrikes did not play out in 120 s"))
}

/// [`airstrike_twice`]: each earn draws on the `dpad`, its key
/// opens the selector, the strike kills the enemy standing under it
/// and its jets' loops end; the splash reads "3 Kill Streak!" and
/// names the slot's key, and the promotion's splash plays
/// `mp_level_up`.
#[test]
fn cod4_live_three_kills_earn_the_airstrike_and_call_it_in() {
    let Some((earns, texts, sounds)) = airstrike_twice() else {
        return;
    };
    println!("{earns:#?}\n{texts:?}\n{sounds:?}");
    assert_eq!(earns.len(), 2, "{earns:#?}");
    for (k, e) in earns.iter().enumerate() {
        assert!(e.icon, "earn {k}: the dpad never drew the airstrike");
        assert!(e.pressed, "earn {k}: action slot 4 selected nothing");
        assert!(e.confirmed, "earn {k}: no selector");
        assert!(e.enemy_killed, "earn {k}: the strike killed nobody");
        assert!(!e.loops.is_empty(), "earn {k}: no jet loops");
        assert!(e.loops_after.is_empty(), "earn {k}: {:?}", e.loops_after);
    }
    for t in [
        "3 Kill Streak!",
        "Press 6 for AIRSTRIKE.",
        "You've been promoted!",
    ] {
        assert!(texts.contains(t), "{t}: {texts:?}");
    }
    assert!(
        texts.iter().all(|t| !t.contains("&&") && !t.contains("[{")),
        "{texts:?}"
    );
    assert!(sounds.iter().any(|s| s == "mp_level_up"), "{sounds:?}");
}

/// [`streak_run`]: the fifth earns the helicopter over the unused
/// airstrike, the `dpad` draws it and its key calls it in.
#[test]
fn cod4_live_five_kills_earn_the_helicopter_and_call_it_in() {
    let Some(out) = streak_run(5, "helicopter_mp", "helicopter") else {
        return;
    };
    println!("{:?}", out.texts);
    assert_eq!(out.kills, 5);
    assert!(out.icon, "the dpad never drew the helicopter");
    assert!(out.pressed, "action slot 4 selected nothing");
    assert_eq!(out.selecting, 0, "a selector for the helicopter");
    assert!(out.texts.contains("5 Kill Streak!"), "{:?}", out.texts);
}

/// What [`cod4_live_airstrike_then_helicopter_in_one_life`] saw.
#[derive(Debug, Default)]
struct OneLife {
    kills: u32,
    pub(super) last: Option<Duration>,
    strike_pressed: bool,
    selecting: u32,
    confirmed: bool,
    /// Our level time when slot 4 took the helicopter.
    heli_pressed: Option<i32>,
    radio_up: bool,
    /// Our compass drew the helicopter as ours.
    pub(super) called: bool,
    /// The weapon in hand once the radio went down.
    pub(super) back: Option<String>,
    pub(super) died: bool,
}

/// [`streak_run`]'s kills in one life: the third earns the airstrike,
/// which we call in from the selector and stand clear of; the fifth
/// earns the helicopter, whose key raises the radio, calls it in and
/// puts the M16 back. The stock `hardpointItemWaiter` once died with
/// the airstrike's `stop_location_selection`, an `endon` its returned
/// `selectAirstrikeLocation` had left on the thread, so the radio stuck
/// in hand until the next life (cod4-gsc.md 4.2).
#[test]
fn cod4_live_airstrike_then_helicopter_in_one_life() {
    use crate::play::input::Action;
    const GSC: &str = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../cod4/tests/fixtures/gsc/streak_kill"
    );
    let t0 = clk_now();
    let mut seen = OneLife::default();
    let out = live_war_pair_in(["assault_mp,0", "assault_mp,0"], Some(GSC), move |f| {
        let icon = |w: &str| {
            let tex = f.hud.dpad_icon(w).and_then(|m| f.hud.texture_of(&m));
            tex.is_some_and(|t| f.quads.iter().any(|q| q.texture == t))
        };
        let ours = f
            .hud
            .texture_of("compass_objpoint_helicopter_friendly")
            .expect("the helicopter's compass icon");
        let t = since(t0);
        let target = if seen.confirmed { 5 } else { 3 };
        if f.both
            && f.level > 9000
            && seen.kills < target
            && f.e.spawn().is_some()
            && seen
                .last
                .is_none_or(|l| t - l > Duration::from_millis(1500))
        {
            f.tx.send((1, "maps/mp/mp_crash_fx", "killedBy", "joiner"))
                .ok();
            seen.kills += 1;
            seen.last = Some(t);
        }
        seen.died |= seen.kills > 0 && f.s.spawn().is_none();
        if seen.kills == 3 && !seen.strike_pressed && icon("airstrike_mp") && f.s.action_slot(4) {
            seen.strike_pressed = true;
        }
        if f.s.selecting_location() != 0 {
            seen.selecting += 1;
            f.s.input.key(Action::Attack, seen.selecting >= 10);
        } else if seen.selecting > 0 && !seen.confirmed {
            seen.confirmed = true;
            f.s.input.key(Action::Attack, false);
            f.tx.send((0, "maps/mp/mp_crash_fx", "standClear", "")).ok();
        }
        if seen.kills == 5
            && seen.heli_pressed.is_none()
            && icon("helicopter_mp")
            && f.s.action_slot(4)
        {
            seen.heli_pressed = Some(f.level);
        }
        let pressed = seen.heli_pressed?;
        let weapon =
            f.s.client()
                .snapshots
                .last()
                .map_or(0, |x| x.ps.get("weapon").unwrap_or(0) as usize);
        let name =
            f.s.level_defs
                .as_ref()
                .and_then(|(_, d)| d.get(weapon))
                .map(|d| d.name.clone());
        seen.radio_up |= name.as_deref() == Some("helicopter_mp");
        seen.called |= f.quads.iter().any(|q| q.texture == ours);
        if seen.radio_up
            && seen.back.is_none()
            // Empty hands between the take and the switch read "".
            && let Some(n) = name.filter(|n| !n.is_empty() && n != "helicopter_mp")
        {
            seen.back = Some(n);
        }
        let done = seen.called && seen.back.is_some();
        (done || f.level - pressed > 10_000).then(|| std::mem::take(&mut seen))
    });
    let Some(out) = out else {
        return;
    };
    let s = out.expect("the helicopter's key never came in 120 s");
    println!("{s:?}");
    assert!(!s.died, "we died: a second life hides it: {s:?}");
    assert!(s.confirmed, "the airstrike was never called: {s:?}");
    assert!(s.radio_up, "the radio never came up: {s:?}");
    assert!(s.called, "the helicopter never came: {s:?}");
    assert_eq!(s.back.as_deref(), Some("m16_gl_mp"), "{s:?}");
}
