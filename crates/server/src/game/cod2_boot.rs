//! CoD2's stock `dm` and `tdm` load on mp_toujane and run their bootstrap
//! (`main`, the map's `main`, `CodeCallback_StartGameType`) to its first
//! wait: every call compiles against cod2_lnxded's tables and no thread
//! dies, then ten seconds of an empty server run clean (cod2-gsc-diff.md
//! 5). Needs `COD2_DIR`; without it the test passes.

use std::rc::Rc;

use crate::game::script::{PakScripts, ScriptRuntime};

fn boot(gametype: &str) -> Option<ScriptRuntime> {
    let fs = Rc::new(vcod_common::testing::cod2_fs()?);
    let bytes = fs
        .read("maps/mp/mp_toujane.d3dbsp")
        .expect("mp_toujane.d3dbsp");
    let world = vcod_cod2::world::from_bytes(&bytes).expect("parse mp_toujane");
    let mut cvars = crate::cvars::Cvars::new();
    cvars.set("g_gametype", gametype);
    cvars.set("mapname", "mp_toujane");
    let rt = ScriptRuntime::load_cod2(
        Box::new(PakScripts::new(fs.clone(), None)),
        fs,
        &world,
        "mp_toujane",
        gametype,
        vec![String::new(); 2048],
        cvars,
        0,
    )
    .unwrap_or_else(|e| panic!("{gametype}: {e:#}"));
    Some(rt)
}

fn check(gametype: &str) {
    let Some(mut rt) = boot(gametype) else {
        return;
    };
    let aborts = rt.aborts();
    assert!(
        aborts.is_empty(),
        "{gametype}: {} bootstrap error(s)\n{}",
        aborts.len(),
        aborts.join("\n")
    );
    // mp_toujane's ten `trigger_radius` and three `trigger_use_touch`.
    let (round, uses) = rt.with_host(|h, _| {
        let kinds: Vec<_> = h.triggers.iter().map(|(_, t)| (t.kind, t.round)).collect();
        (
            kinds.iter().filter(|(_, r)| r.is_some()).count(),
            kinds
                .iter()
                .filter(|(k, _)| *k == crate::game::trigger::TriggerKind::Use)
                .count(),
        )
    });
    assert_eq!((round, uses), (10, 3), "{gametype}");
    // `Callback_StartGameType`'s last statements ran.
    assert_eq!(rt.level_field_str("mapended"), "0", "{gametype}");
    let left = rt.unimplemented_builtins();
    eprintln!(
        "{gametype}: {} builtins unanswered: {}",
        left.len(),
        left.join(" ")
    );
    // And ten seconds of an empty server.
    for f in 1..=200 {
        rt.run_frame(f * 50);
    }
    let aborts = rt.aborts();
    assert!(aborts.is_empty(), "{gametype}: {}", aborts.join("\n"));
}

#[test]
fn cod2_dm_bootstraps_on_mp_toujane() {
    check("dm");
}

#[test]
fn cod2_tdm_bootstraps_on_mp_toujane() {
    check("tdm");
}

/// Two clients through `Cod2Level`: connect, begin, the team menu, the
/// weapon menu, the spawn, then 30 s of running and turning on mp_toujane
/// with no script error and no unanswered builtin.
fn play(gametype: &str, teams: [&str; 2]) {
    use crate::game::cod2::Cod2Level;
    use crate::game::script::MenuRef;
    let Some(fs) = vcod_common::testing::cod2_fs() else {
        return;
    };
    let mut cvars = crate::cvars::Cvars::new();
    cvars.set("developer", "1");
    let mut level = Cod2Level::load(Rc::new(fs), "mp_toujane", gametype, cvars, 0)
        .unwrap_or_else(|e| panic!("{gametype}: {e:#}"));
    assert!(level.rt.unimplemented_builtins().is_empty());
    let mut now = 0;
    let frame = |level: &mut Cod2Level, now: &mut i32, moving: bool| {
        *now += 50;
        for slot in 0..2 {
            let cmd = vcod_cod2::net::UserCmd2 {
                server_time: *now,
                forward_move: if moving { 127 } else { 0 },
                // A slow turn, so the runs go different ways.
                angles: [0, (*now * 8 + slot * 20000) & 0xffff, 0],
                ..Default::default()
            };
            level.client_think(slot as usize, &cmd);
        }
        level.run_frame(*now);
    };
    level.connect(0, "a");
    level.connect(1, "b");
    frame(&mut level, &mut now, false);
    level.begin(0);
    level.begin(1);
    frame(&mut level, &mut now, false);
    for (slot, team) in teams.iter().enumerate() {
        level.menu_response(slot, &MenuRef::Name("team_britishgerman".into()), team);
    }
    for _ in 0..4 {
        frame(&mut level, &mut now, false);
    }
    let weapons = [
        ("weapon_british", "enfield_mp"),
        ("weapon_german", "kar98k_mp"),
    ];
    for slot in 0..2 {
        let session = level
            .rt
            .client_field(slot, "sessionteam")
            .unwrap_or_default();
        let (menu, weapon) = if session == "axis" {
            weapons[1]
        } else {
            weapons[0]
        };
        level.menu_response(slot, &MenuRef::Name(menu.into()), weapon);
    }
    for _ in 0..4 {
        frame(&mut level, &mut now, false);
    }
    let spawned: Vec<[f32; 3]> = (0..2)
        .map(|slot| {
            let p = level.players[slot].as_ref().unwrap();
            assert_eq!(
                level.rt.client_field(slot, "sessionstate").as_deref(),
                Some("playing"),
                "{gametype}: slot {slot} never spawned"
            );
            assert_eq!(p.ps.pm_type, vcod_cod2::pmove::pm_type::NORMAL);
            p.ps.origin.to_array()
        })
        .collect();
    let mut farthest = [0.0f32; 2];
    for _ in 0..600 {
        frame(&mut level, &mut now, true);
        for (slot, far) in farthest.iter_mut().enumerate() {
            let p = &level.players[slot].as_ref().unwrap().ps;
            *far = far.max(glam::Vec3::from(spawned[slot]).distance(p.origin));
        }
    }
    for (slot, far) in farthest.into_iter().enumerate() {
        let p = &level.players[slot].as_ref().unwrap().ps;
        assert!(far > 64.0, "{gametype}: slot {slot} ran {far} at most");
        assert_eq!(level.rt.client_origin(slot), p.origin.to_array());
    }
    // Votes go to script under `g_oldVoting` 0: no engine reply.
    level.rt.with_host(|h, _| h.cvars.set("g_oldVoting", "0"));
    let argv = ["callvote", "map", "mp_toujane"].map(String::from);
    assert!(level.rt.vote_command(0, &argv, now).is_empty());
    let argv = ["callvote", "map;quit", ""].map(String::from);
    assert_eq!(level.rt.vote_command(0, &argv, now).len(), 1);
    // The precaches landed in CoD2's blocks (`CS_LAYOUT_118`): the menus
    // from 1246, the weapons the level named in configstring 7.
    let cs = level.rt.configstrings();
    assert_eq!(cs[1246], "ingame");
    assert!(cs[7].split(' ').any(|w| w == "enfield_mp"), "{}", cs[7]);
    // The loadout's grenade is the offhand (`G_GivePlayerWeapon`).
    for slot in 0..2 {
        let offhand = level.rt.host.cod2().clients[slot].offhand as usize;
        let name = level.rt.host.weapon_name(offhand).unwrap_or_default();
        assert!(
            name.starts_with("frag_grenade_"),
            "{gametype}: offhand {name:?}"
        );
    }
    let aborts = level.rt.aborts();
    assert!(
        aborts.is_empty(),
        "{gametype}: {} script error(s)\n{}",
        aborts.len(),
        aborts.join("\n")
    );
}

#[test]
fn cod2_dm_plays_two_clients_on_mp_toujane() {
    play("dm", ["autoassign", "autoassign"]);
}

#[test]
fn cod2_tdm_plays_two_clients_on_mp_toujane() {
    play("tdm", ["allies", "axis"]);
}

/// `text` with its `//` and `/* */` comments dropped.
fn uncommented(text: &str) -> String {
    let mut out = String::new();
    let mut rest = text;
    while !rest.is_empty() {
        let line = rest.find("//");
        let block = rest.find("/*");
        match (line, block) {
            (Some(l), b) if b.is_none_or(|b| l < b) => {
                out.push_str(&rest[..l]);
                rest = rest[l..].find('\n').map_or("", |e| &rest[l + e..]);
            }
            (_, Some(b)) => {
                out.push_str(&rest[..b]);
                rest = rest[b..].find("*/").map_or("", |e| &rest[b + e + 2..]);
            }
            _ => {
                out.push_str(rest);
                rest = "";
            }
        }
    }
    out
}

/// Every stock CoD2 map's script under `dm`, run three seconds: `main`,
/// its `_fx` script's `loadFx`, `loopfx` and `exploderfx` and `_load`'s
/// lanterns, with no thread dying and no builtin unanswered, and one
/// `ET_FX` entity for each `loopfx` line in the `_fx` script (each map's
/// own counts printed). Needs `COD2_DIR`.
#[test]
fn cod2_stock_map_fx_scripts_run() {
    let Some(fs) = vcod_common::testing::cod2_fs() else {
        return;
    };
    let fs = Rc::new(fs);
    let mut maps: Vec<String> = fs
        .names_with_suffix(".d3dbsp")
        .into_iter()
        .filter_map(|p| {
            Some(
                p.strip_prefix("maps/mp/")?
                    .strip_suffix(".d3dbsp")?
                    .to_string(),
            )
        })
        .collect();
    maps.sort();
    let mut failures = Vec::new();
    for map in &maps {
        let bytes = fs.read(&format!("maps/mp/{map}.d3dbsp")).unwrap();
        let world = vcod_cod2::world::from_bytes(&bytes).expect("parse map");
        let mut cvars = crate::cvars::Cvars::new();
        cvars.set("g_gametype", "dm");
        cvars.set("mapname", map);
        let mut rt = ScriptRuntime::load_cod2(
            Box::new(PakScripts::new(fs.clone(), None)),
            fs.clone(),
            &world,
            map,
            "dm",
            vec![String::new(); 2048],
            cvars,
            0,
        )
        .unwrap_or_else(|e| panic!("{map}: {e:#}"));
        for f in 1..=60 {
            rt.run_frame(f * 50);
        }
        let fx = rt
            .packet_entities(vcod_cod2::net::PROTOCOL_118.as_cod1_shim())
            .values()
            .filter(|e| {
                e.field_i32(vcod_cod2::net::PROTOCOL_118.as_cod1_shim(), "eType")
                    == vcod_cod2::net::events::ET_FX
            })
            .count();
        let effects = rt.with_host(|h, _| {
            let (lo, hi) = h
                .allocators
                .layout()
                .bounds(vcod_common::net::protocol::CsRange::Effect);
            (lo..=hi)
                .filter(|&i| !h.configstrings[i].is_empty())
                .count()
        });
        let script = fs
            .read(&format!("maps/mp/{map}_fx.gsc"))
            .map(|b| String::from_utf8_lossy(&b).into_owned())
            .unwrap_or_default();
        let loops = uncommented(&script).matches("_fx::loopfx(").count();
        // `soundfx`'s `ent = level thread createfx_showOrigin(..)` reads the
        // struct the thread returned before it suspended (cod2-gsc-diff.md
        // 3.5), so no stock level script aborts there.
        let aborts = rt.aborts();
        let left = rt.unimplemented_builtins();
        eprintln!(
            "{map}: {effects} effects, {fx} ET_FX for {loops} loopfx lines, unanswered: {left:?}"
        );
        if !aborts.is_empty() || !left.is_empty() || fx < loops {
            failures.push(format!(
                "{map}: {fx} < {loops}?\n{}\n{left:?}",
                aborts.join("\n")
            ));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
