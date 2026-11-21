//! The HUD on a live server: blast and splash overlays, the dpad, the
//! compass's pings, death icons, perks and the score board.

use vcod_cod4::net::conn::Cp;

use super::*;
use crate::play::cod4_menus::Join as MenuJoin;

/// A live frag 240 units off our side through a joined client
/// (cod4-hud.md 19): its blast leaves us under `pulseStart`, so the
/// `low health` menu's overlay pulses in over the whole screen, and a
/// `hit_direction` icon takes the blast's yaw, drawn on the frag's side
/// of the centre for its 2 s and gone after. The stock
/// `shellshockOnDamage` shocks us with `frag_grenade_mp` for 0.5 s, a
/// blurred screen blend, under which 1.7 draws no icon (0x42c6f3).
#[test]
fn cod4_live_blast_pulses_the_overlay_and_points_an_icon_at_it() {
    let sets = [
        ("scr_game_playerwaittime", "1"),
        ("scr_game_matchstarttime", "1"),
    ];
    let Some(r) = live_join_on("mp_crash", crash_world(), &sets, |s, spawned, t0, tx| {
        spawned.as_ref()?;
        let install = vcod_cod4::testing::cod4_install()?;
        let mut hud = crate::cod4_hud::Cod4Hud::default();
        hud.load(&install.root().join("main"), "mp_crash");
        let shell = crate::console::shell::Shell::new();
        let size = (1280.0, 960.0);
        let frame = |hud: &mut crate::cod4_hud::Cod4Hud, s: &mut Cod4Session| {
            s.step(since(t0).as_secs_f64() * 1000.0).unwrap();
            let fired = s.take_fired();
            let now = s.scene().map_or(0, |x| x.render_time);
            hud.update(s, &fired, now);
            clk_sleep(Duration::from_millis(16));
            now
        };
        // Past the stock spawn protection: a grenade within 250 of
        // the spawn point hurts nobody for 3.5 s from the spawn
        // (`Callback_PlayerDamage`, `lastSpawnTime + 3500`), and the
        // spawn's snapshot is no earlier than `lastSpawnTime`.
        let spawned_at = s.client().snapshots.last()?.server_time;
        let level = |s: &Cod4Session| {
            s.client()
                .snapshots
                .last()
                .map_or(0, |x| x.server_time.raw())
        };
        let end = since(t0) + Duration::from_millis(3000);
        while since(t0) < end || level(s) < (spawned_at + 3600).raw() {
            frame(&mut hud, s);
        }
        // The frag 240 units off along the first yaw with open,
        // level floor that far (the spawn's own surroundings decide no
        // more whether the blast reaches us), and the view a quarter
        // turn right of it, so the blast comes from the left.
        let f = |n: &str| ps_f32(&s.client().snapshots.last().unwrap().ps, n);
        let feet = Vec3::new(f("origin[0]"), f("origin[1]"), f("origin[2]"));
        let clip = &s.world()?.clip;
        let z = Vec3::ZERO;
        let side = (0..24).map(|k| k as f32 * 15.0).find(|y| {
            let d = Vec3::new(y.to_radians().cos(), y.to_radians().sin(), 0.0);
            let at = feet + d * 240.0 + Vec3::Z * 8.0;
            let open = [8.0, 60.0].iter().all(|h| {
                let from = feet + Vec3::Z * *h;
                let mut t = clip.trace(from, at, z, z, 0x280_6891);
                clip.trace_static_models(from, at, 0x280_6891, &mut t);
                t.fraction == 1.0 && !t.startsolid
            });
            let down = clip.trace(at, at - Vec3::Z * 16.0, z, z, 0x280_6891);
            open && down.fraction < 1.0 && down.normal.z > 0.9
        })?;
        let d = Vec3::new(side.to_radians().cos(), side.to_radians().sin(), 0.0);
        let at = (feet + d * 240.0 + Vec3::Z * 8.0).to_array();
        s.view = [0.0, side - 90.0];
        tx.send(Ctl::DropFrag(at, 50)).unwrap();
        let (mut alpha, mut overlay, mut health) = (0.0f32, false, 100);
        let (mut icons, mut gone) = (Vec::new(), false);
        // A frame with a live slot under the shock's blend, and whether
        // any such frame drew an icon.
        let mut shocked = (false, false);
        // The view's damage kick (cod4-client-view.md 14): its peak
        // and its last value.
        let (mut kick_peak, mut kick_last) = ([0.0f32; 3], [0.0f32; 3]);
        let end = since(t0) + Duration::from_millis(3500);
        while since(t0) < end {
            let now = frame(&mut hud, s);
            let dk = hud.damage_kick();
            kick_last = dk.view(now - dk.time, 0.0, false);
            if kick_last[2].abs() > kick_peak[2].abs() {
                kick_peak = kick_last;
            }
            let Some(scene) = s.scene() else { continue };
            health = health.min(s.client().snapshots.last().unwrap().ps.stats[0]);
            let a = hud.low_health_alpha(now);
            if a > alpha {
                alpha = a;
                // A quad over the whole window at the overlay's alpha.
                overlay = hud.build(s, size, now, None, &shell).iter().any(|q| {
                    q.rgba[3] == a
                        && q.verts[0][0] <= 0.0
                        && q.verts[2][0] >= size.0
                        && q.verts[2][1] >= size.1
                });
            }
            let q = hud.damage_icons(s, &scene.view, size, now, scene.angles.y);
            let (p, start, ms) = crate::play::cod4_shock::of_session(s);
            let flashed = crate::play::cod4_shock::flashbanged(p.as_ref(), start, ms, now);
            let live = hud
                .damage_slots()
                .iter()
                .any(|i| i.ms != 0 && now - i.start > 0 && now - i.start < i.ms);
            if flashed && live {
                shocked.0 = true;
                shocked.1 |= !q.is_empty();
            }
            if !q.is_empty() && icons.is_empty() {
                icons = hud
                    .damage_slots()
                    .iter()
                    .filter(|i| i.ms != 0)
                    .map(|i| (i.yaw, q.len(), q[0].verts))
                    .collect();
            }
            gone = !icons.is_empty() && q.is_empty();
        }
        Some((
            health, alpha, overlay, icons, gone, shocked, kick_peak, kick_last, side,
        ))
    }) else {
        return;
    };
    let (health, alpha, overlay, icons, gone, shocked, kick_peak, kick_last, side) =
        r.expect("never spawned");
    assert_eq!(
        shocked,
        (true, false),
        "(hit under the shock, icon drawn then)"
    );
    // A blast from the left, `damageCount` * 0.2 (at least 5): the
    // view rolls right, barely pitches, and is level again by 500 ms.
    assert!(kick_peak[2] >= 4.0, "kick {kick_peak:?}");
    assert!(kick_peak[0].abs() < 1.5, "kick {kick_peak:?}");
    assert_eq!(kick_last, [0.0; 3]);
    assert!(health > 0 && health < 55, "health {health}");
    assert_eq!(alpha, 1.0, "the first pulse's full alpha");
    assert!(overlay, "no overlay quad");
    let [(yaw, n, verts)] = icons[..] else {
        panic!("icons {icons:?}");
    };
    assert_eq!(n, 1);
    // The frag lay at yaw `side` from us, so the blast came along the
    // opposite way (`damageYaw` is the push's way), 10 degrees of
    // jitter either way.
    let off = (yaw - (side + 180.0)).rem_euclid(360.0);
    assert!(
        off.min(360.0 - off) <= 10.0,
        "icon yaw {yaw}, frag at {side}"
    );
    // Facing a quarter turn right of it, the icon turns from below
    // the centre to its left,
    // the frag's side, `cg_hudDamageIconOffset` 128 grid units out.
    let xs = verts.map(|v| v[0]);
    assert!(
        xs.iter().all(|&x| x < 640.0 - 128.0 * 2.0 + 30.0),
        "{verts:?}"
    );
    assert!(gone, "the icon outlived its 2 s");
}

/// The stock first-spawn splash on vcod-server's `war`
/// (`_hud_message.gsc`'s `notifyTitle`, `glowColor` (0.3, 0.6, 0.3),
/// `glowAlpha` 1) draws with its glow: four additive quads a glyph in
/// the glow's colour times 0.1 (cod4-hud.md 22). Once the compass
/// shows, its tickertape slides with the view: facing south its S runs 0.25 to
/// 0.75, and an eighth of a turn left moves it an eighth down.
#[test]
fn cod4_live_splash_glows_and_the_ticker_turns() {
    // The prematch holds the compass back; a short one lets it show.
    let sets = [
        ("scr_game_playerwaittime", "1"),
        ("scr_game_matchstarttime", "1"),
    ];
    let Some(r) = live_join_on("mp_crash", crash_world(), &sets, |s, spawned, t0, _| {
        spawned.as_ref()?;
        let install = vcod_cod4::testing::cod4_install()?;
        let mut hud = crate::cod4_hud::Cod4Hud::default();
        hud.load(&install.root().join("main"), "mp_crash");
        let shell = crate::console::shell::Shell::new();
        let size = (1280.0, 960.0);
        // The glowing text's colour and when its first letter is out
        // (`fx_birth_time` plus a letter's time): the newest snapshot
        // runs ahead of the drawn time, so the elem may be seen a
        // frame or more before its typewriter shows a letter.
        let (mut glow, mut quads) = (None::<([u8; 4], i32)>, Vec::new());
        let end = since(t0) + Duration::from_millis(6000);
        while since(t0) < end && quads.is_empty() {
            s.step(since(t0).as_secs_f64() * 1000.0).unwrap();
            let fired = s.take_fired();
            let now = s.scene().map_or(0, |x| x.render_time);
            hud.update(s, &fired, now);
            if let Some((_, shown)) = glow
                && now >= shown
            {
                quads = hud.build(s, size, now, None, &shell);
            }
            clk_sleep(Duration::from_millis(16));
            let snap = s.client().snapshots.last().unwrap();
            glow = glow.or_else(|| {
                vcod_cod4::hud::hud_elems(&snap.ps)
                    .iter()
                    .find(|e| e.glow_color[3] != 0 && e.kind == vcod_cod4::hud::kind::TEXT)
                    .map(|e| (e.glow_color, e.fx_birth_time + e.fx_letter_time))
            });
        }
        let glow = glow.map(|g| g.0);
        // The compass shows once the spawn's splash has gone.
        let eye = glam::Vec3::ZERO;
        let mut now = 0;
        let end = since(t0) + Duration::from_millis(30000);
        while since(t0) < end {
            s.step(since(t0).as_secs_f64() * 1000.0).unwrap();
            let fired = s.take_fired();
            now = s.scene().map_or(0, |x| x.render_time);
            hud.update(s, &fired, now);
            clk_sleep(Duration::from_millis(16));
            let q = hud.build(s, size, now, Some((eye, 0.0)), &shell);
            if q.iter().any(|q| q.texture.contains("tickertape")) {
                break;
            }
        }
        let tape = |yaw: f32| {
            hud.build(s, size, now, Some((eye, yaw)), &shell)
                .into_iter()
                .filter(|q| q.texture.contains("tickertape") && !q.texture.contains("background"))
                .map(|q| q.uvs[0][0])
                .collect::<Vec<_>>()
        };
        let north = s.gamestate().map_or(0.0, |g| {
            String::from_utf8_lossy(g.configstring(vcod_cod4::hud::CS_NORTH_YAW))
                .trim()
                .parse::<f32>()
                .unwrap_or(0.0)
        });
        Some((glow, quads, tape(north - 180.0), tape(north - 135.0)))
    }) else {
        return;
    };
    let (glow, quads, at0, at90) = r.expect("never spawned");
    let glow = glow
        .expect("no glowing hudelem")
        .map(|c| f32::from(c) / 255.0);
    let add: Vec<_> = quads
        .iter()
        .filter(|q| q.texture.starts_with(crate::hud::ADDITIVE))
        .collect();
    assert!(
        !add.is_empty() && add.len() % 4 == 0,
        "{} glow quads",
        add.len()
    );
    for q in &add {
        for (got, g) in q.rgba.iter().zip(glow).take(3) {
            assert!((got - g * 0.1).abs() < 1e-6, "{:?}", q.rgba);
        }
    }
    // One span each; the left edge's S is the centre less 0.25, the
    // centre `-(yaw - north) / 360`: 0.5 facing south, 0.375 an eighth
    // left of it.
    let (&[a], &[b]) = (&at0[..], &at90[..]) else {
        panic!("tape {at0:?} {at90:?}");
    };
    assert!(
        (a - 0.25).abs() < 1e-4 && (b - 0.125).abs() < 1e-4,
        "{a} {b}"
    );
}

/// [`live_war_pair`]: the stock
/// `_hardpoints::giveHardpointItem("radar_mp")` puts the UAV on action
/// slot 4 and the `dpad` menu draws its icon; the slot's key selects
/// it, the stock `hardpointItemWaiter` calls the UAV in
/// (`setTeamRadar`, `ps.radarEnabled`), the sweep's line crosses the
/// compass and the compass draws the enemy's ping (cod4-hud.md 15).
#[test]
fn cod4_live_uav_hardpoint_shows_on_the_dpad_and_pings_the_enemy() {
    #[derive(Default)]
    struct Seen {
        given: bool,
        icon: Option<Duration>,
        pressed: Option<Duration>,
        radar: Option<Duration>,
        line_s: Vec<f32>,
        pinged: Option<Duration>,
    }
    let t0 = clk_now();
    let mut seen = Seen::default();
    let Some(out) = live_war_pair(move |f| {
        let radar_icon = f
            .hud
            .dpad_icon("radar_mp")
            .and_then(|m| f.hud.texture_of(&m))
            .expect("radar_mp's dpad icon");
        let ping = f
            .hud
            .texture_of("compassping_enemy")
            .expect("the enemy ping");
        let line = f
            .hud
            .texture_of("compass_radarline")
            .expect("the sweep's line");
        if f.both && !seen.given && f.level > 9000 {
            seen.given = true;
            f.tx.send((
                0,
                "maps/mp/gametypes/_hardpoints",
                "giveHardpointItem",
                "radar_mp",
            ))
            .ok();
        }
        let t = since(t0);
        if seen.icon.is_none() && f.quads.iter().any(|q| q.texture == radar_icon) {
            seen.icon = Some(t);
        }
        if seen.icon.is_some() && seen.pressed.is_none() && f.s.action_slot(4) {
            seen.pressed = Some(t);
        }
        let on =
            f.s.client()
                .snapshots
                .last()
                .and_then(|x| x.ps.get("radarEnabled"))
                == Some(1);
        if on && seen.radar.is_none() {
            seen.radar = Some(t);
        }
        let lines: Vec<_> = f.quads.iter().filter(|q| q.texture == line).collect();
        assert!(on || lines.is_empty(), "a sweep line with no UAV up");
        if let Some(q) = lines.first() {
            seen.line_s.push((q.uvs[0][0] + q.uvs[2][0]) * 0.5);
        }
        if seen.radar.is_some() && f.quads.iter().any(|q| q.texture == ping) {
            seen.pinged = Some(t);
        }
        // The ping, and a second of the sweep.
        (seen.pinged.is_some() && seen.line_s.len() > 60).then(|| std::mem::take(&mut seen))
    }) else {
        return;
    };
    let out = out.expect("no ping and a second of the sweep in 120 s");
    println!(
        "given {} icon {:?} pressed {:?} radar {:?} ping {:?} line {}",
        out.given,
        out.icon,
        out.pressed,
        out.radar,
        out.pinged,
        out.line_s.len()
    );
    assert!(out.given, "the two never spawned");
    assert!(out.icon.is_some(), "the dpad never drew the UAV");
    assert!(out.pressed.is_some(), "action slot 4 selected nothing");
    assert!(out.radar.is_some(), "radarEnabled never came");
    assert!(out.pinged.is_some(), "the compass never pinged the enemy");
    // The line's texture slides one way across the compass as the
    // sweep goes east (`compassRadarUpdateTime` 4 s a pass).
    let moved: Vec<f32> = out.line_s.windows(2).map(|w| w[1] - w[0]).collect();
    assert!(
        moved.iter().filter(|d| **d < 0.0).count() > moved.len() * 8 / 10,
        "{:?}",
        out.line_s
    );
}

/// Two clients on vcod-server's war, both on allies: the teammate's
/// stock `pingPlayer` (the quick messages' call) sets his `eFlags`
/// 0x400000 for 3 s and our compass flashes `objective_friendly_chat`
/// over his arrow; then he dies, his body leaves our snapshots and our
/// compass keeps his arrow where he fell for 1.5 s
/// (`iw3mp.exe` 0x429880, cod4-hud.md 15.9).
#[test]
fn cod4_live_compass_flashes_a_pinged_friend_and_keeps_him_when_he_falls() {
    const GSC: &str = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../cod4/tests/fixtures/gsc/compass_friend"
    );
    const PATH: &str = "maps/mp/mp_crash_fx";
    #[derive(Debug, Default)]
    struct Seen {
        pinged: Option<i32>,
        flag: Option<i32>,
        chat: Option<i32>,
        flag_off: Option<i32>,
        killed: Option<i32>,
        gone: Option<i32>,
        back: Option<i32>,
        last_arrow: Option<i32>,
    }
    let mut seen = Seen::default();
    let out = live_war_pair_on(
        ["allies", "allies"],
        ["assault_mp,0", "assault_mp,0"],
        Some(GSC),
        move |f| {
            let arrow = f.hud.texture_of("compassping_friendly_mp")?;
            let chat = f.hud.texture_of("objective_friendly_chat")?;
            let t = f.level;
            if !f.both && seen.killed.is_none() || t < 3000 {
                return None;
            }
            let snap = f.s.client().snapshots.last()?;
            let body = snap
                .entities
                .iter()
                .find(|e| e.etype() == ET_PLAYER && ent_u32(e, "clientNum") == 1);
            let flagged = body.is_some_and(|e| ent_u32(e, "lerp.eFlags") & 0x40_0000 != 0);
            let has = |tex: &str| f.quads.iter().any(|q| q.texture == tex);
            if seen.pinged.is_none() {
                seen.pinged = Some(t);
                f.tx.send((1, PATH, "ping", "")).ok();
            }
            if flagged && seen.flag.is_none() {
                seen.flag = Some(t);
            }
            if seen.flag.is_some() && !flagged && seen.flag_off.is_none() {
                seen.flag_off = Some(t);
            }
            if has(&chat) && seen.chat.is_none() {
                seen.chat = Some(t);
            }
            // Once the flash is over, he dies.
            if seen.flag_off.is_some() && seen.killed.is_none() {
                seen.killed = Some(t);
                f.tx.send((1, PATH, "die", "")).ok();
            }
            if seen.killed.is_some() && body.is_none() && seen.gone.is_none() {
                seen.gone = Some(t);
            }
            if seen.gone.is_some() && body.is_some() && seen.back.is_none() {
                seen.back = Some(t);
            }
            if seen.gone.is_some() && seen.back.is_none() && has(&arrow) {
                seen.last_arrow = Some(t);
            }
            let done = seen.gone.is_some_and(|g| t > g + 2500);
            done.then(|| std::mem::take(&mut seen))
        },
    );
    let Some(out) = out else {
        return;
    };
    let s = out.expect("no ping, death and 2.5 s after it in 120 s");
    println!("{s:?}");
    let (flag, off) = (s.flag.expect("no 0x400000"), s.flag_off.unwrap());
    assert!(
        (2900..=3200).contains(&(off - flag)),
        "the flag held {} ms",
        off - flag
    );
    let chat = s.chat.expect("the compass never flashed the chat icon");
    assert!(chat >= flag && chat < off, "{s:?}");
    let (gone, last) = (
        s.gone.unwrap(),
        s.last_arrow.expect("no arrow once he fell"),
    );
    assert!(
        (1300..=1600).contains(&(last - gone)),
        "the arrow stayed {} ms",
        last - gone
    );
}

/// [`live_war_pair`]: the enemy kills us through the stock damage
/// callback (`gsc/killcam`), and war's `giveTeamScore` sends `G 10`.
/// The bar menus' `team( score )` reads it at once, with no `score`
/// reply: `cg.teamScores` takes `G` and `H` as well as `b`. Read off
/// `b` alone, the top `scorebar` said "Tied 0 - 0" through a match
/// until Tab asked for the board (cod4-hud.md 23).
#[test]
fn cod4_live_a_kill_moves_the_team_scores_without_the_board() {
    const GSC: &str = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../cod4/tests/fixtures/gsc/killcam"
    );
    let mut sent = None;
    let Some(out) = live_war_pair_in(["assault_mp,0", "assault_mp,0"], Some(GSC), move |f| {
        if !f.both || f.level < 9000 {
            return None;
        }
        let Some(at) = sent else {
            f.tx.send((0, "maps/mp/mp_crash_fx", "killedBy", "enemy"))
                .ok();
            sent = Some(f.level);
            return None;
        };
        let now = f.level;
        let scores = f.hud.menu_game(f.s, now).team_scores;
        let bars = f.hud.zone_menu_texts(f.s, "scorebars", f.shell, now);
        let asked = f.hud.counts.scores;
        (scores[1] == 10 || now > at + 3000).then_some((scores, bars, asked))
    }) else {
        return;
    };
    let (scores, bars, asked) = out.expect("the kill never happened");
    assert_eq!(asked, 0, "a `score` reply came in: the gate needs none");
    assert_eq!(scores, [0, 10, 0, 0], "axis's kill never reached the bars");
    assert!(bars.iter().any(|t| t.trim() == "10"), "{bars:?}");
}

/// [`live_war_pair`] with both on allies: the teammate dies
/// (`suicide`), and the stock `_deathicons::addDeathIcon` raises a
/// `headicon_dead` waypoint at the clone's `origin` plus 54, which we
/// draw over his corpse once turned to it. A corpse whose `origin` read `(0, 0, 0)` put
/// it in the middle of the map on every death (cod4-hud.md 16.4).
#[test]
fn cod4_live_a_teammates_death_icon_sits_over_his_corpse() {
    const GSC: &str = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../cod4/tests/fixtures/gsc/compass_friend"
    );
    let mut died: Option<(i32, Vec3)> = None;
    let Some(out) = live_war_pair_on(
        ["allies", "allies"],
        ["assault_mp,0", "assault_mp,0"],
        Some(GSC),
        move |f| {
            let t = f.level;
            if !f.both || t < 4000 {
                return None;
            }
            let Some((k, at)) = died else {
                let ps = &f.e.client().snapshots.last()?.ps;
                died = Some((t, vec3(|k| ps_f32(ps, &format!("origin[{k}]")))));
                f.tx.send((1, "maps/mp/mp_crash_fx", "die", "")).ok();
                return None;
            };
            let snap = f.s.client().snapshots.last()?;
            let icon = vcod_cod4::hud::hud_elems(&snap.ps).into_iter().find(|e| {
                e.kind == vcod_cod4::hud::kind::WAYPOINT
                    && f.hud
                        .configstring(f.s, vcod_cod4::hud::CS_MATERIALS + e.material as u16)
                        == "headicon_dead"
            });
            if t > k + 3000 {
                return Some(Err(format!("no death icon drawn by {t}")));
            }
            let e = icon?;
            let (yaw, pitch) = (f.angles.y.to_radians(), -f.angles.x.to_radians());
            let size = (1280.0, 960.0);
            let up = crate::camera::up_hint(yaw, pitch, 0.0);
            let fwd = crate::camera::basis(yaw, pitch).0;
            let right = fwd.cross(up).normalize();
            let view = crate::cod2_world_hud::WorldView {
                view_proj: crate::camera::view_proj_from(
                    f.eye,
                    yaw,
                    pitch,
                    0.0,
                    crate::camera::DEFAULT_FOV_DEG,
                    size.0 / size.1,
                ),
                right,
                up: right.cross(fwd),
                size,
            };
            let drawn = f
                .hud
                .world_quads(f.s, &view, t)
                .iter()
                .any(|q| q.texture.ends_with("headicon_dead.iwi"));
            let icon = Vec3::new(e.x, e.y, e.z);
            if !drawn {
                // Turned to it: where we look after the spawn is the
                // spawn point's, not the test's.
                let to = icon - f.eye;
                f.s.view = [
                    -to.z.atan2(to.truncate().length()).to_degrees(),
                    to.y.atan2(to.x).to_degrees(),
                ];
                return None;
            }
            Some(Ok((icon, at, drawn)))
        },
    ) else {
        return;
    };
    let (icon, at, drawn) = out.expect("the teammate never died").unwrap();
    // The corpse comes down from where he stood onto the floor under him.
    assert!(
        (icon.truncate() - at.truncate()).length() < 2.0,
        "the icon {icon} is not over the corpse at {at}"
    );
    assert!(
        icon.z <= at.z + 54.5 && icon.z > at.z + 54.0 - 64.0,
        "the icon {icon} is not 54 over the corpse at {at}"
    );
    assert!(drawn, "the icon at {icon} is not drawn as a world sprite");
}

/// [`live_war_pair`]: the enemy fires one M16 round; our session's
/// events play through a headless mixer as the client plays them
/// (`cod4_client::play_event_cues`), and the shot's
/// `compassping_enemyfiring` holds full alpha for the fire sound's
/// length before its 2 s fade, where a 0 ms hold fades at once
/// (`CG_CompassAddWeaponPingInfo`, cod4-hud.md 15.9).
#[test]
fn cod4_live_enemy_shot_ping_holds_for_its_fire_sound() {
    use crate::audio::{AudioOpts, AudioSystem};
    use crate::play::input::Action;
    let Some(install) = vcod_cod4::testing::cod4_install() else {
        return;
    };
    let root = install.root().to_path_buf();
    let mut av = None;
    let (mut pressed, mut fired_at, mut full_until) = (None, None, None);
    let mut faded = false;
    let out = live_war_pair(move |f| {
        let frozen = |s: &Cod4Session| {
            s.client()
                .snapshots
                .last()
                .and_then(|x| x.ps.get("pm_flags"))
                .is_none_or(|p| p & 0x800 != 0)
        };
        let (audio, fs, sounds) = av.get_or_insert_with(|| {
            let ((mut sounds, _), files) = crate::cod4_fly::load_av(&root, "mp_crash").unwrap();
            let mut fs = crate::cod4_fly::open_fs(&root, None, None).unwrap();
            for (p, w) in files {
                fs.overlay(&p, w);
            }
            let mut a = AudioSystem::with_aliases(
                &fs,
                AudioOpts {
                    enabled: false,
                    volume: 1.0,
                    rate: 44100,
                },
                sounds.take_table().unwrap(),
            );
            a.mock_mixer();
            a.on_gamestate("mp_crash");
            (a, fs, sounds)
        });
        let gs = f.s.gamestate()?;
        let weapons = weapon_list(gs);
        let cs = |k: u16| String::from_utf8_lossy(gs.configstring(k)).into_owned();
        crate::cod4_client::play_event_cues(
            audio,
            fs,
            sounds,
            f.hud,
            f.fired,
            &weapons,
            &cs,
            &crate::cod4_audio::Placement {
                own: None,
                flash: &|_| None,
            },
        );
        audio.pump_mixer(1.0 / 60.0);
        audio.step(&HashMap::new(), &HashMap::new(), None);
        if !f.both || frozen(f.e) || frozen(f.s) {
            return None;
        }
        // The enemy's cmds past his spawn's `commandTime`.
        let spawned_at =
            f.e.client()
                .snapshots
                .last()
                .and_then(|x| x.ps.get("commandTime"));
        if pressed.is_none() && spawned_at.is_none_or(|ct| f.e.cmd_time() <= ct as i32) {
            return None;
        }
        let t = f.level;
        // One tap of the trigger.
        match pressed {
            None => {
                f.e.input.key(Action::Attack, true);
                pressed = Some(t);
            }
            Some(p) if t > p + 100 => f.e.input.key(Action::Attack, false),
            _ => {}
        }
        if fired_at.is_none()
            && f.fired
                .iter()
                .any(|x| x.entity == Some(1) && x.name == "EV_FIRE_WEAPON")
        {
            fired_at = Some(t);
        }
        let flash = f.hud.texture_of("compassping_enemyfiring")?;
        let alpha = f
            .quads
            .iter()
            .find(|q| q.texture == flash)
            .map(|q| q.rgba[3]);
        if let (Some(at), Some(a)) = (fired_at, alpha) {
            if a >= 0.999 && !faded {
                full_until = Some(t - at);
            } else if a < 0.999 {
                faded = true;
            }
        }
        (faded && fired_at.is_some_and(|at| t > at + 500)).then_some(full_until)
    });
    let Some(out) = out else {
        return;
    };
    let held = out
        .expect("no shot and fade in 120 s")
        .expect("no full-alpha flash");
    println!("the flash held {held} ms");
    assert!((150..5000).contains(&held), "held {held} ms");
}

/// [`live_war_pair`]: the enemy fires his M16 (unsilenced) with no UAV
/// up and our compass flashes `compassping_enemyfiring` where he
/// stands, never the sweep's ping; then he calls in the stock
/// helicopter (`triggerHardpoint("helicopter_mp")`) and our compass
/// draws the enemy's `compass_objpoint_helicopter_busy`
/// (`CG_CompassAddWeaponPingInfo` and the vehicles' draw,
/// cod4-hud.md 15.7).
#[test]
fn cod4_live_compass_flashes_the_enemys_shots_and_draws_his_helicopter() {
    use crate::play::input::Action;
    let t0 = clk_now();
    let (mut firing, mut swept, mut held, mut called) = (None, 0, false, false);
    let Some(out) = live_war_pair(move |f| {
        let tex = |m: &str| f.hud.texture_of(m).unwrap_or_else(|| panic!("{m}"));
        let (flash, ping, heli) = (
            tex("compassping_enemyfiring"),
            tex("compassping_enemy"),
            tex("compass_objpoint_helicopter_busy"),
        );
        let frozen = |s: &Cod4Session| {
            s.client()
                .snapshots
                .last()
                .and_then(|x| x.ps.get("pm_flags"))
                .is_none_or(|p| p & 0x800 != 0)
        };
        if !f.both || frozen(f.e) || frozen(f.s) {
            return None;
        }
        // Bursts of a third of a second, a second apart.
        let want = firing.is_none() && since(t0).as_millis() % 1000 < 330;
        if want != held {
            f.e.input.key(Action::Attack, want);
            held = want;
        }
        swept += f.quads.iter().filter(|q| q.texture == ping).count();
        if firing.is_none() && f.quads.iter().any(|q| q.texture == flash) {
            firing = Some(since(t0));
        }
        if firing.is_some() && !called {
            called = true;
            f.tx.send((
                1,
                "maps/mp/gametypes/_hardpoints",
                "triggerHardpoint",
                "helicopter_mp",
            ))
            .ok();
        }
        // The helicopter's icon is 40 square: twice a ping's width.
        let wide = |q: &crate::hud::HudQuad| {
            let (a, b) = (q.verts[0], q.verts[1]);
            ((b[0] - a[0]).powi(2) + (b[1] - a[1]).powi(2)).sqrt()
        };
        let pings: Vec<f32> = f
            .quads
            .iter()
            .filter(|q| q.texture == flash)
            .map(wide)
            .collect();
        f.quads
            .iter()
            .find(|q| q.texture == heli)
            .map(|q| (firing, swept, wide(q), pings.first().copied()))
    }) else {
        return;
    };
    let (firing, swept, heli, flash) = out.expect("no helicopter on the compass in 120 s");
    println!("firing {firing:?} sweep pings {swept} helicopter {heli} flash {flash:?}");
    assert!(firing.is_some(), "no shot flashed on the compass");
    // The enemies' draw shows the radar ping too at
    // `1 - cg.time / 4000` with no sweep yet: only in a level's
    // first 4 s.
    assert!(swept < 10, "{swept} sweep pings with no UAV up");
    if let Some(flash) = flash {
        assert!((heli / flash - 40.0 / 18.75).abs() < 0.01, "{heli} {flash}");
    }
}

/// Two clients on vcod-server's war: us on allies with the stock
/// `offline_class2_mp` (Bomb Squad, UAV Jammer, Dead Silence:
/// `mp/offline_classTable.csv`), an enemy on axis with Assault, both
/// walking. Our steps play the quiet `qstep_*_plr_*` set and the
/// enemy's own the plain one, each off the client state's perks; the
/// stock `_weapons::claymoreDetectionTrigger("axis")` laid where we
/// stand makes the stock `showHeadIcon` raise our `waypoint_bombsquad`
/// waypoint, which draws in the world ahead once we back off
/// (cod4-hud.md 16, cod4-sound.md 4).
#[test]
fn cod4_live_dead_silence_steps_quiet_and_bomb_squad_shows_the_trigger() {
    use crate::play::input::Action;
    use vcod_server::cod4::{Cod4Config, Cod4Server};
    let Some(install) = vcod_cod4::testing::cod4_install() else {
        return;
    };
    let world = crash_world();
    let _live = live_slot();
    let mut sv = Cod4Server::new(
        Cod4Config {
            map: "mp_crash".into(),
            gametype: "war".into(),
            hostname: "vcod".into(),
            max_clients: 4,
        },
        7,
    );
    sv.set_cvar("scr_game_playerwaittime", "1");
    sv.set_cvar("scr_game_matchstarttime", "1");
    sv.load(Some(&install)).unwrap();
    let sock = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
    sock.set_nonblocking(true).unwrap();
    let addr = sock.local_addr().unwrap();
    let (tx, rx) = std::sync::mpsc::channel::<usize>();
    let (ready_tx, ready_rx) = std::sync::mpsc::channel::<()>();
    let main = install.root().join("main");
    let root = install.root().to_path_buf();
    let clock = sim::Clock::new();
    let client = sim::spawn(&clock, move || {
        // The sounds and the HUD before the level's clock runs: a
        // loaded machine takes long enough over them to spawn us past
        // the grace period, where the class change waits a respawn.
        let ((mut sounds, _), _) = crate::cod4_fly::load_av(&root, "mp_crash").unwrap();
        let mut hud = crate::cod4_hud::Cod4Hud::default();
        hud.load(&main, "mp_crash");
        ready_tx.send(()).unwrap();
        let join = |team: &str, class: &str| MenuJoin {
            team: Some(team.into()),
            class: Some(class.into()),
        };
        let us = join("allies", "assault_mp,0");
        let mut s = Cod4Session::connect(addr, "joiner", Cp::Bare, us).unwrap();
        let them = join("axis", "assault_mp,0");
        let mut e = Cod4Session::connect(addr, "enemy", Cp::Bare, them).unwrap();
        if let Some(w) = world {
            s.set_world(w.clone());
            e.set_world(w);
        }
        let icon = hud
            .texture_of("waypoint_bombsquad")
            .expect("waypoint_bombsquad's image");
        let shell = crate::console::shell::Shell::new();
        // Assault's two perks, the third its `specialty_null`.
        let spawn_icons: Vec<String> = ["specialty_bulletdamage", "specialty_longersprint"]
            .iter()
            .map(|p| hud.texture_of(p).expect("the perk icon"))
            .collect();
        let mut shown = [false; 2];
        let frozen = |s: &Cod4Session| {
            s.client()
                .snapshots
                .last()
                .and_then(|x| x.ps.get("pm_flags"))
                .is_some_and(|f| f & 0x800 != 0)
        };
        let t0 = clk_now();
        let (mut ours, mut theirs) = (HashSet::new(), HashSet::new());
        let (mut perks, mut chose, mut laid, mut walked, mut drawn) = (0, false, false, None, None);
        let mut up_at = None;
        let backed = |w: Option<Instant>| w.is_some_and(|w| since(w) > Duration::from_millis(1500));
        while since(t0) < Duration::from_secs(120)
            && !(drawn.is_some() && backed(walked) && !ours.is_empty() && !theirs.is_empty())
        {
            let now = since(t0).as_secs_f64() * 1000.0;
            for c in [&mut s, &mut e] {
                c.step(now).unwrap();
                c.take_commands();
            }
            let up = s.spawn().is_some() && e.spawn().is_some() && !frozen(&s) && !frozen(&e);
            let level = s
                .client()
                .snapshots
                .last()
                .map_or(0, |x| x.server_time.raw());
            if up {
                up_at.get_or_insert(level);
            }
            // The class menu's response for the offline class, inside
            // the grace period: `menuClass` gives the loadout at once.
            // Not before the spawn's Assault icons drew (or a second
            // of the server's clock went by without them): a loaded
            // machine can spawn us past the prematch, unfrozen at once.
            let icons_seen = shown == [true; 2] || up_at.is_some_and(|t| level > t + 1000);
            if up && !chose && icons_seen {
                chose = true;
                let gs = s.gamestate().unwrap();
                let menu = crate::play::cod4_menus::menu_slot(gs, "changeclass").unwrap();
                let sid = gs.systeminfo("sv_serverid").unwrap();
                s.command(&format!("mr {sid} {menu} offline_class2_mp,0"));
            }
            if up && !laid && perks & 0x100 != 0 {
                laid = true;
                tx.send(0).ok();
                walked = Some(clk_now());
                s.input.key(Action::Back, true);
                e.input.key(Action::Back, true);
            }
            // A second back from the trigger, then stand.
            if walked.is_some_and(|w| since(w) > Duration::from_millis(1200)) {
                s.input.key(Action::Back, false);
                e.input.key(Action::Back, false);
            }
            // The enemy's predicted ring fires in its scene.
            e.scene();
            for (c, heard) in [(&mut s, &mut ours), (&mut e, &mut theirs)] {
                let fired = c.take_fired();
                let Some(gs) = c.gamestate() else { continue };
                let weapons = weapon_list(gs);
                let cs = |k: u16| String::from_utf8_lossy(gs.configstring(k)).into_owned();
                for f in fired.iter().filter(|f| f.entity.is_none()) {
                    heard.extend(sounds.cues(f, &weapons, &cs).into_iter().map(|c| c.alias));
                }
            }
            if let Some(snap) = s.client().snapshots.last() {
                let own = s.gamestate().map_or(0, |g| g.client_num as u32);
                perks = client_perks(snap, own);
            }
            let Some(scene) = s.scene() else {
                clk_sleep(Duration::from_millis(16));
                continue;
            };
            hud.update(&mut s, &[], scene.server_time);
            // The spawn's perk icons (`_globallogic::spawnPlayer`'s
            // `showPerk`, hudelems at the bottom right for 3 s).
            let flat = hud.build(&s, (1280.0, 960.0), scene.render_time, None, &shell);
            for (k, t) in spawn_icons.iter().enumerate() {
                shown[k] |= flat.iter().any(|q| q.texture == *t);
            }
            let (yaw, pitch) = (scene.angles.y.to_radians(), -scene.angles.x.to_radians());
            let size = (1280.0, 960.0);
            let view_proj = crate::camera::view_proj_from(
                scene.eye,
                yaw,
                pitch,
                0.0,
                crate::camera::DEFAULT_FOV_DEG,
                size.0 / size.1,
            );
            let up = crate::camera::up_hint(yaw, pitch, 0.0);
            let fwd = crate::camera::basis(yaw, pitch).0;
            let right = fwd.cross(up).normalize();
            let view = crate::cod2_world_hud::WorldView {
                view_proj,
                right,
                up: right.cross(fwd),
                size,
            };
            let quads = hud.world_quads(&s, &view, scene.render_time);
            // Backed off: the icon over the trigger is ahead, on screen.
            if backed(walked) {
                drawn = quads
                    .into_iter()
                    .find(|q| q.texture == icon)
                    .map(|q| q.verts);
            }
            clk_sleep(Duration::from_millis(16));
        }
        s.disconnect();
        e.disconnect();
        let missing: Vec<String> = sounds.missing.iter().cloned().collect();
        (perks, laid, ours, theirs, drawn, missing, shown)
    });
    ready_rx.recv().unwrap();
    serve(&mut sv, &sock, &clock, &client, |sv, _, _| {
        if let Ok(slot) = rx.try_recv() {
            let l = sv.game.level.as_mut().unwrap();
            l.start_player_script(
                slot,
                "maps/mp/gametypes/_weapons",
                "claymoreDetectionTrigger",
                &["axis"],
            )
            .unwrap();
        }
        true
    });
    let (perks, laid, ours, theirs, drawn, missing, shown) = client.join().unwrap();
    assert_eq!(shown, [true; 2], "the spawn's perk icons");
    println!("perks {perks:#x} ours {ours:?} theirs {theirs:?} drawn {drawn:?}");
    assert!(laid, "the two never spawned");
    // `specialty_quieter`, `specialty_gpsjammer`, `specialty_detectexplosive`.
    assert_eq!(perks & 0x901, 0x901, "{perks:#x}");
    let steps = |h: &HashSet<String>, quiet: bool| {
        h.iter()
            .filter(|a| a.contains("step_"))
            .all(|a| a.starts_with('q') == quiet)
            && h.iter().any(|a| a.contains("step_") && a.contains("_plr_"))
    };
    assert!(steps(&ours, true), "{ours:?}");
    assert!(steps(&theirs, false), "{theirs:?}");
    assert!(!missing.iter().any(|m| m.starts_with('q')), "{missing:?}");
    let v = drawn.expect("the bomb squad waypoint never drew");
    let centre = [(v[0][0] + v[2][0]) / 2.0, (v[0][1] + v[2][1]) / 2.0];
    assert!(
        (0.0..1280.0).contains(&centre[0]) && (0.0..960.0).contains(&centre[1]),
        "{v:?}"
    );
}

/// `+scores` in a live session: the board draws over the HUD while
/// held (asking for `score`), and goes once let go and faded.
#[test]
fn cod4_live_scores_toggle_the_board() {
    let Some(out) = live_join(|s, spawned, t0| {
        spawned.as_ref()?;
        let install = vcod_cod4::testing::cod4_install()?;
        let mut hud = crate::cod4_hud::Cod4Hud::default();
        hud.load(&install.root().join("main"), "mp_crash");
        let shell = crate::console::shell::Shell::new();
        let size = (1280.0, 960.0);
        // A frame on the session's clock; the HUD's quads on it with
        // the board up and down are counted on one frame, as a loaded
        // machine's frames change the HUD under the toggle.
        let frame = |hud: &mut crate::cod4_hud::Cod4Hud, s: &mut Cod4Session| {
            s.step(since(t0).as_secs_f64() * 1000.0).unwrap();
            let fired = s.take_fired();
            let now = s.scene().map_or(0, |x| x.render_time);
            hud.update(s, &fired, now);
            now
        };
        let count = |hud: &crate::cod4_hud::Cod4Hud, s: &Cod4Session, now: i32| {
            hud.build(s, size, now, None, &shell).len()
        };
        let now = frame(&mut hud, s);
        let before = count(&hud, s, now);
        let asked = hud.set_scores(true, now);
        let held = count(&hud, s, now);
        let now = frame(&mut hud, s);
        hud.set_scores(false, now);
        // The 100 ms fade on the session's clock, which a starved
        // server holds at the newest snapshot: frames until it passes.
        let wait = clk_now();
        let mut at = frame(&mut hud, s);
        while at < now + 250 && since(wait) < Duration::from_secs(20) {
            clk_sleep(Duration::from_millis(16));
            at = frame(&mut hud, s);
        }
        let after = count(&hud, s, at);
        // The same frame with the board held again.
        hud.set_scores(true, at);
        let again = count(&hud, s, at);
        Some((asked, before, held, after, again))
    }) else {
        return;
    };
    let (asked, before, held, after, again) = out.expect("no spawn");
    assert!(asked, "the press asks for `score`");
    assert!(held > before + 4, "{before} quads, {held} with the board");
    assert!(after < again, "{after} quads once let go, {again} held");
}

/// vcod-server's `war` with twenty bots and us on allies, the board
/// held: past one screen of rows it opens at the top with the column
/// headers on the first banner's line and the scroll bar's down
/// arrow, a wheel notch moves it `cg_scoreboardScrollStep` lines down
/// (headers on their own line, the up arrow) and a notch up brings it
/// back (`CG_DrawScoreboard_ScoresList`, `Scoreboard_HandleInput`,
/// cod4-hud.md 7.1). `cg_scoreboardPingGraph 1` typed at the console
/// adds each row's bars.
#[test]
fn cod4_live_a_long_board_scrolls_by_the_wheel() {
    use crate::cod4_scoreboard::Piece;
    use vcod_server::cod4::{Cod4Config, Cod4Server};
    let Some(install) = vcod_cod4::testing::cod4_install() else {
        return;
    };
    let main = install.root().join("main");
    let _live = live_slot();
    let mut sv = Cod4Server::new(
        Cod4Config {
            map: "mp_crash".into(),
            gametype: "war".into(),
            hostname: "vcod".into(),
            max_clients: 24,
        },
        7,
    );
    sv.set_cvar("scr_game_playerwaittime", "1");
    sv.set_cvar("scr_game_matchstarttime", "1");
    sv.load(Some(&install)).unwrap();
    sv.add_bots(20, false);
    let sock = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
    sock.set_nonblocking(true).unwrap();
    let addr = sock.local_addr().unwrap();
    let clock = sim::Clock::new();
    let client = sim::spawn(&clock, move || {
        let join = MenuJoin {
            team: Some("allies".into()),
            class: Some("assault_mp,0".into()),
        };
        let mut s = Cod4Session::connect(addr, "joiner", Cp::Bare, join).unwrap();
        let mut hud = crate::cod4_hud::Cod4Hud::default();
        hud.load(&main, "mp_crash");
        let mut shell = crate::console::shell::Shell::new();
        shell.register_cod4_options();
        let size = (1280.0, 960.0);
        let t0 = clk_now();
        let mut held = false;
        let mut out = None;
        // (pieces at the top, scrolled, back up, with the ping graph)
        while since(t0) < Duration::from_secs(60) && out.is_none() {
            s.step(since(t0).as_secs_f64() * 1000.0).unwrap();
            let fired = s.take_fired();
            clk_sleep(Duration::from_millis(16));
            let Some(now) = s.scene().map(|x| x.render_time) else {
                continue;
            };
            hud.update(&mut s, &fired, now);
            if s.spawn().is_none() {
                continue;
            }
            if !held || hud.score_request(now) {
                held = true;
                if hud.set_scores(true, now) {
                    s.command("score");
                }
            }
            if hud.scores.as_ref().is_none_or(|sc| sc.rows.len() < 21) {
                continue;
            }
            let board = |hud: &crate::cod4_hud::Cod4Hud,
                         s: &Cod4Session,
                         sh: &crate::console::shell::Shell| {
                hud.build(s, size, now, None, sh);
                hud.board_pieces(s, sh)
            };
            let top = board(&hud, &s, &shell);
            assert!(hud.scoreboard_key("MWHEELDOWN", 3));
            let down = board(&hud, &s, &shell);
            assert!(hud.scoreboard_key("MWHEELUP", 3));
            let up = board(&hud, &s, &shell);
            // Typed bare, as on 1.7, where the cgame registers it: an
            // unregistered name went to the server as a command.
            let typed = shell.execute("cg_scoreboardPingGraph 1");
            assert!(
                !typed
                    .iter()
                    .any(|e| matches!(e, crate::console::shell::Effect::Unknown { .. })),
                "{typed:?}"
            );
            let graph = board(&hud, &s, &shell);
            out = Some((top, down, up, graph));
        }
        s.disconnect();
        out
    });
    serve(&mut sv, &sock, &clock, &client, |_, _, _| true);
    let (top, down, up, graph) = client.join().unwrap().expect("no long board");
    let pic = |b: &[Piece], m: &str| {
        b.iter()
            .any(|p| matches!(p, Piece::Pic { material, .. } if material == m))
    };
    let text_y = |b: &[Piece], t: &str| {
        b.iter().find_map(|p| match p {
            Piece::Text { text, y, .. } if text == t => Some(*y),
            _ => None,
        })
    };
    // At the top the headers share the first banner's baseline.
    let banner = text_y(&top, "Marines").expect("our banner");
    assert_eq!(text_y(&top, "Score"), Some(banner), "{top:?}");
    assert!(pic(&top, "hudscoreboardscroll_downarrow"), "{top:?}");
    assert!(!pic(&top, "hudscoreboardscroll_uparrow"));
    // Three lines down: our banner scrolled off, the headers alone.
    assert!(text_y(&down, "Marines").is_none(), "{down:?}");
    assert!(text_y(&down, "Score").is_some());
    assert!(pic(&down, "hudscoreboardscroll_uparrow"), "{down:?}");
    assert_eq!(up, top, "the notch up is the top again");
    // Each drawn row gets the graph's backing and at least one bar.
    let whites = |b: &[Piece]| {
        b.iter()
            .filter(|p| matches!(p, Piece::Pic { material, .. } if material == "white"))
            .count()
    };
    assert!(
        whites(&graph) >= whites(&top) * 3,
        "{} {}",
        whites(&top),
        whites(&graph)
    );
}
