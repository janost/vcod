//! Live play: stance, cheat dvars, mounted guns, grenades thrown back,
//! recoil and landing.

use vcod_cod4::net::conn::angle2short;

use super::*;

/// A joined client crouched with `gocrouch`, then sprinting: the
/// server's forced stand (`EV_STANCE_FORCE_STAND`) becomes the
/// stance keys' state, so the body stays up after the sprint instead
/// of crouching again a cmd later (cod4-pmove.md 8).
#[test]
fn cod4_live_sprint_out_of_a_crouch_stays_standing() {
    use crate::console::shell::Effect;
    use crate::play::cod4_input::Stance;
    use crate::play::input::Action;
    use vcod_cod4::pmove::pmf;
    let sets = [
        ("scr_game_playerwaittime", "1"),
        ("scr_game_matchstarttime", "1"),
    ];
    let Some(r) = live_join_on("mp_crash", crash_world(), &sets, |s, spawned, t0, _| {
        spawned.as_ref()?;
        let step = |s: &mut Cod4Session, ms: u64| {
            let end = since(t0) + Duration::from_millis(ms);
            while since(t0) < end {
                s.step(since(t0).as_secs_f64() * 1000.0).unwrap();
                s.scene();
                s.take_commands();
                s.take_fired();
                clk_sleep(Duration::from_millis(16));
            }
        };
        let flags = |s: &Cod4Session| {
            let f = s.client().snapshots.last();
            f.and_then(|f| f.ps.get("pm_flags")).unwrap_or(0)
        };
        step(s, 3000);
        s.input.effect(&Effect::Impulse(Action::Crouch), &[]);
        step(s, 800);
        let crouched = (s.input.keys.stance, flags(s) & pmf::DUCKED != 0);
        s.input.key(Action::Forward, true);
        s.input.key(Action::Sprint, true);
        let mut sprinted = false;
        for _ in 0..10 {
            step(s, 100);
            sprinted |= flags(s) & pmf::SPRINTING != 0;
        }
        s.input.key(Action::Sprint, false);
        s.input.key(Action::Forward, false);
        step(s, 1000);
        Some((crouched, sprinted, s.input.keys.stance, flags(s)))
    }) else {
        return;
    };
    let (crouched, sprinted, stance, after) = r.expect("never spawned");
    assert_eq!(crouched, (Stance::Crouch, true));
    assert!(sprinted, "never sprinted");
    assert_eq!(stance, Stance::Stand);
    assert_eq!(after & (pmf::DUCKED | pmf::PRONE), 0, "pm_flags {after:#x}");
}

/// The client's cheat dvars follow the server's `sv_cheats` through
/// the systeminfo, as `CL_SystemInfoChanged` (iw3mp.exe 0x473ab0)
/// sets it: set at the server's start, `fx_draw` and `r_glow` take a
/// 0; an rcon-style `sv_cheats 0` on the server's console reaches the
/// client as `d 1`, locks them and puts them back; `sv_cheats 1`
/// unlocks them again (cod4-client-perf.md 20).
#[test]
fn cod4_live_cheat_dvars_follow_the_servers_sv_cheats() {
    use crate::console::shell::{Effect, Shell};
    let sets = [
        ("sv_cheats", "1"),
        ("scr_game_playerwaittime", "1"),
        ("scr_game_matchstarttime", "1"),
    ];
    let Some(r) = live_join_on("mp_crash", None, &sets, |s, _, t0, ctl| {
        let mut shell = Shell::new();
        shell.register_cod4_options();
        // steps until the systeminfo's sv_cheats reads `want`
        let follow = |s: &mut Cod4Session, shell: &mut Shell, want: &str| {
            let end = since(t0) + Duration::from_secs(5);
            while since(t0) < end {
                s.step(since(t0).as_secs_f64() * 1000.0).unwrap();
                s.take_commands();
                if let Some(si) = s.take_systeminfo() {
                    shell.apply_systeminfo(&si);
                }
                if shell.cvar("sv_cheats") == Some(want) {
                    return true;
                }
                clk_sleep(Duration::from_millis(16));
            }
            false
        };
        let mut out = Vec::new();
        out.push(follow(s, &mut shell, "1"));
        shell.execute("fx_draw 0");
        shell.execute("r_glow 0");
        out.push(shell.cvar("fx_draw") == Some("0") && shell.cvar("r_glow") == Some("0"));
        ctl.send(Ctl::Console("sv_cheats 0".into())).unwrap();
        out.push(follow(s, &mut shell, "0"));
        out.push(shell.cvar("fx_draw") == Some("1") && shell.cvar("r_glow") == Some("1"));
        out.push(
            shell.execute("fx_draw 0") == [Effect::Print("fx_draw is cheat protected.".into())],
        );
        ctl.send(Ctl::Console("sv_cheats 1".into())).unwrap();
        out.push(follow(s, &mut shell, "1"));
        shell.execute("fx_draw 0");
        out.push(shell.cvar("fx_draw") == Some("0"));
        out
    }) else {
        return;
    };
    assert_eq!(
        r, [true; 7],
        "start, set at start, rcon 0, reset, locked, rcon 1, set again"
    );
}

/// mp_backlot's crouch SAW (entity 393, yaw 0, arcs 45 and -15..30)
/// through a joined client, as the retail capture plays it
/// (cod4-gsc.md 9.2): behind it the hint names it, a use press mans it
/// (`eFlags` 0x200, the view locked to it), the held trigger fires a
/// round a frame off the gun (`EV_FIRE_WEAPON_MG42`, its firing bit,
/// `viewlocked` 2), a look past its arc leaves the barrel on the arc,
/// and a second press lets go (`EV_STANCE_FORCE_STAND`, the lock off).
#[test]
fn cod4_live_saw_mounts_fires_clamps_and_lets_go() {
    use crate::play::input::Action;
    const GUN: u32 = 393;
    let sets = [
        ("sv_cheats", "1"),
        ("scr_game_playerwaittime", "1"),
        ("scr_game_matchstarttime", "1"),
    ];
    let Some(r) = live_join_on("mp_backlot", None, &sets, |s, spawned, t0, _| {
        spawned.as_ref()?;
        let step = |s: &mut Cod4Session, ms: u64| {
            let end = since(t0) + Duration::from_millis(ms);
            while since(t0) < end {
                s.step(since(t0).as_secs_f64() * 1000.0).unwrap();
                s.take_commands();
                clk_sleep(Duration::from_millis(16));
            }
        };
        let ps = |s: &Cod4Session, n: &str| {
            s.client()
                .snapshots
                .last()
                .and_then(|f| f.ps.get(n))
                .unwrap_or(0)
        };
        let gun = |s: &Cod4Session| {
            let f = s.client().snapshots.last()?;
            f.entities.iter().find(|e| e.number() == GUN).cloned()
        };
        step(s, 2500);
        s.command("setviewpos -770 68 320 0");
        s.view = [0.0, 0.0];
        step(s, 1500);
        let hint = (ps(s, "cursorHint"), ps(s, "cursorHintEntIndex"));
        let weapon = gun(s).map(|e| ent_u32(&e, "weapon"));
        s.input.key(Action::Use, true);
        step(s, 200);
        s.input.key(Action::Use, false);
        step(s, 300);
        let mounted = (
            ps(s, "eFlags") & 0x300,
            ps(s, "viewlocked"),
            ps(s, "viewlocked_entNum"),
        );
        // The gunner's crosshair is the gun's own reticle.
        let reticle = vcod_cod4::testing::cod4_install().and_then(|install| {
            let mut hud = crate::cod4_hud::Cod4Hud::default();
            hud.load(&install.root().join("main"), "mp_backlot");
            let v = s.scene()?.view;
            let shell = crate::console::shell::Shell::new();
            let q = hud.crosshair(s, &v, [0.75, 0.5], (1280.0, 960.0), &shell);
            let name = weapon_list(s.gamestate()?)
                .get(weapon? as usize - 1)?
                .clone();
            Some((q, hud.weapon_hud(&name)?.reticle.center_size))
        });
        let origin =
            |s: &Cod4Session| [0, 1, 2].map(|k| f32::from_bits(ps(s, &format!("origin[{k}]"))));
        let barrel = |s: &Cod4Session| {
            gun(s).map_or(f32::NAN, |e| {
                f32::from_bits(ent_u32(&e, "lerp.u.anonymous.data[1]"))
            })
        };
        let mut placed = vec![(barrel(s), origin(s))];
        let seq0 = gun(s).map_or(0, |e| ent_u32(&e, "eventSequence"));
        s.input.key(Action::Attack, true);
        let mut jitter = false;
        let mut firing = false;
        for _ in 0..20 {
            step(s, 50);
            jitter |= ps(s, "viewlocked") == 2;
            firing |= gun(s).is_some_and(|e| ent_u32(&e, "lerp.eFlags") & 0x40 != 0);
        }
        s.input.key(Action::Attack, false);
        let fired = gun(s).map(|e| {
            let seq = ent_u32(&e, "eventSequence");
            let last = ent_u32(&e, &format!("events[{}]", (seq.wrapping_sub(1)) & 3));
            (seq.wrapping_sub(seq0) & 0xff, last)
        });
        s.view = [0.0, 80.0];
        step(s, 800);
        let yaw = gun(s).map(|e| f32::from_bits(ent_u32(&e, "lerp.u.anonymous.data[1]")));
        placed.push((barrel(s), origin(s)));
        s.view = [0.0, -80.0];
        step(s, 800);
        placed.push((barrel(s), origin(s)));
        s.input.key(Action::Use, true);
        step(s, 200);
        s.input.key(Action::Use, false);
        step(s, 500);
        let released = (
            ps(s, "eFlags") & 0x300,
            ps(s, "viewlocked"),
            ps(s, "viewlocked_entNum"),
        );
        Some((
            hint, weapon, mounted, jitter, firing, fired, yaw, released, placed, reticle,
        ))
    }) else {
        return;
    };
    let (hint, weapon, mounted, jitter, firing, fired, yaw, released, placed, reticle) =
        r.expect("never spawned");
    // `CG_DrawTurretCrossHair`: one centred quad, the gun's
    // `reticleCenterSize` on the 2 px a unit of 1280 by 960.
    let (quads, size) = reticle.expect("no crosshair read");
    assert_eq!(quads.len(), 1);
    let xs = quads[0].verts.map(|v| v[0]);
    let (lo, hi) = (
        xs.iter().copied().fold(f32::MAX, f32::min),
        xs.iter().copied().fold(f32::MIN, f32::max),
    );
    assert!(
        (lo + hi - 1280.0).abs() < 0.01 && (hi - lo - 2.0 * size as f32).abs() < 0.01,
        "{:?} {size}",
        quads[0].verts
    );
    let w = weapon.expect("no gun 393 in the snapshot");
    assert_eq!(hint, (w + 4, GUN), "hint");
    assert_eq!(mounted, (0x200, 1, GUN), "mount");
    assert!(
        jitter && firing,
        "no round: jitter {jitter} firing {firing}"
    );
    let (rounds, last) = fired.unwrap();
    assert!(rounds >= 10, "{rounds} rounds in a second");
    assert_eq!(last, 38, "the gun's last event");
    assert_eq!(yaw, Some(45.0), "the barrel past its arc");
    assert_eq!(released, (0, 0, 1023), "release");
    // `G_PlayerTurretPositionAndBlend`: retail's gunner at barrel yaw 0
    // and 45 (`play-1.7-saw-gunner.txt`, cod4-gsc.md 9.2).
    let retail = [
        (0.0, [-754.039, 75.398, 258.125]),
        (45.0, [-752.46, 51.005, 258.125]),
        (-45.0, [-738.634, 99.883, 258.125]),
    ];
    let mut seen = Vec::new();
    for (yaw, at) in placed {
        let Some((_, want)) = retail.iter().find(|(y, _)| (y - yaw).abs() < 0.02) else {
            continue;
        };
        let near = at.iter().zip(want).all(|(x, y)| (x - y).abs() < 0.02);
        assert!(near, "barrel {yaw}: placed {at:?}, retail {want:?}");
        seen.push(yaw.round());
    }
    assert!(seen.contains(&45.0) && seen.contains(&-45.0), "{seen:?}");
}

/// Another player on a gun, through a joined client: the server still
/// sends his held weapon, and his drawn body leaves it out
/// (`BG_UpdatePlayerDObj`, cod4-player-anims.md 5), so the gun is the
/// only one in his hands. Off the gun the same body holds it.
#[test]
fn cod4_live_a_gunner_holds_no_weapon_of_his_own() {
    const GUN: u32 = 393;
    let sets = [
        ("scr_game_playerwaittime", "1"),
        ("scr_game_matchstarttime", "1"),
    ];
    let Some(r) = live_join_on("mp_backlot", None, &sets, |s, spawned, t0, ctl| {
        spawned.as_ref()?;
        // (sent weapon, drawn weapon) of each player body before and
        // on the gun.
        let mut off = None;
        let mut on = None;
        ctl.send(Ctl::MountBot([-770.0, 68.0, 320.0], GUN)).ok()?;
        let end = since(t0) + Duration::from_secs(20);
        while since(t0) < end && on.is_none() {
            s.step(since(t0).as_secs_f64() * 1000.0).unwrap();
            s.take_commands();
            let snap = s.client().snapshots.last().cloned();
            if let (Some(scene), Some(snap)) = (s.scene(), snap) {
                for d in &scene.entities {
                    let Some(b) = d.body.as_ref().filter(|b| !b.corpse) else {
                        continue;
                    };
                    let Some(e) = snap.entities.iter().find(|e| e.number() == d.number) else {
                        continue;
                    };
                    let seen = (ent_u32(e, "weapon"), b.weapon.clone());
                    if b.gun.as_ref().is_some_and(|g| g.0 == GUN) {
                        on = Some(seen);
                    } else if ent_u32(e, "lerp.eFlags") & 0x300 == 0 && seen.0 != 0 {
                        off = Some(seen);
                    }
                }
            }
            clk_sleep(Duration::from_millis(16));
        }
        Some((off, on))
    }) else {
        return;
    };
    let (off, on) = r.expect("never spawned");
    let (sent, drawn) = off.expect("no armed body off the gun");
    assert!(drawn.is_some(), "off the gun: weapon {sent} not drawn");
    let (sent, drawn) = on.expect("the bot never manned gun 393");
    assert_ne!(sent, 0, "the gunner's entity sends no weapon");
    assert_eq!(drawn, None, "the gunner draws his own weapon {sent}");
}

/// A live frag at our feet through a joined client (cod4-gsc.md 9.3):
/// the hint names it with its fuse's time left, the frag key takes it
/// (`EV_SWITCH_OFFHAND`, then `EV_AMMO_PICKUP`, the owner named, the
/// hand's `grenadeTimeLeft` the fuse's), and the throw puts a new
/// grenade in the world while the frag count stays.
#[test]
fn cod4_live_frag_at_the_feet_is_thrown_back() {
    use crate::play::input::Action;
    let sets = [
        ("scr_game_playerwaittime", "1"),
        ("scr_game_matchstarttime", "1"),
    ];
    let Some(r) = live_join_on("mp_crash", crash_world(), &sets, |s, spawned, t0, tx| {
        spawned.as_ref()?;
        let step = |s: &mut Cod4Session, ms: u64| {
            let end = since(t0) + Duration::from_millis(ms);
            while since(t0) < end {
                s.step(since(t0).as_secs_f64() * 1000.0).unwrap();
                s.take_commands();
                clk_sleep(Duration::from_millis(16));
            }
        };
        let ps = |s: &Cod4Session, n: &str| {
            s.client()
                .snapshots
                .last()
                .and_then(|f| f.ps.get(n))
                .unwrap_or(0)
        };
        // Past the prematch freeze.
        step(s, 3000);
        let f = |n: &str| ps_f32(&s.client().snapshots.last().unwrap().ps, n);
        let at = [f("origin[0]") + 24.0, f("origin[1]"), f("origin[2]") + 2.0];
        let frags = |s: &Cod4Session| {
            s.client()
                .snapshots
                .last()
                .unwrap()
                .ps
                .ammoclip
                .iter()
                .sum::<i32>()
        };
        let before = frags(s);
        s.view = [60.0, 0.0];
        tx.send(Ctl::DropFrag(at, 3000)).unwrap();
        step(s, 300);
        let hint = (ps(s, "cursorHint"), ps(s, "throwBackGrenadeTimeLeft"));
        let mut seen = Vec::new();
        let mut owner = 1023;
        let mut hand = 0;
        s.input.key(Action::Frag, true);
        for _ in 0..12 {
            step(s, 50);
            let p = &s.client().snapshots.last().unwrap().ps;
            for k in 0..4 {
                seen.push(p.get(&format!("events[{k}]")).unwrap_or(0));
            }
            if owner == 1023 {
                owner = ps(s, "throwBackGrenadeOwner");
                hand = ps(s, "grenadeTimeLeft");
            }
        }
        s.input.key(Action::Frag, false);
        let mut thrown = false;
        for _ in 0..40 {
            step(s, 50);
            thrown |= s
                .client()
                .snapshots
                .last()
                .unwrap()
                .entities
                .iter()
                .any(|e| e.etype() == ET_MISSILE);
        }
        Some((hint, seen, owner, hand, thrown, before, frags(s)))
    }) else {
        return;
    };
    let (hint, seen, owner, hand, thrown, before, after) = r.expect("never spawned");
    assert!(
        hint.0 > 4 && hint.1 > 2000 && hint.1 <= 3000,
        "hint {hint:?}"
    );
    assert!(seen.contains(&34) && seen.contains(&10), "events {seen:?}");
    assert_eq!(owner, 0, "the thrower named");
    assert!(hand > 0 && hand <= 3000, "the hand's fuse {hand}");
    assert!(thrown, "nothing thrown back");
    assert_eq!(before, after, "a throw back spends no frag");
}

/// A live frag at our feet through a joined client (cod4-hud.md 20):
/// the server's hint pass puts it under `cursorHint`, and the HUD's
/// `cursorhints` item draws `PLATFORM_THROWBACKGRENADE` with the frag
/// key in place of its `&&1` and the frag's `hudIcon` to its right,
/// over the screen's centre, the grenade indicator's throw back icon
/// over the crosshair; once the blast has killed us no hint draws.
#[test]
fn cod4_live_frag_at_our_feet_draws_the_throw_back_hint() {
    const WIDE: (f32, f32) = (2560.0, 1440.0);
    let sets = [
        ("scr_game_playerwaittime", "1"),
        ("scr_game_matchstarttime", "1"),
    ];
    let Some(r) = live_join_on("mp_crash", crash_world(), &sets, |s, spawned, t0, tx| {
        spawned.as_ref()?;
        let install = vcod_cod4::testing::cod4_install()?;
        let mut hud = crate::cod4_hud::Cod4Hud::default();
        hud.load(&install.root().join("main"), "mp_crash");
        let mut shell = crate::console::shell::Shell::new();
        shell.execute("bind G +frag");
        let size = (1280.0, 960.0);
        let frame = |hud: &mut crate::cod4_hud::Cod4Hud, s: &mut Cod4Session| {
            s.step(since(t0).as_secs_f64() * 1000.0).unwrap();
            let fired = s.take_fired();
            let now = s.scene().map_or(0, |x| x.render_time);
            hud.update(s, &fired, now);
            clk_sleep(Duration::from_millis(16));
            now
        };
        let end = since(t0) + Duration::from_millis(3000);
        while since(t0) < end {
            frame(&mut hud, s);
        }
        let f = |n: &str| ps_f32(&s.client().snapshots.last().unwrap().ps, n);
        let at = [f("origin[0]") + 24.0, f("origin[1]"), f("origin[2]") + 2.0];
        s.view = [60.0, 0.0];
        tx.send(Ctl::DropFrag(at, 2000)).unwrap();
        let (mut drawn, mut quads, mut gone) = (None, 0, false);
        let mut wide = Vec::new();
        let mut indicators = Vec::new();
        let end = since(t0) + Duration::from_millis(3500);
        while since(t0) < end {
            let now = frame(&mut hud, s);
            let d = hud.cursor_hint_now(s, &shell, size, now);
            if let Some(sc) = s.scene() {
                let g = hud.grenade_indicators(s, &sc.view, &|| f32::MAX, size, now);
                if indicators.is_empty() {
                    indicators = g.into_iter().map(|q| (q.verts, q.texture)).collect();
                }
            }
            gone = drawn.is_some() && d.is_none();
            if drawn.is_none() && d.is_some() {
                quads = hud.build(s, size, now, None, &shell).len();
                wide = hud.build(s, WIDE, now, None, &shell);
                drawn = d;
            }
        }
        let strings = hud.strings()?;
        let frag_icon = hud.weapon_icon("frag_grenade_mp");
        let key = shell.key_text("+frag", &strings);
        let throwback = hud.texture_of("hud_grenadethrowback");
        let icon_tex = frag_icon.as_deref().and_then(|m| hud.texture_of(m));
        Some((
            drawn,
            quads,
            gone,
            strings,
            frag_icon,
            key,
            indicators,
            throwback,
            (wide, icon_tex),
        ))
    }) else {
        return;
    };
    let (drawn, quads, gone, strings, frag_icon, key, indicators, throwback, (wide, icon_tex)) =
        r.expect("never spawned");
    // At 1440p the hint's font is not the 640 grid's: the text still
    // ends where its icon starts, as retail measures it with the font
    // it draws (cod4-hud.md 20). The last glyph's image and shadow
    // reach about 3 px past its advance; the grid's width ran 5.5.
    let icon_tex = icon_tex.expect("the frag icon's image");
    let icon = wide
        .iter()
        .filter(|q| q.texture == icon_tex)
        // The hint's, near the middle (the weapon HUD shows the frag too).
        .min_by_key(|q| ((q.verts[0][0] + q.verts[1][0]) - WIDE.0).abs() as i32)
        .expect("the hint's icon at 1080p");
    let (top, bottom) = (icon.verts[0][1], icon.verts[2][1]);
    let text_end = wide
        .iter()
        .filter(|q| q.texture != icon_tex)
        .filter(|q| {
            let mid = (q.verts[0][1] + q.verts[2][1]) * 0.5;
            mid > top && mid < bottom && q.verts[1][0] < icon.verts[1][0]
        })
        .map(|q| q.verts[1][0])
        .fold(f32::MIN, f32::max);
    assert!(text_end > f32::MIN, "no hint text beside the icon");
    assert!(
        text_end <= icon.verts[0][0] + 4.0,
        "text ends at {text_end}, icon starts at {}",
        icon.verts[0][0]
    );
    // The grenade indicator (cod4-hud.md 21): the pointer, then the
    // throw back icon, the frag ahead of a view at yaw 0, so above the
    // centre of the 1280x960 window by `IconOffset` 50 grid units.
    let [(_, _), (icon, tex)] = &indicators[..] else {
        panic!("indicators {indicators:?}");
    };
    assert_eq!(Some(tex.clone()), throwback);
    let mid = |k: usize| icon.iter().map(|v| v[k]).sum::<f32>() / 4.0;
    assert!((mid(0) - 640.0).abs() < 2.0, "{icon:?}");
    assert!((mid(1) - (480.0 - 100.0)).abs() < 2.0, "{icon:?}");
    let (d, alpha) = drawn.expect("no hint drawn");
    assert_eq!(alpha, 1.0);
    let key = key.expect("+frag bound");
    let want = strings
        .get("PLATFORM_THROWBACKGRENADE")
        .expect("localized")
        .replacen("&&1", &key, 1);
    let [(text, x, y)] = &d.texts[..] else {
        panic!("{d:?}");
    };
    assert_eq!(text, &want);
    // Centred on the item's middle with the icon after the text, a
    // 40-unit square on the item's row.
    let (material, rect) = d.icon.clone().expect("an icon");
    assert_eq!(Some(material), frag_icon);
    assert!(
        (x + rect[2] * 0.5 + (rect[0] - x) * 0.5).abs() < 0.01,
        "{d:?}"
    );
    assert!(*x < 0.0 && rect[0] > *x && *y > 60.0, "{d:?}");
    assert_eq!((rect[1], rect[2], rect[3]), (40.0, 40.0, 40.0));
    assert!(quads > text.len() / 2, "quads {quads}");
    assert!(gone, "the hint outlived the frag");
}

/// One frame of a live kick test: the step, the scene (which steps
/// the kicks), the fired events dropped; the scene.
fn kick_frame(s: &mut Cod4Session, t0: Instant) -> Option<Scene> {
    s.step(since(t0).as_secs_f64() * 1000.0).unwrap();
    s.take_commands();
    let scene = s.scene();
    s.take_fired();
    clk_sleep(Duration::from_millis(16));
    scene
}

/// A trigger pull through a joined client on vcod-server
/// (cod4-client-view.md 14): the M16's burst kicks the view
/// (`CG_KickAngles`) and the kick rides the cmds' angles, so the
/// server's view of each cmd is the client's drawn one, kick and all,
/// roll too (1.7's cmd loop converts all three angles, 0x463aa9), and
/// both come back to the mouse's view once the kick has centred.
#[test]
fn cod4_live_recoil_rides_the_cmd_and_the_server_takes_it() {
    use crate::play::input::Action;
    let sets = [
        ("scr_game_playerwaittime", "1"),
        ("scr_game_matchstarttime", "1"),
    ];
    let Some(r) = live_join_on("mp_crash", crash_world(), &sets, |s, spawned, t0, tx| {
        spawned.as_ref()?;
        let end = since(t0) + Duration::from_millis(3000);
        while since(t0) < end {
            kick_frame(s, t0);
        }
        s.view = [0.0, 0.0];
        // How far a view (pitch, yaw) is off the mouse's 0, 0.
        let off = |a: [f32; 3]| {
            use vcod_common::pmove::angle_normalize180;
            angle_normalize180(a[0])
                .abs()
                .max(angle_normalize180(a[1]).abs())
        };
        let (ptx, prx) = std::sync::mpsc::channel();
        let (mut kick, mut drawn, mut server) = (0.0f32, 0.0f32, 0.0f32);
        let (mut kick_roll, mut server_roll) = (0.0f32, 0.0f32);
        let (mut checked, mut worst) = (0, 0.0f32);
        let start = since(t0);
        while since(t0) < start + Duration::from_millis(2500) {
            let held = since(t0) - start < Duration::from_millis(60);
            s.input.key(Action::Attack, held);
            let scene = kick_frame(s, t0)?;
            kick = kick.max(off(s.kick.view_kick()));
            kick_roll = kick_roll.max(s.kick.view_kick()[2].abs());
            drawn = drawn.max(off(scene.angles.to_array()));
            tx.send(Ctl::Probe(ptx.clone())).unwrap();
            let ps = &s.client().snapshots.last()?.ps;
            let delta: [u16; 3] =
                std::array::from_fn(|k| angle2short(ps_f32(ps, &format!("delta_angles[{k}]"))));
            while let Ok((t, va)) = prx.try_recv() {
                server = server.max(off(va));
                server_roll = server_roll.max(va[2].abs());
                let Some(c) = s.sent_cmds.iter().find(|c| c.server_time == t) else {
                    continue;
                };
                for k in 0..3 {
                    let mine =
                        f32::from(c.angles[k].wrapping_add(delta[k]) as i16) * (360.0 / 65536.0);
                    let miss = vcod_common::pmove::angle_normalize180(mine - va[k]);
                    worst = worst.max(miss.abs());
                }
                checked += 1;
            }
        }
        let rest = off(kick_frame(s, t0)?.angles.to_array());
        Some((
            kick,
            drawn,
            server,
            (kick_roll, server_roll),
            checked,
            worst,
            s.kick.view_kick(),
            rest,
        ))
    }) else {
        return;
    };
    let (kick, drawn, server, (kick_roll, server_roll), checked, worst, left, rest) =
        r.expect("never spawned");
    assert!(kick > 0.1, "kick {kick}");
    assert!(drawn > 0.1 && server > 0.1, "drawn {drawn} server {server}");
    assert!(
        kick_roll > 0.01 && server_roll > 0.01,
        "kick roll {kick_roll} server roll {server_roll}"
    );
    assert!(
        checked > 20 && worst < 0.01,
        "{checked} cmds, worst {worst}"
    );
    assert_eq!(left, [0.0; 3]);
    assert!(rest < 0.01, "the view {rest} off the mouse's");
}

/// A joined client's moves carry its predicted origin at `cl.serverTime`,
/// the time its cmds are stamped: the server finds it within 0.1 of the
/// body it ran at that `commandTime` and leaves origin and velocity out
/// of most snapshots, standing and walking, and the client puts its
/// archive row in their place (cod4-netfields.md 13). Before, the moves
/// carried 1e6 at time 0 and every snapshot sent them.
#[test]
fn cod4_live_a_predicted_walk_lets_the_server_leave_origin_out() {
    use crate::play::input::Action;
    let sets = [
        ("scr_game_playerwaittime", "1"),
        ("scr_game_matchstarttime", "1"),
    ];
    let Some(r) = live_join_on("mp_crash", crash_world(), &sets, |s, spawned, t0, _| {
        spawned.as_ref()?;
        // Each new snapshot: sent without origin, its speed, and its
        // origin off the archive row for its `commandTime`.
        let mut last = 0;
        let mut step = |s: &mut Cod4Session, ms: u64, out: &mut Vec<(bool, f32, bool)>| {
            let end = since(t0) + Duration::from_millis(ms);
            while since(t0) < end {
                s.step(since(t0).as_secs_f64() * 1000.0).unwrap();
                s.scene();
                s.take_commands();
                s.take_fired();
                let c = s.client();
                let from = last;
                for snap in c.snapshots.iter().filter(|x| x.server_time.raw() > from) {
                    let f = |n: &str| snap.ps.get_f32(n).unwrap_or(0.0);
                    let speed = glam::Vec2::new(f("velocity[0]"), f("velocity[1]")).length();
                    let row = c
                        .archive
                        .at_or_before(snap.ps.get("commandTime").unwrap_or(0) as i32);
                    let filled = row
                        .is_some_and(|r| r.origin == [0, 1, 2].map(|k| f(&format!("origin[{k}]"))));
                    out.push((!snap.ps.origin_sent, speed, filled));
                    last = snap.server_time.raw();
                }
                clk_sleep(Duration::from_millis(16));
            }
        };
        let mut warmup = Vec::new();
        step(s, 3000, &mut warmup);
        let mut still = Vec::new();
        step(s, 1500, &mut still);
        s.input.key(Action::Forward, true);
        let mut walk = Vec::new();
        step(s, 500, &mut walk);
        walk.clear();
        step(s, 2000, &mut walk);
        s.input.key(Action::Forward, false);
        Some((still, walk))
    }) else {
        return;
    };
    let (still, walk) = r.expect("never spawned");
    let left_out = |v: &[(bool, f32, bool)]| v.iter().filter(|x| x.0).count();
    assert!(
        left_out(&still) * 4 >= still.len() * 3,
        "standing: {} of {} without origin",
        left_out(&still),
        still.len()
    );
    let walking: Vec<_> = walk.iter().filter(|x| x.1 > 100.0).copied().collect();
    assert!(walking.len() >= 20, "{} walking snapshots", walking.len());
    assert!(
        left_out(&walking) * 2 >= walking.len(),
        "walking: {} of {} without origin",
        left_out(&walking),
        walking.len()
    );
    // A snapshot without origin reads its row's.
    assert!(still.iter().chain(&walk).filter(|x| x.0).all(|x| x.2));
}

/// A standing jump through a joined client: the landing's
/// `EV_LANDING_*` dips the eye by its parm over 150 ms and brings it
/// back by 450, and the view weapon's `ViewPs` carries the dip.
#[test]
fn cod4_live_landing_dips_the_eye_and_the_gun() {
    use crate::play::input::Action;
    let sets = [
        ("scr_game_playerwaittime", "1"),
        ("scr_game_matchstarttime", "1"),
    ];
    let Some(r) = live_join_on("mp_crash", crash_world(), &sets, |s, spawned, t0, _| {
        spawned.as_ref()?;
        let end = since(t0) + Duration::from_millis(3000);
        let mut rest = 0.0;
        while since(t0) < end {
            if let Some(sc) = kick_frame(s, t0) {
                rest = sc.eye.z;
            }
        }
        let (mut change, mut low, mut gun) = (0.0f32, rest, (0.0, 0));
        let start = since(t0);
        let end = start + Duration::from_millis(2000);
        let mut back = 0.0;
        while since(t0) < end {
            s.input
                .key(Action::Jump, since(t0) - start < Duration::from_millis(50));
            let sc = kick_frame(s, t0)?;
            let (c, t) = sc.view.land;
            if c < 0.0 && (0..450).contains(&t) {
                change = c;
                low = low.min(sc.eye.z);
                gun = sc.view.land;
            }
            back = sc.eye.z;
        }
        Some((rest, change, low, gun, back))
    }) else {
        return;
    };
    let (rest, change, low, gun, back) = r.expect("never spawned");
    assert!(change <= -1.0, "landChange {change}");
    assert!(
        low < rest + change * 0.5,
        "eye {low} from {rest}, change {change}"
    );
    assert!(
        crate::play::cod4_kick::gun_dip(gun) < 0.0 || gun.1 >= 150,
        "{gun:?}"
    );
    assert!((back - rest).abs() < 0.01, "eye {back} back from {rest}");
}
