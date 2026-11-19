//! The live view: waypoints, crosshairs, names over heads, camos and
//! attachment tags.

use super::*;
use crate::play::cod4::scene::model_cs;

/// [`live_war_pair_in`] under `offscreen_waypoint`'s probe gsc: a
/// client waypoint 2000 units behind us with an offscreen material
/// (`setWaypoint(true, "waypoint_targetneutral")`) draws in the 2D
/// pass, pushed to the screen's top edge padding with
/// `hud_offscreenobjectivepointer` beside it, and never as a world
/// sprite (cod4-hud.md 16.3).
#[test]
fn cod4_live_offscreen_waypoint_sits_on_the_edge_with_its_pointer() {
    const GSC: &str = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../cod4/tests/fixtures/gsc/offscreen_waypoint"
    );
    let out = live_war_pair_in(["assault_mp,0", "assault_mp,0"], Some(GSC), |f| {
        let icon = f
            .hud
            .texture_of("waypoint_targetneutral")
            .expect("the icon's image");
        let pointer = f
            .hud
            .texture_of(crate::cod4_waypoints::POINTER)
            .expect("the pointer's image");
        let (fwd, right, up) =
            crate::camera::basis(f.angles.y.to_radians(), -f.angles.x.to_radians());
        let size = [1280.0f32, 960.0];
        // `cg_fov` 65 on a 4:3 window.
        let tan_x = 32.5f32.to_radians().tan();
        let eye = crate::cod4_waypoints::Eye {
            origin: f.eye,
            forward: fwd,
            left: -right,
            up,
            tan: [tan_x, tan_x * 0.75],
            size,
        };
        let q = f.hud.offscreen_waypoints(f.s, &eye, f.level);
        let found = q.iter().any(|q| q.texture == icon);
        found.then(|| {
            (
                q.iter()
                    .map(|q| (q.texture.clone(), q.verts))
                    .collect::<Vec<_>>(),
                icon,
                pointer,
            )
        })
    });
    let Some(out) = out else {
        return;
    };
    let (quads, icon, pointer) = out.expect("the waypoint never drew in 120 s");
    println!("{quads:?}");
    assert_eq!(quads.len(), 2, "the pointer and the icon");
    assert_eq!(quads[0].0, pointer);
    assert_eq!(quads[1].0, icon);
    let centre = |v: [[f32; 2]; 4]| [(v[0][0] + v[2][0]) * 0.5, (v[0][1] + v[2][1]) * 0.5];
    let [x, y] = centre(quads[1].1);
    // Behind and above the eye: the projection (0x439320) sends it the
    // way its height off the eye points, to the top edge's padding
    // (6 + 30 of the 480 grid, 2 px a unit), the arrow over it; 0.9
    // of 36 square at 2000 units (`waypointDistScale*`).
    assert!(
        (x - 640.0).abs() < 40.0 && (y - 72.0).abs() < 1.0,
        "icon at ({x}, {y})"
    );
    assert!(centre(quads[0].1)[1] < y, "the pointer points up and out");
    let w = quads[1].1[1][0] - quads[1].1[0][0];
    assert!((w - 64.8).abs() < 0.1, "icon {w} wide");
}

/// [`live_war_pair_as`] as Spec Ops (MP5, Steady Aim in `ps.perks`'s
/// bit 1) and as Assault (M16, no Steady Aim): the crosshair draws its
/// centre and four sides off the live playerstate, the sides as far
/// out as `CG_CalcReticleSpread` puts them at the drawn fov, Spec
/// Ops's spread scaled by `perk_weapSpreadMultiplier` 0.65
/// (cod4-hud.md 17).
#[test]
fn cod4_live_crosshair_spreads_by_the_weapon_and_steady_aim() {
    // The crosshair quads, the weapon's def and the playerstate, once
    // we stand spawned with a weapon up.
    type Seen = (
        Vec<crate::hud::HudQuad>,
        vcod_cod4::hud::WeaponHud,
        crate::cod4_viewmodel::ViewPs,
    );
    let run = |class: &'static str| {
        live_war_pair_as([class, "assault_mp,0"], |f| {
            let v = f.view;
            if !f.both || f.level < 3000 || v.weaponstate != 0 || v.weapon == 0 {
                return None;
            }
            let gs = f.s.gamestate()?;
            let name = weapon_list(gs).get(v.weapon as usize - 1)?.clone();
            let def = f.hud.weapon_hud(&name)?.clone();
            let q = f
                .hud
                .crosshair(f.s, v, [TAN_Y * 4.0 / 3.0, TAN_Y], (1280.0, 960.0), f.shell);
            Some((q, def, *v))
        })
    };
    const TAN_Y: f32 = 0.5;
    for (class, steady) in [("specops_mp,0", true), ("assault_mp,0", false)] {
        let Some(out) = run(class) else {
            return;
        };
        let (quads, def, ps): Seen = out.expect("never spawned with a weapon up");
        assert_eq!(ps.perks & 2 != 0, steady, "{class}'s perks {:#x}", ps.perks);
        let r = &def.reticle;
        let drawn: Vec<_> = quads
            .iter()
            .map(|q| (&q.texture, q.verts[0], q.rgba))
            .collect();
        println!("{class}: {drawn:?} {r:?} ass {}", ps.aim_spread_scale);
        let centre = usize::from(r.center.is_some());
        assert!(r.side.is_some(), "{class}: no reticleSide");
        assert_eq!(
            quads.len(),
            centre + 4,
            "{class}: the centre and four sides"
        );
        // The right side (the second, turned 90) sits `spread` right
        // of the centre, on the 1280 by 960 window's 2 px a unit.
        let right = &quads[centre + 1];
        let left_edge = right.verts.iter().map(|v| v[0]).fold(f32::MAX, f32::min);
        let k = if steady { 0.65 } else { 1.0 };
        let (stand_min, stand_max) = (def.def.hip_spread_stand_min, def.def.hip_spread_stand_max);
        let deg = ((stand_max - stand_min) * ps.aim_spread_scale / 255.0 + stand_min) * k;
        let px = (deg.to_radians().tan() * 240.0 / TAN_Y).max(r.min_ofs as f32);
        let spread = px - r.side_pos * r.side_size as f32;
        assert!(
            (left_edge - (640.0 + 2.0 * spread)).abs() < 0.01,
            "{class}: right side at {left_edge}, expected {}",
            640.0 + 2.0 * spread
        );
    }
}

/// [`live_war_pair_on`]: the other player set down 100 ahead of us
/// (`standBefore`), our view on his chest for a second and then
/// turned away. The names over heads (`cod4_names`, cod4-hud.md 17.1)
/// run off the frame's crosshair scan as the client's do: an enemy's
/// crosshair name waits out `cg_enemyNameFadeIn` (250) and fades over
/// `cg_enemyNameFadeOut` (250), with no friendly name; a teammate's
/// friendly name is up at once and fades over
/// `cg_friendlyNameFadeOut` (1500). Each draws in its team colour,
/// across the point it projects to.
#[test]
fn cod4_live_names_over_heads_fade_by_team() {
    use crate::cod4_names::{Dvars, OverheadNames, View, session_frame};
    use crate::cod4_waypoints::Eye;
    /// A frame's sighting `(start, last)` of him, his names' alphas
    /// (the crosshair's, the friendly loop's) and whether their glyphs
    /// took the expected colour across his projected point.
    type Log = Vec<(i32, Option<(i32, i32)>, Option<f32>, Option<f32>, bool)>;
    let run = |teams: [&'static str; 2]| {
        let enemy = teams[0] != teams[1];
        let mut crosshair = crate::cod4_crosshair::CrosshairTarget::default();
        let mut names = OverheadNames::default();
        let mut placed: Option<Duration> = None;
        let mut on_since: Option<i32> = None;
        let mut turned = false;
        let mut log: Log = Vec::new();
        let t0 = clk_now();
        live_war_pair_on(
            teams,
            ["assault_mp,0", "assault_mp,0"],
            Some(GSC),
            move |f| {
                let t = since(t0);
                if f.level < 3000 || !f.both {
                    return None;
                }
                let Some(at) = placed else {
                    f.tx.send((1, "maps/mp/mp_crash_fx", "standBefore", "joiner"))
                        .ok();
                    placed = Some(t);
                    return None;
                };
                if t - at < Duration::from_millis(300) {
                    return None;
                }
                let him = f.e.gamestate()?.client_num as u32;
                if !turned {
                    if on_since.is_none() && t - at > Duration::from_secs(6) {
                        // A wall in the way: set him down again.
                        placed = None;
                        return None;
                    }
                    let ps = &f.e.client().snapshots.last()?.ps;
                    let chest = vec3(|k| ps_f32(ps, &format!("origin[{k}]"))) + Vec3::Z * 40.0;
                    let d = chest - f.eye;
                    let yaw = d.y.atan2(d.x).to_degrees();
                    let pitch = -d.z.atan2(d.truncate().length()).to_degrees();
                    let wrap = |a: f32| (a + 540.0).rem_euclid(360.0) - 180.0;
                    f.s.turn([wrap(pitch - f.angles.x), wrap(yaw - f.angles.y)]);
                }
                let scene = f.s.scene()?;
                let now = scene.render_time;
                let range = f.hud.enemy_crosshair_range(f.s, scene.view.view_weapon());
                let fades = crate::cod4_crosshair::NameFades::default();
                crosshair.scan(scene.crosshair_hit.as_ref(), range, now, &fades);
                let (p, y) = (scene.angles.x.to_radians(), scene.angles.y.to_radians());
                let forward = Vec3::new(p.cos() * y.cos(), p.cos() * y.sin(), -p.sin());
                let view = View {
                    now,
                    eye: scene.eye,
                    forward,
                    third_person: false,
                    pm_type: scene.view.pm_type,
                };
                let dv = Dvars::default();
                let snap = f.s.client().snapshots.last()?;
                let origin = |n: u32| {
                    snap.entities
                        .iter()
                        .find(|e| e.number() == n)
                        .map_or(Vec3::ZERO, origin_of)
                };
                let drawn =
                    session_frame(&mut names, f.s, view, &crosshair, &|_| None, &origin, &dv);
                let alpha = |x: bool| {
                    drawn
                        .iter()
                        .find(|n| n.client == him && n.crosshair == x)
                        .map(|n| n.alpha)
                };
                // His name's glyphs: in his colour, across where he
                // projects.
                let up = forward.cross(Vec3::Z).cross(forward).normalize();
                let eye = Eye {
                    origin: scene.eye,
                    forward,
                    left: up.cross(forward),
                    up,
                    tan: [0.75, 0.5625],
                    size: [1280.0, 960.0],
                };
                let rgb = f.hud.team_rgb(
                    f.s,
                    match enemy {
                        true => "g_TeamColor_EnemyTeam",
                        false => "g_TeamColor_MyTeam",
                    },
                );
                let mine: Vec<_> = drawn.iter().filter(|n| n.client == him).copied().collect();
                let quads = f.hud.overhead_names(f.s, &mine, &eye, &dv);
                // Behind the eye it draws nothing (0x42d6d0).
                let drawn_right = mine.first().is_none_or(|n| {
                    let Some(px) = eye.project(n.at()) else {
                        return quads.is_empty();
                    };
                    let glyphs: Vec<_> = quads
                        .iter()
                        .filter(|q| q.rgba[..3] == rgb && q.rgba[3] == n.alpha)
                        .collect();
                    let xs = glyphs.iter().flat_map(|q| q.verts.iter().map(|v| v[0]));
                    let (lo, hi) = xs.fold((f32::MAX, f32::MIN), |(a, b), x| (a.min(x), b.max(x)));
                    !glyphs.is_empty() && lo < px.x && px.x < hi
                });
                let (who, start, last) = crosshair.sighting();
                let sighting = (who == him).then_some((start, last));
                log.push((now, sighting, alpha(true), alpha(false), drawn_right));
                let named = alpha(enemy).is_some_and(|a| a == 1.0);
                if named && on_since.is_none() {
                    on_since = Some(now);
                }
                if !turned && on_since.is_some_and(|s| now - s > 1000) {
                    turned = true;
                    f.s.turn([0.0, 180.0]);
                }
                (turned && on_since.is_some_and(|s| now - s > 3000)).then(|| log.clone())
            },
        )
    };
    const GSC: &str = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../cod4/tests/fixtures/gsc/streak_kill"
    );
    for teams in [["allies", "axis"], ["allies", "allies"]] {
        let enemy = teams[0] != teams[1];
        let Some(log) = run(teams) else {
            return;
        };
        let log = log.unwrap_or_else(|| panic!("{teams:?}: never named him"));
        let (fade_in, fade_out) = if enemy { (250, 250) } else { (0, 1500) };
        let mut faded = false;
        for &(now, sighting, cross, friendly, drawn_right) in &log {
            assert!(drawn_right, "{teams:?} at {now}: {log:?}");
            if enemy {
                assert_eq!(friendly, None, "an enemy's friendly name at {now}");
            }
            // The crosshair's name follows the sighting's times.
            let want = sighting.and_then(|(start, last)| {
                let a = crate::cod4_names::alpha(now, start, last, fade_in, fade_out);
                (a > 0.001).then_some(a)
            });
            assert_eq!(cross, want, "{teams:?} at {now}: {log:?}");
            if let Some(a) = if enemy { cross } else { friendly } {
                faded |= a < 1.0;
            }
        }
        // On him past the fade-in, then off him and faded out.
        assert!(
            log.iter()
                .any(|l| l.2 == Some(1.0) && l.1.is_some_and(|(s, x)| x - s >= fade_in)),
            "{teams:?}: {log:?}"
        );
        assert!(faded, "{teams:?}: the name never faded: {log:?}");
        assert_eq!(log.last().map(|l| (l.2, l.3)), Some((None, None)));
        if !enemy {
            // A teammate's friendly name is up the frame he is seen.
            let first = log.iter().position(|l| l.3.is_some()).unwrap();
            assert_eq!(log[first].3, Some(1.0), "{log:?}");
        }
    }
}

/// [`live_war_pair`]: the M16's sight raised, its crosshair drawn while
/// the lerp runs. With `cg_crosshairDynamic` 1 the centre sits where
/// the view weapon's turned gun points (`CG_CalcCrosshairPosition`,
/// cod4-hud.md 17), off the centre the stock 0 keeps it on.
#[test]
fn cod4_live_dynamic_crosshair_follows_the_raised_gun() {
    use crate::play::input::Action;
    let root = vcod_cod4::testing::cod4_install().map(|i| i.root().to_path_buf());
    let mut view = crate::cod4_viewmodel::Cod4View::default();
    let mut bodies = None;
    let mut dynamic = crate::console::shell::Shell::new();
    dynamic.execute("set cg_crosshairDynamic 1");
    let mut raised = false;
    let Some(out) = live_war_pair(move |f| {
        let v = *f.view;
        if !f.both || f.level < 3000 || v.weapon == 0 {
            return None;
        }
        // The load takes frames: the sight goes up after it.
        let Some(bodies) = &bodies else {
            let ents = f.s.entity_assets();
            bodies = Some(crate::cod4_fly::load_bodies(root.as_ref()?, "mp_crash", &ents).unwrap());
            return None;
        };
        if !raised {
            raised = true;
            f.s.input.key(Action::Ads, true);
        }
        let scene = f.s.scene()?;
        let list = weapon_list(f.s.gamestate()?);
        let out = view.frame(
            &v,
            &list,
            scene.hands.as_deref(),
            &bodies.1,
            &bodies.0,
            scene.render_time,
        );
        eprintln!(
            "pos {} dir {:?} t {} pmf {:#x} ws {}",
            v.pos_frac, out.gun_dir, scene.render_time, v.pm_flags, v.weaponstate
        );
        let dir = out.gun_dir?;
        let v = crate::cod4_viewmodel::ViewPs {
            gun_dir: Some(dir),
            ..v
        };
        let tan = [0.75, 0.5];
        let size = (1280.0, 960.0);
        let still = f.hud.crosshair(f.s, &v, tan, size, f.shell);
        let moved = f.hud.crosshair(f.s, &v, tan, size, &dynamic);
        (!still.is_empty() && dir[1].abs() + dir[2].abs() > 1e-4)
            .then(|| (still[0].verts, moved[0].verts, dir))
    }) else {
        return;
    };
    let (still, moved, dir) = out.expect("no frame with the sight coming up");
    let centre = |q: [[f32; 2]; 4]| [0, 1].map(|k| q.iter().map(|v| v[k]).sum::<f32>() / 4.0);
    let (a, b) = (centre(still), centre(moved));
    // 2 px a grid unit on 1280 by 960.
    let want = [
        dir[1] / (dir[0] * 0.75) * -640.0,
        dir[2] / (dir[0] * 0.5) * -480.0,
    ];
    // The first piece (the centre, else the top side) moves by the
    // gun's projection, sitting on the centre's column without it.
    assert!((a[0] - 640.0).abs() < 0.01, "{a:?}");
    assert!(
        (b[0] - a[0] - want[0]).abs() < 0.01 && (b[1] - a[1] - want[1]).abs() < 0.01,
        "{b:?} for {dir:?}, still {a:?}"
    );
    assert!(want[0].abs() > 1.0, "the gun turned {dir:?}");
}

/// The camo and the rocket through vcod-server, live: the probe
/// script's `camo` gives `ak47_mp` on `gunModel[3]` and then `rpg_mp`
/// (whose slot 3 is empty: slot 0), switching to each. The snapshot's
/// `weaponmodels[]` reaches the view weapon, which draws the AK-47's
/// Blue Tiger materials and the RPG's rocket (cod4-client-view.md 4.7).
#[test]
fn cod4_live_a_given_camo_and_the_rocket_reach_the_view_weapon() {
    const GSC: &str = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../cod4/tests/fixtures/gsc/killcam"
    );
    let root = vcod_cod4::testing::cod4_install().map(|i| i.root().to_path_buf());
    let mut view = crate::cod4_viewmodel::Cod4View::default();
    let mut bodies = None;
    let mut asked = 0;
    // The AK-47's `weaponmodels` slot and gun materials once drawn.
    let mut ak: Option<(u8, Vec<String>)> = None;
    let out = live_war_pair_in(["assault_mp,0", "assault_mp,0"], Some(GSC), move |f| {
        let v = *f.view;
        if !f.both || f.level < 3000 || v.weapon == 0 {
            return None;
        }
        let Some(bodies) = &bodies else {
            let ents = f.s.entity_assets();
            bodies = Some(crate::cod4_fly::load_bodies(root.as_ref()?, "mp_crash", &ents).unwrap());
            return None;
        };
        let give = ["ak47_mp", "rpg_mp"];
        if asked < give.len() && (asked == 0 || ak.is_some()) {
            f.tx.send((0, "maps/mp/mp_crash_fx", "camo", give[asked]))
                .ok();
            asked += 1;
        }
        let scene = f.s.scene()?;
        let list = weapon_list(f.s.gamestate()?);
        let name = list.get(v.weapon as usize - 1)?.clone();
        let out = view.frame(
            &v,
            &list,
            scene.hands.as_deref(),
            &bodies.1,
            &bodies.0,
            scene.render_time,
        );
        let models = out.frame?.models?;
        match name.as_str() {
            "ak47_mp" if ak.is_none() => {
                let slot = v.weapon_models[v.weapon as usize];
                ak = Some((slot, models[1].lods[0].materials.clone()));
                None
            }
            "rpg_mp" => Some((ak.clone()?, models.len())),
            _ => None,
        }
    });
    let Some(out) = out else {
        return;
    };
    let ((slot, mats), rpg_parts) = out.expect("never drew the AK-47 and then the RPG");
    assert_eq!(slot, 3);
    assert!(
        mats.iter().any(|m| m == "mc/mtl_weapon_ak47_bwmrpt"),
        "{mats:?}"
    );
    assert_eq!(rpg_parts, 3, "hands, launcher and rocket");
}

/// Another player's gun draws his camo, live on vcod-server: the probe
/// script's `camo` gives the enemy `ak47_mp` on slot 3 and `near`
/// stands him beside us. His entity in our snapshot carries
/// `weaponModel` 3, and his posed rig holds `worldModel[3]`, whose
/// materials are the Blue Tiger's (cod4-client-view.md 4.7).
#[test]
fn cod4_live_another_players_gun_draws_his_camo() {
    use crate::play::cod4_players::Cod4Players;
    const GSC: &str = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../cod4/tests/fixtures/gsc/killcam"
    );
    let root = vcod_cod4::testing::cod4_install().map(|i| i.root().to_path_buf());
    let mut bodies = None;
    let mut asked = 0;
    let out = live_war_pair_in(["assault_mp,0", "assault_mp,0"], Some(GSC), move |f| {
        if !f.both || f.level < 3000 {
            return None;
        }
        if bodies.is_none() {
            let ents = f.s.entity_assets();
            bodies = Some(crate::cod4_fly::load_bodies(root.as_ref()?, "mp_crash", &ents).unwrap());
            return None;
        }
        let script = [("camo", "ak47_mp"), ("near", "joiner")];
        if let Some((name, arg)) = script.get(asked) {
            f.tx.send((1, "maps/mp/mp_crash_fx", *name, *arg)).ok();
            asked += 1;
            return None;
        }
        let enemy = f.e.gamestate()?.client_num;
        let scene = f.s.scene()?;
        let d = scene
            .entities
            .iter()
            .find(|d| d.body.as_ref().is_some_and(|b| b.frame.client_num == enemy))?;
        let v = d.body.clone()?;
        (v.weapon.as_deref() == Some("ak47_mp")).then(|| {
            let at = (d.number, scene.render_time, scene.server_time);
            (bodies.take().unwrap(), v, at)
        })
    });
    let Some(out) = out else {
        return;
    };
    let ((_, weapons, src), v, (n, render, server)) =
        out.expect("never saw the enemy hold the AK-47");
    assert_eq!(v.weapon_model, 3);
    // His body posed as the session draws it.
    let mut players = Cod4Players::default();
    let posed = players.posed(&src, &weapons, &[(n, &v)], &[], render, server);
    let (_, rig, _) = posed.first().expect("his rig builds");
    let camo = weapons["ak47_mp"].worlds[3].clone();
    assert_eq!(camo.as_deref(), Some("weapon_ak47_mp_bwmrpt"));
    let (_, gun) = rig
        .models
        .iter()
        .find(|(m, _)| Some(m) == camo.as_ref())
        .expect("the rig holds the camo's world model");
    let mats = &gun.lods[0].materials;
    assert!(
        mats.iter().any(|m| m == "mc/mtl_weapon_ak47_bwmrpt"),
        "{mats:?}"
    );
    assert!(!mats.iter().any(|m| m == "mc/mtl_weapon_ak47"), "{mats:?}");
}

/// A dropped gun draws its camo, live on vcod-server: the enemy takes
/// `ak47_mp` on slot 3 (`camo`), stands beside us and drops it. The
/// item's `index` is the weapon plus 384, it draws as
/// `worldModel[3]`, and that model's materials are the Blue Tiger's
/// (cod4-client-view.md 4.7).
#[test]
fn cod4_live_a_dropped_camo_gun_draws_its_camo() {
    const GSC: &str = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../cod4/tests/fixtures/gsc/killcam"
    );
    let root = vcod_cod4::testing::cod4_install().map(|i| i.root().to_path_buf());
    let mut bodies = None;
    let mut asked = 0;
    let out = live_war_pair_in(["assault_mp,0", "assault_mp,0"], Some(GSC), move |f| {
        if !f.both || f.level < 3000 {
            return None;
        }
        if bodies.is_none() {
            let ents = f.s.entity_assets();
            bodies = Some(crate::cod4_fly::load_bodies(root.as_ref()?, "mp_crash", &ents).unwrap());
            return None;
        }
        let script = [("camo", "ak47_mp"), ("near", "joiner"), ("drop", "ak47_mp")];
        if let Some((name, arg)) = script.get(asked) {
            f.tx.send((1, "maps/mp/mp_crash_fx", *name, *arg)).ok();
            asked += 1;
            return None;
        }
        let snap = f.s.client().snapshots.last()?;
        let e = snap.entities.iter().find(|e| e.etype() == ET_ITEM)?;
        let (index, number) = (ent_u32(e, "index"), e.number());
        let scene = f.s.scene()?;
        let d = scene.entities.iter().find(|d| d.number == number)?;
        Some((bodies.take().unwrap(), index, d.visual.clone()))
    });
    let Some(out) = out else {
        return;
    };
    let ((models, weapons, _), index, visual) = out.expect("never saw the dropped AK-47");
    assert_eq!(index >> 7, 3, "index {index}");
    let Visual::Model(key) = visual else {
        panic!("the item draws {visual:?}");
    };
    assert_eq!(key, "weapon:ak47_mp#3");
    let model = crate::cod4_fly::visual_model(&weapons, &key);
    assert_eq!(model.as_deref(), Some("weapon_ak47_mp_bwmrpt"));
    let mats = &models[model.as_deref().unwrap()].lods[0].materials;
    assert!(
        mats.iter().any(|m| m == "mc/mtl_weapon_ak47_bwmrpt"),
        "{mats:?}"
    );
    assert!(!mats.iter().any(|m| m == "mc/mtl_weapon_ak47"), "{mats:?}");
}

/// [`live_war_pair_as`] with the enemy a demolitions class: the stock
/// `_weapons` stows his RPG on `tag_stowed_back`, a tag first indexed
/// by his client info at his spawn. Every snapshot that names an
/// attachment's tag arrives with the tag's configstring already in
/// the table, as retail's `G_UpdateClientInfo` indexes it inside the
/// frame, so no body grafts an attachment by its root bone for a
/// snapshot (cod4-player-anims.md 5).
#[test]
fn cod4_live_a_new_attachment_tag_arrives_with_its_snapshot() {
    let mut stowed = false;
    let Some(out) = live_war_pair_as(["assault_mp,0", "demolitions_mp,0"], move |f| {
        let gs = f.s.gamestate()?;
        let snap = f.s.client().snapshots.last()?;
        for c in &snap.clients {
            for i in 0..6 {
                let Some(model) = model_cs(gs, client_u32(c, &format!("attachModelIndex[{i}]")))
                else {
                    continue;
                };
                let tag = client_u32(c, &format!("attachTagIndex[{i}]"));
                let named =
                    u16::try_from(tag).is_ok_and(|t| !gs.configstring(CS_TAGS + t).is_empty());
                if model.contains("_stow") {
                    stowed = true;
                }
                if tag != 0 && !named {
                    return Some(Err(format!("{model}: tag {tag} not yet in the table")));
                }
            }
        }
        (stowed && f.both && f.level > 3000).then_some(Ok(()))
    }) else {
        return;
    };
    out.expect("never saw a stowed weapon").unwrap();
}
